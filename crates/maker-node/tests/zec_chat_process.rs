//! Separate-process happy-path proof for the run-local ZEC Chat boundary.

use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _},
    process::{Child, Command},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use lez_bridge_protocol::RequestId;
use lez_maker_node::{
    AuthenticatedOfferRefV1, DeliveryOfferQueryV1, ListRequest, LocalPriceSetRequest,
    OfferPublishRequest, PairConfigureRequest, RunLocalDelivery, ZecChatProposeRequestV1,
    call_local_rpc,
};
use lez_swap_core::{Pair, SwapDirection};
use lez_swap_sdk_core::OfferDiscovery as _;
use lez_swap_store::{
    LocalPriceV1, MakerOfferId, MakerPairConfigurationV1, MakerPriceSourceKind, MakerRouteV1,
    MakerZecNegotiationStatus, SqliteSwapStore, maker_zec_chat_session_id,
};
use lez_zec_swap_sdk::{
    Bip199Contract, ExpectedBip199Output, LezAssetV1, LezChainIdentityV1, LezEnvironmentV1,
    NegotiationTranscriptV1, ZcashTransparentDestinationV1, ZecAgreementBodyV1,
    ZecAgreementDraftV1, ZecLezTermsV1, ZecParticipantIdentityV1, ZecParticipantsV1, ZecProfileId,
    ZecProfileRecordV1, ZecRefundPlanV1, ZecSwapBinding, ZecSwapBindingRecordV1,
    ZecTransactionPolicyV1, derive_lez_metadata_account_v1, derive_lez_native_custody_account_v1,
    derive_lez_swap_id_v1,
};
use rustix::process::{Pid, Signal, kill_process};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use tempfile::tempdir;
use zcash_protocol::{
    consensus::{BranchId, NetworkType},
    value::Zatoshis,
};
use zcash_transparent::address::TransparentAddress;

const CLAIM_PREIMAGE: [u8; 32] = [0x44; 32];

#[tokio::test]
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
    let daemon_paths = DaemonPaths {
        socket: &socket,
        chat_socket: &chat_socket,
        ready: &ready,
        database: &database,
        delivery: &delivery,
        key_file: &key_file,
        claim_key_file: &claim_key_file,
        claim_preimage_file: &claim_preimage_file,
    };
    let mut daemon = start_daemon(&daemon_paths);
    wait_ready(&mut daemon, &ready, &socket);

    let route = MakerRouteV1::new(Pair::Zcash, SwapDirection::TakerSellsLez).unwrap();
    configure_live_route(&socket, route).await;
    let offer_id = MakerOfferId::new("m5-chat-offer-001").unwrap();
    let _: Value = call_local_rpc(
        &socket,
        "maker_offer_publish",
        &OfferPublishRequest {
            request_id: request("m5-chat-publish-001"),
            offer_id: offer_id.clone(),
            route,
        },
    )
    .await
    .unwrap();

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
    let draft_wire = unsigned_draft(&authenticated, &reservation_id, &maker_secret, &key(2));
    let taker_root = run.path().join("taker");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&taker_root)
        .expect("create owner-only taker root");
    let draft_file = taker_root.join("unsigned-draft.borsh");
    let taker_key_file = taker_root.join("agreement.key");
    let agreement_file = taker_root.join("agreement.borsh");
    write_private(&draft_file, &draft_wire);
    write_raw_key(&taker_key_file, 2);
    let proposal_request = ZecChatProposeRequestV1 {
        schema_version: 1,
        request_id: request("m5-chat-propose-001"),
        offer_id: offer_id.clone(),
        expected_offer_revision: 1,
        reservation_id: reservation_id.clone(),
        foreign_units: 10_000,
        signed_offer_envelope: authenticated.signed_envelope().to_vec(),
        unsigned_draft_wire: draft_wire.clone(),
    };

    assert_socket_method_isolation(&socket, &chat_socket, &proposal_request).await;
    let accepted_at = now();
    let taker = TakerProcess {
        delivery: &delivery,
        chat_socket: &chat_socket,
        offer_id: &offer_id,
        reservation_id: &reservation_id,
        draft_file: &draft_file,
        taker_key_file: &taker_key_file,
        agreement_file: &agreement_file,
    };
    let final_wire = assert_taker_accepts_and_replays(&taker, &maker_key, accepted_at);

    stop_daemon_gracefully(&mut daemon, &daemon_paths);
    assert_completed_durable(
        &database,
        &offer_id,
        &reservation_id,
        &authenticated,
        &final_wire,
    );
}

