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
use borsh::BorshDeserialize as _;
use common::{
    HashType,
    block::{BedrockStatus, Block, HashableBlockData},
    transaction::NSSATransaction,
};
use lez_bridge_protocol::{
    AccountIds, ChainPosition, ChainTip, DiscoveryWindow, EscrowMetadataFacts,
    EscrowObservationTarget, EscrowState, ExactTransactionBytes, FundingFoundFacts,
    FundingObservation, Hex32, InitializationFoundFacts, InitializationObservation,
    MAX_DISCOVERY_BLOCKS, NativeCustodyFacts, NativeFundInstructionFacts,
    NativeInitializeInstructionFacts, ObserveEscrowRequest, ObserveEscrowResult,
    ObservedTransactionFacts, Participant, PrepareNativeEscrowRequest, PrepareNativeEscrowResult,
    PrepareRevealingClaimRequest, PrepareRevealingClaimResult, PreparedTransaction,
    ProtocolValueError, RuntimeDescriptor, SubmissionOutcome, SubmitTransactionRequest,
    SubmitTransactionResult, TransactionId,
};
use lez_zec_escrow_compat::{EscrowMetadata, EscrowStatus, Instruction as EscrowInstruction};
use nssa::{
    Account, AccountId, PrivateKey, PublicKey, PublicTransaction,
    program::Program,
    public_transaction::{Message, WitnessSet},
};
use sequencer_service_rpc::{ClientError, RpcClient as _, SequencerClient, SequencerClientBuilder};
use sha2::{Digest as _, Sha256};
use tokio::sync::Mutex;
use url::{Host, Url};
use zeroize::Zeroize as _;

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
    /// Signed escrow terms assign the claimant role to another participant.
    #[error("native escrow claimant role does not match the isolated sidecar")]
    WrongClaimantRole,
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
    /// Another distinct revealing-claim nonce reservation is active.
    #[error("a distinct native revealing-claim preparation is already active")]
    ActiveClaimPrepare,
    /// Revealing preimage does not hash to the signed terms digest.
    #[error("revealing claim preimage does not match the signed terms")]
    WrongClaimPreimage,
    /// Funding identity is the impossible all-zero transaction ID.
    #[error("revealing claim funding transaction identity is invalid")]
    InvalidFundingTransaction,
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
    /// Canonical tips changed while transaction/account facts were being read.
    #[error("official node tip moved during observation")]
    MovingTip,
    /// More than one canonical transaction matched one signed-terms slot.
    #[error("official node discovery matched more than one transaction")]
    AmbiguousDiscovery,
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

    /// Observes one exact owned pair or one counterparty pair by signed terms.
    ///
    /// Implementations without an official canonical/account observation source
    /// fail closed. They must never synthesize absence from an unavailable fact.
    async fn observe_native_escrow(
        &self,
        _planner: &NativeEscrowPlanner,
        _request: &ObserveEscrowRequest,
    ) -> Result<ObserveEscrowResult, SidecarError> {
        Err(SidecarError::NodeObservationUnavailable)
    }
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

#[derive(Clone)]
struct NativeTransactionMatch {
    transaction: ObservedTransactionFacts,
    ordered_account_ids: AccountIds,
}

struct NativePairScan {
    tip_before: ChainTip,
    initialization: Option<NativeTransactionMatch>,
    funding: Option<NativeTransactionMatch>,
    fully_covered: bool,
}

#[derive(Clone)]
struct NativeAccountSnapshot {
    metadata: EscrowMetadataFacts,
    custody: NativeCustodyFacts,
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

