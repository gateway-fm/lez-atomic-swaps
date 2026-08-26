//! Read-only actual-local composer for canonical unsigned M4 Stage A.

use std::{
    fs::{self, File},
    io::{Read as _, Write as _},
    os::unix::fs::MetadataExt as _,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, ensure};
use lez_bridge_protocol::Hex32;
use lez_xmr_monero_adapter::{
    LoopbackRpcEndpoint, MoneroChainIdentity, MoneroChainIdentityAttestor, MoneroNetwork,
};
use lez_xmr_swap_sdk::{
    MAX_XMR_UNSIGNED_STAGE_A_WIRE_BYTES, MoneroAddressNetworkV1, MoneroSharedAddressV1,
    ValidatedXmrAgreementBodyV1, XmrAgreementBodyV1, XmrLezTermsV1, XmrMessagesV1,
    XmrMoneroTermsV1, XmrNamedProfileV1, XmrParticipantsV1, XmrSwapDirectionV1, XmrWindowsV1,
};
use nssa::{Account, AccountId};
use rustix::fs::{Mode, OFlags, open};
use tempfile::NamedTempFile;
use xmr_reference_actor::{ActorRole, ValidatedRolePacket};

use crate::{
    CHECKED_M4_ESCROW_PROGRAM_ID, M4FinalizedAccountIds, M4StageAFutureMessageInput, NonceSource,
    OfficialIndexerRpc, OfficialNativeEscrowFacts, OfficialNodeRpc, compute_custody_pda,
    compute_metadata_pda, plan_m4_stage_a_future_messages, read_stable_m4_finalized_nonce_snapshot,
    validate_checked_m4_escrow_program_id, validate_loopback_http_endpoint,
};

const MAX_CREDENTIAL_FILE_BYTES: u64 = 129;

/// Production residual deliberately retained by the unsigned composer.
///
/// The composer binds the source-controlled checked program identity into the
/// agreement and observes chain/account facts, but a read-only prestate cannot
/// prove that exact image is deployed. The tag-13 deployment/effect boundary
/// must prove the checked program before any chain mutation.
pub const M4_STAGE_A_DEPLOYMENT_RESIDUAL: &str = "unsigned Stage-A composition does not prove the checked M4 ProgramID is deployed; the tag-13 deployment/effect gate must prove that image before submission";

/// Bounded public values committed by a new local M4 Stage-A agreement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct M4StageAParameters {
    swap_id: [u8; 32],
    monero_amount_piconero: u64,
    lez_amount: u128,
    maker_xmr_funding_cutoff_ms: u64,
    refund_at_ms: u64,
    punish_at_ms: u64,
}

impl M4StageAParameters {
    /// Creates the exact public agreement inputs. Full semantic validation is
    /// performed by the canonical SDK before any output is published.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub const fn new(
        swap_id: [u8; 32],
        monero_amount_piconero: u64,
        lez_amount: u128,
        maker_xmr_funding_cutoff_ms: u64,
        refund_at_ms: u64,
        punish_at_ms: u64,
    ) -> Self {
        Self {
            swap_id,
            monero_amount_piconero,
            lez_amount,
            maker_xmr_funding_cutoff_ms,
            refund_at_ms,
            punish_at_ms,
        }
    }
}

/// Exact files and literal-loopback routes for one actual-local composition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActualLocalM4StageAConfig {
    /// Official LEZ v0.2 sequencer root.
    pub sequencer_url: String,
    /// Official LEZ v0.2 finalized-indexer root.
    pub indexer_url: String,
    /// Digest-authenticated official `monerod` root.
    pub monero_daemon_url: String,
    /// Owner-only file containing the Monero RPC username.
    pub monero_rpc_username_file: PathBuf,
    /// Owner-only file containing the Monero RPC password.
    pub monero_rpc_password_file: PathBuf,
    /// Canonical Maker public role packet.
    pub maker_public_packet: PathBuf,
    /// Canonical Taker public role packet.
    pub taker_public_packet: PathBuf,
    /// New canonical unsigned Stage-A wire; never overwritten.
    pub output_unsigned_stage_a: PathBuf,
    /// Bounded public agreement values.
    pub parameters: M4StageAParameters,
}

/// Non-secret receipt for one locally composed unsigned Stage A.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct M4StageAComposeReceipt {
    agreement_commitment: [u8; 32],
    monero_genesis_hash: [u8; 32],
    lez_genesis_hash: [u8; 32],
    lez_channel_id: [u8; 32],
    lez_finalized_block_hash: [u8; 32],
    lez_finalized_height: u64,
    wire_bytes: usize,
}

