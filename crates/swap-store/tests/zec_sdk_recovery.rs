use async_trait::async_trait;
use lez_swap_core::{Participant, Phase, SwapDirection, SwapId, UnixSeconds};
use lez_swap_store::SqliteZecRecoveryStore;
use lez_zec_swap_sdk::{
    AcceptedZecAgreementV1, Bip199Contract, CanonicalLezEscrowObservationV1,
    CanonicalLezEscrowRemovalV1, CanonicalZcashOutputObservation, CanonicalZcashOutputRemoval,
    CreateFirstLockOutcome, ExpectedBip199Output, FirstLockConfirmedEvidenceV1,
    FirstLockDriveOutcome, FirstLockObservation, FirstLockPlanV1, FirstLockProjectionCommit,
    FirstLockStepV1, LezAssetV1, LezChainIdentityV1, LezCustodySnapshotV1, LezEnvironmentV1,
    LezEscrowMetadataSnapshotV1, LezEscrowStatusV1, LezFirstLockPort, LezFundInstructionV1,
    LezFundTransactionSnapshotV1, LezInclusionStatusV1, LezNodeRemovalSnapshotV1,
    LezNodeSnapshotV1, LezObservationTrackerError, LezStableTipV1,
    LezTakerFirstLockObservationPort, MakerFundingEligibilityOutcome, MakerLockDriveOutcome,
    NegotiationChannel, NegotiationTranscriptV1, ObserveTakerFirstLockOutcome,
    ObservedTakerFirstLockTransitionRecordV1, OfferDiscovery, PreparedFirstLockSubmissionV1,
    RecoveryStore, TakerFirstLockObservationV1, TransparentFundingRequest, TransparentUtxo,
    ZEC_CONCRETE_AGREEMENT_SCHEMA_V2, ZcashFirstLockPort, ZcashNodeRemovalSnapshot,
    ZcashNodeSnapshot, ZcashObservationEventRecordV1, ZcashStableTip,
    ZcashTakerFirstLockObservationPort, ZcashTransparentDestinationV1, ZecAgreementBodyV1,
    ZecAgreementRecordV1, ZecLezTermsV1, ZecPairSdk, ZecParticipantIdentityV1, ZecParticipantsV1,
    ZecProfileId, ZecProfileRecordV1, ZecRefundPlanV1, ZecSdkError, ZecSwapBinding,
    ZecSwapBindingRecordV1, ZecTransactionPolicyV1, build_funding_transaction,
    derive_lez_metadata_account_v1, derive_lez_native_custody_account_v1, derive_lez_swap_id_v1,
};
use rusqlite::{Connection, OptionalExtension, params};
use secp256k1::{Message, PublicKey, Secp256k1, SecretKey};
use tempfile::TempDir;
use zcash_primitives::block::BlockHash;
use zcash_protocol::{
    consensus::{BlockHeight, BranchId, NetworkType},
    value::Zatoshis,
};
use zcash_transparent::{
    address::{Script, TransparentAddress},
    bundle::{OutPoint, TxOut},
};

const ACCEPTED_AT: UnixSeconds = UnixSeconds::new(10);

#[derive(Clone, Copy, Debug)]
struct NoDiscovery;

#[async_trait]
impl OfferDiscovery for NoDiscovery {
    type Error = TestPortError;
    type Offer = ();
    type OfferRef = ();
    type Query = ();

    async fn publish(&self, _offer: Self::Offer) -> Result<Self::OfferRef, Self::Error> {
        Ok(())
    }

    async fn discover(&self, _query: &Self::Query) -> Result<Vec<Self::OfferRef>, Self::Error> {
        Ok(Vec::new())
    }
}

#[derive(Clone, Debug)]
struct FixedNegotiation;

#[async_trait]
impl NegotiationChannel for FixedNegotiation {
    type Error = TestPortError;
    type LocalProposal = ();
    type OfferRef = ();

    async fn negotiate(
        &self,
        _local_participant: Participant,
        _offer: &Self::OfferRef,
        _proposal: Self::LocalProposal,
    ) -> Result<Vec<u8>, Self::Error> {
        unreachable!("these persistence tests start from a locally validated agreement")
    }
}

#[derive(Clone, Copy, Debug)]
struct NoChain;

#[derive(Clone, Debug)]
struct MakerObservation(TakerFirstLockObservationV1);

#[async_trait]
impl LezTakerFirstLockObservationPort for MakerObservation {
    type Error = TestPortError;

    async fn observe_taker_first_lock(
        &self,
        _agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        _previous: Option<&CanonicalLezEscrowObservationV1>,
    ) -> Result<TakerFirstLockObservationV1, Self::Error> {
        Ok(self.0.clone())
    }
}

#[async_trait]
impl ZcashTakerFirstLockObservationPort for MakerObservation {
    type Error = TestPortError;

    async fn observe_taker_first_lock(
        &self,
        _agreement: &lez_zec_swap_sdk::ZecAgreementV1,
    ) -> Result<TakerFirstLockObservationV1, Self::Error> {
        Ok(self.0.clone())
    }
}

#[derive(Clone, Debug)]
struct MakerHappyPort {
    taker_observation: TakerFirstLockObservationV1,
    submitted: std::sync::Arc<std::sync::Mutex<Vec<FirstLockStepV1>>>,
}

