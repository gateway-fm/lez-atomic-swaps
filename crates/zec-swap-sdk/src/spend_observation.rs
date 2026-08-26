//! Validation and classification of BIP-199 Zcash spends.
//!
//! Consensus recognition intentionally uses pinned Zebra 5.2.0 script flags, while SDK wallet
//! conventions are reported separately. This one-shot observer does not provide durable reorg
//! removal/persistence. The SDK orchestration layer must derive [`ExpectedBip199Spend`] from a
//! validated agreement and canonical funding observation, and a future multi-input non-ACP
//! adapter must supply every prevout amount/script required by ZIP-244.

use std::{io::Cursor, num::NonZeroU32};

use orchard::bundle as orchard_bundle;
use sapling::bundle as sapling_bundle;
use secp256k1::{PublicKey, ecdsa::Signature};
use sha2::{Digest, Sha256};
use zcash_primitives::{
    block::BlockHash,
    transaction::{
        Authorization as TransactionAuthorization, Transaction,
        sighash::{SignableInput as TransactionSignableInput, signature_hash},
        txid::TxIdDigester,
    },
};
use zcash_protocol::{
    TxId,
    consensus::{BlockHeight, BranchId, NetworkType},
    value::Zatoshis,
};
use zcash_script::{
    Opcode,
    interpreter::{CallbackTransactionSignatureChecker, Flags},
    opcode::PossiblyBad,
    script::{Code, Raw},
    signature::{HashType, SignedOutputs},
};
use zcash_transparent::{
    address::{Script, TransparentAddress},
    bundle::{Authorization as TransparentAuthorization, Bundle, OutPoint, TxIn, TxOut},
    sighash::{
        SighashType, SignableInput as TransparentSignableInput, TransparentAuthorizingContext,
    },
};

use crate::{Bip199Contract, ObservationError, TransparentSpendRequest, ZcashStableTip};

/// Pinned Zebra 5.2.0 maximum serialized block/transaction byte budget.
pub const ZEBRA_MAX_BLOCK_BYTES: usize = 2_000_000;

/// Pinned `zcash_script` consensus maximum for one script.
pub const ZCASH_MAX_SCRIPT_BYTES: usize = 10_000;

/// Immutable contract output that an observed transaction must spend.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpectedBip199Spend {
    network: NetworkType,
    consensus_branch_id: BranchId,
    outpoint: OutPoint,
    funding_output: TxOut,
    contract: Bip199Contract,
    sdk_policy: Option<ExpectedSdkCanonicalPolicy>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ExpectedSdkCanonicalPolicy {
    output: TxOut,
    expiry_height: BlockHeight,
}

impl ExpectedBip199Spend {
    /// Creates expected spend terms after checking the fetched output against the contract.
    ///
    /// # Errors
    ///
    /// Returns [`SpendObservationError::FundingScriptMismatch`] when `funding_output`
    /// is not the exact P2SH commitment for `contract`.
    pub fn new(
        network: NetworkType,
        consensus_branch_id: BranchId,
        outpoint: OutPoint,
        funding_output: TxOut,
        contract: Bip199Contract,
    ) -> Result<Self, SpendObservationError> {
        if funding_output.script_pubkey().0.0 != contract.p2sh_script_pubkey() {
            return Err(SpendObservationError::FundingScriptMismatch);
        }
        Ok(Self {
            network,
            consensus_branch_id,
            outpoint,
            funding_output,
            contract,
            sdk_policy: None,
        })
    }

    /// Creates expected terms from the SDK's canonical one-input/one-output request.
    ///
    /// This binds policy reporting to the destination, fee-derived output value, and expiry
    /// selected by the transaction request. Agreement-to-request derivation remains an SDK
    /// orchestration responsibility rather than an observer assumption.
    ///
    /// # Errors
    ///
    /// Returns an immutable-binding error when the request does not spend the supplied contract
    /// output or is incompatible with the selected consensus branch.
    pub fn from_request(
        network: NetworkType,
        contract: Bip199Contract,
        request: &TransparentSpendRequest,
    ) -> Result<Self, SpendObservationError> {
        let mut expected = Self::new(
            network,
            request.consensus_branch_id(),
            request.prevout().clone(),
            request.funding_output().clone(),
            contract,
        )?;
        let output_value = (request.funding_output().value() - request.fee())
            .ok_or(SpendObservationError::ExpectedPolicyValueOverflow)?;
        expected.sdk_policy = Some(ExpectedSdkCanonicalPolicy {
            output: TxOut::new(output_value, request.destination().script().into()),
            expiry_height: request.expiry_height(),
        });
        Ok(expected)
    }

    /// Exact BIP-199 contract whose branch must execute.
    #[must_use]
    pub const fn contract(&self) -> &Bip199Contract {
        &self.contract
    }

    /// Exact transparent outpoint the spend must consume.
    #[must_use]
    pub const fn outpoint(&self) -> &OutPoint {
        &self.outpoint
    }

