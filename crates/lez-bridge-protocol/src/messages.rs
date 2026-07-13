use serde::{Deserialize, Deserializer, Serialize};

use crate::{
    AccountIds, ChainPosition, ChainTip, DiscoveryWindow, ErrorCode, ErrorMessage,
    ExactTransactionBytes, Hex32, MessageContext, NativeAmount, NativeEscrowTerms, Participant,
    RevealingPreimage, TransactionId,
};

/// Exact official runtime generation isolated behind the sidecar.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[must_use]
pub enum RuntimeCompatibility {
    /// Pinned NSSA/SPeL v0.1.2 compatibility graph.
    NssaV0_1_2,
}

/// Primitive identity reported by a dedicated official LEZ sidecar.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct RuntimeDescriptor {
    /// Participant whose key is isolated in this sidecar.
    pub sidecar_role: Participant,
    /// Pinned official compatibility graph.
    pub compatibility: RuntimeCompatibility,
    /// Configured chain identity.
    pub chain_id: Hex32,
    /// Configured nonzero channel identity.
    pub channel_id: Hex32,
    /// Genesis block hash returned by the node; upstream `BlockId` itself is numeric.
    pub genesis_block_hash: Hex32,
    /// Escrow program account identity.
    pub escrow_program_id: Hex32,
    /// Public signer account controlled by this sidecar.
    pub signer_account_id: Hex32,
}

impl RuntimeDescriptor {
    /// Creates primitive runtime identity facts.
    pub const fn new(
        sidecar_role: Participant,
        compatibility: RuntimeCompatibility,
        chain_id: Hex32,
        channel_id: Hex32,
        genesis_block_hash: Hex32,
        escrow_program_id: Hex32,
        signer_account_id: Hex32,
    ) -> Self {
        Self {
            sidecar_role,
            compatibility,
            chain_id,
            channel_id,
            genesis_block_hash,
            escrow_program_id,
            signer_account_id,
        }
    }
}

/// Requests the sidecar's runtime and signer facts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct DescribeRuntimeRequest {
    /// Version, isolation, correlation, and role fields.
    pub context: MessageContext,
}

impl DescribeRuntimeRequest {
    /// Creates a runtime-description request.
    pub const fn new(context: MessageContext) -> Self {
        Self { context }
    }
}

/// Returns the sidecar's runtime and signer facts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct DescribeRuntimeResult {
    /// Echoed request context.
    pub context: MessageContext,
    /// Primitive runtime identity.
    pub runtime: RuntimeDescriptor,
}

impl DescribeRuntimeResult {
    /// Creates a runtime-description result.
    pub const fn new(context: MessageContext, runtime: RuntimeDescriptor) -> Self {
        Self { context, runtime }
    }
}

/// Exact official-decoder ID and inner `PublicTransaction::to_bytes()` output.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct PreparedTransaction {
    /// Official-decoder transaction identity.
    pub transaction_id: TransactionId,
    /// Inner official `PublicTransaction::to_bytes()`; never outer `NSSATransaction` bytes.
    pub exact_bytes: ExactTransactionBytes,
}

impl PreparedTransaction {
    /// Creates a prepared transaction result.
    pub const fn new(transaction_id: TransactionId, exact_bytes: ExactTransactionBytes) -> Self {
        Self {
            transaction_id,
            exact_bytes,
        }
    }
}

/// Requests consecutive native escrow initialization and funding transactions.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct PrepareNativeEscrowRequest {
    /// Version, isolation, correlation, and role fields.
    pub context: MessageContext,
    /// Expected runtime identity.
    pub runtime: RuntimeDescriptor,
    /// Exact native escrow values.
    pub terms: NativeEscrowTerms,
}

impl PrepareNativeEscrowRequest {
    /// Creates a native escrow preparation request.
    pub const fn new(
        context: MessageContext,
        runtime: RuntimeDescriptor,
        terms: NativeEscrowTerms,
    ) -> Self {
        Self {
            context,
            runtime,
            terms,
        }
    }
}

/// Exact initialization and funding transactions prepared under consecutive nonces.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct PrepareNativeEscrowResult {
    /// Echoed request context.
    pub context: MessageContext,
    /// Exact initialization transaction.
    pub initialization: PreparedTransaction,
    /// Exact funding transaction.
    pub funding: PreparedTransaction,
}

