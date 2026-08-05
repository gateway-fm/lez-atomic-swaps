#![cfg(feature = "sessions")]

use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::Write as _,
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    process::{Command, Output},
};

use clap::Parser as _;
use lez_adaptor_role_runner::{Cli as RunnerCli, execute as execute_runner};
use lez_swap_core::{Participant, SwapId};
use lez_swap_store::{
    MakerActorHeldLock, SqliteXmrWorkflowJournal, XmrWorkflowBranch, XmrWorkflowDecision,
    XmrWorkflowIdentityV1, XmrWorkflowReconciliationSource, XmrWorkflowReconciliationV2,
    XmrWorkflowStep,
};
use lez_xmr_swap_sdk::{
    MoneroAddressNetworkV1, MoneroSharedAddressV1, ValidatedXmrAgreementBodyV1, XmrAgreementBodyV1,
    XmrLezTermsV1, XmrMessagesV1, XmrMoneroTermsV1, XmrNamedProfileV1, XmrParticipantsV1,
    XmrSwapDirectionV1, XmrWindowsV1,
};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;
use xmr_reference_actor::{
    ActorRole, ValidatedRolePacket, XMR_EFFECT_OBSERVER_RESULT_MAX_BYTES, XmrEffectChildModeV1,
    XmrEffectObserverStateV1, XmrPreparedEffectInvocationV1,
    load_validated_xmr_effect_execution_v3_bytes, parse_xmr_effect_child_plan_v1,
    parse_xmr_effect_observer_result_v1, provision_xmr_maker_actor_from_material,
    provision_xmr_taker_actor_from_material, publish_xmr_effect_manifest_v3,
};

const RUN_ID: &str = "m5-xmr-tag14-effect-route-red";
const TAKER_OWNER: &str = "1515151515151515151515151515151515151515151515151515151515151515";
const MAKER_OWNER: &str = "2424242424242424242424242424242424242424242424242424242424242424";
const WORKER: &[u8] = br#"#!/bin/sh
set -eu
for fd in 197 198 199 200 201 202 203 204 205 206 207 208 209 210 211 212 213 214 215 216 217; do
    test -e "/proc/self/fd/$fd"
done
test ! -e /proc/self/fd/218
grep -Fq '"mode":"invoke"' /proc/self/fd/217
if test "${XMR_TEST_EMIT_APPLICATION_HASHES:-}" = "1"; then
    for fd in 211 212 213 214 215 216; do
        sha256sum "/proc/self/fd/$fd" | cut -d ' ' -f 1
    done
    cat /proc/self/fd/217
fi
"#;
const MONERO_WORKER: &[u8] = br#"#!/bin/sh
set -eu
for fd in 197 198 199 200 201 202 203 204 205 206 207 208 209 210 211 212 213 214 215 216 217 218; do
    test -e "/proc/self/fd/$fd"
done
test ! -e /proc/self/fd/219
grep -Fq '"mode":"invoke"' /proc/self/fd/217
grep -Fq '"step":"sweep_monero_claim"' /proc/self/fd/217
grep -Fq '"executable_abi":"lez_xmr_monero_claim_sweep_v2"' /proc/self/fd/217
"#;
const REFUND_WORKER: &[u8] = br#"#!/bin/sh
set -eu
for fd in 197 198 199 200 201 202 203 204 205 206 207 208 209 210 211 212 213 214 215 216 217 218; do
    test -e "/proc/self/fd/$fd"
done
test ! -e /proc/self/fd/219
if grep -Fq '"mode":"preflight"' /proc/self/fd/217; then
    test "${XMR_TEST_PREFLIGHT_FAIL:-}" != "1"
    exit 0
fi
grep -Fq '"mode":"invoke"' /proc/self/fd/217
grep -Fq '"step":"refund_lez_tag16"' /proc/self/fd/217
grep -Fq '"executable_abi":"lez_xmr_tag16_refund_v1"' /proc/self/fd/217
sha256sum /proc/self/fd/218 | cut -d ' ' -f 1
"#;
const MAKER_REFUND_WORKER: &[u8] = br#"#!/bin/sh
set -eu
for fd in 197 198 199 200 201 202 203 204 205 206 207 208 209 210 211 212 213 214 215 216 217 218 219; do
    test -e "/proc/self/fd/$fd"
done
grep -Fq '"mode":"invoke"' /proc/self/fd/217
grep -Fq '"step":"sweep_monero_refund"' /proc/self/fd/217
grep -Fq '"executable_abi":"lez_xmr_monero_refund_sweep_v3"' /proc/self/fd/217
sha256sum /proc/self/fd/218 | cut -d ' ' -f 1
sha256sum /proc/self/fd/219 | cut -d ' ' -f 1
"#;
const RELEASE_WORKER: &[u8] = br#"#!/bin/sh
set -eu
test "$#" -eq 0
for fd in 197 198 199 220 221 222 223; do
    test -e "/proc/self/fd/$fd"
done
for fd in 200 201 202 203 204 205 206 207 208 209 210 211 212 213 214 215 216 217 218 219; do
    test ! -e "/proc/self/fd/$fd"
done
grep -Fq '"schema_version":2' /proc/self/fd/220
grep -Fq '"sidecar_endpoint":"http://127.0.0.1:32978/"' /proc/self/fd/220
grep -Fq '"indexer_endpoint":"http://127.0.0.1:32979/"' /proc/self/fd/220
if grep -Fq '"mode":"preflight"' /proc/self/fd/220; then
    test "${XMR_TEST_PREFLIGHT_FAIL:-}" != "1"
    exit 0
fi
grep -Fq '"mode":"invoke"' /proc/self/fd/220
"#;
const PUNISH_WORKER: &[u8] = br#"#!/bin/sh
set -eu
for fd in 197 198 199 200 201 202 203 204 205 206 207 208 209 210 211 212 213 214 215 216 217; do
    test -e "/proc/self/fd/$fd"
done
test ! -e /proc/self/fd/218
grep -Fq "\"step\":\"punish_lez_tag17\"" /proc/self/fd/217
grep -Fq "\"executable_abi\":\"lez_xmr_tag17_punish_v1\"" /proc/self/fd/217
if grep -Fq "\"mode\":\"preflight\"" /proc/self/fd/217; then
    exit 0
fi
grep -Fq "\"mode\":\"invoke\"" /proc/self/fd/217
"#;
const OBSERVER: &[u8] = br#"#!/bin/sh
set -eu
test "$#" -eq 2
test "$1" = "--xmr-workflow-step"
for fd in 197 198 199 200 201 202 203 204 205 206 207 208 209 210 211 212 213 214 215 216 217; do
    test -e "/proc/self/fd/$fd"
done
test ! -e /proc/self/fd/218
test ! -e /proc/self/fd/219
grep -Fq '"mode":"observe"' /proc/self/fd/217
grep -Fq "\"step\":\"$2\"" /proc/self/fd/217
printf '{"schema_version":1,"step":"%s","state":"pending"}\n' "$2"
"#;

#[derive(Serialize)]
struct ToolFixture {
    program: PathBuf,
    program_sha256: String,
    abi: &'static str,
}

#[derive(Serialize)]
struct MakerToolsFixture {
    monero_fund: ToolFixture,
    lez_claim: ToolFixture,
    finalized_classifier: ToolFixture,
    monero_refund: ToolFixture,
    monero_verify: ToolFixture,
    lez_punish: ToolFixture,
}

#[derive(Serialize)]
struct TakerToolsFixture {
    tag14_authorize: ToolFixture,
    finalized_classifier: ToolFixture,
    monero_claim: ToolFixture,
    monero_verify: ToolFixture,
    tag16_refund: ToolFixture,
}

#[derive(Serialize)]
struct LezFixture {
    sidecar_url: String,
    runtime_file: PathBuf,
    runtime_sha256: String,
    capability_file: PathBuf,
}

#[derive(Serialize)]
struct RpcFixture {
    url: String,
    username_file: PathBuf,
    password_file: PathBuf,
}

#[derive(Serialize)]
struct MoneroFixture {
    daemon: RpcFixture,
    funding_wallet: RpcFixture,
    shared_wallet: RpcFixture,
    role_wallet: RpcFixture,
    shared_wallet_file_password_file: PathBuf,
}

#[derive(Serialize)]
struct Tag14ReleaseFixture {
    sidecar_url: String,
    indexer_url: String,
    node_profile: &'static str,
    state_directory: PathBuf,
    capability_file: PathBuf,
    protection_key_file: PathBuf,
    protection_key_id: &'static str,
}

#[derive(Serialize)]
struct EffectAuthorityFixture {
    schema_version: u16,
    pair: &'static str,
    role: ActorRole,
    swap_id: String,
    agreement_commitment: String,
    activation_commitment: String,
    run_id: &'static str,
    workflow_journal: PathBuf,
    adaptor_journal: PathBuf,
    evidence_root: PathBuf,
    lez: LezFixture,
    monero: MoneroFixture,
    #[serde(skip_serializing_if = "Option::is_none")]
    maker_tools: Option<MakerToolsFixture>,
    #[serde(skip_serializing_if = "Option::is_none")]
    taker_tools: Option<TakerToolsFixture>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tag14_release: Option<Tag14ReleaseFixture>,
}

