use std::{fmt, sync::Arc};

#[cfg(target_os = "linux")]
use std::path::Path;

use async_trait::async_trait;
use lez_bridge_protocol::{
    Hex32, MessageContext, Participant, PreparedTransaction, RuntimeCompatibility,
    RuntimeDescriptor,
};
use nssa::{
    AccountId, PrivateKey, PublicKey, PublicTransaction,
    public_transaction::{Message, WitnessSet},
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use zeroize::Zeroizing;

#[cfg(target_os = "linux")]
use crate::durable_reservation::{
    DurableReservationError, DurableReservationStore, ReservationKind,
};
use crate::{NativePrepareError, decode_prepared_for_signer, prepared_from_transaction};

/// One deterministic actor's public Vault allocation.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct VaultClaimAllocation {
    role: Participant,
    owner_account_id: Hex32,
    amount: u128,
}

impl VaultClaimAllocation {
    /// Constructs an explicit nonzero role allocation.
    ///
    /// # Errors
    ///
    /// Rejects a zero allocation before any claim can be prepared.
    pub const fn new(
        role: Participant,
        owner_account_id: Hex32,
        amount: u128,
    ) -> Result<Self, VaultClaimPrepareError> {
        if amount == 0 {
            return Err(VaultClaimPrepareError::ZeroAllocation);
        }
        Ok(Self {
            role,
            owner_account_id,
            amount,
        })
    }

    /// Returns the isolated actor role receiving this allocation.
    pub const fn role(&self) -> Participant {
        self.role
    }

    /// Returns the public owner account receiving this allocation.
    pub const fn owner_account_id(&self) -> Hex32 {
        self.owner_account_id
    }

    /// Returns the exact native amount supplied to the owner's Vault.
    #[must_use]
    pub const fn amount(&self) -> u128 {
        self.amount
    }
}

/// Complete input for preparing one official public Vault Claim.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct PrepareVaultClaimRequest {
    /// Version, run, request, and isolated role identity.
    pub context: MessageContext,
    /// Complete node/runtime identity expected by this actor.
    pub runtime: RuntimeDescriptor,
    /// Explicit genesis allocation this actor is allowed to claim.
    pub allocation: VaultClaimAllocation,
    /// Owner nonce observed from the official node before this request.
    pub owner_nonce: u128,
}

impl PrepareVaultClaimRequest {
    /// Constructs one complete claim preparation request.
    pub const fn new(
        context: MessageContext,
        runtime: RuntimeDescriptor,
        allocation: VaultClaimAllocation,
        owner_nonce: u128,
    ) -> Self {
        Self {
            context,
            runtime,
            allocation,
            owner_nonce,
        }
    }
}

/// Exact signed official transaction prepared for one actor's Vault Claim.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
#[must_use]
pub struct PrepareVaultClaimResult {
    /// Exact request context echoed for correlation and role checks.
    pub context: MessageContext,
    /// Official public transaction ID and canonical inner bytes.
    pub claim: PreparedTransaction,
}

impl PrepareVaultClaimResult {
    /// Constructs one prepared Vault Claim result.
    pub const fn new(context: MessageContext, claim: PreparedTransaction) -> Self {
        Self { context, claim }
    }
}

