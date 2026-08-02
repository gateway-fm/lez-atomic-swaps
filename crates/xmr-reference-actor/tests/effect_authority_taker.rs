use std::path::{Path, PathBuf};

use serde::Serialize;
use xmr_reference_actor::{ActorRole, load_validated_xmr_effect_authority_bytes};

const SWAP: [u8; 32] = [0x81; 32];
const AGREEMENT: [u8; 32] = [0x82; 32];
const ACTIVATION: [u8; 32] = [0x83; 32];
const RUN: &str = "m5-xmr-taker-effect-run-1";

#[derive(Clone, Serialize)]
struct Tool {
    program: PathBuf,
    program_sha256: String,
    abi: &'static str,
}

#[derive(Clone, Serialize)]
struct TakerTools {
    tag14_authorize: Tool,
    finalized_classifier: Tool,
    monero_claim: Tool,
    monero_verify: Tool,
    tag16_refund: Tool,
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
}

#[derive(Clone, Serialize)]
struct TakerEffectAuthority {
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
    taker_tools: TakerTools,
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

fn manifest() -> TakerEffectAuthority {
    TakerEffectAuthority {
        schema_version: 1,
        pair: "monero",
        role: ActorRole::Taker,
        swap_id: hex::encode(SWAP),
        agreement_commitment: hex::encode(AGREEMENT),
        activation_commitment: hex::encode(ACTIVATION),
        run_id: RUN,
        workflow_journal: PathBuf::from("/var/lib/lez/taker/xmr-workflow.sqlite"),
        adaptor_journal: PathBuf::from("/var/lib/lez/taker/adaptor.sqlite"),
        evidence_root: PathBuf::from("/var/lib/lez/taker/evidence"),
        lez: LezRpc {
            sidecar_url: "http://127.0.0.1:32972/".to_owned(),
            runtime_file: PathBuf::from("/run/lez/taker-runtime.json"),
            runtime_sha256: "84".repeat(32),
            capability_file: PathBuf::from("/run/lez/taker.capability"),
        },
        monero: MoneroRpc {
            daemon: rpc(32974, "daemon"),
            funding_wallet: rpc(32975, "funding"),
            shared_wallet: rpc(32976, "shared"),
            role_wallet: rpc(32977, "taker"),
        },
        taker_tools: TakerTools {
            tag14_authorize: tool("xmr-tag14-authorize", 0x85, "lez_xmr_tag14_authorize_v1"),
            finalized_classifier: tool("xmr-classifier", 0x86, "lez_xmr_finalized_classifier_v1"),
            monero_claim: tool("xmr-claim-sweep", 0x87, "lez_xmr_monero_claim_sweep_v2"),
            monero_verify: tool("xmr-verify", 0x88, "lez_xmr_monero_verify_v2"),
            tag16_refund: tool("xmr-reference-tag16", 0x89, "lez_xmr_tag16_refund_v1"),
        },
    }
}

fn canonical(value: &TakerEffectAuthority) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(value).expect("serialize Taker authority");
    bytes.push(b'\n');
    bytes
}

#[test]
fn taker_profile_is_fixed_and_cannot_cross_role_or_tool_authority() {
    let valid = manifest();
    let authority = load_validated_xmr_effect_authority_bytes(
        &canonical(&valid),
        ActorRole::Taker,
        SWAP,
        AGREEMENT,
        ACTIVATION,
        RUN,
    )
    .expect("canonical Taker effect authority");
    assert_eq!(authority.role(), ActorRole::Taker);
    assert_eq!(
        authority.workflow_journal(),
        Path::new("/var/lib/lez/taker/xmr-workflow.sqlite")
    );

    let mut crossed = manifest();
    crossed.role = ActorRole::Maker;
    assert!(
        load_validated_xmr_effect_authority_bytes(
            &canonical(&crossed),
            ActorRole::Maker,
            SWAP,
            AGREEMENT,
            ACTIVATION,
            RUN,
        )
        .is_err()
    );

    let mut drifted = manifest();
    drifted.taker_tools.tag16_refund.abi = "lez_xmr_tag15_claim_v1";
    assert!(
        load_validated_xmr_effect_authority_bytes(
            &canonical(&drifted),
            ActorRole::Taker,
            SWAP,
            AGREEMENT,
            ACTIVATION,
            RUN,
        )
        .is_err()
    );

    let mut bad_hash = manifest();
    bad_hash.taker_tools.monero_claim.program_sha256 = "AA".repeat(32);
    assert!(
        load_validated_xmr_effect_authority_bytes(
            &canonical(&bad_hash),
            ActorRole::Taker,
            SWAP,
            AGREEMENT,
            ACTIVATION,
            RUN,
        )
        .is_err()
    );
}
