//! Exercises the take-time Zcash agreement over Chat; compiled only with `pair-zec`.
#![cfg(feature = "pair-zec")]

//! Process proof for the run-local ZEC Chat boundary. A real Maker daemon and
//! the real Taker service agree a Zcash swap at take time (`zec_reserve_v1` →
//! `zec_chat_propose_v1` → `zec_chat_complete_v1`) across a Delivery outage, a
//! Chat outage, a Maker restart and a lost answer, each role with its own
//! keys. Zebra and each swap's LEZ sidecar are the fakes of
//! `support/zec_take_time.rs`.

#[path = "support/cross_role_binary.rs"]
mod cross_role;
#[path = "support/zec_take_time.rs"]
mod take_time;

use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::{Read as _, Write as _},
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    process::{Child, Command},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use jsonrpsee::RpcModule;
use lez_bridge_protocol::RequestId;
use lez_maker_node::{
    AuthenticatedOfferRefV1, DeliveryOfferQueryV1, ListRequest, LocalPriceSetRequest,
    PairConfigureRequest, RunLocalDelivery, TakerMakerIdentityV1, ZecChatCompleteRequestV1,
    ZecChatCompleteResponseV1, ZecChatProposeRequestV1, ZecReserveRequestV1, call_local_rpc,
};
use lez_swap_core::{Pair, Participant, Phase, SwapDirection, SwapId, UnixSeconds};
use lez_swap_sdk_core::OfferDiscovery as _;
use lez_swap_store::{
    ActorHeldLock, LocalPriceV1, MakerActorKindV1, MakerActorScheduleState, MakerOfferId,
    MakerOfferStatus, MakerPairConfigurationV1, MakerPriceSourceKind, MakerRouteV1,
    MakerZecNegotiationStatus, SqliteSwapStore, SqliteTakerFacadeStore, SqliteZecRecoveryStore,
    TakerFacadeActionV1,
};
use lez_taker_node::{
    TakerClaimRequestV1, TakerHealthRequestV1, TakerHealthV1, TakerInitiationCommitV1,
    TakerRefundRequestV1, TakerSwapInitiateRequestV1, TakerSwapListRequestV1, TakerSwapListV1,
    TakerSwapMonitorRequestV1, TakerSwapStateV1, TakerSwapViewV1, load_taker_service_context,
    taker_service_rpc_module,
};
use lez_zec_swap_sdk::{AcceptedZecAgreementV1, ProtectedClaimKey, ZecLifecycleAction, ZecPairSdk};
use rusqlite::{Connection, params};
use rustix::process::{Pid, Signal, kill_process};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use take_time::{
    FOREIGN_UNITS, FUNDER_KEY, LEZ_UNITS, OTHER_KEY, TakeTimeMaker, btc_role_config,
    derived_request_id, fake_sidecar_program, private_dir, start_fake_lez_node, start_fake_zebra,
    write_private, zec_role_config,
};
use tempfile::tempdir;
use zec_reference_actor::{ActorConfig, ActorRole, ZEC_ROLE_DRAFT_FILE};

const TAKER_DEPENDENCY_UNAVAILABLE: i64 = -32_010;
const TAKER_SWAP_NOT_FOUND: i64 = -32_014;
const TAKER_ACTION_UNAVAILABLE: i64 = -32_016;
const TAKER_ACTION_CONFLICT: i64 = -32_017;
/// The first test takes its offer across a Chat outage, a Maker restart and
/// two lost answers, all before the offer expires (the reservation and the
/// agreement's transcript expire with it), and then waits for that expiry to
/// replay after it: long enough for the take on a loaded host, short enough
/// to wait out.
const EXPIRING_OFFER_TTL_SECONDS: u64 = 60;
/// The second test never waits for its offer's expiry.
const LIVE_OFFER_TTL_SECONDS: u64 = 300;

