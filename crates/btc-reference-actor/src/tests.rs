use std::{
    fs::{self, OpenOptions},
    os::unix::fs::OpenOptionsExt as _,
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
    AccountIds, ChainPosition, EscrowState, ExactTransactionBytes, FinalizedBlockIdentity,
    FinalizedWitnessedFundingFacts, NativeCustodyFacts, NativeFundInstructionFacts,
    ObservedTransactionFacts, TransactionId, WitnessedEscrowMetadataFacts,
};
use lez_btc_swap_sdk::{
    AdaptorSessionContext, BTC_AGREEMENT_SCHEMA_V1, BtcAgreementBodyV1, BtcAgreementRecordV1,
    BtcChainPolicyV1, BtcClaimTermsV1, BtcFundingTermsV1, BtcLezTermsV1, BtcP2trTermsV1,
    BtcParticipantIdentityV1, BtcParticipantsV1, BtcRecoveryPlanV1, CsvBlockDelay, P2trSwapOutput,
    RefundXOnlyKey, TwoPartyAggregateKey,
};
use lez_swap_core::SwapDirection;
use serde_json::Value;
use tempfile::TempDir;
use tokio::sync::Barrier;

#[allow(dead_code)]
#[path = "../../btc-core-adapter/tests/support.rs"]
mod support;

struct ActorFixture {
    _directory: TempDir,
    config: ActorConfig,
    agreement: BtcAgreementV1,
    agreement_wire: Vec<u8>,
}

impl ActorFixture {
    fn new() -> Self {
        Self::with_agreement(support::swap_fixture().agreement, ActorRole::Taker)
    }

    fn for_direction(direction: SwapDirection, role: ActorRole) -> Self {
        Self::with_agreement(directional_agreement(direction), role)
    }

    fn with_agreement(agreement: BtcAgreementV1, role: ActorRole) -> Self {
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
        };
        config.validate().expect("valid test config");
        Self {
            _directory: directory,
            config,
            agreement,
            agreement_wire,
        }
    }
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
            [19; 32],
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
            assert_eq!(status["next_action"], "later_revision_not_yet_composed");
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
