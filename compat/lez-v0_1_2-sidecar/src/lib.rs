//! Official LEZ v0.1.2 transaction planning and exact-byte boundary.
//!
//! This crate intentionally lives outside the main workspace. It is the only
//! process boundary that imports the pinned official NSSA and SPEL graph.

#![forbid(unsafe_code)]

mod server;

pub use server::{
    BridgeServerCapability, BridgeServerCapabilityError, BridgeServerConfig, BridgeServerError,
    BridgeServerHandle, start_bridge_server,
};

use std::{fmt, net::IpAddr, sync::Arc, time::Duration};

use async_trait::async_trait;
use common::{
    HashType,
    block::{BedrockStatus, Block, HashableBlockData},
    transaction::NSSATransaction,
};
use lez_bridge_protocol::{
    AccountIds, ChainPosition, ChainTip, DiscoveryWindow, ExactTransactionBytes, Hex32,
    ObservedTransactionFacts, Participant, PrepareNativeEscrowRequest, PrepareNativeEscrowResult,
    PreparedTransaction, ProtocolValueError, RuntimeDescriptor, SubmissionOutcome,
    SubmitTransactionRequest, SubmitTransactionResult, TransactionId,
};
use lez_zec_escrow_compat::Instruction as EscrowInstruction;
use nssa::{
    AccountId, PrivateKey, PublicKey, PublicTransaction,
    program::Program,
    public_transaction::{Message, WitnessSet},
};
use sequencer_service_rpc::{ClientError, RpcClient as _, SequencerClient, SequencerClientBuilder};
use tokio::sync::Mutex;
use url::{Host, Url};

const MAX_NODE_REQUEST_BYTES: u32 = 2_800_000;
const MAX_NODE_RESPONSE_BYTES: u32 = 8 * 1024 * 1024;
const NODE_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Fail-closed errors at the official transaction boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SidecarError {
    /// The request or transaction role does not target this sidecar.
    #[error("request targets the wrong sidecar role")]
    WrongSidecarRole,
    /// Runtime signer identity does not equal the isolated official key.
    #[error("runtime or transaction signer does not match the isolated signer")]
    WrongSigner,
    /// Signed escrow terms assign the depositor role to another participant.
    #[error("native escrow depositor role does not match the isolated sidecar")]
    WrongDepositorRole,
    /// Runtime escrow program identity differs from the configured program.
    #[error("runtime escrow program does not match the configured official program")]
    WrongEscrowProgram,
    /// Native terms do not select the official authenticated-transfer program.
    #[error("native terms do not select the official authenticated-transfer program")]
    WrongAuthenticatedTransferProgram,
    /// Runtime generation is not the pinned v0.1.2 compatibility graph.
    #[error("runtime compatibility is not pinned NSSA v0.1.2")]
    WrongRuntimeCompatibility,
    /// Request runtime identity differs from the sidecar's complete configured identity.
    #[error("request runtime identity does not match the configured sidecar runtime")]
    WrongRuntimeIdentity,
    /// Another distinct nonce reservation is active for this one-swap signer.
    #[error("a distinct native escrow preparation is already active")]
    ActivePrepare,
    /// Submission is not one of the exact transactions cached by this planner.
    #[error("transaction was not prepared by this sidecar instance")]
    TransactionNotPrepared,
    /// The official node nonce could not be obtained.
    #[error("official signer nonce is unavailable")]
    NonceUnavailable,
    /// Node RPC URL was not an explicit HTTP loopback IP and port.
    #[error("official node endpoint must be an uncredentialed HTTP loopback IP and port")]
    InvalidNodeEndpoint,
    /// The pinned node proved a stateless invalid-params rejection.
    #[error("official node definitively rejected the transaction")]
    NodeRejected,
    /// Submission may have reached the node, so observe the exact ID before retrying.
    #[error("official node submission outcome is unknown")]
    UnknownSubmissionOutcome,
    /// Bounded block or tip facts could not be obtained from the node.
    #[error("official node observation is unavailable")]
    NodeObservationUnavailable,
    /// Node block facts were incomplete, inconsistent, or not exact.
    #[error("official node returned an invalid block response")]
    InvalidNodeResponse,
    /// The consecutive funding nonce would exceed u128.
    #[error("official signer nonce cannot be incremented")]
    NonceOverflow,
    /// Official instruction serialization failed.
    #[error("official native escrow instruction serialization failed")]
    InstructionEncoding,
    /// Exact bytes are not one canonical official public transaction.
    #[error("exact bytes are not a canonical official public transaction")]
    InvalidTransactionBytes,
    /// The official transaction hash differs from its persisted ID.
    #[error("official transaction hash differs from the persisted transaction ID")]
    WrongTransactionId,
    /// The official witness set is missing, malformed, or cryptographically invalid.
    #[error("official public transaction signature is invalid")]
    InvalidSignature,
    /// The bounded bridge protocol rejected an official transaction representation.
    #[error("official transaction exceeds the bounded bridge protocol")]
    ProtocolEncoding,
}

