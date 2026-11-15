//! The Maker's Monero funding on its role's one wallet-rpc: the exact Stage-A
//! transfer with its confirmations, and the receipt an independent view-only
//! observation of the shared output gives it.
//!
//! Both files land in the swap's evidence root, written once: the funding
//! evidence (`lez_v02_m4_actual_local_monero_funding_v2`) and the receipt
//! (`lez_v02_m4_actual_local_monero_verification_v2`) every Maker branch
//! activation imports. The funding tool writes them right after it mined;
//! the funding observer writes them after a crash, from the shared output it
//! finds. The transfer itself is the workflow's one `FundMonero` invocation.

use std::{path::Path, str::FromStr as _};

use anyhow::{Context as _, Result, ensure};
use lez_bridge_protocol::RunId;
use lez_xmr_monero_adapter::{
    ExpectedMoneroOutput, LoopbackRpcEndpoint, MoneroAddress, MoneroChainIdentity,
    MoneroEvidenceError, MoneroNetwork, MoneroOutputVerifier, MoneroRegtestWalletEffects,
    MoneroTopologyVerifier, MoneroTransactionId, MoneroWalletEffectError as MoneroEffectError,
    MoneroWalletRpcLock, REQUIRED_MONERO_CONFIRMATIONS,
};
use lez_xmr_swap_sdk::{MoneroPrivateViewKey, XmrAgreementV1};

use crate::{
    M4_MONERO_NETWORK_SCOPE, M4_MONERO_RECEIPT_SCHEMA, MoneroReceiptEvidenceV2,
    XmrEffectChildPlanV1, canonical_json_bytes,
    maker_refund_activation::{
        FUNDING_SCHEMA, FundingEvidenceV2, maker_monero_funding_evidence_sha256,
        persist_or_validate_private,
    },
    read_private_input,
};

/// The Maker's funding evidence in the swap's evidence root.
pub const XMR_MAKER_FUNDING_EVIDENCE_FILE: &str = "monero-funding.json";
/// The independent receipt of that funding in the swap's evidence root.
pub const XMR_MAKER_FUNDING_RECEIPT_FILE: &str = "monero-funding-receipt.json";
/// The view-only wallets scan this chain from genesis (regtest only).
const RESTORE_HEIGHT: u64 = 0;
/// Goes at the funding while it fails before its transfer, and the pause
/// between them: a wallet-rpc request can fail in transit (2026-11-14, a
/// `get_address` that never reached the wallet-rpc).
const FUNDING_ATTEMPTS: u32 = 3;
const FUNDING_RETRY_PAUSE: std::time::Duration = std::time::Duration::from_secs(5);

/// The error context of a Maker funding that failed before its transfer was
/// handed to the wallet-rpc: no Monero was sent. The funding tool's exit
/// status tells the actor so.
#[derive(Clone, Copy, Debug)]
pub struct MoneroFundingNotSent;

impl std::fmt::Display for MoneroFundingNotSent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("the Monero funding failed before its transfer: nothing was sent")
    }
}

/// The funding tool's exit status for a [`MoneroFundingNotSent`] failure.
pub const XMR_MONERO_FUNDING_NOT_SENT_EXIT: i32 = 3;

/// Wallet-effect failures of the funding that come before its `transfer`.
fn failed_before_transfer(error: &MoneroEffectError) -> bool {
    matches!(
        error,
        MoneroEffectError::NonStandardAddress
            | MoneroEffectError::ZeroAmount
            | MoneroEffectError::InvalidSharedAddress
            | MoneroEffectError::Rpc {
                operation: "open role wallet" | "refresh current wallet" | "funding wallet address",
                ..
            }
    )
}

/// One role's single wallet-rpc as an effect child reaches it: the daemon,
/// the wallet-rpc, the role wallet every effect reopens (empty password) and
/// the lock all users of that wallet-rpc hold.
pub struct XmrRoleWalletRpcV1 {
    daemon: LoopbackRpcEndpoint,
    wallet: LoopbackRpcEndpoint,
    role_wallet: String,
    lock_file: std::path::PathBuf,
}

