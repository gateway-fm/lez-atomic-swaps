//! Role-local authority bootstrap for production-shaped LEZ/Bitcoin setup.
//!
//! One invocation creates exactly one role's private material and one signed,
//! secret-free contribution. It does not accept a peer key path, derive peer
//! authority, compose an agreement, contact either chain, or authorize funding.

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
    Amount, OutPoint, ScriptBuf, TxOut, Txid,
    hashes::Hash as _,
    secp256k1::{Keypair, Message, PublicKey, Secp256k1, SecretKey},
};
use lez_btc_swap_sdk::{
    BTC_ROLE_CONTRIBUTION_SCHEMA_V1, BtcAgreementBodyV1, BtcAgreementDraftV1, BtcAgreementV1,
    BtcChainPolicyV1, BtcClaimTermsV1, BtcFundingTermsV1, BtcLezChainIdentityV1, BtcLezTermsV1,
    BtcP2trTermsV1, BtcParticipantIdentityV1, BtcRecoveryPlanV1, BtcRoleContributionBodyV1,
    BtcRoleContributionPairV1, BtcRoleContributionRecordV1, BtcRoleContributionV1,
    CooperativeKeyPathSpend, CsvBlockDelay, MAX_BTC_AGREEMENT_RECORD_BYTES,
    MAX_BTC_ROLE_CONTRIBUTION_RECORD_BYTES, P2trSwapOutput, RefundXOnlyKey, TwoPartyAggregateKey,
    derive_btc_pre_session_id_v1,
};
use lez_swap_core::{Participant, SwapDirection};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

const SPEC_SCHEMA_VERSION: u16 = 1;
const MAX_JSON_BYTES: usize = 64 * 1024;
const PRIVATE_DIRECTORY: &str = "private";
const AGREEMENT_KEY_FILE: &str = "agreement.key";
const REFUND_KEY_FILE: &str = "bitcoin-refund.key";
const CLAIM_KEY_FILE: &str = "bitcoin-claim-destination.key";
const FUNDING_KEY_FILE: &str = "bitcoin-funding.key";
const ADAPTOR_FILE: &str = "adaptor-scalar.key";
const CONTRIBUTION_FILE: &str = "contribution.borsh";
const SUMMARY_FILE: &str = "contribution-summary.json";
const AGREEMENT_BINDING_FILE: &str = "agreement-binding.json";
const ACCEPTED_AGREEMENT_FILE: &str = "agreement.borsh";
const PEER_CONTRIBUTION_FILE: &str = "peer-contribution.borsh";
const UNSIGNED_DRAFT_FILE: &str = "unsigned-draft.borsh";
const DRAFT_SUMMARY_FILE: &str = "unsigned-draft-summary.json";

/// Secret-free output of one role-local bootstrap.
#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RoleBootstrapSummary {
    schema_version: u16,
    role: RoleName,
    direction: DirectionName,
    pre_session_id: String,
    contribution_file: PathBuf,
    summary_file: PathBuf,
    contribution_sha256: String,
    contribution_commitment: String,
    agreement_public_key: String,
    bitcoin_refund_x_only_public_key: String,
    bitcoin_claim_destination_script_pubkey: String,
    bitcoin_funding_x_only_public_key: String,
    adaptor_point: Option<String>,
    expires_at_unix_seconds: u64,
    private_material_disclosed: bool,
    peer_private_material_created: bool,
}

impl RoleBootstrapSummary {
    /// Canonical signed public-contribution path.
    #[must_use]
    pub fn contribution_file(&self) -> &Path {
        &self.contribution_file
    }

    /// Owner-private root containing only this role's secret counterparts.
    #[must_use]
    pub fn summary_file(&self) -> &Path {
        &self.summary_file
    }
}

/// Secret-free proof that one countersigned agreement exactly retains both
/// signed contributions and this root's private counterparts.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)]
pub struct AgreementBindingSummary {
    schema_version: u16,
    role: RoleName,
    swap_id: String,
    agreement_file: PathBuf,
    agreement_sha256: String,
    agreement_commitment: String,
    local_contribution_commitment: String,
    peer_contribution_commitment: String,
    accepted_at_unix_seconds: u64,
    contribution_expires_at_unix_seconds: u64,
    roles_and_chain_identities_revalidated: bool,
    local_private_authority_revalidated: bool,
    ready_for_public_effects: bool,
    binding_file: PathBuf,
    private_material_disclosed: bool,
    was_replay: bool,
}

impl AgreementBindingSummary {
    /// Durable no-clobber receipt path inside the role root.
    #[must_use]
    pub fn binding_file(&self) -> &Path {
        &self.binding_file
    }

    /// Whether an existing byte-identical role binding was reused.
    #[must_use]
    pub const fn was_replay(&self) -> bool {
        self.was_replay
    }

    /// The role is intentionally not authorized for public effects yet.
    #[must_use]
    pub const fn ready_for_public_effects(&self) -> bool {
        self.ready_for_public_effects
    }

    /// Original durable acceptance time, retained unchanged across replay.
    #[must_use]
    pub const fn accepted_at_unix_seconds(&self) -> u64 {
        self.accepted_at_unix_seconds
    }
}

/// Secret-free result of composing an unsigned agreement solely from both
/// signed public contributions and explicit observed chain facts.
#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgreementDraftSummary {
    schema_version: u16,
    direction: DirectionName,
    swap_id: String,
    draft_file: PathBuf,
    draft_sha256: String,
    agreement_commitment: String,
    bitcoin_funding_transaction_id: String,
    bitcoin_contract_script_pubkey: String,
    bitcoin_claim_bip341_sighash: String,
    lez_channel_id: String,
    private_material_disclosed: bool,
    role_contributions_revalidated: bool,
}