impl M4StageAComposeReceipt {
    /// Exact commitment that both independent role actors must inspect and sign.
    #[must_use]
    pub const fn agreement_commitment(self) -> [u8; 32] {
        self.agreement_commitment
    }

    /// Actual local Monero height-zero identity bound into Stage A.
    #[must_use]
    pub const fn monero_genesis_hash(self) -> [u8; 32] {
        self.monero_genesis_hash
    }

    /// Stable official LEZ genesis identity bound into Stage A.
    #[must_use]
    pub const fn lez_genesis_hash(self) -> [u8; 32] {
        self.lez_genesis_hash
    }

    /// Actual official LEZ channel bound into Stage A.
    #[must_use]
    pub const fn lez_channel_id(self) -> [u8; 32] {
        self.lez_channel_id
    }

    /// Finalized block hash cross-checked between indexer and sequencer.
    #[must_use]
    pub const fn lez_finalized_block_hash(self) -> [u8; 32] {
        self.lez_finalized_block_hash
    }

    /// Finalized anchor height proven no later than both live sequencer tips.
    #[must_use]
    pub const fn lez_finalized_height(self) -> u64 {
        self.lez_finalized_height
    }

    /// Exact canonical unsigned wire length.
    #[must_use]
    pub const fn wire_bytes(self) -> usize {
        self.wire_bytes
    }
}

