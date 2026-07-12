use async_trait::async_trait;
use lez_swap_core::{Participant, Phase, SwapDirection, SwapId, UnixSeconds};
use lez_swap_store::SqliteZecRecoveryStore;
use lez_zec_swap_sdk::{
    AcceptedZecAgreementV1, Bip199Contract, CreateFirstLockOutcome, ExpectedBip199Output,
    FirstLockConfirmedEvidenceV1, FirstLockPlanV1, FirstLockProjectionCommit, FirstLockStepV1,
    LezAssetV1, LezChainIdentityV1, LezEnvironmentV1, NegotiationChannel, NegotiationTranscriptV1,
    OfferDiscovery, PreparedFirstLockSubmissionV1, RecoveryStore, ZEC_CONCRETE_AGREEMENT_SCHEMA_V1,
    ZcashTransparentDestinationV1, ZecAgreementBodyV1, ZecAgreementRecordV1, ZecLezTermsV1,
    ZecPairSdk, ZecParticipantIdentityV1, ZecParticipantsV1, ZecProfileId, ZecProfileRecordV1,
    ZecRefundPlanV1, ZecSdkError, ZecSwapBinding, ZecSwapBindingRecordV1, ZecTransactionPolicyV1,
    derive_lez_metadata_account_v1, derive_lez_native_custody_account_v1, derive_lez_swap_id_v1,
};
use rusqlite::{Connection, OptionalExtension, params};
use secp256k1::{Message, PublicKey, Secp256k1, SecretKey};
use tempfile::TempDir;
use zcash_protocol::{
    consensus::{BranchId, NetworkType},
    value::Zatoshis,
};
use zcash_transparent::address::TransparentAddress;

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

#[derive(Clone, Copy)]
enum FixtureVariant {
    Local,
    ChangedTranscript,
}

fn agreement_wire(id: &str, variant: FixtureVariant) -> Vec<u8> {
    let maker_secret = SecretKey::from_slice(&[1; 32]).expect("maker key");
    let taker_secret = SecretKey::from_slice(&[2; 32]).expect("taker key");
    let secp = Secp256k1::new();
    let maker_key = PublicKey::from_secret_key(&secp, &maker_secret).serialize();
    let taker_key = PublicKey::from_secret_key(&secp, &taker_secret).serialize();
    let refund_hash = pubkey_hash(&taker_key);
    let claimant_hash = pubkey_hash(&maker_key);
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
        SwapDirection::TakerSellsForeign,
        ZecProfileRecordV1::from(ZecProfileId::DeterministicLocalV1),
        ZecParticipantsV1::new(
            ZecParticipantIdentityV1::new([3; 32], maker_key),
            ZecParticipantIdentityV1::new([4; 32], taker_key),
        ),
        digest,
        ZecLezTermsV1::new(
            LezChainIdentityV1::new(LezEnvironmentV1::DeterministicLocalV0_2, [7; 32]),
            escrow_program,
            LezAssetV1::Native {
                authenticated_transfer_program_id: [2; 8],
            },
            42,
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
        ZEC_CONCRETE_AGREEMENT_SCHEMA_V1,
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