/// The swap sidecar the Nodes spawn (`take_time::fake_sidecar_program`).
#[test]
#[ignore = "the fake LEZ sidecar the take-time tests spawn as a process"]
fn fake_sidecar_process() {
    take_time::serve_fake_sidecar();
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn separate_taker_countersigns_and_maker_atomically_accepts_before_response() {
    // Build the peer Taker first: its build may take a minute, longer than
    // the published offer lives.
    cross_role::workspace_binary("lez-taker-cli");
    let run = tempdir().expect("isolated Chat process root");
    let root = fs::canonicalize(run.path()).unwrap();
    let direction = SwapDirection::TakerSellsForeign;
    let chains = Chains {
        zebra: start_fake_zebra().await,
        lez_node: start_fake_lez_node().await,
        sidecar: fake_sidecar_program(&root),
    };
    let port_base = sidecar_port_base(0);
    // The Taker sells Zcash: it funds it, so the fake Zebra's output is its.
    let maker = Maker::new(&root, OTHER_KEY, &chains, port_base);
    let mut daemon = start_daemon(&maker);
    wait_ready(&mut daemon, &maker.ready, &maker.socket);
    let (route, offer_id) =
        prepare_live_offer_for_direction(&maker, direction, EXPIRING_OFFER_TTL_SECONDS).await;
    let authenticated = discover(&maker, route).await;
    let taker = Taker::new(&root, FUNDER_KEY, &chains, port_base + 10, &maker);
    let request = initiation_request(
        "m5-chat-initiation-001",
        &offer_id,
        route,
        &maker,
        &authenticated,
    );
    let reservation_id = derived_request_id(&request.request_id, b"node-reservation");
    let files = TakerFiles::of(&taker, &reservation_id);
    let take = Take {
        taker: &taker,
        request: &request,
        files: &files,
        direct_config: taker.service_config("direct", &maker.chat_socket),
    };

    assert_socket_method_isolation(&maker, &authenticated, &offer_id, &reservation_id).await;
    // The Maker reserves and stages its proposal; the answer to the proposal
    // is lost, so the Taker holds a prepared swap and no agreement.
    let staged = take_losing_an_answer(
        &take,
        &maker.chat_socket,
        "chat-drop-propose",
        vec![("zec_reserve_v1", false), ("zec_chat_propose_v1", true)],
    );
    assert_eq!(staged[1]["result"]["was_replay"], false);
    assert_eq!(staged[1]["result"]["offer_revision"], 2);
    let swap_id = SwapId::new(staged[0]["result"]["swap_id"].as_str().unwrap()).unwrap();
    assert!(swap_id.as_str().starts_with("zec-"));
    assert_eq!(
        SqliteSwapStore::open(&maker.database)
            .unwrap()
            .load_zec_maker_negotiation(&offer_id)
            .unwrap()
            .expect("the staged proposal is durable")
            .status(),
        MakerZecNegotiationStatus::Proposed
    );

    assert_chat_outage_and_restart(&take, &mut daemon, &maker);
    let pre_receipt = assert_completion_response_loss(&take, &maker, &offer_id, &swap_id);
    let final_wire = accept_and_replay(
        &take,
        &maker,
        direction,
        &swap_id,
        &authenticated,
        &pre_receipt,
    )
    .await;
    assert_post_acceptance_boundary(
        &mut daemon,
        &maker,
        &offer_id,
        &reservation_id,
        &swap_id,
        &authenticated,
        &final_wire,
        &files,
    );
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)]
async fn service_initiation_completes_real_chat_before_not_activated_response() {
    let run = tempdir().expect("isolated service Chat root");
    let root = fs::canonicalize(run.path()).unwrap();
    let chains = Chains {
        zebra: start_fake_zebra().await,
        lez_node: start_fake_lez_node().await,
        sidecar: fake_sidecar_program(&root),
    };
    let port_base = sidecar_port_base(20);
    // The Taker sells LEZ: the Maker funds Zcash from the fake Zebra's output.
    let maker = Maker::new(&root, FUNDER_KEY, &chains, port_base);
    let mut daemon = start_daemon(&maker);
    wait_ready(&mut daemon, &maker.ready, &maker.socket);
    let (route, offer_id) = prepare_live_offer(&maker, LIVE_OFFER_TTL_SECONDS).await;
    let authenticated = discover(&maker, route).await;
    let taker = Taker::new(&root, OTHER_KEY, &chains, port_base + 10, &maker);
    let request = initiation_request(
        "m6-service-chat-initiation-001",
        &offer_id,
        route,
        &maker,
        &authenticated,
    );
    // The Taker Node names the swap's directory, and the reservation, after
    // the request.
    let reservation_id = derived_request_id(&request.request_id, b"node-reservation");
    let taker_files = TakerFiles::of(&taker, &reservation_id);
    let registry = taker.registry.clone();
    let chat_socket = maker.chat_socket.clone();
    let delivery = maker.delivery.clone();
    let database = maker.database.clone();
    let service_config = taker.service_config("service", &chat_socket);

    let module =
        taker_service_rpc_module(load_taker_service_context(&service_config).unwrap()).unwrap();
    // Both roles accept the agreement during the take, before its transcript
    // expires with the offer; the test activates the Taker at that time.
    let taken_at = now();
    let first: TakerInitiationCommitV1 = module
        .call("taker_swap_initiate_v1", [request.clone()])
        .await
        .expect("the take reserves, proposes and completes over Chat");
    let swap_id = first.swap.swap_id.clone();
    assert!(swap_id.as_str().starts_with("zec-"));
    let first_artifacts = ServiceAcceptanceArtifacts::capture(&taker_files);
    drop(module);

    let delivery_offer = delivery.join(format!("{}.offer.json", offer_id.as_str()));
    fs::remove_file(&delivery_offer).unwrap();
    let offline_chat = chat_socket.with_file_name("chat.service-replay-offline");
    fs::rename(&chat_socket, &offline_chat).unwrap();
    let replay_module =
        taker_service_rpc_module(load_taker_service_context(&service_config).unwrap()).unwrap();
    let replay: TakerInitiationCommitV1 = replay_module
        .call("taker_swap_initiate_v1", [request.clone()])
        .await
        .expect("durable receipt replay must not use Delivery or Chat");
    let replay_artifacts = ServiceAcceptanceArtifacts::capture(&taker_files);
    let registered = replay_module
        .method_names()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    let health_response = service_rpc_response(
        &replay_module,
        "taker_health",
        json!([TakerHealthRequestV1 { schema_version: 1 }]),
    )
    .await;
    let list_response = service_rpc_response(
        &replay_module,
        "taker_swap_list_v1",
        json!([TakerSwapListRequestV1 { schema_version: 1 }]),
    )
    .await;
    let monitor_response = service_rpc_response(
        &replay_module,
        "taker_swap_monitor_v1",
        json!([TakerSwapMonitorRequestV1 {
            schema_version: 1,
            swap_id: swap_id.clone(),
        }]),
    )
    .await;
    let unknown_response = service_rpc_response(
        &replay_module,
        "taker_swap_monitor_v1",
        json!([TakerSwapMonitorRequestV1 {
            schema_version: 1,
            swap_id: SwapId::new("m6-service-unknown-swap-001").unwrap(),
        }]),
    )
    .await;
    let mismatched_response = service_rpc_response(
        &replay_module,
        "taker_swap_monitor_v1",
        json!([TakerSwapMonitorRequestV1 {
            schema_version: 1,
            swap_id: SwapId::new(offer_id.as_str()).unwrap(),
        }]),
    )
    .await;
    let canonical_artifacts =
        ServiceAcceptanceArtifacts::capture(&taker_files).expect("canonical acceptance artifacts");
    let original_receipt = taker_files
        .receipt
        .with_file_name("acceptance-receipt.original");
    fs::rename(&taker_files.receipt, &original_receipt).unwrap();
    write_private(&taker_files.receipt, &canonical_artifacts.receipt_bytes);
    let replacement_before =
        ServiceAcceptanceArtifacts::capture(&taker_files).expect("replacement receipt artifacts");
    assert_ne!(
        replacement_before.receipt_inode, canonical_artifacts.receipt_inode,
        "same-byte replacement must use a distinct receipt inode"
    );
    assert_eq!(
        replacement_before.receipt_bytes, canonical_artifacts.receipt_bytes,
        "receipt replacement must preserve the exact accepted bytes"
    );
    let replaced_receipt_response = service_rpc_response(
        &replay_module,
        "taker_swap_monitor_v1",
        json!([TakerSwapMonitorRequestV1 {
            schema_version: 1,
            swap_id: swap_id.clone(),
        }]),
    )
    .await;
    let replacement_after = ServiceAcceptanceArtifacts::capture(&taker_files)
        .expect("post-monitor replacement artifacts");
    fs::remove_file(&taker_files.receipt).unwrap();
    fs::rename(&original_receipt, &taker_files.receipt).unwrap();
    let restored_artifacts =
        ServiceAcceptanceArtifacts::capture(&taker_files).expect("restored acceptance artifacts");

    let monitor_config = ActorConfig::load_private(&canonical_artifacts.config_path).unwrap();
    let actor_lock =
        ActorHeldLock::acquire_for(monitor_config.swap_id(), monitor_config.role_state_db())
            .unwrap();
    let locked_actor_response = service_rpc_response(
        &replay_module,
        "taker_swap_monitor_v1",
        json!([TakerSwapMonitorRequestV1 {
            schema_version: 1,
            swap_id: swap_id.clone(),
        }]),
    )
    .await;
    let locked_artifacts =
        ServiceAcceptanceArtifacts::capture(&taker_files).expect("locked monitor artifacts");
    drop(actor_lock);
    let recovered_monitor_response = service_rpc_response(
        &replay_module,
        "taker_swap_monitor_v1",
        json!([TakerSwapMonitorRequestV1 {
            schema_version: 1,
            swap_id: swap_id.clone(),
        }]),
    )
    .await;
    let recovered_artifacts =
        ServiceAcceptanceArtifacts::capture(&taker_files).expect("recovered monitor artifacts");

    let missing_receipt = taker_files
        .receipt
        .with_file_name("acceptance-receipt.monitor-missing");
    fs::rename(&taker_files.receipt, &missing_receipt).unwrap();
    let missing_receipt_monitor_response = service_rpc_response(
        &replay_module,
        "taker_swap_monitor_v1",
        json!([TakerSwapMonitorRequestV1 {
            schema_version: 1,
            swap_id: swap_id.clone(),
        }]),
    )
    .await;
    let missing_receipt_list_response = service_rpc_response(
        &replay_module,
        "taker_swap_list_v1",
        json!([TakerSwapListRequestV1 { schema_version: 1 }]),
    )
    .await;
    fs::rename(&missing_receipt, &taker_files.receipt).unwrap();
    let missing_receipt_restored = ServiceAcceptanceArtifacts::capture(&taker_files)
        .expect("restored post-removal acceptance artifacts");

    let mut crossed_config: Value =
        serde_json::from_slice(&canonical_artifacts.config_bytes).unwrap();
    crossed_config["swap_id"] = json!("m6-crossed-swap-001");
    let crossed_config_bytes = serde_json::to_vec(&crossed_config).unwrap();
    let mut crossed_receipt: Value =
        serde_json::from_slice(&canonical_artifacts.receipt_bytes).unwrap();
    crossed_receipt["swap_id"] = json!("m6-crossed-swap-001");
    crossed_receipt["actor_config_sha256"] =
        json!(hex::encode(Sha256::digest(&crossed_config_bytes)));
    let crossed_receipt_bytes = serde_json::to_vec(&crossed_receipt).unwrap();
    overwrite_private_in_place(&canonical_artifacts.config_path, &crossed_config_bytes);
    overwrite_private_in_place(&taker_files.receipt, &crossed_receipt_bytes);
    let crossed_pair_before = ServiceAcceptanceArtifacts::capture(&taker_files)
        .expect("coherently crossed config and receipt");
    let crossed_pair_monitor_response = service_rpc_response(
        &replay_module,
        "taker_swap_monitor_v1",
        json!([TakerSwapMonitorRequestV1 {
            schema_version: 1,
            swap_id: swap_id.clone(),
        }]),
    )
    .await;
    let crossed_pair_list_response = service_rpc_response(
        &replay_module,
        "taker_swap_list_v1",
        json!([TakerSwapListRequestV1 { schema_version: 1 }]),
    )
    .await;
    let crossed_pair_after = ServiceAcceptanceArtifacts::capture(&taker_files)
        .expect("post-monitor coherently crossed artifacts");
    overwrite_private_in_place(
        &canonical_artifacts.config_path,
        &canonical_artifacts.config_bytes,
    );
    overwrite_private_in_place(&taker_files.receipt, &canonical_artifacts.receipt_bytes);
    let crossed_pair_restored = ServiceAcceptanceArtifacts::capture(&taker_files)
        .expect("restored post-crossed acceptance artifacts");

    let corrupt_state_bytes = b"m6-corrupt-role-state-not-sqlite";
    write_private(monitor_config.role_state_db(), corrupt_state_bytes);
    let corrupt_state_before = (
        fs::symlink_metadata(monitor_config.role_state_db())
            .unwrap()
            .ino(),
        fs::read(monitor_config.role_state_db()).unwrap(),
    );
    let corrupt_state_monitor_response = service_rpc_response(
        &replay_module,
        "taker_swap_monitor_v1",
        json!([TakerSwapMonitorRequestV1 {
            schema_version: 1,
            swap_id: swap_id.clone(),
        }]),
    )
    .await;
    let corrupt_state_list_response = service_rpc_response(
        &replay_module,
        "taker_swap_list_v1",
        json!([TakerSwapListRequestV1 { schema_version: 1 }]),
    )
    .await;
    let corrupt_state_after = (
        fs::symlink_metadata(monitor_config.role_state_db())
            .unwrap()
            .ino(),
        fs::read(monitor_config.role_state_db()).unwrap(),
    );
    let corrupt_state_artifacts = ServiceAcceptanceArtifacts::capture(&taker_files)
        .expect("corrupt-state monitor acceptance artifacts");
    let corrupt_state_bridge_absent = !monitor_config.bridge_journal_db().exists();
    fs::remove_file(monitor_config.role_state_db()).unwrap();

    let final_recovered_monitor_response = service_rpc_response(
        &replay_module,
        "taker_swap_monitor_v1",
        json!([TakerSwapMonitorRequestV1 {
            schema_version: 1,
            swap_id: swap_id.clone(),
        }]),
    )
    .await;
    let final_recovered_list_response = service_rpc_response(
        &replay_module,
        "taker_swap_list_v1",
        json!([TakerSwapListRequestV1 { schema_version: 1 }]),
    )
    .await;
    let final_recovered_artifacts = ServiceAcceptanceArtifacts::capture(&taker_files)
        .expect("final recovered monitor artifacts");
    let pre_activation_effects_absent =
        !monitor_config.role_state_db().exists() && !monitor_config.bridge_journal_db().exists();

    activate_service_taker_without_chain(
        &monitor_config,
        &canonical_artifacts.config_bytes,
        &canonical_artifacts.agreement_bytes,
        taken_at,
    )
    .await;
    let active_logical_before = active_taker_logical_state(monitor_config.role_state_db());
    let active_monitor_response = service_rpc_response(
        &replay_module,
        "taker_swap_monitor_v1",
        json!([TakerSwapMonitorRequestV1 {
            schema_version: 1,
            swap_id: swap_id.clone(),
        }]),
    )
    .await;
    let active_list_response = service_rpc_response(
        &replay_module,
        "taker_swap_list_v1",
        json!([TakerSwapListRequestV1 { schema_version: 1 }]),
    )
    .await;
    let active_claim_response = service_rpc_response(
        &replay_module,
        "taker_swap_claim_v1",
        json!([TakerClaimRequestV1 {
            schema_version: 1,
            request_id: RequestId::new("m6-active-claim-unavailable").unwrap(),
            swap_id: swap_id.clone(),
            expected_generation: 0,
        }]),
    )
    .await;
    let active_refund_response = service_rpc_response(
        &replay_module,
        "taker_swap_refund_v1",
        json!([TakerRefundRequestV1 {
            schema_version: 1,
            request_id: RequestId::new("m6-active-refund-unavailable").unwrap(),
            swap_id: swap_id.clone(),
            expected_generation: 0,
        }]),
    )
    .await;
    for response in [&active_claim_response, &active_refund_response] {
        assert_service_rpc_error(
            response,
            TAKER_ACTION_UNAVAILABLE,
            "Taker action unavailable",
            "taker_action_unavailable",
        );
    }
    let action_rows: i64 = Connection::open(&registry)
        .unwrap()
        .query_row(
            "SELECT count(*) FROM taker_facade_requests
             WHERE operation IN ('claim', 'refund')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(action_rows, 0, "rejected actions must not be admitted");

    assert_eq!(
        active_taker_logical_state(monitor_config.role_state_db()),
        active_logical_before,
        "active monitoring must not advance durable lifecycle state"
    );

    {
        let connection = Connection::open(monitor_config.role_state_db()).unwrap();
        connection
            .execute(
                "UPDATE zec_sdk_agreements SET payload_version = 99
                 WHERE local_role = 'taker' AND swap_id = ?1",
                params![monitor_config.swap_id().as_str()],
            )
            .unwrap();
    }
    let future_logical_before = active_taker_logical_state(monitor_config.role_state_db());
    let future_monitor_response = service_rpc_response(
        &replay_module,
        "taker_swap_monitor_v1",
        json!([TakerSwapMonitorRequestV1 {
            schema_version: 1,
            swap_id: swap_id.clone(),
        }]),
    )
    .await;
    let future_list_response = service_rpc_response(
        &replay_module,
        "taker_swap_list_v1",
        json!([TakerSwapListRequestV1 { schema_version: 1 }]),
    )
    .await;
    assert_eq!(
        active_taker_logical_state(monitor_config.role_state_db()),
        future_logical_before,
        "future durable payload rejection must not rewrite actor state"
    );
    {
        let connection = Connection::open(monitor_config.role_state_db()).unwrap();
        connection
            .execute(
                "UPDATE zec_sdk_agreements SET payload_version = ?1
                 WHERE local_role = 'taker' AND swap_id = ?2",
                params![
                    active_logical_before.payload_version,
                    monitor_config.swap_id().as_str()
                ],
            )
            .unwrap();
    }
    let future_recovered_response = service_rpc_response(
        &replay_module,
        "taker_swap_monitor_v1",
        json!([TakerSwapMonitorRequestV1 {
            schema_version: 1,
            swap_id: swap_id.clone(),
        }]),
    )
    .await;

    {
        let connection = Connection::open(monitor_config.role_state_db()).unwrap();
        connection
            .execute(
                "UPDATE zec_sdk_agreements SET agreement_wire = X'00'
                 WHERE local_role = 'taker' AND swap_id = ?1",
                params![monitor_config.swap_id().as_str()],
            )
            .unwrap();
    }
    let malformed_logical_before = active_taker_logical_state(monitor_config.role_state_db());
    let malformed_monitor_response = service_rpc_response(
        &replay_module,
        "taker_swap_monitor_v1",
        json!([TakerSwapMonitorRequestV1 {
            schema_version: 1,
            swap_id: swap_id.clone(),
        }]),
    )
    .await;
    let malformed_list_response = service_rpc_response(
        &replay_module,
        "taker_swap_list_v1",
        json!([TakerSwapListRequestV1 { schema_version: 1 }]),
    )
    .await;
    assert_eq!(
        active_taker_logical_state(monitor_config.role_state_db()),
        malformed_logical_before,
        "malformed durable wire rejection must not rewrite actor state"
    );
    {
        let connection = Connection::open(monitor_config.role_state_db()).unwrap();
        connection
            .execute(
                "UPDATE zec_sdk_agreements SET agreement_wire = ?1
                 WHERE local_role = 'taker' AND swap_id = ?2",
                params![
                    &active_logical_before.agreement_wire,
                    monitor_config.swap_id().as_str()
                ],
            )
            .unwrap();
    }
    let active_recovered_response = service_rpc_response(
        &replay_module,
        "taker_swap_monitor_v1",
        json!([TakerSwapMonitorRequestV1 {
            schema_version: 1,
            swap_id: swap_id.clone(),
        }]),
    )
    .await;
    let admitted_claim_request = RequestId::new("m6-monitor-admitted-claim").unwrap();
    let mut action_registry = SqliteTakerFacadeStore::open_existing(&registry).unwrap();
    let admitted_claim = action_registry
        .admit_action(
            &admitted_claim_request,
            monitor_config.swap_id(),
            TakerFacadeActionV1::Claim,
            0,
            taken_at,
        )
        .unwrap();
    assert!(!admitted_claim.was_replay());
    drop(action_registry);
    let conflicting_refund_response = service_rpc_response(
        &replay_module,
        "taker_swap_refund_v1",
        json!([TakerRefundRequestV1 {
            schema_version: 1,
            request_id: RequestId::new("m6-monitor-conflicting-refund").unwrap(),
            swap_id: swap_id.clone(),
            expected_generation: 0,
        }]),
    )
    .await;
    assert_service_rpc_error(
        &conflicting_refund_response,
        TAKER_ACTION_CONFLICT,
        "Taker action conflict",
        "taker_action_conflict",
    );
    let retained_action_rows: Vec<(String, String)> = Connection::open(&registry)
        .unwrap()
        .prepare(
            "SELECT request_id, operation FROM taker_facade_requests
             WHERE operation IN ('claim', 'refund') ORDER BY request_id",
        )
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        retained_action_rows,
        vec![(
            admitted_claim_request.as_str().to_owned(),
            "claim".to_owned()
        )]
    );
    let in_progress_monitor_response = service_rpc_response(
        &replay_module,
        "taker_swap_monitor_v1",
        json!([TakerSwapMonitorRequestV1 {
            schema_version: 1,
            swap_id: swap_id.clone(),
        }]),
    )
    .await;

    let active_logical_after = active_taker_logical_state(monitor_config.role_state_db());
    let active_bridge_absent = !monitor_config.bridge_journal_db().exists();
    let post_read_artifacts = ServiceAcceptanceArtifacts::capture(&taker_files);
    drop(replay_module);
    fs::rename(&offline_chat, &chat_socket).unwrap();
    stop_daemon_gracefully(&mut daemon, &maker);

    assert!(!first.was_replay);
    assert_eq!(first.swap.state, TakerSwapStateV1::NotActivated);
    assert_eq!(first.swap.progress_generation, 0);
    assert_eq!(first.swap.available_action, None);
    assert!(replay.was_replay);
    assert_eq!(replay.swap, first.swap);

    assert_eq!(registered.len(), 9, "registered methods: {registered:?}");
    for method in [
        "taker_health",
        "taker_offer_list_v1",
        "taker_swap_list_v1",
        "taker_swap_lock_v1",
        "taker_swap_initiate_v1",
        "taker_swap_monitor_v1",
        "taker_swap_claim_v1",
        "taker_swap_refund_v1",
        "taker_wallet_balances_v1",
    ] {
        assert!(
            registered.contains(method),
            "missing registered method {method}"
        );
    }
    let health: TakerHealthV1 = serde_json::from_value(health_response["result"].clone()).unwrap();
    let methods = health.registered_methods();
    assert!(methods.swap_list());
    assert!(methods.monitor());
    assert!(methods.claim());
    assert!(methods.refund());

    let listed: TakerSwapListV1 = serde_json::from_value(list_response["result"].clone()).unwrap();
    assert_eq!(listed.schema_version, 1);
    assert_eq!(listed.swaps, vec![first.swap.clone()]);
    let monitored: TakerSwapViewV1 =
        serde_json::from_value(monitor_response["result"].clone()).unwrap();
    assert_eq!(monitored, first.swap);
    assert_eq!(monitored.state, TakerSwapStateV1::NotActivated);
    assert_eq!(monitored.progress_generation, 0);
    assert_eq!(monitored.available_action, None);
    assert_eq!(monitored.privacy_guidance, None);
    for response in [&unknown_response, &mismatched_response] {
        assert_service_rpc_error(
            response,
            TAKER_SWAP_NOT_FOUND,
            "Taker swap not found",
            "swap_not_found",
        );
    }
    // A swap whose bound bundle fails the checks it was pinned by (a replaced
    // receipt, a configuration copied from another swap) needs attention and
    // offers no action; it is not a Node outage and does not hide the list
    // (85e1e5c0).
    for response in [&replaced_receipt_response, &crossed_pair_monitor_response] {
        let view: TakerSwapViewV1 = serde_json::from_value(response["result"].clone())
            .unwrap_or_else(|_| panic!("a swap with an altered bundle is a row: {response}"));
        assert_eq!(
            view.state,
            TakerSwapStateV1::AttentionRequired,
            "{response}"
        );
        assert_eq!(view.available_action, None, "{response}");
    }
    let crossed_list: TakerSwapListV1 =
        serde_json::from_value(crossed_pair_list_response["result"].clone()).unwrap();
    assert_eq!(crossed_list.swaps.len(), 1);
    assert_eq!(
        crossed_list.swaps[0].state,
        TakerSwapStateV1::AttentionRequired
    );
    assert_eq!(crossed_list.swaps[0].available_action, None);
    for response in [
        &locked_actor_response,
        &corrupt_state_monitor_response,
        &corrupt_state_list_response,
        &missing_receipt_monitor_response,
        &missing_receipt_list_response,
        &future_monitor_response,
        &future_list_response,
        &malformed_monitor_response,
        &malformed_list_response,
    ] {
        assert_service_rpc_error(
            response,
            TAKER_DEPENDENCY_UNAVAILABLE,
            "Taker dependency unavailable",
            "taker_monitor_unavailable",
        );
        assert!(
            response.get("result").is_none(),
            "dependency corruption must not downgrade into a plausible swap state: {response}"
        );
    }
    let recovered_monitor: TakerSwapViewV1 =
        serde_json::from_value(recovered_monitor_response["result"].clone()).unwrap();
    assert_eq!(recovered_monitor, first.swap);
    let final_recovered_monitor: TakerSwapViewV1 =
        serde_json::from_value(final_recovered_monitor_response["result"].clone()).unwrap();
    assert_eq!(final_recovered_monitor, first.swap);
    let final_recovered_list: TakerSwapListV1 =
        serde_json::from_value(final_recovered_list_response["result"].clone()).unwrap();
    assert_eq!(final_recovered_list.swaps, vec![first.swap.clone()]);

    let mut expected_active = first.swap.clone();
    expected_active.state = TakerSwapStateV1::AwaitingFirstLock;
    expected_active.progress_generation = 0;
    expected_active.available_action = None;
    expected_active.privacy_guidance = None;
    // An activated Zcash swap's row names its Taker actor phase (REVIEW 08:59Z).
    expected_active.actor_phase = Some("offered".to_owned());
    let active_monitor: TakerSwapViewV1 =
        serde_json::from_value(active_monitor_response["result"].clone()).unwrap();
    assert_eq!(active_monitor, expected_active);
    let active_list: TakerSwapListV1 =
        serde_json::from_value(active_list_response["result"].clone()).unwrap();
    assert_eq!(active_list.swaps, vec![expected_active.clone()]);
    let future_recovered: TakerSwapViewV1 =
        serde_json::from_value(future_recovered_response["result"].clone()).unwrap();
    assert_eq!(future_recovered, expected_active);
    let active_recovered: TakerSwapViewV1 =
        serde_json::from_value(active_recovered_response["result"].clone()).unwrap();
    assert_eq!(active_recovered, expected_active);
    let in_progress: TakerSwapViewV1 =
        serde_json::from_value(in_progress_monitor_response["result"].clone()).unwrap();
    // The admission is retained (below), but the actor offers no claim while
    // it awaits its first lock, so the row is the actor's own view: an
    // admitted action reads "in progress" only while the actor still offers
    // it at that generation (ed22e20b).
    assert_eq!(in_progress, expected_active);
    assert_eq!(in_progress.available_action, None);
    let retained_action = SqliteTakerFacadeStore::open_existing(&registry)
        .unwrap()
        .lookup_action_for_swap(monitor_config.swap_id())
        .unwrap()
        .expect("failed actor replay must retain the sole authorization");
    assert_eq!(retained_action.action(), TakerFacadeActionV1::Claim);
    assert_eq!(retained_action.requested_after_generation(), 0);

    assert_eq!(
        active_logical_after, active_logical_before,
        "restoring exact durable fields must recover the original active state"
    );
    assert!(
        active_bridge_absent,
        "offline active monitoring must not create an LEZ bridge journal"
    );

    assert_service_responses_redacted(
        [
            &health_response,
            &list_response,
            &monitor_response,
            &unknown_response,
            &mismatched_response,
        ],
        &root,
        &reservation_id,
        &taker_files,
    );
    assert_service_responses_redacted(
        [
            &replaced_receipt_response,
            &locked_actor_response,
            &recovered_monitor_response,
        ],
        &root,
        &reservation_id,
        &taker_files,
    );
    assert_service_responses_redacted(
        [
            &missing_receipt_monitor_response,
            &missing_receipt_list_response,
            &crossed_pair_monitor_response,
            &crossed_pair_list_response,
            &corrupt_state_monitor_response,
            &corrupt_state_list_response,
            &final_recovered_monitor_response,
            &final_recovered_list_response,
            &active_monitor_response,
            &active_list_response,
            &future_monitor_response,
            &future_list_response,
            &future_recovered_response,
            &malformed_monitor_response,
            &malformed_list_response,
            &active_recovered_response,
            &in_progress_monitor_response,
        ],
        &root,
        &reservation_id,
        &taker_files,
    );

    let first_artifacts = first_artifacts.expect("service must provision Taker actor and receipt");
    let replay_artifacts =
        replay_artifacts.expect("receipt-bound replay must retain Taker artifacts");
    assert_eq!(replay_artifacts, first_artifacts);
    assert_eq!(replacement_after, replacement_before);
    assert_eq!(restored_artifacts, first_artifacts);
    assert_eq!(locked_artifacts, first_artifacts);
    assert_eq!(recovered_artifacts, first_artifacts);
    assert_eq!(missing_receipt_restored, first_artifacts);
    assert_eq!(
        crossed_pair_before.config_inode,
        first_artifacts.config_inode
    );
    assert_eq!(
        crossed_pair_before.receipt_inode,
        first_artifacts.receipt_inode
    );
    assert_ne!(
        crossed_pair_before.config_bytes,
        first_artifacts.config_bytes
    );
    assert_ne!(
        crossed_pair_before.receipt_bytes,
        first_artifacts.receipt_bytes
    );
    assert_eq!(crossed_pair_after, crossed_pair_before);
    assert_eq!(crossed_pair_restored, first_artifacts);
    assert_eq!(corrupt_state_before, corrupt_state_after);
    assert_eq!(corrupt_state_artifacts, first_artifacts);
    assert!(
        corrupt_state_bridge_absent,
        "corrupt role state must not authorize a bridge effect"
    );
    assert_eq!(final_recovered_artifacts, first_artifacts);
    assert_eq!(
        post_read_artifacts.expect("read RPCs must retain Taker artifacts"),
        first_artifacts,
    );
    assert_private_receipt(&taker_files.receipt);
    assert!(
        pre_activation_effects_absent,
        "admission and pre-activation monitoring must not create Taker chain effects"
    );

    let taker_config = ActorConfig::load_private(&first_artifacts.config_path).unwrap();
    assert_eq!(taker_config.role(), ActorRole::Taker);
    assert_eq!(taker_config.swap_id(), &swap_id);
    assert!(
        taker_config.role_state_db().is_file(),
        "the explicit no-RPC active-state fixture must retain its role database"
    );
    assert!(
        !taker_config.bridge_journal_db().exists(),
        "admission must not execute an LEZ bridge effect"
    );

    let maker_store = SqliteSwapStore::open(&database).unwrap();
    let negotiation = maker_store
        .load_zec_maker_negotiation(&offer_id)
        .unwrap()
        .expect("real Chat negotiation must be durable");
    assert_eq!(negotiation.status(), MakerZecNegotiationStatus::Completed);
    assert_eq!(negotiation.reservation_id(), &reservation_id);
    assert!(!negotiation.maker_proposal_wire().is_empty());
    assert!(negotiation.final_agreement_wire().is_some());
    let maker_actors = maker_store.list_maker_actor_processes().unwrap();
    assert_eq!(maker_actors.len(), 1);
    assert_eq!(
        maker_actors[0].schedule_state(),
        MakerActorScheduleState::Queued
    );
    assert!(
        !maker_actors[0].manifest().state_database_path().exists(),
        "acceptance must not start the Maker actor or perform a chain effect"
    );
    drop(maker_store);

    assert!(
        SqliteTakerFacadeStore::open_existing(&registry)
            .unwrap()
            .lookup_initiation(&request.request_id)
            .unwrap()
            .is_some()
    );
}

