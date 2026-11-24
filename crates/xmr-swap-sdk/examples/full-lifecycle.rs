//! A worked run of the LEZ/XMR SDK through the whole lifecycle, for both
//! roles, offline, including the recovery of the Monero spend key by a role
//! that has lost its session state.
//!
//! Four swaps in `TakerSellsLez`, the one direction XMR offers: the Taker
//! locks LEZ first (tag 13), the Maker funds the shared 2-of-2 Monero address,
//! and then
//!
//! 1. **Completed.** The Taker authorises (tag 14, its withheld claim
//!    partial). The Maker restarts without its adaptor journal and still
//!    claims (tag 15): Stage B and the published partial give the claim
//!    presignature, its share completes it. The Taker rebuilds the spend key
//!    from its journal, restarts without it, and rebuilds the same key from
//!    Stage A, Stage B, its share and the chain
//!    (`XmrActivatedAgreementV1::recover_claim_spend_key`); it sweeps.
//! 2. **Refunded, the Taker's journal lost before it authorises.** Its claim
//!    partial is gone with the journal, so it cannot authorise; it refunds in
//!    [R, P) from Stage B's refund presignature and its share (tag 16). The
//!    Maker rebuilds the key from its journal, restarts without it, and
//!    rebuilds the same key from Stage A, Stage B, its share and tag 16
//!    (`recover_refund_spend_key`); it sweeps its Monero back.
//! 3. **Refunded after authorising.** Tag 14 is on chain, the Maker does not
//!    claim before R, tag 15 is refused from R, the Taker refunds from
//!    `XmrClaimAuthorized`, and the Maker recovers as in 2.
//! 4. **Punished.** The Taker neither authorises nor refunds before P; the
//!    Maker takes the LEZ with tag 17. No signature on chain reveals a share,
//!    so neither role can rebuild the key and the Monero stays locked: the
//!    punish branch of the protocol design.
//!
//! What runs for real: this SDK's keys and shares, both cross-curve DLEQ
//! proofs, the shared address, Stage A composed by both roles and
//! countersigned, both `MuSig2` adaptor sessions (`lez-adaptor-signature`,
//! the Taker's claim partial withheld and only committed in Stage B), Stage
//! B, the LEZ lock check before C, the claim and refund signatures, the share
//! extraction and the spend-key recovery. What is modelled: the LEZ escrow's
//! tags 13 to 17 (states, the C/R/P windows of the devnet `fast` profile on a
//! model LEZ clock, the tag-14 commitment check and the BIP-340 check of tags
//! 15 and 16 under the swap's aggregate keys) and the Monero output (only the
//! key that opens the shared address's public spend key sweeps it). Chat is a
//! function call; nothing is sent anywhere. Keys follow the seed; the `MuSig2`
//! nonces come from the operating system. No private key byte is printed.
//!
//! ```sh
//! cargo run -p lez-xmr-swap-sdk --example full-lifecycle
//! ```
//!
//! `crates/xmr-reference-actor/examples/full-lifecycle.rs` runs the same
//! lifecycle on the actor layer the Nodes run (`SQLite` adaptor journals, the
//! role runner's ceremony seats) with the Monero leg live on the devnet's
//! regtest.

#[path = "../tests/support/lifecycle.rs"]
mod lifecycle;

use lifecycle::{DEFAULT_SEED, EscrowState, Narrator, Result, Run, role_name};

fn main() -> Result<()> {
    let runs: [(&str, Run); 4] = [
        ("swap 1 completed", lifecycle::completed),
        ("swap 2 refunded", lifecycle::refunded),
        (
            "swap 3 refunded after tag 14",
            lifecycle::refunded_after_authorisation,
        ),
        ("swap 4 punished", lifecycle::punished),
    ];
    let mut outcomes = Vec::new();
    for (label, run) in runs {
        println!();
        let mut narrator = Narrator::new(label, true);
        let settlement = run(DEFAULT_SEED, &mut narrator)?;
        let escrow = &settlement.swap.escrow;
        let outcome = match escrow.state {
            EscrowState::Claimed => "Completed",
            EscrowState::Refunded => "Refunded",
            EscrowState::Punished => "Punished",
            EscrowState::Funded | EscrowState::XmrClaimAuthorized => "unsettled",
        };
        let monero = match settlement.swap.output.swept_by {
            Some(role) => format!(
                "Monero swept by the {} with the key it recovered after its restart",
                role_name(role)
            ),
            None => "Monero locked at the shared address".to_owned(),
        };
        outcomes.push(format!(
            "{label}: {outcome} (escrow {:?}, LEZ to the {}); {monero}",
            escrow.state,
            escrow.lez_paid_to.map_or("nobody", role_name),
        ));
    }
    println!();
    for outcome in &outcomes {
        println!("{outcome}");
    }
    println!("private_key_bytes_printed=false");
    Ok(())
}
