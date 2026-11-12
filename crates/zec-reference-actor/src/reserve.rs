//! Take-time composition of one Zcash agreement over Chat (`zec_reserve_v1`).
//!
//! The Bitcoin model (ADR 0213, S1.3): the Taker reserves at take time and the
//! Maker answers with the facts only it holds. Each role contributes its own
//! public facts and both compose the identical unsigned body with the dealer's
//! [`compose_agreement_body`]: the Maker returns the draft, the Taker
//! recomposes it from the offer, its own facts and the Maker's few and
//! requires byte equality. Each role writes only its own material; no private
//! key or preimage crosses Chat. The existing `zec_chat_propose_v1` and
//! `zec_chat_complete_v1` then sign it, each role with its own key.
//!
//! The Zcash funder makes the claim preimage (`local_poc::preimage_owner`):
//! the Taker in `TakerSellsForeign`, the Maker in `TakerSellsLez`; the other
//! role only ever sees its SHA-256 in the agreement.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, ensure};
use lez_bridge_protocol::{DiscoveryWindow, Hex32, RequestId, RunId, RuntimeDescriptor};
use lez_swap_core::{SwapDirection, SwapId, UnixSeconds};
use lez_zebra_node_adapter::{HttpZebraRpc, ZebraRpc};
use lez_zec_swap_sdk::{
    NegotiationTranscriptV1, ZcashFundingInputV1, ZecAgreementDraftV1, ZecProfileId,
    ZecRefundProfile,
};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use url::Url;
use zcash_protocol::consensus::NetworkType;
use zcash_transparent::{address::Script, bundle::OutPoint};
use zeroize::Zeroizing;

use crate::{
    ActorConfig, ActorRole, CandidateOutpoint,
    config::encode_deterministic_local_v0_2_actor_config,
    local_poc::{
        AgreementFacts, FUNDING_FEE_ZATOSHIS, FundingChain, LezChainFacts, LocalPocProfile,
        LocalPocZebraRoute, MAX_TESTNET_FUNDING_SCAN_BLOCKS, MINIMUM_CHANGE_ZATOSHIS, RoleCorridor,
        actor_config_input, block_hash_display_hex, compose_agreement_body, connect_zebra,
        create_private_directory, fresh_claim_preimage, material_paths, read_zcash_key_file,
        read_zebra_credential, select_funding_candidate, write_private_new, zcash_funder,
    },
    secure_file::{FilePrivacy, read_bounded_identified},
};

const ROLE_CONFIG_SCHEMA_VERSION: u16 = 1;
const MAX_ROLE_CONFIG_BYTES: usize = 16 * 1024;
/// Default depth of the funder search below the tip at take time: the
/// devnet's Zebra mines to the Maker's key, so recent mature coinbases are
/// plentiful; a search from height 1 reads the whole chain (minutes).
const DEFAULT_FUNDING_SCAN_BLOCKS: u32 = 1_000;
/// How far the Maker's LEZ anchor time may sit from the Taker's clock.
pub const MAX_ANCHOR_CLOCK_SKEW_SECONDS: u64 = 120;
/// How far the Maker's Zcash anchor may sit outside the Taker's own view of
/// the tip (two Zebra nodes may lag each other by a few blocks).
pub const MAX_ANCHOR_HEIGHT_SKEW_BLOCKS: u32 = 3;
/// The file a role's take-time draft is kept in; its actor template names it
/// as the agreement until the countersigned agreement replaces it.
pub const ZEC_ROLE_DRAFT_FILE: &str = "draft.borsh";

/// A Node's Zcash role: the profile it takes swaps under, its Zebra and its
/// own Zcash key. One file, owner-private (`--zec-role-config` on the Maker,
/// `initiation.zec_role_config` on the Taker).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZecRoleConfigV1 {
    schema_version: u16,
    profile: LocalPocProfile,
    zebra_endpoint: Url,
    #[serde(default)]
    zebra_route: LocalPocZebraRoute,
    /// The role's owner-private Zcash key (32 raw bytes or 64 hex). The
    /// Maker's is its Chat signing key and need not be named here.
    #[serde(default)]
    zcash_key_file: Option<PathBuf>,
    /// Blocks below the mature tip the funder search reads.
    #[serde(default = "default_funding_scan_blocks")]
    funding_scan_blocks: u32,
}

const fn default_funding_scan_blocks() -> u32 {
    DEFAULT_FUNDING_SCAN_BLOCKS
}

impl std::fmt::Debug for ZecRoleConfigV1 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ZecRoleConfigV1")
            .field("profile", &self.profile.id().as_str())
            .field("zebra_endpoint", &self.zebra_endpoint.as_str())
            .finish_non_exhaustive()
    }
}