/// Fail-closed errors while preparing an official v0.2 Vault Claim.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum VaultClaimPrepareError {
    /// Owner-only durable reservation validation or persistence failed closed.
    #[cfg(target_os = "linux")]
    #[error("durable Vault Claim reservation failed: {0}")]
    DurableReservation(#[from] DurableReservationError),
    /// The request or runtime targets a different actor process.
    #[error("request targets the wrong isolated sidecar role")]
    WrongRole,
    /// The complete runtime identity differs from the actor's configuration.
    #[error("request runtime does not match the configured sidecar runtime")]
    WrongRuntime,
    /// The runtime/allocation owner differs from the isolated signing key.
    #[error("runtime or Vault owner does not match the isolated signer")]
    WrongSigner,
    /// The requested role allocation differs from the configured allocation.
    #[error("Vault Claim allocation does not match the configured actor allocation")]
    WrongAllocation,
    /// A Vault allocation must transfer a positive amount.
    #[error("Vault Claim allocation must be nonzero")]
    ZeroAllocation,
    /// Another request already owns this signer's claim nonce.
    #[error("a distinct Vault Claim preparation already owns the nonce reservation")]
    ActivePrepare,
    /// The official owner nonce was unavailable.
    #[error("official owner nonce is unavailable")]
    NonceUnavailable,
    /// The request nonce differs from the official node observation.
    #[error("requested owner nonce differs from the official node nonce")]
    WrongNonce,
    /// The official Claim instruction could not be encoded.
    #[error("official v0.2 Vault Claim instruction encoding failed")]
    InstructionEncoding,
    /// The protocol could not carry the exact official transaction.
    #[error("official transaction exceeds the bounded bridge protocol")]
    ProtocolEncoding,
    /// Exact bytes are not the expected canonical official Claim.
    #[error("exact bytes are not the expected canonical official v0.2 Vault Claim")]
    InvalidTransactionBytes,
    /// The official transaction hash differs from its persisted ID.
    #[error("official transaction hash differs from the persisted transaction ID")]
    WrongTransactionId,
    /// The owner witness is empty, malformed, or invalid.
    #[error("official public transaction signature is invalid")]
    InvalidSignature,
}

impl From<NativePrepareError> for VaultClaimPrepareError {
    fn from(value: NativePrepareError) -> Self {
        match value {
            #[cfg(target_os = "linux")]
            NativePrepareError::DurableReservation(error) => Self::DurableReservation(error),
            NativePrepareError::WrongRole => Self::WrongRole,
            NativePrepareError::WrongRuntime => Self::WrongRuntime,
            NativePrepareError::WrongSigner => Self::WrongSigner,
            NativePrepareError::ActivePrepare
            | NativePrepareError::ActiveClaimPrepare
            | NativePrepareError::ActiveWitnessedClaimPrepare
            | NativePrepareError::ActiveWitnessedClaimCompletion => Self::ActivePrepare,
            NativePrepareError::NonceUnavailable => Self::NonceUnavailable,
            NativePrepareError::ProtocolEncoding => Self::ProtocolEncoding,
            NativePrepareError::InvalidTransactionBytes
            | NativePrepareError::WrongDepositorRole
            | NativePrepareError::WrongClaimant
            | NativePrepareError::WrongAggregateAuthority
            | NativePrepareError::WrongPreimage
            | NativePrepareError::WrongEscrowProgram
            | NativePrepareError::WrongAuthenticatedTransferProgram
            | NativePrepareError::NonceOverflow
            | NativePrepareError::InstructionEncoding => Self::InvalidTransactionBytes,
            NativePrepareError::WrongTransactionId => Self::WrongTransactionId,
            NativePrepareError::InvalidSignature => Self::InvalidSignature,
        }
    }
}

/// Supplies the current nonce for exactly one official Vault owner account.
#[async_trait]
pub trait VaultClaimNonceSource: Send + Sync {
    /// Returns the node-observed current nonce for `account_id`.
    ///
    /// # Errors
    ///
    /// Returns [`VaultClaimPrepareError::NonceUnavailable`] when the official
    /// node fact cannot be obtained exactly.
    async fn account_nonce(&self, account_id: AccountId) -> Result<u128, VaultClaimPrepareError>;
}

#[derive(Clone)]
struct ActiveClaim {
    request: PrepareVaultClaimRequest,
    result: PrepareVaultClaimResult,
}

#[derive(Default)]
struct PlannerState {
    active: Option<ActiveClaim>,
}

/// One-role, one-signer official v0.2 Vault Claim planner.
///
/// The planner prepares and retains exact bytes in memory. It deliberately has
/// no submission API: durable exact-byte recovery must be proven before a
/// later RPC/submission slice can make a transaction eligible for broadcast.
pub struct VaultClaimPlanner {
    role: Participant,
    signer_key_bytes: Zeroizing<[u8; 32]>,
    signer_account_id: AccountId,
    expected_runtime: RuntimeDescriptor,
    expected_allocation: VaultClaimAllocation,
    nonce_source: Arc<dyn VaultClaimNonceSource>,
    #[cfg(target_os = "linux")]
    durable_store: Option<DurableReservationStore>,
    state: Mutex<PlannerState>,
}

