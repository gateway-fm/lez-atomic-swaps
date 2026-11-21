//! Narrow effect-bearing wallet boundary for the private M4 Regtest `PoC`.
//!
//! This module deliberately reuses the typed `monero-rpc` client. It neither
//! observes protocol eligibility nor authorizes release: callers must first
//! satisfy the Stage-B, finalized-LEZ, and adaptor-extraction gates. Each method
//! makes one wallet submission and then mines the agreement's fixed confirmation
//! count on a project-owned Regtest daemon. No submission is made twice: a
//! submission's Digest exchange is made again only after a 401, which the
//! wallet-rpc answers without running it; a transfer whose outcome is unknown
//! is looked up, not sent again. A read is made again after a transport
//! failure (see the crate documentation), the mining call a bounded number of
//! times.
//!
//! A Node runs one wallet-rpc per role and switches its open wallet between
//! the role wallet and per-swap shared wallets. An effect built with
//! [`MoneroRegtestWalletEffects::with_role_wallet`] reopens the role wallet
//! before it acts, because a restarted wallet-rpc has no wallet open and the
//! restore effects close the role wallet. Every tool of one role holds the
//! same [`MoneroWalletRpcLock`] for its whole run, so two swaps never switch
//! the wallet under each other.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::num::NonZeroU64;
use std::path::Path;

use lez_xmr_swap_sdk::{MoneroPrivateViewKey, MoneroSharedAddressV1, ReconstructedMoneroSpendKey};
use monero_rpc::monero::util::address::AddressType;
use monero_rpc::monero::{Address, Amount, PrivateKey};
use monero_rpc::{GenerateFromKeysArgs, RegtestDaemonJsonRpcClient, TransferType, WalletClient};
use thiserror::Error;

use super::{
    LoopbackRpcEndpoint, MoneroTransactionId, REQUIRED_MONERO_CONFIRMATIONS, read_with_retries,
    valid_credential,
};

const WALLET_ACCOUNT_INDEX: u32 = 0;
const MAX_WALLET_FILENAME_BYTES: usize = 96;
/// Goes at `generateblocks`: one call can fail in the client although the
/// daemon is healthy (2026-11-15, a non-JSON body decoded as the answer), and
/// on Regtest nothing else mines. Blocks a failed call did mine are harmless:
/// confirmation checks ask for at least the fixed count.
const MINING_ATTEMPTS: u32 = 3;

/// A submitted exact funding transfer followed by local Regtest confirmations.
#[derive(Debug, Eq, PartialEq)]
#[must_use]
pub struct ConfirmedMoneroFunding {
    transaction_id: MoneroTransactionId,
    amount_piconero: u64,
    confirmation_tip_height: u64,
}

impl ConfirmedMoneroFunding {
    /// Submitted transaction identity. This is not output-observation evidence.
    #[must_use]
    pub const fn transaction_id(&self) -> MoneroTransactionId {
        self.transaction_id
    }

    /// Exact principal requested from the funding wallet.
    #[must_use]
    pub const fn amount_piconero(&self) -> u64 {
        self.amount_piconero
    }

    /// Height returned after generating the fixed local confirmation count.
    #[must_use]
    pub const fn confirmation_tip_height(&self) -> u64 {
        self.confirmation_tip_height
    }
}

/// A reconstructed-wallet sweep followed by local Regtest confirmations.
#[derive(Debug, Eq, PartialEq)]
#[must_use]
pub struct ConfirmedMoneroSweep {
    transaction_id: MoneroTransactionId,
    funded_amount_piconero: u64,
    received_amount_piconero: u64,
    fee_piconero: u64,
    confirmation_tip_height: u64,
}

/// One reconstructed-wallet sweep submission without confirmation mining.
///
/// This receipt is intentionally not finality evidence. A durable caller must
/// persist its transaction identity and use a separate read-only observer.
#[derive(Debug, Eq, PartialEq)]
#[must_use]
pub struct SubmittedMoneroSweep {
    transaction_id: MoneroTransactionId,
    funded_amount_piconero: u64,
    received_amount_piconero: u64,
    fee_piconero: u64,
}

impl SubmittedMoneroSweep {
    /// Sole submitted sweep transaction identity.
    #[must_use]
    pub const fn transaction_id(&self) -> MoneroTransactionId {
        self.transaction_id
    }

    /// Exact unlocked shared-wallet amount checked before submission.
    #[must_use]
    pub const fn funded_amount_piconero(&self) -> u64 {
        self.funded_amount_piconero
    }

    /// Exact amount directed to the destination after the fee.
    #[must_use]
    pub const fn received_amount_piconero(&self) -> u64 {
        self.received_amount_piconero
    }

    /// Exact fee reported for the sole sweep transaction.
    #[must_use]
    pub const fn fee_piconero(&self) -> u64 {
        self.fee_piconero
    }
}

impl ConfirmedMoneroSweep {
    /// Sole sweep transaction identity.
    #[must_use]
    pub const fn transaction_id(&self) -> MoneroTransactionId {
        self.transaction_id
    }

    /// Exact unlocked shared-wallet amount checked before the sweep.
    #[must_use]
    pub const fn funded_amount_piconero(&self) -> u64 {
        self.funded_amount_piconero
    }

    /// Exact amount delivered to the Taker destination after the fee.
    #[must_use]
    pub const fn received_amount_piconero(&self) -> u64 {
        self.received_amount_piconero
    }

    /// Exact fee charged by the sole sweep transaction.
    #[must_use]
    pub const fn fee_piconero(&self) -> u64 {
        self.fee_piconero
    }

    /// Height returned after generating the fixed local confirmation count.
    #[must_use]
    pub const fn confirmation_tip_height(&self) -> u64 {
        self.confirmation_tip_height
    }
}

/// Exclusive advisory lock on one role's wallet-rpc.
///
/// Every tool that opens, uses, or closes a wallet on the role's wallet-rpc
/// acquires this lock on the same path first and holds it until it is done;
/// dropping the guard releases it. [`Self::acquire`] blocks until the lock is
/// free.
#[derive(Debug)]
#[must_use]
pub struct MoneroWalletRpcLock {
    _file: File,
}

impl MoneroWalletRpcLock {
    /// Opens or creates the lock file and blocks until its exclusive lock is held.
    ///
    /// # Errors
    ///
    /// Fails if the file cannot be opened or locked.
    pub fn acquire(path: &Path) -> Result<Self, MoneroWalletEffectError> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .map_err(|_| MoneroWalletEffectError::WalletRpcLock)?;
        file.lock()
            .map_err(|_| MoneroWalletEffectError::WalletRpcLock)?;
        Ok(Self { _file: file })
    }
}

/// The role's own wallet on its wallet-rpc.
struct RoleWallet {
    filename: String,
    password: String,
}

/// Typed one-attempt wallet and Regtest-daemon effect boundary.
pub struct MoneroRegtestWalletEffects {
    daemon: RegtestDaemonJsonRpcClient,
    wallet: WalletClient,
    /// `sweep_single`, which the typed client does not offer, goes through
    /// this client to the same wallet-rpc with the same Digest login.
    raw_wallet: RawWalletRpc,
    daemon_origin: String,
    wallet_origin: String,
    role_wallet: Option<RoleWallet>,
}

/// A bare JSON-RPC route to the wallet-rpc for the calls that send money
/// (`transfer`, and `sweep_single`, which the typed client lacks). Unlike the
/// typed client it reads the HTTP status before the body, so a request the
/// wallet-rpc refused is told apart from one whose answer was lost.
struct RawWalletRpc {
    client: reqwest::Client,
    url: String,
    username: String,
    password: String,
}

/// Attempts of one raw call's Digest exchange. A 401 to the answering leg is
/// what the client's pool causes when it sends the answer on another
/// connection than its challenge (see the crate documentation); the
/// wallet-rpc ran nothing, so the exchange starts once more with a fresh
/// challenge. Credentials it refuses are refused every time.
const RAW_CALL_AUTH_ATTEMPTS: u32 = 3;

/// How a raw wallet-rpc call failed.
#[derive(Debug)]
enum RawCallError {
    /// The wallet-rpc did not run the method: the connection failed, or it
    /// refused the request with a 4xx status before reading its body as a
    /// call (a 401 after every Digest attempt, among them).
    NotRun(String),
    /// It may have run the method: the answer was lost or cut off, came with
    /// a 5xx status, was not a JSON-RPC answer, or was a JSON-RPC error.
    MayHaveRun(String),
}

impl fmt::Display for RawCallError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRun(cause) | Self::MayHaveRun(cause) => formatter.write_str(cause),
        }
    }
}

impl RawWalletRpc {
    fn new(endpoint: &LoopbackRpcEndpoint) -> Result<Self, MoneroWalletEffectError> {
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(super::RPC_REQUEST_TIMEOUT)
                .build()
                .map_err(|_| MoneroWalletEffectError::RpcClientBuild)?,
            url: format!("{}/json_rpc", endpoint.base_url),
            username: endpoint.username.clone(),
            password: endpoint.password.clone(),
        })
    }

    /// One call. Only the Digest exchange is made again, and only after a
    /// 401, which proves the wallet-rpc ran nothing; the caller decides what
    /// any other failure means.
    async fn call(
        &self,
        method: &'static str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, RawCallError> {
        use diqwest::WithDigestAuth as _;
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": "0", "method": method, "params": params,
        })
        .to_string();
        let mut attempt = 1;
        let response = loop {
            let sent = self
                .client
                .post(&self.url)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body.clone())
                .send_with_digest_auth(&self.username, &self.password)
                .await;
            let response = match sent {
                Ok(response) => response,
                Err(diqwest::error::Error::Reqwest(error)) if error.is_connect() => {
                    return Err(RawCallError::NotRun(format!("{method}: {error}")));
                }
                Err(diqwest::error::Error::Reqwest(error)) => {
                    return Err(RawCallError::MayHaveRun(format!("{method}: {error}")));
                }
                // The challenge could not be answered: only the
                // unauthenticated leg reached the wallet-rpc.
                Err(error) => return Err(RawCallError::NotRun(format!("{method}: {error}"))),
            };
            let status = response.status();
            if status == reqwest::StatusCode::UNAUTHORIZED && attempt < RAW_CALL_AUTH_ATTEMPTS {
                attempt += 1;
                continue;
            }
            if status.is_client_error() {
                return Err(RawCallError::NotRun(format!(
                    "{method}: HTTP {}",
                    status.as_u16()
                )));
            }
            if status != reqwest::StatusCode::OK {
                return Err(RawCallError::MayHaveRun(format!(
                    "{method}: HTTP {}",
                    status.as_u16()
                )));
            }
            break response;
        };
        let bytes = response
            .bytes()
            .await
            .map_err(|error| RawCallError::MayHaveRun(format!("{method}: {error}")))?;
        let mut reply: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|error| RawCallError::MayHaveRun(format!("{method}: {error}")))?;
        if let Some(error) = reply.get("error") {
            return Err(RawCallError::MayHaveRun(format!(
                "{method}: wallet-rpc error: {error}"
            )));
        }
        Ok(reply["result"].take())
    }
}

