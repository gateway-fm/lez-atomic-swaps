//! Separate-process proof for the M5 XMR Chat application handoff.
//!
//! The test intentionally stops before chain effects: it uses only temporary
//! Unix sockets, `SQLite`, and role-generated Regtest agreement material. This
//! proves reservation, activation, actor handoff, replay, and restart behavior
//! without hiding any Monero/LEZ RPC dependency behind a fake endpoint.

#[path = "support/cross_role_binary.rs"]
mod cross_role;

use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write as _,
    os::unix::fs::{
        DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _,
    },
    path::{Path, PathBuf},
    process::{Child, Command, Output},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[path = "support/xmr_chat_fixture.rs"]
mod xmr_chat_fixture;
use xmr_chat_fixture::XmrChatFixture;

use lez_bridge_protocol::{
    Hex32, Participant as BridgeParticipant, RequestId, RuntimeCompatibility, RuntimeDescriptor,
};
use lez_maker_node::{
    AuthenticatedOfferRefV1, DeliveryOfferQueryV1, LocalPriceSetRequest, PairConfigureRequest,
    RunLocalDelivery, XmrChatStageARequestV1, XmrChatStageAResponseV1, call_local_chat_rpc,
    call_local_rpc,
};
use lez_swap_core::{Pair, SwapDirection, SwapId};
use lez_swap_sdk_core::OfferDiscovery as _;
use lez_swap_store::{
    LocalPriceV1, MakerActorHeldLock, MakerActorKindV1, MakerActorScheduleState, MakerOfferId,
    MakerOfferStatus, MakerPairConfigurationV1, MakerPriceSourceKind, MakerRouteV1,
    MakerXmrNegotiationStatus, SqliteSwapStore, SqliteXmrWorkflowJournal, XmrWorkflowBranch,
    XmrWorkflowDecision, XmrWorkflowReconciliationSource, XmrWorkflowReconciliationV2,
    XmrWorkflowStep, maker_xmr_chat_swap_id,
};
use lez_xmr_swap_sdk::{XmrAgreementV1, XmrRoleV1};
use rustix::process::{Pid, Signal, kill_process};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use tempfile::tempdir;
use xmr_reference_actor::{ActorRole, load_validated_xmr_effect_execution_v3_bytes};

