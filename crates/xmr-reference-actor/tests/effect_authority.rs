use std::path::{Path, PathBuf};

use serde::Serialize;
use xmr_reference_actor::{ActorRole, load_validated_xmr_effect_authority_bytes};

const SWAP: [u8; 32] = [0x11; 32];
const AGREEMENT: [u8; 32] = [0x22; 32];
const ACTIVATION: [u8; 32] = [0x33; 32];
const RUN: &str = "m5-xmr-effect-run-1";

#[derive(Clone, Serialize)]
struct Tool {
    program: PathBuf,
    program_sha256: String,
    abi: &'static str,
}

#[derive(Clone, Serialize)]
struct MakerTools {
    monero_fund: Tool,
    lez_claim: Tool,
    finalized_classifier: Tool,
    monero_refund: Tool,
    monero_verify: Tool,
    #[serde(skip_serializing_if = "Option::is_none")]
    lez_punish: Option<Tool>,
}

#[derive(Clone, Serialize)]
struct LezRpc {
    sidecar_url: String,
    runtime_file: PathBuf,
    runtime_sha256: String,
    capability_file: PathBuf,
}

#[derive(Clone, Serialize)]
struct AuthenticatedRpc {
    url: String,
    username_file: PathBuf,
    password_file: PathBuf,
}

#[derive(Clone, Serialize)]
struct MoneroRpc {
    daemon: AuthenticatedRpc,
    funding_wallet: AuthenticatedRpc,
    shared_wallet: AuthenticatedRpc,
    role_wallet: AuthenticatedRpc,
    shared_wallet_file_password_file: PathBuf,
}

#[derive(Clone, Serialize)]
struct MakerEffectAuthority {
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
    lez: LezRpc,
    monero: MoneroRpc,
    maker_tools: MakerTools,
}

fn tool(name: &str, byte: u8, abi: &'static str) -> Tool {
    Tool {
        program: PathBuf::from(format!("/opt/lez/bin/{name}")),
        program_sha256: format!("{byte:02x}").repeat(32),
        abi,
    }
}

fn rpc(port: u16, name: &str) -> AuthenticatedRpc {
    AuthenticatedRpc {
        url: format!("http://127.0.0.1:{port}/"),
        username_file: PathBuf::from(format!("/run/monero/{name}.username")),
        password_file: PathBuf::from(format!("/run/monero/{name}.password")),
    }
}

fn manifest() -> MakerEffectAuthority {
    MakerEffectAuthority {
        schema_version: 1,
        pair: "monero",
        role: ActorRole::Maker,
        swap_id: hex::encode(SWAP),
        agreement_commitment: hex::encode(AGREEMENT),
        activation_commitment: hex::encode(ACTIVATION),
        run_id: RUN,
        workflow_journal: PathBuf::from("/var/lib/lez/maker/xmr-workflow.sqlite"),
        adaptor_journal: PathBuf::from("/var/lib/lez/maker/adaptor.sqlite"),
        evidence_root: PathBuf::from("/var/lib/lez/maker/evidence"),
        lez: LezRpc {
            sidecar_url: "http://127.0.0.1:32872/".to_owned(),
            runtime_file: PathBuf::from("/run/lez/maker-runtime.json"),
            runtime_sha256: "44".repeat(32),
            capability_file: PathBuf::from("/run/lez/maker.capability"),
        },
        monero: MoneroRpc {
            daemon: rpc(32874, "daemon"),
            funding_wallet: rpc(32875, "funding"),
            shared_wallet: rpc(32876, "shared"),
            role_wallet: rpc(32877, "maker"),
            shared_wallet_file_password_file: PathBuf::from(
                "/run/monero/shared-wallet-file.password",
            ),
        },
        maker_tools: MakerTools {
            monero_fund: tool("xmr-monero-fund", 0x50, "lez_xmr_monero_fund_v2"),
            lez_claim: tool("xmr-reference-tag15", 0x55, "lez_xmr_tag15_claim_v1"),
            finalized_classifier: tool("xmr-classifier", 0x60, "lez_xmr_finalized_classifier_v1"),
            monero_refund: tool("xmr-refund-sweep", 0x66, "lez_xmr_monero_refund_sweep_v3"),
            monero_verify: tool("xmr-verify", 0x70, "lez_xmr_monero_verify_v2"),
            lez_punish: None,
        },
    }
}

fn canonical(value: &MakerEffectAuthority) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(value).expect("serialize authority fixture");
    bytes.push(b'\n');
    bytes
}

fn load(bytes: &[u8]) -> anyhow::Result<()> {
    let authority = load_validated_xmr_effect_authority_bytes(
        bytes,
        ActorRole::Maker,
        SWAP,
        AGREEMENT,
        ACTIVATION,
        RUN,
    )?;
    assert_eq!(authority.role(), ActorRole::Maker);
    assert_eq!(authority.swap_id(), SWAP);
    assert_eq!(authority.run_id(), RUN);
    assert_eq!(
        authority.workflow_journal(),
        Path::new("/var/lib/lez/maker/xmr-workflow.sqlite")
    );
    assert_eq!(
        authority.adaptor_journal(),
        Path::new("/var/lib/lez/maker/adaptor.sqlite")
    );
    assert_eq!(
        authority.evidence_root(),
        Path::new("/var/lib/lez/maker/evidence")
    );
    assert_eq!(
        authority.lez().sidecar_url().as_str(),
        "http://127.0.0.1:32872/"
    );
    assert_eq!(
        authority.monero().role_wallet().url().as_str(),
        "http://127.0.0.1:32877/"
    );
    assert!(authority.taker_tools().is_none());
    let tools = authority.maker_tools().expect("Maker typed effect tools");
    assert_eq!(tools.monero_fund().program_sha256(), [0x50; 32]);
    assert_eq!(tools.lez_claim().abi(), "lez_xmr_tag15_claim_v1");
    assert_eq!(tools.finalized_classifier().program_sha256(), [0x60; 32]);
    assert_eq!(tools.monero_refund().program_sha256(), [0x66; 32]);
    assert_eq!(tools.monero_verify().program_sha256(), [0x70; 32]);
    Ok(())
}