impl MoneroRegtestWalletEffects {
    /// Creates a composite from distinct credential-configured loopback origins.
    ///
    /// # Errors
    ///
    /// Rejects aliased origins or a typed-client construction failure.
    pub fn new(
        daemon: &LoopbackRpcEndpoint,
        wallet: &LoopbackRpcEndpoint,
    ) -> Result<Self, MoneroWalletEffectError> {
        if daemon.base_url == wallet.base_url {
            return Err(MoneroWalletEffectError::AliasedRpcOrigins);
        }
        let daemon_client = daemon
            .client()
            .map_err(|_| MoneroWalletEffectError::RpcClientBuild)?
            .daemon()
            .regtest();
        let wallet_client = wallet
            .client()
            .map_err(|_| MoneroWalletEffectError::RpcClientBuild)?
            .wallet();
        Ok(Self {
            daemon: daemon_client,
            wallet: wallet_client,
            raw_wallet: RawWalletRpc::new(wallet)?,
            daemon_origin: daemon.base_url.clone(),
            wallet_origin: wallet.base_url.clone(),
            role_wallet: None,
        })
    }

    /// Names the role wallet that every effect reopens before it acts.
    ///
    /// The password may be empty, as the devnet's role wallets have none.
    ///
    /// # Errors
    ///
    /// Rejects an unsafe filename or password before any RPC.
    pub fn with_role_wallet(
        mut self,
        filename: String,
        password: String,
    ) -> Result<Self, MoneroWalletEffectError> {
        validate_wallet_filename(&filename)?;
        if !password.is_empty() && !valid_credential(&password, true) {
            return Err(MoneroWalletEffectError::InvalidWalletPassword);
        }
        self.role_wallet = Some(RoleWallet { filename, password });
        Ok(self)
    }

    /// Opens the role wallet named by [`Self::with_role_wallet`], closing the
    /// wallet-rpc's current wallet; without one it does nothing.
    ///
    /// # Errors
    ///
    /// Fails closed if the typed wallet open operation fails.
    pub async fn open_role_wallet(&self) -> Result<(), MoneroWalletEffectError> {
        let Some(role_wallet) = &self.role_wallet else {
            return Ok(());
        };
        read_with_retries(|| {
            self.wallet.open_wallet(
                role_wallet.filename.clone(),
                Some(role_wallet.password.clone()),
            )
        })
        .await
        .map_err(rpc_error(OP_OPEN_ROLE_WALLET))
    }

    /// Returns the role wallet's (or, without one, the currently open
    /// wallet's) primary standard address.
    ///
    /// The typed wallet RPC is restricted to account zero and address index
    /// zero. The returned account address is then rejected unless it is a
    /// standard address, making it suitable for the destination and local
    /// Regtest-mining boundaries in this module.
    ///
    /// # Errors
    ///
    /// Fails closed if the typed wallet query fails or the wallet returns a
    /// non-standard primary address.
    pub async fn primary_standard_address(&self) -> Result<Address, MoneroWalletEffectError> {
        self.open_role_wallet().await?;
        let address =
            read_with_retries(|| self.wallet.get_address(WALLET_ACCOUNT_INDEX, Some(vec![0])))
                .await
                .map_err(rpc_error("current wallet primary address"))?
                .address;
        validate_standard_address(&address)?;
        Ok(address)
    }

    /// The role wallet's (or, without one, the open wallet's) primary address,
    /// total and unlocked balance in piconero.
    ///
    /// # Errors
    ///
    /// Fails closed if the typed wallet queries fail.
    pub async fn role_wallet_balance(
        &self,
    ) -> Result<(Address, u64, u64), MoneroWalletEffectError> {
        let address = self.primary_standard_address().await?;
        let balance = read_with_retries(|| self.wallet.get_balance(WALLET_ACCOUNT_INDEX, None))
            .await
            .map_err(rpc_error("role wallet balance"))?;
        Ok((
            address,
            balance.balance.as_pico(),
            balance.unlocked_balance.as_pico(),
        ))
    }

    /// Submits one exact transfer from the role wallet (or, without one, the
    /// currently open wallet) and locally generates ten confirmation blocks.
    ///
    /// The destination and amount remain subject to a separate
    /// [`crate::MoneroOutputVerifier`] observation before any secret release.
    ///
    /// # Errors
    ///
    /// Fails before submission for a non-standard destination or zero amount,
    /// and fails closed on any typed wallet/daemon response mismatch.
    pub async fn fund_exact_and_confirm(
        &self,
        destination: Address,
        amount_piconero: u64,
    ) -> Result<ConfirmedMoneroFunding, MoneroWalletEffectError> {
        validate_standard_address(&destination)?;
        let amount = NonZeroU64::new(amount_piconero).ok_or(MoneroWalletEffectError::ZeroAmount)?;
        self.open_role_wallet().await?;
        let mining_address =
            read_with_retries(|| self.wallet.get_address(WALLET_ACCOUNT_INDEX, Some(vec![0])))
                .await
                .map_err(rpc_error(OP_FUNDING_WALLET_ADDRESS))?
                .address;
        let transaction_id = self.submit_exact(destination, amount).await?;
        let confirmation_tip_height = self.mine_confirmations(mining_address).await?;
        Ok(ConfirmedMoneroFunding {
            transaction_id,
            amount_piconero: amount.get(),
            confirmation_tip_height,
        })
    }

    /// Submits one exact transfer from the role wallet (or, without one, the
    /// currently open wallet) to the Stage-A shared address and returns its
    /// transaction; it never mines. This is the funding effect for a network
    /// whose blocks others mine, such as Stagenet: the caller waits for the
    /// confirmations with [`crate::MoneroOutputVerifier`].
    ///
    /// # Errors
    ///
    /// Fails before submission for an address that is not a standard one or a
    /// zero amount, and fails closed on a typed wallet response mismatch.
    pub async fn fund_shared_exact_once(
        &self,
        destination: &MoneroSharedAddressV1,
        amount_piconero: u64,
    ) -> Result<MoneroTransactionId, MoneroWalletEffectError> {
        let destination = parse_shared_standard_address(destination)?;
        let amount = NonZeroU64::new(amount_piconero).ok_or(MoneroWalletEffectError::ZeroAmount)?;
        self.open_role_wallet().await?;
        self.submit_exact(destination, amount).await
    }

    /// Sends one exact transfer from the open wallet and returns its
    /// transaction.
    ///
    /// The transfer goes as one raw JSON-RPC call ([`RawWalletRpc`]), whose
    /// failure says whether the wallet-rpc can have run it. One it did not
    /// run is [`MoneroWalletEffectError::RequestNotSent`]. One it may have
    /// run (the answer lost, cut off or not a transfer receipt) is looked up
    /// in the wallet's outgoing and pending transfers by destination and
    /// amount before the caller may send again: the wallet-rpc runs one
    /// request at a time, so the lookup sees the transfer's outcome. Found,
    /// it is the funding; not found, nothing was sent; a lookup that fails
    /// leaves the transfer in doubt.
    async fn submit_exact(
        &self,
        destination: Address,
        amount: NonZeroU64,
    ) -> Result<MoneroTransactionId, MoneroWalletEffectError> {
        let sent = self
            .raw_wallet
            .call(
                "transfer",
                serde_json::json!({
                    "destinations": [{
                        "address": destination.to_string(),
                        "amount": amount.get(),
                    }],
                    "account_index": WALLET_ACCOUNT_INDEX,
                    "priority": 0,
                    "do_not_relay": false,
                    "get_tx_key": true,
                }),
            )
            .await;
        let reply = match sent {
            Ok(reply) => reply,
            Err(RawCallError::NotRun(cause)) => {
                return Err(MoneroWalletEffectError::RequestNotSent {
                    operation: OP_FUND_EXACT_TRANSFER,
                    cause: bounded_cause(&cause),
                });
            }
            Err(RawCallError::MayHaveRun(cause)) => {
                return self.look_up_transfer(destination, amount, &cause).await;
            }
        };
        let Some(transaction_id) = transaction_hash(&reply["tx_hash"]) else {
            return self
                .look_up_transfer(
                    destination,
                    amount,
                    "transfer answer has no transaction hash",
                )
                .await;
        };
        if reply["amount"].as_u64() != Some(amount.get()) {
            return Err(MoneroWalletEffectError::FundingAmountMismatch);
        }
        Ok(transaction_id.into())
    }

    /// Resolves a transfer whose outcome is unknown (`cause`) from the open
    /// wallet's outgoing and pending transfers: the one paying exactly
    /// `amount` to `destination` is the funding.
    async fn look_up_transfer(
        &self,
        destination: Address,
        amount: NonZeroU64,
        cause: &str,
    ) -> Result<MoneroTransactionId, MoneroWalletEffectError> {
        let in_doubt = |lookup: String| MoneroWalletEffectError::Rpc {
            operation: OP_FUND_EXACT_TRANSFER,
            cause: bounded_cause(&format!("{cause}; {lookup}")),
        };
        let mut found = Vec::new();
        let mut attempt = 1;
        let transfers = loop {
            match self
                .raw_wallet
                .call(
                    "get_transfers",
                    serde_json::json!({
                        "in": false, "out": true, "pending": true, "failed": false,
                        "pool": false, "account_index": WALLET_ACCOUNT_INDEX,
                    }),
                )
                .await
            {
                Ok(transfers) => break transfers,
                Err(_) if attempt < LOOKUP_ATTEMPTS => attempt += 1,
                Err(error) => return Err(in_doubt(format!("lookup failed: {error}"))),
            }
        };
        let destination = destination.to_string();
        for category in ["out", "pending"] {
            let Some(entries) = transfers.get(category) else {
                continue;
            };
            let Some(entries) = entries.as_array() else {
                return Err(in_doubt(format!("lookup: `{category}` is not a list")));
            };
            for entry in entries {
                let pays = entry["destinations"].as_array().is_some_and(|paid| {
                    paid.iter().any(|paid| {
                        paid["address"].as_str() == Some(destination.as_str())
                            && paid["amount"].as_u64() == Some(amount.get())
                    })
                });
                if pays {
                    found.push(transaction_hash(&entry["txid"]).ok_or_else(|| {
                        in_doubt("lookup: a matching transfer has no transaction id".to_owned())
                    })?);
                }
            }
        }
        found.sort_unstable();
        found.dedup();
        match found.as_slice() {
            [] => Err(MoneroWalletEffectError::RequestNotSent {
                operation: OP_FUND_EXACT_TRANSFER,
                cause: bounded_cause(&format!(
                    "{cause}; the wallet holds no transfer of the amount to the destination"
                )),
            }),
            [transaction_id] => Ok((*transaction_id).into()),
            _ => Err(in_doubt(
                "lookup: several transfers of the amount to the destination".to_owned(),
            )),
        }
    }

    /// Funds the exact Stage-A shared address and locally generates ten
    /// confirmation blocks.
    ///
    /// This is the typed shared-address counterpart to
    /// [`Self::fund_exact_and_confirm`]. It keeps callers from reparsing the
    /// SDK-validated address through a second public API boundary.
    ///
    /// # Errors
    ///
    /// Fails before submission if the validated SDK address cannot be parsed as
    /// a standard Monero address, and otherwise preserves the exact funding
    /// method's fail-closed behavior.
    pub async fn fund_shared_exact_and_confirm(
        &self,
        destination: &MoneroSharedAddressV1,
        amount_piconero: u64,
    ) -> Result<ConfirmedMoneroFunding, MoneroWalletEffectError> {
        let destination = parse_shared_standard_address(destination)?;
        self.fund_exact_and_confirm(destination, amount_piconero)
            .await
    }

