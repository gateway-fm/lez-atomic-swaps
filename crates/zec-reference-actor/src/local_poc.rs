use std::{
    fs::{self, DirBuilder, OpenOptions},
    io::{Cursor, Write as _},
    os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _},
    path::{Component, Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context as _, Result, bail, ensure};
use lez_bridge_protocol::{
    DiscoveryWindow, Hex32, Participant as BridgeParticipant, RunId, RuntimeCompatibility,
    RuntimeDescriptor,
};
use lez_swap_core::{SwapDirection, SwapId, UnixSeconds};
use lez_zebra_node_adapter::{
    HttpZebraRpc, HttpZebraRpcConfig, ZebraChainIdentity, ZebraRpc, ZebraRpcChain,
};
use lez_zec_swap_sdk::{
    Bip199Contract, ExpectedBip199Output, LezAssetV1, LezChainIdentityV1, LezEnvironmentV1,
    NegotiationTranscriptV1, ZEC_CONCRETE_AGREEMENT_SCHEMA_V2, ZcashFundingInputSetV1,
    ZcashFundingInputV1, ZcashTransparentDestinationV1, ZecAgreementBodyV1, ZecAgreementRecordV1,
    ZecAgreementV1, ZecLezTermsV1, ZecParticipantIdentityV1, ZecParticipantsV1, ZecProfileId,
    ZecProfileRecordV1, ZecRefundPlanV1, ZecRefundProfile, ZecSwapBinding, ZecSwapBindingRecordV1,
    ZecTransactionPolicyV1, derive_lez_metadata_account_v1, derive_lez_native_custody_account_v1,
    derive_lez_swap_id_v1,
};
use secp256k1::{Message, PublicKey, Secp256k1, SecretKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use url::Url;
use zcash_primitives::{block::BlockHash, transaction::Transaction};
use zcash_protocol::{
    TxId,
    consensus::{BlockHeight, BranchId, NetworkType},
    value::Zatoshis,
};
use zcash_transparent::{
    address::{Script, TransparentAddress},
    bundle::{OutPoint, TxOut},
};
use zeroize::Zeroizing;

use crate::config::{
    DeterministicLocalV0_2ActorConfigInput, encode_deterministic_local_v0_2_actor_config,
};
use crate::secure_file::{FilePrivacy, read_bounded_identified};
use crate::{ActorConfig, ActorRole, CandidateOutpoint, validate_actor_pair};

const SPEC_SCHEMA_VERSION: u16 = 1;
const MAX_SPEC_BYTES: usize = 32 * 1024;
const MAKER_LEZ_ACCOUNT: &str = "B1UN3hPgxacgHKBRoThcAmsPajGcUf6YXUhgB36x4DAd";
const TAKER_LEZ_ACCOUNT: &str = "34Kqgek6R7N1zU5FSJz8ziXwSPEPCuWGcn1T7GCVrfib";
const AUTHENTICATED_TRANSFER_PROGRAM: &str = "FrexXMbyY6iZjwUo8DV3jfB8donj8H4kLRHT7xswCfJg";
const FUNDED_ZCASH_KEY_BYTE: u8 = 4;
const OTHER_ZCASH_KEY_BYTE: u8 = 2;
const PREIMAGE_BYTE: u8 = 0x44;
const LEZ_NATIVE_AMOUNT: u128 = 50_000;
const ZCASH_PRINCIPAL_ZATOSHIS: u64 = 100_000_000;
const FUNDING_FEE_ZATOSHIS: u64 = 10_000;
const MINIMUM_CHANGE_ZATOSHIS: u64 = 1_000;
const CLAIM_FEE_ZATOSHIS: u64 = 10_000;
const REFUND_FEE_ZATOSHIS: u64 = 10_000;
const COINBASE_MATURITY_CONFIRMATIONS: u32 = 100;
const AGREEMENT_LIFETIME_SECONDS: u64 = 3_600;
const COUNTERPARTY_SCAN_BLOCKS: u32 = 1_024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProvisionSpec {
    schema_version: u16,
    run_id: RunId,
    swap_id: String,
    direction: LocalPocDirection,
    lez_runtime: LezRuntimeSpec,
    bridge: BridgeSpec,
    zebra_endpoint: Url,
    lez_discovery_start_height: u64,
    lez_discovery_max_blocks: u32,
}

#[derive(Clone, Copy, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
enum LocalPocDirection {
    TakerSellsForeign,
    TakerSellsLez,
}

impl LocalPocDirection {
    const fn protocol(self) -> SwapDirection {
        match self {
            Self::TakerSellsForeign => SwapDirection::TakerSellsForeign,
            Self::TakerSellsLez => SwapDirection::TakerSellsLez,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::TakerSellsForeign => "taker_sells_foreign",
            Self::TakerSellsLez => "taker_sells_lez",
        }
    }

    const fn zcash_funder(self) -> ActorRole {
        match self {
            Self::TakerSellsForeign => ActorRole::Taker,
            Self::TakerSellsLez => ActorRole::Maker,
        }
    }

    const fn lez_depositor(self) -> ActorRole {
        match self {
            Self::TakerSellsForeign => ActorRole::Maker,
            Self::TakerSellsLez => ActorRole::Taker,
        }
    }

    const fn preimage_owner(self) -> ActorRole {
        self.zcash_funder()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LezRuntimeSpec {
    chain_id: Hex32,
    channel_id: Hex32,
    genesis_block_hash: Hex32,
    escrow_program_id: Hex32,
    authenticated_transfer_program_id_base58: String,
    maker_signer_account_id_base58: String,
    taker_signer_account_id_base58: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BridgeSpec {
    maker_endpoint: Url,
    taker_endpoint: Url,
}

/// Secret-free result of provisioning one pair of local actor inputs.
#[derive(Debug, Serialize)]
pub struct LocalPocProvisionSummary {
    schema_version: u16,
    run_id: RunId,
    direction: &'static str,
    agreement_file: PathBuf,
    signed_agreement_sha256: Hex32,
    authenticated_transfer_program_id: Hex32,
    authenticated_transfer_program_id_words: [u32; 8],
    maker: RolePaths,
    taker: RolePaths,
    zebra_tip_height: u32,
    zcash_candidate_owner: &'static str,
    lez_native_amount: u128,
    lez_depositor_role: &'static str,
    lez_depositor_account_id_base58: String,
    private_material_disclosed: bool,
    actor_pair_validated: bool,
}

#[derive(Debug, Serialize)]
struct RolePaths {
    state_root: PathBuf,
    config_file: PathBuf,
    runtime_file: PathBuf,
    lez_signer_key_file: PathBuf,
    sidecar_capability_file: PathBuf,
}

struct SelectedCandidate {
    outpoint: OutPoint,
    output: TxOut,
    rpc_transaction_id: Hex32,
}

struct AgreementFixture {
    wire: Zeroizing<Vec<u8>>,
    sha256: Hex32,
    maker_zcash_key: Zeroizing<[u8; 32]>,
    taker_zcash_key: Zeroizing<[u8; 32]>,
    preimage: Zeroizing<[u8; 32]>,
    zcash_candidate: CandidateOutpoint,
}

/// Provisions a shared countersigned agreement and two isolated actor inputs.
///
/// The spec selects either supported ZEC direction. The taker always funds the
/// first leg; the participant funding Zcash exclusively receives the preimage
/// and exact mature Regtest candidate, while the opposite participant funds
/// LEZ. Both local identities use the deterministic keys already funded by the
/// retained v0.2 and Zebra fixtures.
///
/// # Errors
///
/// Fails closed for an unsafe spec, unexpected deterministic runtime identity,
/// non-Regtest Zebra, unstable RPC view, missing mature actor-owned output,
/// invalid agreement, unsafe output path, file collision, or actor-pair check.
#[allow(clippy::too_many_lines)]
pub async fn provision_local_v0_2_corridor(
    spec_file: &Path,
    output_root: &Path,
) -> Result<LocalPocProvisionSummary> {
    let spec = load_spec(spec_file)?;
    validate_spec(&spec)?;
    validate_new_output_root(output_root)?;

    let maker_lez_account = decode_base58_32(&spec.lez_runtime.maker_signer_account_id_base58)
        .context("invalid deterministic maker LEZ account")?;
    let taker_lez_account = decode_base58_32(&spec.lez_runtime.taker_signer_account_id_base58)
        .context("invalid deterministic taker LEZ account")?;
    let authenticated_transfer =
        decode_base58_32(&spec.lez_runtime.authenticated_transfer_program_id_base58)
            .context("invalid authenticated-transfer program")?;

    let rpc = HttpZebraRpc::connect(
        &HttpZebraRpcConfig::new(spec.zebra_endpoint.as_str())
            .with_request_timeout(Duration::from_secs(30))
            .with_max_concurrent_requests(1),
    )
    .context("failed to connect bounded local Zebra RPC")?;
    let mut funding_zcash_secret = SecretKey::from_slice(&[FUNDED_ZCASH_KEY_BYTE; 32])
        .context("invalid deterministic Zcash funder key")?;
    let funding_zcash_public =
        PublicKey::from_secret_key(&Secp256k1::signing_only(), &funding_zcash_secret);
    funding_zcash_secret.non_secure_erase();
    let (candidate, tip_height, genesis_hash) =
        select_mature_regtest_candidate(&rpc, &funding_zcash_public).await?;

    let swap_id = SwapId::new(spec.swap_id.clone()).context("invalid swap ID")?;
    let fixture = build_agreement(
        &spec,
        &swap_id,
        maker_lez_account,
        taker_lez_account,
        authenticated_transfer,
        &candidate,
        tip_height,
    )?;

    create_private_directory(output_root)?;
    let shared_root = output_root.join("shared");
    let maker_root = output_root.join("maker");
    let taker_root = output_root.join("taker");
    for directory in [&shared_root, &maker_root, &taker_root] {
        create_private_directory(directory)?;
    }
    let maker_state = maker_root.join("state");
    let taker_state = taker_root.join("state");
    create_private_directory(&maker_state)?;
    create_private_directory(&taker_state)?;

    let agreement_file = shared_root.join("agreement-v2.borsh");
    write_private_new(&agreement_file, fixture.wire.as_slice())?;

    let maker_paths = material_paths(&maker_root, &maker_state);
    let taker_paths = material_paths(&taker_root, &taker_state);
    let mut maker_recovery = Zeroizing::new([0_u8; 32]);
    let mut taker_recovery = Zeroizing::new([0_u8; 32]);
    getrandom::fill(&mut maker_recovery[..])
        .map_err(|_| anyhow::anyhow!("maker recovery randomness unavailable"))?;
    getrandom::fill(&mut taker_recovery[..])
        .map_err(|_| anyhow::anyhow!("taker recovery randomness unavailable"))?;
    avoid_zero_or_equal(&mut maker_recovery, &mut taker_recovery);
    write_private_new(&maker_paths.claim_recovery_key, maker_recovery.as_ref())?;
    write_private_new(&taker_paths.claim_recovery_key, taker_recovery.as_ref())?;
    write_private_new(&maker_paths.zcash_key, fixture.maker_zcash_key.as_ref())?;
    write_private_new(&taker_paths.zcash_key, fixture.taker_zcash_key.as_ref())?;
    match spec.direction.preimage_owner() {
        ActorRole::Maker => {
            write_private_new(&maker_paths.claim_preimage, fixture.preimage.as_ref())?;
        }
        ActorRole::Taker => {
            write_private_new(&taker_paths.claim_preimage, fixture.preimage.as_ref())?;
        }
    }
    write_private_new(
        &maker_paths.lez_signer_key,
        hex::encode([1_u8; 32]).as_bytes(),
    )?;
    write_private_new(
        &taker_paths.lez_signer_key,
        hex::encode([2_u8; 32]).as_bytes(),
    )?;

    let maker_capability = random_capability()?;
    let mut taker_capability = random_capability()?;
    while maker_capability.as_slice() == taker_capability.as_slice() {
        taker_capability = random_capability()?;
    }
    write_private_new(&maker_paths.capability, maker_capability.as_slice())?;
    write_private_new(&taker_paths.capability, taker_capability.as_slice())?;

    let discovery = DiscoveryWindow::new(
        spec.lez_discovery_start_height,
        spec.lez_discovery_max_blocks,
    )
    .context("invalid LEZ discovery window")?;
    let maker_runtime = runtime_descriptor(
        &spec,
        BridgeParticipant::Maker,
        Hex32::from_bytes(maker_lez_account),
    );
    let taker_runtime = runtime_descriptor(
        &spec,
        BridgeParticipant::Taker,
        Hex32::from_bytes(taker_lez_account),
    );
    write_private_new(
        &maker_paths.runtime,
        &serde_json::to_vec_pretty(&maker_runtime)
            .context("failed to encode maker runtime descriptor")?,
    )?;
    write_private_new(
        &taker_paths.runtime,
        &serde_json::to_vec_pretty(&taker_runtime)
            .context("failed to encode taker runtime descriptor")?,
    )?;
    let zebra_genesis = block_hash_display_hex(genesis_hash);
    let (maker_candidates, taker_candidates) = match spec.direction.zcash_funder() {
        ActorRole::Maker => (vec![fixture.zcash_candidate], Vec::new()),
        ActorRole::Taker => (Vec::new(), vec![fixture.zcash_candidate]),
    };

    let maker_config = encode_deterministic_local_v0_2_actor_config(actor_config_input(
        &spec,
        &swap_id,
        ActorRole::Maker,
        &agreement_file,
        fixture.sha256,
        &maker_paths,
        (spec.direction.preimage_owner() == ActorRole::Maker)
            .then(|| maker_paths.claim_preimage.clone()),
        maker_runtime,
        zebra_genesis,
        discovery,
        maker_candidates,
    ))
    .context("failed to encode maker actor config")?;
    let taker_config = encode_deterministic_local_v0_2_actor_config(actor_config_input(
        &spec,
        &swap_id,
        ActorRole::Taker,
        &agreement_file,
        fixture.sha256,
        &taker_paths,
        (spec.direction.preimage_owner() == ActorRole::Taker)
            .then(|| taker_paths.claim_preimage.clone()),
        taker_runtime,
        zebra_genesis,
        discovery,
        taker_candidates,
    ))
    .context("failed to encode taker actor config")?;
    write_private_new(&maker_paths.config, &maker_config)?;
    write_private_new(&taker_paths.config, &taker_config)?;

    let maker = ActorConfig::load_private(&maker_paths.config)
        .context("provisioned maker config did not reload")?;
    let taker = ActorConfig::load_private(&taker_paths.config)
        .context("provisioned taker config did not reload")?;
    ensure!(
        maker.bridge_runtime().escrow_program_id == spec.lez_runtime.escrow_program_id
            && taker.bridge_runtime().escrow_program_id == spec.lez_runtime.escrow_program_id,
        "provisioned actor runtime changed the checked escrow program"
    );
    validate_actor_pair(&maker, &taker).context("provisioned actor pair is not isolated")?;
    let _maker_material = maker
        .load_activate_material()
        .context("provisioned maker activation material is invalid")?;
    let _taker_material = taker
        .load_activate_material()
        .context("provisioned taker activation material is invalid")?;

    Ok(LocalPocProvisionSummary {
        schema_version: 1,
        run_id: spec.run_id,
        direction: spec.direction.as_str(),
        agreement_file,
        signed_agreement_sha256: fixture.sha256,
        authenticated_transfer_program_id: Hex32::from_bytes(authenticated_transfer),
        authenticated_transfer_program_id_words: program_id_words(Hex32::from_bytes(
            authenticated_transfer,
        )),
        maker: role_summary(&maker_paths),
        taker: role_summary(&taker_paths),
        zebra_tip_height: tip_height,
        zcash_candidate_owner: match spec.direction.zcash_funder() {
            ActorRole::Maker => "maker",
            ActorRole::Taker => "taker",
        },
        lez_native_amount: LEZ_NATIVE_AMOUNT,
        lez_depositor_role: match spec.direction.lez_depositor() {
            ActorRole::Maker => "maker",
            ActorRole::Taker => "taker",
        },
        lez_depositor_account_id_base58: match spec.direction.lez_depositor() {
            ActorRole::Maker => spec.lez_runtime.maker_signer_account_id_base58.clone(),
            ActorRole::Taker => spec.lez_runtime.taker_signer_account_id_base58.clone(),
        },
        private_material_disclosed: false,
        actor_pair_validated: true,
    })
}

struct MaterialPaths {
    state_root: PathBuf,
    config: PathBuf,
    runtime: PathBuf,
    role_state_db: PathBuf,
    claim_recovery_key: PathBuf,
    claim_preimage: PathBuf,
    zcash_key: PathBuf,
    lez_signer_key: PathBuf,
    bridge_journal_db: PathBuf,
    capability: PathBuf,
}

fn material_paths(root: &Path, state_root: &Path) -> MaterialPaths {
    MaterialPaths {
        state_root: state_root.to_path_buf(),
        config: root.join("actor-config.json"),
        runtime: root.join("lez-runtime.json"),
        role_state_db: state_root.join("actor.sqlite3"),
        claim_recovery_key: root.join("claim-recovery.key"),
        claim_preimage: root.join("claim-preimage.key"),
        zcash_key: root.join("zcash.key"),
        lez_signer_key: root.join("lez-signer.key"),
        bridge_journal_db: state_root.join("bridge.sqlite3"),
        capability: root.join("sidecar.capability"),
    }
}

#[allow(clippy::too_many_arguments)]
fn actor_config_input(
    spec: &ProvisionSpec,
    swap_id: &SwapId,
    role: ActorRole,
    agreement_file: &Path,
    agreement_sha256: Hex32,
    paths: &MaterialPaths,
    claim_preimage_file: Option<PathBuf>,
    runtime: RuntimeDescriptor,
    zebra_genesis_hash: Hex32,
    discovery: DiscoveryWindow,
    candidates: Vec<CandidateOutpoint>,
) -> DeterministicLocalV0_2ActorConfigInput {
    let (endpoint, role_name) = match role {
        ActorRole::Maker => (spec.bridge.maker_endpoint.clone(), "maker"),
        ActorRole::Taker => (spec.bridge.taker_endpoint.clone(), "taker"),
    };
    DeterministicLocalV0_2ActorConfigInput {
        role,
        run_id: spec.run_id.clone(),
        swap_id: swap_id.clone(),
        signed_agreement_file: agreement_file.to_path_buf(),
        signed_agreement_sha256: agreement_sha256,
        role_state_db: paths.role_state_db.clone(),
        claim_recovery_key_id: format!("{}-{role_name}-claim", spec.run_id.as_str()).into(),
        claim_recovery_key_file: paths.claim_recovery_key.clone(),
        claim_preimage_file,
        zcash_key_file: paths.zcash_key.clone(),
        bridge_endpoint: endpoint,
        bridge_journal_db: paths.bridge_journal_db.clone(),
        bridge_capability_file: paths.capability.clone(),
        bridge_runtime: runtime,
        zebra_endpoint: spec.zebra_endpoint.clone(),
        zebra_genesis_hash,
        counterparty_scan_blocks: COUNTERPARTY_SCAN_BLOCKS,
        lez_discovery_window: discovery,
        zcash_funding_outpoints: candidates,
    }
}

fn runtime_descriptor(
    spec: &ProvisionSpec,
    role: BridgeParticipant,
    signer_account_id: Hex32,
) -> RuntimeDescriptor {
    RuntimeDescriptor::new(
        role,
        RuntimeCompatibility::LeeV0_2_0,
        spec.lez_runtime.chain_id,
        spec.lez_runtime.channel_id,
        spec.lez_runtime.genesis_block_hash,
        spec.lez_runtime.escrow_program_id,
        signer_account_id,
    )
}

fn role_summary(paths: &MaterialPaths) -> RolePaths {
    RolePaths {
        state_root: paths.state_root.clone(),
        config_file: paths.config.clone(),
        runtime_file: paths.runtime.clone(),
        lez_signer_key_file: paths.lez_signer_key.clone(),
        sidecar_capability_file: paths.capability.clone(),
    }
}

fn load_spec(path: &Path) -> Result<ProvisionSpec> {
    let (bytes, _) = read_bounded_identified(path, MAX_SPEC_BYTES, FilePrivacy::OwnerPrivate)
        .map_err(|_| anyhow::anyhow!("local PoC spec is unavailable or unsafe"))?;
    serde_json::from_slice(bytes.as_slice()).context("local PoC spec is invalid")
}

fn validate_spec(spec: &ProvisionSpec) -> Result<()> {
    ensure!(
        spec.schema_version == SPEC_SCHEMA_VERSION,
        "unsupported local PoC spec schema"
    );
    ensure!(
        spec.lez_runtime.maker_signer_account_id_base58 == MAKER_LEZ_ACCOUNT
            && spec.lez_runtime.taker_signer_account_id_base58 == TAKER_LEZ_ACCOUNT,
        "LEZ signer accounts do not match the funded deterministic v0.2 actors"
    );
    ensure!(
        spec.lez_runtime.authenticated_transfer_program_id_base58 == AUTHENTICATED_TRANSFER_PROGRAM,
        "authenticated-transfer program does not match the retained v0.2 runtime"
    );
    ensure!(
        [
            spec.lez_runtime.chain_id,
            spec.lez_runtime.channel_id,
            spec.lez_runtime.genesis_block_hash,
            spec.lez_runtime.escrow_program_id,
        ]
        .into_iter()
        .all(|value| value.as_bytes() != &[0; 32]),
        "LEZ runtime identity contains a zero value"
    );
    ensure!(
        spec.lez_runtime.chain_id == spec.lez_runtime.channel_id,
        "local v0.2 chain ID must equal the channel because upstream exposes no chain-ID RPC"
    );
    ensure!(
        spec.bridge.maker_endpoint != spec.bridge.taker_endpoint
            && spec.bridge.maker_endpoint != spec.zebra_endpoint
            && spec.bridge.taker_endpoint != spec.zebra_endpoint,
        "maker, taker, and Zebra endpoints must be distinct"
    );
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn select_mature_regtest_candidate(
    rpc: &HttpZebraRpc,
    funding_key: &PublicKey,
) -> Result<(SelectedCandidate, u32, BlockHash)> {
    let identity = ZebraChainIdentity::deterministic_regtest_nu6_2();
    let before = rpc
        .chain_info()
        .await
        .context("Zebra identity unavailable")?;
    ensure!(
        before.rpc_chain() == ZebraRpcChain::Test
            && before.consensus_branch_id() == BranchId::Nu6_2,
        "Zebra is not the deterministic Regtest NU6.2 fixture"
    );
    let genesis = rpc
        .block_hash(BlockHeight::from_u32(0))
        .await
        .context("Zebra genesis unavailable")?;
    ensure!(
        identity.network() == NetworkType::Regtest
            && identity.rpc_chain() == before.rpc_chain()
            && identity.consensus_branch_id() == before.consensus_branch_id()
            && identity.genesis_hash() == genesis,
        "Zebra genesis does not match the deterministic Regtest fixture"
    );

    let owner_script: Script = TransparentAddress::from_pubkey(funding_key).script().into();
    let tip_height = u32::from(before.tip_height());
    let mut selected = None;
    for height in 1..=tip_height {
        if tip_height.saturating_sub(height).saturating_add(1) < COINBASE_MATURITY_CONFIRMATIONS {
            break;
        }
        let block_hash = rpc
            .block_hash(BlockHeight::from_u32(height))
            .await
            .context("Zebra canonical block hash unavailable")?;
        let block = rpc
            .canonical_block(block_hash)
            .await
            .context("Zebra canonical block unavailable")?;
        ensure!(
            block.block_height() == BlockHeight::from_u32(height),
            "Zebra returned a mismatched block height"
        );
        for transaction_id in block.transaction_ids() {
            let Some(raw) = rpc
                .block_transaction(*transaction_id, block_hash)
                .await
                .context("Zebra block transaction unavailable")?
            else {
                bail!("Zebra omitted a transaction named by its canonical block");
            };
            let mut cursor = Cursor::new(raw.as_slice());
            let transaction = Transaction::read(&mut cursor, BranchId::Nu6_2)
                .context("Zebra returned a malformed NU6.2 transaction")?;
            ensure!(
                usize::try_from(cursor.position()).ok() == Some(raw.len())
                    && transaction.txid() == *transaction_id,
                "Zebra returned noncanonical transaction bytes"
            );
            let Some(bundle) = transaction.transparent_bundle() else {
                continue;
            };
            if bundle
                .vin
                .first()
                .is_none_or(|input| input.prevout() != &OutPoint::NULL)
            {
                continue;
            }
            for (index, output) in bundle.vout.iter().enumerate() {
                if output.script_pubkey() != &owner_script
                    || u64::from(output.value())
                        < ZCASH_PRINCIPAL_ZATOSHIS
                            .saturating_add(FUNDING_FEE_ZATOSHIS)
                            .saturating_add(MINIMUM_CHANGE_ZATOSHIS)
                {
                    continue;
                }
                let output_index = u32::try_from(index).context("Zcash vout index overflow")?;
                let outpoint = OutPoint::new(*transaction_id.as_ref(), output_index);
                let Some(unspent) = rpc
                    .unspent_output(&outpoint)
                    .await
                    .context("Zebra exact UTXO query failed")?
                else {
                    continue;
                };
                ensure!(
                    unspent.best_block() == before.tip_hash(),
                    "Zebra answered the UTXO query against another tip"
                );
                if unspent.confirmations() < COINBASE_MATURITY_CONFIRMATIONS {
                    continue;
                }
                ensure!(
                    unspent.output() == output,
                    "Zebra UTXO facts differ from the canonical transaction"
                );
                selected = Some(SelectedCandidate {
                    outpoint,
                    output: output.clone(),
                    rpc_transaction_id: txid_display_hex(*transaction_id),
                });
                break;
            }
            if selected.is_some() {
                break;
            }
        }
        if selected.is_some() {
            break;
        }
    }
    let after = rpc
        .chain_info()
        .await
        .context("Zebra closing identity sample unavailable")?;
    ensure!(
        before == after,
        "Zebra tip changed during candidate selection"
    );
    let candidate = selected.context("no mature deterministic Zebra funder output is available")?;
    Ok((candidate, tip_height, genesis))
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn build_agreement(
    spec: &ProvisionSpec,
    swap_id: &SwapId,
    maker_lez_account: [u8; 32],
    taker_lez_account: [u8; 32],
    authenticated_transfer: [u8; 32],
    candidate: &SelectedCandidate,
    zebra_tip_height: u32,
) -> Result<AgreementFixture> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock precedes Unix epoch")?
        .as_secs();
    let profile = ZecRefundProfile::for_id(ZecProfileId::DeterministicLocalV1);
    let zcash_anchor = zebra_tip_height
        .checked_add(1)
        .context("Zebra funding anchor overflow")?;
    let zcash_refund_height = zcash_anchor
        .checked_add(profile.zcash_refund_blocks())
        .context("Zebra refund height overflow")?;
    let lez_refund_seconds = now
        .checked_add(profile.lez_refund_delay().value())
        .context("LEZ refund time overflow")?;
    let earlier_refund_latest_ms = lez_refund_seconds
        .checked_mul(1_000)
        .context("LEZ refund millisecond overflow")?;
    let later_refund_earliest = lez_refund_seconds
        .checked_add(profile.required_margin().value())
        .context("calibrated later refund time overflow")?;
    let expires_at = now
        .checked_add(AGREEMENT_LIFETIME_SECONDS)
        .context("agreement expiry overflow")?;

    let (maker_key_byte, taker_key_byte) = match spec.direction.zcash_funder() {
        ActorRole::Maker => (FUNDED_ZCASH_KEY_BYTE, OTHER_ZCASH_KEY_BYTE),
        ActorRole::Taker => (OTHER_ZCASH_KEY_BYTE, FUNDED_ZCASH_KEY_BYTE),
    };
    let maker_bytes = Zeroizing::new([maker_key_byte; 32]);
    let taker_bytes = Zeroizing::new([taker_key_byte; 32]);
    let mut maker_secret = SecretKey::from_slice(maker_bytes.as_ref())?;
    let mut taker_secret = SecretKey::from_slice(taker_bytes.as_ref())?;
    let secp = Secp256k1::signing_only();
    let maker_public = PublicKey::from_secret_key(&secp, &maker_secret);
    let taker_public = PublicKey::from_secret_key(&secp, &taker_secret);
    let preimage = Zeroizing::new([PREIMAGE_BYTE; 32]);
    let secret_digest: [u8; 32] = Sha256::digest(&preimage[..]).into();
    let maker_hash = public_key_hash(&maker_public);
    let taker_hash = public_key_hash(&taker_public);
    let (zcash_funder_hash, zcash_claimant_hash) = match spec.direction.zcash_funder() {
        ActorRole::Maker => (maker_hash, taker_hash),
        ActorRole::Taker => (taker_hash, maker_hash),
    };
    let contract = Bip199Contract::new(
        zcash_refund_height,
        zcash_funder_hash,
        secret_digest,
        zcash_claimant_hash,
    );
    let expected_output = ExpectedBip199Output::new(
        NetworkType::Regtest,
        BranchId::Nu6_2,
        Zatoshis::from_u64(ZCASH_PRINCIPAL_ZATOSHIS)?,
        contract,
    );
    let binding = ZecSwapBinding::new(ZecProfileId::DeterministicLocalV1, expected_output)?;
    let funding_inputs = ZcashFundingInputSetV1::new(vec![ZcashFundingInputV1::new(
        *candidate.outpoint.hash(),
        candidate.outpoint.n(),
        u64::from(candidate.output.value()),
        candidate.output.script_pubkey().0.0.clone(),
    )])?;

    let escrow_program = program_id_words(spec.lez_runtime.escrow_program_id);
    let authenticated_transfer_program =
        program_id_words(Hex32::from_bytes(authenticated_transfer));
    let onchain_swap_id = derive_lez_swap_id_v1(swap_id.as_str().as_bytes());
    let metadata_account = derive_lez_metadata_account_v1(&escrow_program, &onchain_swap_id);
    let custody_account = derive_lez_native_custody_account_v1(&escrow_program, &onchain_swap_id);
    let body = ZecAgreementBodyV1::new(
        swap_id.as_str(),
        spec.direction.protocol(),
        ZecProfileRecordV1::from(ZecProfileId::DeterministicLocalV1),
        ZecParticipantsV1::new(
            ZecParticipantIdentityV1::new(maker_lez_account, maker_public.serialize()),
            ZecParticipantIdentityV1::new(taker_lez_account, taker_public.serialize()),
        ),
        secret_digest,
        ZecLezTermsV1::new(
            LezChainIdentityV1::new(
                LezEnvironmentV1::DeterministicLocalV0_2,
                *spec.lez_runtime.channel_id.as_bytes(),
                *spec.lez_runtime.genesis_block_hash.as_bytes(),
            ),
            escrow_program,
            LezAssetV1::Native {
                authenticated_transfer_program_id: authenticated_transfer_program,
            },
            LEZ_NATIVE_AMOUNT,
            metadata_account,
            custody_account,
        ),
        ZecSwapBindingRecordV1::from_binding(&binding),
        ZecTransactionPolicyV1::new(
            funding_inputs.commitment(),
            ZcashTransparentDestinationV1::p2pkh(zcash_funder_hash),
            FUNDING_FEE_ZATOSHIS,
            MINIMUM_CHANGE_ZATOSHIS,
            ZcashTransparentDestinationV1::p2pkh(zcash_claimant_hash),
            CLAIM_FEE_ZATOSHIS,
            ZcashTransparentDestinationV1::p2pkh(zcash_funder_hash),
            REFUND_FEE_ZATOSHIS,
            profile.expiry_delta_blocks(),
        ),
        ZecRefundPlanV1::new(
            now,
            zcash_anchor,
            earlier_refund_latest_ms,
            later_refund_earliest,
        ),
        NegotiationTranscriptV1::new(
            labeled_hash(spec.run_id.as_str(), b"session"),
            labeled_hash(spec.run_id.as_str(), b"offer"),
            expires_at,
        ),
    );
    let commitment = body.commitment();
    let maker_signature = sign_agreement(commitment, &maker_secret);
    let taker_signature = sign_agreement(commitment, &taker_secret);
    maker_secret.non_secure_erase();
    taker_secret.non_secure_erase();
    let record = ZecAgreementRecordV1::from_parts(
        ZEC_CONCRETE_AGREEMENT_SCHEMA_V2,
        body,
        commitment,
        maker_signature,
        taker_signature,
    );
    let agreement = ZecAgreementV1::validate_at(record, UnixSeconds::new(now))?;
    let wire = Zeroizing::new(agreement.encode_wire()?);
    let sha256 = Hex32::from_bytes(Sha256::digest(wire.as_slice()).into());
    Ok(AgreementFixture {
        wire,
        sha256,
        maker_zcash_key: maker_bytes,
        taker_zcash_key: taker_bytes,
        preimage,
        zcash_candidate: CandidateOutpoint::new(
            candidate.rpc_transaction_id,
            candidate.outpoint.n(),
        ),
    })
}

fn public_key_hash(key: &PublicKey) -> [u8; 20] {
    match TransparentAddress::from_pubkey(key) {
        TransparentAddress::PublicKeyHash(hash) => hash,
        TransparentAddress::ScriptHash(_) => unreachable!("a public key always maps to P2PKH"),
    }
}

fn sign_agreement(commitment: [u8; 32], key: &SecretKey) -> [u8; 64] {
    Secp256k1::signing_only()
        .sign_ecdsa(&Message::from_digest(commitment), key)
        .serialize_compact()
}

fn labeled_hash(run_id: &str, label: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"logos.gateway.local-poc.v1\0");
    hasher.update(run_id.as_bytes());
    hasher.update([0]);
    hasher.update(label);
    hasher.finalize().into()
}

fn program_id_words(value: Hex32) -> [u32; 8] {
    let mut words = [0_u32; 8];
    for (word, chunk) in words.iter_mut().zip(value.as_bytes().chunks_exact(4)) {
        *word = u32::from_le_bytes(chunk.try_into().expect("four-byte program word"));
    }
    words
}

fn decode_base58_32(value: &str) -> Result<[u8; 32]> {
    let decoded = bs58::decode(value).into_vec()?;
    decoded
        .try_into()
        .map_err(|_| anyhow::anyhow!("base58 value is not 32 bytes"))
}

fn txid_display_hex(transaction_id: TxId) -> Hex32 {
    let mut display = *transaction_id.as_ref();
    display.reverse();
    Hex32::from_bytes(display)
}

fn block_hash_display_hex(block_hash: BlockHash) -> Hex32 {
    let mut display = block_hash.0;
    display.reverse();
    Hex32::from_bytes(display)
}

fn random_capability() -> Result<Zeroizing<Vec<u8>>> {
    let mut random = Zeroizing::new([0_u8; 32]);
    getrandom::fill(&mut random[..])
        .map_err(|_| anyhow::anyhow!("sidecar capability randomness unavailable"))?;
    Ok(Zeroizing::new(hex::encode(&random[..]).into_bytes()))
}

fn avoid_zero_or_equal(maker: &mut [u8; 32], taker: &mut [u8; 32]) {
    if maker.iter().all(|byte| *byte == 0) {
        maker[31] = 1;
    }
    if taker.iter().all(|byte| *byte == 0) || maker == taker {
        taker[31] ^= 1;
    }
}

fn validate_new_output_root(path: &Path) -> Result<()> {
    ensure!(path.is_absolute(), "output root must be absolute");
    ensure!(
        path.components()
            .all(|component| matches!(component, Component::RootDir | Component::Normal(_))),
        "output root must be normalized"
    );
    ensure!(!path.exists(), "output root already exists");
    let parent = path.parent().context("output root has no parent")?;
    let canonical_parent = fs::canonicalize(parent).context("output parent is unavailable")?;
    ensure!(canonical_parent == parent, "output parent is not canonical");
    ensure!(
        fs::symlink_metadata(parent)
            .context("output parent is unavailable")?
            .is_dir(),
        "output parent is not a directory"
    );
    Ok(())
}

fn create_private_directory(path: &Path) -> Result<()> {
    DirBuilder::new()
        .mode(0o700)
        .create(path)
        .with_context(|| format!("failed to create private directory {}", path.display()))
}

fn write_private_new(path: &Path, bytes: &[u8]) -> Result<()> {
    ensure!(
        !bytes.is_empty(),
        "refusing to write empty private material"
    );
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("failed to create private file {}", path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("failed to write private file {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("failed to sync private file {}", path.display()))
}
