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
use jsonrpsee_http_client::{HeaderMap, HeaderValue, HttpClient, HttpClientBuilder};
use serde::{Deserialize, Serialize};

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
}

impl FundingPlan {
    /// The display (reversed) transaction id.
    #[must_use]
    pub fn transaction_id_display(&self) -> String {
        bitcoin::Txid::from_byte_array(self.transaction_id).to_string()
    }
}

/// A Bitcoin Core JSON-RPC client bound to one wallet.
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
        let options = serde_json::json!({ "replaceable": true, "lockUnspents": true });
        let funded: FundedPsbt = wallet
            .request(
                "walletcreatefundedpsbt",
                rpc_params![serde_json::json!([]), outputs, 0_u32, options],
            )
            .await
            .context("walletcreatefundedpsbt")?;
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
        let transaction: Transaction =
            consensus::deserialize(&hex::decode(&transaction_hex).context("funding hex")?)
                .context("funding transaction")?;
        let matching: Vec<(usize, &bitcoin::TxOut)> = transaction
            .output
            .iter()
            .enumerate()
            .filter(|(_, output)| output.script_pubkey == script)
            .collect();
        ensure!(
            matching.len() == 1,
            "funding transaction must pay the contract exactly once"
        );
        let (index, output) = matching[0];
        ensure!(
            output.value == amount,
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
        })
    }

    /// Asks the node's mempool policy about `transaction_hex` without sending.
    ///
    /// # Errors
    ///
    /// Fails when the node rejects the transaction or is unreachable.
    pub async fn test_mempool_accept(&self, transaction_hex: &str) -> Result<()> {
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
        Ok(())
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