#[derive(Debug, Eq, PartialEq)]
struct ServiceAcceptanceArtifacts {
    config_path: PathBuf,
    config_inode: u64,
    config_bytes: Vec<u8>,
    agreement_path: PathBuf,
    agreement_inode: u64,
    agreement_bytes: Vec<u8>,
    receipt_inode: u64,
    receipt_bytes: Vec<u8>,
}

impl ServiceAcceptanceArtifacts {
    fn capture(files: &TakerFiles) -> Option<Self> {
        let config_path = files.actor_root.join("taker/actor-config.json");
        let agreement_path = files.actor_root.join("shared/agreement-v2.borsh");
        if !config_path.is_file() || !agreement_path.is_file() || !files.receipt.is_file() {
            return None;
        }
        Some(Self {
            config_inode: fs::symlink_metadata(&config_path).ok()?.ino(),
            config_bytes: fs::read(&config_path).ok()?,
            agreement_inode: fs::symlink_metadata(&agreement_path).ok()?.ino(),
            agreement_bytes: fs::read(&agreement_path).ok()?,
            receipt_inode: fs::symlink_metadata(&files.receipt).ok()?.ino(),
            receipt_bytes: fs::read(&files.receipt).ok()?,
            config_path,
            agreement_path,
        })
    }
}

