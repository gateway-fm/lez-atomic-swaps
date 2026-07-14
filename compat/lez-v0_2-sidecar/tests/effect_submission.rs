#![cfg(target_os = "linux")]

use std::{
    fs,
    fs::OpenOptions,
    os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink},
    path::{Path, PathBuf},
    sync::{Arc, Barrier, Mutex},
};

use async_trait::async_trait;
use common::{HashType, transaction::LeeTransaction};
use jsonrpsee::{core::ClientError, types::ErrorObjectOwned};
use lez_bridge_protocol::{
    DiscoveryWindow, Hex32, MessageContext, Participant, RequestId, RunId, RuntimeCompatibility,
    RuntimeDescriptor,
};
use lez_v0_2_sidecar::{
    PreparedVaultClaimEffect, SequencerSendFailure, SequencerSubmitApi, VaultClaimAllocation,
    VaultClaimBeforeState, VaultClaimEffectJournal, VaultClaimEffectJournalError,
    VaultClaimEffectScope, VaultClaimEffectState, VaultClaimNonceSource, VaultClaimPlanner,
    VaultClaimPrepareError, VaultClaimSubmissionOutcome, VaultClaimSubmissionUncertainty,
    VaultClaimSubmitError, VaultClaimSubmitter, classify_sequencer_send_error,
    decode_official_public_transaction,
};
use nssa::{Account, AccountId, PrivateKey, PublicKey};
use tempfile::TempDir;

#[derive(Debug)]
struct PrivateDirectory(TempDir);

impl PrivateDirectory {
    fn new(label: &str) -> Self {
        let directory = tempfile::Builder::new()
            .prefix(&format!("lez-v02-effect-{label}-"))
            .tempdir()
            .unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
        Self(directory)
    }

    fn path(&self) -> &Path {
        self.0.path()
    }

    fn database_path(&self) -> PathBuf {
        self.path().join("vault-claim-effects.v1.sqlite")
    }
}

#[derive(Debug)]
struct FixedVaultNonce(u128);

#[async_trait]
impl VaultClaimNonceSource for FixedVaultNonce {
    async fn account_nonce(&self, _account_id: AccountId) -> Result<u128, VaultClaimPrepareError> {
        Ok(self.0)
    }
}

fn keyed_account(byte: u8) -> (AccountId, PrivateKey) {
    let key = PrivateKey::try_new([byte; 32]).unwrap();
    let account = AccountId::from(&PublicKey::new_from_private_key(&key));
    (account, key)
}

const fn h(byte: u8) -> Hex32 {
    Hex32::from_bytes([byte; 32])
}

fn runtime(role: Participant, signer: AccountId) -> RuntimeDescriptor {
    RuntimeDescriptor::new(
        role,
        RuntimeCompatibility::LeeV0_2_0,
        h(1),
        h(2),
        h(3),
        h(4),
        Hex32::from_bytes(signer.into_value()),
    )
}

async fn prepared_effect(
    role: Participant,
    key_byte: u8,
    run_id: &str,
    request_id: &str,
) -> (VaultClaimPlanner, PreparedVaultClaimEffect) {
    let (owner, key) = keyed_account(key_byte);
    let amount = 100_000_u128;
    let descriptor = runtime(role, owner);
    let allocation =
        VaultClaimAllocation::new(role, Hex32::from_bytes(owner.into_value()), amount).unwrap();
    let planner = VaultClaimPlanner::new(
        role,
        key,
        descriptor.clone(),
        allocation.clone(),
        Arc::new(FixedVaultNonce(0)),
    )
    .unwrap();
    let request = lez_v0_2_sidecar::PrepareVaultClaimRequest::new(
        MessageContext::new(
            RunId::new(run_id).unwrap(),
            RequestId::new(request_id).unwrap(),
            role,
        ),
        descriptor,
        allocation,
        0,
    );
    let result = planner.prepare(request.clone()).await.unwrap();
    let owner_vault = vault_core::compute_vault_account_id(programs::vault().id(), owner);
    let before = VaultClaimBeforeState::new(
        owner,
        Account::default(),
        owner_vault,
        Account {
            program_owner: programs::vault().id(),
            balance: amount,
            ..Account::default()
        },
        0,
        Some(0),
    )
    .unwrap();
    let effect = PreparedVaultClaimEffect::new(
        &planner,
        request,
        result,
        before,
        DiscoveryWindow::new(1, 64).unwrap(),
        DiscoveryWindow::new(1, 128).unwrap(),
    )
    .unwrap();
    (planner, effect)
}

#[derive(Clone, Copy, Debug)]
enum SendBehavior {
    ExactHash,
    ExactHashThenBlockLocalCommit,
    WrongHash,
    InvalidParams,
    Ambiguous,
}

