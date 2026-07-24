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
    OfferPublishRequest, PairConfigureRequest, RunLocalDelivery, ZecChatCompleteRequestV1,
    ZecChatCompleteResponseV1, ZecChatProposalV1, ZecChatProposeRequestV1, call_local_rpc,
};
use lez_swap_core::{Pair, SwapDirection, UnixSeconds};
use lez_swap_sdk_core::OfferDiscovery as _;
use lez_swap_store::{
    LocalPriceV1, MakerOfferId, MakerPairConfigurationV1, MakerPriceSourceKind, MakerRouteV1,
    MakerZecNegotiationStatus, SqliteSwapStore, maker_zec_chat_session_id,
};
use lez_zec_swap_sdk::{
    Bip199Contract, ExpectedBip199Output, LezAssetV1, LezChainIdentityV1, LezEnvironmentV1,
    NegotiationTranscriptV1, ZcashTransparentDestinationV1, ZecAgreementBodyV1,
    ZecAgreementDraftV1, ZecLezTermsV1, ZecMakerAgreementProposalV1, ZecParticipantIdentityV1,
    ZecParticipantsV1, ZecProfileId, ZecProfileRecordV1, ZecRefundPlanV1, ZecSwapBinding,
    ZecSwapBindingRecordV1, ZecTransactionPolicyV1, derive_lez_metadata_account_v1,
    derive_lez_native_custody_account_v1, derive_lez_swap_id_v1,
};
use secp256k1::{Message, PublicKey, Secp256k1, SecretKey};
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
    write_key(&key_file, 8);
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
    let proposal_request = ZecChatProposeRequestV1 {
        schema_version: 1,
        request_id: request("m5-chat-propose-001"),
        offer_id: offer_id.clone(),
        expected_offer_revision: 1,
        reservation_id: reservation_id.clone(),
        foreign_units: 10_000,
        signed_offer_envelope: authenticated.signed_envelope().to_vec(),
        unsigned_draft_wire: draft_wire,
    };

    assert_socket_method_isolation(&socket, &chat_socket, &proposal_request).await;
    let proposal: ZecChatProposalV1 =
        call_local_rpc(&chat_socket, "zec_chat_propose_v1", &proposal_request)
            .await
            .unwrap();
    assert_eq!(proposal.offer_revision, 2);
    assert!(!proposal.was_replay);
    assert_eq!(proposal.reservation_id, reservation_id);
    assert_eq!(proposal.lez_units, 25_000);
    assert_eq!(proposal.maker_identity, maker_key.serialize());
    let validated =
        ZecMakerAgreementProposalV1::from_wire_at(&proposal.proposal_wire, UnixSeconds::new(now()))
            .expect("taker validates the exact maker-signed proposal");
    assert_eq!(validated.commitment(), &proposal.agreement_commitment);

    assert_proposal_replay(&chat_socket, &proposal_request, &proposal).await;

    let final_wire = complete_chat(
        &chat_socket,
        &offer_id,
        &reservation_id,
        &proposal,
        validated,
    )
    .await;

    daemon
        .kill()
        .expect("terminate daemon after committed response");
    daemon.wait().expect("reap daemon");
    assert_completed_durable(
        &database,
        &offer_id,
        &reservation_id,
        &proposal,
        &authenticated,
        &final_wire,
    );
}

async fn assert_proposal_replay(
    chat_socket: &std::path::Path,
    request: &ZecChatProposeRequestV1,
    original: &ZecChatProposalV1,
) {
    thread::sleep(Duration::from_millis(1_100));
    let replay: ZecChatProposalV1 = call_local_rpc(chat_socket, "zec_chat_propose_v1", request)
        .await
        .unwrap();
    assert!(replay.was_replay);
    assert_eq!(replay.offer_revision, 2);
    assert_eq!(replay.proposal_wire, original.proposal_wire);
}

async fn complete_chat(
    chat_socket: &std::path::Path,
    offer_id: &MakerOfferId,
    reservation_id: &RequestId,
    proposal: &ZecChatProposalV1,
    validated: ZecMakerAgreementProposalV1,
) -> Vec<u8> {
    let taker_signature = Secp256k1::signing_only()
        .sign_ecdsa(
            &Message::from_digest(proposal.agreement_commitment),
            &key(2),
        )
        .serialize_compact();
    let agreement = validated
        .complete_at(taker_signature, UnixSeconds::new(now()))
        .expect("taker countersigns the exact maker proposal");
    let final_wire = agreement.encode_wire().unwrap();
    let request = ZecChatCompleteRequestV1 {
        schema_version: 1,
        request_id: request("m5-chat-complete-001"),
        offer_id: offer_id.clone(),
        expected_offer_revision: 2,
        reservation_id: reservation_id.clone(),
        final_agreement_wire: final_wire.clone(),
    };
    let completed: ZecChatCompleteResponseV1 =
        call_local_rpc(chat_socket, "zec_chat_complete_v1", &request)
            .await
            .unwrap();
    assert_eq!(completed.offer_revision, 3);
    assert!(!completed.was_replay);
    assert_eq!(completed.swap_id.as_ref(), "m5-chat-swap-001");
    thread::sleep(Duration::from_millis(1_100));
    let replay: ZecChatCompleteResponseV1 =
        call_local_rpc(chat_socket, "zec_chat_complete_v1", &request)
            .await
            .unwrap();
    assert_eq!(replay.offer_revision, 3);
    assert!(replay.was_replay);
    assert_eq!(replay.swap_id, completed.swap_id);
    final_wire
}

fn assert_completed_durable(
    database: &std::path::Path,
    offer_id: &MakerOfferId,
    reservation_id: &RequestId,
    proposal: &ZecChatProposalV1,
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
    assert_eq!(durable.maker_proposal_wire(), proposal.proposal_wire);
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

fn write_key(path: &std::path::Path, byte: u8) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    writeln!(file, "{}", hex::encode([byte; 32])).unwrap();
    file.sync_all().unwrap();
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