struct MaterialFixture {
    _root: TempDir,
    material: PathBuf,
    exchange: PathBuf,
    maker_root: PathBuf,
    taker_root: PathBuf,
    maker_packet: PathBuf,
    taker_packet: PathBuf,
    unsigned_stage_a: PathBuf,
}

struct RouteFixture {
    _material: MaterialFixture,
    swap_id: SwapId,
    workflow: PathBuf,
    actor_state: PathBuf,
    worker: PathBuf,
    effect_bytes: Vec<u8>,
    manifest_bytes: Vec<u8>,
    application_sha256: Vec<String>,
    private_xmr_share_sha256: String,
    refund_signature_sha256: Option<String>,
}

fn actor_binary() -> &'static str {
    env!("CARGO_BIN_EXE_xmr-reference-actor")
}

fn owner_directory(parent: &Path, name: &str) -> PathBuf {
    let path = parent.join(name);
    fs::create_dir(&path).expect("create owner directory");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
        .expect("set owner directory mode");
    path
}

fn write_private(path: &Path, bytes: &[u8], mode: u32) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)
        .expect("create owner-private fixture");
    file.write_all(bytes).expect("write owner-private fixture");
    file.sync_all().expect("sync owner-private fixture");
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("set fixture mode");
}

fn assert_success(output: &Output, label: &str) {
    assert!(
        output.status.success(),
        "{label} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty(), "{label} wrote stdout");
    assert!(output.stderr.is_empty(), "{label} wrote stderr");
}

fn provision_material() -> MaterialFixture {
    let root = TempDir::new().expect("temporary material root");
    let material = owner_directory(root.path(), "material");
    let exchange = owner_directory(root.path(), "exchange");
    let maker_root = material.join("maker");
    let taker_root = material.join("taker");
    let maker_packet = exchange.join("maker.json");
    let taker_packet = exchange.join("taker.json");

    let taker = Command::new(actor_binary())
        .args(["provision", "taker", "--private-root"])
        .arg(&taker_root)
        .args(["--lez-owner-account", TAKER_OWNER, "--public-packet"])
        .arg(&taker_packet)
        .output()
        .expect("spawn Taker provision");
    assert_success(&taker, "Taker provision");
    let maker = Command::new(actor_binary())
        .args(["provision", "maker", "--private-root"])
        .arg(&maker_root)
        .args(["--lez-owner-account", MAKER_OWNER, "--shared-view-key-file"])
        .arg(taker_root.join("monero-view.key"))
        .arg("--public-packet")
        .arg(&maker_packet)
        .output()
        .expect("spawn Maker provision");
    assert_success(&maker, "Maker provision");

    let unsigned_stage_a = exchange.join("unsigned-stage-a.bin");
    write_private(
        &unsigned_stage_a,
        &unsigned_stage_a_wire(&maker_packet, &taker_packet),
        0o600,
    );
    MaterialFixture {
        _root: root,
        material,
        exchange,
        maker_root,
        taker_root,
        maker_packet,
        taker_packet,
        unsigned_stage_a,
    }
}

fn unsigned_stage_a_wire(maker_path: &Path, taker_path: &Path) -> Vec<u8> {
    let maker = ValidatedRolePacket::read(maker_path).expect("Maker packet");
    let taker = ValidatedRolePacket::read(taker_path).expect("Taker packet");
    let participants = XmrParticipantsV1::new(maker.identity().clone(), taker.identity().clone());
    let claim_key = participants
        .claim_aggregate_x_only_key()
        .expect("claim aggregate key");
    let refund_key = participants
        .refund_aggregate_x_only_key()
        .expect("refund aggregate key");
    let shared = MoneroSharedAddressV1::derive_from_public_view_key(
        MoneroAddressNetworkV1::Regtest,
        maker.proof(),
        taker.proof(),
        maker.public_view_key(),
    )
    .expect("shared address");
    let body = XmrAgreementBodyV1::new(
        XmrSwapDirectionV1::TakerSellsLez,
        XmrNamedProfileV1::AcceleratedRegtest,
        [19; 32],
        participants,
        XmrMoneroTermsV1::new(
            MoneroAddressNetworkV1::Regtest,
            [31; 32],
            1_000_000_000_000,
            XmrNamedProfileV1::AcceleratedRegtest.required_monero_confirmations(),
            maker.proof().to_wire_bytes().expect("Maker DLEQ wire"),
            taker.proof().to_wire_bytes().expect("Taker DLEQ wire"),
            shared.public_view_key(),
            shared.public_spend_key(),
            shared.address_string(),
        ),
        XmrLezTermsV1::new(
            [40; 32],
            [41; 32],
            [42; 8],
            [43; 8],
            XmrNamedProfileV1::AcceleratedRegtest.required_lez_finality_units(),
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
            500,
        ),
        XmrMessagesV1::new([51; 32], [52; 32], [53; 32]),
        XmrWindowsV1::new(10_000, 20_000, 30_000),
    );
    ValidatedXmrAgreementBodyV1::validate(body)
        .expect("valid Stage-A body")
        .encode_unsigned_wire()
        .expect("unsigned Stage-A wire")
}

fn signed_stage_a(fixture: &MaterialFixture) -> PathBuf {
    let maker_signature = fixture.exchange.join("maker-stage-a.sig");
    let taker_signature = fixture.exchange.join("taker-stage-a.sig");
    for (role, root, own, peer, output) in [
        (
            "maker",
            &fixture.maker_root,
            &fixture.maker_packet,
            &fixture.taker_packet,
            &maker_signature,
        ),
        (
            "taker",
            &fixture.taker_root,
            &fixture.taker_packet,
            &fixture.maker_packet,
            &taker_signature,
        ),
    ] {
        let signed = Command::new(actor_binary())
            .args(["sign-stage-a", role, "--private-root"])
            .arg(root)
            .arg("--own-public-packet")
            .arg(own)
            .arg("--peer-public-packet")
            .arg(peer)
            .arg("--unsigned-stage-a")
            .arg(&fixture.unsigned_stage_a)
            .arg("--output-signature")
            .arg(output)
            .output()
            .expect("spawn Stage-A signer");
        assert_success(&signed, "Stage-A signer");
    }
    let agreement = fixture.exchange.join("agreement-stage-a.bin");
    let assembled = Command::new(actor_binary())
        .arg("assemble-stage-a")
        .arg("--maker-public-packet")
        .arg(&fixture.maker_packet)
        .arg("--taker-public-packet")
        .arg(&fixture.taker_packet)
        .arg("--unsigned-stage-a")
        .arg(&fixture.unsigned_stage_a)
        .arg("--maker-signature")
        .arg(&maker_signature)
        .arg("--taker-signature")
        .arg(&taker_signature)
        .arg("--output-stage-a")
        .arg(&agreement)
        .output()
        .expect("spawn Stage-A assembler");
    assert_success(&assembled, "Stage-A assembler");
    agreement
}

fn initialize_sessions(fixture: &MaterialFixture, agreement: &Path, role: &str) -> PathBuf {
    let (root, own, peer) = match role {
        "maker" => (
            &fixture.maker_root,
            &fixture.maker_packet,
            &fixture.taker_packet,
        ),
        "taker" => (
            &fixture.taker_root,
            &fixture.taker_packet,
            &fixture.maker_packet,
        ),
        _ => panic!("invalid role"),
    };
    let sessions = fixture.material.join(format!("{role}-sessions"));
    let initialized = Command::new(actor_binary())
        .args(["initialize-sessions", role, "--private-root"])
        .arg(root)
        .arg("--own-public-packet")
        .arg(own)
        .arg("--peer-public-packet")
        .arg(peer)
        .arg("--agreement-stage-a")
        .arg(agreement)
        .arg("--session-root")
        .arg(&sessions)
        .output()
        .expect("spawn session initializer");
    assert_success(&initialized, "session initializer");
    sessions
}

fn runner(role: &str, journal: &Path, session: &Path, action: Vec<OsString>) {
    let mut arguments = vec![
        OsString::from("lez-adaptor-role-runner"),
        OsString::from(role),
        OsString::from("--journal"),
        journal.as_os_str().to_owned(),
        OsString::from("--session"),
        session.as_os_str().to_owned(),
    ];
    arguments.extend(action);
    let cli = RunnerCli::try_parse_from(arguments).expect("parse role runner");
    execute_runner(&cli).expect("execute role runner");
}

#[allow(clippy::too_many_lines)]
fn adaptor_round(
    fixture: &MaterialFixture,
    purpose: &str,
    maker_session: &Path,
    taker_session: &Path,
    maker_journal: &Path,
    taker_journal: &Path,
) {
    let exchange = owner_directory(&fixture.exchange, &format!("{purpose}-round"));
    let maker_commitment = exchange.join("maker-commitment.json");
    let taker_commitment = exchange.join("taker-commitment.json");
    let maker_nonce = exchange.join("maker-nonce.json");
    let taker_nonce = exchange.join("taker-nonce.json");
    let maker_partial = exchange.join("maker-partial.json");
    let taker_partial = fixture.taker_root.join(format!("{purpose}-partial.json"));

    for (role, journal, session, key, output) in [
        (
            "maker",
            maker_journal,
            maker_session,
            fixture.maker_root.join(format!("{purpose}.key")),
            &maker_commitment,
        ),
        (
            "taker",
            taker_journal,
            taker_session,
            fixture.taker_root.join(format!("{purpose}.key")),
            &taker_commitment,
        ),
    ] {
        runner(
            role,
            journal,
            session,
            vec![
                "reserve".into(),
                "--secret-key-file".into(),
                key.into_os_string(),
                "--output".into(),
                output.as_os_str().to_owned(),
            ],
        );
    }
    for (role, journal, session, input) in [
        ("maker", maker_journal, maker_session, &taker_commitment),
        ("taker", taker_journal, taker_session, &maker_commitment),
    ] {
        runner(
            role,
            journal,
            session,
            vec![
                "accept-commitment".into(),
                "--input".into(),
                input.as_os_str().to_owned(),
            ],
        );
    }
    for (role, journal, session, output) in [
        ("maker", maker_journal, maker_session, &maker_nonce),
        ("taker", taker_journal, taker_session, &taker_nonce),
    ] {
        runner(
            role,
            journal,
            session,
            vec![
                "reveal-nonce".into(),
                "--output".into(),
                output.as_os_str().to_owned(),
            ],
        );
    }
    for (role, journal, session, input, key, output) in [
        (
            "maker",
            maker_journal,
            maker_session,
            &taker_nonce,
            fixture.maker_root.join(format!("{purpose}.key")),
            &maker_partial,
        ),
        (
            "taker",
            taker_journal,
            taker_session,
            &maker_nonce,
            fixture.taker_root.join(format!("{purpose}.key")),
            &taker_partial,
        ),
    ] {
        runner(
            role,
            journal,
            session,
            vec![
                "accept-nonce-sign".into(),
                "--input".into(),
                input.as_os_str().to_owned(),
                "--secret-key-file".into(),
                key.into_os_string(),
                "--output".into(),
                output.as_os_str().to_owned(),
            ],
        );
    }
    runner(
        "taker",
        taker_journal,
        taker_session,
        vec![
            "accept-peer-partial".into(),
            "--input".into(),
            maker_partial.as_os_str().to_owned(),
            "--output".into(),
            exchange.join("taker-presignature.json").into_os_string(),
        ],
    );
    if purpose == "refund" {
        runner(
            "maker",
            maker_journal,
            maker_session,
            vec![
                "accept-peer-partial".into(),
                "--input".into(),
                taker_partial.as_os_str().to_owned(),
                "--output".into(),
                exchange.join("maker-presignature.json").into_os_string(),
            ],
        );
    }
}

fn activated_stage_b(
    fixture: &MaterialFixture,
    agreement: &Path,
    maker_sessions: &Path,
    taker_sessions: &Path,
) -> (PathBuf, PathBuf, PathBuf) {
    let journals = owner_directory(&fixture.material, "journals");
    let maker_journal = journals.join("maker.sqlite");
    let taker_journal = journals.join("taker.sqlite");
    adaptor_round(
        fixture,
        "claim",
        &maker_sessions.join("claim.json"),
        &taker_sessions.join("claim.json"),
        &maker_journal,
        &taker_journal,
    );
    adaptor_round(
        fixture,
        "refund",
        &maker_sessions.join("refund.json"),
        &taker_sessions.join("refund.json"),
        &maker_journal,
        &taker_journal,
    );

    let unsigned = fixture.exchange.join("unsigned-stage-b.bin");
    let composed = Command::new(actor_binary())
        .arg("compose-stage-b")
        .arg("--private-root")
        .arg(&fixture.taker_root)
        .arg("--own-public-packet")
        .arg(&fixture.taker_packet)
        .arg("--peer-public-packet")
        .arg(&fixture.maker_packet)
        .arg("--agreement-stage-a")
        .arg(agreement)
        .arg("--journal")
        .arg(&taker_journal)
        .arg("--output-unsigned-stage-b")
        .arg(&unsigned)
        .output()
        .expect("spawn Stage-B composer");
    assert_success(&composed, "Stage-B composer");

    let maker_signature = fixture.exchange.join("maker-stage-b.sig");
    let taker_signature = fixture.exchange.join("taker-stage-b.sig");
    for (role, root, own, peer, output) in [
        (
            "maker",
            &fixture.maker_root,
            &fixture.maker_packet,
            &fixture.taker_packet,
            &maker_signature,
        ),
        (
            "taker",
            &fixture.taker_root,
            &fixture.taker_packet,
            &fixture.maker_packet,
            &taker_signature,
        ),
    ] {
        let signed = Command::new(actor_binary())
            .args(["sign-stage-b", role, "--private-root"])
            .arg(root)
            .arg("--own-public-packet")
            .arg(own)
            .arg("--peer-public-packet")
            .arg(peer)
            .arg("--agreement-stage-a")
            .arg(agreement)
            .arg("--unsigned-stage-b")
            .arg(&unsigned)
            .arg("--output-signature")
            .arg(output)
            .output()
            .expect("spawn Stage-B signer");
        assert_success(&signed, "Stage-B signer");
    }

    let activated = fixture.exchange.join("activated-stage-b.bin");
    let assembled = Command::new(actor_binary())
        .args(["assemble-stage-b", "taker", "--private-root"])
        .arg(&fixture.taker_root)
        .arg("--own-public-packet")
        .arg(&fixture.taker_packet)
        .arg("--peer-public-packet")
        .arg(&fixture.maker_packet)
        .arg("--agreement-stage-a")
        .arg(agreement)
        .arg("--unsigned-stage-b")
        .arg(&unsigned)
        .arg("--maker-signature")
        .arg(&maker_signature)
        .arg("--taker-signature")
        .arg(&taker_signature)
        .arg("--output-stage-b")
        .arg(&activated)
        .output()
        .expect("spawn Stage-B assembler");
    assert_success(&assembled, "Stage-B assembler");
    (activated, maker_journal, taker_journal)
}

fn tool(root: &Path, name: &str, digest: [u8; 32], abi: &'static str) -> ToolFixture {
    ToolFixture {
        program: root.join(name),
        program_sha256: hex::encode(digest),
        abi,
    }
}

fn write_effect_inputs(root: &Path, role: &str) -> (PathBuf, PathBuf, Vec<PathBuf>) {
    let runtime = root.join("runtime.json");
    let capability = root.join("lez.capability");
    write_private(
        &runtime,
        format!("{{\"role\":\"{role}\"}}\n").as_bytes(),
        0o600,
    );
    write_private(&capability, b"capability\n", 0o600);
    let secrets = [
        "daemon.username",
        "daemon.password",
        "funding.username",
        "funding.password",
        "shared.username",
        "shared.password",
        "taker.username",
        "taker.password",
        "shared-wallet-file.password",
    ]
    .map(|name| root.join(name))
    .to_vec();
    for (index, path) in secrets.iter().enumerate() {
        write_private(path, format!("secret-{index}\n").as_bytes(), 0o600);
    }
    (runtime, capability, secrets)
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "one serialized authority fixture keeps every role-fixed path and semantic profile visible"
)]
fn effect_authority(
    root: &Path,
    worker: &Path,
    workflow: &Path,
    actor_state: &Path,
    swap: [u8; 32],
    agreement: [u8; 32],
    activation: [u8; 32],
    semantic_tag14: bool,
) -> Vec<u8> {
    let (runtime, capability, secrets) = write_effect_inputs(root, "taker");
    if semantic_tag14 {
        let runtime_descriptor = lez_bridge_protocol::RuntimeDescriptor::new(
            lez_bridge_protocol::Participant::Taker,
            lez_bridge_protocol::RuntimeCompatibility::LeeV0_2_0,
            lez_bridge_protocol::Hex32::from_bytes([60; 32]),
            lez_bridge_protocol::Hex32::from_bytes([40; 32]),
            lez_bridge_protocol::Hex32::from_bytes([41; 32]),
            lez_bridge_protocol::Hex32::from_bytes([42, 0, 0, 0].repeat(8).try_into().unwrap()),
            lez_bridge_protocol::Hex32::from_bytes([0x15; 32]),
        );
        let mut runtime_bytes = serde_json::to_vec(&runtime_descriptor).expect("runtime JSON");
        runtime_bytes.push(b'\n');
        fs::write(&runtime, runtime_bytes).expect("replace runtime fixture");
    }
    let classifier = root.join("classifier");
    let claim_sweep = root.join("claim-sweep");
    let monero_verify = root.join("monero-verify");
    let tag16_refund = root.join("tag16-refund");
    write_private(&classifier, OBSERVER, 0o700);
    write_private(&claim_sweep, MONERO_WORKER, 0o700);
    write_private(&monero_verify, OBSERVER, 0o700);
    write_private(&tag16_refund, REFUND_WORKER, 0o700);
    let rpc_at = |_name: &str, port: u16, index: usize| RpcFixture {
        url: format!("http://127.0.0.1:{port}/"),
        username_file: secrets[index].clone(),
        password_file: secrets[index + 1].clone(),
    };
    let release_state = semantic_tag14.then(|| owner_directory(root, "tag14-release"));
    let release_capability = root.join("tag14-release.capability");
    let release_key = root.join("tag14-release.key");
    if semantic_tag14 {
        write_private(&release_capability, b"release-capability", 0o600);
        write_private(
            &release_key,
            b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            0o600,
        );
    }
    let runtime_bytes = fs::read(&runtime).expect("runtime fixture");
    let fixture = EffectAuthorityFixture {
        schema_version: if semantic_tag14 { 2 } else { 1 },
        pair: "monero",
        role: ActorRole::Taker,
        swap_id: hex::encode(swap),
        agreement_commitment: hex::encode(agreement),
        activation_commitment: hex::encode(activation),
        run_id: RUN_ID,
        workflow_journal: workflow.to_path_buf(),
        adaptor_journal: actor_state.to_path_buf(),
        evidence_root: root.join("evidence"),
        lez: LezFixture {
            sidecar_url: "http://127.0.0.1:32972/".to_owned(),
            runtime_sha256: hex::encode(Sha256::digest(&runtime_bytes)),
            runtime_file: runtime,
            capability_file: capability,
        },
        monero: MoneroFixture {
            daemon: rpc_at("daemon", 32974, 0),
            funding_wallet: rpc_at("funding", 32975, 2),
            shared_wallet: rpc_at("shared", 32976, 4),
            role_wallet: rpc_at("taker", 32977, 6),
            shared_wallet_file_password_file: secrets[8].clone(),
        },
        maker_tools: None,
        taker_tools: Some(TakerToolsFixture {
            tag14_authorize: tool(
                root,
                worker
                    .file_name()
                    .and_then(|name| name.to_str())
                    .expect("ASCII worker name"),
                Sha256::digest(if semantic_tag14 {
                    RELEASE_WORKER
                } else {
                    WORKER
                })
                .into(),
                if semantic_tag14 {
                    "lez_xmr_tag14_release_v2"
                } else {
                    "lez_xmr_tag14_authorize_v1"
                },
            ),
            finalized_classifier: tool(
                root,
                "classifier",
                Sha256::digest(OBSERVER).into(),
                "lez_xmr_finalized_classifier_v1",
            ),
            monero_claim: tool(
                root,
                "claim-sweep",
                Sha256::digest(MONERO_WORKER).into(),
                "lez_xmr_monero_claim_sweep_v2",
            ),
            monero_verify: tool(
                root,
                "monero-verify",
                Sha256::digest(OBSERVER).into(),
                "lez_xmr_monero_verify_v2",
            ),
            tag16_refund: tool(
                root,
                "tag16-refund",
                Sha256::digest(REFUND_WORKER).into(),
                "lez_xmr_tag16_refund_v1",
            ),
        }),
        tag14_release: release_state.map(|state_directory| Tag14ReleaseFixture {
            sidecar_url: "http://127.0.0.1:32978/".to_owned(),
            indexer_url: "http://127.0.0.1:32979/".to_owned(),
            node_profile: "local",
            state_directory,
            capability_file: release_capability,
            protection_key_file: release_key,
            protection_key_id: "taker-release-key-1",
        }),
    };
    let mut bytes = serde_json::to_vec(&fixture).expect("serialize effect authority");
    bytes.push(b'\n');
    bytes
}

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "Maker fixture keeps the complete schema-v3 authority visible"
)]
fn maker_effect_authority(
    root: &Path,
    worker: &Path,
    workflow: &Path,
    actor_state: &Path,
    swap: [u8; 32],
    agreement: [u8; 32],
    activation: [u8; 32],
) -> Vec<u8> {
    let (runtime, capability, secrets) = write_effect_inputs(root, "maker");
    let classifier = root.join("classifier");
    write_private(&classifier, OBSERVER, 0o700);
    let refund_worker = root.join("maker-refund-sweep");
    write_private(&refund_worker, MAKER_REFUND_WORKER, 0o700);
    let monero_verify = root.join("maker-monero-verify");
    write_private(&monero_verify, OBSERVER, 0o700);
    let _evidence = owner_directory(root, "evidence");
    let rpc_at = |port: u16, index: usize| RpcFixture {
        url: format!("http://127.0.0.1:{port}/"),
        username_file: secrets[index].clone(),
        password_file: secrets[index + 1].clone(),
    };
    let runtime_bytes = fs::read(&runtime).expect("Maker runtime fixture");
    let worker_name = worker
        .file_name()
        .and_then(|name| name.to_str())
        .expect("ASCII Tag17 worker name");
    let fixture = EffectAuthorityFixture {
        schema_version: 3,
        pair: "monero",
        role: ActorRole::Maker,
        swap_id: hex::encode(swap),
        agreement_commitment: hex::encode(agreement),
        activation_commitment: hex::encode(activation),
        run_id: RUN_ID,
        workflow_journal: workflow.to_path_buf(),
        adaptor_journal: actor_state.to_path_buf(),
        evidence_root: root.join("evidence"),
        lez: LezFixture {
            sidecar_url: "http://127.0.0.1:32972/".to_owned(),
            runtime_file: runtime,
            runtime_sha256: hex::encode(Sha256::digest(runtime_bytes)),
            capability_file: capability,
        },
        monero: MoneroFixture {
            daemon: rpc_at(32974, 0),
            funding_wallet: rpc_at(32975, 2),
            shared_wallet: rpc_at(32976, 4),
            role_wallet: rpc_at(32977, 6),
            shared_wallet_file_password_file: secrets[8].clone(),
        },
        maker_tools: Some(MakerToolsFixture {
            monero_fund: tool(
                root,
                "maker-monero-fund",
                [0x50; 32],
                "lez_xmr_monero_fund_v2",
            ),
            lez_claim: tool(root, "maker-tag15", [0x55; 32], "lez_xmr_tag15_claim_v1"),
            finalized_classifier: tool(
                root,
                "classifier",
                Sha256::digest(OBSERVER).into(),
                "lez_xmr_finalized_classifier_v1",
            ),
            monero_refund: tool(
                root,
                "maker-refund-sweep",
                Sha256::digest(MAKER_REFUND_WORKER).into(),
                "lez_xmr_monero_refund_sweep_v3",
            ),
            monero_verify: tool(
                root,
                "maker-monero-verify",
                Sha256::digest(OBSERVER).into(),
                "lez_xmr_monero_verify_v2",
            ),
            lez_punish: tool(
                root,
                worker_name,
                Sha256::digest(PUNISH_WORKER).into(),
                "lez_xmr_tag17_punish_v1",
            ),
        }),
        taker_tools: None,
        tag14_release: None,
    };
    let mut bytes = serde_json::to_vec(&fixture).expect("serialize Maker effect authority");
    bytes.push(b'\n');
    bytes
}

