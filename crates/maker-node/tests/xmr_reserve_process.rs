//! Exercises the take-time Monero agreement; compiled only with `pair-xmr`.
#![cfg(feature = "pair-xmr")]
#![cfg(any(target_os = "linux", target_os = "macos"))]

//! A real Maker daemon agrees one Monero swap with a fake Taker (the
//! reference actor's Taker half, driven over the Maker's Chat socket) through
//! `xmr_reserve_v1` → `xmr_chat_stage_a_v1` → `xmr_ceremony_reserve_v1` →
//! `xmr_ceremony_nonce_v1` → `xmr_ceremony_partial_v1` →
//! `xmr_chat_activate_v1`, with no actor registry and no Node-wide view key:
//! the Maker makes its own per-swap material, imports the Taker's per-swap
//! view key, and provisions its actor at activation. monerod, the swap's LEZ
//! sidecar and the Stage-A planner are fakes that serve only what the rounds
//! read.

#[path = "support/xmr_reserve_fixture.rs"]
mod xmr_reserve_fixture;

use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    os::unix::fs::OpenOptionsExt as _,
    path::{Path, PathBuf},
    process::{Child, Command},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use lez_bridge_protocol::RequestId;
use lez_maker_node::{
    AuthenticatedOfferRefV1, DeliveryOfferQueryV1, LocalPriceSetRequest, PairConfigureRequest,
    RunLocalDelivery, XmrCeremonyReserveRequestV1, XmrCeremonyReserveResponseV1,
    XmrChatActivateRequestV1, XmrChatActivateResponseV1, XmrChatStageARequestV1,
    XmrChatStageAResponseV1, XmrReserveRequestV1, XmrReserveResponseV1, call_local_chat_rpc,
    call_local_rpc,
};
use lez_swap_core::{Pair, SwapDirection};
use lez_swap_sdk_core::OfferDiscovery as _;
use lez_swap_store::{
    LocalPriceV1, MakerActorKindV1, MakerOfferId, MakerPairConfigurationV1, MakerPriceSourceKind,
    MakerRouteV1, SqliteSwapStore, maker_xmr_chat_swap_id,
};
use rustix::process::{Pid, Signal, kill_process};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use tempfile::tempdir;
use xmr_reference_actor::{
    ActorRole, XmrReservationTerms, XmrTakerCeremony, load_validated_xmr_effect_manifest_v3_bytes,
};
use xmr_reserve_fixture::{
    FakeTaker, XmrMakerDeployment, private_dir, round_request, start_fake_monerod,
};

const FOREIGN_UNITS: u64 = 1_000_000_000_000;
/// One LEZ unit per 1,000,000,000 piconero.
const LEZ_UNITS: u128 = 1_000;
const DELIVERY_KEY: u8 = 8;

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

/// One Maker with an XMR offer on its Delivery directory.
struct Maker {
    _run: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    socket: PathBuf,
    chat_socket: PathBuf,
    ready: PathBuf,
    deployment: XmrMakerDeployment,
    daemon: Daemon,
    offer_id: MakerOfferId,
    offer: AuthenticatedOfferRefV1,
    monerod: String,
}

impl Maker {
    async fn start(port_offset: u16, offer_ttl_seconds: u64) -> Self {
        let run = tempdir().unwrap();
        let root = fs::canonicalize(run.path()).unwrap();
        let monerod = start_fake_monerod();
        let home = private_dir(&root.join("maker"));
        let runtime = private_dir(&home.join("run"));
        let alive = root.join("sidecars-alive");
        fs::write(&alive, b"").unwrap();
        let port_base =
            20_000 + u16::try_from(std::process::id() % 400).unwrap() * 50 + port_offset;
        let deployment = XmrMakerDeployment::new(
            &home,
            &alive,
            &monerod,
            &fs::canonicalize("/usr/bin/true").unwrap(),
            port_base,
        );
        write_private(&home.join("delivery-signing.key"), &[DELIVERY_KEY; 32]);
        let socket = runtime.join("maker.sock");
        let chat_socket = runtime.join("chat.sock");
        let ready = runtime.join("ready");
        let daemon = spawn(&home, &socket, &chat_socket, &ready, &deployment);
        let route = MakerRouteV1::new(Pair::Monero, SwapDirection::TakerSellsLez).unwrap();
        configure_live_route(&socket, route, offer_ttl_seconds).await;
        let offer_id = MakerOfferId::new("xmr-reserve-offer-001").unwrap();
        let published = Command::new(env!("CARGO_BIN_EXE_lez-maker-cli"))
            .args(["--socket", path(&socket), "publish-offer"])
            .args(["--request-id", "xmr-reserve-publish-001"])
            .args(["--offer-id", offer_id.as_str(), "--pair", "monero"])
            .args(["--direction", "taker-sells-lez"])
            .output()
            .unwrap();
        assert!(
            published.status.success(),
            "publish-offer: {}",
            String::from_utf8_lossy(&published.stderr)
        );
        let offer = RunLocalDelivery::subscriber(home.join("delivery"), maker_key())
            .unwrap()
            .discover(&DeliveryOfferQueryV1::for_route(route, now()))
            .await
            .unwrap()
            .into_iter()
            .next()
            .expect("the published offer is discoverable");
        Self {
            _run: run,
            root,
            home,
            socket,
            chat_socket,
            ready,
            deployment,
            daemon,
            offer_id,
            offer,
            monerod,
        }
    }