impl From<ProtocolValueError> for SidecarError {
    fn from(_value: ProtocolValueError) -> Self {
        Self::ProtocolEncoding
    }
}

/// Supplies the current official public-account nonce exactly once per preparation.
#[async_trait]
pub trait NonceSource: Send + Sync {
    /// Returns the current u128 nonce for `account_id`.
    async fn account_nonce(&self, account_id: AccountId) -> Result<u128, SidecarError>;
}

/// Submits only a transaction that the planner has cached byte-for-byte.
#[async_trait]
pub trait ExactTransactionSubmitter: Send + Sync {
    /// Decodes cached official bytes without reconstructing or re-signing and
    /// checks the hash returned by the pinned node's `sendTransaction` call.
    ///
    /// # Errors
    ///
    /// Returns an error for any identity/cache mismatch, proven invalid-params
    /// rejection, uncertain outcome, or returned hash mismatch.
    async fn submit_exact(
        &self,
        planner: &NativeEscrowPlanner,
        request: &SubmitTransactionRequest,
    ) -> Result<SubmitTransactionResult, SidecarError>;
}

/// Upstream settlement label attached to the block containing an exact transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OfficialSettlement {
    /// The sequencer still reports the block as pending Bedrock settlement.
    Pending,
    /// The sequencer reports the block as safe.
    Safe,
    /// The sequencer reports the block as finalized.
    Finalized,
}

/// Exact official transaction and its node-reported block settlement label.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OfficialExactFound {
    /// Exact decoder, signer, bytes, and chain-position facts.
    pub transaction: ObservedTransactionFacts,
    /// Official `BedrockStatus` mapped without inferring confirmation depth.
    pub settlement: OfficialSettlement,
}

/// Result of one explicit bounded exact-ID window scan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OfficialExactObservation {
    /// The current tip has not reached the requested window's first height.
    NotYetCovered,
    /// Every available block in this declared window was scanned without a match.
    NotFoundInWindow,
    /// The exact persisted public transaction was present once in the linked range.
    Found(OfficialExactFound),
}

/// Bracketed result of one bounded official block-range scan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OfficialExactScan {
    /// Validated node tip immediately before the bounded range request.
    pub tip_before: ChainTip,
    /// Exact window result; this does not by itself claim canonical stability.
    pub observation: OfficialExactObservation,
    /// Validated node tip immediately after the bounded range request.
    pub tip_after: ChainTip,
}

/// Bounded official v0.1.2 sequencer RPC client for one role and signer.
///
/// The endpoint must be an explicit loopback IP literal. The pinned
/// `jsonrpsee` HTTP transport connects directly through Hyper: it does not
/// consult environment proxy variables and does not implement redirects.
#[derive(Clone)]
pub struct OfficialNodeRpc {
    role: Participant,
    signer_account_id: AccountId,
    runtime: RuntimeDescriptor,
    client: SequencerClient,
}

impl fmt::Debug for OfficialNodeRpc {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OfficialNodeRpc")
            .field("role", &self.role)
            .field("signer_account_id", &self.signer_account_id)
            .field("runtime", &self.runtime)
            .finish_non_exhaustive()
    }
}