impl ZecRoleConfigV1 {
    /// Loads and checks one owner-private role configuration.
    ///
    /// # Errors
    ///
    /// Fails for an unsafe or malformed file, a profile on another network
    /// than its Zebra route, or a funder search outside its bound.
    pub fn load(path: &Path) -> Result<Self> {
        let (bytes, _) =
            read_bounded_identified(path, MAX_ROLE_CONFIG_BYTES, FilePrivacy::OwnerPrivate)
                .map_err(|_| anyhow::anyhow!("Zcash role config is unavailable or unsafe"))?;
        let config: Self =
            serde_json::from_slice(bytes.as_slice()).context("Zcash role config is invalid")?;
        ensure!(
            config.schema_version == ROLE_CONFIG_SCHEMA_VERSION,
            "unsupported Zcash role config schema"
        );
        ensure!(
            ZecRefundProfile::for_id(config.profile.id()).zcash_network()
                == config.zebra_route.network(),
            "the profile's Zcash network is not the Zebra route's"
        );
        ensure!(
            config.funding_scan_blocks > 0
                && (config.zebra_route.network() != NetworkType::Test
                    || config.funding_scan_blocks < MAX_TESTNET_FUNDING_SCAN_BLOCKS),
            "funding_scan_blocks is outside its bound"
        );
        if let Some(key) = &config.zcash_key_file {
            read_zcash_key_file(key, "role")?;
        }
        Ok(config)
    }

    /// The profile every swap of this role is taken under.
    #[must_use]
    pub const fn profile(&self) -> ZecProfileId {
        self.profile.id()
    }

    /// A bounded client of this role's Zebra.
    ///
    /// # Errors
    ///
    /// Fails for an unsafe route credential or an invalid endpoint.
    pub fn connect(&self) -> Result<HttpZebraRpc> {
        let credential = read_zebra_credential(&self.zebra_route)?;
        connect_zebra(
            &self.zebra_endpoint,
            &self.zebra_route,
            credential.as_ref().map(|secret| secret.as_slice()),
        )
    }

    /// The role's own Zcash key, read once.
    ///
    /// # Errors
    ///
    /// Fails when no key file is configured or it is unsafe or invalid.
    pub fn zcash_key(&self) -> Result<Zeroizing<[u8; 32]>> {
        let path = self
            .zcash_key_file
            .as_deref()
            .context("the Zcash role config names no zcash_key_file")?;
        read_zcash_key_file(path, "role")
    }

    fn chain(&self) -> Result<FundingChain> {
        FundingChain::for_profile(ZecRefundProfile::for_id(self.profile.id()))
    }
}

/// One role's own LEZ facts: the descriptor its sidecar runs under (whose
/// signer account is the role's LEZ account) and the chain's
/// authenticated-transfer program.
#[derive(Clone, Debug)]
pub struct ZecLezRole {
    pub runtime: RuntimeDescriptor,
    pub authenticated_transfer_program_id: [u8; 32],
}

impl ZecLezRole {
    fn chain_facts(&self) -> LezChainFacts {
        LezChainFacts {
            channel_id: *self.runtime.channel_id.as_bytes(),
            genesis_block_hash: *self.runtime.genesis_block_hash.as_bytes(),
            escrow_program_id: self.runtime.escrow_program_id,
            authenticated_transfer_program_id: self.authenticated_transfer_program_id,
        }
    }

    const fn account(&self) -> [u8; 32] {
        *self.runtime.signer_account_id.as_bytes()
    }
}

/// The application swap id both roles derive from the authenticated offer and
/// the Taker's reservation, so neither picks it alone.
///
/// # Errors
///
/// Never in practice: the derived id is always a valid swap id.
pub fn zec_dynamic_swap_id(
    offer_commitment: &[u8; 32],
    reservation_id: &RequestId,
) -> Result<SwapId> {
    let mut hasher = Sha256::new();
    hasher.update(b"lez-atomic-swaps/zec-dynamic-swap/v1\0");
    hasher.update(offer_commitment);
    hasher.update(reservation_id.as_str().as_bytes());
    SwapId::new(format!("zec-{}", hex::encode(hasher.finalize()))).context("derived swap id")
}

/// What both roles agree before composing: the offer-bound terms.
#[derive(Clone, Debug)]
pub struct ZecReservationTerms {
    pub direction: SwapDirection,
    pub swap_id: SwapId,
    pub transcript: NegotiationTranscriptV1,
    pub zcash_principal: u64,
    pub lez_amount: u128,
}

/// The Maker's answer: its draft and the facts only it chose.
#[derive(Clone, Debug)]
pub struct ZecMakerReservation {
    pub draft_wire: Vec<u8>,
    /// Zcash funding anchor: the block after the Maker's Zebra tip.
    pub zcash_anchor_height: u32,
    /// LEZ funding anchor, Unix seconds.
    pub lez_anchor_unix_seconds: u64,
    /// The Maker's committed funding outpoint (`TakerSellsLez`).
    pub maker_funding: Option<CandidateOutpoint>,
    /// SHA-256 of the Maker's claim preimage (`TakerSellsLez`).
    pub maker_secret_digest: Option<[u8; 32]>,
    /// The Zebra genesis the actor template pins, RPC display order.
    pub zebra_genesis: Hex32,
}

/// What the Taker brings to a reservation.
#[derive(Clone, Debug)]
pub struct ZecTakerContribution {
    pub taker_zcash_key: [u8; 33],
    pub taker_lez_account: [u8; 32],
    /// The Taker's funding outpoint (`TakerSellsForeign`).
    pub taker_funding: Option<CandidateOutpoint>,
    /// SHA-256 of the Taker's claim preimage (`TakerSellsForeign`).
    pub taker_secret_digest: Option<[u8; 32]>,
}

fn outpoint(candidate: &CandidateOutpoint) -> OutPoint {
    let mut hash = *candidate.transaction_id().as_bytes();
    hash.reverse();
    OutPoint::new(hash, candidate.output_index())
}