fn complete_step(
    workflow: &mut SqliteXmrWorkflowJournal,
    identity: &XmrWorkflowIdentityV1,
    step: XmrWorkflowStep,
    byte: u8,
) {
    workflow
        .prepare_step(identity, step)
        .expect("prepare predecessor");
    assert_eq!(
        workflow.authorize_once(identity, step).unwrap(),
        XmrWorkflowDecision::InvokeOnce
    );
    let evidence = XmrWorkflowReconciliationV2::new(
        [byte; 32],
        [byte.wrapping_add(1); 32],
        XmrWorkflowReconciliationSource::LezFinalizedEvent,
    )
    .expect("valid predecessor evidence");
    workflow
        .reconcile_succeeded(identity, step, &evidence)
        .expect("reconcile predecessor");
}

#[allow(clippy::too_many_lines)]
fn route_fixture() -> RouteFixture {
    route_fixture_for(XmrWorkflowBranch::Claim, XmrWorkflowStep::AuthorizeLezTag14)
}

fn refund_route_fixture() -> RouteFixture {
    route_fixture_for(XmrWorkflowBranch::Refund, XmrWorkflowStep::RefundLezTag16)
}

#[allow(clippy::too_many_lines)]
fn route_fixture_for(
    terminal_branch: XmrWorkflowBranch,
    terminal_step: XmrWorkflowStep,
) -> RouteFixture {
    route_fixture_for_mode(terminal_branch, terminal_step, false)
}