impl MakerHappyPort {
    fn new(taker_observation: TakerFirstLockObservationV1) -> Self {
        Self {
            taker_observation,
            submitted: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    fn first_lock_observation(
        &self,
        submission: &PreparedFirstLockSubmissionV1,
    ) -> FirstLockObservation {
        if self
            .submitted
            .lock()
            .expect("submitted-step lock")
            .contains(&submission.step())
        {
            FirstLockObservation::Confirmed
        } else {
            FirstLockObservation::Absent
        }
    }

    fn record_submission(&self, submission: &PreparedFirstLockSubmissionV1) {
        let mut submitted = self.submitted.lock().expect("submitted-step lock");
        if !submitted.contains(&submission.step()) {
            submitted.push(submission.step());
        }
    }

    fn submitted_steps(&self) -> Vec<FirstLockStepV1> {
        self.submitted.lock().expect("submitted-step lock").clone()
    }
}

#[async_trait]
impl LezTakerFirstLockObservationPort for MakerHappyPort {
    type Error = TestPortError;

    async fn observe_taker_first_lock(
        &self,
        _agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        _previous: Option<&CanonicalLezEscrowObservationV1>,
    ) -> Result<TakerFirstLockObservationV1, Self::Error> {
        Ok(self.taker_observation.clone())
    }
}

#[async_trait]
impl ZcashTakerFirstLockObservationPort for MakerHappyPort {
    type Error = TestPortError;

    async fn observe_taker_first_lock(
        &self,
        _agreement: &lez_zec_swap_sdk::ZecAgreementV1,
    ) -> Result<TakerFirstLockObservationV1, Self::Error> {
        Ok(self.taker_observation.clone())
    }
}

#[async_trait]
impl LezFirstLockPort for MakerHappyPort {
    type Error = TestPortError;

    async fn observe_first_lock(
        &self,
        _agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<FirstLockObservation, Self::Error> {
        Ok(self.first_lock_observation(submission))
    }

    async fn submit_first_lock(
        &self,
        _agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<(), Self::Error> {
        self.record_submission(submission);
        Ok(())
    }
}

#[async_trait]
impl ZcashFirstLockPort for MakerHappyPort {
    type Error = TestPortError;

    async fn observe_first_lock(
        &self,
        _agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<FirstLockObservation, Self::Error> {
        Ok(self.first_lock_observation(submission))
    }

    async fn submit_first_lock(
        &self,
        _agreement: &lez_zec_swap_sdk::ZecAgreementV1,
        submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<(), Self::Error> {
        self.record_submission(submission);
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{0}")]
struct TestPortError(&'static str);

type TestSdk = ZecPairSdk<NoDiscovery, FixedNegotiation, NoChain, NoChain, SqliteZecRecoveryStore>;

#[tokio::test]
async fn agreement_replay_conflict_and_same_swap_role_isolation_are_durable() {
    let data = TempDir::new().expect("temporary store");
    let path = data.path().join("recovery.sqlite3");
    let id = "sqlite-role-isolation";
    let wire = agreement_wire(id, FixtureVariant::Local);

    let taker_store = SqliteZecRecoveryStore::open(&path, Participant::Taker)
        .expect("open role-fixed taker store");
    let taker = sdk(Participant::Taker, taker_store.clone());
    let accepted = accept(&wire, Participant::Taker);
    taker
        .activate(accepted.clone())
        .await
        .expect("first agreement create");
    taker
        .activate(accepted)
        .await
        .expect("exact agreement replay");

    let changed = accept(
        &agreement_wire(id, FixtureVariant::ChangedTranscript),
        Participant::Taker,
    );
    assert!(matches!(
        taker.activate(changed).await,
        Err(ZecSdkError::AgreementConflict)
    ));

    let maker_store = SqliteZecRecoveryStore::open(&path, Participant::Maker)
        .expect("open independent maker view");
    sdk(Participant::Maker, maker_store.clone())
        .activate(accept(&wire, Participant::Maker))
        .await
        .expect("same application swap ID is isolated by role");

    let raw = Connection::open(&path).expect("inspect durable roles");
    let roles: i64 = raw
        .query_row(
            "SELECT COUNT(*) FROM zec_sdk_agreements WHERE swap_id = ?1",
            params![id],
            |row| row.get(0),
        )
        .expect("role rows");
    assert_eq!(roles, 2);
    let swap_id = SwapId::new(id).expect("swap ID");
    assert_eq!(
        taker_store
            .load_agreement(&swap_id)
            .await
            .expect("taker agreement")
            .expect("taker row")
            .local_participant(),
        Participant::Taker
    );
    assert_eq!(
        maker_store
            .load_agreement(&swap_id)
            .await
            .expect("maker agreement")
            .expect("maker row")
            .local_participant(),
        Participant::Maker
    );
}

#[tokio::test]
async fn first_lock_intent_transition_and_closed_recovery_survive_reopen() {
    let data = TempDir::new().expect("temporary store");
    let path = data.path().join("recovery.sqlite3");
    let id = "sqlite-first-lock-restart";
    let store =
        SqliteZecRecoveryStore::open(&path, Participant::Taker).expect("open role-fixed store");
    let pair_sdk = sdk(Participant::Taker, store.clone());
    let mut active = pair_sdk
        .activate(accept(
            &agreement_wire(id, FixtureVariant::Local),
            Participant::Taker,
        ))
        .await
        .expect("activation");

    assert_eq!(
        active
            .stage_first_lock(zcash_plan([0x31; 32], vec![0x51, 0x52]))
            .await
            .expect("intent is durable before effects"),
        CreateFirstLockOutcome::Created
    );
    let raw = Connection::open(&path).expect("inspect staged intent");
    let (intent_count, open_count): (i64, i64) = raw
        .query_row(
            "SELECT COUNT(*), SUM(closed_revision IS NULL) \
             FROM zec_sdk_first_lock_intents \
             WHERE local_role = 'taker' AND swap_id = ?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("durable pre-effect intent");
    assert_eq!((intent_count, open_count), (1, 1));

    let commit = active
        .project_first_lock(confirmed([0x31; 32], "zcash-first-lock"))
        .await
        .expect("atomic first-lock projection");
    assert_eq!(commit, FirstLockProjectionCommit::new(1, false));
    assert_eq!(active.status(), Phase::TakerLockConfirmed);
    assert_eq!(active.revision(), 1);

    let (closed_revision, transition_count, active_revision): (i64, i64, i64) = raw
        .query_row(
            "SELECT i.closed_revision, \
                    (SELECT COUNT(*) FROM zec_sdk_first_lock_transitions t \
                     WHERE t.local_role = i.local_role AND t.swap_id = i.swap_id), \
                    a.active_revision \
             FROM zec_sdk_first_lock_intents i \
             JOIN zec_sdk_agreements a USING (local_role, swap_id) \
             WHERE i.local_role = 'taker' AND i.swap_id = ?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("closed intent and transition remain durable together");
    assert_eq!(
        (closed_revision, transition_count, active_revision),
        (1, 1, 1)
    );

    let swap_id = SwapId::new(id).expect("swap ID");
    assert!(
        store
            .load_first_lock_intent(&swap_id)
            .await
            .expect("closed intent lookup")
            .is_none(),
        "closed intent must not be presented as pending"
    );
    let transition = store
        .load_first_lock_transition(&swap_id, 0)
        .await
        .expect("transition lookup")
        .expect("transition retained");
    assert_eq!(
        store
            .commit_first_lock_transition(&transition)
            .await
            .expect("exact transition replay"),
        FirstLockProjectionCommit::new(1, true)
    );

    drop(active);
    drop(pair_sdk);
    drop(store);
    let reopened =
        SqliteZecRecoveryStore::open(&path, Participant::Taker).expect("reopen role-fixed store");
    let resumed = sdk(Participant::Taker, reopened)
        .resume(&swap_id)
        .await
        .expect("resume succeeds")
        .expect("agreement remains durable");
    assert_eq!(resumed.status(), Phase::TakerLockConfirmed);
    assert_eq!(resumed.revision(), 1);
    drop(resumed);
    assert_orphan_future_taker_transition_fails_closed(&path, id, &swap_id).await;
}

async fn assert_orphan_future_taker_transition_fails_closed(
    path: &std::path::Path,
    id: &str,
    swap_id: &SwapId,
) {
    Connection::open(path)
        .expect("inject orphan taker transition")
        .execute(
            "INSERT INTO zec_sdk_first_lock_transitions (
                local_role, swap_id, predecessor_revision, committed_revision,
                payload_version, payload_json
             )
             SELECT local_role, swap_id, 1, 2, payload_version, payload_json
             FROM zec_sdk_first_lock_transitions
             WHERE local_role = 'taker' AND swap_id = ?1 AND predecessor_revision = 0",
            params![id],
        )
        .expect("inject future taker row without aggregate advance");
    let corrupt =
        SqliteZecRecoveryStore::open(path, Participant::Taker).expect("reopen orphan fixture");
    assert!(matches!(
        sdk(Participant::Taker, corrupt).resume(swap_id).await,
        Err(ZecSdkError::Persistence(_))
    ));
}

#[tokio::test]
async fn transition_trigger_failure_rolls_back_revision_transition_and_intent_close() {
    let data = TempDir::new().expect("temporary store");
    let path = data.path().join("rollback.sqlite3");
    let id = "sqlite-first-lock-rollback";
    let store = SqliteZecRecoveryStore::open(&path, Participant::Taker).expect("open store");
    let sdk = sdk(Participant::Taker, store);
    let mut active = sdk
        .activate(accept(
            &agreement_wire(id, FixtureVariant::Local),
            Participant::Taker,
        ))
        .await
        .expect("activation");
    active
        .stage_first_lock(zcash_plan([0x41; 32], vec![0x61]))
        .await
        .expect("staged intent");

    let raw = Connection::open(&path).expect("external fault injector");
    raw.execute_batch(
        "CREATE TRIGGER fail_zec_sdk_revision \
         BEFORE UPDATE OF active_revision ON zec_sdk_agreements \
         BEGIN SELECT RAISE(ABORT, 'forced SDK transition rollback'); END;",
    )
    .expect("install deterministic external failure");
    assert!(matches!(
        active
            .project_first_lock(confirmed([0x41; 32], "rollback-first-lock"))
            .await,
        Err(ZecSdkError::Persistence(_))
    ));
    assert_eq!(active.status(), Phase::Offered);
    assert_eq!(active.revision(), 0);

    let (active_revision, closed_revision, transitions): (i64, Option<i64>, i64) = raw
        .query_row(
            "SELECT a.active_revision, i.closed_revision, \
                    (SELECT COUNT(*) FROM zec_sdk_first_lock_transitions t \
                     WHERE t.local_role = a.local_role AND t.swap_id = a.swap_id) \
             FROM zec_sdk_agreements a \
             JOIN zec_sdk_first_lock_intents i USING (local_role, swap_id) \
             WHERE a.local_role = 'taker' AND a.swap_id = ?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("inspect rolled-back unit");
    assert_eq!(
        (active_revision, closed_revision, transitions),
        (0, None, 0)
    );

    raw.execute_batch("DROP TRIGGER fail_zec_sdk_revision;")
        .expect("remove fault");
    assert_eq!(
        active
            .project_first_lock(confirmed([0x41; 32], "rollback-first-lock"))
            .await
            .expect("retry commits the entire unit"),
        FirstLockProjectionCommit::new(1, false)
    );
}

#[tokio::test]
async fn future_and_corrupt_recovery_payloads_fail_closed() {
    let future_data = TempDir::new().expect("future store");
    let future_path = future_data.path().join("future.sqlite3");
    let future_id = "sqlite-future-agreement";
    let future_store = SqliteZecRecoveryStore::open(&future_path, Participant::Taker)
        .expect("open future fixture");
    let future_sdk = sdk(Participant::Taker, future_store);
    future_sdk
        .activate(accept(
            &agreement_wire(future_id, FixtureVariant::Local),
            Participant::Taker,
        ))
        .await
        .expect("seed agreement");
    Connection::open(&future_path)
        .expect("corrupt future fixture")
        .execute(
            "UPDATE zec_sdk_agreements SET payload_version = 99 \
             WHERE local_role = 'taker' AND swap_id = ?1",
            params![future_id],
        )
        .expect("write future payload version");
    assert!(matches!(
        future_sdk
            .resume(&SwapId::new(future_id).expect("swap ID"))
            .await,
        Err(ZecSdkError::Persistence(_))
    ));

    let corrupt_data = TempDir::new().expect("corrupt store");
    let corrupt_path = corrupt_data.path().join("corrupt.sqlite3");
    let corrupt_id = "sqlite-corrupt-intent";
    let corrupt_store = SqliteZecRecoveryStore::open(&corrupt_path, Participant::Taker)
        .expect("open corrupt fixture");
    let corrupt_sdk = sdk(Participant::Taker, corrupt_store);
    let mut active = corrupt_sdk
        .activate(accept(
            &agreement_wire(corrupt_id, FixtureVariant::Local),
            Participant::Taker,
        ))
        .await
        .expect("seed agreement");
    active
        .stage_first_lock(zcash_plan([0x51; 32], vec![0x71]))
        .await
        .expect("seed intent");
    Connection::open(&corrupt_path)
        .expect("corrupt intent fixture")
        .execute(
            "UPDATE zec_sdk_first_lock_intents SET payload_json = '{' \
             WHERE local_role = 'taker' AND swap_id = ?1",
            params![corrupt_id],
        )
        .expect("write malformed primitive payload");
    assert!(matches!(
        active
            .project_first_lock(confirmed([0x51; 32], "corrupt-first-lock"))
            .await,
        Err(ZecSdkError::Persistence(_))
    ));

    let raw = Connection::open(&corrupt_path).expect("inspect corrupt fixture");
    let transition: Option<i64> = raw
        .query_row(
            "SELECT committed_revision FROM zec_sdk_first_lock_transitions \
             WHERE local_role = 'taker' AND swap_id = ?1",
            params![corrupt_id],
            |row| row.get(0),
        )
        .optional()
        .expect("transition lookup");
    assert_eq!(transition, None);
}

#[tokio::test]
async fn active_revision_without_its_transition_fails_closed_on_resume() {
    let data = TempDir::new().expect("torn store");
    let path = data.path().join("torn.sqlite3");
    let id = "sqlite-torn-active-revision";
    let store = SqliteZecRecoveryStore::open(&path, Participant::Taker).expect("open torn fixture");
    let pair_sdk = sdk(Participant::Taker, store);
    pair_sdk
        .activate(accept(
            &agreement_wire(id, FixtureVariant::Local),
            Participant::Taker,
        ))
        .await
        .expect("seed agreement");
    Connection::open(&path)
        .expect("tear active revision")
        .execute(
            "UPDATE zec_sdk_agreements SET active_revision = 1
             WHERE local_role = 'taker' AND swap_id = ?1",
            params![id],
        )
        .expect("simulate a missing transition");

    assert!(matches!(
        pair_sdk.resume(&SwapId::new(id).expect("swap ID")).await,
        Err(ZecSdkError::Persistence(_))
    ));
}

#[tokio::test]
async fn closed_intent_without_its_transition_fails_closed_on_resume() {
    let data = TempDir::new().expect("torn store");
    let path = data.path().join("closed-intent.sqlite3");
    let id = "sqlite-torn-closed-intent";
    let store = SqliteZecRecoveryStore::open(&path, Participant::Taker).expect("open torn fixture");
    let pair_sdk = sdk(Participant::Taker, store);
    let active = pair_sdk
        .activate(accept(
            &agreement_wire(id, FixtureVariant::Local),
            Participant::Taker,
        ))
        .await
        .expect("seed agreement");
    active
        .stage_first_lock(zcash_plan([0x61; 32], vec![0x81]))
        .await
        .expect("seed open intent");
    Connection::open(&path)
        .expect("tear closed intent")
        .execute(
            "UPDATE zec_sdk_first_lock_intents SET closed_revision = 1
             WHERE local_role = 'taker' AND swap_id = ?1",
            params![id],
        )
        .expect("simulate a missing transition");

    assert!(matches!(
        pair_sdk.resume(&SwapId::new(id).expect("swap ID")).await,
        Err(ZecSdkError::Persistence(_))
    ));
}

#[tokio::test]
async fn maker_observation_is_role_local_and_survives_sqlite_reopen() {
    let data = TempDir::new().expect("maker observation store");
    let path = data.path().join("maker-observation.sqlite3");
    let id = "sqlite-maker-observes-taker";
    let accepted = accept(
        &agreement_wire(id, FixtureVariant::Local),
        Participant::Maker,
    );
    let observation = MakerObservation(TakerFirstLockObservationV1::CanonicalZcash(Box::new(
        canonical_zcash_taker_lock(accepted.agreement()),
    )));
    let canonical = match &observation.0 {
        TakerFirstLockObservationV1::CanonicalZcash(value) => value.as_ref().clone(),
        _ => unreachable!("canonical fixture"),
    };
    assert_initial_maker_observation_persists(&path, id, &accepted, observation).await;
    assert_store_rejects_unproved_canonical_replacement(&path, id, &accepted).await;
    let reopened =
        SqliteZecRecoveryStore::open(&path, Participant::Maker).expect("reopen maker store");
    let resumed = ZecPairSdk::new(
        Participant::Maker,
        NoDiscovery,
        FixedNegotiation,
        MakerObservation(TakerFirstLockObservationV1::Absent),
        MakerObservation(TakerFirstLockObservationV1::Absent),
        reopened,
    )
    .resume(&SwapId::new(id).expect("swap ID"))
    .await
    .expect("maker resume")
    .expect("maker agreement");
    assert_eq!(resumed.status(), Phase::TakerLockConfirmed);
    assert_eq!(resumed.revision(), 1);

    drop(resumed);
    assert_fresh_eligibility_after_reopen(&path, id, canonical.clone()).await;
    let removed = canonical_zcash_removal(&canonical);
    let replacement = canonical_zcash_replacement(accepted.agreement(), &removed);
    commit_maker_observation_after_reopen(
        &path,
        id,
        TakerFirstLockObservationV1::ZcashReplaced {
            removed: Box::new(removed),
            canonical: Box::new(replacement),
        },
        Phase::TakerLockConfirmed,
        2,
    )
    .await;
    let deeper = canonical_zcash_replacement_depth_update(accepted.agreement());
    commit_maker_observation_after_reopen(
        &path,
        id,
        TakerFirstLockObservationV1::CanonicalZcash(Box::new(deeper.clone())),
        Phase::TakerLockConfirmed,
        3,
    )
    .await;
    commit_maker_observation_after_reopen(
        &path,
        id,
        TakerFirstLockObservationV1::ZcashRemoved(Box::new(canonical_zcash_removal(&deeper))),
        Phase::Offered,
        4,
    )
    .await;

    let removal_replay =
        SqliteZecRecoveryStore::open(&path, Participant::Maker).expect("reopen removed state");
    let removed = ZecPairSdk::new(
        Participant::Maker,
        NoDiscovery,
        FixedNegotiation,
        MakerObservation(TakerFirstLockObservationV1::Absent),
        MakerObservation(TakerFirstLockObservationV1::Absent),
        removal_replay,
    )
    .resume(&SwapId::new(id).expect("swap ID"))
    .await
    .expect("replay removal")
    .expect("maker agreement");
    assert_eq!(removed.status(), Phase::Offered);
    assert_eq!(removed.revision(), 4);
    drop(removed);
    assert_orphan_future_maker_transition_fails_closed(&path, id).await;
}

#[tokio::test]
async fn maker_second_lock_happy_path_survives_sqlite_reopen_in_both_directions() {
    for (id, direction) in [
        (
            "sqlite-maker-lock-zcash-to-lez",
            SwapDirection::TakerSellsForeign,
        ),
        (
            "sqlite-maker-lock-lez-to-zcash",
            SwapDirection::TakerSellsLez,
        ),
    ] {
        let data = TempDir::new().expect("maker-lock store");
        let path = data.path().join("maker-lock.sqlite3");
        assert_sqlite_maker_lock_happy_path(&path, id, direction).await;
    }
}

async fn assert_sqlite_maker_lock_happy_path(
    path: &std::path::Path,
    id: &str,
    direction: SwapDirection,
) {
    let wire = agreement_wire_direction(id, FixtureVariant::Local, direction);
    let accepted = accept(&wire, Participant::Maker);
    let taker_observation = match direction {
        SwapDirection::TakerSellsForeign => TakerFirstLockObservationV1::CanonicalZcash(Box::new(
            canonical_zcash_taker_lock(accepted.agreement()),
        )),
        SwapDirection::TakerSellsLez => TakerFirstLockObservationV1::CanonicalLez(Box::new(
            canonical_lez_taker_lock(accepted.agreement()),
        )),
    };
    let chain = MakerHappyPort::new(taker_observation);
    let store =
        SqliteZecRecoveryStore::open(path, Participant::Maker).expect("open maker-lock store");
    let mut active = ZecPairSdk::new(
        Participant::Maker,
        NoDiscovery,
        FixedNegotiation,
        chain.clone(),
        chain.clone(),
        store.clone(),
    )
    .activate(accepted)
    .await
    .expect("activate maker");
    assert!(matches!(
        active.observe_taker_first_lock().await,
        Ok(ObserveTakerFirstLockOutcome::Projected(_))
    ));
    assert_eq!(
        (active.status(), active.revision()),
        (Phase::TakerLockConfirmed, 1)
    );
    let mut stale = ZecPairSdk::new(
        Participant::Maker,
        NoDiscovery,
        FixedNegotiation,
        chain.clone(),
        chain.clone(),
        store,
    )
    .resume(&SwapId::new(id).expect("swap ID"))
    .await
    .expect("stale maker resume")
    .expect("stale maker agreement");
    let (plan, expected_drives, evidence) = maker_lock_fixture(direction);
    let mut actual_drives = Vec::new();
    for _ in 0..expected_drives.len() {
        let MakerLockDriveOutcome::Lock(outcome) = active
            .drive_maker_lock(plan.clone())
            .await
            .expect("fresh eligible maker drive")
        else {
            panic!("canonical taker lock must remain maker-eligible")
        };
        actual_drives.push(outcome);
    }
    assert_eq!(actual_drives, expected_drives);
    active
        .project_maker_lock(evidence)
        .await
        .expect("commit confirmed maker lock");
    assert_eq!(
        (active.status(), active.revision()),
        (Phase::BothLegsLocked, 2)
    );
    let submitted = chain.submitted_steps();
    assert_eq!(
        stale
            .drive_maker_lock(plan)
            .await
            .expect("stale maker replays committed funding"),
        MakerLockDriveOutcome::AlreadyLocked { revision: 2 }
    );
    assert_eq!(
        (stale.status(), stale.revision()),
        (Phase::BothLegsLocked, 2)
    );
    assert_eq!(chain.submitted_steps(), submitted);
    drop(active);
    assert_closed_maker_lock_rows(path, id);
    assert_maker_lock_reopens(path, id).await;
}

fn maker_lock_fixture(
    direction: SwapDirection,
) -> (
    FirstLockPlanV1,
    Vec<FirstLockDriveOutcome>,
    FirstLockConfirmedEvidenceV1,
) {
    match direction {
        SwapDirection::TakerSellsForeign => {
            let initialize = PreparedFirstLockSubmissionV1::new(
                FirstLockStepV1::LezInitialize,
                [0x71; 32],
                vec![0xa1],
            )
            .expect("LEZ initialize");
            let fund = PreparedFirstLockSubmissionV1::new(
                FirstLockStepV1::LezFund,
                [0x72; 32],
                vec![0xa2],
            )
            .expect("LEZ fund");
            (
                FirstLockPlanV1::lez(initialize, fund).expect("maker LEZ plan"),
                vec![
                    FirstLockDriveOutcome::Submitted(FirstLockStepV1::LezInitialize),
                    FirstLockDriveOutcome::Submitted(FirstLockStepV1::LezFund),
                    FirstLockDriveOutcome::ReadyForFundingProjection,
                ],
                FirstLockConfirmedEvidenceV1::new(
                    FirstLockStepV1::LezFund,
                    [0x72; 32],
                    "sqlite-maker-lez-fund",
                    100,
                )
                .expect("maker LEZ evidence"),
            )
        }
        SwapDirection::TakerSellsLez => (
            zcash_plan([0x81; 32], vec![0xb1]),
            vec![
                FirstLockDriveOutcome::Submitted(FirstLockStepV1::ZcashFund),
                FirstLockDriveOutcome::ReadyForFundingProjection,
            ],
            confirmed([0x81; 32], "sqlite-maker-zcash-fund"),
        ),
    }
}

fn assert_closed_maker_lock_rows(path: &std::path::Path, id: &str) {
    let raw = Connection::open(path).expect("inspect maker-lock rows");
    let row: (i64, i64, i64, i64) = raw
        .query_row(
            "SELECT i.staged_revision, i.closed_revision,
                    t.predecessor_revision, t.committed_revision
             FROM zec_sdk_maker_lock_intents i
             JOIN zec_sdk_maker_lock_transitions t USING (local_role, swap_id)
             WHERE i.local_role = 'maker' AND i.swap_id = ?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("closed maker-lock journal");
    assert_eq!(row, (1, 2, 1, 2));
}

async fn assert_maker_lock_reopens(path: &std::path::Path, id: &str) {
    let reopened =
        SqliteZecRecoveryStore::open(path, Participant::Maker).expect("reopen maker-lock store");
    let absent = MakerHappyPort::new(TakerFirstLockObservationV1::Absent);
    let resumed = ZecPairSdk::new(
        Participant::Maker,
        NoDiscovery,
        FixedNegotiation,
        absent.clone(),
        absent,
        reopened,
    )
    .resume(&SwapId::new(id).expect("swap ID"))
    .await
    .expect("replay maker-lock journal")
    .expect("durable agreement");
    assert_eq!(
        (resumed.status(), resumed.revision()),
        (Phase::BothLegsLocked, 2)
    );
}

#[tokio::test]
async fn maker_lock_trigger_failure_rolls_back_transition_revision_and_intent_close() {
    let id = "sqlite-maker-lock-rollback";
    let data = TempDir::new().expect("maker-lock rollback store");
    let path = data.path().join("maker-lock-rollback.sqlite3");
    let accepted = accept(
        &agreement_wire_direction(id, FixtureVariant::Local, SwapDirection::TakerSellsForeign),
        Participant::Maker,
    );
    let chain = MakerHappyPort::new(TakerFirstLockObservationV1::CanonicalZcash(Box::new(
        canonical_zcash_taker_lock(accepted.agreement()),
    )));
    let store = SqliteZecRecoveryStore::open(&path, Participant::Maker)
        .expect("open maker-lock rollback store");
    let mut active = ZecPairSdk::new(
        Participant::Maker,
        NoDiscovery,
        FixedNegotiation,
        chain.clone(),
        chain,
        store,
    )
    .activate(accepted)
    .await
    .expect("activate maker");
    active
        .observe_taker_first_lock()
        .await
        .expect("canonical taker funding");
    active
        .drive_maker_lock(
            FirstLockPlanV1::lez(
                PreparedFirstLockSubmissionV1::new(
                    FirstLockStepV1::LezInitialize,
                    [0xb1; 32],
                    vec![0xe1],
                )
                .expect("initialize"),
                PreparedFirstLockSubmissionV1::new(
                    FirstLockStepV1::LezFund,
                    [0xb2; 32],
                    vec![0xe2],
                )
                .expect("fund"),
            )
            .expect("maker plan"),
        )
        .await
        .expect("durable maker intent");
    let evidence = FirstLockConfirmedEvidenceV1::new(
        FirstLockStepV1::LezFund,
        [0xb2; 32],
        "sqlite-maker-lock-rollback",
        100,
    )
    .expect("maker evidence");

    let raw = Connection::open(&path).expect("open fault injector");
    raw.execute_batch(
        "CREATE TRIGGER fail_maker_intent_close
         BEFORE UPDATE OF closed_revision ON zec_sdk_maker_lock_intents
         WHEN NEW.closed_revision IS NOT NULL
         BEGIN
             SELECT RAISE(FAIL, 'forced maker intent close failure');
         END;",
    )
    .expect("install maker close trigger");
    assert!(active.project_maker_lock(evidence.clone()).await.is_err());
    assert_eq!(
        (active.status(), active.revision()),
        (Phase::TakerLockConfirmed, 1)
    );
    let state: (i64, i64, i64) = raw
        .query_row(
            "SELECT a.active_revision,
                    (SELECT COUNT(*) FROM zec_sdk_maker_lock_transitions t
                     WHERE t.local_role = a.local_role AND t.swap_id = a.swap_id),
                    (SELECT COUNT(*) FROM zec_sdk_maker_lock_intents i
                     WHERE i.local_role = a.local_role AND i.swap_id = a.swap_id
                       AND i.closed_revision IS NULL)
             FROM zec_sdk_agreements a
             WHERE a.local_role = 'maker' AND a.swap_id = ?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("inspect rolled-back maker state");
    assert_eq!(state, (1, 0, 1));

    raw.execute_batch("DROP TRIGGER fail_maker_intent_close;")
        .expect("remove maker close trigger");
    active
        .project_maker_lock(evidence)
        .await
        .expect("retry maker projection");
    assert_eq!(
        (active.status(), active.revision()),
        (Phase::BothLegsLocked, 2)
    );
}

#[tokio::test]
async fn canonical_lez_maker_observation_survives_sqlite_close_and_reopen() {
    let data = TempDir::new().expect("reverse maker observation store");
    let path = data.path().join("reverse-maker-observation.sqlite3");
    let id = "sqlite-maker-observes-lez";
    let accepted = accept(
        &agreement_wire_direction_with_lez_amount(
            id,
            FixtureVariant::Local,
            SwapDirection::TakerSellsLez,
            u128::from(u64::MAX) + 1,
        ),
        Participant::Maker,
    );
    let canonical = canonical_lez_taker_lock(accepted.agreement());
    let observation = MakerObservation(TakerFirstLockObservationV1::CanonicalLez(Box::new(
        canonical.clone(),
    )));
    let store =
        SqliteZecRecoveryStore::open(&path, Participant::Maker).expect("open reverse maker store");
    let mut active = ZecPairSdk::new(
        Participant::Maker,
        NoDiscovery,
        FixedNegotiation,
        observation.clone(),
        MakerObservation(TakerFirstLockObservationV1::Absent),
        store,
    )
    .activate(accepted.clone())
    .await
    .expect("activate reverse maker");
    let initial = active.observe_taker_first_lock().await;
    assert!(
        matches!(initial, Ok(ObserveTakerFirstLockOutcome::Projected(_))),
        "large-amount initial projection failed: {initial:?}"
    );
    assert_eq!(active.status(), Phase::TakerLockConfirmed);
    drop(active);

    assert_eq!(
        reverse_maker_state_after_reopen(&path, id).await,
        (Phase::TakerLockConfirmed, 1),
        "current payload preserves a valid LEZ amount above u64"
    );
    let (eligible, phase, revision) = refresh_reverse_maker_after_reopen(
        &path,
        id,
        TakerFirstLockObservationV1::CanonicalLez(Box::new(canonical.clone())),
    )
    .await;
    assert_eq!(
        eligible,
        MakerFundingEligibilityOutcome::Eligible { revision: 1 }
    );
    assert_eq!((phase, revision), (Phase::TakerLockConfirmed, 1));
    assert_eq!(maker_transition_count(&path, id), 1);
    rewrite_lez_row_as_legacy_payload_v1(&path, id, *accepted.agreement().onchain_swap_id());
    let (duplicate, phase, revision) = poll_reverse_maker_after_reopen(&path, id, canonical).await;
    assert_eq!(
        duplicate,
        ObserveTakerFirstLockOutcome::Unchanged(FirstLockStepV1::LezFund)
    );
    assert_eq!((phase, revision), (Phase::TakerLockConfirmed, 1));
    assert_eq!(maker_transition_count(&path, id), 1);

    let deeper = canonical_lez_taker_lock_at(
        accepted.agreement(),
        LezInclusionStatusV1::Safe,
        [0x44; 32],
        103,
    );
    let (update, phase, revision) =
        poll_reverse_maker_after_reopen(&path, id, deeper.clone()).await;
    assert!(matches!(update, ObserveTakerFirstLockOutcome::Projected(_)));
    assert_eq!((phase, revision), (Phase::TakerLockConfirmed, 2));
    assert_lez_replacement_and_removal(&path, id, accepted.agreement(), &deeper).await;
}

async fn assert_lez_replacement_and_removal(
    path: &std::path::Path,
    id: &str,
    agreement: &lez_zec_swap_sdk::ZecAgreementV1,
    previous: &CanonicalLezEscrowObservationV1,
) {
    let removal = canonical_lez_removal_at(agreement, previous, [0x55; 32], [0x56; 32], 104);
    let replacement = canonical_lez_taker_lock_with_inclusion(
        agreement,
        LezInclusionStatusV1::Safe,
        [0x56; 32],
        104,
        [0x32; 32],
        101,
        [0x51; 32],
    );
    let replacement_observation = TakerFirstLockObservationV1::LezReplaced {
        removed: Box::new(removal.clone()),
        canonical: Box::new(replacement.clone()),
    };
    let (replaced, phase, revision) =
        poll_reverse_maker_observation_after_reopen(path, id, replacement_observation.clone())
            .await;
    assert!(matches!(
        replaced,
        ObserveTakerFirstLockOutcome::Projected(_)
    ));
    assert_eq!((phase, revision), (Phase::TakerLockConfirmed, 3));
    let (duplicate, phase, revision) =
        refresh_reverse_maker_after_reopen(path, id, replacement_observation).await;
    assert_eq!(
        duplicate,
        MakerFundingEligibilityOutcome::Eligible { revision: 3 }
    );
    assert_eq!((phase, revision), (Phase::TakerLockConfirmed, 3));
    assert_eq!(maker_transition_count(path, id), 3);
    let (stale, phase, revision) = try_poll_reverse_maker_observation_after_reopen(
        path,
        id,
        TakerFirstLockObservationV1::LezRemoved(Box::new(removal)),
    )
    .await;
    assert!(matches!(
        stale,
        Err(ZecSdkError::InvalidLezObservationHistory(
            LezObservationTrackerError::StaleEvidence
        ))
    ));
    assert_eq!((phase, revision), (Phase::TakerLockConfirmed, 3));
    assert_eq!(maker_transition_count(path, id), 3);

    let removal = canonical_lez_removal_at(agreement, &replacement, [0x57; 32], [0x58; 32], 105);
    let removal_observation = TakerFirstLockObservationV1::LezRemoved(Box::new(removal));
    let (removed, phase, revision) =
        poll_reverse_maker_observation_after_reopen(path, id, removal_observation.clone()).await;
    assert!(matches!(
        removed,
        ObserveTakerFirstLockOutcome::Projected(_)
    ));
    assert_eq!((phase, revision), (Phase::Offered, 4));
    assert_eq!(
        reverse_maker_state_after_reopen(path, id).await,
        (Phase::Offered, 4)
    );
    let (no_head, phase, revision) =
        refresh_reverse_maker_after_reopen(path, id, removal_observation).await;
    assert_eq!(
        no_head,
        MakerFundingEligibilityOutcome::AwaitingStableObservation(FirstLockStepV1::LezFund)
    );
    assert_eq!((phase, revision), (Phase::Offered, 4));
    assert_eq!(maker_transition_count(path, id), 4);
}

async fn reverse_maker_state_after_reopen(path: &std::path::Path, id: &str) -> (Phase, u64) {
    let store =
        SqliteZecRecoveryStore::open(path, Participant::Maker).expect("reopen reverse maker store");
    let active = ZecPairSdk::new(
        Participant::Maker,
        NoDiscovery,
        FixedNegotiation,
        MakerObservation(TakerFirstLockObservationV1::Absent),
        MakerObservation(TakerFirstLockObservationV1::Absent),
        store,
    )
    .resume(&SwapId::new(id).expect("swap ID"))
    .await
    .expect("resume reverse maker")
    .expect("durable reverse agreement");
    (active.status(), active.revision())
}

async fn poll_reverse_maker_after_reopen(
    path: &std::path::Path,
    id: &str,
    canonical: CanonicalLezEscrowObservationV1,
) -> (ObserveTakerFirstLockOutcome, Phase, u64) {
    poll_reverse_maker_observation_after_reopen(
        path,
        id,
        TakerFirstLockObservationV1::CanonicalLez(Box::new(canonical)),
    )
    .await
}

async fn poll_reverse_maker_observation_after_reopen(
    path: &std::path::Path,
    id: &str,
    observation: TakerFirstLockObservationV1,
) -> (ObserveTakerFirstLockOutcome, Phase, u64) {
    let (outcome, phase, revision) =
        try_poll_reverse_maker_observation_after_reopen(path, id, observation).await;
    (outcome.expect("reverse maker poll"), phase, revision)
}

async fn try_poll_reverse_maker_observation_after_reopen(
    path: &std::path::Path,
    id: &str,
    observation: TakerFirstLockObservationV1,
) -> (
    Result<ObserveTakerFirstLockOutcome, ZecSdkError>,
    Phase,
    u64,
) {
    let store =
        SqliteZecRecoveryStore::open(path, Participant::Maker).expect("reopen reverse maker poll");
    let mut active = ZecPairSdk::new(
        Participant::Maker,
        NoDiscovery,
        FixedNegotiation,
        MakerObservation(observation),
        MakerObservation(TakerFirstLockObservationV1::Absent),
        store,
    )
    .resume(&SwapId::new(id).expect("swap ID"))
    .await
    .expect("resume reverse maker poll")
    .expect("durable reverse agreement");
    let outcome = active.observe_taker_first_lock().await;
    (outcome, active.status(), active.revision())
}

async fn refresh_reverse_maker_after_reopen(
    path: &std::path::Path,
    id: &str,
    observation: TakerFirstLockObservationV1,
) -> (MakerFundingEligibilityOutcome, Phase, u64) {
    let store =
        SqliteZecRecoveryStore::open(path, Participant::Maker).expect("reopen reverse eligibility");
    let mut active = ZecPairSdk::new(
        Participant::Maker,
        NoDiscovery,
        FixedNegotiation,
        MakerObservation(observation),
        MakerObservation(TakerFirstLockObservationV1::Absent),
        store,
    )
    .resume(&SwapId::new(id).expect("swap ID"))
    .await
    .expect("resume reverse eligibility")
    .expect("durable reverse agreement");
    let outcome = active
        .refresh_maker_funding_eligibility()
        .await
        .expect("refresh reverse eligibility");
    (outcome, active.status(), active.revision())
}

fn rewrite_lez_row_as_legacy_payload_v1(
    path: &std::path::Path,
    id: &str,
    onchain_swap_id: [u8; 32],
) {
    let connection = Connection::open(path).expect("open legacy LEZ payload fixture");
    let current: String = connection
        .query_row(
            "SELECT payload_json FROM zec_sdk_first_lock_transitions
             WHERE local_role = 'maker' AND swap_id = ?1 AND predecessor_revision = 0",
            params![id],
            |row| row.get(0),
        )
        .expect("current LEZ payload");
    let instruction = serde_json::to_string(&LezFundInstructionV1::Native {
        swap_id: onchain_swap_id,
    })
    .expect("instruction JSON");
    let swap_id = serde_json::to_string(&onchain_swap_id).expect("swap ID JSON");
    let legacy = current
        .replacen(
            &format!("\"instruction\":{instruction}"),
            &format!("\"swap_id\":{swap_id}"),
            1,
        )
        .replacen(",\"lez_change\":null", "", 1);
    assert_ne!(legacy, current, "historical instruction shape installed");
    connection
        .execute(
            "UPDATE zec_sdk_first_lock_transitions SET payload_json = ?1
             WHERE local_role = 'maker' AND swap_id = ?2 AND predecessor_revision = 0",
            params![legacy, id],
        )
        .expect("install historical payload-v1 row");
}

fn maker_transition_count(path: &std::path::Path, id: &str) -> i64 {
    Connection::open(path)
        .expect("inspect maker journal")
        .query_row(
            "SELECT COUNT(*) FROM zec_sdk_first_lock_transitions
             WHERE local_role = 'maker' AND swap_id = ?1",
            params![id],
            |row| row.get(0),
        )
        .expect("maker journal count")
}

fn canonical_lez_removal_at(
    agreement: &lez_zec_swap_sdk::ZecAgreementV1,
    previous: &CanonicalLezEscrowObservationV1,
    canonical_block_hash_at_removed_height: [u8; 32],
    tip_hash: [u8; 32],
    tip_height: u64,
) -> CanonicalLezEscrowRemovalV1 {
    let chain = agreement.lez_terms().chain();
    CanonicalLezEscrowRemovalV1::validate(
        previous,
        &LezNodeRemovalSnapshotV1::new(
            chain.environment(),
            *chain.channel_id(),
            *chain.genesis_block_hash(),
            canonical_block_hash_at_removed_height,
            LezStableTipV1::new(tip_hash, tip_height, tip_hash, tip_height),
        ),
    )
    .expect("stable affirmative LEZ removal")
}

async fn assert_fresh_eligibility_after_reopen(
    path: &std::path::Path,
    id: &str,
    canonical: CanonicalZcashOutputObservation,
) {
    let store =
        SqliteZecRecoveryStore::open(path, Participant::Maker).expect("reopen eligibility store");
    let observation = MakerObservation(TakerFirstLockObservationV1::CanonicalZcash(Box::new(
        canonical,
    )));
    let mut active = ZecPairSdk::new(
        Participant::Maker,
        NoDiscovery,
        FixedNegotiation,
        observation.clone(),
        observation,
        store,
    )
    .resume(&SwapId::new(id).expect("swap ID"))
    .await
    .expect("resume eligibility state")
    .expect("maker agreement");
    assert_eq!(
        active
            .refresh_maker_funding_eligibility()
            .await
            .expect("fresh exact-head eligibility"),
        MakerFundingEligibilityOutcome::Eligible { revision: 1 }
    );
    assert_eq!(active.revision(), 1);
    assert_eq!(
        active.next_action(),
        lez_zec_swap_sdk::ZecLifecycleAction::Wait
    );
    let raw = Connection::open(path).expect("inspect eligibility no-write");
    let rows: i64 = raw
        .query_row(
            "SELECT COUNT(*) FROM zec_sdk_first_lock_transitions
             WHERE local_role = 'maker' AND swap_id = ?1",
            params![id],
            |row| row.get(0),
        )
        .expect("maker journal count");
    assert_eq!(rows, 1);
}

#[tokio::test]
async fn stale_maker_instance_catches_up_before_an_absent_poll_returns() {
    let data = TempDir::new().expect("concurrent maker store");
    let path = data.path().join("maker-head-ahead.sqlite3");
    let id = "sqlite-maker-head-ahead";
    let accepted = accept(
        &agreement_wire(id, FixtureVariant::Local),
        Participant::Maker,
    );
    let canonical = canonical_zcash_taker_lock(accepted.agreement());
    let initial = MakerObservation(TakerFirstLockObservationV1::CanonicalZcash(Box::new(
        canonical,
    )));
    let store = SqliteZecRecoveryStore::open(&path, Participant::Maker).expect("open maker store");
    let stale_sdk = ZecPairSdk::new(
        Participant::Maker,
        NoDiscovery,
        FixedNegotiation,
        MakerObservation(TakerFirstLockObservationV1::Absent),
        MakerObservation(TakerFirstLockObservationV1::Absent),
        store.clone(),
    );
    let leader_sdk = ZecPairSdk::new(
        Participant::Maker,
        NoDiscovery,
        FixedNegotiation,
        initial.clone(),
        initial,
        store,
    );
    let mut stale = stale_sdk
        .activate(accepted.clone())
        .await
        .expect("activate stale maker");
    let mut leader = leader_sdk
        .activate(accepted.clone())
        .await
        .expect("activate leader maker");
    leader
        .observe_taker_first_lock()
        .await
        .expect("leader commits canonical revision");

    let deeper = canonical_zcash_taker_lock_at_depth(accepted.agreement(), [9; 32], 4);
    commit_maker_observation_after_reopen(
        &path,
        id,
        TakerFirstLockObservationV1::CanonicalZcash(Box::new(deeper)),
        Phase::TakerLockConfirmed,
        2,
    )
    .await;
    assert_eq!(
        stale
            .observe_taker_first_lock()
            .await
            .expect("absent poll returns only after durable catch-up"),
        ObserveTakerFirstLockOutcome::AwaitingStableObservation(FirstLockStepV1::ZcashFund)
    );
    assert_eq!(stale.status(), Phase::TakerLockConfirmed);
    assert_eq!(stale.revision(), 2);
}

#[tokio::test]
async fn maker_observation_trigger_failure_rolls_back_row_and_revision() {
    let data = TempDir::new().expect("maker rollback store");
    let path = data.path().join("maker-rollback.sqlite3");
    let id = "sqlite-maker-rollback";
    let accepted = accept(
        &agreement_wire(id, FixtureVariant::Local),
        Participant::Maker,
    );
    let observation = MakerObservation(TakerFirstLockObservationV1::CanonicalZcash(Box::new(
        canonical_zcash_taker_lock(accepted.agreement()),
    )));
    let store = SqliteZecRecoveryStore::open(&path, Participant::Maker).expect("open maker store");
    let sdk = ZecPairSdk::new(
        Participant::Maker,
        NoDiscovery,
        FixedNegotiation,
        observation.clone(),
        observation,
        store,
    );
    let mut active = sdk.activate(accepted).await.expect("activate maker");
    let raw = Connection::open(&path).expect("install maker fault");
    raw.execute_batch(
        "CREATE TRIGGER fail_maker_revision
         BEFORE UPDATE OF active_revision ON zec_sdk_agreements
         BEGIN SELECT RAISE(ABORT, 'forced maker rollback'); END;",
    )
    .expect("install maker rollback trigger");
    assert!(active.observe_taker_first_lock().await.is_err());
    assert_eq!(active.revision(), 0);
    let rows: (i64, i64) = raw
        .query_row(
            "SELECT active_revision,
                    (SELECT COUNT(*) FROM zec_sdk_first_lock_transitions
                     WHERE local_role = 'maker' AND swap_id = ?1)
             FROM zec_sdk_agreements
             WHERE local_role = 'maker' AND swap_id = ?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("rolled-back maker unit");
    assert_eq!(rows, (0, 0));
    raw.execute_batch("DROP TRIGGER fail_maker_revision;")
        .expect("remove maker rollback trigger");
    active
        .observe_taker_first_lock()
        .await
        .expect("maker retry commits");
    assert_eq!(active.revision(), 1);
}

async fn assert_initial_maker_observation_persists(
    path: &std::path::Path,
    id: &str,
    accepted: &AcceptedZecAgreementV1,
    observation: MakerObservation,
) {
    let store = SqliteZecRecoveryStore::open(path, Participant::Maker).expect("open maker store");
    let pair_sdk = ZecPairSdk::new(
        Participant::Maker,
        NoDiscovery,
        FixedNegotiation,
        observation.clone(),
        observation,
        store.clone(),
    );
    let mut active = pair_sdk
        .activate(accepted.clone())
        .await
        .expect("activate maker");
    assert_eq!(
        active
            .observe_taker_first_lock()
            .await
            .expect("maker projects its own canonical observation"),
        ObserveTakerFirstLockOutcome::Projected(FirstLockProjectionCommit::new(1, false))
    );
    assert_eq!(active.status(), Phase::TakerLockConfirmed);
    let maker_transition = store
        .load_observed_taker_first_lock_transition(&SwapId::new(id).expect("swap ID"), 0)
        .await
        .expect("load maker transition")
        .expect("maker transition retained");
    assert_eq!(
        store
            .commit_observed_taker_first_lock_transition(&maker_transition)
            .await
            .expect("exact maker transition replay"),
        FirstLockProjectionCommit::new(1, true)
    );

    let raw = Connection::open(path).expect("inspect maker recovery");
    let rows: (i64, i64, i64) = raw
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM zec_sdk_first_lock_transitions
                 WHERE local_role = 'maker' AND swap_id = ?1),
                (SELECT COUNT(*) FROM zec_sdk_first_lock_intents
                 WHERE local_role = 'maker' AND swap_id = ?1),
                active_revision
             FROM zec_sdk_agreements
             WHERE local_role = 'maker' AND swap_id = ?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("maker rows");
    assert_eq!(rows, (1, 0, 1));
}

async fn assert_store_rejects_unproved_canonical_replacement(
    path: &std::path::Path,
    id: &str,
    accepted: &AcceptedZecAgreementV1,
) {
    let raw = Connection::open(path).expect("read canonical transition fixture");
    let payload: String = raw
        .query_row(
            "SELECT payload_json FROM zec_sdk_first_lock_transitions
             WHERE local_role = 'maker' AND swap_id = ?1 AND predecessor_revision = 0",
            params![id],
            |row| row.get(0),
        )
        .expect("canonical transition payload");
    let store =
        SqliteZecRecoveryStore::open(path, Participant::Maker).expect("open tracker boundary");
    let initial = canonical_zcash_taker_lock(accepted.agreement());
    let duplicate = observed_transition_from_event(&payload, accepted, &initial, 1);
    assert!(
        store
            .commit_observed_taker_first_lock_transition(&duplicate)
            .await
            .is_err(),
        "store must reject an unchanged canonical poll"
    );

    let changed_inclusion = canonical_zcash_taker_lock_fixture(
        accepted.agreement(),
        [9; 32],
        BlockHash([0x55; 32]),
        BlockHash([0xbb; 32]),
        BlockHeight::from_u32(103),
    );
    let changed_inclusion_transition =
        observed_transition_from_event(&payload, accepted, &changed_inclusion, 1);
    assert!(
        store
            .commit_observed_taker_first_lock_transition(&changed_inclusion_transition)
            .await
            .is_err(),
        "same transaction in a different inclusion requires replacement proof"
    );

    let stale_removal = canonical_zcash_removal(&changed_inclusion);
    let stale_transaction_id = stale_removal.previous().transaction_id().to_string();
    let stale_removal_transition = observed_transition_from_record(
        &payload,
        accepted,
        &stale_transaction_id,
        stale_removal.previous().confirmations().get(),
        ZcashObservationEventRecordV1::from_event(
            &lez_zec_swap_sdk::ZcashObservationEvent::Removed(stale_removal),
        ),
        1,
    );
    assert!(
        store
            .commit_observed_taker_first_lock_transition(&stale_removal_transition)
            .await
            .is_err(),
        "removal must name the exact tracker inclusion, not only the same transaction"
    );

    let changed_transaction = canonical_zcash_taker_lock_with_input(accepted.agreement(), [7; 32]);
    let changed_transaction_transition =
        observed_transition_from_event(&payload, accepted, &changed_transaction, 1);
    assert!(
        store
            .commit_observed_taker_first_lock_transition(&changed_transaction_transition)
            .await
            .is_err(),
        "changed canonical transaction requires replacement evidence"
    );
}

fn observed_transition_from_event(
    payload: &str,
    accepted: &AcceptedZecAgreementV1,
    canonical: &CanonicalZcashOutputObservation,
    predecessor: u64,
) -> lez_zec_swap_sdk::ObservedTakerFirstLockTransitionV1 {
    let transaction_id = canonical.transaction_id().to_string();
    observed_transition_from_record(
        payload,
        accepted,
        &transaction_id,
        canonical.confirmations().get(),
        ZcashObservationEventRecordV1::from_canonical(canonical),
        predecessor,
    )
}

fn observed_transition_from_record(
    payload: &str,
    accepted: &AcceptedZecAgreementV1,
    transaction_id: &str,
    confirmations: u32,
    event: ZcashObservationEventRecordV1,
    predecessor: u64,
) -> lez_zec_swap_sdk::ObservedTakerFirstLockTransitionV1 {
    let mut value: serde_json::Value = serde_json::from_str(payload).expect("transition JSON");
    value["predecessor_revision"] = serde_json::json!(predecessor);
    value["transaction_id"] = serde_json::json!(transaction_id);
    value["confirmations"] = serde_json::json!(confirmations);
    value["zcash_canonical"] = serde_json::to_value(event).expect("canonical event JSON");
    let record: ObservedTakerFirstLockTransitionRecordV1 =
        serde_json::from_value(value).expect("well-formed transition record");
    record
        .revalidate(accepted, predecessor)
        .expect("individually agreement-valid transition")
}

async fn commit_maker_observation_after_reopen(
    path: &std::path::Path,
    id: &str,
    observation: TakerFirstLockObservationV1,
    expected_phase: Phase,
    expected_revision: u64,
) {
    let store =
        SqliteZecRecoveryStore::open(path, Participant::Maker).expect("reopen maker journal");
    let mut active = ZecPairSdk::new(
        Participant::Maker,
        NoDiscovery,
        FixedNegotiation,
        MakerObservation(TakerFirstLockObservationV1::Absent),
        MakerObservation(observation),
        store,
    )
    .resume(&SwapId::new(id).expect("swap ID"))
    .await
    .expect("resume maker journal")
    .expect("maker agreement");
    assert!(matches!(
        active.observe_taker_first_lock().await,
        Ok(ObserveTakerFirstLockOutcome::Projected(_))
    ));
    assert_eq!(active.status(), expected_phase);
    assert_eq!(active.revision(), expected_revision);
}

fn canonical_zcash_taker_lock(
    agreement: &lez_zec_swap_sdk::ZecAgreementV1,
) -> CanonicalZcashOutputObservation {
    canonical_zcash_taker_lock_at_depth(agreement, [9; 32], 3)
}

fn canonical_zcash_taker_lock_with_input(
    agreement: &lez_zec_swap_sdk::ZecAgreementV1,
    input_transaction_id: [u8; 32],
) -> CanonicalZcashOutputObservation {
    canonical_zcash_taker_lock_at_depth(agreement, input_transaction_id, 3)
}

fn canonical_zcash_taker_lock_at_depth(
    agreement: &lez_zec_swap_sdk::ZecAgreementV1,
    input_transaction_id: [u8; 32],
    confirmations: u32,
) -> CanonicalZcashOutputObservation {
    canonical_zcash_taker_lock_fixture(
        agreement,
        input_transaction_id,
        BlockHash([0x44; 32]),
        BlockHash([0xaa; 32]),
        BlockHeight::from_u32(100 + confirmations - 1),
    )
}

fn canonical_zcash_replacement(
    agreement: &lez_zec_swap_sdk::ZecAgreementV1,
    removed: &CanonicalZcashOutputRemoval,
) -> CanonicalZcashOutputObservation {
    canonical_zcash_taker_lock_fixture(
        agreement,
        [8; 32],
        removed.canonical_block_hash_at_removed_height(),
        removed.tip_block_hash(),
        removed.tip_height(),
    )
}

fn canonical_zcash_replacement_depth_update(
    agreement: &lez_zec_swap_sdk::ZecAgreementV1,
) -> CanonicalZcashOutputObservation {
    canonical_zcash_taker_lock_fixture(
        agreement,
        [8; 32],
        BlockHash([0x55; 32]),
        BlockHash([0xcc; 32]),
        BlockHeight::from_u32(104),
    )
}

fn canonical_zcash_taker_lock_fixture(
    agreement: &lez_zec_swap_sdk::ZecAgreementV1,
    input_transaction_id: [u8; 32],
    transaction_block_hash: BlockHash,
    tip_block_hash: BlockHash,
    tip_height: BlockHeight,
) -> CanonicalZcashOutputObservation {
    let expected = agreement.binding().expected_output();
    let key = SecretKey::from_slice(&[7; 32]).expect("canonical observation owner key");
    let public_key = PublicKey::from_secret_key(&Secp256k1::new(), &key);
    let owner_script: Script = TransparentAddress::from_pubkey(&public_key).script().into();
    let request = TransparentFundingRequest::new(
        vec![TransparentUtxo::new(
            OutPoint::new(input_transaction_id, 0),
            TxOut::new(
                Zatoshis::from_u64(
                    u64::from(expected.value())
                        .checked_add(20_000)
                        .expect("fixture input value"),
                )
                .expect("fixture input"),
                owner_script,
            ),
        )],
        public_key,
        expected.value(),
        Zatoshis::from_u64(1_000).expect("fee"),
        Zatoshis::from_u64(1_000).expect("change floor"),
        BlockHeight::from_u32(4_100_000),
        expected.consensus_branch_id(),
    )
    .expect("canonical funding request");
    let transaction = build_funding_transaction(expected.contract(), &request, &key)
        .expect("funding transaction");
    let mut raw = Vec::new();
    transaction.write(&mut raw).expect("canonical bytes");
    CanonicalZcashOutputObservation::validate(
        expected,
        &ZcashNodeSnapshot::new(
            expected.network(),
            expected.consensus_branch_id(),
            true,
            transaction_block_hash,
            transaction_block_hash,
            BlockHeight::from_u32(100),
            ZcashStableTip::new(tip_block_hash, tip_height, tip_block_hash, tip_height),
            transaction.txid(),
            raw,
            0,
            u32::from(tip_height) - 100 + 1,
        ),
    )
    .expect("agreement-bound canonical observation")
}

fn canonical_zcash_removal(
    previous: &CanonicalZcashOutputObservation,
) -> CanonicalZcashOutputRemoval {
    let (replacement_block_hash, tip_block_hash) = if previous.block_hash() == BlockHash([0x44; 32])
    {
        (BlockHash([0x55; 32]), BlockHash([0xbb; 32]))
    } else {
        (BlockHash([0x66; 32]), BlockHash([0xdd; 32]))
    };
    let tip_height = BlockHeight::from_u32(u32::from(previous.tip_height()) + 1);
    CanonicalZcashOutputRemoval::validate(
        previous,
        &ZcashNodeRemovalSnapshot::new(
            previous.network(),
            previous.consensus_branch_id(),
            replacement_block_hash,
            ZcashStableTip::new(tip_block_hash, tip_height, tip_block_hash, tip_height),
        ),
    )
    .expect("affirmative stable canonical removal")
}

async fn assert_orphan_future_maker_transition_fails_closed(path: &std::path::Path, id: &str) {
    let raw = Connection::open(path).expect("inject orphan future maker transition");
    raw.execute(
        "DELETE FROM zec_sdk_first_lock_transitions
         WHERE local_role = 'maker' AND swap_id = ?1 AND predecessor_revision = 1",
        params![id],
    )
    .expect("remove middle maker transition");
    raw.execute(
        "INSERT INTO zec_sdk_first_lock_transitions (
            local_role, swap_id, predecessor_revision, committed_revision,
            payload_version, payload_json
        ) VALUES ('maker', ?1, 4, 5, 1, '{}')",
        params![id],
    )
    .expect("substitute future maker transition while preserving row count");
    drop(raw);
    let corrupt =
        SqliteZecRecoveryStore::open(path, Participant::Maker).expect("reopen corrupt maker store");
    let result = ZecPairSdk::new(
        Participant::Maker,
        NoDiscovery,
        FixedNegotiation,
        MakerObservation(TakerFirstLockObservationV1::Absent),
        MakerObservation(TakerFirstLockObservationV1::Absent),
        corrupt,
    )
    .resume(&SwapId::new(id).expect("swap ID"))
    .await;
    assert!(matches!(
        result,
        Err(ZecSdkError::Persistence(error))
            if error.to_string().contains("internally inconsistent")
    ));
}

fn sdk(participant: Participant, store: SqliteZecRecoveryStore) -> TestSdk {
    ZecPairSdk::new(
        participant,
        NoDiscovery,
        FixedNegotiation,
        NoChain,
        NoChain,
        store,
    )
}

fn accept(wire: &[u8], participant: Participant) -> AcceptedZecAgreementV1 {
    AcceptedZecAgreementV1::accept_wire_at(wire, ACCEPTED_AT, participant, 0)
        .expect("real dual-signed concrete agreement")
}

fn zcash_plan(expected_submission_id: [u8; 32], bytes: Vec<u8>) -> FirstLockPlanV1 {
    FirstLockPlanV1::zcash(
        PreparedFirstLockSubmissionV1::new(
            FirstLockStepV1::ZcashFund,
            expected_submission_id,
            bytes,
        )
        .expect("bounded exact transaction"),
    )
    .expect("Zcash-first plan")
}

fn confirmed(
    expected_submission_id: [u8; 32],
    transaction_id: &str,
) -> FirstLockConfirmedEvidenceV1 {
    FirstLockConfirmedEvidenceV1::new(
        FirstLockStepV1::ZcashFund,
        expected_submission_id,
        transaction_id.to_owned(),
        100,
    )
    .expect("stable confirmed first lock")
}

fn canonical_lez_taker_lock(
    agreement: &lez_zec_swap_sdk::ZecAgreementV1,
) -> CanonicalLezEscrowObservationV1 {
    canonical_lez_taker_lock_at(agreement, LezInclusionStatusV1::Pending, [0x42; 32], 102)
}

fn canonical_lez_taker_lock_at(
    agreement: &lez_zec_swap_sdk::ZecAgreementV1,
    inclusion_status: LezInclusionStatusV1,
    tip_hash: [u8; 32],
    tip_height: u64,
) -> CanonicalLezEscrowObservationV1 {
    canonical_lez_taker_lock_with_inclusion(
        agreement,
        inclusion_status,
        tip_hash,
        tip_height,
        [0x31; 32],
        100,
        [0x41; 32],
    )
}

fn canonical_lez_taker_lock_with_inclusion(
    agreement: &lez_zec_swap_sdk::ZecAgreementV1,
    inclusion_status: LezInclusionStatusV1,
    tip_hash: [u8; 32],
    tip_height: u64,
    transaction_id: [u8; 32],
    inclusion_height: u64,
    inclusion_block_hash: [u8; 32],
) -> CanonicalLezEscrowObservationV1 {
    let terms = agreement.lez_terms();
    let LezAssetV1::Native {
        authenticated_transfer_program_id,
    } = terms.asset()
    else {
        panic!("SQLite fixture uses native LEZ")
    };
    let depositor = *agreement.lez_account(agreement.lez_depositor());
    let claimant = *agreement.lez_account(agreement.lez_claimant());
    let metadata = LezEscrowMetadataSnapshotV1::new(
        1,
        *agreement.onchain_swap_id(),
        *agreement.agreement_commitment(),
        *agreement.secret_digest(),
        depositor,
        depositor,
        claimant,
        claimant,
        *terms.custody_account(),
        *authenticated_transfer_program_id,
        *authenticated_transfer_program_id,
        [0; 32],
        terms.amount(),
        agreement.lez_refund_at_ms(),
        LezEscrowStatusV1::Funded,
    );
    let transaction = LezFundTransactionSnapshotV1::new(
        transaction_id,
        *terms.escrow_program_id(),
        depositor,
        vec![
            *terms.metadata_account(),
            *terms.custody_account(),
            depositor,
        ],
        LezFundInstructionV1::Native {
            swap_id: *agreement.onchain_swap_id(),
        },
        true,
        true,
        inclusion_height,
        inclusion_block_hash,
        inclusion_block_hash,
        inclusion_status,
    );
    CanonicalLezEscrowObservationV1::validate(
        agreement,
        &LezNodeSnapshotV1::new(
            terms.chain().environment(),
            *terms.chain().channel_id(),
            *terms.chain().genesis_block_hash(),
            LezStableTipV1::new(tip_hash, tip_height, tip_hash, tip_height),
            transaction,
            *terms.escrow_program_id(),
            *terms.metadata_account(),
            metadata,
            *terms.custody_account(),
            LezCustodySnapshotV1::Native {
                program_owner: *authenticated_transfer_program_id,
                balance: terms.amount(),
            },
        ),
    )
    .expect("agreement-bound canonical LEZ observation")
}

#[derive(Clone, Copy)]
enum FixtureVariant {
    Local,
    ChangedTranscript,
}

fn agreement_wire(id: &str, variant: FixtureVariant) -> Vec<u8> {
    agreement_wire_direction(id, variant, SwapDirection::TakerSellsForeign)
}

fn agreement_wire_direction(
    id: &str,
    variant: FixtureVariant,
    direction: SwapDirection,
) -> Vec<u8> {
    agreement_wire_direction_with_lez_amount(id, variant, direction, 42)
}

fn agreement_wire_direction_with_lez_amount(
    id: &str,
    variant: FixtureVariant,
    direction: SwapDirection,
    lez_amount: u128,
) -> Vec<u8> {
    let maker_secret = SecretKey::from_slice(&[1; 32]).expect("maker key");
    let taker_secret = SecretKey::from_slice(&[2; 32]).expect("taker key");
    let secp = Secp256k1::new();
    let maker_key = PublicKey::from_secret_key(&secp, &maker_secret).serialize();
    let taker_key = PublicKey::from_secret_key(&secp, &taker_secret).serialize();
    let (refund_key, claimant_key) = match direction {
        SwapDirection::TakerSellsForeign => (taker_key, maker_key),
        SwapDirection::TakerSellsLez => (maker_key, taker_key),
    };
    let refund_hash = pubkey_hash(&refund_key);
    let claimant_hash = pubkey_hash(&claimant_key);
    let escrow_program = [1; 8];
    let onchain_swap_id = derive_lez_swap_id_v1(id.as_bytes());
    let metadata_account = derive_lez_metadata_account_v1(&escrow_program, &onchain_swap_id);
    let custody_account = derive_lez_native_custody_account_v1(&escrow_program, &onchain_swap_id);
    let digest = [9; 32];
    let binding = ZecSwapBinding::new(
        ZecProfileId::DeterministicLocalV1,
        ExpectedBip199Output::new(
            NetworkType::Regtest,
            BranchId::Nu6_2,
            Zatoshis::from_u64(100_000_000).expect("value"),
            Bip199Contract::new(120, refund_hash, digest, claimant_hash),
        ),
    )
    .expect("binding");
    let body = ZecAgreementBodyV1::new(
        id.to_owned(),
        direction,
        ZecProfileRecordV1::from(ZecProfileId::DeterministicLocalV1),
        ZecParticipantsV1::new(
            ZecParticipantIdentityV1::new([3; 32], maker_key),
            ZecParticipantIdentityV1::new([4; 32], taker_key),
        ),
        digest,
        ZecLezTermsV1::new(
            LezChainIdentityV1::new(LezEnvironmentV1::DeterministicLocalV0_2, [8; 32], [7; 32]),
            escrow_program,
            LezAssetV1::Native {
                authenticated_transfer_program_id: [2; 8],
            },
            lez_amount,
            metadata_account,
            custody_account,
        ),
        ZecSwapBindingRecordV1::from_binding(&binding),
        ZecTransactionPolicyV1::new(
            [12; 32],
            ZcashTransparentDestinationV1::p2pkh(refund_hash),
            10_000,
            1_000,
            ZcashTransparentDestinationV1::p2pkh(claimant_hash),
            10_000,
            ZcashTransparentDestinationV1::p2pkh(refund_hash),
            10_000,
            40,
        ),
        ZecRefundPlanV1::new(100, 116, 160_000, 200),
        NegotiationTranscriptV1::new(
            [5; 32],
            match variant {
                FixtureVariant::Local => [6; 32],
                FixtureVariant::ChangedTranscript => [0x66; 32],
            },
            1_000,
        ),
    );
    let commitment = body.commitment();
    ZecAgreementRecordV1::from_parts(
        ZEC_CONCRETE_AGREEMENT_SCHEMA_V2,
        body,
        commitment,
        secp.sign_ecdsa(&Message::from_digest(commitment), &maker_secret)
            .serialize_compact(),
        secp.sign_ecdsa(&Message::from_digest(commitment), &taker_secret)
            .serialize_compact(),
    )
    .encode_wire()
    .expect("bounded fixture wire")
}

fn pubkey_hash(bytes: &[u8; 33]) -> [u8; 20] {
    match TransparentAddress::from_pubkey(&PublicKey::from_slice(bytes).expect("fixture pubkey")) {
        TransparentAddress::PublicKeyHash(hash) => hash,
        TransparentAddress::ScriptHash(_) => unreachable!("public keys produce P2PKH"),
    }
}