#[derive(Debug, Eq, PartialEq)]
struct ActiveTakerLogicalState {
    payload_version: i64,
    agreement_wire: Vec<u8>,
    accepted_at: i64,
    accepted_revision: i64,
    active_revision: i64,
    taker_agreement_rows: i64,
}

async fn activate_service_taker_without_chain(
    config: &ActorConfig,
    config_bytes: &[u8],
    agreement_wire: &[u8],
    accepted_at_unix_seconds: u64,
) {
    let accepted = AcceptedZecAgreementV1::accept_wire_at(
        agreement_wire,
        UnixSeconds::new(accepted_at_unix_seconds),
        Participant::Taker,
        0,
    )
    .expect("the agreement is valid at the time of the take");
    // The swap's own claim-recovery key, as its actor configuration names it.
    let recovery = &serde_json::from_slice::<Value>(config_bytes).unwrap()["claim_recovery"];
    let key: [u8; 32] = fs::read(recovery["key_file"].as_str().unwrap())
        .unwrap()
        .try_into()
        .expect("a raw 32-byte claim-recovery key");
    let claim_key = ProtectedClaimKey::new(recovery["key_id"].as_str().unwrap(), key)
        .expect("the swap's claim key");
    let store = SqliteZecRecoveryStore::open_claim_capable(
        config.role_state_db(),
        Participant::Taker,
        claim_key,
    )
    .expect("create exact Taker role-state store");
    let sdk = ZecPairSdk::new(Participant::Taker, (), (), (), (), store);
    let active = sdk
        .activate(accepted)
        .await
        .expect("activate Taker using unit ports only");
    assert_eq!(active.status(), Phase::Offered);
    assert_eq!(active.revision(), 0);
    assert_eq!(active.next_action(), ZecLifecycleAction::CreateAndFundLez);
}

fn active_taker_logical_state(path: &Path) -> ActiveTakerLogicalState {
    let connection = Connection::open(path).expect("inspect active Taker state");
    let state = connection
        .query_row(
            "SELECT payload_version, agreement_wire, accepted_at,
                    accepted_revision, active_revision,
                    (SELECT COUNT(*) FROM zec_sdk_agreements
                     WHERE local_role = 'taker')
             FROM zec_sdk_agreements
             WHERE local_role = 'taker'",
            [],
            |row| {
                Ok(ActiveTakerLogicalState {
                    payload_version: row.get(0)?,
                    agreement_wire: row.get(1)?,
                    accepted_at: row.get(2)?,
                    accepted_revision: row.get(3)?,
                    active_revision: row.get(4)?,
                    taker_agreement_rows: row.get(5)?,
                })
            },
        )
        .expect("one exact active Taker agreement");
    assert_eq!(state.taker_agreement_rows, 1);
    state
}

