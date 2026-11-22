//! What a Maker Node needs around the take-time Monero agreement in a process
//! test, and a fake Taker that drives it: a fake monerod (a raw loopback
//! socket serving the two reads R1 makes), a fake swap sidecar and a fixture
//! Stage-A planner (both this test binary again), the Maker's role configs,
//! and the Taker's half of R1 and R3–R5 through the reference actor's
//! library, over the Maker's Chat socket.
#![allow(dead_code)] // Each including test binary uses its own part.

use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::{Read as _, Write as _},
    net::SocketAddr,
    os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use jsonrpsee::{RpcModule, server::ServerBuilder};
use lez_bridge_protocol::RequestId;
use lez_btc_role_lifecycle::BtcRoleRuntime;
use lez_maker_node::{
    AuthenticatedOfferRefV1, XmrCeremonyNonceRequestV1, XmrCeremonyNonceResponseV1,
    XmrCeremonyPartialResponseV1, XmrCeremonyReserveRequestV1, XmrCeremonyReserveResponseV1,
    XmrReserveRequestV1, XmrReserveResponseV1, call_local_chat_rpc,
};
use lez_swap_core::{Participant, SwapId};
use lez_swap_store::{MakerOfferId, maker_xmr_chat_swap_id};
use lez_xmr_swap_sdk::{
    MoneroAddressNetworkV1, MoneroSharedAddressV1, ValidatedXmrAgreementBodyV1, XmrAgreementBodyV1,
    XmrLezTermsV1, XmrMessagesV1, XmrMoneroTermsV1, XmrNamedProfileV1, XmrParticipantsV1,
    XmrSwapDirectionV1, XmrWindowsV1,
};
use serde_json::json;
use sha2::{Digest as _, Sha256};
use xmr_reference_actor::{
    ValidatedRolePacket, XMR_ADAPTOR_JOURNAL_FILE, XMR_MAKER_PACKET_FILE, XMR_STAGE_A_FILE,
    XMR_STAGE_B_FILE, XMR_TAKER_PACKET_FILE, XmrCeremonyPackets, XmrLezRole, XmrMakerReservation,
    XmrMoneroChainView, XmrReservationTerms, XmrRoleConfigV1, XmrStageAPlanInputV1,
    XmrTakerCeremony, XmrTakerCheck, begin_xmr_taker_role, check_xmr_taker_role,
    xmr_ceremony_partial_answer, xmr_taker_contribution,
};

/// What the fake monerod reports: its genesis and its tip height.
pub const MONERO_GENESIS: [u8; 32] = [0x31; 32];
pub const MONERO_TIP: u64 = 150;
/// The name the including test binary gives this module, for `--exact`.
const MODULE: &str = "xmr_reserve_fixture";
const MAKER_SIGNER: u8 = 0x77;
const TAKER_SIGNER: u8 = 0x78;

/// A loopback monerod answering `on_get_block_hash`, `get_block_count` and
/// `get_info` (the offline fakechain a Regtest role checks before it
/// reserves), one request per connection. It lives as long as the test
/// process.
pub fn start_fake_monerod() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut bytes = Vec::new();
            let mut buffer = [0_u8; 4_096];
            let body = loop {
                let Ok(count) = stream.read(&mut buffer) else {
                    break None;
                };
                if count == 0 {
                    break None;
                }
                bytes.extend_from_slice(&buffer[..count]);
                let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") else {
                    continue;
                };
                let length = String::from_utf8_lossy(&bytes[..end])
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())?
                    })
                    .unwrap_or(0);
                if bytes.len() >= end + 4 + length {
                    break Some(bytes[end + 4..end + 4 + length].to_vec());
                }
            };
            let Some(body) = body else { continue };
            let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
            let result = match request["method"].as_str().unwrap_or_default() {
                "on_get_block_hash" => json!(hex::encode(MONERO_GENESIS)),
                "get_block_count" => {
                    json!({"count": MONERO_TIP + 1, "status": "OK", "untrusted": false})
                }
                "get_info" => json!({
                    "status": "OK", "untrusted": false, "nettype": "fakechain",
                    "mainnet": false, "testnet": false, "stagenet": false, "offline": true,
                    "incoming_connections_count": 0, "outgoing_connections_count": 0,
                    "version": "0.18.5.1-release",
                }),
                _ => serde_json::Value::Null,
            };
            let response =
                json!({"jsonrpc": "2.0", "id": request["id"], "result": result}).to_string();
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
                response.len()
            );
            let _ = stream.write_all(reply.as_bytes());
        }
    });
    format!("http://{address}/")
}

