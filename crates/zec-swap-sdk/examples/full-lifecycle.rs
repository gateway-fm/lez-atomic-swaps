//! A worked run of the LEZ/ZEC SDK through all five lifecycle stages, offline.
//!
//! Two swaps in `TakerSellsLez`, the direction the Nodes drive on the local
//! stack: the Taker locks LEZ first, the Maker funds the Zcash HTLC second,
//! the Maker claims LEZ revealing the secret, the Taker claims Zcash. The
//! first swap settles to `Completed`; the second is locked on both legs and
//! never claimed, so it runs the ordered refunds (LEZ first, then Zcash) to
//! `Refunded`. Both roles run as independent actors on their own in-memory
//! stores, and both are resumed from those stores at the end.
//!
//! The chains are simulated: the same in-memory ports and store the lifecycle
//! tests use (`tests/support/memory.rs`, included below), fed with fixed keys
//! and canonical observations built from the agreement itself. Nothing is
//! sent anywhere; the run is deterministic.
//!
//! ```sh
//! cargo run -p lez-zec-swap-sdk --example full-lifecycle
//! ```

#[path = "../tests/support/memory.rs"]
mod memory;

use std::sync::Arc;

use lez_swap_core::{
    Chain, ChainPosition, LezUnixMilliseconds, Participant, Phase, SwapDirection, SwapId,
};
use lez_zec_swap_sdk::{
    ActiveZecSwap, ClaimPreimage, ClaimStepV1, FirstLockDriveOutcome, FirstLockObservation,
    FirstLockStepV1, MakerLockDriveOutcome, MakerLockObservationV1, ObserveTakerFirstLockOutcome,
    RefundDriveOutcome, RefundEligibilityObservationV1, RefundStepV1, RefundSubmitOutcomeV1,
    TakerFirstLockObservationV1, ZecLifecycleAction, ZecPairSdk,
};
use memory::{
    ACCEPTED_AT, FixtureVariant, MemoryClaimCorridor, MemoryDiscovery, MemoryFirstLockPort,
    MemoryLezTakerLockObservation, MemoryMakerLockObservation, MemoryNegotiation,
    MemoryRefundCorridor, MemoryStore, MemoryTakerLockObservation, MemoryZcashTakerLockObservation,
    Offer, Proposal, agreement_wire_with_digest, canonical_lez_taker_lock,
    canonical_zcash_taker_lock, confirmed_first_lock, confirmed_first_lock_observation,
    confirmed_zcash_first_lock, lez_first_lock_plan, zcash_first_lock_plan,
};
use sha2::{Digest as _, Sha256};

type Actor =
    ActiveZecSwap<MemoryLezTakerLockObservation, MemoryZcashTakerLockObservation, MemoryStore>;

/// One corridor: the simulated LEZ and Zcash chains both roles observe, and a
/// store per role. The claim and refund corridors are shared so what one role
/// submits is what the other observes.
struct Corridor {
    discovery: MemoryDiscovery,
    lez: MemoryLezTakerLockObservation,
    zcash: MemoryZcashTakerLockObservation,
    claims: MemoryClaimCorridor,
    refunds: MemoryRefundCorridor,
    maker_store: MemoryStore,
    taker_store: MemoryStore,
}

impl Corridor {
    fn new() -> Self {
        let claims = MemoryClaimCorridor::default();
        let refunds = MemoryRefundCorridor::default();
        let lez = MemoryLezTakerLockObservation(
            MemoryTakerLockObservation::default(),
            Arc::default(),
            MemoryFirstLockPort::default(),
            MemoryMakerLockObservation::default(),
            claims.clone(),
            refunds.clone(),
        );
        let zcash = MemoryZcashTakerLockObservation(
            MemoryTakerLockObservation::default(),
            MemoryFirstLockPort::default(),
            MemoryMakerLockObservation::default(),
            claims.clone(),
            Arc::default(),
            refunds.clone(),
        );
        Self {
            discovery: MemoryDiscovery::default(),
            lez,
            zcash,
            claims,
            refunds,
            maker_store: MemoryStore::default(),
            taker_store: MemoryStore::default(),
        }
    }