    /// Closes the current wallet (after reopening the role wallet, so that a
    /// restarted wallet-rpc has one to close) and restores the exact Stage-A
    /// shared address as an official view-only wallet, which stays open.
    ///
    /// All caller-controlled configuration and the private/public view-key
    /// binding are validated before the current wallet is closed. The private
    /// view key is consumed and handed directly to the typed wallet client;
    /// `spendkey` is deliberately `None`, so the resulting wallet cannot spend.
    /// This method does not refresh. Call [`Self::refresh_from_height`] after
    /// the funding transfer has been confirmed.
    ///
    /// # Errors
    ///
    /// Fails before the first RPC for an unsafe filename/password, malformed or
    /// non-standard shared address, mismatched private view key, or invalid key
    /// representation. Fails closed if closing or generating the wallet fails,
    /// or if wallet RPC returns an address other than the exact Stage-A address.
    pub async fn restore_shared_view_only(
        &self,
        expected_address: &MoneroSharedAddressV1,
        private_view_key: MoneroPrivateViewKey,
        wallet_filename: String,
        wallet_password: String,
        restore_height: u64,
    ) -> Result<(), MoneroWalletEffectError> {
        validate_wallet_filename(&wallet_filename)?;
        if !valid_credential(&wallet_password, true) {
            return Err(MoneroWalletEffectError::InvalidWalletPassword);
        }
        let address = parse_shared_standard_address(expected_address)?;
        validate_shared_view_key(expected_address.public_view_key(), &private_view_key)?;
        let view_bytes = private_view_key.into_monero_little_endian();
        let view_key = PrivateKey::from_slice(&*view_bytes)
            .map_err(|_| MoneroWalletEffectError::InvalidPrivateKey)?;

        self.open_role_wallet().await?;
        self.wallet
            .close_wallet()
            .await
            .map_err(rpc_error(OP_CLOSE_CURRENT_WALLET))?;
        let created = self
            .wallet
            .generate_from_keys(GenerateFromKeysArgs {
                restore_height: Some(restore_height),
                filename: wallet_filename,
                address,
                spendkey: None,
                viewkey: view_key,
                password: wallet_password,
                autosave_current: Some(true),
            })
            .await
            .map_err(rpc_error("generate shared view-only wallet"))?;
        if created.address != address {
            return Err(MoneroWalletEffectError::RestoredAddressMismatch);
        }
        Ok(())
    }

    /// Refreshes the currently open wallet from one exact chain height.
    ///
    /// This separate effect lets an operator fund and confirm the shared output
    /// before asking the newly created view-only wallet to scan for it.
    ///
    /// # Errors
    ///
    /// Fails closed if the typed wallet refresh operation fails.
    pub async fn refresh_from_height(
        &self,
        restore_height: u64,
    ) -> Result<(), MoneroWalletEffectError> {
        read_with_retries(|| self.wallet.refresh(Some(restore_height)))
            .await
            .map_err(rpc_error(OP_REFRESH_CURRENT_WALLET))?;
        Ok(())
    }

    /// Restores the point-checked shared wallet, verifies its sole unlocked
    /// principal, submits one sweep, and locally mines ten confirmation blocks.
    ///
    /// The reconstructed spend key and private view key are consumed. They are
    /// handed directly to the typed wallet client and are never returned,
    /// serialized by this crate, or included in diagnostics.
    ///
    /// # Errors
    ///
    /// Fails closed on an unsafe wallet filename/password, address or key
    /// mismatch, no unspent output of the exact principal, typed RPC failure,
    /// or sweep accounting that does not partition the principal.
    #[allow(clippy::too_many_arguments)]
    pub async fn restore_shared_and_sweep(
        &self,
        expected_address: &MoneroSharedAddressV1,
        reconstructed_spend_key: ReconstructedMoneroSpendKey,
        private_view_key: MoneroPrivateViewKey,
        wallet_filename: String,
        wallet_password: String,
        restore_height: u64,
        expected_amount_piconero: u64,
        destination: Address,
        mining_address: Address,
    ) -> Result<ConfirmedMoneroSweep, MoneroWalletEffectError> {
        validate_standard_address(&mining_address)?;
        let submitted = self
            .restore_shared_and_sweep_once(
                expected_address,
                reconstructed_spend_key,
                private_view_key,
                wallet_filename,
                wallet_password,
                restore_height,
                expected_amount_piconero,
                destination,
            )
            .await?;
        let confirmation_tip_height = self.mine_confirmations(mining_address).await?;
        Ok(ConfirmedMoneroSweep {
            transaction_id: submitted.transaction_id,
            funded_amount_piconero: submitted.funded_amount_piconero,
            received_amount_piconero: submitted.received_amount_piconero,
            fee_piconero: submitted.fee_piconero,
            confirmation_tip_height,
        })
    }

    /// Restores the shared wallet from its keys, or reopens the file of the
    /// same name a stopped earlier run restored (a sweep driven again); returns
    /// the wallet's primary address. What the stopped run sent, if anything,
    /// spent the one output: the sweep finds no unspent output of the amount
    /// after it, and the daemon's key-image check refuses a second spend.
    async fn generate_or_reopen_shared(
        &self,
        arguments: GenerateFromKeysArgs,
    ) -> Result<Address, MoneroWalletEffectError> {
        let (filename, password) = (arguments.filename.clone(), arguments.password.clone());
        if let Ok(created) = self.wallet.generate_from_keys(arguments).await {
            return Ok(created.address);
        }
        read_with_retries(|| {
            self.wallet
                .open_wallet(filename.clone(), Some(password.clone()))
        })
        .await
        .map_err(rpc_error(OP_GENERATE_SHARED_FROM_KEYS))?;
        Ok(
            read_with_retries(|| self.wallet.get_address(WALLET_ACCOUNT_INDEX, Some(vec![0])))
                .await
                .map_err(rpc_error(OP_RESTORED_PRIMARY_ADDRESS))?
                .address,
        )
    }

    /// Restores the point-checked shared wallet and submits exactly one sweep.
    ///
    /// Unlike [`Self::restore_shared_and_sweep`], this method never mines or
    /// waits for confirmation. It is the sending half of the durable actor
    /// boundary: callers persist the returned transaction identity, then mine
    /// with [`Self::mine_confirmations`] in the same run, and restart through a
    /// separate read-only finality observer. Only a run stopped before it
    /// persisted the identity runs again; it reopens the wallet file of the
    /// same name if one exists, and a spent output is never spent again.
    ///
    /// # Errors
    ///
    /// Fails closed on an unsafe wallet filename/password, address or key
    /// mismatch, no unspent output of the exact principal, typed RPC failure,
    /// or sweep accounting that does not partition the principal.
    #[allow(clippy::too_many_arguments)]
    pub async fn restore_shared_and_sweep_once(
        &self,
        expected_address: &MoneroSharedAddressV1,
        reconstructed_spend_key: ReconstructedMoneroSpendKey,
        private_view_key: MoneroPrivateViewKey,
        wallet_filename: String,
        wallet_password: String,
        restore_height: u64,
        expected_amount_piconero: u64,
        destination: Address,
    ) -> Result<SubmittedMoneroSweep, MoneroWalletEffectError> {
        validate_wallet_filename(&wallet_filename)?;
        if !valid_credential(&wallet_password, true) {
            return Err(MoneroWalletEffectError::InvalidWalletPassword);
        }
        validate_standard_address(&destination)?;
        let expected_amount =
            NonZeroU64::new(expected_amount_piconero).ok_or(MoneroWalletEffectError::ZeroAmount)?;
        let address = parse_shared_standard_address(expected_address)?;
        validate_shared_view_key(expected_address.public_view_key(), &private_view_key)?;

        let spend_bytes = reconstructed_spend_key.into_monero_little_endian();
        let view_bytes = private_view_key.into_monero_little_endian();
        let spend_key = PrivateKey::from_slice(&*spend_bytes)
            .map_err(|_| MoneroWalletEffectError::InvalidPrivateKey)?;
        let view_key = PrivateKey::from_slice(&*view_bytes)
            .map_err(|_| MoneroWalletEffectError::InvalidPrivateKey)?;

        self.open_role_wallet().await?;
        self.wallet
            .close_wallet()
            .await
            .map_err(rpc_error(OP_CLOSE_CURRENT_WALLET))?;
        let created = self
            .generate_or_reopen_shared(GenerateFromKeysArgs {
                restore_height: Some(restore_height),
                filename: wallet_filename,
                address,
                spendkey: Some(spend_key),
                viewkey: view_key,
                password: wallet_password,
                autosave_current: Some(true),
            })
            .await?;
        if created != address {
            return Err(MoneroWalletEffectError::RestoredAddressMismatch);
        }

        self.sweep_exact_output(restore_height, expected_amount, destination)
            .await
    }