/// A shell wrapper that runs this test binary as the ignored test `name`.
fn self_as(root: &Path, file: &str, name: &str, environment: &str, after: &str) -> PathBuf {
    let script = root.join(file);
    let mut handle = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(&script)
        .unwrap();
    writeln!(
        handle,
        "#!/bin/sh\n{environment} '{}' --exact {MODULE}::{name} --nocapture --include-ignored \
         --test-threads 1{after}",
        std::env::current_exe().unwrap().display()
    )
    .unwrap();
    script
}

/// The swap sidecar the Maker spawns (btc-role-lifecycle sidecar.rs): it
/// listens on the port the Node chose until `run` is removed.
pub fn fake_sidecar_program(root: &Path, run: &Path) -> PathBuf {
    self_as(
        root,
        "fake-sidecar.sh",
        "fake_sidecar_process",
        &format!(
            "LEZ_FAKE_SIDECAR_ARGS=\"$*\" LEZ_FAKE_SIDECAR_RUN='{}' exec",
            run.display()
        ),
        "",
    )
}

/// The Stage-A planner both roles run: one JSON input on stdin, the wire on
/// stdout. The test harness prints to stdout itself, so the child writes the
/// wire to a file the wrapper then prints.
pub fn fake_planner_program(root: &Path) -> PathBuf {
    self_as(
        root,
        "fake-planner.sh",
        "fake_planner_process",
        "out=\"$(mktemp)\"\nLEZ_FAKE_PLANNER_OUT=\"$out\"",
        " >/dev/null 2>&1 || { rm -f \"$out\"; exit 1; }\ncat \"$out\"\nrm -f \"$out\"",
    )
}

#[test]
#[ignore = "the fake LEZ sidecar the XMR process tests spawn as a process"]
fn fake_sidecar_process() {
    let Ok(arguments) = std::env::var("LEZ_FAKE_SIDECAR_ARGS") else {
        return;
    };
    let run = PathBuf::from(std::env::var("LEZ_FAKE_SIDECAR_RUN").unwrap());
    let address: SocketAddr = arguments
        .split_whitespace()
        .skip_while(|word| *word != "--listen-address")
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async move {
        let server = ServerBuilder::default().build(address).await.unwrap();
        let _handle = server.start(RpcModule::new(()));
        println!("{{\"event\":\"ready\"}}");
        while run.exists() {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    });
}

#[test]
#[ignore = "the fixture Stage-A planner the XMR process tests run as a process"]
fn fake_planner_process() {
    let Ok(output) = std::env::var("LEZ_FAKE_PLANNER_OUT") else {
        return;
    };
    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input).unwrap();
    let input: XmrStageAPlanInputV1 = serde_json::from_slice(&input).unwrap();
    fs::write(output, plan(&input)).unwrap();
}

