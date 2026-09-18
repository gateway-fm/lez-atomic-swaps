//! The Bitcoin funder's plan: a signed funding transaction from a Core wallet.
//!
//! The wallet is a plain Bitcoin Core wallet on the configured node; the
//! network comes from configuration and only decides address encoding.

use std::{path::Path, time::Duration};

/// Upper bound for the Core cookie file (`user:password`).
const MAX_COOKIE_BYTES: usize = 4096;

use anyhow::{Context as _, Result, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use bitcoin::{Address, Amount, Network, ScriptBuf, Transaction, consensus, hashes::Hash as _};
use jsonrpsee::core::client::ClientT as _;
use jsonrpsee::rpc_params;
use jsonrpsee_http_client::{HeaderMap, HeaderValue, HttpClientBuilder};
use lez_btc_core_adapter::{AcceptJsonRpc1, Json1TolerantHttpClient as HttpClient};
use serde::{Deserialize, Serialize};

use crate::config::LockFeePolicyV1;

const MAX_REQUEST_BYTES: u32 = 1024 * 1024;
const MAX_RESPONSE_BYTES: u32 = 4 * 1024 * 1024;

/// The exact funding transaction and the outpoint the contract lives at.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FundingPlan {
    pub schema_version: u16,
    /// Signed, not yet broadcast, lowercase hex.
    pub transaction_hex: String,
    /// Internal byte order, as the agreement records it.
    pub transaction_id: [u8; 32],
    pub output_index: u32,
    pub value_sat: u64,
    /// Chain height when the plan was made; the refund height anchors here.
    pub anchor_height: u32,
    /// What the lock pays the miners; zero in plans made before it was kept.
    #[serde(default)]
    pub fee_sat: u64,
}

impl FundingPlan {
    /// The display (reversed) transaction id.
    #[must_use]
    pub fn transaction_id_display(&self) -> String {
        bitcoin::Txid::from_byte_array(self.transaction_id).to_string()
    }
}

/// A Bitcoin Core JSON-RPC client bound to one wallet.
/// A wallet's balances in satoshis, as Core reports them.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WalletBalances {
    /// Confirmed coins plus the wallet's own unconfirmed change.
    pub trusted_sat: u64,
    /// Unconfirmed coins received from others.
    pub untrusted_pending_sat: u64,
    /// Coinbase outputs that cannot be spent yet.
    pub immature_sat: u64,
}

pub struct BitcoinWallet {
    node: HttpClient,
    wallet: Option<HttpClient>,
    network: Network,
}

impl std::fmt::Debug for BitcoinWallet {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BitcoinWallet")
            .field("network", &self.network)
            .field("wallet", &self.wallet.as_ref().map(|_| "configured"))
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct FundedPsbt {
    psbt: String,
    /// BTC, as Core reports it.
    fee: f64,
}

#[derive(Deserialize)]
struct SmartFeeEstimate {
    #[serde(default)]
    feerate: Option<f64>,
}

#[derive(Deserialize)]
struct DecodedPsbt {
    tx: DecodedPsbtTransaction,
}

#[derive(Deserialize)]
struct DecodedPsbtTransaction {
    vin: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct ProcessedPsbt {
    psbt: String,
    complete: bool,
}

#[derive(Deserialize)]
struct FinalizedPsbt {
    hex: Option<String>,
    complete: bool,
}

#[derive(Deserialize)]
struct MempoolAcceptEntry {
    allowed: bool,
    #[serde(default, rename = "reject-reason")]
    reject_reason: Option<String>,
    #[serde(default)]
    fees: Option<MempoolAcceptFees>,
}

#[derive(Deserialize)]
struct MempoolAcceptFees {
    /// BTC, as Core reports it.
    base: f64,
}

/// A take stopped at funding: this role has no Bitcoin wallet, so the owner
/// signs a transaction paying `value_sat` to `address` and replays the take
/// with it.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("sign a transaction paying {value_sat} sat to {address} and replay the take with it")]
pub struct FundingRequired {
    pub address: String,
    pub value_sat: u64,
}

fn decode(transaction_hex: &str) -> Result<Transaction> {
    consensus::deserialize(&hex::decode(transaction_hex).context("funding hex")?)
        .context("funding transaction")
}

impl BitcoinWallet {
    /// Connects to `endpoint` with the cookie file's credentials; `wallet`
    /// selects the node wallet used for funding.
    ///
    /// # Errors
    ///
    /// Fails when the cookie file is unreadable or the endpoint is invalid.
    pub fn connect(
        endpoint: &str,
        cookie_file: &Path,
        wallet: Option<&str>,
        network: Network,
        timeout: Duration,
    ) -> Result<Self> {
        let cookie = crate::layout::read_private(cookie_file, MAX_COOKIE_BYTES)
            .context("read cookie file")?;
        let cookie = std::str::from_utf8(&cookie)
            .context("cookie file is not UTF-8")?
            .trim();
        ensure!(cookie.contains(':'), "cookie file must hold user:password");
        let mut headers = HeaderMap::new();
        let mut value = HeaderValue::from_str(&format!("Basic {}", BASE64_STANDARD.encode(cookie)))
            .context("authorization header")?;
        value.set_sensitive(true);
        headers.insert("authorization", value);
        let base = endpoint.trim_end_matches('/');
        let build = |url: &str| -> Result<HttpClient> {
            HttpClientBuilder::default()
                .max_request_size(MAX_REQUEST_BYTES)
                .max_response_size(MAX_RESPONSE_BYTES)
                .request_timeout(timeout)
                .set_headers(headers.clone())
                // A node that is not ours may answer in the 1.x envelope (#49).
                .set_http_middleware(tower::ServiceBuilder::new().layer_fn(AcceptJsonRpc1::new))
                .build(url)
                .with_context(|| format!("build Bitcoin Core client for {url}"))
        };
        let node = build(&format!("{base}/"))?;
        let wallet = wallet
            .map(|name| {
                ensure!(
                    !name.is_empty()
                        && name.bytes().all(|byte| byte.is_ascii_alphanumeric()
                            || matches!(byte, b'-' | b'_' | b'.')),
                    "wallet name must be a plain identifier"
                );
                build(&format!("{base}/wallet/{name}"))
            })
            .transpose()?;
        Ok(Self {
            node,
            wallet,
            network,
        })
    }

