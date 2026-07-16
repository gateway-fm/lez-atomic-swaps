use std::{fmt, sync::Arc};

#[cfg(target_os = "linux")]
use std::path::Path;

use async_trait::async_trait;
use borsh::BorshDeserialize as _;
use common::transaction::LeeTransaction;
use lez_bridge_protocol::{
    CompleteWitnessedClaimRequest, CompleteWitnessedClaimResult, ExactMessageBytes,
    ExactTransactionBytes, Hex32, Participant, PrepareNativeEscrowRequest,
    PrepareNativeEscrowResult, PrepareNativeRefundRequest, PrepareNativeRefundResult,
    PrepareRevealingClaimRequest, PrepareRevealingClaimResult, PrepareWitnessedClaimRequest,
    PrepareWitnessedClaimResult, PrepareWitnessedEscrowRequest, PrepareWitnessedEscrowResult,
    PreparedTransaction, PreparedWitnessedClaim, ProtocolValueError, RuntimeCompatibility,
    RuntimeDescriptor, TransactionId,
};
use nssa::{
    AccountId, PrivateKey, PublicKey, PublicTransaction, Signature,
    public_transaction::{Message, WitnessSet},
};
use sha2::{Digest as _, Sha256};
use tokio::sync::Mutex;
use zeroize::{Zeroize as _, Zeroizing};

#[cfg(target_os = "linux")]
use crate::durable_reservation::{
    DurableReservationError, DurableReservationStore, ReservationKind,
};

#[allow(dead_code, unused_imports, unused_mut)]
mod generated_escrow_client {
    include!(concat!(env!("OUT_DIR"), "/zec_escrow_client_module.rs"));
}

pub use generated_escrow_client::{
    ZecEscrowInstruction, compute_custody_pda, compute_metadata_pda,
};

/// Fail-closed errors while preparing official v0.2 native escrow transactions.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum NativePrepareError {
    /// Owner-only durable reservation validation or persistence failed closed.
    #[cfg(target_os = "linux")]
    #[error("durable native reservation failed: {0}")]
    DurableReservation(#[from] DurableReservationError),
    /// The request or runtime targets a different actor process.
    #[error("request targets the wrong isolated sidecar role")]
    WrongRole,
    /// The complete runtime identity differs from the configured actor runtime.
    #[error("request runtime does not match the configured sidecar runtime")]
    WrongRuntime,
    /// The runtime or terms signer differs from the isolated key.
    #[error("runtime or escrow depositor does not match the isolated signer")]
    WrongSigner,
    /// The signed terms assign the deposit to the other participant.
    #[error("native escrow depositor role does not match the isolated sidecar")]
    WrongDepositorRole,
    /// The runtime escrow program differs from the configured deployment.
    #[error("runtime escrow program does not match the configured deployment")]
    WrongEscrowProgram,
    /// Native terms select a different authenticated-transfer deployment.
    #[error("native terms select the wrong authenticated-transfer program")]
    WrongAuthenticatedTransferProgram,
    /// Another request already owns this signer's active nonce pair.
    #[error("a distinct native escrow preparation already owns the nonce reservation")]
    ActivePrepare,
    /// Another request already owns this signer's witnessed escrow nonce pair.
    #[error("a distinct witnessed escrow preparation already owns the nonce reservation")]
    ActiveWitnessedEscrowPrepare,
    /// Another revealing claim already owns this signer's active nonce.
    #[error("a distinct native revealing claim already owns the nonce reservation")]
    ActiveClaimPrepare,
    /// Another witnessed claim already owns the aggregate authority nonce.
    #[error("a distinct witnessed claim already owns the aggregate authority nonce reservation")]
    ActiveWitnessedClaimPrepare,
    /// Another fixed-destination refund already owns this planner reservation.
    #[error("a distinct native refund preparation already owns the reservation")]
    ActiveRefundPrepare,
    /// A witnessed claim was already completed with different bytes.
    #[error("the witnessed claim reservation was already completed differently")]
    ActiveWitnessedClaimCompletion,
    /// The claim is assigned to another role or signer.
    #[error("native revealing claim does not belong to this isolated claimant")]
    WrongClaimant,
    /// Aggregate key/account identity or destination separation was invalid.
    #[error("witnessed claim aggregate authority binding is invalid")]
    WrongAggregateAuthority,
    /// The revealing preimage does not match the signed escrow digest.
    #[error("native revealing claim preimage does not match the signed digest")]
    WrongPreimage,
    /// The official account nonce was unavailable.
    #[error("official signer nonce is unavailable")]
    NonceUnavailable,
    /// The funding nonce cannot follow the initialization nonce.
    #[error("official signer nonce cannot be incremented")]
    NonceOverflow,
    /// The generated official instruction could not be encoded.
    #[error("official v0.2 escrow instruction encoding failed")]
    InstructionEncoding,
    /// The protocol could not carry the exact official transaction.
    #[error("official transaction exceeds the bounded bridge protocol")]
    ProtocolEncoding,
    /// Exact bytes are not one canonical official public transaction.
    #[error("exact bytes are not a canonical official v0.2 public transaction")]
    InvalidTransactionBytes,
    /// The official transaction hash differs from its persisted ID.
    #[error("official transaction hash differs from the persisted transaction ID")]
    WrongTransactionId,
    /// The witness set is empty, malformed, or invalid.
    #[error("official public transaction signature is invalid")]
    InvalidSignature,
}

impl From<ProtocolValueError> for NativePrepareError {
    fn from(_value: ProtocolValueError) -> Self {
        Self::ProtocolEncoding
    }
}

/// Supplies the current nonce for exactly one official public account.
#[async_trait]
pub trait NonceSource: Send + Sync {
    /// Returns the node-observed current nonce for `account_id`.
    ///
    /// # Errors
    ///
    /// Returns [`NativePrepareError::NonceUnavailable`] when the official fact
    /// cannot be obtained exactly.
    async fn account_nonce(&self, account_id: AccountId) -> Result<u128, NativePrepareError>;
}

#[derive(Clone)]
struct ActivePrepare {
    request: PrepareNativeEscrowRequest,
    result: PrepareNativeEscrowResult,
}

#[derive(Default)]
struct PlannerState {
    active: Option<ActivePrepare>,
    active_witnessed_escrow: Option<ActiveWitnessedEscrowPrepare>,
    active_claim: Option<ActiveClaimPrepare>,
    active_witnessed_claim: Option<ActiveWitnessedClaimPrepare>,
    completed_witnessed_claim: Option<ActiveWitnessedClaimCompletion>,
    active_refund: Option<ActiveRefundPrepare>,
}

#[derive(Clone)]
struct ActiveWitnessedEscrowPrepare {
    request: PrepareWitnessedEscrowRequest,
    result: PrepareWitnessedEscrowResult,
}

#[derive(Clone)]
struct ActiveClaimPrepare {
    request_sha256: [u8; 32],
    result: PrepareRevealingClaimResult,
    terms: lez_bridge_protocol::NativeEscrowTerms,
    funding_transaction_id: TransactionId,
    preimage: Zeroizing<[u8; 32]>,
}

#[derive(Clone)]
struct ActiveWitnessedClaimPrepare {
    request: PrepareWitnessedClaimRequest,
    result: PrepareWitnessedClaimResult,
}

#[derive(Clone)]
struct ActiveWitnessedClaimCompletion {
    request: CompleteWitnessedClaimRequest,
    result: CompleteWitnessedClaimResult,
}

#[derive(Clone)]
struct ActiveRefundPrepare {
    request: PrepareNativeRefundRequest,
    result: PrepareNativeRefundResult,
}

/// One-role, one-signer official v0.2 native escrow planner.
///
/// Signed operations and permissionless refunds are prepared as exact official
/// bytes. On Linux, an owner-only durable store can reserve those bytes before
/// exposure. The generic submission validator admits only active, revalidated
/// preparations; chain eligibility and one-attempt authority remain actor
/// concerns.
pub struct NativeEscrowPlanner {
    role: Participant,
    signer_key_bytes: Zeroizing<[u8; 32]>,
    signer_account_id: AccountId,
    escrow_program_id: [u32; 8],
    authenticated_transfer_program_id: [u32; 8],
    expected_runtime: RuntimeDescriptor,
    nonce_source: Arc<dyn NonceSource>,
    #[cfg(target_os = "linux")]
    durable_store: Option<DurableReservationStore>,
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
            .field(
                "authenticated_transfer_program_id",
                &program_id_to_hex(self.authenticated_transfer_program_id),
            )
            .field("expected_runtime", &self.expected_runtime)
            .field(
                "durable_store",
                &if cfg!(target_os = "linux") {
                    "[REDACTED]"
                } else {
                    "unavailable"
                },
            )
            .finish_non_exhaustive()
    }
}