    fn sdk(
        &self,
        role: Participant,
        wire: Vec<u8>,
    ) -> ZecPairSdk<
        MemoryDiscovery,
        MemoryNegotiation,
        MemoryLezTakerLockObservation,
        MemoryZcashTakerLockObservation,
        MemoryStore,
    > {
        let store = match role {
            Participant::Maker => self.maker_store.clone(),
            Participant::Taker => self.taker_store.clone(),
        };
        // One Delivery for both roles: what the Maker publishes is what the Taker discovers.
        ZecPairSdk::new(
            role,
            self.discovery.clone(),
            MemoryNegotiation { wire },
            self.lez.clone(),
            self.zcash.clone(),
            store,
        )
    }
}

/// Stages 1–3: both roles discover the same offer, negotiate the same
/// countersigned agreement, and activate their own actor. The Maker is the
/// first claimant in `TakerSellsLez`, so it is the one that protects the
/// claim preimage at activation.
async fn through_activation(
    corridor: &Corridor,
    wire: Vec<u8>,
    secret: [u8; 32],
) -> (Actor, Actor) {
    let maker_sdk = corridor.sdk(Participant::Maker, wire.clone());
    let taker_sdk = corridor.sdk(Participant::Taker, wire);

    let published = maker_sdk.publish_offer(Offer(1)).await.expect("publish");
    println!("  1. offer discovery   Maker published offer {published:?}");
    let found = taker_sdk.discover_offers(&()).await.expect("discover");
    println!(
        "  2. offer discovery   Taker discovered {} offer(s)",
        found.len()
    );

    let maker_terms = maker_sdk
        .negotiate_at(&found[0], Proposal, ACCEPTED_AT)
        .await
        .expect("maker negotiation");
    let taker_terms = taker_sdk
        .negotiate_at(&found[0], Proposal, ACCEPTED_AT)
        .await
        .expect("taker negotiation");
    assert_eq!(maker_terms.agreement(), taker_terms.agreement());
    println!(
        "  3. negotiation       both roles hold the same countersigned agreement ({}, {:?})",
        maker_terms.agreement().application_swap_id(),
        maker_terms.agreement().direction()
    );

    let maker = maker_sdk
        .activate_with_claim_preimage(maker_terms, ClaimPreimage::new(secret))
        .await
        .expect("maker activation protects the preimage");
    let taker = taker_sdk
        .activate(taker_terms)
        .await
        .expect("taker activation");
    println!(
        "  4. activation        Maker next: {:?}; Taker next: {:?}",
        maker.next_action(),
        taker.next_action()
    );
    assert_eq!(taker.next_action(), ZecLifecycleAction::CreateAndFundLez);
    (maker, taker)
}

/// Stage 4: the Taker's LEZ escrow (initialise, then fund), observed by the
/// Maker; the Maker's Zcash HTLC funding, observed by the Taker.
async fn lock_both_legs(corridor: &Corridor, maker: &mut Actor, taker: &mut Actor) {
    let plan = lez_first_lock_plan([0x41; 32], vec![0x61], [0x42; 32], vec![0x62]);
    taker
        .stage_first_lock(plan)
        .await
        .expect("stage the LEZ first lock");
    taker
        .drive_first_lock()
        .await
        .expect("submit LEZ initialize");
    corridor.lez.2.observe_as(
        FirstLockStepV1::LezInitialize,
        confirmed_first_lock_observation(
            FirstLockStepV1::LezInitialize,
            [0x41; 32],
            "taker-lez-initialize",
        ),
    );
    taker.drive_first_lock().await.expect("submit LEZ fund");
    let lez_evidence = confirmed_first_lock(FirstLockStepV1::LezFund, [0x42; 32], "taker-lez-fund");
    corridor.lez.2.observe_as(
        FirstLockStepV1::LezFund,
        FirstLockObservation::Confirmed(lez_evidence),
    );
    let FirstLockDriveOutcome::ReadyForFundingProjection(observed) = taker
        .drive_first_lock()
        .await
        .expect("observe the LEZ fund")
    else {
        panic!("the confirmed LEZ fund must be ready for projection")
    };
    taker
        .project_first_lock(observed)
        .await
        .expect("project the LEZ lock");
    println!(
        "  5. escrow creation   Taker's LEZ escrow initialised and funded; Taker at {:?}",
        taker.status()
    );

    corridor
        .lez
        .0
        .respond(Ok(TakerFirstLockObservationV1::CanonicalLez(Box::new(
            canonical_lez_taker_lock(maker.agreement()),
        ))));
    let observed = maker
        .observe_taker_first_lock()
        .await
        .expect("maker observes the LEZ lock");
    assert!(matches!(
        observed,
        ObserveTakerFirstLockOutcome::Projected(_)
    ));
    println!(
        "  6. escrow creation   Maker observed the Taker's LEZ lock; Maker next: {:?}",
        maker.next_action()
    );

    let zcash_plan = zcash_first_lock_plan([0x81; 32], vec![0xb1]);
    maker
        .drive_maker_lock(zcash_plan.clone())
        .await
        .expect("submit the Zcash HTLC funding");
    let funding_id = canonical_zcash_taker_lock(maker.agreement())
        .transaction_id()
        .to_string();
    let zcash_evidence = confirmed_zcash_first_lock([0x81; 32], &funding_id);
    corridor.zcash.1.observe_as(
        FirstLockStepV1::ZcashFund,
        FirstLockObservation::Confirmed(zcash_evidence.clone()),
    );
    let MakerLockDriveOutcome::Lock(FirstLockDriveOutcome::ReadyForFundingProjection(observed)) =
        maker
            .drive_maker_lock(zcash_plan)
            .await
            .expect("observe the Zcash funding")
    else {
        panic!("the confirmed Zcash funding must be ready for projection")
    };
    maker
        .project_maker_lock(observed)
        .await
        .expect("project the Zcash lock");
    corridor
        .zcash
        .2
        .respond(Ok(MakerLockObservationV1::Confirmed(zcash_evidence)));
    taker
        .observe_maker_lock()
        .await
        .expect("taker observes the Zcash funding");
    assert_eq!(maker.status(), Phase::BothLegsLocked);
    assert_eq!(taker.status(), Phase::BothLegsLocked);
    println!("  7. escrow creation   Maker funded the Zcash HTLC; both legs locked on both actors");
}

