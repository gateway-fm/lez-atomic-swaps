use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _},
    path::{Path, PathBuf},
};

use lez_swap_core::Participant;
use lez_swap_store::{
    SqliteXmrWorkflowJournal, XmrWorkflowBranch, XmrWorkflowDecision, XmrWorkflowIdentityV1,
    XmrWorkflowReconciliationSource, XmrWorkflowReconciliationV2, XmrWorkflowStep,
};
use lez_xmr_swap_sdk::{MoneroPrivateViewKey, XmrActivatedAgreementV1, XmrAgreementV1};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use xmr_reference_actor::{ActorRole, provision_xmr_effect_manifest_v3};

use super::xmr_chat_fixture::XmrChatFixture;

const RUN_ID: &str = "m7-maker-tag17-supervisor-e2e";

pub struct MakerTag17EffectFixture {
    pub config: PathBuf,
    pub workflow: PathBuf,
    pub effect_log: PathBuf,
}

#[derive(Serialize)]
struct Tool {
    program: PathBuf,
    program_sha256: String,
    abi: &'static str,
}

#[derive(Serialize)]
struct MakerTools {
    monero_fund: Tool,
    lez_claim: Tool,
    finalized_classifier: Tool,
    monero_refund: Tool,
    monero_verify: Tool,
    lez_punish: Tool,
}

#[derive(Serialize)]
struct Lez {
    sidecar_url: String,
    runtime_file: PathBuf,
    runtime_sha256: String,
    capability_file: PathBuf,
}

#[derive(Serialize)]
struct Rpc {
    url: String,
    username_file: PathBuf,
    password_file: PathBuf,
}

#[derive(Serialize)]
struct Monero {
    daemon: Rpc,
    funding_wallet: Rpc,
    shared_wallet: Rpc,
    role_wallet: Rpc,
    shared_wallet_file_password_file: PathBuf,
}

#[derive(Serialize)]
struct EffectAuthority {
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
    lez: Lez,
    monero: Monero,
    maker_tools: MakerTools,
}