    /// Sweeps the swap's one output out of the open shared wallet.
    ///
    /// The shared address is known to the counterparty, so anyone who learns
    /// it can send it more outputs. They are left where they are: the sweep
    /// spends exactly one unspent output of the expected amount at the
    /// primary address (`sweep_single` by its key image), so an extra output,
    /// confirmed or not, of any amount, neither blocks the sweep nor adds an
    /// input, a fee or a second transaction. Among outputs of the expected
    /// amount the earliest confirmed is taken: the swap's, which was
    /// verified with its confirmations before any sweep. A later output of
    /// the same amount can only be taken once the swap's is spent, and then
    /// pays the same amount to the same destination.
    async fn sweep_exact_output(
        &self,
        restore_height: u64,
        expected_amount: NonZeroU64,
        destination: Address,
    ) -> Result<SubmittedMoneroSweep, MoneroWalletEffectError> {
        read_with_retries(|| self.wallet.refresh(Some(restore_height)))
            .await
            .map_err(rpc_error(OP_REFRESH_RESTORED_WALLET))?;
        let transfers = read_with_retries(|| {
            self.wallet.incoming_transfers(
                TransferType::Available,
                Some(WALLET_ACCOUNT_INDEX),
                Some(vec![0]),
            )
        })
        .await
        .map_err(rpc_error(OP_READ_RESTORED_OUTPUTS))?;
        let output = transfers
            .transfers
            .unwrap_or_default()
            .into_iter()
            .filter(|transfer| {
                !transfer.spent
                    && transfer.subaddr_index.major == WALLET_ACCOUNT_INDEX
                    && transfer.subaddr_index.minor == 0
                    && transfer.amount.as_pico() == expected_amount.get()
                    && transfer.block_height.is_some()
            })
            .min_by_key(|transfer| (transfer.block_height, transfer.global_index))
            .ok_or(MoneroWalletEffectError::RestoredBalanceMismatch)?;
        let key_image = output
            .key_image
            .filter(|image| image.len() == 64 && image.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .ok_or(MoneroWalletEffectError::RestoredBalanceMismatch)?;
        let sweep = self
            .raw_wallet
            .call(
                "sweep_single",
                serde_json::json!({
                    "address": destination.to_string(),
                    "key_image": key_image,
                    "priority": 0,
                    "ring_size": 0,
                    "unlock_time": 0,
                    "do_not_relay": false,
                }),
            )
            .await
            .map_err(raw_call_error(OP_SWEEP_RECONSTRUCTED_WALLET))?;
        let transaction_id: [u8; 32] = sweep["tx_hash"]
            .as_str()
            .and_then(|hash| hex::decode(hash).ok())
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or(MoneroWalletEffectError::InvalidSweepTransactionCount)?;
        let amount = sweep["amount"]
            .as_u64()
            .ok_or(MoneroWalletEffectError::InvalidSweepAccountingCount)?;
        let fee = sweep["fee"]
            .as_u64()
            .ok_or(MoneroWalletEffectError::InvalidSweepAccountingCount)?;
        let (received_amount_piconero, fee_piconero) = validate_sweep_accounting(
            &[Amount::from_pico(amount)],
            &[Amount::from_pico(fee)],
            expected_amount.get(),
        )?;
        Ok(SubmittedMoneroSweep {
            transaction_id: transaction_id.into(),
            funded_amount_piconero: expected_amount.get(),
            received_amount_piconero,
            fee_piconero,
        })
    }

    /// Locally generates the fixed confirmation count on the Regtest daemon,
    /// paying the coinbase to `mining_address`, and returns the new height.
    ///
    /// # Errors
    ///
    /// Fails before mining for a non-standard address, and fails closed when
    /// each of `MINING_ATTEMPTS` typed daemon calls fails or the one that
    /// succeeds reports a block count other than the fixed one.
    pub async fn mine_confirmations(
        &self,
        mining_address: Address,
    ) -> Result<u64, MoneroWalletEffectError> {
        validate_standard_address(&mining_address)?;
        let mut attempt = 1;
        let generated = loop {
            match self
                .daemon
                .generate_blocks(REQUIRED_MONERO_CONFIRMATIONS, mining_address)
                .await
            {
                Ok(generated) => break generated,
                Err(_) if attempt < MINING_ATTEMPTS => attempt += 1,
                Err(error) => return Err(rpc_error("generate confirmation blocks")(error)),
            }
        };
        let block_count = generated.blocks.as_ref().map_or(0, Vec::len);
        if block_count != usize::try_from(REQUIRED_MONERO_CONFIRMATIONS).unwrap_or(usize::MAX) {
            return Err(MoneroWalletEffectError::ConfirmationBlockCountMismatch);
        }
        Ok(generated.height)
    }
}

impl fmt::Debug for MoneroRegtestWalletEffects {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MoneroRegtestWalletEffects")
            .field("daemon_origin", &self.daemon_origin)
            .field("wallet_origin", &self.wallet_origin)
            .field(
                "role_wallet",
                &self.role_wallet.as_ref().map(|wallet| &wallet.filename),
            )
            .field("credentials", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

/// Fail-closed configuration and typed effect errors.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum MoneroWalletEffectError {
    /// Daemon and wallet trust roles cannot share one origin.
    #[error("Monero daemon and wallet RPC origins must be distinct")]
    AliasedRpcOrigins,
    /// The maintained typed RPC client could not be built.
    #[error("failed to build typed Monero wallet-effect RPC client")]
    RpcClientBuild,
    /// One typed RPC operation failed; `cause` is the client's error text
    /// (transport, HTTP or the wallet-rpc's own error), which carries no
    /// credential: the login travels in a Digest header, never in a URL.
    #[error("typed Monero wallet-effect operation `{operation}` failed: {cause}")]
    Rpc {
        /// The operation, a fixed name.
        operation: &'static str,
        /// The underlying error, bounded.
        cause: String,
    },
    /// Funding or destination amount cannot be zero.
    #[error("Monero wallet-effect amount must be nonzero")]
    ZeroAmount,
    /// Wallet transfer response did not preserve the exact requested principal.
    #[error("Monero funding response amount does not match the requested principal")]
    FundingAmountMismatch,
    /// Only standard addresses participate in this `PoC`.
    #[error("Monero wallet effect requires standard addresses")]
    NonStandardAddress,
    /// Shared address from the validated SDK could not be parsed.
    #[error("validated shared Monero address could not be parsed")]
    InvalidSharedAddress,
    /// Owner-selected wallet filename is unsafe for the fixed wallet directory.
    #[error("reconstructed Monero wallet filename is invalid")]
    InvalidWalletFilename,
    /// Wallet password violates the bounded local credential policy.
    #[error("reconstructed Monero wallet password is invalid")]
    InvalidWalletPassword,
    /// Point-checked private key could not be represented by the maintained client.
    #[error("reconstructed Monero private key is invalid")]
    InvalidPrivateKey,
    /// Private view key does not open the exact Stage-A shared address.
    #[error("private Monero view key differs from the agreed shared address")]
    PrivateViewKeyMismatch,
    /// Official wallet returned an address different from the agreed address.
    #[error("restored Monero wallet address differs from the agreed shared address")]
    RestoredAddressMismatch,
    /// Restored wallet holds no unspent confirmed output of the exact
    /// expected principal: nothing was sent.
    #[error("restored Monero wallet holds no unspent output of the exact expected principal")]
    RestoredBalanceMismatch,
    /// Local daemon did not return the fixed number of generated blocks.
    #[error("Monero Regtest confirmation block count mismatch")]
    ConfirmationBlockCountMismatch,
    /// Sweep response was empty or exceeded the semantic bound.
    #[error("Monero sweep transaction count is invalid")]
    InvalidSweepTransactionCount,
    /// Sweep amount and fee vectors must each describe the sole transaction.
    #[error("Monero sweep accounting entry count is invalid")]
    InvalidSweepAccountingCount,
    /// The sole delivered amount plus fee must equal the exact unlocked principal.
    #[error("Monero sweep amount and fee do not partition the exact principal")]
    SweepAccountingMismatch,
    /// A multi-transaction sweep is outside the first vertical `PoC`.
    #[error("split Monero sweep is unsupported by the first vertical PoC")]
    SplitSweepUnsupported,
    /// The role's wallet-rpc lock file could not be opened or locked.
    #[error("Monero wallet-rpc lock could not be acquired")]
    WalletRpcLock,
    /// The wallet-rpc did not run the call that sends money (`operation`):
    /// it could not be reached or refused the request, or the wallet holds
    /// no transfer it made. Nothing was sent; the call may be made again.
    #[error("Monero wallet-effect operation `{operation}` sent nothing: {cause}")]
    RequestNotSent {
        /// The operation, a fixed name.
        operation: &'static str,
        /// What failed, bounded; it carries no credential.
        cause: String,
    },
}

impl MoneroWalletEffectError {
    /// Whether this error of [`MoneroRegtestWalletEffects::restore_shared_and_sweep_once`]
    /// proves the sweep was never sent: it failed a check, or an RPC call it
    /// makes before `sweep_single`. Anything else (the sweep call itself, or
    /// its answer) may have sent it.
    #[must_use]
    pub fn sweep_not_sent(&self) -> bool {
        match self {
            Self::InvalidWalletFilename
            | Self::InvalidWalletPassword
            | Self::NonStandardAddress
            | Self::ZeroAmount
            | Self::InvalidSharedAddress
            | Self::PrivateViewKeyMismatch
            | Self::InvalidPrivateKey
            | Self::RestoredAddressMismatch
            | Self::RestoredBalanceMismatch
            | Self::WalletRpcLock => true,
            Self::RequestNotSent { operation, .. } => *operation == OP_SWEEP_RECONSTRUCTED_WALLET,
            Self::Rpc { operation, .. } => matches!(
                *operation,
                OP_OPEN_ROLE_WALLET
                    | OP_CLOSE_CURRENT_WALLET
                    | OP_GENERATE_SHARED_FROM_KEYS
                    | OP_RESTORED_PRIMARY_ADDRESS
                    | OP_REFRESH_RESTORED_WALLET
                    | OP_READ_RESTORED_OUTPUTS
            ),
            _ => false,
        }
    }
}

impl MoneroWalletEffectError {
    /// Whether this error of a funding effect
    /// ([`MoneroRegtestWalletEffects::fund_exact_and_confirm`],
    /// [`MoneroRegtestWalletEffects::fund_shared_exact_once`] and their
    /// shared-address forms, with the role wallet's refresh a caller makes
    /// before them) proves the transfer was never sent: a check failed, an
    /// RPC call before the transfer failed, or the transfer call
    /// [`MoneroWalletEffectError::RequestNotSent`]. Anything else, a mining
    /// failure after the transfer among them, may follow a sent transfer.
    #[must_use]
    pub fn funding_not_sent(&self) -> bool {
        match self {
            Self::NonStandardAddress
            | Self::ZeroAmount
            | Self::InvalidSharedAddress
            | Self::InvalidWalletPassword
            | Self::InvalidWalletFilename
            | Self::WalletRpcLock => true,
            Self::RequestNotSent { operation, .. } => *operation == OP_FUND_EXACT_TRANSFER,
            Self::Rpc { operation, .. } => matches!(
                *operation,
                OP_OPEN_ROLE_WALLET | OP_REFRESH_CURRENT_WALLET | OP_FUNDING_WALLET_ADDRESS
            ),
            _ => false,
        }
    }
}

/// The longest underlying RPC error text an [`MoneroWalletEffectError::Rpc`] keeps.
const MAX_RPC_CAUSE_CHARS: usize = 400;

/// Maps a typed client failure of `operation` to [`MoneroWalletEffectError::Rpc`],
/// keeping its error chain.
// The operations a reconstructed-wallet sweep runs before it sends
// (`sweep_single`): a typed RPC failure of one of them proves nothing was sent
// ([`MoneroWalletEffectError::sweep_not_sent`]).
const OP_OPEN_ROLE_WALLET: &str = "open role wallet";
const OP_CLOSE_CURRENT_WALLET: &str = "close current wallet";
const OP_GENERATE_SHARED_FROM_KEYS: &str = "generate shared wallet from keys";
const OP_RESTORED_PRIMARY_ADDRESS: &str = "restored wallet primary address";
const OP_REFRESH_RESTORED_WALLET: &str = "refresh restored wallet";
const OP_READ_RESTORED_OUTPUTS: &str = "read restored wallet outputs";
const OP_SWEEP_RECONSTRUCTED_WALLET: &str = "sweep reconstructed wallet";
// The operations of a funding before and of its transfer
// ([`MoneroWalletEffectError::funding_not_sent`]).
const OP_REFRESH_CURRENT_WALLET: &str = "refresh current wallet";
const OP_FUNDING_WALLET_ADDRESS: &str = "funding wallet address";
const OP_FUND_EXACT_TRANSFER: &str = "fund exact transfer";
/// Attempts of the read that looks up a transfer in doubt.
const LOOKUP_ATTEMPTS: u32 = 3;

fn rpc_error(operation: &'static str) -> impl FnOnce(anyhow::Error) -> MoneroWalletEffectError {
    move |error| MoneroWalletEffectError::Rpc {
        operation,
        cause: bounded_cause(&format!("{error:#}")),
    }
}

/// Maps a raw call's failure: one the wallet-rpc did not run sent nothing.
fn raw_call_error(operation: &'static str) -> impl FnOnce(RawCallError) -> MoneroWalletEffectError {
    move |error| match error {
        RawCallError::NotRun(cause) => MoneroWalletEffectError::RequestNotSent {
            operation,
            cause: bounded_cause(&cause),
        },
        RawCallError::MayHaveRun(cause) => MoneroWalletEffectError::Rpc {
            operation,
            cause: bounded_cause(&cause),
        },
    }
}

fn bounded_cause(cause: &str) -> String {
    cause.chars().take(MAX_RPC_CAUSE_CHARS).collect()
}

/// A 32-byte transaction hash from its hexadecimal JSON string.
fn transaction_hash(value: &serde_json::Value) -> Option<[u8; 32]> {
    value
        .as_str()
        .and_then(|hash| hex::decode(hash).ok())
        .and_then(|bytes| bytes.try_into().ok())
}

fn validate_sweep_accounting(
    amounts: &[Amount],
    fees: &[Amount],
    expected_principal: u64,
) -> Result<(u64, u64), MoneroWalletEffectError> {
    if amounts.len() != 1 || fees.len() != 1 {
        return Err(MoneroWalletEffectError::InvalidSweepAccountingCount);
    }
    let amount = amounts[0].as_pico();
    let fee = fees[0].as_pico();
    if amount == 0 || fee == 0 || amount.checked_add(fee) != Some(expected_principal) {
        return Err(MoneroWalletEffectError::SweepAccountingMismatch);
    }
    Ok((amount, fee))
}

fn validate_standard_address(address: &Address) -> Result<(), MoneroWalletEffectError> {
    if address.addr_type != AddressType::Standard {
        return Err(MoneroWalletEffectError::NonStandardAddress);
    }
    Ok(())
}

fn parse_shared_standard_address(
    address: &MoneroSharedAddressV1,
) -> Result<Address, MoneroWalletEffectError> {
    let parsed = address
        .address_string()
        .parse()
        .map_err(|_| MoneroWalletEffectError::InvalidSharedAddress)?;
    validate_standard_address(&parsed)?;
    Ok(parsed)
}

fn validate_shared_view_key(
    expected_public_view_key: [u8; 32],
    private_view_key: &MoneroPrivateViewKey,
) -> Result<(), MoneroWalletEffectError> {
    if private_view_key.public_key() != expected_public_view_key {
        return Err(MoneroWalletEffectError::PrivateViewKeyMismatch);
    }
    Ok(())
}

fn validate_wallet_filename(filename: &str) -> Result<(), MoneroWalletEffectError> {
    if filename.is_empty()
        || filename.len() > MAX_WALLET_FILENAME_BYTES
        || filename.starts_with('.')
        || !filename
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(MoneroWalletEffectError::InvalidWalletFilename);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use monero_rpc::monero::{Network, PublicKey};

    fn address(address_type: AddressType) -> Address {
        let mut spend = [0_u8; 32];
        spend[0] = 1;
        let mut view = [0_u8; 32];
        view[0] = 2;
        let spend = PrivateKey::from_slice(&spend).expect("canonical private spend scalar");
        let view = PrivateKey::from_slice(&view).expect("canonical private view scalar");
        Address {
            network: Network::Mainnet,
            addr_type: address_type,
            public_spend: PublicKey::from_private_key(&spend),
            public_view: PublicKey::from_private_key(&view),
        }
    }

    #[test]
    fn primary_wallet_address_must_be_standard() {
        assert_eq!(
            validate_standard_address(&address(AddressType::Standard)),
            Ok(())
        );
        assert_eq!(
            validate_standard_address(&address(AddressType::SubAddress)),
            Err(MoneroWalletEffectError::NonStandardAddress)
        );
    }

    #[test]
    fn wallet_filename_is_one_safe_component() {
        assert!(validate_wallet_filename("m4_claim_0123").is_ok());
        for invalid in ["", ".hidden", "../escape", "has/slash", "has space"] {
            assert_eq!(
                validate_wallet_filename(invalid),
                Err(MoneroWalletEffectError::InvalidWalletFilename)
            );
        }
        assert_eq!(
            validate_wallet_filename(&"a".repeat(MAX_WALLET_FILENAME_BYTES + 1)),
            Err(MoneroWalletEffectError::InvalidWalletFilename)
        );
    }

    #[test]
    fn shared_private_view_key_must_match_stage_a_public_key() {
        let mut first_bytes = [0_u8; 32];
        first_bytes[0] = 1;
        let first = MoneroPrivateViewKey::from_monero_little_endian(first_bytes)
            .expect("canonical first private view key");
        let expected_public = first.public_key();
        assert_eq!(validate_shared_view_key(expected_public, &first), Ok(()));

        let mut second_bytes = [0_u8; 32];
        second_bytes[0] = 2;
        let second = MoneroPrivateViewKey::from_monero_little_endian(second_bytes)
            .expect("canonical second private view key");
        assert_eq!(
            validate_shared_view_key(expected_public, &second),
            Err(MoneroWalletEffectError::PrivateViewKeyMismatch)
        );
    }

    #[test]
    fn shared_wallet_password_uses_bounded_printable_credential_policy() {
        assert!(valid_credential("view-only:password", true));
        for invalid in ["", "contains space", "contains\nnewline"] {
            assert!(!valid_credential(invalid, true));
        }
        assert!(!valid_credential(
            &"x".repeat(super::super::MAX_CREDENTIAL_BYTES + 1),
            true
        ));
    }

    #[test]
    fn sweep_accounting_requires_one_exact_principal_partition() {
        let amount = Amount::from_pico(999_000);
        let fee = Amount::from_pico(1_000);
        assert_eq!(
            validate_sweep_accounting(&[amount], &[fee], 1_000_000),
            Ok((999_000, 1_000))
        );
    }

    #[test]
    fn submitted_sweep_is_explicitly_nonfinal_and_preserves_exact_accounting() {
        let transaction_id = [7_u8; 32].into();
        let submitted = SubmittedMoneroSweep {
            transaction_id,
            funded_amount_piconero: 1_000_000,
            received_amount_piconero: 999_000,
            fee_piconero: 1_000,
        };
        assert_eq!(submitted.transaction_id(), transaction_id);
        assert_eq!(submitted.funded_amount_piconero(), 1_000_000);
        assert_eq!(submitted.received_amount_piconero(), 999_000);
        assert_eq!(submitted.fee_piconero(), 1_000);
    }

    #[test]
    fn sweep_accounting_rejects_missing_split_and_mismatched_values() {
        let amount = Amount::from_pico(999_000);
        let fee = Amount::from_pico(1_000);
        assert_eq!(
            validate_sweep_accounting(&[], &[fee], 1_000_000),
            Err(MoneroWalletEffectError::InvalidSweepAccountingCount)
        );
        assert_eq!(
            validate_sweep_accounting(&[amount, amount], &[fee, fee], 1_000_000),
            Err(MoneroWalletEffectError::InvalidSweepAccountingCount)
        );
        assert_eq!(
            validate_sweep_accounting(&[amount], &[fee], 2_000_000),
            Err(MoneroWalletEffectError::SweepAccountingMismatch)
        );
        assert_eq!(
            validate_sweep_accounting(
                &[Amount::from_pico(u64::MAX)],
                &[Amount::from_pico(1)],
                u64::MAX
            ),
            Err(MoneroWalletEffectError::SweepAccountingMismatch)
        );
    }

    /// How a fake origin fails a request of its failing method.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Failure {
        /// Monero's HTML 401 page.
        Unauthorized,
        /// The connection closes after the request, with no answer.
        Lost,
    }