/// A swap that settles: the revealing LEZ claim, then the follow-up Zcash claim.
/// The trusted clock `drive_claim` takes (Unix milliseconds). A real caller
/// passes its wall clock: after the claim window (the agreement's LEZ refund
/// time minus the profile's margin) the SDK refuses to send a revealing claim
/// (FINDINGS 26). The offline fixture's agreement is dated, so the example
/// claims at time zero, inside its window.
const EXAMPLE_NOW: lez_zec_swap_sdk::LezUnixMilliseconds =
    lez_zec_swap_sdk::LezUnixMilliseconds::new(0);

async fn settled_swap() {
    println!("A swap that settles (TakerSellsLez)");
    let secret = [0x5a; 32];
    let wire = agreement_wire_with_digest(
        "example-settles",
        SwapDirection::TakerSellsLez,
        FixtureVariant::Local,
        Sha256::digest(secret).into(),
    );
    let corridor = Corridor::new();
    let (mut maker, mut taker) = through_activation(&corridor, wire, secret).await;
    lock_both_legs(&corridor, &mut maker, &mut taker).await;

    assert_eq!(maker.next_action(), ZecLifecycleAction::ClaimLez);
    maker
        .drive_claim(EXAMPLE_NOW)
        .await
        .expect("maker prepares and submits the revealing LEZ claim");
    assert_eq!(
        corridor.claims.submissions(),
        vec![ClaimStepV1::RevealingLez]
    );
    corridor.claims.confirm_revealing_claim(secret);
    maker
        .drive_claim(EXAMPLE_NOW)
        .await
        .expect("maker observes its LEZ claim");
    taker
        .drive_claim(EXAMPLE_NOW)
        .await
        .expect("taker observes the LEZ claim and extracts the secret");
    assert_eq!(taker.status(), Phase::ClaimEvidenceAvailable);
    println!(
        "  8. claim             Maker's revealing LEZ claim confirmed; Taker next: {:?}",
        taker.next_action()
    );

    taker
        .drive_claim(EXAMPLE_NOW)
        .await
        .expect("taker prepares and submits the follow-up Zcash claim");
    assert_eq!(
        corridor.claims.submissions(),
        vec![ClaimStepV1::RevealingLez, ClaimStepV1::FollowupZcash]
    );
    corridor.claims.confirm_followup_claim();
    maker
        .drive_claim(EXAMPLE_NOW)
        .await
        .expect("maker observes the Zcash claim");
    taker
        .drive_claim(EXAMPLE_NOW)
        .await
        .expect("taker observes its Zcash claim");
    assert_eq!(maker.status(), Phase::Completed);
    assert_eq!(taker.status(), Phase::Completed);
    println!(
        "  9. claim             Taker's follow-up Zcash claim confirmed; both actors {:?} at revision {}",
        maker.status(),
        maker.revision()
    );

    let swap_id = SwapId::new("example-settles").expect("swap id");
    for (role, store) in [
        (Participant::Maker, corridor.maker_store.clone()),
        (Participant::Taker, corridor.taker_store.clone()),
    ] {
        let offline: ZecPairSdk<(), (), (), (), MemoryStore> =
            ZecPairSdk::new(role, (), (), (), (), store);
        let resumed = offline
            .resume_all_capable(&swap_id)
            .await
            .expect("offline restart")
            .expect("durable swap");
        assert_eq!(resumed.status(), Phase::Completed);
    }
    println!(
        " 10. durability       both roles resumed from their stores with no ports: still Completed"
    );
}