    /// Fetched output supplying the amount and script committed by ZIP-244.
    #[must_use]
    pub const fn funding_output(&self) -> &TxOut {
        &self.funding_output
    }
}

/// One untrusted, stable-query attempt for a transaction that spends a BIP-199 output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ZcashSpendNodeSnapshot {
    network: NetworkType,
    consensus_branch_id: BranchId,
    in_active_chain: bool,
    transaction_block_hash: BlockHash,
    canonical_block_hash: BlockHash,
    block_height: BlockHeight,
    tip: ZcashStableTip,
    reported_transaction_id: TxId,
    raw_transaction: Vec<u8>,
    reported_confirmations: u32,
}

impl ZcashSpendNodeSnapshot {
    /// Creates an untrusted spend snapshot assembled by the node adapter.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        network: NetworkType,
        consensus_branch_id: BranchId,
        in_active_chain: bool,
        transaction_block_hash: BlockHash,
        canonical_block_hash: BlockHash,
        block_height: BlockHeight,
        tip: ZcashStableTip,
        reported_transaction_id: TxId,
        raw_transaction: Vec<u8>,
        reported_confirmations: u32,
    ) -> Self {
        Self {
            network,
            consensus_branch_id,
            in_active_chain,
            transaction_block_hash,
            canonical_block_hash,
            block_height,
            tip,
            reported_transaction_id,
            raw_transaction,
            reported_confirmations,
        }
    }

    /// Replaces the untrusted active-chain flag during snapshot assembly/testing.
    pub const fn set_in_active_chain(&mut self, value: bool) {
        self.in_active_chain = value;
    }

    /// Replaces the untrusted network during snapshot assembly/testing.
    pub const fn set_network(&mut self, value: NetworkType) {
        self.network = value;
    }

    /// Replaces the untrusted consensus branch during snapshot assembly/testing.
    pub const fn set_consensus_branch_id(&mut self, value: BranchId) {
        self.consensus_branch_id = value;
    }

    /// Replaces the canonical height-lookup hash during snapshot assembly/testing.
    pub const fn set_canonical_block_hash(&mut self, value: BlockHash) {
        self.canonical_block_hash = value;
    }

    /// Replaces the untrusted inclusion height during snapshot assembly/testing.
    pub const fn set_block_height(&mut self, value: BlockHeight) {
        self.block_height = value;
    }

    /// Replaces the second best-chain tip sample during snapshot assembly/testing.
    pub const fn set_tip_after(&mut self, hash: BlockHash, height: BlockHeight) {
        self.tip.set_after(hash, height);
    }

    /// Replaces the RPC-reported transaction identifier during snapshot assembly/testing.
    pub const fn set_reported_transaction_id(&mut self, value: TxId) {
        self.reported_transaction_id = value;
    }

    /// Replaces the RPC-reported confirmation count during snapshot assembly/testing.
    pub const fn set_reported_confirmations(&mut self, value: u32) {
        self.reported_confirmations = value;
    }
}

/// Which immutable BIP-199 branch an observed input validly executed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Bip199SpendKind {
    /// The claimant supplied the negotiated secret and claimant key.
    Claim {
        /// Secret revealed by the canonical claim input.
        preimage: Box<[u8]>,
        /// Exact compressed claimant public key used by `CHECKSIG`.
        claimant_public_key: [u8; 33],
    },
    /// The refund key spent through the absolute-lock-time branch.
    Refund {
        /// Exact compressed refund public key used by `CHECKSIG`.
        refund_public_key: [u8; 33],
    },
}

/// One way a consensus-valid spend differs from the SDK's canonical transaction policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SdkCanonicalSpendDeviation {
    /// The stack is semantically valid but is not the SDK's exact branch encoding.
    NonCanonicalScriptSig,
    /// One or more pushes use a consensus-valid non-minimal encoding.
    NonMinimalPush,
    /// The signature uses a defined ZIP-244 mode other than `SIGHASH_ALL`.
    NonAllSighash,
    /// The signature uses the defined ZIP-244 `ANYONECANPAY` modifier.
    AnyoneCanPay,
    /// The ECDSA signature is consensus-valid but uses high-S form.
    HighS,
    /// The transaction is not transparent-only with exactly one input and one output.
    UnexpectedShape,
    /// The sole transparent output does not pay the expected destination.
    UnexpectedDestination,
    /// The sole transparent output does not preserve the expected fee.
    UnexpectedFee,
    /// The transaction expiry differs from the request policy.
    UnexpectedExpiryHeight,
    /// Branch lock time or input sequence differs from the SDK constructor's values.
    UnexpectedBranchFields,
    /// No request-level destination/fee/expiry policy was attached to expected terms.
    MissingExpectedPolicy,
}

