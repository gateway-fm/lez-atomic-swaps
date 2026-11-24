//! The lifecycle of `examples/full-lifecycle.rs`, asserted: each run ends in
//! the escrow state and the Monero outcome the protocol gives it, and every
//! spend key a role rebuilds after losing its session state is the original
//! spend key of the shared address, the sum of both roles' shares.

#[path = "support/lifecycle.rs"]
mod lifecycle;

use lez_xmr_swap_sdk::XmrRoleV1;
use lifecycle::{DEFAULT_SEED, EscrowState, Narrator, Settlement, Swap};
use sigma_fun::ed25519::curve25519_dalek::{
    constants::ED25519_BASEPOINT_TABLE, scalar::Scalar as EdScalar,
};

/// The shared address's private spend key, from both shares directly.
fn original_spend_key(swap: &Swap) -> [u8; 32] {
    let share = |bytes: [u8; 32]| EdScalar::from_canonical_bytes(bytes).expect("canonical share");
    (share(*swap.maker.durable.share) + share(*swap.taker.durable.share)).to_bytes()
}

fn assert_recovered_original(settlement: &Settlement) {
    let original = original_spend_key(&settlement.swap);
    let recovered = settlement.recovered.as_ref().expect("a recovered key");
    let from_journal = settlement.from_journal.as_ref().expect("a journal key");
    assert_eq!(
        **recovered, original,
        "the recovered key is the original spend key"
    );
    assert_eq!(
        **from_journal, original,
        "the journal key is the original spend key"
    );
    let public = (&EdScalar::from_canonical_bytes(**recovered).expect("canonical key")
        * &ED25519_BASEPOINT_TABLE)
        .compress()
        .to_bytes();
    let agreement =
        lez_xmr_swap_sdk::XmrAgreementV1::from_wire(&settlement.swap.maker.durable.stage_a)
            .expect("Stage A");
    assert_eq!(public, agreement.shared_address().public_spend_key());
}

fn refusals(narrator: &Narrator) -> Vec<&str> {
    narrator
        .lines
        .iter()
        .filter(|line| line.contains("refused "))
        .map(String::as_str)
        .collect()
}

#[test]
fn a_taker_without_its_session_state_recovers_the_original_key_after_the_claim() {
    let mut narrator = Narrator::new("completed", false);
    let settlement = lifecycle::completed(DEFAULT_SEED, &mut narrator).expect("completed swap");
    assert_eq!(settlement.swap.escrow.state, EscrowState::Claimed);
    assert_eq!(settlement.swap.escrow.lez_paid_to, Some(XmrRoleV1::Maker));
    assert_eq!(settlement.swap.output.swept_by, Some(XmrRoleV1::Taker));
    assert!(!settlement.swap.maker.has_session_state());
    assert!(!settlement.swap.taker.has_session_state());
    assert_recovered_original(&settlement);
    let refused = refusals(&narrator);
    for expected in [
        "tag 14 with another partial",
        "tag 16 after the claim",
        "claim recovery with another share than the Taker's",
        "claim recovery with a partial Stage B does not commit to",
        "refund recovery from the claim signature",
        "a second sweep",
    ] {
        assert!(
            refused.iter().any(|line| line.contains(expected)),
            "{expected} was not refused: {refused:#?}"
        );
    }
}

#[test]
fn a_maker_without_its_session_state_recovers_the_original_key_after_the_refund() {
    let mut narrator = Narrator::new("refunded", false);
    let settlement = lifecycle::refunded(DEFAULT_SEED, &mut narrator).expect("refunded swap");
    assert_eq!(settlement.swap.escrow.state, EscrowState::Refunded);
    assert_eq!(settlement.swap.escrow.lez_paid_to, Some(XmrRoleV1::Taker));
    assert_eq!(settlement.swap.escrow.claim_partial, None);
    assert_eq!(settlement.swap.output.swept_by, Some(XmrRoleV1::Maker));
    assert_recovered_original(&settlement);
    let refused = refusals(&narrator);
    for expected in [
        "tag 14 from a Taker without its journal",
        "tag 16 before R",
        "tag 17 before P",
        "tag 17 after the refund",
        "refund recovery with another share than the Maker's",
    ] {
        assert!(
            refused.iter().any(|line| line.contains(expected)),
            "{expected} was not refused: {refused:#?}"
        );
    }
}

#[test]
fn a_refund_after_an_unanswered_authorisation_recovers_the_original_key() {
    let mut narrator = Narrator::new("refunded after tag 14", false);
    let settlement = lifecycle::refunded_after_authorisation(DEFAULT_SEED, &mut narrator)
        .expect("refunded swap");
    assert_eq!(settlement.swap.escrow.state, EscrowState::Refunded);
    assert!(settlement.swap.escrow.claim_partial.is_some());
    assert_eq!(settlement.swap.escrow.claim_signature, None);
    assert_eq!(settlement.swap.output.swept_by, Some(XmrRoleV1::Maker));
    assert_recovered_original(&settlement);
    assert!(
        refusals(&narrator)
            .iter()
            .any(|line| line.contains("the Maker's tag 15 at R"))
    );
}

#[test]
fn a_punished_swap_reveals_no_share_and_no_key_can_be_recovered() {
    let mut narrator = Narrator::new("punished", false);
    let settlement = lifecycle::punished(DEFAULT_SEED, &mut narrator).expect("punished swap");
    assert_eq!(settlement.swap.escrow.state, EscrowState::Punished);
    assert_eq!(settlement.swap.escrow.lez_paid_to, Some(XmrRoleV1::Maker));
    assert_eq!(settlement.swap.output.swept_by, None);
    assert!(settlement.recovered.is_none());
    assert!(settlement.swap.maker_recovers_refund_key().is_err());
    assert!(settlement.swap.taker_recovers_claim_key().is_err());
    let refused = refusals(&narrator);
    for expected in [
        "tag 17 by the Taker owner",
        "the Taker's tag 16 at P",
        "the Maker's key without a tag 16",
        "the Taker's key without a tag 15",
    ] {
        assert!(
            refused.iter().any(|line| line.contains(expected)),
            "{expected} was not refused: {refused:#?}"
        );
    }
}

#[test]
fn a_lock_final_after_the_funding_cutoff_is_refused_before_the_maker_funds() {
    let mut narrator = Narrator::new("cutoff", false);
    lifecycle::Swap::lock_both(
        lifecycle::Terms::new(9, 1_000_000, 10),
        DEFAULT_SEED,
        &mut narrator,
    )
    .expect("both legs locked");
    assert!(
        refusals(&narrator)
            .iter()
            .any(|line| line.contains("a LEZ lock final after C"))
    );
}