impl fmt::Debug for VaultClaimPlanner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VaultClaimPlanner")
            .field("role", &self.role)
            .field("signer_key", &"[REDACTED]")
            .field("signer_account_id", &self.signer_account_id)
            .field("expected_runtime", &self.expected_runtime)
            .field("expected_allocation", &self.expected_allocation)
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

impl VaultClaimPlanner {
    /// Binds one official key, role, complete runtime, and explicit allocation.
    ///
    /// # Errors
    ///
    /// Rejects role, compatibility, signer, owner, or allocation drift before
    /// retaining a zeroizing copy of the key bytes.
    #[allow(
        clippy::needless_pass_by_value,
        reason = "the isolated planner takes ownership of the upstream key before retaining only zeroizing bytes"
    )]
    pub fn new<N>(
        role: Participant,
        signer_key: PrivateKey,
        expected_runtime: RuntimeDescriptor,
        expected_allocation: VaultClaimAllocation,
        nonce_source: Arc<N>,
    ) -> Result<Self, VaultClaimPrepareError>
    where
        N: VaultClaimNonceSource + 'static,
    {
        let signer_account_id = AccountId::from(&PublicKey::new_from_private_key(&signer_key));
        if expected_runtime.sidecar_role != role {
            return Err(VaultClaimPrepareError::WrongRole);
        }
        if expected_runtime.compatibility != RuntimeCompatibility::LeeV0_2_0 {
            return Err(VaultClaimPrepareError::WrongRuntime);
        }
        if expected_allocation.role != role {
            return Err(VaultClaimPrepareError::WrongAllocation);
        }
        if expected_allocation.amount == 0 {
            return Err(VaultClaimPrepareError::ZeroAllocation);
        }
        let signer = Hex32::from_bytes(signer_account_id.into_value());
        if expected_runtime.signer_account_id != signer
            || expected_allocation.owner_account_id != signer
        {
            return Err(VaultClaimPrepareError::WrongSigner);
        }
        let owner_vault =
            vault_core::compute_vault_account_id(programs::vault().id(), signer_account_id);
        if owner_vault == signer_account_id {
            return Err(VaultClaimPrepareError::WrongAllocation);
        }

        let signer_key_bytes = Zeroizing::new(*signer_key.value());
        let nonce_source: Arc<dyn VaultClaimNonceSource> = nonce_source;
        Ok(Self {
            role,
            signer_key_bytes,
            signer_account_id,
            expected_runtime,
            expected_allocation,
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
        expected_runtime: RuntimeDescriptor,
        expected_allocation: VaultClaimAllocation,
        nonce_source: Arc<N>,
        state_directory: P,
    ) -> Result<Self, VaultClaimPrepareError>
    where
        N: VaultClaimNonceSource + 'static,
        P: AsRef<Path>,
    {
        let mut planner = Self::new(
            role,
            signer_key,
            expected_runtime,
            expected_allocation,
            nonce_source,
        )?;
        planner.durable_store = Some(DurableReservationStore::open(state_directory.as_ref())?);
        Ok(planner)
    }

    /// Prepares and caches one exact signed public Vault Claim.
    ///
    /// An identical replay returns the original exact bytes. A distinct
    /// request fails closed while this one-actor planner owns the nonce.
    ///
    /// # Errors
    ///
    /// Rejects identity/allocation drift before nonce lookup, unavailable
    /// nonces, instruction/transaction encoding failure, and a distinct active
    /// reservation.
    pub async fn prepare(
        &self,
        request: PrepareVaultClaimRequest,
    ) -> Result<PrepareVaultClaimResult, VaultClaimPrepareError> {
        self.validate_request(&request)?;
        let mut state = self.state.lock().await;
        if let Some(active) = state.active.as_ref() {
            return if active.request == request {
                Ok(active.result.clone())
            } else {
                Err(VaultClaimPrepareError::ActivePrepare)
            };
        }

        #[cfg(target_os = "linux")]
        if let Some(recovered) = self.recover_durable(&request)? {
            state.active = Some(ActiveClaim {
                request,
                result: recovered.clone(),
            });
            return Ok(recovered);
        }

        let owner_nonce = self
            .nonce_source
            .account_nonce(self.signer_account_id)
            .await?;
        if owner_nonce != request.owner_nonce {
            return Err(VaultClaimPrepareError::WrongNonce);
        }
        let message = self.plan_message(&request, owner_nonce)?;
        let signer_key = PrivateKey::try_new(*self.signer_key_bytes)
            .map_err(|_| VaultClaimPrepareError::InvalidSignature)?;
        let witnesses = WitnessSet::for_message(&message, &[&signer_key]);
        let claim = prepared_from_transaction(&PublicTransaction::new(message, witnesses))?;
        let result = PrepareVaultClaimResult::new(request.context.clone(), claim);
        #[cfg(target_os = "linux")]
        if let Some(store) = self.durable_store.as_ref()
            && let Err(error) = store.create(ReservationKind::VaultClaim, &request, &result)
        {
            if error == DurableReservationError::AlreadyReserved {
                let recovered = self
                    .recover_durable(&request)?
                    .ok_or(DurableReservationError::Filesystem)?;
                state.active = Some(ActiveClaim {
                    request,
                    result: recovered.clone(),
                });
                return Ok(recovered);
            }
            return Err(error.into());
        }
        state.active = Some(ActiveClaim {
            request,
            result: result.clone(),
        });
        Ok(result)
    }

    #[cfg(target_os = "linux")]
    fn recover_durable(
        &self,
        request: &PrepareVaultClaimRequest,
    ) -> Result<Option<PrepareVaultClaimResult>, VaultClaimPrepareError> {
        let Some(store) = self.durable_store.as_ref() else {
            return Ok(None);
        };
        let Some((stored_request, stored_result)) = store
            .load::<PrepareVaultClaimRequest, PrepareVaultClaimResult>(
                ReservationKind::VaultClaim,
            )?
        else {
            return Ok(None);
        };
        self.validate_request(&stored_request)?;
        self.validate_prepared(&stored_request, &stored_result)?;
        if &stored_request != request {
            return Err(VaultClaimPrepareError::ActivePrepare);
        }
        Ok(Some(stored_result))
    }

    /// Validates recovered exact bytes against the complete original request.
    ///
    /// This primitive does not retain recovered bytes and does not make them
    /// eligible for submission.
    ///
    /// # Errors
    ///
    /// Rejects context/runtime/role/owner/allocation drift, noncanonical bytes,
    /// hash or signature substitution, and any program, ordered-account,
    /// amount, or nonce change.
    pub fn validate_prepared(
        &self,
        request: &PrepareVaultClaimRequest,
        result: &PrepareVaultClaimResult,
    ) -> Result<(), VaultClaimPrepareError> {
        self.validate_request(request)?;
        if result.context != request.context {
            return Err(VaultClaimPrepareError::InvalidTransactionBytes);
        }
        let claim = decode_prepared_for_signer(&result.claim, self.signer_account_id)?;
        let expected = self.plan_message(request, request.owner_nonce)?;
        if claim.message() != &expected {
            return Err(VaultClaimPrepareError::InvalidTransactionBytes);
        }
        Ok(())
    }

    fn validate_request(
        &self,
        request: &PrepareVaultClaimRequest,
    ) -> Result<(), VaultClaimPrepareError> {
        if request.context.sidecar_role != self.role || request.runtime.sidecar_role != self.role {
            return Err(VaultClaimPrepareError::WrongRole);
        }
        if request.runtime != self.expected_runtime
            || request.runtime.compatibility != RuntimeCompatibility::LeeV0_2_0
        {
            return Err(VaultClaimPrepareError::WrongRuntime);
        }
        if request.allocation.owner_account_id
            != Hex32::from_bytes(self.signer_account_id.into_value())
        {
            return Err(VaultClaimPrepareError::WrongSigner);
        }
        if request.allocation != self.expected_allocation {
            return Err(VaultClaimPrepareError::WrongAllocation);
        }
        Ok(())
    }

    fn plan_message(
        &self,
        request: &PrepareVaultClaimRequest,
        owner_nonce: u128,
    ) -> Result<Message, VaultClaimPrepareError> {
        let vault_program = programs::vault().id();
        let owner_vault =
            vault_core::compute_vault_account_id(vault_program, self.signer_account_id);
        Message::try_new(
            vault_program,
            vec![self.signer_account_id, owner_vault],
            vec![owner_nonce.into()],
            vault_core::Instruction::Claim {
                amount: request.allocation.amount,
            },
        )
        .map_err(|_| VaultClaimPrepareError::InstructionEncoding)
    }
}
