use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::Write as _,
    os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use clap::Parser as _;
use lez_adaptor_role_runner::{Cli as RunnerCli, execute as execute_runner};
use lez_swap_core::SwapId;
use lez_xmr_swap_sdk::{
    MoneroAddressNetworkV1, MoneroSharedAddressV1, ValidatedXmrAgreementBodyV1, XmrAgreementBodyV1,
    XmrLezTermsV1, XmrMessagesV1, XmrMoneroTermsV1, XmrNamedProfileV1, XmrParticipantsV1,
    XmrSwapDirectionV1, XmrWindowsV1,
};
use serde_json::json;
use sha2::{Digest as _, Sha256};
use xmr_reference_actor::{
    ActorRole, Cli as ActorCli, ValidatedRolePacket, execute as execute_actor,
    provision_xmr_maker_actor_from_material,
};

pub struct XmrChatFixture {
    pub stage_a: PathBuf,
    pub stage_b: PathBuf,
    pub taker_private_root: PathBuf,
    pub taker_public_packet: PathBuf,
    pub maker_public_packet: PathBuf,
    pub taker_journal: PathBuf,
    pub maker_public_key_file: PathBuf,
    pub maker_view_key_file: PathBuf,
    pub maker_registry_file: PathBuf,
    pub taker_actor_root: PathBuf,
    pub receipt: PathBuf,
    pub maker_actor_config: PathBuf,
    pub maker_actor_state: PathBuf,
    pub swap_id: SwapId,
}

