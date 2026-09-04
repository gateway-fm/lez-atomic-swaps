//! Exercises the ZEC reference actor; compiled only with `pair-zec`.
#![cfg(feature = "pair-zec")]

//! Separate-process happy-path proof for the run-local ZEC Chat boundary.

#[path = "support/cross_role_binary.rs"]
mod cross_role;
mod support;

use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::{Read as _, Write as _},
    os::unix::fs::{
        DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _,
    },
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
    PairConfigureRequest, RunLocalDelivery, TakerMakerIdentityV1, ZecChatProposalV1,
    ZecChatProposeRequestV1, call_local_rpc,
};
use lez_swap_core::{Pair, Participant, Phase, SwapDirection, SwapId, UnixSeconds};
use lez_swap_sdk_core::OfferDiscovery as _;
use lez_swap_store::{
    ActorHeldLock, LocalPriceV1, MakerActorKindV1, MakerActorScheduleState, MakerOfferId,
    MakerOfferStatus, MakerPairConfigurationV1, MakerPriceSourceKind, MakerRouteV1,
    MakerZecNegotiationStatus, SqliteSwapStore, SqliteTakerFacadeStore, SqliteZecRecoveryStore,
    TakerFacadeActionV1, maker_zec_chat_session_id,
};
use lez_taker_node::{
    TakerClaimRequestV1, TakerHealthRequestV1, TakerHealthV1, TakerInitiationCommitV1,
    TakerRefundRequestV1, TakerSwapInitiateRequestV1, TakerSwapListRequestV1, TakerSwapListV1,
    TakerSwapMonitorRequestV1, TakerSwapStateV1, TakerSwapViewV1, load_taker_service_context,
    taker_service_rpc_module,
};
use lez_zec_swap_sdk::{
    AcceptedZecAgreementV1, Bip199Contract, ExpectedBip199Output, LezAssetV1, LezChainIdentityV1,
    LezEnvironmentV1, NegotiationTranscriptV1, ProtectedClaimKey, ZcashTransparentDestinationV1,
    ZecAgreementBodyV1, ZecAgreementDraftV1, ZecLezTermsV1, ZecLifecycleAction, ZecPairSdk,
    ZecParticipantIdentityV1, ZecParticipantsV1, ZecProfileId, ZecProfileRecordV1, ZecRefundPlanV1,
    ZecSwapBinding, ZecSwapBindingRecordV1, ZecTransactionPolicyV1, derive_lez_metadata_account_v1,
    derive_lez_native_custody_account_v1, derive_lez_swap_id_v1,
};
use rusqlite::{Connection, params};
use rustix::process::{Pid, Signal, kill_process};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use support::{actor_deployment, actor_deployment_with_direction};
use tempfile::tempdir;
use zcash_protocol::{
    consensus::{BranchId, NetworkType},
    value::Zatoshis,
};
use zcash_transparent::address::TransparentAddress;