impl OfficialNodeRpc {
    /// Connects to one local pinned sequencer endpoint without proxy or redirects.
    ///
    /// This applies finite body bounds and timeout, permits one request at a
    /// time, and configures no retry middleware.
    ///
    /// # Errors
    ///
    /// Returns an error unless `endpoint` is an uncredentialed `http` URL with
    /// an explicit loopback IP and port, or role/signer/compatibility disagree.
    pub fn connect(
        endpoint: &str,
        role: Participant,
        signer_account_id: AccountId,
        runtime: RuntimeDescriptor,
    ) -> Result<Self, SidecarError> {
        validate_node_endpoint(endpoint)?;
        if runtime.sidecar_role != role {
            return Err(SidecarError::WrongSidecarRole);
        }
        if runtime.compatibility != lez_bridge_protocol::RuntimeCompatibility::NssaV0_1_2 {
            return Err(SidecarError::WrongRuntimeCompatibility);
        }
        if runtime.signer_account_id != Hex32::from_bytes(signer_account_id.into_value()) {
            return Err(SidecarError::WrongSigner);
        }
        let client = SequencerClientBuilder::default()
            .max_request_size(MAX_NODE_REQUEST_BYTES)
            .max_response_size(MAX_NODE_RESPONSE_BYTES)
            .request_timeout(NODE_REQUEST_TIMEOUT)
            .max_concurrent_requests(1)
            .build(endpoint)
            .map_err(|_| SidecarError::InvalidNodeEndpoint)?;
        Ok(Self {
            role,
            signer_account_id,
            runtime,
            client,
        })
    }

    /// Scans one bounded official block window for an exact persisted transaction.
    ///
    /// Blocks must cover the requested available range exactly, have recomputable
    /// official hashes, form a linked sequence, and contain the target at most
    /// once. A matching hash with different public transaction bytes is rejected.
    /// The caller must compare the returned tips before treating the result as a
    /// stable-chain fact.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid persisted bytes/signature/signer, unavailable
    /// bounded RPC facts, malformed block ranges, broken links, duplicate target
    /// placement, or a hash match whose exact bytes differ.
    pub async fn scan_exact(
        &self,
        expected: &PreparedTransaction,
        window: DiscoveryWindow,
    ) -> Result<OfficialExactScan, SidecarError> {
        let target =
            decode_prepared_for_role(expected, self.role, self.role, self.signer_account_id)?;
        if target.message.program_id != program_id_from_hex(self.runtime.escrow_program_id) {
            return Err(SidecarError::WrongEscrowProgram);
        }
        let tip_before = self.read_tip().await?;
        if window.start_height() > tip_before.height {
            return Ok(OfficialExactScan {
                tip_before,
                observation: OfficialExactObservation::NotYetCovered,
                tip_after: self.read_tip().await?,
            });
        }

        let declared_end = window
            .start_height()
            .checked_add(u64::from(window.max_blocks() - 1))
            .ok_or(SidecarError::InvalidNodeResponse)?;
        let end = declared_end.min(tip_before.height);
        let anchor_start = window.start_height().saturating_sub(1);
        let blocks = self
            .client
            .get_block_range(anchor_start, end)
            .await
            .map_err(|_| SidecarError::NodeObservationUnavailable)?;
        validate_block_range(&blocks, anchor_start, end, self.runtime.genesis_block_hash)?;

        let skip_anchor = usize::from(window.start_height() > 0);
        let mut found = None;
        for block in blocks.iter().skip(skip_anchor) {
            for (index, transaction) in block.body.transactions.iter().enumerate() {
                if transaction.hash().0 != *expected.transaction_id.as_bytes() {
                    continue;
                }
                if found.is_some() {
                    return Err(SidecarError::InvalidNodeResponse);
                }
                let NSSATransaction::Public(public) = transaction else {
                    return Err(SidecarError::InvalidNodeResponse);
                };
                let observed = prepared_from_transaction(public)?;
                if observed != *expected {
                    return Err(SidecarError::InvalidNodeResponse);
                }
                let signer_account_ids = public
                    .witness_set()
                    .signatures_and_public_keys()
                    .iter()
                    .map(|(_, key)| Hex32::from_bytes(AccountId::from(key).into_value()))
                    .collect::<Vec<_>>();
                let position = ChainPosition::new(
                    Hex32::from_bytes(block.header.hash.0),
                    block.header.block_id,
                    u32::try_from(index).map_err(|_| SidecarError::InvalidNodeResponse)?,
                );
                found = Some(OfficialExactFound {
                    transaction: ObservedTransactionFacts::new(
                        observed.transaction_id,
                        observed.exact_bytes,
                        position,
                        AccountIds::new(signer_account_ids)?,
                        true,
                    ),
                    settlement: settlement(&block.bedrock_status),
                });
            }
        }

        Ok(OfficialExactScan {
            tip_before,
            observation: found.map_or(
                OfficialExactObservation::NotFoundInWindow,
                OfficialExactObservation::Found,
            ),
            tip_after: self.read_tip().await?,
        })
    }

