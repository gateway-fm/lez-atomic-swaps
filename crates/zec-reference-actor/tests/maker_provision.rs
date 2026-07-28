//! Focused filesystem and role-boundary tests for daemon-owned Maker provisioning.

use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    os::unix::fs::{
        DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _,
    },
    path::{Path, PathBuf},
    sync::{Arc, Barrier},
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use lez_swap_core::{SwapDirection, UnixSeconds};
use lez_zec_swap_sdk::{
    Bip199Contract, ExpectedBip199Output, LezAssetV1, LezChainIdentityV1, LezEnvironmentV1,
    NegotiationTranscriptV1, ZEC_CONCRETE_AGREEMENT_SCHEMA_V2, ZcashTransparentDestinationV1,
    ZecAgreementBodyV1, ZecAgreementRecordV1, ZecAgreementV1, ZecLezTermsV1,
    ZecParticipantIdentityV1, ZecParticipantsV1, ZecProfileId, ZecProfileRecordV1, ZecRefundPlanV1,
    ZecSwapBinding, ZecSwapBindingRecordV1, ZecTransactionPolicyV1, derive_lez_metadata_account_v1,
    derive_lez_native_custody_account_v1, derive_lez_swap_id_v1,
};
use secp256k1::{Message, PublicKey, Secp256k1, SecretKey};
use serde_json::json;
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;
use zcash_protocol::{
    consensus::{BranchId, NetworkType},
    value::Zatoshis,
};
use zcash_transparent::address::TransparentAddress;
use zec_reference_actor::{ActorConfig, ActorRole, provision_zec_maker_actor_from_chat};

const SWAP_ID: &str = "maker-provision-swap-001";
const PREIMAGE: [u8; 32] = [0x44; 32];

#[test]
fn maker_only_publish_is_exactly_replayable_without_replacing_artifacts() {
    let fixture = Fixture::new();
    let output = fixture.output("maker-success");

    let first = provision_zec_maker_actor_from_chat(
        &fixture.maker_config,
        &fixture.final_wire,
        fixture.accepted_at,
        &output,
    )
    .expect("publish Maker actor");
    assert!(!first.was_replay());
    assert_eq!(first.swap_id().as_str(), SWAP_ID);
    let agreement_sha256: [u8; 32] = Sha256::digest(&fixture.final_wire).into();
    let config_sha256: [u8; 32] = Sha256::digest(fs::read(first.config_file()).unwrap()).into();
    assert_eq!(first.agreement_sha256(), agreement_sha256);
    assert_eq!(first.config_sha256(), config_sha256);
    assert_eq!(
        fs::read(first.agreement_file()).unwrap(),
        fixture.final_wire
    );
    assert_private_file(first.agreement_file());
    assert_private_file(first.config_file());
    assert_private_directory(&output);
    assert_private_directory(&output.join("shared"));
    assert_private_directory(&output.join("maker"));
    assert_private_directory(&output.join("maker/state"));
    assert!(!output.join("taker").exists());

    let config = ActorConfig::load_private(first.config_file()).expect("reload Maker config");
    assert_eq!(config.role(), ActorRole::Maker);
    assert_eq!(config.swap_id(), first.swap_id());
    assert_eq!(config.role_state_db(), first.state_database());
    config
        .load_activate_material()
        .expect("provisioned Maker config activates");

    let config_inode = fs::symlink_metadata(first.config_file()).unwrap().ino();
    let agreement_inode = fs::symlink_metadata(first.agreement_file()).unwrap().ino();
    let config_bytes = fs::read(first.config_file()).unwrap();
    let replay = provision_zec_maker_actor_from_chat(
        &fixture.maker_config,
        &fixture.final_wire,
        fixture.accepted_at,
        &output,
    )
    .expect("exact replay");
    assert!(replay.was_replay());
    assert_eq!(replay.config_file(), first.config_file());
    assert_eq!(replay.state_database(), first.state_database());
    assert_eq!(
        fs::symlink_metadata(replay.config_file()).unwrap().ino(),
        config_inode
    );
    assert_eq!(
        fs::symlink_metadata(replay.agreement_file()).unwrap().ino(),
        agreement_inode
    );
    assert_eq!(fs::read(replay.config_file()).unwrap(), config_bytes);
}