/// A valid Stage A from the input as the real planner composes it, with
/// message hashes derived from the whole input in place of the LEZ ones, so
/// any differing fact yields a different body (xmr-reference-actor's
/// `FixturePlanner`).
fn plan(input: &XmrStageAPlanInputV1) -> Vec<u8> {
    let maker =
        ValidatedRolePacket::from_bytes(&hex::decode(&input.maker_packet).unwrap()).unwrap();
    let taker =
        ValidatedRolePacket::from_bytes(&hex::decode(&input.taker_packet).unwrap()).unwrap();
    let participants = XmrParticipantsV1::new(maker.identity().clone(), taker.identity().clone());
    let claim_key = participants.claim_aggregate_x_only_key().unwrap();
    let refund_key = participants.refund_aggregate_x_only_key().unwrap();
    let shared = MoneroSharedAddressV1::derive_from_public_view_key(
        MoneroAddressNetworkV1::Regtest,
        maker.proof(),
        taker.proof(),
        maker.public_view_key(),
    )
    .unwrap();
    let seed = serde_json::to_vec(input).unwrap();
    let message = |purpose: &[u8]| -> [u8; 32] {
        Sha256::new()
            .chain_update(&seed)
            .chain_update(purpose)
            .finalize()
            .into()
    };
    let profile = XmrNamedProfileV1::AcceleratedRegtest;
    let body = XmrAgreementBodyV1::new(
        XmrSwapDirectionV1::TakerSellsLez,
        profile,
        *input.swap_id.as_bytes(),
        participants,
        XmrMoneroTermsV1::new(
            MoneroAddressNetworkV1::Regtest,
            *input.monero_genesis_hash.as_bytes(),
            input.monero_amount_piconero,
            profile.required_monero_confirmations(),
            maker.proof().to_wire_bytes().unwrap(),
            taker.proof().to_wire_bytes().unwrap(),
            shared.public_view_key(),
            shared.public_spend_key(),
            shared.address_string(),
        ),
        XmrLezTermsV1::new(
            *input.lez_channel_id.as_bytes(),
            *input.lez_genesis_hash.as_bytes(),
            [42; 8],
            [43; 8],
            profile.required_lez_finality_units(),
            [44; 32],
            [45; 32],
            taker.identity().lez_owner_account(),
            maker.identity().lez_owner_account(),
            claim_key,
            XmrLezTermsV1::authority_account_for_key(claim_key),
            refund_key,
            XmrLezTermsV1::authority_account_for_key(refund_key),
            maker.proof().transcript_commitment(),
            taker.proof().transcript_commitment(),
            input.lez_amount,
        ),
        XmrMessagesV1::new(message(b"claim"), message(b"refund"), message(b"punish")),
        XmrWindowsV1::new(
            input.maker_xmr_funding_cutoff_ms,
            input.refund_at_ms,
            input.punish_at_ms,
        ),
    );
    ValidatedXmrAgreementBodyV1::validate(body)
        .unwrap()
        .encode_unsigned_wire()
        .unwrap()
}

/// One role's LEZ role (the Node-owned lifecycle's config) with its own
/// signer; both roles name the same LEZ chain.
pub fn btc_role_config(home: &Path, sidecar: &Path, port_base: u16, signer: u8) -> PathBuf {
    let swaps = private_dir(&home.join("swaps"));
    write_private(&home.join("cookie"), b"user:pass\n");
    write_private(
        &home.join("lez-signer.key"),
        format!("{}\n", hex::encode([signer; 32])).as_bytes(),
    );
    let path = home.join("btc-role.json");
    write_private(
        &path,
        &serde_json::to_vec(&json!({
            "schema_version": 1,
            "swaps_root": swaps,
            "bitcoin": {
                "network": "regtest",
                "endpoint": "http://127.0.0.1:18443/",
                "cookie_file": home.join("cookie"),
                "genesis_block_hash": "0f9188f13cb7b2c71f2a335e3a4fc328bf5beb436012afca590b1a11466e2206",
                "required_confirmations": 2,
                "refund_csv_blocks": 144,
                "second_lock_refund_csv_blocks": 7,
                "claim_fee_sat": 1000
            },
            "lez": {
                "channel_id": hex::encode([0x23; 32]),
                "genesis_block_hash": hex::encode([0x22; 32]),
                "escrow_program_id": hex::encode([0x24; 32]),
                "authenticated_transfer_program_id": hex::encode([0x25; 32]),
                "sidecar_program": sidecar,
                "sequencer_url": "http://127.0.0.1:3040",
                "indexer_url": "http://127.0.0.1:8779",
                "sidecar_port_base": port_base,
                "sidecar_port_count": 10,
                "signer_key_file": home.join("lez-signer.key"),
                "request_timeout_millis": 5000,
                "discovery_max_blocks": 512
            },
            "recovery": {
                "maker_second_lock_cutoff_seconds": 1800,
                "earlier_refund_latest_seconds": 3600,
                "later_refund_earliest_seconds": 7200,
                "required_margin_seconds": 300
            },
            "actor": {"program": "/usr/bin/true", "program_sha256": hex::encode([0x11; 32])}
        }))
        .unwrap(),
    );
    path
}

