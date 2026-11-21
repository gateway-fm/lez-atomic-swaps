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
//! The receipt is written first; a receipt without its evidence (a crash
//! between the two) is completed from that receipt, and it also stops the
//! funding tool from sending again.

use std::{path::Path, str::FromStr as _};

use anyhow::{Context as _, Result, ensure};
use lez_bridge_protocol::RunId;
use lez_xmr_monero_adapter::{
    ExpectedMoneroOutput, LoopbackRpcEndpoint, MoneroAddress, MoneroChainIdentityAttestor,
    MoneroEvidenceError, MoneroOutputVerifier, MoneroRegtestMining, MoneroRegtestWalletEffects,
    MoneroTopologyVerifier, MoneroTransactionId, MoneroWalletEffectError as MoneroEffectError,
    MoneroWalletRpcLock, REQUIRED_MONERO_CONFIRMATIONS,
};
use lez_xmr_swap_sdk::{MoneroPrivateViewKey, XmrAgreementV1};

use crate::{
    M4_MONERO_RECEIPT_SCHEMA, MoneroEvidenceScope, MoneroReceiptEvidenceV2, XmrEffectChildPlanV1,
    agreement_monero_identity, agreement_restore_height, canonical_json_bytes,
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
/// The role wallet refreshes from where it stopped: a start height at or
/// below its own height leaves its scan incremental.
const ROLE_WALLET_REFRESH_FROM: u64 = 0;
/// Goes at the funding while it fails without sending, and the pause between
/// them: a wallet-rpc request can fail in transit (2026-11-14, a
/// `get_address` that never reached the wallet-rpc; 2026-11-21, a `transfer`
/// the wallet-rpc refused with a 401).
const FUNDING_ATTEMPTS: u32 = 3;
const FUNDING_RETRY_PAUSE: std::time::Duration = std::time::Duration::from_secs(5);

/// The error context of a Maker funding that failed before its transfer was
/// handed to the wallet-rpc, or whose transfer the wallet-rpc did not run:
/// no Monero was sent. The funding tool's exit status tells the actor so.
#[derive(Clone, Copy, Debug)]
pub struct MoneroFundingNotSent;

impl std::fmt::Display for MoneroFundingNotSent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("the Monero funding failed before its transfer: nothing was sent")
    }
}

/// The funding tool's exit status for a [`MoneroFundingNotSent`] failure.
pub const XMR_MONERO_FUNDING_NOT_SENT_EXIT: i32 = 3;