    /// The node's genesis block hash (internal byte order).
    ///
    /// # Errors
    ///
    /// Fails when the node is unreachable.
    pub async fn genesis_hash(&self) -> Result<[u8; 32]> {
        let hash: String = self
            .node
            .request("getblockhash", rpc_params![0_u32])
            .await
            .context("getblockhash")?;
        let parsed: bitcoin::BlockHash = hash.parse().context("genesis block hash")?;
        Ok(parsed.to_byte_array())
    }

    /// The wallet's balances in satoshis as Core reports them (`getbalances`):
    /// trusted (confirmed and own unconfirmed change), untrusted pending, and
    /// immature coinbase. Coins locked by a funding plan that was not yet
    /// broadcast are not counted as trusted.
    ///
    /// # Errors
    ///
    /// Fails without a wallet or when the node is unreachable.
    pub async fn balances(&self) -> Result<WalletBalances> {
        let wallet = self
            .wallet
            .as_ref()
            .context("no Bitcoin wallet is configured for this role")?;
        let value: serde_json::Value = wallet
            .request("getbalances", rpc_params![])
            .await
            .context("getbalances")?;
        let mine = value
            .get("mine")
            .context("getbalances: no `mine` balances")?;
        let sat = |key: &str| -> Result<u64> {
            let number = mine
                .get(key)
                .and_then(serde_json::Value::as_number)
                .with_context(|| format!("getbalances: no `mine.{key}`"))?;
            // Core prints BTC as a decimal; parse the decimal text, never a float.
            let amount =
                Amount::from_str_in(&number.to_string(), bitcoin::Denomination::Bitcoin)
                    .with_context(|| format!("getbalances: `mine.{key}` is not a BTC amount"))?;
            Ok(amount.to_sat())
        };
        Ok(WalletBalances {
            trusted_sat: sat("trusted")?,
            untrusted_pending_sat: sat("untrusted_pending")?,
            immature_sat: sat("immature")?,
        })
    }

    /// The current chain height.
    ///
    /// # Errors
    ///
    /// Fails when the node is unreachable.
    pub async fn block_count(&self) -> Result<u32> {
        let count: u64 = self
            .node
            .request("getblockcount", rpc_params![])
            .await
            .context("getblockcount")?;
        u32::try_from(count).context("block count overflow")
    }