/// One role's Monero role config: the fake monerod, a wallet-rpc nobody
/// calls before activation, the fast windows, the fixture planner, and (for
/// the Maker) every effect tool slot pinned to a stand-in program.
pub fn xmr_role_config(home: &Path, monerod: &str, planner: &Path, wallet: &str) -> PathBuf {
    write_private(&home.join("monerod.login"), b"daemon:0123abcd\n");
    write_private(
        &home.join("wallet-rpc.login"),
        format!("{wallet}:4567cdef\n").as_bytes(),
    );
    let tool = fs::canonicalize("/usr/bin/true").unwrap();
    let path = home.join("xmr-role.json");
    write_private(
        &path,
        &serde_json::to_vec(&json!({
            "schema_version": 1,
            "profile": "accelerated-regtest",
            "monerod_endpoint": monerod,
            "monerod_login_file": home.join("monerod.login"),
            "wallet_rpc_endpoint": "http://127.0.0.1:18083/",
            "wallet_rpc_login_file": home.join("wallet-rpc.login"),
            "wallet_name": wallet,
            "funding_cutoff_seconds": 260,
            "refund_seconds": 360,
            "punish_seconds": 480,
            "stage_a_planner": planner,
            "effect_tools": {
                "monero_fund": tool,
                "lez_claim": tool,
                "finalized_classifier": tool,
                "monero_refund": tool,
                "monero_verify": tool,
                "lez_punish": tool,
            },
        }))
        .unwrap(),
    );
    path
}

/// The Maker's take-time Monero deployment: its LEZ and Monero role
/// configs, its actor root and program.
pub struct XmrMakerDeployment {
    pub swaps_root: PathBuf,
    pub actor_root: PathBuf,
    arguments: Vec<OsString>,
}

impl XmrMakerDeployment {
    /// `home` is the Maker's private directory; `run` keeps the swap
    /// sidecars alive while it exists.
    pub fn new(
        home: &Path,
        run: &Path,
        monerod: &str,
        actor_program: &Path,
        port_base: u16,
    ) -> Self {
        let tools = private_dir(&home.join("xmr-tools"));
        let sidecar = fake_sidecar_program(&tools, run);
        let planner = fake_planner_program(&tools);
        let btc_key = home.join("btc-chat-signing.key");
        write_private(&btc_key, &[0x31; 32]);
        let btc = btc_role_config(home, &sidecar, port_base, MAKER_SIGNER);
        let xmr = xmr_role_config(home, monerod, &planner, "maker");
        let actor_root = private_dir(&home.join("xmr-actors"));
        let program_sha256 = hex::encode(Sha256::digest(fs::read(actor_program).unwrap()));
        let arguments = [
            "--btc-maker-signing-key-file".into(),
            btc_key.into_os_string(),
            "--btc-role-config".into(),
            btc.into_os_string(),
            "--xmr-role-config".into(),
            xmr.into_os_string(),
            "--xmr-maker-actor-root".into(),
            actor_root.clone().into_os_string(),
            "--xmr-actor-program".into(),
            actor_program.as_os_str().to_owned(),
            "--xmr-actor-program-sha256".into(),
            program_sha256.into(),
        ]
        .into();
        Self {
            swaps_root: home.join("swaps"),
            actor_root,
            arguments,
        }
    }

    /// The Maker Node's Monero flags (and the LEZ role they need).
    pub fn arguments(&self) -> &[OsString] {
        &self.arguments
    }

    /// The reservation's directory under the Maker's swaps root.
    pub fn swap(&self, reservation_id: &RequestId) -> PathBuf {
        self.swaps_root.join(reservation_id.as_str())
    }
}

/// The Taker's side of one reservation, as its Node would run it.
pub struct FakeTaker {
    pub role_root: PathBuf,
    config: XmrRoleConfigV1,
    lez: XmrLezRole,
}

impl FakeTaker {
    /// Makes the Taker's per-swap material in `home/xmr` (a new role root).
    pub fn new(home: &Path, monerod: &str) -> Self {
        let tools = private_dir(&home.join("xmr-tools"));
        let planner = fake_planner_program(&tools);
        // Its sidecar never runs: the Taker reads only its runtime descriptor.
        let sidecar = fake_sidecar_program(&tools, &tools);
        let btc = btc_role_config(home, &sidecar, 1, TAKER_SIGNER);
        let config =
            XmrRoleConfigV1::load(&xmr_role_config(home, monerod, &planner, "taker")).unwrap();
        let lez = XmrLezRole {
            runtime: BtcRoleRuntime::load(Participant::Taker, &btc)
                .unwrap()
                .runtime_descriptor(),
        };
        let role_root = home.join("xmr");
        begin_xmr_taker_role(&role_root, *lez.runtime.signer_account_id.as_bytes()).unwrap();
        Self {
            role_root,
            config,
            lez,
        }
    }