#[test]
fn taker_source_is_rejected_without_publishing_any_bundle() {
    let fixture = Fixture::new();
    let output = fixture.output("taker-rejected");

    assert!(
        provision_zec_maker_actor_from_chat(
            &fixture.taker_config,
            &fixture.final_wire,
            fixture.accepted_at,
            &output,
        )
        .is_err()
    );
    assert!(!output.exists());
}

#[test]
fn corrupt_preseeded_output_conflicts_without_clobbering_it() {
    let fixture = Fixture::new();
    let output = fixture.output("preseeded-collision");
    private_directory(&output);
    let marker = output.join("attacker-marker");
    private_file(&marker, b"do-not-replace");
    let marker_inode = fs::symlink_metadata(&marker).unwrap().ino();

    assert!(
        provision_zec_maker_actor_from_chat(
            &fixture.maker_config,
            &fixture.final_wire,
            fixture.accepted_at,
            &output,
        )
        .is_err()
    );
    assert_eq!(fs::read(&marker).unwrap(), b"do-not-replace");
    assert_eq!(fs::symlink_metadata(&marker).unwrap().ino(), marker_inode);
    assert_eq!(fs::read_dir(&output).unwrap().count(), 1);
}

#[test]
fn exact_replay_rejects_unsafe_existing_actor_state() {
    assert_unsafe_mutable_artifact_is_rejected("maker/state/actor.sqlite3");
}

#[test]
fn exact_replay_rejects_unsafe_existing_bridge_journal() {
    assert_unsafe_mutable_artifact_is_rejected("maker/state/bridge.sqlite3");
}

fn assert_unsafe_mutable_artifact_is_rejected(relative: &str) {
    let fixture = Fixture::new();
    let output = fixture.output(relative.rsplit_once('/').expect("fixture relative path").1);
    provision_zec_maker_actor_from_chat(
        &fixture.maker_config,
        &fixture.final_wire,
        fixture.accepted_at,
        &output,
    )
    .expect("initial publish");
    let unsafe_file = output.join(relative);
    fs::write(&unsafe_file, b"unsafe preseeded mutable state").unwrap();
    fs::set_permissions(&unsafe_file, fs::Permissions::from_mode(0o644)).unwrap();

    assert!(
        provision_zec_maker_actor_from_chat(
            &fixture.maker_config,
            &fixture.final_wire,
            fixture.accepted_at,
            &output,
        )
        .is_err(),
        "unsafe existing {relative} must fail closed"
    );
    assert_eq!(
        fs::read(&unsafe_file).unwrap(),
        b"unsafe preseeded mutable state"
    );
}

#[test]
fn concurrent_same_wire_publication_has_one_creator_and_one_exact_replay() {
    let fixture = Arc::new(Fixture::new());
    let output = fixture.output("concurrent");
    let barrier = Arc::new(Barrier::new(3));
    let mut workers = Vec::new();
    for _ in 0..2 {
        let fixture = Arc::clone(&fixture);
        let output = output.clone();
        let barrier = Arc::clone(&barrier);
        workers.push(thread::spawn(move || {
            barrier.wait();
            provision_zec_maker_actor_from_chat(
                &fixture.maker_config,
                &fixture.final_wire,
                fixture.accepted_at,
                &output,
            )
        }));
    }
    barrier.wait();
    let mut results = workers
        .into_iter()
        .map(|worker| worker.join().expect("publisher thread").expect("publish"))
        .collect::<Vec<_>>();
    results.sort_by_key(zec_reference_actor::ZecMakerActorProvisionV1::was_replay);
    assert!(!results[0].was_replay());
    assert!(results[1].was_replay());
    assert_eq!(results[0].config_file(), results[1].config_file());
    assert_eq!(
        fs::read(results[0].config_file()).unwrap(),
        fs::read(results[1].config_file()).unwrap()
    );
    assert!(!output.join("taker").exists());
    assert_eq!(
        fs::read_dir(fixture.root.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".lez-maker-actor-stage-")
            })
            .count(),
        0,
        "successful race must not leak staging roots"
    );
}

