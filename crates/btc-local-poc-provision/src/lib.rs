//! Fixture-only two-stage local LEZ/Bitcoin agreement provisioner.
//!
//! Stage one creates fresh private secp256k1 material and a public planning
//! document. Stage two consumes actual local-node facts, reconstructs every
//! public value from the private files, and emits one completely validated,
//! countersigned canonical agreement. This crate never contacts a node.

#![forbid(unsafe_code)]

use std::{
    fs::{self, DirBuilder, OpenOptions},
    io::{Read as _, Write as _},
    os::unix::fs::{
        DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _,
    },
    path::{Component, Path, PathBuf},
    str::FromStr as _,
};

use anyhow::{Context as _, Result, bail, ensure};
use bitcoin::{
    Amount, BlockHash, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid, Witness,
    absolute,
    consensus::{deserialize, serialize},
    hashes::Hash as _,
    key::TweakedPublicKey,
    secp256k1::{Keypair, Message, PublicKey, Secp256k1, SecretKey, XOnlyPublicKey},
    sighash::{Prevouts, SighashCache, TapSighashType},
    taproot, transaction,
};
use lez_btc_swap_sdk::{
    AdaptorSessionContext, BTC_AGREEMENT_SCHEMA_V1, BtcAgreementBodyV1, BtcAgreementRecordV1,
    BtcAgreementV1, BtcChainPolicyV1, BtcClaimTermsV1, BtcFundingTermsV1, BtcLezTermsV1,
    BtcP2trTermsV1, BtcParticipantIdentityV1, BtcParticipantsV1, BtcRecoveryPlanV1,
    CooperativeKeyPathSpend, CsvBlockDelay, P2trSwapOutput, RefundXOnlyKey, TwoPartyAggregateKey,
};
use lez_swap_core::{Participant, SwapDirection};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

const SCHEMA_VERSION: u16 = 1;
const MAX_JSON_BYTES: usize = 64 * 1024;
const PUBLIC_SPEC_FILE: &str = "public-spec.json";
const AGREEMENT_FILE: &str = "agreement.borsh";
const SUMMARY_FILE: &str = "agreement-summary.json";
const FUNDING_TRANSACTION_FILE: &str = "funding-transaction.hex";
const FUNDING_SUMMARY_FILE: &str = "funding-transaction-summary.json";
const PRIVATE_DIRECTORY: &str = "private";
const MAKER_SIGNING_FILE: &str = "maker-signing.key";
const TAKER_SIGNING_FILE: &str = "taker-signing.key";
const MAKER_REFUND_FILE: &str = "maker-refund.key";
const TAKER_REFUND_FILE: &str = "taker-refund.key";
const MAKER_CLAIM_FILE: &str = "maker-claim-destination.key";
const TAKER_CLAIM_FILE: &str = "taker-claim-destination.key";
const ADAPTOR_FILE: &str = "adaptor-scalar.key";

/// Secret-free result emitted by stage one.
#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Stage1Summary {
    schema_version: u16,
    public_spec_file: PathBuf,
    public_spec_sha256: String,
    aggregate_internal_key: String,
    lez_authority_helper: LezAuthorityHelper,
    private_material_disclosed: bool,
}

impl Stage1Summary {
    /// Path to the strict, secret-free public planning document.
    #[must_use]
    pub fn public_spec_file(&self) -> &Path {
        &self.public_spec_file
    }

    /// SHA-256 of the exact public planning document bytes.
    #[must_use]
    pub fn public_spec_sha256(&self) -> &str {
        &self.public_spec_sha256
    }
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct LezAuthorityHelper {
    manifest_path: &'static str,
    package: &'static str,
    example: &'static str,
    argument: String,
    result_schema: &'static str,
    result_version: u8,
}

/// Secret-free result emitted by stage two and persisted beside the agreement.
#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Stage2Summary {
    schema_version: u16,
    direction: Direction,
    agreement_file: PathBuf,
    summary_file: PathBuf,
    agreement_sha256: String,
    agreement_commitment: String,
    bitcoin_funding_transaction_id: String,
    bitcoin_funding_output_index: u32,
    bitcoin_funding_transaction_file: PathBuf,
    bitcoin_funding_transaction_sha256: String,
    bitcoin_funding_authorization: AuthorizationStatus,
    bitcoin_node_state: NodeStateStatus,
    planned_bitcoin_funding_anchor_height: u32,
    bitcoin_contract_script_pubkey: String,
    bitcoin_claim_unsigned_transaction: String,
    bitcoin_claim_bip341_sighash: String,
    lez_channel_id: String,
    lez_aggregate_authority_account: String,
    private_material_disclosed: bool,
    agreement_revalidated: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum AuthorizationStatus {
    Verified,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum NodeStateStatus {
    NotAsserted,
}

/// Secret-free evidence for one exact offline-signed funding transaction.
#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FundingPreparationSummary {
    schema_version: u16,
    direction: Direction,
    signed_transaction_file: PathBuf,
    summary_file: PathBuf,
    signed_transaction_sha256: String,
    transaction_id: String,
    witness_transaction_id: String,
    input_transaction_id: String,
    input_output_index: u32,
    input_value_sat: u64,
    input_script_pubkey: String,
    contract_output_index: u32,
    contract_value_sat: u64,
    contract_script_pubkey: String,
    contract_merkle_root: String,
    change_output_index: u32,
    change_value_sat: u64,
    fee_sat: u64,
    bip341_sighash: String,
    private_material_disclosed: bool,
    node_state_asserted: bool,
}

impl FundingPreparationSummary {
    /// Owner-only file containing the exact signed transaction as lowercase hex.
    #[must_use]
    pub fn signed_transaction_file(&self) -> &Path {
        &self.signed_transaction_file
    }
}

impl Stage2Summary {
    /// Canonical Borsh agreement path.
    #[must_use]
    pub fn agreement_file(&self) -> &Path {
        &self.agreement_file
    }

