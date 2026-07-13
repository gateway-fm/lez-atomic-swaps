//! Official LEZ v0.1.2 transaction planning and exact-byte boundary.
//!
//! This crate intentionally lives outside the main workspace. It is the only
//! process boundary that imports the pinned official NSSA and SPEL graph.

#![forbid(unsafe_code)]

use std::{fmt, sync::Arc};

use async_trait::async_trait;
use common::transaction::NSSATransaction;
use lez_bridge_protocol::{
    ExactTransactionBytes, Hex32, Participant, PrepareNativeEscrowRequest,
    PrepareNativeEscrowResult, PreparedTransaction, ProtocolValueError, RuntimeDescriptor,
    TransactionId,
};
use lez_zec_escrow_compat::Instruction as EscrowInstruction;
use nssa::{
    AccountId, PrivateKey, PublicKey, PublicTransaction,
    program::Program,
    public_transaction::{Message, WitnessSet},
};
use tokio::sync::Mutex;

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

        let initialization = self.prepare_transaction(
            vec![metadata, custody, depositor, claimant],
            initialization_nonce,
            EscrowInstruction::InitializeNative {
                swap_id,
                terms_hash: *terms.terms_hash().as_bytes(),
                secret_digest: *terms.secret_digest().as_bytes(),
                amount: terms.amount().as_u128(),
                refund_at: terms.refund_at_ms(),
                authenticated_transfer_program,
            },
        )?;
        let funding = self.prepare_transaction(
            vec![metadata, custody, depositor],
            funding_nonce,
            EscrowInstruction::FundNative { swap_id },
        )?;

        Ok(PrepareNativeEscrowResult::new(
            request.context.clone(),
            initialization,
            funding,
        ))
    }

    fn prepare_transaction(
        &self,
        account_ids: Vec<AccountId>,
        nonce: u128,
        instruction: EscrowInstruction,
    ) -> Result<PreparedTransaction, SidecarError> {
        let message = Message::try_new(
            self.escrow_program_id,
            account_ids,
            vec![nonce.into()],
            instruction,
        )
        .map_err(|_| SidecarError::InstructionEncoding)?;
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
