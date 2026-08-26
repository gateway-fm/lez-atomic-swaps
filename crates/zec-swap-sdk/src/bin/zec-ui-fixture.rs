//! Emits the deterministic local ZEC corridor fixture used for UI-initiated
//! swap preparation. Mirrors the upstream `zec_chat_process` service test's
//! `actor_deployment` fixture (maker claim keys + source actor config +
//! countersigned agreement wire), so Chat acceptance works without a live
//! Zebra/LEZ corridor.

use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use clap::Parser;
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
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use zcash_protocol::{
    consensus::{BranchId, NetworkType},
    value::Zatoshis,
};
use zcash_transparent::address::TransparentAddress;

const CLAIM_PREIMAGE: [u8; 32] = [0x44; 32];

#[derive(Parser)]
struct Arguments {
    /// New owner-private output root for the fixture tree.
    #[arg(long)]
    output_root: PathBuf,
    /// Exact swap id bound into the agreement.
    #[arg(long)]
    swap_id: String,
    /// Validity window for the source agreement used during Chat acceptance.
    #[arg(long, default_value_t = 7_200)]
    valid_for_seconds: u64,
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    if path.exists() {
        bail!("refusing to replace {}", path.display());
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("create {}", path.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn write_raw_key(path: &Path, byte: u8) -> Result<()> {
    write_private(path, &[byte; 32])
}

fn pubkey_hash(public: &PublicKey) -> [u8; 20] {
    match TransparentAddress::from_pubkey(public) {
        TransparentAddress::PublicKeyHash(hash) => hash,
        TransparentAddress::ScriptHash(_) => unreachable!(),
    }
}

fn sign(commitment: [u8; 32], secret: &SecretKey) -> [u8; 64] {
    let mut signature =
        Secp256k1::signing_only().sign_ecdsa(&Message::from_digest(commitment), secret);
    signature.normalize_s();
    signature.serialize_compact()
}

struct FixturePaths {
    maker: PathBuf,
    taker: PathBuf,
    shared: PathBuf,
}

impl FixturePaths {
    fn create(root: &Path) -> Result<Self> {
        if root.exists() {
            bail!("refusing to replace {}", root.display());
        }
        let paths = Self {
            maker: root.join("maker"),
            taker: root.join("taker"),
            shared: root.join("shared"),
        };
        fs::DirBuilder::new().mode(0o700).create(root)?;
        for directory in [&paths.maker, &paths.taker, &paths.shared] {
            fs::DirBuilder::new().mode(0o700).create(directory)?;
        }
        Ok(paths)
    }
}

struct FixtureKeys {
    maker_secret: SecretKey,
    taker_secret: SecretKey,
    maker_public: PublicKey,
    taker_public: PublicKey,
    maker_hash: [u8; 20],
    taker_hash: [u8; 20],
}

impl FixtureKeys {
    fn deterministic() -> Self {
        let secp = Secp256k1::signing_only();
        let maker_secret = SecretKey::from_slice(&[8u8; 32]).unwrap();
        let taker_secret = SecretKey::from_slice(&[2u8; 32]).unwrap();
        let maker_public = PublicKey::from_secret_key(&secp, &maker_secret);
        let taker_public = PublicKey::from_secret_key(&secp, &taker_secret);
        Self {
            maker_hash: pubkey_hash(&maker_public),
            taker_hash: pubkey_hash(&taker_public),
            maker_secret,
            taker_secret,
            maker_public,
            taker_public,
        }
    }
}

fn write_authority_material(paths: &FixturePaths) -> Result<()> {
    write_raw_key(&paths.maker.join("claim-recovery.key"), 0x7a)?;
    write_private(&paths.maker.join("claim-preimage.key"), &CLAIM_PREIMAGE)?;
    write_raw_key(&paths.maker.join("zcash.key"), 8)?;
    write_raw_key(&paths.taker.join("zcash.key"), 2)?;
    write_private(
        &paths.maker.join("bridge.capability"),
        b"ui_fixture_capability_0123456789",
    )
}

fn create_agreement(arguments: &Arguments, basis_time: u64, keys: &FixtureKeys) -> Result<Vec<u8>> {
    let escrow_program = [1u32; 8];
    let onchain_swap_id = derive_lez_swap_id_v1(arguments.swap_id.as_bytes());
    let secret_digest: [u8; 32] = Sha256::digest(CLAIM_PREIMAGE).into();
    let binding = ZecSwapBinding::new(
        ZecProfileId::DeterministicLocalV1,
        ExpectedBip199Output::new(
            NetworkType::Regtest,
            BranchId::Nu6_2,
            Zatoshis::from_u64(10_000)?,
            Bip199Contract::new(120, keys.maker_hash, secret_digest, keys.taker_hash),
        ),
    )?;
    let body = ZecAgreementBodyV1::new(
        arguments.swap_id.clone(),
        SwapDirection::TakerSellsLez,
        ZecProfileRecordV1::from(ZecProfileId::DeterministicLocalV1),
        ZecParticipantsV1::new(
            ZecParticipantIdentityV1::new([3; 32], keys.maker_public.serialize()),
            ZecParticipantIdentityV1::new([4; 32], keys.taker_public.serialize()),
        ),
        secret_digest,
        ZecLezTermsV1::new(
            LezChainIdentityV1::new(LezEnvironmentV1::DeterministicLocalV0_2, [8; 32], [7; 32]),
            escrow_program,
            LezAssetV1::Native {
                authenticated_transfer_program_id: [2u32; 8],
            },
            25_000,
            derive_lez_metadata_account_v1(&escrow_program, &onchain_swap_id),
            derive_lez_native_custody_account_v1(&escrow_program, &onchain_swap_id),
        ),
        ZecSwapBindingRecordV1::from_binding(&binding),
        ZecTransactionPolicyV1::new(
            [12; 32],
            ZcashTransparentDestinationV1::p2pkh(keys.maker_hash),
            1,
            1,
            ZcashTransparentDestinationV1::p2pkh(keys.taker_hash),
            1,
            ZcashTransparentDestinationV1::p2pkh(keys.maker_hash),
            1,
            40,
        ),
        ZecRefundPlanV1::new(basis_time, 116, (basis_time + 60) * 1_000, basis_time + 90),
        NegotiationTranscriptV1::new([9; 32], [10; 32], basis_time + arguments.valid_for_seconds),
    );
    let commitment = body.commitment();
    let record = ZecAgreementRecordV1::from_parts(
        ZEC_CONCRETE_AGREEMENT_SCHEMA_V2,
        body,
        commitment,
        sign(commitment, &keys.maker_secret),
        sign(commitment, &keys.taker_secret),
    );
    Ok(ZecAgreementV1::validate_at(record, UnixSeconds::new(basis_time))?.encode_wire()?)
}

fn create_maker_config(
    arguments: &Arguments,
    paths: &FixturePaths,
    agreement_sha256: &str,
) -> Value {
    json!({
        "schema_version": 3,
        "role": "maker",
        "run_id": "ui-fixture-authority",
        "swap_id": arguments.swap_id,
        "signed_agreement_file": paths.shared.join("agreement-v2.borsh"),
        "signed_agreement_sha256": agreement_sha256,
        "role_state_db": paths.maker.join("unused-source-state.sqlite3"),
        "claim_recovery": {
            "key_id": "ui-zec-claim",
            "key_file": paths.maker.join("claim-recovery.key")
        },
        "claim_preimage_file": paths.maker.join("claim-preimage.key"),
        "zcash_key_file": paths.maker.join("zcash.key"),
        "bridge": {
            "endpoint": "http://127.0.0.1:19001",
            "journal_db": paths.maker.join("unused-source-bridge.sqlite3"),
            "capability_file": paths.maker.join("bridge.capability"),
            "runtime": {
                "sidecar_role": "maker",
                "compatibility": "lee_v0_2_0",
                "chain_id": "06".repeat(32),
                "channel_id": "08".repeat(32),
                "genesis_block_hash": "07".repeat(32),
                "escrow_program_id": "01".repeat(32),
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
    })
}

fn create_taker_config(paths: &FixturePaths, maker_config: &Value) -> Result<Value> {
    let taker_claim_key = paths.taker.join("actor-claim-recovery.key");
    let taker_capability = paths.taker.join("actor-bridge.capability");
    write_raw_key(&taker_claim_key, 0x7b)?;
    write_private(&taker_capability, b"m5_taker_actor_capability_0123456789")?;
    let mut config = maker_config.clone();
    config["role"] = json!("taker");
    config["role_state_db"] = json!(paths.taker.join("unused-taker-source-state.sqlite3"));
    config["claim_recovery"]["key_id"] = json!("ui-zec-taker-claim");
    config["claim_recovery"]["key_file"] = json!(taker_claim_key);
    config["claim_preimage_file"] = Value::Null;
    config["zcash_key_file"] = json!(paths.taker.join("zcash.key"));
    config["bridge"]["endpoint"] = json!("http://127.0.0.1:19002");
    config["bridge"]["journal_db"] = json!(paths.taker.join("unused-taker-source-bridge.sqlite3"));
    config["bridge"]["capability_file"] = json!(taker_capability);
    config["bridge"]["runtime"]["sidecar_role"] = json!("taker");
    config["bridge"]["runtime"]["signer_account_id"] = json!("04".repeat(32));
    config["zcash_funding_outpoints"] = json!([]);
    Ok(config)
}

fn main() -> Result<()> {
    let arguments = Arguments::parse();
    let basis_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let paths = FixturePaths::create(&arguments.output_root)?;
    let keys = FixtureKeys::deterministic();
    write_authority_material(&paths)?;

    let agreement = create_agreement(&arguments, basis_time, &keys)?;
    write_private(&paths.shared.join("agreement-v2.borsh"), &agreement)?;
    let agreement_sha256 = hex::encode(Sha256::digest(&agreement));
    let maker_config = create_maker_config(&arguments, &paths, &agreement_sha256);
    write_private(
        &paths.maker.join("actor-config.json"),
        &serde_json::to_vec_pretty(&maker_config)?,
    )?;
    let taker_config = create_taker_config(&paths, &maker_config)?;
    write_private(
        &paths.taker.join("actor-config.json"),
        &serde_json::to_vec_pretty(&taker_config)?,
    )?;

    println!(
        "{}",
        json!({
            "schema_version": 1,
            "agreement_file": paths.shared.join("agreement-v2.borsh"),
            "maker_config": paths.maker.join("actor-config.json"),
            "taker_config": paths.taker.join("actor-config.json"),
            "taker_zcash_key": paths.taker.join("zcash.key"),
            "private_material_disclosed": false
        })
    );
    Ok(())
}