    async fn read_tip(&self) -> Result<ChainTip, SidecarError> {
        let height = self
            .client
            .get_last_block_id()
            .await
            .map_err(|_| SidecarError::NodeObservationUnavailable)?;
        let block = self
            .client
            .get_block(height)
            .await
            .map_err(|_| SidecarError::NodeObservationUnavailable)?
            .ok_or(SidecarError::InvalidNodeResponse)?;
        validate_block_hash(&block)?;
        if block.header.block_id != height {
            return Err(SidecarError::InvalidNodeResponse);
        }
        if height == 0 && Hex32::from_bytes(block.header.hash.0) != self.runtime.genesis_block_hash
        {
            return Err(SidecarError::InvalidNodeResponse);
        }
        Ok(ChainTip::new(
            Hex32::from_bytes(block.header.hash.0),
            height,
        ))
    }
}

#[async_trait]
impl NonceSource for OfficialNodeRpc {
    async fn account_nonce(&self, account_id: AccountId) -> Result<u128, SidecarError> {
        if account_id != self.signer_account_id {
            return Err(SidecarError::WrongSigner);
        }
        let nonces = self
            .client
            .get_accounts_nonces(vec![account_id])
            .await
            .map_err(|_| SidecarError::NonceUnavailable)?;
        let [nonce] = nonces.as_slice() else {
            return Err(SidecarError::NonceUnavailable);
        };
        Ok((*nonce).into())
    }
}

#[async_trait]
impl ExactTransactionSubmitter for OfficialNodeRpc {
    async fn submit_exact(
        &self,
        planner: &NativeEscrowPlanner,
        request: &SubmitTransactionRequest,
    ) -> Result<SubmitTransactionResult, SidecarError> {
        if request.context.sidecar_role != self.role {
            return Err(SidecarError::WrongSidecarRole);
        }
        if request.runtime != self.runtime {
            return Err(SidecarError::WrongRuntimeIdentity);
        }
        let transaction = planner
            .decode_exact_for_submission(&request.transaction, request.context.sidecar_role)
            .await?;
        let returned_hash = self
            .client
            .send_transaction(transaction)
            .await
            .map_err(classify_submission_error)?;
        if returned_hash != HashType(*request.transaction.transaction_id.as_bytes()) {
            return Err(SidecarError::UnknownSubmissionOutcome);
        }
        Ok(SubmitTransactionResult::new(
            request.context.clone(),
            request.transaction.transaction_id,
            SubmissionOutcome::Accepted,
        ))
    }
}

#[derive(Clone)]
struct ActivePrepare {
    request: PrepareNativeEscrowRequest,
    result: PrepareNativeEscrowResult,
}

/// One-role, one-signer native planner for an isolated composed run.
pub struct NativeEscrowPlanner {
    role: Participant,
    signer_key: PrivateKey,
    signer_account_id: AccountId,
    escrow_program_id: [u32; 8],
    expected_runtime: RuntimeDescriptor,
    nonce_source: Arc<dyn NonceSource>,
    active: Mutex<Option<ActivePrepare>>,
}