impl NativeEscrowPlanner {
    /// Binds one official key, role, runtime, and the two deployed programs.
    ///
    /// # Errors
    ///
    /// Rejects cross-wired role, compatibility, signer, program, or impossible
    /// program identities before retaining the key.
    #[allow(
        clippy::needless_pass_by_value,
        reason = "the isolated planner takes ownership of the upstream key before retaining only zeroizing bytes"
    )]
    pub fn new<N>(
        role: Participant,
        signer_key: PrivateKey,
        escrow_program_id: [u32; 8],
        authenticated_transfer_program_id: [u32; 8],
        expected_runtime: RuntimeDescriptor,
        nonce_source: Arc<N>,
    ) -> Result<Self, NativePrepareError>
    where
        N: NonceSource + 'static,
    {
        let signer_account_id = AccountId::from(&PublicKey::new_from_private_key(&signer_key));
        if expected_runtime.sidecar_role != role {
            return Err(NativePrepareError::WrongRole);
        }
        if expected_runtime.compatibility != RuntimeCompatibility::LeeV0_2_0 {
            return Err(NativePrepareError::WrongRuntime);
        }
        if expected_runtime.signer_account_id != Hex32::from_bytes(signer_account_id.into_value()) {
            return Err(NativePrepareError::WrongSigner);
        }
        if escrow_program_id == [0; 8]
            || expected_runtime.escrow_program_id != program_id_to_hex(escrow_program_id)
        {
            return Err(NativePrepareError::WrongEscrowProgram);
        }
        if authenticated_transfer_program_id == [0; 8]
            || authenticated_transfer_program_id == escrow_program_id
        {
            return Err(NativePrepareError::WrongAuthenticatedTransferProgram);
        }
        let signer_key_bytes = Zeroizing::new(*signer_key.value());
        let nonce_source: Arc<dyn NonceSource> = nonce_source;
        Ok(Self {
            role,
            signer_key_bytes,
            signer_account_id,
            escrow_program_id,
            authenticated_transfer_program_id,
            expected_runtime,
            nonce_source,
            #[cfg(target_os = "linux")]
            durable_store: None,
            state: Mutex::new(PlannerState::default()),
        })
    }

    /// Binds this planner to one existing owner-only actor state directory.
    ///
    /// The held directory descriptor and fixed reservation filename prevent
    /// path substitution after construction. Maker and taker processes must
    /// pass distinct directories.
    ///
    /// # Errors
    ///
    /// Returns the same configuration errors as [`Self::new`] and rejects a
    /// symlinked, non-directory, foreign-owned, or non-`0700` state directory.
    #[cfg(target_os = "linux")]
    pub fn new_durable<N, P>(
        role: Participant,
        signer_key: PrivateKey,
        escrow_program_id: [u32; 8],
        authenticated_transfer_program_id: [u32; 8],
        expected_runtime: RuntimeDescriptor,
        nonce_source: Arc<N>,
        state_directory: P,
    ) -> Result<Self, NativePrepareError>
    where
        N: NonceSource + 'static,
        P: AsRef<Path>,
    {
        let mut planner = Self::new(
            role,
            signer_key,
            escrow_program_id,
            authenticated_transfer_program_id,
            expected_runtime,
            nonce_source,
        )?;
        planner.durable_store = Some(DurableReservationStore::open(state_directory.as_ref())?);
        Ok(planner)
    }

    /// Prepares and caches one signed initialization/funding nonce pair.
    ///
    /// An identical replay returns the originally signed exact bytes. A
    /// distinct request fails closed while this one-swap signer instance lives.
    /// Durable per-operation reservations and concurrent swaps remain required
    /// before this fail-closed foundation can become a submitting sidecar.
    ///
    /// # Errors
    ///
    /// Rejects identity drift before consulting the nonce source, nonce
    /// overflow, instruction/transaction encoding failure, and a distinct
    /// active reservation.
    pub async fn prepare(
        &self,
        request: PrepareNativeEscrowRequest,
    ) -> Result<PrepareNativeEscrowResult, NativePrepareError> {
        self.validate_request(&request)?;
        let mut state = self.state.lock().await;
        if state.active_witnessed_escrow.is_some() {
            return Err(NativePrepareError::ActivePrepare);
        }
        if let Some(active) = state.active.as_ref() {
            return if active.request == request {
                Ok(active.result.clone())
            } else {
                Err(NativePrepareError::ActivePrepare)
            };
        }

        #[cfg(target_os = "linux")]
        if let Some(recovered) = self.recover_durable(&request)? {
            state.active = Some(ActivePrepare {
                request,
                result: recovered.clone(),
            });
            return Ok(recovered);
        }

        let initialization_nonce = self
            .nonce_source
            .account_nonce(self.signer_account_id)
            .await?;
        let funding_nonce = initialization_nonce
            .checked_add(1)
            .ok_or(NativePrepareError::NonceOverflow)?;
        let result = self.plan_pair(&request, initialization_nonce, funding_nonce)?;
        #[cfg(target_os = "linux")]
        if let Some(store) = self.durable_store.as_ref()
            && let Err(error) = store.create(ReservationKind::NativeEscrow, &request, &result)
        {
            if error == DurableReservationError::AlreadyReserved {
                let recovered = self
                    .recover_durable(&request)?
                    .ok_or(DurableReservationError::Filesystem)?;
                state.active = Some(ActivePrepare {
                    request,
                    result: recovered.clone(),
                });
                return Ok(recovered);
            }
            return Err(error.into());
        }
        state.active = Some(ActivePrepare {
            request,
            result: result.clone(),
        });
        Ok(result)
    }

    /// Prepares and durably reserves one witnessed initialization/funding pair.
    ///
    /// The generated `InitializeNativeWitnessed` ABI fixes the ordered
    /// metadata, custody, depositor, claimant, and aggregate-authority accounts.
    /// The isolated depositor signs both consecutive-nonce transactions.
    /// Identical replay returns the original exact bytes; submission remains a
    /// separate operation.
    ///
    /// # Errors
    ///
    /// Rejects role, runtime, program, signer, authority, nonce, canonical
    /// encoding, durable-state, or active-reservation drift.
    pub async fn prepare_witnessed_escrow(
        &self,
        request: &PrepareWitnessedEscrowRequest,
    ) -> Result<PrepareWitnessedEscrowResult, NativePrepareError> {
        self.validate_witnessed_escrow_request(request)?;
        let mut state = self.state.lock().await;
        if state.active.is_some() {
            return Err(NativePrepareError::ActiveWitnessedEscrowPrepare);
        }
        if let Some(active) = state.active_witnessed_escrow.as_ref() {
            return if active.request == *request {
                self.validate_prepared_witnessed_escrow(&active.request, &active.result)?;
                Ok(active.result.clone())
            } else {
                Err(NativePrepareError::ActiveWitnessedEscrowPrepare)
            };
        }

        #[cfg(target_os = "linux")]
        if let Some(recovered) = self.recover_durable_witnessed_escrow(request)? {
            state.active_witnessed_escrow = Some(ActiveWitnessedEscrowPrepare {
                request: request.clone(),
                result: recovered.clone(),
            });
            return Ok(recovered);
        }

        let initialization_nonce = self
            .nonce_source
            .account_nonce(self.signer_account_id)
            .await?;
        let funding_nonce = initialization_nonce
            .checked_add(1)
            .ok_or(NativePrepareError::NonceOverflow)?;
        let result = self.plan_witnessed_pair(request, initialization_nonce, funding_nonce)?;
        #[cfg(target_os = "linux")]
        if let Some(store) = self.durable_store.as_ref()
            && let Err(error) = store.create(ReservationKind::WitnessedEscrow, request, &result)
        {
            if error == DurableReservationError::AlreadyReserved {
                let recovered = self
                    .recover_durable_witnessed_escrow(request)?
                    .ok_or(DurableReservationError::Filesystem)?;
                state.active_witnessed_escrow = Some(ActiveWitnessedEscrowPrepare {
                    request: request.clone(),
                    result: recovered.clone(),
                });
                return Ok(recovered);
            }
            return Err(error.into());
        }
        state.active_witnessed_escrow = Some(ActiveWitnessedEscrowPrepare {
            request: request.clone(),
            result: result.clone(),
        });
        Ok(result)
    }

    /// Prepares one exact permissionless fixed-destination native refund.
    ///
    /// The official ABI has three ordered accounts and deliberately carries no
    /// signer nonce or witness. Exact bytes are durably installed before they
    /// can be returned or admitted by the generic submission boundary.
    ///
    /// # Errors
    ///
    /// Rejects role, runtime, depositor, program, witnessed-authority, canonical
    /// encoding, durable-state, or active-reservation drift.
    pub async fn prepare_native_refund(
        &self,
        request: &PrepareNativeRefundRequest,
    ) -> Result<PrepareNativeRefundResult, NativePrepareError> {
        self.validate_refund_request(request)?;
        let mut state = self.state.lock().await;
        if let Some(active) = state.active_refund.as_ref() {
            return if active.request == *request {
                self.validate_prepared_refund(&active.request, &active.result)?;
                Ok(active.result.clone())
            } else {
                Err(NativePrepareError::ActiveRefundPrepare)
            };
        }

        #[cfg(target_os = "linux")]
        if let Some(recovered) = self.recover_durable_refund(request)? {
            state.active_refund = Some(ActiveRefundPrepare {
                request: request.clone(),
                result: recovered.clone(),
            });
            return Ok(recovered);
        }

        let transaction = PublicTransaction::new(
            self.refund_message(request)?,
            WitnessSet::from_raw_parts(Vec::new()),
        );
        let result = PrepareNativeRefundResult::new(
            request.context.clone(),
            prepared_from_transaction(&transaction)?,
        );
        self.validate_prepared_refund(request, &result)?;
        #[cfg(target_os = "linux")]
        if let Some(store) = self.durable_store.as_ref()
            && let Err(error) = store.create(ReservationKind::NativeRefund, request, &result)
        {
            if error == DurableReservationError::AlreadyReserved {
                let recovered = self
                    .recover_durable_refund(request)?
                    .ok_or(DurableReservationError::Filesystem)?;
                state.active_refund = Some(ActiveRefundPrepare {
                    request: request.clone(),
                    result: recovered.clone(),
                });
                return Ok(recovered);
            }
            return Err(error.into());
        }
        state.active_refund = Some(ActiveRefundPrepare {
            request: request.clone(),
            result: result.clone(),
        });
        Ok(result)
    }

    /// Prepares and durably reserves one official v0.2 revealing claim.
    ///
    /// The exact generated `ClaimNative` ABI binds the ordered metadata,
    /// custody, and claimant accounts. An identical replay returns the
    /// originally signed bytes rather than consuming another nonce.
    ///
    /// # Errors
    ///
    /// Rejects runtime, role, signer, terms, preimage, nonce, durable-state,
    /// canonical encoding, or active-reservation drift.
    pub async fn prepare_revealing_claim(
        &self,
        request: &PrepareRevealingClaimRequest,
    ) -> Result<PrepareRevealingClaimResult, NativePrepareError> {
        self.validate_claim_request(request)?;
        let request_sha256 = claim_request_sha256(request)?;
        let mut state = self.state.lock().await;
        if let Some(active) = state.active_claim.as_ref() {
            return if active.request_sha256 == request_sha256 {
                Ok(active.result.clone())
            } else {
                Err(NativePrepareError::ActiveClaimPrepare)
            };
        }

        #[cfg(target_os = "linux")]
        if let Some(recovered) = self.recover_durable_claim(request)? {
            state.active_claim = Some(ActiveClaimPrepare {
                request_sha256,
                result: recovered.clone(),
                terms: request.terms.clone(),
                funding_transaction_id: request.funding_transaction_id,
                preimage: Zeroizing::new(*request.preimage().expose_secret()),
            });
            return Ok(recovered);
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
        #[cfg(target_os = "linux")]
        if let Some(store) = self.durable_store.as_ref()
            && let Err(error) = store.create(ReservationKind::NativeClaim, request, &result)
        {
            if error == DurableReservationError::AlreadyReserved {
                let recovered = self
                    .recover_durable_claim(request)?
                    .ok_or(DurableReservationError::Filesystem)?;
                state.active_claim = Some(ActiveClaimPrepare {
                    request_sha256,
                    result: recovered.clone(),
                    terms: request.terms.clone(),
                    funding_transaction_id: request.funding_transaction_id,
                    preimage: Zeroizing::new(*request.preimage().expose_secret()),
                });
                return Ok(recovered);
            }
            return Err(error.into());
        }
        state.active_claim = Some(ActiveClaimPrepare {
            request_sha256,
            result: result.clone(),
            terms: request.terms.clone(),
            funding_transaction_id: request.funding_transaction_id,
            preimage: Zeroizing::new(*request.preimage().expose_secret()),
        });
        Ok(result)
    }

    /// Reserves the exact unsigned official message used by both aggregate signers.
    ///
    /// The aggregate authority's current nonce is read once, and the exact
    /// canonical message bytes/hash are durably installed before being exposed.
    /// An identical replay returns the same transcript without rereading a nonce.
    ///
    /// # Errors
    ///
    /// Rejects role, runtime, destination, aggregate key/account, funding ID,
    /// nonce, canonical encoding, durable state, or active-reservation drift.
    pub async fn prepare_witnessed_claim(
        &self,
        request: &PrepareWitnessedClaimRequest,
    ) -> Result<PrepareWitnessedClaimResult, NativePrepareError> {
        let authority = self.validate_witnessed_claim_request(request)?;
        let mut state = self.state.lock().await;
        if let Some(active) = state.active_witnessed_claim.as_ref() {
            return if active.request == *request {
                self.validate_prepared_witnessed_claim(&active.request, &active.result)?;
                Ok(active.result.clone())
            } else {
                Err(NativePrepareError::ActiveWitnessedClaimPrepare)
            };
        }

        #[cfg(target_os = "linux")]
        if let Some(recovered) = self.recover_durable_witnessed_claim(request)? {
            state.active_witnessed_claim = Some(ActiveWitnessedClaimPrepare {
                request: request.clone(),
                result: recovered.clone(),
            });
            return Ok(recovered);
        }

        let nonce = self.nonce_source.account_nonce(authority).await?;
        let message = self.witnessed_claim_message(request, nonce)?;
        let result = PrepareWitnessedClaimResult::new(
            request.context.clone(),
            prepared_witnessed_from_message(request.context.request_id.clone(), &message)?,
        );
        self.validate_prepared_witnessed_claim(request, &result)?;
        #[cfg(target_os = "linux")]
        if let Some(store) = self.durable_store.as_ref()
            && let Err(error) = store.create(ReservationKind::WitnessedClaim, request, &result)
        {
            if error == DurableReservationError::AlreadyReserved {
                let recovered = self
                    .recover_durable_witnessed_claim(request)?
                    .ok_or(DurableReservationError::Filesystem)?;
                state.active_witnessed_claim = Some(ActiveWitnessedClaimPrepare {
                    request: request.clone(),
                    result: recovered.clone(),
                });
                return Ok(recovered);
            }
            return Err(error.into());
        }
        state.active_witnessed_claim = Some(ActiveWitnessedClaimPrepare {
            request: request.clone(),
            result: result.clone(),
        });
        Ok(result)
    }

    /// Completes one exact reservation with an externally aggregated signature.
    ///
    /// This performs no signing or curve arithmetic. The pinned official LEZ
    /// verifier checks the supplied 64-byte BIP340 signature, then this planner
    /// builds and durably retains one canonical public transaction. Submission
    /// remains a separate exact-byte operation.
    ///
    /// # Errors
    ///
    /// Rejects unstored or mutated messages, context/runtime/role drift, an
    /// invalid aggregate signature, noncanonical output, or a second differing
    /// completion for the same reservation.
    pub async fn complete_witnessed_claim(
        &self,
        request: &CompleteWitnessedClaimRequest,
    ) -> Result<CompleteWitnessedClaimResult, NativePrepareError> {
        let mut state = self.state.lock().await;
        #[cfg(target_os = "linux")]
        if state.active_witnessed_claim.is_none()
            && let Some(recovered) = self.load_durable_witnessed_claim_for_completion(request)?
        {
            state.active_witnessed_claim = Some(recovered);
        }
        let active = state
            .active_witnessed_claim
            .clone()
            .ok_or(NativePrepareError::ActiveWitnessedClaimPrepare)?;
        self.validate_witnessed_completion_request(&active, request)?;
        if let Some(completed) = state.completed_witnessed_claim.as_ref() {
            return if completed.request == *request {
                self.validate_completed_witnessed_claim(&active, request, &completed.result)?;
                Ok(completed.result.clone())
            } else {
                Err(NativePrepareError::ActiveWitnessedClaimCompletion)
            };
        }

        #[cfg(target_os = "linux")]
        if let Some(recovered) = self.recover_durable_witnessed_completion(&active, request)? {
            state.completed_witnessed_claim = Some(ActiveWitnessedClaimCompletion {
                request: request.clone(),
                result: recovered.clone(),
            });
            return Ok(recovered);
        }

        let message = decode_witnessed_message(&active.result.claim)?;
        let public_key = PublicKey::try_new(
            *active
                .request
                .terms
                .aggregate_x_only_public_key()
                .as_bytes(),
        )
        .map_err(|_| NativePrepareError::WrongAggregateAuthority)?;
        let signature = Signature {
            value: *request.aggregate_signature.as_bytes(),
        };
        if !signature.is_valid_for(&message.hash(), &public_key) {
            return Err(NativePrepareError::InvalidSignature);
        }
        let witness = WitnessSet::from_raw_parts(vec![(signature, public_key)]);
        let prepared = prepared_from_transaction(&PublicTransaction::new(message, witness))?;
        let result = CompleteWitnessedClaimResult::new(request.context.clone(), prepared);
        self.validate_completed_witnessed_claim(&active, request, &result)?;
        #[cfg(target_os = "linux")]
        if let Some(store) = self.durable_store.as_ref()
            && let Err(error) =
                store.create(ReservationKind::WitnessedClaimCompletion, request, &result)
        {
            if error == DurableReservationError::AlreadyReserved {
                let recovered = self
                    .recover_durable_witnessed_completion(&active, request)?
                    .ok_or(DurableReservationError::Filesystem)?;
                state.completed_witnessed_claim = Some(ActiveWitnessedClaimCompletion {
                    request: request.clone(),
                    result: recovered.clone(),
                });
                return Ok(recovered);
            }
            return Err(error.into());
        }
        state.completed_witnessed_claim = Some(ActiveWitnessedClaimCompletion {
            request: request.clone(),
            result: result.clone(),
        });
        Ok(result)
    }

    /// Returns the exact owned initialization/funding pair after checking the
    /// complete observation request and both persisted transaction identities.
    ///
    /// # Errors
    ///
    /// Rejects role, runtime, terms, preparation, or transaction-ID drift.
    pub async fn owned_native_pair(
        &self,
        request: &lez_bridge_protocol::ObserveEscrowRequest,
        initialization_transaction_id: TransactionId,
        funding_transaction_id: TransactionId,
    ) -> Result<PrepareNativeEscrowResult, NativePrepareError> {
        let state = self.state.lock().await;
        let active = state
            .active
            .as_ref()
            .ok_or(NativePrepareError::InvalidTransactionBytes)?;
        if active.request.context.run_id != request.context.run_id
            || active.request.context.sidecar_role != request.context.sidecar_role
            || active.request.runtime != request.runtime
            || active.request.terms != request.terms
            || active.result.initialization.transaction_id != initialization_transaction_id
            || active.result.funding.transaction_id != funding_transaction_id
        {
            return Err(NativePrepareError::InvalidTransactionBytes);
        }
        self.validate_prepared(&active.request, &active.result)?;
        Ok(active.result.clone())
    }

    /// Returns the exact owned witnessed initialization/funding pair after
    /// checking complete observation identity and persisted transaction IDs.
    ///
    /// # Errors
    ///
    /// Rejects role, runtime, witnessed terms, preparation, or transaction-ID drift.
    pub async fn owned_witnessed_pair(
        &self,
        request: &lez_bridge_protocol::ObserveWitnessedEscrowRequest,
        initialization_transaction_id: TransactionId,
        funding_transaction_id: TransactionId,
    ) -> Result<PrepareWitnessedEscrowResult, NativePrepareError> {
        let state = self.state.lock().await;
        let active = state
            .active_witnessed_escrow
            .as_ref()
            .ok_or(NativePrepareError::InvalidTransactionBytes)?;
        if active.request.context.run_id != request.context.run_id
            || active.request.context.sidecar_role != request.context.sidecar_role
            || active.request.runtime != request.runtime
            || active.request.terms != request.terms
            || active.result.initialization.transaction_id != initialization_transaction_id
            || active.result.funding.transaction_id != funding_transaction_id
        {
            return Err(NativePrepareError::InvalidTransactionBytes);
        }
        self.validate_prepared_witnessed_escrow(&active.request, &active.result)?;
        Ok(active.result.clone())
    }

    /// Returns the exact owned revealing claim and its source request after
    /// checking the complete observation identity.
    ///
    /// # Errors
    ///
    /// Rejects role, runtime, terms, preparation, or transaction-ID drift.
    pub async fn owned_revealing_claim(
        &self,
        request: &lez_bridge_protocol::ObserveRevealingClaimRequest,
        claim_transaction_id: TransactionId,
    ) -> Result<(PreparedTransaction, [u8; 32]), NativePrepareError> {
        let state = self.state.lock().await;
        let active = state
            .active_claim
            .as_ref()
            .ok_or(NativePrepareError::InvalidTransactionBytes)?;
        if active.result.context.run_id != request.context.run_id
            || active.result.context.sidecar_role != request.context.sidecar_role
            || request.runtime != self.expected_runtime
            || active.terms != request.terms
            || active.result.claim.transaction_id != claim_transaction_id
            || active.funding_transaction_id.as_bytes() == &[0; 32]
        {
            return Err(NativePrepareError::InvalidTransactionBytes);
        }
        let source = PrepareRevealingClaimRequest::new(
            active.result.context.clone(),
            self.expected_runtime.clone(),
            active.terms.clone(),
            active.funding_transaction_id,
            lez_bridge_protocol::RevealingPreimage::new(*active.preimage),
        );
        self.validate_prepared_claim(&source, &active.result)?;
        Ok((active.result.claim.clone(), *active.preimage))
    }

    /// Checks that a generic submission is one of this actor's exact active
    /// durable preparations, without reconstructing or re-signing it.
    ///
    /// # Errors
    ///
    /// Rejects every byte sequence not owned by an active escrow, claim, or
    /// refund reservation and revalidates official canonical bytes and signatures.
    pub async fn validate_owned_submission(
        &self,
        prepared: &PreparedTransaction,
    ) -> Result<(), NativePrepareError> {
        let state = self.state.lock().await;
        if let Some(active) = state.active.as_ref()
            && (&active.result.initialization == prepared || &active.result.funding == prepared)
        {
            self.validate_prepared(&active.request, &active.result)?;
            return Ok(());
        }
        if let Some(active) = state.active_witnessed_escrow.as_ref()
            && (&active.result.initialization == prepared || &active.result.funding == prepared)
        {
            self.validate_prepared_witnessed_escrow(&active.request, &active.result)?;
            return Ok(());
        }
        if let Some(active) = state.active_refund.as_ref()
            && &active.result.refund == prepared
        {
            self.validate_prepared_refund(&active.request, &active.result)?;
            return Ok(());
        }
        if let Some(active) = state.active_claim.as_ref()
            && &active.result.claim == prepared
        {
            let source = PrepareRevealingClaimRequest::new(
                active.result.context.clone(),
                self.expected_runtime.clone(),
                active.terms.clone(),
                active.funding_transaction_id,
                lez_bridge_protocol::RevealingPreimage::new(*active.preimage),
            );
            self.validate_prepared_claim(&source, &active.result)?;
            return Ok(());
        }
        if let (Some(active), Some(completed)) = (
            state.active_witnessed_claim.as_ref(),
            state.completed_witnessed_claim.as_ref(),
        ) && &completed.result.claim == prepared
        {
            self.validate_completed_witnessed_claim(active, &completed.request, &completed.result)?;
            return Ok(());
        }
        Err(NativePrepareError::InvalidTransactionBytes)
    }

    #[cfg(target_os = "linux")]
    fn recover_durable(
        &self,
        request: &PrepareNativeEscrowRequest,
    ) -> Result<Option<PrepareNativeEscrowResult>, NativePrepareError> {
        let Some(store) = self.durable_store.as_ref() else {
            return Ok(None);
        };
        if store
            .load::<PrepareWitnessedEscrowRequest, PrepareWitnessedEscrowResult>(
                ReservationKind::WitnessedEscrow,
            )?
            .is_some()
        {
            return Err(NativePrepareError::ActivePrepare);
        }
        let Some((stored_request, stored_result)) = store
            .load::<PrepareNativeEscrowRequest, PrepareNativeEscrowResult>(
                ReservationKind::NativeEscrow,
            )?
        else {
            return Ok(None);
        };
        self.validate_request(&stored_request)?;
        self.validate_prepared(&stored_request, &stored_result)?;
        if &stored_request != request {
            return Err(NativePrepareError::ActivePrepare);
        }
        Ok(Some(stored_result))
    }

    #[cfg(target_os = "linux")]
    fn recover_durable_witnessed_escrow(
        &self,
        request: &PrepareWitnessedEscrowRequest,
    ) -> Result<Option<PrepareWitnessedEscrowResult>, NativePrepareError> {
        let Some(store) = self.durable_store.as_ref() else {
            return Ok(None);
        };
        if store
            .load::<PrepareNativeEscrowRequest, PrepareNativeEscrowResult>(
                ReservationKind::NativeEscrow,
            )?
            .is_some()
        {
            return Err(NativePrepareError::ActiveWitnessedEscrowPrepare);
        }
        let Some((stored_request, stored_result)) = store
            .load::<PrepareWitnessedEscrowRequest, PrepareWitnessedEscrowResult>(
                ReservationKind::WitnessedEscrow,
            )?
        else {
            return Ok(None);
        };
        self.validate_prepared_witnessed_escrow(&stored_request, &stored_result)?;
        if &stored_request != request {
            return Err(NativePrepareError::ActiveWitnessedEscrowPrepare);
        }
        Ok(Some(stored_result))
    }

    #[cfg(target_os = "linux")]
    fn recover_durable_refund(
        &self,
        request: &PrepareNativeRefundRequest,
    ) -> Result<Option<PrepareNativeRefundResult>, NativePrepareError> {
        let Some(store) = self.durable_store.as_ref() else {
            return Ok(None);
        };
        let Some((stored_request, stored_result)) = store
            .load::<PrepareNativeRefundRequest, PrepareNativeRefundResult>(
                ReservationKind::NativeRefund,
            )?
        else {
            return Ok(None);
        };
        self.validate_prepared_refund(&stored_request, &stored_result)?;
        if &stored_request != request {
            return Err(NativePrepareError::ActiveRefundPrepare);
        }
        Ok(Some(stored_result))
    }

    #[cfg(target_os = "linux")]
    fn recover_durable_claim(
        &self,
        request: &PrepareRevealingClaimRequest,
    ) -> Result<Option<PrepareRevealingClaimResult>, NativePrepareError> {
        let Some(store) = self.durable_store.as_ref() else {
            return Ok(None);
        };
        let Some((stored_request, stored_result)) = store
            .load::<PrepareRevealingClaimRequest, PrepareRevealingClaimResult>(
                ReservationKind::NativeClaim,
            )?
        else {
            return Ok(None);
        };
        self.validate_claim_request(&stored_request)?;
        self.validate_prepared_claim(&stored_request, &stored_result)?;
        if &stored_request != request {
            return Err(NativePrepareError::ActiveClaimPrepare);
        }
        Ok(Some(stored_result))
    }

    #[cfg(target_os = "linux")]
    fn recover_durable_witnessed_claim(
        &self,
        request: &PrepareWitnessedClaimRequest,
    ) -> Result<Option<PrepareWitnessedClaimResult>, NativePrepareError> {
        let Some(store) = self.durable_store.as_ref() else {
            return Ok(None);
        };
        let Some((stored_request, stored_result)) = store
            .load::<PrepareWitnessedClaimRequest, PrepareWitnessedClaimResult>(
                ReservationKind::WitnessedClaim,
            )?
        else {
            return Ok(None);
        };
        self.validate_prepared_witnessed_claim(&stored_request, &stored_result)?;
        if &stored_request != request {
            return Err(NativePrepareError::ActiveWitnessedClaimPrepare);
        }
        Ok(Some(stored_result))
    }

    #[cfg(target_os = "linux")]
    fn load_durable_witnessed_claim_for_completion(
        &self,
        completion: &CompleteWitnessedClaimRequest,
    ) -> Result<Option<ActiveWitnessedClaimPrepare>, NativePrepareError> {
        let Some(store) = self.durable_store.as_ref() else {
            return Ok(None);
        };
        let Some((stored_request, stored_result)) = store
            .load::<PrepareWitnessedClaimRequest, PrepareWitnessedClaimResult>(
                ReservationKind::WitnessedClaim,
            )?
        else {
            return Ok(None);
        };
        self.validate_prepared_witnessed_claim(&stored_request, &stored_result)?;
        if stored_request.context.request_id != completion.claim.preparation_request_id
            || stored_request.context.run_id != completion.context.run_id
            || stored_request.context.sidecar_role != completion.context.sidecar_role
            || stored_request.runtime != completion.runtime
            || stored_result.claim != completion.claim
        {
            return Err(NativePrepareError::ActiveWitnessedClaimPrepare);
        }
        Ok(Some(ActiveWitnessedClaimPrepare {
            request: stored_request,
            result: stored_result,
        }))
    }

    #[cfg(target_os = "linux")]
    fn recover_durable_witnessed_completion(
        &self,
        active: &ActiveWitnessedClaimPrepare,
        request: &CompleteWitnessedClaimRequest,
    ) -> Result<Option<CompleteWitnessedClaimResult>, NativePrepareError> {
        let Some(store) = self.durable_store.as_ref() else {
            return Ok(None);
        };
        let Some((stored_request, stored_result)) = store
            .load::<CompleteWitnessedClaimRequest, CompleteWitnessedClaimResult>(
                ReservationKind::WitnessedClaimCompletion,
            )?
        else {
            return Ok(None);
        };
        self.validate_witnessed_completion_request(active, &stored_request)?;
        self.validate_completed_witnessed_claim(active, &stored_request, &stored_result)?;
        if &stored_request != request {
            return Err(NativePrepareError::ActiveWitnessedClaimCompletion);
        }
        Ok(Some(stored_result))
    }

    /// Validates a recovered exact pair against its original complete request.
    ///
    /// This validation primitive neither caches recovered bytes nor makes them
    /// eligible for submission.
    ///
    /// # Errors
    ///
    /// Rejects context/runtime/role/signer drift, noncanonical bytes, hash or
    /// signature substitution, nonconsecutive nonces, and any generated
    /// instruction or ordered-account mismatch.
    pub fn validate_prepared(
        &self,
        request: &PrepareNativeEscrowRequest,
        result: &PrepareNativeEscrowResult,
    ) -> Result<(), NativePrepareError> {
        self.validate_request(request)?;
        if result.context != request.context
            || result.initialization == result.funding
            || result.initialization.transaction_id == result.funding.transaction_id
        {
            return Err(NativePrepareError::InvalidTransactionBytes);
        }
        let initialization =
            decode_prepared_for_signer(&result.initialization, self.signer_account_id)?;
        let funding = decode_prepared_for_signer(&result.funding, self.signer_account_id)?;
        let [initialization_nonce] = initialization.message().nonces.as_slice() else {
            return Err(NativePrepareError::InvalidTransactionBytes);
        };
        let initialization_nonce = u128::from(*initialization_nonce);
        let expected_funding_nonce = initialization_nonce
            .checked_add(1)
            .ok_or(NativePrepareError::NonceOverflow)?;
        let [funding_nonce] = funding.message().nonces.as_slice() else {
            return Err(NativePrepareError::InvalidTransactionBytes);
        };
        if u128::from(*funding_nonce) != expected_funding_nonce {
            return Err(NativePrepareError::InvalidTransactionBytes);
        }
        let (expected_initialization, expected_funding) =
            self.plan_messages(request, initialization_nonce, expected_funding_nonce)?;
        if initialization.message() != &expected_initialization
            || funding.message() != &expected_funding
        {
            return Err(NativePrepareError::InvalidTransactionBytes);
        }
        Ok(())
    }

    /// Validates one recovered witnessed pair against its complete request.
    ///
    /// # Errors
    ///
    /// Rejects request identity, role, runtime, authority, ordered accounts,
    /// consecutive nonces, canonical bytes, IDs, signatures, or instruction drift.
    pub fn validate_prepared_witnessed_escrow(
        &self,
        request: &PrepareWitnessedEscrowRequest,
        result: &PrepareWitnessedEscrowResult,
    ) -> Result<(), NativePrepareError> {
        self.validate_witnessed_escrow_request(request)?;
        if result.context != request.context
            || result.initialization == result.funding
            || result.initialization.transaction_id == result.funding.transaction_id
        {
            return Err(NativePrepareError::InvalidTransactionBytes);
        }
        let initialization =
            decode_prepared_for_signer(&result.initialization, self.signer_account_id)?;
        let funding = decode_prepared_for_signer(&result.funding, self.signer_account_id)?;
        let [initialization_nonce] = initialization.message().nonces.as_slice() else {
            return Err(NativePrepareError::InvalidTransactionBytes);
        };
        let initialization_nonce = u128::from(*initialization_nonce);
        let funding_nonce = initialization_nonce
            .checked_add(1)
            .ok_or(NativePrepareError::NonceOverflow)?;
        let [observed_funding_nonce] = funding.message().nonces.as_slice() else {
            return Err(NativePrepareError::InvalidTransactionBytes);
        };
        if u128::from(*observed_funding_nonce) != funding_nonce {
            return Err(NativePrepareError::InvalidTransactionBytes);
        }
        let (expected_initialization, expected_funding) =
            self.plan_witnessed_messages(request, initialization_nonce, funding_nonce)?;
        if initialization.message() != &expected_initialization
            || funding.message() != &expected_funding
        {
            return Err(NativePrepareError::InvalidTransactionBytes);
        }
        Ok(())
    }

    /// Validates one recovered exact permissionless refund.
    ///
    /// # Errors
    ///
    /// Rejects request identity, role, runtime, authority, account order,
    /// instruction, nonce, witness, exact-byte, or transaction-ID drift.
    pub fn validate_prepared_refund(
        &self,
        request: &PrepareNativeRefundRequest,
        result: &PrepareNativeRefundResult,
    ) -> Result<(), NativePrepareError> {
        self.validate_refund_request(request)?;
        if result.context != request.context {
            return Err(NativePrepareError::InvalidTransactionBytes);
        }
        let expected = self.refund_message(request)?;
        decode_unsigned_refund(&result.refund, &expected)?;
        Ok(())
    }

    /// Validates one recovered exact revealing claim against its full request.
    ///
    /// # Errors
    ///
    /// Rejects any request, signer, nonce, instruction, account-order, exact
    /// bytes, transaction-ID, signature, or context substitution.
    pub fn validate_prepared_claim(
        &self,
        request: &PrepareRevealingClaimRequest,
        result: &PrepareRevealingClaimResult,
    ) -> Result<(), NativePrepareError> {
        self.validate_claim_request(request)?;
        if result.context != request.context {
            return Err(NativePrepareError::InvalidTransactionBytes);
        }
        let claim = decode_prepared_for_signer(&result.claim, self.signer_account_id)?;
        let [nonce] = claim.message().nonces.as_slice() else {
            return Err(NativePrepareError::InvalidTransactionBytes);
        };
        if claim.message() != &self.claim_message(request, u128::from(*nonce))? {
            return Err(NativePrepareError::InvalidTransactionBytes);
        }
        Ok(())
    }

    /// Recomputes and validates one stored unsigned witnessed message.
    ///
    /// # Errors
    ///
    /// Rejects request identity, role, runtime, authority, nonce, canonical
    /// encoding, message hash, instruction, or ordered-account drift.
    pub fn validate_prepared_witnessed_claim(
        &self,
        request: &PrepareWitnessedClaimRequest,
        result: &PrepareWitnessedClaimResult,
    ) -> Result<(), NativePrepareError> {
        self.validate_witnessed_claim_request(request)?;
        if result.context != request.context
            || result.claim.preparation_request_id != request.context.request_id
        {
            return Err(NativePrepareError::InvalidTransactionBytes);
        }
        let message = decode_witnessed_message(&result.claim)?;
        let [nonce] = message.nonces.as_slice() else {
            return Err(NativePrepareError::InvalidTransactionBytes);
        };
        if message != self.witnessed_claim_message(request, u128::from(*nonce))? {
            return Err(NativePrepareError::InvalidTransactionBytes);
        }
        Ok(())
    }

    fn validate_witnessed_completion_request(
        &self,
        active: &ActiveWitnessedClaimPrepare,
        request: &CompleteWitnessedClaimRequest,
    ) -> Result<(), NativePrepareError> {
        self.validate_prepared_witnessed_claim(&active.request, &active.result)?;
        if request.context.run_id != active.request.context.run_id
            || request.context.sidecar_role != self.role
            || request.context.request_id == active.request.context.request_id
            || request.runtime != active.request.runtime
            || request.runtime != self.expected_runtime
            || request.claim != active.result.claim
            || request.claim.preparation_request_id != active.request.context.request_id
        {
            return Err(NativePrepareError::InvalidTransactionBytes);
        }
        Ok(())
    }

    fn validate_completed_witnessed_claim(
        &self,
        active: &ActiveWitnessedClaimPrepare,
        request: &CompleteWitnessedClaimRequest,
        result: &CompleteWitnessedClaimResult,
    ) -> Result<(), NativePrepareError> {
        self.validate_witnessed_completion_request(active, request)?;
        if result.context != request.context {
            return Err(NativePrepareError::InvalidTransactionBytes);
        }
        let expected_authority = AccountId::new(
            *active
                .request
                .terms
                .aggregate_authority_account_id()
                .as_bytes(),
        );
        let transaction = decode_prepared_for_signer(&result.claim, expected_authority)?;
        let expected_message = decode_witnessed_message(&active.result.claim)?;
        if transaction.message() != &expected_message {
            return Err(NativePrepareError::InvalidTransactionBytes);
        }
        let [(signature, public_key)] = transaction.witness_set().signatures_and_public_keys()
        else {
            return Err(NativePrepareError::InvalidSignature);
        };
        if signature.value != *request.aggregate_signature.as_bytes()
            || public_key.value()
                != active
                    .request
                    .terms
                    .aggregate_x_only_public_key()
                    .as_bytes()
        {
            return Err(NativePrepareError::InvalidSignature);
        }
        Ok(())
    }

    fn validate_request(
        &self,
        request: &PrepareNativeEscrowRequest,
    ) -> Result<(), NativePrepareError> {
        if request.context.sidecar_role != self.role || request.runtime.sidecar_role != self.role {
            return Err(NativePrepareError::WrongRole);
        }
        if request.runtime != self.expected_runtime
            || request.runtime.compatibility != RuntimeCompatibility::LeeV0_2_0
        {
            return Err(NativePrepareError::WrongRuntime);
        }
        if request.terms.depositor() != self.role {
            return Err(NativePrepareError::WrongDepositorRole);
        }
        let signer = Hex32::from_bytes(self.signer_account_id.into_value());
        if request.runtime.signer_account_id != signer
            || request.terms.depositor_account_id() != signer
        {
            return Err(NativePrepareError::WrongSigner);
        }
        if request.runtime.escrow_program_id != program_id_to_hex(self.escrow_program_id) {
            return Err(NativePrepareError::WrongEscrowProgram);
        }
        if request.terms.authenticated_transfer_program_id()
            != program_id_to_hex(self.authenticated_transfer_program_id)
        {
            return Err(NativePrepareError::WrongAuthenticatedTransferProgram);
        }
        Ok(())
    }

    fn validate_refund_request(
        &self,
        request: &PrepareNativeRefundRequest,
    ) -> Result<(), NativePrepareError> {
        if request.context.sidecar_role != self.role || request.runtime.sidecar_role != self.role {
            return Err(NativePrepareError::WrongRole);
        }
        if request.runtime != self.expected_runtime
            || request.runtime.compatibility != RuntimeCompatibility::LeeV0_2_0
        {
            return Err(NativePrepareError::WrongRuntime);
        }
        if request.terms.depositor() != self.role {
            return Err(NativePrepareError::WrongDepositorRole);
        }
        let signer = Hex32::from_bytes(self.signer_account_id.into_value());
        if request.runtime.signer_account_id != signer
            || request.terms.depositor_account_id() != signer
        {
            return Err(NativePrepareError::WrongSigner);
        }
        if request.runtime.escrow_program_id != program_id_to_hex(self.escrow_program_id) {
            return Err(NativePrepareError::WrongEscrowProgram);
        }
        if request.terms.authenticated_transfer_program_id()
            != program_id_to_hex(self.authenticated_transfer_program_id)
        {
            return Err(NativePrepareError::WrongAuthenticatedTransferProgram);
        }
        if let Some(terms) = request.terms.witnessed() {
            let aggregate_key = PublicKey::try_new(*terms.aggregate_x_only_public_key().as_bytes())
                .map_err(|_| NativePrepareError::WrongAggregateAuthority)?;
            let authority = AccountId::from(&aggregate_key);
            if authority == self.signer_account_id
                || authority.into_value() != *terms.aggregate_authority_account_id().as_bytes()
            {
                return Err(NativePrepareError::WrongAggregateAuthority);
            }
        }
        Ok(())
    }

    fn validate_claim_request(
        &self,
        request: &PrepareRevealingClaimRequest,
    ) -> Result<(), NativePrepareError> {
        if request.context.sidecar_role != self.role || request.runtime.sidecar_role != self.role {
            return Err(NativePrepareError::WrongRole);
        }
        if request.runtime != self.expected_runtime
            || request.runtime.compatibility != RuntimeCompatibility::LeeV0_2_0
        {
            return Err(NativePrepareError::WrongRuntime);
        }
        let signer = Hex32::from_bytes(self.signer_account_id.into_value());
        if request.terms.claimant() != self.role
            || request.runtime.signer_account_id != signer
            || request.terms.claimant_account_id() != signer
        {
            return Err(NativePrepareError::WrongClaimant);
        }
        if request.runtime.escrow_program_id != program_id_to_hex(self.escrow_program_id) {
            return Err(NativePrepareError::WrongEscrowProgram);
        }
        if request.terms.authenticated_transfer_program_id()
            != program_id_to_hex(self.authenticated_transfer_program_id)
        {
            return Err(NativePrepareError::WrongAuthenticatedTransferProgram);
        }
        let observed_digest: [u8; 32] = Sha256::digest(request.preimage().expose_secret()).into();
        if observed_digest != *request.terms.secret_digest().as_bytes() {
            return Err(NativePrepareError::WrongPreimage);
        }
        Ok(())
    }

    fn validate_witnessed_claim_request(
        &self,
        request: &PrepareWitnessedClaimRequest,
    ) -> Result<AccountId, NativePrepareError> {
        if request.context.sidecar_role != self.role || request.runtime.sidecar_role != self.role {
            return Err(NativePrepareError::WrongRole);
        }
        if request.runtime != self.expected_runtime
            || request.runtime.compatibility != RuntimeCompatibility::LeeV0_2_0
        {
            return Err(NativePrepareError::WrongRuntime);
        }
        let destination = Hex32::from_bytes(self.signer_account_id.into_value());
        if request.terms.claimant() != self.role
            || request.runtime.signer_account_id != destination
            || request.terms.claimant_account_id() != destination
        {
            return Err(NativePrepareError::WrongClaimant);
        }
        if request.runtime.escrow_program_id != program_id_to_hex(self.escrow_program_id) {
            return Err(NativePrepareError::WrongEscrowProgram);
        }
        if request.terms.authenticated_transfer_program_id()
            != program_id_to_hex(self.authenticated_transfer_program_id)
        {
            return Err(NativePrepareError::WrongAuthenticatedTransferProgram);
        }
        if request.funding_transaction_id.as_bytes() == &[0; 32] {
            return Err(NativePrepareError::InvalidTransactionBytes);
        }
        let aggregate_key =
            PublicKey::try_new(*request.terms.aggregate_x_only_public_key().as_bytes())
                .map_err(|_| NativePrepareError::WrongAggregateAuthority)?;
        let authority = AccountId::from(&aggregate_key);
        if authority == self.signer_account_id
            || authority.into_value() != *request.terms.aggregate_authority_account_id().as_bytes()
        {
            return Err(NativePrepareError::WrongAggregateAuthority);
        }
        Ok(authority)
    }

    fn validate_witnessed_escrow_request(
        &self,
        request: &PrepareWitnessedEscrowRequest,
    ) -> Result<AccountId, NativePrepareError> {
        if request.context.sidecar_role != self.role || request.runtime.sidecar_role != self.role {
            return Err(NativePrepareError::WrongRole);
        }
        if request.runtime != self.expected_runtime
            || request.runtime.compatibility != RuntimeCompatibility::LeeV0_2_0
        {
            return Err(NativePrepareError::WrongRuntime);
        }
        let signer = Hex32::from_bytes(self.signer_account_id.into_value());
        if request.terms.depositor() != self.role
            || request.runtime.signer_account_id != signer
            || request.terms.depositor_account_id() != signer
        {
            return Err(NativePrepareError::WrongSigner);
        }
        if request.runtime.escrow_program_id != program_id_to_hex(self.escrow_program_id) {
            return Err(NativePrepareError::WrongEscrowProgram);
        }
        if request.terms.authenticated_transfer_program_id()
            != program_id_to_hex(self.authenticated_transfer_program_id)
        {
            return Err(NativePrepareError::WrongAuthenticatedTransferProgram);
        }
        let aggregate_key =
            PublicKey::try_new(*request.terms.aggregate_x_only_public_key().as_bytes())
                .map_err(|_| NativePrepareError::WrongAggregateAuthority)?;
        let authority = AccountId::from(&aggregate_key);
        if authority == self.signer_account_id
            || authority.into_value() != *request.terms.aggregate_authority_account_id().as_bytes()
        {
            return Err(NativePrepareError::WrongAggregateAuthority);
        }
        Ok(authority)
    }

    fn refund_message(
        &self,
        request: &PrepareNativeRefundRequest,
    ) -> Result<Message, NativePrepareError> {
        let swap_id = *request.terms.swap_id().as_bytes();
        let metadata = compute_metadata_pda(&self.escrow_program_id, &swap_id);
        let custody = compute_custody_pda(&self.escrow_program_id, &swap_id);
        let depositor = AccountId::new(*request.terms.depositor_account_id().as_bytes());
        Message::try_new(
            self.escrow_program_id,
            vec![metadata, custody, depositor],
            Vec::new(),
            ZecEscrowInstruction::RefundNative { swap_id },
        )
        .map_err(|_| NativePrepareError::InstructionEncoding)
    }

    fn witnessed_claim_message(
        &self,
        request: &PrepareWitnessedClaimRequest,
        nonce: u128,
    ) -> Result<Message, NativePrepareError> {
        let swap_id = *request.terms.swap_id().as_bytes();
        let metadata = compute_metadata_pda(&self.escrow_program_id, &swap_id);
        let custody = compute_custody_pda(&self.escrow_program_id, &swap_id);
        let claimant = AccountId::new(*request.terms.claimant_account_id().as_bytes());
        let aggregate_authority =
            AccountId::new(*request.terms.aggregate_authority_account_id().as_bytes());
        Message::try_new(
            self.escrow_program_id,
            vec![metadata, custody, claimant, aggregate_authority],
            vec![nonce.into()],
            ZecEscrowInstruction::ClaimNativeWitnessed { swap_id },
        )
        .map_err(|_| NativePrepareError::InstructionEncoding)
    }

    fn plan_pair(
        &self,
        request: &PrepareNativeEscrowRequest,
        initialization_nonce: u128,
        funding_nonce: u128,
    ) -> Result<PrepareNativeEscrowResult, NativePrepareError> {
        let (initialization, funding) =
            self.plan_messages(request, initialization_nonce, funding_nonce)?;
        let initialization = self.prepare_message(initialization)?;
        let funding = self.prepare_message(funding)?;
        Ok(PrepareNativeEscrowResult::new(
            request.context.clone(),
            initialization,
            funding,
        ))
    }

    fn plan_witnessed_pair(
        &self,
        request: &PrepareWitnessedEscrowRequest,
        initialization_nonce: u128,
        funding_nonce: u128,
    ) -> Result<PrepareWitnessedEscrowResult, NativePrepareError> {
        let (initialization, funding) =
            self.plan_witnessed_messages(request, initialization_nonce, funding_nonce)?;
        Ok(PrepareWitnessedEscrowResult::new(
            request.context.clone(),
            self.prepare_message(initialization)?,
            self.prepare_message(funding)?,
        ))
    }

    fn plan_witnessed_messages(
        &self,
        request: &PrepareWitnessedEscrowRequest,
        initialization_nonce: u128,
        funding_nonce: u128,
    ) -> Result<(Message, Message), NativePrepareError> {
        let terms = &request.terms;
        let swap_id = *terms.swap_id().as_bytes();
        let metadata = compute_metadata_pda(&self.escrow_program_id, &swap_id);
        let custody = compute_custody_pda(&self.escrow_program_id, &swap_id);
        let depositor = AccountId::new(*terms.depositor_account_id().as_bytes());
        let claimant = AccountId::new(*terms.claimant_account_id().as_bytes());
        let aggregate_authority =
            AccountId::new(*terms.aggregate_authority_account_id().as_bytes());
        let initialization = Message::try_new(
            self.escrow_program_id,
            vec![metadata, custody, depositor, claimant, aggregate_authority],
            vec![initialization_nonce.into()],
            ZecEscrowInstruction::InitializeNativeWitnessed {
                swap_id,
                terms_hash: *terms.terms_hash().as_bytes(),
                aggregate_x_only_public_key: *terms.aggregate_x_only_public_key().as_bytes(),
                amount: terms.amount().as_u128(),
                refund_at: terms.refund_at_ms(),
                authenticated_transfer_program: self.authenticated_transfer_program_id,
            },
        )
        .map_err(|_| NativePrepareError::InstructionEncoding)?;
        let funding = Message::try_new(
            self.escrow_program_id,
            vec![metadata, custody, depositor],
            vec![funding_nonce.into()],
            ZecEscrowInstruction::FundNative { swap_id },
        )
        .map_err(|_| NativePrepareError::InstructionEncoding)?;
        Ok((initialization, funding))
    }

    fn plan_messages(
        &self,
        request: &PrepareNativeEscrowRequest,
        initialization_nonce: u128,
        funding_nonce: u128,
    ) -> Result<(Message, Message), NativePrepareError> {
        let terms = &request.terms;
        let swap_id = *terms.swap_id().as_bytes();
        let metadata = compute_metadata_pda(&self.escrow_program_id, &swap_id);
        let custody = compute_custody_pda(&self.escrow_program_id, &swap_id);
        let depositor = AccountId::new(*terms.depositor_account_id().as_bytes());
        let claimant = AccountId::new(*terms.claimant_account_id().as_bytes());
        let initialization = Message::try_new(
            self.escrow_program_id,
            vec![metadata, custody, depositor, claimant],
            vec![initialization_nonce.into()],
            ZecEscrowInstruction::InitializeNative {
                swap_id,
                terms_hash: *terms.terms_hash().as_bytes(),
                secret_digest: *terms.secret_digest().as_bytes(),
                amount: terms.amount().as_u128(),
                refund_at: terms.refund_at_ms(),
                authenticated_transfer_program: self.authenticated_transfer_program_id,
            },
        )
        .map_err(|_| NativePrepareError::InstructionEncoding)?;
        let funding = Message::try_new(
            self.escrow_program_id,
            vec![metadata, custody, depositor],
            vec![funding_nonce.into()],
            ZecEscrowInstruction::FundNative { swap_id },
        )
        .map_err(|_| NativePrepareError::InstructionEncoding)?;
        Ok((initialization, funding))
    }

    fn claim_message(
        &self,
        request: &PrepareRevealingClaimRequest,
        nonce: u128,
    ) -> Result<Message, NativePrepareError> {
        let swap_id = *request.terms.swap_id().as_bytes();
        let metadata = compute_metadata_pda(&self.escrow_program_id, &swap_id);
        let custody = compute_custody_pda(&self.escrow_program_id, &swap_id);
        Message::try_new(
            self.escrow_program_id,
            vec![metadata, custody, self.signer_account_id],
            vec![nonce.into()],
            ZecEscrowInstruction::ClaimNative {
                swap_id,
                preimage: *request.preimage().expose_secret(),
            },
        )
        .map_err(|_| NativePrepareError::InstructionEncoding)
    }

    fn prepare_message(&self, message: Message) -> Result<PreparedTransaction, NativePrepareError> {
        let signer_key = PrivateKey::try_new(*self.signer_key_bytes)
            .map_err(|_| NativePrepareError::InvalidSignature)?;
        let witnesses = WitnessSet::for_message(&message, &[&signer_key]);
        prepared_from_transaction(&PublicTransaction::new(message, witnesses))
    }
}