    /// Builds and signs a transaction paying `value_sat` to `contract_script`
    /// from the wallet, without broadcasting it.
    ///
    /// # Errors
    ///
    /// Fails without a wallet, when the wallet cannot fund or fully sign, or
    /// when the signed transaction does not pay the contract exactly once.
    pub async fn plan_funding(
        &self,
        contract_script: &[u8],
        value_sat: u64,
        fee_policy: &LockFeePolicyV1,
    ) -> Result<FundingPlan> {
        let wallet = self
            .wallet
            .as_ref()
            .context("this role has no Bitcoin funding wallet")?;
        let script = ScriptBuf::from_bytes(contract_script.to_vec());
        let address = Address::from_script(&script, self.network).context("contract address")?;
        let amount = Amount::from_sat(value_sat);
        let outputs = serde_json::json!([{ address.to_string(): amount.to_string_in(bitcoin::Denomination::Bitcoin) }]);
        // The selected inputs are locked in the wallet so that a second swap
        // planned before this one broadcasts picks other coins; without the
        // lock two concurrent takes plan the same inputs and the later lock can
        // never broadcast. A plan that never broadcasts (an aborted take) keeps
        // its coins locked until `lockunspent true` or a Core restart releases
        // them; deploy/scripts/reset-swaps.sh does that.
        // An explicit, capped rate: without one Core's estimator alone prices a
        // transaction that can never be fee-bumped.
        let estimate: Option<SmartFeeEstimate> = self
            .node
            .request(
                "estimatesmartfee",
                rpc_params![fee_policy.confirmation_target],
            )
            .await
            .ok();
        let fee_rate = fee_policy.rate_sat_per_vb(estimate.and_then(|value| value.feerate));
        let options = serde_json::json!({
            "replaceable": true, "lockUnspents": true, "fee_rate": fee_rate,
        });
        let funded: FundedPsbt = wallet
            .request(
                "walletcreatefundedpsbt",
                rpc_params![serde_json::json!([]), outputs, 0_u32, options],
            )
            .await
            .context("walletcreatefundedpsbt")?;
        let fee_sat = Amount::from_btc(funded.fee)
            .context("funding fee")?
            .to_sat();
        if !fee_policy.admits(fee_sat, value_sat) {
            // The refused plan's coins must not stay reserved.
            if let Ok(decoded) = wallet
                .request::<DecodedPsbt, _>("decodepsbt", rpc_params![funded.psbt.clone()])
                .await
            {
                let inputs: Vec<serde_json::Value> = decoded
                    .tx
                    .vin
                    .iter()
                    .map(|vin| serde_json::json!({ "txid": vin["txid"], "vout": vin["vout"] }))
                    .collect();
                let _: Result<bool, _> = wallet
                    .request("lockunspent", rpc_params![true, inputs])
                    .await;
            }
            anyhow::bail!(
                "locking {value_sat} sat would cost {fee_sat} sat in fees at {fee_rate} sat/vB, \
                 above the {}% this Node allows",
                fee_policy.max_percent_of_value
            );
        }
        let processed: ProcessedPsbt = wallet
            .request("walletprocesspsbt", rpc_params![funded.psbt, true])
            .await
            .context("walletprocesspsbt")?;
        ensure!(
            processed.complete,
            "wallet could not fully sign the funding transaction"
        );
        let finalized: FinalizedPsbt = self
            .node
            .request("finalizepsbt", rpc_params![processed.psbt, true])
            .await
            .context("finalizepsbt")?;
        let transaction_hex = finalized
            .hex
            .filter(|_| finalized.complete)
            .context("funding transaction not finalized")?;
        let transaction = decode(&transaction_hex)?;
        self.plan_of(&transaction_hex, &transaction, &script, value_sat, fee_sat)
            .await
    }

    /// The plan for a signed transaction that pays the contract exactly once.
    async fn plan_of(
        &self,
        transaction_hex: &str,
        transaction: &Transaction,
        script: &ScriptBuf,
        value_sat: u64,
        fee_sat: u64,
    ) -> Result<FundingPlan> {
        let matching: Vec<(usize, &bitcoin::TxOut)> = transaction
            .output
            .iter()
            .enumerate()
            .filter(|(_, output)| &output.script_pubkey == script)
            .collect();
        ensure!(
            matching.len() == 1,
            "funding transaction must pay the contract exactly once"
        );
        let (index, output) = matching[0];
        ensure!(
            output.value == Amount::from_sat(value_sat),
            "funding output value differs from the plan"
        );
        let anchor_height = self.block_count().await?;
        Ok(FundingPlan {
            schema_version: 1,
            transaction_hex: transaction_hex.to_ascii_lowercase(),
            transaction_id: transaction.compute_txid().to_byte_array(),
            output_index: u32::try_from(index).context("output index")?,
            value_sat,
            anchor_height,
            fee_sat,
        })
    }

