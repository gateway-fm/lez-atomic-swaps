//! What a role's own wallets hold, for the owner's desk: the role's Bitcoin
//! Core wallet and its LEZ owner account. Read only; nothing here signs or
//! spends, and every failure is reported as a state rather than an error so a
//! desk can show what it can.

use anyhow::{Context as _, Result};
use jsonrpsee::core::client::ClientT as _;
use jsonrpsee::rpc_params;
use jsonrpsee_http_client::HttpClientBuilder;
use serde::{Deserialize, Serialize};

use crate::config::BtcRoleRuntime;
use crate::funding::BitcoinWallet;

/// Whether a balance could be read.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BalanceStateV1 {
    /// Read from the chain just now.
    Available,
    /// The wallet or account exists but could not be read.
    Unavailable,
    /// This role has no such wallet configured.
    Disabled,
}

/// The role's Bitcoin Core wallet.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BitcoinBalanceV1 {
    pub state: BalanceStateV1,
    /// The Core wallet name, when one is configured.
    pub wallet: Option<String>,
    pub network: String,
    /// Confirmed coins plus own unconfirmed change, in satoshis.
    pub trusted_sat: u64,
    /// Unconfirmed coins from others, in satoshis.
    pub untrusted_pending_sat: u64,
    /// Coinbase outputs not yet spendable, in satoshis.
    pub immature_sat: u64,
}

/// The role's LEZ owner account.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LezBalanceV1 {
    pub state: BalanceStateV1,
    pub owner_account_hex: String,
    pub owner_account_base58: String,
    /// Atomic units.
    pub balance_atomic_units: u128,
    pub nonce: u64,
}

/// Both of a role's wallets.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WalletBalancesV1 {
    pub schema_version: u16,
    pub bitcoin: BitcoinBalanceV1,
    pub lez: LezBalanceV1,
}

#[derive(Deserialize)]
struct IndexerAccount {
    balance: u128,
    nonce: u64,
}

/// The LEZ owner account's balance and nonce from the configured indexer.
///
/// # Errors
///
/// Fails when the indexer is unreachable or answers something else.
pub async fn lez_owner_balance(runtime: &BtcRoleRuntime) -> Result<(u128, u64)> {
    let client = HttpClientBuilder::default()
        .request_timeout(runtime.request_timeout())
        .build(&runtime.config().lez.indexer_url)
        .context("indexer client")?;
    let account = bs58::encode(runtime.lez_owner_account()).into_string();
    let account: IndexerAccount = client
        .request("getAccount", rpc_params![account])
        .await
        .context("getAccount")?;
    Ok((account.balance, account.nonce))
}

/// Everything the role's own wallets hold, never failing: a wallet that
/// cannot be read reports `unavailable` with zero amounts, a role without a
/// Bitcoin wallet reports `disabled`.
pub async fn role_wallet_balances(runtime: &BtcRoleRuntime) -> WalletBalancesV1 {
    let bitcoin_config = &runtime.config().bitcoin;
    let mut bitcoin = BitcoinBalanceV1 {
        state: BalanceStateV1::Disabled,
        wallet: bitcoin_config.wallet.clone(),
        network: bitcoin_config.network.network().to_string(),
        trusted_sat: 0,
        untrusted_pending_sat: 0,
        immature_sat: 0,
    };
    if bitcoin_config.wallet.is_some() {
        let read = async {
            BitcoinWallet::connect(
                &bitcoin_config.endpoint,
                &bitcoin_config.cookie_file,
                bitcoin_config.wallet.as_deref(),
                bitcoin_config.network.network(),
                runtime.request_timeout(),
            )?
            .balances()
            .await
        };
        match read.await {
            Ok(balances) => {
                bitcoin.state = BalanceStateV1::Available;
                bitcoin.trusted_sat = balances.trusted_sat;
                bitcoin.untrusted_pending_sat = balances.untrusted_pending_sat;
                bitcoin.immature_sat = balances.immature_sat;
            }
            Err(_) => bitcoin.state = BalanceStateV1::Unavailable,
        }
    }
    let owner = runtime.lez_owner_account();
    let mut lez = LezBalanceV1 {
        state: BalanceStateV1::Unavailable,
        owner_account_hex: hex::encode(owner),
        owner_account_base58: bs58::encode(owner).into_string(),
        balance_atomic_units: 0,
        nonce: 0,
    };
    if let Ok((balance, nonce)) = lez_owner_balance(runtime).await {
        lez.state = BalanceStateV1::Available;
        lez.balance_atomic_units = balance;
        lez.nonce = nonce;
    }
    WalletBalancesV1 {
        schema_version: 1,
        bitcoin,
        lez,
    }
}
