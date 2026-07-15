#![forbid(unsafe_code)]

use std::{
    fs,
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    str::FromStr as _,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context as _, Result, bail, ensure};
use borsh::BorshDeserialize as _;
use clap::{Args as ClapArgs, Parser, Subcommand, ValueEnum};
use lez_bridge_protocol::{
    EscrowState, Hex32, MessageContext, NativeEscrowTerms, NativeEscrowTermsInput, Participant,
    PrepareNativeEscrowRequest, PrepareRevealingClaimRequest, PreparedTransaction,
    RevealingPreimage, RunId, RuntimeCompatibility, RuntimeDescriptor, TransactionId,
};
use lez_v0_2_sidecar::{
    NativeEscrowPlanner, OfficialNativeEscrowFacts, OfficialNodeRpc, RuntimeBoundary,
    RuntimeBoundaryError, compute_custody_pda, compute_metadata_pda, program_id_from_hex,
    program_id_to_hex,
};
use lez_zec_escrow_v02::{ClaimAuthority, EscrowMetadata, EscrowStatus};
use nssa::{Account, AccountId, PrivateKey, PublicKey};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use zeroize::{Zeroize as _, Zeroizing};

const EVIDENCE_SCHEMA: &str = "lez_v02_native_escrow_poc_v1";
const DEFAULT_INCLUSION_POLLS: u32 = 120;
const INCLUSION_POLL_INTERVAL: Duration = Duration::from_millis(100);
const SNAPSHOT_ATTEMPTS: u8 = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum RoleArgument {
    Maker,
    Taker,
}

impl From<RoleArgument> for Participant {
    fn from(value: RoleArgument) -> Self {
        match value {
            RoleArgument::Maker => Self::Maker,
            RoleArgument::Taker => Self::Taker,
        }
    }
}

/// Exercise the checked native escrow against an actual local LEZ v0.2 node.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Arguments {
    #[command(subcommand)]
    action: Action,
}

#[derive(Debug, Subcommand)]
enum Action {
    /// As the signed depositor, initialize, observe, then fund the escrow.
    Deposit(DepositArguments),
    /// As the signed claimant, reveal the preimage and claim funded custody.
    Claim(ClaimArguments),
    /// Read and validate the current escrow accounts without an actor key.
    Observe(ObserveArguments),
}

#[derive(Debug, ClapArgs)]
struct NodeArguments {
    /// Explicit literal-loopback official sequencer HTTP URL and port.
    #[arg(long)]
    sequencer_url: String,
    /// Configured local-devnet chain identity as 64 lowercase hex characters.
    #[arg(long)]
    chain_id: String,
    /// Already-deployed checked escrow program ID as 64 lowercase hex characters.
    #[arg(long)]
    escrow_program_id: String,
}

#[derive(Debug, ClapArgs)]
struct TermsArguments {
    #[arg(long)]
    swap_id: String,
    #[arg(long)]
    terms_hash: String,
    #[arg(long)]
    secret_digest: String,
    #[arg(long, value_enum)]
    depositor_role: RoleArgument,
    #[arg(long)]
    depositor_account_id: String,
    #[arg(long, value_enum)]
    claimant_role: RoleArgument,
    #[arg(long)]
    claimant_account_id: String,
    #[arg(long)]
    amount: u128,
    #[arg(long)]
    refund_at_ms: u64,
}

#[derive(Debug, ClapArgs)]
struct ActorArguments {
    /// Isolated swap actor whose key and state this process owns.
    #[arg(long, value_enum)]
    role: RoleArgument,
    #[arg(long)]
    run_id: String,
    #[arg(long)]
    request_id: String,
    /// Existing owner-only 0700 directory dedicated to this actor.
    #[arg(long)]
    state_directory: PathBuf,
    /// Owner-only file containing one lowercase-hex 32-byte LEZ private key.
    #[arg(long)]
    private_key_file: PathBuf,
    #[command(flatten)]
    node: NodeArguments,
    #[command(flatten)]
    terms: TermsArguments,
    /// Maximum canonical-inclusion polls after each submission.
    #[arg(long, default_value_t = DEFAULT_INCLUSION_POLLS)]
    max_inclusion_polls: u32,
}

