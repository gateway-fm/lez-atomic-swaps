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
    OfferPublishRequest, PairConfigureRequest, RunLocalDelivery, ZecChatProposalV1,
    ZecChatProposeRequestV1, call_local_rpc,
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
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use serde_json::Value;
use tempfile::tempdir;
use zcash_protocol::{
    consensus::{BranchId, NetworkType},
    value::Zatoshis,
};
use zcash_transparent::address::TransparentAddress;

#[tokio::test]
async fn separate_taker_stages_an_offer_bound_maker_proposal_before_response() {
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
    write_key(&key_file, 8);
    let mut daemon = start_daemon(
        &socket,
        &chat_socket,
        &ready,
        &database,
        &delivery,
        &key_file,
    );
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

    thread::sleep(Duration::from_millis(1_100));
    let replay: ZecChatProposalV1 =
        call_local_rpc(&chat_socket, "zec_chat_propose_v1", &proposal_request)
            .await
            .unwrap();
    assert!(replay.was_replay);
    assert_eq!(replay.offer_revision, 2);
    assert_eq!(replay.proposal_wire, proposal.proposal_wire);

    daemon
        .kill()
        .expect("terminate daemon after committed response");
    daemon.wait().expect("reap daemon");
    let store = SqliteSwapStore::open(&database).unwrap();
    let durable = store
        .load_zec_maker_negotiation(&offer_id)
        .unwrap()
        .expect("proposal remains durable after process termination");
    assert_eq!(durable.status(), MakerZecNegotiationStatus::Proposed);
    assert_eq!(durable.reservation_id(), &reservation_id);
    assert_eq!(durable.maker_proposal_wire(), proposal.proposal_wire);
    assert_eq!(durable.offer_commitment(), &authenticated.commitment());
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
    let contract = Bip199Contract::new(120, maker_hash, [9; 32], taker_hash);
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
        [9; 32],
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

fn start_daemon(
    socket: &std::path::Path,
    chat_socket: &std::path::Path,
    ready: &std::path::Path,
    database: &std::path::Path,
    delivery: &std::path::Path,
    key_file: &std::path::Path,
) -> Child {
    Command::new(env!("CARGO_BIN_EXE_lez-maker-daemon"))
        .arg("--socket")
        .arg(socket)
        .arg("--chat-socket")
        .arg(chat_socket)
        .arg("--database")
        .arg(database)
        .arg("--ready-file")
        .arg(ready)
        .arg("--delivery-directory")
        .arg(delivery)
        .arg("--delivery-signing-key-file")
        .arg(key_file)
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