struct Fixture {
    root: TempDir,
    maker_config: PathBuf,
    taker_config: PathBuf,
    final_wire: Vec<u8>,
    accepted_at: UnixSeconds,
}

impl Fixture {
    fn new() -> Self {
        let root = TempDir::new().expect("temporary provisioner root");
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700))
            .expect("make provisioner root owner-private");
        let source = root.path().join("source");
        private_directory(&source);
        let accepted_at = UnixSeconds::new(now());
        let maker_secret = secret(8);
        let taker_secret = secret(2);
        let source_wire = agreement_wire(
            &maker_secret,
            &taker_secret,
            NegotiationTranscriptV1::new([9; 32], [10; 32], accepted_at.value() + 300),
            accepted_at,
        );
        let final_wire = agreement_wire(
            &maker_secret,
            &taker_secret,
            NegotiationTranscriptV1::new([11; 32], [12; 32], accepted_at.value() + 300),
            accepted_at,
        );
        let agreement_file = source.join("agreement-v2.borsh");
        private_file(&agreement_file, &source_wire);

        let maker_config = source.join("maker-config.json");
        let taker_config = source.join("taker-config.json");
        write_role_config(
            &source,
            &maker_config,
            &agreement_file,
            &source_wire,
            "maker",
            &maker_secret,
            true,
        );
        write_role_config(
            &source,
            &taker_config,
            &agreement_file,
            &source_wire,
            "taker",
            &taker_secret,
            false,
        );
        ActorConfig::load_private(&maker_config)
            .expect("Maker source config")
            .load_activate_material()
            .expect("Maker source authority");
        ActorConfig::load_private(&taker_config)
            .expect("Taker source config")
            .load_activate_material()
            .expect("Taker source authority");
        Self {
            root,
            maker_config,
            taker_config,
            final_wire,
            accepted_at,
        }
    }

    fn output(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }
}

#[allow(clippy::too_many_arguments)]
fn write_role_config(
    root: &Path,
    config_file: &Path,
    agreement_file: &Path,
    agreement_wire: &[u8],
    role: &str,
    zcash_secret: &SecretKey,
    funder: bool,
) {
    let prefix = root.join(role);
    let claim_key = prefix.with_extension("claim-key");
    let preimage = prefix.with_extension("preimage");
    let zcash_key = prefix.with_extension("zcash-key");
    let capability = prefix.with_extension("capability");
    private_file(&claim_key, &[if role == "maker" { 0x71 } else { 0x72 }; 32]);
    if funder {
        private_file(&preimage, &PREIMAGE);
    }
    private_file(&zcash_key, &zcash_secret.secret_bytes());
    private_file(&capability, b"maker_provision_test_capability_1234");
    let sidecar_port = if role == "maker" { 19_001 } else { 19_002 };
    let signer = if role == "maker" {
        "03".repeat(32)
    } else {
        "04".repeat(32)
    };
    let config = json!({
        "schema_version": 3,
        "role": role,
        "run_id": "maker-provision-test",
        "swap_id": SWAP_ID,
        "signed_agreement_file": agreement_file,
        "signed_agreement_sha256": hex::encode(Sha256::digest(agreement_wire)),
        "role_state_db": root.join(format!("{role}-source-state.sqlite3")),
        "claim_recovery": {
            "key_id": format!("{role}-claim-key-v1"),
            "key_file": claim_key
        },
        "claim_preimage_file": funder.then_some(preimage),
        "zcash_key_file": zcash_key,
        "bridge": {
            "endpoint": format!("http://127.0.0.1:{sidecar_port}"),
            "journal_db": root.join(format!("{role}-source-bridge.sqlite3")),
            "capability_file": capability,
            "runtime": {
                "sidecar_role": role,
                "compatibility": "lee_v0_2_0",
                "chain_id": "06".repeat(32),
                "channel_id": "08".repeat(32),
                "genesis_block_hash": "07".repeat(32),
                "escrow_program_id": "01000000".repeat(8),
                "signer_account_id": signer
            },
            "request_timeout_millis": 5000
        },
        "zebra": {
            "route": {
                "kind": "deterministic_local",
                "endpoint": "http://127.0.0.1:19101",
                "cookie_file": null
            },
            "identity": {
                "network": "regtest",
                "rpc_chain": "test",
                "consensus_branch_id": "c8e71055",
                "genesis_hash": "77".repeat(32)
            },
            "counterparty_scan_blocks": 1000
        },
        "lez_discovery_window": {"start_height": 1, "max_blocks": 256},
        "zcash_funding_outpoints": if funder {
            json!([{"transaction_id": "aa".repeat(32), "output_index": 0}])
        } else {
            json!([])
        }
    });
    private_file(
        config_file,
        &serde_json::to_vec_pretty(&config).expect("config JSON"),
    );
}