/// Wallet-effect failures of the funding that prove nothing was sent: a
/// failure before its `transfer`, or a transfer the wallet-rpc did not run
/// (refused, or not found by the lookup that follows a lost answer).
fn failed_before_transfer(error: &MoneroEffectError) -> bool {
    error.funding_not_sent()
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

/// Sends the exact Stage-A amount from the role wallet to the shared address
/// and, on Regtest, mines its confirmations and records the funding with its
/// receipt (the evidence digest). On a public network the miners confirm it:
/// the tool returns `None` once the transfer is out, and the funding observer
/// records the funding at its ten confirmations. The caller holds the
/// workflow's one `FundMonero` invocation.
///
/// A failure that sent nothing (before the transfer, or a transfer the
/// wallet-rpc did not run) is retried; once the attempts are spent it
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
) -> Result<Option<[u8; 32]>> {
    // Either file means a funding was observed: a receipt alone is a record
    // the observer completes, never a reason to send once more.
    ensure!(
        !recorded_funding_files(plan.evidence_root()),
        "the Monero funding is already recorded"
    );
    let identity = agreement_monero_identity(agreement).context(MoneroFundingNotSent)?;
    let _lock = rpc.lock().context(MoneroFundingNotSent)?;
    // Regtest's daemon proves it is the fakechain before this tool mines; on
    // a public network there is no mining capability at all.
    let mining = MoneroRegtestMining::where_the_role_mines(identity, rpc.daemon())
        .await
        .context("prove the Monero daemon's network")
        .context(MoneroFundingNotSent)?;
    let role = rpc.role_effects().context(MoneroFundingNotSent)?;
    // The role wallet is open from here on, under the lock: the transfer and
    // the shared wallet's restore do not open it once more (an open takes the
    // wallet-rpc two to seven seconds on the devnet).
    let open = MoneroRegtestWalletEffects::new(rpc.daemon(), rpc.wallet())
        .context(MoneroFundingNotSent)?;
    let mut attempt = 1;
    let funding = loop {
        // A prior swap may have mined after spending from this wallet: refresh
        // so its unlocked change is visible to this one transfer.
        let result = async {
            role.open_role_wallet().await?;
            role.refresh_from_height(ROLE_WALLET_REFRESH_FROM).await?;
            let (shared, amount) = (
                agreement.shared_address(),
                agreement.body().monero().amount_piconero(),
            );
            match &mining {
                Some(mining) => open
                    .fund_shared_exact_and_confirm(mining, shared, amount)
                    .await
                    .map(|funding| funding.transaction_id()),
                None => open.fund_shared_exact_once(shared, amount).await,
            }
        }
        .await;
        match result {
            Ok(funding) => break funding,
            Err(error) if failed_before_transfer(&error) && attempt < FUNDING_ATTEMPTS => {
                eprintln!(
                    "Maker Monero funding: {error}; nothing was sent (attempt {attempt} of {FUNDING_ATTEMPTS}); trying again"
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
    if mining.is_none() {
        eprintln!(
            "Maker Monero funding: sent {} in {}; the network's miners confirm it, the observer records it at {REQUIRED_MONERO_CONFIRMATIONS} confirmations",
            agreement.body().monero().amount_piconero(),
            hex::encode(funding.0)
        );
        let _ = role.open_role_wallet().await;
        return Ok(None);
    }
    let recorded = record_maker_monero_funding_locked(
        rpc,
        plan,
        agreement,
        view_key,
        shared_wallet_password,
        Some(funding),
    )
    .await;
    let _ = role.open_role_wallet().await;
    recorded?
        .context("the confirmed Monero funding is not yet observable")
        .map(Some)
}

/// Returns the funding observer's evidence digest once the Maker's funding is
/// recorded with its receipt: from the files when present, else from the
/// shared output found in the swap's view-only wallet (a crash after the
/// transfer, or on a public network the confirmations its miners give).
/// `None` while no confirmed exact output is visible; on Regtest one seen in
/// the pool or short of its confirmations it mines first
/// (`mine_unconfirmed_funding`), on a public network it waits.
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

/// Whether either funding record exists: the funding was observed.
fn recorded_funding_files(evidence_root: &Path) -> bool {
    [
        XMR_MAKER_FUNDING_EVIDENCE_FILE,
        XMR_MAKER_FUNDING_RECEIPT_FILE,
    ]
    .iter()
    .any(|file| evidence_root.join(file).exists())
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
    let identity = agreement_monero_identity(agreement)?;
    let scope = MoneroEvidenceScope::new(identity.network());
    let attestor = MoneroChainIdentityAttestor::new(identity.network(), rpc.daemon())?;
    let restore_height = agreement_restore_height(agreement, &attestor).await?;
    let mining = MoneroRegtestMining::where_the_role_mines(identity, rpc.daemon()).await?;
    let name = format!("xmr-{}-view", hex::encode(plan.swap_id()));
    let password = shared_wallet_password.to_owned();
    let shared = MoneroRegtestWalletEffects::new(rpc.daemon(), rpc.wallet())?
        .with_role_wallet(name.clone(), password.clone())?;
    // Right after its transfer the funding tool holds the role wallet open;
    // an observer reopens it first, so that a restarted wallet-rpc has one to
    // close. A crash after the view-only wallet was made leaves its file: it
    // is opened instead of made again.
    let restorer = if transaction.is_some() {
        MoneroRegtestWalletEffects::new(rpc.daemon(), rpc.wallet())?
    } else {
        rpc.role_effects()?
    };
    if restorer
        .restore_shared_view_only(
            agreement.shared_address(),
            view_key,
            name,
            password,
            restore_height,
        )
        .await
        .is_err()
    {
        shared.open_role_wallet().await?;
    }
    shared.refresh_from_height(restore_height).await?;
    let amount = agreement.body().monero().amount_piconero();
    let verifier = MoneroOutputVerifier::new(identity, rpc.daemon(), rpc.wallet())?;
    let observing = transaction.is_none();
    let transaction = if let Some(transaction) = transaction {
        transaction
    } else {
        let Some(transaction) = verifier.find_incoming(amount).await? else {
            if let Some(mining) = &mining
                && verifier.find_incoming_in_pool(amount).await?.is_some()
            {
                mine_unconfirmed_funding(rpc, mining).await?;
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
            if let Some(mining) = &mining
                && observing
                && error.awaits_confirmations()
            {
                mine_unconfirmed_funding(rpc, mining).await?;
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
    .context("verify the Monero topology")?;
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
        network_scope: scope.name().to_owned(),
        public_rpc_used: false,
        faucet_used: false,
    };
    record_funding_files(
        plan.evidence_root(),
        &receipt,
        &FundingFacts {
            agreement_commitment: hex::encode(agreement.agreement_commitment()),
            shared_address: agreement.shared_address().address_string(),
            amount_piconero: amount,
            restore_height,
        },
    )
    .map(Some)
}

/// The agreement's facts the funding evidence repeats.
struct FundingFacts {
    agreement_commitment: String,
    shared_address: String,
    amount_piconero: u64,
    restore_height: u64,
}

/// The funding evidence a receipt implies: every field comes from the
/// receipt or the agreement, so one receipt always gives the same bytes.
fn funding_evidence(facts: &FundingFacts, receipt: &MoneroReceiptEvidenceV2) -> FundingEvidenceV2 {
    FundingEvidenceV2 {
        schema: FUNDING_SCHEMA.to_owned(),
        attempt_state: "confirmed".to_owned(),
        agreement_commitment: facts.agreement_commitment.clone(),
        shared_address: facts.shared_address.clone(),
        amount_piconero: facts.amount_piconero,
        transaction_id: Some(receipt.transaction_id.clone()),
        confirmation_tip_height: Some(receipt.stable_tip_height),
        required_confirmations: REQUIRED_MONERO_CONFIRMATIONS,
        restore_height: facts.restore_height,
        wallet_role: "stage_a_shared_view_only".to_owned(),
        network_scope: receipt.network_scope.clone(),
        public_rpc_used: false,
        faucet_used: false,
        automatic_submission_retry: false,
    }
}

/// Whether two receipts observe the same output in the same block. The tip,
/// its confirmation count and the topology facts move with every block.
fn same_funding(a: &MoneroReceiptEvidenceV2, b: &MoneroReceiptEvidenceV2) -> bool {
    a.schema == b.schema
        && a.run_id == b.run_id
        && a.agreement_commitment == b.agreement_commitment
        && a.monero_genesis_hash == b.monero_genesis_hash
        && a.transaction_id == b.transaction_id
        && a.destination_address == b.destination_address
        && a.amount_piconero == b.amount_piconero
        && a.containing_block_hash == b.containing_block_hash
        && a.containing_block_height == b.containing_block_height
}

/// Writes the receipt, then the evidence it implies, and returns their
/// digest.
///
/// The receipt is written first, so a crash can leave a receipt without its
/// evidence. The next observation sees a later tip, so its own receipt
/// differs in bytes. A receipt already on disk is kept when it observes the
/// same output in the same block, and the evidence is made from it: the
/// half-written pair is completed exactly as the first run would have
/// completed it. A receipt of another output or block is a conflict.
fn record_funding_files(
    evidence_root: &Path,
    fresh: &MoneroReceiptEvidenceV2,
    facts: &FundingFacts,
) -> Result<[u8; 32]> {
    let receipt_path = evidence_root.join(XMR_MAKER_FUNDING_RECEIPT_FILE);
    let (receipt, receipt_bytes) = if receipt_path.exists() {
        let bytes = read_private_input(
            &receipt_path,
            crate::M4_MONERO_EVIDENCE_MAX_BYTES,
            "recorded Monero funding receipt",
        )?;
        let recorded: MoneroReceiptEvidenceV2 =
            serde_json::from_slice(&bytes).context("decode the recorded Monero funding receipt")?;
        ensure!(
            canonical_json_bytes(&recorded, "re-encode the recorded Monero funding receipt")?
                == bytes,
            "the recorded Monero funding receipt is not canonical"
        );
        ensure!(
            same_funding(&recorded, fresh),
            "the recorded Monero funding receipt names another output or block"
        );
        (recorded, bytes)
    } else {
        let bytes = canonical_json_bytes(fresh, "encode the Monero funding receipt")?;
        persist_or_validate_private(&receipt_path, &bytes)?;
        (fresh.clone(), bytes)
    };
    let evidence_bytes = canonical_json_bytes(
        &funding_evidence(facts, &receipt),
        "encode the Monero funding evidence",
    )?;
    persist_or_validate_private(
        &evidence_root.join(XMR_MAKER_FUNDING_EVIDENCE_FILE),
        &evidence_bytes,
    )?;
    Ok(maker_monero_funding_evidence_sha256(
        &evidence_bytes,
        &receipt_bytes,
    ))
}

/// A Regtest duty, here only: on a public network miners confirm the
/// funding (and no [`MoneroRegtestMining`] exists for it), on Regtest nothing
/// does but the funding tool, which mines its confirmations right after its
/// transfer. When that tool stopped in between (killed, or a failed mining
/// call), the observer that finds the Maker's exact funding in the pool or
/// short of its confirmations mines them, paying the coinbase to the role
/// wallet; the next observation records the funding.
async fn mine_unconfirmed_funding(
    rpc: &XmrRoleWalletRpcV1,
    mining: &MoneroRegtestMining,
) -> Result<()> {
    eprintln!(
        "Maker Monero funding observer: the funding is in the pool or short of its \
         confirmations; mining them (regtest)"
    );
    let role = rpc.role_effects()?;
    role.mine_confirmations(mining, role.primary_standard_address().await?)
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

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt as _, path::PathBuf};

    use super::{
        FundingFacts, MoneroReceiptEvidenceV2, XMR_MAKER_FUNDING_EVIDENCE_FILE,
        XMR_MAKER_FUNDING_RECEIPT_FILE, record_funding_files, recorded_funding_files,
    };
    use crate::canonical_json_bytes;

    fn receipt(tip: u64) -> MoneroReceiptEvidenceV2 {
        MoneroReceiptEvidenceV2 {
            schema: crate::M4_MONERO_RECEIPT_SCHEMA.to_owned(),
            run_id: "xmr-funding-run-0001".to_owned(),
            agreement_commitment: "aa".repeat(32),
            monero_genesis_hash: "bb".repeat(32),
            transaction_id: "cc".repeat(32),
            destination_address: "shared-address".to_owned(),
            amount_piconero: 1_000_000,
            containing_block_hash: "dd".repeat(32),
            containing_block_height: 100,
            confirmations: tip - 99,
            stable_tip_hash: format!("{tip:064x}"),
            stable_tip_height: tip,
            peer_count: 0,
            daemon_version: "0.18".to_owned(),
            target_wallet_version: 1,
            foreign_wallet_version: 1,
            network_scope: "isolated_official_monero_regtest".to_owned(),
            public_rpc_used: false,
            faucet_used: false,
        }
    }

    fn facts() -> FundingFacts {
        FundingFacts {
            agreement_commitment: "aa".repeat(32),
            shared_address: "shared-address".to_owned(),
            amount_piconero: 1_000_000,
            restore_height: 0,
        }
    }

    /// An owner-private evidence root by its canonical path (the secure
    /// writer refuses a parent reached through a symlink, as macOS's
    /// temporary directory is).
    fn evidence_root() -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let path = fs::canonicalize(root.path()).unwrap();
        (root, path)
    }

    /// The crash the review found: the receipt is on disk, the evidence is
    /// not, and the next observation sees a later tip. The pair is completed
    /// from the first receipt, to the bytes an uninterrupted run writes.
    #[test]
    fn a_crash_between_receipt_and_evidence_is_completed_from_the_receipt() {
        let (_whole, whole) = evidence_root();
        let expected = record_funding_files(whole.as_path(), &receipt(110), &facts()).unwrap();

        let (_crashed, crashed) = evidence_root();
        let receipt_bytes = canonical_json_bytes(&receipt(110), "receipt").unwrap();
        fs::write(
            crashed.as_path().join(XMR_MAKER_FUNDING_RECEIPT_FILE),
            &receipt_bytes,
        )
        .unwrap();
        fs::set_permissions(
            crashed.as_path().join(XMR_MAKER_FUNDING_RECEIPT_FILE),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        assert!(
            !crashed
                .as_path()
                .join(XMR_MAKER_FUNDING_EVIDENCE_FILE)
                .exists()
        );
        // Blocks were mined since: the fresh observation's tip moved.
        let digest = record_funding_files(crashed.as_path(), &receipt(125), &facts())
            .expect("the half-written pair is completed");
        assert_eq!(digest, expected);
        for file in [
            XMR_MAKER_FUNDING_RECEIPT_FILE,
            XMR_MAKER_FUNDING_EVIDENCE_FILE,
        ] {
            assert_eq!(
                fs::read(crashed.as_path().join(file)).unwrap(),
                fs::read(whole.as_path().join(file)).unwrap(),
                "{file}"
            );
        }
        assert!(recorded_funding_files(crashed.as_path()));
        // Any later observation is a replay of the same record.
        assert_eq!(
            record_funding_files(crashed.as_path(), &receipt(140), &facts()).unwrap(),
            expected
        );
    }

    #[test]
    fn a_recorded_receipt_of_another_output_is_a_conflict() {
        let (_root, root) = evidence_root();
        record_funding_files(root.as_path(), &receipt(110), &facts()).unwrap();
        fs::remove_file(root.as_path().join(XMR_MAKER_FUNDING_EVIDENCE_FILE)).unwrap();
        let mut other = receipt(125);
        other.transaction_id = "ee".repeat(32);
        assert!(record_funding_files(root.as_path(), &other, &facts()).is_err());
        assert!(
            !root
                .as_path()
                .join(XMR_MAKER_FUNDING_EVIDENCE_FILE)
                .exists()
        );
    }
}
