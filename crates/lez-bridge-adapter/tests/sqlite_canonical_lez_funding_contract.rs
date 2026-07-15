//! Durable authority contract for canonical LEZ claim funding.

#![forbid(unsafe_code)]

use std::{
    convert::Infallible,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use async_trait::async_trait;
use lez_bridge_adapter::{
    CanonicalLezFundingSource, SqliteCanonicalLezFundingSource,
    SqliteCanonicalLezFundingSourceError,
};
use lez_swap_core::{Participant, SwapDirection, UnixSeconds};
use lez_swap_store::SqliteZecRecoveryStore;
use lez_zec_swap_sdk::{
    AcceptedZecAgreementV1, Bip199Contract, CanonicalLezEscrowObservationV1, ExpectedBip199Output,
    FirstLockConfirmedEvidenceV1, FirstLockDriveOutcome, FirstLockObservation, FirstLockPlanV1,
    FirstLockStepV1, LezAssetV1, LezChainIdentityV1, LezCustodySnapshotV1, LezEnvironmentV1,
    LezEscrowMetadataSnapshotV1, LezEscrowStatusV1, LezFirstLockPort, LezFundInstructionV1,
    LezFundTransactionSnapshotV1, LezInclusionStatusV1, LezMakerLockObservationPort,
    LezNodeSnapshotV1, LezObservationError, LezStableTipV1, LezTakerFirstLockObservationPort,
    MakerLockDriveOutcome, MakerLockObservationV1, NegotiationChannel, NegotiationTranscriptV1,
    OfferDiscovery, PreparedFirstLockSubmissionV1, TakerFirstLockObservationV1,
    ZEC_CONCRETE_AGREEMENT_SCHEMA_V2, ZcashFirstLockPort, ZcashMakerLockObservationPort,
    ZcashTakerFirstLockObservationPort, ZcashTransparentDestinationV1, ZecAgreementBodyV1,
    ZecAgreementRecordV1, ZecAgreementV1, ZecLezTermsV1, ZecPairSdk, ZecParticipantIdentityV1,
    ZecParticipantsV1, ZecProfileId, ZecProfileRecordV1, ZecRefundPlanV1, ZecSwapBinding,
    ZecSwapBindingRecordV1, ZecTransactionPolicyV1, derive_lez_metadata_account_v1,
    derive_lez_native_custody_account_v1, derive_lez_swap_id_v1,
};
use rusqlite::{Connection, params};
use secp256k1::{Message, PublicKey, Secp256k1, SecretKey};
use sha2::{Digest as _, Sha256};
use zcash_protocol::{
    consensus::{BranchId, NetworkType},
    value::Zatoshis,
};
use zcash_transparent::address::TransparentAddress;

const LEZ_FUNDING_BYTES: [u8; 32] = [0xab; 32];
static TEST_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[test]
fn v0_2_funding_snapshot_rejects_v0_1_2_metadata_version() {
    let agreement = signed_agreement(
        "sqlite-lez-wrong-metadata-generation",
        SwapDirection::TakerSellsForeign,
        false,
    );
    assert_eq!(
        CanonicalLezEscrowObservationV1::validate(
            &agreement,
            &lez_taker_lock_snapshot(&agreement, 1),
        )
        .expect_err("v0.1.2 metadata cannot authenticate a signed v0.2 agreement"),
        LezObservationError::MetadataBindingMismatch,
    );
}

#[tokio::test]
async fn taker_claimant_reopens_observed_maker_lez_funding() {
    let directory = TestDirectory::new("taker-claimant");
    let path = directory.path().join("taker.sqlite3");
    let agreement = seed_taker_claimant(&path, "sqlite-lez-taker-claimant").await;

    let source = reopened_source(&path, Participant::Taker);
    let evidence = source
        .canonical_lez_funding(&agreement)
        .await
        .expect("reopened taker source reconstructs maker LEZ funding");

    assert_eq!(evidence.step(), FirstLockStepV1::LezFund);
    assert_eq!(evidence.expected_submission_id(), &LEZ_FUNDING_BYTES);
    assert_eq!(evidence.transaction_id(), lowerhex(LEZ_FUNDING_BYTES));
}

#[tokio::test]
async fn maker_claimant_requires_zcash_second_leg_then_reopens_lez_funding() {
    let directory = TestDirectory::new("maker-claimant");
    let incomplete_path = directory.path().join("incomplete.sqlite3");
    let incomplete =
        seed_maker_claimant(&incomplete_path, "sqlite-lez-maker-incomplete", false).await;
    assert_eq!(
        reopened_source(&incomplete_path, Participant::Maker)
            .canonical_lez_funding(&incomplete)
            .await,
        Err(SqliteCanonicalLezFundingSourceError::SecondLockUnavailable)
    );

    let complete_path = directory.path().join("complete.sqlite3");
    let complete = seed_maker_claimant(&complete_path, "sqlite-lez-maker-complete", true).await;
    let evidence = reopened_source(&complete_path, Participant::Maker)
        .canonical_lez_funding(&complete)
        .await
        .expect("both durable legs authorize maker claim preparation");
    assert_eq!(evidence.step(), FirstLockStepV1::LezFund);
    assert_eq!(evidence.expected_submission_id(), &[0x31; 32]);
    assert_eq!(evidence.transaction_id(), lowerhex([0x31; 32]));
}

#[tokio::test]
async fn wrong_claimant_and_substituted_signed_agreement_are_rejected() {
    let directory = TestDirectory::new("agreement-binding");
    let path = directory.path().join("bound.sqlite3");
    let id = "sqlite-lez-agreement-binding";
    let agreement = seed_taker_claimant(&path, id).await;
    let wrong_role = SqliteCanonicalLezFundingSource::new(
        SqliteZecRecoveryStore::open(&path, Participant::Taker).expect("reopen taker store"),
        Participant::Maker,
    );
    assert_eq!(
        wrong_role.canonical_lez_funding(&agreement).await,
        Err(SqliteCanonicalLezFundingSourceError::WrongClaimant)
    );

    let substituted = signed_agreement(id, SwapDirection::TakerSellsForeign, true);
    assert_eq!(
        reopened_source(&path, Participant::Taker)
            .canonical_lez_funding(&substituted)
            .await,
        Err(SqliteCanonicalLezFundingSourceError::AgreementMismatch)
    );
}

#[tokio::test]
async fn mutated_durable_lez_identity_is_rejected_and_diagnostics_are_redacted() {
    let directory = TestDirectory::new("private-path-marker");
    let path = directory.path().join("secret-store-marker.sqlite3");
    let id = "sqlite-lez-mutated-transition";
    let agreement = seed_taker_claimant(&path, id).await;
    let lowercase = lowerhex(LEZ_FUNDING_BYTES);
    let uppercase = lowercase.to_ascii_uppercase();
    let connection = Connection::open(&path).expect("open durable row for mutation");
    let updated = connection
        .execute(
            "UPDATE zec_sdk_observed_maker_lock_transitions
             SET payload_json = replace(payload_json, ?1, ?2)
             WHERE local_role = 'taker' AND swap_id = ?3 AND predecessor_revision = 1",
            params![lowercase, uppercase, id],
        )
        .expect("mutate only LEZ display identity");
    assert_eq!(updated, 1);
    drop(connection);

    let source = reopened_source(&path, Participant::Taker);
    let error = source
        .canonical_lez_funding(&agreement)
        .await
        .expect_err("non-lowerhex durable LEZ identity is rejected");
    assert_eq!(
        error,
        SqliteCanonicalLezFundingSourceError::InvalidFundingIdentity
    );
    let cloned = source.clone();
    for diagnostic in [
        format!("{source:?}"),
        format!("{cloned:?}"),
        format!("{error:?}"),
        error.to_string(),
    ] {
        assert!(!diagnostic.contains("private-path-marker"));
        assert!(!diagnostic.contains("secret-store-marker"));
        assert!(!diagnostic.contains(&uppercase));
    }
}

fn reopened_source(path: &Path, role: Participant) -> SqliteCanonicalLezFundingSource {
    let reopened = SqliteZecRecoveryStore::open(path, role).expect("reopen role-local store");
    SqliteCanonicalLezFundingSource::new(reopened, role)
}

async fn seed_taker_claimant(path: &Path, id: &str) -> ZecAgreementV1 {
    let agreement = signed_agreement(id, SwapDirection::TakerSellsForeign, false);
    let accepted = accept(&agreement, Participant::Taker);
    let store = SqliteZecRecoveryStore::open(path, Participant::Taker).expect("open taker store");
    let ports = TestPorts::default();
    let sdk = ZecPairSdk::new(
        Participant::Taker,
        NoDiscovery,
        NoNegotiation,
        ports.clone(),
        ports,
        store,
    );
    let mut active = sdk.activate(accepted).await.expect("activate taker");
    let zcash_id = [0x21; 32];
    let _ = active
        .stage_first_lock(
            FirstLockPlanV1::zcash(prepared(FirstLockStepV1::ZcashFund, zcash_id))
                .expect("Zcash first-lock plan"),
        )
        .await
        .expect("stage taker Zcash intent");
    let _ = active
        .project_first_lock(confirmed(FirstLockStepV1::ZcashFund, zcash_id))
        .await
        .expect("commit taker Zcash first lock");
    let _ = active
        .observe_maker_lock()
        .await
        .expect("commit observed maker LEZ lock");
    drop(active);
    agreement
}

async fn seed_maker_claimant(path: &Path, id: &str, complete: bool) -> ZecAgreementV1 {
    let agreement = signed_agreement(id, SwapDirection::TakerSellsLez, false);
    let accepted = accept(&agreement, Participant::Maker);
    let canonical = canonical_lez_taker_lock(&agreement);
    let ports = TestPorts {
        taker_lez: Some(canonical),
    };
    let store = SqliteZecRecoveryStore::open(path, Participant::Maker).expect("open maker store");
    let sdk = ZecPairSdk::new(
        Participant::Maker,
        NoDiscovery,
        NoNegotiation,
        ports.clone(),
        ports,
        store,
    );
    let mut active = sdk.activate(accepted).await.expect("activate maker");
    let _ = active
        .observe_taker_first_lock()
        .await
        .expect("commit observed taker LEZ lock");
    if complete {
        let zcash_id = [0x51; 32];
        let outcome = active
            .drive_maker_lock(
                FirstLockPlanV1::zcash(prepared(FirstLockStepV1::ZcashFund, zcash_id))
                    .expect("maker Zcash plan"),
            )
            .await
            .expect("drive maker second lock");
        let MakerLockDriveOutcome::Lock(FirstLockDriveOutcome::ReadyForFundingProjection(evidence)) =
            outcome
        else {
            panic!("expected confirmed maker Zcash funding, got {outcome:?}")
        };
        let _ = active
            .project_maker_lock(evidence)
            .await
            .expect("commit maker Zcash second lock");
    }
    drop(active);
    agreement
}

#[derive(Clone, Default)]
struct TestPorts {
    taker_lez: Option<CanonicalLezEscrowObservationV1>,
}

#[async_trait]
impl LezFirstLockPort for TestPorts {
    type Error = Infallible;

    async fn observe_first_lock(
        &self,
        _agreement: &ZecAgreementV1,
        submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<FirstLockObservation, Self::Error> {
        Ok(FirstLockObservation::Confirmed(confirmed(
            submission.step(),
            *submission.expected_submission_id(),
        )))
    }

    async fn submit_first_lock(
        &self,
        _agreement: &ZecAgreementV1,
        _submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[async_trait]
impl ZcashFirstLockPort for TestPorts {
    type Error = Infallible;

    async fn observe_first_lock(
        &self,
        _agreement: &ZecAgreementV1,
        submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<FirstLockObservation, Self::Error> {
        Ok(FirstLockObservation::Confirmed(confirmed(
            submission.step(),
            *submission.expected_submission_id(),
        )))
    }

    async fn submit_first_lock(
        &self,
        _agreement: &ZecAgreementV1,
        _submission: &PreparedFirstLockSubmissionV1,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[async_trait]
impl LezTakerFirstLockObservationPort for TestPorts {
    type Error = Infallible;

    async fn observe_taker_first_lock(
        &self,
        _agreement: &ZecAgreementV1,
        _previous: Option<&CanonicalLezEscrowObservationV1>,
    ) -> Result<TakerFirstLockObservationV1, Self::Error> {
        Ok(self
            .taker_lez
            .clone()
            .map_or(TakerFirstLockObservationV1::Unstable, |canonical| {
                TakerFirstLockObservationV1::CanonicalLez(Box::new(canonical))
            }))
    }
}

#[async_trait]
impl ZcashTakerFirstLockObservationPort for TestPorts {
    type Error = Infallible;

    async fn observe_taker_first_lock(
        &self,
        _agreement: &ZecAgreementV1,
        _previous: Option<&lez_zec_swap_sdk::CanonicalZcashOutputObservation>,
    ) -> Result<TakerFirstLockObservationV1, Self::Error> {
        Ok(TakerFirstLockObservationV1::Unstable)
    }
}

#[async_trait]
impl LezMakerLockObservationPort for TestPorts {
    type Error = Infallible;

    async fn observe_maker_lock(
        &self,
        _agreement: &ZecAgreementV1,
    ) -> Result<MakerLockObservationV1, Self::Error> {
        Ok(MakerLockObservationV1::Confirmed(confirmed(
            FirstLockStepV1::LezFund,
            LEZ_FUNDING_BYTES,
        )))
    }
}

#[async_trait]
impl ZcashMakerLockObservationPort for TestPorts {
    type Error = Infallible;

    async fn observe_maker_lock(
        &self,
        _agreement: &ZecAgreementV1,
    ) -> Result<MakerLockObservationV1, Self::Error> {
        Ok(MakerLockObservationV1::Confirmed(confirmed(
            FirstLockStepV1::ZcashFund,
            [0x51; 32],
        )))
    }
}

#[derive(Clone, Copy)]
struct NoDiscovery;

#[async_trait]
impl OfferDiscovery for NoDiscovery {
    type Error = Infallible;
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

#[derive(Clone, Copy)]
struct NoNegotiation;

#[async_trait]
impl NegotiationChannel for NoNegotiation {
    type Error = Infallible;
    type LocalProposal = ();
    type OfferRef = ();

    async fn negotiate(
        &self,
        _local_participant: Participant,
        _offer: &Self::OfferRef,
        _proposal: Self::LocalProposal,
    ) -> Result<Vec<u8>, Self::Error> {
        Ok(Vec::new())
    }
}

fn prepared(step: FirstLockStepV1, identity: [u8; 32]) -> PreparedFirstLockSubmissionV1 {
    PreparedFirstLockSubmissionV1::new(step, identity, vec![0xa1, 0xb2])
        .expect("prepared first-lock submission")
}

fn confirmed(step: FirstLockStepV1, identity: [u8; 32]) -> FirstLockConfirmedEvidenceV1 {
    FirstLockConfirmedEvidenceV1::new(step, identity, lowerhex(identity), 10)
        .expect("confirmed first-lock evidence")
}

fn accept(agreement: &ZecAgreementV1, role: Participant) -> AcceptedZecAgreementV1 {
    AcceptedZecAgreementV1::accept_wire_at(
        &agreement.encode_wire().expect("agreement wire"),
        UnixSeconds::new(10),
        role,
        0,
    )
    .expect("accepted agreement")
}

#[allow(clippy::too_many_lines)]
fn signed_agreement(
    id: &str,
    direction: SwapDirection,
    changed_transcript: bool,
) -> ZecAgreementV1 {
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
    let digest: [u8; 32] = Sha256::digest([9; 32]).into();
    let binding = ZecSwapBinding::new(
        ZecProfileId::DeterministicLocalV1,
        ExpectedBip199Output::new(
            NetworkType::Regtest,
            BranchId::Nu6_2,
            Zatoshis::from_u64(100_000_000).expect("principal"),
            Bip199Contract::new(120, refund_hash, digest, claimant_hash),
        ),
    )
    .expect("binding");
    let escrow_program = [1; 8];
    let onchain_swap_id = derive_lez_swap_id_v1(id.as_bytes());
    let metadata = derive_lez_metadata_account_v1(&escrow_program, &onchain_swap_id);
    let custody = derive_lez_native_custody_account_v1(&escrow_program, &onchain_swap_id);
    let body = ZecAgreementBodyV1::new(
        id,
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
            42,
            metadata,
            custody,
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
            if changed_transcript {
                [0x66; 32]
            } else {
                [6; 32]
            },
            1_000,
        ),
    );
    let commitment = body.commitment();
    let record = ZecAgreementRecordV1::from_parts(
        ZEC_CONCRETE_AGREEMENT_SCHEMA_V2,
        body,
        commitment,
        secp.sign_ecdsa(&Message::from_digest(commitment), &maker_secret)
            .serialize_compact(),
        secp.sign_ecdsa(&Message::from_digest(commitment), &taker_secret)
            .serialize_compact(),
    );
    ZecAgreementV1::from_wire_at(
        &record.encode_wire().expect("bounded agreement"),
        UnixSeconds::new(10),
    )
    .expect("valid agreement")
}

fn canonical_lez_taker_lock(agreement: &ZecAgreementV1) -> CanonicalLezEscrowObservationV1 {
    let metadata_version = match agreement.lez_terms().chain().environment() {
        LezEnvironmentV1::DeterministicLocalV0_1_2Compatibility => 1,
        LezEnvironmentV1::DeterministicLocalV0_2 | LezEnvironmentV1::PublicTestnetV0_2 => 2,
    };
    CanonicalLezEscrowObservationV1::validate(
        agreement,
        &lez_taker_lock_snapshot(agreement, metadata_version),
    )
    .expect("canonical taker LEZ observation")
}

fn lez_taker_lock_snapshot(agreement: &ZecAgreementV1, metadata_version: u8) -> LezNodeSnapshotV1 {
    let terms = agreement.lez_terms();
    let LezAssetV1::Native {
        authenticated_transfer_program_id,
    } = terms.asset()
    else {
        panic!("fixture uses native LEZ")
    };
    let depositor = *agreement.lez_account(agreement.lez_depositor());
    let claimant = *agreement.lez_account(agreement.lez_claimant());
    let metadata = LezEscrowMetadataSnapshotV1::new(
        metadata_version,
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
    LezNodeSnapshotV1::new(
        terms.chain().environment(),
        *terms.chain().channel_id(),
        *terms.chain().genesis_block_hash(),
        LezStableTipV1::new([0x42; 32], 102, [0x42; 32], 102),
        LezFundTransactionSnapshotV1::new(
            [0x31; 32],
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
            100,
            [0x41; 32],
            [0x41; 32],
            LezInclusionStatusV1::Pending,
        ),
        *terms.escrow_program_id(),
        *terms.metadata_account(),
        metadata,
        *terms.custody_account(),
        LezCustodySnapshotV1::Native {
            program_owner: *authenticated_transfer_program_id,
            balance: terms.amount(),
        },
    )
}

fn lowerhex(bytes: [u8; 32]) -> String {
    let mut encoded = String::with_capacity(64);
    for byte in bytes {
        write!(&mut encoded, "{byte:02x}").expect("write to String");
    }
    encoded
}

fn pubkey_hash(bytes: &[u8; 33]) -> [u8; 20] {
    match TransparentAddress::from_pubkey(&PublicKey::from_slice(bytes).expect("public key")) {
        TransparentAddress::PublicKeyHash(hash) => hash,
        TransparentAddress::ScriptHash(_) => unreachable!("public key yields P2PKH"),
    }
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "lez-canonical-funding-{label}-{}-{}",
            std::process::id(),
            TEST_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("create test directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