impl PrepareNativeEscrowResult {
    /// Creates a native escrow preparation result.
    pub const fn new(
        context: MessageContext,
        initialization: PreparedTransaction,
        funding: PreparedTransaction,
    ) -> Self {
        Self {
            context,
            initialization,
            funding,
        }
    }
}

/// Selects owned exact IDs or counterparty discovery by signed terms.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
#[must_use]
pub enum EscrowObservationTarget {
    /// Observe the actor's exact persisted initialization and funding IDs.
    Exact {
        /// Exact initialization transaction ID.
        initialization_transaction_id: TransactionId,
        /// Exact funding transaction ID.
        funding_transaction_id: TransactionId,
    },
    /// Discover a counterparty pair in one explicitly bounded canonical window.
    DiscoverByTerms {
        /// Inclusive bounded scan range that must be fully covered before absence.
        window: DiscoveryWindow,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ExactMode {
    Exact,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum DiscoverByTermsMode {
    DiscoverByTerms,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExactEscrowObservationTargetWire {
    mode: ExactMode,
    initialization_transaction_id: TransactionId,
    funding_transaction_id: TransactionId,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DiscoverByTermsObservationTargetWire {
    mode: DiscoverByTermsMode,
    window: DiscoveryWindow,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum EscrowObservationTargetWire {
    Exact(ExactEscrowObservationTargetWire),
    DiscoverByTerms(DiscoverByTermsObservationTargetWire),
}

impl<'de> Deserialize<'de> for EscrowObservationTarget {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match EscrowObservationTargetWire::deserialize(deserializer)? {
            EscrowObservationTargetWire::Exact(wire) => {
                let ExactMode::Exact = wire.mode;
                Ok(Self::Exact {
                    initialization_transaction_id: wire.initialization_transaction_id,
                    funding_transaction_id: wire.funding_transaction_id,
                })
            }
            EscrowObservationTargetWire::DiscoverByTerms(wire) => {
                let DiscoverByTermsMode::DiscoverByTerms = wire.mode;
                Ok(Self::DiscoverByTerms {
                    window: wire.window,
                })
            }
        }
    }
}

/// Requests primitive observations for an initialization/funding pair.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct ObserveEscrowRequest {
    /// Version, isolation, correlation, and role fields.
    pub context: MessageContext,
    /// Expected runtime identity.
    pub runtime: RuntimeDescriptor,
    /// Expected native escrow values.
    pub terms: NativeEscrowTerms,
    /// Exact-ID lookup or terms-based counterparty discovery.
    pub target: EscrowObservationTarget,
}

impl ObserveEscrowRequest {
    /// Creates an escrow observation request.
    pub const fn new(
        context: MessageContext,
        runtime: RuntimeDescriptor,
        terms: NativeEscrowTerms,
        target: EscrowObservationTarget,
    ) -> Self {
        Self {
            context,
            runtime,
            terms,
            target,
        }
    }
}

/// Primitive transaction facts from the official decoder and node scan.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct ObservedTransactionFacts {
    /// Official-decoder transaction identity.
    pub transaction_id: TransactionId,
    /// Inner official `PublicTransaction::to_bytes()`; never outer `NSSATransaction` bytes.
    pub exact_bytes: ExactTransactionBytes,
    /// Node-reported placement in the scanned chain.
    pub position: ChainPosition,
    /// Ordered official-decoder signer identities.
    pub signer_account_ids: AccountIds,
    /// Whether the decoded official transaction is public.
    pub is_public: bool,
}

impl ObservedTransactionFacts {
    /// Creates primitive observed transaction facts.
    pub const fn new(
        transaction_id: TransactionId,
        exact_bytes: ExactTransactionBytes,
        position: ChainPosition,
        signer_account_ids: AccountIds,
        is_public: bool,
    ) -> Self {
        Self {
            transaction_id,
            exact_bytes,
            position,
            signer_account_ids,
            is_public,
        }
    }
}

/// Primitive fields of the pinned guest's `InitializeNative` instruction.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct NativeInitializeInstructionFacts {
    /// Runtime escrow program targeted by the instruction.
    pub program_id: Hex32,
    /// Exact `[metadata, custody, depositor, claimant]` account order.
    pub ordered_account_ids: AccountIds,
    /// Every decoded `InitializeNative` argument and signed role binding.
    pub terms: NativeEscrowTerms,
}

impl NativeInitializeInstructionFacts {
    /// Creates primitive `InitializeNative` instruction facts.
    pub const fn new(
        program_id: Hex32,
        ordered_account_ids: AccountIds,
        terms: NativeEscrowTerms,
    ) -> Self {
        Self {
            program_id,
            ordered_account_ids,
            terms,
        }
    }
}

/// Primitive fields of the pinned guest's `FundNative` instruction.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct NativeFundInstructionFacts {
    /// Runtime escrow program targeted by the instruction.
    pub program_id: Hex32,
    /// Exact `[metadata, custody, depositor]` account order.
    pub ordered_account_ids: AccountIds,
    /// Only argument encoded by `FundNative`.
    pub swap_id: Hex32,
}

impl NativeFundInstructionFacts {
    /// Creates primitive `FundNative` instruction facts.
    pub const fn new(program_id: Hex32, ordered_account_ids: AccountIds, swap_id: Hex32) -> Self {
        Self {
            program_id,
            ordered_account_ids,
            swap_id,
        }
    }
}

/// Primitive escrow metadata states understood by v0.1.2.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[must_use]
pub enum EscrowState {
    /// Exact upstream `EscrowStatus::Empty` after initialization.
    Empty,
    /// Custody was funded.
    Funded,
    /// The claimant revealed the preimage.
    Claimed,
    /// The depositor refunded after expiry.
    Refunded,
}

/// Primitive decoded metadata account fields.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct EscrowMetadataFacts {
    /// Metadata account identity read from the node.
    pub account_id: Hex32,
    /// Node-reported metadata account owner.
    pub owner_program_id: Hex32,
    /// Exact pinned metadata schema version.
    pub version: u8,
    /// Swap identifier stored by the guest.
    pub swap_id: Hex32,
    /// Exact signed agreement commitment stored as `terms_hash`.
    pub terms_hash: Hex32,
    /// Secret digest stored by the guest.
    pub secret_digest: Hex32,
    /// Native depositor account stored by the guest.
    pub depositor_account_id: Hex32,
    /// Native depositor asset account; equal to `depositor_account_id` for native swaps.
    pub depositor_asset_account_id: Hex32,
    /// Native claimant account stored by the guest.
    pub claimant_account_id: Hex32,
    /// Native claimant asset account; equal to `claimant_account_id` for native swaps.
    pub claimant_asset_account_id: Hex32,
    /// Custody account stored by the guest.
    pub custody_account_id: Hex32,
    /// Asset program stored by the guest.
    pub asset_program_id: Hex32,
    /// Custody program stored by the guest.
    pub custody_program_id: Hex32,
    /// Exact asset-definition sentinel; all zeroes for native swaps.
    pub asset_definition: Hex32,
    /// Full-width amount stored by the guest.
    pub amount: NativeAmount,
    /// Guest `refund_at` expressed explicitly as Unix milliseconds at this boundary.
    pub refund_at_ms: u64,
    /// Exact upstream escrow status.
    pub status: EscrowState,
}