/// Reads one committed funding output from Zebra: unspent, paying `owner`
/// P2PKH, at least the principal plus fee and change, and as deep as the
/// profile's funder outputs must be.
async fn read_funding_input<R>(
    rpc: &R,
    chain: &FundingChain,
    candidate: &CandidateOutpoint,
    owner: &PublicKey,
    zcash_principal: u64,
) -> Result<ZcashFundingInputV1>
where
    R: ZebraRpc,
    R::Error: std::error::Error + Send + Sync + 'static,
{
    let point = outpoint(candidate);
    let unspent = rpc
        .unspent_output(&point)
        .await
        .context("Zebra exact UTXO query failed")?
        .context("the committed funding output is spent or unknown")?;
    let output = unspent.output();
    let owner_script: Script = lez_zec_swap_sdk::p2pkh_address(owner).script().into();
    let minimum = zcash_principal
        .saturating_add(FUNDING_FEE_ZATOSHIS)
        .saturating_add(MINIMUM_CHANGE_ZATOSHIS);
    ensure!(
        output.script_pubkey() == &owner_script
            && u64::from(output.value()) >= minimum
            && unspent.confirmations() >= chain.min_confirmations,
        "the committed funding output is not its funder's, too small or too shallow"
    );
    Ok(ZcashFundingInputV1::new(
        *point.hash(),
        point.n(),
        u64::from(output.value()),
        output.script_pubkey().0.0.clone(),
    ))
}

async fn tip_height<R>(rpc: &R, chain: &FundingChain) -> Result<(u32, Hex32)>
where
    R: ZebraRpc,
    R::Error: std::error::Error + Send + Sync + 'static,
{
    let info = rpc
        .chain_info()
        .await
        .context("Zebra identity unavailable")?;
    ensure!(
        info.rpc_chain() == chain.identity.rpc_chain()
            && info.consensus_branch_id() == chain.identity.consensus_branch_id(),
        "Zebra is not the profile's chain and branch"
    );
    let genesis = rpc
        .block_hash(zcash_protocol::consensus::BlockHeight::from_u32(0))
        .await
        .context("Zebra genesis unavailable")?;
    ensure!(
        genesis == chain.identity.genesis_hash(),
        "Zebra genesis is not the profile's"
    );
    Ok((
        u32::from(info.tip_height()),
        block_hash_display_hex(genesis),
    ))
}

/// The role's Zebra tip, after checking it is the profile's chain.
///
/// # Errors
///
/// Fails when Zebra is unreachable or another chain or branch.
pub async fn zec_tip_height<R>(config: &ZecRoleConfigV1, rpc: &R) -> Result<u32>
where
    R: ZebraRpc,
    R::Error: std::error::Error + Send + Sync + 'static,
{
    Ok(tip_height(rpc, &config.chain()?).await?.0)
}

/// Selects the funder's own output for a new reservation: the first mature
/// unspent one in the last `funding_scan_blocks` below the tip that no other
/// live reservation (`exclude`) committed.
///
/// # Errors
///
/// Fails when Zebra is not the profile's chain or holds no such output.
pub async fn select_zec_funding<R>(
    config: &ZecRoleConfigV1,
    rpc: &R,
    funder: &PublicKey,
    zcash_principal: u64,
    exclude: &[CandidateOutpoint],
) -> Result<CandidateOutpoint>
where
    R: ZebraRpc,
    R::Error: std::error::Error + Send + Sync + 'static,
{
    let chain = config.chain()?;
    let (tip, _) = tip_height(rpc, &chain).await?;
    let scan_from = tip
        .saturating_sub(chain.min_confirmations)
        .saturating_sub(config.funding_scan_blocks)
        .max(1);
    let exclude: Vec<OutPoint> = exclude.iter().map(outpoint).collect();
    let (candidate, _, _) = select_funding_candidate(
        rpc,
        &chain,
        funder,
        0,
        &exclude,
        scan_from,
        zcash_principal
            .saturating_add(FUNDING_FEE_ZATOSHIS)
            .saturating_add(MINIMUM_CHANGE_ZATOSHIS),
    )
    .await?;
    Ok(CandidateOutpoint::new(
        candidate.rpc_transaction_id,
        candidate.outpoint.n(),
    ))
}

/// Writes a role's own secrets into a new private `role_root`: its Zcash key,
/// a fresh claim-recovery key and, when it funds Zcash, its claim preimage.
fn write_role_secrets(
    role_root: &Path,
    zcash_secret: &[u8; 32],
    preimage: Option<&[u8; 32]>,
) -> Result<()> {
    let paths = material_paths(role_root, &role_root.join("state"));
    create_private_directory(role_root)?;
    create_private_directory(&paths.state_root)?;
    let mut recovery = Zeroizing::new([0_u8; 32]);
    while recovery.iter().all(|byte| *byte == 0) {
        getrandom::fill(&mut recovery[..])
            .map_err(|_| anyhow::anyhow!("claim recovery randomness unavailable"))?;
    }
    write_private_new(&paths.claim_recovery_key, recovery.as_ref())?;
    write_private_new(&paths.zcash_key, zcash_secret)?;
    if let Some(preimage) = preimage {
        write_private_new(&paths.claim_preimage, preimage)?;
    }
    Ok(())
}

