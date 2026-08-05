//! Narrow effect-bearing wallet boundary for the private M4 Regtest `PoC`.
//!
//! This module deliberately reuses the typed `monero-rpc` client. It neither
//! observes protocol eligibility nor authorizes release: callers must first
//! satisfy the Stage-B, finalized-LEZ, and adaptor-extraction gates. Each method
//! makes one wallet submission and then mines the agreement's fixed confirmation
//! count on a project-owned Regtest daemon. There is no automatic retry.

use std::collections::HashMap;
use std::fmt;
use std::num::NonZeroU64;

use lez_xmr_swap_sdk::{MoneroPrivateViewKey, MoneroSharedAddressV1, ReconstructedMoneroSpendKey};
use monero_rpc::monero::util::address::AddressType;
use monero_rpc::monero::{Address, Amount, PrivateKey};
use monero_rpc::{
    GenerateFromKeysArgs, RegtestDaemonJsonRpcClient, SweepAllArgs, TransferOptions,
    TransferPriority, WalletClient,
};
use thiserror::Error;

use super::{
    LoopbackRpcEndpoint, MoneroTransactionId, REQUIRED_MONERO_CONFIRMATIONS, valid_credential,
};

const WALLET_ACCOUNT_INDEX: u32 = 0;
const MAX_WALLET_FILENAME_BYTES: usize = 96;
const MAX_SWEEP_TRANSACTIONS: usize = 16;

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