    async fn observe_native_escrow_core(
        &self,
        planner: &NativeEscrowPlanner,
        request: &ObserveEscrowRequest,
    ) -> Result<ObserveEscrowResult, SidecarError> {
        let exact = matches!(request.target, EscrowObservationTarget::Exact { .. });
        self.validate_observe_request(request, exact)?;
        let (window, expected) = match request.target {
            EscrowObservationTarget::Exact {
                initialization_transaction_id,
                funding_transaction_id,
            } => {
                let pair = planner
                    .owned_native_pair(
                        request,
                        initialization_transaction_id,
                        funding_transaction_id,
                    )
                    .await?;
                let tip = self.read_tip().await?;
                let start = tip
                    .height
                    .saturating_sub(u64::from(MAX_DISCOVERY_BLOCKS - 1));
                (
                    DiscoveryWindow::new(start, MAX_DISCOVERY_BLOCKS)?,
                    Some((pair.initialization, pair.funding)),
                )
            }
            EscrowObservationTarget::DiscoverByTerms { window } => (window, None),
        };
        let scan = self
            .scan_native_pair(&request.terms, window, expected.as_ref())
            .await?;
        validate_pair_order(scan.initialization.as_ref(), scan.funding.as_ref())?;

        let snapshot = if scan.initialization.is_some() || scan.funding.is_some() {
            Some(self.read_native_account_snapshot(&request.terms).await?)
        } else {
            None
        };
        let tip_after = self.read_tip().await?;
        if tip_after != scan.tip_before {
            return Err(SidecarError::MovingTip);
        }

        let missing_is_absent = !exact && scan.fully_covered;
        let initialization = match scan.initialization {
            Some(found) => {
                let snapshot = snapshot.as_ref().ok_or(SidecarError::InvalidNodeResponse)?;
                InitializationObservation::found(InitializationFoundFacts::new(
                    found.transaction,
                    NativeInitializeInstructionFacts::new(
                        self.runtime.escrow_program_id,
                        found.ordered_account_ids,
                        request.terms.clone(),
                    ),
                    snapshot.metadata.clone(),
                ))
            }
            None if missing_is_absent => InitializationObservation::Absent,
            None => InitializationObservation::UnknownOrPending,
        };
        let funding = match scan.funding {
            Some(found) => {
                let snapshot = snapshot.as_ref().ok_or(SidecarError::InvalidNodeResponse)?;
                if snapshot.metadata.status == EscrowState::Empty {
                    return Err(SidecarError::InvalidNodeResponse);
                }
                FundingObservation::found(FundingFoundFacts::new(
                    found.transaction,
                    NativeFundInstructionFacts::new(
                        self.runtime.escrow_program_id,
                        found.ordered_account_ids,
                        request.terms.swap_id(),
                    ),
                    snapshot.metadata.clone(),
                    snapshot.custody.clone(),
                ))
            }
            None if missing_is_absent => FundingObservation::Absent,
            None => FundingObservation::UnknownOrPending,
        };

        Ok(ObserveEscrowResult::new(
            request.context.clone(),
            scan.tip_before,
            initialization,
            funding,
            tip_after,
        ))
    }