/// Makes the Taker's claim preimage before it reserves (it funds Zcash in
/// `TakerSellsForeign`): written into a new private `role_root`, with the
/// Taker's own Zcash key and recovery key; returns its SHA-256.
///
/// # Errors
///
/// Fails when `role_root` exists or cannot be written.
pub fn begin_zec_taker_role(role_root: &Path, taker_zcash_secret: &[u8; 32]) -> Result<[u8; 32]> {
    let preimage = fresh_claim_preimage()?;
    write_role_secrets(role_root, taker_zcash_secret, Some(&preimage))?;
    Ok(Sha256::digest(&preimage[..]).into())
}

/// The Maker's half of a reservation: checks the Taker's contribution against
/// its own Zebra, fixes the anchors at `now`, selects its own funding output
/// (`TakerSellsLez`, never one in `exclude`) and makes the claim preimage
/// there, composes the unsigned draft and writes the Maker's own material and
/// the draft into a new private `role_root`.
///
/// # Errors
///
/// Fails for a Taker contribution that does not fit the direction, a Taker
/// funding output that is not its own, unspent, large and deep enough, a
/// Zebra off the profile's chain, no Maker funding output, or a draft the
/// SDK refuses.
#[allow(clippy::too_many_arguments)]
pub async fn reserve_zec_maker_role<R>(
    config: &ZecRoleConfigV1,
    rpc: &R,
    terms: &ZecReservationTerms,
    lez: &ZecLezRole,
    maker_zcash_secret: &[u8; 32],
    taker: &ZecTakerContribution,
    exclude: &[CandidateOutpoint],
    now: u64,
    role_root: &Path,
) -> Result<ZecMakerReservation>
where
    R: ZebraRpc,
    R::Error: std::error::Error + Send + Sync + 'static,
{
    let chain = config.chain()?;
    let mut maker_secret = SecretKey::from_slice(maker_zcash_secret)?;
    let maker_zcash_key = PublicKey::from_secret_key(&Secp256k1::signing_only(), &maker_secret);
    maker_secret.non_secure_erase();
    let taker_zcash_key =
        PublicKey::from_slice(&taker.taker_zcash_key).context("invalid Taker Zcash key")?;
    ensure!(
        taker_zcash_key != maker_zcash_key && taker.taker_lez_account != lez.account(),
        "the Taker's keys are the Maker's"
    );
    let maker_funds = zcash_funder(terms.direction) == ActorRole::Maker;
    ensure!(
        maker_funds == taker.taker_funding.is_none()
            && maker_funds == taker.taker_secret_digest.is_none(),
        "the Taker's funding facts do not fit the direction"
    );
    let (tip, genesis) = tip_height(rpc, &chain).await?;
    let (funding_input, maker_funding, preimage, secret_digest) = if maker_funds {
        let candidate = select_zec_funding(
            config,
            rpc,
            &maker_zcash_key,
            terms.zcash_principal,
            exclude,
        )
        .await?;
        let input = read_funding_input(
            rpc,
            &chain,
            &candidate,
            &maker_zcash_key,
            terms.zcash_principal,
        )
        .await?;
        let preimage = fresh_claim_preimage()?;
        let digest: [u8; 32] = Sha256::digest(&preimage[..]).into();
        (input, Some(candidate), Some(preimage), digest)
    } else {
        let candidate = taker.taker_funding.as_ref().context("Taker funding")?;
        ensure!(
            !exclude.contains(candidate),
            "the Taker's funding output backs another reservation"
        );
        let input = read_funding_input(
            rpc,
            &chain,
            candidate,
            &taker_zcash_key,
            terms.zcash_principal,
        )
        .await?;
        let digest = taker.taker_secret_digest.context("Taker secret digest")?;
        ensure!(digest != [0; 32], "the Taker's secret digest is zero");
        (input, None, None, digest)
    };
    let facts = AgreementFacts {
        profile: config.profile(),
        direction: terms.direction,
        swap_id: terms.swap_id.clone(),
        maker_lez_account: lez.account(),
        taker_lez_account: taker.taker_lez_account,
        maker_zcash_key,
        taker_zcash_key,
        secret_digest,
        lez: lez.chain_facts(),
        lez_amount: terms.lez_amount,
        zcash_principal: terms.zcash_principal,
        funding_input,
        zebra_tip_height: tip,
        now,
        transcript: terms.transcript,
    };
    let (body, _) = compose_agreement_body(&facts)?;
    let draft_wire = ZecAgreementDraftV1::new(body)
        .encode_wire()
        .context("encode the reservation draft")?;
    ZecAgreementDraftV1::from_wire_at(&draft_wire, UnixSeconds::new(now))
        .context("the reservation draft does not validate")?;
    write_role_secrets(role_root, maker_zcash_secret, preimage.as_deref())?;
    write_private_new(&role_root.join(ZEC_ROLE_DRAFT_FILE), &draft_wire)?;
    Ok(ZecMakerReservation {
        draft_wire,
        zcash_anchor_height: tip.checked_add(1).context("anchor overflow")?,
        lez_anchor_unix_seconds: now,
        maker_funding,
        maker_secret_digest: maker_funds.then_some(secret_digest),
        zebra_genesis: genesis,
    })
}

/// What the Taker checks the Maker's answer against.
#[derive(Clone, Debug)]
pub struct ZecTakerCheck {
    /// The Taker's own Zebra tip read before it reserved.
    pub tip_before: u32,
    /// The Taker's clock when the answer arrived.
    pub now: u64,
    /// The offer's Maker identity, which is its Zcash key.
    pub maker_zcash_key: [u8; 33],
}