    /// One fake JSON-RPC origin: answers each method through `answer` and
    /// records every request body in arrival order.
    struct FakeRpc {
        endpoint: LoopbackRpcEndpoint,
        requests: std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
        task: tokio::task::JoinHandle<()>,
    }

    impl Drop for FakeRpc {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    impl FakeRpc {
        async fn spawn(answer: fn(&str) -> serde_json::Value) -> Self {
            Self::spawn_slow(answer, "", std::time::Duration::ZERO).await
        }

        /// As [`Self::spawn`], answering `slow_method` only after `delay`.
        async fn spawn_slow(
            answer: fn(&str) -> serde_json::Value,
            slow_method: &'static str,
            delay: std::time::Duration,
        ) -> Self {
            Self::spawn_with(answer, slow_method, delay, "", 0, Failure::Unauthorized).await
        }

        /// As [`Self::spawn`], answering the first `failures` requests of
        /// `failing_method` with monero's HTML 401 page: what the answering
        /// leg of a Digest call sent on another connection than its challenge
        /// gets, and the client decodes as the JSON answer. (Sent without a
        /// challenge, so the client takes the first leg's answer as final.)
        async fn spawn_failing(
            answer: fn(&str) -> serde_json::Value,
            failing_method: &'static str,
            failures: usize,
        ) -> Self {
            Self::spawn_with(
                answer,
                "",
                std::time::Duration::ZERO,
                failing_method,
                failures,
                Failure::Unauthorized,
            )
            .await
        }

        /// As [`Self::spawn`], closing the connection of the first
        /// `failures` requests of `failing_method` once it has read them,
        /// with no answer: a request whose outcome the client cannot know.
        async fn spawn_losing(
            answer: fn(&str) -> serde_json::Value,
            failing_method: &'static str,
            failures: usize,
        ) -> Self {
            Self::spawn_with(
                answer,
                "",
                std::time::Duration::ZERO,
                failing_method,
                failures,
                Failure::Lost,
            )
            .await
        }