type ObservedSend = (VaultClaimEffectState, u32, u64, Vec<u8>);

struct FakeSequencer {
    behavior: SendBehavior,
    journal_directory: PathBuf,
    actor_binding: lez_v0_2_sidecar::VaultClaimActorBinding,
    identity: lez_v0_2_sidecar::VaultClaimEffectIdentity,
    observed: Mutex<Vec<ObservedSend>>,
}

impl FakeSequencer {
    fn new(behavior: SendBehavior, directory: &Path, effect: &PreparedVaultClaimEffect) -> Self {
        Self {
            behavior,
            journal_directory: directory.to_path_buf(),
            actor_binding: effect.actor_binding().clone(),
            identity: effect.identity().clone(),
            observed: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> usize {
        self.observed.lock().unwrap().len()
    }
}

#[async_trait]
impl SequencerSubmitApi for FakeSequencer {
    async fn send_transaction(&self, transaction: LeeTransaction) -> Result<HashType, ClientError> {
        let journal =
            VaultClaimEffectJournal::open(&self.journal_directory, self.actor_binding.clone())
                .unwrap();
        let journaled = journal.load(&self.identity).unwrap().unwrap();
        let LeeTransaction::Public(public) = transaction else {
            panic!("a Vault Claim must wrap the exact inner public transaction once");
        };
        self.observed.lock().unwrap().push((
            journaled.state(),
            journaled.attempt_count(),
            journaled.revision(),
            public.to_bytes(),
        ));
        match self.behavior {
            SendBehavior::ExactHash => Ok(HashType(public.hash())),
            SendBehavior::ExactHashThenBlockLocalCommit => {
                let connection = rusqlite::Connection::open(
                    self.journal_directory.join("vault-claim-effects.v1.sqlite"),
                )
                .unwrap();
                connection
                    .execute_batch(
                        "CREATE TRIGGER fail_admission BEFORE UPDATE OF state
                         ON vault_claim_effects
                         BEGIN SELECT RAISE(ABORT, 'forced admission failure'); END;",
                    )
                    .unwrap();
                Ok(HashType(public.hash()))
            }
            SendBehavior::WrongHash => Ok(HashType([0x63; 32])),
            SendBehavior::InvalidParams => Err(ClientError::Call(ErrorObjectOwned::owned(
                -32602,
                "invalid params",
                None::<()>,
            ))),
            SendBehavior::Ambiguous => Err(ClientError::RequestTimeout),
        }
    }
}

#[tokio::test]
async fn typed_onboarding_scope_binds_owner_vault_allocation_runtime_and_transaction() {
    let (_planner, effect) = prepared_effect(
        Participant::Maker,
        5,
        "effect-run-0001",
        "effect-request-0001",
    )
    .await;
    let VaultClaimEffectScope::VaultOnboarding {
        owner_account_id,
        vault_account_id,
        allocation,
    } = effect.identity().scope();
    assert_eq!(owner_account_id, effect.before_state().owner_account_id());
    assert_eq!(vault_account_id, effect.before_state().vault_account_id());
    assert_eq!(*allocation, 100_000);
    assert_eq!(
        effect.identity().runtime(),
        effect.actor_binding().runtime()
    );
    assert_eq!(
        effect.identity().transaction_id(),
        &effect.result().claim.transaction_id
    );
}

#[tokio::test]
async fn typed_before_state_substitutions_never_make_an_effect_eligible() {
    let (planner, effect) = prepared_effect(
        Participant::Maker,
        5,
        "effect-run-0001",
        "effect-request-0001-before-state",
    )
    .await;
    let (owner, _) = keyed_account(5);
    let (wrong_owner, _) = keyed_account(8);
    let owner_vault = vault_core::compute_vault_account_id(programs::vault().id(), owner);
    let amount = 100_000_u128;
    let invalid_before_states = [
        VaultClaimBeforeState::new(
            wrong_owner,
            Account::default(),
            owner_vault,
            Account {
                program_owner: programs::vault().id(),
                balance: amount,
                ..Account::default()
            },
            0,
            Some(0),
        ),
        VaultClaimBeforeState::new(
            owner,
            Account::default(),
            wrong_owner,
            Account {
                program_owner: programs::vault().id(),
                balance: amount,
                ..Account::default()
            },
            0,
            Some(0),
        ),
        VaultClaimBeforeState::new(
            owner,
            Account::default(),
            owner_vault,
            Account {
                balance: amount,
                ..Account::default()
            },
            0,
            Some(0),
        ),
        VaultClaimBeforeState::new(
            owner,
            Account::default(),
            owner_vault,
            Account {
                program_owner: programs::vault().id(),
                balance: amount - 1,
                ..Account::default()
            },
            0,
            Some(0),
        ),
        VaultClaimBeforeState::new(
            owner,
            Account {
                nonce: 1_u128.into(),
                ..Account::default()
            },
            owner_vault,
            Account {
                program_owner: programs::vault().id(),
                balance: amount,
                ..Account::default()
            },
            0,
            Some(0),
        ),
    ];

    for before in invalid_before_states.into_iter().flatten() {
        assert!(
            PreparedVaultClaimEffect::new(
                &planner,
                effect.request().clone(),
                effect.result().clone(),
                before,
                effect.sequencer_window(),
                effect.indexer_window(),
            )
            .is_err()
        );
    }
}

#[tokio::test]
async fn empty_indexer_tip_binds_discovery_to_genesis_height() {
    let (planner, effect) = prepared_effect(
        Participant::Maker,
        5,
        "effect-run-0001",
        "effect-request-0001-empty-indexer",
    )
    .await;
    let (owner, _) = keyed_account(5);
    let owner_vault = vault_core::compute_vault_account_id(programs::vault().id(), owner);
    let before = VaultClaimBeforeState::new(
        owner,
        Account::default(),
        owner_vault,
        Account {
            program_owner: programs::vault().id(),
            balance: 100_000,
            ..Account::default()
        },
        0,
        None,
    )
    .unwrap();
    let prepare = |window| {
        PreparedVaultClaimEffect::new(
            &planner,
            effect.request().clone(),
            effect.result().clone(),
            before.clone(),
            effect.sequencer_window(),
            window,
        )
    };
    assert!(prepare(DiscoveryWindow::new(0, 128).unwrap()).is_ok());
    assert!(prepare(DiscoveryWindow::new(1, 128).unwrap()).is_err());
}

#[tokio::test]
async fn attempt_is_durable_before_the_single_send_and_exact_hash_admits() {
    let directory = PrivateDirectory::new("admit");
    let (planner, effect) = prepared_effect(
        Participant::Maker,
        5,
        "effect-run-0001",
        "effect-request-0002",
    )
    .await;
    let journal =
        VaultClaimEffectJournal::open(directory.path(), effect.actor_binding().clone()).unwrap();
    journal.record_prepared(&effect).unwrap();
    let prepared = journal.load(effect.identity()).unwrap().unwrap();
    assert_eq!(prepared.state(), VaultClaimEffectState::Prepared);
    assert_eq!(prepared.attempt_count(), 0);
    assert_eq!(prepared.revision(), 0);

    let sequencer = FakeSequencer::new(SendBehavior::ExactHash, directory.path(), &effect);
    let submitter = VaultClaimSubmitter::new(
        VaultClaimEffectJournal::open(directory.path(), effect.actor_binding().clone()).unwrap(),
        &planner,
    )
    .unwrap();
    let outcome = submitter
        .submit_or_observe(effect.identity(), &sequencer)
        .await
        .unwrap();
    assert_eq!(outcome, VaultClaimSubmissionOutcome::Admitted);
    assert_eq!(sequencer.calls(), 1);

    let observed = sequencer.observed.lock().unwrap().clone();
    let (state_at_call, attempts_at_call, revision_at_call, sent_bytes) = observed.first().unwrap();
    assert_eq!(*state_at_call, VaultClaimEffectState::AttemptStarted);
    assert_eq!(*attempts_at_call, 1);
    assert_eq!(*revision_at_call, 1);
    assert_eq!(sent_bytes, effect.result().claim.exact_bytes.as_slice());

    let admitted = journal.load(effect.identity()).unwrap().unwrap();
    assert_eq!(admitted.state(), VaultClaimEffectState::Admitted);
    assert_eq!(admitted.attempt_count(), 1);
    assert_eq!(admitted.revision(), 2);
    assert_eq!(admitted.uncertainty(), None);

    let replay = submitter
        .submit_or_observe(effect.identity(), &sequencer)
        .await
        .unwrap();
    assert_eq!(
        replay,
        VaultClaimSubmissionOutcome::ObserveOnly {
            state: VaultClaimEffectState::Admitted,
            uncertainty: None,
        }
    );
    assert_eq!(sequencer.calls(), 1);
}

#[tokio::test]
async fn crash_before_call_and_crash_after_call_are_both_observe_only() {
    for (label, remote_call_happened) in [("before-call", false), ("after-call", true)] {
        let directory = PrivateDirectory::new(label);
        let (planner, effect) = prepared_effect(
            Participant::Maker,
            5,
            "effect-run-0001",
            if remote_call_happened {
                "effect-request-0003b"
            } else {
                "effect-request-0003a"
            },
        )
        .await;
        let journal =
            VaultClaimEffectJournal::open(directory.path(), effect.actor_binding().clone())
                .unwrap();
        journal.record_prepared(&effect).unwrap();
        journal.begin_attempt(effect.identity()).unwrap();
        let sequencer = FakeSequencer::new(SendBehavior::ExactHash, directory.path(), &effect);
        if remote_call_happened {
            let public =
                decode_official_public_transaction(effect.result().claim.exact_bytes.as_slice())
                    .unwrap();
            sequencer
                .send_transaction(LeeTransaction::Public(public))
                .await
                .unwrap();
        }
        drop(journal);

        let submitter = VaultClaimSubmitter::new(
            VaultClaimEffectJournal::open(directory.path(), effect.actor_binding().clone())
                .unwrap(),
            &planner,
        )
        .unwrap();
        let outcome = submitter
            .submit_or_observe(effect.identity(), &sequencer)
            .await
            .unwrap();
        assert_eq!(
            outcome,
            VaultClaimSubmissionOutcome::ObserveOnly {
                state: VaultClaimEffectState::AttemptStarted,
                uncertainty: None,
            },
            "{label}"
        );
        assert_eq!(
            sequencer.calls(),
            usize::from(remote_call_happened),
            "{label}"
        );
    }
}

#[tokio::test]
async fn exact_remote_response_with_failed_local_commit_never_resends_after_reopen() {
    let directory = PrivateDirectory::new("response-commit-failure");
    let (planner, effect) = prepared_effect(
        Participant::Maker,
        5,
        "effect-run-0001",
        "effect-request-0003c",
    )
    .await;
    let journal =
        VaultClaimEffectJournal::open(directory.path(), effect.actor_binding().clone()).unwrap();
    journal.record_prepared(&effect).unwrap();
    let sequencer = FakeSequencer::new(
        SendBehavior::ExactHashThenBlockLocalCommit,
        directory.path(),
        &effect,
    );
    let submitter = VaultClaimSubmitter::new(
        VaultClaimEffectJournal::open(directory.path(), effect.actor_binding().clone()).unwrap(),
        &planner,
    )
    .unwrap();
    assert_eq!(
        submitter
            .submit_or_observe(effect.identity(), &sequencer)
            .await
            .unwrap_err(),
        VaultClaimSubmitError::Journal(VaultClaimEffectJournalError::CorruptState)
    );
    assert_eq!(sequencer.calls(), 1);

    let connection = rusqlite::Connection::open(directory.database_path()).unwrap();
    connection
        .execute_batch("DROP TRIGGER fail_admission;")
        .unwrap();
    drop(connection);
    let durable = journal.load(effect.identity()).unwrap().unwrap();
    assert_eq!(durable.state(), VaultClaimEffectState::AttemptStarted);
    assert_eq!(durable.attempt_count(), 1);
    assert_eq!(durable.revision(), 1);

    assert_eq!(
        submitter
            .submit_or_observe(effect.identity(), &sequencer)
            .await
            .unwrap(),
        VaultClaimSubmissionOutcome::ObserveOnly {
            state: VaultClaimEffectState::AttemptStarted,
            uncertainty: None,
        }
    );
    assert_eq!(sequencer.calls(), 1);
}

#[tokio::test]
async fn ambiguous_transport_and_wrong_hash_retain_attempt_state_without_resend() {
    for (label, behavior, request_id, uncertainty) in [
        (
            "transport",
            SendBehavior::Ambiguous,
            "effect-request-0004a",
            VaultClaimSubmissionUncertainty::AmbiguousRpc,
        ),
        (
            "wrong-hash",
            SendBehavior::WrongHash,
            "effect-request-0004b",
            VaultClaimSubmissionUncertainty::ReturnedHashMismatch,
        ),
    ] {
        let directory = PrivateDirectory::new(label);
        let (planner, effect) =
            prepared_effect(Participant::Maker, 5, "effect-run-0001", request_id).await;
        let journal =
            VaultClaimEffectJournal::open(directory.path(), effect.actor_binding().clone())
                .unwrap();
        journal.record_prepared(&effect).unwrap();
        let sequencer = FakeSequencer::new(behavior, directory.path(), &effect);
        let submitter = VaultClaimSubmitter::new(
            VaultClaimEffectJournal::open(directory.path(), effect.actor_binding().clone())
                .unwrap(),
            &planner,
        )
        .unwrap();

        let outcome = submitter
            .submit_or_observe(effect.identity(), &sequencer)
            .await
            .unwrap();
        assert_eq!(outcome, VaultClaimSubmissionOutcome::Unknown(uncertainty));
        assert_eq!(sequencer.calls(), 1);
        let durable = journal.load(effect.identity()).unwrap().unwrap();
        assert_eq!(durable.state(), VaultClaimEffectState::AttemptStarted);
        assert_eq!(durable.uncertainty(), Some(uncertainty));

        let reopened = submitter
            .submit_or_observe(effect.identity(), &sequencer)
            .await
            .unwrap();
        assert_eq!(
            reopened,
            VaultClaimSubmissionOutcome::ObserveOnly {
                state: VaultClaimEffectState::AttemptStarted,
                uncertainty: Some(uncertainty),
            }
        );
        assert_eq!(sequencer.calls(), 1);
    }
}

#[tokio::test]
async fn only_json_rpc_invalid_params_is_a_definitive_rejection() {
    let invalid_params = ClientError::Call(ErrorObjectOwned::owned(
        -32602,
        "invalid params",
        None::<()>,
    ));
    let internal = ClientError::Call(ErrorObjectOwned::owned(
        -32603,
        "internal error",
        None::<()>,
    ));
    assert_eq!(
        classify_sequencer_send_error(&invalid_params),
        SequencerSendFailure::DefinitiveInvalidParams
    );
    assert_eq!(
        classify_sequencer_send_error(&internal),
        SequencerSendFailure::Ambiguous
    );
    assert_eq!(
        classify_sequencer_send_error(&ClientError::RequestTimeout),
        SequencerSendFailure::Ambiguous
    );

    let directory = PrivateDirectory::new("rejected");
    let (planner, effect) = prepared_effect(
        Participant::Maker,
        5,
        "effect-run-0001",
        "effect-request-0005",
    )
    .await;
    let journal =
        VaultClaimEffectJournal::open(directory.path(), effect.actor_binding().clone()).unwrap();
    journal.record_prepared(&effect).unwrap();
    let sequencer = FakeSequencer::new(SendBehavior::InvalidParams, directory.path(), &effect);
    let submitter = VaultClaimSubmitter::new(
        VaultClaimEffectJournal::open(directory.path(), effect.actor_binding().clone()).unwrap(),
        &planner,
    )
    .unwrap();
    assert_eq!(
        submitter
            .submit_or_observe(effect.identity(), &sequencer)
            .await
            .unwrap(),
        VaultClaimSubmissionOutcome::Rejected
    );
    assert_eq!(sequencer.calls(), 1);
    assert_eq!(
        journal.load(effect.identity()).unwrap().unwrap().state(),
        VaultClaimEffectState::Rejected
    );
    assert_eq!(
        submitter
            .submit_or_observe(effect.identity(), &sequencer)
            .await
            .unwrap(),
        VaultClaimSubmissionOutcome::ObserveOnly {
            state: VaultClaimEffectState::Rejected,
            uncertainty: None,
        }
    );
    assert_eq!(sequencer.calls(), 1);
}

#[tokio::test]
async fn two_handles_compare_and_swap_to_exactly_one_network_call() {
    let directory = PrivateDirectory::new("concurrent");
    let (planner, effect) = prepared_effect(
        Participant::Maker,
        5,
        "effect-run-0001",
        "effect-request-0006",
    )
    .await;
    let journal =
        VaultClaimEffectJournal::open(directory.path(), effect.actor_binding().clone()).unwrap();
    journal.record_prepared(&effect).unwrap();
    let first = VaultClaimSubmitter::new(
        VaultClaimEffectJournal::open(directory.path(), effect.actor_binding().clone()).unwrap(),
        &planner,
    )
    .unwrap();
    let second = VaultClaimSubmitter::new(
        VaultClaimEffectJournal::open(directory.path(), effect.actor_binding().clone()).unwrap(),
        &planner,
    )
    .unwrap();
    let sequencer = FakeSequencer::new(SendBehavior::ExactHash, directory.path(), &effect);

    let (left, right) = tokio::join!(
        first.submit_or_observe(effect.identity(), &sequencer),
        second.submit_or_observe(effect.identity(), &sequencer)
    );
    let outcomes = [left.unwrap(), right.unwrap()];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| **outcome == VaultClaimSubmissionOutcome::Admitted)
            .count(),
        1
    );
    assert_eq!(sequencer.calls(), 1);
    let durable = journal.load(effect.identity()).unwrap().unwrap();
    assert_eq!(durable.state(), VaultClaimEffectState::Admitted);
    assert_eq!(durable.attempt_count(), 1);
}