/// A swap that refunds: both legs locked, nobody claims, the LEZ leg refunds
/// at its deadline and the Zcash leg at its height, in that order.
async fn refunded_swap() {
    println!("\nA swap that refunds (TakerSellsLez)");
    let secret = [0x6b; 32];
    let wire = agreement_wire_with_digest(
        "example-refunds",
        SwapDirection::TakerSellsLez,
        FixtureVariant::Local,
        Sha256::digest(secret).into(),
    );
    let corridor = Corridor::new();
    let (mut maker, mut taker) = through_activation(&corridor, wire, secret).await;
    lock_both_legs(&corridor, &mut maker, &mut taker).await;
    println!("  8. escrow creation   both legs locked, and nobody claims");

    let agreement = maker.agreement().clone();
    let lez_deadline = ChainPosition::lez_timestamp_from_milliseconds_floor(
        LezUnixMilliseconds::new(agreement.lez_refund_at_ms()),
    );
    let zcash_deadline =
        ChainPosition::block_height(Chain::Zcash, u64::from(agreement.zcash_refund_at_height()));
    assert_eq!(agreement.lez_depositor(), Participant::Taker);
    assert_eq!(agreement.lez_claimant(), Participant::Maker);

    corridor.refunds.set_eligibility(
        RefundStepV1::Lez,
        RefundEligibilityObservationV1::canonical(lez_deadline),
    );
    corridor
        .refunds
        .set_submit_outcome(RefundStepV1::Lez, RefundSubmitOutcomeV1::Accepted);
    assert_eq!(
        taker
            .drive_refund(EXAMPLE_NOW)
            .await
            .expect("taker refunds its LEZ escrow"),
        RefundDriveOutcome::Submitted(RefundStepV1::Lez)
    );
    corridor.refunds.confirm(RefundStepV1::Lez);
    let projected = taker
        .drive_refund(EXAMPLE_NOW)
        .await
        .expect("taker observes its LEZ refund");
    assert!(matches!(
        projected,
        RefundDriveOutcome::Projected {
            step: RefundStepV1::Lez,
            ..
        }
    ));
    maker
        .drive_refund(EXAMPLE_NOW)
        .await
        .expect("maker observes the LEZ refund");
    println!(
        "  9. refund            Taker's LEZ refund confirmed at the agreement's LEZ deadline; Maker at {:?}",
        maker.status()
    );

    corridor.refunds.set_eligibility(
        RefundStepV1::Zcash,
        RefundEligibilityObservationV1::canonical(zcash_deadline),
    );
    assert_eq!(
        maker
            .drive_refund(EXAMPLE_NOW)
            .await
            .expect("maker refunds its Zcash HTLC"),
        RefundDriveOutcome::Submitted(RefundStepV1::Zcash)
    );
    corridor.refunds.confirm(RefundStepV1::Zcash);
    assert!(matches!(
        maker
            .drive_refund(EXAMPLE_NOW)
            .await
            .expect("maker observes its Zcash refund"),
        RefundDriveOutcome::Refunded { .. }
    ));
    assert!(matches!(
        taker
            .drive_refund(EXAMPLE_NOW)
            .await
            .expect("taker observes the Zcash refund"),
        RefundDriveOutcome::Refunded { .. }
    ));
    assert_eq!(maker.status(), Phase::Refunded);
    assert_eq!(taker.status(), Phase::Refunded);
    println!(
        " 10. refund            Maker's Zcash refund confirmed at the agreement's refund height; both actors {:?} at revision {}",
        maker.status(),
        maker.revision()
    );
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    println!("lez-zec-swap-sdk: offer discovery, negotiation, escrow, claim, refund\n");
    Box::pin(settled_swap()).await;
    Box::pin(refunded_swap()).await;
    println!("\nBoth terminal states reached. No I/O was performed.");
}
