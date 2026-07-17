use serde::{Deserialize, Deserializer, Serialize};

use crate::{
    AccountIds, AggregateBip340Signature, ChainClock, ChainPosition, ChainTip, DiscoveryWindow,
    ErrorCode, ErrorMessage, ExactMessageBytes, ExactTransactionBytes, Hex32, MessageContext,
    NativeAmount, NativeEscrowTerms, Participant, ProtocolValueError, RevealingPreimage,
    TransactionId, WitnessedLezAssetTermsV2, WitnessedLezAssetV2, WitnessedNativeEscrowTerms,
    WitnessedTokenEscrowTermsV2,
};

/// Exact official runtime generation isolated behind the sidecar.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[must_use]
pub enum RuntimeCompatibility {
    /// Pinned NSSA/SPeL v0.1.2 compatibility graph.
    NssaV0_1_2,
    /// Pinned LEE/SPeL LEZ v0.2.0 compatibility graph.
    LeeV0_2_0,
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

/// Requests one stable current canonical LEZ clock from the official node.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct ObserveCurrentClockRequest {
    /// Version, run, request, and dedicated sidecar-role binding.
    pub context: MessageContext,
    /// Complete expected pinned runtime identity.
    pub runtime: RuntimeDescriptor,
}

impl ObserveCurrentClockRequest {
    /// Creates one authenticated current-clock observation request.
    pub const fn new(context: MessageContext, runtime: RuntimeDescriptor) -> Self {
        Self { context, runtime }
    }
}

/// One stable current canonical LEZ clock from the official node.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct ObserveCurrentClockResult {
    /// Exact echoed request context.
    pub context: MessageContext,
    /// Exact echoed runtime identity used for the official-node read.
    pub runtime: RuntimeDescriptor,
    /// Stable current block identity, height, and consensus timestamp.
    pub clock: ChainClock,
}

impl ObserveCurrentClockResult {
    /// Creates one stable current-clock result.
    pub const fn new(
        context: MessageContext,
        runtime: RuntimeDescriptor,
        clock: ChainClock,
    ) -> Self {
        Self {
            context,
            runtime,
            clock,
        }
    }
}

/// Exact official-decoder ID and inner `PublicTransaction::to_bytes()` output.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct PreparedTransaction {
    /// Official-decoder transaction identity.
    pub transaction_id: TransactionId,
    /// Inner official `PublicTransaction::to_bytes()`; never outer transaction-enum bytes.
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

/// Requests consecutive aggregate-witness escrow initialization and funding transactions.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct PrepareWitnessedEscrowRequest {
    /// Version, isolation, correlation, and depositor-role fields.
    pub context: MessageContext,
    /// Expected pinned LEZ v0.2 runtime identity.
    pub runtime: RuntimeDescriptor,
    /// Complete agreement binding, including the aggregate claim authority.
    pub terms: WitnessedNativeEscrowTerms,
}

impl PrepareWitnessedEscrowRequest {
    /// Creates one aggregate-witness escrow preparation request.
    pub const fn new(
        context: MessageContext,
        runtime: RuntimeDescriptor,
        terms: WitnessedNativeEscrowTerms,
    ) -> Self {
        Self {
            context,
            runtime,
            terms,
        }
    }
}

/// Exact aggregate-witness initialization and funding transactions.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct PrepareWitnessedEscrowResult {
    /// Echoed request context.
    pub context: MessageContext,
    /// Exact signed `InitializeNativeWitnessed` transaction.
    pub initialization: PreparedTransaction,
    /// Exact signed `FundNative` transaction.
    pub funding: PreparedTransaction,
}

impl PrepareWitnessedEscrowResult {
    /// Creates one aggregate-witness escrow preparation result.
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

/// Requests primitive observations for an aggregate-witness initialization/funding pair.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct ObserveWitnessedEscrowRequest {
    /// Version, isolation, correlation, and role fields.
    pub context: MessageContext,
    /// Expected pinned LEZ v0.2 runtime identity.
    pub runtime: RuntimeDescriptor,
    /// Exact witnessed escrow values, including aggregate claim authority.
    pub terms: WitnessedNativeEscrowTerms,
    /// Exact-ID lookup or terms-based counterparty discovery.
    pub target: EscrowObservationTarget,
}