    fn restart(&mut self) {
        kill_process(Pid::from_child(&self.daemon.0), Signal::INT).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.daemon.0.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "the Maker did not stop");
            thread::sleep(Duration::from_millis(20));
        }
        self.daemon = spawn(
            &self.home,
            &self.socket,
            &self.chat_socket,
            &self.ready,
            &self.deployment,
        );
    }

    fn taker(&self) -> FakeTaker {
        FakeTaker::new(&private_dir(&self.root.join("taker")), &self.monerod)
    }

    fn terms(&self, reservation_id: &RequestId) -> XmrReservationTerms {
        XmrReservationTerms {
            swap_id: maker_xmr_chat_swap_id(&self.offer.commitment(), reservation_id),
            amount_piconero: FOREIGN_UNITS,
            lez_amount: LEZ_UNITS,
        }
    }

    async fn chat<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        request: &P,
    ) -> anyhow::Result<R> {
        call_local_chat_rpc(&self.chat_socket, method, request).await
    }

    fn reserve_request(
        &self,
        taker: &FakeTaker,
        reservation_id: &RequestId,
    ) -> XmrReserveRequestV1 {
        taker.reserve_request(
            reservation_id,
            &self.offer_id,
            self.offer.signed_envelope(),
            FOREIGN_UNITS,
        )
    }

    fn stage_a_request(&self, reservation_id: &RequestId, wire: &[u8]) -> XmrChatStageARequestV1 {
        XmrChatStageARequestV1 {
            schema_version: 1,
            request_id: round_request(reservation_id, b"stage-a"),
            offer_id: self.offer_id.clone(),
            expected_offer_revision: 1,
            reservation_id: reservation_id.clone(),
            foreign_units: FOREIGN_UNITS,
            signed_offer_envelope: self.offer.signed_envelope().to_vec(),
            stage_a_wire: wire.to_vec(),
        }
    }

    fn activate_request(
        &self,
        reservation_id: &RequestId,
        wire: &[u8],
    ) -> XmrChatActivateRequestV1 {
        XmrChatActivateRequestV1 {
            schema_version: 1,
            request_id: round_request(reservation_id, b"activate"),
            offer_id: self.offer_id.clone(),
            expected_offer_revision: 2,
            reservation_id: reservation_id.clone(),
            activation_wire: wire.to_vec(),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::too_many_lines)] // The six rounds and their replays are one audit surface.