/// Typed one-attempt wallet and Regtest-daemon effect boundary.
pub struct MoneroRegtestWalletEffects {
    daemon: RegtestDaemonJsonRpcClient,
    wallet: WalletClient,
    daemon_origin: String,
    wallet_origin: String,
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
            daemon_origin: daemon.base_url.clone(),
            wallet_origin: wallet.base_url.clone(),
        })
    }

    /// Returns the currently open wallet's primary standard address.
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
        let address = self
            .wallet
            .get_address(WALLET_ACCOUNT_INDEX, Some(vec![0]))
            .await
            .map_err(|_| MoneroWalletEffectError::Rpc("current wallet primary address"))?
            .address;
        validate_standard_address(&address)?;
        Ok(address)
    }

    /// Submits one exact transfer and locally generates ten confirmation blocks.
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
        let mining_address = self
            .wallet
            .get_address(WALLET_ACCOUNT_INDEX, Some(vec![0]))
            .await
            .map_err(|_| MoneroWalletEffectError::Rpc("funding wallet address"))?
            .address;

        let mut destinations = HashMap::with_capacity(1);
        destinations.insert(destination, Amount::from_pico(amount.get()));
        let transfer = self
            .wallet
            .transfer(
                destinations,
                TransferPriority::Default,
                TransferOptions {
                    account_index: Some(WALLET_ACCOUNT_INDEX),
                    do_not_relay: Some(false),
                    ..TransferOptions::default()
                },
            )
            .await
            .map_err(|_| MoneroWalletEffectError::Rpc("fund exact transfer"))?;
        if transfer.amount.as_pico() != amount.get() {
            return Err(MoneroWalletEffectError::FundingAmountMismatch);
        }

        let confirmation_tip_height = self.mine_confirmations(mining_address).await?;
        Ok(ConfirmedMoneroFunding {
            transaction_id: transfer.tx_hash.0,
            amount_piconero: amount.get(),
            confirmation_tip_height,
        })
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

    /// Closes the current wallet and restores the exact Stage-A shared address
    /// as an official view-only wallet.
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

        self.wallet
            .close_wallet()
            .await
            .map_err(|_| MoneroWalletEffectError::Rpc("close current wallet"))?;
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
            .map_err(|_| MoneroWalletEffectError::Rpc("generate shared view-only wallet"))?;
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
        self.wallet
            .refresh(Some(restore_height))
            .await
            .map_err(|_| MoneroWalletEffectError::Rpc("refresh current wallet"))?;
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
    /// mismatch, non-exact balance, typed RPC failure, or a split/empty sweep.
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

    /// Restores the point-checked shared wallet and submits exactly one sweep.
    ///
    /// Unlike [`Self::restore_shared_and_sweep`], this method never mines or
    /// waits for confirmation. It is the sending half of the durable actor
    /// boundary: callers persist the returned transaction identity and restart
    /// through a separate read-only finality observer rather than retrying.
    ///
    /// # Errors
    ///
    /// Fails closed on an unsafe wallet filename/password, address or key
    /// mismatch, non-exact balance, typed RPC failure, or a split/empty sweep.
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

        self.wallet
            .close_wallet()
            .await
            .map_err(|_| MoneroWalletEffectError::Rpc("close current wallet"))?;
        let created = self
            .wallet
            .generate_from_keys(GenerateFromKeysArgs {
                restore_height: Some(restore_height),
                filename: wallet_filename,
                address,
                spendkey: Some(spend_key),
                viewkey: view_key,
                password: wallet_password,
                autosave_current: Some(true),
            })
            .await
            .map_err(|_| MoneroWalletEffectError::Rpc("generate shared wallet from keys"))?;
        if created.address != address {
            return Err(MoneroWalletEffectError::RestoredAddressMismatch);
        }

        self.wallet
            .refresh(Some(restore_height))
            .await
            .map_err(|_| MoneroWalletEffectError::Rpc("refresh restored wallet"))?;
        let balance = self
            .wallet
            .get_balance(WALLET_ACCOUNT_INDEX, Some(vec![0]))
            .await
            .map_err(|_| MoneroWalletEffectError::Rpc("read restored wallet balance"))?;
        if balance.balance.as_pico() != expected_amount.get()
            || balance.unlocked_balance.as_pico() != expected_amount.get()
        {
            return Err(MoneroWalletEffectError::RestoredBalanceMismatch);
        }

        let sweep = self
            .wallet
            .sweep_all(SweepAllArgs {
                address: destination,
                account_index: WALLET_ACCOUNT_INDEX,
                subaddr_indices: Some(vec![0]),
                priority: TransferPriority::Default,
                mixin: 0,
                ring_size: 0,
                unlock_time: 0,
                get_tx_keys: None,
                below_amount: None,
                do_not_relay: Some(false),
                get_tx_hex: None,
                get_tx_metadata: None,
            })
            .await
            .map_err(|_| MoneroWalletEffectError::Rpc("sweep reconstructed wallet"))?;
        if sweep.tx_hash_list.is_empty() || sweep.tx_hash_list.len() > MAX_SWEEP_TRANSACTIONS {
            return Err(MoneroWalletEffectError::InvalidSweepTransactionCount);
        }
        if sweep.tx_hash_list.len() != 1 {
            return Err(MoneroWalletEffectError::SplitSweepUnsupported);
        }
        let (received_amount_piconero, fee_piconero) =
            validate_sweep_accounting(&sweep.amount_list, &sweep.fee_list, expected_amount.get())?;
        let transaction_id = sweep
            .tx_hash_list
            .into_iter()
            .next()
            .ok_or(MoneroWalletEffectError::InvalidSweepTransactionCount)?
            .0;
        Ok(SubmittedMoneroSweep {
            transaction_id,
            funded_amount_piconero: expected_amount.get(),
            received_amount_piconero,
            fee_piconero,
        })
    }

    async fn mine_confirmations(
        &self,
        mining_address: Address,
    ) -> Result<u64, MoneroWalletEffectError> {
        let generated = self
            .daemon
            .generate_blocks(REQUIRED_MONERO_CONFIRMATIONS, mining_address)
            .await
            .map_err(|_| MoneroWalletEffectError::Rpc("generate confirmation blocks"))?;
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
            .field("credentials", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

/// Fail-closed configuration and typed effect errors.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum MoneroWalletEffectError {
    /// Daemon and wallet trust roles cannot share one origin.
    #[error("Monero daemon and wallet RPC origins must be distinct")]
    AliasedRpcOrigins,
    /// The maintained typed RPC client could not be built.
    #[error("failed to build typed Monero wallet-effect RPC client")]
    RpcClientBuild,
    /// One redacted typed RPC operation failed.
    #[error("typed Monero wallet-effect operation `{0}` failed")]
    Rpc(&'static str),
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
    /// Restored wallet did not contain exactly the expected unlocked principal.
    #[error("restored Monero wallet balance differs from the exact expected principal")]
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
}