fn semantic_release_route_fixture() -> RouteFixture {
    route_fixture_for_mode(
        XmrWorkflowBranch::Claim,
        XmrWorkflowStep::AuthorizeLezTag14,
        true,
    )
}

fn maker_punish_route_fixture() -> RouteFixture {
    maker_recovery_route_fixture(XmrWorkflowBranch::Punish, XmrWorkflowStep::PunishLezTag17)
}

fn maker_refund_route_fixture() -> RouteFixture {
    maker_recovery_route_fixture(
        XmrWorkflowBranch::Refund,
        XmrWorkflowStep::SweepMoneroRefund,
    )
}

#[allow(clippy::too_many_lines)]
fn maker_recovery_route_fixture(branch: XmrWorkflowBranch, step: XmrWorkflowStep) -> RouteFixture {
    let material = provision_material();
    let agreement = signed_stage_a(&material);
    let maker_sessions = initialize_sessions(&material, &agreement, "maker");
    let taker_sessions = initialize_sessions(&material, &agreement, "taker");
    let (activation, maker_journal, _taker_journal) =
        activated_stage_b(&material, &agreement, &maker_sessions, &taker_sessions);

    let actors = owner_directory(&material.material, "actors");
    let maker_actor = provision_xmr_maker_actor_from_material(
        &material.maker_root,
        &material.maker_packet,
        &material.taker_packet,
        &agreement,
        &activation,
        &maker_journal,
        &actors.join("maker"),
    )
    .expect("provision Maker application actor");
    let swap_bytes = maker_actor.swap_id();
    let swap_id = SwapId::new(hex::encode(swap_bytes)).expect("canonical swap ID");
    let workflow = maker_actor
        .state_directory()
        .join("xmr-effect-workflow.sqlite3");
    let effect_root = owner_directory(&material.material, "effect-inputs");
    let worker = match step {
        XmrWorkflowStep::PunishLezTag17 => {
            let worker = effect_root.join("tag17-worker");
            write_private(&worker, PUNISH_WORKER, 0o700);
            worker
        }
        XmrWorkflowStep::SweepMoneroRefund => effect_root.join("maker-refund-sweep"),
        _ => panic!("unsupported Maker recovery fixture step"),
    };
    let effect_bytes = maker_effect_authority(
        &effect_root,
        &worker,
        &workflow,
        &maker_journal,
        swap_bytes,
        maker_actor.agreement_commitment(),
        maker_actor.activation_commitment(),
    );
    let refund_signature_sha256 = (step == XmrWorkflowStep::SweepMoneroRefund).then(|| {
        let bytes = b"canonical-finalized-refund-signature\n";
        let path = effect_root
            .join("evidence")
            .join("finalized-refund-signature.json");
        write_private(&path, bytes, 0o600);
        hex::encode(Sha256::digest(bytes))
    });
    let authority_digest: [u8; 32] = Sha256::digest(&effect_bytes).into();
    let identity = XmrWorkflowIdentityV1::new(
        swap_id.clone(),
        Participant::Maker,
        RUN_ID.into(),
        maker_actor.agreement_commitment(),
        maker_actor.activation_commitment(),
        authority_digest,
    )
    .expect("valid Maker workflow identity");
    let mut journal =
        SqliteXmrWorkflowJournal::create_new(&workflow).expect("create Maker workflow journal");
    journal
        .initialize(&identity)
        .expect("initialize Maker workflow");
    journal
        .prepare_step(&identity, XmrWorkflowStep::FundMonero)
        .expect("prepare Maker Monero funding");
    assert_eq!(
        journal
            .authorize_once(&identity, XmrWorkflowStep::FundMonero)
            .unwrap(),
        XmrWorkflowDecision::InvokeOnce
    );
    journal
        .reconcile_succeeded(
            &identity,
            XmrWorkflowStep::FundMonero,
            &XmrWorkflowReconciliationV2::new(
                [0x81; 32],
                [0x82; 32],
                XmrWorkflowReconciliationSource::MoneroWalletTransaction,
            )
            .unwrap(),
        )
        .expect("reconcile Maker Monero funding");
    journal
        .select_branch(&identity, branch)
        .expect("select Maker punishment branch");
    journal
        .prepare_step(&identity, step)
        .expect("prepare Maker Tag17 step");
    drop(journal);

    let effect_file = effect_root.join("effect-authority.json");
    write_private(&effect_file, &effect_bytes, 0o600);
    let effect_manifest = effect_root.join("effect-manifest.json");
    publish_xmr_effect_manifest_v3(
        maker_actor.manifest_file(),
        ActorRole::Maker,
        &effect_file,
        &workflow,
        RUN_ID,
        &effect_manifest,
    )
    .expect("publish Maker schema-v3 effect manifest");
    let manifest_bytes = fs::read(effect_manifest).expect("read Maker effect manifest");
    let application_sha256 = [
        maker_actor.stage_a_file().to_path_buf(),
        maker_actor.stage_b_file().to_path_buf(),
        material.maker_packet.clone(),
        material.taker_packet.clone(),
        material.maker_root.join("manifest.json"),
        material.maker_root.join("monero-view.key"),
    ]
    .iter()
    .map(|path| hex::encode(Sha256::digest(fs::read(path).expect("read Maker input"))))
    .collect();
    let share_hex = fs::read_to_string(material.maker_root.join("xmr-share.key"))
        .expect("read Maker XMR share");
    let private_xmr_share_sha256 = hex::encode(Sha256::digest(
        hex::decode(share_hex.trim()).expect("decode Maker XMR share"),
    ));

    RouteFixture {
        _material: material,
        swap_id,
        workflow,
        actor_state: maker_journal,
        worker,
        effect_bytes,
        manifest_bytes,
        application_sha256,
        private_xmr_share_sha256,
        refund_signature_sha256,
    }
}