impl fmt::Debug for NativeEscrowPlanner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NativeEscrowPlanner")
            .field("role", &self.role)
            .field("signer_key", &"[REDACTED]")
            .field("signer_account_id", &self.signer_account_id)
            .field(
                "escrow_program_id",
                &program_id_to_hex(self.escrow_program_id),
            )
            .field("expected_runtime", &self.expected_runtime)
            .finish_non_exhaustive()
    }
}

impl NativeEscrowPlanner {
    /// Creates a planner around one isolated official NSSA signing key.
    ///
    /// # Errors
    ///
    /// Returns an error when the configured runtime role, compatibility,
    /// signer, or escrow program does not match the isolated inputs.
    pub fn new<N>(
        role: Participant,
        signer_key: PrivateKey,
        escrow_program_id: [u32; 8],
        expected_runtime: RuntimeDescriptor,
        nonce_source: Arc<N>,
    ) -> Result<Self, SidecarError>
    where
        N: NonceSource + 'static,
    {
        let signer_account_id = AccountId::from(&PublicKey::new_from_private_key(&signer_key));
        if expected_runtime.sidecar_role != role {
            return Err(SidecarError::WrongSidecarRole);
        }
        if expected_runtime.compatibility != lez_bridge_protocol::RuntimeCompatibility::NssaV0_1_2 {
            return Err(SidecarError::WrongRuntimeCompatibility);
        }
        if expected_runtime.signer_account_id != Hex32::from_bytes(signer_account_id.into_value()) {
            return Err(SidecarError::WrongSigner);
        }
        if expected_runtime.escrow_program_id != program_id_to_hex(escrow_program_id) {
            return Err(SidecarError::WrongEscrowProgram);
        }
        let nonce_source: Arc<dyn NonceSource> = nonce_source;
        Ok(Self {
            role,
            signer_key,
            signer_account_id,
            escrow_program_id,
            expected_runtime,
            nonce_source,
            active: Mutex::new(None),
        })
    }

    /// Prepares and caches one exact initialization/funding nonce pair.
    ///
    /// Repeating the identical request returns the first randomized BIP340
    /// signatures byte-for-byte. A distinct request is rejected until this
    /// one-swap sidecar is replaced.
    ///
    /// # Errors
    ///
    /// Returns an error for a mismatched runtime or terms, an active distinct
    /// request, an unavailable/overflowing nonce, or official encoding failure.
    pub async fn prepare(
        &self,
        request: PrepareNativeEscrowRequest,
    ) -> Result<PrepareNativeEscrowResult, SidecarError> {
        self.validate_request(&request)?;

        let mut active = self.active.lock().await;
        if let Some(active) = active.as_ref() {
            return if active.request == request {
                Ok(active.result.clone())
            } else {
                Err(SidecarError::ActivePrepare)
            };
        }

        let initialization_nonce = self
            .nonce_source
            .account_nonce(self.signer_account_id)
            .await?;
        let funding_nonce = initialization_nonce
            .checked_add(1)
            .ok_or(SidecarError::NonceOverflow)?;
        let result = self.plan_pair(&request, initialization_nonce, funding_nonce)?;
        *active = Some(ActivePrepare {
            request,
            result: result.clone(),
        });
        Ok(result)
    }

