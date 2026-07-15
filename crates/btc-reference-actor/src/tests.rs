use std::{
    fs::{self, OpenOptions},
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _, symlink},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use super::*;
use bitcoin::{
    Amount, OutPoint, ScriptBuf, TxOut, Txid,
    hashes::Hash as _,
    secp256k1::{Keypair, Message, PublicKey, Secp256k1, SecretKey},
};
use lez_bridge_protocol::{
    AccountIds, ChainPosition, CompleteWitnessedClaimRequest, CompleteWitnessedClaimResult,
    EscrowState, ExactMessageBytes, ExactTransactionBytes, FinalizedBlockIdentity,
    FinalizedWitnessedFundingFacts, NativeCustodyFacts, NativeFundInstructionFacts,
    ObservedTransactionFacts, PrepareWitnessedClaimResult, PreparedTransaction,
    PreparedWitnessedClaim, SubmissionOutcome, SubmitTransactionRequest, SubmitTransactionResult,
    TransactionId, WitnessedClaimInstructionFacts, WitnessedEscrowMetadataFacts,
};
use lez_btc_swap_sdk::{
    AdaptorSessionContext, BTC_AGREEMENT_SCHEMA_V1, BtcAgreementBodyV1, BtcAgreementRecordV1,
    BtcChainPolicyV1, BtcClaimTermsV1, BtcFundingTermsV1, BtcLezTermsV1, BtcP2trTermsV1,
    BtcParticipantIdentityV1, BtcParticipantsV1, BtcRecoveryPlanV1, CsvBlockDelay,
    FreshAdaptorNonce, P2trSwapOutput, PersistedAdaptorSigningMaterial, RefundXOnlyKey,
    SigningRole, TwoPartyAggregateKey, aggregate_adaptor_presignature,
    sign_persisted_adaptor_partial, verify_adaptor_partial_signature, verify_nonce_commitment,
};
use lez_swap_core::SwapDirection;
use lez_swap_store::{
    AdaptorNonceCommitment, AdaptorPartialSignature, AdaptorPresignature, AdaptorPublicNonce,
    AdaptorSessionReservation, SecretNonceBytes,
};
use serde_json::Value;
use tempfile::TempDir;
use tokio::sync::Barrier;

#[allow(dead_code)]
#[path = "../../btc-core-adapter/tests/support.rs"]
mod support;

struct ActorFixture {
    directory: TempDir,
    config: ActorConfig,
    agreement: BtcAgreementV1,
    agreement_wire: Vec<u8>,
}

impl ActorFixture {
    fn new() -> Self {
        Self::with_agreement(
            support::swap_fixture().agreement,
            ActorRole::Taker,
            support::MAKER_SECRET,
            support::TAKER_SECRET,
            support::ADAPTOR_SECRET,
            true,
        )
    }

    fn without_activation_material() -> Self {
        Self::with_agreement(
            support::swap_fixture().agreement,
            ActorRole::Taker,
            support::MAKER_SECRET,
            support::TAKER_SECRET,
            support::ADAPTOR_SECRET,
            false,
        )
    }

    fn for_direction(direction: SwapDirection, role: ActorRole) -> Self {
        Self::with_agreement(
            directional_agreement(direction),
            role,
            [1; 32],
            [2; 32],
            [7; 32],
            true,
        )
    }

    fn with_agreement(
        agreement: BtcAgreementV1,
        role: ActorRole,
        maker_secret: [u8; 32],
        taker_secret: [u8; 32],
        adaptor_secret: [u8; 32],
        seed_material: bool,
    ) -> Self {
        let directory = tempfile::tempdir().expect("actor tempdir");
        let agreement_wire = agreement.encode_wire().expect("agreement wire");
        let agreement_file = directory.path().join("agreement.json");
        fs::write(&agreement_file, &agreement_wire).expect("write agreement");
        let config = ActorConfig {
            schema_version: CONFIG_SCHEMA_VERSION,
            role,
            agreement_file,
            state_db: directory.path().join("actor.sqlite3"),
            accepted_at_unix_seconds: 1_700_000_000,
            bitcoin_core: BitcoinCoreConfig {
                endpoint: "http://127.0.0.1:1".into(),
                cookie_file: directory.path().join("bitcoin.cookie"),
                connectivity: BitcoinConnectivity::IsolatedLocal,
            },
            lez_bridge: LezBridgeConfig {
                endpoint: "http://127.0.0.1:2".into(),
                capability_file: directory.path().join("lez.capability"),
                run_id: RunId::new("m3-internal-actor-test").expect("run ID"),
                runtime: RuntimeDescriptor::new(
                    role.bridge(),
                    RuntimeCompatibility::LeeV0_2_0,
                    Hex32::from_bytes([99; 32]),
                    Hex32::from_bytes([17; 32]),
                    Hex32::from_bytes([18; 32]),
                    Hex32::from_bytes([15; 32]),
                    Hex32::from_bytes(match role {
                        ActorRole::Maker => [10; 32],
                        ActorRole::Taker => [11; 32],
                    }),
                ),
                request_timeout_millis: 1_000,
                discovery_start_height: 1,
                discovery_max_blocks: 10,
            },
            signing: ClaimRecoveryConfig {
                bitcoin: SigningSessionConfig {
                    session_id: Hex32::from_bytes([41; 32]),
                    journal_db: directory.path().join("bitcoin-adaptor.sqlite3"),
                },
                lez: SigningSessionConfig {
                    session_id: Hex32::from_bytes([42; 32]),
                    journal_db: directory.path().join("lez-adaptor.sqlite3"),
                },
                prepared_witnessed_claim_result_file: directory
                    .path()
                    .join("prepared-witnessed-claim.json"),
                adaptor_secret_file: (role == ActorRole::Taker)
                    .then(|| directory.path().join("adaptor-secret.key")),
            },
        };
        config.validate().expect("valid test config");
        if seed_material {
            if let Some(path) = &config.signing.adaptor_secret_file {
                write_private_secret(path, adaptor_secret);
            }
            seed_activation_material(&config, &agreement, maker_secret, taker_secret);
        }
        Self {
            directory,
            config,
            agreement,
            agreement_wire,
        }
    }
}

fn write_private_secret(path: &Path, secret: [u8; 32]) {
    fs::write(path, format!("{}\n", hex::encode(secret))).expect("write adaptor secret");
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .expect("owner-only adaptor secret");
}

fn seed_activation_material(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    maker_secret: [u8; 32],
    taker_secret: [u8; 32],
) {
    let preparation_request_id =
        RequestId::new("prepared-claim-001").expect("preparation request ID");
    let prepared = PrepareWitnessedClaimResult::new(
        MessageContext::new(
            config.lez_bridge.run_id.clone(),
            preparation_request_id.clone(),
            bridge_participant(agreement.lez_claimant()),
        ),
        PreparedWitnessedClaim::new(
            preparation_request_id,
            Hex32::from_bytes(support::lez_claim_message_hash()),
            ExactMessageBytes::new(support::LEZ_PREPARED_MESSAGE_BYTES.to_vec())
                .expect("prepared message bytes"),
        ),
    );
    fs::write(
        &config.signing.prepared_witnessed_claim_result_file,
        serde_json::to_vec(&prepared).expect("prepared claim JSON"),
    )
    .expect("write prepared claim");

    seed_signing_journal(
        config,
        agreement,
        BtcAdaptorSessionDomain::Bitcoin,
        &config.signing.bitcoin,
        maker_secret,
        taker_secret,
    );
    seed_signing_journal(
        config,
        agreement,
        BtcAdaptorSessionDomain::Lez,
        &config.signing.lez,
        maker_secret,
        taker_secret,
    );
}

#[allow(clippy::too_many_lines)] // The fixture spells out every durable ceremony transition.
fn seed_signing_journal(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    domain: BtcAdaptorSessionDomain,
    signing: &SigningSessionConfig,
    maker_secret: [u8; 32],
    taker_secret: [u8; 32],
) {
    let session_id = *signing.session_id.as_bytes();
    let context = agreement
        .adaptor_session_context(domain, session_id)
        .expect("agreement-derived signing context");
    let maker_nonce = FreshAdaptorNonce::generate(&context, SigningRole::Maker, maker_secret)
        .expect("maker fresh nonce");
    let taker_nonce = FreshAdaptorNonce::generate(&context, SigningRole::Taker, taker_secret)
        .expect("taker fresh nonce");
    let (local_role, local_secret, local_nonce, peer_role, peer_secret, peer_nonce) =
        match config.role {
            ActorRole::Maker => (
                SigningRole::Maker,
                maker_secret,
                &maker_nonce,
                SigningRole::Taker,
                taker_secret,
                &taker_nonce,
            ),
            ActorRole::Taker => (
                SigningRole::Taker,
                taker_secret,
                &taker_nonce,
                SigningRole::Maker,
                maker_secret,
                &maker_nonce,
            ),
        };
    let identity = AdaptorSessionIdentity::new(
        session_id,
        config.role.signer(),
        context.durable_context_binding(),
        context.message(),
        context.adaptor_point(),
        context.ordered_public_keys(),
    );
    let mut journal =
        SqliteAdaptorSessionJournal::open(&signing.journal_db).expect("create signer journal");
    let _ = journal
        .reserve(AdaptorSessionReservation::new(
            identity.clone(),
            SecretNonceBytes::new(*local_nonce.secret_nonce()),
            AdaptorPublicNonce::new(local_nonce.public_nonce()),
            AdaptorNonceCommitment::new(local_nonce.commitment()),
        ))
        .expect("reserve local nonce");
    let _ = journal
        .record_peer_commitment(
            &identity,
            AdaptorNonceCommitment::new(peer_nonce.commitment()),
        )
        .expect("record peer commitment");
    verify_nonce_commitment(
        &context,
        peer_role,
        peer_nonce.commitment(),
        peer_nonce.public_nonce(),
    )
    .expect("verify peer nonce");
    let _ = journal
        .record_verified_peer_public_nonce(
            &identity,
            AdaptorPublicNonce::new(peer_nonce.public_nonce()),
        )
        .expect("record peer nonce");
    let own_partial = journal
        .sign_and_persist_partial(&identity, |material| {
            sign_persisted_adaptor_partial(
                &context,
                local_role,
                local_secret,
                PersistedAdaptorSigningMaterial::new(
                    *material.identity().signing_domain(),
                    material.secret_nonce(),
                    *material.own_public_nonce().bytes(),
                    local_nonce.commitment(),
                    peer_nonce.commitment(),
                    *material.peer_public_nonce().bytes(),
                ),
            )
            .map(AdaptorPartialSignature::new)
            .map_err(|_| ())
        })
        .expect("persist local partial")
        .partial();
    let peer_partial = sign_persisted_adaptor_partial(
        &context,
        peer_role,
        peer_secret,
        PersistedAdaptorSigningMaterial::new(
            context.durable_context_binding(),
            peer_nonce.secret_nonce(),
            peer_nonce.public_nonce(),
            peer_nonce.commitment(),
            local_nonce.commitment(),
            local_nonce.public_nonce(),
        ),
    )
    .expect("peer partial");
    let (maker_public_nonce, taker_public_nonce, maker_partial, taker_partial) = match config.role {
        ActorRole::Maker => (
            local_nonce.public_nonce(),
            peer_nonce.public_nonce(),
            *own_partial.bytes(),
            peer_partial,
        ),
        ActorRole::Taker => (
            peer_nonce.public_nonce(),
            local_nonce.public_nonce(),
            peer_partial,
            *own_partial.bytes(),
        ),
    };
    verify_adaptor_partial_signature(
        &context,
        peer_role,
        maker_public_nonce,
        taker_public_nonce,
        peer_partial,
    )
    .expect("verify peer partial");
    let _ = journal
        .record_verified_peer_partial(&identity, AdaptorPartialSignature::new(peer_partial))
        .expect("record peer partial");
    let presignature = aggregate_adaptor_presignature(
        &context,
        maker_public_nonce,
        taker_public_nonce,
        maker_partial,
        taker_partial,
    )
    .expect("aggregate presignature");
    let _ = journal
        .record_verified_presignature(&identity, AdaptorPresignature::new(presignature))
        .expect("record presignature");
}