impl EscrowMetadataFacts {
    /// Builds the exact native metadata shape emitted by the pinned guest.
    pub const fn from_native_terms(
        account_id: Hex32,
        owner_program_id: Hex32,
        custody_account_id: Hex32,
        terms: &NativeEscrowTerms,
        status: EscrowState,
    ) -> Self {
        Self {
            account_id,
            owner_program_id,
            version: 1,
            swap_id: terms.swap_id(),
            terms_hash: terms.terms_hash(),
            secret_digest: terms.secret_digest(),
            depositor_account_id: terms.depositor_account_id(),
            depositor_asset_account_id: terms.depositor_account_id(),
            claimant_account_id: terms.claimant_account_id(),
            claimant_asset_account_id: terms.claimant_account_id(),
            custody_account_id,
            asset_program_id: terms.authenticated_transfer_program_id(),
            custody_program_id: terms.authenticated_transfer_program_id(),
            asset_definition: Hex32::from_bytes([0; 32]),
            amount: terms.amount(),
            refund_at_ms: terms.refund_at_ms(),
            status,
        }
    }
}

/// Primitive native custody account fields.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct NativeCustodyFacts {
    /// Custody account identity read from the node.
    pub account_id: Hex32,
    /// Node-reported custody account owner.
    pub owner_program_id: Hex32,
    /// Node-reported native balance.
    pub balance: NativeAmount,
}