#[derive(Clone, Debug)]
struct LiveSnapshot {
    channel_id: [u8; 32],
    genesis_hash: [u8; 32],
    tip_hash: [u8; 32],
    tip_timestamp_ms: u64,
    tip_height: u64,
    metadata: Account,
    custody: Account,
    taker: Account,
    maker: Account,
    reobserved_prior_tip: Option<TipSnapshot>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TipSnapshot {
    hash: [u8; 32],
    timestamp_ms: u64,
    height: u64,
}

impl From<&OfficialNativeEscrowFacts> for LiveSnapshot {
    fn from(facts: &OfficialNativeEscrowFacts) -> Self {
        Self {
            channel_id: facts.channel_id(),
            genesis_hash: facts.genesis_block_hash(),
            tip_hash: facts.tip_block_hash(),
            tip_timestamp_ms: facts.tip_timestamp_ms(),
            tip_height: facts.sequencer_tip(),
            metadata: facts.metadata_account().clone(),
            custody: facts.custody_account().clone(),
            taker: facts.depositor_account().clone(),
            maker: facts.claimant_account().clone(),
            reobserved_prior_tip: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FinalizedSnapshot {
    genesis_hash: [u8; 32],
    block_hash: [u8; 32],
    sequencer_block_hash: [u8; 32],
    height: u64,
    timestamp_ms: u64,
    maker_nonce: u128,
    taker_nonce: u128,
    claim_nonce: u128,
    refund_nonce: u128,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LiveNonces {
    maker: u128,
    taker: u128,
    claim: u128,
    refund: u128,
}

/// Composes one canonical unsigned Stage A from actual local chain facts.
///
/// This function has only read clients. It cannot submit to either chain. It
/// discovers Monero height zero through the reviewed typed Digest-authenticated
/// adapter, brackets the official LEZ state around the finalized/indexed view,
/// plans exact future NSSA messages, validates the complete SDK body, and then
/// publishes one create-new canonical wire.
///
/// # Errors
///
/// Fails closed on unsafe files/routes, wrong actor roles or public view key,
/// stale/moving chain facts, pre-existing escrow state, insufficient balance,
/// invalid policy/body/wire, or an existing output path.
#[allow(
    clippy::too_many_lines,
    reason = "the actual-local read sequence remains linear so both chain brackets are auditable"
)]
pub async fn compose_m4_stage_a_actual_local(
    config: &ActualLocalM4StageAConfig,
) -> Result<M4StageAComposeReceipt> {
    validate_loopback_http_endpoint(&config.sequencer_url)
        .context("sequencer endpoint is not a literal-loopback HTTP root")?;
    validate_loopback_http_endpoint(&config.indexer_url)
        .context("indexer endpoint is not a literal-loopback HTTP root")?;
    ensure_output_absent(&config.output_unsigned_stage_a)?;
    validate_checked_m4_escrow_program_id(CHECKED_M4_ESCROW_PROGRAM_ID)
        .context("checked M4 escrow program ID is invalid")?;

    let maker = ValidatedRolePacket::read(&config.maker_public_packet)
        .context("Maker public role packet is invalid")?;
    let taker = ValidatedRolePacket::read(&config.taker_public_packet)
        .context("Taker public role packet is invalid")?;
    validate_packet_roles(&maker, &taker)?;

    let participants = XmrParticipantsV1::new(maker.identity().clone(), taker.identity().clone());
    let claim_key = participants
        .claim_aggregate_x_only_key()
        .context("claim aggregate key is invalid")?;
    let refund_key = participants
        .refund_aggregate_x_only_key()
        .context("refund aggregate key is invalid")?;
    let preliminary = plan_m4_stage_a_future_messages(M4StageAFutureMessageInput::new(
        CHECKED_M4_ESCROW_PROGRAM_ID,
        config.parameters.swap_id,
        AccountId::new(maker.identity().lez_owner_account()),
        AccountId::new(taker.identity().lez_owner_account()),
        claim_key,
        refund_key,
        crate::M4StageAFinalizedNonces::new(0, 0, 0, 0),
    ))
    .context("M4 authority derivation failed")?;

    let username = read_secret_credential(&config.monero_rpc_username_file, "username")?;
    let password = read_secret_credential(&config.monero_rpc_password_file, "password")?;
    let monero_endpoint = LoopbackRpcEndpoint::new(&config.monero_daemon_url, username, password)
        .context("Monero daemon endpoint or credential file is invalid")?;
    let monero_identity =
        MoneroChainIdentityAttestor::new(MoneroNetwork::Regtest, &monero_endpoint)
            .context("typed Monero height-zero attestor construction failed")?
            .discover()
            .await
            .context("actual local Monero height-zero discovery failed")?;

    let node = OfficialNodeRpc::connect_local(&config.sequencer_url)
        .context("official local sequencer connection failed")?;
    let indexer = OfficialIndexerRpc::connect_local(&config.indexer_url)
        .context("official local finalized-indexer connection failed")?;
    let maker_id = AccountId::new(maker.identity().lez_owner_account());
    let taker_id = AccountId::new(taker.identity().lez_owner_account());
    let metadata_id =
        compute_metadata_pda(&CHECKED_M4_ESCROW_PROGRAM_ID, &config.parameters.swap_id);
    let custody_id = compute_custody_pda(&CHECKED_M4_ESCROW_PROGRAM_ID, &config.parameters.swap_id);

    let first_facts = node
        .native_escrow_facts(metadata_id, custody_id, taker_id, maker_id)
        .await
        .context("first stable official LEZ snapshot failed")?;
    let first = LiveSnapshot::from(&first_facts);
    let finalized = read_stable_m4_finalized_nonce_snapshot(
        &indexer,
        Hex32::from_bytes(first.genesis_hash),
        M4FinalizedAccountIds::new(
            maker_id,
            taker_id,
            preliminary.claim_authority(),
            preliminary.refund_authority(),
        ),
    )
    .await
    .context("stable finalized four-account snapshot failed")?;
    let finalized_clock = finalized.finalized_clock();
    let sequencer_anchor = node
        .block_range(finalized_clock.height, finalized_clock.height)
        .await
        .context("official sequencer finalized-anchor read failed")?;
    ensure!(
        sequencer_anchor.len() == 1
            && sequencer_anchor[0].header.block_id == finalized_clock.height,
        "official sequencer did not return exactly the finalized anchor"
    );
    let sequencer_block_hash = sequencer_anchor[0].header.hash.0;
    let finalized = FinalizedSnapshot {
        genesis_hash: *finalized.genesis_block_hash().as_bytes(),
        block_hash: *finalized_clock.block_hash.as_bytes(),
        sequencer_block_hash,
        height: finalized_clock.height,
        timestamp_ms: finalized_clock.timestamp_ms,
        maker_nonce: finalized.maker_owner().nonce(),
        taker_nonce: finalized.taker_owner().nonce(),
        claim_nonce: finalized.claim_authority().nonce(),
        refund_nonce: finalized.refund_authority().nonce(),
    };

    let live_before = read_live_nonces(
        &node,
        maker_id,
        taker_id,
        preliminary.claim_authority(),
        preliminary.refund_authority(),
    )
    .await?;
    let second_facts = node
        .native_escrow_facts(metadata_id, custody_id, taker_id, maker_id)
        .await
        .context("second stable official LEZ snapshot failed")?;
    let mut second = LiveSnapshot::from(&second_facts);
    if second.tip_height > first.tip_height {
        let reobserved = node
            .block_range(first.tip_height, first.tip_height)
            .await
            .context("official sequencer prior-tip re-read failed")?;
        ensure!(
            reobserved.len() == 1 && reobserved[0].header.block_id == first.tip_height,
            "official sequencer did not return exactly the prior live tip"
        );
        second.reobserved_prior_tip = Some(TipSnapshot {
            hash: reobserved[0].header.hash.0,
            timestamp_ms: reobserved[0].header.timestamp,
            height: reobserved[0].header.block_id,
        });
    }
    let live_after = read_live_nonces(
        &node,
        maker_id,
        taker_id,
        preliminary.claim_authority(),
        preliminary.refund_authority(),
    )
    .await?;

    let validated = build_unsigned_stage_a(
        config.parameters,
        &maker,
        &taker,
        monero_identity,
        &first,
        finalized,
        live_before,
        &second,
        live_after,
    )?;
    let wire = validated
        .encode_unsigned_wire()
        .context("canonical unsigned Stage-A encoding failed")?;
    ensure!(
        wire.len() <= MAX_XMR_UNSIGNED_STAGE_A_WIRE_BYTES,
        "canonical unsigned Stage-A wire exceeds its SDK bound"
    );
    let reparsed = ValidatedXmrAgreementBodyV1::from_unsigned_wire(&wire)
        .context("canonical unsigned Stage-A self-check failed")?;
    ensure!(
        reparsed.encode_unsigned_wire()? == wire,
        "canonical unsigned Stage-A round trip changed bytes"
    );
    publish_new(&config.output_unsigned_stage_a, &wire)?;

    Ok(M4StageAComposeReceipt {
        agreement_commitment: validated.commitment(),
        monero_genesis_hash: monero_identity.genesis_hash(),
        lez_genesis_hash: first.genesis_hash,
        lez_channel_id: first.channel_id,
        lez_finalized_block_hash: finalized.block_hash,
        lez_finalized_height: finalized.height,
        wire_bytes: wire.len(),
    })
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn build_unsigned_stage_a(
    parameters: M4StageAParameters,
    maker: &ValidatedRolePacket,
    taker: &ValidatedRolePacket,
    monero_identity: MoneroChainIdentity,
    first: &LiveSnapshot,
    finalized: FinalizedSnapshot,
    live_before: LiveNonces,
    second: &LiveSnapshot,
    live_after: LiveNonces,
) -> Result<ValidatedXmrAgreementBodyV1> {
    validate_packet_roles(maker, taker)?;
    validate_live_progression(first, second)?;
    ensure!(
        first.genesis_hash == finalized.genesis_hash,
        "finalized indexer genesis differs from the actual sequencer"
    );
    ensure!(
        finalized.block_hash != [0; 32] && finalized.block_hash == finalized.sequencer_block_hash,
        "finalized indexer anchor hash differs from the official sequencer"
    );
    ensure!(
        finalized.height <= first.tip_height && finalized.height <= second.tip_height,
        "finalized indexer anchor is ahead of a bracketed live sequencer tip"
    );
    let expected_nonces = LiveNonces {
        maker: finalized.maker_nonce,
        taker: finalized.taker_nonce,
        claim: finalized.claim_nonce,
        refund: finalized.refund_nonce,
    };
    ensure!(
        live_before == expected_nonces && live_after == expected_nonces,
        "live LEZ nonce moved ahead of the stable finalized Stage-A snapshot"
    );
    ensure!(
        same_account(&first.metadata, &Account::default())
            && same_account(&first.custody, &Account::default()),
        "metadata or custody is not the exact absent/default account state required by initialization"
    );
    ensure!(
        first.taker.balance >= parameters.lez_amount,
        "Taker balance is below the exact LEZ principal"
    );
    ensure!(
        parameters.maker_xmr_funding_cutoff_ms > first.tip_timestamp_ms
            && parameters.maker_xmr_funding_cutoff_ms > finalized.timestamp_ms,
        "Maker XMR funding cutoff is not later than stable LEZ consensus time"
    );

    let participants = XmrParticipantsV1::new(maker.identity().clone(), taker.identity().clone());
    let claim_key = participants.claim_aggregate_x_only_key()?;
    let refund_key = participants.refund_aggregate_x_only_key()?;
    let maker_id = AccountId::new(maker.identity().lez_owner_account());
    let taker_id = AccountId::new(taker.identity().lez_owner_account());
    let future = plan_m4_stage_a_future_messages(M4StageAFutureMessageInput::new(
        CHECKED_M4_ESCROW_PROGRAM_ID,
        parameters.swap_id,
        maker_id,
        taker_id,
        claim_key,
        refund_key,
        crate::M4StageAFinalizedNonces::new(
            finalized.maker_nonce,
            finalized.taker_nonce,
            finalized.claim_nonce,
            finalized.refund_nonce,
        ),
    ))?;
    let public_view_key = maker.public_view_key();
    ensure!(
        taker.public_view_key() == public_view_key,
        "Maker and Taker packets do not bind one shared public view key"
    );
    let shared_address = MoneroSharedAddressV1::derive_from_public_view_key(
        MoneroAddressNetworkV1::Regtest,
        maker.proof(),
        taker.proof(),
        public_view_key,
    )?;
    let profile = XmrNamedProfileV1::AcceleratedRegtest;
    let metadata = compute_metadata_pda(&CHECKED_M4_ESCROW_PROGRAM_ID, &parameters.swap_id);
    let custody = compute_custody_pda(&CHECKED_M4_ESCROW_PROGRAM_ID, &parameters.swap_id);
    let transfer_program = programs::authenticated_transfer().id();
    let monero = XmrMoneroTermsV1::new(
        MoneroAddressNetworkV1::Regtest,
        monero_identity.genesis_hash(),
        parameters.monero_amount_piconero,
        profile.required_monero_confirmations(),
        maker.proof().to_wire_bytes()?,
        taker.proof().to_wire_bytes()?,
        shared_address.public_view_key(),
        shared_address.public_spend_key(),
        shared_address.address_string(),
    );
    let lez = XmrLezTermsV1::new(
        first.channel_id,
        first.genesis_hash,
        CHECKED_M4_ESCROW_PROGRAM_ID,
        transfer_program,
        profile.required_lez_finality_units(),
        metadata.into_value(),
        custody.into_value(),
        taker.identity().lez_owner_account(),
        maker.identity().lez_owner_account(),
        claim_key,
        future.claim_authority().into_value(),
        refund_key,
        future.refund_authority().into_value(),
        maker.proof().transcript_commitment(),
        taker.proof().transcript_commitment(),
        parameters.lez_amount,
    );
    let body = XmrAgreementBodyV1::new(
        XmrSwapDirectionV1::TakerSellsLez,
        profile,
        parameters.swap_id,
        participants,
        monero,
        lez,
        XmrMessagesV1::new(
            future.claim_hash(),
            future.refund_hash(),
            future.punish_hash(),
        ),
        XmrWindowsV1::new(
            parameters.maker_xmr_funding_cutoff_ms,
            parameters.refund_at_ms,
            parameters.punish_at_ms,
        ),
    );
    ValidatedXmrAgreementBodyV1::validate(body).context("complete unsigned Stage-A body is invalid")
}

fn validate_packet_roles(maker: &ValidatedRolePacket, taker: &ValidatedRolePacket) -> Result<()> {
    ensure!(
        maker.role() == ActorRole::Maker,
        "Maker packet has the wrong fixed role"
    );
    ensure!(
        taker.role() == ActorRole::Taker,
        "Taker packet has the wrong fixed role"
    );
    ensure!(
        maker.identity().lez_owner_account() != taker.identity().lez_owner_account(),
        "Maker and Taker owner accounts are aliased"
    );
    Ok(())
}

fn validate_live_progression(first: &LiveSnapshot, second: &LiveSnapshot) -> Result<()> {
    ensure!(
        first.channel_id == second.channel_id && first.genesis_hash == second.genesis_hash,
        "official LEZ chain identity changed while composing Stage A"
    );
    ensure!(
        same_account(&first.metadata, &second.metadata)
            && same_account(&first.custody, &second.custody)
            && same_account(&first.taker, &second.taker)
            && same_account(&first.maker, &second.maker),
        "relevant official LEZ account state changed while composing Stage A"
    );
    ensure!(
        second.tip_height >= first.tip_height,
        "official LEZ tip regressed while composing Stage A"
    );
    if second.tip_height == first.tip_height {
        ensure!(
            second.tip_hash == first.tip_hash && second.tip_timestamp_ms == first.tip_timestamp_ms,
            "same-height official LEZ tip identity changed while composing Stage A"
        );
        ensure!(
            second.reobserved_prior_tip.is_none(),
            "unexpected prior-tip proof for an unchanged official LEZ height"
        );
        return Ok(());
    }

    ensure!(
        second.tip_timestamp_ms >= first.tip_timestamp_ms,
        "advancing official LEZ tip timestamp regressed"
    );
    let prior = second.reobserved_prior_tip.ok_or_else(|| {
        anyhow::anyhow!("advancing official LEZ tip lacks an exact prior-tip re-read")
    })?;
    ensure!(
        prior.height == first.tip_height
            && prior.hash == first.tip_hash
            && prior.timestamp_ms == first.tip_timestamp_ms,
        "historical official LEZ prior-tip re-read differs from the first bracket"
    );
    Ok(())
}

fn same_account(left: &Account, right: &Account) -> bool {
    left.program_owner == right.program_owner
        && left.balance == right.balance
        && left.data == right.data
        && left.nonce == right.nonce
}

async fn read_live_nonces(
    node: &OfficialNodeRpc,
    maker: AccountId,
    taker: AccountId,
    claim: AccountId,
    refund: AccountId,
) -> Result<LiveNonces> {
    Ok(LiveNonces {
        maker: NonceSource::account_nonce(node, maker)
            .await
            .context("Maker live nonce read failed")?,
        taker: NonceSource::account_nonce(node, taker)
            .await
            .context("Taker live nonce read failed")?,
        claim: NonceSource::account_nonce(node, claim)
            .await
            .context("claim-authority live nonce read failed")?,
        refund: NonceSource::account_nonce(node, refund)
            .await
            .context("refund-authority live nonce read failed")?,
    })
}

fn read_secret_credential(path: &Path, label: &'static str) -> Result<String> {
    let owned = open(
        path,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .with_context(|| format!("open owner-only Monero RPC {label} file"))?;
    let mut file = File::from(owned);
    let metadata = file
        .metadata()
        .with_context(|| format!("inspect Monero RPC {label} file"))?;
    ensure!(
        metadata.is_file(),
        "Monero RPC {label} path is not a regular file"
    );
    ensure!(
        metadata.len() <= MAX_CREDENTIAL_FILE_BYTES,
        "Monero RPC {label} file is oversized"
    );
    ensure!(
        metadata.nlink() == 1,
        "Monero RPC {label} file has multiple links"
    );
    ensure!(
        metadata.uid() == rustix::process::geteuid().as_raw()
            && metadata.mode().trailing_zeros() >= 6,
        "Monero RPC {label} file is not owner-only"
    );
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_CREDENTIAL_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("read Monero RPC {label} file"))?;
    ensure!(
        bytes.len() as u64 <= MAX_CREDENTIAL_FILE_BYTES,
        "Monero RPC {label} file is oversized"
    );
    let after = file
        .metadata()
        .with_context(|| format!("reinspect Monero RPC {label} file"))?;
    ensure!(
        metadata.dev() == after.dev()
            && metadata.ino() == after.ino()
            && metadata.len() == after.len()
            && metadata.mode() == after.mode()
            && metadata.uid() == after.uid()
            && metadata.nlink() == after.nlink(),
        "Monero RPC {label} file changed while it was read"
    );
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
        if bytes.last() == Some(&b'\r') {
            bytes.pop();
        }
    }
    String::from_utf8(bytes).with_context(|| format!("Monero RPC {label} is not UTF-8"))
}

fn ensure_output_absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => anyhow::bail!("unsigned Stage-A output already exists"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("inspect unsigned Stage-A output"),
    }
}

fn publish_new(path: &Path, wire: &[u8]) -> Result<()> {
    ensure_output_absent(path)?;
    let parent = path
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut staged =
        NamedTempFile::new_in(parent).context("create staged unsigned Stage-A output")?;
    staged
        .write_all(wire)
        .context("write staged unsigned Stage-A output")?;
    staged
        .as_file()
        .sync_all()
        .context("sync staged unsigned Stage-A output")?;
    let published = staged
        .persist_noclobber(path)
        .map_err(|error| error.error)
        .context("publish create-new unsigned Stage-A output")?;
    published
        .sync_all()
        .context("sync published unsigned Stage-A output")?;
    File::open(parent)
        .context("open unsigned Stage-A output parent")?
        .sync_all()
        .context("sync unsigned Stage-A output parent")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;
    use tempfile::TempDir;
    use xmr_reference_actor::{Action, Cli, execute};

    struct Packets {
        _directory: TempDir,
        maker: ValidatedRolePacket,
        taker: ValidatedRolePacket,
    }

    fn packets() -> Packets {
        let directory = TempDir::new().expect("fixture directory");
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
            .expect("owner-only fixture directory");
        let taker_root = directory.path().join("taker-private");
        let taker_packet = directory.path().join("taker.json");
        execute(Cli {
            action: Action::Provision {
                role: ActorRole::Taker,
                private_root: taker_root.clone(),
                lez_owner_account: hex::encode([2; 32]),
                shared_view_key_file: None,
                public_packet: taker_packet.clone(),
            },
        })
        .expect("provision Taker fixture");
        let maker_packet = directory.path().join("maker.json");
        execute(Cli {
            action: Action::Provision {
                role: ActorRole::Maker,
                private_root: directory.path().join("maker-private"),
                lez_owner_account: hex::encode([1; 32]),
                shared_view_key_file: Some(taker_root.join("monero-view.key")),
                public_packet: maker_packet.clone(),
            },
        })
        .expect("provision Maker fixture");
        Packets {
            maker: ValidatedRolePacket::read(&maker_packet).expect("Maker packet"),
            taker: ValidatedRolePacket::read(&taker_packet).expect("Taker packet"),
            _directory: directory,
        }
    }

    fn parameters() -> M4StageAParameters {
        M4StageAParameters::new([9; 32], 5_000_000_000, 75, 5_000, 15_000, 25_000)
    }

    fn live() -> LiveSnapshot {
        LiveSnapshot {
            channel_id: [40; 32],
            genesis_hash: [41; 32],
            tip_hash: [42; 32],
            tip_timestamp_ms: 1_000,
            tip_height: 3,
            metadata: Account::default(),
            custody: Account::default(),
            taker: Account {
                balance: 100,
                nonce: 7_u128.into(),
                ..Account::default()
            },
            maker: Account {
                nonce: 11_u128.into(),
                ..Account::default()
            },
            reobserved_prior_tip: None,
        }
    }

    const fn finalized() -> FinalizedSnapshot {
        FinalizedSnapshot {
            genesis_hash: [41; 32],
            block_hash: [44; 32],
            sequencer_block_hash: [44; 32],
            height: 3,
            timestamp_ms: 1_000,
            maker_nonce: 11,
            taker_nonce: 7,
            claim_nonce: 0,
            refund_nonce: 0,
        }
    }

    const fn nonces() -> LiveNonces {
        LiveNonces {
            maker: 11,
            taker: 7,
            claim: 0,
            refund: 0,
        }
    }

    #[test]
    fn actual_role_packets_and_official_facts_compose_canonical_unsigned_stage_a() {
        let packets = packets();
        let first = live();
        let validated = build_unsigned_stage_a(
            parameters(),
            &packets.maker,
            &packets.taker,
            MoneroChainIdentity::new(MoneroNetwork::Regtest, [50; 32]).expect("Monero identity"),
            &first,
            finalized(),
            nonces(),
            &first,
            nonces(),
        )
        .expect("canonical unsigned Stage A");
        let wire = validated.encode_unsigned_wire().expect("canonical wire");
        let decoded = ValidatedXmrAgreementBodyV1::from_unsigned_wire(&wire).expect("round trip");
        assert_eq!(decoded.commitment(), validated.commitment());
        assert_eq!(decoded.body().monero().genesis_hash(), [50; 32]);
        assert_eq!(decoded.body().lez().channel_id(), [40; 32]);
        assert_eq!(decoded.body().lez().genesis_hash(), [41; 32]);
    }

    #[test]
    fn stale_live_nonce_fails_closed() {
        let packets = packets();
        let first = live();
        let mut stale = nonces();
        stale.taker += 1;
        let error = build_unsigned_stage_a(
            parameters(),
            &packets.maker,
            &packets.taker,
            MoneroChainIdentity::new(MoneroNetwork::Regtest, [50; 32]).unwrap(),
            &first,
            finalized(),
            stale,
            &first,
            stale,
        )
        .expect_err("stale nonce must fail");
        assert!(error.to_string().contains("nonce moved ahead"));
    }

    #[test]
    fn moving_live_snapshot_fails_closed() {
        let packets = packets();
        let first = live();
        let mut moved = first.clone();
        moved.tip_hash[0] ^= 1;
        let error = build_unsigned_stage_a(
            parameters(),
            &packets.maker,
            &packets.taker,
            MoneroChainIdentity::new(MoneroNetwork::Regtest, [50; 32]).unwrap(),
            &first,
            finalized(),
            nonces(),
            &moved,
            nonces(),
        )
        .expect_err("moving snapshot must fail");
        assert!(error.to_string().contains("same-height"));
    }

    #[test]
    fn unrelated_monotonic_tip_advance_with_exact_prior_reread_succeeds() {
        let packets = packets();
        let first = live();
        let mut advanced = first.clone();
        advanced.tip_height += 1;
        advanced.tip_hash = [43; 32];
        advanced.tip_timestamp_ms += 1_000;
        advanced.reobserved_prior_tip = Some(TipSnapshot {
            hash: first.tip_hash,
            timestamp_ms: first.tip_timestamp_ms,
            height: first.tip_height,
        });
        drop(
            build_unsigned_stage_a(
                parameters(),
                &packets.maker,
                &packets.taker,
                MoneroChainIdentity::new(MoneroNetwork::Regtest, [50; 32]).unwrap(),
                &first,
                finalized(),
                nonces(),
                &advanced,
                nonces(),
            )
            .expect("unrelated monotonic chain advance remains reproducible"),
        );
    }

    #[test]
    fn regressing_tip_or_wrong_prior_reread_fails_closed() {
        let packets = packets();
        let first = live();
        let mut regressed = first.clone();
        regressed.tip_height -= 1;
        let regression = build_unsigned_stage_a(
            parameters(),
            &packets.maker,
            &packets.taker,
            MoneroChainIdentity::new(MoneroNetwork::Regtest, [50; 32]).unwrap(),
            &first,
            finalized(),
            nonces(),
            &regressed,
            nonces(),
        )
        .expect_err("tip regression must fail");
        assert!(regression.to_string().contains("regressed"));

        let mut wrong_prior = first.clone();
        wrong_prior.tip_height += 1;
        wrong_prior.tip_hash = [43; 32];
        wrong_prior.tip_timestamp_ms += 1_000;
        wrong_prior.reobserved_prior_tip = Some(TipSnapshot {
            hash: [99; 32],
            timestamp_ms: first.tip_timestamp_ms,
            height: first.tip_height,
        });
        let mismatch = build_unsigned_stage_a(
            parameters(),
            &packets.maker,
            &packets.taker,
            MoneroChainIdentity::new(MoneroNetwork::Regtest, [50; 32]).unwrap(),
            &first,
            finalized(),
            nonces(),
            &wrong_prior,
            nonces(),
        )
        .expect_err("wrong prior-tip proof must fail");
        assert!(mismatch.to_string().contains("prior-tip re-read differs"));
    }

    #[test]
    fn wrong_role_identity_fails_closed() {
        let packets = packets();
        let first = live();
        let error = build_unsigned_stage_a(
            parameters(),
            &packets.taker,
            &packets.maker,
            MoneroChainIdentity::new(MoneroNetwork::Regtest, [50; 32]).unwrap(),
            &first,
            finalized(),
            nonces(),
            &first,
            nonces(),
        )
        .expect_err("crossed roles must fail");
        assert!(error.to_string().contains("wrong fixed role"));
    }

    #[test]
    fn create_new_output_never_clobbers_existing_bytes() {
        let directory = TempDir::new().expect("fixture directory");
        let output = directory.path().join("stage-a.bin");
        fs::write(&output, b"existing").expect("existing output");
        let error = publish_new(&output, b"replacement").expect_err("must not clobber");
        assert!(error.to_string().contains("already exists"));
        assert_eq!(fs::read(output).unwrap(), b"existing");
    }

    #[test]
    fn checked_program_deployment_remains_an_explicit_effect_gate() {
        assert!(M4_STAGE_A_DEPLOYMENT_RESIDUAL.contains("does not prove"));
        assert!(M4_STAGE_A_DEPLOYMENT_RESIDUAL.contains("tag-13"));
        assert!(M4_STAGE_A_DEPLOYMENT_RESIDUAL.contains("before submission"));
    }

    #[test]
    fn nondefault_escrow_owner_or_nonce_fails_before_composition() {
        let packets = packets();
        let first = live();
        let mut owned = first.clone();
        owned.metadata.program_owner = programs::authenticated_transfer().id();
        let owned_error = build_unsigned_stage_a(
            parameters(),
            &packets.maker,
            &packets.taker,
            MoneroChainIdentity::new(MoneroNetwork::Regtest, [50; 32]).unwrap(),
            &owned,
            finalized(),
            nonces(),
            &owned,
            nonces(),
        )
        .expect_err("owned metadata cannot be initialized");
        assert!(owned_error.to_string().contains("exact absent/default"));

        let mut consumed = first;
        consumed.custody.nonce = 1_u128.into();
        let nonce_error = build_unsigned_stage_a(
            parameters(),
            &packets.maker,
            &packets.taker,
            MoneroChainIdentity::new(MoneroNetwork::Regtest, [50; 32]).unwrap(),
            &consumed,
            finalized(),
            nonces(),
            &consumed,
            nonces(),
        )
        .expect_err("nonzero custody nonce cannot be initialized");
        assert!(nonce_error.to_string().contains("exact absent/default"));
    }

    #[test]
    fn finalized_anchor_mismatch_or_future_height_fails_closed() {
        let packets = packets();
        let first = live();
        for changed in [
            FinalizedSnapshot {
                sequencer_block_hash: [99; 32],
                ..finalized()
            },
            FinalizedSnapshot {
                height: 4,
                ..finalized()
            },
        ] {
            assert!(
                build_unsigned_stage_a(
                    parameters(),
                    &packets.maker,
                    &packets.taker,
                    MoneroChainIdentity::new(MoneroNetwork::Regtest, [50; 32]).unwrap(),
                    &first,
                    changed,
                    nonces(),
                    &first,
                    nonces(),
                )
                .is_err()
            );
        }
    }
}