    /// Restores one durably cached native pair without obtaining a new nonce
    /// or reconstructing either randomized signature.
    ///
    /// Both exact transactions are decoded through the official codec and
    /// their signer, program, ordered accounts, instructions, and consecutive
    /// nonces are checked against the original strict request before the pair
    /// becomes eligible for submission.
    ///
    /// # Errors
    ///
    /// Returns an error for any request/result mismatch, invalid official
    /// transaction, non-consecutive nonce pair, or distinct active preparation.
    pub async fn restore_prepared(
        &self,
        request: PrepareNativeEscrowRequest,
        result: PrepareNativeEscrowResult,
    ) -> Result<(), SidecarError> {
        self.validate_request(&request)?;
        if result.context != request.context
            || result.initialization == result.funding
            || result.initialization.transaction_id == result.funding.transaction_id
        {
            return Err(SidecarError::ProtocolEncoding);
        }
        let initialization = decode_prepared_for_role(
            &result.initialization,
            self.role,
            self.role,
            self.signer_account_id,
        )?;
        let funding = decode_prepared_for_role(
            &result.funding,
            self.role,
            self.role,
            self.signer_account_id,
        )?;
        let [initialization_nonce] = initialization.message.nonces.as_slice() else {
            return Err(SidecarError::InvalidTransactionBytes);
        };
        let [funding_nonce] = funding.message.nonces.as_slice() else {
            return Err(SidecarError::InvalidTransactionBytes);
        };
        let initialization_nonce = u128::from(*initialization_nonce);
        let expected_funding_nonce = initialization_nonce
            .checked_add(1)
            .ok_or(SidecarError::NonceOverflow)?;
        if u128::from(*funding_nonce) != expected_funding_nonce {
            return Err(SidecarError::InvalidTransactionBytes);
        }
        let (expected_initialization, expected_funding) =
            self.plan_messages(&request, initialization_nonce, expected_funding_nonce)?;
        if initialization.message != expected_initialization || funding.message != expected_funding
        {
            return Err(SidecarError::InvalidTransactionBytes);
        }

        let mut active = self.active.lock().await;
        if let Some(active) = active.as_ref() {
            return if active.request == request && active.result == result {
                Ok(())
            } else {
                Err(SidecarError::ActivePrepare)
            };
        }
        *active = Some(ActivePrepare { request, result });
        Ok(())
    }

    /// Wraps one exact cached transaction for the official submission RPC.
    ///
    /// Membership is checked before decoding, so this capability cannot act as
    /// a generic relay for another valid transaction signed by the same key.
    /// The message and randomized signature are never reconstructed.
    ///
    /// # Errors
    ///
    /// Returns an error for the wrong role, a transaction outside this
    /// planner's cached pair, or invalid official bytes, ID, program, or witness.
    pub async fn decode_exact_for_submission(
        &self,
        prepared: &PreparedTransaction,
        transaction_role: Participant,
    ) -> Result<NSSATransaction, SidecarError> {
        if transaction_role != self.role {
            return Err(SidecarError::WrongSidecarRole);
        }
        let active = self.active.lock().await;
        let active = active
            .as_ref()
            .ok_or(SidecarError::TransactionNotPrepared)?;
        if prepared != &active.result.initialization && prepared != &active.result.funding {
            return Err(SidecarError::TransactionNotPrepared);
        }
        let transaction = decode_prepared_for_role(
            prepared,
            transaction_role,
            self.role,
            self.signer_account_id,
        )?;
        if transaction.message.program_id != self.escrow_program_id {
            return Err(SidecarError::WrongEscrowProgram);
        }
        Ok(NSSATransaction::Public(transaction))
    }

    fn validate_request(&self, request: &PrepareNativeEscrowRequest) -> Result<(), SidecarError> {
        if request.runtime != self.expected_runtime {
            return Err(SidecarError::WrongRuntimeIdentity);
        }
        if request.context.sidecar_role != self.role || request.runtime.sidecar_role != self.role {
            return Err(SidecarError::WrongSidecarRole);
        }
        if request.runtime.compatibility != lez_bridge_protocol::RuntimeCompatibility::NssaV0_1_2 {
            return Err(SidecarError::WrongRuntimeCompatibility);
        }
        if request.terms.depositor() != self.role {
            return Err(SidecarError::WrongDepositorRole);
        }
        let signer = Hex32::from_bytes(self.signer_account_id.into_value());
        if request.runtime.signer_account_id != signer
            || request.terms.depositor_account_id() != signer
        {
            return Err(SidecarError::WrongSigner);
        }
        if request.runtime.escrow_program_id != program_id_to_hex(self.escrow_program_id) {
            return Err(SidecarError::WrongEscrowProgram);
        }
        if request.terms.authenticated_transfer_program_id()
            != program_id_to_hex(Program::authenticated_transfer_program().id())
        {
            return Err(SidecarError::WrongAuthenticatedTransferProgram);
        }
        Ok(())
    }