#[test]
fn schema_v3_maker_requires_exact_tag17_tool_and_preserves_older_profiles() {
    let mut schema_v3 = manifest();
    schema_v3.schema_version = 3;
    schema_v3.maker_tools.lez_punish =
        Some(tool("xmr-reference-tag17", 0x77, "lez_xmr_tag17_punish_v1"));
    let authority = load_validated_xmr_effect_authority_bytes(
        &canonical(&schema_v3),
        ActorRole::Maker,
        SWAP,
        AGREEMENT,
        ACTIVATION,
        RUN,
    )
    .expect("canonical schema-v3 Maker authority");
    assert_eq!(authority.schema_version(), 3);
    assert_eq!(
        authority
            .maker_tools()
            .unwrap()
            .lez_punish()
            .expect("schema-v3 Tag17 tool")
            .abi(),
        "lez_xmr_tag17_punish_v1"
    );

    let mut missing = schema_v3.clone();
    missing.maker_tools.lez_punish = None;
    assert!(load(&canonical(&missing)).is_err());

    let mut wrong_abi = schema_v3.clone();
    wrong_abi.maker_tools.lez_punish.as_mut().unwrap().abi = "lez_xmr_tag15_claim_v1";
    assert!(load(&canonical(&wrong_abi)).is_err());

    let mut legacy_with_tag17 = schema_v3;
    legacy_with_tag17.schema_version = 1;
    assert!(load(&canonical(&legacy_with_tag17)).is_err());
    load(&canonical(&manifest())).expect("unchanged schema-v1 Maker authority");
}

#[test]
fn canonical_role_fixed_effect_authority_is_the_only_effect_loader_input() {
    let valid = canonical(&manifest());
    load(&valid).expect("canonical Maker effect authority");

    let mut unknown = valid.clone();
    unknown.splice(
        unknown.len() - 2..unknown.len() - 2,
        b",\"password\":\"secret\"".iter().copied(),
    );
    assert!(load(&unknown).is_err(), "unknown/secret fields must fail");

    let mut noncanonical = valid.clone();
    noncanonical.push(b' ');
    assert!(load(&noncanonical).is_err(), "noncanonical JSON must fail");

    let mut unsafe_manifest = manifest();
    unsafe_manifest.workflow_journal = unsafe_manifest.adaptor_journal.clone();
    assert!(
        load(&canonical(&unsafe_manifest)).is_err(),
        "workflow and adaptor journals must be distinct"
    );
    unsafe_manifest = manifest();
    unsafe_manifest.evidence_root = PathBuf::from("/var/lib/lez/maker/../evidence");
    assert!(
        load(&canonical(&unsafe_manifest)).is_err(),
        "all authority paths must be normalized absolute paths"
    );
    unsafe_manifest = manifest();
    unsafe_manifest.monero.daemon.url = "http://localhost:32874/".to_owned();
    assert!(
        load(&canonical(&unsafe_manifest)).is_err(),
        "RPC roots must use literal loopback addresses"
    );
    unsafe_manifest = manifest();
    unsafe_manifest.lez.sidecar_url = "http://127.0.0.1/".to_owned();
    assert!(
        load(&canonical(&unsafe_manifest)).is_err(),
        "RPC roots must include an explicit port"
    );

    let legacy_v2 = concat!(
        "{\"schema_version\":2,\"role\":\"maker\",",
        "\"swap_id\":\"1111111111111111111111111111111111111111111111111111111111111111\",",
        "\"published_stage_a\":\"/application/shared/stage-a-v1.borsh\",",
        "\"stage_a_sha256\":\"2121212121212121212121212121212121212121212121212121212121212121\",",
        "\"published_stage_b\":\"/application/shared/stage-b-v1.borsh\",",
        "\"stage_b_sha256\":\"2222222222222222222222222222222222222222222222222222222222222222\",",
        "\"source_private_root\":\"/private/maker\",",
        "\"source_private_manifest_sha256\":\"2323232323232323232323232323232323232323232323232323232323232323\",",
        "\"source_view_key_sha256\":\"2424242424242424242424242424242424242424242424242424242424242424\",",
        "\"own_public_packet\":\"/exchange/maker.json\",",
        "\"own_public_packet_sha256\":\"2525252525252525252525252525252525252525252525252525252525252525\",",
        "\"peer_public_packet\":\"/exchange/taker.json\",",
        "\"peer_public_packet_sha256\":\"2626262626262626262626262626262626262626262626262626262626262626\",",
        "\"role_journal\":\"/private/journals/maker.sqlite\",",
        "\"role_journal_sha256\":\"2727272727272727272727272727272727272727272727272727272727272727\"}\n"
    );
    assert!(
        load(legacy_v2.as_bytes()).is_err(),
        "legacy schema-v2 application authority must remain monitor-only"
    );
}