#[tokio::test]
async fn independent_handles_forced_to_race_grant_exactly_one_attempt() {
    let directory = PrivateDirectory::new("forced-cas-race");
    let (_planner, effect) = prepared_effect(
        Participant::Maker,
        5,
        "effect-run-0001",
        "effect-request-0006-race",
    )
    .await;
    let journal =
        VaultClaimEffectJournal::open(directory.path(), effect.actor_binding().clone()).unwrap();
    journal.record_prepared(&effect).unwrap();

    let barrier = Arc::new(Barrier::new(2));
    let mut handles = Vec::new();
    for _ in 0..2 {
        let path = directory.path().to_path_buf();
        let binding = effect.actor_binding().clone();
        let identity = effect.identity().clone();
        let barrier = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            let contender = VaultClaimEffectJournal::open(path, binding).unwrap();
            barrier.wait();
            contender.begin_attempt(&identity).unwrap()
        }));
    }
    let results = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|won| **won).count(), 1);
    assert_eq!(results.iter().filter(|won| !**won).count(), 1);
    let durable = journal.load(effect.identity()).unwrap().unwrap();
    assert_eq!(durable.state(), VaultClaimEffectState::AttemptStarted);
    assert_eq!(durable.attempt_count(), 1);
    assert_eq!(durable.revision(), 1);
}