/// The Taker's half: recomposes the Maker's draft from the offer terms, its
/// own contribution and the Maker's few facts and requires byte equality,
/// after bounding the Maker's facts by its own Zebra and clock (the anchors,
/// and in `TakerSellsLez` the Maker's funding output and its digest). On
/// success writes the draft into `role_root` (its secrets are already there:
/// [`begin_zec_taker_role`] in `TakerSellsForeign`, written here otherwise);
/// the Maker's Zebra genesis is then the one the Taker's template pins.
///
/// # Errors
///
/// Fails for any field of the draft that is not exactly what the Taker
/// composes, anchors outside the Taker's view, a Maker funding output that is
/// not the Maker's, unspent, large and deep enough, or a draft the SDK refuses.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub async fn check_zec_taker_role<R>(
    config: &ZecRoleConfigV1,
    rpc: &R,
    terms: &ZecReservationTerms,
    lez: &ZecLezRole,
    taker_zcash_secret: &[u8; 32],
    taker: &ZecTakerContribution,
    maker_lez_account: [u8; 32],
    maker: &ZecMakerReservation,
    check: &ZecTakerCheck,
    role_root: &Path,
) -> Result<()>
where
    R: ZebraRpc,
    R::Error: std::error::Error + Send + Sync + 'static,
{
    let chain = config.chain()?;
    let mut taker_secret = SecretKey::from_slice(taker_zcash_secret)?;
    let taker_zcash_key = PublicKey::from_secret_key(&Secp256k1::signing_only(), &taker_secret);
    taker_secret.non_secure_erase();
    ensure!(
        taker_zcash_key.serialize() == taker.taker_zcash_key
            && taker.taker_lez_account == lez.account(),
        "the contribution is not this Taker's"
    );
    let maker_zcash_key =
        PublicKey::from_slice(&check.maker_zcash_key).context("invalid Maker identity")?;
    let (tip_after, genesis) = tip_height(rpc, &chain).await?;
    ensure!(
        maker.zcash_anchor_height
            >= check
                .tip_before
                .saturating_add(1)
                .saturating_sub(MAX_ANCHOR_HEIGHT_SKEW_BLOCKS)
            && maker.zcash_anchor_height
                <= tip_after
                    .saturating_add(1)
                    .saturating_add(MAX_ANCHOR_HEIGHT_SKEW_BLOCKS),
        "the Maker's Zcash anchor is outside this Taker's view of the chain"
    );
    ensure!(
        maker.lez_anchor_unix_seconds.abs_diff(check.now) <= MAX_ANCHOR_CLOCK_SKEW_SECONDS,
        "the Maker's LEZ anchor time is too far from this Taker's clock"
    );
    ensure!(
        maker.zebra_genesis == genesis,
        "the Maker's Zebra is another chain"
    );
    let maker_funds = zcash_funder(terms.direction) == ActorRole::Maker;
    let (funding_input, secret_digest) = if maker_funds {
        ensure!(
            taker.taker_funding.is_none() && taker.taker_secret_digest.is_none(),
            "the Taker funds nothing in this direction"
        );
        let candidate = maker
            .maker_funding
            .as_ref()
            .context("the Maker named no funding output")?;
        let digest = maker
            .maker_secret_digest
            .context("the Maker named no secret digest")?;
        ensure!(digest != [0; 32], "the Maker's secret digest is zero");
        let input = read_funding_input(
            rpc,
            &chain,
            candidate,
            &maker_zcash_key,
            terms.zcash_principal,
        )
        .await?;
        (input, digest)
    } else {
        ensure!(
            maker.maker_funding.is_none() && maker.maker_secret_digest.is_none(),
            "the Maker funds nothing in this direction"
        );
        let candidate = taker.taker_funding.as_ref().context("Taker funding")?;
        let input = read_funding_input(
            rpc,
            &chain,
            candidate,
            &taker_zcash_key,
            terms.zcash_principal,
        )
        .await?;
        (input, taker.taker_secret_digest.context("Taker digest")?)
    };
    let facts = AgreementFacts {
        profile: config.profile(),
        direction: terms.direction,
        swap_id: terms.swap_id.clone(),
        maker_lez_account,
        taker_lez_account: lez.account(),
        maker_zcash_key,
        taker_zcash_key,
        secret_digest,
        lez: lez.chain_facts(),
        lez_amount: terms.lez_amount,
        zcash_principal: terms.zcash_principal,
        funding_input,
        zebra_tip_height: maker
            .zcash_anchor_height
            .checked_sub(1)
            .context("anchor underflow")?,
        now: maker.lez_anchor_unix_seconds,
        transcript: terms.transcript,
    };
    let (body, _) = compose_agreement_body(&facts)?;
    let expected = ZecAgreementDraftV1::new(body)
        .encode_wire()
        .context("encode the expected draft")?;
    ensure!(
        expected == maker.draft_wire,
        "the Maker's draft is not the agreement this Taker composes from the offer and its facts"
    );
    ZecAgreementDraftV1::from_wire_at(&maker.draft_wire, UnixSeconds::new(check.now))
        .context("the Maker's draft does not validate")?;
    if maker_funds {
        write_role_secrets(role_root, taker_zcash_secret, None)?;
    } else {
        ensure!(
            role_root.join("claim-preimage.key").is_file(),
            "the Taker's preimage was not made before it reserved"
        );
    }
    write_private_new(&role_root.join(ZEC_ROLE_DRAFT_FILE), &maker.draft_wire)
}

