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
    AccountIds, ChainPosition, EscrowState, ExactMessageBytes, ExactTransactionBytes,
    FinalizedBlockIdentity, FinalizedWitnessedFundingFacts, NativeCustodyFacts,
    NativeFundInstructionFacts, ObservedTransactionFacts, PrepareWitnessedClaimResult,
    PreparedWitnessedClaim, TransactionId, WitnessedEscrowMetadataFacts,
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