#[allow(clippy::too_many_lines)]
fn route_fixture_for_mode(
    terminal_branch: XmrWorkflowBranch,
    terminal_step: XmrWorkflowStep,
    semantic_tag14: bool,
) -> RouteFixture {
    let material = provision_material();
    let agreement = signed_stage_a(&material);
    let maker_sessions = initialize_sessions(&material, &agreement, "maker");
    let taker_sessions = initialize_sessions(&material, &agreement, "taker");
    let (activation, _maker_journal, taker_journal) =
        activated_stage_b(&material, &agreement, &maker_sessions, &taker_sessions);

    let actors = owner_directory(&material.material, "actors");
    let taker_actor = provision_xmr_taker_actor_from_material(
        &material.taker_root,
        &material.taker_packet,
        &material.maker_packet,
        &agreement,
        &activation,
        &taker_journal,
        &actors.join("taker"),
    )
    .expect("provision Taker application actor");
    let swap_bytes = taker_actor.swap_id();
    let swap_id = SwapId::new(hex::encode(swap_bytes)).expect("canonical swap ID");
    let workflow = taker_actor
        .state_directory()
        .join("xmr-effect-workflow.sqlite3");
    let effect_root = owner_directory(&material.material, "effect-inputs");
    let worker = effect_root.join("tag14-worker");
    write_private(
        &worker,
        if semantic_tag14 {
            RELEASE_WORKER
        } else {
            WORKER
        },
        0o700,
    );
    let effect_bytes = effect_authority(
        &effect_root,
        &worker,
        &workflow,
        &taker_journal,
        swap_bytes,
        taker_actor.agreement_commitment(),
        taker_actor.activation_commitment(),
        semantic_tag14,
    );
    let authority_digest: [u8; 32] = Sha256::digest(&effect_bytes).into();
    let identity = XmrWorkflowIdentityV1::new(
        swap_id.clone(),
        Participant::Taker,
        RUN_ID.into(),
        taker_actor.agreement_commitment(),
        taker_actor.activation_commitment(),
        authority_digest,
    )
    .expect("valid workflow identity");
    let mut journal =
        SqliteXmrWorkflowJournal::create_new(&workflow).expect("create workflow journal");
    journal.initialize(&identity).expect("initialize workflow");
    complete_step(
        &mut journal,
        &identity,
        XmrWorkflowStep::InitializeLezTag13,
        0x41,
    );
    complete_step(&mut journal, &identity, XmrWorkflowStep::FundLezTag13, 0x43);
    journal
        .select_branch(&identity, terminal_branch)
        .expect("select terminal branch");
    journal
        .prepare_step(&identity, terminal_step)
        .expect("prepare terminal step");
    drop(journal);

    let effect_file = effect_root.join("effect-authority.json");
    write_private(&effect_file, &effect_bytes, 0o600);
    let effect_manifest = effect_root.join("effect-manifest.json");
    publish_xmr_effect_manifest_v3(
        taker_actor.manifest_file(),
        ActorRole::Taker,
        &effect_file,
        &workflow,
        RUN_ID,
        &effect_manifest,
    )
    .expect("publish schema-v3 effect manifest");
    let manifest_bytes = fs::read(effect_manifest).expect("read schema-v3 effect manifest");
    let application_sha256 = [
        taker_actor.stage_a_file().to_path_buf(),
        taker_actor.stage_b_file().to_path_buf(),
        material.taker_packet.clone(),
        material.maker_packet.clone(),
        material.taker_root.join("manifest.json"),
        material.taker_root.join("monero-view.key"),
    ]
    .iter()
    .map(|path| {
        hex::encode(Sha256::digest(
            fs::read(path).expect("read application input"),
        ))
    })
    .collect();
    let share_hex = fs::read_to_string(material.taker_root.join("xmr-share.key"))
        .expect("read Taker XMR share");
    let private_xmr_share_sha256 = hex::encode(Sha256::digest(
        hex::decode(share_hex.trim()).expect("decode XMR share"),
    ));
    let selected_worker = match terminal_step {
        XmrWorkflowStep::AuthorizeLezTag14 => worker,
        XmrWorkflowStep::RefundLezTag16 => effect_root.join("tag16-refund"),
        _ => panic!("unsupported fixture terminal step"),
    };

    RouteFixture {
        _material: material,
        swap_id,
        workflow,
        actor_state: taker_journal,
        worker: selected_worker,
        effect_bytes,
        manifest_bytes,
        application_sha256,
        private_xmr_share_sha256,
        refund_signature_sha256: None,
    }
}