use zec_reference_actor::{ActorConfig, ActorRole};
const CLAIM_PREIMAGE: [u8; 32] = [0x44; 32];
const TAKER_DEPENDENCY_UNAVAILABLE: i64 = -32_010;
const TAKER_SWAP_NOT_FOUND: i64 = -32_014;
const TAKER_ACTION_UNAVAILABLE: i64 = -32_016;
const TAKER_ACTION_CONFLICT: i64 = -32_017;

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn separate_taker_countersigns_and_maker_atomically_accepts_before_response() {
    let run = tempdir().expect("isolated Chat process root");
    let runtime = run.path().join("runtime");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&runtime)
        .expect("create owner-only runtime");
    let socket = runtime.join("maker.sock");
    let chat_socket = runtime.join("chat.sock");
    let ready = runtime.join("ready");
    let database = run.path().join("maker.sqlite3");
    let delivery = run.path().join("delivery");
    let key_file = run.path().join("delivery-signing.key");
    let claim_key_file = run.path().join("maker-claim-recovery.key");
    let claim_preimage_file = run.path().join("maker-claim-preimage.key");
    write_raw_key(&key_file, 8);
    write_raw_key(&claim_key_file, 0x7a);
    write_raw_key(&claim_preimage_file, CLAIM_PREIMAGE[0]);
    let actor = actor_deployment_with_direction(
        run.path(),
        "m5-chat-swap-001",
        SwapDirection::TakerSellsForeign,
    );
    let daemon_paths = DaemonPaths {
        socket: &socket,
        chat_socket: &chat_socket,
        ready: &ready,
        database: &database,
        delivery: &delivery,
        key_file: &key_file,
        claim_key_file: &claim_key_file,
        claim_preimage_file: None,
        actor_root: &actor.root,
        actor_source_config: &actor.source_config,
        actor_program: &actor.program,
        actor_program_sha256: &actor.program_sha256,
    };
    assert_duplicate_actor_authority_is_rejected(&daemon_paths);
    let mut daemon = start_daemon(&daemon_paths);
    wait_ready(&mut daemon, &ready, &socket);
    let (route, offer_id) = prepare_live_offer_for_direction(
        &socket,
        &database,
        &delivery,
        SwapDirection::TakerSellsForeign,
    )
    .await;

    let maker_secret = key(8);
    let maker_key = public_key(&maker_secret);
    let subscriber = RunLocalDelivery::subscriber(&delivery, maker_key).unwrap();
    let authenticated = subscriber
        .discover(&DeliveryOfferQueryV1::for_route(route, now()))
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("published offer is discoverable");
    let reservation_id = request("m5-chat-reservation-001");
    let draft_wire = unsigned_draft(
        &authenticated,
        &reservation_id,
        &maker_secret,
        &key(2),
        actor.agreement_basis_time,
    );
    let taker_files = prepare_taker_files(
        run.path(),
        &draft_wire,
        &actor.source_config,
        route.direction(),
    );
    let proposal_request = ZecChatProposeRequestV1 {
        schema_version: 1,
        request_id: derived_chat_request(&reservation_id, b"propose"),
        offer_id: offer_id.clone(),
        expected_offer_revision: 1,
        reservation_id: reservation_id.clone(),
        foreign_units: 10_000,
        signed_offer_envelope: authenticated.signed_envelope().to_vec(),
        unsigned_draft_wire: draft_wire.clone(),
    };

    assert_socket_method_isolation(&socket, &chat_socket, &proposal_request).await;
    let staged = stage_proposal(&chat_socket, &proposal_request).await;
    assert_eq!(staged.offer_revision, 2);
    let accepted_at = now();
    let taker = TakerProcess {
        direction: route.direction(),
        delivery: &delivery,
        chat_socket: &chat_socket,
        offer_id: &offer_id,
        reservation_id: &reservation_id,
        draft_file: &taker_files.draft,
        taker_key_file: &taker_files.key,
        agreement_file: &taker_files.agreement,
        source_actor_config: &taker_files.source_actor_config,
        actor_root: &taker_files.actor_root,
        receipt: &taker_files.receipt,
    };

    assert_chat_outage_and_restart(&taker, &maker_key, accepted_at, &mut daemon, &daemon_paths);
    let pre_receipt = assert_completion_response_loss(&taker, &maker_key, accepted_at, &database);
    let final_wire = accept_and_replay(
        &taker,
        &maker_key,
        accepted_at,
        &authenticated,
        &database,
        &pre_receipt,
    );
    assert_post_acceptance_boundary(
        &mut daemon,
        &daemon_paths,
        &offer_id,
        &reservation_id,
        &authenticated,
        &final_wire,
        &taker_files.receipt,
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn service_initiation_completes_real_chat_before_not_activated_response() {
    let run = tempdir().expect("isolated service Chat root");
    let runtime = run.path().join("runtime");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&runtime)
        .expect("create owner-only runtime");
    let socket = runtime.join("maker.sock");
    let chat_socket = runtime.join("chat.sock");
    let ready = runtime.join("ready");
    let database = run.path().join("maker.sqlite3");
    let delivery = run.path().join("delivery");
    let key_file = run.path().join("delivery-signing.key");
    let claim_key_file = run.path().join("maker-claim-recovery.key");
    let claim_preimage_file = run.path().join("maker-claim-preimage.key");
    write_raw_key(&key_file, 8);
    write_raw_key(&claim_key_file, 0x7a);
    write_raw_key(&claim_preimage_file, CLAIM_PREIMAGE[0]);
    let actor = actor_deployment(run.path(), "m5-chat-swap-001");
    let daemon_paths = DaemonPaths {
        socket: &socket,
        chat_socket: &chat_socket,
        ready: &ready,
        database: &database,
        delivery: &delivery,
        key_file: &key_file,
        claim_key_file: &claim_key_file,
        claim_preimage_file: Some(&claim_preimage_file),
        actor_root: &actor.root,
        actor_source_config: &actor.source_config,
        actor_program: &actor.program,
        actor_program_sha256: &actor.program_sha256,
    };
    let mut daemon = start_daemon(&daemon_paths);
    wait_ready(&mut daemon, &ready, &socket);
    let (route, offer_id) = prepare_live_offer(&socket, &database, &delivery).await;

    let maker_secret = key(8);
    let maker_key = public_key(&maker_secret);
    let subscriber = RunLocalDelivery::subscriber(&delivery, maker_key).unwrap();
    let authenticated = subscriber
        .discover(&DeliveryOfferQueryV1::for_route(route, now()))
        .await
        .unwrap()
        .into_iter()
        .next()
        .expect("published offer is discoverable");
    let reservation_id = request("m6-service-chat-reservation-001");
    let draft_wire = unsigned_draft(
        &authenticated,
        &reservation_id,
        &maker_secret,
        &key(2),
        actor.agreement_basis_time,
    );
    let taker_files = prepare_taker_files(
        run.path(),
        &draft_wire,
        &actor.source_config,
        route.direction(),
    );
    let signed_envelope = taker_files
        .draft
        .with_file_name("prepared-signed-offer.json");
    write_private(&signed_envelope, authenticated.signed_envelope());

    let registry = taker_files.draft.with_file_name("taker-service.sqlite3");
    drop(SqliteTakerFacadeStore::create_new(&registry).unwrap());
    let service_config = taker_files.draft.with_file_name("service.json");
    let service_value = json!({
        "schema_version": 1,
        "delivery_sources": [{
            "source_id": "m6-service-maker",
            "directory": delivery,
            "maker_public_key": hex::encode(maker_key.serialize()),
        }],
        "chat_socket": chat_socket,
        "maximum_offers": 16,
        "initiation": {
            "execute_prepared_zec": true,
            "registry_database": registry,
            "prepared_zec": [{
                "source_id": "m6-service-maker",
                "swap_id": "m5-chat-swap-001",
                "offer_id": offer_id,
                "reservation_id": reservation_id,
                "foreign_units": 10_000,
                "lez_units": 25_000,
                "signed_envelope": service_digest_binding(&signed_envelope),
                "unsigned_draft": service_digest_binding(&taker_files.draft),
                "signing_key": {"path": taker_files.key},
                "source_config": service_digest_binding(&taker_files.source_actor_config),
                "agreement_output": taker_files.agreement,
                "actor_root": taker_files.actor_root,
                "receipt_output": taker_files.receipt,
            }]
        }
    });
    write_private(
        &service_config,
        &serde_json::to_vec(&service_value).unwrap(),
    );
    let request = TakerSwapInitiateRequestV1 {
        schema_version: 1,
        request_id: RequestId::new("m6-service-chat-initiation-001").unwrap(),
        offer_id: offer_id.clone(),
        route,
        maker_identity: TakerMakerIdentityV1::new(maker_key.serialize()).unwrap(),
        signed_envelope_sha256: authenticated.commitment(),
        foreign_units: 10_000,
        expected_lez_units: 25_000,
        logos_offer_announcement_base64: None,
    };

    let module =
        taker_service_rpc_module(load_taker_service_context(&service_config).unwrap()).unwrap();
    let first: TakerInitiationCommitV1 = module
        .call("taker_swap_initiate_v1", [request.clone()])
        .await
        .unwrap();
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
            swap_id: SwapId::new("m5-chat-swap-001").unwrap(),
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
            swap_id: SwapId::new("m5-chat-swap-001").unwrap(),
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
            swap_id: SwapId::new("m5-chat-swap-001").unwrap(),
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
            swap_id: SwapId::new("m5-chat-swap-001").unwrap(),
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
            swap_id: SwapId::new("m5-chat-swap-001").unwrap(),
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
            swap_id: SwapId::new("m5-chat-swap-001").unwrap(),
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
            swap_id: SwapId::new("m5-chat-swap-001").unwrap(),
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
            swap_id: SwapId::new("m5-chat-swap-001").unwrap(),
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
        &canonical_artifacts.agreement_bytes,
        actor.agreement_basis_time,
    )
    .await;
    let active_logical_before = active_taker_logical_state(monitor_config.role_state_db());
    let active_monitor_response = service_rpc_response(
        &replay_module,
        "taker_swap_monitor_v1",
        json!([TakerSwapMonitorRequestV1 {
            schema_version: 1,
            swap_id: SwapId::new("m5-chat-swap-001").unwrap(),
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
            swap_id: SwapId::new("m5-chat-swap-001").unwrap(),
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
            swap_id: SwapId::new("m5-chat-swap-001").unwrap(),
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
            swap_id: SwapId::new("m5-chat-swap-001").unwrap(),
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
            swap_id: SwapId::new("m5-chat-swap-001").unwrap(),
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
            swap_id: SwapId::new("m5-chat-swap-001").unwrap(),
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
            swap_id: SwapId::new("m5-chat-swap-001").unwrap(),
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
            actor.agreement_basis_time,
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
            swap_id: SwapId::new("m5-chat-swap-001").unwrap(),
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
            swap_id: SwapId::new("m5-chat-swap-001").unwrap(),
        }]),
    )
    .await;

    let active_logical_after = active_taker_logical_state(monitor_config.role_state_db());
    let active_bridge_absent = !monitor_config.bridge_journal_db().exists();
    let post_read_artifacts = ServiceAcceptanceArtifacts::capture(&taker_files);
    drop(replay_module);
    fs::rename(&offline_chat, &chat_socket).unwrap();
    stop_daemon_gracefully(&mut daemon, &daemon_paths);

    assert!(!first.was_replay);
    assert_eq!(first.swap.state, TakerSwapStateV1::NotActivated);
    assert_eq!(first.swap.progress_generation, 0);
    assert_eq!(first.swap.available_action, None);
    assert!(replay.was_replay);
    assert_eq!(replay.swap, first.swap);

    assert_eq!(registered.len(), 7, "registered methods: {registered:?}");
    for method in [
        "taker_health",
        "taker_offer_list_v1",
        "taker_swap_list_v1",
        "taker_swap_initiate_v1",
        "taker_swap_monitor_v1",
        "taker_swap_claim_v1",
        "taker_swap_refund_v1",
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
    for response in [
        &replaced_receipt_response,
        &locked_actor_response,
        &crossed_pair_monitor_response,
        &crossed_pair_list_response,
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
    let mut expected_in_progress = expected_active.clone();
    expected_in_progress.state = TakerSwapStateV1::ClaimInProgress;
    assert_eq!(in_progress, expected_in_progress);
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
        run.path(),
        &reservation_id,
        &taker_files,
    );
    assert_service_responses_redacted(
        [
            &replaced_receipt_response,
            &locked_actor_response,
            &recovered_monitor_response,
        ],
        run.path(),
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
        run.path(),
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
    assert_eq!(taker_config.swap_id().as_str(), "m5-chat-swap-001");
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
    agreement_wire: &[u8],
    accepted_at_unix_seconds: u64,
) {
    let accepted = AcceptedZecAgreementV1::accept_wire_at(
        agreement_wire,
        UnixSeconds::new(accepted_at_unix_seconds),
        Participant::Taker,
        0,
    )
    .expect("service agreement is valid at its deterministic fixture time");
    let claim_key =
        ProtectedClaimKey::new("m5-chat-taker-claim-v1", [0x7b; 32]).expect("fixture claim key");
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
    connection
        .query_row(
            "SELECT payload_version, agreement_wire, accepted_at,
                    accepted_revision, active_revision,
                    (SELECT COUNT(*) FROM zec_sdk_agreements
                     WHERE local_role = 'taker')
             FROM zec_sdk_agreements
             WHERE local_role = 'taker' AND swap_id = 'm5-chat-swap-001'",
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
        .expect("one exact active Taker agreement")
}

fn service_digest_binding(path: &Path) -> Value {
    json!({
        "path": path,
        "sha256": hex::encode(Sha256::digest(fs::read(path).unwrap())),
    })
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

async fn prepare_live_offer(
    socket: &Path,
    database: &Path,
    delivery: &Path,
) -> (MakerRouteV1, MakerOfferId) {
    prepare_live_offer_for_direction(socket, database, delivery, SwapDirection::TakerSellsLez).await
}

async fn prepare_live_offer_for_direction(
    socket: &Path,
    database: &Path,
    delivery: &Path,
    direction: SwapDirection,
) -> (MakerRouteV1, MakerOfferId) {
    let route = MakerRouteV1::new(Pair::Zcash, direction).unwrap();
    configure_live_route(socket, route).await;
    let offer_id = MakerOfferId::new("m5-chat-offer-001").unwrap();
    assert_delivery_outage_is_visible_and_exact_retry_recovers(
        socket, database, delivery, &offer_id,
    );
    (route, offer_id)
}

async fn stage_proposal(
    chat_socket: &Path,
    request: &ZecChatProposeRequestV1,
) -> ZecChatProposalV1 {
    call_local_rpc(chat_socket, "zec_chat_propose_v1", request)
        .await
        .expect("stage proposal before Chat outage")
}

fn assert_post_acceptance_boundary(
    daemon: &mut Child,
    daemon_paths: &DaemonPaths<'_>,
    offer_id: &MakerOfferId,
    reservation_id: &RequestId,
    authenticated: &AuthenticatedOfferRefV1,
    final_wire: &[u8],
    receipt: &Path,
) {
    assert_completed_process(
        daemon,
        daemon_paths,
        offer_id,
        reservation_id,
        authenticated,
        final_wire,
    );
    fs::rename(
        daemon_paths.delivery,
        daemon_paths.delivery.with_file_name("delivery.offline"),
    )
    .expect("remove Delivery from the post-lock Taker boundary");
    assert_receipt_monitor_is_offline(receipt);
}

fn assert_completed_process(
    daemon: &mut Child,
    paths: &DaemonPaths<'_>,
    offer_id: &MakerOfferId,
    reservation_id: &RequestId,
    authenticated: &AuthenticatedOfferRefV1,
    final_wire: &[u8],
) {
    let database = paths.database;
    let socket = paths.socket;
    let completed_health = run_maker_health(socket);
    assert_eq!(completed_health["ready"], true);
    // Health reconciliation removes the short-TTL consumed envelope after
    // expiry, so the durable offer set and Delivery projection agree again.
    assert_eq!(
        completed_health["degraded"], false,
        "unexpected completed health: {completed_health}"
    );
    assert_eq!(completed_health["delivery"], "available");
    assert_eq!(completed_health["chat"], "available");
    assert_eq!(completed_health["routes"][0]["state"], "disabled");
    assert_eq!(fs::read_dir(paths.delivery).unwrap().count(), 0);
    stop_daemon_gracefully(daemon, paths);
    assert_completed_durable(
        database,
        offer_id,
        reservation_id,
        authenticated,
        final_wire,
    );
}

fn assert_chat_outage_and_restart(
    taker: &TakerProcess<'_>,
    maker_key: &PublicKey,
    accepted_at: u64,
    daemon: &mut Child,
    paths: &DaemonPaths<'_>,
) {
    let offline_chat = paths.chat_socket.with_file_name("chat.offline");
    fs::rename(paths.chat_socket, &offline_chat)
        .expect("make Chat unavailable without stopping owner RPC");
    let degraded = run_maker_health(paths.socket);
    assert_eq!(degraded["ready"], true);
    assert_eq!(degraded["degraded"], true);
    assert_eq!(degraded["delivery"], "available");
    assert_eq!(degraded["chat"], "unavailable");
    let unavailable = taker_command(taker, maker_key, accepted_at)
        .output()
        .expect("run real taker during Chat outage");
    assert!(
        !unavailable.status.success(),
        "Chat outage must be visible to the real taker"
    );
    assert!(
        !taker.agreement_file.exists(),
        "Chat outage must not create a final agreement"
    );
    assert!(
        !taker.actor_root.exists(),
        "failed Chat completion must not publish Taker actor authority"
    );
    assert!(
        !taker.receipt.exists(),
        "failed Chat completion must not publish an acceptance receipt"
    );
    fs::rename(&offline_chat, paths.chat_socket)
        .expect("restore Chat socket identity before shutdown");
    stop_daemon_gracefully(daemon, paths);
    *daemon = start_daemon(paths);
    wait_ready(daemon, paths.ready, paths.socket);
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
    assert_eq!(healthy["degraded"], false);
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

struct TakerFiles {
    draft: PathBuf,
    key: PathBuf,
    agreement: PathBuf,
    source_actor_config: PathBuf,
    actor_root: PathBuf,
    receipt: PathBuf,
}

fn prepare_taker_files(
    run_root: &Path,
    draft_wire: &[u8],
    maker_source: &Path,
    direction: SwapDirection,
) -> TakerFiles {
    let root = run_root.join("taker");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .expect("create owner-only taker root");
    let files = TakerFiles {
        draft: root.join("unsigned-draft.borsh"),
        key: root.join("agreement.key"),
        agreement: root.join("agreement.borsh"),
        source_actor_config: root.join("source-actor-config.json"),
        actor_root: root.join("accepted-actor"),
        receipt: root.join("acceptance-receipt.json"),
    };
    write_private(&files.draft, draft_wire);
    write_raw_key(&files.key, 2);
    prepare_taker_actor_source(&root, maker_source, &files.source_actor_config, direction);
    files
}

fn prepare_taker_actor_source(
    root: &Path,
    maker_source: &Path,
    output: &Path,
    direction: SwapDirection,
) {
    let claim_key = root.join("actor-claim-recovery.key");
    let zcash_key = root.join("actor-zcash.key");
    let capability = root.join("actor-bridge.capability");
    let preimage = root.join("actor-claim-preimage.key");
    write_raw_key(&claim_key, 0x7b);
    write_raw_key(&zcash_key, 2);
    if direction == SwapDirection::TakerSellsForeign {
        write_raw_key(&preimage, CLAIM_PREIMAGE[0]);
    }
    write_private(&capability, b"m5_taker_actor_capability_0123456789");

    let mut config: Value =
        serde_json::from_slice(&fs::read(maker_source).unwrap()).expect("Maker source JSON");
    config["role"] = json!("taker");
    config["role_state_db"] = json!(root.join("unused-taker-source-state.sqlite3"));
    config["claim_recovery"]["key_id"] = json!("m5-chat-taker-claim-v1");
    config["claim_recovery"]["key_file"] = json!(claim_key);
    config["claim_preimage_file"] = if direction == SwapDirection::TakerSellsForeign {
        json!(preimage)
    } else {
        Value::Null
    };
    config["zcash_key_file"] = json!(zcash_key);
    config["bridge"]["endpoint"] = json!("http://127.0.0.1:19002");
    config["bridge"]["journal_db"] = json!(root.join("unused-taker-source-bridge.sqlite3"));
    config["bridge"]["capability_file"] = json!(capability);
    config["bridge"]["runtime"]["sidecar_role"] = json!("taker");
    config["bridge"]["runtime"]["signer_account_id"] = json!("04".repeat(32));
    config["zcash_funding_outpoints"] = if direction == SwapDirection::TakerSellsForeign {
        json!([{"transaction_id":"bb".repeat(32),"output_index":0}])
    } else {
        json!([])
    };
    write_private(output, &serde_json::to_vec_pretty(&config).unwrap());

    let config = ActorConfig::load_private(output).expect("valid source Taker config");
    assert_eq!(config.role(), ActorRole::Taker);
    config
        .load_activate_material()
        .expect("source Taker activation material");
}

struct TakerProcess<'a> {
    direction: SwapDirection,
    delivery: &'a std::path::Path,
    chat_socket: &'a std::path::Path,
    offer_id: &'a MakerOfferId,
    reservation_id: &'a RequestId,
    draft_file: &'a std::path::Path,
    taker_key_file: &'a std::path::Path,
    agreement_file: &'a std::path::Path,
    source_actor_config: &'a std::path::Path,
    actor_root: &'a std::path::Path,
    receipt: &'a std::path::Path,
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
    taker: &TakerProcess<'_>,
    maker_key: &PublicKey,
    accepted_at: u64,
    accepted: &Value,
    pre_receipt: &PreReceiptArtifactSnapshot,
) -> TakerArtifactSnapshot {
    assert_eq!(accepted["actor"]["role"], "taker");
    assert_eq!(accepted["actor"]["provisioning_replay"], true);
    assert_eq!(accepted["actor"]["receipt_replay"], false);
    let config_path = taker.actor_root.join("taker/actor-config.json");
    let agreement_path = taker.actor_root.join("shared/agreement-v2.borsh");
    assert!(taker.actor_root.join("taker").is_dir());
    assert!(!taker.actor_root.join("maker").exists());
    let config = ActorConfig::load_private(&config_path).unwrap();
    assert_eq!(config.role(), ActorRole::Taker);
    assert_eq!(config.swap_id().as_str(), "m5-chat-swap-001");
    assert_eq!(
        config.is_local_zcash_funder(),
        taker.direction == SwapDirection::TakerSellsForeign
    );
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
    assert_private_receipt(taker.receipt);
    let receipt_inode = fs::symlink_metadata(taker.receipt).unwrap().ino();
    let receipt_bytes = fs::read(taker.receipt).unwrap();
    let receipt: Value = serde_json::from_slice(&receipt_bytes).unwrap();
    assert_eq!(receipt.as_object().unwrap().len(), 7);
    assert_eq!(receipt["schema_version"], 1);
    assert_eq!(receipt["swap_id"], "m5-chat-swap-001");
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
    assert_eq!(accepted["actor"]["receipt_file"], json!(taker.receipt));
    assert_eq!(
        accepted["actor"]["receipt_sha256"],
        hex::encode(Sha256::digest(&receipt_bytes))
    );
    let poisoned_receipt = taker
        .actor_root
        .join("taker/poisoned-acceptance-receipt.json");
    let poisoned = taker_command_with_receipt(taker, maker_key, accepted_at, &poisoned_receipt)
        .output()
        .unwrap();
    assert!(!poisoned.status.success());
    assert!(!poisoned_receipt.exists());
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

fn assert_replayed_taker_artifacts(
    taker: &TakerProcess<'_>,
    accepted: &Value,
    replay: &Value,
    snapshot: &TakerArtifactSnapshot,
) {
    assert_eq!(replay["actor"]["role"], "taker");
    assert_eq!(replay["actor"]["provisioning_replay"], true);
    assert_eq!(replay["actor"]["receipt_replay"], true);
    assert_eq!(
        replay["actor"]["receipt_sha256"],
        accepted["actor"]["receipt_sha256"]
    );
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
        fs::symlink_metadata(taker.receipt).unwrap().ino(),
        snapshot.receipt_inode
    );
    assert_eq!(fs::read(taker.receipt).unwrap(), snapshot.receipt_bytes);
}

const MAX_FAULT_PROXY_HTTP_BYTES: usize = 128 * 1024;

fn assert_completion_response_loss(
    taker: &TakerProcess<'_>,
    maker_key: &PublicKey,
    accepted_at: u64,
    database: &Path,
) -> PreReceiptArtifactSnapshot {
    let proxy_socket = taker.chat_socket.with_file_name("chat-drop-complete.sock");
    let listener = UnixListener::bind(&proxy_socket).expect("bind completion-loss proxy");
    fs::set_permissions(&proxy_socket, fs::Permissions::from_mode(0o600)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let upstream = taker.chat_socket.to_path_buf();
    let proxy = thread::spawn(move || proxy_completion_response_loss(&listener, &upstream));

    let output = taker_command_with_chat(taker, maker_key, accepted_at, &proxy_socket)
        .output()
        .expect("run Taker through completion-loss proxy");
    assert!(!output.status.success(), "lost response must fail closed");
    assert!(
        output.stdout.is_empty(),
        "failed acceptance must not emit JSON"
    );
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    assert!(!diagnostic.contains(taker.taker_key_file.to_str().unwrap()));
    assert!(!diagnostic.contains(&hex::encode(fs::read(taker.taker_key_file).unwrap())));
    proxy.join().expect("completion-loss proxy succeeds");
    fs::remove_file(&proxy_socket).expect("remove completion-loss proxy socket");

    assert!(taker.agreement_file.is_file());
    assert!(taker.actor_root.join("taker").is_dir());
    assert!(!taker.actor_root.join("maker").exists());
    assert!(
        !taker.receipt.exists(),
        "no receipt before observed completion"
    );
    let store = SqliteSwapStore::open(database).unwrap();
    let durable = store
        .load_zec_maker_negotiation(taker.offer_id)
        .unwrap()
        .expect("Maker completion committed before response loss");
    assert_eq!(durable.status(), MakerZecNegotiationStatus::Completed);
    assert_eq!(store.list_maker_actor_processes().unwrap().len(), 1);
    pre_receipt_artifact_snapshot(taker)
}

fn pre_receipt_artifact_snapshot(taker: &TakerProcess<'_>) -> PreReceiptArtifactSnapshot {
    let config_path = taker.actor_root.join("taker/actor-config.json");
    let agreement_path = taker.actor_root.join("shared/agreement-v2.borsh");
    PreReceiptArtifactSnapshot {
        config_inode: fs::symlink_metadata(&config_path).unwrap().ino(),
        config_bytes: fs::read(&config_path).unwrap(),
        agreement_inode: fs::symlink_metadata(&agreement_path).unwrap().ino(),
        agreement_bytes: fs::read(&agreement_path).unwrap(),
        config_path,
        agreement_path,
    }
}

fn proxy_completion_response_loss(listener: &UnixListener, upstream_path: &Path) {
    for (expected_method, drop_response) in [
        ("zec_chat_propose_v1", false),
        ("zec_chat_complete_v1", true),
    ] {
        let mut downstream = accept_proxy_connection(listener);
        let request = read_bounded_http_message(&mut downstream);
        let request_json: Value = serde_json::from_slice(http_body(&request)).unwrap();
        assert_eq!(request_json["method"], expected_method);

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
        if drop_response {
            assert_eq!(response_json["result"]["was_replay"], false);
            assert_eq!(response_json["result"]["swap_id"], "m5-chat-swap-001");
        } else {
            downstream.write_all(&response).unwrap();
            downstream.flush().unwrap();
        }
    }
}

fn accept_proxy_connection(listener: &UnixListener) -> UnixStream {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
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

fn accept_and_replay(
    taker: &TakerProcess<'_>,
    maker_key: &PublicKey,
    accepted_at: u64,
    authenticated: &AuthenticatedOfferRefV1,
    database: &Path,
    pre_receipt: &PreReceiptArtifactSnapshot,
) -> Vec<u8> {
    let expires_at_unix_seconds = authenticated.offer().expires_at_unix_seconds();
    let accepted = run_taker(taker, maker_key, accepted_at);
    assert_eq!(accepted["schema_version"], 1);
    assert_eq!(accepted["offer_revision"], 3);
    assert_eq!(accepted["swap_id"], "m5-chat-swap-001");
    assert_eq!(accepted["replay"]["proposal"], true);
    assert_eq!(accepted["replay"]["completion"], true);
    assert_eq!(accepted["replay"]["agreement_file"], true);
    assert_eq!(accepted["private_material_disclosed"], false);
    let taker_artifacts =
        assert_fresh_taker_artifacts(taker, maker_key, accepted_at, &accepted, pre_receipt);
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
    assert_eq!(config.swap_id().as_str(), "m5-chat-swap-001");
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
    let expiry_wait = Instant::now();
    while now() <= expires_at_unix_seconds {
        assert!(
            expiry_wait.elapsed() < Duration::from_secs(6),
            "short-lived local offer did not expire on schedule"
        );
        thread::sleep(Duration::from_millis(25));
    }
    assert!(now() > expires_at_unix_seconds);
    let offline_delivery = taker
        .delivery
        .with_file_name("delivery.acceptance-replay-offline");
    fs::rename(taker.delivery, &offline_delivery).unwrap();
    let replay = run_taker(taker, maker_key, accepted_at);
    fs::rename(&offline_delivery, taker.delivery).unwrap();
    assert_eq!(replay["replay"]["proposal"], true);
    assert_eq!(replay["replay"]["completion"], true);
    assert_eq!(replay["replay"]["agreement_file"], true);
    assert_eq!(replay["agreement_sha256"], accepted["agreement_sha256"]);
    assert_replayed_taker_artifacts(taker, &accepted, &replay, &taker_artifacts);
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
    fs::read(taker.agreement_file).expect("read taker-persisted final agreement")
}

fn run_taker(taker: &TakerProcess<'_>, maker_key: &PublicKey, accepted_at: u64) -> Value {
    let output = taker_command(taker, maker_key, accepted_at)
        .output()
        .expect("run real taker process");
    assert!(
        output.status.success(),
        "real taker failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("real taker returns bounded JSON")
}

fn taker_command(taker: &TakerProcess<'_>, maker_key: &PublicKey, accepted_at: u64) -> Command {
    taker_command_with_receipt(taker, maker_key, accepted_at, taker.receipt)
}

fn taker_command_with_receipt(
    taker: &TakerProcess<'_>,
    maker_key: &PublicKey,
    accepted_at: u64,
    receipt: &Path,
) -> Command {
    taker_command_with_overrides(taker, maker_key, accepted_at, taker.chat_socket, receipt)
}

fn taker_command_with_chat(
    taker: &TakerProcess<'_>,
    maker_key: &PublicKey,
    accepted_at: u64,
    chat_socket: &Path,
) -> Command {
    taker_command_with_overrides(taker, maker_key, accepted_at, chat_socket, taker.receipt)
}

fn taker_command_with_overrides(
    taker: &TakerProcess<'_>,
    maker_key: &PublicKey,
    accepted_at: u64,
    chat_socket: &Path,
    receipt: &Path,
) -> Command {
    let mut command = Command::new(cross_role::workspace_binary("lez-taker-cli"));
    command
        .arg("--delivery-directory")
        .arg(taker.delivery)
        .arg("--maker-public-key")
        .arg(hex::encode(maker_key.serialize()))
        .arg("--now-unix-seconds")
        .arg(accepted_at.to_string())
        .arg("--pair")
        .arg("zcash")
        .arg("--direction")
        .arg(match taker.direction {
            SwapDirection::TakerSellsForeign => "taker-sells-foreign",
            SwapDirection::TakerSellsLez => "taker-sells-lez",
        })
        .arg("--accept-zec-offer")
        .arg(taker.offer_id.as_str())
        .arg("--chat-socket")
        .arg(chat_socket)
        .arg("--reservation-id")
        .arg(taker.reservation_id.as_str())
        .arg("--foreign-units")
        .arg("10000")
        .arg("--unsigned-draft-file")
        .arg(taker.draft_file)
        .arg("--taker-signing-key-file")
        .arg(taker.taker_key_file)
        .arg("--agreement-output-file")
        .arg(taker.agreement_file)
        .arg("--zec-source-taker-config")
        .arg(taker.source_actor_config)
        .arg("--zec-taker-actor-root")
        .arg(taker.actor_root)
        .arg("--zec-acceptance-receipt")
        .arg(receipt);
    command
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

fn assert_completed_durable(
    database: &std::path::Path,
    offer_id: &MakerOfferId,
    reservation_id: &RequestId,
    authenticated: &AuthenticatedOfferRefV1,
    final_wire: &[u8],
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
    assert_eq!(durable.swap_id(), Some("m5-chat-swap-001"));
    let actor = store.list_maker_actor_processes().unwrap();
    assert_eq!(actor.len(), 1);
    let maker_config = ActorConfig::load_private(actor[0].manifest().config_path()).unwrap();
    assert_eq!(maker_config.role(), ActorRole::Maker);
    assert!(!maker_config.is_local_zcash_funder());
    let connection = Connection::open(database).unwrap();
    let maker_claim_rows: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM zec_sdk_claim_materials WHERE local_role = 'maker'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(maker_claim_rows, 0);
    assert!(
        !fs::read(database)
            .unwrap()
            .windows(CLAIM_PREIMAGE.len())
            .any(|window| window == CLAIM_PREIMAGE),
        "maker claim preimage must not occur in plaintext SQLite bytes"
    );
}

async fn assert_socket_method_isolation(
    owner: &std::path::Path,
    chat: &std::path::Path,
    proposal: &ZecChatProposeRequestV1,
) {
    assert!(
        call_local_rpc::<_, Value>(owner, "zec_chat_propose_v1", proposal)
            .await
            .is_err(),
        "owner socket must not expose taker Chat methods"
    );
    assert!(
        call_local_rpc::<_, Value>(chat, "maker_offer_list", &ListRequest::default())
            .await
            .is_err(),
        "Chat socket must not expose owner-control methods"
    );
}

async fn configure_live_route(socket: &std::path::Path, route: MakerRouteV1) {
    let disabled =
        MakerPairConfigurationV1::new(route, false, MakerPriceSourceKind::Local, 10, 10_000, 3)
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
    let enabled =
        MakerPairConfigurationV1::new(route, true, MakerPriceSourceKind::Local, 10, 10_000, 3)
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

fn unsigned_draft(
    authenticated: &AuthenticatedOfferRefV1,
    reservation_id: &RequestId,
    maker_secret: &SecretKey,
    taker_secret: &SecretKey,
    agreement_basis_time: u64,
) -> Vec<u8> {
    let maker_public = public_key(maker_secret);
    let taker_public = public_key(taker_secret);
    let maker_hash = pubkey_hash(&maker_public);
    let taker_hash = pubkey_hash(&taker_public);
    let application_swap_id = "m5-chat-swap-001";
    let escrow_program = [1; 8];
    let onchain_swap_id = derive_lez_swap_id_v1(application_swap_id.as_bytes());
    let metadata = derive_lez_metadata_account_v1(&escrow_program, &onchain_swap_id);
    let custody = derive_lez_native_custody_account_v1(&escrow_program, &onchain_swap_id);
    let secret_digest: [u8; 32] = Sha256::digest(CLAIM_PREIMAGE).into();
    let direction = authenticated.offer().route().direction();
    let (zcash_funder_hash, zcash_claimant_hash) = match direction {
        SwapDirection::TakerSellsForeign => (taker_hash, maker_hash),
        SwapDirection::TakerSellsLez => (maker_hash, taker_hash),
    };
    let contract = Bip199Contract::new(120, zcash_funder_hash, secret_digest, zcash_claimant_hash);
    let output = ExpectedBip199Output::new(
        NetworkType::Regtest,
        BranchId::Nu6_2,
        Zatoshis::from_u64(10_000).unwrap(),
        contract,
    );
    let binding = ZecSwapBinding::new(ZecProfileId::DeterministicLocalV1, output).unwrap();
    let body = ZecAgreementBodyV1::new(
        application_swap_id.to_owned(),
        direction,
        ZecProfileRecordV1::from(ZecProfileId::DeterministicLocalV1),
        ZecParticipantsV1::new(
            ZecParticipantIdentityV1::new([3; 32], maker_public.serialize()),
            ZecParticipantIdentityV1::new([4; 32], taker_public.serialize()),
        ),
        secret_digest,
        ZecLezTermsV1::new(
            LezChainIdentityV1::new(LezEnvironmentV1::DeterministicLocalV0_2, [8; 32], [7; 32]),
            escrow_program,
            LezAssetV1::Native {
                authenticated_transfer_program_id: [2; 8],
            },
            25_000,
            metadata,
            custody,
        ),
        ZecSwapBindingRecordV1::from_binding(&binding),
        ZecTransactionPolicyV1::new(
            [12; 32],
            ZcashTransparentDestinationV1::p2pkh(zcash_funder_hash),
            1,
            1,
            ZcashTransparentDestinationV1::p2pkh(zcash_claimant_hash),
            1,
            ZcashTransparentDestinationV1::p2pkh(zcash_funder_hash),
            1,
            40,
        ),
        ZecRefundPlanV1::new(
            agreement_basis_time,
            116,
            (agreement_basis_time + 60) * 1_000,
            agreement_basis_time + 90,
        ),
        NegotiationTranscriptV1::new(
            maker_zec_chat_session_id(reservation_id),
            authenticated.commitment(),
            authenticated.offer().expires_at_unix_seconds(),
        ),
    );
    ZecAgreementDraftV1::new(body).encode_wire().unwrap()
}

struct DaemonPaths<'a> {
    socket: &'a std::path::Path,
    chat_socket: &'a std::path::Path,
    ready: &'a std::path::Path,
    database: &'a std::path::Path,
    delivery: &'a std::path::Path,
    key_file: &'a std::path::Path,
    claim_key_file: &'a std::path::Path,
    claim_preimage_file: Option<&'a std::path::Path>,
    actor_root: &'a Path,
    actor_source_config: &'a Path,
    actor_program: &'a Path,
    actor_program_sha256: &'a str,
}

fn daemon_command(paths: &DaemonPaths<'_>) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lez-maker-node"));
    command
        .arg("--socket")
        .arg(paths.socket)
        .arg("--chat-socket")
        .arg(paths.chat_socket)
        .arg("--database")
        .arg(paths.database)
        .arg("--ready-file")
        .arg(paths.ready)
        .arg("--delivery-directory")
        .arg(paths.delivery)
        .arg("--delivery-signing-key-file")
        .arg(paths.key_file)
        .arg("--maker-claim-key-id")
        .arg("m5-chat-claim-key-v1")
        .arg("--maker-claim-key-file")
        .arg(paths.claim_key_file)
        .arg("--zec-source-maker-config")
        .arg(paths.actor_source_config)
        .arg("--zec-maker-actor-root")
        .arg(paths.actor_root)
        .arg("--zec-actor-program")
        .arg(paths.actor_program)
        .arg("--zec-actor-program-sha256")
        .arg(paths.actor_program_sha256);
    if let Some(claim_preimage_file) = paths.claim_preimage_file {
        command
            .arg("--maker-claim-preimage-file")
            .arg(claim_preimage_file);
    }
    command
}

fn start_daemon(paths: &DaemonPaths<'_>) -> Child {
    daemon_command(paths)
        .spawn()
        .expect("start isolated maker daemon")
}

fn assert_duplicate_actor_authority_is_rejected(paths: &DaemonPaths<'_>) {
    let output = daemon_command(paths)
        .arg("--zec-source-maker-config")
        .arg(paths.actor_source_config)
        .output()
        .expect("run maker daemon with duplicate authority");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("duplicate swap or state identity"),
        "unexpected duplicate-authority error: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn wait_ready(daemon: &mut Child, ready: &std::path::Path, socket: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(published) = fs::read_to_string(ready) {
            assert_eq!(published.trim(), socket.to_str().unwrap());
            return;
        }
        assert!(daemon.try_wait().unwrap().is_none(), "daemon exited early");
        assert!(Instant::now() < deadline, "daemon readiness timed out");
        thread::sleep(Duration::from_millis(20));
    }
}

fn stop_daemon_gracefully(daemon: &mut Child, paths: &DaemonPaths<'_>) {
    kill_process(Pid::from_child(daemon), Signal::INT).expect("signal maker daemon");
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = daemon.try_wait().expect("poll maker daemon") {
            break status;
        }
        if Instant::now() >= deadline {
            daemon.kill().expect("kill wedged maker daemon");
            daemon.wait().expect("reap wedged maker daemon");
            panic!("maker daemon did not stop within the graceful deadline");
        }
        thread::sleep(Duration::from_millis(20));
    };
    assert!(
        status.success(),
        "maker daemon exited unsuccessfully: {status}"
    );
    assert!(
        !paths.socket.exists(),
        "graceful shutdown must remove the owner socket"
    );
    assert!(
        !paths.chat_socket.exists(),
        "graceful shutdown must remove the Chat socket"
    );
    assert!(
        !paths.ready.exists(),
        "graceful shutdown must remove the readiness handoff"
    );
}

fn write_raw_key(path: &std::path::Path, byte: u8) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(&[byte; 32]).unwrap();
    file.sync_all().unwrap();
}

fn write_private(path: &std::path::Path, bytes: &[u8]) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
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

fn derived_chat_request(reservation_id: &RequestId, label: &[u8]) -> RequestId {
    let mut digest = Sha256::new();
    digest.update(b"lez-atomic-swaps/zec-taker-chat-request/v1\0");
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

fn pubkey_hash(public: &PublicKey) -> [u8; 20] {
    match TransparentAddress::from_pubkey(public) {
        TransparentAddress::PublicKeyHash(hash) => hash,
        TransparentAddress::ScriptHash(_) => unreachable!("a public key yields P2PKH"),
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