    async fn scan_native_pair(
        &self,
        terms: &lez_bridge_protocol::NativeEscrowTerms,
        window: DiscoveryWindow,
        expected: Option<&(PreparedTransaction, PreparedTransaction)>,
    ) -> Result<NativePairScan, SidecarError> {
        let tip_before = self.read_tip().await?;
        if window.start_height() > tip_before.height {
            return Ok(NativePairScan {
                tip_before,
                initialization: None,
                funding: None,
                fully_covered: false,
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

        let mut initialization = None;
        let mut funding = None;
        for block in blocks.iter().skip(usize::from(window.start_height() > 0)) {
            for (index, transaction) in block.body.transactions.iter().enumerate() {
                let Some((is_initialization, found)) =
                    self.decode_native_match(transaction, block, index, terms, expected)?
                else {
                    continue;
                };
                let slot = if is_initialization {
                    &mut initialization
                } else {
                    &mut funding
                };
                if slot.replace(found).is_some() {
                    return Err(if expected.is_some() {
                        SidecarError::InvalidNodeResponse
                    } else {
                        SidecarError::AmbiguousDiscovery
                    });
                }
            }
        }
        Ok(NativePairScan {
            tip_before,
            initialization,
            funding,
            fully_covered: tip_before.height >= declared_end,
        })
    }

    fn decode_native_match(
        &self,
        transaction: &NSSATransaction,
        block: &Block,
        index: usize,
        terms: &lez_bridge_protocol::NativeEscrowTerms,
        expected: Option<&(PreparedTransaction, PreparedTransaction)>,
    ) -> Result<Option<(bool, NativeTransactionMatch)>, SidecarError> {
        let transaction_id = TransactionId::from_bytes(transaction.hash().0);
        let exact_kind = expected.and_then(|(initialization, funding)| {
            if transaction_id == initialization.transaction_id {
                Some((true, initialization))
            } else if transaction_id == funding.transaction_id {
                Some((false, funding))
            } else {
                None
            }
        });
        if expected.is_some() && exact_kind.is_none() {
            return Ok(None);
        }
        let NSSATransaction::Public(public) = transaction else {
            return if exact_kind.is_some() {
                Err(SidecarError::InvalidNodeResponse)
            } else {
                Ok(None)
            };
        };
        if public.message().program_id != program_id_from_hex(self.runtime.escrow_program_id) {
            return if exact_kind.is_some() {
                Err(SidecarError::WrongEscrowProgram)
            } else {
                Ok(None)
            };
        }
        let [nonce] = public.message().nonces.as_slice() else {
            return if exact_kind.is_some() {
                Err(SidecarError::InvalidTransactionBytes)
            } else {
                Ok(None)
            };
        };
        let (expected_initialization, expected_funding) = native_messages(
            terms,
            program_id_from_hex(self.runtime.escrow_program_id),
            u128::from(*nonce),
        )?;
        let is_initialization = public.message() == &expected_initialization;
        let is_funding = public.message() == &expected_funding;
        let prepared = prepared_from_transaction(public)?;
        if let Some((must_initialize, persisted)) = exact_kind {
            if &prepared != persisted
                || must_initialize != is_initialization
                || must_initialize == is_funding
            {
                return Err(SidecarError::InvalidTransactionBytes);
            }
        } else if !is_initialization && !is_funding {
            return Ok(None);
        }
        let decoded = decode_prepared_for_role(
            &prepared,
            self.role,
            self.role,
            AccountId::new(*terms.depositor_account_id().as_bytes()),
        )?;
        let signer_account_ids = decoded
            .witness_set()
            .signatures_and_public_keys()
            .iter()
            .map(|(_, key)| Hex32::from_bytes(AccountId::from(key).into_value()))
            .collect::<Vec<_>>();
        Ok(Some((
            is_initialization,
            NativeTransactionMatch {
                transaction: ObservedTransactionFacts::new(
                    prepared.transaction_id,
                    prepared.exact_bytes,
                    ChainPosition::new(
                        Hex32::from_bytes(block.header.hash.0),
                        block.header.block_id,
                        u32::try_from(index).map_err(|_| SidecarError::InvalidNodeResponse)?,
                    ),
                    AccountIds::new(signer_account_ids)?,
                    true,
                ),
                ordered_account_ids: AccountIds::new(
                    decoded
                        .message()
                        .account_ids
                        .iter()
                        .map(|account| Hex32::from_bytes(account.into_value()))
                        .collect(),
                )?,
            },
        )))
    }

    async fn read_native_account_snapshot(
        &self,
        terms: &lez_bridge_protocol::NativeEscrowTerms,
    ) -> Result<NativeAccountSnapshot, SidecarError> {
        let swap_id = *terms.swap_id().as_bytes();
        let metadata_id = spel_framework_core::pda::compute_pda(
            &program_id_from_hex(self.runtime.escrow_program_id),
            &[&swap_id],
        );
        let custody_label = spel_framework_core::pda::seed_from_str("custody");
        let custody_id = spel_framework_core::pda::compute_pda(
            &program_id_from_hex(self.runtime.escrow_program_id),
            &[&custody_label, &swap_id],
        );
        let metadata_account = self.read_account(metadata_id).await?;
        let custody_account = self.read_account(custody_id).await?;
        let metadata = EscrowMetadata::try_from_slice(metadata_account.data.as_ref())
            .map_err(|_| SidecarError::InvalidNodeResponse)?;
        validate_metadata(
            &metadata,
            terms,
            custody_id,
            metadata_account.program_owner,
            program_id_from_hex(self.runtime.escrow_program_id),
        )?;
        let status = escrow_state(metadata.status);
        let expected_balance = match status {
            EscrowState::Empty | EscrowState::Claimed | EscrowState::Refunded => 0,
            EscrowState::Funded => terms.amount().as_u128(),
        };
        let authenticated_transfer = program_id_from_hex(terms.authenticated_transfer_program_id());
        if custody_account.program_owner != authenticated_transfer
            || custody_account.balance != expected_balance
        {
            return Err(SidecarError::InvalidNodeResponse);
        }
        Ok(NativeAccountSnapshot {
            metadata: EscrowMetadataFacts::from_native_terms(
                Hex32::from_bytes(metadata_id.into_value()),
                self.runtime.escrow_program_id,
                Hex32::from_bytes(custody_id.into_value()),
                terms,
                status,
            ),
            custody: NativeCustodyFacts::new(
                Hex32::from_bytes(custody_id.into_value()),
                terms.authenticated_transfer_program_id(),
                custody_account.balance,
            ),
        })
    }

    async fn read_account(&self, account_id: AccountId) -> Result<Account, SidecarError> {
        self.client
            .get_account(account_id)
            .await
            .map_err(|_| SidecarError::NodeObservationUnavailable)
    }

    fn validate_observe_request(
        &self,
        request: &ObserveEscrowRequest,
        exact: bool,
    ) -> Result<(), SidecarError> {
        if request.runtime != self.runtime {
            return Err(SidecarError::WrongRuntimeIdentity);
        }
        if request.context.sidecar_role != self.role || request.runtime.sidecar_role != self.role {
            return Err(SidecarError::WrongSidecarRole);
        }
        if request.runtime.compatibility != lez_bridge_protocol::RuntimeCompatibility::NssaV0_1_2 {
            return Err(SidecarError::WrongRuntimeCompatibility);
        }
        if request.runtime.escrow_program_id
            != program_id_to_hex(program_id_from_hex(self.runtime.escrow_program_id))
        {
            return Err(SidecarError::WrongEscrowProgram);
        }
        if request.terms.authenticated_transfer_program_id()
            != program_id_to_hex(Program::authenticated_transfer_program().id())
        {
            return Err(SidecarError::WrongAuthenticatedTransferProgram);
        }
        let signer = Hex32::from_bytes(self.signer_account_id.into_value());
        if exact {
            if request.terms.depositor() != self.role {
                return Err(SidecarError::WrongDepositorRole);
            }
            if request.terms.depositor_account_id() != signer {
                return Err(SidecarError::WrongSigner);
            }
        } else {
            if request.terms.claimant() != self.role || request.terms.depositor() == self.role {
                return Err(SidecarError::WrongClaimantRole);
            }
            if request.terms.claimant_account_id() != signer {
                return Err(SidecarError::WrongSigner);
            }
        }
        Ok(())
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

    async fn observe_native_escrow(
        &self,
        planner: &NativeEscrowPlanner,
        request: &ObserveEscrowRequest,
    ) -> Result<ObserveEscrowResult, SidecarError> {
        self.observe_native_escrow_core(planner, request).await
    }
}

#[derive(Clone)]
struct ActivePrepare {
    request: PrepareNativeEscrowRequest,
    result: PrepareNativeEscrowResult,
}

#[derive(Clone)]
struct ActiveClaimPrepare {
    request_sha256: [u8; 32],
    result: PrepareRevealingClaimResult,
}

#[derive(Default)]
struct PlannerState {
    native: Option<ActivePrepare>,
    claim: Option<ActiveClaimPrepare>,
}

/// One-role, one-signer native planner for an isolated composed run.
pub struct NativeEscrowPlanner {
    role: Participant,
    signer_key: PrivateKey,
    signer_account_id: AccountId,
    escrow_program_id: [u32; 8],
    expected_runtime: RuntimeDescriptor,
    nonce_source: Arc<dyn NonceSource>,
    state: Mutex<PlannerState>,
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
            state: Mutex::new(PlannerState::default()),
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

        let mut state = self.state.lock().await;
        if state.claim.is_some() {
            return Err(SidecarError::ActivePrepare);
        }
        if let Some(active) = state.native.as_ref() {
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
        state.native = Some(ActivePrepare {
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

        let mut state = self.state.lock().await;
        if state.claim.is_some() {
            return Err(SidecarError::ActivePrepare);
        }
        if let Some(active) = state.native.as_ref() {
            return if active.request == request && active.result == result {
                Ok(())
            } else {
                Err(SidecarError::ActivePrepare)
            };
        }
        state.native = Some(ActivePrepare { request, result });
        Ok(())
    }

    /// Prepares and caches one exact official native revealing-claim transaction.
    ///
    /// The pinned guest ABI signs `ClaimNative { swap_id, preimage }` over the
    /// exact ordered accounts `[metadata, custody, claimant]`. The funding
    /// transaction ID is not a guest field; it is instead bound to the complete
    /// cached bridge request so a different funding identity cannot reuse the
    /// nonce reservation or randomized signature.
    ///
    /// # Errors
    ///
    /// Returns an error for role/runtime/terms/signer/preimage/funding mismatch,
    /// another active preparation, unavailable nonce, or official encoding failure.
    pub async fn prepare_revealing_claim(
        &self,
        request: &PrepareRevealingClaimRequest,
    ) -> Result<PrepareRevealingClaimResult, SidecarError> {
        self.validate_claim_request(request)?;
        let request_sha256 = claim_request_sha256(request)?;
        let mut state = self.state.lock().await;
        if state.native.is_some() {
            return Err(SidecarError::ActiveClaimPrepare);
        }
        if let Some(active) = state.claim.as_ref() {
            return if active.request_sha256 == request_sha256 {
                Ok(active.result.clone())
            } else {
                Err(SidecarError::ActiveClaimPrepare)
            };
        }
        let nonce = self
            .nonce_source
            .account_nonce(self.signer_account_id)
            .await?;
        let message = self.claim_message(request, nonce)?;
        let result = PrepareRevealingClaimResult::new(
            request.context.clone(),
            self.prepare_message(message)?,
        );
        state.claim = Some(ActiveClaimPrepare {
            request_sha256,
            result: result.clone(),
        });
        Ok(result)
    }

    /// Restores an exact durably cached revealing claim without obtaining a
    /// nonce or reconstructing its randomized signature.
    ///
    /// # Errors
    ///
    /// Returns an error unless the official transaction, signer, nonce,
    /// program, ordered accounts, instruction, context, and request fingerprint
    /// all match this isolated planner.
    pub async fn restore_revealing_claim(
        &self,
        request: &PrepareRevealingClaimRequest,
        result: PrepareRevealingClaimResult,
    ) -> Result<(), SidecarError> {
        self.validate_claim_request(request)?;
        if result.context != request.context {
            return Err(SidecarError::ProtocolEncoding);
        }
        let claim =
            decode_prepared_for_role(&result.claim, self.role, self.role, self.signer_account_id)?;
        let [nonce] = claim.message.nonces.as_slice() else {
            return Err(SidecarError::InvalidTransactionBytes);
        };
        if claim.message != self.claim_message(request, u128::from(*nonce))? {
            return Err(SidecarError::InvalidTransactionBytes);
        }
        let request_sha256 = claim_request_sha256(request)?;
        let mut state = self.state.lock().await;
        if state.native.is_some() {
            return Err(SidecarError::ActiveClaimPrepare);
        }
        if let Some(active) = state.claim.as_ref() {
            return if active.request_sha256 == request_sha256 && active.result == result {
                Ok(())
            } else {
                Err(SidecarError::ActiveClaimPrepare)
            };
        }
        state.claim = Some(ActiveClaimPrepare {
            request_sha256,
            result,
        });
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
        let state = self.state.lock().await;
        let native_match = state.native.as_ref().is_some_and(|active| {
            prepared == &active.result.initialization || prepared == &active.result.funding
        });
        let claim_match = state
            .claim
            .as_ref()
            .is_some_and(|active| prepared == &active.result.claim);
        if !native_match && !claim_match {
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

    async fn owned_native_pair(
        &self,
        request: &ObserveEscrowRequest,
        initialization_transaction_id: TransactionId,
        funding_transaction_id: TransactionId,
    ) -> Result<PrepareNativeEscrowResult, SidecarError> {
        if request.runtime != self.expected_runtime
            || request.context.sidecar_role != self.role
            || request.terms.depositor() != self.role
            || request.terms.depositor_account_id()
                != Hex32::from_bytes(self.signer_account_id.into_value())
        {
            return Err(SidecarError::WrongRuntimeIdentity);
        }
        let state = self.state.lock().await;
        let active = state
            .native
            .as_ref()
            .ok_or(SidecarError::TransactionNotPrepared)?;
        if active.request.runtime != request.runtime
            || active.request.terms != request.terms
            || active.result.initialization.transaction_id != initialization_transaction_id
            || active.result.funding.transaction_id != funding_transaction_id
        {
            return Err(SidecarError::TransactionNotPrepared);
        }
        Ok(active.result.clone())
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

    fn validate_claim_request(
        &self,
        request: &PrepareRevealingClaimRequest,
    ) -> Result<(), SidecarError> {
        if request.context.sidecar_role != self.role || request.runtime.sidecar_role != self.role {
            return Err(SidecarError::WrongSidecarRole);
        }
        if request.runtime != self.expected_runtime {
            return Err(SidecarError::WrongRuntimeIdentity);
        }
        if request.runtime.compatibility != lez_bridge_protocol::RuntimeCompatibility::NssaV0_1_2 {
            return Err(SidecarError::WrongRuntimeCompatibility);
        }
        if request.terms.claimant() != self.role {
            return Err(SidecarError::WrongClaimantRole);
        }
        let signer = Hex32::from_bytes(self.signer_account_id.into_value());
        if request.runtime.signer_account_id != signer
            || request.terms.claimant_account_id() != signer
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
        if request.funding_transaction_id.as_bytes() == &[0; 32] {
            return Err(SidecarError::InvalidFundingTransaction);
        }
        let digest: [u8; 32] = Sha256::digest(request.preimage().expose_secret()).into();
        if request.terms.secret_digest().as_bytes() != &digest {
            return Err(SidecarError::WrongClaimPreimage);
        }
        Ok(())
    }

    fn claim_message(
        &self,
        request: &PrepareRevealingClaimRequest,
        nonce: u128,
    ) -> Result<Message, SidecarError> {
        let swap_id = *request.terms.swap_id().as_bytes();
        let metadata = spel_framework_core::pda::compute_pda(&self.escrow_program_id, &[&swap_id]);
        let custody_label = spel_framework_core::pda::seed_from_str("custody");
        let custody = spel_framework_core::pda::compute_pda(
            &self.escrow_program_id,
            &[&custody_label, &swap_id],
        );
        Message::try_new(
            self.escrow_program_id,
            vec![metadata, custody, self.signer_account_id],
            vec![nonce.into()],
            EscrowInstruction::ClaimNative {
                swap_id,
                preimage: *request.preimage().expose_secret(),
            },
        )
        .map_err(|_| SidecarError::InstructionEncoding)
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

fn claim_request_sha256(request: &PrepareRevealingClaimRequest) -> Result<[u8; 32], SidecarError> {
    let mut encoded = serde_json::to_vec(request).map_err(|_| SidecarError::ProtocolEncoding)?;
    let digest = Sha256::digest(&encoded).into();
    encoded.zeroize();
    Ok(digest)
}

fn program_id_from_hex(value: Hex32) -> [u32; 8] {
    let mut program_id = [0_u32; 8];
    for (word, chunk) in program_id.iter_mut().zip(value.as_bytes().chunks_exact(4)) {
        *word = u32::from_le_bytes(chunk.try_into().expect("four-byte chunk"));
    }
    program_id
}

fn native_messages(
    terms: &lez_bridge_protocol::NativeEscrowTerms,
    escrow_program_id: [u32; 8],
    nonce: u128,
) -> Result<(Message, Message), SidecarError> {
    let swap_id = *terms.swap_id().as_bytes();
    let metadata = spel_framework_core::pda::compute_pda(&escrow_program_id, &[&swap_id]);
    let custody_label = spel_framework_core::pda::seed_from_str("custody");
    let custody =
        spel_framework_core::pda::compute_pda(&escrow_program_id, &[&custody_label, &swap_id]);
    let depositor = AccountId::new(*terms.depositor_account_id().as_bytes());
    let claimant = AccountId::new(*terms.claimant_account_id().as_bytes());
    let initialization = Message::try_new(
        escrow_program_id,
        vec![metadata, custody, depositor, claimant],
        vec![nonce.into()],
        EscrowInstruction::InitializeNative {
            swap_id,
            terms_hash: *terms.terms_hash().as_bytes(),
            secret_digest: *terms.secret_digest().as_bytes(),
            amount: terms.amount().as_u128(),
            refund_at: terms.refund_at_ms(),
            authenticated_transfer_program: program_id_from_hex(
                terms.authenticated_transfer_program_id(),
            ),
        },
    )
    .map_err(|_| SidecarError::InstructionEncoding)?;
    let funding = Message::try_new(
        escrow_program_id,
        vec![metadata, custody, depositor],
        vec![nonce.into()],
        EscrowInstruction::FundNative { swap_id },
    )
    .map_err(|_| SidecarError::InstructionEncoding)?;
    Ok((initialization, funding))
}

fn validate_pair_order(
    initialization: Option<&NativeTransactionMatch>,
    funding: Option<&NativeTransactionMatch>,
) -> Result<(), SidecarError> {
    let (Some(initialization), Some(funding)) = (initialization, funding) else {
        return Ok(());
    };
    let initialization_position = initialization.transaction.position;
    let funding_position = funding.transaction.position;
    if (funding_position.height, funding_position.transaction_index)
        <= (
            initialization_position.height,
            initialization_position.transaction_index,
        )
    {
        return Err(SidecarError::InvalidNodeResponse);
    }
    Ok(())
}

fn validate_metadata(
    metadata: &EscrowMetadata,
    terms: &lez_bridge_protocol::NativeEscrowTerms,
    custody_id: AccountId,
    metadata_owner: [u32; 8],
    escrow_program_id: [u32; 8],
) -> Result<(), SidecarError> {
    let authenticated_transfer = program_id_from_hex(terms.authenticated_transfer_program_id());
    if metadata_owner != escrow_program_id
        || metadata.version != 1
        || metadata.swap_id != *terms.swap_id().as_bytes()
        || metadata.terms_hash != *terms.terms_hash().as_bytes()
        || metadata.secret_digest != *terms.secret_digest().as_bytes()
        || metadata.depositor.into_value() != *terms.depositor_account_id().as_bytes()
        || metadata.depositor_asset.into_value() != *terms.depositor_account_id().as_bytes()
        || metadata.claimant.into_value() != *terms.claimant_account_id().as_bytes()
        || metadata.claimant_asset.into_value() != *terms.claimant_account_id().as_bytes()
        || metadata.custody != custody_id
        || metadata.asset_program != authenticated_transfer
        || metadata.custody_program != authenticated_transfer
        || metadata.asset_definition != [0; 32]
        || metadata.amount != terms.amount().as_u128()
        || metadata.refund_at != terms.refund_at_ms()
    {
        return Err(SidecarError::InvalidNodeResponse);
    }
    Ok(())
}

const fn escrow_state(status: EscrowStatus) -> EscrowState {
    match status {
        EscrowStatus::Empty => EscrowState::Empty,
        EscrowStatus::Funded => EscrowState::Funded,
        EscrowStatus::Claimed => EscrowState::Claimed,
        EscrowStatus::Refunded => EscrowState::Refunded,
    }
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