    /// The plan for a funding transaction the owner signed in a wallet of
    /// their own, for a role whose node has none (a public RPC provider).
    /// Without `transaction_hex` it answers [`FundingRequired`].
    ///
    /// # Errors
    ///
    /// Fails when the transaction does not pay the contract exactly once, could
    /// be given another id, is refused by the mempool, or overpays in fees.
    pub async fn adopt_funding(
        &self,
        contract_script: &[u8],
        value_sat: u64,
        transaction_hex: Option<&str>,
        fee_policy: &LockFeePolicyV1,
    ) -> Result<FundingPlan> {
        let script = ScriptBuf::from_bytes(contract_script.to_vec());
        let Some(transaction_hex) = transaction_hex else {
            let address =
                Address::from_script(&script, self.network).context("contract address")?;
            anyhow::bail!(FundingRequired {
                address: address.to_string(),
                value_sat,
            });
        };
        let transaction = decode(transaction_hex)?;
        // The refund and the claim are signed against this transaction's id
        // before it is sent. A non-witness input lets anyone change that id in
        // flight, which would leave the lock spendable by neither.
        ensure!(
            transaction
                .input
                .iter()
                .all(|input| input.script_sig.is_empty() && !input.witness.is_empty()),
            "every funding input must be a native SegWit spend"
        );
        let fee_sat = self.test_mempool_accept(transaction_hex).await?;
        ensure!(
            fee_policy.admits(fee_sat, value_sat),
            "locking {value_sat} sat would cost {fee_sat} sat in fees, above the {}% this Node allows",
            fee_policy.max_percent_of_value
        );
        self.plan_of(transaction_hex, &transaction, &script, value_sat, fee_sat)
            .await
    }

    /// Asks the node's mempool policy about `transaction_hex` without sending;
    /// answers the fee it pays, in satoshis.
    ///
    /// # Errors
    ///
    /// Fails when the node rejects the transaction or is unreachable.
    pub async fn test_mempool_accept(&self, transaction_hex: &str) -> Result<u64> {
        let entries: Vec<MempoolAcceptEntry> = self
            .node
            .request(
                "testmempoolaccept",
                rpc_params![serde_json::json!([transaction_hex])],
            )
            .await
            .context("testmempoolaccept")?;
        let entry = entries.first().context("empty testmempoolaccept answer")?;
        ensure!(
            entry.allowed,
            "mempool rejects the funding transaction: {}",
            entry.reject_reason.as_deref().unwrap_or("unknown")
        );
        let fees = entry.fees.as_ref().context("testmempoolaccept: no fees")?;
        Ok(Amount::from_btc(fees.base).context("funding fee")?.to_sat())
    }

    /// Broadcasts `transaction_hex`; returns the display transaction id.
    ///
    /// # Errors
    ///
    /// Fails when the node rejects the transaction or is unreachable.
    pub async fn broadcast(&self, transaction_hex: &str) -> Result<String> {
        let txid: String = self
            .node
            .request("sendrawtransaction", rpc_params![transaction_hex])
            .await
            .context("sendrawtransaction")?;
        Ok(txid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::{
        OutPoint, Sequence, TxIn, TxOut, Witness, absolute::LockTime, transaction::Version,
    };

    /// A wallet-less role; nothing listens on the endpoint.
    fn wallet(directory: &Path) -> BitcoinWallet {
        let cookie = directory.join("cookie");
        crate::layout::write_private_exact(&cookie, b"user:password").unwrap();
        BitcoinWallet::connect(
            "http://127.0.0.1:1",
            &cookie,
            None,
            Network::Regtest,
            Duration::from_secs(1),
        )
        .unwrap()
    }

    fn contract() -> ScriptBuf {
        ScriptBuf::new_p2tr_tweaked(bitcoin::key::TweakedPublicKey::dangerous_assume_tweaked(
            bitcoin::XOnlyPublicKey::from_slice(&[2; 32]).unwrap(),
        ))
    }

    #[tokio::test]
    async fn a_role_without_a_wallet_answers_where_to_pay() {
        let directory = tempfile::tempdir().unwrap();
        let error = wallet(directory.path())
            .adopt_funding(
                contract().as_bytes(),
                10_000,
                None,
                &LockFeePolicyV1::default(),
            )
            .await
            .unwrap_err();
        let required: FundingRequired = error.downcast().unwrap();
        assert_eq!(required.value_sat, 10_000);
        assert!(required.address.starts_with("bcrt1p"));
    }

    #[tokio::test]
    async fn a_funding_transaction_whose_id_can_be_changed_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let legacy = Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::null(),
                script_sig: ScriptBuf::from_bytes(vec![0x51]),
                sequence: Sequence::MAX,
                witness: Witness::new(),
            }],
            output: vec![TxOut {
                value: Amount::from_sat(10_000),
                script_pubkey: contract(),
            }],
        };
        let error = wallet(directory.path())
            .adopt_funding(
                contract().as_bytes(),
                10_000,
                Some(&consensus::encode::serialize_hex(&legacy)),
                &LockFeePolicyV1::default(),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("native SegWit"), "{error:#}");
    }
}