fn prepared_witnessed_from_message(
    preparation_request_id: lez_bridge_protocol::RequestId,
    message: &Message,
) -> Result<PreparedWitnessedClaim, NativePrepareError> {
    let exact_message_bytes = ExactMessageBytes::new(
        borsh::to_vec(message).map_err(|_| NativePrepareError::ProtocolEncoding)?,
    )?;
    Ok(PreparedWitnessedClaim::new(
        preparation_request_id,
        Hex32::from_bytes(message.hash()),
        exact_message_bytes,
    ))
}

fn decode_witnessed_message(
    prepared: &PreparedWitnessedClaim,
) -> Result<Message, NativePrepareError> {
    let message = Message::try_from_slice(prepared.exact_message_bytes.as_slice())
        .map_err(|_| NativePrepareError::InvalidTransactionBytes)?;
    let canonical =
        borsh::to_vec(&message).map_err(|_| NativePrepareError::InvalidTransactionBytes)?;
    if canonical != prepared.exact_message_bytes.as_slice()
        || message.hash() != *prepared.message_hash.as_bytes()
    {
        return Err(NativePrepareError::InvalidTransactionBytes);
    }
    Ok(message)
}

fn decode_unsigned_refund(
    prepared: &PreparedTransaction,
    expected_message: &Message,
) -> Result<PublicTransaction, NativePrepareError> {
    let transaction = PublicTransaction::from_bytes(prepared.exact_bytes.as_slice())
        .map_err(|_| NativePrepareError::InvalidTransactionBytes)?;
    if transaction.to_bytes() != prepared.exact_bytes.as_slice() {
        return Err(NativePrepareError::InvalidTransactionBytes);
    }
    if transaction.hash() != *prepared.transaction_id.as_bytes() {
        return Err(NativePrepareError::WrongTransactionId);
    }
    if transaction.message() != expected_message
        || !transaction.message().nonces.is_empty()
        || !transaction
            .witness_set()
            .signatures_and_public_keys()
            .is_empty()
        || LeeTransaction::Public(transaction.clone())
            .transaction_stateless_check()
            .is_err()
    {
        return Err(NativePrepareError::InvalidTransactionBytes);
    }
    Ok(transaction)
}