impl std::fmt::Debug for XmrRoleWalletRpcV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("XmrRoleWalletRpcV1")
            .field("daemon", &self.daemon.base_url())
            .field("wallet", &self.wallet.base_url())
            .field("role_wallet", &self.role_wallet)
            .field("credentials", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl XmrRoleWalletRpcV1 {
    /// Binds the plan's role wallet and lock to the two endpoints.
    ///
    /// # Errors
    ///
    /// Fails when the plan names no role wallet (a two-wallet-rpc authority).
    pub fn new(
        plan: &XmrEffectChildPlanV1,
        daemon: LoopbackRpcEndpoint,
        wallet: LoopbackRpcEndpoint,
    ) -> Result<Self> {
        let (role_wallet, lock_file) = plan
            .monero_role_wallet_file()
            .context("the effect authority names no role wallet on one wallet-rpc")?;
        Ok(Self {
            daemon,
            wallet,
            role_wallet: role_wallet.to_owned(),
            lock_file: lock_file.to_path_buf(),
        })
    }

    /// Blocks until this role's wallet-rpc is free; hold it for the whole
    /// open → act → reopen sequence, once per process.
    ///
    /// # Errors
    ///
    /// Fails if the lock file cannot be opened or locked.
    pub fn lock(&self) -> Result<MoneroWalletRpcLock> {
        MoneroWalletRpcLock::acquire(&self.lock_file).context("lock the role's Monero wallet-rpc")
    }

    /// The daemon endpoint.
    #[must_use]
    pub const fn daemon(&self) -> &LoopbackRpcEndpoint {
        &self.daemon
    }

    /// The wallet-rpc endpoint.
    #[must_use]
    pub const fn wallet(&self) -> &LoopbackRpcEndpoint {
        &self.wallet
    }

    /// Effects that reopen the role wallet before they act.
    ///
    /// # Errors
    ///
    /// Fails for aliased endpoints or an unsafe wallet name.
    pub fn role_effects(&self) -> Result<MoneroRegtestWalletEffects> {
        Ok(MoneroRegtestWalletEffects::new(&self.daemon, &self.wallet)?
            .with_role_wallet(self.role_wallet.clone(), String::new())?)
    }
}

/// Sends the exact Stage-A amount from the role wallet to the shared address,
/// mines its confirmations and records the funding with its receipt. The
/// caller holds the workflow's one `FundMonero` invocation.
///
/// A failure before the transfer is retried; once the attempts are spent it
/// carries [`MoneroFundingNotSent`].
///
/// # Errors
///
/// Fails closed on any wallet, daemon or evidence failure; a failure after
/// the transfer leaves the funding for the observer to find.
pub async fn fund_maker_monero(
    rpc: &XmrRoleWalletRpcV1,
    plan: &XmrEffectChildPlanV1,
    agreement: &XmrAgreementV1,
    view_key: MoneroPrivateViewKey,
    shared_wallet_password: &str,
) -> Result<[u8; 32]> {
    ensure!(
        !plan
            .evidence_root()
            .join(XMR_MAKER_FUNDING_EVIDENCE_FILE)
            .exists(),
        "the Monero funding is already recorded"
    );
    let _lock = rpc.lock().context(MoneroFundingNotSent)?;
    let role = rpc.role_effects().context(MoneroFundingNotSent)?;
    let mut attempt = 1;
    let funding = loop {
        // A prior swap may have mined after spending from this wallet: refresh
        // so its unlocked change is visible to this one transfer.
        let result = async {
            role.open_role_wallet().await?;
            role.refresh_from_height(RESTORE_HEIGHT).await?;
            role.fund_shared_exact_and_confirm(
                agreement.shared_address(),
                agreement.body().monero().amount_piconero(),
            )
            .await
        }
        .await;
        match result {
            Ok(funding) => break funding,
            Err(error) if failed_before_transfer(&error) && attempt < FUNDING_ATTEMPTS => {
                eprintln!(
                    "Maker Monero funding: {error} before the transfer (attempt {attempt} of {FUNDING_ATTEMPTS}); trying again"
                );
                attempt += 1;
                std::thread::sleep(FUNDING_RETRY_PAUSE);
            }
            Err(error) if failed_before_transfer(&error) => {
                return Err(anyhow::Error::new(error)
                    .context("fund the exact Stage-A shared address")
                    .context(MoneroFundingNotSent));
            }
            Err(error) => {
                return Err(error).context("fund the exact Stage-A shared address");
            }
        }
    };
    let recorded = record_maker_monero_funding_locked(
        rpc,
        plan,
        agreement,
        view_key,
        shared_wallet_password,
        Some(funding.transaction_id()),
    )
    .await;
    let _ = role.open_role_wallet().await;
    recorded?.context("the confirmed Monero funding is not yet observable")
}

/// Returns the funding observer's evidence digest once the Maker's funding is
/// recorded with its receipt: from the files when present, else from the
/// shared output found in the swap's view-only wallet (a crash after the
/// transfer). `None` while no confirmed exact output is visible; one seen in
/// the pool or short of its confirmations it mines first (regtest,
/// `mine_unconfirmed_funding`).
///
/// # Errors
///
/// Fails closed on any wallet, daemon or evidence failure.
pub async fn observe_maker_monero_funding(
    rpc: &XmrRoleWalletRpcV1,
    plan: &XmrEffectChildPlanV1,
    agreement: &XmrAgreementV1,
    view_key: MoneroPrivateViewKey,
    shared_wallet_password: &str,
) -> Result<Option<[u8; 32]>> {
    if let Some(digest) = recorded_funding(plan)? {
        return Ok(Some(digest));
    }
    let _lock = rpc.lock()?;
    let recorded = record_maker_monero_funding_locked(
        rpc,
        plan,
        agreement,
        view_key,
        shared_wallet_password,
        None,
    )
    .await;
    let _ = rpc.role_effects()?.open_role_wallet().await;
    recorded
}

fn recorded_funding(plan: &XmrEffectChildPlanV1) -> Result<Option<[u8; 32]>> {
    let evidence = plan.evidence_root().join(XMR_MAKER_FUNDING_EVIDENCE_FILE);
    let receipt = plan.evidence_root().join(XMR_MAKER_FUNDING_RECEIPT_FILE);
    if !evidence.exists() || !receipt.exists() {
        return Ok(None);
    }
    let read = |path: &Path| {
        read_private_input(path, crate::M4_MONERO_EVIDENCE_MAX_BYTES, "Monero funding")
    };
    Ok(Some(maker_monero_funding_evidence_sha256(
        &read(&evidence)?,
        &read(&receipt)?,
    )))
}

/// Observes the exact shared output in the swap's view-only wallet and
/// writes the receipt, then the confirmed funding evidence. Holds the lock.
#[allow(
    clippy::too_many_lines,
    reason = "the observation, topology and both records stay one auditable sequence"
)]
async fn record_maker_monero_funding_locked(
    rpc: &XmrRoleWalletRpcV1,
    plan: &XmrEffectChildPlanV1,
    agreement: &XmrAgreementV1,
    view_key: MoneroPrivateViewKey,
    shared_wallet_password: &str,
    transaction: Option<MoneroTransactionId>,
) -> Result<Option<[u8; 32]>> {
    let name = format!("xmr-{}-view", hex::encode(plan.swap_id()));
    let password = shared_wallet_password.to_owned();
    let shared = MoneroRegtestWalletEffects::new(rpc.daemon(), rpc.wallet())?
        .with_role_wallet(name.clone(), password.clone())?;
    // A crash after the view-only wallet was made leaves its file: it is
    // opened instead of made again.
    if rpc
        .role_effects()?
        .restore_shared_view_only(
            agreement.shared_address(),
            view_key,
            name,
            password,
            RESTORE_HEIGHT,
        )
        .await
        .is_err()
    {
        shared.open_role_wallet().await?;
    }
    shared.refresh_from_height(RESTORE_HEIGHT).await?;
    let amount = agreement.body().monero().amount_piconero();
    let identity = MoneroChainIdentity::new(
        MoneroNetwork::Regtest,
        agreement.body().monero().genesis_hash(),
    )?;
    let verifier = MoneroOutputVerifier::new(identity, rpc.daemon(), rpc.wallet())?;
    let observing = transaction.is_none();
    let transaction = if let Some(transaction) = transaction {
        transaction
    } else {
        let Some(transaction) = verifier.find_incoming(amount).await? else {
            if verifier.find_incoming_in_pool(amount).await?.is_some() {
                mine_unconfirmed_funding(rpc).await?;
            }
            return Ok(None);
        };
        transaction
    };
    let destination = MoneroAddress::from_str(&agreement.shared_address().address_string())
        .context("the Stage-A shared address")?;
    let observation = match verifier
        .verify(&ExpectedMoneroOutput::new(
            transaction,
            destination,
            amount,
        )?)
        .await
    {
        Ok(observation) => observation,
        Err(error) if pending(&error) => {
            if observing && error.awaits_confirmations() {
                mine_unconfirmed_funding(rpc).await?;
            }
            return Ok(None);
        }
        Err(error) => return Err(error).context("verify the exact shared Monero output"),
    };
    let run_id = RunId::new(plan.run_id().to_owned()).context("the run identity")?;
    let topology = MoneroTopologyVerifier::for_one_wallet_rpc(
        run_id.clone(),
        identity,
        rpc.daemon(),
        rpc.wallet(),
    )?
    .verify()
    .await
    .context("verify the peerless local Monero topology")?;
    topology
        .validate_observation(&run_id, &observation)
        .context("bind the topology to the observed output")?;
    let transaction_id = hex::encode(observation.transaction_id().0);
    let receipt = MoneroReceiptEvidenceV2 {
        schema: M4_MONERO_RECEIPT_SCHEMA.to_owned(),
        run_id: plan.run_id().to_owned(),
        agreement_commitment: hex::encode(agreement.agreement_commitment()),
        monero_genesis_hash: hex::encode(observation.genesis_hash()),
        transaction_id: transaction_id.clone(),
        destination_address: observation.destination().to_string(),
        amount_piconero: observation.amount_piconero(),
        containing_block_hash: hex::encode(observation.containing_block_hash()),
        containing_block_height: observation.containing_block_height(),
        confirmations: observation.confirmations(),
        stable_tip_hash: hex::encode(observation.stable_tip_hash()),
        stable_tip_height: observation.stable_tip_height(),
        peer_count: topology.peer_count(),
        daemon_version: topology.daemon_version().to_owned(),
        target_wallet_version: topology.target_wallet_version(),
        foreign_wallet_version: topology.foreign_wallet_version(),
        network_scope: M4_MONERO_NETWORK_SCOPE.to_owned(),
        public_rpc_used: false,
        faucet_used: false,
    };
    let evidence = FundingEvidenceV2 {
        schema: FUNDING_SCHEMA.to_owned(),
        attempt_state: "confirmed".to_owned(),
        agreement_commitment: hex::encode(agreement.agreement_commitment()),
        shared_address: agreement.shared_address().address_string(),
        amount_piconero: amount,
        transaction_id: Some(transaction_id),
        confirmation_tip_height: Some(observation.stable_tip_height()),
        required_confirmations: REQUIRED_MONERO_CONFIRMATIONS,
        restore_height: RESTORE_HEIGHT,
        wallet_role: "stage_a_shared_view_only".to_owned(),
        network_scope: M4_MONERO_NETWORK_SCOPE.to_owned(),
        public_rpc_used: false,
        faucet_used: false,
        automatic_submission_retry: false,
    };
    let receipt_bytes = canonical_json_bytes(&receipt, "encode the Monero funding receipt")?;
    let evidence_bytes = canonical_json_bytes(&evidence, "encode the Monero funding evidence")?;
    // The receipt first: the evidence file is what marks the funding recorded.
    persist_or_validate_private(
        &plan.evidence_root().join(XMR_MAKER_FUNDING_RECEIPT_FILE),
        &receipt_bytes,
    )?;
    persist_or_validate_private(
        &plan.evidence_root().join(XMR_MAKER_FUNDING_EVIDENCE_FILE),
        &evidence_bytes,
    )?;
    Ok(Some(maker_monero_funding_evidence_sha256(
        &evidence_bytes,
        &receipt_bytes,
    )))
}