fn agreement_wire(
    maker_secret: &SecretKey,
    taker_secret: &SecretKey,
    transcript: NegotiationTranscriptV1,
    now: UnixSeconds,
) -> Vec<u8> {
    let maker_public = public_key(maker_secret);
    let taker_public = public_key(taker_secret);
    let maker_hash = pubkey_hash(&maker_public);
    let taker_hash = pubkey_hash(&taker_public);
    let escrow_program = [1; 8];
    let onchain_swap_id = derive_lez_swap_id_v1(SWAP_ID.as_bytes());
    let metadata = derive_lez_metadata_account_v1(&escrow_program, &onchain_swap_id);
    let custody = derive_lez_native_custody_account_v1(&escrow_program, &onchain_swap_id);
    let secret_digest: [u8; 32] = Sha256::digest(PREIMAGE).into();
    let binding = ZecSwapBinding::new(
        ZecProfileId::DeterministicLocalV1,
        ExpectedBip199Output::new(
            NetworkType::Regtest,
            BranchId::Nu6_2,
            Zatoshis::from_u64(10_000).unwrap(),
            Bip199Contract::new(120, maker_hash, secret_digest, taker_hash),
        ),
    )
    .unwrap();
    let body = ZecAgreementBodyV1::new(
        SWAP_ID.to_owned(),
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
        ZecRefundPlanV1::new(
            now.value(),
            116,
            (now.value() + 60) * 1_000,
            now.value() + 90,
        ),
        transcript,
    );
    let commitment = body.commitment();
    let record = ZecAgreementRecordV1::from_parts(
        ZEC_CONCRETE_AGREEMENT_SCHEMA_V2,
        body,
        commitment,
        sign(commitment, maker_secret),
        sign(commitment, taker_secret),
    );
    ZecAgreementV1::validate_at(record, now)
        .expect("valid agreement")
        .encode_wire()
        .expect("agreement wire")
}

fn sign(commitment: [u8; 32], secret: &SecretKey) -> [u8; 64] {
    let mut signature =
        Secp256k1::signing_only().sign_ecdsa(&Message::from_digest(commitment), secret);
    signature.normalize_s();
    signature.serialize_compact()
}

fn secret(byte: u8) -> SecretKey {
    SecretKey::from_slice(&[byte; 32]).unwrap()
}

fn public_key(secret: &SecretKey) -> PublicKey {
    PublicKey::from_secret_key(&Secp256k1::signing_only(), secret)
}

fn pubkey_hash(public: &PublicKey) -> [u8; 20] {
    match TransparentAddress::from_pubkey(public) {
        TransparentAddress::PublicKeyHash(hash) => hash,
        TransparentAddress::ScriptHash(_) => unreachable!("public key creates P2PKH"),
    }
}

fn private_directory(path: &Path) {
    fs::DirBuilder::new()
        .mode(0o700)
        .create(path)
        .expect("create private directory");
}

fn private_file(path: &Path, bytes: &[u8]) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .expect("create private file");
    file.write_all(bytes).expect("write private file");
    file.sync_all().expect("sync private file");
}

fn assert_private_directory(path: &Path) {
    let metadata = fs::symlink_metadata(path).expect("private directory metadata");
    assert!(metadata.file_type().is_dir());
    assert_eq!(metadata.permissions().mode() & 0o7777, 0o700);
}

fn assert_private_file(path: &Path) {
    let metadata = fs::symlink_metadata(path).expect("private file metadata");
    assert!(metadata.file_type().is_file());
    assert_eq!(metadata.permissions().mode() & 0o7777, 0o600);
    assert_eq!(metadata.nlink(), 1);
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_secs()
}