#[tokio::test]
async fn role_run_request_and_payload_drift_never_share_or_replace_state() {
    let directory = PrivateDirectory::new("isolation");
    let (maker_planner, maker) = prepared_effect(
        Participant::Maker,
        5,
        "effect-run-0001",
        "effect-request-0007",
    )
    .await;
    let journal =
        VaultClaimEffectJournal::open(directory.path(), maker.actor_binding().clone()).unwrap();
    journal.record_prepared(&maker).unwrap();

    let (_taker_planner, taker) = prepared_effect(
        Participant::Taker,
        6,
        "effect-run-0001",
        "effect-request-0007",
    )
    .await;
    assert_eq!(
        VaultClaimEffectJournal::open(directory.path(), taker.actor_binding().clone()).unwrap_err(),
        VaultClaimEffectJournalError::ActorBindingMismatch
    );

    let (_other_run_planner, other_run) = prepared_effect(
        Participant::Maker,
        5,
        "effect-run-0002",
        "effect-request-0007",
    )
    .await;
    assert_eq!(
        VaultClaimEffectJournal::open(directory.path(), other_run.actor_binding().clone())
            .unwrap_err(),
        VaultClaimEffectJournalError::ActorBindingMismatch
    );

    let (_other_request_planner, other_request) = prepared_effect(
        Participant::Maker,
        5,
        "effect-run-0001",
        "effect-request-0007-substituted",
    )
    .await;
    assert!(journal.load(other_request.identity()).unwrap().is_none());
    assert_eq!(
        journal.begin_attempt(other_request.identity()).unwrap_err(),
        VaultClaimEffectJournalError::UnknownEffect
    );
    assert_eq!(
        journal.record_prepared(&other_request).unwrap_err(),
        VaultClaimEffectJournalError::Conflict
    );

    let changed_window = PreparedVaultClaimEffect::new(
        &maker_planner,
        maker.request().clone(),
        maker.result().clone(),
        maker.before_state().clone(),
        DiscoveryWindow::new(1, 65).unwrap(),
        maker.indexer_window(),
    )
    .unwrap();
    assert_eq!(changed_window.identity(), maker.identity());
    assert_eq!(
        journal.record_prepared(&changed_window).unwrap_err(),
        VaultClaimEffectJournalError::Conflict
    );
}