/// Non-consensus SDK policy report for a valid BIP-199 spend.
///
/// Deviations never turn a consensus-valid claim/refund into an unobserved spend. Callers may use
/// this report for alerts or routing decisions, but secret extraction depends only on consensus
/// validation and semantic branch classification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SdkCanonicalSpendPolicy {
    deviations: Box<[SdkCanonicalSpendDeviation]>,
    sighash_type: u8,
}

impl SdkCanonicalSpendPolicy {
    /// Whether the spend exactly follows all attached SDK transaction policy.
    #[must_use]
    pub fn is_compliant(&self) -> bool {
        self.deviations.is_empty()
    }

    /// Complete deterministic list of policy deviations.
    #[must_use]
    pub fn deviations(&self) -> &[SdkCanonicalSpendDeviation] {
        &self.deviations
    }

    /// Defined ZIP-244 signature-hash byte used by the selected branch signature.
    #[must_use]
    pub const fn sighash_type(&self) -> u8 {
        self.sighash_type
    }

    /// Whether the stack uses the SDK's exact semantic and byte encoding.
    #[must_use]
    pub fn has_exact_script_sig(&self) -> bool {
        !self
            .deviations
            .contains(&SdkCanonicalSpendDeviation::NonCanonicalScriptSig)
    }

    /// Whether every stack element uses its minimal push opcode.
    #[must_use]
    pub fn uses_minimal_pushes(&self) -> bool {
        !self
            .deviations
            .contains(&SdkCanonicalSpendDeviation::NonMinimalPush)
    }

    /// Whether the ECDSA signature uses low-S form.
    #[must_use]
    pub fn uses_low_s(&self) -> bool {
        !self.deviations.contains(&SdkCanonicalSpendDeviation::HighS)
    }

    /// Whether the selected signature mode is exactly ALL without ANYONECANPAY.
    #[must_use]
    pub fn uses_all_without_anyone_can_pay(&self) -> bool {
        !self
            .deviations
            .contains(&SdkCanonicalSpendDeviation::NonAllSighash)
            && !self
                .deviations
                .contains(&SdkCanonicalSpendDeviation::AnyoneCanPay)
    }

    /// Whether transparent/shielded component shape follows the SDK constructor.
    #[must_use]
    pub fn has_expected_shape(&self) -> bool {
        !self
            .deviations
            .contains(&SdkCanonicalSpendDeviation::UnexpectedShape)
    }

    /// Whether the transparent payout script matches request policy.
    #[must_use]
    pub fn has_expected_destination(&self) -> bool {
        !self
            .deviations
            .contains(&SdkCanonicalSpendDeviation::UnexpectedDestination)
            && !self
                .deviations
                .contains(&SdkCanonicalSpendDeviation::MissingExpectedPolicy)
    }

    /// Whether value conservation yields the request's exact fee.
    #[must_use]
    pub fn has_expected_fee(&self) -> bool {
        !self
            .deviations
            .contains(&SdkCanonicalSpendDeviation::UnexpectedFee)
            && !self
                .deviations
                .contains(&SdkCanonicalSpendDeviation::MissingExpectedPolicy)
    }

    /// Whether expiry height matches request policy.
    #[must_use]
    pub fn has_expected_expiry_height(&self) -> bool {
        !self
            .deviations
            .contains(&SdkCanonicalSpendDeviation::UnexpectedExpiryHeight)
            && !self
                .deviations
                .contains(&SdkCanonicalSpendDeviation::MissingExpectedPolicy)
    }
}

/// A complete canonical-chain observation of a consensus-valid claim or refund transaction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalZcashSpendObservation {
    network: NetworkType,
    consensus_branch_id: BranchId,
    block_hash: BlockHash,
    block_height: BlockHeight,
    tip_block_hash: BlockHash,
    tip_height: BlockHeight,
    transaction_id: TxId,
    spent_outpoint: OutPoint,
    confirmations: NonZeroU32,
    raw_transaction: Box<[u8]>,
    kind: Bip199SpendKind,
    transparent_outputs: Box<[TxOut]>,
    lock_time: u32,
    expiry_height: BlockHeight,
    input_sequence: u32,
    sdk_canonical_policy: SdkCanonicalSpendPolicy,
}