#[test]
fn maker_refund_route_receives_signature_and_share_only_for_invocation() {
    let fixture = maker_refund_route_fixture();
    let actor_lock = MakerActorHeldLock::acquire_for(&fixture.swap_id, &fixture.actor_state)
        .expect("acquire Maker state lock");
    let workflow_lock = MakerActorHeldLock::acquire_for(&fixture.swap_id, &fixture.workflow)
        .expect("acquire Maker workflow lock");
    let execution = load_validated_xmr_effect_execution_v3_bytes(
        &fixture.manifest_bytes,
        &fixture.effect_bytes,
        ActorRole::Maker,
        RUN_ID,
    )
    .expect("load schema-v3 Maker Refund authority");
    let step = XmrWorkflowStep::SweepMoneroRefund;
    let (mut command, sending_plan) = match execution
        .prepare_effect_invocation(step, &actor_lock, &workflow_lock)
        .expect("prepare Maker Refund invocation")
    {
        XmrPreparedEffectInvocationV1::InvokeOnce {
            command,
            tool_plan_identity_sha256,
        } => (command, tool_plan_identity_sha256),
        XmrPreparedEffectInvocationV1::ObserveOnly { .. }
        | XmrPreparedEffectInvocationV1::Complete { .. } => {
            panic!("Prepared Maker Refund must invoke exactly once")
        }
    };
    let output = command.output().expect("run Maker Refund descriptor probe");
    assert!(
        output.status.success(),
        "Maker Refund descriptor probe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let hashes = String::from_utf8(output.stdout).unwrap();
    let hashes = hashes.lines().collect::<Vec<_>>();
    assert_eq!(hashes[0], fixture.private_xmr_share_sha256);
    assert_eq!(
        hashes[1],
        fixture.refund_signature_sha256.as_deref().unwrap()
    );

    let journal_before = Sha256::digest(fs::read(&fixture.actor_state).unwrap());
    {
        let connection = rusqlite::Connection::open(&fixture.actor_state)
            .expect("open Maker role journal for a semantic-preserving rewrite");
        connection
            .execute_batch("VACUUM;")
            .expect("rewrite the same semantic Maker role journal");
    }
    let journal_after = Sha256::digest(fs::read(&fixture.actor_state).unwrap());
    assert_ne!(
        journal_after, journal_before,
        "the regression must exercise changed SQLite representation bytes"
    );

    let reopened = load_validated_xmr_effect_execution_v3_bytes(
        &fixture.manifest_bytes,
        &fixture.effect_bytes,
        ActorRole::Maker,
        RUN_ID,
    )
    .expect("reopen Maker Refund authority");
    assert!(matches!(
        reopened
            .prepare_effect_invocation(step, &actor_lock, &workflow_lock)
            .expect("restart Maker Refund route"),
        XmrPreparedEffectInvocationV1::ObserveOnly {
            tool_plan_identity_sha256
        } if tool_plan_identity_sha256 == sending_plan
    ));
    let (mut observer, observed_plan, source) = reopened
        .prepare_effect_observation(step, &actor_lock, &workflow_lock)
        .expect("Started Maker Refund admits Monero verifier")
        .into_parts();
    assert_eq!(observed_plan, sending_plan);
    assert_eq!(
        source,
        XmrWorkflowReconciliationSource::MoneroWalletTransaction
    );
    assert_observer_pending(&mut observer, step);
}

#[test]
fn maker_tag17_route_preflights_invokes_once_and_observes_without_private_share() {
    let fixture = maker_punish_route_fixture();
    let actor_lock = MakerActorHeldLock::acquire_for(&fixture.swap_id, &fixture.actor_state)
        .expect("acquire Maker state lock");
    let workflow_lock = MakerActorHeldLock::acquire_for(&fixture.swap_id, &fixture.workflow)
        .expect("acquire Maker workflow lock");
    let execution = load_validated_xmr_effect_execution_v3_bytes(
        &fixture.manifest_bytes,
        &fixture.effect_bytes,
        ActorRole::Maker,
        RUN_ID,
    )
    .expect("load schema-v3 Maker Tag17 authority");
    let step = XmrWorkflowStep::PunishLezTag17;

    let prepared = fs::read(&fixture.workflow).unwrap();
    let mut preflight = execution
        .prepare_effect_preflight(step, &actor_lock, &workflow_lock)
        .expect("compose Maker Tag17 preflight")
        .expect("Prepared Tag17 requires preflight");
    assert!(preflight.status().expect("run Tag17 preflight").success());
    assert_eq!(
        fs::read(&fixture.workflow).unwrap(),
        prepared,
        "preflight must not consume the workflow CAS"
    );

    let sending_plan = match execution
        .prepare_effect_invocation(step, &actor_lock, &workflow_lock)
        .expect("prepare Tag17 invocation")
    {
        XmrPreparedEffectInvocationV1::InvokeOnce {
            mut command,
            tool_plan_identity_sha256,
        } => {
            assert!(
                command
                    .status()
                    .expect("run Tag17 descriptor probe")
                    .success()
            );
            tool_plan_identity_sha256
        }
        XmrPreparedEffectInvocationV1::ObserveOnly { .. }
        | XmrPreparedEffectInvocationV1::Complete { .. } => {
            panic!("Prepared Tag17 must invoke exactly once")
        }
    };
    assert!(sending_plan.iter().any(|byte| *byte != 0));

    let reopened = load_validated_xmr_effect_execution_v3_bytes(
        &fixture.manifest_bytes,
        &fixture.effect_bytes,
        ActorRole::Maker,
        RUN_ID,
    )
    .expect("reopen Maker Tag17 authority");
    assert!(matches!(
        reopened
            .prepare_effect_invocation(step, &actor_lock, &workflow_lock)
            .expect("restart Tag17 route"),
        XmrPreparedEffectInvocationV1::ObserveOnly {
            tool_plan_identity_sha256
        } if tool_plan_identity_sha256 == sending_plan
    ));
    let (mut observer, observed_plan, source) = reopened
        .prepare_effect_observation(step, &actor_lock, &workflow_lock)
        .expect("Started Tag17 admits finalized classifier")
        .into_parts();
    assert_eq!(observed_plan, sending_plan);
    assert_eq!(source, XmrWorkflowReconciliationSource::LezFinalizedEvent);
    assert_observer_pending(&mut observer, step);
}

#[test]
fn semantic_tag14_preflight_is_least_privilege_and_does_not_consume_cas() {
    let fixture = semantic_release_route_fixture();
    let actor_lock = MakerActorHeldLock::acquire_for(&fixture.swap_id, &fixture.actor_state)
        .expect("acquire Taker state lock");
    let workflow_lock = MakerActorHeldLock::acquire_for(&fixture.swap_id, &fixture.workflow)
        .expect("acquire Taker workflow lock");
    let execution = load_validated_xmr_effect_execution_v3_bytes(
        &fixture.manifest_bytes,
        &fixture.effect_bytes,
        ActorRole::Taker,
        RUN_ID,
    )
    .expect("load semantic Tag14 authority");

    let mut preflight = execution
        .prepare_effect_preflight(
            XmrWorkflowStep::AuthorizeLezTag14,
            &actor_lock,
            &workflow_lock,
        )
        .expect("compose Tag14 preflight")
        .expect("Prepared Tag14 requires preflight");
    preflight.env("XMR_TEST_PREFLIGHT_FAIL", "1");
    assert!(!preflight.status().expect("run failing preflight").success());

    let mut invocation = match execution
        .prepare_effect_invocation(
            XmrWorkflowStep::AuthorizeLezTag14,
            &actor_lock,
            &workflow_lock,
        )
        .expect("failed preflight leaves Tag14 invocation available")
    {
        XmrPreparedEffectInvocationV1::InvokeOnce { command, .. } => command,
        XmrPreparedEffectInvocationV1::ObserveOnly { .. }
        | XmrPreparedEffectInvocationV1::Complete { .. } => {
            panic!("prepared semantic Tag14 must invoke exactly once")
        }
    };
    let output = invocation
        .output()
        .expect("run Tag14 release descriptor probe");
    assert!(
        output.status.success(),
        "Tag14 descriptor probe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn taker_tag16_invocation_alone_receives_the_validated_xmr_share() {
    let fixture = refund_route_fixture();
    let actor_lock = MakerActorHeldLock::acquire_for(&fixture.swap_id, &fixture.actor_state)
        .expect("acquire Taker state lock");
    let workflow_lock = MakerActorHeldLock::acquire_for(&fixture.swap_id, &fixture.workflow)
        .expect("acquire Taker workflow lock");
    let execution = load_validated_xmr_effect_execution_v3_bytes(
        &fixture.manifest_bytes,
        &fixture.effect_bytes,
        ActorRole::Taker,
        RUN_ID,
    )
    .expect("load executable refund authority");
    let mut command = match execution
        .prepare_effect_invocation(XmrWorkflowStep::RefundLezTag16, &actor_lock, &workflow_lock)
        .expect("prepare Tag16 invocation")
    {
        XmrPreparedEffectInvocationV1::InvokeOnce { command, .. } => command,
        XmrPreparedEffectInvocationV1::ObserveOnly { .. }
        | XmrPreparedEffectInvocationV1::Complete { .. } => {
            panic!("prepared Tag16 must invoke exactly once")
        }
    };
    let output = command.output().expect("run Tag16 descriptor probe");
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        fixture.private_xmr_share_sha256
    );
}

#[test]
fn failed_tag16_preflight_does_not_consume_the_one_attempt_cas() {
    let fixture = refund_route_fixture();
    let actor_lock = MakerActorHeldLock::acquire_for(&fixture.swap_id, &fixture.actor_state)
        .expect("acquire Taker state lock");
    let workflow_lock = MakerActorHeldLock::acquire_for(&fixture.swap_id, &fixture.workflow)
        .expect("acquire Taker workflow lock");
    let execution = load_validated_xmr_effect_execution_v3_bytes(
        &fixture.manifest_bytes,
        &fixture.effect_bytes,
        ActorRole::Taker,
        RUN_ID,
    )
    .expect("load executable refund authority");
    let mut preflight = execution
        .prepare_effect_preflight(XmrWorkflowStep::RefundLezTag16, &actor_lock, &workflow_lock)
        .expect("compose Tag16 preflight")
        .expect("Prepared Tag16 requires preflight");
    preflight.env("XMR_TEST_PREFLIGHT_FAIL", "1");
    assert!(!preflight.status().expect("run failing preflight").success());
    assert!(matches!(
        execution
            .prepare_effect_invocation(
                XmrWorkflowStep::RefundLezTag16,
                &actor_lock,
                &workflow_lock,
            )
            .expect("preflight failure leaves invocation available"),
        XmrPreparedEffectInvocationV1::InvokeOnce { .. }
    ));
}

#[test]
fn taker_tag14_effect_route_pins_before_authorizing_and_never_rearms() {
    let fixture = route_fixture();
    let actor_lock = MakerActorHeldLock::acquire_for(&fixture.swap_id, &fixture.actor_state)
        .expect("acquire Taker state lock");
    let workflow_lock = MakerActorHeldLock::acquire_for(&fixture.swap_id, &fixture.workflow)
        .expect("acquire Taker workflow lock");
    let execution = load_validated_xmr_effect_execution_v3_bytes(
        &fixture.manifest_bytes,
        &fixture.effect_bytes,
        ActorRole::Taker,
        RUN_ID,
    )
    .expect("load executable schema-v3 authority");

    fs::write(&fixture.worker, b"#!/bin/sh\nexit 99\n").expect("corrupt named worker");
    assert!(
        execution
            .prepare_effect_invocation(
                XmrWorkflowStep::AuthorizeLezTag14,
                &actor_lock,
                &workflow_lock,
            )
            .is_err(),
        "program drift must fail before consuming workflow authority"
    );
    assert!(
        execution
            .prepare_effect_invocation(XmrWorkflowStep::ClaimLezTag15, &actor_lock, &workflow_lock,)
            .is_err(),
        "Taker authority must reject the Maker claim slot without mutation"
    );

    fs::write(&fixture.worker, WORKER).expect("restore exact worker bytes");
    let (mut command, first_plan) = match execution
        .prepare_effect_invocation(
            XmrWorkflowStep::AuthorizeLezTag14,
            &actor_lock,
            &workflow_lock,
        )
        .expect("authorize exact Tag14 invocation")
    {
        XmrPreparedEffectInvocationV1::InvokeOnce {
            command,
            tool_plan_identity_sha256,
        } => (command, tool_plan_identity_sha256),
        XmrPreparedEffectInvocationV1::ObserveOnly { .. }
        | XmrPreparedEffectInvocationV1::Complete { .. } => {
            panic!("prepared Tag14 must grant exactly one invocation")
        }
    };
    assert!(first_plan.iter().any(|byte| *byte != 0));
    let output = command
        .env("XMR_TEST_EMIT_APPLICATION_HASHES", "1")
        .output()
        .expect("run pinned Tag14 worker");
    assert_sender_material_and_plan(&fixture, first_plan, output);

    let reopened = load_validated_xmr_effect_execution_v3_bytes(
        &fixture.manifest_bytes,
        &fixture.effect_bytes,
        ActorRole::Taker,
        RUN_ID,
    )
    .expect("reopen executable schema-v3 authority");
    match reopened
        .prepare_effect_invocation(
            XmrWorkflowStep::AuthorizeLezTag14,
            &actor_lock,
            &workflow_lock,
        )
        .expect("reconcile restarted Tag14 route")
    {
        XmrPreparedEffectInvocationV1::ObserveOnly {
            tool_plan_identity_sha256,
        } => assert_eq!(tool_plan_identity_sha256, first_plan),
        XmrPreparedEffectInvocationV1::InvokeOnce { .. }
        | XmrPreparedEffectInvocationV1::Complete { .. } => {
            panic!("started Tag14 must be observe-only and expose no command")
        }
    }
}

fn assert_sender_material_and_plan(fixture: &RouteFixture, first_plan: [u8; 32], output: Output) {
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).expect("ASCII application hashes and plan");
    let lines = stdout.lines().collect::<Vec<_>>();
    assert_eq!(
        lines[..6],
        fixture
            .application_sha256
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        "child descriptors must contain the exact validated application bytes"
    );
    let plan_bytes = format!("{}\n", lines[6]);
    let plan = parse_xmr_effect_child_plan_v1(plan_bytes.as_bytes())
        .expect("parse sealed sending child plan");
    assert_eq!(plan.role(), ActorRole::Taker);
    assert_eq!(plan.mode(), XmrEffectChildModeV1::Invoke);
    assert_eq!(plan.step(), XmrWorkflowStep::AuthorizeLezTag14);
    assert_eq!(plan.run_id(), RUN_ID);
    assert_eq!(plan.sending_tool_plan_sha256(), first_plan);
    assert_eq!(plan.executable_abi(), "lez_xmr_tag14_authorize_v1");
    assert_eq!(plan.adaptor_journal(), fixture.actor_state);
    assert_eq!(
        plan.evidence_root(),
        fixture.worker.parent().unwrap().join("evidence")
    );
    assert_eq!(plan.lez_sidecar_url().as_str(), "http://127.0.0.1:32972/");
    assert_eq!(plan.monero_daemon_url().as_str(), "http://127.0.0.1:32974/");
    let mut noncanonical = plan_bytes.clone().into_bytes();
    noncanonical.pop();
    assert!(parse_xmr_effect_child_plan_v1(&noncanonical).is_err());
    let crossed = format!(
        "{}\n",
        lines[6].replacen("\"role\":\"taker\"", "\"role\":\"maker\"", 1)
    );
    assert!(parse_xmr_effect_child_plan_v1(crossed.as_bytes()).is_err());
    let public_rpc = format!(
        "{}\n",
        lines[6].replacen("http://127.0.0.1:32972/", "http://192.0.2.1:32972/", 1,)
    );
    assert!(parse_xmr_effect_child_plan_v1(public_rpc.as_bytes()).is_err());
    let zero_plan = format!(
        "{}\n",
        lines[6].replacen(&hex::encode(first_plan), &"0".repeat(64), 1)
    );
    assert!(parse_xmr_effect_child_plan_v1(zero_plan.as_bytes()).is_err());
}