async fn service_rpc_response(module: &RpcModule<()>, method: &str, params: Value) -> Value {
    let request = json!({
        "jsonrpc": "2.0",
        "id": 17,
        "method": method,
        "params": params,
    });
    let (response, _) = module
        .raw_json_request(&request.to_string(), 1)
        .await
        .unwrap();
    serde_json::from_str(response.get()).unwrap()
}

fn assert_service_rpc_error(response: &Value, code: i64, message: &str, category: &str) {
    assert_eq!(response["error"]["code"], code, "{response}");
    assert_eq!(response["error"]["message"], message, "{response}");
    assert_eq!(
        response["error"]["data"]["category"], category,
        "{response}"
    );
}

fn assert_service_responses_redacted<const N: usize>(
    responses: [&Value; N],
    run_root: &Path,
    reservation_id: &RequestId,
    files: &TakerFiles,
) {
    let private_markers = [
        run_root.display().to_string(),
        reservation_id.as_str().to_owned(),
        files.draft.display().to_string(),
        files.key.display().to_string(),
        files.source_actor_config.display().to_string(),
        files.agreement.display().to_string(),
        files.actor_root.display().to_string(),
        files.receipt.display().to_string(),
    ];
    for response in responses {
        let wire = response.to_string();
        for marker in &private_markers {
            assert!(!wire.contains(marker), "private marker leaked in {wire}");
        }
    }
}

async fn prepare_live_offer(maker: &Maker, ttl_seconds: u64) -> (MakerRouteV1, MakerOfferId) {
    prepare_live_offer_for_direction(maker, SwapDirection::TakerSellsLez, ttl_seconds).await
}

async fn prepare_live_offer_for_direction(
    maker: &Maker,
    direction: SwapDirection,
    ttl_seconds: u64,
) -> (MakerRouteV1, MakerOfferId) {
    let route = MakerRouteV1::new(Pair::Zcash, direction).unwrap();
    configure_live_route(&maker.socket, route, ttl_seconds).await;
    let offer_id = MakerOfferId::new("m5-chat-offer-001").unwrap();
    assert_delivery_outage_is_visible_and_exact_retry_recovers(
        &maker.socket,
        &maker.database,
        &maker.delivery,
        &offer_id,
    );
    (route, offer_id)
}

async fn discover(maker: &Maker, route: MakerRouteV1) -> AuthenticatedOfferRefV1 {
    RunLocalDelivery::subscriber(&maker.delivery, maker.key)
        .unwrap()
        .discover(&DeliveryOfferQueryV1::for_route(route, now()))
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("published offer is discoverable")
}

fn initiation_request(
    request_id: &str,
    offer_id: &MakerOfferId,
    route: MakerRouteV1,
    maker: &Maker,
    authenticated: &AuthenticatedOfferRefV1,
) -> TakerSwapInitiateRequestV1 {
    TakerSwapInitiateRequestV1 {
        schema_version: 1,
        request_id: request(request_id),
        offer_id: offer_id.clone(),
        route,
        maker_identity: TakerMakerIdentityV1::new(maker.key.serialize()).unwrap(),
        signed_envelope_sha256: authenticated.commitment(),
        foreign_units: FOREIGN_UNITS,
        expected_lez_units: LEZ_UNITS,
        logos_offer_announcement_base64: None,
        funding_transaction_hex: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn assert_post_acceptance_boundary(
    daemon: &mut Daemon,
    maker: &Maker,
    offer_id: &MakerOfferId,
    reservation_id: &RequestId,
    swap_id: &SwapId,
    authenticated: &AuthenticatedOfferRefV1,
    final_wire: &[u8],
    files: &TakerFiles,
) {
    assert_completed_process(
        daemon,
        maker,
        offer_id,
        reservation_id,
        swap_id,
        authenticated,
        final_wire,
        &fs::read(&files.preimage).expect("the Taker's swap secret"),
    );
    fs::rename(
        &maker.delivery,
        maker.delivery.with_file_name("delivery.offline"),
    )
    .expect("remove Delivery from the post-lock Taker boundary");
    assert_receipt_monitor_is_offline(&files.receipt);
}

#[allow(clippy::too_many_arguments)]
fn assert_completed_process(
    daemon: &mut Daemon,
    maker: &Maker,
    offer_id: &MakerOfferId,
    reservation_id: &RequestId,
    swap_id: &SwapId,
    authenticated: &AuthenticatedOfferRefV1,
    final_wire: &[u8],
    preimage: &[u8],
) {
    let completed_health = run_maker_health(&maker.socket);
    assert_eq!(completed_health["ready"], true);
    // Health reconciliation removes the consumed envelope after its expiry,
    // so the durable offer set and Delivery projection agree again.
    assert_eq!(
        completed_health["degraded"], false,
        "unexpected completed health: {completed_health}"
    );
    assert_eq!(completed_health["delivery"], "available");
    assert_eq!(completed_health["chat"], "available");
    assert_eq!(completed_health["routes"][0]["state"], "disabled");
    assert_eq!(fs::read_dir(&maker.delivery).unwrap().count(), 0);
    stop_daemon_gracefully(daemon, maker);
    assert_completed_durable(
        &maker.database,
        offer_id,
        reservation_id,
        swap_id,
        authenticated,
        final_wire,
        preimage,
    );
}

/// One take as the Taker Node's owner repeats it: the same request against
/// the same Node home, whatever Chat socket the attempt reaches the Maker by.
struct Take<'a> {
    taker: &'a Taker,
    request: &'a TakerSwapInitiateRequestV1,
    files: &'a TakerFiles,
    /// The Taker's configuration naming the Maker's own Chat socket.
    direct_config: PathBuf,
}

impl Take<'_> {
    fn run(&self) -> Result<TakerInitiationCommitV1, String> {
        take_once(&self.direct_config, self.request)
    }
}

fn assert_chat_outage_and_restart(take: &Take<'_>, daemon: &mut Daemon, maker: &Maker) {
    let offline_chat = maker.chat_socket.with_file_name("chat.offline");
    fs::rename(&maker.chat_socket, &offline_chat)
        .expect("make Chat unavailable without stopping owner RPC");
    let degraded = run_maker_health(&maker.socket);
    assert_eq!(degraded["ready"], true);
    assert_eq!(degraded["degraded"], true);
    assert_eq!(degraded["delivery"], "available");
    assert_eq!(degraded["chat"], "unavailable");
    let unavailable = take.run();
    assert!(
        unavailable.is_err(),
        "Chat outage must be visible to the real taker"
    );
    assert!(
        !take.files.agreement.exists(),
        "Chat outage must not create a final agreement"
    );
    assert!(
        !take.files.actor_root.exists(),
        "failed Chat completion must not publish Taker actor authority"
    );
    assert!(
        !take.files.receipt.exists(),
        "failed Chat completion must not publish an acceptance receipt"
    );
    fs::rename(&offline_chat, &maker.chat_socket)
        .expect("restore Chat socket identity before shutdown");
    stop_daemon_gracefully(daemon, maker);
    *daemon = start_daemon(maker);
    wait_ready(daemon, &maker.ready, &maker.socket);
}

fn assert_delivery_outage_is_visible_and_exact_retry_recovers(
    socket: &std::path::Path,
    database: &std::path::Path,
    delivery: &std::path::Path,
    offer_id: &MakerOfferId,
) {
    fs::set_permissions(delivery, fs::Permissions::from_mode(0o755))
        .expect("make Delivery projection insecure");
    let pair_store = SqliteSwapStore::open(database)
        .expect("open Maker store for exact publish route")
        .list_maker_pairs()
        .expect("list configured ZEC routes");
    let route = pair_store
        .iter()
        .find(|record| record.value().route().pair() == Pair::Zcash)
        .expect("configured ZEC route")
        .value()
        .route();
    let failed = maker_publish_command(socket, offer_id, route.direction())
        .output()
        .expect("run real maker during Delivery outage");
    assert!(
        !failed.status.success(),
        "Delivery outage must be visible to the real maker"
    );

    let store = SqliteSwapStore::open(database).expect("open maker state after failed projection");
    let offers = store
        .list_maker_offer_history(now())
        .expect("list durable offers after failed projection");
    assert_eq!(
        offers.len(),
        1,
        "projection failure must not duplicate durable offers"
    );
    assert_eq!(offers[0].status(), MakerOfferStatus::Active);
    drop(store);

    let degraded = run_maker_health(socket);
    assert_eq!(degraded["schema_version"], 1);
    assert_eq!(degraded["ready"], true, "SQLite owner RPC remains ready");
    assert_eq!(degraded["degraded"], true);
    assert_eq!(degraded["delivery"], "unavailable");
    assert_eq!(degraded["chat"], "available");

    fs::set_permissions(delivery, fs::Permissions::from_mode(0o700))
        .expect("restore owner-private Delivery projection");
    let replay = maker_publish_command(socket, offer_id, route.direction())
        .output()
        .expect("retry exact real maker request");
    assert!(
        replay.status.success(),
        "exact publish retry failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&replay.stdout),
        String::from_utf8_lossy(&replay.stderr)
    );
    let commit: Value = serde_json::from_slice(&replay.stdout).expect("maker retry returns JSON");
    assert_eq!(commit["was_replay"], true);

    let healthy = run_maker_health(socket);
    assert_eq!(healthy["ready"], true);
    assert_eq!(healthy["degraded"], false, "{healthy}");
    assert_eq!(healthy["delivery"], "available");
    assert_eq!(healthy["chat"], "available");
}

fn maker_publish_command(
    socket: &std::path::Path,
    offer_id: &MakerOfferId,
    direction: SwapDirection,
) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lez-maker-cli"));
    command
        .arg("--socket")
        .arg(socket)
        .arg("publish-offer")
        .arg("--request-id")
        .arg("m5-chat-publish-001")
        .arg("--offer-id")
        .arg(offer_id.as_str())
        .arg("--pair")
        .arg("zcash")
        .arg("--direction")
        .arg(match direction {
            SwapDirection::TakerSellsForeign => "taker-sells-foreign",
            SwapDirection::TakerSellsLez => "taker-sells-lez",
        });
    command
}