impl AgreementDraftSummary {
    /// Canonical unsigned agreement draft consumed by Chat v2.
    #[must_use]
    pub fn draft_file(&self) -> &Path {
        &self.draft_file
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum RoleName {
    Maker,
    Taker,
}

impl RoleName {
    const fn protocol(self) -> Participant {
        match self {
            Self::Maker => Participant::Maker,
            Self::Taker => Participant::Taker,
        }
    }

    const fn from_protocol(role: Participant) -> Self {
        match role {
            Participant::Maker => Self::Maker,
            Participant::Taker => Self::Taker,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum DirectionName {
    TakerSellsForeign,
    TakerSellsLez,
}

impl DirectionName {
    const fn protocol(self) -> SwapDirection {
        match self {
            Self::TakerSellsForeign => SwapDirection::TakerSellsForeign,
            Self::TakerSellsLez => SwapDirection::TakerSellsLez,
        }
    }

    const fn from_protocol(direction: SwapDirection) -> Self {
        match direction {
            SwapDirection::TakerSellsForeign => Self::TakerSellsForeign,
            SwapDirection::TakerSellsLez => Self::TakerSellsLez,
        }
    }

    const fn bitcoin_funder(self) -> Participant {
        match self {
            Self::TakerSellsForeign => Participant::Taker,
            Self::TakerSellsLez => Participant::Maker,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RoleBootstrapSpec {
    schema_version: u16,
    role: RoleName,
    direction: DirectionName,
    offer_commitment: String,
    reservation_binding: String,
    bitcoin: BitcoinIdentity,
    lez: LezIdentity,
    lez_owner_account: String,
    expires_at_unix_seconds: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BitcoinIdentity {
    genesis_block_hash: String,
    required_confirmations: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LezIdentity {
    genesis_block_hash: String,
    channel_id: String,
    escrow_program_id: String,
    authenticated_transfer_program_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgreementDraftSpec {
    schema_version: u16,
    bitcoin: DraftBitcoinFacts,
    lez: DraftLezFacts,
    recovery: DraftRecoveryPolicy,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DraftBitcoinFacts {
    funding_transaction_id: String,
    funding_output_index: u32,
    funding_value_sat: u64,
    claim_value_sat: u64,
    refund_csv_blocks: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DraftLezFacts {
    aggregate_authority_account: String,
    metadata_account: String,
    custody_account: String,
    amount: u128,
    refund_at_ms: u64,
    prepared_claim_message_hash: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DraftRecoveryPolicy {
    planned_bitcoin_funding_anchor_height: u32,
    bitcoin_refund_height: u32,
    maker_second_lock_cutoff_unix_seconds: u64,
    earlier_refund_latest_unix_seconds: u64,
    later_refund_earliest_unix_seconds: u64,
    required_margin_seconds: u64,
}

struct RoleSecrets {
    agreement: Zeroizing<[u8; 32]>,
    refund: Zeroizing<[u8; 32]>,
    claim: Zeroizing<[u8; 32]>,
    funding: Zeroizing<[u8; 32]>,
    adaptor: Option<Zeroizing<[u8; 32]>>,
}

impl RoleSecrets {
    fn fresh(role: Participant, agreement: Option<Zeroizing<[u8; 32]>>) -> Result<Self> {
        Ok(Self {
            agreement: match agreement {
                Some(existing) => existing,
                None => random_secret()?,
            },
            refund: random_secret()?,
            claim: random_secret()?,
            funding: random_secret()?,
            adaptor: (role == Participant::Taker)
                .then(random_secret)
                .transpose()?,
        })
    }
}

/// Creates fresh authority for one immutable role and publishes its signed
/// public contribution without constructing peer authority.
///
/// # Errors
///
/// Rejects malformed or unsafe input, invalid chain/session identities,
/// unavailable randomness, non-new output paths, or unsafe filesystem state.
#[allow(clippy::too_many_lines)]
pub fn bootstrap_role(spec_file: &Path, output_root: &Path) -> Result<RoleBootstrapSummary> {
    validate_new_output_root(output_root)?;
    let spec: RoleBootstrapSpec = read_strict_json(spec_file)?;
    ensure!(
        spec.schema_version == SPEC_SCHEMA_VERSION,
        "unsupported role bootstrap schema"
    );
    let input = RoleBootstrapInput {
        role: spec.role.protocol(),
        direction: spec.direction.protocol(),
        offer_commitment: parse_hex32(&spec.offer_commitment, "offer commitment")?,
        reservation_binding: parse_hex_variable(
            &spec.reservation_binding,
            "reservation binding",
            lez_btc_swap_sdk::MAX_BTC_PRE_SESSION_RESERVATION_BYTES,
        )?,
        bitcoin: BtcChainPolicyV1::new(
            parse_hex32(
                &spec.bitcoin.genesis_block_hash,
                "Bitcoin genesis block hash",
            )?,
            spec.bitcoin.required_confirmations,
        ),
        lez: BtcLezChainIdentityV1::new(
            parse_hex32(&spec.lez.genesis_block_hash, "LEZ genesis block hash")?,
            parse_hex32(&spec.lez.channel_id, "LEZ channel ID")?,
            parse_hex32(&spec.lez.escrow_program_id, "LEZ escrow program ID")?,
            parse_hex32(
                &spec.lez.authenticated_transfer_program_id,
                "LEZ authenticated-transfer program ID",
            )?,
        ),
        lez_owner_account: parse_hex32(&spec.lez_owner_account, "LEZ owner account")?,
        expires_at_unix_seconds: spec.expires_at_unix_seconds,
    };
    Ok(bootstrap_role_in_process(&input, None, output_root)?.summary)
}

/// The public facts one role commits to when it bootstraps for a reservation.
#[derive(Clone, Debug)]
pub struct RoleBootstrapInput {
    pub role: Participant,
    pub direction: SwapDirection,
    pub offer_commitment: [u8; 32],
    /// The reservation bytes the pre-session id derives from (≤ 256 bytes).
    pub reservation_binding: Vec<u8>,
    pub bitcoin: BtcChainPolicyV1,
    pub lez: BtcLezChainIdentityV1,
    pub lez_owner_account: [u8; 32],
    pub expires_at_unix_seconds: u64,
}

/// A bootstrapped role root plus the signed contribution it published.
#[derive(Debug)]
pub struct BootstrappedRole {
    pub summary: RoleBootstrapSummary,
    pub contribution_wire: Vec<u8>,
}

/// Bootstraps one role root in-process.
///
/// `agreement_key` reuses an existing `MuSig2` agreement key (a Maker's
/// offer-bound signer); `None` mints a fresh one. Refund, claim-destination
/// and funding keys — and the Taker's adaptor scalar — are always fresh.
///
/// # Errors
///
/// Fails when the output root exists, the identity fields are invalid, or
/// any private file cannot be created owner-private.
#[allow(clippy::too_many_lines)]
pub fn bootstrap_role_in_process(
    input: &RoleBootstrapInput,
    agreement_key: Option<Zeroizing<[u8; 32]>>,
    output_root: &Path,
) -> Result<BootstrappedRole> {
    validate_new_output_root(output_root)?;
    let offer_commitment = input.offer_commitment;
    let reservation_binding = input.reservation_binding.clone();
    ensure!(
        !reservation_binding.is_empty()
            && reservation_binding.len() <= lez_btc_swap_sdk::MAX_BTC_PRE_SESSION_RESERVATION_BYTES,
        "reservation binding is empty or oversized"
    );
    let direction = input.direction;
    let role = input.role;
    let pre_session_id =
        derive_btc_pre_session_id_v1(&offer_commitment, &reservation_binding, direction)?;
    let bitcoin_chain_policy = input.bitcoin;
    let lez_chain_identity = input.lez;
    let lez_owner_account = input.lez_owner_account;
    ensure!(
        input.expires_at_unix_seconds != 0,
        "contribution expiry must be nonzero"
    );
    let spec = RoleSummaryNames {
        role: RoleName::from_protocol(role),
        direction: DirectionName::from_protocol(direction),
        expires_at_unix_seconds: input.expires_at_unix_seconds,
    };
    let secrets = RoleSecrets::fresh(role, agreement_key)?;
    let secp = Secp256k1::new();
    let agreement_public_key =
        compressed_public_key(&secp, &secrets.agreement, "agreement key")?.serialize();
    let refund_public_key = x_only_public_key(&secp, &secrets.refund, "refund key")?;
    let claim_public_key = x_only_public_key(&secp, &secrets.claim, "claim key")?;
    let claim_destination =
        ScriptBuf::new_p2tr(&Secp256k1::verification_only(), claim_public_key, None).into_bytes();
    let funding_public_key = x_only_public_key(&secp, &secrets.funding, "funding key")?;
    let adaptor_point = secrets
        .adaptor
        .as_ref()
        .map(|bytes| {
            compressed_public_key(&secp, bytes, "adaptor scalar").map(|public| public.serialize())
        })
        .transpose()?;
    let mut role_entropy = Zeroizing::new([0_u8; 32]);
    getrandom::fill(role_entropy.as_mut())
        .map_err(|_| anyhow::anyhow!("OS randomness unavailable"))?;
    ensure!(
        role_entropy.iter().any(|byte| *byte != 0),
        "zero role entropy"
    );
    let identity = BtcParticipantIdentityV1::new(
        lez_owner_account,
        agreement_public_key,
        refund_public_key.serialize(),
        claim_destination.clone(),
    );
    let body = BtcRoleContributionBodyV1::new(
        pre_session_id,
        role,
        direction,
        bitcoin_chain_policy,
        lez_chain_identity,
        identity,
        funding_public_key.serialize(),
        adaptor_point,
        *role_entropy,
        spec.expires_at_unix_seconds,
    )?;
    let commitment = body.commitment();
    let mut signing_aux = Zeroizing::new([0_u8; 32]);
    getrandom::fill(signing_aux.as_mut())
        .map_err(|_| anyhow::anyhow!("OS randomness unavailable"))?;
    let mut agreement_keypair = keypair(&secp, &secrets.agreement, "agreement key")?;
    let signature = secp
        .sign_schnorr_with_aux_rand(
            &Message::from_digest(commitment),
            &agreement_keypair,
            &signing_aux,
        )
        .serialize();
    agreement_keypair.non_secure_erase();
    let contribution = BtcRoleContributionV1::validate(BtcRoleContributionRecordV1::from_parts(
        BTC_ROLE_CONTRIBUTION_SCHEMA_V1,
        body,
        commitment,
        signature,
    ))?;
    let contribution_wire = contribution.encode_wire()?;

    create_private_directory(output_root)?;
    let private_root = output_root.join(PRIVATE_DIRECTORY);
    create_private_directory(&private_root)?;
    write_private_new(
        &private_root.join(AGREEMENT_KEY_FILE),
        secrets.agreement.as_ref(),
    )?;
    write_private_new(&private_root.join(REFUND_KEY_FILE), secrets.refund.as_ref())?;
    write_private_new(&private_root.join(CLAIM_KEY_FILE), secrets.claim.as_ref())?;
    write_private_new(
        &private_root.join(FUNDING_KEY_FILE),
        secrets.funding.as_ref(),
    )?;
    if let Some(adaptor) = &secrets.adaptor {
        write_private_new(&private_root.join(ADAPTOR_FILE), adaptor.as_ref())?;
    }
    let contribution_file = output_root.join(CONTRIBUTION_FILE);
    let summary_file = output_root.join(SUMMARY_FILE);
    write_private_new(&contribution_file, &contribution_wire)?;
    let summary = RoleBootstrapSummary {
        schema_version: SPEC_SCHEMA_VERSION,
        role: spec.role,
        direction: spec.direction,
        pre_session_id: hex::encode(pre_session_id),
        contribution_file,
        summary_file: summary_file.clone(),
        contribution_sha256: hex::encode(Sha256::digest(&contribution_wire)),
        contribution_commitment: hex::encode(commitment),
        agreement_public_key: hex::encode(agreement_public_key),
        bitcoin_refund_x_only_public_key: hex::encode(refund_public_key.serialize()),
        bitcoin_claim_destination_script_pubkey: hex::encode(claim_destination),
        bitcoin_funding_x_only_public_key: hex::encode(funding_public_key.serialize()),
        adaptor_point: adaptor_point.map(hex::encode),
        expires_at_unix_seconds: spec.expires_at_unix_seconds,
        private_material_disclosed: false,
        peer_private_material_created: false,
    };
    let mut summary_bytes = serde_json::to_vec_pretty(&summary).context("encode role summary")?;
    summary_bytes.push(b'\n');
    write_private_new(&summary_file, &summary_bytes)?;
    fs::File::open(&private_root)?.sync_all()?;
    fs::File::open(output_root)?.sync_all()?;
    Ok(BootstrappedRole {
        summary,
        contribution_wire,
    })
}

/// The role-name view of the summary, kept next to the typed input.
struct RoleSummaryNames {
    role: RoleName,
    direction: DirectionName,
    expires_at_unix_seconds: u64,
}

/// Composes the canonical unsigned Chat-v2 agreement from independently
/// signed role contributions and explicit chain facts, without access to any
/// role's private keys.
///
/// # Errors
///
/// Rejects malformed/cross-wired contributions, unsafe or inconsistent chain
/// facts, an invalid funding/claim/refund construction, or an existing output.
#[allow(clippy::too_many_lines)]
pub fn compose_agreement_draft(
    spec_file: &Path,
    maker_contribution_file: &Path,
    taker_contribution_file: &Path,
    output_root: &Path,
) -> Result<AgreementDraftSummary> {
    validate_new_output_root(output_root)?;
    let spec: AgreementDraftSpec = read_strict_json(spec_file)?;
    ensure!(
        spec.schema_version == SPEC_SCHEMA_VERSION,
        "unsupported agreement-draft schema"
    );
    let maker_wire = read_stable_private_file(
        maker_contribution_file,
        MAX_BTC_ROLE_CONTRIBUTION_RECORD_BYTES,
    )?;
    let taker_wire = read_stable_private_file(
        taker_contribution_file,
        MAX_BTC_ROLE_CONTRIBUTION_RECORD_BYTES,
    )?;
    let funding_txid = Txid::from_str(&spec.bitcoin.funding_transaction_id)
        .context("invalid Bitcoin funding transaction ID")?;
    let facts = AgreementDraftFacts {
        funding_transaction_id: funding_txid.to_byte_array(),
        funding_output_index: spec.bitcoin.funding_output_index,
        funding_value_sat: spec.bitcoin.funding_value_sat,
        claim_value_sat: spec.bitcoin.claim_value_sat,
        refund_csv_blocks: spec.bitcoin.refund_csv_blocks,
        lez_aggregate_authority_account: parse_hex32(
            &spec.lez.aggregate_authority_account,
            "LEZ aggregate authority account",
        )?,
        lez_metadata_account: parse_hex32(&spec.lez.metadata_account, "LEZ metadata account")?,
        lez_custody_account: parse_hex32(&spec.lez.custody_account, "LEZ custody account")?,
        lez_amount: spec.lez.amount,
        lez_refund_at_ms: spec.lez.refund_at_ms,
        lez_prepared_claim_message_hash: parse_hex32(
            &spec.lez.prepared_claim_message_hash,
            "LEZ prepared claim message hash",
        )?,
        planned_bitcoin_funding_anchor_height: spec.recovery.planned_bitcoin_funding_anchor_height,
        bitcoin_refund_height: spec.recovery.bitcoin_refund_height,
        maker_second_lock_cutoff_unix_seconds: spec.recovery.maker_second_lock_cutoff_unix_seconds,
        earlier_refund_latest_unix_seconds: spec.recovery.earlier_refund_latest_unix_seconds,
        later_refund_earliest_unix_seconds: spec.recovery.later_refund_earliest_unix_seconds,
        required_margin_seconds: spec.recovery.required_margin_seconds,
    };
    let composed = compose_agreement_draft_wire(&facts, &maker_wire, &taker_wire)?;
    let direction = DirectionName::from_protocol(composed.direction);
    let draft_wire = composed.wire;

    create_private_directory(output_root)?;
    let draft_file = output_root.join(UNSIGNED_DRAFT_FILE);
    let summary_file = output_root.join(DRAFT_SUMMARY_FILE);
    write_private_new(&draft_file, &draft_wire)?;
    let summary = AgreementDraftSummary {
        schema_version: SPEC_SCHEMA_VERSION,
        direction,
        swap_id: hex::encode(composed.swap_id),
        draft_file,
        draft_sha256: hex::encode(Sha256::digest(&draft_wire)),
        agreement_commitment: hex::encode(composed.agreement_commitment),
        bitcoin_funding_transaction_id: funding_txid.to_string(),
        bitcoin_contract_script_pubkey: hex::encode(&composed.bitcoin_contract_script_pubkey),
        bitcoin_claim_bip341_sighash: hex::encode(composed.bitcoin_claim_bip341_sighash),
        lez_channel_id: hex::encode(composed.lez_channel_id),
        private_material_disclosed: false,
        role_contributions_revalidated: true,
    };
    let mut summary_bytes = serde_json::to_vec_pretty(&summary).context("encode draft summary")?;
    summary_bytes.push(b'\n');
    write_private_new(&summary_file, &summary_bytes)?;
    fs::File::open(output_root)?.sync_all()?;
    Ok(summary)
}

/// The chain facts a draft binds on top of the two signed contributions.
#[derive(Clone, Debug)]
pub struct AgreementDraftFacts {
    pub funding_transaction_id: [u8; 32],
    pub funding_output_index: u32,
    pub funding_value_sat: u64,
    pub claim_value_sat: u64,
    pub refund_csv_blocks: u32,
    pub lez_aggregate_authority_account: [u8; 32],
    pub lez_metadata_account: [u8; 32],
    pub lez_custody_account: [u8; 32],
    pub lez_amount: u128,
    pub lez_refund_at_ms: u64,
    pub lez_prepared_claim_message_hash: [u8; 32],
    pub planned_bitcoin_funding_anchor_height: u32,
    pub bitcoin_refund_height: u32,
    pub maker_second_lock_cutoff_unix_seconds: u64,
    pub earlier_refund_latest_unix_seconds: u64,
    pub later_refund_earliest_unix_seconds: u64,
    pub required_margin_seconds: u64,
}

/// A canonical unsigned draft and the public facts derived while composing it.
#[derive(Clone, Debug)]
pub struct ComposedAgreementDraft {
    pub wire: Vec<u8>,
    pub direction: SwapDirection,
    pub swap_id: [u8; 32],
    pub agreement_commitment: [u8; 32],
    pub bitcoin_contract_script_pubkey: Vec<u8>,
    pub bitcoin_claim_bip341_sighash: [u8; 32],
    pub lez_channel_id: [u8; 32],
}

/// Composes the unsigned agreement draft in-process from both signed
/// contributions and the chain facts, revalidating the contributions.
///
/// # Errors
///
/// Fails when a contribution is invalid, the contract or claim cannot be
/// constructed, the recovery heights are inconsistent, or the draft is not
/// canonical.
#[allow(clippy::too_many_lines)]
pub fn compose_agreement_draft_wire(
    facts: &AgreementDraftFacts,
    maker_wire: &[u8],
    taker_wire: &[u8],
) -> Result<ComposedAgreementDraft> {
    let pair = BtcRoleContributionPairV1::new(
        BtcRoleContributionV1::from_wire(maker_wire)?,
        BtcRoleContributionV1::from_wire(taker_wire)?,
    )?;
    let direction = DirectionName::from_protocol(pair.maker().body().direction());
    let participants = pair.participants();
    let aggregate = participants
        .aggregate_internal_key()
        .context("derive participant aggregate key")?;
    let bitcoin_funder = direction.bitcoin_funder();
    let refund_key = *participants
        .for_participant(bitcoin_funder)
        .bitcoin_refund_key();
    let refund_delay =
        CsvBlockDelay::new(facts.refund_csv_blocks).context("invalid Bitcoin refund CSV")?;
    let contract = P2trSwapOutput::new(
        TwoPartyAggregateKey::from_bytes(aggregate).context("invalid aggregate key")?,
        RefundXOnlyKey::from_bytes(refund_key).context("invalid refund key")?,
        refund_delay,
    )
    .context("construct contribution-bound P2TR contract")?;
    let funding_txid = Txid::from_byte_array(facts.funding_transaction_id);
    let funding = BtcFundingTermsV1::new(
        funding_txid.to_byte_array(),
        facts.funding_output_index,
        facts.funding_value_sat,
    );
    let bitcoin_claimant = bitcoin_funder.other();
    let claim = CooperativeKeyPathSpend::new(
        &contract,
        OutPoint {
            txid: funding_txid,
            vout: facts.funding_output_index,
        },
        Amount::from_sat(facts.funding_value_sat),
        vec![TxOut {
            value: Amount::from_sat(facts.claim_value_sat),
            script_pubkey: ScriptBuf::from_bytes(
                participants
                    .for_participant(bitcoin_claimant)
                    .claim_destination_script_pubkey()
                    .to_vec(),
            ),
        }],
    )
    .context("construct contribution-bound cooperative claim")?;
    ensure!(
        facts.bitcoin_refund_height
            == facts
                .planned_bitcoin_funding_anchor_height
                .checked_add(facts.refund_csv_blocks)
                .context("Bitcoin refund height overflow")?,
        "Bitcoin refund height differs from anchor plus CSV"
    );
    let chain = pair.maker().body().lez_chain_identity();
    let lez_depositor = bitcoin_claimant;
    let lez_claimant = bitcoin_funder;
    let lez = BtcLezTermsV1::new(
        *chain.channel_id(),
        *chain.genesis_block_hash(),
        *chain.escrow_program_id(),
        *chain.authenticated_transfer_program_id(),
        facts.lez_aggregate_authority_account,
        facts.lez_metadata_account,
        facts.lez_custody_account,
        *participants
            .for_participant(lez_depositor)
            .lez_owner_account(),
        *participants
            .for_participant(lez_claimant)
            .lez_owner_account(),
        facts.lez_amount,
        facts.lez_refund_at_ms,
        facts.lez_prepared_claim_message_hash,
    );
    let recovery = BtcRecoveryPlanV1::new(
        facts.planned_bitcoin_funding_anchor_height,
        facts.bitcoin_refund_height,
        facts.maker_second_lock_cutoff_unix_seconds,
        facts.earlier_refund_latest_unix_seconds,
        facts.later_refund_earliest_unix_seconds,
        facts.required_margin_seconds,
    );
    let body = BtcAgreementBodyV1::new(
        *pair.swap_id(),
        pair.maker().body().direction(),
        *pair.maker().body().bitcoin_chain_policy(),
        participants,
        *pair.adaptor_point(),
        lez,
        BtcP2trTermsV1::from_contract(&contract),
        funding,
        BtcClaimTermsV1::from_spend(&claim).context("construct Bitcoin claim terms")?,
        recovery,
    );
    pair.validate_agreement_body_fields(&body)
        .context("composed draft changed signed role contributions")?;
    let draft = BtcAgreementDraftV1::validate(body).context("validate composed agreement draft")?;
    let draft_wire = draft
        .encode_wire()
        .context("encode unsigned agreement draft")?;
    let replay = BtcAgreementDraftV1::from_wire(&draft_wire)
        .context("revalidate encoded agreement draft")?;
    ensure!(
        replay.encode_wire()? == draft_wire,
        "unsigned agreement draft is not canonical"
    );
    Ok(ComposedAgreementDraft {
        wire: draft_wire,
        direction: direction.protocol(),
        swap_id: *pair.swap_id(),
        agreement_commitment: draft.commitment(),
        bitcoin_contract_script_pubkey: contract.script_pubkey_bytes().to_vec(),
        bitcoin_claim_bip341_sighash: claim.sighash_bytes(),
        lez_channel_id: *chain.channel_id(),
    })
}

/// The private scalars a bootstrapped role root holds (raw 32-byte files).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RoleSecret {
    /// The `MuSig2` agreement key that signs contributions and agreements.
    Agreement,
    /// The Bitcoin refund key (only meaningful for the Bitcoin funder).
    BitcoinRefund,
    /// The Taker's adaptor scalar; absent on a Maker root.
    Adaptor,
}

/// The fixed file layout of one role root, so callers never spell file names.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoleRootLayout {
    root: PathBuf,
}

impl RoleRootLayout {
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn contribution_file(&self) -> PathBuf {
        self.root.join(CONTRIBUTION_FILE)
    }

    #[must_use]
    pub fn peer_contribution_file(&self) -> PathBuf {
        self.root.join(PEER_CONTRIBUTION_FILE)
    }

    /// The accepted countersigned agreement, present once bound.
    #[must_use]
    pub fn agreement_file(&self) -> PathBuf {
        self.root.join(ACCEPTED_AGREEMENT_FILE)
    }

    #[must_use]
    pub fn binding_file(&self) -> PathBuf {
        self.root.join(AGREEMENT_BINDING_FILE)
    }

    #[must_use]
    pub fn secret_file(&self, secret: RoleSecret) -> PathBuf {
        self.root.join(PRIVATE_DIRECTORY).join(match secret {
            RoleSecret::Agreement => AGREEMENT_KEY_FILE,
            RoleSecret::BitcoinRefund => REFUND_KEY_FILE,
            RoleSecret::Adaptor => ADAPTOR_FILE,
        })
    }

    /// Reads one private scalar of this root, validating it as a secp256k1 key.
    ///
    /// # Errors
    ///
    /// Fails when the file is missing, not owner-private, or not a valid scalar.
    pub fn read_secret(&self, secret: RoleSecret) -> Result<Zeroizing<[u8; 32]>> {
        let name = match secret {
            RoleSecret::Agreement => "agreement key",
            RoleSecret::BitcoinRefund => "refund key",
            RoleSecret::Adaptor => "adaptor scalar",
        };
        read_secret_bytes(&self.secret_file(secret), name)
    }
}

/// Imports and binds a final countersigned agreement against both signed public
/// contributions and every private counterpart owned by one role root.
///
/// This intentionally reports `ready_for_public_effects: false`: presignature
/// journals, refunds, exact lock effects, and role-owned funding authorization
/// are separate post-agreement gates and cannot be implied by countersigning.
///
/// # Errors
///
/// Rejects expired/cross-wired contributions, agreement substitution, a local
/// private-key mismatch, unsafe files, or a conflicting binding receipt.
pub fn bind_countersigned_agreement(
    role_root: &Path,
    peer_contribution_file: &Path,
    agreement_file: &Path,
    accepted_at_unix_seconds: u64,
) -> Result<AgreementBindingSummary> {
    let peer_wire = read_stable_private_file(
        peer_contribution_file,
        MAX_BTC_ROLE_CONTRIBUTION_RECORD_BYTES,
    )?;
    let agreement_wire = read_stable_private_file(agreement_file, MAX_BTC_AGREEMENT_RECORD_BYTES)?;
    persist_and_bind_countersigned_agreement(
        role_root,
        &peer_wire,
        &agreement_wire,
        accepted_at_unix_seconds,
    )
}

/// Persists and binds one exact countersigned agreement plus its peer public
/// contribution inside an existing role-local authority root.
///
/// The two files and the binding receipt are create-new or exact-replay only.
/// This is the fixture-independent acceptance boundary used by Chat v2; it
/// creates no actor, prepares no public effect, and authorizes no funding.
///
/// # Errors
///
/// Rejects every invalid role binding, unsafe path, partial/cross-wired replay,
/// or byte collision.
pub fn persist_and_bind_countersigned_agreement(
    role_root: &Path,
    peer_contribution_wire: &[u8],
    agreement_wire: &[u8],
    accepted_at_unix_seconds: u64,
) -> Result<AgreementBindingSummary> {
    validate_existing_output_root(role_root)?;
    ensure!(
        !peer_contribution_wire.is_empty()
            && peer_contribution_wire.len() <= MAX_BTC_ROLE_CONTRIBUTION_RECORD_BYTES
            && !agreement_wire.is_empty()
            && agreement_wire.len() <= MAX_BTC_AGREEMENT_RECORD_BYTES,
        "accepted agreement material is empty or oversized"
    );
    let local_wire = read_stable_private_file(
        &role_root.join(CONTRIBUTION_FILE),
        MAX_BTC_ROLE_CONTRIBUTION_RECORD_BYTES,
    )?;
    let peer_file = role_root.join(PEER_CONTRIBUTION_FILE);
    let agreement_file = role_root.join(ACCEPTED_AGREEMENT_FILE);
    let summary = prepare_agreement_binding(
        role_root,
        &local_wire,
        peer_contribution_wire,
        &agreement_file,
        agreement_wire,
        accepted_at_unix_seconds,
    )?;
    // The inactive receipt is the durable acceptance linearization point. It
    // is published before the two exact artifacts so a crash can leave only a
    // repairable, non-effect-authorizing prefix; replay then restores missing
    // artifacts without applying a new wall-clock expiry decision.
    let summary = publish_agreement_binding(role_root, summary)?;
    persist_or_match_private(&peer_file, peer_contribution_wire)?;
    persist_or_match_private(&agreement_file, agreement_wire)?;
    fs::File::open(role_root)?.sync_all()?;
    Ok(summary)
}

#[allow(clippy::too_many_lines)]
fn prepare_agreement_binding(
    role_root: &Path,
    local_wire: &[u8],
    peer_wire: &[u8],
    agreement_file: &Path,
    agreement_wire: &[u8],
    accepted_at_unix_seconds: u64,
) -> Result<AgreementBindingSummary> {
    let local = BtcRoleContributionV1::from_wire(local_wire)?;
    let peer = BtcRoleContributionV1::from_wire(peer_wire)?;
    ensure!(
        local.body().role() != peer.body().role(),
        "peer role aliases local role"
    );
    let pair = match local.body().role() {
        Participant::Maker => BtcRoleContributionPairV1::new(local.clone(), peer.clone()),
        Participant::Taker => BtcRoleContributionPairV1::new(peer.clone(), local.clone()),
    }?;
    let agreement = BtcAgreementV1::from_wire(agreement_wire)?;
    pair.validate_agreement_body_fields(agreement.body())
        .context("agreement substituted contribution-bound roles or chain identities")?;

    let private_root = role_root.join(PRIVATE_DIRECTORY);
    validate_existing_output_root(&private_root)?;
    let agreement_secret =
        read_secret_bytes(&private_root.join(AGREEMENT_KEY_FILE), "agreement key")?;
    let refund_secret = read_secret_bytes(&private_root.join(REFUND_KEY_FILE), "refund key")?;
    let claim_secret = read_secret_bytes(&private_root.join(CLAIM_KEY_FILE), "claim key")?;
    let funding_secret = read_secret_bytes(&private_root.join(FUNDING_KEY_FILE), "funding key")?;
    let secp = Secp256k1::new();
    let identity = local.body().participant_identity();
    ensure!(
        compressed_public_key(&secp, &agreement_secret, "agreement key")?.serialize()
            == *identity.musig2_public_key()
            && x_only_public_key(&secp, &refund_secret, "refund key")?.serialize()
                == *identity.bitcoin_refund_key()
            && ScriptBuf::new_p2tr(
                &Secp256k1::verification_only(),
                x_only_public_key(&secp, &claim_secret, "claim key")?,
                None,
            )
            .into_bytes()
                == identity.claim_destination_script_pubkey()
            && x_only_public_key(&secp, &funding_secret, "funding key")?.serialize()
                == *local.body().bitcoin_funding_key(),
        "role-private authority differs from the signed local contribution"
    );
    let adaptor_file = private_root.join(ADAPTOR_FILE);
    match local.body().role() {
        Participant::Maker => ensure!(
            fs::symlink_metadata(&adaptor_file)
                .is_err_and(|error| { error.kind() == std::io::ErrorKind::NotFound }),
            "Maker role root must not contain Taker adaptor authority"
        ),
        Participant::Taker => {
            let adaptor = read_secret_bytes(&adaptor_file, "adaptor scalar")?;
            ensure!(
                compressed_public_key(&secp, &adaptor, "adaptor scalar")?.serialize()
                    == *pair.adaptor_point(),
                "Taker adaptor scalar differs from the signed contribution"
            );
        }
    }

    let binding_file = role_root.join(AGREEMENT_BINDING_FILE);
    let (local_commitment, peer_commitment) = match local.body().role() {
        Participant::Maker => (
            pair.maker().contribution_commitment(),
            pair.taker().contribution_commitment(),
        ),
        Participant::Taker => (
            pair.taker().contribution_commitment(),
            pair.maker().contribution_commitment(),
        ),
    };
    let summary = AgreementBindingSummary {
        schema_version: SPEC_SCHEMA_VERSION,
        role: RoleName::from_protocol(local.body().role()),
        swap_id: hex::encode(pair.swap_id()),
        agreement_file: agreement_file.to_path_buf(),
        agreement_sha256: hex::encode(Sha256::digest(agreement_wire)),
        agreement_commitment: hex::encode(agreement.agreement_commitment()),
        local_contribution_commitment: hex::encode(local_commitment),
        peer_contribution_commitment: hex::encode(peer_commitment),
        accepted_at_unix_seconds,
        contribution_expires_at_unix_seconds: local.body().expires_at_unix_seconds(),
        roles_and_chain_identities_revalidated: true,
        local_private_authority_revalidated: true,
        ready_for_public_effects: false,
        binding_file: binding_file.clone(),
        private_material_disclosed: false,
        was_replay: false,
    };
    match fs::symlink_metadata(&binding_file) {
        Ok(_) => {
            let bytes = read_stable_private_file(&binding_file, MAX_JSON_BYTES)?;
            let mut persisted: AgreementBindingSummary =
                serde_json::from_slice(&bytes).context("invalid agreement binding replay")?;
            ensure!(
                persisted.schema_version == summary.schema_version
                    && persisted.role as u8 == summary.role as u8
                    && persisted.swap_id == summary.swap_id
                    && persisted.agreement_file == summary.agreement_file
                    && persisted.agreement_sha256 == summary.agreement_sha256
                    && persisted.agreement_commitment == summary.agreement_commitment
                    && persisted.local_contribution_commitment
                        == summary.local_contribution_commitment
                    && persisted.peer_contribution_commitment
                        == summary.peer_contribution_commitment
                    && persisted.contribution_expires_at_unix_seconds
                        == summary.contribution_expires_at_unix_seconds
                    && persisted.roles_and_chain_identities_revalidated
                    && persisted.local_private_authority_revalidated
                    && !persisted.ready_for_public_effects
                    && persisted.binding_file == binding_file
                    && !persisted.private_material_disclosed
                    && !persisted.was_replay,
                "agreement binding replay changed role authority"
            );
            persisted.was_replay = true;
            return Ok(persisted);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("inspect agreement binding replay"),
    }
    ensure!(
        accepted_at_unix_seconds != 0
            && accepted_at_unix_seconds < local.body().expires_at_unix_seconds(),
        "role contributions expired before acceptance"
    );
    pair.validate_agreement_body(agreement.body(), accepted_at_unix_seconds)
        .context("agreement expired before local acceptance")?;
    Ok(summary)
}

fn publish_agreement_binding(
    role_root: &Path,
    summary: AgreementBindingSummary,
) -> Result<AgreementBindingSummary> {
    if summary.was_replay {
        return Ok(summary);
    }
    let mut bytes = serde_json::to_vec_pretty(&summary).context("encode agreement binding")?;
    bytes.push(b'\n');
    write_private_new(&summary.binding_file, &bytes)?;
    fs::File::open(role_root)?.sync_all()?;
    Ok(summary)
}

fn persist_or_match_private(path: &Path, expected: &[u8]) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            let actual = read_stable_private_file(path, expected.len())?;
            ensure!(actual == expected, "persisted role material changed");
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            write_private_new(path, expected)?;
            Ok(false)
        }
        Err(error) => Err(error).context("inspect persisted role material"),
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
    bail!("OS randomness did not produce a valid secp256k1 secret")
}

fn read_secret_bytes(path: &Path, name: &str) -> Result<Zeroizing<[u8; 32]>> {
    let bytes = Zeroizing::new(read_stable_private_file(path, 32)?);
    ensure!(bytes.len() == 32, "{name} has invalid length");
    let mut value = Zeroizing::new([0_u8; 32]);
    value.copy_from_slice(&bytes);
    let mut parsed =
        SecretKey::from_slice(value.as_ref()).with_context(|| format!("invalid {name}"))?;
    parsed.non_secure_erase();
    Ok(value)
}

fn secret_key(bytes: &[u8; 32], name: &str) -> Result<SecretKey> {
    SecretKey::from_slice(bytes).with_context(|| format!("invalid {name}"))
}

fn compressed_public_key(
    secp: &Secp256k1<bitcoin::secp256k1::All>,
    bytes: &[u8; 32],
    name: &str,
) -> Result<PublicKey> {
    let mut secret = secret_key(bytes, name)?;
    let public = PublicKey::from_secret_key(secp, &secret);
    secret.non_secure_erase();
    Ok(public)
}

fn keypair(
    secp: &Secp256k1<bitcoin::secp256k1::All>,
    bytes: &[u8; 32],
    name: &str,
) -> Result<Keypair> {
    let mut secret = secret_key(bytes, name)?;
    let keypair = Keypair::from_secret_key(secp, &secret);
    secret.non_secure_erase();
    Ok(keypair)
}

fn x_only_public_key(
    secp: &Secp256k1<bitcoin::secp256k1::All>,
    bytes: &[u8; 32],
    name: &str,
) -> Result<bitcoin::secp256k1::XOnlyPublicKey> {
    let mut keypair = keypair(secp, bytes, name)?;
    let public = keypair.x_only_public_key().0;
    keypair.non_secure_erase();
    Ok(public)
}

fn parse_hex32(value: &str, name: &str) -> Result<[u8; 32]> {
    parse_hex_variable(value, name, 32)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("{name} must contain exactly 32 bytes"))
}

fn parse_hex_variable(value: &str, name: &str, maximum_bytes: usize) -> Result<Vec<u8>> {
    ensure!(
        !value.is_empty()
            && value.len() <= maximum_bytes.saturating_mul(2)
            && value.len().is_multiple_of(2)
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "{name} must be nonempty bounded canonical lowercase hex"
    );
    let decoded = hex::decode(value).with_context(|| format!("invalid {name}"))?;
    ensure!(decoded.iter().any(|byte| *byte != 0), "zero {name}");
    Ok(decoded)
}

fn read_strict_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let bytes = read_stable_private_file(path, MAX_JSON_BYTES)?;
    serde_json::from_slice(&bytes).context("invalid strict JSON input")
}

fn read_stable_private_file(path: &Path, maximum: usize) -> Result<Vec<u8>> {
    let before = fs::symlink_metadata(path).context("input file unavailable")?;
    validate_private_file(&before, maximum)?;
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .context("input file unavailable or unsafe")?;
    let opened = file.metadata().context("input metadata unavailable")?;
    validate_private_file(&opened, maximum)?;
    ensure!(same_file(&before, &opened), "input file identity changed");
    let mut bytes = Vec::with_capacity(maximum.min(4_096));
    std::io::Read::by_ref(&mut file)
        .take(u64::try_from(maximum)?.saturating_add(1))
        .read_to_end(&mut bytes)?;
    let after = fs::symlink_metadata(path).context("input file disappeared")?;
    validate_private_file(&after, maximum)?;
    ensure!(
        same_file(&opened, &after),
        "input file changed while reading"
    );
    ensure!(
        !bytes.is_empty() && bytes.len() <= maximum,
        "invalid input size"
    );
    Ok(bytes)
}

fn validate_private_file(metadata: &fs::Metadata, maximum: usize) -> Result<()> {
    ensure!(
        metadata.file_type().is_file()
            && metadata.len() > 0
            && metadata.len() <= u64::try_from(maximum)?
            && metadata.nlink() == 1
            && metadata.permissions().mode() & 0o7777 == 0o600,
        "input must be one owner-only regular file"
    );
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
        fs::canonicalize(parent)? == parent,
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
    let metadata = fs::symlink_metadata(path).context("role root unavailable")?;
    ensure!(
        metadata.file_type().is_dir()
            && metadata.permissions().mode() & 0o7777 == 0o700
            && fs::canonicalize(path)? == path,
        "role root is unsafe"
    );
    Ok(())
}

fn ensure_normalized_absolute(path: &Path) -> Result<()> {
    ensure!(path.is_absolute(), "path must be absolute");
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => normalized.push(Path::new("/")),
            Component::Normal(value) => normalized.push(value),
            Component::CurDir | Component::ParentDir | Component::Prefix(_) => {
                bail!("path must be normalized")
            }
        }
    }
    ensure!(normalized == path, "path must be normalized");
    Ok(())
}

fn create_private_directory(path: &Path) -> Result<()> {
    DirBuilder::new().mode(0o700).create(path)?;
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.file_type().is_dir() && metadata.permissions().mode() & 0o7777 == 0o700,
        "created directory permissions are unsafe"
    );
    Ok(())
}

fn write_private_new(path: &Path, bytes: &[u8]) -> Result<()> {
    ensure!(!bytes.is_empty(), "refusing to write an empty file");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file()
            && metadata.permissions().mode() & 0o7777 == 0o600
            && metadata.nlink() == 1,
        "created file permissions are unsafe"
    );
    Ok(())
}