fn test_secret(byte: u8) -> SecretKey {
    SecretKey::from_slice(&[byte; 32]).expect("valid fixture secret")
}

fn compressed_public_key(secret: &SecretKey) -> [u8; 33] {
    PublicKey::from_secret_key(&Secp256k1::new(), secret).serialize()
}

fn x_only_public_key(secret: &SecretKey) -> [u8; 32] {
    Keypair::from_secret_key(&Secp256k1::new(), secret)
        .x_only_public_key()
        .0
        .serialize()
}

fn claim_destination(secret: &SecretKey) -> Vec<u8> {
    let key = Keypair::from_secret_key(&Secp256k1::new(), secret)
        .x_only_public_key()
        .0;
    ScriptBuf::new_p2tr(&Secp256k1::verification_only(), key, None).into_bytes()
}

fn agreement_signature(secret: &SecretKey, commitment: [u8; 32]) -> [u8; 64] {
    let secp = Secp256k1::new();
    secp.sign_schnorr_no_aux_rand(
        &Message::from_digest(commitment),
        &Keypair::from_secret_key(&secp, secret),
    )
    .serialize()
}

#[allow(clippy::too_many_lines)]
fn directional_agreement(direction: SwapDirection) -> BtcAgreementV1 {
    let maker_secret = test_secret(1);
    let taker_secret = test_secret(2);
    let maker_refund_secret = test_secret(3);
    let taker_refund_secret = test_secret(4);
    let maker_claim_secret = test_secret(5);
    let taker_claim_secret = test_secret(6);
    let adaptor_secret = test_secret(7);
    let participants = BtcParticipantsV1::new(
        BtcParticipantIdentityV1::new(
            [10; 32],
            compressed_public_key(&maker_secret),
            x_only_public_key(&maker_refund_secret),
            claim_destination(&maker_claim_secret),
        ),
        BtcParticipantIdentityV1::new(
            [11; 32],
            compressed_public_key(&taker_secret),
            x_only_public_key(&taker_refund_secret),
            claim_destination(&taker_claim_secret),
        ),
    );
    let adaptor_point = compressed_public_key(&adaptor_secret);
    let aggregate_key = AdaptorSessionContext::untweaked(
        [
            compressed_public_key(&maker_secret),
            compressed_public_key(&taker_secret),
        ],
        [30; 32],
        adaptor_point,
        [31; 32],
    )
    .expect("aggregate context")
    .output_key();
    let bitcoin_funder = match direction {
        SwapDirection::TakerSellsForeign => Participant::Taker,
        SwapDirection::TakerSellsLez => Participant::Maker,
    };
    let refund_key = match bitcoin_funder {
        Participant::Maker => x_only_public_key(&maker_refund_secret),
        Participant::Taker => x_only_public_key(&taker_refund_secret),
    };
    let contract = P2trSwapOutput::new(
        TwoPartyAggregateKey::from_bytes(aggregate_key).expect("aggregate key"),
        RefundXOnlyKey::from_bytes(refund_key).expect("refund key"),
        CsvBlockDelay::new(144).expect("CSV"),
    )
    .expect("P2TR contract");
    let funding = BtcFundingTermsV1::new([21; 32], 1, 100_000);
    let claim = lez_btc_swap_sdk::CooperativeKeyPathSpend::new(
        &contract,
        OutPoint {
            txid: Txid::from_byte_array(*funding.transaction_id()),
            vout: funding.output_index(),
        },
        Amount::from_sat(funding.value_sat()),
        vec![TxOut {
            value: Amount::from_sat(99_000),
            script_pubkey: ScriptBuf::from_bytes(
                participants
                    .for_participant(bitcoin_funder.other())
                    .claim_destination_script_pubkey()
                    .to_vec(),
            ),
        }],
    )
    .expect("cooperative claim");
    let lez_depositor = match direction {
        SwapDirection::TakerSellsForeign => Participant::Maker,
        SwapDirection::TakerSellsLez => Participant::Taker,
    };
    let lez_claimant = lez_depositor.other();
    let body = BtcAgreementBodyV1::new(
        [20; 32],
        direction,
        BtcChainPolicyV1::new([8; 32], support::REQUIRED_CONFIRMATIONS),
        participants.clone(),
        adaptor_point,
        BtcLezTermsV1::new(
            [17; 32],
            [18; 32],
            [15; 32],
            [16; 32],
            [12; 32],
            [13; 32],
            [14; 32],
            *participants
                .for_participant(lez_depositor)
                .lez_owner_account(),
            *participants
                .for_participant(lez_claimant)
                .lez_owner_account(),
            5_000,
            match direction {
                SwapDirection::TakerSellsForeign => 1_700_000_100_000,
                SwapDirection::TakerSellsLez => 1_700_000_500_000,
            },
            support::lez_claim_message_hash(),
        ),
        BtcP2trTermsV1::from_contract(&contract),
        funding,
        BtcClaimTermsV1::from_spend(&claim).expect("claim terms"),
        BtcRecoveryPlanV1::new(1_000, 1_144, 1_700_000_100, 1_700_000_500, 300),
    );
    let commitment = body.commitment();
    BtcAgreementV1::validate(BtcAgreementRecordV1::from_parts(
        BTC_AGREEMENT_SCHEMA_V1,
        body,
        commitment,
        agreement_signature(&maker_secret, commitment),
        agreement_signature(&taker_secret, commitment),
    ))
    .expect("valid directional agreement")
}

struct FixedObserver {
    observation: ActorFundingObservation,
    calls: AtomicUsize,
    transitions: Mutex<Vec<FundingTransition>>,
}

impl FixedObserver {
    fn new(observation: ActorFundingObservation) -> Self {
        Self {
            observation,
            calls: AtomicUsize::new(0),
            transitions: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn transitions(&self) -> Vec<FundingTransition> {
        self.transitions.lock().expect("transition log").clone()
    }
}

#[async_trait]
impl FundingObservationPort for FixedObserver {
    async fn observe(
        &self,
        _agreement: &BtcAgreementV1,
        transition: FundingTransition,
    ) -> Result<ActorFundingObservation, ActorCommandError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.transitions
            .lock()
            .expect("transition log")
            .push(transition);
        Ok(self.observation.clone())
    }
}

struct BarrierObserver {
    barrier: Arc<Barrier>,
    observation: ActorFundingObservation,
    expected_transition: FundingTransition,
}

#[async_trait]
impl FundingObservationPort for BarrierObserver {
    async fn observe(
        &self,
        _agreement: &BtcAgreementV1,
        transition: FundingTransition,
    ) -> Result<ActorFundingObservation, ActorCommandError> {
        assert_eq!(transition, self.expected_transition);
        self.barrier.wait().await;
        Ok(self.observation.clone())
    }
}

fn output_json(output: impl Serialize) -> Value {
    serde_json::to_value(output).expect("secret-free actor output")
}

async fn activate_and_project_taker_lock(fixture: &ActorFixture) {
    execute_actor_command(&fixture.config, ActorCommand::Activate)
        .await
        .expect("activate actor");
    let mut store = open_existing_store(
        &fixture.config,
        &fixture.agreement,
        fixture.agreement_wire.clone(),
    )
    .expect("open activated actor");
    let chain = fixture
        .agreement
        .coordinator()
        .funded_chain(Participant::Taker);
    let confirmations = if chain == Chain::Bitcoin {
        support::REQUIRED_CONFIRMATIONS
    } else {
        FINALIZED_LEZ_CONFIRMATION_UNITS
    };
    let _ = store
        .project(
            0,
            &BtcLifecycleEvidenceV1::taker_lock(
                chain,
                "canonical-taker-lock",
                confirmations,
                b"canonical-taker-lock-evidence".to_vec(),
            )
            .expect("taker lock evidence"),
        )
        .expect("project taker lock");
}

fn maker_lock_observation(
    agreement: &BtcAgreementV1,
    evidence_suffix: u8,
) -> ActorFundingObservation {
    let chain = agreement.coordinator().funded_chain(Participant::Maker);
    ActorFundingObservation::Ready {
        chain,
        transaction_id: "canonical-maker-lock".into(),
        confirmations: if chain == Chain::Bitcoin {
            support::REQUIRED_CONFIRMATIONS
        } else {
            FINALIZED_LEZ_CONFIRMATION_UNITS
        },
        chain_evidence: vec![b'm', b'a', b'k', b'e', b'r', evidence_suffix],
    }
}

#[test]
fn claim_recovery_config_requires_distinct_sessions_and_paths() {
    let fixture = ActorFixture::new();
    let mut config = fixture.config.clone();
    let valid = config.signing.clone();
    config
        .validate()
        .expect("distinct claim recovery authority");

    let mut zero = valid.clone();
    zero.bitcoin.session_id = Hex32::from_bytes([0; 32]);
    config.signing = zero;
    assert_eq!(config.validate(), Err(ActorConfigError::Invalid));

    let mut reused_id = valid.clone();
    reused_id.lez.session_id = reused_id.bitcoin.session_id;
    config.signing = reused_id;
    assert_eq!(config.validate(), Err(ActorConfigError::Invalid));

    let mut missing_taker_secret = valid.clone();
    missing_taker_secret.adaptor_secret_file = None;
    config.signing = missing_taker_secret;
    assert_eq!(config.validate(), Err(ActorConfigError::Invalid));

    let mut aliased_path = valid.clone();
    aliased_path.lez.journal_db = aliased_path.bitcoin.journal_db.clone();
    config.signing = aliased_path;
    assert_eq!(config.validate(), Err(ActorConfigError::Invalid));

    let mut aliased_secret = valid;
    aliased_secret.adaptor_secret_file = Some(aliased_secret.bitcoin.journal_db.clone());
    config.signing = aliased_secret;
    assert_eq!(config.validate(), Err(ActorConfigError::Invalid));

    let mut maker = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Maker);
    assert_eq!(maker.config.signing.adaptor_secret_file, None);
    maker.config.signing.adaptor_secret_file =
        Some(maker.directory.path().join("unexpected-maker-secret"));
    assert_eq!(maker.config.validate(), Err(ActorConfigError::Invalid));

    let debug = format!("{:?}", fixture.config);
    assert!(!debug.contains("adaptor-secret.key"));
    assert!(!debug.contains(&hex::encode(support::ADAPTOR_SECRET)));
}