        async fn spawn_with(
            answer: fn(&str) -> serde_json::Value,
            slow_method: &'static str,
            delay: std::time::Duration,
            failing_method: &'static str,
            mut failures: usize,
            failure: Failure,
        ) -> Self {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};

            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind fake RPC");
            let address = listener.local_addr().expect("fake RPC address");
            let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let recorded = std::sync::Arc::clone(&requests);
            let task = tokio::spawn(async move {
                while let Ok((mut stream, _peer)) = listener.accept().await {
                    let mut bytes = Vec::new();
                    let mut buffer = [0_u8; 4_096];
                    let body = loop {
                        let Ok(count) = stream.read(&mut buffer).await else {
                            break None;
                        };
                        if count == 0 {
                            break None;
                        }
                        bytes.extend_from_slice(&buffer[..count]);
                        let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") else {
                            continue;
                        };
                        let length = String::from_utf8_lossy(&bytes[..end])
                            .lines()
                            .find_map(|line| {
                                let (name, value) = line.split_once(':')?;
                                name.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().ok())?
                            })
                            .unwrap_or(0);
                        if bytes.len() >= end + 4 + length {
                            break Some(bytes[end + 4..end + 4 + length].to_vec());
                        }
                    };
                    let Some(body) = body else { continue };
                    let request: serde_json::Value =
                        serde_json::from_slice(&body).expect("JSON-RPC request");
                    let method = request["method"].as_str().unwrap_or_default().to_owned();
                    recorded.lock().expect("requests").push(request);
                    if method == slow_method {
                        tokio::time::sleep(delay).await;
                    }
                    if method == failing_method && failures > 0 && failure == Failure::Lost {
                        failures -= 1;
                        let _ = stream.shutdown().await;
                        continue;
                    }
                    if method == failing_method && failures > 0 {
                        failures -= 1;
                        let page = "<html><head><title>Unauthorized Access</title></head><body><h1>401 Unauthorized</h1></body></html>";
                        let reply = format!(
                            "HTTP/1.1 401 Unauthorized\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
                            page.len()
                        );
                        let _ = stream.write_all(reply.as_bytes()).await;
                        let _ = stream.shutdown().await;
                        continue;
                    }
                    let mut response = answer(&method);
                    response["jsonrpc"] = "2.0".into();
                    response["id"] = "0".into();
                    let response = response.to_string();
                    let reply = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
                        response.len()
                    );
                    let _ = stream.write_all(reply.as_bytes()).await;
                    let _ = stream.shutdown().await;
                }
            });
            let endpoint =
                LoopbackRpcEndpoint::new(&format!("http://{address}"), "rpc-user", "rpc-secret")
                    .expect("fake RPC endpoint");
            Self {
                endpoint,
                requests,
                task,
            }
        }

        fn methods(&self) -> Vec<String> {
            self.requests
                .lock()
                .expect("requests")
                .iter()
                .map(|request| request["method"].as_str().unwrap_or_default().to_owned())
                .collect()
        }

        fn request(&self, index: usize) -> serde_json::Value {
            self.requests.lock().expect("requests")[index].clone()
        }
    }

    fn wallet_answer(method: &str) -> serde_json::Value {
        match method {
            "open_wallet" => serde_json::json!({ "result": {} }),
            "get_address" => serde_json::json!({ "result": {
                "address": address(AddressType::Standard).to_string(),
                "addresses": [],
            }}),
            "get_balance" => serde_json::json!({ "result": {
                "balance": 7_000, "unlocked_balance": 5_000, "multisig_import_needed": false,
            }}),
            "transfer" => serde_json::json!({ "result": {
                "amount": 5_000, "fee": 10, "tx_blob": "", "tx_hash": "07".repeat(32),
                "tx_key": "", "tx_metadata": "", "unsigned_txset": "",
            }}),
            "get_transfers" => {
                let pool = |amount: u64, txid: &str| {
                    serde_json::json!({
                        "address": address(AddressType::Standard).to_string(), "amount": amount,
                        "confirmations": 0, "double_spend_seen": false, "fee": 0, "height": 0,
                        "note": "", "destinations": [], "payment_id": "0000000000000000",
                        "subaddr_index": {"major": 0, "minor": 0},
                        "suggested_confirmations_threshold": 10, "timestamp": 1_700_000_000,
                        "txid": txid.repeat(32), "type": "pool", "unlock_time": 0,
                    })
                };
                serde_json::json!({ "result": { "pool": [pool(4_000, "06"), pool(5_000, "07")] } })
            }
            _ => serde_json::json!({ "error": { "code": -1, "message": "unexpected" } }),
        }
    }

    fn wallet_without_role_wallet_file(method: &str) -> serde_json::Value {
        if method == "open_wallet" {
            return serde_json::json!({ "error": { "code": -1, "message": "Failed to open wallet" } });
        }
        wallet_answer(method)
    }

    fn wallet_without_balance(method: &str) -> serde_json::Value {
        if method == "get_balance" {
            return serde_json::json!({ "error": { "code": -1, "message": "wallet is busy" } });
        }
        wallet_answer(method)
    }

    fn generated_blocks(count: usize) -> serde_json::Value {
        serde_json::json!({ "result": {
            "status": "OK", "height": 120, "blocks": vec!["01".repeat(32); count],
        }})
    }

    fn daemon_answer(method: &str) -> serde_json::Value {
        match method {
            "generateblocks" => generated_blocks(10),
            "get_block_count" => serde_json::json!({ "result": { "status": "OK", "count": 121 } }),
            _ => serde_json::json!({ "error": { "code": -1, "message": "unexpected" } }),
        }
    }

    fn daemon_short_of_blocks(_method: &str) -> serde_json::Value {
        generated_blocks(9)
    }

    fn effects(daemon: &FakeRpc, wallet: &FakeRpc) -> MoneroRegtestWalletEffects {
        MoneroRegtestWalletEffects::new(&daemon.endpoint, &wallet.endpoint).expect("effects")
    }

    const SWAP_PICONERO: u64 = 1_000_000;

    fn incoming(amount: u64, height: Option<u64>, image: &str, index: u64) -> serde_json::Value {
        serde_json::json!({
            "amount": amount, "global_index": index, "key_image": image.repeat(32),
            "spent": false, "subaddr_index": {"major": 0, "minor": 0},
            "tx_hash": format!("{index:064x}"), "tx_size": 1_500, "block_height": height,
        })
    }

    /// The shared wallet as anyone who knows its address can leave it after
    /// the swap's output was verified: dust, a larger gift, a second output
    /// of the swap's amount, and an output not confirmed yet.
    fn shared_wallet_with_extra_outputs(method: &str) -> serde_json::Value {
        match method {
            "refresh" => {
                serde_json::json!({ "result": { "blocks_fetched": 0, "received_money": false } })
            }
            "incoming_transfers" => serde_json::json!({ "result": { "transfers": [
                incoming(1, Some(130), "bb", 2),
                incoming(SWAP_PICONERO, Some(140), "cc", 3),
                incoming(SWAP_PICONERO, None, "ee", 5),
                incoming(SWAP_PICONERO, Some(100), "aa", 1),
                incoming(5 * SWAP_PICONERO, Some(150), "dd", 4),
            ]}}),
            "get_balance" => serde_json::json!({ "result": {
                "balance": 8 * SWAP_PICONERO + 1, "unlocked_balance": 2 * SWAP_PICONERO + 1,
                "multisig_import_needed": false,
            }}),
            "sweep_single" => serde_json::json!({ "result": {
                "tx_hash": "07".repeat(32), "amount": SWAP_PICONERO - 1_000, "fee": 1_000,
            }}),
            _ => serde_json::json!({ "error": { "code": -1, "message": "unexpected" } }),
        }
    }

    fn shared_wallet_with_only_dust(method: &str) -> serde_json::Value {
        if method == "incoming_transfers" {
            return serde_json::json!({ "result": { "transfers": [
                incoming(1, Some(130), "bb", 2),
                incoming(SWAP_PICONERO, None, "ee", 5),
            ]}});
        }
        shared_wallet_with_extra_outputs(method)
    }

    /// One piconero sent to the shared address after verification used to
    /// make the exact-balance check fail on every sweep. Extra outputs,
    /// confirmed or not, are now left in place: the sweep spends the swap's
    /// verified output alone, with one input's fee.
    #[tokio::test]
    async fn extra_outputs_at_the_shared_address_do_not_block_the_sweep() {
        let daemon = FakeRpc::spawn(daemon_answer).await;
        let wallet = FakeRpc::spawn(shared_wallet_with_extra_outputs).await;
        let swept = effects(&daemon, &wallet)
            .sweep_exact_output(
                0,
                NonZeroU64::new(SWAP_PICONERO).unwrap(),
                address(AddressType::Standard),
            )
            .await
            .expect("the swap's output is swept");
        assert_eq!(swept.transaction_id(), [7_u8; 32].into());
        assert_eq!(swept.received_amount_piconero(), SWAP_PICONERO - 1_000);
        assert_eq!(swept.fee_piconero(), 1_000);
        assert_eq!(
            wallet.methods(),
            ["refresh", "incoming_transfers", "sweep_single"]
        );
        let sweep = wallet.request(2);
        assert_eq!(sweep["params"]["key_image"], "aa".repeat(32));
        assert_eq!(
            sweep["params"]["address"],
            address(AddressType::Standard).to_string()
        );
    }

    fn wallet_error() -> serde_json::Value {
        serde_json::json!({ "error": { "code": -1, "message": "failing on purpose" } })
    }

    fn shared_wallet_failing_outputs(method: &str) -> serde_json::Value {
        match method {
            "incoming_transfers" => wallet_error(),
            other => shared_wallet_with_extra_outputs(other),
        }
    }

    fn shared_wallet_failing_sweep(method: &str) -> serde_json::Value {
        match method {
            "sweep_single" => wallet_error(),
            other => shared_wallet_with_extra_outputs(other),
        }
    }

    /// The reopen path of a sweep driven again: the wallet file exists, so
    /// `generate_from_keys` fails, `open_wallet` succeeds and the address read
    /// fails.
    fn shared_wallet_failing_reopened_address(method: &str) -> serde_json::Value {
        match method {
            "open_wallet" => serde_json::json!({ "result": {} }),
            _ => wallet_error(),
        }
    }

    /// A failure before `sweep_single` proves the sweep was not sent; one of
    /// the sweep call itself does not. The Taker supervisor relies on this
    /// (a sweep not sent is retried, one in doubt is not), so each operation
    /// a sweep runs before it sends is named.
    #[tokio::test]
    async fn only_a_failure_before_the_sweep_call_proves_nothing_was_sent() {
        let daemon = FakeRpc::spawn(daemon_answer).await;
        let amount = NonZeroU64::new(SWAP_PICONERO).unwrap();
        let outputs = FakeRpc::spawn(shared_wallet_failing_outputs).await;
        let failed = effects(&daemon, &outputs)
            .sweep_exact_output(0, amount, address(AddressType::Standard))
            .await
            .map(|_| ())
            .unwrap_err();
        assert!(
            matches!(&failed, MoneroWalletEffectError::Rpc { operation, .. } if *operation == OP_READ_RESTORED_OUTPUTS)
        );
        assert!(failed.sweep_not_sent());

        let sweep = FakeRpc::spawn(shared_wallet_failing_sweep).await;
        let failed = effects(&daemon, &sweep)
            .sweep_exact_output(0, amount, address(AddressType::Standard))
            .await
            .map(|_| ())
            .unwrap_err();
        assert!(
            matches!(&failed, MoneroWalletEffectError::Rpc { operation, .. } if *operation == "sweep reconstructed wallet")
        );
        assert!(!failed.sweep_not_sent());

        let reopened = FakeRpc::spawn(shared_wallet_failing_reopened_address).await;
        let key = || PrivateKey::from_slice(&[1_u8; 32]).expect("scalar");
        let failed = effects(&daemon, &reopened)
            .generate_or_reopen_shared(GenerateFromKeysArgs {
                restore_height: Some(0),
                filename: "shared-swap".to_owned(),
                address: address(AddressType::Standard),
                spendkey: Some(key()),
                viewkey: key(),
                password: "password".to_owned(),
                autosave_current: Some(true),
            })
            .await
            .unwrap_err();
        assert!(
            matches!(&failed, MoneroWalletEffectError::Rpc { operation, .. } if *operation == OP_RESTORED_PRIMARY_ADDRESS)
        );
        assert!(failed.sweep_not_sent());

        for operation in [
            OP_OPEN_ROLE_WALLET,
            OP_CLOSE_CURRENT_WALLET,
            OP_GENERATE_SHARED_FROM_KEYS,
            OP_RESTORED_PRIMARY_ADDRESS,
            OP_REFRESH_RESTORED_WALLET,
            OP_READ_RESTORED_OUTPUTS,
        ] {
            assert!(rpc_error(operation)(anyhow::anyhow!("down")).sweep_not_sent());
        }
        for operation in ["sweep reconstructed wallet", "generate confirmation blocks"] {
            assert!(!rpc_error(operation)(anyhow::anyhow!("down")).sweep_not_sent());
        }
        assert!(!MoneroWalletEffectError::InvalidSweepTransactionCount.sweep_not_sent());
    }

    #[tokio::test]
    async fn no_confirmed_output_of_the_swap_amount_sends_nothing() {
        let daemon = FakeRpc::spawn(daemon_answer).await;
        let wallet = FakeRpc::spawn(shared_wallet_with_only_dust).await;
        let refused = effects(&daemon, &wallet)
            .sweep_exact_output(
                0,
                NonZeroU64::new(SWAP_PICONERO).unwrap(),
                address(AddressType::Standard),
            )
            .await;
        assert_eq!(
            refused.map(|swept| swept.fee_piconero()),
            Err(MoneroWalletEffectError::RestoredBalanceMismatch)
        );
        assert_eq!(wallet.methods(), ["refresh", "incoming_transfers"]);
    }

    #[tokio::test]
    async fn every_role_wallet_effect_reopens_the_role_wallet_first() {
        let daemon = FakeRpc::spawn(daemon_answer).await;
        let wallet = FakeRpc::spawn(wallet_answer).await;
        let effects = effects(&daemon, &wallet)
            .with_role_wallet("maker".to_owned(), String::new())
            .expect("role wallet");

        assert_eq!(
            effects.primary_standard_address().await,
            Ok(address(AddressType::Standard))
        );
        let funding = effects
            .fund_exact_and_confirm(address(AddressType::Standard), 5_000)
            .await
            .expect("funding");
        assert_eq!(funding.amount_piconero(), 5_000);
        assert_eq!(funding.confirmation_tip_height(), 120);

        assert_eq!(
            wallet.methods(),
            [
                "open_wallet",
                "get_address",
                "open_wallet",
                "get_address",
                "transfer"
            ]
        );
        assert_eq!(
            wallet.request(0)["params"],
            serde_json::json!({ "filename": "maker", "password": "" })
        );
        assert_eq!(daemon.methods(), ["generateblocks"]);
    }

    #[tokio::test]
    async fn shared_funding_once_submits_one_transfer_and_never_mines() {
        let daemon = FakeRpc::spawn(daemon_answer).await;
        let wallet = FakeRpc::spawn(wallet_answer).await;
        let effects = effects(&daemon, &wallet)
            .with_role_wallet("maker".to_owned(), String::new())
            .expect("role wallet");
        let prove = || {
            let scalar = lez_xmr_swap_sdk::CrossCurveScalar::generate().expect("entropy");
            lez_xmr_swap_sdk::CrossCurveDleqProofV1::prove(&scalar, &mut rand_core::OsRng)
                .expect("DLEQ proof")
        };
        let view = MoneroPrivateViewKey::generate().expect("entropy");
        let shared = MoneroSharedAddressV1::derive(
            lez_xmr_swap_sdk::MoneroAddressNetworkV1::Stagenet,
            &prove(),
            &prove(),
            &view,
        )
        .expect("Stagenet shared address");

        assert_eq!(
            effects.fund_shared_exact_once(&shared, 0).await,
            Err(MoneroWalletEffectError::ZeroAmount)
        );
        let transaction = effects
            .fund_shared_exact_once(&shared, 5_000)
            .await
            .expect("submitted");
        assert_eq!(transaction, MoneroTransactionId::from([7; 32]));
        assert_eq!(wallet.methods(), ["open_wallet", "transfer"]);
        assert_eq!(
            wallet.request(1)["params"]["destinations"][0]["address"],
            serde_json::json!(shared.address_string())
        );
        assert!(daemon.methods().is_empty());
    }

    #[tokio::test]
    async fn without_a_role_wallet_effects_use_the_open_wallet() {
        let daemon = FakeRpc::spawn(daemon_answer).await;
        let wallet = FakeRpc::spawn(wallet_answer).await;
        let effects = effects(&daemon, &wallet);

        effects.open_role_wallet().await.expect("no-op open");
        effects
            .primary_standard_address()
            .await
            .expect("open wallet address");
        assert_eq!(wallet.methods(), ["get_address"]);
    }

    #[tokio::test]
    async fn a_role_wallet_that_fails_to_open_stops_the_effect() {
        let daemon = FakeRpc::spawn(daemon_answer).await;
        let wallet = FakeRpc::spawn(wallet_without_role_wallet_file).await;
        let effects = effects(&daemon, &wallet)
            .with_role_wallet("maker".to_owned(), String::new())
            .expect("role wallet");

        assert_eq!(
            effects
                .fund_exact_and_confirm(address(AddressType::Standard), 5_000)
                .await,
            Err(MoneroWalletEffectError::Rpc {
                operation: "open role wallet",
                cause: "Server error: Failed to open wallet".to_owned(),
            })
        );
        assert_eq!(wallet.methods(), ["open_wallet"]);
        assert!(daemon.methods().is_empty());
    }

    #[tokio::test]
    async fn a_slow_role_wallet_open_still_reads_the_balance() {
        // The devnet's wallet-rpc took 4.3 s to open a role wallet under load.
        let daemon = FakeRpc::spawn(daemon_answer).await;
        let wallet = FakeRpc::spawn_slow(
            wallet_answer,
            "open_wallet",
            std::time::Duration::from_secs(5),
        )
        .await;
        let effects = effects(&daemon, &wallet)
            .with_role_wallet("taker".to_owned(), String::new())
            .expect("role wallet");

        assert_eq!(
            effects.role_wallet_balance().await,
            Ok((address(AddressType::Standard), 7_000, 5_000))
        );
        assert_eq!(
            wallet.methods(),
            ["open_wallet", "get_address", "get_balance"]
        );
    }

    #[tokio::test]
    async fn a_failed_balance_read_names_its_cause() {
        let daemon = FakeRpc::spawn(daemon_answer).await;
        let wallet = FakeRpc::spawn(wallet_without_balance).await;
        let effects = effects(&daemon, &wallet)
            .with_role_wallet("taker".to_owned(), String::new())
            .expect("role wallet");

        let error = effects.role_wallet_balance().await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "typed Monero wallet-effect operation `role wallet balance` failed: \
             Server error: wallet is busy"
        );
        // The login travels in a header: no credential reaches the text.
        assert!(!error.to_string().contains("rpc-secret"));
    }

    #[tokio::test]
    async fn role_wallet_name_and_password_are_checked_before_any_rpc() {
        let daemon = FakeRpc::spawn(daemon_answer).await;
        let wallet = FakeRpc::spawn(wallet_answer).await;
        for (filename, password, error) in [
            (
                "../maker",
                "",
                MoneroWalletEffectError::InvalidWalletFilename,
            ),
            (
                "maker",
                "has space",
                MoneroWalletEffectError::InvalidWalletPassword,
            ),
        ] {
            assert_eq!(
                effects(&daemon, &wallet)
                    .with_role_wallet(filename.to_owned(), password.to_owned())
                    .map(|_| ())
                    .unwrap_err(),
                error
            );
        }
        assert!(
            effects(&daemon, &wallet)
                .with_role_wallet("maker".to_owned(), "role:secret".to_owned())
                .is_ok()
        );
        assert!(wallet.methods().is_empty());
    }

    #[tokio::test]
    async fn confirmations_are_mined_with_the_fixed_block_count() {
        let wallet = FakeRpc::spawn(wallet_answer).await;
        let daemon = FakeRpc::spawn(daemon_answer).await;
        let effects = effects(&daemon, &wallet);
        assert_eq!(
            effects
                .mine_confirmations(address(AddressType::SubAddress))
                .await,
            Err(MoneroWalletEffectError::NonStandardAddress)
        );
        assert!(daemon.methods().is_empty());
        assert_eq!(
            effects
                .mine_confirmations(address(AddressType::Standard))
                .await,
            Ok(120)
        );
        assert_eq!(
            daemon.request(0)["params"]["amount_of_blocks"],
            REQUIRED_MONERO_CONFIRMATIONS
        );

        let short = FakeRpc::spawn(daemon_short_of_blocks).await;
        assert_eq!(
            MoneroRegtestWalletEffects::new(&short.endpoint, &wallet.endpoint)
                .expect("effects")
                .mine_confirmations(address(AddressType::Standard))
                .await,
            Err(MoneroWalletEffectError::ConfirmationBlockCountMismatch)
        );
    }

    #[tokio::test]
    async fn a_failed_mining_call_is_made_again_a_bounded_number_of_times() {
        let wallet = FakeRpc::spawn(wallet_answer).await;
        let mine = async |daemon: &FakeRpc| {
            effects(daemon, &wallet)
                .mine_confirmations(address(AddressType::Standard))
                .await
        };

        let flaky = FakeRpc::spawn_failing(daemon_answer, "generateblocks", 2).await;
        assert_eq!(mine(&flaky).await, Ok(120));
        assert_eq!(flaky.methods(), ["generateblocks"; 3]);

        let down = FakeRpc::spawn_failing(daemon_answer, "generateblocks", 3).await;
        let Err(MoneroWalletEffectError::Rpc { operation, cause }) = mine(&down).await else {
            panic!("three failed mining calls fail the effect");
        };
        assert_eq!(operation, "generate confirmation blocks");
        // The 2026-11-15 failure, word for word.
        assert!(
            cause.contains("expected value at line 1 column 1"),
            "{cause}"
        );
        assert_eq!(down.methods().len(), 3);

        // The block count is checked on the call that succeeded.
        let short = FakeRpc::spawn_failing(daemon_short_of_blocks, "generateblocks", 1).await;
        assert_eq!(
            mine(&short).await,
            Err(MoneroWalletEffectError::ConfirmationBlockCountMismatch)
        );
        assert_eq!(short.methods().len(), 2);
    }

    #[tokio::test]
    async fn an_exact_incoming_pool_transfer_is_found_and_awaits_its_confirmations() {
        let daemon = FakeRpc::spawn(daemon_answer).await;
        let wallet = FakeRpc::spawn(wallet_answer).await;
        let verifier = crate::MoneroOutputVerifier::new(
            crate::MoneroChainIdentity::new(crate::MoneroNetwork::Regtest, [1; 32])
                .expect("identity"),
            &daemon.endpoint,
            &wallet.endpoint,
        )
        .expect("verifier");
        assert_eq!(
            verifier.find_incoming_in_pool(5_000).await.expect("pool"),
            Some(MoneroTransactionId([7; 32]))
        );
        assert_eq!(
            verifier.find_incoming_in_pool(6_000).await.expect("pool"),
            None
        );
        assert_eq!(wallet.methods(), ["get_transfers"; 2]);
        assert_eq!(wallet.request(0)["params"]["pool"], true);

        // In the pool or short of its depth: mining is what it waits for.
        for seen in [
            crate::MoneroEvidenceError::WalletTransferInPool,
            crate::MoneroEvidenceError::DaemonTransactionInPool,
            crate::MoneroEvidenceError::NonzeroUnlockDistance,
            crate::MoneroEvidenceError::OutputNotUnlocked,
            crate::MoneroEvidenceError::InsufficientConfirmations,
        ] {
            assert!(seen.awaits_confirmations(), "{seen}");
        }
        // Not seen, or seen wrong: mining is not the answer.
        for other in [
            crate::MoneroEvidenceError::MissingWalletTransfer,
            crate::MoneroEvidenceError::DaemonMissedTransaction,
            crate::MoneroEvidenceError::AmountMismatch,
            crate::MoneroEvidenceError::DoubleSpendSeen,
        ] {
            assert!(!other.awaits_confirmations(), "{other}");
        }
    }

    #[tokio::test]
    async fn a_read_failed_in_transport_is_made_again_a_bounded_number_of_times() {
        let daemon = FakeRpc::spawn(daemon_answer).await;
        let role = |wallet: &FakeRpc| {
            effects(&daemon, wallet)
                .with_role_wallet("taker".to_owned(), String::new())
                .expect("role wallet")
        };

        let flaky = FakeRpc::spawn_failing(wallet_answer, "get_balance", 2).await;
        assert_eq!(
            role(&flaky).role_wallet_balance().await,
            Ok((address(AddressType::Standard), 7_000, 5_000))
        );
        assert_eq!(
            flaky.methods(),
            [
                "open_wallet",
                "get_address",
                "get_balance",
                "get_balance",
                "get_balance"
            ]
        );

        // READ_ATTEMPTS calls, no more.
        let down = FakeRpc::spawn_failing(wallet_answer, "open_wallet", 3).await;
        let Err(MoneroWalletEffectError::Rpc { operation, .. }) =
            role(&down).role_wallet_balance().await
        else {
            panic!("a read failed every time fails the effect");
        };
        assert_eq!(operation, "open role wallet");
        assert_eq!(
            down.methods(),
            ["open_wallet"; crate::READ_ATTEMPTS as usize]
        );

        // An error the wallet-rpc answered is its answer.
        let busy = FakeRpc::spawn(wallet_without_balance).await;
        assert!(role(&busy).role_wallet_balance().await.is_err());
        assert_eq!(
            busy.methods(),
            ["open_wallet", "get_address", "get_balance"]
        );

        // The verifier's and the attestor's daemon reads take the same path.
        let flaky_daemon = FakeRpc::spawn_failing(daemon_answer, "get_block_count", 2).await;
        let attestor = crate::MoneroChainIdentityAttestor::new(
            crate::MoneroNetwork::Regtest,
            &flaky_daemon.endpoint,
        )
        .expect("attestor");
        assert_eq!(attestor.tip_height().await.expect("tip height"), 120);
        assert_eq!(flaky_daemon.methods(), ["get_block_count"; 3]);
    }

    /// The funding wallet after a transfer to the test destination went out:
    /// its outgoing transfers hold it with its destination and amount.
    fn wallet_that_sent_the_transfer(method: &str) -> serde_json::Value {
        if method == "get_transfers" {
            let out = |txid: &str, destination: String, amount: u64| {
                serde_json::json!({
                    "txid": txid.repeat(32), "amount": amount, "fee": 10, "type": "out",
                    "destinations": [{ "address": destination, "amount": amount }],
                })
            };
            let other = address(AddressType::SubAddress).to_string();
            return serde_json::json!({ "result": {
                "out": [out("05", other, 5_000), out("04", address(AddressType::Standard).to_string(), 4_000)],
                "pending": [out("07", address(AddressType::Standard).to_string(), 5_000)],
            }});
        }
        wallet_answer(method)
    }

    /// The funding wallet with no transfer to the test destination.
    fn wallet_that_sent_nothing(method: &str) -> serde_json::Value {
        if method == "get_transfers" {
            return serde_json::json!({ "result": {} });
        }
        wallet_answer(method)
    }

    fn wallet_whose_lookup_fails(method: &str) -> serde_json::Value {
        if method == "get_transfers" {
            return serde_json::json!({ "error": { "code": -1, "message": "wallet is busy" } });
        }
        wallet_answer(method)
    }

    /// 2026-11-21 on the devnet: the wallet-rpc refused the transfer's
    /// Digest answer with its HTML 401 page (the answer went on another
    /// pooled connection than its challenge), the typed client decoded the
    /// page as the answer, and the Maker took a transfer that was never made
    /// for one in doubt. A 401 proves the wallet-rpc ran nothing: the
    /// exchange is made again, and the transfer goes out once.
    #[tokio::test]
    async fn a_transfer_refused_with_the_401_page_is_made_again_and_sent_once() {
        let daemon = FakeRpc::spawn(daemon_answer).await;
        let wallet = FakeRpc::spawn_failing(wallet_answer, "transfer", 2).await;
        let funding = effects(&daemon, &wallet)
            .with_role_wallet("maker".to_owned(), String::new())
            .expect("role wallet")
            .fund_exact_and_confirm(address(AddressType::Standard), 5_000)
            .await
            .expect("the third exchange is answered");
        assert_eq!(funding.transaction_id(), MoneroTransactionId::from([7; 32]));
        assert_eq!(
            wallet.methods(),
            [
                "open_wallet",
                "get_address",
                "transfer",
                "transfer",
                "transfer"
            ]
        );
        assert_eq!(daemon.methods(), ["generateblocks"]);

        // Refused every time: nothing was sent, and the error says so.
        let refused = FakeRpc::spawn_failing(wallet_answer, "transfer", 3).await;
        let failed = effects(&daemon, &refused)
            .fund_exact_and_confirm(address(AddressType::Standard), 5_000)
            .await
            .map(|_| ())
            .unwrap_err();
        assert!(
            matches!(&failed, MoneroWalletEffectError::RequestNotSent { operation, cause }
                if *operation == OP_FUND_EXACT_TRANSFER && cause.contains("HTTP 401")),
            "{failed}"
        );
        assert!(failed.funding_not_sent());
        assert!(!failed.sweep_not_sent());
        assert_eq!(
            refused.methods(),
            ["get_address", "transfer", "transfer", "transfer"]
        );
    }

    /// A transfer whose answer is lost may have gone out. It is looked up by
    /// destination and amount, never sent again: found, it is the funding;
    /// not found, nothing was sent; a failed lookup leaves it in doubt.
    #[tokio::test]
    async fn a_transfer_whose_answer_is_lost_is_looked_up_not_sent_again() {
        let daemon = FakeRpc::spawn(daemon_answer).await;

        let sent = FakeRpc::spawn_losing(wallet_that_sent_the_transfer, "transfer", 1).await;
        let funding = effects(&daemon, &sent)
            .fund_exact_and_confirm(address(AddressType::Standard), 5_000)
            .await
            .expect("the lookup finds the transfer");
        assert_eq!(funding.transaction_id(), MoneroTransactionId::from([7; 32]));
        assert_eq!(sent.methods(), ["get_address", "transfer", "get_transfers"]);
        assert_eq!(
            sent.request(2)["params"],
            serde_json::json!({
                "in": false, "out": true, "pending": true, "failed": false, "pool": false,
                "account_index": 0,
            })
        );
        assert_eq!(daemon.methods(), ["generateblocks"]);

        let nothing = FakeRpc::spawn_losing(wallet_that_sent_nothing, "transfer", 1).await;
        let failed = effects(&daemon, &nothing)
            .fund_exact_and_confirm(address(AddressType::Standard), 5_000)
            .await
            .map(|_| ())
            .unwrap_err();
        assert!(
            matches!(&failed, MoneroWalletEffectError::RequestNotSent { operation, .. }
                if *operation == OP_FUND_EXACT_TRANSFER),
            "{failed}"
        );
        assert!(failed.funding_not_sent());
        assert_eq!(
            nothing.methods(),
            ["get_address", "transfer", "get_transfers"]
        );

        let unknown = FakeRpc::spawn_losing(wallet_whose_lookup_fails, "transfer", 1).await;
        let failed = effects(&daemon, &unknown)
            .fund_exact_and_confirm(address(AddressType::Standard), 5_000)
            .await
            .map(|_| ())
            .unwrap_err();
        assert!(
            matches!(&failed, MoneroWalletEffectError::Rpc { operation, .. }
                if *operation == OP_FUND_EXACT_TRANSFER),
            "{failed}"
        );
        assert!(!failed.funding_not_sent());
        assert_eq!(
            unknown.methods(),
            [
                "get_address",
                "transfer",
                "get_transfers",
                "get_transfers",
                "get_transfers"
            ]
        );
        // Nothing is mined after a funding that did not go out or is in doubt.
        assert_eq!(daemon.methods(), ["generateblocks"]);
    }

    #[test]
    fn only_failures_that_sent_nothing_let_a_funding_go_again() {
        for operation in [
            OP_OPEN_ROLE_WALLET,
            OP_REFRESH_CURRENT_WALLET,
            OP_FUNDING_WALLET_ADDRESS,
        ] {
            assert!(rpc_error(operation)(anyhow::anyhow!("down")).funding_not_sent());
        }
        for operation in [OP_FUND_EXACT_TRANSFER, "generate confirmation blocks"] {
            assert!(!rpc_error(operation)(anyhow::anyhow!("down")).funding_not_sent());
        }
        let not_run = |operation| raw_call_error(operation)(RawCallError::NotRun("x".into()));
        assert!(not_run(OP_FUND_EXACT_TRANSFER).funding_not_sent());
        assert!(!not_run(OP_SWEEP_RECONSTRUCTED_WALLET).funding_not_sent());
        assert!(not_run(OP_SWEEP_RECONSTRUCTED_WALLET).sweep_not_sent());
        assert!(!not_run(OP_FUND_EXACT_TRANSFER).sweep_not_sent());
        let may_have_run =
            |operation| raw_call_error(operation)(RawCallError::MayHaveRun("x".into()));
        assert!(!may_have_run(OP_FUND_EXACT_TRANSFER).funding_not_sent());
        assert!(!may_have_run(OP_SWEEP_RECONSTRUCTED_WALLET).sweep_not_sent());
        for checked in [
            MoneroWalletEffectError::NonStandardAddress,
            MoneroWalletEffectError::ZeroAmount,
            MoneroWalletEffectError::InvalidSharedAddress,
        ] {
            assert!(checked.funding_not_sent());
        }
        assert!(!MoneroWalletEffectError::FundingAmountMismatch.funding_not_sent());
        assert!(!MoneroWalletEffectError::ConfirmationBlockCountMismatch.funding_not_sent());
    }

    fn daemon_with_block(method: &str) -> serde_json::Value {
        if method == "get_block" {
            let blob = monero_rpc::monero::consensus::encode::serialize_hex(
                &monero_rpc::monero::blockdata::block::Block::default(),
            );
            return serde_json::json!({ "result": { "blob": blob, "status": "OK" } });
        }
        daemon_answer(method)
    }

    #[tokio::test]
    async fn the_verifiers_block_read_is_made_again_although_its_error_is_text() {
        // `get_block` hands back its client's decode error of the 401 page as
        // text ("Can not fetch block: error decoding response body").
        let read = |daemon: &FakeRpc| {
            let client = daemon.endpoint.client().expect("client").daemon();
            async move {
                crate::MoneroRpcPort::rpc("get_block_by_hash", || {
                    client.get_block(monero_rpc::GetBlockHeaderSelector::Last)
                })
                .await
                .map(|block| block.tx_hashes.len())
            }
        };
        let flaky = FakeRpc::spawn_failing(daemon_with_block, "get_block", 2).await;
        assert_eq!(read(&flaky).await.expect("block on the third call"), 0);
        assert_eq!(flaky.methods(), ["get_block"; 3]);

        let down = FakeRpc::spawn_failing(daemon_with_block, "get_block", 3).await;
        assert!(matches!(
            read(&down).await,
            Err(crate::MoneroEvidenceError::Rpc {
                operation: "get_block_by_hash",
                ..
            })
        ));
        assert_eq!(down.methods(), ["get_block"; 3]);
    }

    #[test]
    fn wallet_rpc_lock_is_exclusive_until_dropped() {
        let path = std::env::temp_dir().join(format!(
            "lez-xmr-wallet-rpc-lock-test-{}",
            std::process::id()
        ));
        let other = || {
            OpenOptions::new()
                .write(true)
                .open(&path)
                .expect("open lock file")
        };

        let held = MoneroWalletRpcLock::acquire(&path).expect("first lock");
        assert!(matches!(
            other().try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));

        let (sender, receiver) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn({
            let path = path.clone();
            move || {
                let lock = MoneroWalletRpcLock::acquire(&path).expect("second lock");
                sender.send(()).expect("signal");
                lock
            }
        });
        assert!(
            receiver
                .recv_timeout(std::time::Duration::from_millis(200))
                .is_err()
        );
        drop(held);
        receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("waiter gets the lock once it is released");
        drop(waiter.join().expect("waiter"));

        other().try_lock().expect("lock is free again");
        std::fs::remove_file(&path).expect("remove lock file");
    }

    #[test]
    fn wallet_rpc_lock_reports_an_unopenable_path() {
        assert_eq!(
            MoneroWalletRpcLock::acquire(Path::new("/nonexistent-lez-dir/wallet.lock"))
                .map(|_| ())
                .unwrap_err(),
            MoneroWalletEffectError::WalletRpcLock
        );
    }
}
