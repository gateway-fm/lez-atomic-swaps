use std::{fmt, sync::Arc};

use async_trait::async_trait;
use common::transaction::LeeTransaction;
use lez_bridge_protocol::{
    ExactTransactionBytes, Hex32, Participant, PrepareNativeEscrowRequest,
    PrepareNativeEscrowResult, PreparedTransaction, ProtocolValueError, RuntimeCompatibility,
    RuntimeDescriptor, TransactionId,
};
use nssa::{
    AccountId, PrivateKey, PublicKey, PublicTransaction,
    public_transaction::{Message, WitnessSet},
};
use tokio::sync::Mutex;
use zeroize::Zeroizing;

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
}

/// One-role, one-signer official v0.2 native escrow planner.
///
/// This foundation prepares and retains exact bytes in memory. It intentionally
/// exposes no submission API: a later slice must prove durable exact-byte
/// recovery before any transaction becomes eligible for submission.
pub struct NativeEscrowPlanner {
    role: Participant,
    signer_key_bytes: Zeroizing<[u8; 32]>,
    signer_account_id: AccountId,
    escrow_program_id: [u32; 8],
    authenticated_transfer_program_id: [u32; 8],
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
            .field(
                "authenticated_transfer_program_id",
                &program_id_to_hex(self.authenticated_transfer_program_id),
            )
            .field("expected_runtime", &self.expected_runtime)
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
            state: Mutex::new(PlannerState::default()),
        })
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
        if let Some(active) = state.active.as_ref() {
            return if active.request == request {
                Ok(active.result.clone())
            } else {
                Err(NativePrepareError::ActivePrepare)
            };
        }

        let initialization_nonce = self
            .nonce_source
            .account_nonce(self.signer_account_id)
            .await?;
        let funding_nonce = initialization_nonce
            .checked_add(1)
            .ok_or(NativePrepareError::NonceOverflow)?;
        let result = self.plan_pair(&request, initialization_nonce, funding_nonce)?;
        state.active = Some(ActivePrepare {
            request,
            result: result.clone(),
        });
        Ok(result)
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

    fn prepare_message(&self, message: Message) -> Result<PreparedTransaction, NativePrepareError> {
        let signer_key = PrivateKey::try_new(*self.signer_key_bytes)
            .map_err(|_| NativePrepareError::InvalidSignature)?;
        let witnesses = WitnessSet::for_message(&message, &[&signer_key]);
        prepared_from_transaction(&PublicTransaction::new(message, witnesses))
    }
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

fn program_id_to_hex(program_id: [u32; 8]) -> Hex32 {
    let mut bytes = [0_u8; 32];
    for (chunk, word) in bytes.chunks_exact_mut(4).zip(program_id) {
        chunk.copy_from_slice(&word.to_le_bytes());
    }
    Hex32::from_bytes(bytes)
}