/// A rejected canonical spend snapshot or immutable spend binding.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SpendObservationError {
    /// The expected fetched output does not commit to the immutable contract.
    #[error("funding output scriptPubKey does not match the BIP-199 contract")]
    FundingScriptMismatch,
    /// Request-level output policy arithmetic was inconsistent.
    #[error("expected SDK spend policy value arithmetic failed")]
    ExpectedPolicyValueOverflow,
    /// The best-chain tip changed during the multi-query observation.
    #[error("best-chain tip changed during Zcash spend observation")]
    UnstableTip,
    /// The transaction is not in the active best chain.
    #[error("spend transaction is not in the active chain")]
    InactiveChain,
    /// The observed node network differs from the immutable terms.
    #[error("Zcash spend observation network mismatch")]
    NetworkMismatch,
    /// The observed or encoded consensus branch differs from immutable terms.
    #[error("Zcash spend observation consensus branch mismatch")]
    ConsensusBranchMismatch,
    /// The transaction context and canonical height lookup name different blocks.
    #[error("spend transaction block hash is not canonical at its height")]
    BlockHashMismatch,
    /// The claimed inclusion height is above the stable canonical tip.
    #[error("spend transaction block height is above the canonical tip")]
    BlockAboveTip,
    /// The selected-branch transaction bytes could not be decoded.
    #[error("canonical spend transaction decoding failed")]
    MalformedTransaction,
    /// The untrusted raw transaction exceeded pinned Zebra's maximum byte budget.
    #[error("raw spend transaction is {actual} bytes; maximum is {maximum}")]
    RawTransactionTooLarge {
        /// Actual untrusted byte length.
        actual: usize,
        /// Pinned Zebra maximum.
        maximum: usize,
    },
    /// Bytes remained after one canonical transaction was decoded.
    #[error("raw spend transaction contains trailing bytes")]
    TrailingTransactionBytes,
    /// The selected scriptSig exceeded the pinned consensus script-size limit.
    #[error("spend scriptSig is {actual} bytes; maximum is {maximum}")]
    ScriptSigTooLarge {
        /// Actual decoded scriptSig length.
        actual: usize,
        /// Pinned script maximum.
        maximum: usize,
    },
    /// The RPC transaction identifier differs from the canonical bytes.
    #[error("reported spend transaction ID differs from canonical bytes")]
    TransactionIdMismatch,
    /// The transaction does not contain exactly one transparent input.
    #[error("spend transaction must contain exactly one transparent input")]
    UnexpectedTransparentShape,
    /// The transparent input does not consume the exact expected output.
    #[error("transparent input does not spend the expected BIP-199 outpoint")]
    OutpointMismatch,
    /// The input stack is not the exact immutable BIP-199 redeem script.
    #[error("spend input redeem script differs from immutable BIP-199 terms")]
    SpendScriptMismatch,
    /// The claim input does not reveal the negotiated SHA-256 preimage.
    #[error("claim input preimage does not match the BIP-199 secret digest")]
    WrongPreimage,
    /// The selected branch public key does not own its immutable role.
    #[error("spend input public key does not control the selected BIP-199 role")]
    WrongSpendingRole,
    /// The stack is malformed, incorrectly signed, or fails consensus execution.
    #[error("spend input does not validly execute the exact BIP-199 contract")]
    InvalidSpendScript,
    /// Confirmation depth arithmetic overflowed.
    #[error("canonical spend confirmation depth overflowed")]
    ConfirmationOverflow,
    /// RPC confirmation depth differs from the height-derived depth.
    #[error("reported spend confirmations differ from canonical block depth")]
    ConfirmationMismatch,
}

#[derive(Debug)]
struct VerificationTransparent {
    funding_output: TxOut,
}

impl TransparentAuthorization for VerificationTransparent {
    type ScriptSig = Script;
}

impl TransparentAuthorizingContext for VerificationTransparent {
    fn input_amounts(&self) -> Vec<Zatoshis> {
        vec![self.funding_output.value()]
    }

    fn input_scriptpubkeys(&self) -> Vec<Script> {
        vec![self.funding_output.script_pubkey().clone()]
    }
}

#[derive(Debug)]
struct VerificationAuthorization;

impl TransactionAuthorization for VerificationAuthorization {
    type TransparentAuth = VerificationTransparent;
    type SaplingAuth = sapling_bundle::Authorized;
    type OrchardAuth = orchard_bundle::Authorized;
}

impl CanonicalZcashSpendObservation {
    /// Validates canonical inclusion, exact outpoint, semantic branch stack, and ZIP-244 signature.
    ///
    /// # Errors
    ///
    /// Fails closed for any inconsistent node binding, malformed transaction, wrong contract
    /// branch data, or invalid signature. Consensus-valid wallet-policy deviations are retained
    /// in `SdkCanonicalSpendPolicy` rather than rejected.
    pub fn validate(
        expected: &ExpectedBip199Spend,
        snapshot: &ZcashSpendNodeSnapshot,
    ) -> Result<Self, SpendObservationError> {
        validate_snapshot_binding(expected, snapshot)?;
        let (tip_block_hash, tip_height) =
            snapshot.tip.validated().map_err(|error| match error {
                ObservationError::UnstableTip => SpendObservationError::UnstableTip,
                _ => unreachable!("stable-tip validation has one failure mode"),
            })?;
        let depth = u32::from(tip_height)
            .checked_sub(u32::from(snapshot.block_height))
            .ok_or(SpendObservationError::BlockAboveTip)?
            .checked_add(1)
            .ok_or(SpendObservationError::ConfirmationOverflow)?;
        let confirmations =
            NonZeroU32::new(depth).ok_or(SpendObservationError::ConfirmationOverflow)?;
        if depth != snapshot.reported_confirmations {
            return Err(SpendObservationError::ConfirmationMismatch);
        }

        let recognized = decode_and_recognize(expected, snapshot)?;

        Ok(Self {
            network: snapshot.network,
            consensus_branch_id: snapshot.consensus_branch_id,
            block_hash: snapshot.transaction_block_hash,
            block_height: snapshot.block_height,
            tip_block_hash,
            tip_height,
            transaction_id: recognized.transaction_id,
            spent_outpoint: expected.outpoint.clone(),
            confirmations,
            raw_transaction: snapshot.raw_transaction.clone().into(),
            kind: recognized.kind,
            transparent_outputs: recognized.transparent_outputs,
            lock_time: recognized.lock_time,
            expiry_height: recognized.expiry_height,
            input_sequence: recognized.input_sequence,
            sdk_canonical_policy: recognized.sdk_canonical_policy,
        })
    }