/// What a role's actor template takes besides its role root.
#[derive(Clone, Debug)]
pub struct ZecRoleTemplate {
    pub role: ActorRole,
    pub run_id: RunId,
    pub swap_id: SwapId,
    /// The role's own sidecar for this swap (`http://127.0.0.1:<port>/`).
    pub bridge_endpoint: String,
    pub capability_file: PathBuf,
    pub runtime: RuntimeDescriptor,
    pub zebra_genesis: Hex32,
    pub lez_discovery: DiscoveryWindow,
    /// The funder's committed outpoint; empty for the other role.
    pub funding: Option<CandidateOutpoint>,
}

/// Writes the role's actor template into `role_root`, around its take-time
/// draft (the countersigned agreement replaces it at provisioning, which
/// checks the two bodies are equal), and proves it loads with its material.
/// Returns the template's path.
///
/// # Errors
///
/// Fails when the role root lacks its material or draft, the template exists,
/// or the written config does not reload and activate.
pub fn write_zec_role_template(
    config: &ZecRoleConfigV1,
    role_root: &Path,
    template: &ZecRoleTemplate,
) -> Result<PathBuf> {
    let mut paths = material_paths(role_root, &role_root.join("state"));
    paths.capability.clone_from(&template.capability_file);
    let draft_file = role_root.join(ZEC_ROLE_DRAFT_FILE);
    let draft = fs::read(&draft_file).context("read the role's draft")?;
    let preimage_file = paths.claim_preimage.clone();
    let funds = preimage_file.is_file();
    ensure!(
        funds == template.funding.is_some(),
        "the role's preimage and funding output disagree"
    );
    if let Some(secret) = read_zebra_credential(&config.zebra_route)? {
        write_private_new(&paths.zebra_credential, secret.as_slice())?;
    }
    let bridge_endpoint =
        Url::parse(&template.bridge_endpoint).context("invalid sidecar endpoint")?;
    let corridor = RoleCorridor {
        run_id: &template.run_id,
        bridge_endpoint: &bridge_endpoint,
        zebra_endpoint: &config.zebra_endpoint,
        zebra_route: &config.zebra_route,
        profile: config.profile(),
    };
    let bytes = encode_deterministic_local_v0_2_actor_config(actor_config_input(
        &corridor,
        &template.swap_id,
        template.role,
        &draft_file,
        Hex32::from_bytes(Sha256::digest(&draft).into()),
        &paths,
        funds.then_some(preimage_file),
        template.runtime.clone(),
        template.zebra_genesis,
        template.lez_discovery,
        template.funding.iter().cloned().collect(),
    ))
    .map_err(|_| anyhow::anyhow!("failed to encode the role's actor template"))?;
    write_private_new(&paths.config, &bytes)?;
    let loaded = ActorConfig::load_private(&paths.config)
        .map_err(|_| anyhow::anyhow!("the role's actor template does not reload"))?;
    ensure!(
        loaded.role() == template.role,
        "the role's actor template changed role"
    );
    loaded
        .load_activate_material()
        .map_err(|_| anyhow::anyhow!("the role's actor template material is invalid"))?;
    Ok(paths.config)
}