#[allow(clippy::too_many_lines)]
pub fn provision_maker_tag17(fixture: &XmrChatFixture, root: &Path) -> MakerTag17EffectFixture {
    let effect_root = root.join("maker-tag17-effect");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&effect_root)
        .unwrap();
    let evidence_root = effect_root.join("evidence");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&evidence_root)
        .unwrap();
    let effect_log = effect_root.join("effect.log");
    let punish = effect_root.join("tag17-worker");
    let punish_script = format!(
        "#!/bin/sh\nset -eu\n         for fd in 197 198 199 200 201 202 203 204 205 206 207 208 209 210 211 212 213 214 215 216 217; do test -e \"/proc/self/fd/$fd\"; done\n         test ! -e /proc/self/fd/218\n         grep -Fq '\"step\":\"punish_lez_tag17\"' /proc/self/fd/217\n         if grep -Fq '\"mode\":\"preflight\"' /proc/self/fd/217; then printf 'preflight\\n' >> '{}'; exit 0; fi\n         grep -Fq '\"mode\":\"invoke\"' /proc/self/fd/217\n         printf 'invoke\\n' >> '{}'\n",
        effect_log.display(),
        effect_log.display(),
    );
    write(&punish, punish_script.as_bytes(), 0o700);
    let observer = effect_root.join("observer");
    let observer_script = format!(
        "#!/bin/sh\nset -eu\n         test \"$1\" = \"--xmr-workflow-step\"\n         test \"$2\" = \"punish_lez_tag17\"\n         for fd in 197 198 199 200 201 202 203 204 205 206 207 208 209 210 211 212 213 214 215 216 217; do test -e \"/proc/self/fd/$fd\"; done\n         test ! -e /proc/self/fd/218\n         grep -Fq '\"mode\":\"observe\"' /proc/self/fd/217\n         printf 'observe\\n' >> '{}'\n         printf '%s\\n' '{{\"schema_version\":1,\"step\":\"punish_lez_tag17\",\"state\":\"finalized\",\"effect_evidence_sha256\":\"{}\"}}'\n",
        effect_log.display(),
        "ab".repeat(32),
    );
    write(&observer, observer_script.as_bytes(), 0o700);
    let unused = effect_root.join("unused-worker");
    write(&unused, b"#!/bin/sh\nexit 97\n", 0o700);

    let runtime = effect_root.join("runtime.json");
    write(&runtime, b"{\"role\":\"maker\"}\n", 0o600);
    let capability = effect_root.join("lez.capability");
    write(&capability, b"local-test-capability\n", 0o600);
    let secrets = [
        "daemon.username",
        "daemon.password",
        "funding.username",
        "funding.password",
        "shared.username",
        "shared.password",
        "maker.username",
        "maker.password",
        "shared-wallet-file.password",
    ]
    .map(|name| effect_root.join(name));
    for (index, path) in secrets.iter().enumerate() {
        write(path, format!("secret-{index}\n").as_bytes(), 0o600);
    }

    let agreement = XmrAgreementV1::from_wire(&fs::read(&fixture.stage_a).unwrap()).unwrap();
    let view_bytes: [u8; 32] = fs::read(&fixture.maker_view_key_file)
        .unwrap()
        .try_into()
        .unwrap();
    let view = MoneroPrivateViewKey::from_monero_little_endian(view_bytes).unwrap();
    let activation =
        XmrActivatedAgreementV1::from_wire(&agreement, &fs::read(&fixture.stage_b).unwrap(), &view)
            .unwrap();
    let workflow = effect_root.join("workflow.sqlite3");
    let rpc = |port: u16, username: usize, password: usize| Rpc {
        url: format!("http://127.0.0.1:{port}/"),
        username_file: secrets[username].clone(),
        password_file: secrets[password].clone(),
    };
    let tool = |program: &Path, abi| Tool {
        program: program.to_path_buf(),
        program_sha256: hex::encode(Sha256::digest(fs::read(program).unwrap())),
        abi,
    };
    let authority = EffectAuthority {
        schema_version: 3,
        pair: "monero",
        role: ActorRole::Maker,
        swap_id: fixture.swap_id.as_str().to_owned(),
        agreement_commitment: hex::encode(agreement.agreement_commitment()),
        activation_commitment: hex::encode(activation.activation_commitment()),
        run_id: RUN_ID,
        workflow_journal: workflow.clone(),
        adaptor_journal: fixture.maker_actor_state.clone(),
        evidence_root,
        lez: Lez {
            sidecar_url: "http://127.0.0.1:32972/".to_owned(),
            runtime_sha256: hex::encode(Sha256::digest(fs::read(&runtime).unwrap())),
            runtime_file: runtime,
            capability_file: capability,
        },
        monero: Monero {
            daemon: rpc(32974, 0, 1),
            funding_wallet: rpc(32975, 2, 3),
            shared_wallet: rpc(32976, 4, 5),
            role_wallet: rpc(32977, 6, 7),
            shared_wallet_file_password_file: secrets[8].clone(),
        },
        maker_tools: MakerTools {
            monero_fund: tool(&unused, "lez_xmr_monero_fund_v2"),
            lez_claim: tool(&unused, "lez_xmr_tag15_claim_v1"),
            finalized_classifier: tool(&observer, "lez_xmr_finalized_classifier_v1"),
            monero_refund: tool(&unused, "lez_xmr_monero_refund_sweep_v3"),
            monero_verify: tool(&observer, "lez_xmr_monero_verify_v2"),
            lez_punish: tool(&punish, "lez_xmr_tag17_punish_v1"),
        },
    };
    let mut authority_bytes = serde_json::to_vec(&authority).unwrap();
    authority_bytes.push(b'\n');
    let authority_file = effect_root.join("authority.json");
    write(&authority_file, &authority_bytes, 0o600);
    let config = effect_root.join("actor-provision-v3.json");
    let _provisioned = provision_xmr_effect_manifest_v3(
        &fixture.maker_actor_config,
        ActorRole::Maker,
        &authority_file,
        &workflow,
        RUN_ID,
        &config,
    )
    .unwrap();

    let identity = XmrWorkflowIdentityV1::new(
        fixture.swap_id.clone(),
        Participant::Maker,
        RUN_ID.into(),
        agreement.agreement_commitment(),
        activation.activation_commitment(),
        Sha256::digest(&authority_bytes).into(),
    )
    .unwrap();
    let mut journal = SqliteXmrWorkflowJournal::open_existing(&workflow).unwrap();
    journal
        .prepare_step(&identity, XmrWorkflowStep::FundMonero)
        .unwrap();
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
        .unwrap();
    journal
        .select_branch(&identity, XmrWorkflowBranch::Punish)
        .unwrap();
    journal
        .prepare_step(&identity, XmrWorkflowStep::PunishLezTag17)
        .unwrap();

    MakerTag17EffectFixture {
        config,
        workflow,
        effect_log,
    }
}

fn write(path: &Path, bytes: &[u8], mode: u32) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}