#[derive(Debug, ClapArgs)]
struct DepositArguments {
    #[command(flatten)]
    actor: ActorArguments,
}

#[derive(Debug, ClapArgs)]
struct ClaimArguments {
    #[command(flatten)]
    actor: ActorArguments,
    /// Exact funding transaction emitted by the depositor process.
    #[arg(long)]
    funding_transaction_id: String,
    /// Owner-only file containing one lowercase-hex 32-byte preimage.
    #[arg(long)]
    preimage_file: PathBuf,
}

#[derive(Debug, ClapArgs)]
struct ObserveArguments {
    #[command(flatten)]
    node: NodeArguments,
    #[command(flatten)]
    terms: TermsArguments,
}

struct ParsedTerms {
    terms: NativeEscrowTerms,
    escrow_program_hex: Hex32,
    escrow_program_id: [u32; 8],
    authenticated_transfer_program_id: [u32; 8],
    depositor_account_id: AccountId,
    claimant_account_id: AccountId,
    metadata_account_id: AccountId,
    custody_account_id: AccountId,
}

struct ActorContext {
    role: Participant,
    run_id: RunId,
    request_id: lez_bridge_protocol::RequestId,
    signer_key: PrivateKey,
    signer_account_id: AccountId,
    chain_id: Hex32,
    parsed: ParsedTerms,
    node: Arc<OfficialNodeRpc>,
    initial_facts: OfficialNativeEscrowFacts,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct RuntimeEvidence {
    compatibility: RuntimeCompatibility,
    chain_id: Hex32,
    chain_id_source: &'static str,
    channel_id: Hex32,
    channel_id_source: &'static str,
    genesis_block_hash: Hex32,
    genesis_block_hash_source: &'static str,
    escrow_program_id: Hex32,
    escrow_program_id_source: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct AccountEvidence {
    account_id: Hex32,
    program_owner: Hex32,
    balance: u128,
    nonce: u128,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct SnapshotEvidence {
    sequencer_tip: u64,
    tip_block_hash: Hex32,
    tip_timestamp_ms: u64,
    escrow_state: Option<EscrowState>,
    metadata: AccountEvidence,
    custody: AccountEvidence,
    depositor: AccountEvidence,
    claimant: AccountEvidence,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct TransactionEvidence {
    kind: &'static str,
    transaction_id: TransactionId,
    already_included_before_call: bool,
    observation: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
struct LifecycleEvidence {
    schema: &'static str,
    action: &'static str,
    role: Option<Participant>,
    run_id: Option<RunId>,
    request_id: Option<lez_bridge_protocol::RequestId>,
    runtime: RuntimeEvidence,
    swap_id: Hex32,
    amount: u128,
    transactions: Vec<TransactionEvidence>,
    before: SnapshotEvidence,
    after_initialization: Option<SnapshotEvidence>,
    after: SnapshotEvidence,
    observation_scope: &'static str,
    finality: &'static str,
    crash_atomic_submission: bool,
}

#[tokio::main]
async fn main() {
    match execute(Arguments::parse()).await {
        Ok(evidence) => match serde_json::to_string(&evidence) {
            Ok(json) => println!("{json}"),
            Err(error) => {
                eprintln!("native escrow PoC evidence encoding failed: {error}");
                std::process::exit(1);
            }
        },
        Err(error) => {
            eprintln!("native escrow PoC failed: {error:#}");
            std::process::exit(1);
        }
    }
}

async fn execute(arguments: Arguments) -> Result<LifecycleEvidence> {
    match arguments.action {
        Action::Deposit(arguments) => execute_deposit(arguments).await,
        Action::Claim(arguments) => execute_claim(arguments).await,
        Action::Observe(arguments) => execute_observe(arguments).await,
    }
}

async fn execute_deposit(arguments: DepositArguments) -> Result<LifecycleEvidence> {
    let actor = prepare_actor(&arguments.actor).await?;
    ensure!(
        actor.parsed.terms.depositor() == actor.role
            && actor.parsed.depositor_account_id == actor.signer_account_id,
        "deposit command must run as the signed terms depositor"
    );
    ensure!(
        escrow_state(&actor.initial_facts)?.is_none(),
        "deposit requires a previously uninitialized swap ID"
    );
    ensure!(
        actor.parsed.terms.refund_at_ms() > actor.initial_facts.tip_timestamp_ms(),
        "deposit refund boundary must be later than the consensus-visible tip clock"
    );
    let descriptor = descriptor(&actor);
    RuntimeBoundary::new(
        descriptor.clone(),
        actor.role,
        actor.signer_account_id,
        actor.node.clone(),
    )?
    .verify_health()
    .await?;
    let runtime = runtime_evidence(&actor, &actor.initial_facts);
    let planner = NativeEscrowPlanner::new_durable(
        actor.role,
        actor.signer_key,
        actor.parsed.escrow_program_id,
        actor.parsed.authenticated_transfer_program_id,
        descriptor.clone(),
        actor.node.clone(),
        &arguments.actor.state_directory,
    )?;
    let request = PrepareNativeEscrowRequest::new(
        MessageContext::new(actor.run_id.clone(), actor.request_id.clone(), actor.role),
        descriptor,
        actor.parsed.terms.clone(),
    );
    let prepared = planner.prepare(request).await?;
    let initialization = submit_or_observe(
        actor.node.as_ref(),
        "initialize_native",
        &prepared.initialization,
        arguments.actor.max_inclusion_polls,
    )
    .await?;
    let initialized = read_stable_facts(actor.node.as_ref(), &actor.parsed).await?;
    validate_snapshot(&actor.parsed, &initialized, Some(EscrowState::Empty), 0)?;

    let funding = submit_or_observe(
        actor.node.as_ref(),
        "fund_native",
        &prepared.funding,
        arguments.actor.max_inclusion_polls,
    )
    .await?;
    let funded = read_stable_facts(actor.node.as_ref(), &actor.parsed).await?;
    validate_snapshot(
        &actor.parsed,
        &funded,
        Some(EscrowState::Funded),
        actor.parsed.terms.amount().as_u128(),
    )?;
    ensure!(
        funded.depositor_account().balance
            == actor
                .initial_facts
                .depositor_account()
                .balance
                .checked_sub(actor.parsed.terms.amount().as_u128())
                .context("depositor did not have enough native funds")?,
        "funding must debit exactly the signed amount from the depositor"
    );

    Ok(LifecycleEvidence {
        schema: EVIDENCE_SCHEMA,
        action: "deposit",
        role: Some(actor.role),
        run_id: Some(actor.run_id.clone()),
        request_id: Some(actor.request_id.clone()),
        runtime,
        swap_id: actor.parsed.terms.swap_id(),
        amount: actor.parsed.terms.amount().as_u128(),
        transactions: vec![initialization, funding],
        before: snapshot_evidence(&actor.parsed, &actor.initial_facts)?,
        after_initialization: Some(snapshot_evidence(&actor.parsed, &initialized)?),
        after: snapshot_evidence(&actor.parsed, &funded)?,
        observation_scope: "canonical_sequencer_inclusion_and_same_tip_accounts",
        finality: "not_observed_in_this_poc_slice",
        crash_atomic_submission: false,
    })
}

async fn execute_claim(arguments: ClaimArguments) -> Result<LifecycleEvidence> {
    let actor = prepare_actor(&arguments.actor).await?;
    ensure!(
        actor.parsed.terms.claimant() == actor.role
            && actor.parsed.claimant_account_id == actor.signer_account_id,
        "claim command must run as the signed terms claimant"
    );
    validate_snapshot(
        &actor.parsed,
        &actor.initial_facts,
        Some(EscrowState::Funded),
        actor.parsed.terms.amount().as_u128(),
    )?;
    let funding_transaction_id = TransactionId::from_bytes(
        *Hex32::from_hex(&arguments.funding_transaction_id)
            .context("invalid funding transaction ID")?
            .as_bytes(),
    );
    let preimage = read_secret32(&arguments.preimage_file, "preimage")?;
    let observed_digest: [u8; 32] = Sha256::digest(preimage.as_slice()).into();
    ensure!(
        observed_digest == *actor.parsed.terms.secret_digest().as_bytes(),
        "preimage does not match the signed secret digest"
    );
    let descriptor = descriptor(&actor);
    RuntimeBoundary::new(
        descriptor.clone(),
        actor.role,
        actor.signer_account_id,
        actor.node.clone(),
    )?
    .verify_health()
    .await?;
    let runtime = runtime_evidence(&actor, &actor.initial_facts);
    let planner = NativeEscrowPlanner::new_durable(
        actor.role,
        actor.signer_key,
        actor.parsed.escrow_program_id,
        actor.parsed.authenticated_transfer_program_id,
        descriptor.clone(),
        actor.node.clone(),
        &arguments.actor.state_directory,
    )?;
    let request = PrepareRevealingClaimRequest::new(
        MessageContext::new(actor.run_id.clone(), actor.request_id.clone(), actor.role),
        descriptor,
        actor.parsed.terms.clone(),
        funding_transaction_id,
        RevealingPreimage::new(*preimage),
    );
    let prepared = planner.prepare_revealing_claim(&request).await?;
    let claim = submit_or_observe(
        actor.node.as_ref(),
        "claim_native",
        &prepared.claim,
        arguments.actor.max_inclusion_polls,
    )
    .await?;
    let claimed = read_stable_facts(actor.node.as_ref(), &actor.parsed).await?;
    validate_snapshot(&actor.parsed, &claimed, Some(EscrowState::Claimed), 0)?;
    ensure!(
        claimed.claimant_account().balance
            == actor
                .initial_facts
                .claimant_account()
                .balance
                .checked_add(actor.parsed.terms.amount().as_u128())
                .context("claimant balance overflow")?,
        "claim must credit exactly the signed amount to the claimant"
    );

    Ok(LifecycleEvidence {
        schema: EVIDENCE_SCHEMA,
        action: "claim",
        role: Some(actor.role),
        run_id: Some(actor.run_id.clone()),
        request_id: Some(actor.request_id.clone()),
        runtime,
        swap_id: actor.parsed.terms.swap_id(),
        amount: actor.parsed.terms.amount().as_u128(),
        transactions: vec![claim],
        before: snapshot_evidence(&actor.parsed, &actor.initial_facts)?,
        after_initialization: None,
        after: snapshot_evidence(&actor.parsed, &claimed)?,
        observation_scope: "canonical_sequencer_inclusion_and_same_tip_accounts",
        finality: "not_observed_in_this_poc_slice",
        crash_atomic_submission: false,
    })
}

async fn execute_observe(arguments: ObserveArguments) -> Result<LifecycleEvidence> {
    let parsed = parse_terms(&arguments.node, &arguments.terms)?;
    let chain_id = parse_nonzero_hex(&arguments.node.chain_id, "chain ID")?;
    let node = OfficialNodeRpc::connect(&arguments.node.sequencer_url)?;
    let facts = read_stable_facts(&node, &parsed).await?;
    ensure!(
        chain_id.as_bytes() == &facts.channel_id(),
        "local PoC chain ID must equal the observed runtime channel"
    );
    let state = escrow_state(&facts)?;
    if let Some(state) = state {
        let custody_balance = match state {
            EscrowState::Funded => parsed.terms.amount().as_u128(),
            EscrowState::Empty | EscrowState::Claimed | EscrowState::Refunded => 0,
        };
        validate_snapshot(&parsed, &facts, Some(state), custody_balance)?;
    }
    let runtime = RuntimeEvidence {
        compatibility: RuntimeCompatibility::LeeV0_2_0,
        chain_id,
        chain_id_source: "explicit_configuration_no_v02_rpc",
        channel_id: Hex32::from_bytes(facts.channel_id()),
        channel_id_source: "sequencer_getChannelId",
        genesis_block_hash: Hex32::from_bytes(facts.genesis_block_hash()),
        genesis_block_hash_source: "sequencer_getBlock_genesis",
        escrow_program_id: parsed.escrow_program_hex,
        escrow_program_id_source: "explicit_checked_deployment",
    };
    let snapshot = snapshot_evidence(&parsed, &facts)?;
    Ok(LifecycleEvidence {
        schema: EVIDENCE_SCHEMA,
        action: "observe",
        role: None,
        run_id: None,
        request_id: None,
        runtime,
        swap_id: parsed.terms.swap_id(),
        amount: parsed.terms.amount().as_u128(),
        transactions: Vec::new(),
        before: snapshot,
        after_initialization: None,
        after: snapshot_evidence(&parsed, &facts)?,
        observation_scope: "same_tip_canonical_sequencer_accounts",
        finality: "not_observed_in_this_poc_slice",
        crash_atomic_submission: false,
    })
}

async fn prepare_actor(arguments: &ActorArguments) -> Result<ActorContext> {
    validate_state_directory(&arguments.state_directory)?;
    ensure!(
        arguments.max_inclusion_polls > 0,
        "inclusion poll bound must be nonzero"
    );
    let signer_key = read_private_key(&arguments.private_key_file)?;
    let signer_account_id = AccountId::from(&PublicKey::new_from_private_key(&signer_key));
    let role = Participant::from(arguments.role);
    let run_id = RunId::new(arguments.run_id.clone()).context("invalid run ID")?;
    let request_id = lez_bridge_protocol::RequestId::new(arguments.request_id.clone())
        .context("invalid request ID")?;
    let chain_id = parse_nonzero_hex(&arguments.node.chain_id, "chain ID")?;
    let parsed = parse_terms(&arguments.node, &arguments.terms)?;
    let node = Arc::new(OfficialNodeRpc::connect(&arguments.node.sequencer_url)?);
    let initial_facts = read_stable_facts(node.as_ref(), &parsed).await?;
    ensure!(
        chain_id.as_bytes() == &initial_facts.channel_id(),
        "local PoC chain ID must equal the observed runtime channel"
    );
    Ok(ActorContext {
        role,
        run_id,
        request_id,
        signer_key,
        signer_account_id,
        chain_id,
        parsed,
        node,
        initial_facts,
    })
}

fn parse_terms(node: &NodeArguments, arguments: &TermsArguments) -> Result<ParsedTerms> {
    let escrow_program_hex = parse_nonzero_hex(&node.escrow_program_id, "escrow program ID")?;
    let escrow_program_id = program_id_from_hex(escrow_program_hex);
    let authenticated_transfer_program_id = programs::authenticated_transfer().id();
    let depositor_account_hex =
        parse_nonzero_hex(&arguments.depositor_account_id, "depositor account ID")?;
    let claimant_account_hex =
        parse_nonzero_hex(&arguments.claimant_account_id, "claimant account ID")?;
    let depositor_account_id = AccountId::new(*depositor_account_hex.as_bytes());
    let claimant_account_id = AccountId::new(*claimant_account_hex.as_bytes());
    let terms_hash = Hex32::from_hex(&arguments.terms_hash).context("invalid terms hash")?;
    ensure!(
        terms_hash.as_bytes() != &[0; 32],
        "terms hash must be nonzero"
    );
    let terms = NativeEscrowTerms::new(NativeEscrowTermsInput {
        swap_id: Hex32::from_hex(&arguments.swap_id).context("invalid swap ID")?,
        terms_hash,
        secret_digest: Hex32::from_hex(&arguments.secret_digest)
            .context("invalid secret digest")?,
        depositor: arguments.depositor_role.into(),
        depositor_account_id: depositor_account_hex,
        claimant: arguments.claimant_role.into(),
        claimant_account_id: claimant_account_hex,
        amount: arguments.amount,
        refund_at_ms: arguments.refund_at_ms,
        authenticated_transfer_program_id: program_id_to_hex(authenticated_transfer_program_id),
    })?;
    let metadata_account_id = compute_metadata_pda(&escrow_program_id, terms.swap_id().as_bytes());
    let custody_account_id = compute_custody_pda(&escrow_program_id, terms.swap_id().as_bytes());
    Ok(ParsedTerms {
        terms,
        escrow_program_hex,
        escrow_program_id,
        authenticated_transfer_program_id,
        depositor_account_id,
        claimant_account_id,
        metadata_account_id,
        custody_account_id,
    })
}

fn descriptor(actor: &ActorContext) -> RuntimeDescriptor {
    RuntimeDescriptor::new(
        actor.role,
        RuntimeCompatibility::LeeV0_2_0,
        actor.chain_id,
        Hex32::from_bytes(actor.initial_facts.channel_id()),
        Hex32::from_bytes(actor.initial_facts.genesis_block_hash()),
        actor.parsed.escrow_program_hex,
        Hex32::from_bytes(actor.signer_account_id.into_value()),
    )
}

async fn read_stable_facts(
    node: &OfficialNodeRpc,
    parsed: &ParsedTerms,
) -> Result<OfficialNativeEscrowFacts> {
    for attempt in 0..SNAPSHOT_ATTEMPTS {
        match node
            .native_escrow_facts(
                parsed.metadata_account_id,
                parsed.custody_account_id,
                parsed.depositor_account_id,
                parsed.claimant_account_id,
            )
            .await
        {
            Ok(facts) => return Ok(facts),
            Err(RuntimeBoundaryError::InconsistentSnapshot) if attempt + 1 < SNAPSHOT_ATTEMPTS => {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            Err(error) => return Err(error.into()),
        }
    }
    bail!("same-tip native escrow snapshot retry bound exhausted")
}

async fn submit_or_observe(
    node: &OfficialNodeRpc,
    kind: &'static str,
    prepared: &PreparedTransaction,
    max_polls: u32,
) -> Result<TransactionEvidence> {
    let already_included = node.prepared_transaction_is_included(prepared).await?;
    if !already_included {
        node.submit_prepared_transaction(prepared).await?;
    }
    for _ in 0..max_polls {
        if node.prepared_transaction_is_included(prepared).await? {
            return Ok(TransactionEvidence {
                kind,
                transaction_id: prepared.transaction_id,
                already_included_before_call: already_included,
                observation: "canonical_sequencer_inclusion",
            });
        }
        tokio::time::sleep(INCLUSION_POLL_INTERVAL).await;
    }
    bail!("{kind} was not canonically observed before the bounded inclusion timeout")
}

fn validate_snapshot(
    parsed: &ParsedTerms,
    facts: &OfficialNativeEscrowFacts,
    expected_state: Option<EscrowState>,
    expected_custody_balance: u128,
) -> Result<()> {
    ensure!(
        escrow_state(facts)? == expected_state,
        "escrow metadata state differs from the required lifecycle state"
    );
    let Some(metadata) = decode_metadata(facts)? else {
        ensure!(
            expected_state.is_none(),
            "required escrow metadata is absent"
        );
        return Ok(());
    };
    let secret_matches = matches!(
        metadata.claim_authority,
        ClaimAuthority::Sha256Preimage { secret_digest }
            if secret_digest == *parsed.terms.secret_digest().as_bytes()
    );
    ensure!(
        facts.metadata_account().program_owner == parsed.escrow_program_id
            && metadata.version == 2
            && metadata.swap_id == *parsed.terms.swap_id().as_bytes()
            && metadata.terms_hash == *parsed.terms.terms_hash().as_bytes()
            && secret_matches
            && metadata.depositor == parsed.depositor_account_id
            && metadata.depositor_asset == parsed.depositor_account_id
            && metadata.claimant == parsed.claimant_account_id
            && metadata.claimant_asset == parsed.claimant_account_id
            && metadata.custody == parsed.custody_account_id
            && metadata.asset_program == parsed.authenticated_transfer_program_id
            && metadata.custody_program == parsed.authenticated_transfer_program_id
            && metadata.asset_definition == [0; 32]
            && metadata.amount == parsed.terms.amount().as_u128()
            && metadata.refund_at == parsed.terms.refund_at_ms(),
        "official metadata does not exactly match the signed native terms"
    );
    ensure!(
        facts.custody_account().program_owner == parsed.authenticated_transfer_program_id
            && facts.custody_account().balance == expected_custody_balance,
        "official custody owner or balance differs from the required lifecycle state"
    );
    Ok(())
}

fn escrow_state(facts: &OfficialNativeEscrowFacts) -> Result<Option<EscrowState>> {
    Ok(
        decode_metadata(facts)?.map(|metadata| match metadata.status {
            EscrowStatus::Empty => EscrowState::Empty,
            EscrowStatus::Funded => EscrowState::Funded,
            EscrowStatus::Claimed => EscrowState::Claimed,
            EscrowStatus::Refunded => EscrowState::Refunded,
        }),
    )
}

fn decode_metadata(facts: &OfficialNativeEscrowFacts) -> Result<Option<EscrowMetadata>> {
    let bytes = facts.metadata_account().data.as_ref();
    if bytes.is_empty() {
        return Ok(None);
    }
    EscrowMetadata::try_from_slice(bytes)
        .map(Some)
        .context("official escrow metadata account is malformed")
}

fn snapshot_evidence(
    parsed: &ParsedTerms,
    facts: &OfficialNativeEscrowFacts,
) -> Result<SnapshotEvidence> {
    Ok(SnapshotEvidence {
        sequencer_tip: facts.sequencer_tip(),
        tip_block_hash: Hex32::from_bytes(facts.tip_block_hash()),
        tip_timestamp_ms: facts.tip_timestamp_ms(),
        escrow_state: escrow_state(facts)?,
        metadata: account_evidence(parsed.metadata_account_id, facts.metadata_account()),
        custody: account_evidence(parsed.custody_account_id, facts.custody_account()),
        depositor: account_evidence(parsed.depositor_account_id, facts.depositor_account()),
        claimant: account_evidence(parsed.claimant_account_id, facts.claimant_account()),
    })
}

fn account_evidence(account_id: AccountId, account: &Account) -> AccountEvidence {
    AccountEvidence {
        account_id: Hex32::from_bytes(account_id.into_value()),
        program_owner: program_id_to_hex(account.program_owner),
        balance: account.balance,
        nonce: u128::from(account.nonce),
    }
}

fn runtime_evidence(actor: &ActorContext, facts: &OfficialNativeEscrowFacts) -> RuntimeEvidence {
    RuntimeEvidence {
        compatibility: RuntimeCompatibility::LeeV0_2_0,
        chain_id: actor.chain_id,
        chain_id_source: "explicit_configuration_no_v02_rpc",
        channel_id: Hex32::from_bytes(facts.channel_id()),
        channel_id_source: "sequencer_getChannelId",
        genesis_block_hash: Hex32::from_bytes(facts.genesis_block_hash()),
        genesis_block_hash_source: "sequencer_getBlock_genesis",
        escrow_program_id: actor.parsed.escrow_program_hex,
        escrow_program_id_source: "explicit_checked_deployment",
    }
}

fn parse_nonzero_hex(value: &str, name: &str) -> Result<Hex32> {
    let value = Hex32::from_hex(value).with_context(|| format!("invalid {name}"))?;
    ensure!(value.as_bytes() != &[0; 32], "{name} must be nonzero");
    Ok(value)
}

fn validate_state_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).context("state directory is unavailable")?;
    ensure!(
        metadata.is_dir(),
        "state directory must be a real directory"
    );
    ensure!(
        metadata.permissions().mode() & 0o7777 == 0o700,
        "state directory must already have mode 0700"
    );
    Ok(())
}

fn read_private_key(path: &Path) -> Result<PrivateKey> {
    let encoded = read_secret_file(path, "private key")?;
    let encoded = std::str::from_utf8(encoded.as_slice())
        .context("private key file must be UTF-8")?
        .trim();
    if encoded.len() != 64 {
        bail!("private key file must contain exactly 64 hex characters");
    }
    PrivateKey::from_str(encoded).context("private key file contains an invalid key")
}

fn read_secret32(path: &Path, name: &str) -> Result<Zeroizing<[u8; 32]>> {
    let mut encoded = read_secret_file(path, name)?;
    let text = std::str::from_utf8(encoded.as_slice())
        .with_context(|| format!("{name} file must be UTF-8"))?
        .trim();
    ensure!(
        text.len() == 64,
        "{name} file must contain exactly 64 hex characters"
    );
    let mut bytes = Zeroizing::new([0_u8; 32]);
    hex::decode_to_slice(text, bytes.as_mut()).with_context(|| format!("invalid {name}"))?;
    encoded.zeroize();
    Ok(bytes)
}

fn read_secret_file(path: &Path, name: &str) -> Result<Zeroizing<Vec<u8>>> {
    let metadata =
        fs::symlink_metadata(path).with_context(|| format!("{name} file unavailable"))?;
    ensure!(metadata.is_file(), "{name} must be a regular file");
    ensure!(
        metadata.permissions().mode().trailing_zeros() >= 6,
        "{name} file must not be accessible by group or others"
    );
    ensure!(metadata.nlink() == 1, "{name} file must have one link");
    let encoded = Zeroizing::new(fs::read(path).with_context(|| format!("read {name} file"))?);
    ensure!(encoded.len() <= 128, "{name} file is too large");
    Ok(encoded)
}