impl NativeCustodyFacts {
    /// Creates primitive custody facts.
    pub const fn new(account_id: Hex32, owner_program_id: Hex32, balance: u128) -> Self {
        Self {
            account_id,
            owner_program_id,
            balance: NativeAmount::new(balance),
        }
    }
}

/// Complete primitive facts for a found initialization transaction.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct InitializationFoundFacts {
    /// Primitive transaction and placement facts.
    pub transaction: ObservedTransactionFacts,
    /// Primitive official-decoder initialization fields.
    pub instruction: NativeInitializeInstructionFacts,
    /// Current metadata read at the bracketed stable tip.
    pub metadata: EscrowMetadataFacts,
}

impl InitializationFoundFacts {
    /// Creates complete found initialization facts.
    pub const fn new(
        transaction: ObservedTransactionFacts,
        instruction: NativeInitializeInstructionFacts,
        metadata: EscrowMetadataFacts,
    ) -> Self {
        Self {
            transaction,
            instruction,
            metadata,
        }
    }
}

/// Initialization lookup result without claiming finality or absence prematurely.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", content = "facts", rename_all = "snake_case")]
#[must_use]
pub enum InitializationObservation {
    /// Stable tips fully covered the declared window, or a durable ledger proves never submitted.
    Absent,
    /// Upstream cannot distinguish pending from absent, including an exact-ID canonical miss.
    UnknownOrPending,
    /// The sidecar found and decoded one initialization transaction.
    Found(Box<InitializationFoundFacts>),
}