    fn plan_pair(
        &self,
        request: &PrepareNativeEscrowRequest,
        initialization_nonce: u128,
        funding_nonce: u128,
    ) -> Result<PrepareNativeEscrowResult, SidecarError> {
        let (initialization_message, funding_message) =
            self.plan_messages(request, initialization_nonce, funding_nonce)?;
        let initialization = self.prepare_message(initialization_message)?;
        let funding = self.prepare_message(funding_message)?;

        Ok(PrepareNativeEscrowResult::new(
            request.context.clone(),
            initialization,
            funding,
        ))
    }

    fn plan_messages(
        &self,
        request: &PrepareNativeEscrowRequest,
        initialization_nonce: u128,
        funding_nonce: u128,
    ) -> Result<(Message, Message), SidecarError> {
        let terms = &request.terms;
        let swap_id = *terms.swap_id().as_bytes();
        let metadata = spel_framework_core::pda::compute_pda(&self.escrow_program_id, &[&swap_id]);
        let custody_label = spel_framework_core::pda::seed_from_str("custody");
        let custody = spel_framework_core::pda::compute_pda(
            &self.escrow_program_id,
            &[&custody_label, &swap_id],
        );
        let depositor = AccountId::new(*terms.depositor_account_id().as_bytes());
        let claimant = AccountId::new(*terms.claimant_account_id().as_bytes());
        let authenticated_transfer_program =
            program_id_from_hex(terms.authenticated_transfer_program_id());

        let initialization = Message::try_new(
            self.escrow_program_id,
            vec![metadata, custody, depositor, claimant],
            vec![initialization_nonce.into()],
            EscrowInstruction::InitializeNative {
                swap_id,
                terms_hash: *terms.terms_hash().as_bytes(),
                secret_digest: *terms.secret_digest().as_bytes(),
                amount: terms.amount().as_u128(),
                refund_at: terms.refund_at_ms(),
                authenticated_transfer_program,
            },
        )
        .map_err(|_| SidecarError::InstructionEncoding)?;
        let funding = Message::try_new(
            self.escrow_program_id,
            vec![metadata, custody, depositor],
            vec![funding_nonce.into()],
            EscrowInstruction::FundNative { swap_id },
        )
        .map_err(|_| SidecarError::InstructionEncoding)?;
        Ok((initialization, funding))
    }

    fn prepare_message(&self, message: Message) -> Result<PreparedTransaction, SidecarError> {
        let witnesses = WitnessSet::for_message(&message, &[&self.signer_key]);
        prepared_from_transaction(&PublicTransaction::new(message, witnesses))
    }
}

/// Converts one official public transaction into the bridge's exact persisted form.
///
/// # Errors
///
/// Returns an error if the exact official encoding exceeds the protocol bound.
pub fn prepared_from_transaction(
    transaction: &PublicTransaction,
) -> Result<PreparedTransaction, SidecarError> {
    let exact_bytes = ExactTransactionBytes::new(transaction.to_bytes())?;
    Ok(PreparedTransaction::new(
        TransactionId::from_bytes(transaction.hash()),
        exact_bytes,
    ))
}

/// Decodes and validates persisted inner official transaction bytes for one role.
///
/// # Errors
///
/// Returns an error for the wrong role or signer, non-canonical bytes, a hash/ID
/// mismatch, or a missing, malformed, or invalid official witness set.
pub fn decode_prepared_for_role(
    prepared: &PreparedTransaction,
    transaction_role: Participant,
    sidecar_role: Participant,
    expected_signer: AccountId,
) -> Result<PublicTransaction, SidecarError> {
    if transaction_role != sidecar_role {
        return Err(SidecarError::WrongSidecarRole);
    }
    let transaction = PublicTransaction::from_bytes(prepared.exact_bytes.as_slice())
        .map_err(|_| SidecarError::InvalidTransactionBytes)?;
    if transaction.to_bytes() != prepared.exact_bytes.as_slice() {
        return Err(SidecarError::InvalidTransactionBytes);
    }
    if transaction.hash() != *prepared.transaction_id.as_bytes() {
        return Err(SidecarError::WrongTransactionId);
    }
    let witnesses = transaction.witness_set();
    if transaction.message().nonces.len() != witnesses.signatures_and_public_keys().len()
        || witnesses.signatures_and_public_keys().is_empty()
        || !witnesses.is_valid_for(transaction.message())
    {
        return Err(SidecarError::InvalidSignature);
    }
    let signer_ids = witnesses
        .signatures_and_public_keys()
        .iter()
        .map(|(_, key)| AccountId::from(key))
        .collect::<Vec<_>>();
    if signer_ids != [expected_signer] {
        return Err(SidecarError::WrongSigner);
    }
    Ok(transaction)
}