    /// R1's request; byte-identical every time (it is reread from the role
    /// root).
    pub fn reserve_request(
        &self,
        reservation_id: &RequestId,
        offer_id: &MakerOfferId,
        signed_offer_envelope: &[u8],
        foreign_units: u64,
    ) -> XmrReserveRequestV1 {
        xmr_taker_contribution(&self.role_root)
            .unwrap()
            .reserve_request(
                reservation_id.clone(),
                offer_id.clone(),
                signed_offer_envelope.to_vec(),
                foreign_units,
            )
    }

    /// Checks the Maker's R1 answer as the Taker Node does and returns the
    /// countersigned Stage A (R2's wire).
    pub fn check(
        &self,
        response: &XmrReserveResponseV1,
        terms: &XmrReservationTerms,
        maker_identity: [u8; 33],
    ) -> anyhow::Result<Vec<u8>> {
        anyhow::ensure!(
            response.schema_version == 1 && *response.swap_id == hex::encode(terms.swap_id),
            "the Maker answered another swap"
        );
        check_xmr_taker_role(
            &self.config,
            &self.config.stage_a_planner(),
            terms,
            &self.lez,
            &self.role_root,
            &XmrMakerReservation::from(response),
            &XmrTakerCheck {
                now: now(),
                monero: XmrMoneroChainView {
                    genesis: MONERO_GENESIS,
                    tip_height: MONERO_TIP,
                },
                maker_identity,
            },
        )
    }

    /// R3–R5 over the Maker's Chat socket once R2 is done; returns the
    /// activated Stage B (R6's wire, also written into the role root) and
    /// whether the Maker answered each round as a replay. Run again, every
    /// request is byte-identical (the seats replay from the journal).
    pub async fn ceremony(
        &self,
        chat_socket: &Path,
        reservation_id: &RequestId,
    ) -> (Vec<u8>, [bool; 3]) {
        let mut taker = XmrTakerCeremony::open(&self.role_root).unwrap();
        let reserved: XmrCeremonyReserveResponseV1 = call_local_chat_rpc(
            chat_socket,
            "xmr_ceremony_reserve_v1",
            &XmrCeremonyReserveRequestV1 {
                schema_version: 1,
                request_id: round_request(reservation_id, b"ceremony-reserve"),
                reservation_id: reservation_id.clone(),
                taker_commitments: taker.commitments().unwrap().into(),
            },
        )
        .await
        .expect("R3");
        taker
            .accept_maker_commitments(&reserved.maker_commitments.into())
            .unwrap();
        let nonces: XmrCeremonyNonceResponseV1 = call_local_chat_rpc(
            chat_socket,
            "xmr_ceremony_nonce_v1",
            &XmrCeremonyNonceRequestV1 {
                schema_version: 1,
                request_id: round_request(reservation_id, b"ceremony-nonce"),
                reservation_id: reservation_id.clone(),
                taker_nonces: taker.nonces().unwrap().into(),
            },
        )
        .await
        .expect("R4");
        let refund_partial = taker
            .sign(&XmrCeremonyPackets::from(nonces.maker_nonces))
            .unwrap();
        let stage_b = taker
            .finish(&XmrCeremonyPackets::from(nonces.maker_partials))
            .unwrap();
        let partial: XmrCeremonyPartialResponseV1 = call_local_chat_rpc(
            chat_socket,
            "xmr_ceremony_partial_v1",
            &stage_b.partial_request(
                round_request(reservation_id, b"ceremony-partial"),
                reservation_id.clone(),
                refund_partial,
            ),
        )
        .await
        .expect("R5");
        let (presignature, signature) = xmr_ceremony_partial_answer(&partial).unwrap();
        (
            taker.activate(presignature, signature).unwrap(),
            [reserved.was_replay, nonces.was_replay, partial.was_replay],
        )
    }
}