#[tokio::test(flavor = "current_thread")]
async fn activation_requires_a_private_point_matching_taker_adaptor_secret() {
    for mutation in [
        "missing",
        "short",
        "wrong",
        "world_readable",
        "symlink",
        "hardlink",
    ] {
        let fixture = ActorFixture::new();
        let secret_path = fixture
            .config
            .signing
            .adaptor_secret_file
            .as_ref()
            .expect("taker secret path");
        match mutation {
            "missing" => fs::remove_file(secret_path).expect("remove secret"),
            "short" => {
                fs::write(secret_path, b"53\n").expect("write short secret");
            }
            "wrong" => write_private_secret(secret_path, [0x54; 32]),
            "world_readable" => {
                fs::set_permissions(secret_path, fs::Permissions::from_mode(0o644))
                    .expect("loosen secret permissions");
            }
            "symlink" => {
                let target = fixture.directory.path().join("secret-target");
                write_private_secret(&target, support::ADAPTOR_SECRET);
                fs::remove_file(secret_path).expect("remove original secret");
                symlink(&target, secret_path).expect("replace with symlink");
            }
            "hardlink" => {
                let alias = fixture.directory.path().join("secret-hardlink");
                fs::hard_link(secret_path, alias).expect("create hard-link alias");
            }
            _ => unreachable!("fixed mutation"),
        }
        let error = execute_actor_command(&fixture.config, ActorCommand::Activate)
            .await
            .expect_err("unsafe or mismatched secret must fail closed");
        assert_eq!(
            error,
            ActorCommandError::ActivationMaterialUnavailable,
            "mutation: {mutation}"
        );
        assert!(!fixture.config.state_db.exists(), "mutation: {mutation}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn activation_requires_existing_signing_material_before_state_creation() {
    let fixture = ActorFixture::without_activation_material();
    assert!(!fixture.config.state_db.exists());

    let error = execute_actor_command(&fixture.config, ActorCommand::Activate)
        .await
        .expect_err("missing signer journals and prepared claim must fail closed");
    assert_eq!(error, ActorCommandError::ActivationMaterialUnavailable);
    assert!(
        !fixture.config.state_db.exists(),
        "failed pre-lock validation must not create actor state"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn activation_rejects_crosswired_journals_and_prepared_result_drift() {
    for mutation in [
        "journal_domain",
        "run",
        "claimant",
        "request",
        "invalid_hash",
        "valid_other_message",
    ] {
        let mut fixture = ActorFixture::new();
        match mutation {
            "journal_domain" => {
                std::mem::swap(
                    &mut fixture.config.signing.bitcoin.journal_db,
                    &mut fixture.config.signing.lez.journal_db,
                );
            }
            "run" | "claimant" | "request" | "invalid_hash" | "valid_other_message" => {
                let bytes = fs::read(&fixture.config.signing.prepared_witnessed_claim_result_file)
                    .expect("prepared");
                let mut prepared: PrepareWitnessedClaimResult =
                    serde_json::from_slice(&bytes).expect("prepared result");
                match mutation {
                    "run" => {
                        prepared.context.run_id =
                            RunId::new("different-run").expect("different run");
                    }
                    "claimant" => {
                        prepared.context.sidecar_role = match prepared.context.sidecar_role {
                            BridgeParticipant::Maker => BridgeParticipant::Taker,
                            BridgeParticipant::Taker => BridgeParticipant::Maker,
                        };
                    }
                    "request" => {
                        prepared.context.request_id =
                            RequestId::new("different-request").expect("different request");
                    }
                    "invalid_hash" => {
                        prepared.claim.message_hash = Hex32::from_bytes([1; 32]);
                    }
                    "valid_other_message" => {
                        let other = b"internally-valid-but-not-agreement-bound".to_vec();
                        prepared.claim.message_hash =
                            Hex32::from_bytes(support::lez_message_hash(&other));
                        prepared.claim.exact_message_bytes =
                            ExactMessageBytes::new(other).expect("other message");
                    }
                    _ => unreachable!("fixed mutation"),
                }
                fs::write(
                    &fixture.config.signing.prepared_witnessed_claim_result_file,
                    serde_json::to_vec(&prepared).expect("mutated result"),
                )
                .expect("rewrite prepared result");
            }
            _ => unreachable!("fixed mutation"),
        }
        fixture
            .config
            .validate()
            .expect("structurally valid config");
        let error = execute_actor_command(&fixture.config, ActorCommand::Activate)
            .await
            .expect_err("crosswired activation material must fail closed");
        assert_eq!(
            error,
            ActorCommandError::ActivationMaterialUnavailable,
            "mutation: {mutation}"
        );
        assert!(!fixture.config.state_db.exists(), "mutation: {mutation}");
    }
}

#[test]
fn finalized_lez_request_identity_binds_the_complete_discovery_window() {
    let fixture = ActorFixture::new();
    let original = finalized_lez_funding_request(&fixture.config, &fixture.agreement)
        .expect("original request");
    let exact_retry =
        finalized_lez_funding_request(&fixture.config, &fixture.agreement).expect("exact retry");
    assert_eq!(exact_retry, original);
    assert_eq!(original.context.request_id.as_str().len(), 64);

    let mut changed_start_config = fixture.config.clone();
    changed_start_config.lez_bridge.discovery_start_height += 1;
    let changed_start = finalized_lez_funding_request(&changed_start_config, &fixture.agreement)
        .expect("changed-start request");
    assert_ne!(
        changed_start.context.request_id,
        original.context.request_id
    );
    assert_eq!(
        changed_start.window.start_height(),
        original.window.start_height() + 1
    );
    assert_eq!(
        changed_start.window.max_blocks(),
        original.window.max_blocks()
    );
    assert_eq!(changed_start.terms, original.terms);
    assert_eq!(changed_start.runtime, original.runtime);

    let mut changed_max_config = fixture.config.clone();
    changed_max_config.lez_bridge.discovery_max_blocks += 1;
    let changed_max = finalized_lez_funding_request(&changed_max_config, &fixture.agreement)
        .expect("changed-max request");
    assert_ne!(changed_max.context.request_id, original.context.request_id);
    assert_ne!(
        changed_max.context.request_id,
        changed_start.context.request_id
    );
    assert_eq!(
        changed_max.window.start_height(),
        original.window.start_height()
    );
    assert_eq!(
        changed_max.window.max_blocks(),
        original.window.max_blocks() + 1
    );
    assert_eq!(changed_max.terms, original.terms);
    assert_eq!(changed_max.runtime, original.runtime);
}

fn finalized_funding_facts(
    request: &ObserveFinalizedWitnessedFundingRequest,
    agreement: &BtcAgreementV1,
) -> FinalizedWitnessedFundingFacts {
    let block_hash = Hex32::from_bytes([93; 32]);
    let metadata_id = Hex32::from_bytes(*agreement.lez_terms().metadata_account());
    let custody_id = Hex32::from_bytes(*agreement.lez_terms().custody_account());
    FinalizedWitnessedFundingFacts::new(
        ObservedTransactionFacts::new(
            TransactionId::from_bytes([90; 32]),
            ExactTransactionBytes::new(vec![90; 128]).expect("exact transaction bytes"),
            ChainPosition::new(block_hash, 4, 0),
            AccountIds::new(vec![request.terms.depositor_account_id()])
                .expect("single funding signer"),
            true,
        ),
        NativeFundInstructionFacts::new(
            request.runtime.escrow_program_id,
            AccountIds::new(vec![
                metadata_id,
                custody_id,
                request.terms.depositor_account_id(),
            ])
            .expect("fund account order"),
            request.terms.swap_id(),
        ),
        FinalizedBlockIdentity::new(4, block_hash, 1_850_000_000_050),
        WitnessedEscrowMetadataFacts::from_witnessed_native_terms(
            metadata_id,
            request.runtime.escrow_program_id,
            custody_id,
            &request.terms,
            EscrowState::Funded,
        ),
        NativeCustodyFacts::new(
            custody_id,
            request.terms.authenticated_transfer_program_id(),
            request.terms.amount().as_u128(),
        ),
    )
}

#[test]
fn finalized_lez_evidence_retains_the_ancestry_tip() {
    let fixture = ActorFixture::new();
    let request = finalized_lez_funding_request(&fixture.config, &fixture.agreement)
        .expect("signed witnessed terms");
    let funding = finalized_funding_facts(&request, &fixture.agreement);
    let finalized_tip = ChainTip::new(Hex32::from_bytes([95; 32]), 11);

    let encoded = encode_finalized_lez_funding_evidence(
        &fixture.config,
        &fixture.agreement,
        &request,
        finalized_tip,
        &funding,
    )
    .expect("durable LEZ evidence");
    let decoded =
        decode_finalized_lez_funding_evidence(&fixture.config, &fixture.agreement, &encoded)
            .expect("offline evidence audit");
    assert_eq!(decoded.request, request);
    let value: Value = serde_json::from_slice(&encoded).expect("evidence JSON");
    let keys: std::collections::BTreeSet<_> = value
        .as_object()
        .expect("evidence object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        std::collections::BTreeSet::from([
            "agreement_commitment",
            "finalized_tip",
            "funding",
            "request",
            "schema_version",
        ])
    );
    assert_eq!(
        value["request"]["context"]["run_id"],
        "m3-internal-actor-test"
    );
    assert_eq!(value["request"]["runtime"]["compatibility"], "lee_v0_2_0");
    assert_eq!(value["request"]["window"]["start_height"], 1);
    assert_eq!(
        value["request"]["terms"]["terms_hash"],
        hex::encode(fixture.agreement.agreement_commitment())
    );
    assert_eq!(value["finalized_tip"]["height"], 11);
    assert_eq!(value["finalized_tip"]["block_hash"], hex::encode([95; 32]));
    assert_eq!(
        value["agreement_commitment"],
        hex::encode(fixture.agreement.agreement_commitment())
    );
    assert_eq!(value["funding"]["containing_block"]["block_id"], 4);

    for mutation in ["unknown", "missing", "changed_terms"] {
        let mut changed = value.clone();
        match mutation {
            "unknown" => {
                changed
                    .as_object_mut()
                    .expect("evidence object")
                    .insert("unexpected".to_owned(), Value::Bool(true));
            }
            "missing" => {
                changed
                    .as_object_mut()
                    .expect("evidence object")
                    .remove("request");
            }
            "changed_terms" => {
                changed["request"]["terms"]["terms_hash"] = Value::String("00".repeat(32));
            }
            _ => unreachable!("fixed mutation"),
        }
        assert!(
            decode_finalized_lez_funding_evidence(
                &fixture.config,
                &fixture.agreement,
                &serde_json::to_vec(&changed).expect("mutated JSON"),
            )
            .is_err(),
            "mutation must fail: {mutation}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn drive_requires_explicit_activation_before_observation() {
    let fixture = ActorFixture::new();
    let observer = FixedObserver::new(ActorFundingObservation::Pending {
        chain: Chain::Bitcoin,
    });

    let error = drive_with_observer(
        &fixture.config,
        fixture.agreement,
        fixture.agreement_wire,
        &observer,
    )
    .await
    .expect_err("drive must not implicitly activate");
    assert_eq!(error, ActorCommandError::NotActivated);
    assert_eq!(observer.calls(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn private_empty_or_interrupted_database_is_not_activation() {
    let fixture = ActorFixture::new();
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&fixture.config.state_db)
        .expect("precreate private empty database");
    let observer = FixedObserver::new(ActorFundingObservation::Pending {
        chain: Chain::Bitcoin,
    });

    for _ in 0..2 {
        let status = output_json(
            execute_actor_command(&fixture.config, ActorCommand::Status)
                .await
                .expect("empty or migrated no-acceptance status"),
        );
        assert_eq!(status["state"], "not_activated");
        let error = drive_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &observer,
        )
        .await
        .expect_err("no-acceptance database cannot drive");
        assert_eq!(error, ActorCommandError::NotActivated);
    }
    assert_eq!(observer.calls(), 0);

    let first = output_json(
        execute_actor_command(&fixture.config, ActorCommand::Activate)
            .await
            .expect("explicit first activation"),
    );
    assert_eq!(first["was_replay"], false);
    let replay = output_json(
        execute_actor_command(&fixture.config, ActorCommand::Activate)
            .await
            .expect("exact activation replay"),
    );
    assert_eq!(replay["was_replay"], true);
}

#[tokio::test(flavor = "current_thread")]
async fn ready_first_lock_is_observed_then_projected_once() {
    let fixture = ActorFixture::new();
    execute_actor_command(&fixture.config, ActorCommand::Activate)
        .await
        .expect("activate actor");
    let observer = FixedObserver::new(ActorFundingObservation::Ready {
        chain: Chain::Bitcoin,
        transaction_id: support::swap_fixture()
            .funding
            .compute_txid()
            .to_string()
            .into_boxed_str(),
        confirmations: support::REQUIRED_CONFIRMATIONS,
        chain_evidence: b"canonical-adapter-evidence".to_vec(),
    });

    let projected = output_json(
        drive_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &observer,
        )
        .await
        .expect("project first lock"),
    );
    assert_eq!(projected["outcome"], "observed_then_projected");
    assert_eq!(projected["chain"], "bitcoin");
    assert_eq!(projected["revision"], 1);
    assert_eq!(projected["phase"], "taker_lock_confirmed");
    assert_eq!(observer.calls(), 1);
    let status = output_json(
        execute_actor_command(&fixture.config, ActorCommand::Status)
            .await
            .expect("offline revision-one status"),
    );
    assert_eq!(status["next_action"], "observe_maker_second_lock");
}

#[tokio::test(flavor = "current_thread")]
async fn maker_lock_projects_in_both_directions_for_both_roles() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        for role in [ActorRole::Maker, ActorRole::Taker] {
            let fixture = ActorFixture::for_direction(direction, role);
            activate_and_project_taker_lock(&fixture).await;
            let expected_chain = fixture
                .agreement
                .coordinator()
                .funded_chain(Participant::Maker);
            let observer = FixedObserver::new(maker_lock_observation(&fixture.agreement, 1));

            let projected = output_json(
                drive_with_observer(
                    &fixture.config,
                    fixture.agreement.clone(),
                    fixture.agreement_wire.clone(),
                    &observer,
                )
                .await
                .expect("project maker lock"),
            );
            assert_eq!(projected["outcome"], "observed_then_projected");
            assert_eq!(
                projected["chain"],
                match expected_chain {
                    Chain::Bitcoin => "bitcoin",
                    Chain::Lez => "lez",
                    Chain::Zcash | Chain::Monero => unreachable!("Bitcoin agreement chain set"),
                }
            );
            assert_eq!(projected["revision"], 2);
            assert_eq!(projected["phase"], "both_legs_locked");
            assert_eq!(observer.calls(), 1);
            assert_eq!(observer.transitions(), vec![FundingTransition::MakerLock]);

            let later = output_json(
                drive_with_observer(
                    &fixture.config,
                    fixture.agreement.clone(),
                    fixture.agreement_wire.clone(),
                    &observer,
                )
                .await
                .expect("revision two is outside this slice"),
            );
            assert_eq!(later["outcome"], "not_yet_composed");
            assert_eq!(later["durable_revision"], 2);
            assert_eq!(observer.calls(), 1, "revision two must not observe");

            let status = output_json(
                execute_actor_command(&fixture.config, ActorCommand::Status)
                    .await
                    .expect("offline revision-two status"),
            );
            assert_eq!(status["revision"], 2);
            assert_eq!(status["phase"], "both_legs_locked");
            assert_eq!(status["next_action"], "observe_revealing_claim");
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn pending_maker_lock_preserves_revision_one_in_both_directions() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        let fixture = ActorFixture::for_direction(direction, ActorRole::Taker);
        activate_and_project_taker_lock(&fixture).await;
        let expected_chain = fixture
            .agreement
            .coordinator()
            .funded_chain(Participant::Maker);
        let observer = FixedObserver::new(ActorFundingObservation::Pending {
            chain: expected_chain,
        });

        let pending = output_json(
            drive_with_observer(
                &fixture.config,
                fixture.agreement.clone(),
                fixture.agreement_wire.clone(),
                &observer,
            )
            .await
            .expect("maker lock remains pending"),
        );
        assert_eq!(pending["outcome"], "awaiting_observation");
        assert_eq!(pending["revision"], 1);
        assert_eq!(pending["phase"], "taker_lock_confirmed");
        assert_eq!(observer.calls(), 1);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn contradictory_maker_lock_chain_fails_before_projection() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Maker);
    activate_and_project_taker_lock(&fixture).await;
    let expected_chain = fixture
        .agreement
        .coordinator()
        .funded_chain(Participant::Maker);
    let observer = FixedObserver::new(ActorFundingObservation::Pending {
        chain: match expected_chain {
            Chain::Bitcoin => Chain::Lez,
            Chain::Lez => Chain::Bitcoin,
            Chain::Zcash | Chain::Monero => unreachable!("Bitcoin agreement chain set"),
        },
    });

    let error = drive_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &observer,
    )
    .await
    .expect_err("wrong maker-lock chain must fail closed");
    assert_eq!(error, ActorCommandError::AgreementBindingInvalid);
    let status = output_json(
        execute_actor_command(&fixture.config, ActorCommand::Status)
            .await
            .expect("offline status after contradiction"),
    );
    assert_eq!(status["revision"], 1);
    assert_eq!(status["phase"], "taker_lock_confirmed");
}

#[tokio::test(flavor = "current_thread")]
async fn concurrent_maker_lock_drives_converge_only_on_revision_two_winner() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Taker);
    activate_and_project_taker_lock(&fixture).await;
    let barrier = Arc::new(Barrier::new(2));
    let first_observer = BarrierObserver {
        barrier: Arc::clone(&barrier),
        observation: maker_lock_observation(&fixture.agreement, 1),
        expected_transition: FundingTransition::MakerLock,
    };
    let second_observer = BarrierObserver {
        barrier,
        observation: maker_lock_observation(&fixture.agreement, 2),
        expected_transition: FundingTransition::MakerLock,
    };
    let first = drive_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &first_observer,
    );
    let second = drive_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &second_observer,
    );

    let (first, second) = tokio::join!(first, second);
    let outputs = [
        output_json(first.expect("first concurrent maker-lock drive")),
        output_json(second.expect("second concurrent maker-lock drive")),
    ];
    let outcomes: std::collections::BTreeSet<_> = outputs
        .iter()
        .map(|output| output["outcome"].as_str().expect("typed outcome"))
        .collect();
    assert_eq!(
        outcomes,
        std::collections::BTreeSet::from([
            "converged_on_existing_projection",
            "observed_then_projected",
        ])
    );
    let converged = outputs
        .iter()
        .find(|output| output["outcome"] == "converged_on_existing_projection")
        .expect("truthful maker-lock winner convergence");
    assert_eq!(converged["durable_revision"], 2);
    assert_eq!(converged["phase"], "both_legs_locked");
}

