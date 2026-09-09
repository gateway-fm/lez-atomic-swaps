//! Windowed transfer guest.
//!
//! Moves `amount` from the sender to the receiver by chain-calling the built-in
//! authenticated-transfer program, and stamps caller-supplied block and
//! timestamp validity windows on its own output. Signer authorization
//! propagates through the chained call, so the accounts stay owned by the
//! built-in program and no test-only ownership is needed.
//!
//! Expected pre-states (in order):
//!   0 - sender account (authorized),
//!   1 - receiver account.

use authenticated_transfer_core::Instruction as TransferInstruction;
use lee_core::program::{
    AccountPostState, BlockValidityWindow, ChainedCall, ProgramId, ProgramInput, ProgramOutput,
    TimestampValidityWindow, read_lee_inputs,
};
use risc0_zkvm::serde::to_vec;

/// (`amount`, `authenticated_transfer_program_id`, `block_validity_window`,
/// `timestamp_validity_window`).
type Instruction = (
    u128,
    ProgramId,
    BlockValidityWindow,
    TimestampValidityWindow,
);

fn main() {
    let (
        ProgramInput {
            self_program_id,
            caller_program_id,
            pre_states,
            instruction:
                (amount, transfer_program_id, block_validity_window, timestamp_validity_window),
        },
        instruction_words,
    ) = read_lee_inputs::<Instruction>();

    let Ok([sender_pre, receiver_pre]) = <[_; 2]>::try_from(pre_states) else {
        panic!("Expected exactly 2 input accounts: sender, receiver");
    };

    let chained_call = ChainedCall {
        program_id: transfer_program_id,
        instruction_data: to_vec(&TransferInstruction::Transfer { amount })
            .expect("transfer instruction serializes"),
        pre_states: vec![sender_pre.clone(), receiver_pre.clone()],
        pda_seeds: vec![],
    };

    let sender_post = AccountPostState::new(sender_pre.account.clone());
    let receiver_post = AccountPostState::new(receiver_pre.account.clone());

    ProgramOutput::new(
        self_program_id,
        caller_program_id,
        instruction_words,
        vec![sender_pre, receiver_pre],
        vec![sender_post, receiver_post],
    )
    .with_block_validity_window(block_validity_window)
    .with_timestamp_validity_window(timestamp_validity_window)
    .with_chained_calls(vec![chained_call])
    .write();
}