    /// Validated Zcash network.
    #[must_use]
    pub const fn network(&self) -> NetworkType {
        self.network
    }

    /// Validated consensus branch.
    #[must_use]
    pub const fn consensus_branch_id(&self) -> BranchId {
        self.consensus_branch_id
    }

    /// Canonical inclusion block hash.
    #[must_use]
    pub const fn block_hash(&self) -> BlockHash {
        self.block_hash
    }

    /// Canonical inclusion block height.
    #[must_use]
    pub const fn block_height(&self) -> BlockHeight {
        self.block_height
    }

    /// Stable best-chain tip hash.
    #[must_use]
    pub const fn tip_block_hash(&self) -> BlockHash {
        self.tip_block_hash
    }

    /// Stable best-chain tip height.
    #[must_use]
    pub const fn tip_height(&self) -> BlockHeight {
        self.tip_height
    }

    /// Canonical spend transaction identifier.
    #[must_use]
    pub const fn transaction_id(&self) -> TxId {
        self.transaction_id
    }

    /// Exact BIP-199 outpoint consumed by the spend.
    #[must_use]
    pub const fn spent_outpoint(&self) -> &OutPoint {
        &self.spent_outpoint
    }

    /// Canonical confirmation depth recomputed from inclusion and tip heights.
    #[must_use]
    pub const fn confirmations(&self) -> NonZeroU32 {
        self.confirmations
    }

    /// Exact canonical transaction bytes.
    #[must_use]
    pub fn raw_transaction(&self) -> &[u8] {
        &self.raw_transaction
    }

    /// Validly executed BIP-199 branch and role evidence.
    #[must_use]
    pub const fn kind(&self) -> &Bip199SpendKind {
        &self.kind
    }

    /// Complete transparent output effects preserved from the validated transaction.
    #[must_use]
    pub fn transparent_outputs(&self) -> &[TxOut] {
        &self.transparent_outputs
    }

    /// Consensus transaction lock time preserved from the validated transaction.
    #[must_use]
    pub const fn lock_time(&self) -> u32 {
        self.lock_time
    }

    /// Consensus transaction expiry height preserved from the validated transaction.
    #[must_use]
    pub const fn expiry_height(&self) -> BlockHeight {
        self.expiry_height
    }

    /// Sequence of the exact input that consumed the contract outpoint.
    #[must_use]
    pub const fn input_sequence(&self) -> u32 {
        self.input_sequence
    }

    /// Separate SDK policy result for this consensus-valid spend.
    #[must_use]
    pub const fn sdk_canonical_policy(&self) -> &SdkCanonicalSpendPolicy {
        &self.sdk_canonical_policy
    }
}

struct RecognizedTransaction {
    transaction_id: TxId,
    kind: Bip199SpendKind,
    transparent_outputs: Box<[TxOut]>,
    lock_time: u32,
    expiry_height: BlockHeight,
    input_sequence: u32,
    sdk_canonical_policy: SdkCanonicalSpendPolicy,
}

fn decode_and_recognize(
    expected: &ExpectedBip199Spend,
    snapshot: &ZcashSpendNodeSnapshot,
) -> Result<RecognizedTransaction, SpendObservationError> {
    if snapshot.raw_transaction.len() > ZEBRA_MAX_BLOCK_BYTES {
        return Err(SpendObservationError::RawTransactionTooLarge {
            actual: snapshot.raw_transaction.len(),
            maximum: ZEBRA_MAX_BLOCK_BYTES,
        });
    }
    let mut cursor = Cursor::new(snapshot.raw_transaction.as_slice());
    let transaction = Transaction::read(&mut cursor, snapshot.consensus_branch_id)
        .map_err(|_| SpendObservationError::MalformedTransaction)?;
    if cursor.position()
        != u64::try_from(snapshot.raw_transaction.len())
            .map_err(|_| SpendObservationError::TrailingTransactionBytes)?
    {
        return Err(SpendObservationError::TrailingTransactionBytes);
    }
    if transaction.consensus_branch_id() != expected.consensus_branch_id {
        return Err(SpendObservationError::ConsensusBranchMismatch);
    }
    let transaction_id = transaction.txid();
    if transaction_id != snapshot.reported_transaction_id {
        return Err(SpendObservationError::TransactionIdMismatch);
    }
    recognize_transaction(expected, &transaction, transaction_id)
}