/// A regtest duty, here only: on a real network miners confirm the funding,
/// on this regtest nothing does but the funding tool, which mines its
/// confirmations right after its transfer. When that tool stopped in between
/// (killed, or a failed mining call), the observer that finds the Maker's
/// exact funding in the pool or short of its confirmations mines them,
/// paying the coinbase to the role wallet; the next observation records the
/// funding. (The adapter, and this module, admit regtest only.)
async fn mine_unconfirmed_funding(rpc: &XmrRoleWalletRpcV1) -> Result<()> {
    eprintln!(
        "Maker Monero funding observer: the funding is in the pool or short of its \
         confirmations; mining them (regtest)"
    );
    let role = rpc.role_effects()?;
    role.mine_confirmations(role.primary_standard_address().await?)
        .await
        .context("mine the unconfirmed Monero funding's confirmations")?;
    Ok(())
}

const fn pending(error: &MoneroEvidenceError) -> bool {
    matches!(
        error,
        MoneroEvidenceError::MissingWalletTransfer
            | MoneroEvidenceError::WalletTransferInPool
            | MoneroEvidenceError::OutputNotUnlocked
            | MoneroEvidenceError::DaemonMissedTransaction
            | MoneroEvidenceError::DaemonTransactionInPool
            | MoneroEvidenceError::InsufficientConfirmations
            | MoneroEvidenceError::NonzeroUnlockDistance
    )
}