#[tokio::test]
async fn injected_reset_trigger_is_rejected_before_it_can_restore_send_permission() {
    let directory = PrivateDirectory::new("rollback");
    let (_planner, effect) = prepared_effect(
        Participant::Maker,
        5,
        "effect-run-0001",
        "effect-request-0008",
    )
    .await;
    let journal =
        VaultClaimEffectJournal::open(directory.path(), effect.actor_binding().clone()).unwrap();
    journal.record_prepared(&effect).unwrap();

    let blocker = rusqlite::Connection::open(directory.database_path()).unwrap();
    blocker
        .execute_batch(
            "CREATE TRIGGER reset_attempt AFTER UPDATE OF state ON vault_claim_effects
             WHEN NEW.state = 'attempt_started'
             BEGIN
                 UPDATE vault_claim_effects
                 SET state = 'prepared', attempt_count = 0, revision = 0
                 WHERE singleton = 1;
             END;",
        )
        .unwrap();
    assert_eq!(
        journal.begin_attempt(effect.identity()).unwrap_err(),
        VaultClaimEffectJournalError::CorruptState
    );
    blocker
        .execute_batch("DROP TRIGGER reset_attempt;")
        .unwrap();
    drop(blocker);

    let durable = journal.load(effect.identity()).unwrap().unwrap();
    assert_eq!(durable.state(), VaultClaimEffectState::Prepared);
    assert_eq!(durable.attempt_count(), 0);
    assert_eq!(durable.revision(), 0);
}