fn recognize_transaction(
    expected: &ExpectedBip199Spend,
    transaction: &Transaction,
    transaction_id: TxId,
) -> Result<RecognizedTransaction, SpendObservationError> {
    let bundle = transaction
        .transparent_bundle()
        .ok_or(SpendObservationError::UnexpectedTransparentShape)?;
    let matching_inputs = bundle
        .vin
        .iter()
        .enumerate()
        .filter(|(_, input)| input.prevout() == &expected.outpoint)
        .collect::<Vec<_>>();
    let [(input_index, input)] = matching_inputs.as_slice() else {
        return Err(if matching_inputs.is_empty() {
            SpendObservationError::OutpointMismatch
        } else {
            SpendObservationError::UnexpectedTransparentShape
        });
    };
    let input_index = *input_index;
    let script_sig = input.script_sig().0.0.as_slice();
    if script_sig.len() > ZCASH_MAX_SCRIPT_BYTES {
        return Err(SpendObservationError::ScriptSigTooLarge {
            actual: script_sig.len(),
            maximum: ZCASH_MAX_SCRIPT_BYTES,
        });
    }
    let classified = classify_script_sig(&expected.contract, script_sig)?;
    let sighash_type = classified
        .signature
        .last()
        .copied()
        .and_then(SighashType::parse)
        .ok_or(SpendObservationError::InvalidSpendScript)?;
    if bundle.vin.len() > 1 && sighash_type.encode() & 0x80 == 0 {
        // The adapter must supply every prevout before non-ACP multi-input recognition is safe.
        return Err(SpendObservationError::UnexpectedTransparentShape);
    }
    if sighash_type.encode() & 0x1f == 0x03 && input_index >= bundle.vout.len() {
        return Err(SpendObservationError::InvalidSpendScript);
    }
    if !executes_contract(transaction, expected, script_sig, input_index, sighash_type) {
        return Err(SpendObservationError::InvalidSpendScript);
    }
    let sdk_canonical_policy = evaluate_sdk_policy(
        expected,
        transaction,
        input_index,
        script_sig,
        &classified,
        sighash_type,
    );
    Ok(RecognizedTransaction {
        transaction_id,
        kind: classified.kind,
        transparent_outputs: bundle.vout.clone().into_boxed_slice(),
        lock_time: transaction.lock_time(),
        expiry_height: transaction.expiry_height(),
        input_sequence: input.sequence(),
        sdk_canonical_policy,
    })
}

fn validate_snapshot_binding(
    expected: &ExpectedBip199Spend,
    snapshot: &ZcashSpendNodeSnapshot,
) -> Result<(), SpendObservationError> {
    if snapshot.network != expected.network {
        return Err(SpendObservationError::NetworkMismatch);
    }
    if snapshot.consensus_branch_id != expected.consensus_branch_id {
        return Err(SpendObservationError::ConsensusBranchMismatch);
    }
    if !snapshot.in_active_chain {
        return Err(SpendObservationError::InactiveChain);
    }
    if snapshot.transaction_block_hash != snapshot.canonical_block_hash {
        return Err(SpendObservationError::BlockHashMismatch);
    }
    Ok(())
}

#[derive(Debug)]
struct ClassifiedSpend {
    kind: Bip199SpendKind,
    signature: Vec<u8>,
    canonical_script_sig: Vec<u8>,
    minimal_pushes: bool,
}

