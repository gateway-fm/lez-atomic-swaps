use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use lez_swap_core::{SwapDirection, UnixSeconds};
use lez_swap_store::validate_maker_actor_program;
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
use zcash_protocol::{
    consensus::{BranchId, NetworkType},
    value::Zatoshis,
};
use zcash_transparent::address::TransparentAddress;
use zec_reference_actor::{ActorConfig, ActorRole};

const CLAIM_PREIMAGE: [u8; 32] = [0x44; 32];

#[allow(dead_code)]
pub struct ActorDeployment {
    pub root: PathBuf,
    pub source_config: PathBuf,
    pub program: PathBuf,
    pub program_sha256: String,
    pub claim_key: PathBuf,
    pub claim_preimage: PathBuf,
    pub agreement_basis_time: u64,
}

pub fn actor_deployment(run_root: &Path, swap_id: &str) -> ActorDeployment {
    let source_root = run_root.join("actor-source");
    let root = run_root.join("maker-actors");
    let source_config = source_root.join("actor-config.json");
    let program = fs::canonicalize("/usr/bin/true").unwrap();
    let program_identity: [u8; 32] = Sha256::digest(fs::read(&program).unwrap()).into();
    validate_maker_actor_program(&program, program_identity)
        .expect("fixture program satisfies production artifact policy");
    let agreement_basis_time = now();

    if !source_config.exists() {
        for directory in [&source_root, &root] {
            fs::DirBuilder::new().mode(0o700).create(directory).unwrap();
        }
        let agreement = agreement_wire(agreement_basis_time, swap_id);
        let agreement_file = source_root.join("agreement-v2.borsh");
        let claim_key = source_root.join("claim-recovery.key");
        let preimage = source_root.join("claim-preimage.key");
        let zcash_key = source_root.join("zcash.key");
        let capability = source_root.join("bridge.capability");
        write_private(&agreement_file, &agreement);
        write_raw_key(&claim_key, 0x7a);
        write_raw_key(&preimage, CLAIM_PREIMAGE[0]);
        write_raw_key(&zcash_key, 8);
        write_private(&capability, b"m5_actor_capability_0123456789abcdef");
        let config = json!({
            "schema_version": 3,
            "role": "maker",
            "run_id": "m5-integration-authority",
            "swap_id": swap_id,
            "signed_agreement_file": agreement_file,
            "signed_agreement_sha256": hex::encode(Sha256::digest(&agreement)),
            "role_state_db": source_root.join("unused-source-state.sqlite3"),
            "claim_recovery": {
                "key_id": "m5-integration-authority-claim-v1",
                "key_file": claim_key
            },
            "claim_preimage_file": preimage,
            "zcash_key_file": zcash_key,
            "bridge": {
                "endpoint": "http://127.0.0.1:19001",
                "journal_db": source_root.join("unused-source-bridge.sqlite3"),
                "capability_file": capability,
                "runtime": {
                    "sidecar_role": "maker",
                    "compatibility": "lee_v0_2_0",
                    "chain_id": "06".repeat(32),
                    "channel_id": "08".repeat(32),
                    "genesis_block_hash": "07".repeat(32),
                    "escrow_program_id": "01000000".repeat(8),
                    "signer_account_id": "03".repeat(32)
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
            "zcash_funding_outpoints": [{
                "transaction_id": "aa".repeat(32),
                "output_index": 0
            }]
        });
        write_private(&source_config, &serde_json::to_vec_pretty(&config).unwrap());
    }

    let loaded = ActorConfig::load_private(&source_config).expect("valid source Maker config");
    assert_eq!(loaded.role(), ActorRole::Maker);
    loaded
        .load_activate_material()
        .expect("source Maker activation material");
    ActorDeployment {
        root,
        source_config,
        program,
        program_sha256: hex::encode(program_identity),
        claim_key: source_root.join("claim-recovery.key"),
        claim_preimage: source_root.join("claim-preimage.key"),
        agreement_basis_time,
    }
}

fn agreement_wire(basis_time: u64, swap_id: &str) -> Vec<u8> {
    let maker_secret = SecretKey::from_slice(&[8; 32]).unwrap();
    let taker_secret = SecretKey::from_slice(&[2; 32]).unwrap();
    let maker_public = PublicKey::from_secret_key(&Secp256k1::signing_only(), &maker_secret);
    let taker_public = PublicKey::from_secret_key(&Secp256k1::signing_only(), &taker_secret);
    let maker_hash = pubkey_hash(&maker_public);
    let taker_hash = pubkey_hash(&taker_public);
    let escrow_program = [1; 8];
    let onchain_swap_id = derive_lez_swap_id_v1(swap_id.as_bytes());
    let secret_digest: [u8; 32] = Sha256::digest(CLAIM_PREIMAGE).into();
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
        swap_id.to_owned(),
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
            derive_lez_metadata_account_v1(&escrow_program, &onchain_swap_id),
            derive_lez_native_custody_account_v1(&escrow_program, &onchain_swap_id),
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
        ZecRefundPlanV1::new(basis_time, 116, (basis_time + 60) * 1_000, basis_time + 90),
        NegotiationTranscriptV1::new([9; 32], [10; 32], basis_time + 300),
    );
    let commitment = body.commitment();
    let record = ZecAgreementRecordV1::from_parts(
        ZEC_CONCRETE_AGREEMENT_SCHEMA_V2,
        body,
        commitment,
        sign(commitment, &maker_secret),
        sign(commitment, &taker_secret),
    );
    ZecAgreementV1::validate_at(record, UnixSeconds::new(basis_time))
        .unwrap()
        .encode_wire()
        .unwrap()
}

fn sign(commitment: [u8; 32], secret: &SecretKey) -> [u8; 64] {
    let mut signature =
        Secp256k1::signing_only().sign_ecdsa(&Message::from_digest(commitment), secret);
    signature.normalize_s();
    signature.serialize_compact()
}

fn pubkey_hash(public: &PublicKey) -> [u8; 20] {
    match TransparentAddress::from_pubkey(public) {
        TransparentAddress::PublicKeyHash(hash) => hash,
        TransparentAddress::ScriptHash(_) => unreachable!(),
    }
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

fn write_raw_key(path: &Path, byte: u8) {
    write_private(path, &[byte; 32]);
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