impl InitializationObservation {
    /// Wraps complete found initialization facts without inflating absent results.
    pub fn found(facts: InitializationFoundFacts) -> Self {
        Self::Found(Box::new(facts))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum AbsentStatus {
    Absent,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum UnknownOrPendingStatus {
    UnknownOrPending,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum FoundStatus {
    Found,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AbsentObservationWire {
    status: AbsentStatus,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UnknownOrPendingObservationWire {
    status: UnknownOrPendingStatus,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FoundObservationWire<T> {
    status: FoundStatus,
    facts: T,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum InitializationObservationWire {
    Absent(AbsentObservationWire),
    UnknownOrPending(UnknownOrPendingObservationWire),
    Found(Box<FoundObservationWire<InitializationFoundFacts>>),
}

impl<'de> Deserialize<'de> for InitializationObservation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match InitializationObservationWire::deserialize(deserializer)? {
            InitializationObservationWire::Absent(wire) => {
                let AbsentStatus::Absent = wire.status;
                Ok(Self::Absent)
            }
            InitializationObservationWire::UnknownOrPending(wire) => {
                let UnknownOrPendingStatus::UnknownOrPending = wire.status;
                Ok(Self::UnknownOrPending)
            }
            InitializationObservationWire::Found(wire) => {
                let FoundStatus::Found = wire.status;
                Ok(Self::found(wire.facts))
            }
        }
    }
}

/// Complete primitive facts for a found funding transaction.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct FundingFoundFacts {
    /// Primitive transaction and placement facts.
    pub transaction: ObservedTransactionFacts,
    /// Primitive official-decoder funding fields.
    pub instruction: NativeFundInstructionFacts,
    /// Current metadata read at the bracketed stable tip.
    pub metadata: EscrowMetadataFacts,
    /// Primitive funded custody account fields.
    pub custody: NativeCustodyFacts,
}

impl FundingFoundFacts {
    /// Creates complete found funding facts.
    pub const fn new(
        transaction: ObservedTransactionFacts,
        instruction: NativeFundInstructionFacts,
        metadata: EscrowMetadataFacts,
        custody: NativeCustodyFacts,
    ) -> Self {
        Self {
            transaction,
            instruction,
            metadata,
            custody,
        }
    }
}

/// Funding lookup result without claiming finality or absence prematurely.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", content = "facts", rename_all = "snake_case")]
#[must_use]
pub enum FundingObservation {
    /// Stable tips fully covered the declared window, or a durable ledger proves never submitted.
    Absent,
    /// Upstream cannot distinguish pending from absent, including an exact-ID canonical miss.
    UnknownOrPending,
    /// The sidecar found and decoded one funding transaction.
    Found(Box<FundingFoundFacts>),
}

impl FundingObservation {
    /// Wraps complete found funding facts without inflating absent results.
    pub fn found(facts: FundingFoundFacts) -> Self {
        Self::Found(Box::new(facts))
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum FundingObservationWire {
    Absent(AbsentObservationWire),
    UnknownOrPending(UnknownOrPendingObservationWire),
    Found(Box<FoundObservationWire<FundingFoundFacts>>),
}

impl<'de> Deserialize<'de> for FundingObservation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match FundingObservationWire::deserialize(deserializer)? {
            FundingObservationWire::Absent(wire) => {
                let AbsentStatus::Absent = wire.status;
                Ok(Self::Absent)
            }
            FundingObservationWire::UnknownOrPending(wire) => {
                let UnknownOrPendingStatus::UnknownOrPending = wire.status;
                Ok(Self::UnknownOrPending)
            }
            FundingObservationWire::Found(wire) => {
                let FoundStatus::Found = wire.status;
                Ok(Self::found(wire.facts))
            }
        }
    }
}

/// Primitive escrow observations bracketed by node tips.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct ObserveEscrowResult {
    /// Echoed request context.
    pub context: MessageContext,
    /// Tip immediately before the node reads.
    pub tip_before: ChainTip,
    /// Explicit initialization lookup state and any complete found facts.
    pub initialization: InitializationObservation,
    /// Explicit funding lookup state and any complete found facts.
    pub funding: FundingObservation,
    /// Tip immediately after all node reads.
    pub tip_after: ChainTip,
}

impl ObserveEscrowResult {
    /// Creates primitive escrow observation facts.
    pub const fn new(
        context: MessageContext,
        tip_before: ChainTip,
        initialization: InitializationObservation,
        funding: FundingObservation,
        tip_after: ChainTip,
    ) -> Self {
        Self {
            context,
            tip_before,
            initialization,
            funding,
            tip_after,
        }
    }
}

/// Requests a revealing claim transaction for a funded native escrow.
#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct PrepareRevealingClaimRequest {
    /// Version, isolation, correlation, and role fields.
    pub context: MessageContext,
    /// Expected runtime identity.
    pub runtime: RuntimeDescriptor,
    /// Expected native escrow values.
    pub terms: NativeEscrowTerms,
    /// Funding transaction being claimed.
    pub funding_transaction_id: TransactionId,
    /// Revealing preimage, redacted from Debug output.
    preimage: RevealingPreimage,
}

impl PrepareRevealingClaimRequest {
    /// Creates a revealing claim preparation request.
    pub const fn new(
        context: MessageContext,
        runtime: RuntimeDescriptor,
        terms: NativeEscrowTerms,
        funding_transaction_id: TransactionId,
        preimage: RevealingPreimage,
    ) -> Self {
        Self {
            context,
            runtime,
            terms,
            funding_transaction_id,
            preimage,
        }
    }

    /// Explicitly exposes the preimage wrapper to the official transaction builder.
    pub const fn preimage(&self) -> &RevealingPreimage {
        &self.preimage
    }
}

/// Exact revealing claim transaction prepared by the official sidecar.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct PrepareRevealingClaimResult {
    /// Echoed request context.
    pub context: MessageContext,
    /// Exact revealing claim transaction.
    pub claim: PreparedTransaction,
}

impl PrepareRevealingClaimResult {
    /// Creates a revealing claim preparation result.
    pub const fn new(context: MessageContext, claim: PreparedTransaction) -> Self {
        Self { context, claim }
    }
}

/// Selects an owned exact claim ID or counterparty discovery by signed terms.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
#[must_use]
pub enum RevealingClaimObservationTarget {
    /// Observe the actor's exact persisted claim ID.
    Exact {
        /// Exact revealing claim transaction ID.
        claim_transaction_id: TransactionId,
    },
    /// Discover a counterparty claim in one explicitly bounded canonical window.
    DiscoverByTerms {
        /// Inclusive bounded scan range that must be fully covered before absence.
        window: DiscoveryWindow,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExactRevealingClaimObservationTargetWire {
    mode: ExactMode,
    claim_transaction_id: TransactionId,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RevealingClaimObservationTargetWire {
    Exact(ExactRevealingClaimObservationTargetWire),
    DiscoverByTerms(DiscoverByTermsObservationTargetWire),
}

impl<'de> Deserialize<'de> for RevealingClaimObservationTarget {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match RevealingClaimObservationTargetWire::deserialize(deserializer)? {
            RevealingClaimObservationTargetWire::Exact(wire) => {
                let ExactMode::Exact = wire.mode;
                Ok(Self::Exact {
                    claim_transaction_id: wire.claim_transaction_id,
                })
            }
            RevealingClaimObservationTargetWire::DiscoverByTerms(wire) => {
                let DiscoverByTermsMode::DiscoverByTerms = wire.mode;
                Ok(Self::DiscoverByTerms {
                    window: wire.window,
                })
            }
        }
    }
}

/// Requests primitive observations for a revealing claim.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct ObserveRevealingClaimRequest {
    /// Version, isolation, correlation, and role fields.
    pub context: MessageContext,
    /// Expected runtime identity.
    pub runtime: RuntimeDescriptor,
    /// Expected native escrow values.
    pub terms: NativeEscrowTerms,
    /// Exact-ID lookup or terms-based counterparty discovery.
    pub target: RevealingClaimObservationTarget,
}

impl ObserveRevealingClaimRequest {
    /// Creates a revealing claim observation request.
    pub const fn new(
        context: MessageContext,
        runtime: RuntimeDescriptor,
        terms: NativeEscrowTerms,
        target: RevealingClaimObservationTarget,
    ) -> Self {
        Self {
            context,
            runtime,
            terms,
            target,
        }
    }
}

/// Primitive decoded claim instruction fields.
#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct NativeClaimInstructionFacts {
    /// Program account targeted by the claim.
    pub program_id: Hex32,
    /// Exact ordered accounts consumed by the claim.
    pub ordered_account_ids: AccountIds,
    /// Swap identifier encoded by `ClaimNative`.
    pub swap_id: Hex32,
    /// Publicly revealed preimage, still redacted from Debug output.
    pub preimage: RevealingPreimage,
}

impl NativeClaimInstructionFacts {
    /// Creates primitive claim instruction facts.
    pub const fn new(
        program_id: Hex32,
        ordered_account_ids: AccountIds,
        swap_id: Hex32,
        preimage: RevealingPreimage,
    ) -> Self {
        Self {
            program_id,
            ordered_account_ids,
            swap_id,
            preimage,
        }
    }
}

/// Complete primitive facts for a found revealing claim transaction.
#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct RevealingClaimFoundFacts {
    /// Primitive transaction and placement facts.
    pub transaction: ObservedTransactionFacts,
    /// Primitive decoded claim instruction fields.
    pub instruction: NativeClaimInstructionFacts,
    /// Primitive decoded terminal metadata fields.
    pub metadata: EscrowMetadataFacts,
    /// Primitive terminal custody account fields.
    pub custody: NativeCustodyFacts,
}

impl RevealingClaimFoundFacts {
    /// Creates complete found revealing claim facts.
    pub const fn new(
        transaction: ObservedTransactionFacts,
        instruction: NativeClaimInstructionFacts,
        metadata: EscrowMetadataFacts,
        custody: NativeCustodyFacts,
    ) -> Self {
        Self {
            transaction,
            instruction,
            metadata,
            custody,
        }
    }
}

/// Revealing claim lookup result without claiming finality or absence prematurely.
#[derive(Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", content = "facts", rename_all = "snake_case")]
#[must_use]
pub enum RevealingClaimObservation {
    /// Stable tips fully covered the declared window, or a durable ledger proves never submitted.
    Absent,
    /// Upstream cannot distinguish pending from absent, including an exact-ID canonical miss.
    UnknownOrPending,
    /// The sidecar found and decoded one revealing claim transaction.
    Found(Box<RevealingClaimFoundFacts>),
}

impl RevealingClaimObservation {
    /// Wraps complete found claim facts without inflating absent results.
    pub fn found(facts: RevealingClaimFoundFacts) -> Self {
        Self::Found(Box::new(facts))
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RevealingClaimObservationWire {
    Absent(AbsentObservationWire),
    UnknownOrPending(UnknownOrPendingObservationWire),
    Found(Box<FoundObservationWire<RevealingClaimFoundFacts>>),
}

impl<'de> Deserialize<'de> for RevealingClaimObservation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match RevealingClaimObservationWire::deserialize(deserializer)? {
            RevealingClaimObservationWire::Absent(wire) => {
                let AbsentStatus::Absent = wire.status;
                Ok(Self::Absent)
            }
            RevealingClaimObservationWire::UnknownOrPending(wire) => {
                let UnknownOrPendingStatus::UnknownOrPending = wire.status;
                Ok(Self::UnknownOrPending)
            }
            RevealingClaimObservationWire::Found(wire) => {
                let FoundStatus::Found = wire.status;
                Ok(Self::found(wire.facts))
            }
        }
    }
}

/// Primitive revealing claim observations bracketed by node tips.
#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct ObserveRevealingClaimResult {
    /// Echoed request context.
    pub context: MessageContext,
    /// Tip immediately before the node reads.
    pub tip_before: ChainTip,
    /// Explicit claim lookup state and any complete found facts.
    pub claim: RevealingClaimObservation,
    /// Tip immediately after all node reads.
    pub tip_after: ChainTip,
}