fn classify_script_sig(
    contract: &Bip199Contract,
    script_sig: &[u8],
) -> Result<ClassifiedSpend, SpendObservationError> {
    let pushes = Code(script_sig.to_vec())
        .parse()
        .map(|parsed| match parsed {
            Ok(PossiblyBad::Good(Opcode::PushValue(value))) => Ok(value.value()),
            _ => Err(SpendObservationError::InvalidSpendScript),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let minimal_pushes = minimally_encode_pushes(&pushes) == script_sig;

    if pushes.len() >= 5 && script_bool(&pushes[pushes.len() - 2]) {
        let tail = &pushes[pushes.len() - 5..];
        let [signature, public_key, preimage, _, redeem_script] = tail else {
            unreachable!("five-element tail")
        };
        if redeem_script != contract.redeem_script() {
            return Err(SpendObservationError::SpendScriptMismatch);
        }
        if <[u8; 32]>::from(Sha256::digest(preimage)) != contract.secret_digest() {
            return Err(SpendObservationError::WrongPreimage);
        }
        let public_key = validated_role_key(public_key, contract.claimant_pubkey_hash())?;
        let canonical_script_sig = contract
            .claim_script_sig(signature, &public_key, preimage)
            .map_err(|_| SpendObservationError::InvalidSpendScript)?;
        return Ok(ClassifiedSpend {
            kind: Bip199SpendKind::Claim {
                preimage: preimage.clone().into_boxed_slice(),
                claimant_public_key: public_key,
            },
            signature: signature.clone(),
            canonical_script_sig,
            minimal_pushes,
        });
    }

    if pushes.len() >= 4 && !script_bool(&pushes[pushes.len() - 2]) {
        let tail = &pushes[pushes.len() - 4..];
        let [signature, public_key, _, redeem_script] = tail else {
            unreachable!("four-element tail")
        };
        if redeem_script != contract.redeem_script() {
            return Err(SpendObservationError::SpendScriptMismatch);
        }
        let public_key = validated_role_key(public_key, contract.refund_pubkey_hash())?;
        let canonical_script_sig = contract
            .refund_script_sig(signature, &public_key)
            .map_err(|_| SpendObservationError::InvalidSpendScript)?;
        return Ok(ClassifiedSpend {
            kind: Bip199SpendKind::Refund {
                refund_public_key: public_key,
            },
            signature: signature.clone(),
            canonical_script_sig,
            minimal_pushes,
        });
    }

    Err(SpendObservationError::InvalidSpendScript)
}

fn script_bool(value: &[u8]) -> bool {
    value
        .iter()
        .enumerate()
        .any(|(index, byte)| *byte != 0 && !(index + 1 == value.len() && *byte == 0x80))
}

fn minimally_encode_pushes(pushes: &[Vec<u8>]) -> Vec<u8> {
    let mut encoded = Vec::new();
    for value in pushes {
        match value.as_slice() {
            [] => encoded.push(0),
            [0x81] => encoded.push(0x4f),
            [value @ 1..=16] => encoded.push(0x50 + value),
            _ if value.len() <= 75 => {
                encoded.push(u8::try_from(value.len()).expect("length is at most 75"));
                encoded.extend_from_slice(value);
            }
            _ if value.len() <= 255 => {
                encoded.extend_from_slice(&[0x4c, u8::try_from(value.len()).expect("at most 255")]);
                encoded.extend_from_slice(value);
            }
            _ => {
                encoded.push(0x4d);
                encoded.extend_from_slice(
                    &u16::try_from(value.len())
                        .expect("script pushes are consensus-limited to 520 bytes")
                        .to_le_bytes(),
                );
                encoded.extend_from_slice(value);
            }
        }
    }
    encoded
}

fn validated_role_key(
    encoded: &[u8],
    expected_hash: [u8; 20],
) -> Result<[u8; 33], SpendObservationError> {
    let public_key =
        PublicKey::from_slice(encoded).map_err(|_| SpendObservationError::WrongSpendingRole)?;
    let compressed = public_key.serialize();
    if encoded != compressed {
        return Err(SpendObservationError::WrongSpendingRole);
    }
    let actual_hash = match TransparentAddress::from_pubkey(&public_key) {
        TransparentAddress::PublicKeyHash(hash) => hash,
        TransparentAddress::ScriptHash(_) => unreachable!("public keys always yield P2PKH"),
    };
    if actual_hash != expected_hash {
        return Err(SpendObservationError::WrongSpendingRole);
    }
    Ok(compressed)
}

fn evaluate_sdk_policy(
    expected: &ExpectedBip199Spend,
    transaction: &Transaction,
    input_index: usize,
    script_sig: &[u8],
    classified: &ClassifiedSpend,
    sighash_type: SighashType,
) -> SdkCanonicalSpendPolicy {
    let mut deviations = Vec::new();
    if script_sig != classified.canonical_script_sig {
        deviations.push(SdkCanonicalSpendDeviation::NonCanonicalScriptSig);
    }
    if !classified.minimal_pushes {
        deviations.push(SdkCanonicalSpendDeviation::NonMinimalPush);
    }
    let sighash_byte = sighash_type.encode();
    if sighash_byte & 0x1f != 0x01 {
        deviations.push(SdkCanonicalSpendDeviation::NonAllSighash);
    }
    if sighash_byte & 0x80 != 0 {
        deviations.push(SdkCanonicalSpendDeviation::AnyoneCanPay);
    }
    if !is_low_s(&classified.signature) {
        deviations.push(SdkCanonicalSpendDeviation::HighS);
    }

    let bundle = transaction
        .transparent_bundle()
        .expect("consensus recognition selected a transparent input");
    let exact_shape = bundle.vin.len() == 1
        && input_index == 0
        && bundle.vout.len() == 1
        && transaction.sprout_bundle().is_none()
        && transaction.sapling_bundle().is_none()
        && transaction.orchard_bundle().is_none();
    if !exact_shape {
        deviations.push(SdkCanonicalSpendDeviation::UnexpectedShape);
    }

    match &expected.sdk_policy {
        Some(policy) => {
            if bundle.vout.len() != 1
                || bundle.vout[0].script_pubkey() != policy.output.script_pubkey()
            {
                deviations.push(SdkCanonicalSpendDeviation::UnexpectedDestination);
            }
            if !exact_shape || bundle.vout[0].value() != policy.output.value() {
                deviations.push(SdkCanonicalSpendDeviation::UnexpectedFee);
            }
            if transaction.expiry_height() != policy.expiry_height {
                deviations.push(SdkCanonicalSpendDeviation::UnexpectedExpiryHeight);
            }
        }
        None => deviations.push(SdkCanonicalSpendDeviation::MissingExpectedPolicy),
    }

    let input = &bundle.vin[input_index];
    let canonical_branch_fields = match &classified.kind {
        Bip199SpendKind::Claim { .. } => {
            transaction.lock_time() == 0 && input.sequence() == u32::MAX
        }
        Bip199SpendKind::Refund { .. } => {
            transaction.lock_time() == expected.contract.refund_lock_time()
                && input.sequence() == expected.contract.refund_input_sequence()
        }
    };
    if !canonical_branch_fields {
        deviations.push(SdkCanonicalSpendDeviation::UnexpectedBranchFields);
    }

    SdkCanonicalSpendPolicy {
        deviations: deviations.into_boxed_slice(),
        sighash_type: sighash_byte,
    }
}

fn is_low_s(signature_with_hash_type: &[u8]) -> bool {
    let Some((_, der)) = signature_with_hash_type.split_last() else {
        return false;
    };
    let Ok(signature) = Signature::from_der(der) else {
        return false;
    };
    let mut normalized = signature;
    normalized.normalize_s();
    signature == normalized
}

fn script_flags() -> Flags {
    // Exact pinned Zebra 5.2.0 consensus flags. Canonical wallet policy belongs in
    // `SdkCanonicalSpendPolicy` and must never alter spend recognition.
    Flags::P2SH | Flags::CHECKLOCKTIMEVERIFY
}

fn executes_contract(
    transaction: &Transaction,
    expected: &ExpectedBip199Spend,
    script_sig: &[u8],
    input_index: usize,
    selected_sighash_type: SighashType,
) -> bool {
    let funding_output = expected.funding_output.clone();
    let data = transaction
        .clone()
        .into_data()
        .map_bundles::<VerificationAuthorization>(
            |bundle| {
                bundle.map(|bundle| Bundle {
                    vin: bundle
                        .vin
                        .into_iter()
                        .map(|input| {
                            TxIn::from_parts(
                                input.prevout().clone(),
                                input.script_sig().clone(),
                                input.sequence(),
                            )
                        })
                        .collect(),
                    vout: bundle.vout,
                    authorization: VerificationTransparent {
                        funding_output: funding_output.clone(),
                    },
                })
            },
            |bundle| bundle,
            |bundle| bundle,
        );
    let txid_parts = data.digest(TxIdDigester);
    let Some(bundle) = data.transparent_bundle() else {
        return false;
    };
    let sighash = |script_code: &Code, hash_type: &HashType| {
        let interpreted_sighash_type = match hash_type.signed_outputs() {
            SignedOutputs::All => {
                if hash_type.anyone_can_pay() {
                    SighashType::ALL_ANYONECANPAY
                } else {
                    SighashType::ALL
                }
            }
            SignedOutputs::None => {
                if hash_type.anyone_can_pay() {
                    SighashType::NONE_ANYONECANPAY
                } else {
                    SighashType::NONE
                }
            }
            SignedOutputs::Single => {
                if hash_type.anyone_can_pay() {
                    SighashType::SINGLE_ANYONECANPAY
                } else {
                    SighashType::SINGLE
                }
            }
        };
        if interpreted_sighash_type != selected_sighash_type
            || script_code.0 != expected.contract.redeem_script()
        {
            return None;
        }
        let script_code = Script(script_code.clone());
        let signable = TransparentSignableInput::from_parts(
            bundle,
            selected_sighash_type,
            input_index,
            &script_code,
            funding_output.script_pubkey(),
            funding_output.value(),
        )
        .ok()?;
        Some(
            *signature_hash(
                &data,
                &TransactionSignableInput::Transparent(signable),
                &txid_parts,
            )
            .as_ref(),
        )
    };
    let input = &transaction
        .transparent_bundle()
        .expect("validated transparent shape")
        .vin[input_index];
    let checker = CallbackTransactionSignatureChecker {
        sighash: &sighash,
        lock_time: i64::from(transaction.lock_time()),
        is_final: input.sequence() == u32::MAX,
    };
    matches!(
        Raw::from_raw_parts(
            script_sig.to_vec(),
            funding_output.script_pubkey().0.0.clone(),
        )
        .eval(script_flags(), &checker),
        Ok(true)
    )
}