#[tokio::test(flavor = "current_thread")]
async fn concurrent_exact_maker_lock_observation_replays_revision_two_evidence() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Maker);
    activate_and_project_taker_lock(&fixture).await;
    let barrier = Arc::new(Barrier::new(2));
    let observation = maker_lock_observation(&fixture.agreement, 1);
    let first_observer = BarrierObserver {
        barrier: Arc::clone(&barrier),
        observation: observation.clone(),
        expected_transition: FundingTransition::MakerLock,
    };
    let second_observer = BarrierObserver {
        barrier,
        observation,
        expected_transition: FundingTransition::MakerLock,
    };
    let first = drive_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &first_observer,
    );
    let second = drive_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &second_observer,
    );

    let (first, second) = tokio::join!(first, second);
    let outputs = [
        output_json(first.expect("first exact maker-lock drive")),
        output_json(second.expect("second exact maker-lock drive")),
    ];
    assert!(outputs.iter().all(|output| {
        output["outcome"] == "observed_then_projected" && output["revision"] == 2
    }));
    let replay_values: std::collections::BTreeSet<_> = outputs
        .iter()
        .map(|output| output["was_replay"].as_bool().expect("replay flag"))
        .collect();
    assert_eq!(
        replay_values,
        std::collections::BTreeSet::from([false, true])
    );
}

#[tokio::test(flavor = "current_thread")]
async fn revision_two_gate_prevents_any_later_observer_call() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
    activate_and_project_taker_lock(&fixture).await;
    let first = FixedObserver::new(maker_lock_observation(&fixture.agreement, 1));
    drive_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &first,
    )
    .await
    .expect("project revision two");
    let forbidden = FixedObserver::new(maker_lock_observation(&fixture.agreement, 2));

    let later = output_json(
        drive_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &forbidden,
        )
        .await
        .expect("later revision remains uncomposed"),
    );
    assert_eq!(later["outcome"], "not_yet_composed");
    assert_eq!(later["durable_revision"], 2);
    assert_eq!(forbidden.calls(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn concurrent_revision_zero_drives_converge_on_a_valid_winner() {
    let fixture = ActorFixture::new();
    execute_actor_command(&fixture.config, ActorCommand::Activate)
        .await
        .expect("activate actor");
    let barrier = Arc::new(Barrier::new(2));
    let transaction_id = support::swap_fixture().funding.compute_txid().to_string();
    let observer = |tip_height: u64| BarrierObserver {
        barrier: Arc::clone(&barrier),
        observation: ActorFundingObservation::Ready {
            chain: Chain::Bitcoin,
            transaction_id: transaction_id.clone().into_boxed_str(),
            confirmations: support::REQUIRED_CONFIRMATIONS,
            chain_evidence: serde_json::to_vec(&serde_json::json!({
                "immutable_funding": "same",
                "finalized_tip": { "height": tip_height }
            }))
            .expect("moving-tip evidence"),
        },
        expected_transition: FundingTransition::TakerLock,
    };
    let first_observer = observer(100);
    let second_observer = observer(101);
    let first = drive_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &first_observer,
    );
    let second = drive_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &second_observer,
    );

    let (first, second) = tokio::join!(first, second);
    let outputs = [
        output_json(first.expect("first concurrent drive")),
        output_json(second.expect("second concurrent drive")),
    ];
    let outcomes: std::collections::BTreeSet<_> = outputs
        .iter()
        .map(|output| output["outcome"].as_str().expect("typed outcome"))
        .collect();
    assert_eq!(
        outcomes,
        std::collections::BTreeSet::from([
            "converged_on_existing_projection",
            "observed_then_projected",
        ])
    );
    let converged = outputs
        .iter()
        .find(|output| output["outcome"] == "converged_on_existing_projection")
        .expect("truthful non-identical winner outcome");
    assert_eq!(converged["durable_revision"], 1);
    assert!(converged.get("was_replay").is_none());
    let status = output_json(
        execute_actor_command(&fixture.config, ActorCommand::Status)
            .await
            .expect("reconstruct concurrent winner"),
    );
    assert_eq!(status["revision"], 1);
    assert_eq!(status["phase"], "taker_lock_confirmed");
}

#[tokio::test(flavor = "current_thread")]
async fn contradictory_pending_chain_is_rejected_before_projection() {
    let fixture = ActorFixture::new();
    execute_actor_command(&fixture.config, ActorCommand::Activate)
        .await
        .expect("activate actor");
    let observer = FixedObserver::new(ActorFundingObservation::Pending { chain: Chain::Lez });

    let error = drive_with_observer(
        &fixture.config,
        fixture.agreement,
        fixture.agreement_wire,
        &observer,
    )
    .await
    .expect_err("wrong pending chain must fail closed");
    assert_eq!(error, ActorCommandError::AgreementBindingInvalid);
    let status = output_json(
        execute_actor_command(&fixture.config, ActorCommand::Status)
            .await
            .expect("offline status after contradiction"),
    );
    assert_eq!(status["revision"], 0);
}

struct FixedClaimObserver {
    observation: ActorClaimObservation,
    calls: AtomicUsize,
}

impl FixedClaimObserver {
    fn new(observation: ActorClaimObservation) -> Self {
        Self {
            observation,
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ClaimObservationPort for FixedClaimObserver {
    async fn observe(
        &self,
        _agreement: &BtcAgreementV1,
        _transition: ClaimTransition,
    ) -> Result<ActorClaimObservation, ActorCommandError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.observation.clone())
    }
}

async fn activate_and_project_both_locks(fixture: &ActorFixture) {
    activate_and_project_taker_lock(fixture).await;
    let observer = FixedObserver::new(maker_lock_observation(&fixture.agreement, 1));
    drive_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &observer,
    )
    .await
    .expect("project maker lock");
}

fn revealing_signature(fixture: &ActorFixture) -> [u8; 64] {
    let chain = fixture
        .agreement
        .coordinator()
        .funded_chain(Participant::Maker);
    let (domain, signing) = match chain {
        Chain::Bitcoin => (
            BtcAdaptorSessionDomain::Bitcoin,
            &fixture.config.signing.bitcoin,
        ),
        Chain::Lez => (BtcAdaptorSessionDomain::Lez, &fixture.config.signing.lez),
        Chain::Zcash | Chain::Monero => unreachable!("Bitcoin agreement chain set"),
    };
    let context = fixture
        .agreement
        .adaptor_session_context(domain, *signing.session_id.as_bytes())
        .expect("agreement claim context");
    let journal = SqliteAdaptorSessionJournal::open_existing(&signing.journal_db)
        .expect("open signer journal");
    let snapshot = journal
        .load(signing.session_id.as_bytes())
        .expect("load signer snapshot")
        .expect("signer snapshot");
    let presignature = snapshot.presignature().expect("durable presignature");
    lez_btc_swap_sdk::adapt_presignature(&context, *presignature.bytes(), Zeroizing::new([7; 32]))
        .expect("adapt revealing signature")
}