impl ObserveRevealingClaimResult {
    /// Creates primitive revealing claim observation facts.
    pub const fn new(
        context: MessageContext,
        tip_before: ChainTip,
        claim: RevealingClaimObservation,
        tip_after: ChainTip,
    ) -> Self {
        Self {
            context,
            tip_before,
            claim,
            tip_after,
        }
    }
}

/// Requests byte-exact submission of a previously persisted transaction.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct SubmitTransactionRequest {
    /// Version, isolation, correlation, and role fields.
    pub context: MessageContext,
    /// Expected runtime identity.
    pub runtime: RuntimeDescriptor,
    /// Exact expected ID and inner `PublicTransaction::to_bytes()` to submit unchanged.
    pub transaction: PreparedTransaction,
}

impl SubmitTransactionRequest {
    /// Creates a byte-exact submission request.
    pub const fn new(
        context: MessageContext,
        runtime: RuntimeDescriptor,
        transaction: PreparedTransaction,
    ) -> Self {
        Self {
            context,
            runtime,
            transaction,
        }
    }
}

/// Primitive successful submission outcomes.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[must_use]
pub enum SubmissionOutcome {
    /// The node accepted the exact transaction.
    Accepted,
    /// The node already knew the exact transaction ID.
    AlreadyKnown,
}

/// Successful byte-exact submission result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct SubmitTransactionResult {
    /// Echoed request context.
    pub context: MessageContext,
    /// ID returned after official decoding and node submission.
    pub transaction_id: TransactionId,
    /// Primitive node submission outcome.
    pub outcome: SubmissionOutcome,
}

impl SubmitTransactionResult {
    /// Creates a successful submission result.
    pub const fn new(
        context: MessageContext,
        transaction_id: TransactionId,
        outcome: SubmissionOutcome,
    ) -> Self {
        Self {
            context,
            transaction_id,
            outcome,
        }
    }
}

/// Typed bounded failure reply shared by all protocol operations.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct ProtocolErrorReply {
    /// Echoed request context.
    pub context: MessageContext,
    /// Stable machine-readable error category.
    pub code: ErrorCode,
    /// Bounded human-readable detail.
    pub message: ErrorMessage,
}

impl ProtocolErrorReply {
    /// Creates a bounded typed failure reply.
    pub const fn new(context: MessageContext, code: ErrorCode, message: ErrorMessage) -> Self {
        Self {
            context,
            code,
            message,
        }
    }
}