struct TakerProcess<'a> {
    delivery: &'a std::path::Path,
    chat_socket: &'a std::path::Path,
    offer_id: &'a MakerOfferId,
    reservation_id: &'a RequestId,
    draft_file: &'a std::path::Path,
    taker_key_file: &'a std::path::Path,
    agreement_file: &'a std::path::Path,
}

fn assert_taker_accepts_and_replays(
    taker: &TakerProcess<'_>,
    maker_key: &PublicKey,
    accepted_at: u64,
) -> Vec<u8> {
    let accepted = run_taker(taker, maker_key, accepted_at);
    assert_eq!(accepted["schema_version"], 1);
    assert_eq!(accepted["offer_revision"], 3);
    assert_eq!(accepted["swap_id"], "m5-chat-swap-001");
    assert_eq!(accepted["replay"]["proposal"], false);
    assert_eq!(accepted["replay"]["completion"], false);
    assert_eq!(accepted["replay"]["agreement_file"], false);
    assert_eq!(accepted["private_material_disclosed"], false);
    thread::sleep(Duration::from_millis(1_100));
    let replay = run_taker(taker, maker_key, accepted_at);
    assert_eq!(replay["replay"]["proposal"], true);
    assert_eq!(replay["replay"]["completion"], true);
    assert_eq!(replay["replay"]["agreement_file"], true);
    assert_eq!(replay["agreement_sha256"], accepted["agreement_sha256"]);
    fs::read(taker.agreement_file).expect("read taker-persisted final agreement")
}

fn run_taker(taker: &TakerProcess<'_>, maker_key: &PublicKey, accepted_at: u64) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_lez-taker"))
        .arg("--delivery-directory")
        .arg(taker.delivery)
        .arg("--maker-public-key")
        .arg(hex::encode(maker_key.serialize()))
        .arg("--now-unix-seconds")
        .arg(accepted_at.to_string())
        .arg("--pair")
        .arg("zcash")
        .arg("--direction")
        .arg("taker-sells-lez")
        .arg("--accept-zec-offer")
        .arg(taker.offer_id.as_str())
        .arg("--chat-socket")
        .arg(taker.chat_socket)
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
        MakerPairConfigurationV1::new(route, false, MakerPriceSourceKind::Local, 10, 10_000, 300)
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
        MakerPairConfigurationV1::new(route, true, MakerPriceSourceKind::Local, 10, 10_000, 300)
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
) -> Vec<u8> {
    let current = now();
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
    let contract = Bip199Contract::new(120, maker_hash, secret_digest, taker_hash);
    let output = ExpectedBip199Output::new(
        NetworkType::Regtest,
        BranchId::Nu6_2,
        Zatoshis::from_u64(10_000).unwrap(),
        contract,
    );
    let binding = ZecSwapBinding::new(ZecProfileId::DeterministicLocalV1, output).unwrap();
    let body = ZecAgreementBodyV1::new(
        application_swap_id.to_owned(),
        SwapDirection::TakerSellsLez,
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
            ZcashTransparentDestinationV1::p2pkh(maker_hash),
            1,
            1,
            ZcashTransparentDestinationV1::p2pkh(taker_hash),
            1,
            ZcashTransparentDestinationV1::p2pkh(maker_hash),
            1,
            40,
        ),
        ZecRefundPlanV1::new(current, 116, (current + 60) * 1_000, current + 90),
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
    claim_preimage_file: &'a std::path::Path,
}

fn start_daemon(paths: &DaemonPaths<'_>) -> Child {
    Command::new(env!("CARGO_BIN_EXE_lez-maker-daemon"))
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
        .arg("--maker-claim-preimage-file")
        .arg(paths.claim_preimage_file)
        .spawn()
        .expect("start isolated maker daemon")
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

fn request(value: &str) -> RequestId {
    RequestId::new(value).unwrap()
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