    /// Stable direction spelling used by both input and output JSON.
    #[must_use]
    pub const fn direction(&self) -> &'static str {
        self.direction.as_str()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanningSpec {
    schema_version: u16,
    maker_lez_owner_account: String,
    taker_lez_owner_account: String,
    refund_csv_blocks: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct PublicSpec {
    schema_version: u16,
    maker: PublicParticipant,
    taker: PublicParticipant,
    adaptor_point: String,
    aggregate_internal_key: String,
    lez_aggregate_x_only_public_key: String,
    refund_csv_blocks: u32,
    contracts: DirectionContracts,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct PublicParticipant {
    lez_owner_account: String,
    musig2_public_key: String,
    bitcoin_refund_x_only_public_key: String,
    bitcoin_claim_destination_script_pubkey: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct DirectionContracts {
    taker_sells_foreign: PublicContract,
    taker_sells_lez: PublicContract,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct PublicContract {
    bitcoin_funder: ParticipantName,
    refund_x_only_public_key: String,
    script_pubkey: String,
    refund_script: String,
    refund_control_block: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ParticipantName {
    Maker,
    Taker,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Direction {
    TakerSellsForeign,
    TakerSellsLez,
}

impl Direction {
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

    const fn bitcoin_funder(self) -> Participant {
        match self {
            Self::TakerSellsForeign => Participant::Taker,
            Self::TakerSellsLez => Participant::Maker,
        }
    }

    const fn lez_depositor(self) -> Participant {
        match self {
            Self::TakerSellsForeign => Participant::Maker,
            Self::TakerSellsLez => Participant::Taker,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FinalizeSpec {
    schema_version: u16,
    stage1_public_sha256: String,
    swap_id: String,
    direction: Direction,
    bitcoin: BitcoinFacts,
    lez_runtime: LezRuntime,
    lez_terms: LezTerms,
    recovery: RecoveryPolicy,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FundingPreparationSpec {
    schema_version: u16,
    stage1_public_sha256: String,
    direction: Direction,
    service_input: ServiceInput,
    contract_value_sat: u64,
    fee_sat: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ServiceInput {
    transaction_id: String,
    output_index: u32,
    value_sat: u64,
    script_pubkey: String,
    signing_secret_key_file: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BitcoinFacts {
    genesis_block_hash: String,
    required_confirmations: u32,
    funding_signed_transaction: String,
    funding_signed_transaction_sha256: String,
    funding_input_value_sat: u64,
    funding_input_script_pubkey: String,
    funding_transaction_id: String,
    funding_output_index: u32,
    funding_value_sat: u64,
    claim_value_sat: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LezRuntime {
    compatibility: RuntimeCompatibility,
    chain_id: String,
    channel_id: String,
    genesis_block_hash: String,
    escrow_program_id: String,
    authenticated_transfer_program_id: String,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RuntimeCompatibility {
    LeeV0_2_0,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LezTerms {
    aggregate_authority_mapping: AuthorityMapping,
    metadata_account: String,
    custody_account: String,
    depositor_account: String,
    claimant_account: String,
    amount: u128,
    refund_at_ms: u64,
    prepared_claim_message_hash: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorityMapping {
    schema: String,
    version: u8,
    x_only_public_key: String,
    account_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryPolicy {
    refund_csv_blocks: u32,
    planned_bitcoin_funding_anchor_height: u32,
    bitcoin_refund_height: u32,
    maker_second_lock_cutoff_unix_seconds: u64,
    earlier_refund_latest_unix_seconds: u64,
    later_refund_earliest_unix_seconds: u64,
    required_margin_seconds: u64,
}

struct Secrets {
    maker_signing: Zeroizing<[u8; 32]>,
    taker_signing: Zeroizing<[u8; 32]>,
    maker_refund: Zeroizing<[u8; 32]>,
    taker_refund: Zeroizing<[u8; 32]>,
    maker_claim: Zeroizing<[u8; 32]>,
    taker_claim: Zeroizing<[u8; 32]>,
    adaptor: Zeroizing<[u8; 32]>,
}

/// Creates fresh private material and the public planning document.
///
/// # Errors
///
/// Rejects malformed planning JSON, invalid public accounts or CSV policy,
/// unsafe/non-new output roots, unavailable randomness, or any failed write.
pub fn generate_stage1(planning_file: &Path, output_root: &Path) -> Result<Stage1Summary> {
    validate_new_output_root(output_root)?;
    let planning: PlanningSpec = read_strict_json(planning_file)?;
    ensure!(
        planning.schema_version == SCHEMA_VERSION,
        "unsupported planning schema"
    );
    let maker_account = parse_hex32(&planning.maker_lez_owner_account, "maker LEZ account")?;
    let taker_account = parse_hex32(&planning.taker_lez_owner_account, "taker LEZ account")?;
    ensure!(
        maker_account != taker_account,
        "participant LEZ accounts must be distinct"
    );
    let _ = CsvBlockDelay::new(planning.refund_csv_blocks).context("invalid refund CSV policy")?;

    let secrets = Secrets::fresh()?;
    let public = public_spec(
        &secrets,
        maker_account,
        taker_account,
        planning.refund_csv_blocks,
    )?;
    let mut public_bytes = serde_json::to_vec_pretty(&public).context("encode public spec")?;
    public_bytes.push(b'\n');
    let public_sha256 = sha256_hex(&public_bytes);

    create_private_directory(output_root)?;
    let private_root = output_root.join(PRIVATE_DIRECTORY);
    create_private_directory(&private_root)?;
    write_private_new(
        &private_root.join(MAKER_SIGNING_FILE),
        secrets.maker_signing.as_ref(),
    )?;
    write_private_new(
        &private_root.join(TAKER_SIGNING_FILE),
        secrets.taker_signing.as_ref(),
    )?;
    write_private_new(
        &private_root.join(MAKER_REFUND_FILE),
        secrets.maker_refund.as_ref(),
    )?;
    write_private_new(
        &private_root.join(TAKER_REFUND_FILE),
        secrets.taker_refund.as_ref(),
    )?;
    write_private_new(
        &private_root.join(MAKER_CLAIM_FILE),
        secrets.maker_claim.as_ref(),
    )?;
    write_private_new(
        &private_root.join(TAKER_CLAIM_FILE),
        secrets.taker_claim.as_ref(),
    )?;
    write_private_new(&private_root.join(ADAPTOR_FILE), secrets.adaptor.as_ref())?;
    let public_spec_file = output_root.join(PUBLIC_SPEC_FILE);
    write_private_new(&public_spec_file, &public_bytes)?;

    Ok(Stage1Summary {
        schema_version: SCHEMA_VERSION,
        public_spec_file,
        public_spec_sha256: public_sha256,
        aggregate_internal_key: public.aggregate_internal_key.clone(),
        lez_authority_helper: LezAuthorityHelper {
            manifest_path: "compat/lez-v0_2-sidecar/Cargo.toml",
            package: "lez-v0-2-sidecar",
            example: "lez-v02-account-id",
            argument: public.lez_aggregate_x_only_public_key,
            result_schema: "lez-v0.2-nssa-account-id",
            result_version: 1,
        },
        private_material_disclosed: false,
    })
}

/// Creates and signs the exact Bitcoin funding transaction without broadcasting it.
///
/// The caller supplies one `rawtr` service-output candidate obtained from local
/// Core. Its owner-only key file is used only for one BIP-341 key-path signature.
/// The resulting exact transaction and secret-free summary are persisted
/// create-new. This function proves construction and authorization, but neither
/// contacts Core nor claims that the input is unspent, policy-accepted,
/// broadcast, mined, or final.
///
/// # Errors
///
/// Rejects malformed/cross-wired stage-one material, an unsafe or mismatched
/// service key, invalid input/value/fee facts, or existing output evidence.
#[allow(clippy::too_many_lines)]
pub fn prepare_funding(spec_file: &Path, output_root: &Path) -> Result<FundingPreparationSummary> {
    validate_existing_output_root(output_root)?;
    let spec: FundingPreparationSpec = read_strict_json(spec_file)?;
    ensure!(
        spec.schema_version == SCHEMA_VERSION,
        "unsupported funding-preparation schema"
    );

    let public_path = output_root.join(PUBLIC_SPEC_FILE);
    let public_bytes = read_stable_file(&public_path, MAX_JSON_BYTES, true)?;
    ensure!(
        canonical_hex(&spec.stage1_public_sha256, 32, "stage-one public SHA-256")?
            == sha256_hex(&public_bytes),
        "stage-one public document hash mismatch"
    );
    let public: PublicSpec =
        serde_json::from_slice(&public_bytes).context("invalid public spec")?;
    ensure!(
        public.schema_version == SCHEMA_VERSION,
        "unsupported public schema"
    );
    let secrets = Secrets::load(&output_root.join(PRIVATE_DIRECTORY))?;
    let reconstructed = public_spec(
        &secrets,
        parse_hex32(&public.maker.lez_owner_account, "maker LEZ account")?,
        parse_hex32(&public.taker.lez_owner_account, "taker LEZ account")?,
        public.refund_csv_blocks,
    )?;
    ensure!(
        public == reconstructed,
        "stage-one public and private material mismatch"
    );

    let contract_public = match spec.direction {
        Direction::TakerSellsForeign => &public.contracts.taker_sells_foreign,
        Direction::TakerSellsLez => &public.contracts.taker_sells_lez,
    };
    let contract = contract_from_public(&public, spec.direction)?;
    let contract_script = ScriptBuf::from_bytes(contract.script_pubkey_bytes().to_vec());
    ensure!(
        hex::encode(contract.script_pubkey_bytes()) == contract_public.script_pubkey,
        "reconstructed P2TR contract mismatch"
    );

    ensure_normalized_absolute(&spec.service_input.signing_secret_key_file)?;
    let signing_bytes = read_secret(&spec.service_input.signing_secret_key_file)?;
    let signing_secret = secret_key(&signing_bytes, "service input signing key")?;
    let secp = Secp256k1::new();
    let signing_keypair = Keypair::from_secret_key(&secp, &signing_secret);
    let input_script = ScriptBuf::from_bytes(parse_hex(
        &spec.service_input.script_pubkey,
        34,
        "service input scriptPubKey",
    )?);
    let expected_input_script = ScriptBuf::new_p2tr_tweaked(
        TweakedPublicKey::dangerous_assume_tweaked(signing_keypair.x_only_public_key().0),
    );
    ensure!(
        input_script == expected_input_script,
        "service input scriptPubKey does not belong to the signing key"
    );
    let input_txid = Txid::from_str(&spec.service_input.transaction_id)
        .context("invalid service input transaction ID")?;
    ensure!(
        input_txid.to_byte_array() != [0; 32],
        "zero service input transaction ID"
    );
    ensure!(
        spec.service_input.value_sat <= Amount::MAX_MONEY.to_sat()
            && spec.contract_value_sat > 0
            && spec.fee_sat > 0,
        "invalid Bitcoin funding values"
    );
    let change_value_sat = spec
        .service_input
        .value_sat
        .checked_sub(spec.contract_value_sat)
        .and_then(|remaining| remaining.checked_sub(spec.fee_sat))
        .context("service input cannot cover the contract output and fee")?;
    ensure!(
        change_value_sat > 0,
        "offline funding requires a nonzero rawtr change output"
    );

    let mut transaction = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint {
                txid: input_txid,
                vout: spec.service_input.output_index,
            },
            script_sig: ScriptBuf::new(),
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            witness: Witness::new(),
        }],
        output: vec![
            TxOut {
                value: Amount::from_sat(spec.contract_value_sat),
                script_pubkey: contract_script.clone(),
            },
            TxOut {
                value: Amount::from_sat(change_value_sat),
                script_pubkey: input_script.clone(),
            },
        ],
    };
    let prevouts = [TxOut {
        value: Amount::from_sat(spec.service_input.value_sat),
        script_pubkey: input_script.clone(),
    }];
    let sighash = SighashCache::new(&transaction)
        .taproot_key_spend_signature_hash(0, &Prevouts::All(&prevouts), TapSighashType::Default)
        .context("compute service input BIP-341 sighash")?;
    let signature = secp.sign_schnorr_no_aux_rand(
        &Message::from_digest(sighash.to_byte_array()),
        &signing_keypair,
    );
    let taproot_signature = taproot::Signature::from_slice(&signature.serialize())
        .context("encode service input signature")?;
    transaction.input[0].witness = Witness::p2tr_key_spend(&taproot_signature);
    Secp256k1::verification_only()
        .verify_schnorr(
            &taproot_signature.signature,
            &Message::from_digest(sighash.to_byte_array()),
            &signing_keypair.x_only_public_key().0,
        )
        .context("verify service input signature")?;

    let raw = serialize(&transaction);
    let raw_hex = hex::encode(&raw);
    let transaction_id = transaction.compute_txid();
    let _ = validate_signed_funding_transaction(
        &raw_hex,
        transaction_id,
        0,
        spec.contract_value_sat,
        &contract_script,
        spec.service_input.value_sat,
        &input_script,
    )?;

    let transaction_file = output_root.join(FUNDING_TRANSACTION_FILE);
    let summary_file = output_root.join(FUNDING_SUMMARY_FILE);
    ensure_new_file_path(&transaction_file)?;
    ensure_new_file_path(&summary_file)?;
    let summary = FundingPreparationSummary {
        schema_version: SCHEMA_VERSION,
        direction: spec.direction,
        signed_transaction_file: transaction_file.clone(),
        summary_file: summary_file.clone(),
        signed_transaction_sha256: sha256_hex(&raw),
        transaction_id: transaction_id.to_string(),
        witness_transaction_id: transaction.compute_wtxid().to_string(),
        input_transaction_id: input_txid.to_string(),
        input_output_index: spec.service_input.output_index,
        input_value_sat: spec.service_input.value_sat,
        input_script_pubkey: hex::encode(input_script.as_bytes()),
        contract_output_index: 0,
        contract_value_sat: spec.contract_value_sat,
        contract_script_pubkey: hex::encode(contract.script_pubkey_bytes()),
        contract_merkle_root: hex::encode(contract.merkle_root_bytes()),
        change_output_index: 1,
        change_value_sat,
        fee_sat: spec.fee_sat,
        bip341_sighash: hex::encode(sighash.to_byte_array()),
        private_material_disclosed: false,
        node_state_asserted: false,
    };
    let mut transaction_file_bytes = raw_hex.into_bytes();
    transaction_file_bytes.push(b'\n');
    let mut summary_bytes =
        serde_json::to_vec_pretty(&summary).context("encode funding summary")?;
    summary_bytes.push(b'\n');
    write_private_new(&transaction_file, &transaction_file_bytes)?;
    write_private_new(&summary_file, &summary_bytes)?;
    Ok(summary)
}

/// Constructs, countersigns, validates, and writes the canonical agreement.
///
/// # Errors
///
/// Rejects unsafe stage-one material, malformed or cross-wired facts, a Core
/// output that does not use the planned P2TR script, invalid recovery terms,
/// an unofficial/mismatched LEZ authority mapping, or existing outputs.
#[allow(clippy::too_many_lines)]
pub fn finalize_stage2(spec_file: &Path, output_root: &Path) -> Result<Stage2Summary> {
    validate_existing_output_root(output_root)?;
    let spec: FinalizeSpec = read_strict_json(spec_file)?;
    ensure!(
        spec.schema_version == SCHEMA_VERSION,
        "unsupported finalize schema"
    );
    let public_path = output_root.join(PUBLIC_SPEC_FILE);
    let public_bytes = read_stable_file(&public_path, MAX_JSON_BYTES, true)?;
    ensure!(
        canonical_hex(&spec.stage1_public_sha256, 32, "stage-one public SHA-256")?
            == sha256_hex(&public_bytes),
        "stage-one public document hash mismatch"
    );
    let public: PublicSpec =
        serde_json::from_slice(&public_bytes).context("invalid public spec")?;
    ensure!(
        public.schema_version == SCHEMA_VERSION,
        "unsupported public schema"
    );

    let secrets = Secrets::load(&output_root.join(PRIVATE_DIRECTORY))?;
    let maker_account = parse_hex32(&public.maker.lez_owner_account, "maker LEZ account")?;
    let taker_account = parse_hex32(&public.taker.lez_owner_account, "taker LEZ account")?;
    let reconstructed = public_spec(
        &secrets,
        maker_account,
        taker_account,
        public.refund_csv_blocks,
    )?;
    ensure!(
        public == reconstructed,
        "stage-one public and private material mismatch"
    );

    let direction = spec.direction;
    let protocol_direction = direction.protocol();
    let contract_public = match direction {
        Direction::TakerSellsForeign => &public.contracts.taker_sells_foreign,
        Direction::TakerSellsLez => &public.contracts.taker_sells_lez,
    };
    ensure!(
        spec.recovery.refund_csv_blocks == public.refund_csv_blocks,
        "recovery CSV differs from stage-one planning"
    );
    let contract = contract_from_public(&public, direction)?;
    let contract_script = ScriptBuf::from_bytes(contract.script_pubkey_bytes().to_vec());
    ensure!(
        hex::encode(contract.script_pubkey_bytes()) == contract_public.script_pubkey,
        "reconstructed P2TR contract mismatch"
    );

    let funding_txid = Txid::from_str(&spec.bitcoin.funding_transaction_id)
        .context("invalid prepared funding transaction ID")?;
    let funding_input_script = ScriptBuf::from_bytes(parse_hex(
        &spec.bitcoin.funding_input_script_pubkey,
        34,
        "prepared funding input scriptPubKey",
    )?);
    let funding_transaction = validate_signed_funding_transaction(
        &spec.bitcoin.funding_signed_transaction,
        funding_txid,
        spec.bitcoin.funding_output_index,
        spec.bitcoin.funding_value_sat,
        &contract_script,
        spec.bitcoin.funding_input_value_sat,
        &funding_input_script,
    )?;
    let funding_raw = serialize(&funding_transaction);
    ensure!(
        canonical_hex(
            &spec.bitcoin.funding_signed_transaction_sha256,
            32,
            "prepared funding transaction SHA-256"
        )? == sha256_hex(&funding_raw),
        "prepared funding transaction hash mismatch"
    );
    let funding_transaction_file = output_root.join(FUNDING_TRANSACTION_FILE);
    let mut funding_file_bytes = spec.bitcoin.funding_signed_transaction.as_bytes().to_vec();
    funding_file_bytes.push(b'\n');
    persist_or_match_private(&funding_transaction_file, &funding_file_bytes)?;

    let maker_identity = participant_identity(&public.maker)?;
    let taker_identity = participant_identity(&public.taker)?;
    let participants = BtcParticipantsV1::new(maker_identity, taker_identity);
    let funding = BtcFundingTermsV1::new(
        funding_txid.to_byte_array(),
        spec.bitcoin.funding_output_index,
        spec.bitcoin.funding_value_sat,
    );
    let bitcoin_claimant = direction.bitcoin_funder().other();
    let destination = participants
        .for_participant(bitcoin_claimant)
        .claim_destination_script_pubkey()
        .to_vec();
    let spend = CooperativeKeyPathSpend::new(
        &contract,
        OutPoint {
            txid: funding_txid,
            vout: spec.bitcoin.funding_output_index,
        },
        Amount::from_sat(spec.bitcoin.funding_value_sat),
        vec![TxOut {
            value: Amount::from_sat(spec.bitcoin.claim_value_sat),
            script_pubkey: ScriptBuf::from_bytes(destination),
        }],
    )
    .context("invalid cooperative claim transaction")?;
    let claim = BtcClaimTermsV1::from_spend(&spend).context("invalid claim terms")?;

    ensure!(
        matches!(
            spec.lez_runtime.compatibility,
            RuntimeCompatibility::LeeV0_2_0
        ),
        "unsupported LEZ runtime"
    );
    let chain_id = parse_hex32(&spec.lez_runtime.chain_id, "LEZ chain ID")?;
    let channel_id = parse_hex32(&spec.lez_runtime.channel_id, "LEZ channel ID")?;
    ensure!(
        chain_id == channel_id,
        "local LEZ v0.2 chain and channel IDs differ"
    );
    let lez_genesis = parse_hex32(&spec.lez_runtime.genesis_block_hash, "LEZ genesis")?;
    let escrow_program = parse_hex32(&spec.lez_runtime.escrow_program_id, "LEZ escrow program")?;
    let transfer_program = parse_hex32(
        &spec.lez_runtime.authenticated_transfer_program_id,
        "LEZ authenticated-transfer program",
    )?;
    ensure!(
        escrow_program != transfer_program,
        "LEZ programs must be distinct"
    );
    ensure!(
        spec.lez_terms.aggregate_authority_mapping.schema == "lez-v0.2-nssa-account-id"
            && spec.lez_terms.aggregate_authority_mapping.version == 1,
        "unsupported official LEZ authority mapping"
    );
    ensure!(
        canonical_hex(
            &spec.lez_terms.aggregate_authority_mapping.x_only_public_key,
            32,
            "LEZ authority x-only key"
        )? == public.lez_aggregate_x_only_public_key,
        "LEZ authority key differs from the participant aggregate"
    );
    let authority_account = parse_hex32(
        &spec.lez_terms.aggregate_authority_mapping.account_id,
        "LEZ aggregate authority account",
    )?;
    let metadata = parse_hex32(&spec.lez_terms.metadata_account, "LEZ metadata account")?;
    let custody = parse_hex32(&spec.lez_terms.custody_account, "LEZ custody account")?;
    let depositor = parse_hex32(&spec.lez_terms.depositor_account, "LEZ depositor account")?;
    let claimant = parse_hex32(&spec.lez_terms.claimant_account, "LEZ claimant account")?;
    let expected_depositor = *participants
        .for_participant(direction.lez_depositor())
        .lez_owner_account();
    let expected_claimant = *participants
        .for_participant(direction.lez_depositor().other())
        .lez_owner_account();
    ensure!(
        depositor == expected_depositor && claimant == expected_claimant,
        "LEZ accounts do not match the direction-derived roles"
    );
    let message_hash = parse_hex32(
        &spec.lez_terms.prepared_claim_message_hash,
        "prepared LEZ claim message hash",
    )?;
    let lez = BtcLezTermsV1::new(
        channel_id,
        lez_genesis,
        escrow_program,
        transfer_program,
        authority_account,
        metadata,
        custody,
        depositor,
        claimant,
        spec.lez_terms.amount,
        spec.lez_terms.refund_at_ms,
        message_hash,
    );

    ensure!(
        spec.recovery.bitcoin_refund_height
            == spec
                .recovery
                .planned_bitcoin_funding_anchor_height
                .checked_add(spec.recovery.refund_csv_blocks)
                .context("Bitcoin refund height overflow")?,
        "Bitcoin refund height does not equal the planned funding anchor plus CSV"
    );
    let recovery = BtcRecoveryPlanV1::new(
        spec.recovery.planned_bitcoin_funding_anchor_height,
        spec.recovery.bitcoin_refund_height,
        spec.recovery.maker_second_lock_cutoff_unix_seconds,
        spec.recovery.earlier_refund_latest_unix_seconds,
        spec.recovery.later_refund_earliest_unix_seconds,
        spec.recovery.required_margin_seconds,
    );
    let genesis = BlockHash::from_str(&spec.bitcoin.genesis_block_hash)
        .context("invalid Core genesis block hash")?;
    ensure!(
        genesis.to_byte_array() != [0; 32],
        "zero Core genesis block hash"
    );
    let body = BtcAgreementBodyV1::new(
        parse_hex32(&spec.swap_id, "swap ID")?,
        protocol_direction,
        BtcChainPolicyV1::new(genesis.to_byte_array(), spec.bitcoin.required_confirmations),
        participants,
        parse_hex33(&public.adaptor_point, "adaptor point")?,
        lez,
        BtcP2trTermsV1::from_contract(&contract),
        funding,
        claim,
        recovery,
    );
    let commitment = body.commitment();
    let maker_secret = secret_key(&secrets.maker_signing, "maker signing key")?;
    let taker_secret = secret_key(&secrets.taker_signing, "taker signing key")?;
    let record = BtcAgreementRecordV1::from_parts(
        BTC_AGREEMENT_SCHEMA_V1,
        body,
        commitment,
        sign_commitment(&maker_secret, commitment),
        sign_commitment(&taker_secret, commitment),
    );
    let expected_policy =
        BtcChainPolicyV1::new(genesis.to_byte_array(), spec.bitcoin.required_confirmations);
    let agreement = BtcAgreementV1::validate_for_bitcoin_policy(record, &expected_policy)
        .context("constructed agreement did not validate")?;
    ensure!(
        agreement.direction() == protocol_direction
            && agreement.bitcoin_funder() == direction.bitcoin_funder()
            && agreement.lez_depositor() == direction.lez_depositor(),
        "validated agreement role projection mismatch"
    );
    let wire = agreement.encode_wire().context("encode agreement")?;
    let replay = BtcAgreementV1::from_wire_for_bitcoin_policy(&wire, &expected_policy)
        .context("encoded agreement did not revalidate")?;
    ensure!(
        replay.encode_wire()? == wire,
        "agreement canonical replay mismatch"
    );

    let agreement_file = output_root.join(AGREEMENT_FILE);
    let summary_file = output_root.join(SUMMARY_FILE);
    ensure_new_file_path(&agreement_file)?;
    ensure_new_file_path(&summary_file)?;
    let summary = Stage2Summary {
        schema_version: SCHEMA_VERSION,
        direction,
        agreement_file: agreement_file.clone(),
        summary_file: summary_file.clone(),
        agreement_sha256: sha256_hex(&wire),
        agreement_commitment: hex::encode(replay.agreement_commitment()),
        bitcoin_funding_transaction_id: funding_txid.to_string(),
        bitcoin_funding_output_index: spec.bitcoin.funding_output_index,
        bitcoin_funding_transaction_file: funding_transaction_file,
        bitcoin_funding_transaction_sha256: sha256_hex(&funding_raw),
        bitcoin_funding_authorization: AuthorizationStatus::Verified,
        bitcoin_node_state: NodeStateStatus::NotAsserted,
        planned_bitcoin_funding_anchor_height: spec.recovery.planned_bitcoin_funding_anchor_height,
        bitcoin_contract_script_pubkey: hex::encode(replay.p2tr_contract().script_pubkey_bytes()),
        bitcoin_claim_unsigned_transaction: hex::encode(
            replay.cooperative_claim().unsigned_transaction_bytes(),
        ),
        bitcoin_claim_bip341_sighash: hex::encode(replay.cooperative_claim().sighash_bytes()),
        lez_channel_id: hex::encode(replay.lez_terms().channel_id()),
        lez_aggregate_authority_account: hex::encode(
            replay.lez_terms().aggregate_authority_account(),
        ),
        private_material_disclosed: false,
        agreement_revalidated: true,
    };
    let mut summary_bytes =
        serde_json::to_vec_pretty(&summary).context("encode agreement summary")?;
    summary_bytes.push(b'\n');
    write_private_new(&agreement_file, &wire)?;
    write_private_new(&summary_file, &summary_bytes)?;
    Ok(summary)
}

impl Secrets {
    fn fresh() -> Result<Self> {
        let mut generated: Vec<Zeroizing<[u8; 32]>> = Vec::with_capacity(7);
        while generated.len() < 7 {
            let candidate = random_secret()?;
            if generated.iter().all(|existing| **existing != *candidate) {
                generated.push(candidate);
            }
        }
        let mut take = generated.into_iter();
        Ok(Self {
            maker_signing: take.next().expect("seven generated secrets"),
            taker_signing: take.next().expect("seven generated secrets"),
            maker_refund: take.next().expect("seven generated secrets"),
            taker_refund: take.next().expect("seven generated secrets"),
            maker_claim: take.next().expect("seven generated secrets"),
            taker_claim: take.next().expect("seven generated secrets"),
            adaptor: take.next().expect("seven generated secrets"),
        })
    }

    fn load(private_root: &Path) -> Result<Self> {
        validate_private_directory(private_root)?;
        Ok(Self {
            maker_signing: read_secret(&private_root.join(MAKER_SIGNING_FILE))?,
            taker_signing: read_secret(&private_root.join(TAKER_SIGNING_FILE))?,
            maker_refund: read_secret(&private_root.join(MAKER_REFUND_FILE))?,
            taker_refund: read_secret(&private_root.join(TAKER_REFUND_FILE))?,
            maker_claim: read_secret(&private_root.join(MAKER_CLAIM_FILE))?,
            taker_claim: read_secret(&private_root.join(TAKER_CLAIM_FILE))?,
            adaptor: read_secret(&private_root.join(ADAPTOR_FILE))?,
        })
    }
}

fn public_spec(
    secrets: &Secrets,
    maker_lez_owner_account: [u8; 32],
    taker_lez_owner_account: [u8; 32],
    refund_csv_blocks: u32,
) -> Result<PublicSpec> {
    let maker = public_participant(
        maker_lez_owner_account,
        &secrets.maker_signing,
        &secrets.maker_refund,
        &secrets.maker_claim,
    )?;
    let taker = public_participant(
        taker_lez_owner_account,
        &secrets.taker_signing,
        &secrets.taker_refund,
        &secrets.taker_claim,
    )?;
    let adaptor_secret = secret_key(&secrets.adaptor, "adaptor scalar")?;
    let adaptor_point =
        PublicKey::from_secret_key(&Secp256k1::signing_only(), &adaptor_secret).serialize();
    let aggregate = AdaptorSessionContext::untweaked(
        [
            parse_hex33(&maker.musig2_public_key, "maker signing public key")?,
            parse_hex33(&taker.musig2_public_key, "taker signing public key")?,
        ],
        [1; 32],
        adaptor_point,
        [2; 32],
    )
    .context("derive aggregate key")?
    .output_key();
    let delay = CsvBlockDelay::new(refund_csv_blocks).context("invalid refund CSV")?;
    let foreign_contract = contract_public(
        ParticipantName::Taker,
        aggregate,
        parse_hex32(&taker.bitcoin_refund_x_only_public_key, "taker refund key")?,
        delay,
    )?;
    let lez_contract = contract_public(
        ParticipantName::Maker,
        aggregate,
        parse_hex32(&maker.bitcoin_refund_x_only_public_key, "maker refund key")?,
        delay,
    )?;
    Ok(PublicSpec {
        schema_version: SCHEMA_VERSION,
        maker,
        taker,
        adaptor_point: hex::encode(adaptor_point),
        aggregate_internal_key: hex::encode(aggregate),
        lez_aggregate_x_only_public_key: hex::encode(aggregate),
        refund_csv_blocks,
        contracts: DirectionContracts {
            taker_sells_foreign: foreign_contract,
            taker_sells_lez: lez_contract,
        },
    })
}

fn public_participant(
    lez_owner_account: [u8; 32],
    signing: &[u8; 32],
    refund: &[u8; 32],
    claim: &[u8; 32],
) -> Result<PublicParticipant> {
    let secp = Secp256k1::signing_only();
    let signing = secret_key(signing, "participant signing key")?;
    let refund = secret_key(refund, "participant refund key")?;
    let claim = secret_key(claim, "participant claim key")?;
    let refund_x_only = Keypair::from_secret_key(&secp, &refund)
        .x_only_public_key()
        .0;
    let claim_x_only = Keypair::from_secret_key(&secp, &claim)
        .x_only_public_key()
        .0;
    let claim_script = ScriptBuf::new_p2tr(&Secp256k1::verification_only(), claim_x_only, None);
    Ok(PublicParticipant {
        lez_owner_account: hex::encode(lez_owner_account),
        musig2_public_key: hex::encode(PublicKey::from_secret_key(&secp, &signing).serialize()),
        bitcoin_refund_x_only_public_key: hex::encode(refund_x_only.serialize()),
        bitcoin_claim_destination_script_pubkey: hex::encode(claim_script.as_bytes()),
    })
}

fn contract_public(
    funder: ParticipantName,
    aggregate: [u8; 32],
    refund: [u8; 32],
    delay: CsvBlockDelay,
) -> Result<PublicContract> {
    let contract = P2trSwapOutput::new(
        TwoPartyAggregateKey::from_bytes(aggregate).context("invalid aggregate key")?,
        RefundXOnlyKey::from_bytes(refund).context("invalid refund key")?,
        delay,
    )
    .context("build planned P2TR contract")?;
    Ok(PublicContract {
        bitcoin_funder: funder,
        refund_x_only_public_key: hex::encode(refund),
        script_pubkey: hex::encode(contract.script_pubkey_bytes()),
        refund_script: hex::encode(contract.refund_script_bytes()),
        refund_control_block: hex::encode(contract.refund_control_block_bytes()),
    })
}

fn contract_from_public(public: &PublicSpec, direction: Direction) -> Result<P2trSwapOutput> {
    let refund_key = match direction.bitcoin_funder() {
        Participant::Maker => parse_hex32(
            &public.maker.bitcoin_refund_x_only_public_key,
            "maker refund key",
        )?,
        Participant::Taker => parse_hex32(
            &public.taker.bitcoin_refund_x_only_public_key,
            "taker refund key",
        )?,
    };
    P2trSwapOutput::new(
        TwoPartyAggregateKey::from_bytes(parse_hex32(
            &public.aggregate_internal_key,
            "aggregate internal key",
        )?)
        .context("invalid aggregate key")?,
        RefundXOnlyKey::from_bytes(refund_key).context("invalid refund key")?,
        CsvBlockDelay::new(public.refund_csv_blocks).context("invalid refund CSV")?,
    )
    .context("invalid P2TR contract")
}

fn participant_identity(public: &PublicParticipant) -> Result<BtcParticipantIdentityV1> {
    Ok(BtcParticipantIdentityV1::new(
        parse_hex32(&public.lez_owner_account, "participant LEZ account")?,
        parse_hex33(&public.musig2_public_key, "participant signing public key")?,
        parse_hex32(
            &public.bitcoin_refund_x_only_public_key,
            "participant refund public key",
        )?,
        parse_hex(
            &public.bitcoin_claim_destination_script_pubkey,
            34,
            "claim destination",
        )?,
    ))
}

#[allow(clippy::too_many_arguments)]
fn validate_signed_funding_transaction(
    raw_hex: &str,
    expected_txid: Txid,
    contract_output_index: u32,
    contract_value_sat: u64,
    contract_script: &ScriptBuf,
    input_value_sat: u64,
    input_script: &ScriptBuf,
) -> Result<Transaction> {
    ensure!(
        !raw_hex.is_empty()
            && raw_hex.len().is_multiple_of(2)
            && raw_hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "prepared funding transaction must be canonical lowercase hex"
    );
    let raw = hex::decode(raw_hex).context("decode prepared funding transaction")?;
    let transaction: Transaction =
        deserialize(&raw).context("decode prepared Bitcoin funding transaction")?;
    ensure!(
        serialize(&transaction) == raw,
        "prepared funding transaction is not canonically encoded"
    );
    ensure!(
        transaction.version == transaction::Version::TWO
            && transaction.lock_time == absolute::LockTime::ZERO,
        "prepared funding transaction has unexpected version or locktime"
    );
    let [input] = transaction.input.as_slice() else {
        bail!("prepared funding transaction must have exactly one service input")
    };
    ensure!(
        !input.previous_output.is_null()
            && input.script_sig.is_empty()
            && input.sequence == Sequence::ENABLE_RBF_NO_LOCKTIME,
        "prepared funding transaction has invalid rawtr input semantics"
    );
    let witness_items = input.witness.iter().collect::<Vec<_>>();
    let [signature_bytes] = witness_items.as_slice() else {
        bail!("prepared funding input must have exactly one key-path witness item")
    };
    ensure!(
        signature_bytes.len() == 64,
        "prepared funding input must use BIP-341 SIGHASH_DEFAULT"
    );
    let signature = taproot::Signature::from_slice(signature_bytes)
        .context("invalid prepared funding Taproot signature")?;
    ensure!(
        input_value_sat > 0 && input_value_sat <= Amount::MAX_MONEY.to_sat(),
        "prepared funding input value is invalid"
    );
    let input_script_bytes = input_script.as_bytes();
    ensure!(
        input_script_bytes.len() == 34
            && input_script_bytes[0] == 0x51
            && input_script_bytes[1] == 0x20,
        "prepared funding input is not a canonical rawtr scriptPubKey"
    );
    let input_key = XOnlyPublicKey::from_slice(&input_script_bytes[2..])
        .context("invalid prepared funding rawtr key")?;
    let prevouts = [TxOut {
        value: Amount::from_sat(input_value_sat),
        script_pubkey: input_script.clone(),
    }];
    let sighash = SighashCache::new(&transaction)
        .taproot_key_spend_signature_hash(0, &Prevouts::All(&prevouts), TapSighashType::Default)
        .context("compute prepared funding BIP-341 sighash")?;
    Secp256k1::verification_only()
        .verify_schnorr(
            &signature.signature,
            &Message::from_digest(sighash.to_byte_array()),
            &input_key,
        )
        .context("prepared funding signature does not authorize the exact transaction")?;

    ensure!(
        expected_txid.to_byte_array() != [0; 32] && transaction.compute_txid() == expected_txid,
        "prepared funding transaction ID mismatch"
    );
    let output_index = usize::try_from(contract_output_index)
        .context("prepared funding output index is not representable")?;
    let output = transaction
        .output
        .get(output_index)
        .context("prepared funding output index is absent")?;
    ensure!(
        contract_value_sat > 0
            && contract_value_sat <= Amount::MAX_MONEY.to_sat()
            && output.value.to_sat() == contract_value_sat,
        "prepared funding output value mismatch"
    );
    ensure!(
        &output.script_pubkey == contract_script,
        "prepared funding output does not use the planned P2TR script"
    );
    let total_output = transaction.output.iter().try_fold(0_u64, |sum, candidate| {
        sum.checked_add(candidate.value.to_sat())
    });
    ensure!(
        total_output.is_some_and(|value| value < input_value_sat),
        "prepared funding outputs do not leave a positive bounded fee"
    );
    Ok(transaction)
}

fn persist_or_match_private(path: &Path, bytes: &[u8]) -> Result<()> {
    if path.exists() {
        ensure!(
            read_stable_file(path, MAX_JSON_BYTES, true)? == bytes,
            "persisted funding transaction differs from the validated exact bytes"
        );
        Ok(())
    } else {
        write_private_new(path, bytes)
    }
}

fn random_secret() -> Result<Zeroizing<[u8; 32]>> {
    for _ in 0..128 {
        let mut bytes = Zeroizing::new([0_u8; 32]);
        getrandom::fill(bytes.as_mut())
            .map_err(|_| anyhow::anyhow!("OS randomness unavailable"))?;
        if SecretKey::from_slice(bytes.as_ref()).is_ok() {
            return Ok(bytes);
        }
    }
    bail!("OS randomness did not produce a valid secret")
}

fn secret_key(bytes: &[u8; 32], name: &str) -> Result<SecretKey> {
    SecretKey::from_slice(bytes).with_context(|| format!("invalid {name}"))
}

fn sign_commitment(secret: &SecretKey, commitment: [u8; 32]) -> [u8; 64] {
    let secp = Secp256k1::signing_only();
    secp.sign_schnorr_no_aux_rand(
        &Message::from_digest(commitment),
        &Keypair::from_secret_key(&secp, secret),
    )
    .serialize()
}

fn parse_hex32(value: &str, name: &str) -> Result<[u8; 32]> {
    parse_hex(value, 32, name)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid {name}"))
}

fn parse_hex33(value: &str, name: &str) -> Result<[u8; 33]> {
    parse_hex(value, 33, name)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid {name}"))
}

fn parse_hex(value: &str, bytes: usize, name: &str) -> Result<Vec<u8>> {
    let canonical = canonical_hex(value, bytes, name)?;
    let decoded = hex::decode(canonical).with_context(|| format!("invalid {name}"))?;
    ensure!(decoded.iter().any(|byte| *byte != 0), "zero {name}");
    Ok(decoded)
}

fn canonical_hex<'a>(value: &'a str, bytes: usize, name: &str) -> Result<&'a str> {
    ensure!(
        value.len() == bytes.saturating_mul(2)
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "{name} must be canonical lowercase hex"
    );
    Ok(value)
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn read_secret(path: &Path) -> Result<Zeroizing<[u8; 32]>> {
    let bytes = Zeroizing::new(read_stable_file(path, 32, true)?);
    let array: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| anyhow::anyhow!("private key file has invalid length"))?;
    let secret = Zeroizing::new(array);
    let _ = secret_key(&secret, "private key file")?;
    Ok(secret)
}

fn read_strict_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let bytes = read_stable_file(path, MAX_JSON_BYTES, true)?;
    serde_json::from_slice(&bytes).context("invalid strict JSON input")
}

fn read_stable_file(path: &Path, maximum: usize, owner_private: bool) -> Result<Vec<u8>> {
    let before = fs::symlink_metadata(path).context("input file unavailable")?;
    validate_file_metadata(&before, maximum, owner_private)?;
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .context("input file unavailable or unsafe")?;
    let opened = file.metadata().context("input metadata unavailable")?;
    validate_file_metadata(&opened, maximum, owner_private)?;
    ensure!(same_file(&before, &opened), "input file identity changed");
    let mut bytes = Vec::with_capacity(maximum.min(4_096));
    std::io::Read::by_ref(&mut file)
        .take(u64::try_from(maximum)?.saturating_add(1))
        .read_to_end(&mut bytes)
        .context("input read failed")?;
    let after = fs::symlink_metadata(path).context("input file disappeared")?;
    validate_file_metadata(&after, maximum, owner_private)?;
    ensure!(
        same_file(&opened, &after),
        "input file changed while reading"
    );
    ensure!(
        !bytes.is_empty() && bytes.len() <= maximum,
        "input size is invalid"
    );
    Ok(bytes)
}

fn validate_file_metadata(
    metadata: &fs::Metadata,
    maximum: usize,
    owner_private: bool,
) -> Result<()> {
    ensure!(
        metadata.file_type().is_file()
            && metadata.len() > 0
            && metadata.len() <= u64::try_from(maximum)?
            && metadata.nlink() == 1,
        "input file type, size, or link count is unsafe"
    );
    if owner_private {
        ensure!(
            metadata.permissions().mode() & 0o7777 == 0o600,
            "input file must be owner-only mode 0600"
        );
    }
    Ok(())
}

fn same_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.len() == right.len()
        && left.mode() == right.mode()
        && left.nlink() == right.nlink()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.ctime() == right.ctime()
        && left.ctime_nsec() == right.ctime_nsec()
}

fn validate_new_output_root(path: &Path) -> Result<()> {
    ensure_normalized_absolute(path)?;
    ensure!(!path.exists(), "output root already exists");
    let parent = path.parent().context("output root has no parent")?;
    ensure!(
        fs::canonicalize(parent).context("output parent unavailable")? == parent,
        "output parent is not canonical"
    );
    ensure!(
        fs::symlink_metadata(parent)?.is_dir(),
        "output parent is not a directory"
    );
    Ok(())
}

fn validate_existing_output_root(path: &Path) -> Result<()> {
    ensure_normalized_absolute(path)?;
    let metadata = fs::symlink_metadata(path).context("output root unavailable")?;
    ensure!(
        metadata.file_type().is_dir(),
        "output root is not a directory"
    );
    ensure!(
        metadata.permissions().mode() & 0o7777 == 0o700,
        "output root must be mode 0700"
    );
    ensure!(
        fs::canonicalize(path)? == path,
        "output root is not canonical"
    );
    Ok(())
}

fn validate_private_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).context("private directory unavailable")?;
    ensure!(
        metadata.file_type().is_dir(),
        "private path is not a directory"
    );
    ensure!(
        metadata.permissions().mode() & 0o7777 == 0o700,
        "private directory must be mode 0700"
    );
    ensure!(
        fs::canonicalize(path)? == path,
        "private directory is not canonical"
    );
    Ok(())
}

fn ensure_normalized_absolute(path: &Path) -> Result<()> {
    ensure!(path.is_absolute(), "output root must be absolute");
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => normalized.push(Path::new("/")),
            Component::Normal(value) => normalized.push(value),
            Component::CurDir | Component::ParentDir | Component::Prefix(_) => {
                bail!("output root must be normalized")
            }
        }
    }
    ensure!(normalized == path, "output root must be normalized");
    Ok(())
}

fn create_private_directory(path: &Path) -> Result<()> {
    DirBuilder::new()
        .mode(0o700)
        .create(path)
        .with_context(|| format!("create private directory {}", path.display()))?;
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_dir() && metadata.permissions().mode() & 0o7777 == 0o700,
        "created directory permissions are unsafe"
    );
    Ok(())
}

fn ensure_new_file_path(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => bail!("output file already exists"),
        Err(error) => Err(error).context("output path unavailable"),
    }
}

fn write_private_new(path: &Path, bytes: &[u8]) -> Result<()> {
    ensure!(!bytes.is_empty(), "refusing to write an empty file");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .with_context(|| format!("create private file {}", path.display()))?;
    file.write_all(bytes).context("write private file")?;
    file.sync_all().context("sync private file")?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file()
            && metadata.permissions().mode() & 0o7777 == 0o600
            && metadata.nlink() == 1,
        "created file permissions are unsafe"
    );
    Ok(())
}