fn run_maker_health(socket: &std::path::Path) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_lez-maker-cli"))
        .arg("--socket")
        .arg(socket)
        .arg("health")
        .output()
        .expect("run real maker health");
    assert!(output.status.success(), "real maker health failed");
    serde_json::from_slice(&output.stdout).expect("maker health returns JSON")
}

/// A Taker Node's home: its own LEZ role, its own Zcash key and its registry.
/// Nothing in it was made by the Maker or before the take.
struct Taker {
    home: PathBuf,
    zcash_key: PathBuf,
    registry: PathBuf,
    btc_role: PathBuf,
    zec_role: PathBuf,
    delivery: PathBuf,
    maker_key: PublicKey,
}

impl Taker {
    fn new(root: &Path, key_byte: u8, chains: &Chains, port_base: u16, maker: &Maker) -> Self {
        let home = private_dir(&root.join("taker"));
        let zcash_key = home.join("zcash.key");
        write_private(&zcash_key, &[key_byte; 32]);
        let btc_role = btc_role_config(
            &home,
            &chains.sidecar,
            port_base,
            0x78,
            Some(&chains.lez_node),
        );
        let zec_role = zec_role_config(&home, &chains.zebra, Some(&zcash_key));
        let registry = home.join("registry.sqlite3");
        drop(SqliteTakerFacadeStore::create_new(&registry).unwrap());
        Self {
            home,
            zcash_key,
            registry,
            btc_role,
            zec_role,
            delivery: maker.delivery.clone(),
            maker_key: maker.key,
        }
    }

    /// The Node's startup configuration `<name>.json`, reaching the Maker's
    /// Chat by `chat_socket`.
    fn service_config(&self, name: &str, chat_socket: &Path) -> PathBuf {
        let path = self.home.join(format!("{name}.json"));
        write_private(
            &path,
            &serde_json::to_vec(&json!({
                "schema_version": 1,
                "delivery_sources": [{
                    "source_id": "m6-service-maker",
                    "directory": self.delivery,
                    "maker_public_key": hex::encode(self.maker_key.serialize()),
                }],
                "chat_socket": chat_socket,
                "maximum_offers": 16,
                "initiation": {
                    "execute_prepared": true,
                    "registry_database": self.registry,
                    "btc_role_config": self.btc_role,
                    "zec_role_config": self.zec_role,
                }
            }))
            .unwrap(),
        );
        path
    }
}

/// One `taker_swap_initiate_v1` by a Node started from `config` and stopped
/// after its answer, as a Node process restarted for every attempt: the
/// Node's runtime ends with the call, so its actor supervisor does not run
/// on into the test's later steps.
fn take_once(
    config: &Path,
    request: &TakerSwapInitiateRequestV1,
) -> Result<TakerInitiationCommitV1, String> {
    let config = config.to_path_buf();
    let request = request.clone();
    let node = thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let answer = runtime.block_on(async {
            let module =
                taker_service_rpc_module(load_taker_service_context(&config).unwrap()).unwrap();
            module
                .call("taker_swap_initiate_v1", [request])
                .await
                .map_err(|error| error.to_string())
        });
        runtime.shutdown_background();
        answer
    });
    tokio::task::block_in_place(|| node.join().expect("the Taker Node's thread"))
}

/// Where the Taker Node keeps one take-time swap (`zec_dynamic.rs`): all of
/// it in the swap's directory, named by the reservation, except the role's
/// own Zcash key.
struct TakerFiles {
    draft: PathBuf,
    key: PathBuf,
    agreement: PathBuf,
    source_actor_config: PathBuf,
    preimage: PathBuf,
    actor_root: PathBuf,
    receipt: PathBuf,
}

impl TakerFiles {
    fn of(taker: &Taker, reservation_id: &RequestId) -> Self {
        let swap = taker.home.join("swaps").join(reservation_id.as_str());
        Self {
            draft: swap.join("zec").join(ZEC_ROLE_DRAFT_FILE),
            key: taker.zcash_key.clone(),
            agreement: swap.join("zec-agreement.borsh"),
            source_actor_config: swap.join("zec/actor-config.json"),
            preimage: swap.join("zec/claim-preimage.key"),
            actor_root: swap.join("zec-actor"),
            receipt: swap.join("zec-receipt.json"),
        }
    }
}

struct PreReceiptArtifactSnapshot {
    config_path: PathBuf,
    config_inode: u64,
    config_bytes: Vec<u8>,
    agreement_path: PathBuf,
    agreement_inode: u64,
    agreement_bytes: Vec<u8>,
}

struct TakerArtifactSnapshot {
    config_path: PathBuf,
    config_inode: u64,
    config_bytes: Vec<u8>,
    agreement_path: PathBuf,
    agreement_inode: u64,
    agreement_bytes: Vec<u8>,
    receipt_inode: u64,
    receipt_bytes: Vec<u8>,
}

fn assert_fresh_taker_artifacts(
    take: &Take<'_>,
    direction: SwapDirection,
    swap_id: &SwapId,
    pre_receipt: &PreReceiptArtifactSnapshot,
) -> TakerArtifactSnapshot {
    let files = take.files;
    let config_path = files.actor_root.join("taker/actor-config.json");
    let agreement_path = files.actor_root.join("shared/agreement-v2.borsh");
    assert!(files.actor_root.join("taker").is_dir());
    assert!(!files.actor_root.join("maker").exists());
    let config = ActorConfig::load_private(&config_path).unwrap();
    assert_eq!(config.role(), ActorRole::Taker);
    assert_eq!(config.swap_id(), swap_id);
    assert_eq!(
        config.is_local_zcash_funder(),
        direction == SwapDirection::TakerSellsForeign
    );
    // The Taker reveals first in both directions, so it holds the preimage.
    assert!(config.owns_claim_preimage());
    // The Taker's actor activates at the time the Taker admitted the take,
    // not at the time a retry or its activation runs.
    let admitted_at = SqliteTakerFacadeStore::open_existing(&take.taker.registry)
        .unwrap()
        .lookup_initiation_admitted_at(&take.request.request_id)
        .unwrap()
        .expect("the take is admitted");
    assert_eq!(config.accepted_at(), Some(UnixSeconds::new(admitted_at)));
    let config_inode = fs::symlink_metadata(&config_path).unwrap().ino();
    let config_bytes = fs::read(&config_path).unwrap();
    let agreement_inode = fs::symlink_metadata(&agreement_path).unwrap().ino();
    let agreement_bytes = fs::read(&agreement_path).unwrap();
    assert_eq!(config_path, pre_receipt.config_path);
    assert_eq!(config_inode, pre_receipt.config_inode);
    assert_eq!(config_bytes, pre_receipt.config_bytes);
    assert_eq!(agreement_path, pre_receipt.agreement_path);
    assert_eq!(agreement_inode, pre_receipt.agreement_inode);
    assert_eq!(agreement_bytes, pre_receipt.agreement_bytes);
    assert_private_receipt(&files.receipt);
    let receipt_inode = fs::symlink_metadata(&files.receipt).unwrap().ino();
    let receipt_bytes = fs::read(&files.receipt).unwrap();
    let receipt: Value = serde_json::from_slice(&receipt_bytes).unwrap();
    assert_eq!(receipt.as_object().unwrap().len(), 7);
    assert_eq!(receipt["schema_version"], 1);
    assert_eq!(receipt["swap_id"], swap_id.as_str());
    assert_eq!(receipt["role"], "taker");
    assert_eq!(receipt["actor_config_file"], json!(config_path));
    assert_eq!(
        receipt["actor_config_sha256"],
        hex::encode(Sha256::digest(&config_bytes))
    );
    assert_eq!(
        receipt["agreement_sha256"],
        hex::encode(Sha256::digest(&agreement_bytes))
    );
    assert_eq!(
        receipt["actor_state_database"],
        json!(config.role_state_db())
    );
    TakerArtifactSnapshot {
        config_path,
        config_inode,
        config_bytes,
        agreement_path,
        agreement_inode,
        agreement_bytes,
        receipt_inode,
        receipt_bytes,
    }
}

fn assert_replayed_taker_artifacts(files: &TakerFiles, snapshot: &TakerArtifactSnapshot) {
    assert_eq!(
        fs::symlink_metadata(&snapshot.config_path).unwrap().ino(),
        snapshot.config_inode
    );
    assert_eq!(
        fs::read(&snapshot.config_path).unwrap(),
        snapshot.config_bytes
    );
    assert_eq!(
        fs::symlink_metadata(&snapshot.agreement_path)
            .unwrap()
            .ino(),
        snapshot.agreement_inode
    );
    assert_eq!(
        fs::read(&snapshot.agreement_path).unwrap(),
        snapshot.agreement_bytes
    );
    assert_eq!(
        fs::symlink_metadata(&files.receipt).unwrap().ino(),
        snapshot.receipt_inode
    );
    assert_eq!(fs::read(&files.receipt).unwrap(), snapshot.receipt_bytes);
}

const MAX_FAULT_PROXY_HTTP_BYTES: usize = 128 * 1024;