#[test]
fn taker_observer_route_is_read_only_role_fixed_and_uses_sending_plan_identity() {
    let fixture = route_fixture();
    let actor_lock = MakerActorHeldLock::acquire_for(&fixture.swap_id, &fixture.actor_state)
        .expect("acquire Taker state lock");
    let workflow_lock = MakerActorHeldLock::acquire_for(&fixture.swap_id, &fixture.workflow)
        .expect("acquire Taker workflow lock");
    let execution = load_validated_xmr_effect_execution_v3_bytes(
        &fixture.manifest_bytes,
        &fixture.effect_bytes,
        ActorRole::Taker,
        RUN_ID,
    )
    .expect("load executable schema-v3 authority");
    let identity = execution.workflow_identity();
    let tag14 = XmrWorkflowStep::AuthorizeLezTag14;

    let prepared = fs::read(&fixture.workflow).unwrap();
    assert!(
        execution
            .prepare_effect_observation(tag14, &actor_lock, &workflow_lock)
            .is_err(),
        "Prepared cannot start an observer"
    );
    assert_eq!(fs::read(&fixture.workflow).unwrap(), prepared);
    let (mut sender, sending_plan) = match execution
        .prepare_effect_invocation(tag14, &actor_lock, &workflow_lock)
        .unwrap()
    {
        XmrPreparedEffectInvocationV1::InvokeOnce {
            command,
            tool_plan_identity_sha256,
        } => (command, tool_plan_identity_sha256),
        XmrPreparedEffectInvocationV1::ObserveOnly { .. }
        | XmrPreparedEffectInvocationV1::Complete { .. } => {
            panic!("Prepared Tag14 must grant invocation")
        }
    };
    assert!(sender.status().unwrap().success());
    let started = fs::read(&fixture.workflow).unwrap();
    let preparation = execution
        .prepare_effect_observation(tag14, &actor_lock, &workflow_lock)
        .expect("Started Tag14 admits its classifier");
    let (mut classifier_command, observed_plan, source) = preparation.into_parts();
    assert_eq!(observed_plan, sending_plan);
    assert_eq!(source, XmrWorkflowReconciliationSource::LezFinalizedEvent);
    assert_observer_pending(&mut classifier_command, tag14);
    assert_eq!(fs::read(&fixture.workflow).unwrap(), started);

    let mut journal = SqliteXmrWorkflowJournal::open_existing(&fixture.workflow).unwrap();
    journal.mark_unknown(identity, tag14).unwrap();
    drop(journal);
    let unknown = fs::read(&fixture.workflow).unwrap();
    let (mut replay_command, replay_plan, replay_source) = execution
        .prepare_effect_observation(tag14, &actor_lock, &workflow_lock)
        .expect("Unknown Tag14 remains observation-only")
        .into_parts();
    assert_eq!(replay_plan, sending_plan);
    assert_eq!(replay_source, source);
    assert_observer_pending(&mut replay_command, tag14);
    assert_eq!(fs::read(&fixture.workflow).unwrap(), unknown);

    let classifier = fixture
        .worker
        .parent()
        .expect("effect input root")
        .join("classifier");
    fs::write(&classifier, b"#!/bin/sh\nexit 99\n").unwrap();
    assert!(
        execution
            .prepare_effect_observation(tag14, &actor_lock, &workflow_lock)
            .is_err(),
        "observer digest drift fails before journal eligibility"
    );
    assert_eq!(fs::read(&fixture.workflow).unwrap(), unknown);
    fs::write(&classifier, OBSERVER).unwrap();

    let exact = XmrWorkflowReconciliationV2::new(
        [0xa4; 32],
        sending_plan,
        XmrWorkflowReconciliationSource::LezFinalizedEvent,
    )
    .unwrap();
    let mut journal = SqliteXmrWorkflowJournal::open_existing(&fixture.workflow).unwrap();
    journal
        .reconcile_succeeded(identity, tag14, &exact)
        .unwrap();
    drop(journal);
    let succeeded = fs::read(&fixture.workflow).unwrap();
    assert!(
        execution
            .prepare_effect_observation(tag14, &actor_lock, &workflow_lock)
            .is_err(),
        "Succeeded cannot start another observer"
    );
    assert_eq!(fs::read(&fixture.workflow).unwrap(), succeeded);
}