fn program_id_to_hex(program_id: [u32; 8]) -> Hex32 {
    let mut bytes = [0_u8; 32];
    for (chunk, word) in bytes.chunks_exact_mut(4).zip(program_id) {
        chunk.copy_from_slice(&word.to_le_bytes());
    }
    Hex32::from_bytes(bytes)
}

fn program_id_from_hex(value: Hex32) -> [u32; 8] {
    let mut program_id = [0_u32; 8];
    for (word, chunk) in program_id.iter_mut().zip(value.as_bytes().chunks_exact(4)) {
        *word = u32::from_le_bytes(chunk.try_into().expect("four-byte chunk"));
    }
    program_id
}

fn settlement(status: &BedrockStatus) -> OfficialSettlement {
    match status {
        BedrockStatus::Pending => OfficialSettlement::Pending,
        BedrockStatus::Safe => OfficialSettlement::Safe,
        BedrockStatus::Finalized => OfficialSettlement::Finalized,
    }
}

fn validate_block_hash(block: &Block) -> Result<(), SidecarError> {
    if HashableBlockData::from(block.clone()).block_hash() != block.header.hash {
        return Err(SidecarError::InvalidNodeResponse);
    }
    Ok(())
}

fn validate_block_range(
    blocks: &[Block],
    start: u64,
    end: u64,
    genesis_block_hash: Hex32,
) -> Result<(), SidecarError> {
    let expected_len = end
        .checked_sub(start)
        .and_then(|distance| distance.checked_add(1))
        .and_then(|length| usize::try_from(length).ok())
        .ok_or(SidecarError::InvalidNodeResponse)?;
    if blocks.len() != expected_len {
        return Err(SidecarError::InvalidNodeResponse);
    }
    for (offset, block) in blocks.iter().enumerate() {
        let offset = u64::try_from(offset).map_err(|_| SidecarError::InvalidNodeResponse)?;
        if block.header.block_id != start + offset {
            return Err(SidecarError::InvalidNodeResponse);
        }
        validate_block_hash(block)?;
        if block.header.block_id == 0
            && Hex32::from_bytes(block.header.hash.0) != genesis_block_hash
        {
            return Err(SidecarError::InvalidNodeResponse);
        }
        if let Some(previous) = offset
            .checked_sub(1)
            .and_then(|previous| usize::try_from(previous).ok())
            .and_then(|previous| blocks.get(previous))
            && block.header.prev_block_hash != previous.header.hash
        {
            return Err(SidecarError::InvalidNodeResponse);
        }
    }
    Ok(())
}

fn validate_node_endpoint(endpoint: &str) -> Result<(), SidecarError> {
    let parsed = Url::parse(endpoint).map_err(|_| SidecarError::InvalidNodeEndpoint)?;
    let loopback = match parsed.host() {
        Some(Host::Ipv4(address)) => IpAddr::V4(address).is_loopback(),
        Some(Host::Ipv6(address)) => IpAddr::V6(address).is_loopback(),
        Some(Host::Domain(_)) | None => false,
    };
    if parsed.scheme() != "http"
        || !loopback
        || parsed.port().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.path() != "/"
    {
        return Err(SidecarError::InvalidNodeEndpoint);
    }
    Ok(())
}

fn classify_submission_error(error: ClientError) -> SidecarError {
    match error {
        ClientError::Call(error) if error.code() == -32602 => SidecarError::NodeRejected,
        _ => SidecarError::UnknownSubmissionOutcome,
    }
}