impl ObserveWitnessedEscrowRequest {
    /// Creates an aggregate-witness escrow observation request.
    pub const fn new(
        context: MessageContext,
        runtime: RuntimeDescriptor,
        terms: WitnessedNativeEscrowTerms,
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
    /// Inner official `PublicTransaction::to_bytes()`; never outer transaction-enum bytes.
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

/// Primitive fields of the pinned guest's `InitializeNativeWitnessed` instruction.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct WitnessedNativeInitializeInstructionFacts {
    /// Runtime escrow program targeted by the instruction.
    pub program_id: Hex32,
    /// Exact `[metadata, custody, depositor, claimant, aggregate_authority]` order.
    pub ordered_account_ids: AccountIds,
    /// Every decoded `InitializeNativeWitnessed` argument and role binding.
    pub terms: WitnessedNativeEscrowTerms,
}

impl WitnessedNativeInitializeInstructionFacts {
    /// Creates primitive `InitializeNativeWitnessed` instruction facts.
    pub const fn new(
        program_id: Hex32,
        ordered_account_ids: AccountIds,
        terms: WitnessedNativeEscrowTerms,
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

/// Primitive escrow metadata states shared by the supported runtime generations.
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
    /// Builds the exact native metadata shape emitted by the pinned NSSA v0.1.2 guest.
    pub const fn from_nssa_v0_1_2_native_terms(
        account_id: Hex32,
        owner_program_id: Hex32,
        custody_account_id: Hex32,
        terms: &NativeEscrowTerms,
        status: EscrowState,
    ) -> Self {
        Self::from_generation_native_terms(
            1,
            account_id,
            owner_program_id,
            custody_account_id,
            terms,
            status,
        )
    }

    /// Builds the exact native metadata shape emitted by the pinned Lee v0.2 guest.
    pub const fn from_lee_v0_2_native_terms(
        account_id: Hex32,
        owner_program_id: Hex32,
        custody_account_id: Hex32,
        terms: &NativeEscrowTerms,
        status: EscrowState,
    ) -> Self {
        Self::from_generation_native_terms(
            2,
            account_id,
            owner_program_id,
            custody_account_id,
            terms,
            status,
        )
    }

    const fn from_generation_native_terms(
        version: u8,
        account_id: Hex32,
        owner_program_id: Hex32,
        custody_account_id: Hex32,
        terms: &NativeEscrowTerms,
        status: EscrowState,
    ) -> Self {
        Self {
            account_id,
            owner_program_id,
            version,
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

/// Primitive decoded metadata for an aggregate-witness native escrow.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct WitnessedEscrowMetadataFacts {
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
    /// Aggregate authority account stored by the guest.
    pub aggregate_authority_account_id: Hex32,
    /// Aggregate x-only BIP340 public key stored by the guest.
    pub aggregate_x_only_public_key: Hex32,
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

impl WitnessedEscrowMetadataFacts {
    /// Builds the exact witnessed-native metadata shape emitted by the pinned guest.
    pub const fn from_witnessed_native_terms(
        account_id: Hex32,
        owner_program_id: Hex32,
        custody_account_id: Hex32,
        terms: &WitnessedNativeEscrowTerms,
        status: EscrowState,
    ) -> Self {
        Self {
            account_id,
            owner_program_id,
            version: 2,
            swap_id: terms.swap_id(),
            terms_hash: terms.terms_hash(),
            aggregate_authority_account_id: terms.aggregate_authority_account_id(),
            aggregate_x_only_public_key: terms.aggregate_x_only_public_key(),
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

    /// Builds the exact witnessed custom-token metadata shape emitted by the pinned guest.
    pub const fn from_witnessed_token_terms(
        account_id: Hex32,
        owner_program_id: Hex32,
        terms: &WitnessedTokenEscrowTermsV2,
        status: EscrowState,
    ) -> Self {
        Self {
            account_id,
            owner_program_id,
            version: 2,
            swap_id: terms.swap_id(),
            terms_hash: terms.terms_hash(),
            aggregate_authority_account_id: terms.aggregate_authority_account_id(),
            aggregate_x_only_public_key: terms.aggregate_x_only_public_key(),
            depositor_account_id: terms.depositor_owner_account_id(),
            depositor_asset_account_id: terms.depositor_ata_account_id(),
            claimant_account_id: terms.claimant_owner_account_id(),
            claimant_asset_account_id: terms.claimant_ata_account_id(),
            custody_account_id: terms.custody_ata_account_id(),
            asset_program_id: terms.token_program_id(),
            custody_program_id: terms.ata_program_id(),
            asset_definition: terms.token_definition_account_id(),
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

/// Complete primitive facts for a found aggregate-witness initialization.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct WitnessedInitializationFoundFacts {
    /// Primitive transaction and placement facts.
    pub transaction: ObservedTransactionFacts,
    /// Primitive official-decoder witnessed initialization fields.
    pub instruction: WitnessedNativeInitializeInstructionFacts,
    /// Current witnessed metadata read at the bracketed stable tip.
    pub metadata: WitnessedEscrowMetadataFacts,
}

impl WitnessedInitializationFoundFacts {
    /// Creates complete witnessed initialization facts.
    pub const fn new(
        transaction: ObservedTransactionFacts,
        instruction: WitnessedNativeInitializeInstructionFacts,
        metadata: WitnessedEscrowMetadataFacts,
    ) -> Self {
        Self {
            transaction,
            instruction,
            metadata,
        }
    }
}

/// Witnessed initialization lookup without overstating upstream finality or absence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", content = "facts", rename_all = "snake_case")]
#[must_use]
pub enum WitnessedInitializationObservation {
    /// Stable tips fully covered the declared window, or durable state proves no submission.
    Absent,
    /// Upstream cannot distinguish pending from absent, including an exact-ID canonical miss.
    UnknownOrPending,
    /// The sidecar found and decoded one aggregate-witness initialization.
    Found(Box<WitnessedInitializationFoundFacts>),
}

impl WitnessedInitializationObservation {
    /// Wraps complete found facts without inflating absent results.
    pub fn found(facts: WitnessedInitializationFoundFacts) -> Self {
        Self::Found(Box::new(facts))
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum WitnessedInitializationObservationWire {
    Absent(AbsentObservationWire),
    UnknownOrPending(UnknownOrPendingObservationWire),
    Found(Box<FoundObservationWire<WitnessedInitializationFoundFacts>>),
}

impl<'de> Deserialize<'de> for WitnessedInitializationObservation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match WitnessedInitializationObservationWire::deserialize(deserializer)? {
            WitnessedInitializationObservationWire::Absent(wire) => {
                let AbsentStatus::Absent = wire.status;
                Ok(Self::Absent)
            }
            WitnessedInitializationObservationWire::UnknownOrPending(wire) => {
                let UnknownOrPendingStatus::UnknownOrPending = wire.status;
                Ok(Self::UnknownOrPending)
            }
            WitnessedInitializationObservationWire::Found(wire) => {
                let FoundStatus::Found = wire.status;
                Ok(Self::found(wire.facts))
            }
        }
    }
}

/// Complete primitive facts for a found funding transaction of a witnessed escrow.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct WitnessedFundingFoundFacts {
    /// Primitive transaction and placement facts.
    pub transaction: ObservedTransactionFacts,
    /// Primitive official-decoder funding fields.
    pub instruction: NativeFundInstructionFacts,
    /// Current witnessed metadata read at the bracketed stable tip.
    pub metadata: WitnessedEscrowMetadataFacts,
    /// Primitive funded custody account fields.
    pub custody: NativeCustodyFacts,
}

impl WitnessedFundingFoundFacts {
    /// Creates complete witnessed funding facts.
    pub const fn new(
        transaction: ObservedTransactionFacts,
        instruction: NativeFundInstructionFacts,
        metadata: WitnessedEscrowMetadataFacts,
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

/// Witnessed funding lookup without overstating upstream finality or absence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", content = "facts", rename_all = "snake_case")]
#[must_use]
pub enum WitnessedFundingObservation {
    /// Stable tips fully covered the declared window, or durable state proves no submission.
    Absent,
    /// Upstream cannot distinguish pending from absent, including an exact-ID canonical miss.
    UnknownOrPending,
    /// The sidecar found and decoded one funding transaction.
    Found(Box<WitnessedFundingFoundFacts>),
}

impl WitnessedFundingObservation {
    /// Wraps complete found facts without inflating absent results.
    pub fn found(facts: WitnessedFundingFoundFacts) -> Self {
        Self::Found(Box::new(facts))
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum WitnessedFundingObservationWire {
    Absent(AbsentObservationWire),
    UnknownOrPending(UnknownOrPendingObservationWire),
    Found(Box<FoundObservationWire<WitnessedFundingFoundFacts>>),
}

impl<'de> Deserialize<'de> for WitnessedFundingObservation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match WitnessedFundingObservationWire::deserialize(deserializer)? {
            WitnessedFundingObservationWire::Absent(wire) => {
                let AbsentStatus::Absent = wire.status;
                Ok(Self::Absent)
            }
            WitnessedFundingObservationWire::UnknownOrPending(wire) => {
                let UnknownOrPendingStatus::UnknownOrPending = wire.status;
                Ok(Self::UnknownOrPending)
            }
            WitnessedFundingObservationWire::Found(wire) => {
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

/// Aggregate-witness escrow observations bracketed by the same canonical node tip.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct ObserveWitnessedEscrowResult {
    /// Echoed request context.
    pub context: MessageContext,
    /// Tip immediately before the canonical scan and account reads.
    pub tip_before: ChainTip,
    /// Explicit witnessed initialization lookup state and complete found facts.
    pub initialization: WitnessedInitializationObservation,
    /// Explicit funding lookup state and complete found facts.
    pub funding: WitnessedFundingObservation,
    /// Tip immediately after all canonical scan and account reads.
    pub tip_after: ChainTip,
}

impl ObserveWitnessedEscrowResult {
    /// Creates primitive aggregate-witness escrow observation facts.
    pub const fn new(
        context: MessageContext,
        tip_before: ChainTip,
        initialization: WitnessedInitializationObservation,
        funding: WitnessedFundingObservation,
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

/// Authority-specific native escrow terms accepted by the refund boundary.
///
/// The enum is intentionally untagged: existing hashlock JSON remains
/// byte-for-byte compatible, while each strict inner type rejects mixed
/// hashlock and aggregate-witness authority fields.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
#[must_use]
pub enum NativeRefundTerms {
    /// Preimage/hashlock native escrow terms used by the earlier corridor.
    Hashlock(NativeEscrowTerms),
    /// Aggregate-witness native escrow terms used by the BTC corridor.
    Witnessed(WitnessedNativeEscrowTerms),
}

impl NativeRefundTerms {
    /// Returns the hashlock terms only when that authority shape was decoded.
    #[must_use]
    pub const fn hashlock(&self) -> Option<&NativeEscrowTerms> {
        match self {
            Self::Hashlock(terms) => Some(terms),
            Self::Witnessed(_) => None,
        }
    }

    /// Returns the aggregate-witness terms only when that shape was decoded.
    #[must_use]
    pub const fn witnessed(&self) -> Option<&WitnessedNativeEscrowTerms> {
        match self {
            Self::Hashlock(_) => None,
            Self::Witnessed(terms) => Some(terms),
        }
    }

    /// Returns the swap identifier common to both authority variants.
    pub const fn swap_id(&self) -> Hex32 {
        match self {
            Self::Hashlock(terms) => terms.swap_id(),
            Self::Witnessed(terms) => terms.swap_id(),
        }
    }

    /// Returns the countersigned agreement commitment.
    pub const fn terms_hash(&self) -> Hex32 {
        match self {
            Self::Hashlock(terms) => terms.terms_hash(),
            Self::Witnessed(terms) => terms.terms_hash(),
        }
    }

    /// Returns the immutable refund beneficiary role.
    pub const fn depositor(&self) -> Participant {
        match self {
            Self::Hashlock(terms) => terms.depositor(),
            Self::Witnessed(terms) => terms.depositor(),
        }
    }

    /// Returns the immutable refund beneficiary account.
    pub const fn depositor_account_id(&self) -> Hex32 {
        match self {
            Self::Hashlock(terms) => terms.depositor_account_id(),
            Self::Witnessed(terms) => terms.depositor_account_id(),
        }
    }

    /// Returns the claimant role.
    pub const fn claimant(&self) -> Participant {
        match self {
            Self::Hashlock(terms) => terms.claimant(),
            Self::Witnessed(terms) => terms.claimant(),
        }
    }

    /// Returns the claimant asset account.
    pub const fn claimant_account_id(&self) -> Hex32 {
        match self {
            Self::Hashlock(terms) => terms.claimant_account_id(),
            Self::Witnessed(terms) => terms.claimant_account_id(),
        }
    }

    /// Returns the exact native amount.
    pub const fn amount(&self) -> NativeAmount {
        match self {
            Self::Hashlock(terms) => terms.amount(),
            Self::Witnessed(terms) => terms.amount(),
        }
    }

    /// Returns the guest refund deadline in Unix milliseconds.
    #[must_use]
    pub const fn refund_at_ms(&self) -> u64 {
        match self {
            Self::Hashlock(terms) => terms.refund_at_ms(),
            Self::Witnessed(terms) => terms.refund_at_ms(),
        }
    }

    /// Returns the authenticated-transfer program identity.
    pub const fn authenticated_transfer_program_id(&self) -> Hex32 {
        match self {
            Self::Hashlock(terms) => terms.authenticated_transfer_program_id(),
            Self::Witnessed(terms) => terms.authenticated_transfer_program_id(),
        }
    }
}

impl From<NativeEscrowTerms> for NativeRefundTerms {
    fn from(value: NativeEscrowTerms) -> Self {
        Self::Hashlock(value)
    }
}

impl From<WitnessedNativeEscrowTerms> for NativeRefundTerms {
    fn from(value: WitnessedNativeEscrowTerms) -> Self {
        Self::Witnessed(value)
    }
}

/// Requests one native fixed-destination refund transaction.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct PrepareNativeRefundRequest {
    /// Version, isolation, correlation, and role fields.
    pub context: MessageContext,
    /// Expected runtime identity.
    pub runtime: RuntimeDescriptor,
    /// Exact native escrow values, including the immutable refund destination.
    pub terms: NativeRefundTerms,
}

impl PrepareNativeRefundRequest {
    /// Creates a native refund preparation request.
    pub const fn new(
        context: MessageContext,
        runtime: RuntimeDescriptor,
        terms: NativeEscrowTerms,
    ) -> Self {
        Self {
            context,
            runtime,
            terms: NativeRefundTerms::Hashlock(terms),
        }
    }

    /// Creates an aggregate-witness native refund preparation request.
    pub const fn new_witnessed(
        context: MessageContext,
        runtime: RuntimeDescriptor,
        terms: WitnessedNativeEscrowTerms,
    ) -> Self {
        Self {
            context,
            runtime,
            terms: NativeRefundTerms::Witnessed(terms),
        }
    }
}

/// Exact native refund transaction prepared by the official sidecar.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct PrepareNativeRefundResult {
    /// Echoed request context.
    pub context: MessageContext,
    /// Exact fixed-destination refund transaction.
    pub refund: PreparedTransaction,
}

impl PrepareNativeRefundResult {
    /// Creates a native refund preparation result.
    pub const fn new(context: MessageContext, refund: PreparedTransaction) -> Self {
        Self { context, refund }
    }
}

/// Selects account-state inspection, an owned exact refund, or bounded discovery.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
#[must_use]
pub enum NativeRefundObservationTarget {
    /// Read only the canonical escrow accounts and chain clock before eligibility.
    StateOnly,
    /// Observe an actor's exact persisted refund ID in the caller-selected window.
    Exact {
        /// Exact native refund transaction ID.
        refund_transaction_id: TransactionId,
        /// Inclusive bounded scan range supplied by the caller.
        window: DiscoveryWindow,
    },
    /// Discover a permissionless counterparty refund by signed terms.
    DiscoverByTerms {
        /// Inclusive bounded scan range that must be fully covered before absence.
        window: DiscoveryWindow,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum StateOnlyMode {
    StateOnly,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StateOnlyRefundObservationTargetWire {
    mode: StateOnlyMode,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExactNativeRefundObservationTargetWire {
    mode: ExactMode,
    refund_transaction_id: TransactionId,
    window: DiscoveryWindow,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum NativeRefundObservationTargetWire {
    StateOnly(StateOnlyRefundObservationTargetWire),
    Exact(ExactNativeRefundObservationTargetWire),
    DiscoverByTerms(DiscoverByTermsObservationTargetWire),
}

impl<'de> Deserialize<'de> for NativeRefundObservationTarget {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match NativeRefundObservationTargetWire::deserialize(deserializer)? {
            NativeRefundObservationTargetWire::StateOnly(wire) => {
                let StateOnlyMode::StateOnly = wire.mode;
                Ok(Self::StateOnly)
            }
            NativeRefundObservationTargetWire::Exact(wire) => {
                let ExactMode::Exact = wire.mode;
                Ok(Self::Exact {
                    refund_transaction_id: wire.refund_transaction_id,
                    window: wire.window,
                })
            }
            NativeRefundObservationTargetWire::DiscoverByTerms(wire) => {
                let DiscoverByTermsMode::DiscoverByTerms = wire.mode;
                Ok(Self::DiscoverByTerms {
                    window: wire.window,
                })
            }
        }
    }
}

/// Requests canonical escrow state and an optional native refund observation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct ObserveNativeRefundRequest {
    /// Version, isolation, correlation, and role fields.
    pub context: MessageContext,
    /// Expected runtime identity.
    pub runtime: RuntimeDescriptor,
    /// Expected native escrow values.
    pub terms: NativeRefundTerms,
    /// Account-only, exact-ID, or terms-discovery observation mode.
    pub target: NativeRefundObservationTarget,
}

impl ObserveNativeRefundRequest {
    /// Creates a native refund observation request.
    pub const fn new(
        context: MessageContext,
        runtime: RuntimeDescriptor,
        terms: NativeEscrowTerms,
        target: NativeRefundObservationTarget,
    ) -> Self {
        Self {
            context,
            runtime,
            terms: NativeRefundTerms::Hashlock(terms),
            target,
        }
    }

    /// Creates an aggregate-witness native refund observation request.
    pub const fn new_witnessed(
        context: MessageContext,
        runtime: RuntimeDescriptor,
        terms: WitnessedNativeEscrowTerms,
        target: NativeRefundObservationTarget,
    ) -> Self {
        Self {
            context,
            runtime,
            terms: NativeRefundTerms::Witnessed(terms),
            target,
        }
    }
}

/// Authority-specific decoded metadata returned by native refund observation.
///
/// Untagged serialization preserves the existing hashlock response shape.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
#[must_use]
pub enum NativeRefundMetadataFacts {
    /// Metadata for a preimage/hashlock escrow.
    Hashlock(EscrowMetadataFacts),
    /// Metadata for an aggregate-witness escrow.
    Witnessed(WitnessedEscrowMetadataFacts),
}

impl NativeRefundMetadataFacts {
    /// Returns the hashlock metadata only for that authority shape.
    #[must_use]
    pub const fn hashlock(&self) -> Option<&EscrowMetadataFacts> {
        match self {
            Self::Hashlock(metadata) => Some(metadata),
            Self::Witnessed(_) => None,
        }
    }

    /// Returns mutable hashlock metadata only for that authority shape.
    #[must_use]
    pub const fn hashlock_mut(&mut self) -> Option<&mut EscrowMetadataFacts> {
        match self {
            Self::Hashlock(metadata) => Some(metadata),
            Self::Witnessed(_) => None,
        }
    }

    /// Returns the witnessed metadata only for that authority shape.
    #[must_use]
    pub const fn witnessed(&self) -> Option<&WitnessedEscrowMetadataFacts> {
        match self {
            Self::Hashlock(_) => None,
            Self::Witnessed(metadata) => Some(metadata),
        }
    }

    /// Returns the decoded guest escrow status.
    pub const fn status(&self) -> EscrowState {
        match self {
            Self::Hashlock(metadata) => metadata.status,
            Self::Witnessed(metadata) => metadata.status,
        }
    }
}

impl From<EscrowMetadataFacts> for NativeRefundMetadataFacts {
    fn from(value: EscrowMetadataFacts) -> Self {
        Self::Hashlock(value)
    }
}

impl From<WitnessedEscrowMetadataFacts> for NativeRefundMetadataFacts {
    fn from(value: WitnessedEscrowMetadataFacts) -> Self {
        Self::Witnessed(value)
    }
}

/// Current primitive metadata and native custody fields at one canonical clock.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct NativeEscrowAccountFacts {
    /// Current metadata decoded at the bracketed stable clock.
    pub metadata: NativeRefundMetadataFacts,
    /// Current native custody decoded at the bracketed stable clock.
    pub custody: NativeCustodyFacts,
}

impl NativeEscrowAccountFacts {
    /// Creates current native escrow account facts.
    pub const fn new(metadata: EscrowMetadataFacts, custody: NativeCustodyFacts) -> Self {
        Self {
            metadata: NativeRefundMetadataFacts::Hashlock(metadata),
            custody,
        }
    }

    /// Creates aggregate-witness native escrow account facts.
    pub const fn new_witnessed(
        metadata: WitnessedEscrowMetadataFacts,
        custody: NativeCustodyFacts,
    ) -> Self {
        Self {
            metadata: NativeRefundMetadataFacts::Witnessed(metadata),
            custody,
        }
    }
}

/// Whether canonical native escrow accounts exist at the bracketed clock.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", content = "facts", rename_all = "snake_case")]
#[must_use]
pub enum NativeEscrowAccountObservation {
    /// Both expected accounts are absent at the stable clock.
    ///
    /// A partial or malformed account pair is a typed RPC error, never absence.
    Absent,
    /// Both accounts exist and were decoded into complete primitive facts.
    Found(Box<NativeEscrowAccountFacts>),
}

impl NativeEscrowAccountObservation {
    /// Wraps complete account facts without inflating absent results.
    pub fn found(facts: NativeEscrowAccountFacts) -> Self {
        Self::Found(Box::new(facts))
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum NativeEscrowAccountObservationWire {
    Absent(AbsentObservationWire),
    Found(Box<FoundObservationWire<NativeEscrowAccountFacts>>),
}

impl<'de> Deserialize<'de> for NativeEscrowAccountObservation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match NativeEscrowAccountObservationWire::deserialize(deserializer)? {
            NativeEscrowAccountObservationWire::Absent(wire) => {
                let AbsentStatus::Absent = wire.status;
                Ok(Self::Absent)
            }
            NativeEscrowAccountObservationWire::Found(wire) => {
                let FoundStatus::Found = wire.status;
                Ok(Self::found(wire.facts))
            }
        }
    }
}

/// Primitive fields of the pinned guest's `RefundNative` instruction.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct NativeRefundInstructionFacts {
    /// Runtime escrow program targeted by the instruction.
    pub program_id: Hex32,
    /// Exact `[metadata, custody, depositor]` account order.
    pub ordered_account_ids: AccountIds,
    /// Only argument encoded by `RefundNative`.
    pub swap_id: Hex32,
}

impl NativeRefundInstructionFacts {
    /// Creates primitive `RefundNative` instruction facts.
    pub const fn new(program_id: Hex32, ordered_account_ids: AccountIds, swap_id: Hex32) -> Self {
        Self {
            program_id,
            ordered_account_ids,
            swap_id,
        }
    }
}

/// Complete primitive facts for a found native refund transaction.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct NativeRefundFoundFacts {
    /// Primitive transaction and placement facts.
    pub transaction: ObservedTransactionFacts,
    /// Primitive decoded refund instruction fields.
    pub instruction: NativeRefundInstructionFacts,
}

impl NativeRefundFoundFacts {
    /// Creates complete found native refund facts.
    pub const fn new(
        transaction: ObservedTransactionFacts,
        instruction: NativeRefundInstructionFacts,
    ) -> Self {
        Self {
            transaction,
            instruction,
        }
    }
}

/// Native refund lookup result without claiming finality or absence prematurely.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", content = "facts", rename_all = "snake_case")]
#[must_use]
pub enum NativeRefundObservation {
    /// The caller requested account state only, so no refund lookup occurred.
    NotRequested,
    /// Stable tips fully covered the caller's declared window.
    Absent,
    /// Upstream cannot distinguish pending from absent for the declared lookup.
    UnknownOrPending,
    /// The sidecar found and decoded one native refund transaction.
    Found(Box<NativeRefundFoundFacts>),
}

impl NativeRefundObservation {
    /// Wraps complete found refund facts without inflating absent results.
    pub fn found(facts: NativeRefundFoundFacts) -> Self {
        Self::Found(Box::new(facts))
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum NotRequestedStatus {
    NotRequested,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NotRequestedObservationWire {
    status: NotRequestedStatus,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum NativeRefundObservationWire {
    NotRequested(NotRequestedObservationWire),
    Absent(AbsentObservationWire),
    UnknownOrPending(UnknownOrPendingObservationWire),
    Found(Box<FoundObservationWire<NativeRefundFoundFacts>>),
}

impl<'de> Deserialize<'de> for NativeRefundObservation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match NativeRefundObservationWire::deserialize(deserializer)? {
            NativeRefundObservationWire::NotRequested(wire) => {
                let NotRequestedStatus::NotRequested = wire.status;
                Ok(Self::NotRequested)
            }
            NativeRefundObservationWire::Absent(wire) => {
                let AbsentStatus::Absent = wire.status;
                Ok(Self::Absent)
            }
            NativeRefundObservationWire::UnknownOrPending(wire) => {
                let UnknownOrPendingStatus::UnknownOrPending = wire.status;
                Ok(Self::UnknownOrPending)
            }
            NativeRefundObservationWire::Found(wire) => {
                let FoundStatus::Found = wire.status;
                Ok(Self::found(wire.facts))
            }
        }
    }
}

/// Primitive native refund state bracketed by canonical clocks.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct ObserveNativeRefundResult {
    /// Echoed request context.
    pub context: MessageContext,
    /// Canonical clock immediately before account and transaction reads.
    pub clock_before: ChainClock,
    /// Current metadata and custody state at the stable bracketed clock.
    pub accounts: NativeEscrowAccountObservation,
    /// Explicit refund lookup state and any complete found facts.
    pub refund: NativeRefundObservation,
    /// Canonical clock immediately after all node reads.
    pub clock_after: ChainClock,
}

impl ObserveNativeRefundResult {
    /// Creates primitive native refund observation facts.
    pub const fn new(
        context: MessageContext,
        clock_before: ChainClock,
        accounts: NativeEscrowAccountObservation,
        refund: NativeRefundObservation,
        clock_after: ChainClock,
    ) -> Self {
        Self {
            context,
            clock_before,
            accounts,
            refund,
            clock_after,
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

/// Canonical unsigned official LEZ message reserved for aggregate signing.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct PreparedWitnessedClaim {
    /// Exact prepare request that owns this durable reservation within the run.
    pub preparation_request_id: crate::RequestId,
    /// Pinned official `Message::hash()` that both `MuSig` participants sign.
    pub message_hash: Hex32,
    /// Canonical Borsh encoding of that exact unsigned official `Message`.
    pub exact_message_bytes: ExactMessageBytes,
}

impl PreparedWitnessedClaim {
    /// Creates one exact unsigned witnessed claim transcript.
    pub const fn new(
        preparation_request_id: crate::RequestId,
        message_hash: Hex32,
        exact_message_bytes: ExactMessageBytes,
    ) -> Self {
        Self {
            preparation_request_id,
            message_hash,
            exact_message_bytes,
        }
    }
}

/// Reserves the exact official LEZ witnessed-claim message before either chain locks.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct PrepareWitnessedClaimRequest {
    /// Version, isolation, correlation, and destination-role fields.
    pub context: MessageContext,
    /// Expected pinned LEZ v0.2 runtime identity.
    pub runtime: RuntimeDescriptor,
    /// Complete agreement binding, including separate destination and authority accounts.
    pub terms: WitnessedNativeEscrowTerms,
    /// Exact prebuilt funding transaction expected to establish the escrow.
    pub funding_transaction_id: TransactionId,
}

impl PrepareWitnessedClaimRequest {
    /// Creates one witnessed-claim message-reservation request.
    pub const fn new(
        context: MessageContext,
        runtime: RuntimeDescriptor,
        terms: WitnessedNativeEscrowTerms,
        funding_transaction_id: TransactionId,
    ) -> Self {
        Self {
            context,
            runtime,
            terms,
            funding_transaction_id,
        }
    }
}

/// Exact unsigned LEZ message/hash returned for external adaptor signing.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct PrepareWitnessedClaimResult {
    /// Exact echoed preparation context.
    pub context: MessageContext,
    /// Reserved immutable unsigned claim transcript.
    pub claim: PreparedWitnessedClaim,
}

impl PrepareWitnessedClaimResult {
    /// Creates one witnessed-claim preparation result.
    pub const fn new(context: MessageContext, claim: PreparedWitnessedClaim) -> Self {
        Self { context, claim }
    }
}

/// Completes an exact reserved message with one external aggregate BIP340 signature.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct CompleteWitnessedClaimRequest {
    /// Version, isolation, correlation, and destination-role fields.
    pub context: MessageContext,
    /// Expected pinned LEZ v0.2 runtime identity.
    pub runtime: RuntimeDescriptor,
    /// Exact prepared transcript that was externally signed.
    pub claim: PreparedWitnessedClaim,
    /// Completed 64-byte aggregate BIP340 signature.
    pub aggregate_signature: AggregateBip340Signature,
}

impl CompleteWitnessedClaimRequest {
    /// Creates one exact witnessed-claim completion request.
    pub const fn new(
        context: MessageContext,
        runtime: RuntimeDescriptor,
        claim: PreparedWitnessedClaim,
        aggregate_signature: AggregateBip340Signature,
    ) -> Self {
        Self {
            context,
            runtime,
            claim,
            aggregate_signature,
        }
    }
}

/// Exact completed public transaction eligible for the existing submission method.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct CompleteWitnessedClaimResult {
    /// Exact echoed completion context.
    pub context: MessageContext,
    /// Canonical signed public transaction built by the official LEZ sidecar.
    pub claim: PreparedTransaction,
}

impl CompleteWitnessedClaimResult {
    /// Creates one completed witnessed-claim transaction result.
    pub const fn new(context: MessageContext, claim: PreparedTransaction) -> Self {
        Self { context, claim }
    }
}

/// Requests exact witnessed-initialization classification in one finalized window.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct ClassifyFinalizedWitnessedInitializationRequest {
    /// Version, run, request, and destination-role binding.
    pub context: MessageContext,
    /// Expected pinned LEZ runtime identity.
    pub runtime: RuntimeDescriptor,
    /// Complete witnessed agreement terms.
    pub terms: WitnessedNativeEscrowTerms,
    /// Exact prepared initialization identity and complete signed bytes.
    pub initialization: PreparedTransaction,
    /// Companion funding identity from the same durable prepared pair.
    pub funding_transaction_id: TransactionId,
    /// Inclusive bounded range that must be entirely finalized and scanned.
    pub window: DiscoveryWindow,
}

impl ClassifyFinalizedWitnessedInitializationRequest {
    /// Creates one exact finalized witnessed-initialization classification request.
    pub const fn new(
        context: MessageContext,
        runtime: RuntimeDescriptor,
        terms: WitnessedNativeEscrowTerms,
        initialization: PreparedTransaction,
        funding_transaction_id: TransactionId,
        window: DiscoveryWindow,
    ) -> Self {
        Self {
            context,
            runtime,
            terms,
            initialization,
            funding_transaction_id,
            window,
        }
    }
}

/// Exact finalized witnessed initialization and historical empty escrow state.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct FinalizedWitnessedInitializationFacts {
    /// Canonical exact transaction bytes, signer, and finalized position.
    pub transaction: ObservedTransactionFacts,
    /// Exact decoded witnessed initialization instruction and ordered accounts.
    pub instruction: WitnessedNativeInitializeInstructionFacts,
    /// Identity and consensus timestamp of the containing finalized block.
    pub containing_block: FinalizedBlockIdentity,
    /// Exact empty witnessed metadata at the containing block.
    pub metadata: WitnessedEscrowMetadataFacts,
    /// Exact zero-balance custody account at the containing block.
    pub custody: NativeCustodyFacts,
}

impl FinalizedWitnessedInitializationFacts {
    /// Creates one complete finalized witnessed-initialization proof bundle.
    pub const fn new(
        transaction: ObservedTransactionFacts,
        instruction: WitnessedNativeInitializeInstructionFacts,
        containing_block: FinalizedBlockIdentity,
        metadata: WitnessedEscrowMetadataFacts,
        custody: NativeCustodyFacts,
    ) -> Self {
        Self {
            transaction,
            instruction,
            containing_block,
            metadata,
            custody,
        }
    }
}

/// Safe three-way classification for one exact witnessed initialization.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
#[must_use]
pub enum FinalizedWitnessedInitializationScanOutcome {
    /// Exact bytes and decoded facts occur once in stable finalized ancestry.
    Found {
        /// Complete exact finalized initialization facts.
        initialization: Box<FinalizedWitnessedInitializationFacts>,
    },
    /// Stable finalized and current observations both proved exact absence.
    Absent {},
    /// Finalized absence could not exclude current pending or unknown presence.
    Uncertain {},
}

/// Exact three-way result for one stable finalized initialization scan.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct ClassifyFinalizedWitnessedInitializationResult {
    /// Echoed request context.
    pub context: MessageContext,
    /// Stable finalized clock covering the exact requested window.
    pub finalized_clock: ChainClock,
    /// Exact inclusive bounded range completely scanned.
    pub scanned_window: DiscoveryWindow,
    /// Finalized exact presence, affirmative absence, or conservative uncertainty.
    pub outcome: FinalizedWitnessedInitializationScanOutcome,
}

impl ClassifyFinalizedWitnessedInitializationResult {
    /// Creates exact found evidence.
    pub fn found(
        context: MessageContext,
        finalized_clock: ChainClock,
        scanned_window: DiscoveryWindow,
        initialization: FinalizedWitnessedInitializationFacts,
    ) -> Self {
        Self {
            context,
            finalized_clock,
            scanned_window,
            outcome: FinalizedWitnessedInitializationScanOutcome::Found {
                initialization: Box::new(initialization),
            },
        }
    }

    /// Creates affirmative absence after both finalized and current checks.
    pub const fn absent(
        context: MessageContext,
        finalized_clock: ChainClock,
        scanned_window: DiscoveryWindow,
    ) -> Self {
        Self {
            context,
            finalized_clock,
            scanned_window,
            outcome: FinalizedWitnessedInitializationScanOutcome::Absent {},
        }
    }

    /// Creates conservative uncertainty when pending and absence cannot be separated.
    pub const fn uncertain(
        context: MessageContext,
        finalized_clock: ChainClock,
        scanned_window: DiscoveryWindow,
    ) -> Self {
        Self {
            context,
            finalized_clock,
            scanned_window,
            outcome: FinalizedWitnessedInitializationScanOutcome::Uncertain {},
        }
    }
}

/// Selects an exact funding ID or peerless discovery by witnessed agreement terms.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
#[must_use]
pub enum FinalizedWitnessedFundingObservationTarget {
    /// Require the one exact witnessed funding transaction identity.
    Exact {
        /// Exact completed witnessed funding transaction ID.
        funding_transaction_id: TransactionId,
    },
    /// Discover the unique canonical funding transaction from the signed terms.
    DiscoverByTerms,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExactFinalizedWitnessedFundingObservationTargetWire {
    mode: ExactMode,
    funding_transaction_id: TransactionId,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DiscoverFinalizedWitnessedFundingObservationTargetWire {
    mode: DiscoverByTermsMode,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum FinalizedWitnessedFundingObservationTargetWire {
    Exact(ExactFinalizedWitnessedFundingObservationTargetWire),
    DiscoverByTerms(DiscoverFinalizedWitnessedFundingObservationTargetWire),
}

impl<'de> Deserialize<'de> for FinalizedWitnessedFundingObservationTarget {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match FinalizedWitnessedFundingObservationTargetWire::deserialize(deserializer)? {
            FinalizedWitnessedFundingObservationTargetWire::Exact(wire) => {
                let ExactMode::Exact = wire.mode;
                Ok(Self::Exact {
                    funding_transaction_id: wire.funding_transaction_id,
                })
            }
            FinalizedWitnessedFundingObservationTargetWire::DiscoverByTerms(wire) => {
                let DiscoverByTermsMode::DiscoverByTerms = wire.mode;
                Ok(Self::DiscoverByTerms)
            }
        }
    }
}

/// Requests proof that one witnessed funding transaction occurs in a finalized window.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct ObserveFinalizedWitnessedFundingRequest {
    /// Version, run, request, and destination-role binding.
    pub context: MessageContext,
    /// Expected pinned LEZ runtime identity.
    pub runtime: RuntimeDescriptor,
    /// Complete witnessed agreement terms, including aggregate claim authority.
    pub terms: WitnessedNativeEscrowTerms,
    /// Exact-ID lookup or peerless canonical discovery.
    pub target: FinalizedWitnessedFundingObservationTarget,
    /// Inclusive bounded range that must be entirely finalized and scanned.
    pub window: DiscoveryWindow,
}

impl ObserveFinalizedWitnessedFundingRequest {
    /// Creates one exact, bounded finalized witnessed-funding observation request.
    pub const fn new(
        context: MessageContext,
        runtime: RuntimeDescriptor,
        terms: WitnessedNativeEscrowTerms,
        funding_transaction_id: TransactionId,
        window: DiscoveryWindow,
    ) -> Self {
        Self {
            context,
            runtime,
            terms,
            target: FinalizedWitnessedFundingObservationTarget::Exact {
                funding_transaction_id,
            },
            window,
        }
    }

    /// Creates one bounded terms-based peerless discovery request.
    pub const fn discover_by_terms(
        context: MessageContext,
        runtime: RuntimeDescriptor,
        terms: WitnessedNativeEscrowTerms,
        window: DiscoveryWindow,
    ) -> Self {
        Self {
            context,
            runtime,
            terms,
            target: FinalizedWitnessedFundingObservationTarget::DiscoverByTerms,
            window,
        }
    }
}

/// Complete exact facts proving one witnessed funding effect in a finalized block.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct FinalizedWitnessedFundingFacts {
    /// Canonical transaction bytes, hash, depositor signer, and block position.
    pub transaction: ObservedTransactionFacts,
    /// Exact decoded `FundNative` program, accounts, and swap identifier.
    pub instruction: NativeFundInstructionFacts,
    /// Identity and consensus timestamp of the containing finalized block.
    pub containing_block: FinalizedBlockIdentity,
    /// Funded witnessed metadata read at the exact containing finalized block.
    pub metadata: WitnessedEscrowMetadataFacts,
    /// Funded native custody read at the exact containing finalized block.
    pub custody: NativeCustodyFacts,
}

impl FinalizedWitnessedFundingFacts {
    /// Creates one complete finalized witnessed-funding proof bundle.
    pub const fn new(
        transaction: ObservedTransactionFacts,
        instruction: NativeFundInstructionFacts,
        containing_block: FinalizedBlockIdentity,
        metadata: WitnessedEscrowMetadataFacts,
        custody: NativeCustodyFacts,
    ) -> Self {
        Self {
            transaction,
            instruction,
            containing_block,
            metadata,
            custody,
        }
    }
}

/// Returns one exact witnessed funding effect only after its window is finalized.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct ObserveFinalizedWitnessedFundingResult {
    /// Echoed request context.
    pub context: MessageContext,
    /// Stable finalized indexer tip that completely covers the requested window.
    pub finalized_tip: ChainTip,
    /// Exact finalized witnessed funding facts.
    pub funding: FinalizedWitnessedFundingFacts,
}

impl ObserveFinalizedWitnessedFundingResult {
    /// Creates one exact finalized witnessed-funding result.
    pub const fn new(
        context: MessageContext,
        finalized_tip: ChainTip,
        funding: FinalizedWitnessedFundingFacts,
    ) -> Self {
        Self {
            context,
            finalized_tip,
            funding,
        }
    }
}

/// Exact outcome of one completely validated finalized witnessed-funding scan.
///
/// `Absent` is affirmative only because the enclosing result carries the exact
/// fully scanned window and stable finalized tip. Transport, history, finality,
/// malformed evidence, and moving-tip failures cannot inhabit this enum.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
#[must_use]
pub enum FinalizedWitnessedFundingScanOutcome {
    /// The canonical finalized scan contained one exact validated funding effect.
    Found {
        /// Complete exact finalized funding facts.
        funding: Box<FinalizedWitnessedFundingFacts>,
    },
    /// The complete stable finalized scan contained no matching funding effect.
    Absent {},
}

/// Result of the additive v1 witnessed-funding classifier.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct ClassifyFinalizedWitnessedFundingResult {
    /// Echoed request context.
    pub context: MessageContext,
    /// Stable finalized indexer clock completely covering `scanned_window`.
    pub finalized_clock: ChainClock,
    /// Exact inclusive bounded range completely scanned.
    pub scanned_window: DiscoveryWindow,
    /// Exact funding evidence or affirmative stable-finalized absence.
    pub outcome: FinalizedWitnessedFundingScanOutcome,
}

impl ClassifyFinalizedWitnessedFundingResult {
    /// Creates exact found evidence for a completely scanned stable window.
    pub fn found(
        context: MessageContext,
        finalized_clock: ChainClock,
        scanned_window: DiscoveryWindow,
        funding: FinalizedWitnessedFundingFacts,
    ) -> Self {
        Self {
            context,
            finalized_clock,
            scanned_window,
            outcome: FinalizedWitnessedFundingScanOutcome::Found {
                funding: Box::new(funding),
            },
        }
    }

    /// Creates affirmative absence evidence for a completely scanned stable window.
    pub const fn absent(
        context: MessageContext,
        finalized_clock: ChainClock,
        scanned_window: DiscoveryWindow,
    ) -> Self {
        Self {
            context,
            finalized_clock,
            scanned_window,
            outcome: FinalizedWitnessedFundingScanOutcome::Absent {},
        }
    }
}

/// Selects an exact completed claim ID or peerless discovery by agreement terms.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
#[must_use]
pub enum FinalizedWitnessedClaimObservationTarget {
    /// Require the one exact completed public-transaction identity.
    Exact {
        /// Exact completed witnessed-claim transaction ID.
        claim_transaction_id: TransactionId,
    },
    /// Discover the unique canonical claim from the signed terms and transcript.
    DiscoverByTerms,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExactFinalizedWitnessedClaimObservationTargetWire {
    mode: ExactMode,
    claim_transaction_id: TransactionId,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DiscoverFinalizedWitnessedClaimObservationTargetWire {
    mode: DiscoverByTermsMode,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum FinalizedWitnessedClaimObservationTargetWire {
    Exact(ExactFinalizedWitnessedClaimObservationTargetWire),
    DiscoverByTerms(DiscoverFinalizedWitnessedClaimObservationTargetWire),
}

impl<'de> Deserialize<'de> for FinalizedWitnessedClaimObservationTarget {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match FinalizedWitnessedClaimObservationTargetWire::deserialize(deserializer)? {
            FinalizedWitnessedClaimObservationTargetWire::Exact(wire) => {
                let ExactMode::Exact = wire.mode;
                Ok(Self::Exact {
                    claim_transaction_id: wire.claim_transaction_id,
                })
            }
            FinalizedWitnessedClaimObservationTargetWire::DiscoverByTerms(wire) => {
                let DiscoverByTermsMode::DiscoverByTerms = wire.mode;
                Ok(Self::DiscoverByTerms)
            }
        }
    }
}

/// Requests proof that one witnessed claim occurs exactly once in a finalized window.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct ObserveFinalizedWitnessedClaimRequest {
    /// Version, run, request, and destination-role binding.
    pub context: MessageContext,
    /// Expected pinned LEZ v0.2 runtime identity.
    pub runtime: RuntimeDescriptor,
    /// Complete agreement, including claimant destination and aggregate authority.
    pub terms: WitnessedNativeEscrowTerms,
    /// Exact unsigned message that the aggregate authority signed.
    pub claim: PreparedWitnessedClaim,
    /// Exact-ID lookup or peerless canonical discovery.
    pub target: FinalizedWitnessedClaimObservationTarget,
    /// Inclusive bounded range that must be entirely finalized and scanned.
    pub window: DiscoveryWindow,
}

impl ObserveFinalizedWitnessedClaimRequest {
    /// Creates one exact, bounded finalized witnessed-claim observation request.
    pub const fn new(
        context: MessageContext,
        runtime: RuntimeDescriptor,
        terms: WitnessedNativeEscrowTerms,
        claim: PreparedWitnessedClaim,
        claim_transaction_id: TransactionId,
        window: DiscoveryWindow,
    ) -> Self {
        Self {
            context,
            runtime,
            terms,
            claim,
            target: FinalizedWitnessedClaimObservationTarget::Exact {
                claim_transaction_id,
            },
            window,
        }
    }

    /// Creates a peerless terms-and-transcript discovery request.
    pub const fn discover_by_terms(
        context: MessageContext,
        runtime: RuntimeDescriptor,
        terms: WitnessedNativeEscrowTerms,
        claim: PreparedWitnessedClaim,
        window: DiscoveryWindow,
    ) -> Self {
        Self {
            context,
            runtime,
            terms,
            claim,
            target: FinalizedWitnessedClaimObservationTarget::DiscoverByTerms,
            window,
        }
    }
}

/// Exact decoded message and role bindings for `ClaimNativeWitnessed`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct WitnessedClaimInstructionFacts {
    /// Runtime escrow program targeted by the claim.
    pub program_id: Hex32,
    /// Exact `[metadata, custody, claimant_destination, aggregate_authority]` order.
    pub ordered_account_ids: AccountIds,
    /// Swap identifier encoded by `ClaimNativeWitnessed`.
    pub swap_id: Hex32,
    /// Fixed asset destination that does not supply the claim signature.
    pub claimant_account_id: Hex32,
    /// Sole public account whose aggregate key supplies the claim signature.
    pub aggregate_authority_account_id: Hex32,
    /// Canonical unsigned message and hash recovered from the found transaction.
    pub claim: PreparedWitnessedClaim,
}

impl WitnessedClaimInstructionFacts {
    /// Creates exact decoded witnessed-claim instruction and transcript facts.
    pub const fn new(
        program_id: Hex32,
        ordered_account_ids: AccountIds,
        swap_id: Hex32,
        claimant_account_id: Hex32,
        aggregate_authority_account_id: Hex32,
        claim: PreparedWitnessedClaim,
    ) -> Self {
        Self {
            program_id,
            ordered_account_ids,
            swap_id,
            claimant_account_id,
            aggregate_authority_account_id,
            claim,
        }
    }
}

/// Explicit identity of the containing finalized indexer block.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct FinalizedBlockIdentity {
    /// Official numeric indexer `BlockId` used for the by-ID lookup.
    pub block_id: u64,
    /// Exact block hash used for the independent by-hash lookup.
    pub block_hash: Hex32,
    /// Consensus-visible timestamp committed by that block.
    pub timestamp_ms: u64,
}

impl FinalizedBlockIdentity {
    /// Creates one explicit finalized containing-block identity.
    pub const fn new(block_id: u64, block_hash: Hex32, timestamp_ms: u64) -> Self {
        Self {
            block_id,
            block_hash,
            timestamp_ms,
        }
    }
}

/// Complete exact facts proving one witnessed claim in a finalized indexer block.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct FinalizedWitnessedClaimFacts {
    /// Canonical transaction bytes, hash, signer identity, and block position.
    pub transaction: ObservedTransactionFacts,
    /// Exact decoded claim message, instruction, destination, and authority facts.
    pub instruction: WitnessedClaimInstructionFacts,
    /// The only signature/witness in the canonical public transaction.
    pub aggregate_signature: AggregateBip340Signature,
    /// Identity and consensus timestamp of the containing finalized block.
    pub containing_block: FinalizedBlockIdentity,
    /// Terminal escrow metadata read at the exact containing finalized block.
    pub metadata: WitnessedEscrowMetadataFacts,
    /// Empty native custody read at the exact containing finalized block.
    pub custody: NativeCustodyFacts,
}

impl FinalizedWitnessedClaimFacts {
    /// Creates one complete finalized witnessed-claim proof bundle.
    pub const fn new(
        transaction: ObservedTransactionFacts,
        instruction: WitnessedClaimInstructionFacts,
        aggregate_signature: AggregateBip340Signature,
        containing_block: FinalizedBlockIdentity,
        metadata: WitnessedEscrowMetadataFacts,
        custody: NativeCustodyFacts,
    ) -> Self {
        Self {
            transaction,
            instruction,
            aggregate_signature,
            containing_block,
            metadata,
            custody,
        }
    }
}

/// Returns one exact witnessed claim only after the whole requested window is finalized.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct ObserveFinalizedWitnessedClaimResult {
    /// Echoed request context.
    pub context: MessageContext,
    /// Stable finalized indexer tip that completely covers the requested window.
    pub finalized_tip: ChainTip,
    /// Exact finalized claim facts; absence and pending states are errors.
    pub claim: FinalizedWitnessedClaimFacts,
}

impl ObserveFinalizedWitnessedClaimResult {
    /// Creates one exact finalized witnessed-claim result.
    pub const fn new(
        context: MessageContext,
        finalized_tip: ChainTip,
        claim: FinalizedWitnessedClaimFacts,
    ) -> Self {
        Self {
            context,
            finalized_tip,
            claim,
        }
    }
}

/// Exact outcome of one completely validated finalized witnessed-claim scan.
///
/// `NotFound` is a positive result only because its enclosing result also
/// carries the exact fully scanned window and the stable finalized tip that
/// covered it. Node, history, finality, or tip-stability failures are protocol
/// errors and can never be represented by this variant.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
#[must_use]
pub enum FinalizedWitnessedClaimScanOutcome {
    /// The canonical finalized scan contained the one exact validated claim.
    PresentExact {
        /// Complete exact finalized claim facts.
        claim: Box<FinalizedWitnessedClaimFacts>,
    },
    /// The complete stable finalized scan contained no matching claim.
    NotFound,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum PresentExactStatus {
    PresentExact,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum NotFoundStatus {
    NotFound,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PresentExactWitnessedClaimScanOutcomeWire {
    status: PresentExactStatus,
    claim: Box<FinalizedWitnessedClaimFacts>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NotFoundWitnessedClaimScanOutcomeWire {
    status: NotFoundStatus,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum FinalizedWitnessedClaimScanOutcomeWire {
    PresentExact(PresentExactWitnessedClaimScanOutcomeWire),
    NotFound(NotFoundWitnessedClaimScanOutcomeWire),
}

impl<'de> Deserialize<'de> for FinalizedWitnessedClaimScanOutcome {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match FinalizedWitnessedClaimScanOutcomeWire::deserialize(deserializer)? {
            FinalizedWitnessedClaimScanOutcomeWire::PresentExact(wire) => {
                let PresentExactStatus::PresentExact = wire.status;
                Ok(Self::PresentExact { claim: wire.claim })
            }
            FinalizedWitnessedClaimScanOutcomeWire::NotFound(wire) => {
                let NotFoundStatus::NotFound = wire.status;
                Ok(Self::NotFound)
            }
        }
    }
}

/// Result of the additive v1 exact witnessed-claim presence classifier.
///
/// Unlike [`ObserveFinalizedWitnessedClaimResult`], this type can prove absence.
/// The echoed window prevents a stale funding-era range from being silently
/// substituted for the fresh bounded range selected by the caller.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct ClassifyFinalizedWitnessedClaimResult {
    /// Echoed request context.
    pub context: MessageContext,
    /// Stable finalized indexer tip that completely covers `scanned_window`.
    pub finalized_tip: ChainTip,
    /// Exact inclusive bounded range that was completely scanned.
    pub scanned_window: DiscoveryWindow,
    /// Exact presence or definitive absence within that range.
    pub outcome: FinalizedWitnessedClaimScanOutcome,
}

impl ClassifyFinalizedWitnessedClaimResult {
    /// Creates exact present evidence for a completely scanned stable window.
    pub fn present_exact(
        context: MessageContext,
        finalized_tip: ChainTip,
        scanned_window: DiscoveryWindow,
        claim: FinalizedWitnessedClaimFacts,
    ) -> Self {
        Self {
            context,
            finalized_tip,
            scanned_window,
            outcome: FinalizedWitnessedClaimScanOutcome::PresentExact {
                claim: Box::new(claim),
            },
        }
    }

    /// Creates definitive absence evidence for a completely scanned stable window.
    pub const fn not_found(
        context: MessageContext,
        finalized_tip: ChainTip,
        scanned_window: DiscoveryWindow,
    ) -> Self {
        Self {
            context,
            finalized_tip,
            scanned_window,
            outcome: FinalizedWitnessedClaimScanOutcome::NotFound,
        }
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

/// Asset-specific transaction step in one additive v2 witnessed escrow plan.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[must_use]
pub enum WitnessedAssetPrepareStepV2 {
    /// `InitializeNativeWitnessed` or `InitializeTokenWitnessed`.
    InitializeWitnessed,
    /// Permissionless creation of exact `ATA(metadata, definition)` custody.
    CreateCustodyAta,
    /// `FundNative` or `FundToken` under depositor authority.
    Fund,
}

/// One exact prepared transaction in an asset-specific ordered plan.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct WitnessedAssetPreparedEffectV2 {
    /// Required semantic position in the plan.
    pub step: WitnessedAssetPrepareStepV2,
    /// Exact official transaction identity and bytes.
    pub transaction: PreparedTransaction,
}

impl WitnessedAssetPreparedEffectV2 {
    /// Creates one exact prepared effect.
    pub const fn new(step: WitnessedAssetPrepareStepV2, transaction: PreparedTransaction) -> Self {
        Self { step, transaction }
    }
}

/// Requests one complete ordered native or custom-token witnessed escrow plan.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct PrepareWitnessedAssetEscrowV2Request {
    /// Version, isolation, correlation, and depositor-role fields.
    pub context: MessageContext,
    /// Expected pinned LEZ v0.2 runtime identity.
    pub runtime: RuntimeDescriptor,
    /// Explicit v2 native-or-token terms.
    pub terms: WitnessedLezAssetTermsV2,
}

impl PrepareWitnessedAssetEscrowV2Request {
    /// Creates one additive v2 witnessed-asset preparation request.
    pub const fn new(
        context: MessageContext,
        runtime: RuntimeDescriptor,
        terms: WitnessedLezAssetTermsV2,
    ) -> Self {
        Self {
            context,
            runtime,
            terms,
        }
    }
}

/// Exact ordered native or custom-token witnessed escrow preparation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct PrepareWitnessedAssetEscrowV2Result {
    /// Echoed request context.
    pub context: MessageContext,
    /// Exact asset terms used to prepare every transaction.
    pub terms: WitnessedLezAssetTermsV2,
    /// Native `[initialize, fund]` or token `[initialize, create_custody_ata, fund]`.
    pub effects: Vec<WitnessedAssetPreparedEffectV2>,
}

impl PrepareWitnessedAssetEscrowV2Result {
    /// Validates and creates one complete ordered preparation result.
    ///
    /// # Errors
    ///
    /// Rejects missing, extra, duplicated, or reordered asset-specific effects.
    pub fn new(
        context: MessageContext,
        terms: WitnessedLezAssetTermsV2,
        effects: Vec<WitnessedAssetPreparedEffectV2>,
    ) -> Result<Self, ProtocolValueError> {
        validate_prepare_steps(&terms, effects.iter().map(|effect| effect.step))?;
        Ok(Self {
            context,
            terms,
            effects,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PrepareWitnessedAssetEscrowV2ResultWire {
    context: MessageContext,
    terms: WitnessedLezAssetTermsV2,
    effects: Vec<WitnessedAssetPreparedEffectV2>,
}

impl<'de> Deserialize<'de> for PrepareWitnessedAssetEscrowV2Result {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = PrepareWitnessedAssetEscrowV2ResultWire::deserialize(deserializer)?;
        Self::new(wire.context, wire.terms, wire.effects).map_err(serde::de::Error::custom)
    }
}

/// Primitive custom-token holding facts returned by the official decoder.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct TokenHoldingFactsV2 {
    /// Exact ATA identity.
    pub account_id: Hex32,
    /// Node-reported Token program owner.
    pub owner_program_id: Hex32,
    /// Fungible definition decoded from the holding.
    pub token_definition_account_id: Hex32,
    /// Full-width decoded fungible balance.
    pub balance: NativeAmount,
}

impl TokenHoldingFactsV2 {
    /// Creates exact primitive custom-token holding facts.
    pub const fn new(
        account_id: Hex32,
        owner_program_id: Hex32,
        token_definition_account_id: Hex32,
        balance: u128,
    ) -> Self {
        Self {
            account_id,
            owner_program_id,
            token_definition_account_id,
            balance: NativeAmount::new(balance),
        }
    }
}

/// Native custody or custom-token ATA holding facts for additive v2 observations.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "facts", rename_all = "snake_case")]
#[must_use]
pub enum WitnessedAssetCustodyFactsV2 {
    /// Native authenticated-transfer custody.
    Native(NativeCustodyFacts),
    /// Custom-token custody ATA holding.
    CustomToken(TokenHoldingFactsV2),
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum NativeCustodyKindV2 {
    Native,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum CustomTokenCustodyKindV2 {
    CustomToken,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeCustodyV2Wire {
    kind: NativeCustodyKindV2,
    facts: NativeCustodyFacts,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CustomTokenCustodyV2Wire {
    kind: CustomTokenCustodyKindV2,
    facts: TokenHoldingFactsV2,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum WitnessedAssetCustodyFactsV2Wire {
    Native(NativeCustodyV2Wire),
    CustomToken(CustomTokenCustodyV2Wire),
}

impl<'de> Deserialize<'de> for WitnessedAssetCustodyFactsV2 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match WitnessedAssetCustodyFactsV2Wire::deserialize(deserializer)? {
            WitnessedAssetCustodyFactsV2Wire::Native(wire) => {
                let NativeCustodyKindV2::Native = wire.kind;
                Ok(Self::Native(wire.facts))
            }
            WitnessedAssetCustodyFactsV2Wire::CustomToken(wire) => {
                let CustomTokenCustodyKindV2::CustomToken = wire.kind;
                Ok(Self::CustomToken(wire.facts))
            }
        }
    }
}

/// One observed transaction plus decoded top-level escrow instruction accounts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct WitnessedAssetObservedPrepareEffectV2 {
    /// Expected semantic position in the asset-specific plan.
    pub step: WitnessedAssetPrepareStepV2,
    /// Canonical transaction bytes, ID, signers, and chain position.
    pub transaction: ObservedTransactionFacts,
    /// Escrow program targeted by the decoded instruction.
    pub program_id: Hex32,
    /// Exact decoded instruction account order.
    pub ordered_account_ids: AccountIds,
}

impl WitnessedAssetObservedPrepareEffectV2 {
    /// Creates one decoded observed preparation effect.
    pub const fn new(
        step: WitnessedAssetPrepareStepV2,
        transaction: ObservedTransactionFacts,
        program_id: Hex32,
        ordered_account_ids: AccountIds,
    ) -> Self {
        Self {
            step,
            transaction,
            program_id,
            ordered_account_ids,
        }
    }
}

/// Requests exact observation of one previously prepared witnessed-asset plan.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct ObserveWitnessedAssetEscrowV2Request {
    /// Version, isolation, correlation, and role fields.
    pub context: MessageContext,
    /// Expected pinned LEZ v0.2 runtime identity.
    pub runtime: RuntimeDescriptor,
    /// Exact v2 native-or-token terms.
    pub terms: WitnessedLezAssetTermsV2,
    /// Exact prepared bytes/IDs in required asset-specific order.
    pub prepared_effects: Vec<WitnessedAssetPreparedEffectV2>,
    /// Inclusive bounded range to scan for every prepared effect.
    pub window: DiscoveryWindow,
}

impl ObserveWitnessedAssetEscrowV2Request {
    /// Validates and creates an exact prepared-plan observation request.
    ///
    /// # Errors
    ///
    /// Rejects a prepared transaction order inconsistent with the selected asset.
    pub fn new(
        context: MessageContext,
        runtime: RuntimeDescriptor,
        terms: WitnessedLezAssetTermsV2,
        prepared_effects: Vec<WitnessedAssetPreparedEffectV2>,
        window: DiscoveryWindow,
    ) -> Result<Self, ProtocolValueError> {
        validate_prepare_steps(&terms, prepared_effects.iter().map(|effect| effect.step))?;
        Ok(Self {
            context,
            runtime,
            terms,
            prepared_effects,
            window,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ObserveWitnessedAssetEscrowV2RequestWire {
    context: MessageContext,
    runtime: RuntimeDescriptor,
    terms: WitnessedLezAssetTermsV2,
    prepared_effects: Vec<WitnessedAssetPreparedEffectV2>,
    window: DiscoveryWindow,
}

impl<'de> Deserialize<'de> for ObserveWitnessedAssetEscrowV2Request {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ObserveWitnessedAssetEscrowV2RequestWire::deserialize(deserializer)?;
        Self::new(
            wire.context,
            wire.runtime,
            wire.terms,
            wire.prepared_effects,
            wire.window,
        )
        .map_err(serde::de::Error::custom)
    }
}

/// Stable-tip exact observations for every required witnessed-asset preparation effect.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct ObserveWitnessedAssetEscrowV2Result {
    /// Echoed request context.
    pub context: MessageContext,
    /// Terms validated against instructions, metadata, and custody.
    pub terms: WitnessedLezAssetTermsV2,
    /// Tip immediately before the canonical reads.
    pub tip_before: ChainTip,
    /// Exact observed effects in required asset-specific order.
    pub effects: Vec<WitnessedAssetObservedPrepareEffectV2>,
    /// Funded metadata read at the stable bracketed tip.
    pub metadata: WitnessedEscrowMetadataFacts,
    /// Exact funded native custody or token holding.
    pub custody: WitnessedAssetCustodyFactsV2,
    /// Tip immediately after the canonical reads.
    pub tip_after: ChainTip,
}

impl ObserveWitnessedAssetEscrowV2Result {
    /// Validates exact funded state and instruction account order.
    ///
    /// # Errors
    ///
    /// Rejects definition, ATA, authority, program, metadata, custody, or order drift.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        context: MessageContext,
        terms: WitnessedLezAssetTermsV2,
        tip_before: ChainTip,
        effects: Vec<WitnessedAssetObservedPrepareEffectV2>,
        metadata: WitnessedEscrowMetadataFacts,
        custody: WitnessedAssetCustodyFactsV2,
        tip_after: ChainTip,
    ) -> Result<Self, ProtocolValueError> {
        validate_asset_state(
            &terms,
            &metadata,
            &custody,
            Some(EscrowState::Funded),
            Some(asset_amount(&terms)),
        )?;
        validate_observed_prepare_effects(&terms, &effects, &metadata)?;
        Ok(Self {
            context,
            terms,
            tip_before,
            effects,
            metadata,
            custody,
            tip_after,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ObserveWitnessedAssetEscrowV2ResultWire {
    context: MessageContext,
    terms: WitnessedLezAssetTermsV2,
    tip_before: ChainTip,
    effects: Vec<WitnessedAssetObservedPrepareEffectV2>,
    metadata: WitnessedEscrowMetadataFacts,
    custody: WitnessedAssetCustodyFactsV2,
    tip_after: ChainTip,
}

impl<'de> Deserialize<'de> for ObserveWitnessedAssetEscrowV2Result {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ObserveWitnessedAssetEscrowV2ResultWire::deserialize(deserializer)?;
        Self::new(
            wire.context,
            wire.terms,
            wire.tip_before,
            wire.effects,
            wire.metadata,
            wire.custody,
            wire.tip_after,
        )
        .map_err(serde::de::Error::custom)
    }
}

/// Reserves one exact witnessed native-or-token claim transcript.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct PrepareWitnessedAssetClaimV2Request {
    /// Version, isolation, correlation, and destination-role fields.
    pub context: MessageContext,
    /// Expected pinned LEZ v0.2 runtime identity.
    pub runtime: RuntimeDescriptor,
    /// Exact v2 native-or-token agreement terms.
    pub terms: WitnessedLezAssetTermsV2,
    /// Exact funding transaction whose escrow is being claimed.
    pub funding_transaction_id: TransactionId,
}

impl PrepareWitnessedAssetClaimV2Request {
    /// Creates one witnessed-asset claim reservation request.
    pub const fn new(
        context: MessageContext,
        runtime: RuntimeDescriptor,
        terms: WitnessedLezAssetTermsV2,
        funding_transaction_id: TransactionId,
    ) -> Self {
        Self {
            context,
            runtime,
            terms,
            funding_transaction_id,
        }
    }
}

/// Exact unsigned witnessed-asset claim transcript.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct PrepareWitnessedAssetClaimV2Result {
    /// Echoed request context.
    pub context: MessageContext,
    /// Exact terms bound to the reserved transcript.
    pub terms: WitnessedLezAssetTermsV2,
    /// Reserved immutable unsigned claim transcript.
    pub claim: PreparedWitnessedClaim,
}

impl PrepareWitnessedAssetClaimV2Result {
    /// Creates one witnessed-asset claim reservation result.
    pub const fn new(
        context: MessageContext,
        terms: WitnessedLezAssetTermsV2,
        claim: PreparedWitnessedClaim,
    ) -> Self {
        Self {
            context,
            terms,
            claim,
        }
    }
}

/// Completes one exact witnessed-asset claim transcript.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct CompleteWitnessedAssetClaimV2Request {
    /// Version, isolation, correlation, and destination-role fields.
    pub context: MessageContext,
    /// Expected pinned LEZ v0.2 runtime identity.
    pub runtime: RuntimeDescriptor,
    /// Exact v2 native-or-token agreement terms.
    pub terms: WitnessedLezAssetTermsV2,
    /// Exact prepared transcript that was externally signed.
    pub claim: PreparedWitnessedClaim,
    /// Completed 64-byte aggregate BIP340 signature.
    pub aggregate_signature: AggregateBip340Signature,
}

impl CompleteWitnessedAssetClaimV2Request {
    /// Creates one witnessed-asset claim completion request.
    pub const fn new(
        context: MessageContext,
        runtime: RuntimeDescriptor,
        terms: WitnessedLezAssetTermsV2,
        claim: PreparedWitnessedClaim,
        aggregate_signature: AggregateBip340Signature,
    ) -> Self {
        Self {
            context,
            runtime,
            terms,
            claim,
            aggregate_signature,
        }
    }
}

/// Exact completed witnessed-asset public transaction.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct CompleteWitnessedAssetClaimV2Result {
    /// Echoed request context.
    pub context: MessageContext,
    /// Exact terms bound to the completed transaction.
    pub terms: WitnessedLezAssetTermsV2,
    /// Canonical signed public transaction prepared by the official sidecar.
    pub claim: PreparedTransaction,
}

impl CompleteWitnessedAssetClaimV2Result {
    /// Creates one completed witnessed-asset claim result.
    pub const fn new(
        context: MessageContext,
        terms: WitnessedLezAssetTermsV2,
        claim: PreparedTransaction,
    ) -> Self {
        Self {
            context,
            terms,
            claim,
        }
    }
}

/// Requests one exact finalized witnessed-asset claim observation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct ObserveFinalizedWitnessedAssetClaimV2Request {
    /// Version, isolation, correlation, and destination-role fields.
    pub context: MessageContext,
    /// Expected pinned LEZ v0.2 runtime identity.
    pub runtime: RuntimeDescriptor,
    /// Exact v2 native-or-token agreement terms.
    pub terms: WitnessedLezAssetTermsV2,
    /// Exact unsigned transcript completed by the observed transaction.
    pub claim: PreparedWitnessedClaim,
    /// Exact-ID lookup or peerless canonical discovery.
    pub target: FinalizedWitnessedClaimObservationTarget,
    /// Inclusive bounded range that must be entirely finalized and scanned.
    pub window: DiscoveryWindow,
}

impl ObserveFinalizedWitnessedAssetClaimV2Request {
    /// Creates one exact bounded witnessed-asset claim observation request.
    pub const fn new(
        context: MessageContext,
        runtime: RuntimeDescriptor,
        terms: WitnessedLezAssetTermsV2,
        claim: PreparedWitnessedClaim,
        claim_transaction_id: TransactionId,
        window: DiscoveryWindow,
    ) -> Self {
        Self {
            context,
            runtime,
            terms,
            claim,
            target: FinalizedWitnessedClaimObservationTarget::Exact {
                claim_transaction_id,
            },
            window,
        }
    }

    /// Creates one bounded terms-and-transcript discovery request.
    pub const fn discover_by_terms(
        context: MessageContext,
        runtime: RuntimeDescriptor,
        terms: WitnessedLezAssetTermsV2,
        claim: PreparedWitnessedClaim,
        window: DiscoveryWindow,
    ) -> Self {
        Self {
            context,
            runtime,
            terms,
            claim,
            target: FinalizedWitnessedClaimObservationTarget::DiscoverByTerms,
            window,
        }
    }
}

/// Exact decoded witnessed native-or-token claim instruction facts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct WitnessedAssetClaimInstructionFactsV2 {
    /// Runtime escrow program targeted by the claim.
    pub program_id: Hex32,
    /// Exact native or custom-token claim account order.
    pub ordered_account_ids: AccountIds,
    /// Swap identifier encoded by the witnessed claim.
    pub swap_id: Hex32,
    /// Canonical unsigned message and hash recovered from the transaction.
    pub claim: PreparedWitnessedClaim,
}

impl WitnessedAssetClaimInstructionFactsV2 {
    /// Creates exact decoded witnessed-asset claim instruction facts.
    pub const fn new(
        program_id: Hex32,
        ordered_account_ids: AccountIds,
        swap_id: Hex32,
        claim: PreparedWitnessedClaim,
    ) -> Self {
        Self {
            program_id,
            ordered_account_ids,
            swap_id,
            claim,
        }
    }
}

/// Complete facts proving one witnessed native-or-token claim in a finalized block.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct FinalizedWitnessedAssetClaimFactsV2 {
    /// Canonical transaction bytes, hash, signer identity, and block position.
    pub transaction: ObservedTransactionFacts,
    /// Exact decoded program, account order, swap, and claim transcript.
    pub instruction: WitnessedAssetClaimInstructionFactsV2,
    /// The aggregate signature carried by the canonical public transaction.
    pub aggregate_signature: AggregateBip340Signature,
    /// Identity and consensus timestamp of the containing finalized block.
    pub containing_block: FinalizedBlockIdentity,
    /// Terminal metadata read at the containing finalized block.
    pub metadata: WitnessedEscrowMetadataFacts,
    /// Empty native custody or custom-token holding at that block.
    pub custody: WitnessedAssetCustodyFactsV2,
}

impl FinalizedWitnessedAssetClaimFactsV2 {
    /// Creates one complete finalized witnessed-asset claim proof bundle.
    pub const fn new(
        transaction: ObservedTransactionFacts,
        instruction: WitnessedAssetClaimInstructionFactsV2,
        aggregate_signature: AggregateBip340Signature,
        containing_block: FinalizedBlockIdentity,
        metadata: WitnessedEscrowMetadataFacts,
        custody: WitnessedAssetCustodyFactsV2,
    ) -> Self {
        Self {
            transaction,
            instruction,
            aggregate_signature,
            containing_block,
            metadata,
            custody,
        }
    }
}

/// Exact finalized witnessed-asset claim result with cross-field validation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct ObserveFinalizedWitnessedAssetClaimV2Result {
    /// Echoed request context.
    pub context: MessageContext,
    /// Exact terms validated against claim, metadata, and custody facts.
    pub terms: WitnessedLezAssetTermsV2,
    /// Stable finalized tip that completely covers the requested window.
    pub finalized_tip: ChainTip,
    /// Exact finalized witnessed-asset claim proof bundle.
    pub claim: FinalizedWitnessedAssetClaimFactsV2,
}

impl ObserveFinalizedWitnessedAssetClaimV2Result {
    /// Validates and creates a finalized witnessed-asset claim result.
    ///
    /// # Errors
    ///
    /// Rejects definition, destination, authority, program, state, or order drift.
    pub fn new(
        context: MessageContext,
        terms: WitnessedLezAssetTermsV2,
        finalized_tip: ChainTip,
        claim: FinalizedWitnessedAssetClaimFactsV2,
    ) -> Result<Self, ProtocolValueError> {
        validate_asset_state(
            &terms,
            &claim.metadata,
            &claim.custody,
            Some(EscrowState::Claimed),
            Some(NativeAmount::new(0)),
        )?;
        validate_claim_facts(&terms, &claim.instruction, &claim.metadata)?;
        Ok(Self {
            context,
            terms,
            finalized_tip,
            claim,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ObserveFinalizedWitnessedAssetClaimV2ResultWire {
    context: MessageContext,
    terms: WitnessedLezAssetTermsV2,
    finalized_tip: ChainTip,
    claim: FinalizedWitnessedAssetClaimFactsV2,
}

impl<'de> Deserialize<'de> for ObserveFinalizedWitnessedAssetClaimV2Result {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ObserveFinalizedWitnessedAssetClaimV2ResultWire::deserialize(deserializer)?;
        Self::new(wire.context, wire.terms, wire.finalized_tip, wire.claim)
            .map_err(serde::de::Error::custom)
    }
}

/// Requests one fixed-destination witnessed native-or-token refund transaction.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct PrepareWitnessedAssetRefundV2Request {
    /// Version, isolation, correlation, and depositor-role fields.
    pub context: MessageContext,
    /// Expected pinned LEZ v0.2 runtime identity.
    pub runtime: RuntimeDescriptor,
    /// Exact v2 native-or-token agreement terms.
    pub terms: WitnessedLezAssetTermsV2,
}

impl PrepareWitnessedAssetRefundV2Request {
    /// Creates one witnessed-asset refund preparation request.
    pub const fn new(
        context: MessageContext,
        runtime: RuntimeDescriptor,
        terms: WitnessedLezAssetTermsV2,
    ) -> Self {
        Self {
            context,
            runtime,
            terms,
        }
    }
}

/// Exact fixed-destination witnessed-asset refund transaction.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct PrepareWitnessedAssetRefundV2Result {
    /// Echoed request context.
    pub context: MessageContext,
    /// Exact terms bound to the prepared transaction.
    pub terms: WitnessedLezAssetTermsV2,
    /// Canonical refund transaction prepared by the official sidecar.
    pub refund: PreparedTransaction,
}

impl PrepareWitnessedAssetRefundV2Result {
    /// Creates one witnessed-asset refund preparation result.
    pub const fn new(
        context: MessageContext,
        terms: WitnessedLezAssetTermsV2,
        refund: PreparedTransaction,
    ) -> Self {
        Self {
            context,
            terms,
            refund,
        }
    }
}

/// Requests canonical witnessed-asset state and an optional refund observation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct ObserveWitnessedAssetRefundV2Request {
    /// Version, isolation, correlation, and role fields.
    pub context: MessageContext,
    /// Expected pinned LEZ v0.2 runtime identity.
    pub runtime: RuntimeDescriptor,
    /// Exact v2 native-or-token agreement terms.
    pub terms: WitnessedLezAssetTermsV2,
    /// Account-only, exact-ID, or terms-discovery observation mode.
    pub target: NativeRefundObservationTarget,
}

impl ObserveWitnessedAssetRefundV2Request {
    /// Creates one witnessed-asset refund observation request.
    pub const fn new(
        context: MessageContext,
        runtime: RuntimeDescriptor,
        terms: WitnessedLezAssetTermsV2,
        target: NativeRefundObservationTarget,
    ) -> Self {
        Self {
            context,
            runtime,
            terms,
            target,
        }
    }
}

/// Exact decoded witnessed native-or-token refund instruction facts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct WitnessedAssetRefundInstructionFactsV2 {
    /// Runtime escrow program targeted by the refund.
    pub program_id: Hex32,
    /// Exact native or custom-token refund account order.
    pub ordered_account_ids: AccountIds,
    /// Swap identifier encoded by the refund instruction.
    pub swap_id: Hex32,
}

impl WitnessedAssetRefundInstructionFactsV2 {
    /// Creates exact decoded witnessed-asset refund instruction facts.
    pub const fn new(program_id: Hex32, ordered_account_ids: AccountIds, swap_id: Hex32) -> Self {
        Self {
            program_id,
            ordered_account_ids,
            swap_id,
        }
    }
}

/// Complete primitive facts for one found witnessed-asset refund transaction.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct WitnessedAssetRefundFoundFactsV2 {
    /// Canonical transaction bytes, ID, signers, and chain position.
    pub transaction: ObservedTransactionFacts,
    /// Exact decoded refund program, account order, and swap identity.
    pub instruction: WitnessedAssetRefundInstructionFactsV2,
}

impl WitnessedAssetRefundFoundFactsV2 {
    /// Creates one complete witnessed-asset refund fact bundle.
    pub const fn new(
        transaction: ObservedTransactionFacts,
        instruction: WitnessedAssetRefundInstructionFactsV2,
    ) -> Self {
        Self {
            transaction,
            instruction,
        }
    }
}

/// Witnessed-asset refund lookup without overstating absence or finality.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", content = "facts", rename_all = "snake_case")]
#[must_use]
pub enum WitnessedAssetRefundObservationV2 {
    /// Account state only was requested; no refund lookup occurred.
    NotRequested,
    /// Stable tips fully covered the caller's declared window.
    Absent,
    /// Upstream cannot distinguish pending from absent for the lookup.
    UnknownOrPending,
    /// The sidecar found and decoded one refund transaction.
    Found(Box<WitnessedAssetRefundFoundFactsV2>),
}

impl WitnessedAssetRefundObservationV2 {
    /// Wraps complete found facts without inflating absent results.
    pub fn found(facts: WitnessedAssetRefundFoundFactsV2) -> Self {
        Self::Found(Box::new(facts))
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum WitnessedAssetRefundObservationV2Wire {
    NotRequested(NotRequestedObservationWire),
    Absent(AbsentObservationWire),
    UnknownOrPending(UnknownOrPendingObservationWire),
    Found(Box<FoundObservationWire<WitnessedAssetRefundFoundFactsV2>>),
}

impl<'de> Deserialize<'de> for WitnessedAssetRefundObservationV2 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match WitnessedAssetRefundObservationV2Wire::deserialize(deserializer)? {
            WitnessedAssetRefundObservationV2Wire::NotRequested(wire) => {
                let NotRequestedStatus::NotRequested = wire.status;
                Ok(Self::NotRequested)
            }
            WitnessedAssetRefundObservationV2Wire::Absent(wire) => {
                let AbsentStatus::Absent = wire.status;
                Ok(Self::Absent)
            }
            WitnessedAssetRefundObservationV2Wire::UnknownOrPending(wire) => {
                let UnknownOrPendingStatus::UnknownOrPending = wire.status;
                Ok(Self::UnknownOrPending)
            }
            WitnessedAssetRefundObservationV2Wire::Found(wire) => {
                let FoundStatus::Found = wire.status;
                Ok(Self::found(wire.facts))
            }
        }
    }
}

/// Stable-clock witnessed-asset state and optional refund evidence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct ObserveWitnessedAssetRefundV2Result {
    /// Echoed request context.
    pub context: MessageContext,
    /// Exact terms validated against metadata, custody, and refund facts.
    pub terms: WitnessedLezAssetTermsV2,
    /// Canonical clock immediately before account and transaction reads.
    pub clock_before: ChainClock,
    /// Current metadata read at the stable bracketed clock.
    pub metadata: WitnessedEscrowMetadataFacts,
    /// Current native custody or custom-token holding.
    pub custody: WitnessedAssetCustodyFactsV2,
    /// Explicit refund lookup state and any complete found facts.
    pub refund: WitnessedAssetRefundObservationV2,
    /// Canonical clock immediately after all node reads.
    pub clock_after: ChainClock,
}

impl ObserveWitnessedAssetRefundV2Result {
    /// Validates and creates one witnessed-asset refund observation result.
    ///
    /// # Errors
    ///
    /// Rejects definition, destination, program, state, or account-order drift.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        context: MessageContext,
        terms: WitnessedLezAssetTermsV2,
        clock_before: ChainClock,
        metadata: WitnessedEscrowMetadataFacts,
        custody: WitnessedAssetCustodyFactsV2,
        refund: WitnessedAssetRefundObservationV2,
        clock_after: ChainClock,
    ) -> Result<Self, ProtocolValueError> {
        let found = matches!(refund, WitnessedAssetRefundObservationV2::Found(_));
        validate_asset_state(
            &terms,
            &metadata,
            &custody,
            found.then_some(EscrowState::Refunded),
            found.then_some(NativeAmount::new(0)),
        )?;
        if let WitnessedAssetRefundObservationV2::Found(facts) = &refund {
            validate_refund_facts(&terms, &facts.instruction, &metadata)?;
        }
        Ok(Self {
            context,
            terms,
            clock_before,
            metadata,
            custody,
            refund,
            clock_after,
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ObserveWitnessedAssetRefundV2ResultWire {
    context: MessageContext,
    terms: WitnessedLezAssetTermsV2,
    clock_before: ChainClock,
    metadata: WitnessedEscrowMetadataFacts,
    custody: WitnessedAssetCustodyFactsV2,
    refund: WitnessedAssetRefundObservationV2,
    clock_after: ChainClock,
}

impl<'de> Deserialize<'de> for ObserveWitnessedAssetRefundV2Result {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ObserveWitnessedAssetRefundV2ResultWire::deserialize(deserializer)?;
        Self::new(
            wire.context,
            wire.terms,
            wire.clock_before,
            wire.metadata,
            wire.custody,
            wire.refund,
            wire.clock_after,
        )
        .map_err(serde::de::Error::custom)
    }
}

fn validate_prepare_steps(
    terms: &WitnessedLezAssetTermsV2,
    actual: impl Iterator<Item = WitnessedAssetPrepareStepV2>,
) -> Result<(), ProtocolValueError> {
    const NATIVE: [WitnessedAssetPrepareStepV2; 2] = [
        WitnessedAssetPrepareStepV2::InitializeWitnessed,
        WitnessedAssetPrepareStepV2::Fund,
    ];
    const TOKEN: [WitnessedAssetPrepareStepV2; 3] = [
        WitnessedAssetPrepareStepV2::InitializeWitnessed,
        WitnessedAssetPrepareStepV2::CreateCustodyAta,
        WitnessedAssetPrepareStepV2::Fund,
    ];
    let expected = match terms.asset() {
        WitnessedLezAssetV2::Native(_) => NATIVE.as_slice(),
        WitnessedLezAssetV2::CustomToken(_) => TOKEN.as_slice(),
    };
    if actual.eq(expected.iter().copied()) {
        Ok(())
    } else {
        Err(ProtocolValueError::InvalidWitnessedAssetPrepareEffects)
    }
}

fn asset_amount(terms: &WitnessedLezAssetTermsV2) -> NativeAmount {
    match terms.asset() {
        WitnessedLezAssetV2::Native(terms) => terms.amount(),
        WitnessedLezAssetV2::CustomToken(terms) => terms.amount(),
    }
}

fn validate_asset_state(
    terms: &WitnessedLezAssetTermsV2,
    metadata: &WitnessedEscrowMetadataFacts,
    custody: &WitnessedAssetCustodyFactsV2,
    expected_status: Option<EscrowState>,
    expected_balance: Option<NativeAmount>,
) -> Result<(), ProtocolValueError> {
    ensure(metadata.version == 2, "metadata version")?;
    if let Some(status) = expected_status {
        ensure(metadata.status == status, "metadata status")?;
    }

    match (terms.asset(), custody) {
        (WitnessedLezAssetV2::Native(terms), WitnessedAssetCustodyFactsV2::Native(custody)) => {
            validate_native_asset_state(terms, metadata, custody, expected_balance)
        }
        (
            WitnessedLezAssetV2::CustomToken(terms),
            WitnessedAssetCustodyFactsV2::CustomToken(custody),
        ) => validate_token_asset_state(terms, metadata, custody, expected_balance),
        _ => Err(ProtocolValueError::WitnessedAssetFactsMismatch(
            "asset kind",
        )),
    }
}

fn validate_native_asset_state(
    terms: &WitnessedNativeEscrowTerms,
    metadata: &WitnessedEscrowMetadataFacts,
    custody: &NativeCustodyFacts,
    expected_balance: Option<NativeAmount>,
) -> Result<(), ProtocolValueError> {
    ensure(metadata.swap_id == terms.swap_id(), "swap id")?;
    ensure(metadata.terms_hash == terms.terms_hash(), "terms hash")?;
    ensure(
        metadata.aggregate_authority_account_id == terms.aggregate_authority_account_id(),
        "aggregate authority account",
    )?;
    ensure(
        metadata.aggregate_x_only_public_key == terms.aggregate_x_only_public_key(),
        "aggregate public key",
    )?;
    ensure(
        metadata.depositor_account_id == terms.depositor_account_id()
            && metadata.depositor_asset_account_id == terms.depositor_account_id(),
        "native depositor account",
    )?;
    ensure(
        metadata.claimant_account_id == terms.claimant_account_id()
            && metadata.claimant_asset_account_id == terms.claimant_account_id(),
        "native claimant account",
    )?;
    ensure(
        metadata.asset_program_id == terms.authenticated_transfer_program_id()
            && metadata.custody_program_id == terms.authenticated_transfer_program_id(),
        "native program",
    )?;
    ensure(
        metadata.asset_definition == Hex32::from_bytes([0; 32]),
        "native asset definition",
    )?;
    ensure(metadata.amount == terms.amount(), "native amount")?;
    ensure(
        metadata.refund_at_ms == terms.refund_at_ms(),
        "native refund time",
    )?;
    ensure(
        metadata.custody_account_id == custody.account_id,
        "native custody account",
    )?;
    ensure(
        custody.owner_program_id == terms.authenticated_transfer_program_id(),
        "native custody owner",
    )?;
    if let Some(balance) = expected_balance {
        ensure(custody.balance == balance, "native custody balance")?;
    }
    Ok(())
}

fn validate_token_asset_state(
    terms: &WitnessedTokenEscrowTermsV2,
    metadata: &WitnessedEscrowMetadataFacts,
    custody: &TokenHoldingFactsV2,
    expected_balance: Option<NativeAmount>,
) -> Result<(), ProtocolValueError> {
    ensure(metadata.swap_id == terms.swap_id(), "swap id")?;
    ensure(metadata.terms_hash == terms.terms_hash(), "terms hash")?;
    ensure(
        metadata.aggregate_authority_account_id == terms.aggregate_authority_account_id(),
        "aggregate authority account",
    )?;
    ensure(
        metadata.aggregate_x_only_public_key == terms.aggregate_x_only_public_key(),
        "aggregate public key",
    )?;
    ensure(
        metadata.depositor_account_id == terms.depositor_owner_account_id()
            && metadata.depositor_asset_account_id == terms.depositor_ata_account_id(),
        "token depositor accounts",
    )?;
    ensure(
        metadata.claimant_account_id == terms.claimant_owner_account_id()
            && metadata.claimant_asset_account_id == terms.claimant_ata_account_id(),
        "token claimant accounts",
    )?;
    ensure(
        metadata.custody_account_id == terms.custody_ata_account_id()
            && custody.account_id == terms.custody_ata_account_id(),
        "token custody ATA",
    )?;
    ensure(
        metadata.asset_program_id == terms.token_program_id()
            && custody.owner_program_id == terms.token_program_id(),
        "token program",
    )?;
    ensure(
        metadata.custody_program_id == terms.ata_program_id(),
        "ATA program",
    )?;
    ensure(
        metadata.asset_definition == terms.token_definition_account_id()
            && custody.token_definition_account_id == terms.token_definition_account_id(),
        "token definition",
    )?;
    ensure(metadata.amount == terms.amount(), "token amount")?;
    ensure(
        metadata.refund_at_ms == terms.refund_at_ms(),
        "token refund time",
    )?;
    if let Some(balance) = expected_balance {
        ensure(custody.balance == balance, "token custody balance")?;
    }
    Ok(())
}

fn validate_observed_prepare_effects(
    terms: &WitnessedLezAssetTermsV2,
    effects: &[WitnessedAssetObservedPrepareEffectV2],
    metadata: &WitnessedEscrowMetadataFacts,
) -> Result<(), ProtocolValueError> {
    validate_prepare_steps(terms, effects.iter().map(|effect| effect.step))?;
    for effect in effects {
        ensure(
            effect.program_id == metadata.owner_program_id,
            "prepare program",
        )?;
    }
    let expected: Vec<Vec<Hex32>> = match terms.asset() {
        WitnessedLezAssetV2::Native(terms) => vec![
            vec![
                metadata.account_id,
                metadata.custody_account_id,
                terms.depositor_account_id(),
                terms.claimant_account_id(),
                terms.aggregate_authority_account_id(),
            ],
            vec![
                metadata.account_id,
                metadata.custody_account_id,
                terms.depositor_account_id(),
            ],
        ],
        WitnessedLezAssetV2::CustomToken(terms) => vec![
            vec![
                metadata.account_id,
                terms.depositor_owner_account_id(),
                terms.claimant_owner_account_id(),
                terms.token_definition_account_id(),
                terms.aggregate_authority_account_id(),
            ],
            vec![
                metadata.account_id,
                terms.token_definition_account_id(),
                terms.custody_ata_account_id(),
            ],
            vec![
                metadata.account_id,
                terms.depositor_owner_account_id(),
                terms.depositor_ata_account_id(),
                terms.custody_ata_account_id(),
            ],
        ],
    };
    ensure(
        effects
            .iter()
            .zip(expected)
            .all(|(effect, expected)| effect.ordered_account_ids.as_slice() == expected),
        "prepare account order",
    )
}

fn validate_claim_facts(
    terms: &WitnessedLezAssetTermsV2,
    instruction: &WitnessedAssetClaimInstructionFactsV2,
    metadata: &WitnessedEscrowMetadataFacts,
) -> Result<(), ProtocolValueError> {
    ensure(
        instruction.program_id == metadata.owner_program_id,
        "claim program",
    )?;
    ensure(instruction.swap_id == metadata.swap_id, "claim swap id")?;
    let expected = match terms.asset() {
        WitnessedLezAssetV2::Native(terms) => vec![
            metadata.account_id,
            metadata.custody_account_id,
            terms.claimant_account_id(),
            terms.aggregate_authority_account_id(),
        ],
        WitnessedLezAssetV2::CustomToken(terms) => vec![
            metadata.account_id,
            terms.custody_ata_account_id(),
            terms.claimant_owner_account_id(),
            terms.claimant_ata_account_id(),
            terms.aggregate_authority_account_id(),
        ],
    };
    ensure(
        instruction.ordered_account_ids.as_slice() == expected,
        "claim account order",
    )
}

fn validate_refund_facts(
    terms: &WitnessedLezAssetTermsV2,
    instruction: &WitnessedAssetRefundInstructionFactsV2,
    metadata: &WitnessedEscrowMetadataFacts,
) -> Result<(), ProtocolValueError> {
    ensure(
        instruction.program_id == metadata.owner_program_id,
        "refund program",
    )?;
    ensure(instruction.swap_id == metadata.swap_id, "refund swap id")?;
    let expected = match terms.asset() {
        WitnessedLezAssetV2::Native(terms) => vec![
            metadata.account_id,
            metadata.custody_account_id,
            terms.depositor_account_id(),
        ],
        WitnessedLezAssetV2::CustomToken(terms) => vec![
            metadata.account_id,
            terms.custody_ata_account_id(),
            terms.depositor_ata_account_id(),
        ],
    };
    ensure(
        instruction.ordered_account_ids.as_slice() == expected,
        "refund account order",
    )
}

fn ensure(condition: bool, field: &'static str) -> Result<(), ProtocolValueError> {
    if condition {
        Ok(())
    } else {
        Err(ProtocolValueError::WitnessedAssetFactsMismatch(field))
    }
}