/// Whether `wire` is a take-time draft rather than a countersigned agreement.
pub(crate) fn is_draft_wire(wire: &[u8], at: UnixSeconds) -> bool {
    ZecAgreementDraftV1::from_wire_at(wire, at).is_ok()
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use lez_bridge_protocol::{Participant as BridgeParticipant, RuntimeCompatibility};
    use lez_swap_core::Participant;
    use lez_zec_swap_sdk::{ZecAgreementV1, ZecMakerAgreementProposalV1};
    use secp256k1::Message;
    use tempfile::tempdir;

    use super::*;
    use crate::local_poc::{sign_agreement_body, testnet_provision_tests::FakeTestnet};

    /// The Regtest fixture's coinbase pays `[4; 32]`; the other role is `[2; 32]`.
    const FUNDER: [u8; 32] = [4; 32];
    const OTHER: [u8; 32] = [2; 32];
    const PRINCIPAL: u64 = 100_000_000;
    const NOW: u64 = 1_900_000_000;

    fn config(root: &Path) -> ZecRoleConfigV1 {
        let path = root.join("zec-role.json");
        fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 1,
                "profile": "deterministic-local-devnet-v5",
                "zebra_endpoint": "http://127.0.0.1:18232",
            }))
            .unwrap(),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        ZecRoleConfigV1::load(&path).unwrap()
    }

    fn lez(role: BridgeParticipant, account: u8) -> ZecLezRole {
        ZecLezRole {
            runtime: RuntimeDescriptor::new(
                role,
                RuntimeCompatibility::LeeV0_2_0,
                Hex32::from_bytes([1; 32]),
                Hex32::from_bytes([1; 32]),
                Hex32::from_bytes([2; 32]),
                Hex32::from_bytes([3; 32]),
                Hex32::from_bytes([account; 32]),
            ),
            authenticated_transfer_program_id: [9; 32],
        }
    }

    fn public(secret: [u8; 32]) -> PublicKey {
        PublicKey::from_secret_key(
            &Secp256k1::signing_only(),
            &SecretKey::from_slice(&secret).unwrap(),
        )
    }

    fn terms(direction: SwapDirection) -> ZecReservationTerms {
        let reservation = RequestId::new("zec-reserve-test-0001").unwrap();
        ZecReservationTerms {
            direction,
            swap_id: zec_dynamic_swap_id(&[5; 32], &reservation).unwrap(),
            transcript: NegotiationTranscriptV1::new([6; 32], [5; 32], NOW + 3_600),
            zcash_principal: PRINCIPAL,
            lez_amount: 50_000,
        }
    }

    fn zebra() -> FakeTestnet {
        let mut zebra = FakeTestnet::regtest_nu7();
        let txid = zebra.txid();
        zebra.unspent.insert((txid, 0), zebra.tip);
        zebra
    }

    fn private_dir(path: &Path) -> PathBuf {
        fs::create_dir(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        fs::canonicalize(path).unwrap()
    }

    struct Round {
        maker: ZecMakerReservation,
        taker: ZecTakerContribution,
        maker_root: PathBuf,
        taker_root: PathBuf,
        tip: u32,
    }

    /// One reserve round as the Nodes run it: the Taker's preimage first when
    /// it funds Zcash, then the Maker's half.
    async fn reserve(root: &Path, direction: SwapDirection) -> Round {
        let config = config(root);
        let zebra = zebra();
        let (maker_secret, taker_secret) = match direction {
            SwapDirection::TakerSellsLez => (FUNDER, OTHER),
            SwapDirection::TakerSellsForeign => (OTHER, FUNDER),
        };
        let maker_root = private_dir(&root.join("maker")).join("zec");
        let taker_root = private_dir(&root.join("taker")).join("zec");
        let tip = zec_tip_height(&config, &zebra).await.unwrap();
        let (taker_funding, taker_secret_digest) = match direction {
            SwapDirection::TakerSellsForeign => {
                let funding = select_zec_funding(&config, &zebra, &public(FUNDER), PRINCIPAL, &[])
                    .await
                    .unwrap();
                let digest = begin_zec_taker_role(&taker_root, &taker_secret).unwrap();
                (Some(funding), Some(digest))
            }
            SwapDirection::TakerSellsLez => (None, None),
        };
        let taker = ZecTakerContribution {
            taker_zcash_key: public(taker_secret).serialize(),
            taker_lez_account: [0x22; 32],
            taker_funding,
            taker_secret_digest,
        };
        let maker = reserve_zec_maker_role(
            &config,
            &zebra,
            &terms(direction),
            &lez(BridgeParticipant::Maker, 0x11),
            &maker_secret,
            &taker,
            &[],
            NOW,
            &maker_root,
        )
        .await
        .unwrap();
        Round {
            maker,
            taker,
            maker_root,
            taker_root,
            tip,
        }
    }

    async fn check(
        root: &Path,
        direction: SwapDirection,
        round: &Round,
        maker: &ZecMakerReservation,
    ) -> Result<()> {
        let taker_secret = match direction {
            SwapDirection::TakerSellsLez => OTHER,
            SwapDirection::TakerSellsForeign => FUNDER,
        };
        check_zec_taker_role(
            &config(root),
            &zebra(),
            &terms(direction),
            &lez(BridgeParticipant::Taker, 0x22),
            &taker_secret,
            &round.taker,
            [0x11; 32],
            maker,
            &ZecTakerCheck {
                tip_before: round.tip,
                now: NOW + 5,
                maker_zcash_key: public(match direction {
                    SwapDirection::TakerSellsLez => FUNDER,
                    SwapDirection::TakerSellsForeign => OTHER,
                })
                .serialize(),
            },
            &round.taker_root,
        )
        .await
    }

    /// The draft both halves agree on, signed the way Chat signs it (Maker
    /// proposal, Taker countersignature), is byte for byte the agreement the
    /// dealer signs in one step for the same facts.
    #[tokio::test]
    async fn both_halves_compose_the_dealers_agreement_in_either_direction() {
        for direction in [
            SwapDirection::TakerSellsLez,
            SwapDirection::TakerSellsForeign,
        ] {
            let directory = tempdir().unwrap();
            let root = fs::canonicalize(directory.path()).unwrap();
            let round = reserve(&root, direction).await;
            check(&root, direction, &round, &round.maker).await.unwrap();
            // Each role holds only its own material, the funder its preimage.
            let maker_funds = direction == SwapDirection::TakerSellsLez;
            assert_eq!(
                round.maker_root.join("claim-preimage.key").is_file(),
                maker_funds
            );
            assert_eq!(
                round.taker_root.join("claim-preimage.key").is_file(),
                !maker_funds
            );
            assert_eq!(
                fs::read(round.maker_root.join(ZEC_ROLE_DRAFT_FILE)).unwrap(),
                fs::read(round.taker_root.join(ZEC_ROLE_DRAFT_FILE)).unwrap()
            );
            let (maker_secret, taker_secret) = if maker_funds {
                (FUNDER, OTHER)
            } else {
                (OTHER, FUNDER)
            };
            let maker_key = SecretKey::from_slice(&maker_secret).unwrap();
            let taker_key = SecretKey::from_slice(&taker_secret).unwrap();
            let at = UnixSeconds::new(NOW + 5);
            let validated = ZecAgreementDraftV1::from_wire_at(&round.maker.draft_wire, at).unwrap();
            let sign = |key: &SecretKey, commitment: [u8; 32]| {
                Secp256k1::signing_only()
                    .sign_ecdsa(&Message::from_digest(commitment), key)
                    .serialize_compact()
            };
            let commitment = validated.commitment();
            let proposal_wire = validated
                .with_maker_signature(sign(&maker_key, commitment))
                .unwrap()
                .encode_wire()
                .unwrap();
            let chat = ZecMakerAgreementProposalV1::from_wire_at(&proposal_wire, at)
                .unwrap()
                .complete_at(sign(&taker_key, commitment), at)
                .unwrap()
                .encode_wire()
                .unwrap();
            let body = ZecAgreementDraftV1::from_wire_at(&round.maker.draft_wire, at)
                .unwrap()
                .body()
                .clone();
            let dealer = sign_agreement_body(body, &maker_key, &taker_key, NOW + 5).unwrap();
            assert_eq!(chat, dealer.as_slice(), "{direction:?}");
            let agreement = ZecAgreementV1::from_wire_at(&chat, at).unwrap();
            assert_eq!(
                agreement.lez_claimant() == Participant::Maker,
                maker_funds,
                "the Zcash funder claims LEZ"
            );
        }
    }

    #[tokio::test]
    async fn the_taker_refuses_a_draft_it_does_not_compose_itself() {
        let directory = tempdir().unwrap();
        let root = fs::canonicalize(directory.path()).unwrap();
        let direction = SwapDirection::TakerSellsLez;
        let round = reserve(&root, direction).await;
        // Anchors outside the Taker's view of the chain or of its clock.
        let mut early = round.maker.clone();
        early.zcash_anchor_height -= MAX_ANCHOR_HEIGHT_SKEW_BLOCKS + 1;
        assert!(check(&root, direction, &round, &early).await.is_err());
        let mut late = round.maker.clone();
        late.lez_anchor_unix_seconds += MAX_ANCHOR_CLOCK_SKEW_SECONDS + 10;
        assert!(check(&root, direction, &round, &late).await.is_err());
        // A different digest than the draft carries, or a changed draft byte.
        let mut digest = round.maker.clone();
        digest.maker_secret_digest = Some([7; 32]);
        assert!(check(&root, direction, &round, &digest).await.is_err());
        let mut edited = round.maker.clone();
        let last = edited.draft_wire.len() - 1;
        edited.draft_wire[last] ^= 1;
        assert!(check(&root, direction, &round, &edited).await.is_err());
        // A funding output the Maker does not own.
        let mut foreign = round.maker.clone();
        foreign.maker_funding = Some(CandidateOutpoint::new(Hex32::from_bytes([8; 32]), 0));
        assert!(check(&root, direction, &round, &foreign).await.is_err());
        // Nothing was written by the refusals; the honest answer still passes.
        assert!(!round.taker_root.exists());
        check(&root, direction, &round, &round.maker).await.unwrap();
    }

    #[tokio::test]
    async fn the_maker_refuses_what_it_cannot_commit() {
        let directory = tempdir().unwrap();
        let root = fs::canonicalize(directory.path()).unwrap();
        let config = config(&root);
        let zebra = zebra();
        let lez = lez(BridgeParticipant::Maker, 0x11);
        let base = ZecTakerContribution {
            taker_zcash_key: public(OTHER).serialize(),
            taker_lez_account: [0x22; 32],
            taker_funding: None,
            taker_secret_digest: None,
        };
        let attempt = |taker: ZecTakerContribution,
                       direction: SwapDirection,
                       exclude: Vec<CandidateOutpoint>,
                       name: &'static str| {
            let config = &config;
            let zebra = &zebra;
            let lez = &lez;
            let root = root.clone();
            async move {
                reserve_zec_maker_role(
                    config,
                    zebra,
                    &terms(direction),
                    lez,
                    &FUNDER,
                    &taker,
                    &exclude,
                    NOW,
                    &root.join(name),
                )
                .await
            }
        };
        // The only mature output is committed elsewhere: nothing to fund from.
        let only = select_zec_funding(&config, &zebra, &public(FUNDER), PRINCIPAL, &[])
            .await
            .unwrap();
        assert!(
            attempt(
                base.clone(),
                SwapDirection::TakerSellsLez,
                vec![only.clone()],
                "a"
            )
            .await
            .is_err()
        );
        // The Taker's own key, or funding facts in the wrong direction.
        let mut same = base.clone();
        same.taker_zcash_key = public(FUNDER).serialize();
        assert!(
            attempt(same, SwapDirection::TakerSellsLez, Vec::new(), "b")
                .await
                .is_err()
        );
        let mut funded = base.clone();
        funded.taker_funding = Some(only.clone());
        assert!(
            attempt(funded, SwapDirection::TakerSellsLez, Vec::new(), "c")
                .await
                .is_err()
        );
        // TakerSellsForeign: the Taker's outpoint must be the Taker's.
        let mut not_takers = base.clone();
        not_takers.taker_funding = Some(only.clone());
        not_takers.taker_secret_digest = Some([7; 32]);
        assert!(
            attempt(
                not_takers,
                SwapDirection::TakerSellsForeign,
                Vec::new(),
                "d"
            )
            .await
            .is_err()
        );
        for name in ["a", "b", "c", "d"] {
            assert!(!root.join(name).exists(), "a refusal wrote {name}");
        }
        attempt(base, SwapDirection::TakerSellsLez, Vec::new(), "e")
            .await
            .unwrap();
    }
}