fn claim_request_sha256(
    request: &PrepareRevealingClaimRequest,
) -> Result<[u8; 32], NativePrepareError> {
    let mut encoded =
        serde_json::to_vec(request).map_err(|_| NativePrepareError::ProtocolEncoding)?;
    let digest = Sha256::digest(&encoded).into();
    encoded.zeroize();
    Ok(digest)
}

/// Converts one official transaction into the bounded persisted representation.
///
/// # Errors
///
/// Returns an error when the protocol exact-byte bound is exceeded.
pub fn prepared_from_transaction(
    transaction: &PublicTransaction,
) -> Result<PreparedTransaction, NativePrepareError> {
    let exact_bytes = ExactTransactionBytes::new(transaction.to_bytes())?;
    Ok(PreparedTransaction::new(
        TransactionId::from_bytes(transaction.hash()),
        exact_bytes,
    ))
}

/// Decodes one exact persisted transaction for the expected signer.
///
/// # Errors
///
/// Rejects noncanonical bytes, ID substitution, empty or
/// malformed witness sets, invalid signatures, and signer substitution.
pub fn decode_prepared_for_signer(
    prepared: &PreparedTransaction,
    expected_signer: AccountId,
) -> Result<PublicTransaction, NativePrepareError> {
    let transaction = PublicTransaction::from_bytes(prepared.exact_bytes.as_slice())
        .map_err(|_| NativePrepareError::InvalidTransactionBytes)?;
    if transaction.to_bytes() != prepared.exact_bytes.as_slice() {
        return Err(NativePrepareError::InvalidTransactionBytes);
    }
    if transaction.hash() != *prepared.transaction_id.as_bytes() {
        return Err(NativePrepareError::WrongTransactionId);
    }
    let witnesses = transaction.witness_set();
    if witnesses.signatures_and_public_keys().is_empty()
        || transaction.message().nonces.len() != witnesses.signatures_and_public_keys().len()
        || LeeTransaction::Public(transaction.clone())
            .transaction_stateless_check()
            .is_err()
    {
        return Err(NativePrepareError::InvalidSignature);
    }
    let signer_ids = witnesses
        .signatures_and_public_keys()
        .iter()
        .map(|(_, public_key)| AccountId::from(public_key))
        .collect::<Vec<_>>();
    if signer_ids != [expected_signer] {
        return Err(NativePrepareError::WrongSigner);
    }
    Ok(transaction)
}

pub fn program_id_to_hex(program_id: [u32; 8]) -> Hex32 {
    let mut bytes = [0_u8; 32];
    for (chunk, word) in bytes.chunks_exact_mut(4).zip(program_id) {
        chunk.copy_from_slice(&word.to_le_bytes());
    }
    Hex32::from_bytes(bytes)
}

/// Converts protocol little-endian bytes into one official program ID.
#[must_use]
pub fn program_id_from_hex(value: Hex32) -> [u32; 8] {
    let mut program_id = [0_u32; 8];
    for (word, chunk) in program_id.iter_mut().zip(value.as_bytes().chunks_exact(4)) {
        let mut bytes = [0_u8; 4];
        bytes.copy_from_slice(chunk);
        *word = u32::from_le_bytes(bytes);
    }
    program_id
}
