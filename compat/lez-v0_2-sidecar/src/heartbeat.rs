//! The devnet finality heartbeat's transaction: one authenticated transfer of
//! one LEZ unit from a funded devnet account to another, signed locally. It is
//! the smallest state change a plain funded account can make on the official
//! v0.2 chain (the vault claim and every escrow fund use the same program) and
//! carries no escrow, no terms and no program deployment; the heartbeat
//! service (`lez-v02-heartbeat`) submits one every few seconds to keep
//! finality moving on an otherwise quiet devnet chain (logos-blockchain#3643).

use authenticated_transfer_core::Instruction as AuthenticatedTransferInstruction;
use lez_bridge_protocol::PreparedTransaction;
use nssa::{
    AccountId, PrivateKey, PublicTransaction,
    public_transaction::{Message, WitnessSet},
};

use crate::{NativePrepareError, prepared_from_transaction};

/// One signed transfer of one unit from `sender` (the signer's account) to
/// `recipient` at the sender's current `nonce`.
///
/// # Errors
///
/// Rejects an instruction or transaction the official types cannot encode.
pub fn prepare_unit_transfer(
    signer: &PrivateKey,
    authenticated_transfer_program_id: [u32; 8],
    sender: AccountId,
    recipient: AccountId,
    nonce: u128,
) -> Result<PreparedTransaction, NativePrepareError> {
    let message = Message::try_new(
        authenticated_transfer_program_id,
        vec![sender, recipient],
        vec![nonce.into()],
        AuthenticatedTransferInstruction::Transfer { amount: 1 },
    )
    .map_err(|_| NativePrepareError::InstructionEncoding)?;
    let witnesses = WitnessSet::for_message(&message, &[signer]);
    prepared_from_transaction(&PublicTransaction::new(message, witnesses))
}