#[test]
fn taker_observer_rejects_maker_step_without_workflow_mutation() {
    let fixture = route_fixture();
    let actor_lock =
        MakerActorHeldLock::acquire_for(&fixture.swap_id, &fixture.actor_state).unwrap();
    let workflow_lock =
        MakerActorHeldLock::acquire_for(&fixture.swap_id, &fixture.workflow).unwrap();
    let execution = load_validated_xmr_effect_execution_v3_bytes(
        &fixture.manifest_bytes,
        &fixture.effect_bytes,
        ActorRole::Taker,
        RUN_ID,
    )
    .unwrap();
    let before = fs::read(&fixture.workflow).unwrap();

    assert!(
        execution
            .prepare_effect_observation(
                XmrWorkflowStep::ClaimLezTag15,
                &actor_lock,
                &workflow_lock,
            )
            .is_err(),
        "Taker authority cannot select a Maker observer route"
    );
    assert_eq!(fs::read(&fixture.workflow).unwrap(), before);
}

#[test]
fn taker_monero_observer_uses_wallet_evidence_and_the_sending_plan() {
    let fixture = route_fixture();
    let actor_lock = MakerActorHeldLock::acquire_for(&fixture.swap_id, &fixture.actor_state)
        .expect("acquire Taker state lock");
    let workflow_lock = MakerActorHeldLock::acquire_for(&fixture.swap_id, &fixture.workflow)
        .expect("acquire Taker workflow lock");
    let execution = load_validated_xmr_effect_execution_v3_bytes(
        &fixture.manifest_bytes,
        &fixture.effect_bytes,
        ActorRole::Taker,
        RUN_ID,
    )
    .expect("load executable schema-v3 authority");
    let identity = execution.workflow_identity();
    let tag14 = XmrWorkflowStep::AuthorizeLezTag14;
    let tag14_plan = match execution
        .prepare_effect_invocation(tag14, &actor_lock, &workflow_lock)
        .unwrap()
    {
        XmrPreparedEffectInvocationV1::InvokeOnce {
            mut command,
            tool_plan_identity_sha256,
        } => {
            assert!(command.status().unwrap().success());
            tool_plan_identity_sha256
        }
        XmrPreparedEffectInvocationV1::ObserveOnly { .. }
        | XmrPreparedEffectInvocationV1::Complete { .. } => {
            panic!("Prepared Tag14 must grant invocation")
        }
    };
    let mut journal = SqliteXmrWorkflowJournal::open_existing(&fixture.workflow).unwrap();
    journal
        .reconcile_succeeded(
            identity,
            tag14,
            &XmrWorkflowReconciliationV2::new(
                [0xa4; 32],
                tag14_plan,
                XmrWorkflowReconciliationSource::LezFinalizedEvent,
            )
            .unwrap(),
        )
        .unwrap();
    let monero_step = XmrWorkflowStep::SweepMoneroClaim;
    journal.prepare_step(identity, monero_step).unwrap();
    drop(journal);
    let (mut sender, monero_plan) = match execution
        .prepare_effect_invocation(monero_step, &actor_lock, &workflow_lock)
        .unwrap()
    {
        XmrPreparedEffectInvocationV1::InvokeOnce {
            command,
            tool_plan_identity_sha256,
        } => (command, tool_plan_identity_sha256),
        XmrPreparedEffectInvocationV1::ObserveOnly { .. }
        | XmrPreparedEffectInvocationV1::Complete { .. } => {
            panic!("Prepared Monero sweep must grant invocation")
        }
    };
    assert!(sender.status().unwrap().success());
    let (mut verifier, observed_monero_plan, monero_source) = execution
        .prepare_effect_observation(monero_step, &actor_lock, &workflow_lock)
        .expect("Started Monero sweep admits its verifier")
        .into_parts();
    assert_eq!(observed_monero_plan, monero_plan);
    assert_eq!(
        monero_source,
        XmrWorkflowReconciliationSource::MoneroWalletTransaction
    );
    assert_observer_pending(&mut verifier, monero_step);
}

fn assert_observer_pending(command: &mut Command, step: XmrWorkflowStep) {
    let output = command.output().expect("run sealed observer");
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let result = parse_xmr_effect_observer_result_v1(&output.stdout, step).unwrap();
    assert_eq!(result.step(), step);
    assert_eq!(result.state(), XmrEffectObserverStateV1::Pending);
    assert_eq!(result.effect_evidence_sha256(), None);
}

#[test]
fn observer_result_parser_is_bounded_step_exact_and_source_free() {
    let step = XmrWorkflowStep::AuthorizeLezTag14;
    let pending = parse_xmr_effect_observer_result_v1(
        br#"{"schema_version":1,"step":"authorize_lez_tag14","state":"pending"}"#,
        step,
    )
    .unwrap();
    assert_eq!(pending.state(), XmrEffectObserverStateV1::Pending);
    assert_eq!(pending.effect_evidence_sha256(), None);

    let finalized_bytes = format!(
        r#"{{"schema_version":1,"step":"authorize_lez_tag14","state":"finalized","effect_evidence_sha256":"{}"}}"#,
        "a5".repeat(32)
    );
    let finalized = parse_xmr_effect_observer_result_v1(finalized_bytes.as_bytes(), step).unwrap();
    assert_eq!(finalized.state(), XmrEffectObserverStateV1::Finalized);
    assert_eq!(finalized.effect_evidence_sha256(), Some([0xa5; 32]));

    let invalid = [
        br#"{"schema_version":2,"step":"authorize_lez_tag14","state":"pending"}"#.as_slice(),
        br#"{"schema_version":1,"step":"refund_lez_tag16","state":"pending"}"#,
        br#"{"schema_version":1,"step":"authorize_lez_tag14","state":"pending","effect_evidence_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
        br#"{"schema_version":1,"step":"authorize_lez_tag14","state":"finalized"}"#,
        br#"{"schema_version":1,"step":"authorize_lez_tag14","state":"finalized","effect_evidence_sha256":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"}"#,
        br#"{"schema_version":1,"step":"authorize_lez_tag14","state":"finalized","effect_evidence_sha256":"0000000000000000000000000000000000000000000000000000000000000000"}"#,
        br#"{"schema_version":1,"step":"authorize_lez_tag14","state":"pending","source":"lez_finalized_event"}"#,
        br#"{"schema_version":1,"step":"authorize_lez_tag14","state":"pending","unknown":true}"#,
    ];
    for bytes in invalid {
        assert!(parse_xmr_effect_observer_result_v1(bytes, step).is_err());
    }
    assert!(parse_xmr_effect_observer_result_v1(&[], step).is_err());
    assert!(
        parse_xmr_effect_observer_result_v1(
            &vec![b' '; XMR_EFFECT_OBSERVER_RESULT_MAX_BYTES + 1],
            step,
        )
        .is_err()
    );
}