const OFFER_ID: &str = "m5-xmr-chat-offer-001";
const RESERVATION_ID: &str = "m5-xmr-chat-reservation-001";
const CROSSED_RESERVATION_ID: &str = "m5-xmr-chat-crossed-001";
const FOREIGN_UNITS_PICONERO: u64 = 1_000_000_000_000;
const LEZ_UNITS: u128 = 1_000;
const FINALIZED_TAG14_EVIDENCE_SHA256: [u8; 32] = [0xc3; 32];
const FINALIZED_TAG16_EVIDENCE_SHA256: [u8; 32] = [0xd6; 32];
static XMR_CHAT_PROCESS_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test]
#[allow(clippy::too_many_lines)] // The full user-visible ordering is intentionally one audit surface.
async fn real_taker_and_daemon_activate_role_generated_xmr_agreement_atomically() {
    let _process_test_guard = XMR_CHAT_PROCESS_TEST_LOCK.lock().await;
    let run = tempdir().expect("isolated XMR Chat process root");
    make_private_directory(run.path());
    let runtime = run.path().join("runtime");
    make_private_directory(&runtime);
    let delivery = run.path().join("delivery");
    let socket = runtime.join("maker.sock");
    let chat_socket = runtime.join("chat.sock");
    let ready = runtime.join("ready");
    let database = run.path().join("maker.sqlite3");
    let delivery_key = run.path().join("delivery.key");
    write_raw_key(&delivery_key, 8);
    let daemon = DaemonPaths {
        socket: &socket,
        chat_socket: &chat_socket,
        ready: &ready,
        database: &database,
        delivery: &delivery,
        delivery_key: &delivery_key,
    };
    let route = MakerRouteV1::new(Pair::Monero, SwapDirection::TakerSellsLez).unwrap();
    let offer_id = MakerOfferId::new(OFFER_ID).unwrap();
    let reservation_id = request(RESERVATION_ID);

    // Bootstrap only the Delivery projection. The XMR authority registry is
    // necessarily keyed by the authenticated Delivery commitment plus the
    // owner-chosen reservation, so it is introduced on the second start.
    let mut bootstrap = start_delivery_only_daemon(&daemon);
    wait_ready(&mut bootstrap, &daemon, false);
    configure_live_route(&socket, route).await;
    publish_offer(&socket, &offer_id);
    let delivery_maker = public_key(&key(8));
    let authenticated = discover_offer(&delivery, delivery_maker, route).await;
    stop_daemon(&mut bootstrap, &daemon, false);

    let binary_swap_id = maker_xmr_chat_swap_id(&authenticated.commitment(), &reservation_id);
    let fixture = XmrChatFixture::new(
        run.path(),
        binary_swap_id,
        FOREIGN_UNITS_PICONERO,
        LEZ_UNITS,
        Path::new(env!("CARGO_BIN_EXE_lez-maker-cli")),
    );
    let role_journals = RoleJournalSnapshot::capture(&fixture);
    let authority = XmrDaemonAuthority {
        maker_public_key: &fixture.maker_public_key_file,
        private_view_key: &fixture.maker_view_key_file,
        actor_registry: &fixture.maker_registry_file,
    };
    let mut maker = start_xmr_daemon(&daemon, &authority);
    wait_ready(&mut maker, &daemon, true);

    // A crossed reservation changes the derived public swap ID and therefore
    // fails before any offer, negotiation, coordinator, actor, or effect write.
    let crossed = XmrChatStageARequestV1 {
        schema_version: 1,
        request_id: chat_request(&request(CROSSED_RESERVATION_ID), b"stage-a"),
        offer_id: offer_id.clone(),
        expected_offer_revision: 1,
        reservation_id: request(CROSSED_RESERVATION_ID),
        foreign_units: FOREIGN_UNITS_PICONERO,
        signed_offer_envelope: authenticated.signed_envelope().to_vec(),
        stage_a_wire: fs::read(&fixture.stage_a).unwrap(),
    };
    assert!(
        call_local_chat_rpc::<_, XmrChatStageAResponseV1>(
            &chat_socket,
            "xmr_chat_stage_a_v1",
            &crossed,
        )
        .await
        .is_err()
    );
    assert_zero_application_writes(&database, &offer_id);
    role_journals.assert_unchanged(&fixture);

    // Stage A is an intentionally non-executable reservation. It advances the
    // offer to revision two but creates neither coordinator nor actor.
    let staged: XmrChatStageAResponseV1 = call_local_chat_rpc(
        &chat_socket,
        "xmr_chat_stage_a_v1",
        &XmrChatStageARequestV1 {
            schema_version: 1,
            request_id: chat_request(&reservation_id, b"stage-a"),
            offer_id: offer_id.clone(),
            expected_offer_revision: 1,
            reservation_id: reservation_id.clone(),
            foreign_units: FOREIGN_UNITS_PICONERO,
            signed_offer_envelope: authenticated.signed_envelope().to_vec(),
            stage_a_wire: fs::read(&fixture.stage_a).unwrap(),
        },
    )
    .await
    .expect("Maker durably reserves exact role-generated Stage A");
    assert_eq!(staged.offer_revision, 2);
    assert!(!staged.was_replay);
    assert_eq!(staged.swap_id.as_ref(), hex::encode(binary_swap_id));
    assert_stage_a_only(&database, &offer_id, &reservation_id, &fixture);
    role_journals.assert_unchanged(&fixture);

    // The real Taker process replays the already durable Stage A and submits
    // Stage B. The Maker commits activation, coordinator, and one Maker actor
    // in one SQLite transaction; no public chain effect is emitted.
    let accepted_at = now();
    let accepted = run_taker(
        &fixture,
        &delivery,
        &chat_socket,
        &offer_id,
        &reservation_id,
        &delivery_maker,
        accepted_at,
    );
    assert_initial_acceptance(&accepted);
    assert_activated(
        &database,
        &offer_id,
        &reservation_id,
        &authenticated,
        &fixture,
    );
    role_journals.assert_unchanged(&fixture);

    // Upgrade the exact accepted application without replacing its v1 receipt:
    // role-fixed effect authority and schema-v3 projection are new owner-private
    // artifacts, while Stage A/B and the actor journal are exact replays.
    let effect = XmrTakerEffectFixture::from_v1_receipt(run.path(), &fixture.receipt);
    let accepted_v2 = run_taker_with_effect(
        &fixture,
        &delivery,
        &chat_socket,
        &offer_id,
        &reservation_id,
        &delivery_maker,
        accepted_at,
        &effect,
    );
    assert_eq!(accepted_v2["offer_revision"], 3);
    assert_eq!(accepted_v2["replay"]["stage_a"], true);
    assert_eq!(accepted_v2["replay"]["activation"], true);
    assert_eq!(accepted_v2["actor"]["provisioning_replay"], true);
    assert_eq!(accepted_v2["actor"]["effect_provisioning_replay"], false);
    assert_eq!(accepted_v2["actor"]["receipt_replay"], false);
    let receipt_v2: Value = serde_json::from_slice(&fs::read(&effect.receipt).unwrap()).unwrap();
    assert_eq!(receipt_v2["schema_version"], 2);
    assert_eq!(receipt_v2["run_id"], effect.run_id);
    assert_eq!(
        receipt_v2["effect_authority_file"].as_str(),
        effect.authority.to_str()
    );
    assert_eq!(
        receipt_v2["effect_manifest_file"].as_str(),
        effect.manifest.to_str()
    );
    assert_eq!(
        receipt_v2["workflow_journal"].as_str(),
        effect.workflow_journal.to_str()
    );
    assert_eq!(
        receipt_v2["effect_authority_sha256"],
        hex::encode(Sha256::digest(fs::read(&effect.authority).unwrap()))
    );
    assert_eq!(
        receipt_v2["effect_manifest_sha256"],
        hex::encode(Sha256::digest(fs::read(&effect.manifest).unwrap()))
    );
    prepare_taker_claim_workflow(&effect);

    let snapshot = ArtifactSnapshot::capture(&fixture);
    RunLocalDelivery::publisher(&delivery, key(8))
        .unwrap()
        .withdraw(&offer_id)
        .unwrap();
    stop_daemon(&mut maker, &daemon, true);

    // The actual Taker must retain a receipt-only, transport-free view after
    // Delivery and Chat disappear. Monitoring validates application authority only;
    // it neither infers enduring chain progress nor rewrites accepted artifacts.
    let before_monitor = ArtifactSnapshot::capture(&fixture);
    let monitor = run_taker_monitor(&fixture.receipt);
    assert_eq!(
        monitor,
        serde_json::json!({
            "schema_version": 1,
            "pair": "monero",
            "role": "taker",
            "state": "active",
            "phase": "application_activated",
            "claim_session": "presignature_verified",
            "refund_session": "presignature_verified"
        })
    );
    before_monitor.assert_unchanged(&fixture);

    // Unsupported effects fail closed after authority validation; they never
    // silently fall through to the legacy chain-effect path.
    for action in ["claim", "refund"] {
        let rejected = run_taker_lifecycle(action, &fixture.receipt);
        assert!(!rejected.status.success());
        assert!(rejected.stdout.is_empty());
        assert_eq!(
            String::from_utf8(rejected.stderr).unwrap(),
            "XMR Taker claim and refund are not yet composed\n"
        );
    }

    // Receipt v2 remains transport-free but validates both the immutable
    // application authority and its effect authority under both owner locks.
    let effect_monitor = run_taker_monitor(&effect.receipt);
    assert_eq!(
        effect_monitor,
        serde_json::json!({
            "schema_version": 2,
            "pair": "monero",
            "role": "taker",
            "state": "active",
            "phase": "application_activated",
            "run_id": effect.run_id,
            "effect_authority": "validated"
        })
    );
    write_private_bytes(&effect.preflight_rejection_marker, b"reject\n");
    let rejected_claim_preflight = run_taker_lifecycle("claim", &effect.receipt);
    assert!(!rejected_claim_preflight.status.success());
    assert!(rejected_claim_preflight.stdout.is_empty());
    assert_eq!(
        String::from_utf8(rejected_claim_preflight.stderr).unwrap(),
        "XMR Taker claim is not yet eligible or its preflight failed\n"
    );
    assert!(fs::read(&effect.invocation_marker).unwrap().is_empty());
    fs::remove_file(&effect.preflight_rejection_marker)
        .expect("remove injected claim preflight rejection");

    let first_claim = run_taker_lifecycle("claim", &effect.receipt);
    assert!(
        first_claim.status.success(),
        "first receipt-v2 claim failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&first_claim.stdout),
        String::from_utf8_lossy(&first_claim.stderr)
    );
    assert!(first_claim.stderr.is_empty());
    let first_claim: Value = serde_json::from_slice(&first_claim.stdout).unwrap();
    assert_eq!(first_claim["state"], "invoked_unreconciled");
    assert_eq!(first_claim["step"], "authorize_lez_tag14");
    let plan_identity = first_claim["tool_plan_identity_sha256"]
        .as_str()
        .expect("claim output includes its tool-plan identity")
        .to_owned();
    assert_eq!(plan_identity.len(), 64);
    assert!(
        plan_identity
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    );
    assert_ne!(plan_identity, "0".repeat(64));
    assert_eq!(
        fs::read(&effect.invocation_marker).unwrap(),
        b"preflight\ninvoked\n"
    );
    assert!(fs::read(&effect.observer_marker).unwrap().is_empty());

    // This process fixture proves sealed invocation but deliberately has no
    // prepared release journal or chain RPC. Since exact Tag14 observation was
    // hardened to authenticate and disclose only the current protected release
    // transaction, its marker-only sender can no longer fabricate finality.
    // The actual-node corridor supplies the separate semantic completion proof.
    for _ in 0..2 {
        let replayed_claim = run_taker_lifecycle("claim", &effect.receipt);
        assert!(!replayed_claim.status.success());
        assert!(replayed_claim.stdout.is_empty());
        assert_eq!(
            String::from_utf8(replayed_claim.stderr).unwrap(),
            "XMR Taker effect observation is unavailable or unsafe\n"
        );
        assert_eq!(
            fs::read(&effect.invocation_marker).unwrap(),
            b"preflight\ninvoked\n"
        );
        assert!(fs::read(&effect.observer_marker).unwrap().is_empty());
    }

    // The claim branch is now durable, so the losing refund branch fails
    // before it can invoke its tool or change accepted effect authority.
    let rejected_refund = run_taker_lifecycle("refund", &effect.receipt);
    assert!(!rejected_refund.status.success());
    assert!(rejected_refund.stdout.is_empty());
    assert_eq!(
        String::from_utf8(rejected_refund.stderr).unwrap(),
        "XMR Taker effect route is unavailable or unsafe\n"
    );
    let effect_snapshot = EffectArtifactSnapshot::capture(&effect);

    // Stage A/B are duplicate receipt fields, never independent authority.
    // Canonical nonzero digest substitutions reach the under-lock semantic
    // comparison and fail before any effect can be armed.
    for (field, byte) in [("stage_a_sha256", "91"), ("stage_b_sha256", "92")] {
        let drifted = run.path().join(format!("drifted-v2-{field}.json"));
        write_mutated_receipt(
            &effect.receipt,
            &drifted,
            field,
            &Value::String(byte.repeat(32)),
        );
        let rejected = run_taker_lifecycle("monitor", &drifted);
        assert!(!rejected.status.success());
        assert!(rejected.stdout.is_empty());
        assert_eq!(
            String::from_utf8(rejected.stderr).unwrap(),
            "XMR Taker effect authority is unavailable or unsafe\n"
        );
    }

    let held_state = MakerActorHeldLock::acquire_for(&fixture.swap_id, &fixture.taker_journal)
        .expect("hold exact adaptor-state lock");
    let state_locked = run_taker_lifecycle("monitor", &effect.receipt);
    assert!(!state_locked.status.success());
    assert!(state_locked.stdout.is_empty());
    assert_eq!(
        String::from_utf8(state_locked.stderr).unwrap(),
        "XMR Taker actor is already running or unsafe\n"
    );
    drop(held_state);

    let held_workflow = MakerActorHeldLock::acquire_for(&fixture.swap_id, &effect.workflow_journal)
        .expect("hold exact workflow lock");
    let workflow_locked = run_taker_lifecycle("monitor", &effect.receipt);
    assert!(!workflow_locked.status.success());
    assert!(workflow_locked.stdout.is_empty());
    assert_eq!(
        String::from_utf8(workflow_locked.stderr).unwrap(),
        "XMR Taker workflow is already running or unsafe\n"
    );
    drop(held_workflow);

    // Monitoring and all failed effect attempts preserve both accepted
    // application and effect authority, including all inode identities.
    before_monitor.assert_unchanged(&fixture);
    effect_snapshot.assert_unchanged(&effect);

    // Strict/canonical receipts and the manifest digest are fail-closed. The
    // CLI's stable outer error does not disclose any authority path or bytes.
    let unknown_receipt = run.path().join("unknown-xmr-receipt.json");
    write_receipt_with_unknown_field(&fixture.receipt, &unknown_receipt);
    assert_rejected_taker_monitor(&unknown_receipt);

    let drifted_receipt = run.path().join("drifted-xmr-receipt.json");
    write_mutated_receipt(
        &fixture.receipt,
        &drifted_receipt,
        "actor_manifest_sha256",
        &Value::String("00".repeat(32)),
    );
    assert_rejected_taker_monitor(&drifted_receipt);

    // A canonical receipt that selects the genuine manifest still cannot alter
    // any authority duplicate after the full under-lock semantic validation.
    let crossed_receipt = run.path().join("crossed-xmr-receipt.json");
    write_mutated_receipt(
        &fixture.receipt,
        &crossed_receipt,
        "agreement_commitment",
        &Value::String("00".repeat(32)),
    );
    let crossed = run_taker_lifecycle("monitor", &crossed_receipt);
    assert!(!crossed.status.success());
    assert!(crossed.stdout.is_empty());
    assert_eq!(
        String::from_utf8(crossed.stderr).unwrap(),
        "receipt-bound XMR Taker actor semantics changed\n"
    );

    let uppercase_receipt = run.path().join("uppercase-xmr-receipt.json");
    let receipt_value: Value =
        serde_json::from_slice(&fs::read(&fixture.receipt).unwrap()).unwrap();
    write_mutated_receipt(
        &fixture.receipt,
        &uppercase_receipt,
        "stage_a_sha256",
        &Value::String(
            receipt_value["stage_a_sha256"]
                .as_str()
                .unwrap()
                .to_uppercase(),
        ),
    );
    assert_rejected_taker_monitor(&uppercase_receipt);

    // A receipt cannot select an arbitrary lock-file location: its state path
    // must match the canonical manifest before lock acquisition.
    let unbound_state = run.path().join("unbound-state.sqlite3");
    let unbound_receipt = run.path().join("unbound-state-xmr-receipt.json");
    write_mutated_receipt(
        &fixture.receipt,
        &unbound_receipt,
        "actor_state_database",
        &Value::String(unbound_state.to_string_lossy().into_owned()),
    );
    assert_rejected_taker_monitor(&unbound_receipt);
    assert!(!lock_path(&unbound_state).exists());

    // A live owner of the exact role-state lock excludes the monitor before
    // any semantic read/output, matching the scheduler's concurrency boundary.
    let swap_id = SwapId::new(hex::encode(binary_swap_id)).unwrap();
    let held = MakerActorHeldLock::acquire_for(&swap_id, &fixture.taker_journal).unwrap();
    let locked = run_taker_lifecycle("monitor", &fixture.receipt);
    assert!(!locked.status.success());
    assert!(locked.stdout.is_empty());
    assert_eq!(
        String::from_utf8(locked.stderr).unwrap(),
        "XMR Taker actor is already running or unsafe\n"
    );
    drop(held);
    before_monitor.assert_unchanged(&fixture);

    // Reopen the daemon against the same durable database with Delivery absent.
    // The real Taker detects its durable actor, bypasses discovery, and exact
    // replays both stages without replacing any published file or inode.
    let mut reopened = start_xmr_daemon(&daemon, &authority);
    wait_ready(&mut reopened, &daemon, true);
    let replay = run_taker(
        &fixture,
        &delivery,
        &chat_socket,
        &offer_id,
        &reservation_id,
        &delivery_maker,
        accepted_at,
    );
    assert_exact_replay(&replay);
    snapshot.assert_unchanged(&fixture);
    assert_activated(
        &database,
        &offer_id,
        &reservation_id,
        &authenticated,
        &fixture,
    );
    role_journals.assert_unchanged(&fixture);
    stop_daemon(&mut reopened, &daemon, true);
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One independent user-visible refund journey is one audit surface.
async fn receipt_v2_refund_invokes_observes_and_completes_exact_tag16_once() {
    let _process_test_guard = XMR_CHAT_PROCESS_TEST_LOCK.lock().await;
    let run = tempdir().expect("isolated XMR refund process root");
    make_private_directory(run.path());
    let runtime = run.path().join("runtime");
    make_private_directory(&runtime);
    let delivery = run.path().join("delivery");
    let socket = runtime.join("maker.sock");
    let chat_socket = runtime.join("chat.sock");
    let ready = runtime.join("ready");
    let database = run.path().join("maker.sqlite3");
    let delivery_key = run.path().join("delivery.key");
    write_raw_key(&delivery_key, 8);
    let daemon = DaemonPaths {
        socket: &socket,
        chat_socket: &chat_socket,
        ready: &ready,
        database: &database,
        delivery: &delivery,
        delivery_key: &delivery_key,
    };
    let route = MakerRouteV1::new(Pair::Monero, SwapDirection::TakerSellsLez).unwrap();
    let offer_id = MakerOfferId::new(OFFER_ID).unwrap();
    let reservation_id = request(RESERVATION_ID);

    let mut bootstrap = start_delivery_only_daemon(&daemon);
    wait_ready(&mut bootstrap, &daemon, false);
    configure_live_route(&socket, route).await;
    publish_offer(&socket, &offer_id);
    let delivery_maker = public_key(&key(8));
    let authenticated = discover_offer(&delivery, delivery_maker, route).await;
    stop_daemon(&mut bootstrap, &daemon, false);

    let binary_swap_id = maker_xmr_chat_swap_id(&authenticated.commitment(), &reservation_id);
    let fixture = XmrChatFixture::new(
        run.path(),
        binary_swap_id,
        FOREIGN_UNITS_PICONERO,
        LEZ_UNITS,
        Path::new(env!("CARGO_BIN_EXE_lez-maker-cli")),
    );
    let authority = XmrDaemonAuthority {
        maker_public_key: &fixture.maker_public_key_file,
        private_view_key: &fixture.maker_view_key_file,
        actor_registry: &fixture.maker_registry_file,
    };
    let mut maker = start_xmr_daemon(&daemon, &authority);
    wait_ready(&mut maker, &daemon, true);

    let accepted_at = now();
    let accepted = run_taker(
        &fixture,
        &delivery,
        &chat_socket,
        &offer_id,
        &reservation_id,
        &delivery_maker,
        accepted_at,
    );
    assert_eq!(accepted["schema_version"], 1);
    assert_eq!(accepted["offer_revision"], 3);
    assert_eq!(accepted["replay"]["activation"], false);
    assert_eq!(accepted["actor"]["role"], "taker");
    assert_eq!(accepted["actor"]["provisioning_replay"], false);

    let effect = XmrTakerEffectFixture::from_v1_receipt_for_step(
        run.path(),
        &fixture.receipt,
        XmrWorkflowStep::RefundLezTag16,
    );
    let accepted_v2 = run_taker_with_effect(
        &fixture,
        &delivery,
        &chat_socket,
        &offer_id,
        &reservation_id,
        &delivery_maker,
        accepted_at,
        &effect,
    );
    assert_eq!(accepted_v2["replay"]["stage_a"], true);
    assert_eq!(accepted_v2["replay"]["activation"], true);
    assert_eq!(accepted_v2["actor"]["provisioning_replay"], true);
    assert_eq!(accepted_v2["actor"]["effect_provisioning_replay"], false);
    assert_eq!(accepted_v2["actor"]["receipt_replay"], false);
    let receipt_v2: Value = serde_json::from_slice(&fs::read(&effect.receipt).unwrap()).unwrap();
    assert_eq!(receipt_v2["schema_version"], 2);
    assert_eq!(receipt_v2["run_id"], effect.run_id);
    prepare_taker_refund_workflow(&effect);

    RunLocalDelivery::publisher(&delivery, key(8))
        .unwrap()
        .withdraw(&offer_id)
        .unwrap();
    stop_daemon(&mut maker, &daemon, true);

    write_private_bytes(&effect.preflight_rejection_marker, b"reject\n");
    let rejected_preflight = run_taker_lifecycle("refund", &effect.receipt);
    assert!(!rejected_preflight.status.success());
    assert!(rejected_preflight.stdout.is_empty());
    assert_eq!(
        String::from_utf8(rejected_preflight.stderr).unwrap(),
        "XMR Taker refund is not yet eligible or its preflight failed\n"
    );
    assert!(fs::read(&effect.invocation_marker).unwrap().is_empty());
    fs::remove_file(&effect.preflight_rejection_marker)
        .expect("remove injected preflight rejection");

    let first_refund = run_taker_lifecycle("refund", &effect.receipt);
    assert!(
        first_refund.status.success(),
        "first receipt-v2 refund failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&first_refund.stdout),
        String::from_utf8_lossy(&first_refund.stderr)
    );
    assert!(first_refund.stderr.is_empty());
    let first_refund: Value = serde_json::from_slice(&first_refund.stdout).unwrap();
    assert_eq!(first_refund["action"], "refund");
    assert_eq!(first_refund["state"], "invoked_unreconciled");
    assert_eq!(first_refund["step"], "refund_lez_tag16");
    assert_eq!(first_refund["chain_effect_finalized"], false);
    let plan_identity = first_refund["tool_plan_identity_sha256"]
        .as_str()
        .expect("refund output includes its tool-plan identity")
        .to_owned();
    assert_eq!(plan_identity.len(), 64);
    assert_ne!(plan_identity, "0".repeat(64));
    assert_eq!(
        fs::read(&effect.invocation_marker).unwrap(),
        b"preflight\ninvoked\n"
    );
    assert!(fs::read(&effect.observer_marker).unwrap().is_empty());

    let observed_refund = run_taker_lifecycle("refund", &effect.receipt);
    assert!(
        observed_refund.status.success(),
        "observed receipt-v2 refund failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&observed_refund.stdout),
        String::from_utf8_lossy(&observed_refund.stderr)
    );
    assert!(observed_refund.stderr.is_empty());
    let observed_refund: Value = serde_json::from_slice(&observed_refund.stdout).unwrap();
    assert_eq!(observed_refund["state"], "complete");
    assert_eq!(observed_refund["step"], "refund_lez_tag16");
    assert_eq!(observed_refund["tool_plan_identity_sha256"], plan_identity);
    assert_eq!(observed_refund["chain_effect_finalized"], true);
    assert_eq!(
        fs::read(&effect.invocation_marker).unwrap(),
        b"preflight\ninvoked\n"
    );
    assert_eq!(fs::read(&effect.observer_marker).unwrap(), b"observed\n");
    assert_taker_refund_reconciliation(&effect, &plan_identity);

    let completed_refund = run_taker_lifecycle("refund", &effect.receipt);
    assert!(completed_refund.status.success());
    assert!(completed_refund.stderr.is_empty());
    let completed_refund: Value = serde_json::from_slice(&completed_refund.stdout).unwrap();
    assert_eq!(completed_refund["state"], "complete");
    assert_eq!(completed_refund["step"], "refund_lez_tag16");
    assert_eq!(completed_refund["tool_plan_identity_sha256"], plan_identity);
    assert_eq!(completed_refund["chain_effect_finalized"], true);
    assert_eq!(
        fs::read(&effect.invocation_marker).unwrap(),
        b"preflight\ninvoked\n"
    );
    assert_eq!(fs::read(&effect.observer_marker).unwrap(), b"observed\n");
    assert_taker_refund_reconciliation(&effect, &plan_identity);

    let completed_snapshot = EffectArtifactSnapshot::capture(&effect);
    let rejected_claim = run_taker_lifecycle("claim", &effect.receipt);
    assert!(!rejected_claim.status.success());
    assert!(rejected_claim.stdout.is_empty());
    // The terminal Refund branch is rejected while selecting the next Claim
    // workflow step, before any effect route can be prepared or invoked.
    assert_eq!(
        String::from_utf8(rejected_claim.stderr).unwrap(),
        "XMR Taker claim workflow step is unavailable or unsafe\n"
    );
    completed_snapshot.assert_unchanged(&effect);
}

async fn configure_live_route(socket: &Path, route: MakerRouteV1) {
    let disabled = MakerPairConfigurationV1::new(
        route,
        false,
        MakerPriceSourceKind::Local,
        FOREIGN_UNITS_PICONERO,
        FOREIGN_UNITS_PICONERO,
        300,
    )
    .unwrap();
    let _: Value = call_local_rpc(
        socket,
        "maker_pair_configure",
        &PairConfigureRequest {
            request_id: request("m5-xmr-route-create-001"),
            expected_revision: None,
            configuration: disabled,
        },
    )
    .await
    .unwrap();
    let _: Value = call_local_rpc(
        socket,
        "maker_local_price_set",
        &LocalPriceSetRequest {
            request_id: request("m5-xmr-price-create-001"),
            expected_revision: None,
            // Keep the canonical price ratio reduced: one LEZ unit per
            // 1_000_000_000 piconero quotes this one-XMR fixture at 1_000 LEZ.
            price: LocalPriceV1::new(route, 1, 1_000_000_000).unwrap(),
        },
    )
    .await
    .unwrap();
    let enabled = MakerPairConfigurationV1::new(
        route,
        true,
        MakerPriceSourceKind::Local,
        FOREIGN_UNITS_PICONERO,
        FOREIGN_UNITS_PICONERO,
        300,
    )
    .unwrap();
    let _: Value = call_local_rpc(
        socket,
        "maker_pair_configure",
        &PairConfigureRequest {
            request_id: request("m5-xmr-route-enable-001"),
            expected_revision: Some(1),
            configuration: enabled,
        },
    )
    .await
    .unwrap();
}

fn publish_offer(socket: &Path, offer_id: &MakerOfferId) {
    let output = Command::new(env!("CARGO_BIN_EXE_lez-maker-cli"))
        .arg("--socket")
        .arg(socket)
        .arg("publish-offer")
        .arg("--request-id")
        .arg("m5-xmr-publish-001")
        .arg("--offer-id")
        .arg(offer_id.as_str())
        .arg("--pair")
        .arg("monero")
        .arg("--direction")
        .arg("taker-sells-lez")
        .output()
        .expect("run real Maker CLI");
    assert!(
        output.status.success(),
        "XMR offer publication failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

async fn discover_offer(
    delivery: &Path,
    maker_key: PublicKey,
    route: MakerRouteV1,
) -> AuthenticatedOfferRefV1 {
    RunLocalDelivery::subscriber(delivery, maker_key)
        .unwrap()
        .discover(&DeliveryOfferQueryV1::for_route(route, now()))
        .await
        .unwrap()
        .into_iter()
        .find(|candidate| candidate.offer().id().as_str() == OFFER_ID)
        .expect("published XMR offer is discoverable")
}

fn run_taker(
    fixture: &XmrChatFixture,
    delivery: &Path,
    chat_socket: &Path,
    offer_id: &MakerOfferId,
    reservation_id: &RequestId,
    maker_key: &PublicKey,
    accepted_at: u64,
) -> Value {
    let output = Command::new(cross_role::workspace_binary("lez-taker-cli"))
        .arg("--delivery-directory")
        .arg(delivery)
        .arg("--maker-public-key")
        .arg(hex::encode(maker_key.serialize()))
        .arg("--now-unix-seconds")
        .arg(accepted_at.to_string())
        .arg("--pair")
        .arg("monero")
        .arg("--direction")
        .arg("taker-sells-lez")
        .arg("--accept-xmr-offer")
        .arg(offer_id.as_str())
        .arg("--chat-socket")
        .arg(chat_socket)
        .arg("--reservation-id")
        .arg(reservation_id.as_str())
        .arg("--foreign-units")
        .arg(FOREIGN_UNITS_PICONERO.to_string())
        .arg("--xmr-stage-a-file")
        .arg(&fixture.stage_a)
        .arg("--xmr-activation-file")
        .arg(&fixture.stage_b)
        .arg("--xmr-source-taker-root")
        .arg(&fixture.taker_private_root)
        .arg("--xmr-taker-public-packet")
        .arg(&fixture.taker_public_packet)
        .arg("--xmr-maker-public-packet")
        .arg(&fixture.maker_public_packet)
        .arg("--xmr-taker-role-journal")
        .arg(&fixture.taker_journal)
        .arg("--xmr-taker-actor-root")
        .arg(&fixture.taker_actor_root)
        .arg("--xmr-acceptance-receipt")
        .arg(&fixture.receipt)
        .output()
        .expect("run real XMR Taker process");
    assert!(
        output.status.success(),
        "real XMR Taker failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("real Taker returns bounded XMR JSON")
}

#[allow(clippy::too_many_arguments)]
fn run_taker_with_effect(
    fixture: &XmrChatFixture,
    delivery: &Path,
    chat_socket: &Path,
    offer_id: &MakerOfferId,
    reservation_id: &RequestId,
    maker_key: &PublicKey,
    accepted_at: u64,
    effect: &XmrTakerEffectFixture,
) -> Value {
    let output = Command::new(cross_role::workspace_binary("lez-taker-cli"))
        .arg("--delivery-directory")
        .arg(delivery)
        .arg("--maker-public-key")
        .arg(hex::encode(maker_key.serialize()))
        .arg("--now-unix-seconds")
        .arg(accepted_at.to_string())
        .arg("--pair")
        .arg("monero")
        .arg("--direction")
        .arg("taker-sells-lez")
        .arg("--accept-xmr-offer")
        .arg(offer_id.as_str())
        .arg("--chat-socket")
        .arg(chat_socket)
        .arg("--reservation-id")
        .arg(reservation_id.as_str())
        .arg("--foreign-units")
        .arg(FOREIGN_UNITS_PICONERO.to_string())
        .arg("--xmr-stage-a-file")
        .arg(&fixture.stage_a)
        .arg("--xmr-activation-file")
        .arg(&fixture.stage_b)
        .arg("--xmr-source-taker-root")
        .arg(&fixture.taker_private_root)
        .arg("--xmr-taker-public-packet")
        .arg(&fixture.taker_public_packet)
        .arg("--xmr-maker-public-packet")
        .arg(&fixture.maker_public_packet)
        .arg("--xmr-taker-role-journal")
        .arg(&fixture.taker_journal)
        .arg("--xmr-taker-actor-root")
        .arg(&fixture.taker_actor_root)
        .arg("--xmr-acceptance-receipt")
        .arg(&effect.receipt)
        .arg("--xmr-effect-authority-file")
        .arg(&effect.authority)
        .arg("--xmr-effect-manifest-file")
        .arg(&effect.manifest)
        .arg("--xmr-workflow-journal")
        .arg(&effect.workflow_journal)
        .arg("--xmr-run-id")
        .arg(&effect.run_id)
        .output()
        .expect("run effect-capable real XMR Taker process");
    assert!(
        output.status.success(),
        "effect-capable real XMR Taker failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout)
        .expect("effect-capable real Taker returns bounded XMR JSON")
}

fn run_taker_monitor(receipt: &Path) -> Value {
    let output = run_taker_lifecycle("monitor", receipt);
    assert!(
        output.status.success(),
        "XMR Taker monitor failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).expect("XMR Taker monitor returns one bounded JSON")
}

fn run_taker_lifecycle(action: &str, receipt: &Path) -> Output {
    Command::new(cross_role::workspace_binary("lez-taker-cli"))
        .arg(action)
        .arg("--receipt")
        .arg(receipt)
        .output()
        .expect("run receipt-only real XMR Taker lifecycle command")
}

fn prepare_taker_claim_workflow(effect: &XmrTakerEffectFixture) {
    prepare_taker_terminal_workflow(
        effect,
        XmrWorkflowBranch::Claim,
        XmrWorkflowStep::AuthorizeLezTag14,
    );
}

fn prepare_taker_refund_workflow(effect: &XmrTakerEffectFixture) {
    prepare_taker_terminal_workflow(
        effect,
        XmrWorkflowBranch::Refund,
        XmrWorkflowStep::RefundLezTag16,
    );
}

fn prepare_taker_terminal_workflow(
    effect: &XmrTakerEffectFixture,
    branch: XmrWorkflowBranch,
    terminal_step: XmrWorkflowStep,
) {
    let manifest_bytes = fs::read(&effect.manifest).unwrap();
    let authority_bytes = fs::read(&effect.authority).unwrap();
    let execution = load_validated_xmr_effect_execution_v3_bytes(
        &manifest_bytes,
        &authority_bytes,
        ActorRole::Taker,
        &effect.run_id,
    )
    .expect("load the exact provisioned Taker effect execution");
    let identity = execution.workflow_identity();
    let mut workflow = SqliteXmrWorkflowJournal::open_existing(&effect.workflow_journal)
        .expect("open the provisioned Taker workflow");
    workflow
        .validate_initialized(identity)
        .expect("workflow retains the execution identity");
    for (step, evidence_byte, plan_byte) in [
        (XmrWorkflowStep::InitializeLezTag13, 0xa1, 0xb1),
        (XmrWorkflowStep::FundLezTag13, 0xa2, 0xb2),
    ] {
        workflow.prepare_step(identity, step).unwrap();
        assert_eq!(
            workflow.authorize_once(identity, step).unwrap(),
            XmrWorkflowDecision::InvokeOnce
        );
        let reconciliation = XmrWorkflowReconciliationV2::new(
            [evidence_byte; 32],
            [plan_byte; 32],
            XmrWorkflowReconciliationSource::LezFinalizedEvent,
        )
        .unwrap();
        workflow
            .reconcile_succeeded(identity, step, &reconciliation)
            .unwrap();
    }
    workflow.select_branch(identity, branch).unwrap();
    workflow.prepare_step(identity, terminal_step).unwrap();
}

fn assert_taker_refund_reconciliation(effect: &XmrTakerEffectFixture, plan_identity: &str) {
    assert_taker_reconciliation(
        effect,
        XmrWorkflowStep::RefundLezTag16,
        FINALIZED_TAG16_EVIDENCE_SHA256,
        plan_identity,
    );
}

fn assert_taker_reconciliation(
    effect: &XmrTakerEffectFixture,
    step: XmrWorkflowStep,
    effect_evidence_sha256: [u8; 32],
    plan_identity: &str,
) {
    let manifest_bytes = fs::read(&effect.manifest).unwrap();
    let authority_bytes = fs::read(&effect.authority).unwrap();
    let execution = load_validated_xmr_effect_execution_v3_bytes(
        &manifest_bytes,
        &authority_bytes,
        ActorRole::Taker,
        &effect.run_id,
    )
    .expect("reload the exact provisioned Taker effect execution");
    let workflow = SqliteXmrWorkflowJournal::open_existing(&effect.workflow_journal)
        .expect("reopen the provisioned Taker workflow");
    let reconciliation = workflow
        .load_reconciliation(execution.workflow_identity(), step)
        .expect("load durable Taker effect reconciliation")
        .expect("Taker effect reconciliation is durable");
    assert_eq!(
        reconciliation.effect_evidence_sha256(),
        effect_evidence_sha256
    );
    assert_eq!(
        hex::encode(reconciliation.tool_plan_identity_sha256()),
        plan_identity
    );
    assert_eq!(
        reconciliation.source(),
        XmrWorkflowReconciliationSource::LezFinalizedEvent
    );
}

fn assert_rejected_taker_monitor(receipt: &Path) {
    let output = run_taker_lifecycle("monitor", receipt);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "Taker acceptance receipt is unavailable or ambiguous\n"
    );
}

fn write_mutated_receipt(source: &Path, destination: &Path, field: &str, replacement: &Value) {
    let original = fs::read_to_string(source).unwrap();
    let parsed: Value = serde_json::from_str(&original).unwrap();
    let old = parsed.get(field).expect("receipt mutation field exists");
    let key = serde_json::to_string(field).unwrap();
    let needle = format!("{key}:{}", serde_json::to_string(old).unwrap());
    assert_eq!(original.matches(&needle).count(), 1);
    let replacement = format!("{key}:{}", serde_json::to_string(&replacement).unwrap());
    let mutated = original.replacen(&needle, &replacement, 1);
    assert_ne!(mutated, original);
    write_private_bytes(destination, mutated.as_bytes());
}

fn write_receipt_with_unknown_field(source: &Path, destination: &Path) {
    let mut bytes = fs::read(source).unwrap();
    assert_eq!(bytes.pop(), Some(b'}'));
    bytes.extend_from_slice(br#","unexpected":true}"#);
    write_private_bytes(destination, &bytes);
}

fn write_private_bytes(path: &Path, bytes: &[u8]) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

fn write_private_executable(path: &Path, bytes: &[u8]) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

fn lock_path(state_database: &Path) -> PathBuf {
    let mut value = state_database.as_os_str().to_os_string();
    value.push(".maker-actor.lock");
    PathBuf::from(value)
}

fn assert_initial_acceptance(accepted: &Value) {
    assert_eq!(accepted["schema_version"], 1);
    assert_eq!(accepted["offer_revision"], 3);
    assert_eq!(accepted["replay"]["stage_a"], true);
    assert_eq!(accepted["replay"]["activation"], false);
    assert_eq!(accepted["private_material_disclosed"], false);
    assert_eq!(accepted["actor"]["role"], "taker");
    assert_eq!(accepted["actor"]["provisioning_replay"], false);
    assert_eq!(accepted["actor"]["receipt_replay"], false);
}

fn assert_exact_replay(replay: &Value) {
    assert_eq!(replay["offer_revision"], 3);
    assert_eq!(replay["replay"]["stage_a"], true);
    assert_eq!(replay["replay"]["activation"], true);
    assert_eq!(replay["private_material_disclosed"], false);
    assert_eq!(replay["actor"]["role"], "taker");
    assert_eq!(replay["actor"]["provisioning_replay"], true);
    assert_eq!(replay["actor"]["receipt_replay"], true);
}

fn assert_zero_application_writes(database: &Path, offer_id: &MakerOfferId) {
    let store = SqliteSwapStore::open(database).unwrap();
    assert!(
        store
            .load_xmr_maker_negotiation(offer_id)
            .unwrap()
            .is_none()
    );
    assert!(store.list_maker_actor_processes().unwrap().is_empty());
    let offer = store
        .list_maker_offer_history(now())
        .unwrap()
        .into_iter()
        .find(|record| record.offer().id() == offer_id)
        .unwrap();
    assert_eq!(offer.revision(), 1);
    assert_eq!(offer.status(), MakerOfferStatus::Active);
}

fn assert_stage_a_only(
    database: &Path,
    offer_id: &MakerOfferId,
    reservation_id: &RequestId,
    fixture: &XmrChatFixture,
) {
    let store = SqliteSwapStore::open(database).unwrap();
    let negotiation = store
        .load_xmr_maker_negotiation(offer_id)
        .unwrap()
        .expect("durable XMR Stage A");
    assert_eq!(
        negotiation.status(),
        MakerXmrNegotiationStatus::StageAAccepted
    );
    assert_eq!(negotiation.reservation_id(), reservation_id);
    assert_eq!(
        negotiation.stage_a_wire(),
        fs::read(&fixture.stage_a).unwrap()
    );
    assert!(negotiation.activation_wire().is_none());
    assert!(negotiation.coordinator_swap_id().is_none());
    assert!(store.list_maker_actor_processes().unwrap().is_empty());
    let offer = store
        .list_maker_offer_history(now())
        .unwrap()
        .into_iter()
        .find(|record| record.offer().id() == offer_id)
        .unwrap();
    assert_eq!(offer.revision(), 2);
    assert_eq!(offer.status(), MakerOfferStatus::Reserved);
}

fn assert_activated(
    database: &Path,
    offer_id: &MakerOfferId,
    reservation_id: &RequestId,
    authenticated: &AuthenticatedOfferRefV1,
    fixture: &XmrChatFixture,
) {
    let store = SqliteSwapStore::open(database).unwrap();
    let negotiation = store
        .load_xmr_maker_negotiation(offer_id)
        .unwrap()
        .expect("durable activated XMR negotiation");
    assert_eq!(negotiation.status(), MakerXmrNegotiationStatus::Activated);
    assert_eq!(negotiation.reservation_id(), reservation_id);
    assert_eq!(negotiation.offer_commitment(), &authenticated.commitment());
    assert_eq!(
        negotiation.stage_a_wire(),
        fs::read(&fixture.stage_a).unwrap()
    );
    assert_eq!(
        negotiation.activation_wire(),
        Some(fs::read(&fixture.stage_b).unwrap().as_slice())
    );
    assert!(negotiation.coordinator_swap_id().is_some());
    let actors = store.list_maker_actor_processes().unwrap();
    assert_eq!(actors.len(), 1);
    assert_eq!(actors[0].manifest().kind(), MakerActorKindV1::Monero);
    assert_eq!(actors[0].manifest().swap_id(), &fixture.swap_id);
    assert_eq!(actors[0].schedule_state(), MakerActorScheduleState::Queued);
    assert_eq!(
        actors[0].manifest().config_path(),
        fixture.maker_actor_config
    );
    assert_eq!(
        actors[0].manifest().state_database_path(),
        fixture.maker_actor_state
    );
    let offer = store
        .list_maker_offer_history(now())
        .unwrap()
        .into_iter()
        .find(|record| record.offer().id() == offer_id)
        .unwrap();
    assert_eq!(offer.revision(), 3);
    assert_eq!(offer.status(), MakerOfferStatus::Consumed);
}

struct RoleJournalSnapshot {
    maker: (u64, Vec<u8>),
    taker: (u64, Vec<u8>),
}

impl RoleJournalSnapshot {
    fn capture(fixture: &XmrChatFixture) -> Self {
        Self {
            maker: (
                inode(&fixture.maker_actor_state),
                fs::read(&fixture.maker_actor_state).unwrap(),
            ),
            taker: (
                inode(&fixture.taker_journal),
                fs::read(&fixture.taker_journal).unwrap(),
            ),
        }
    }

    fn assert_unchanged(&self, fixture: &XmrChatFixture) {
        assert_eq!(self.maker.0, inode(&fixture.maker_actor_state));
        assert_eq!(self.maker.1, fs::read(&fixture.maker_actor_state).unwrap());
        assert_eq!(self.taker.0, inode(&fixture.taker_journal));
        assert_eq!(self.taker.1, fs::read(&fixture.taker_journal).unwrap());
    }
}

struct ArtifactSnapshot(BTreeMap<PathBuf, (u64, Vec<u8>)>);

impl ArtifactSnapshot {
    fn capture(fixture: &XmrChatFixture) -> Self {
        let mut artifacts = BTreeMap::new();
        capture_tree(
            &fixture.taker_actor_root,
            &fixture.taker_actor_root,
            &mut artifacts,
        );
        artifacts.insert(
            PathBuf::from("receipt"),
            (inode(&fixture.receipt), fs::read(&fixture.receipt).unwrap()),
        );
        Self(artifacts)
    }

    fn assert_unchanged(&self, fixture: &XmrChatFixture) {
        let mut replay = BTreeMap::new();
        capture_tree(
            &fixture.taker_actor_root,
            &fixture.taker_actor_root,
            &mut replay,
        );
        replay.insert(
            PathBuf::from("receipt"),
            (inode(&fixture.receipt), fs::read(&fixture.receipt).unwrap()),
        );
        assert_eq!(replay, self.0);
    }
}

struct EffectArtifactSnapshot(BTreeMap<PathBuf, (u64, Vec<u8>)>);

impl EffectArtifactSnapshot {
    fn capture(effect: &XmrTakerEffectFixture) -> Self {
        let artifacts = Self::artifacts(effect);
        Self::assert_no_sqlite_sidecars(&effect.workflow_journal);
        Self(artifacts)
    }

    fn assert_unchanged(&self, effect: &XmrTakerEffectFixture) {
        assert_eq!(Self::artifacts(effect), self.0);
        Self::assert_no_sqlite_sidecars(&effect.workflow_journal);
    }

    fn artifacts(effect: &XmrTakerEffectFixture) -> BTreeMap<PathBuf, (u64, Vec<u8>)> {
        [
            ("effect-authority", effect.authority.as_path()),
            ("effect-manifest-v3", effect.manifest.as_path()),
            ("workflow-journal", effect.workflow_journal.as_path()),
            ("acceptance-receipt-v2", effect.receipt.as_path()),
            (
                "tag14-invocation-marker",
                effect.invocation_marker.as_path(),
            ),
            ("tag14-observer-marker", effect.observer_marker.as_path()),
        ]
        .into_iter()
        .map(|(label, path)| {
            let metadata = fs::symlink_metadata(path).expect("effect artifact exists");
            assert!(metadata.is_file());
            assert!(!metadata.file_type().is_symlink());
            (
                PathBuf::from(label),
                (metadata.ino(), fs::read(path).unwrap()),
            )
        })
        .collect()
    }

    fn assert_no_sqlite_sidecars(journal: &Path) {
        for suffix in ["-wal", "-shm", "-journal"] {
            let mut sidecar = journal.as_os_str().to_os_string();
            sidecar.push(suffix);
            assert!(
                !PathBuf::from(sidecar).exists(),
                "closed workflow journal retained SQLite sidecar {suffix}"
            );
        }
    }
}

fn capture_tree(root: &Path, path: &Path, output: &mut BTreeMap<PathBuf, (u64, Vec<u8>)>) {
    let mut entries = fs::read_dir(path)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    entries.sort();
    for entry in entries {
        let metadata = fs::symlink_metadata(&entry).unwrap();
        assert!(!metadata.file_type().is_symlink());
        if metadata.is_dir() {
            capture_tree(root, &entry, output);
        } else if metadata.is_file() {
            output.insert(
                entry.strip_prefix(root).unwrap().to_path_buf(),
                (metadata.ino(), fs::read(&entry).unwrap()),
            );
        } else {
            panic!("actor artifact is not a regular file");
        }
    }
}

struct DaemonPaths<'a> {
    socket: &'a Path,
    chat_socket: &'a Path,
    ready: &'a Path,
    database: &'a Path,
    delivery: &'a Path,
    delivery_key: &'a Path,
}

struct XmrDaemonAuthority<'a> {
    maker_public_key: &'a Path,
    private_view_key: &'a Path,
    actor_registry: &'a Path,
}

fn daemon_command(paths: &DaemonPaths<'_>) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lez-maker-node"));
    command
        .arg("--socket")
        .arg(paths.socket)
        .arg("--database")
        .arg(paths.database)
        .arg("--ready-file")
        .arg(paths.ready)
        .arg("--delivery-directory")
        .arg(paths.delivery)
        .arg("--delivery-signing-key-file")
        .arg(paths.delivery_key);
    command
}

fn start_delivery_only_daemon(paths: &DaemonPaths<'_>) -> Child {
    daemon_command(paths)
        .spawn()
        .expect("start isolated Delivery-only Maker daemon")
}

fn start_xmr_daemon(paths: &DaemonPaths<'_>, authority: &XmrDaemonAuthority<'_>) -> Child {
    let mut command = daemon_command(paths);
    command
        .arg("--chat-socket")
        .arg(paths.chat_socket)
        .arg("--xmr-maker-agreement-public-key-file")
        .arg(authority.maker_public_key)
        .arg("--xmr-private-view-key-file")
        .arg(authority.private_view_key)
        .arg("--xmr-actor-manifest-registry-file")
        .arg(authority.actor_registry);
    command.spawn().expect("start isolated XMR Maker daemon")
}

fn wait_ready(daemon: &mut Child, paths: &DaemonPaths<'_>, expect_chat: bool) {
    // The role-generated fixture pins a single-link copy of a large debug actor
    // binary, so startup includes secure full-file hashing before readiness.
    let deadline = Instant::now() + Duration::from_mins(2);
    loop {
        if let Ok(published) = fs::read_to_string(paths.ready) {
            assert_eq!(published.trim(), paths.socket.to_str().unwrap());
            assert_eq!(paths.chat_socket.exists(), expect_chat);
            return;
        }
        assert!(daemon.try_wait().unwrap().is_none(), "daemon exited early");
        if Instant::now() >= deadline {
            daemon.kill().expect("kill unready Maker daemon");
            daemon.wait().expect("reap unready Maker daemon");
            panic!("daemon readiness timed out");
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn stop_daemon(daemon: &mut Child, paths: &DaemonPaths<'_>, had_chat: bool) {
    kill_process(Pid::from_child(daemon), Signal::INT).expect("signal Maker daemon");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = daemon.try_wait().expect("poll Maker daemon") {
            assert!(status.success(), "Maker daemon shutdown failed: {status}");
            break;
        }
        if Instant::now() >= deadline {
            daemon.kill().expect("kill wedged Maker daemon");
            daemon.wait().expect("reap wedged Maker daemon");
            panic!("Maker daemon did not stop");
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(!paths.socket.exists());
    if had_chat {
        assert!(!paths.chat_socket.exists());
    }
    assert!(!paths.ready.exists());
}

fn make_private_directory(path: &Path) {
    if path.exists() {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    } else {
        fs::DirBuilder::new().mode(0o700).create(path).unwrap();
    }
}

fn write_raw_key(path: &Path, byte: u8) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(&[byte; 32]).unwrap();
    file.sync_all().unwrap();
}

fn request(value: &str) -> RequestId {
    RequestId::new(value).unwrap()
}

fn chat_request(reservation_id: &RequestId, label: &[u8]) -> RequestId {
    let mut digest = Sha256::new();
    digest.update(b"lez-atomic-swaps/xmr-taker-chat-request/v1\0");
    digest.update(reservation_id.as_str().as_bytes());
    digest.update([0]);
    digest.update(label);
    RequestId::new(hex::encode(digest.finalize())).unwrap()
}

fn key(byte: u8) -> SecretKey {
    SecretKey::from_slice(&[byte; 32]).unwrap()
}

fn public_key(secret: &SecretKey) -> PublicKey {
    PublicKey::from_secret_key(&Secp256k1::signing_only(), secret)
}

fn inode(path: &Path) -> u64 {
    fs::symlink_metadata(path).unwrap().ino()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

struct XmrTakerEffectFixture {
    receipt: PathBuf,
    authority: PathBuf,
    manifest: PathBuf,
    workflow_journal: PathBuf,
    invocation_marker: PathBuf,
    observer_marker: PathBuf,
    preflight_rejection_marker: PathBuf,
    run_id: String,
}

impl XmrTakerEffectFixture {
    fn from_v1_receipt(root: &Path, receipt_file: &Path) -> Self {
        Self::from_v1_receipt_for_step(root, receipt_file, XmrWorkflowStep::AuthorizeLezTag14)
    }

    fn from_v1_receipt_for_step(
        root: &Path,
        receipt_file: &Path,
        terminal_step: XmrWorkflowStep,
    ) -> Self {
        let receipt: Value = serde_json::from_slice(&fs::read(receipt_file).unwrap()).unwrap();
        assert_eq!(receipt["schema_version"], 1);
        let (fixture_name, step_name, run_id) = match terminal_step {
            XmrWorkflowStep::AuthorizeLezTag14 => (
                "xmr-taker-claim-effect",
                "authorize_lez_tag14",
                "m5-xmr-taker-claim-effect-run-1",
            ),
            XmrWorkflowStep::RefundLezTag16 => (
                "xmr-taker-refund-effect",
                "refund_lez_tag16",
                "m5-xmr-taker-refund-effect-run-1",
            ),
            _ => panic!("fixture supports only Taker LEZ terminal effects"),
        };
        let effect_root = root.join(fixture_name);
        make_private_directory(&effect_root);
        let authority_file = effect_root.join("effect-authority-v1.json");
        let manifest_file = effect_root.join("actor-effect-provision-v3.json");
        let workflow_journal = effect_root.join("workflow.sqlite3");
        let invocation_marker = effect_root.join(format!("{step_name}-invocations.log"));
        let observer_marker = effect_root.join(format!("{step_name}-observations.log"));
        let preflight_rejection_marker = effect_root.join("reject-tag16-preflight");
        write_private_bytes(&invocation_marker, b"");
        write_private_bytes(&observer_marker, b"");
        let run_id = run_id.to_owned();
        let authority = taker_effect_authority(
            &receipt,
            &effect_root,
            &workflow_journal,
            &invocation_marker,
            &observer_marker,
            &preflight_rejection_marker,
            terminal_step,
            &run_id,
        );
        let mut bytes = serde_json::to_vec(&authority).unwrap();
        bytes.push(b'\n');
        write_private_bytes(&authority_file, &bytes);
        Self {
            receipt: effect_root.join("acceptance-receipt-v2.json"),
            authority: authority_file,
            manifest: manifest_file,
            workflow_journal,
            run_id,
            invocation_marker,
            observer_marker,
            preflight_rejection_marker,
        }
    }
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "one visible fixture binds every role-fixed tool slot and test control"
)]
fn taker_effect_authority(
    receipt: &Value,
    effect_root: &Path,
    workflow_journal: &Path,
    invocation_marker: &Path,
    observer_marker: &Path,
    preflight_rejection_marker: &Path,
    terminal_step: XmrWorkflowStep,
    run_id: &str,
) -> TakerEffectAuthority {
    let agreement = XmrAgreementV1::from_wire(
        &fs::read(
            receipt["stage_a_file"]
                .as_str()
                .expect("Stage-A receipt path"),
        )
        .expect("read accepted Stage A"),
    )
    .expect("parse accepted Stage A");
    let lez = agreement.body().lez();
    let runtime = RuntimeDescriptor::new(
        BridgeParticipant::Taker,
        RuntimeCompatibility::LeeV0_2_0,
        Hex32::from_bytes([0x77; 32]),
        Hex32::from_bytes(lez.channel_id()),
        Hex32::from_bytes(lez.genesis_hash()),
        Hex32::from_bytes(effect_program_id_bytes(lez.escrow_program_id())),
        Hex32::from_bytes(
            agreement
                .body()
                .participants()
                .for_role(XmrRoleV1::Taker)
                .lez_owner_account(),
        ),
    );
    let runtime_file = effect_root.join("lez-runtime.json");
    let mut runtime_bytes = serde_json::to_vec(&runtime).expect("encode fixture runtime");
    runtime_bytes.push(b'\n');
    write_private_bytes(&runtime_file, &runtime_bytes);
    let capability_file = effect_root.join("lez.capability");
    write_private_bytes(&capability_file, b"fixture-capability\n");
    for name in [
        "daemon.username",
        "daemon.password",
        "funding.username",
        "funding.password",
        "shared.username",
        "shared.password",
        "taker.username",
        "taker.password",
        "shared-wallet-file.password",
    ] {
        write_private_bytes(
            &effect_root.join(name),
            format!("fixture-{name}\n").as_bytes(),
        );
    }
    let marker_text = invocation_marker.to_str().expect("UTF-8 marker path");
    let rejection_marker_text = preflight_rejection_marker
        .to_str()
        .expect("UTF-8 preflight rejection marker path");
    assert!(!marker_text.contains('\''));
    assert!(!rejection_marker_text.contains('\''));
    let tag14_release_worker = format!(
        "#!/bin/sh\nset -eu\n[ \"$#\" -eq 0 ]\nfor fd in 197 198 199 220 221 222 223; do test -e \"/proc/self/fd/$fd\"; done\nfor fd in 200 201 202 203 204 205 206 207 208 209 210 211 212 213 214 215 216 217 218 219; do test ! -e \"/proc/self/fd/$fd\"; done\nif grep -Fq '\"mode\":\"preflight\"' /proc/self/fd/220; then\n  test ! -e '{rejection_marker_text}'\n  printf 'preflight\\n' >> '{marker_text}'\n  exit 0\nfi\ngrep -Fq '\"mode\":\"invoke\"' /proc/self/fd/220\nprintf 'invoked\\n' >> '{marker_text}'\n"
    );
    let preflight_aware_sender_worker = format!(
        "#!/bin/sh\nset -eu\nif grep -Fq '\"mode\":\"preflight\"' /proc/self/fd/217; then\n  test ! -e '{rejection_marker_text}'\n  printf 'preflight\\n' >> '{marker_text}'\n  exit 0\nfi\nprintf 'invoked\\n' >> '{marker_text}'\n"
    );
    let inert_worker = "#!/bin/sh\nexit 0\n";
    let (step_name, effect_evidence_sha256, tag14_worker, tag16_worker) = match terminal_step {
        XmrWorkflowStep::AuthorizeLezTag14 => (
            "authorize_lez_tag14",
            FINALIZED_TAG14_EVIDENCE_SHA256,
            tag14_release_worker.as_str(),
            inert_worker,
        ),
        XmrWorkflowStep::RefundLezTag16 => (
            "refund_lez_tag16",
            FINALIZED_TAG16_EVIDENCE_SHA256,
            inert_worker,
            preflight_aware_sender_worker.as_str(),
        ),
        _ => panic!("fixture supports only Taker LEZ terminal effects"),
    };
    let observer_marker_text = observer_marker
        .to_str()
        .expect("UTF-8 observer marker path");
    assert!(!observer_marker_text.contains('\''));
    let finalized_result = serde_json::json!({
        "schema_version": 1,
        "step": step_name,
        "state": "finalized",
        "effect_evidence_sha256": hex::encode(effect_evidence_sha256),
    })
    .to_string();
    assert!(!finalized_result.contains('\''));
    let finalized_worker = format!(
        "#!/bin/sh\nset -eu\n[ \"$#\" -eq 2 ]\n[ \"$1\" = '--xmr-workflow-step' ]\n[ \"$2\" = '{step_name}' ]\nprintf 'observed\\n' >> '{observer_marker_text}'\nprintf '%s\\n' '{finalized_result}'\n"
    );
    let tag14_release = if terminal_step == XmrWorkflowStep::AuthorizeLezTag14 {
        let state_directory = effect_root.join("tag14-release-state");
        make_private_directory(&state_directory);
        let capability_file = effect_root.join("tag14-release.capability");
        let protection_key_file = effect_root.join("tag14-release.key");
        write_private_bytes(&capability_file, b"fixture-release-capability");
        write_private_bytes(
            &protection_key_file,
            b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        );
        Some(Tag14ReleaseAuthority {
            sidecar_url: "http://127.0.0.1:36978/".to_owned(),
            indexer_url: "http://127.0.0.1:36979/".to_owned(),
            node_profile: "local",
            state_directory,
            capability_file,
            protection_key_file,
            protection_key_id: "taker-release-key-1",
        })
    } else {
        None
    };
    TakerEffectAuthority {
        schema_version: if tag14_release.is_some() { 2 } else { 1 },
        pair: "monero",
        role: "taker",
        swap_id: receipt["swap_id"].as_str().unwrap().to_owned(),
        agreement_commitment: receipt["agreement_commitment"].as_str().unwrap().to_owned(),
        activation_commitment: receipt["activation_commitment"]
            .as_str()
            .unwrap()
            .to_owned(),
        run_id: run_id.to_owned(),
        workflow_journal: workflow_journal.to_owned(),
        adaptor_journal: PathBuf::from(receipt["actor_state_database"].as_str().unwrap()),
        evidence_root: effect_root.join("evidence"),
        lez: EffectLezRpc {
            sidecar_url: "http://127.0.0.1:36972/".to_owned(),
            runtime_file,
            runtime_sha256: hex::encode(Sha256::digest(&runtime_bytes)),
            capability_file,
        },
        monero: EffectMoneroRpc {
            daemon: effect_rpc(effect_root, "daemon", 36974),
            funding_wallet: effect_rpc(effect_root, "funding", 36975),
            shared_wallet: effect_rpc(effect_root, "shared", 36976),
            role_wallet: effect_rpc(effect_root, "taker", 36977),
            shared_wallet_file_password_file: effect_root.join("shared-wallet-file.password"),
        },
        taker_tools: TakerEffectTools {
            tag14_authorize: effect_tool(
                effect_root,
                "tag14-authorize",
                tag14_worker.as_bytes(),
                if tag14_release.is_some() {
                    "lez_xmr_tag14_release_v2"
                } else {
                    "lez_xmr_tag14_authorize_v1"
                },
            ),
            finalized_classifier: effect_tool(
                effect_root,
                "finalized-classifier",
                finalized_worker.as_bytes(),
                "lez_xmr_finalized_classifier_v1",
            ),
            monero_claim: effect_tool(
                effect_root,
                "monero-claim",
                b"#!/bin/sh\nexit 0\n",
                "lez_xmr_monero_claim_sweep_v2",
            ),
            monero_verify: effect_tool(
                effect_root,
                "monero-verify",
                b"#!/bin/sh\nexit 0\n",
                "lez_xmr_monero_verify_v2",
            ),
            tag16_refund: effect_tool(
                effect_root,
                "tag16-refund",
                tag16_worker.as_bytes(),
                "lez_xmr_tag16_refund_v1",
            ),
        },
        tag14_release,
    }
}

fn effect_program_id_bytes(words: [u32; 8]) -> [u8; 32] {
    let mut bytes = [0_u8; 32];
    for (index, word) in words.into_iter().enumerate() {
        bytes[index * 4..index * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    bytes
}

#[derive(Serialize)]
struct EffectTool {
    program: PathBuf,
    program_sha256: String,
    abi: &'static str,
}

#[derive(Serialize)]
struct TakerEffectTools {
    tag14_authorize: EffectTool,
    finalized_classifier: EffectTool,
    monero_claim: EffectTool,
    monero_verify: EffectTool,
    tag16_refund: EffectTool,
}

#[derive(Serialize)]
struct EffectLezRpc {
    sidecar_url: String,
    runtime_file: PathBuf,
    runtime_sha256: String,
    capability_file: PathBuf,
}

#[derive(Serialize)]
struct EffectAuthenticatedRpc {
    url: String,
    username_file: PathBuf,
    password_file: PathBuf,
}

#[derive(Serialize)]
struct EffectMoneroRpc {
    daemon: EffectAuthenticatedRpc,
    funding_wallet: EffectAuthenticatedRpc,
    shared_wallet: EffectAuthenticatedRpc,
    role_wallet: EffectAuthenticatedRpc,
    shared_wallet_file_password_file: PathBuf,
}

#[derive(Serialize)]
struct Tag14ReleaseAuthority {
    sidecar_url: String,
    indexer_url: String,
    node_profile: &'static str,
    state_directory: PathBuf,
    capability_file: PathBuf,
    protection_key_file: PathBuf,
    protection_key_id: &'static str,
}

#[derive(Serialize)]
struct TakerEffectAuthority {
    schema_version: u16,
    pair: &'static str,
    role: &'static str,
    swap_id: String,
    agreement_commitment: String,
    activation_commitment: String,
    run_id: String,
    workflow_journal: PathBuf,
    adaptor_journal: PathBuf,
    evidence_root: PathBuf,
    lez: EffectLezRpc,
    monero: EffectMoneroRpc,
    taker_tools: TakerEffectTools,
    #[serde(skip_serializing_if = "Option::is_none")]
    tag14_release: Option<Tag14ReleaseAuthority>,
}

fn effect_tool(root: &Path, name: &str, program: &[u8], abi: &'static str) -> EffectTool {
    let path = root.join(name);
    write_private_executable(&path, program);
    EffectTool {
        program: path,
        program_sha256: hex::encode(Sha256::digest(program)),
        abi,
    }
}

fn effect_rpc(root: &Path, name: &str, port: u16) -> EffectAuthenticatedRpc {
    EffectAuthenticatedRpc {
        url: format!("http://127.0.0.1:{port}/"),
        username_file: root.join(format!("{name}.username")),
        password_file: root.join(format!("{name}.password")),
    }
}