impl XmrChatFixture {
    #[allow(clippy::similar_names, clippy::too_many_lines)]
    pub fn new(
        root: &Path,
        swap_id: [u8; 32],
        foreign_units: u64,
        lez_units: u128,
        actor_program: &Path,
    ) -> Self {
        let material = root.join("xmr-material");
        private_dir(&material);
        let installed_actor_program = material.join("installed-xmr-actor");
        fs::copy(actor_program, &installed_actor_program).unwrap();
        fs::set_permissions(&installed_actor_program, fs::Permissions::from_mode(0o700)).unwrap();
        let taker_private_root = material.join("taker-private");
        let maker_private_root = material.join("maker-private");
        let taker_public_packet = material.join("taker-public.json");
        let maker_public_packet = material.join("maker-public.json");
        actor(&[
            "provision".into(),
            "taker".into(),
            "--private-root".into(),
            os(&taker_private_root),
            "--lez-owner-account".into(),
            "22".repeat(32).into(),
            "--public-packet".into(),
            os(&taker_public_packet),
        ]);
        let view_handoff = material.join("shared-view.key");
        write_private(
            &view_handoff,
            &fs::read(taker_private_root.join("monero-view.key")).unwrap(),
        );
        actor(&[
            "provision".into(),
            "maker".into(),
            "--private-root".into(),
            os(&maker_private_root),
            "--lez-owner-account".into(),
            "11".repeat(32).into(),
            "--shared-view-key-file".into(),
            os(&view_handoff),
            "--public-packet".into(),
            os(&maker_public_packet),
        ]);

        let maker = ValidatedRolePacket::read(&maker_public_packet).unwrap();
        let taker = ValidatedRolePacket::read(&taker_public_packet).unwrap();
        assert_eq!(maker.role(), ActorRole::Maker);
        assert_eq!(taker.role(), ActorRole::Taker);
        assert_eq!(maker.public_view_key(), taker.public_view_key());
        let participants =
            XmrParticipantsV1::new(maker.identity().clone(), taker.identity().clone());
        let claim_key = participants.claim_aggregate_x_only_key().unwrap();
        let refund_key = participants.refund_aggregate_x_only_key().unwrap();
        let address = MoneroSharedAddressV1::derive_from_public_view_key(
            MoneroAddressNetworkV1::Regtest,
            maker.proof(),
            taker.proof(),
            maker.public_view_key(),
        )
        .unwrap();
        let now_ms = now().checked_mul(1_000).unwrap();
        let body = XmrAgreementBodyV1::new(
            XmrSwapDirectionV1::TakerSellsLez,
            XmrNamedProfileV1::AcceleratedRegtest,
            swap_id,
            participants,
            XmrMoneroTermsV1::new(
                MoneroAddressNetworkV1::Regtest,
                [0x31; 32],
                foreign_units,
                10,
                maker.proof().to_wire_bytes().unwrap(),
                taker.proof().to_wire_bytes().unwrap(),
                address.public_view_key(),
                address.public_spend_key(),
                address.address_string(),
            ),
            XmrLezTermsV1::new(
                [0x40; 32],
                [0x41; 32],
                [0x42; 8],
                [0x43; 8],
                2,
                [0x44; 32],
                [0x45; 32],
                taker.identity().lez_owner_account(),
                maker.identity().lez_owner_account(),
                claim_key,
                XmrLezTermsV1::authority_account_for_key(claim_key),
                refund_key,
                XmrLezTermsV1::authority_account_for_key(refund_key),
                maker.proof().transcript_commitment(),
                taker.proof().transcript_commitment(),
                lez_units,
            ),
            XmrMessagesV1::new([0x51; 32], [0x52; 32], [0x53; 32]),
            XmrWindowsV1::new(now_ms + 3_600_000, now_ms + 7_200_000, now_ms + 10_800_000),
        );
        let validated = ValidatedXmrAgreementBodyV1::validate(body).unwrap();
        let unsigned_stage_a = material.join("unsigned-stage-a.bin");
        write_private(
            &unsigned_stage_a,
            &validated.encode_unsigned_wire().unwrap(),
        );
        let maker_stage_a_signature = material.join("maker-stage-a.sig");
        let taker_stage_a_signature = material.join("taker-stage-a.sig");
        let stage_a = material.join("stage-a.bin");
        sign_stage_a(
            "maker",
            &maker_private_root,
            &maker_public_packet,
            &taker_public_packet,
            &unsigned_stage_a,
            &maker_stage_a_signature,
        );
        sign_stage_a(
            "taker",
            &taker_private_root,
            &taker_public_packet,
            &maker_public_packet,
            &unsigned_stage_a,
            &taker_stage_a_signature,
        );
        actor(&[
            "assemble-stage-a".into(),
            "--maker-public-packet".into(),
            os(&maker_public_packet),
            "--taker-public-packet".into(),
            os(&taker_public_packet),
            "--unsigned-stage-a".into(),
            os(&unsigned_stage_a),
            "--maker-signature".into(),
            os(&maker_stage_a_signature),
            "--taker-signature".into(),
            os(&taker_stage_a_signature),
            "--output-stage-a".into(),
            os(&stage_a),
        ]);

        let maker_sessions = material.join("maker-sessions");
        let taker_sessions = material.join("taker-sessions");
        initialize_sessions(
            "maker",
            &maker_private_root,
            &maker_public_packet,
            &taker_public_packet,
            &stage_a,
            &maker_sessions,
        );
        initialize_sessions(
            "taker",
            &taker_private_root,
            &taker_public_packet,
            &maker_public_packet,
            &stage_a,
            &taker_sessions,
        );
        let maker_journal = material.join("maker-journal.sqlite3");
        let taker_journal = material.join("taker-journal.sqlite3");
        run_round(
            "claim",
            &material,
            &maker_sessions,
            &taker_sessions,
            &maker_private_root,
            &taker_private_root,
            &maker_journal,
            &taker_journal,
            false,
        );
        run_round(
            "refund",
            &material,
            &maker_sessions,
            &taker_sessions,
            &maker_private_root,
            &taker_private_root,
            &maker_journal,
            &taker_journal,
            true,
        );
        let unsigned_stage_b = material.join("unsigned-stage-b.bin");
        let maker_stage_b_signature = material.join("maker-stage-b.sig");
        let taker_stage_b_signature = material.join("taker-stage-b.sig");
        let stage_b = material.join("stage-b.bin");
        actor(&[
            "compose-stage-b".into(),
            "--private-root".into(),
            os(&taker_private_root),
            "--own-public-packet".into(),
            os(&taker_public_packet),
            "--peer-public-packet".into(),
            os(&maker_public_packet),
            "--agreement-stage-a".into(),
            os(&stage_a),
            "--journal".into(),
            os(&taker_journal),
            "--output-unsigned-stage-b".into(),
            os(&unsigned_stage_b),
        ]);
        sign_stage_b(
            "maker",
            &maker_private_root,
            &maker_public_packet,
            &taker_public_packet,
            &stage_a,
            &unsigned_stage_b,
            &maker_stage_b_signature,
        );
        sign_stage_b(
            "taker",
            &taker_private_root,
            &taker_public_packet,
            &maker_public_packet,
            &stage_a,
            &unsigned_stage_b,
            &taker_stage_b_signature,
        );
        actor(&[
            "assemble-stage-b".into(),
            "taker".into(),
            "--private-root".into(),
            os(&taker_private_root),
            "--own-public-packet".into(),
            os(&taker_public_packet),
            "--peer-public-packet".into(),
            os(&maker_public_packet),
            "--agreement-stage-a".into(),
            os(&stage_a),
            "--unsigned-stage-b".into(),
            os(&unsigned_stage_b),
            "--maker-signature".into(),
            os(&maker_stage_b_signature),
            "--taker-signature".into(),
            os(&taker_stage_b_signature),
            "--output-stage-b".into(),
            os(&stage_b),
        ]);

        let maker_actor_root = material.join("maker-actor");
        let provisioned = provision_xmr_maker_actor_from_material(
            &maker_private_root,
            &maker_public_packet,
            &taker_public_packet,
            &stage_a,
            &stage_b,
            &maker_journal,
            &maker_actor_root,
        )
        .unwrap();
        let maker_actor_config = provisioned.manifest_file().to_path_buf();
        let maker_actor_state = maker_journal.clone();
        let maker_public_key_file = material.join("maker-agreement.pub");
        write_private(
            &maker_public_key_file,
            &maker.identity().agreement_public_key(),
        );
        let maker_view_key_file = material.join("maker-view.raw");
        let encoded_view = fs::read(maker_private_root.join("monero-view.key")).unwrap();
        let encoded_view = encoded_view
            .strip_suffix(b"\r\n")
            .or_else(|| encoded_view.strip_suffix(b"\n"))
            .unwrap_or(&encoded_view);
        let decoded_view = hex::decode(encoded_view).unwrap();
        assert_eq!(decoded_view.len(), 32);
        write_private(&maker_view_key_file, &decoded_view);
        let maker_registry_file = material.join("maker-registry.json");
        let registry = serde_json::to_vec(&json!({
            "schema_version": 1,
            "actors": [{
                "swap_id": hex::encode(swap_id),
                "config_path": maker_actor_config,
                "config_sha256": hex::encode(provisioned.manifest_sha256()),
                "program_path": installed_actor_program,
                "program_sha256": hex::encode(sha256_file(&installed_actor_program)),
                "state_database_path": maker_actor_state,
            }]
        }))
        .unwrap();
        write_private(&maker_registry_file, &registry);
        Self {
            stage_a,
            stage_b,
            taker_private_root,
            taker_public_packet,
            maker_public_packet,
            taker_journal,
            maker_public_key_file,
            maker_view_key_file,
            maker_registry_file,
            taker_actor_root: material.join("taker-actor"),
            receipt: material.join("acceptance-receipt.json"),
            maker_actor_config,
            maker_actor_state,
            swap_id: SwapId::new(hex::encode(swap_id)).unwrap(),
        }
    }
}