#[tokio::test]
async fn noncanonical_transition_counters_fail_closed() {
    let directory = PrivateDirectory::new("noncanonical-transition");
    let (_planner, effect) = prepared_effect(
        Participant::Maker,
        5,
        "effect-run-0001",
        "effect-request-0008-transition",
    )
    .await;
    let journal =
        VaultClaimEffectJournal::open(directory.path(), effect.actor_binding().clone()).unwrap();
    journal.record_prepared(&effect).unwrap();
    journal.begin_attempt(effect.identity()).unwrap();
    let connection = rusqlite::Connection::open(directory.database_path()).unwrap();
    connection
        .execute("UPDATE vault_claim_effects SET revision = 3", [])
        .unwrap();
    drop(connection);
    assert_eq!(
        journal.load(effect.identity()).unwrap_err(),
        VaultClaimEffectJournalError::CorruptState
    );
}

#[tokio::test]
async fn corrupted_durable_exact_bytes_fail_closed_before_any_attempt() {
    let directory = PrivateDirectory::new("tampered");
    let (planner, effect) = prepared_effect(
        Participant::Maker,
        5,
        "effect-run-0001",
        "effect-request-0009",
    )
    .await;
    let journal =
        VaultClaimEffectJournal::open(directory.path(), effect.actor_binding().clone()).unwrap();
    journal.record_prepared(&effect).unwrap();
    let connection = rusqlite::Connection::open(directory.database_path()).unwrap();
    connection
        .execute(
            "UPDATE vault_claim_effects SET exact_transaction_bytes = X'00'",
            [],
        )
        .unwrap();
    drop(connection);

    let sequencer = FakeSequencer::new(SendBehavior::ExactHash, directory.path(), &effect);
    let submitter = VaultClaimSubmitter::new(
        VaultClaimEffectJournal::open(directory.path(), effect.actor_binding().clone()).unwrap(),
        &planner,
    )
    .unwrap();
    assert_eq!(
        submitter
            .submit_or_observe(effect.identity(), &sequencer)
            .await
            .unwrap_err(),
        VaultClaimSubmitError::InvalidDurableEffect
    );
    assert_eq!(sequencer.calls(), 0);
}

