use std::{
    fs::{self, OpenOptions},
    os::unix::fs::OpenOptionsExt as _,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use super::*;
use lez_bridge_protocol::{
    AccountIds, ChainPosition, EscrowState, ExactTransactionBytes, FinalizedBlockIdentity,
    FinalizedWitnessedFundingFacts, NativeCustodyFacts, NativeFundInstructionFacts,
    ObservedTransactionFacts, TransactionId, WitnessedEscrowMetadataFacts,
};
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
        let directory = tempfile::tempdir().expect("actor tempdir");
        let swap = support::swap_fixture();
        let agreement_wire = swap.agreement.encode_wire().expect("agreement wire");
        let agreement_file = directory.path().join("agreement.json");
        fs::write(&agreement_file, &agreement_wire).expect("write agreement");
        let config = ActorConfig {
            schema_version: CONFIG_SCHEMA_VERSION,
            role: ActorRole::Taker,
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
                    BridgeParticipant::Taker,
                    RuntimeCompatibility::LeeV0_2_0,
                    Hex32::from_bytes([99; 32]),
                    Hex32::from_bytes([17; 32]),
                    Hex32::from_bytes([18; 32]),
                    Hex32::from_bytes([15; 32]),
                    Hex32::from_bytes([11; 32]),
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
            agreement: swap.agreement,
            agreement_wire,
        }
    }
}

struct FixedObserver {
    observation: FirstLockObservation,
    calls: AtomicUsize,
}

impl FixedObserver {
    fn new(observation: FirstLockObservation) -> Self {
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
impl FirstLockObservationPort for FixedObserver {
    async fn observe(
        &self,
        _agreement: &BtcAgreementV1,
    ) -> Result<FirstLockObservation, ActorCommandError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.observation.clone())
    }
}

struct BarrierObserver {
    barrier: Arc<Barrier>,
    observation: FirstLockObservation,
}

#[async_trait]
impl FirstLockObservationPort for BarrierObserver {
    async fn observe(
        &self,
        _agreement: &BtcAgreementV1,
    ) -> Result<FirstLockObservation, ActorCommandError> {
        self.barrier.wait().await;
        Ok(self.observation.clone())
    }
}

fn output_json(output: impl Serialize) -> Value {
    serde_json::to_value(output).expect("secret-free actor output")
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
    let observer = FixedObserver::new(FirstLockObservation::Pending {
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
    let observer = FixedObserver::new(FirstLockObservation::Pending {
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
    let observer = FixedObserver::new(FirstLockObservation::Ready {
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

    let later_revision = output_json(
        drive_with_observer(
            &fixture.config,
            fixture.agreement,
            fixture.agreement_wire,
            &observer,
        )
        .await
        .expect("later revision is explicit"),
    );
    assert_eq!(later_revision["outcome"], "not_yet_composed");
    assert_eq!(later_revision["durable_revision"], 1);
    assert_eq!(observer.calls(), 1, "revision one must not re-observe");

    let live_later_revision = output_json(
        execute_actor_command(&fixture.config, ActorCommand::Drive)
            .await
            .expect("durable revision gate precedes unavailable Core credentials"),
    );
    assert_eq!(live_later_revision["outcome"], "not_yet_composed");
    assert_eq!(live_later_revision["durable_revision"], 1);
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
        observation: FirstLockObservation::Ready {
            chain: Chain::Bitcoin,
            transaction_id: transaction_id.clone().into_boxed_str(),
            confirmations: support::REQUIRED_CONFIRMATIONS,
            chain_evidence: serde_json::to_vec(&serde_json::json!({
                "immutable_funding": "same",
                "finalized_tip": { "height": tip_height }
            }))
            .expect("moving-tip evidence"),
        },
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
    let observer = FixedObserver::new(FirstLockObservation::Pending { chain: Chain::Lez });

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