fn sign_stage_a(
    role: &str,
    private_root: &Path,
    own: &Path,
    peer: &Path,
    unsigned: &Path,
    output: &Path,
) {
    actor(&[
        "sign-stage-a".into(),
        role.into(),
        "--private-root".into(),
        os(private_root),
        "--own-public-packet".into(),
        os(own),
        "--peer-public-packet".into(),
        os(peer),
        "--unsigned-stage-a".into(),
        os(unsigned),
        "--output-signature".into(),
        os(output),
    ]);
}

fn initialize_sessions(
    role: &str,
    private_root: &Path,
    own: &Path,
    peer: &Path,
    stage_a: &Path,
    output: &Path,
) {
    actor(&[
        "initialize-sessions".into(),
        role.into(),
        "--private-root".into(),
        os(private_root),
        "--own-public-packet".into(),
        os(own),
        "--peer-public-packet".into(),
        os(peer),
        "--agreement-stage-a".into(),
        os(stage_a),
        "--session-root".into(),
        os(output),
    ]);
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn run_round(
    purpose: &str,
    root: &Path,
    maker_sessions: &Path,
    taker_sessions: &Path,
    maker_private: &Path,
    taker_private: &Path,
    maker_journal: &Path,
    taker_journal: &Path,
    complete_maker: bool,
) {
    let round = root.join(format!("{purpose}-round"));
    private_dir(&round);
    let maker_commitment = round.join("maker-commitment.json");
    let taker_commitment = round.join("taker-commitment.json");
    let maker_nonce = round.join("maker-nonce.json");
    let taker_nonce = round.join("taker-nonce.json");
    let maker_partial = round.join("maker-partial.json");
    let taker_partial = round.join("taker-partial.json");
    let taker_presignature = round.join("taker-presignature.json");
    runner(
        "maker",
        maker_journal,
        &maker_sessions.join(format!("{purpose}.json")),
        &[
            "reserve".into(),
            "--secret-key-file".into(),
            os(&maker_private.join(format!("{purpose}.key"))),
            "--output".into(),
            os(&maker_commitment),
        ],
    );
    runner(
        "taker",
        taker_journal,
        &taker_sessions.join(format!("{purpose}.json")),
        &[
            "reserve".into(),
            "--secret-key-file".into(),
            os(&taker_private.join(format!("{purpose}.key"))),
            "--output".into(),
            os(&taker_commitment),
        ],
    );
    runner_simple(
        "maker",
        maker_journal,
        maker_sessions,
        purpose,
        "accept-commitment",
        "--input",
        &taker_commitment,
    );
    runner_simple(
        "taker",
        taker_journal,
        taker_sessions,
        purpose,
        "accept-commitment",
        "--input",
        &maker_commitment,
    );
    runner_simple(
        "maker",
        maker_journal,
        maker_sessions,
        purpose,
        "reveal-nonce",
        "--output",
        &maker_nonce,
    );
    runner_simple(
        "taker",
        taker_journal,
        taker_sessions,
        purpose,
        "reveal-nonce",
        "--output",
        &taker_nonce,
    );
    runner(
        "maker",
        maker_journal,
        &maker_sessions.join(format!("{purpose}.json")),
        &[
            "accept-nonce-sign".into(),
            "--input".into(),
            os(&taker_nonce),
            "--secret-key-file".into(),
            os(&maker_private.join(format!("{purpose}.key"))),
            "--output".into(),
            os(&maker_partial),
        ],
    );
    runner(
        "taker",
        taker_journal,
        &taker_sessions.join(format!("{purpose}.json")),
        &[
            "accept-nonce-sign".into(),
            "--input".into(),
            os(&maker_nonce),
            "--secret-key-file".into(),
            os(&taker_private.join(format!("{purpose}.key"))),
            "--output".into(),
            os(&taker_partial),
        ],
    );
    runner(
        "taker",
        taker_journal,
        &taker_sessions.join(format!("{purpose}.json")),
        &[
            "accept-peer-partial".into(),
            "--input".into(),
            os(&maker_partial),
            "--output".into(),
            os(&taker_presignature),
        ],
    );
    if complete_maker {
        runner(
            "maker",
            maker_journal,
            &maker_sessions.join(format!("{purpose}.json")),
            &[
                "accept-peer-partial".into(),
                "--input".into(),
                os(&taker_partial),
                "--output".into(),
                os(&round.join("maker-presignature.json")),
            ],
        );
    }
}

fn runner_simple(
    role: &str,
    journal: &Path,
    sessions: &Path,
    purpose: &str,
    action: &str,
    option: &str,
    path: &Path,
) {
    runner(
        role,
        journal,
        &sessions.join(format!("{purpose}.json")),
        &[action.into(), option.into(), os(path)],
    );
}

fn runner(role: &str, journal: &Path, session: &Path, action: &[OsString]) {
    let mut args = vec![
        "lez-adaptor-role-runner".into(),
        role.into(),
        "--journal".into(),
        os(journal),
        "--session".into(),
        os(session),
    ];
    args.extend_from_slice(action);
    let cli = RunnerCli::try_parse_from(args).unwrap();
    execute_runner(&cli).unwrap();
}

fn sign_stage_b(
    role: &str,
    private_root: &Path,
    own: &Path,
    peer: &Path,
    stage_a: &Path,
    unsigned: &Path,
    output: &Path,
) {
    actor(&[
        "sign-stage-b".into(),
        role.into(),
        "--private-root".into(),
        os(private_root),
        "--own-public-packet".into(),
        os(own),
        "--peer-public-packet".into(),
        os(peer),
        "--agreement-stage-a".into(),
        os(stage_a),
        "--unsigned-stage-b".into(),
        os(unsigned),
        "--output-signature".into(),
        os(output),
    ]);
}

fn actor(arguments: &[OsString]) {
    let mut args = vec![OsString::from("xmr-reference-actor")];
    args.extend_from_slice(arguments);
    let cli = ActorCli::try_parse_from(args).unwrap();
    execute_actor(cli).unwrap();
}

fn os(path: &Path) -> OsString {
    path.as_os_str().to_owned()
}

fn private_dir(path: &Path) {
    fs::DirBuilder::new().mode(0o700).create(path).unwrap();
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

fn sha256_file(path: &Path) -> [u8; 32] {
    Sha256::digest(fs::read(path).unwrap()).into()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