#[tokio::test(flavor = "current_thread")]
async fn revealing_claim_projects_revision_three_for_both_roles_and_directions() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        for role in [ActorRole::Maker, ActorRole::Taker] {
            let fixture = ActorFixture::for_direction(direction, role);
            activate_and_project_both_locks(&fixture).await;
            let chain = fixture
                .agreement
                .coordinator()
                .funded_chain(Participant::Maker);
            let observer = FixedClaimObserver::new(ActorClaimObservation::Ready {
                chain,
                transaction_id: "canonical-revealing-claim".into(),
                confirmations: if chain == Chain::Bitcoin {
                    support::REQUIRED_CONFIRMATIONS
                } else {
                    FINALIZED_LEZ_CONFIRMATION_UNITS
                },
                chain_evidence: b"canonical-revealing-claim-evidence".to_vec(),
                revealing_public_signature: Some(revealing_signature(&fixture)),
            });

            let projected = output_json(
                drive_claim_with_observer(
                    &fixture.config,
                    fixture.agreement.clone(),
                    fixture.agreement_wire.clone(),
                    &observer,
                )
                .await
                .expect("project revealing claim"),
            );
            assert_eq!(projected["outcome"], "observed_then_projected");
            assert_eq!(projected["revision"], 3);
            assert_eq!(projected["phase"], "claim_evidence_available");
            assert_eq!(observer.calls(), 1);

            let status = output_json(
                execute_actor_command(&fixture.config, ActorCommand::Status)
                    .await
                    .expect("offline revision-three status"),
            );
            assert_eq!(status["next_action"], "observe_followup_claim");
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn revealing_claim_requires_the_exact_related_public_signature() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Maker);
    activate_and_project_both_locks(&fixture).await;
    let chain = fixture
        .agreement
        .coordinator()
        .funded_chain(Participant::Maker);
    let mut signature = revealing_signature(&fixture);
    signature[63] ^= 1;
    let observer = FixedClaimObserver::new(ActorClaimObservation::Ready {
        chain,
        transaction_id: "canonical-revealing-claim".into(),
        confirmations: support::REQUIRED_CONFIRMATIONS,
        chain_evidence: b"canonical-revealing-claim-evidence".to_vec(),
        revealing_public_signature: Some(signature),
    });

    let error = drive_claim_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &observer,
    )
    .await
    .expect_err("unrelated public signature must fail closed");
    assert_eq!(error, ActorCommandError::ObservationUnavailable);
    let status = execute_actor_command(&fixture.config, ActorCommand::Status)
        .await
        .expect("offline status remains readable");
    assert_eq!(output_json(status)["revision"], 2);
}

async fn project_revealing_claim(fixture: &ActorFixture) {
    let chain = fixture
        .agreement
        .coordinator()
        .funded_chain(Participant::Maker);
    let observer = FixedClaimObserver::new(ActorClaimObservation::Ready {
        chain,
        transaction_id: "canonical-revealing-claim".into(),
        confirmations: if chain == Chain::Bitcoin {
            support::REQUIRED_CONFIRMATIONS
        } else {
            FINALIZED_LEZ_CONFIRMATION_UNITS
        },
        chain_evidence: b"canonical-revealing-claim-evidence".to_vec(),
        revealing_public_signature: Some(revealing_signature(fixture)),
    });
    drive_claim_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &observer,
    )
    .await
    .expect("project revealing claim");
}

#[tokio::test(flavor = "current_thread")]
async fn pending_revealing_claim_preserves_both_locks_without_secret_use() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
    activate_and_project_both_locks(&fixture).await;
    let chain = fixture
        .agreement
        .coordinator()
        .funded_chain(Participant::Maker);
    let observer = FixedClaimObserver::new(ActorClaimObservation::Pending { chain });

    let pending = output_json(
        drive_claim_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &observer,
        )
        .await
        .expect("revealing claim remains pending"),
    );
    assert_eq!(pending["outcome"], "awaiting_observation");
    assert_eq!(pending["revision"], 2);
    assert_eq!(pending["phase"], "both_legs_locked");
    assert_eq!(observer.calls(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn followup_claim_completes_both_roles_and_directions() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        for role in [ActorRole::Maker, ActorRole::Taker] {
            let fixture = ActorFixture::for_direction(direction, role);
            activate_and_project_both_locks(&fixture).await;
            project_revealing_claim(&fixture).await;
            let chain = fixture
                .agreement
                .coordinator()
                .funded_chain(Participant::Taker);
            let observer = FixedClaimObserver::new(ActorClaimObservation::Ready {
                chain,
                transaction_id: "canonical-followup-claim".into(),
                confirmations: if chain == Chain::Bitcoin {
                    support::REQUIRED_CONFIRMATIONS
                } else {
                    FINALIZED_LEZ_CONFIRMATION_UNITS
                },
                chain_evidence: b"canonical-followup-claim-evidence".to_vec(),
                revealing_public_signature: None,
            });

            let completed = output_json(
                drive_claim_with_observer(
                    &fixture.config,
                    fixture.agreement.clone(),
                    fixture.agreement_wire.clone(),
                    &observer,
                )
                .await
                .expect("project followup claim"),
            );
            assert_eq!(completed["outcome"], "observed_then_projected");
            assert_eq!(completed["revision"], 4);
            assert_eq!(completed["phase"], "completed");
            assert_eq!(observer.calls(), 1);

            let status = output_json(
                execute_actor_command(&fixture.config, ActorCommand::Status)
                    .await
                    .expect("offline completed status"),
            );
            assert_eq!(status["revision"], 4);
            assert_eq!(status["phase"], "completed");
            assert_eq!(status["next_action"], "complete");
        }
    }
}

#[derive(Clone)]
struct FixedBitcoinClaimPort {
    scan: BitcoinClaimScan,
    submission: Result<AuthorizedClaimSubmission, ActorCommandError>,
    observe_calls: Arc<AtomicUsize>,
    submit_calls: Arc<AtomicUsize>,
}

#[derive(Clone)]
struct FixedLezClaimPort {
    completed: PreparedTransaction,
    presence: lez_bridge_client::FinalizedWitnessedClaimPresence,
    submission: Result<SubmissionOutcome, ActorCommandError>,
    complete_calls: Arc<AtomicUsize>,
    classify_calls: Arc<AtomicUsize>,
    submit_calls: Arc<AtomicUsize>,
    complete_requests: Arc<Mutex<Vec<CompleteWitnessedClaimRequest>>>,
    classify_requests: Arc<Mutex<Vec<ObserveFinalizedWitnessedClaimRequest>>>,
    submit_requests: Arc<Mutex<Vec<SubmitTransactionRequest>>>,
}

#[async_trait]
impl LezClaimChainPort for FixedLezClaimPort {
    async fn complete_witnessed_claim(
        &self,
        request: CompleteWitnessedClaimRequest,
    ) -> Result<CompleteWitnessedClaimResult, ActorCommandError> {
        self.complete_calls.fetch_add(1, Ordering::SeqCst);
        self.complete_requests
            .lock()
            .expect("complete request lock")
            .push(request.clone());
        Ok(CompleteWitnessedClaimResult::new(
            request.context,
            self.completed.clone(),
        ))
    }

    async fn classify_finalized_witnessed_claim(
        &self,
        request: ObserveFinalizedWitnessedClaimRequest,
    ) -> Result<lez_bridge_client::FinalizedWitnessedClaimPresence, ActorCommandError> {
        self.classify_calls.fetch_add(1, Ordering::SeqCst);
        self.classify_requests
            .lock()
            .expect("classify request lock")
            .push(request);
        Ok(self.presence.clone())
    }

    async fn submit_transaction(
        &self,
        request: SubmitTransactionRequest,
    ) -> Result<SubmitTransactionResult, ActorCommandError> {
        self.submit_calls.fetch_add(1, Ordering::SeqCst);
        self.submit_requests
            .lock()
            .expect("submit request lock")
            .push(request.clone());
        self.submission.map(|outcome| {
            SubmitTransactionResult::new(
                request.context,
                request.transaction.transaction_id,
                outcome,
            )
        })
    }
}

impl FixedLezClaimPort {
    fn with_presence(
        completed: PreparedTransaction,
        presence: lez_bridge_client::FinalizedWitnessedClaimPresence,
        submission: Result<SubmissionOutcome, ActorCommandError>,
    ) -> Self {
        Self {
            completed,
            presence,
            submission,
            complete_calls: Arc::new(AtomicUsize::new(0)),
            classify_calls: Arc::new(AtomicUsize::new(0)),
            submit_calls: Arc::new(AtomicUsize::new(0)),
            complete_requests: Arc::new(Mutex::new(Vec::new())),
            classify_requests: Arc::new(Mutex::new(Vec::new())),
            submit_requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn submit_calls(&self) -> usize {
        self.submit_calls.load(Ordering::SeqCst)
    }
}

fn placeholder_lez_port() -> FixedLezClaimPort {
    let transaction = PreparedTransaction::new(
        TransactionId::from_bytes([91; 32]),
        ExactTransactionBytes::new(vec![92; 128]).expect("exact LEZ transaction bytes"),
    );
    FixedLezClaimPort::with_presence(
        transaction,
        lez_bridge_client::FinalizedWitnessedClaimPresence::Uncertain(
            lez_bridge_client::FinalizedWitnessedClaimUncertain::Transport,
        ),
        Ok(SubmissionOutcome::Accepted),
    )
}

fn prepared_lez_claim(config: &ActorConfig, agreement: &BtcAgreementV1) -> PreparedWitnessedClaim {
    load_prepared_witnessed_claim(config, agreement)
        .expect("load prepared LEZ claim")
        .claim
}

fn finalized_lez_tip(window: DiscoveryWindow) -> ChainTip {
    let end = window.start_height() + u64::from(window.max_blocks() - 1);
    ChainTip::new(Hex32::from_bytes([94; 32]), end)
}

fn not_found_lez_claim_presence(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    transition: ClaimTransition,
    effect: Option<&PreparedLezClaimEffect>,
) -> lez_bridge_client::FinalizedWitnessedClaimPresence {
    let request = finalized_lez_claim_request(
        config,
        agreement,
        transition,
        &prepared_lez_claim(config, agreement),
        effect,
    )
    .expect("finalized LEZ claim request");
    lez_bridge_client::FinalizedWitnessedClaimPresence::NotFound {
        context: request.context,
        finalized_tip: finalized_lez_tip(request.window),
        scanned_window: request.window,
    }
}

fn exact_lez_claim_presence(
    config: &ActorConfig,
    agreement: &BtcAgreementV1,
    transition: ClaimTransition,
    effect: Option<&PreparedLezClaimEffect>,
    transaction: PreparedTransaction,
    aggregate_signature: [u8; 64],
) -> lez_bridge_client::FinalizedWitnessedClaimPresence {
    let prepared = prepared_lez_claim(config, agreement);
    let request = finalized_lez_claim_request(config, agreement, transition, &prepared, effect)
        .expect("finalized LEZ claim request");
    let block_height = request.window.start_height() + 1;
    let block_hash = Hex32::from_bytes([93; 32]);
    let authority = request.terms.aggregate_authority_account_id();
    let metadata_account = Hex32::from_bytes(*agreement.lez_terms().metadata_account());
    let custody_account = Hex32::from_bytes(*agreement.lez_terms().custody_account());
    let transaction_facts = ObservedTransactionFacts::new(
        transaction.transaction_id,
        transaction.exact_bytes,
        ChainPosition::new(block_hash, block_height, 0),
        AccountIds::new(vec![authority]).expect("one signer"),
        true,
    );
    let instruction = WitnessedClaimInstructionFacts::new(
        request.runtime.escrow_program_id,
        AccountIds::new(vec![
            metadata_account,
            custody_account,
            request.terms.claimant_account_id(),
            authority,
        ])
        .expect("claim account list"),
        request.terms.swap_id(),
        request.terms.claimant_account_id(),
        authority,
        prepared,
    );
    let metadata = WitnessedEscrowMetadataFacts::from_witnessed_native_terms(
        metadata_account,
        request.runtime.escrow_program_id,
        custody_account,
        &request.terms,
        EscrowState::Claimed,
    );
    let custody = NativeCustodyFacts::new(
        custody_account,
        request.terms.authenticated_transfer_program_id(),
        0,
    );
    let claim = FinalizedWitnessedClaimFacts::new(
        transaction_facts,
        instruction,
        AggregateBip340Signature::from_bytes(aggregate_signature),
        FinalizedBlockIdentity::new(block_height, block_hash, 1_700_000_001_000),
        metadata,
        custody,
    );
    lez_bridge_client::FinalizedWitnessedClaimPresence::PresentExact {
        context: request.context,
        finalized_tip: finalized_lez_tip(request.window),
        scanned_window: request.window,
        claim: Box::new(claim),
    }
}

fn valid_lez_claim_signature(fixture: &ActorFixture, transition: ClaimTransition) -> [u8; 64] {
    if transition == ClaimTransition::RevealingClaim {
        return revealing_signature(fixture);
    }
    let revealing_public_signature = revealing_signature(fixture);
    let (bitcoin_context, bitcoin_presignature) =
        verified_chain_presignature(&fixture.config, &fixture.agreement, Chain::Bitcoin)
            .expect("verified Bitcoin presignature");
    let secret = extract_adaptor_secret(
        &bitcoin_context,
        bitcoin_presignature,
        revealing_public_signature,
    )
    .expect("extract followup adaptor secret");
    let (lez_context, lez_presignature) =
        verified_chain_presignature(&fixture.config, &fixture.agreement, Chain::Lez)
            .expect("verified LEZ presignature");
    adapt_presignature(&lez_context, lez_presignature, secret)
        .expect("adapt LEZ followup signature")
}

#[tokio::test(flavor = "current_thread")]
async fn lez_claim_effect_is_role_and_revision_ordered() {
    let taker = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
    activate_and_project_both_locks(&taker).await;
    assert!(
        prepare_lez_claim_effect(
            &taker.config,
            &taker.agreement,
            ClaimTransition::RevealingClaim,
            &durable_status(&taker),
            &placeholder_lez_port(),
        )
        .await
        .expect("taker owns maker-funded LEZ claim")
        .is_some()
    );

    let maker = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Maker);
    activate_and_project_both_locks(&maker).await;
    assert!(
        prepare_lez_claim_effect(
            &maker.config,
            &maker.agreement,
            ClaimTransition::RevealingClaim,
            &durable_status(&maker),
            &placeholder_lez_port(),
        )
        .await
        .expect("maker only observes its LEZ claim")
        .is_none()
    );

    let maker_owner = ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Maker);
    activate_and_project_both_locks(&maker_owner).await;
    project_revealing_claim(&maker_owner).await;
    assert!(
        prepare_lez_claim_effect(
            &maker_owner.config,
            &maker_owner.agreement,
            ClaimTransition::FollowupClaim,
            &durable_status(&maker_owner),
            &placeholder_lez_port(),
        )
        .await
        .expect("maker owns taker-funded LEZ followup claim")
        .is_some()
    );