/// A derived Chat request id for one round of one reservation.
pub fn round_request(reservation_id: &RequestId, label: &[u8]) -> RequestId {
    let mut digest = Sha256::new();
    digest.update(b"lez-atomic-swaps/xmr-taker-chat-request/v1\0");
    digest.update(reservation_id.as_str().as_bytes());
    digest.update([0]);
    digest.update(label);
    RequestId::new(hex::encode(digest.finalize())).unwrap()
}

pub fn private_dir(path: &Path) -> PathBuf {
    fs::DirBuilder::new().mode(0o700).create(path).unwrap();
    fs::canonicalize(path).unwrap()
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

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// One Monero agreement reached at take time with a running Maker: the
/// Taker's role-generated material, where both roles keep it, and what the
/// Maker registers at activation.
pub struct XmrChatFixture {
    pub stage_a: PathBuf,
    pub stage_b: PathBuf,
    pub taker_private_root: PathBuf,
    pub taker_public_packet: PathBuf,
    pub maker_public_packet: PathBuf,
    pub taker_journal: PathBuf,
    pub taker_actor_root: PathBuf,
    pub receipt: PathBuf,
    pub maker_actor_config: PathBuf,
    pub maker_actor_state: PathBuf,
    pub swap_id: SwapId,
    taker: FakeTaker,
}

/// The offer and reservation the fixture's Taker takes.
pub struct XmrTake<'a> {
    pub offer: &'a AuthenticatedOfferRefV1,
    pub offer_id: &'a MakerOfferId,
    pub reservation_id: &'a RequestId,
    pub maker_identity: [u8; 33],
    pub foreign_units: u64,
    pub lez_units: u128,
}

impl XmrChatFixture {
    /// R1 against the Maker behind `chat_socket`, checked as the Taker Node
    /// checks it: afterwards `stage_a` holds the countersigned Stage A.
    pub async fn reserve(
        root: &Path,
        chat_socket: &Path,
        maker: &XmrMakerDeployment,
        monerod: &str,
        take: &XmrTake<'_>,
    ) -> Self {
        let material = private_dir(&root.join("xmr-material"));
        let taker = FakeTaker::new(&private_dir(&material.join("taker")), monerod);
        let swap_id = maker_xmr_chat_swap_id(&take.offer.commitment(), take.reservation_id);
        let reserved: XmrReserveResponseV1 = call_local_chat_rpc(
            chat_socket,
            "xmr_reserve_v1",
            &taker.reserve_request(
                take.reservation_id,
                take.offer_id,
                take.offer.signed_envelope(),
                take.foreign_units,
            ),
        )
        .await
        .expect("the Maker reserves the Monero swap");
        taker
            .check(
                &reserved,
                &XmrReservationTerms {
                    swap_id,
                    amount_piconero: take.foreign_units,
                    lez_amount: take.lez_units,
                },
                take.maker_identity,
            )
            .expect("the Taker composes the Maker's Stage A");
        let role_root = taker.role_root.clone();
        let maker_swap = maker.swap(take.reservation_id);
        Self {
            stage_a: role_root.join(XMR_STAGE_A_FILE),
            stage_b: role_root.join(XMR_STAGE_B_FILE),
            taker_public_packet: role_root.join(XMR_TAKER_PACKET_FILE),
            maker_public_packet: role_root.join(XMR_MAKER_PACKET_FILE),
            taker_journal: role_root.join(XMR_ADAPTOR_JOURNAL_FILE),
            taker_private_root: role_root,
            taker_actor_root: material.join("taker-actor"),
            receipt: material.join("acceptance-receipt.json"),
            maker_actor_config: maker_swap.join("xmr-effect/actor-manifest.json"),
            maker_actor_state: maker_swap.join("xmr").join(XMR_ADAPTOR_JOURNAL_FILE),
            swap_id: SwapId::new(hex::encode(swap_id)).unwrap(),
            taker,
        }
    }

    /// R3–R5 once Stage A is staged; afterwards `stage_b` holds the activated
    /// Stage B and both role journals are complete.
    pub async fn ceremony(&self, chat_socket: &Path, reservation_id: &RequestId) {
        let (_, replays) = self.taker.ceremony(chat_socket, reservation_id).await;
        assert_eq!(replays, [false; 3]);
    }
}