#[tokio::test]
async fn journal_rejects_symlink_components_wrong_modes_hardlinks_and_future_schemas() {
    let (_planner, effect) = prepared_effect(
        Participant::Maker,
        5,
        "effect-run-0001",
        "effect-request-0010",
    )
    .await;

    let insecure = PrivateDirectory::new("insecure-directory");
    fs::set_permissions(insecure.path(), fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        VaultClaimEffectJournal::open(insecure.path(), effect.actor_binding().clone()).unwrap_err(),
        VaultClaimEffectJournalError::InsecureDirectory
    );

    let actual = PrivateDirectory::new("actual-directory");
    let alias_parent = PrivateDirectory::new("alias-parent");
    let alias = alias_parent.path().join("state-link");
    symlink(actual.path(), &alias).unwrap();
    assert_eq!(
        VaultClaimEffectJournal::open(&alias, effect.actor_binding().clone()).unwrap_err(),
        VaultClaimEffectJournalError::InsecureDirectory
    );

    let untrusted_parent_root = PrivateDirectory::new("untrusted-parent-root");
    let untrusted_parent = untrusted_parent_root.path().join("shared");
    let state_under_untrusted_parent = untrusted_parent.join("state");
    fs::create_dir(&untrusted_parent).unwrap();
    fs::create_dir(&state_under_untrusted_parent).unwrap();
    fs::set_permissions(&untrusted_parent, fs::Permissions::from_mode(0o777)).unwrap();
    fs::set_permissions(
        &state_under_untrusted_parent,
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    assert_eq!(
        VaultClaimEffectJournal::open(
            &state_under_untrusted_parent,
            effect.actor_binding().clone(),
        )
        .unwrap_err(),
        VaultClaimEffectJournalError::InsecureDirectory
    );

    let symlinked_database = PrivateDirectory::new("symlinked-database");
    symlink("/dev/null", symlinked_database.database_path()).unwrap();
    assert_eq!(
        VaultClaimEffectJournal::open(symlinked_database.path(), effect.actor_binding().clone(),)
            .unwrap_err(),
        VaultClaimEffectJournalError::UnsafeDatabaseFile
    );

    let wrong_mode = PrivateDirectory::new("wrong-database-mode");
    drop(VaultClaimEffectJournal::open(wrong_mode.path(), effect.actor_binding().clone()).unwrap());
    fs::set_permissions(
        wrong_mode.database_path(),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert_eq!(
        VaultClaimEffectJournal::open(wrong_mode.path(), effect.actor_binding().clone())
            .unwrap_err(),
        VaultClaimEffectJournalError::UnsafeDatabaseFile
    );

    let hardlinked = PrivateDirectory::new("hardlinked-database");
    drop(VaultClaimEffectJournal::open(hardlinked.path(), effect.actor_binding().clone()).unwrap());
    fs::hard_link(
        hardlinked.database_path(),
        hardlinked.path().join("database-hardlink"),
    )
    .unwrap();
    assert_eq!(
        VaultClaimEffectJournal::open(hardlinked.path(), effect.actor_binding().clone())
            .unwrap_err(),
        VaultClaimEffectJournalError::UnsafeDatabaseFile
    );

    let future = PrivateDirectory::new("future-schema");
    drop(VaultClaimEffectJournal::open(future.path(), effect.actor_binding().clone()).unwrap());
    let connection = rusqlite::Connection::open(future.database_path()).unwrap();
    connection.pragma_update(None, "user_version", 99).unwrap();
    drop(connection);
    assert_eq!(
        VaultClaimEffectJournal::open(future.path(), effect.actor_binding().clone()).unwrap_err(),
        VaultClaimEffectJournalError::FutureSchema
    );

    let corrupt_v1 = PrivateDirectory::new("corrupt-v1-schema");
    let connection = rusqlite::Connection::open(corrupt_v1.database_path()).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE attacker_named_table (value TEXT);
             PRAGMA application_id = 1280988739;
             PRAGMA user_version = 1;",
        )
        .unwrap();
    drop(connection);
    fs::set_permissions(
        corrupt_v1.database_path(),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    assert_eq!(
        VaultClaimEffectJournal::open(corrupt_v1.path(), effect.actor_binding().clone())
            .unwrap_err(),
        VaultClaimEffectJournalError::CorruptState
    );
}

#[tokio::test]
async fn opened_journal_rejects_late_directory_or_database_inode_replacement() {
    let (_planner, effect) = prepared_effect(
        Participant::Maker,
        5,
        "effect-run-0001",
        "effect-request-0011",
    )
    .await;

    let parent = PrivateDirectory::new("late-directory-replacement");
    let state = parent.path().join("state");
    fs::create_dir(&state).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    let journal = VaultClaimEffectJournal::open(&state, effect.actor_binding().clone()).unwrap();
    fs::rename(&state, parent.path().join("moved-state")).unwrap();
    fs::create_dir(&state).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        journal.record_prepared(&effect).unwrap_err(),
        VaultClaimEffectJournalError::InsecureDirectory
    );

    let parent = PrivateDirectory::new("late-parent-permission-drift");
    let state = parent.path().join("state");
    fs::create_dir(&state).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    let journal = VaultClaimEffectJournal::open(&state, effect.actor_binding().clone()).unwrap();
    fs::set_permissions(parent.path(), fs::Permissions::from_mode(0o777)).unwrap();
    assert_eq!(
        journal.record_prepared(&effect).unwrap_err(),
        VaultClaimEffectJournalError::InsecureDirectory
    );

    let directory = PrivateDirectory::new("late-database-replacement");
    let journal =
        VaultClaimEffectJournal::open(directory.path(), effect.actor_binding().clone()).unwrap();
    fs::rename(
        directory.database_path(),
        directory.path().join("moved-database"),
    )
    .unwrap();
    OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(directory.database_path())
        .unwrap();
    assert_eq!(
        journal.record_prepared(&effect).unwrap_err(),
        VaultClaimEffectJournalError::UnsafeDatabaseFile
    );
}

#[tokio::test]
async fn actor_binding_and_journal_diagnostics_redact_state_paths() {
    let directory = PrivateDirectory::new("redacted-sensitive-path");
    let (_planner, effect) = prepared_effect(
        Participant::Maker,
        5,
        "effect-run-0001",
        "effect-request-0012",
    )
    .await;
    let journal =
        VaultClaimEffectJournal::open(directory.path(), effect.actor_binding().clone()).unwrap();
    let rendered = format!("{journal:?}");
    assert!(rendered.contains("[REDACTED]"));
    assert!(!rendered.contains("redacted-sensitive-path"));
    assert!(!format!("{:?}", effect.actor_binding()).contains(directory.path().to_str().unwrap()));
}