    let taker_observer =
        ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Taker);
    activate_and_project_both_locks(&taker_observer).await;
    project_revealing_claim(&taker_observer).await;
    assert!(
        prepare_lez_claim_effect(
            &taker_observer.config,
            &taker_observer.agreement,
            ClaimTransition::FollowupClaim,
            &durable_status(&taker_observer),
            &placeholder_lez_port(),
        )
        .await
        .expect("taker only observes its LEZ followup claim")
        .is_none()
    );
}

#[tokio::test(flavor = "current_thread")]
#[allow(clippy::too_many_lines)] // One scenario proves completion, send, restart window, and projection.
async fn lez_claim_owner_submits_once_then_projects_exact_finalized_presence() {
    for (direction, role, transition) in [
        (
            SwapDirection::TakerSellsForeign,
            ActorRole::Taker,
            ClaimTransition::RevealingClaim,
        ),
        (
            SwapDirection::TakerSellsLez,
            ActorRole::Maker,
            ClaimTransition::FollowupClaim,
        ),
    ] {
        let fixture = ActorFixture::for_direction(direction, role);
        activate_and_project_both_locks(&fixture).await;
        if transition == ClaimTransition::FollowupClaim {
            project_revealing_claim(&fixture).await;
        }
        let preparation_port = placeholder_lez_port();
        let effect = prepare_lez_claim_effect(
            &fixture.config,
            &fixture.agreement,
            transition,
            &durable_status(&fixture),
            &preparation_port,
        )
        .await
        .expect("prepare owned LEZ claim")
        .expect("role owns LEZ claim");
        let repeated = prepare_lez_claim_effect(
            &fixture.config,
            &fixture.agreement,
            transition,
            &durable_status(&fixture),
            &preparation_port,
        )
        .await
        .expect("repeat deterministic completion")
        .expect("role still owns LEZ claim");
        assert_eq!(effect, repeated);
        {
            let completion_requests = preparation_port
                .complete_requests
                .lock()
                .expect("completion request lock");
            assert_eq!(completion_requests.len(), 2);
            assert_eq!(completion_requests[0], completion_requests[1]);
        }

        let absence_port = FixedLezClaimPort::with_presence(
            effect.transaction.clone(),
            not_found_lez_claim_presence(
                &fixture.config,
                &fixture.agreement,
                transition,
                Some(&effect),
            ),
            Ok(SubmissionOutcome::Accepted),
        );
        let absence_observer = LezClaimObserver {
            config: &fixture.config,
            chain: absence_port.clone(),
            effect: Some(effect.clone()),
            prepared_claim: prepared_lez_claim(&fixture.config, &fixture.agreement),
            state_db: fixture.config.state_db.clone(),
        };
        let awaiting = output_json(
            drive_claim_with_observer(
                &fixture.config,
                fixture.agreement.clone(),
                fixture.agreement_wire.clone(),
                &absence_observer,
            )
            .await
            .expect("one exact LEZ submission"),
        );
        assert_eq!(awaiting["outcome"], "awaiting_observation");
        assert_eq!(awaiting["revision"], transition.predecessor_revision());
        assert_eq!(absence_port.submit_calls(), 1);
        let first_request_id = {
            let absence_requests = absence_port
                .classify_requests
                .lock()
                .expect("absence request lock");
            assert!(matches!(
                absence_requests[0].target,
                FinalizedWitnessedClaimObservationTarget::Exact {
                    claim_transaction_id
                } if claim_transaction_id == effect.transaction.transaction_id
            ));
            absence_requests[0].context.request_id.clone()
        };

        let mut later_config = fixture.config.clone();
        later_config.lez_bridge.discovery_start_height = 21;
        later_config
            .validate()
            .expect("later valid discovery window");
        let exact_port = FixedLezClaimPort::with_presence(
            effect.transaction.clone(),
            exact_lez_claim_presence(
                &later_config,
                &fixture.agreement,
                transition,
                Some(&effect),
                effect.transaction.clone(),
                effect.aggregate_signature,
            ),
            Err(ActorCommandError::ObservationUnavailable),
        );
        let exact_observer = LezClaimObserver {
            config: &later_config,
            chain: exact_port.clone(),
            effect: Some(effect),
            prepared_claim: prepared_lez_claim(&later_config, &fixture.agreement),
            state_db: later_config.state_db.clone(),
        };
        let projected = output_json(
            drive_claim_with_observer(
                &later_config,
                fixture.agreement.clone(),
                fixture.agreement_wire.clone(),
                &exact_observer,
            )
            .await
            .expect("project finalized exact LEZ claim"),
        );
        assert_eq!(projected["outcome"], "observed_then_projected");
        assert_eq!(projected["revision"], transition.revision());
        assert_eq!(exact_port.submit_calls(), 0);
        let exact_requests = exact_port
            .classify_requests
            .lock()
            .expect("exact request lock");
        assert_ne!(exact_requests[0].context.request_id, first_request_id);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn lez_claim_non_submitter_discovers_peerlessly_without_completion_or_send() {
    for (direction, role, transition) in [
        (
            SwapDirection::TakerSellsForeign,
            ActorRole::Maker,
            ClaimTransition::RevealingClaim,
        ),
        (
            SwapDirection::TakerSellsLez,
            ActorRole::Taker,
            ClaimTransition::FollowupClaim,
        ),
    ] {
        let fixture = ActorFixture::for_direction(direction, role);
        activate_and_project_both_locks(&fixture).await;
        if transition == ClaimTransition::FollowupClaim {
            project_revealing_claim(&fixture).await;
        }
        let transaction = PreparedTransaction::new(
            TransactionId::from_bytes([101; 32]),
            ExactTransactionBytes::new(vec![102; 128]).expect("observer transaction bytes"),
        );
        let port = FixedLezClaimPort::with_presence(
            transaction.clone(),
            exact_lez_claim_presence(
                &fixture.config,
                &fixture.agreement,
                transition,
                None,
                transaction,
                valid_lez_claim_signature(&fixture, transition),
            ),
            Err(ActorCommandError::ObservationUnavailable),
        );
        let observer = LezClaimObserver {
            config: &fixture.config,
            chain: port.clone(),
            effect: None,
            prepared_claim: prepared_lez_claim(&fixture.config, &fixture.agreement),
            state_db: fixture.config.state_db.clone(),
        };
        let projected = output_json(
            drive_claim_with_observer(
                &fixture.config,
                fixture.agreement.clone(),
                fixture.agreement_wire.clone(),
                &observer,
            )
            .await
            .expect("peerless finalized LEZ discovery"),
        );
        assert_eq!(projected["outcome"], "observed_then_projected");
        assert_eq!(projected["revision"], transition.revision());
        assert_eq!(port.complete_calls.load(Ordering::SeqCst), 0);
        assert_eq!(port.submit_calls(), 0);
        let requests = port
            .classify_requests
            .lock()
            .expect("peerless classify request lock");
        assert!(matches!(
            requests[0].target,
            FinalizedWitnessedClaimObservationTarget::DiscoverByTerms
        ));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn lez_claim_unavailable_and_uncertain_are_retryable_observe_only() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
    activate_and_project_both_locks(&fixture).await;
    let transition = ClaimTransition::RevealingClaim;
    let effect = prepare_lez_claim_effect(
        &fixture.config,
        &fixture.agreement,
        transition,
        &durable_status(&fixture),
        &placeholder_lez_port(),
    )
    .await
    .expect("prepare LEZ claim")
    .expect("taker owns LEZ claim");
    for presence in [
        lez_bridge_client::FinalizedWitnessedClaimPresence::Unavailable(
            lez_bridge_client::FinalizedWitnessedClaimUnavailable::NodeFinalityOrHistory,
        ),
        lez_bridge_client::FinalizedWitnessedClaimPresence::Unavailable(
            lez_bridge_client::FinalizedWitnessedClaimUnavailable::MovingTip,
        ),
        lez_bridge_client::FinalizedWitnessedClaimPresence::Uncertain(
            lez_bridge_client::FinalizedWitnessedClaimUncertain::Timeout,
        ),
        lez_bridge_client::FinalizedWitnessedClaimPresence::Uncertain(
            lez_bridge_client::FinalizedWitnessedClaimUncertain::Transport,
        ),
    ] {
        let port = FixedLezClaimPort::with_presence(
            effect.transaction.clone(),
            presence,
            Ok(SubmissionOutcome::Accepted),
        );
        let observer = LezClaimObserver {
            config: &fixture.config,
            chain: port.clone(),
            effect: Some(effect.clone()),
            prepared_claim: prepared_lez_claim(&fixture.config, &fixture.agreement),
            state_db: fixture.config.state_db.clone(),
        };
        let awaiting = output_json(
            drive_claim_with_observer(
                &fixture.config,
                fixture.agreement.clone(),
                fixture.agreement_wire.clone(),
                &observer,
            )
            .await
            .expect("uncertain observation stays pending"),
        );
        assert_eq!(awaiting["revision"], 2);
        assert_eq!(port.submit_calls(), 0);
    }

    let stable_absence_port = FixedLezClaimPort::with_presence(
        effect.transaction.clone(),
        not_found_lez_claim_presence(
            &fixture.config,
            &fixture.agreement,
            transition,
            Some(&effect),
        ),
        Ok(SubmissionOutcome::Accepted),
    );
    let stable_absence_observer = LezClaimObserver {
        config: &fixture.config,
        chain: stable_absence_port.clone(),
        effect: Some(effect),
        prepared_claim: prepared_lez_claim(&fixture.config, &fixture.agreement),
        state_db: fixture.config.state_db.clone(),
    };
    drive_claim_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &stable_absence_observer,
    )
    .await
    .expect("later stable absence authorizes one send");
    assert_eq!(stable_absence_port.submit_calls(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn lez_claim_post_authority_error_is_unknown_and_never_rearms() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
    activate_and_project_both_locks(&fixture).await;
    let transition = ClaimTransition::RevealingClaim;
    let effect = prepare_lez_claim_effect(
        &fixture.config,
        &fixture.agreement,
        transition,
        &durable_status(&fixture),
        &placeholder_lez_port(),
    )
    .await
    .expect("prepare LEZ claim")
    .expect("taker owns LEZ claim");
    let failing_port = FixedLezClaimPort::with_presence(
        effect.transaction.clone(),
        not_found_lez_claim_presence(
            &fixture.config,
            &fixture.agreement,
            transition,
            Some(&effect),
        ),
        Err(ActorCommandError::ObservationUnavailable),
    );
    let failing_observer = LezClaimObserver {
        config: &fixture.config,
        chain: failing_port.clone(),
        effect: Some(effect.clone()),
        prepared_claim: prepared_lez_claim(&fixture.config, &fixture.agreement),
        state_db: fixture.config.state_db.clone(),
    };
    assert_eq!(
        drive_claim_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &failing_observer,
        )
        .await
        .expect_err("post-authority failure is surfaced after Unknown"),
        ActorCommandError::ObservationUnavailable
    );
    assert_eq!(failing_port.submit_calls(), 1);

    let restarted_port = FixedLezClaimPort::with_presence(
        effect.transaction.clone(),
        not_found_lez_claim_presence(
            &fixture.config,
            &fixture.agreement,
            transition,
            Some(&effect),
        ),
        Ok(SubmissionOutcome::Accepted),
    );
    let restarted_observer = LezClaimObserver {
        config: &fixture.config,
        chain: restarted_port.clone(),
        effect: Some(effect),
        prepared_claim: prepared_lez_claim(&fixture.config, &fixture.agreement),
        state_db: fixture.config.state_db.clone(),
    };
    drive_claim_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &restarted_observer,
    )
    .await
    .expect("Unknown restart remains observe-only");
    assert_eq!(restarted_port.submit_calls(), 0);
}

#[tokio::test(flavor = "current_thread")]
#[allow(clippy::too_many_lines)] // Crash and activation gates share one exact owned-effect setup.
async fn lez_claim_started_restart_and_missing_activation_never_send() {
    let started_fixture =
        ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
    activate_and_project_both_locks(&started_fixture).await;
    let transition = ClaimTransition::RevealingClaim;
    let started_effect = prepare_lez_claim_effect(
        &started_fixture.config,
        &started_fixture.agreement,
        transition,
        &durable_status(&started_fixture),
        &placeholder_lez_port(),
    )
    .await
    .expect("prepare LEZ claim")
    .expect("taker owns LEZ claim");
    let mut journal = SqlitePublicEffectJournal::open(&started_fixture.config.state_db)
        .expect("open effect journal");
    let _ = journal
        .record_prepared(&started_effect.effect)
        .expect("persist exact public bytes");
    assert!(matches!(
        journal
            .reconcile(started_effect.effect.key(), PublicEffectObservation::Absent,)
            .expect("consume send authority before simulated crash"),
        PublicEffectDecision::SubmitOnce(_)
    ));
    drop(journal);
    let restarted_port = FixedLezClaimPort::with_presence(
        started_effect.transaction.clone(),
        not_found_lez_claim_presence(
            &started_fixture.config,
            &started_fixture.agreement,
            transition,
            Some(&started_effect),
        ),
        Ok(SubmissionOutcome::Accepted),
    );
    let restarted_observer = LezClaimObserver {
        config: &started_fixture.config,
        chain: restarted_port.clone(),
        effect: Some(started_effect),
        prepared_claim: prepared_lez_claim(&started_fixture.config, &started_fixture.agreement),
        state_db: started_fixture.config.state_db.clone(),
    };
    drive_claim_with_observer(
        &started_fixture.config,
        started_fixture.agreement.clone(),
        started_fixture.agreement_wire.clone(),
        &restarted_observer,
    )
    .await
    .expect("Started restart remains observe-only");
    assert_eq!(restarted_port.submit_calls(), 0);

    let gated_fixture =
        ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
    activate_and_project_both_locks(&gated_fixture).await;
    let gated_effect = prepare_lez_claim_effect(
        &gated_fixture.config,
        &gated_fixture.agreement,
        transition,
        &durable_status(&gated_fixture),
        &placeholder_lez_port(),
    )
    .await
    .expect("prepare gated LEZ claim")
    .expect("taker owns gated LEZ claim");
    let gated_port = FixedLezClaimPort::with_presence(
        gated_effect.transaction.clone(),
        not_found_lez_claim_presence(
            &gated_fixture.config,
            &gated_fixture.agreement,
            transition,
            Some(&gated_effect),
        ),
        Ok(SubmissionOutcome::Accepted),
    );
    let gated_observer = LezClaimObserver {
        config: &gated_fixture.config,
        chain: gated_port.clone(),
        effect: Some(gated_effect),
        prepared_claim: prepared_lez_claim(&gated_fixture.config, &gated_fixture.agreement),
        state_db: gated_fixture.config.state_db.clone(),
    };
    fs::remove_file(
        &gated_fixture
            .config
            .signing
            .prepared_witnessed_claim_result_file,
    )
    .expect("remove activation material");
    assert_eq!(
        drive_claim_with_observer(
            &gated_fixture.config,
            gated_fixture.agreement.clone(),
            gated_fixture.agreement_wire.clone(),
            &gated_observer,
        )
        .await
        .expect_err("activation gate reruns before chain observation"),
        ActorCommandError::ActivationMaterialUnavailable
    );
    assert_eq!(gated_port.classify_calls.load(Ordering::SeqCst), 0);
    assert_eq!(gated_port.submit_calls(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn lez_claim_conflicting_exact_presence_never_sends_or_rearms() {
    for conflict_signature in [false, true] {
        let fixture =
            ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
        activate_and_project_both_locks(&fixture).await;
        let transition = ClaimTransition::RevealingClaim;
        let effect = prepare_lez_claim_effect(
            &fixture.config,
            &fixture.agreement,
            transition,
            &durable_status(&fixture),
            &placeholder_lez_port(),
        )
        .await
        .expect("prepare LEZ claim")
        .expect("taker owns LEZ claim");
        let transaction = if conflict_signature {
            effect.transaction.clone()
        } else {
            PreparedTransaction::new(
                effect.transaction.transaction_id,
                ExactTransactionBytes::new(vec![103; 128]).expect("conflicting exact bytes"),
            )
        };
        let mut signature = effect.aggregate_signature;
        if conflict_signature {
            signature[63] ^= 1;
        }
        let conflicting_port = FixedLezClaimPort::with_presence(
            effect.transaction.clone(),
            exact_lez_claim_presence(
                &fixture.config,
                &fixture.agreement,
                transition,
                Some(&effect),
                transaction,
                signature,
            ),
            Ok(SubmissionOutcome::Accepted),
        );
        let conflicting_observer = LezClaimObserver {
            config: &fixture.config,
            chain: conflicting_port.clone(),
            effect: Some(effect.clone()),
            prepared_claim: prepared_lez_claim(&fixture.config, &fixture.agreement),
            state_db: fixture.config.state_db.clone(),
        };
        assert_eq!(
            drive_claim_with_observer(
                &fixture.config,
                fixture.agreement.clone(),
                fixture.agreement_wire.clone(),
                &conflicting_observer,
            )
            .await
            .expect_err("conflicting canonical facts fail closed"),
            ActorCommandError::AgreementBindingInvalid
        );
        assert_eq!(conflicting_port.submit_calls(), 0);
        assert_eq!(durable_status(&fixture).revision(), 2);

        let restarted_port = FixedLezClaimPort::with_presence(
            effect.transaction.clone(),
            not_found_lez_claim_presence(
                &fixture.config,
                &fixture.agreement,
                transition,
                Some(&effect),
            ),
            Ok(SubmissionOutcome::Accepted),
        );
        let restarted_observer = LezClaimObserver {
            config: &fixture.config,
            chain: restarted_port.clone(),
            effect: Some(effect),
            prepared_claim: prepared_lez_claim(&fixture.config, &fixture.agreement),
            state_db: fixture.config.state_db.clone(),
        };
        drive_claim_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &restarted_observer,
        )
        .await
        .expect("conflicting-presence restart remains observe-only");
        assert_eq!(restarted_port.submit_calls(), 0);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn lez_claim_rejects_containing_block_outside_scanned_finalized_window() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Maker);
    activate_and_project_both_locks(&fixture).await;
    let transition = ClaimTransition::RevealingClaim;
    let transaction = PreparedTransaction::new(
        TransactionId::from_bytes([104; 32]),
        ExactTransactionBytes::new(vec![105; 128]).expect("observer transaction bytes"),
    );
    let mut presence = exact_lez_claim_presence(
        &fixture.config,
        &fixture.agreement,
        transition,
        None,
        transaction.clone(),
        valid_lez_claim_signature(&fixture, transition),
    );
    let lez_bridge_client::FinalizedWitnessedClaimPresence::PresentExact {
        finalized_tip,
        claim,
        ..
    } = &mut presence
    else {
        unreachable!("exact presence fixture")
    };
    let outside_height = finalized_tip.height + 1;
    claim.containing_block.block_id = outside_height;
    claim.transaction.position.height = outside_height;
    let port = FixedLezClaimPort::with_presence(
        transaction,
        presence,
        Err(ActorCommandError::ObservationUnavailable),
    );
    let observer = LezClaimObserver {
        config: &fixture.config,
        chain: port.clone(),
        effect: None,
        prepared_claim: prepared_lez_claim(&fixture.config, &fixture.agreement),
        state_db: fixture.config.state_db.clone(),
    };
    assert_eq!(
        drive_claim_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &observer,
        )
        .await
        .expect_err("claim outside scanned finalized window fails closed"),
        ActorCommandError::AgreementBindingInvalid
    );
    assert_eq!(durable_status(&fixture).revision(), 2);
    assert_eq!(port.submit_calls(), 0);
}

impl FixedBitcoinClaimPort {
    fn new(
        scan: BitcoinClaimScan,
        submission: Result<AuthorizedClaimSubmission, ActorCommandError>,
    ) -> Self {
        Self {
            scan,
            submission,
            observe_calls: Arc::new(AtomicUsize::new(0)),
            submit_calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn submit_calls(&self) -> usize {
        self.submit_calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl BitcoinClaimChainPort for FixedBitcoinClaimPort {
    async fn observe_claim(&self, _agreement: &BtcAgreementV1) -> BitcoinClaimScan {
        self.observe_calls.fetch_add(1, Ordering::SeqCst);
        self.scan.clone()
    }

    async fn submit_authorized_claim(
        &self,
        _agreement: &BtcAgreementV1,
        _transaction_bytes: &[u8],
        _expected_transaction_id: bitcoin::Txid,
    ) -> Result<AuthorizedClaimSubmission, ActorCommandError> {
        self.submit_calls.fetch_add(1, Ordering::SeqCst);
        self.submission.clone()
    }
}

fn durable_status(fixture: &ActorFixture) -> BtcOfflineStatus {
    open_existing_store(
        &fixture.config,
        &fixture.agreement,
        fixture.agreement_wire.clone(),
    )
    .expect("open actor state")
    .status()
    .expect("replay actor state")
}

fn exact_finalized_scan(
    effect: &PreparedBitcoinClaimEffect,
    public_signature: [u8; 64],
) -> BitcoinClaimScan {
    BitcoinClaimScan::Exact(BitcoinExactClaim {
        transaction_bytes: effect.effect.exact_public_bytes().to_vec(),
        transaction_id: effect.expected_transaction_id.to_string().into_boxed_str(),
        confirmations: support::REQUIRED_CONFIRMATIONS,
        chain_evidence: b"exact-finalized-bitcoin-claim".to_vec(),
        public_signature,
        finalized: true,
    })
}

#[tokio::test(flavor = "current_thread")]
#[allow(clippy::similar_names)] // Parallel role names make the authority matrix explicit.
async fn bitcoin_claim_effect_is_role_and_revision_ordered() {
    let revealing_taker =
        ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Taker);
    activate_and_project_both_locks(&revealing_taker).await;
    assert!(
        prepare_bitcoin_claim_effect(
            &revealing_taker.config,
            &revealing_taker.agreement,
            ClaimTransition::RevealingClaim,
            &durable_status(&revealing_taker),
        )
        .expect("prepare taker revealing claim")
        .is_some()
    );

    let revealing_maker =
        ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Maker);
    activate_and_project_both_locks(&revealing_maker).await;
    assert!(
        prepare_bitcoin_claim_effect(
            &revealing_maker.config,
            &revealing_maker.agreement,
            ClaimTransition::RevealingClaim,
            &durable_status(&revealing_maker),
        )
        .expect("maker only observes revealing claim")
        .is_none()
    );

    let followup_maker =
        ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Maker);
    activate_and_project_both_locks(&followup_maker).await;
    project_revealing_claim(&followup_maker).await;
    assert!(
        prepare_bitcoin_claim_effect(
            &followup_maker.config,
            &followup_maker.agreement,
            ClaimTransition::FollowupClaim,
            &durable_status(&followup_maker),
        )
        .expect("prepare maker followup claim")
        .is_some()
    );

    let followup_taker =
        ActorFixture::for_direction(SwapDirection::TakerSellsForeign, ActorRole::Taker);
    activate_and_project_both_locks(&followup_taker).await;
    project_revealing_claim(&followup_taker).await;
    assert!(
        prepare_bitcoin_claim_effect(
            &followup_taker.config,
            &followup_taker.agreement,
            ClaimTransition::FollowupClaim,
            &durable_status(&followup_taker),
        )
        .expect("taker only observes followup claim")
        .is_none()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn bitcoin_claim_effect_persists_before_one_send_then_projects_only_when_finalized() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Taker);
    activate_and_project_both_locks(&fixture).await;
    let transition = ClaimTransition::RevealingClaim;
    let effect = prepare_bitcoin_claim_effect(
        &fixture.config,
        &fixture.agreement,
        transition,
        &durable_status(&fixture),
    )
    .expect("prepare revealing effect")
    .expect("taker owns revealing effect");
    let first_port = FixedBitcoinClaimPort::new(
        BitcoinClaimScan::Unspent,
        Ok(AuthorizedClaimSubmission::Accepted {
            transaction_id: effect.expected_transaction_id,
        }),
    );
    let first_observer = BitcoinClaimObserver {
        chain: first_port.clone(),
        effect: Some(effect.clone()),
        state_db: fixture.config.state_db.clone(),
    };

    let first = output_json(
        drive_claim_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &first_observer,
        )
        .await
        .expect("submit exact revealing claim"),
    );
    assert_eq!(first["outcome"], "awaiting_observation");
    assert_eq!(first["revision"], 2);
    assert_eq!(first_port.submit_calls(), 1);

    let finalized_port = FixedBitcoinClaimPort::new(
        exact_finalized_scan(&effect, revealing_signature(&fixture)),
        Err(ActorCommandError::ObservationUnavailable),
    );
    let finalized_observer = BitcoinClaimObserver {
        chain: finalized_port.clone(),
        effect: Some(effect),
        state_db: fixture.config.state_db.clone(),
    };
    let finalized = output_json(
        drive_claim_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &finalized_observer,
        )
        .await
        .expect("project only finalized exact claim"),
    );
    assert_eq!(finalized["outcome"], "observed_then_projected");
    assert_eq!(finalized["revision"], 3);
    assert_eq!(finalized_port.submit_calls(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn bitcoin_claim_uncertain_scan_stays_prepared_then_stable_scan_submits_once() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Taker);
    activate_and_project_both_locks(&fixture).await;
    let effect = prepare_bitcoin_claim_effect(
        &fixture.config,
        &fixture.agreement,
        ClaimTransition::RevealingClaim,
        &durable_status(&fixture),
    )
    .expect("prepare revealing effect")
    .expect("taker owns revealing effect");

    let uncertain_port = FixedBitcoinClaimPort::new(
        BitcoinClaimScan::Uncertain,
        Ok(AuthorizedClaimSubmission::Accepted {
            transaction_id: effect.expected_transaction_id,
        }),
    );
    let uncertain_observer = BitcoinClaimObserver {
        chain: uncertain_port.clone(),
        effect: Some(effect.clone()),
        state_db: fixture.config.state_db.clone(),
    };
    let pending = output_json(
        drive_claim_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &uncertain_observer,
        )
        .await
        .expect("uncertain scan remains observation-only"),
    );
    assert_eq!(pending["outcome"], "awaiting_observation");
    assert_eq!(pending["revision"], 2);
    assert_eq!(uncertain_port.submit_calls(), 0);

    let stable_port = FixedBitcoinClaimPort::new(
        BitcoinClaimScan::Unspent,
        Ok(AuthorizedClaimSubmission::Accepted {
            transaction_id: effect.expected_transaction_id,
        }),
    );
    let stable_observer = BitcoinClaimObserver {
        chain: stable_port.clone(),
        effect: Some(effect.clone()),
        state_db: fixture.config.state_db.clone(),
    };
    drive_claim_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &stable_observer,
    )
    .await
    .expect("stable absence consumes one send authority");
    assert_eq!(stable_port.submit_calls(), 1);

    let replay_port = FixedBitcoinClaimPort::new(
        BitcoinClaimScan::Unspent,
        Ok(AuthorizedClaimSubmission::Accepted {
            transaction_id: effect.expected_transaction_id,
        }),
    );
    let replay_observer = BitcoinClaimObserver {
        chain: replay_port.clone(),
        effect: Some(effect),
        state_db: fixture.config.state_db.clone(),
    };
    drive_claim_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &replay_observer,
    )
    .await
    .expect("accepted send authority remains consumed after restart");
    assert_eq!(replay_port.submit_calls(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn bitcoin_claim_unknown_never_rearms_after_restart() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Taker);
    activate_and_project_both_locks(&fixture).await;
    let effect = prepare_bitcoin_claim_effect(
        &fixture.config,
        &fixture.agreement,
        ClaimTransition::RevealingClaim,
        &durable_status(&fixture),
    )
    .expect("prepare revealing effect")
    .expect("taker owns revealing effect");
    let failing_port = FixedBitcoinClaimPort::new(
        BitcoinClaimScan::Unspent,
        Err(ActorCommandError::ObservationUnavailable),
    );
    let failing_observer = BitcoinClaimObserver {
        chain: failing_port.clone(),
        effect: Some(effect.clone()),
        state_db: fixture.config.state_db.clone(),
    };
    assert_eq!(
        drive_claim_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &failing_observer,
        )
        .await
        .expect_err("post-authority error is surfaced after recording Unknown"),
        ActorCommandError::ObservationUnavailable
    );
    assert_eq!(failing_port.submit_calls(), 1);

    let restarted_port = FixedBitcoinClaimPort::new(
        BitcoinClaimScan::Unspent,
        Ok(AuthorizedClaimSubmission::Accepted {
            transaction_id: effect.expected_transaction_id,
        }),
    );
    let restarted_observer = BitcoinClaimObserver {
        chain: restarted_port.clone(),
        effect: Some(effect),
        state_db: fixture.config.state_db.clone(),
    };
    drive_claim_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &restarted_observer,
    )
    .await
    .expect("Unknown restart remains observe-only");
    assert_eq!(restarted_port.submit_calls(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn bitcoin_claim_started_never_rearms_after_restart() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Taker);
    activate_and_project_both_locks(&fixture).await;
    let effect = prepare_bitcoin_claim_effect(
        &fixture.config,
        &fixture.agreement,
        ClaimTransition::RevealingClaim,
        &durable_status(&fixture),
    )
    .expect("prepare revealing effect")
    .expect("taker owns revealing effect");
    let mut journal =
        SqlitePublicEffectJournal::open(&fixture.config.state_db).expect("open effect journal");
    let _ = journal
        .record_prepared(&effect.effect)
        .expect("persist exact public bytes");
    assert!(matches!(
        journal
            .reconcile(effect.effect.key(), PublicEffectObservation::Absent)
            .expect("consume send authority before simulated crash"),
        PublicEffectDecision::SubmitOnce(_)
    ));
    drop(journal);

    let restarted_port = FixedBitcoinClaimPort::new(
        BitcoinClaimScan::Unspent,
        Ok(AuthorizedClaimSubmission::Accepted {
            transaction_id: effect.expected_transaction_id,
        }),
    );
    let restarted_observer = BitcoinClaimObserver {
        chain: restarted_port.clone(),
        effect: Some(effect),
        state_db: fixture.config.state_db.clone(),
    };
    drive_claim_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &restarted_observer,
    )
    .await
    .expect("Started restart remains observe-only");
    assert_eq!(restarted_port.submit_calls(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn bitcoin_claim_wrong_bytes_and_missing_secret_never_gain_send_authority() {
    let fixture = ActorFixture::for_direction(SwapDirection::TakerSellsLez, ActorRole::Taker);
    activate_and_project_both_locks(&fixture).await;
    let effect = prepare_bitcoin_claim_effect(
        &fixture.config,
        &fixture.agreement,
        ClaimTransition::RevealingClaim,
        &durable_status(&fixture),
    )
    .expect("prepare revealing effect")
    .expect("taker owns revealing effect");
    let mut wrong = exact_finalized_scan(&effect, revealing_signature(&fixture));
    let BitcoinClaimScan::Exact(exact) = &mut wrong else {
        unreachable!("exact scan")
    };
    exact.transaction_bytes.push(0);
    let wrong_port = FixedBitcoinClaimPort::new(
        wrong,
        Ok(AuthorizedClaimSubmission::Accepted {
            transaction_id: effect.expected_transaction_id,
        }),
    );
    let wrong_observer = BitcoinClaimObserver {
        chain: wrong_port.clone(),
        effect: Some(effect.clone()),
        state_db: fixture.config.state_db.clone(),
    };
    let pending = output_json(
        drive_claim_with_observer(
            &fixture.config,
            fixture.agreement.clone(),
            fixture.agreement_wire.clone(),
            &wrong_observer,
        )
        .await
        .expect("wrong exact bytes become a durable no-send conflict"),
    );
    assert_eq!(pending["revision"], 2);
    assert_eq!(wrong_port.submit_calls(), 0);

    let restarted_port = FixedBitcoinClaimPort::new(
        BitcoinClaimScan::Unspent,
        Ok(AuthorizedClaimSubmission::Accepted {
            transaction_id: effect.expected_transaction_id,
        }),
    );
    let restarted_observer = BitcoinClaimObserver {
        chain: restarted_port.clone(),
        effect: Some(effect.clone()),
        state_db: fixture.config.state_db.clone(),
    };
    drive_claim_with_observer(
        &fixture.config,
        fixture.agreement.clone(),
        fixture.agreement_wire.clone(),
        &restarted_observer,
    )
    .await
    .expect("conflicting-presence restart remains observe-only");
    assert_eq!(restarted_port.submit_calls(), 0);

    fs::remove_file(
        fixture
            .config
            .signing
            .adaptor_secret_file
            .as_ref()
            .expect("taker secret path"),
    )
    .expect("remove owner secret");
    assert_eq!(
        prepare_bitcoin_claim_effect(
            &fixture.config,
            &fixture.agreement,
            ClaimTransition::RevealingClaim,
            &durable_status(&fixture),
        )
        .expect_err("missing secret fails before effect preparation"),
        ActorCommandError::ActivationMaterialUnavailable
    );
}