/// Runs the take through a proxy in front of the Maker's Chat socket that
/// forwards each expected round and withholds the answers marked lost. The
/// take fails; the Maker's answers, lost ones included, are returned.
fn take_losing_an_answer(
    take: &Take<'_>,
    chat_socket: &Path,
    name: &str,
    rounds: Vec<(&'static str, bool)>,
) -> Vec<Value> {
    let proxy_socket = chat_socket.with_file_name(format!("{name}.sock"));
    let listener = UnixListener::bind(&proxy_socket).expect("bind the answer-loss proxy");
    fs::set_permissions(&proxy_socket, fs::Permissions::from_mode(0o600)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let upstream = chat_socket.to_path_buf();
    let proxy = thread::spawn(move || proxy_chat_losing_answers(&listener, &upstream, &rounds));
    let config = take.taker.service_config(name, &proxy_socket);
    let lost = take_once(&config, take.request);
    let diagnostic = lost.expect_err("a lost answer must fail the take closed");
    assert!(!diagnostic.contains(take.files.key.to_str().unwrap()));
    assert!(!diagnostic.contains(&hex::encode(fs::read(&take.files.key).unwrap())));
    let answers = proxy.join().expect("the answer-loss proxy succeeds");
    fs::remove_file(&proxy_socket).expect("remove the answer-loss proxy socket");
    answers
}

fn assert_completion_response_loss(
    take: &Take<'_>,
    maker: &Maker,
    offer_id: &MakerOfferId,
    swap_id: &SwapId,
) -> PreReceiptArtifactSnapshot {
    let answers = take_losing_an_answer(
        take,
        &maker.chat_socket,
        "chat-drop-complete",
        vec![
            ("zec_chat_propose_v1", false),
            ("zec_chat_complete_v1", true),
        ],
    );
    // The proposal staged before the restart is answered again as a replay.
    assert_eq!(answers[0]["result"]["was_replay"], true);
    assert_eq!(answers[0]["result"]["offer_revision"], 2);
    assert_eq!(answers[1]["result"]["was_replay"], false);
    assert_eq!(answers[1]["result"]["swap_id"], swap_id.as_str());

    let files = take.files;
    assert!(files.agreement.is_file());
    assert!(files.actor_root.join("taker").is_dir());
    assert!(!files.actor_root.join("maker").exists());
    assert!(
        !files.receipt.exists(),
        "no receipt before observed completion"
    );
    let store = SqliteSwapStore::open(&maker.database).unwrap();
    let durable = store
        .load_zec_maker_negotiation(offer_id)
        .unwrap()
        .expect("Maker completion committed before response loss");
    assert_eq!(durable.status(), MakerZecNegotiationStatus::Completed);
    assert_eq!(store.list_maker_actor_processes().unwrap().len(), 1);
    pre_receipt_artifact_snapshot(files)
}

fn pre_receipt_artifact_snapshot(files: &TakerFiles) -> PreReceiptArtifactSnapshot {
    let config_path = files.actor_root.join("taker/actor-config.json");
    let agreement_path = files.actor_root.join("shared/agreement-v2.borsh");
    PreReceiptArtifactSnapshot {
        config_inode: fs::symlink_metadata(&config_path).unwrap().ino(),
        config_bytes: fs::read(&config_path).unwrap(),
        agreement_inode: fs::symlink_metadata(&agreement_path).unwrap().ino(),
        agreement_bytes: fs::read(&agreement_path).unwrap(),
        config_path,
        agreement_path,
    }
}

fn proxy_chat_losing_answers(
    listener: &UnixListener,
    upstream_path: &Path,
    rounds: &[(&str, bool)],
) -> Vec<Value> {
    let mut answers = Vec::new();
    for (expected_method, lose_answer) in rounds {
        let mut downstream = accept_proxy_connection(listener);
        let request = read_bounded_http_message(&mut downstream);
        let request_json: Value = serde_json::from_slice(http_body(&request)).unwrap();
        assert_eq!(request_json["method"], *expected_method);

        let mut upstream = UnixStream::connect(upstream_path).unwrap();
        configure_proxy_stream(&upstream);
        upstream.write_all(&request).unwrap();
        upstream.flush().unwrap();
        let response = read_bounded_http_message(&mut upstream);
        let response_json: Value = serde_json::from_slice(http_body(&response)).unwrap();
        assert!(
            response_json.get("error").is_none(),
            "fault proxy upstream error: {response_json}"
        );
        if !lose_answer {
            downstream.write_all(&response).unwrap();
            downstream.flush().unwrap();
        }
        answers.push(response_json);
    }
    answers
}

fn accept_proxy_connection(listener: &UnixListener) -> UnixStream {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                // macOS hands the accepted stream the listener's O_NONBLOCK:
                // a read before the peer wrote would fail as WouldBlock
                // instead of waiting out the read timeout.
                stream.set_nonblocking(false).unwrap();
                configure_proxy_stream(&stream);
                return stream;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "fault proxy accept timed out");
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("fault proxy accept failed: {error}"),
        }
    }
}

fn configure_proxy_stream(stream: &UnixStream) {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
}

fn read_bounded_http_message(stream: &mut UnixStream) -> Vec<u8> {
    let mut message = Vec::new();
    let mut expected_total = None;
    loop {
        if let Some(total) = expected_total
            && message.len() >= total
        {
            message.truncate(total);
            return message;
        }
        let mut chunk = [0_u8; 4096];
        let read = stream.read(&mut chunk).expect("read proxied HTTP message");
        assert!(read > 0, "proxied HTTP message closed before completion");
        message.extend_from_slice(&chunk[..read]);
        assert!(message.len() <= MAX_FAULT_PROXY_HTTP_BYTES);
        if expected_total.is_none()
            && let Some(header_end) = message.windows(4).position(|bytes| bytes == b"\r\n\r\n")
        {
            let body_start = header_end + 4;
            let content_length = parse_content_length(&message[..header_end]);
            let total = body_start.checked_add(content_length).unwrap();
            assert!(total <= MAX_FAULT_PROXY_HTTP_BYTES);
            expected_total = Some(total);
        }
    }
}

fn parse_content_length(header: &[u8]) -> usize {
    let header = std::str::from_utf8(header).expect("HTTP header is ASCII");
    let mut length = None;
    for line in header.lines().skip(1) {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            assert!(length.is_none(), "duplicate Content-Length");
            length = Some(value.trim().parse::<usize>().unwrap());
        }
        assert!(!name.eq_ignore_ascii_case("transfer-encoding"));
    }
    length.expect("proxied HTTP message has Content-Length")
}

fn http_body(message: &[u8]) -> &[u8] {
    let header_end = message
        .windows(4)
        .position(|bytes| bytes == b"\r\n\r\n")
        .expect("complete HTTP header");
    &message[(header_end + 4)..]
}

async fn accept_and_replay(
    take: &Take<'_>,
    maker: &Maker,
    direction: SwapDirection,
    swap_id: &SwapId,
    authenticated: &AuthenticatedOfferRefV1,
    pre_receipt: &PreReceiptArtifactSnapshot,
) -> Vec<u8> {
    let database = &maker.database;
    let expires_at_unix_seconds = authenticated.offer().expires_at_unix_seconds();
    // The take was admitted when its proposal was staged: this attempt
    // resumes it from the persisted agreement, and the Maker answers the
    // completion it committed before the lost answer as a replay.
    let accepted = take.run().expect("the repeated take completes");
    assert!(accepted.was_replay);
    assert_eq!(&accepted.swap.swap_id, swap_id);
    assert_eq!(accepted.swap.state, TakerSwapStateV1::NotActivated);
    let taker_artifacts = assert_fresh_taker_artifacts(take, direction, swap_id, pre_receipt);
    let first = SqliteSwapStore::open(database)
        .unwrap()
        .list_maker_actor_processes()
        .unwrap();
    assert_eq!(first.len(), 1, "acceptance must expose one scheduled actor");
    let first = &first[0];
    assert_eq!(first.manifest().kind(), MakerActorKindV1::Zcash);
    assert_eq!(first.schedule_state(), MakerActorScheduleState::Queued);
    let config_path = first.manifest().config_path();
    let config_inode = fs::symlink_metadata(config_path).unwrap().ino();
    let config_bytes = fs::read(config_path).unwrap();
    let config = ActorConfig::load_private(config_path).unwrap();
    assert_eq!(config.role(), ActorRole::Maker);
    assert_eq!(config.swap_id(), swap_id);
    assert_eq!(
        config.role_state_db(),
        first.manifest().state_database_path()
    );
    assert!(
        !config_path
            .ancestors()
            .nth(2)
            .unwrap()
            .join("taker")
            .exists()
    );
    let final_wire = fs::read(&take.files.agreement).expect("read taker-persisted final agreement");
    let expiry_wait = Instant::now();
    while now() <= expires_at_unix_seconds {
        assert!(
            expiry_wait.elapsed() < Duration::from_secs(EXPIRING_OFFER_TTL_SECONDS + 10),
            "the local offer did not expire on schedule"
        );
        thread::sleep(Duration::from_millis(100));
    }
    assert!(now() > expires_at_unix_seconds);
    // After the offer expired and with Delivery unreadable, the Taker answers
    // the same take from its receipt, and the Maker the same completion from
    // its record.
    fs::set_permissions(&maker.delivery, fs::Permissions::from_mode(0o000)).unwrap();
    let replay = take.run();
    fs::set_permissions(&maker.delivery, fs::Permissions::from_mode(0o700)).unwrap();
    let replay = replay.expect("the take replays after the offer expired, without Delivery");
    assert!(replay.was_replay);
    assert_eq!(replay.swap, accepted.swap);
    assert_replayed_taker_artifacts(take.files, &taker_artifacts);
    let reservation_id = derived_request_id(&take.request.request_id, b"node-reservation");
    let completion: ZecChatCompleteResponseV1 = call_local_rpc(
        &maker.chat_socket,
        "zec_chat_complete_v1",
        &ZecChatCompleteRequestV1 {
            schema_version: 1,
            request_id: derived_request_id(&reservation_id, b"complete"),
            offer_id: take.request.offer_id.clone(),
            expected_offer_revision: 2,
            reservation_id,
            final_agreement_wire: final_wire.clone(),
        },
    )
    .await
    .expect("the Maker replays the completion after the transcript expired");
    assert!(completion.was_replay);
    assert_eq!(completion.offer_revision, 3);
    assert_eq!(completion.swap_id.as_ref(), swap_id.as_str());
    let replayed = SqliteSwapStore::open(database)
        .unwrap()
        .list_maker_actor_processes()
        .unwrap();
    assert_eq!(
        replayed.len(),
        1,
        "exact replay must not duplicate the actor"
    );
    assert_eq!(replayed[0].manifest(), first.manifest());
    assert_eq!(
        fs::symlink_metadata(replayed[0].manifest().config_path())
            .unwrap()
            .ino(),
        config_inode,
        "exact replay must not replace the provisioned config inode"
    );
    assert_eq!(
        fs::read(replayed[0].manifest().config_path()).unwrap(),
        config_bytes
    );
    final_wire
}

fn assert_private_receipt(path: &Path) {
    let metadata = fs::symlink_metadata(path).expect("acceptance receipt exists");
    assert!(metadata.file_type().is_file());
    assert_eq!(metadata.permissions().mode() & 0o7777, 0o600);
    assert_eq!(metadata.nlink(), 1);
}