async fn a_monero_swap_is_agreed_at_take_time_in_six_rounds_and_every_round_replays() {
    let mut maker = Maker::start(0, 300).await;
    let taker = maker.taker();
    let reservation_id = RequestId::new("xmr-reserve-reservation-001").unwrap();
    let terms = maker.terms(&reservation_id);

    // R1: the Maker answers with its own packet and a signed Stage A over the
    // Taker's per-swap view key; the Taker composes the same Stage A itself.
    let request = maker.reserve_request(&taker, &reservation_id);
    let started = Instant::now();
    let reserved: XmrReserveResponseV1 = maker.chat("xmr_reserve_v1", &request).await.unwrap();
    eprintln!("xmr_reserve_v1 answered in {:?}", started.elapsed());
    assert!(!reserved.was_replay);
    assert_eq!(reserved.maker_owner_nonce, 0);
    assert_eq!(
        reserved.monero_restore_height,
        xmr_reserve_fixture::MONERO_TIP
    );
    let stage_a = taker
        .check(&reserved, &terms, maker_key().serialize())
        .expect("the Taker composes the Maker's Stage A");
    let replayed: XmrReserveResponseV1 = maker.chat("xmr_reserve_v1", &request).await.unwrap();
    assert!(replayed.was_replay);
    assert_eq!(
        XmrReserveResponseV1 {
            was_replay: false,
            ..replayed
        },
        reserved
    );
    // The same reservation id with another request is taken.
    let mut other = maker.reserve_request(&taker, &reservation_id);
    other.foreign_units -= 1;
    assert!(
        maker
            .chat::<_, XmrReserveResponseV1>("xmr_reserve_v1", &other)
            .await
            .is_err()
    );
    let swap = maker.deployment.swap(&reservation_id);
    assert!(swap.join("xmr-reservation.json").is_file());
    assert!(swap.join("xmr/monero-view.key").is_file());
    assert!(
        !swap.join("sidecar.json").exists(),
        "no sidecar before activation"
    );

    // R2 accepts only the reserved Stage A: a Stage A of another reservation
    // id fails before any store write, and the real one replays.
    let crossed = RequestId::new("xmr-reserve-crossed-001").unwrap();
    assert!(
        maker
            .chat::<_, XmrChatStageAResponseV1>(
                "xmr_chat_stage_a_v1",
                &maker.stage_a_request(&crossed, &stage_a),
            )
            .await
            .is_err()
    );
    let staged: XmrChatStageAResponseV1 = maker
        .chat(
            "xmr_chat_stage_a_v1",
            &maker.stage_a_request(&reservation_id, &stage_a),
        )
        .await
        .unwrap();
    assert!(!staged.was_replay);
    assert_eq!(staged.offer_revision, 2);
    assert_eq!(*staged.swap_id, hex::encode(terms.swap_id));
    let staged_again: XmrChatStageAResponseV1 = maker
        .chat(
            "xmr_chat_stage_a_v1",
            &maker.stage_a_request(&reservation_id, &stage_a),
        )
        .await
        .unwrap();
    assert!(staged_again.was_replay);

    // R3–R5: the Taker drives the ceremony. Resent, every request is
    // byte-identical and every answer a replay; a different request for a
    // completed round is refused.
    let (stage_b, replays) = taker.ceremony(&maker.chat_socket, &reservation_id).await;
    assert_eq!(replays, [false; 3]);
    assert!(
        !swap.join("xmr/stage-b.wire").exists(),
        "the Maker keeps Stage B only once it is activated"
    );
    let (resent, replays) = taker.ceremony(&maker.chat_socket, &reservation_id).await;
    assert_eq!(resent, stage_b);
    assert_eq!(replays, [true; 3]);
    let different = XmrCeremonyReserveRequestV1 {
        schema_version: 1,
        request_id: round_request(&reservation_id, b"another"),
        reservation_id: reservation_id.clone(),
        taker_commitments: XmrTakerCeremony::open(&taker.role_root)
            .unwrap()
            .commitments()
            .unwrap()
            .into(),
    };
    assert!(
        maker
            .chat::<_, XmrCeremonyReserveResponseV1>("xmr_ceremony_reserve_v1", &different)
            .await
            .is_err()
    );

    // R6: the Maker validates Stage B with the swap's own view key and
    // provisions its actor before the store registers it.
    let activated: XmrChatActivateResponseV1 = maker
        .chat(
            "xmr_chat_activate_v1",
            &maker.activate_request(&reservation_id, &stage_b),
        )
        .await
        .unwrap();
    assert!(!activated.was_replay);
    assert_eq!(activated.offer_revision, 3);
    assert_eq!(*activated.swap_id, hex::encode(terms.swap_id));
    assert_eq!(fs::read(swap.join("xmr/stage-b.wire")).unwrap(), stage_b);
    assert_eq!(
        fs::read(taker.role_root.join("stage-b.wire")).unwrap(),
        stage_b
    );
    assert!(swap.join("sidecar.json").is_file());
    let manifest_file = swap.join("xmr-effect/actor-manifest.json");
    let authority_file = swap.join("xmr-effect/effect-authority.json");
    let run_id = format!("swap-{}", reservation_id.as_str());
    let _ = load_validated_xmr_effect_manifest_v3_bytes(
        &fs::read(&manifest_file).unwrap(),
        &fs::read(&authority_file).unwrap(),
        ActorRole::Maker,
        &run_id,
    )
    .expect("the registered manifest is a complete schema-3 effect authority");
    let actors = SqliteSwapStore::open(maker.home.join("maker.sqlite3"))
        .unwrap()
        .list_maker_actor_processes()
        .unwrap();
    assert_eq!(actors.len(), 1);
    assert_eq!(actors[0].manifest().kind(), MakerActorKindV1::Monero);
    assert_eq!(
        actors[0].manifest().swap_id().as_str(),
        hex::encode(terms.swap_id)
    );
    assert_eq!(actors[0].manifest().config_path(), manifest_file);
    assert_eq!(
        actors[0].manifest().state_database_path(),
        swap.join("xmr/adaptor.sqlite3")
    );
    assert_eq!(
        fs::read_dir(&maker.deployment.actor_root).unwrap().count(),
        1
    );

    // A restarted Maker replays the activation from its durable state.
    maker.restart();
    let replayed: XmrChatActivateResponseV1 = maker
        .chat(
            "xmr_chat_activate_v1",
            &maker.activate_request(&reservation_id, &stage_b),
        )
        .await
        .unwrap();
    assert!(replayed.was_replay);
    assert_eq!(
        replayed.activation_commitment,
        activated.activation_commitment
    );
    // The replay left the journals the actor pinned as they were.
    let _ = load_validated_xmr_effect_manifest_v3_bytes(
        &fs::read(&manifest_file).unwrap(),
        &fs::read(&authority_file).unwrap(),
        ActorRole::Maker,
        &run_id,
    )
    .expect("the registered manifest still validates after the replay");
    let replayed: XmrReserveResponseV1 = maker.chat("xmr_reserve_v1", &request).await.unwrap();
    assert!(replayed.was_replay);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_expired_reservation_refuses_every_later_round() {
    // The reservation's grace is capped at the offer's expiry.
    let maker = Maker::start(10, 5).await;
    let taker = maker.taker();
    let reservation_id = RequestId::new("xmr-reserve-reservation-002").unwrap();
    let reserved: XmrReserveResponseV1 = maker
        .chat(
            "xmr_reserve_v1",
            &maker.reserve_request(&taker, &reservation_id),
        )
        .await
        .expect("the reservation is made while the offer is live");
    let stage_a = taker
        .check(
            &reserved,
            &maker.terms(&reservation_id),
            maker_key().serialize(),
        )
        .unwrap();
    let expires = maker.offer.offer().expires_at_unix_seconds();
    while now() <= expires {
        thread::sleep(Duration::from_millis(200));
    }
    let staged = maker
        .chat::<_, XmrChatStageAResponseV1>(
            "xmr_chat_stage_a_v1",
            &maker.stage_a_request(&reservation_id, &stage_a),
        )
        .await;
    assert!(staged.is_err(), "Stage A after the expiry is refused");
    let error = maker
        .chat::<_, XmrCeremonyReserveResponseV1>(
            "xmr_ceremony_reserve_v1",
            &XmrCeremonyReserveRequestV1 {
                schema_version: 1,
                request_id: round_request(&reservation_id, b"ceremony-reserve"),
                reservation_id: reservation_id.clone(),
                taker_commitments: lez_maker_node::XmrLegPacketsV1 {
                    claim: vec![1],
                    refund: vec![1],
                },
            },
        )
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("expired"), "{error:#}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_crash_between_stage_and_rename_leaves_no_reservation() {
    let maker = Maker::start(20, 300).await;
    let taker = maker.taker();
    let reservation_id = RequestId::new("xmr-reserve-reservation-003").unwrap();
    // What a Maker that died after composing, before its rename, leaves: a
    // stage with a role root and even a record, beside no swap directory.
    let swap = maker.deployment.swap(&reservation_id);
    let stages = swap.parent().unwrap().join(".stage");
    private_dir(&stages);
    let stage = stages.join(format!("{}.xmr", reservation_id.as_str()));
    private_dir(&stage);
    private_dir(&stage.join("xmr"));
    write_private(&stage.join("xmr-reservation.json"), b"{\"half\":true}");
    assert!(!swap.exists());
    let reserved: XmrReserveResponseV1 = maker
        .chat(
            "xmr_reserve_v1",
            &maker.reserve_request(&taker, &reservation_id),
        )
        .await
        .expect("the reservation is made again from nothing");
    assert!(!reserved.was_replay);
    assert!(!stage.exists());
    assert!(swap.join("xmr-reservation.json").is_file());
    taker
        .check(
            &reserved,
            &maker.terms(&reservation_id),
            maker_key().serialize(),
        )
        .expect("the remade reservation is whole");
}

/// The attack the stage once allowed: a Taker reserves `x.xmr-stage`, the
/// Maker funds Monero for it, then the Taker reserves `x`, and composing the
/// stage of `x` deleted the funded swap's directory with the only copy of
/// the Maker's Monero share. A dotted id is now refused on the wire, and a
/// directory with such a name (a swap reserved before the grammar) is no
/// stage of anything.
#[tokio::test(flavor = "multi_thread")]
async fn a_reservation_cannot_delete_a_swap_named_like_its_stage() {
    let maker = Maker::start(20, 300).await;
    let taker = maker.taker();
    let reservation_id = RequestId::new("xmr-reserve-reservation-004").unwrap();
    let dotted = RequestId::new(format!("{}.xmr-stage", reservation_id.as_str())).unwrap();
    let error = maker
        .chat::<_, XmrReserveResponseV1>("xmr_reserve_v1", &maker.reserve_request(&taker, &dotted))
        .await
        .expect_err("a dotted reservation id is refused");
    assert!(format!("{error:#}").contains("reservation id"), "{error:#}");
    assert!(!maker.deployment.swap(&dotted).exists());

    // A funded swap that was named so before the grammar existed.
    let funded = maker.deployment.swap(&dotted);
    private_dir(&funded);
    private_dir(&funded.join("xmr"));
    write_private(&funded.join("xmr/maker-share.key"), b"the only copy");
    let reserved: XmrReserveResponseV1 = maker
        .chat(
            "xmr_reserve_v1",
            &maker.reserve_request(&taker, &reservation_id),
        )
        .await
        .expect("the prefix id reserves");
    assert!(!reserved.was_replay);
    assert_eq!(
        fs::read(funded.join("xmr/maker-share.key")).unwrap(),
        b"the only copy",
        "reserving the prefix id left the funded swap whole"
    );
}

fn spawn(
    home: &Path,
    socket: &Path,
    chat_socket: &Path,
    ready: &Path,
    deployment: &XmrMakerDeployment,
) -> Daemon {
    let _ = fs::remove_file(ready);
    let mut daemon = Daemon(
        Command::new(env!("CARGO_BIN_EXE_lez-maker-node"))
            .args(["--socket", path(socket), "--chat-socket", path(chat_socket)])
            .args(["--database", path(&home.join("maker.sqlite3"))])
            .args(["--ready-file", path(ready)])
            .args(["--delivery-directory", path(&home.join("delivery"))])
            .args([
                "--delivery-signing-key-file",
                path(&home.join("delivery-signing.key")),
            ])
            .args(deployment.arguments())
            .spawn()
            .expect("start the Maker daemon"),
    );
    let deadline = Instant::now() + Duration::from_mins(1);
    while !ready.exists() {
        assert!(
            daemon.0.try_wait().unwrap().is_none(),
            "the Maker exited early"
        );
        assert!(Instant::now() < deadline, "the Maker did not become ready");
        thread::sleep(Duration::from_millis(20));
    }
    daemon
}

async fn configure_live_route(socket: &Path, route: MakerRouteV1, offer_ttl_seconds: u64) {
    for (request_id, enabled, revision) in [
        ("xmr-reserve-route-create-001", false, None),
        ("xmr-reserve-route-enable-001", true, Some(1)),
    ] {
        if enabled {
            let _: Value = call_local_rpc(
                socket,
                "maker_local_price_set",
                &LocalPriceSetRequest {
                    request_id: RequestId::new("xmr-reserve-price-create-001").unwrap(),
                    expected_revision: None,
                    price: LocalPriceV1::new(route, 1, 1_000_000_000).unwrap(),
                },
            )
            .await
            .unwrap();
        }
        let _: Value = call_local_rpc(
            socket,
            "maker_pair_configure",
            &PairConfigureRequest {
                request_id: RequestId::new(request_id).unwrap(),
                expected_revision: revision,
                configuration: MakerPairConfigurationV1::new(
                    route,
                    enabled,
                    MakerPriceSourceKind::Local,
                    FOREIGN_UNITS,
                    FOREIGN_UNITS,
                    offer_ttl_seconds,
                )
                .unwrap(),
            },
        )
        .await
        .unwrap();
    }
}

fn maker_key() -> PublicKey {
    PublicKey::from_secret_key(
        &Secp256k1::signing_only(),
        &SecretKey::from_slice(&[DELIVERY_KEY; 32]).unwrap(),
    )
}

fn write_private(path: &Path, bytes: &[u8]) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}

fn path(value: &Path) -> &str {
    value.to_str().unwrap()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
