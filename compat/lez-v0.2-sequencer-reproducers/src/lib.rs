//! Test-only guest program for the LEZ v0.2.0 sequencer reproducers.
//!
//! The reproducers themselves live in `tests/`. This crate only embeds the
//! `windowed_transfer` guest so they can deploy it through the real mempool.

#![forbid(unsafe_code)]

use std::borrow::Cow;

use lee::program::Program;

mod guests {
    #![allow(dead_code, missing_docs, clippy::all, clippy::pedantic)]
    include!(concat!(env!("OUT_DIR"), "/methods.rs"));
}

/// A balance transfer, delegated to the built-in authenticated-transfer
/// program through a chained call, whose output carries caller-supplied block
/// and timestamp validity windows.
///
/// Instruction: `(amount, authenticated_transfer_program_id,
/// block_validity_window, timestamp_validity_window)`. Pre-states: sender
/// (authorized), receiver.
#[must_use]
pub fn windowed_transfer() -> Program {
    Program::new_unchecked(
        guests::WINDOWED_TRANSFER_ID,
        Cow::Borrowed(guests::WINDOWED_TRANSFER_ELF),
    )
}