fn assert_receipt_monitor_is_offline(receipt: &Path) {
    let output = Command::new(cross_role::workspace_binary("lez-taker-cli"))
        .arg("monitor")
        .arg("--receipt")
        .arg(receipt)
        .output()
        .expect("run receipt-bound Taker monitor");
    assert!(
        output.status.success(),
        "receipt monitor failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        json!({"schema_version": 1, "role": "taker", "state": "not_activated"})
    );
}

#[allow(clippy::too_many_arguments)]
fn assert_completed_durable(
    database: &std::path::Path,
    offer_id: &MakerOfferId,
    reservation_id: &RequestId,
    swap_id: &SwapId,
    authenticated: &AuthenticatedOfferRefV1,
    final_wire: &[u8],
    preimage: &[u8],
) {
    let store = SqliteSwapStore::open(database).unwrap();
    let durable = store
        .load_zec_maker_negotiation(offer_id)
        .unwrap()
        .expect("completion remains durable after process termination");
    assert_eq!(durable.status(), MakerZecNegotiationStatus::Completed);
    assert_eq!(durable.reservation_id(), reservation_id);
    assert!(!durable.maker_proposal_wire().is_empty());
    assert_eq!(durable.offer_commitment(), &authenticated.commitment());
    assert_eq!(durable.final_agreement_wire(), Some(final_wire));
    assert_eq!(durable.swap_id(), Some(swap_id.as_str()));
    let actor = store.list_maker_actor_processes().unwrap();
    assert_eq!(actor.len(), 1);
    let maker_config = ActorConfig::load_private(actor[0].manifest().config_path()).unwrap();
    assert_eq!(maker_config.role(), ActorRole::Maker);
    assert!(!maker_config.is_local_zcash_funder());
    // The Maker's actor activates at the time the Maker accepted, read from
    // its own clock while the offer was open.
    let maker_accepted_at = maker_config
        .accepted_at()
        .expect("the Maker's actor config records its acceptance")
        .value();
    assert!(
        authenticated.offer().created_at_unix_seconds() <= maker_accepted_at
            && maker_accepted_at < authenticated.offer().expires_at_unix_seconds()
            && maker_accepted_at <= now(),
        "the Maker's recorded acceptance {maker_accepted_at} is outside the offer's life"
    );
    let connection = Connection::open(database).unwrap();
    let maker_claim_rows: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM zec_sdk_claim_materials WHERE local_role = 'maker'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(maker_claim_rows, 0);
    // The swap secret is the Taker's alone until its claim reveals it.
    assert_eq!(preimage.len(), 32);
    assert!(
        !fs::read(database)
            .unwrap()
            .windows(preimage.len())
            .any(|window| window == preimage),
        "maker claim preimage must not occur in plaintext SQLite bytes"
    );
}

/// The owner socket serves no Chat round and the Chat socket no owner
/// method, whatever the request holds.
async fn assert_socket_method_isolation(
    maker: &Maker,
    authenticated: &AuthenticatedOfferRefV1,
    offer_id: &MakerOfferId,
    reservation_id: &RequestId,
) {
    let reserve = ZecReserveRequestV1 {
        schema_version: 1,
        reservation_id: reservation_id.clone(),
        offer_id: offer_id.clone(),
        signed_offer_envelope: authenticated.signed_envelope().to_vec(),
        foreign_units: FOREIGN_UNITS,
        taker_zcash_public_key: public_key(&key(FUNDER_KEY)).serialize().to_vec(),
        taker_lez_account: [0x55; 32],
        taker_funding: None,
        taker_secret_digest: Some([0x09; 32]),
    };
    let proposal = ZecChatProposeRequestV1 {
        schema_version: 1,
        request_id: derived_request_id(reservation_id, b"propose"),
        offer_id: offer_id.clone(),
        expected_offer_revision: 1,
        reservation_id: reservation_id.clone(),
        foreign_units: FOREIGN_UNITS,
        signed_offer_envelope: authenticated.signed_envelope().to_vec(),
        unsigned_draft_wire: vec![0],
    };
    assert!(
        call_local_rpc::<_, Value>(&maker.socket, "zec_reserve_v1", &reserve)
            .await
            .is_err()
            && call_local_rpc::<_, Value>(&maker.socket, "zec_chat_propose_v1", &proposal)
                .await
                .is_err(),
        "owner socket must not expose taker Chat methods"
    );
    assert!(
        call_local_rpc::<_, Value>(
            &maker.chat_socket,
            "maker_offer_list",
            &ListRequest::default()
        )
        .await
        .is_err(),
        "Chat socket must not expose owner-control methods"
    );
    assert!(
        !maker.swaps.join(reservation_id.as_str()).exists(),
        "a round refused on the owner socket reserves nothing"
    );
}

async fn configure_live_route(socket: &std::path::Path, route: MakerRouteV1, ttl_seconds: u64) {
    let disabled = MakerPairConfigurationV1::new(
        route,
        false,
        MakerPriceSourceKind::Local,
        10,
        FOREIGN_UNITS,
        ttl_seconds,
    )
    .unwrap();
    let _: Value = call_local_rpc(
        socket,
        "maker_pair_configure",
        &PairConfigureRequest {
            request_id: request("m5-chat-route-create-001"),
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
            request_id: request("m5-chat-price-create-001"),
            expected_revision: None,
            price: LocalPriceV1::new(route, 5, 2).unwrap(),
        },
    )
    .await
    .unwrap();
    let enabled = MakerPairConfigurationV1::new(
        route,
        true,
        MakerPriceSourceKind::Local,
        10,
        FOREIGN_UNITS,
        ttl_seconds,
    )
    .unwrap();
    let _: Value = call_local_rpc(
        socket,
        "maker_pair_configure",
        &PairConfigureRequest {
            request_id: request("m5-chat-route-enable-001"),
            expected_revision: Some(1),
            configuration: enabled,
        },
    )
    .await
    .unwrap();
}

/// The sidecar ports of one test's two roles; each test of this file has its
/// own `offset`.
fn sidecar_port_base(offset: u16) -> u16 {
    42_000 + u16::try_from(std::process::id() % 400).unwrap() * 50 + offset
}

/// The fakes both roles read in place of their chains.
struct Chains {
    zebra: String,
    lez_node: String,
    sidecar: PathBuf,
}

/// A Maker daemon's files: its Delivery key is its Zcash key, and its Zcash
/// authority is the take-time one.
struct Maker {
    socket: PathBuf,
    chat_socket: PathBuf,
    ready: PathBuf,
    database: PathBuf,
    delivery: PathBuf,
    key_file: PathBuf,
    swaps: PathBuf,
    key: PublicKey,
    authority: TakeTimeMaker,
}

impl Maker {
    fn new(root: &Path, key_byte: u8, chains: &Chains, port_base: u16) -> Self {
        let runtime = private_dir(&root.join("runtime"));
        let key_file = root.join("delivery-signing.key");
        write_private(&key_file, &[key_byte; 32]);
        let home = root.join("maker");
        let authority = TakeTimeMaker::new(
            &home,
            &chains.zebra,
            &chains.sidecar,
            port_base,
            Some(&chains.lez_node),
        );
        Self {
            socket: runtime.join("maker.sock"),
            chat_socket: runtime.join("chat.sock"),
            ready: runtime.join("ready"),
            database: root.join("maker.sqlite3"),
            delivery: root.join("delivery"),
            key_file,
            swaps: home.join("swaps"),
            key: public_key(&key(key_byte)),
            authority,
        }
    }
}

fn daemon_command(maker: &Maker) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lez-maker-node"));
    command
        .arg("--socket")
        .arg(&maker.socket)
        .arg("--chat-socket")
        .arg(&maker.chat_socket)
        .arg("--database")
        .arg(&maker.database)
        .arg("--ready-file")
        .arg(&maker.ready)
        .arg("--delivery-directory")
        .arg(&maker.delivery)
        .arg("--delivery-signing-key-file")
        .arg(&maker.key_file)
        .args(maker.authority.arguments());
    command
}

/// The Maker daemon, stopped when the test ends however it ends.
struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn start_daemon(maker: &Maker) -> Daemon {
    Daemon(
        daemon_command(maker)
            .spawn()
            .expect("start isolated maker daemon"),
    )
}

fn wait_ready(daemon: &mut Daemon, ready: &std::path::Path, socket: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(published) = fs::read_to_string(ready) {
            assert_eq!(published.trim(), socket.to_str().unwrap());
            return;
        }
        assert!(
            daemon.0.try_wait().unwrap().is_none(),
            "daemon exited early"
        );
        assert!(Instant::now() < deadline, "daemon readiness timed out");
        thread::sleep(Duration::from_millis(20));
    }
}

fn stop_daemon_gracefully(daemon: &mut Daemon, maker: &Maker) {
    kill_process(Pid::from_child(&daemon.0), Signal::INT).expect("signal maker daemon");
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = daemon.0.try_wait().expect("poll maker daemon") {
            break status;
        }
        if Instant::now() >= deadline {
            daemon.0.kill().expect("kill wedged maker daemon");
            daemon.0.wait().expect("reap wedged maker daemon");
            panic!("maker daemon did not stop within the graceful deadline");
        }
        thread::sleep(Duration::from_millis(20));
    };
    assert!(
        status.success(),
        "maker daemon exited unsuccessfully: {status}"
    );
    assert!(
        !maker.socket.exists(),
        "graceful shutdown must remove the owner socket"
    );
    assert!(
        !maker.chat_socket.exists(),
        "graceful shutdown must remove the Chat socket"
    );
    assert!(
        !maker.ready.exists(),
        "graceful shutdown must remove the readiness handoff"
    );
}

fn overwrite_private_in_place(path: &Path, bytes: &[u8]) {
    let mut file = OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

fn request(value: &str) -> RequestId {
    RequestId::new(value).unwrap()
}

fn key(byte: u8) -> SecretKey {
    SecretKey::from_slice(&[byte; 32]).unwrap()
}

fn public_key(secret: &SecretKey) -> PublicKey {
    PublicKey::from_secret_key(&Secp256k1::signing_only(), secret)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
