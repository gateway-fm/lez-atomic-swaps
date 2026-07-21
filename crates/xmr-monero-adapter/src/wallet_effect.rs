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
    confirmation_tip_height: u64,
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
        validate_wallet_filename(&wallet_filename)?;
        if !valid_credential(&wallet_password, true) {
            return Err(MoneroWalletEffectError::InvalidWalletPassword);
        }
        validate_standard_address(&destination)?;
        validate_standard_address(&mining_address)?;
        let expected_amount =
            NonZeroU64::new(expected_amount_piconero).ok_or(MoneroWalletEffectError::ZeroAmount)?;
        let address: Address = expected_address
            .address_string()
            .parse()
            .map_err(|_| MoneroWalletEffectError::InvalidSharedAddress)?;
        validate_standard_address(&address)?;

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
        let transaction_id = sweep
            .tx_hash_list
            .into_iter()
            .next()
            .ok_or(MoneroWalletEffectError::InvalidSweepTransactionCount)?
            .0;
        let confirmation_tip_height = self.mine_confirmations(mining_address).await?;
        Ok(ConfirmedMoneroSweep {
            transaction_id,
            funded_amount_piconero: expected_amount.get(),
            confirmation_tip_height,
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
    /// A multi-transaction sweep is outside the first vertical `PoC`.
    #[error("split Monero sweep is unsupported by the first vertical PoC")]
    SplitSweepUnsupported,
}

fn validate_standard_address(address: &Address) -> Result<(), MoneroWalletEffectError> {
    if address.addr_type != AddressType::Standard {
        return Err(MoneroWalletEffectError::NonStandardAddress);
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

    #[test]
    fn wallet_filename_is_one_safe_component() {
        assert!(validate_wallet_filename("m4_claim_0123").is_ok());
        for invalid in ["", ".hidden", "../escape", "has/slash", "has space"] {
            assert_eq!(
                validate_wallet_filename(invalid),
                Err(MoneroWalletEffectError::InvalidWalletFilename)
            );
        }
    }
}
