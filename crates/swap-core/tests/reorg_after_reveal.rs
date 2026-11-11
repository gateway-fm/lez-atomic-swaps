//! Lock removals and depth regressions around the revealing claim (issue #103, items 1 and 16).
//!
//! A lock removal moves the swap into `TakerLockReorged` or `MakerLockReorged` whatever the
//! claim evidence says, and those phases admit refunds; the refund entry points must still
//! refuse once a revealing claim is recorded. A maker-leg observation that arrives while the
//! taker leg is the reason for the phase must still update what claim authority reads.

use lez_swap_core::{
    Chain, ChainPosition, ChainProof, ClaimEvidence, ConfirmationPolicy, Error, Pair, Participant,
    Phase, RecoverySchedule, SwapCoordinator, SwapDirection, SwapId, TimelockSafety,
};

const PROFILES: [(Pair, SwapDirection); 4] = [
    (Pair::Bitcoin, SwapDirection::TakerSellsForeign),
    (Pair::Bitcoin, SwapDirection::TakerSellsLez),
    (Pair::Zcash, SwapDirection::TakerSellsForeign),
    (Pair::Zcash, SwapDirection::TakerSellsLez),
];

fn coordinator(pair: Pair, direction: SwapDirection, maker_policy: u32) -> SwapCoordinator {
    let foreign = Chain::from(pair);
    let (maker_chain, taker_chain) = match direction {
        SwapDirection::TakerSellsForeign => (Chain::Lez, foreign),
        SwapDirection::TakerSellsLez => (foreign, Chain::Lez),
    };
    let safety_chains = if pair == Pair::Zcash {
        [Chain::Lez, Chain::Zcash]
    } else {
        [maker_chain, taker_chain]
    };
    SwapCoordinator::new_with_confirmation_policies(
        SwapId::new("reorg-after-reveal").unwrap(),
        pair,
        direction,
        ConfirmationPolicy::new(2).unwrap(),
        ConfirmationPolicy::new(maker_policy).unwrap(),
        RecoverySchedule::new(
            pair,
            direction,
            ChainPosition::block_height(maker_chain, 100),
            ChainPosition::block_height(taker_chain, 120),
            TimelockSafety::between(safety_chains[0], safety_chains[1], 1_000, 1_200, 100).unwrap(),
        )
        .unwrap(),
    )
}

/// Both refund deadlines are long past.
fn maker_refund_position(swap: &SwapCoordinator) -> ChainPosition {
    ChainPosition::block_height(swap.funded_chain(Participant::Maker), 500)
}

fn taker_refund_position(swap: &SwapCoordinator) -> ChainPosition {
    ChainPosition::block_height(swap.funded_chain(Participant::Taker), 500)
}

fn revealed(pair: Pair, direction: SwapDirection) -> SwapCoordinator {
    let mut swap = coordinator(pair, direction, 1);
    swap.observe_taker_lock(ChainProof::new("taker-lock", 2).unwrap())
        .unwrap();
    swap.observe_maker_lock(ChainProof::new("maker-lock", 1).unwrap())
        .unwrap();
    swap.observe_revealing_claim(
        swap.first_claimant(),
        ChainProof::new("revealing-claim", 1).unwrap(),
        ClaimEvidence::new([5; 32]),
    )
    .unwrap();
    assert_eq!(swap.phase(), Phase::ClaimEvidenceAvailable);
    swap
}

/// Asserts both refunds are refused with the invalid-phase error and change nothing.
fn assert_refunds_refused(swap: &mut SwapCoordinator) {
    let before = swap.clone();
    let phase = swap.phase();
    let maker_position = maker_refund_position(swap);
    let taker_position = taker_refund_position(swap);
    assert_eq!(
        swap.refund_maker_leg(maker_position),
        Err(Error::InvalidPhase {
            expected: Phase::BothLegsLocked,
            actual: phase,
        })
    );
    assert_eq!(
        swap.refund_taker_leg(taker_position),
        Err(Error::InvalidPhase {
            expected: Phase::TakerLockConfirmed,
            actual: phase,
        })
    );
    assert_eq!(*swap, before, "a refused refund mutated the coordinator");
}

#[test]
fn maker_lock_removal_after_revealing_claim_admits_no_refund() {
    for (pair, direction) in PROFILES {
        let mut swap = revealed(pair, direction);
        assert_refunds_refused(&mut swap);

        swap.observe_maker_lock_removed("maker-lock").unwrap();
        assert_eq!(swap.phase(), Phase::MakerLockReorged);
        assert!(swap.claim_evidence().is_some());
        assert_refunds_refused(&mut swap);

        // The claim path is what stays open: the leg comes back and the follow-up completes.
        swap.observe_maker_lock(ChainProof::new("maker-lock", 1).unwrap())
            .unwrap();
        assert_eq!(swap.phase(), Phase::ClaimEvidenceAvailable);
        swap.observe_followup_claim(
            swap.first_claimant().other(),
            ChainProof::new("followup-claim", 1).unwrap(),
        )
        .unwrap();
        assert_eq!(swap.phase(), Phase::Completed, "{pair:?} {direction:?}");
    }
}

#[test]
fn taker_lock_removal_after_revealing_claim_admits_no_refund() {
    for (pair, direction) in PROFILES {
        let mut swap = revealed(pair, direction);
        assert_refunds_refused(&mut swap);

        swap.observe_taker_lock_removed("taker-lock").unwrap();
        assert_eq!(swap.phase(), Phase::TakerLockReorged);
        assert!(swap.claim_evidence().is_some());
        assert_refunds_refused(&mut swap);

        swap.observe_taker_lock(ChainProof::new("taker-lock", 2).unwrap())
            .unwrap();
        assert_eq!(swap.phase(), Phase::ClaimEvidenceAvailable);
        swap.observe_followup_claim(
            swap.first_claimant().other(),
            ChainProof::new("followup-claim", 1).unwrap(),
        )
        .unwrap();
        assert_eq!(swap.phase(), Phase::Completed, "{pair:?} {direction:?}");
    }
}

#[test]
fn depth_regression_after_revealing_claim_admits_no_refund() {
    for (pair, direction) in PROFILES {
        // The same reorg phases are reachable through a shallower re-observation of a lock.
        let mut taker_shallow = revealed(pair, direction);
        taker_shallow
            .observe_taker_lock(ChainProof::new("taker-lock", 1).unwrap())
            .unwrap();
        assert_eq!(taker_shallow.phase(), Phase::TakerLockReorged);
        assert_refunds_refused(&mut taker_shallow);

        let mut maker_shallow = coordinator(pair, direction, 2);
        maker_shallow
            .observe_taker_lock(ChainProof::new("taker-lock", 2).unwrap())
            .unwrap();
        maker_shallow
            .observe_maker_lock(ChainProof::new("maker-lock", 2).unwrap())
            .unwrap();
        maker_shallow
            .observe_revealing_claim(
                maker_shallow.first_claimant(),
                ChainProof::new("revealing-claim", 1).unwrap(),
                ClaimEvidence::new([5; 32]),
            )
            .unwrap();
        maker_shallow
            .observe_maker_lock(ChainProof::new("maker-lock", 1).unwrap())
            .unwrap();
        assert_eq!(maker_shallow.phase(), Phase::MakerLockReorged);
        assert_refunds_refused(&mut maker_shallow);
    }
}

#[test]
fn lock_removal_before_any_claim_still_refunds_both_legs() {
    for (pair, direction) in PROFILES {
        for remove_maker in [false, true] {
            let mut swap = coordinator(pair, direction, 1);
            swap.observe_taker_lock(ChainProof::new("taker-lock", 2).unwrap())
                .unwrap();
            swap.observe_maker_lock(ChainProof::new("maker-lock", 1).unwrap())
                .unwrap();
            if remove_maker {
                swap.observe_maker_lock_removed("maker-lock").unwrap();
            } else {
                swap.observe_taker_lock_removed("taker-lock").unwrap();
            }
            let maker_position = maker_refund_position(&swap);
            let taker_position = taker_refund_position(&swap);
            swap.refund_maker_leg(maker_position).unwrap();
            swap.refund_taker_leg(taker_position).unwrap();
            assert_eq!(swap.phase(), Phase::Refunded);
            assert!(swap.claim_evidence().is_none());
        }
    }
}

#[test]
fn shallow_maker_lock_during_taker_reorg_blocks_the_claim() {
    for (pair, direction) in PROFILES {
        // Issue #103 item 1: TakerLock(2), MakerLock(3), remove taker, MakerLock(1),
        // TakerLock(2), claim. The maker leg is below its policy of 2 when the claim arrives.
        let mut swap = coordinator(pair, direction, 2);
        swap.observe_taker_lock(ChainProof::new("taker-lock", 2).unwrap())
            .unwrap();
        swap.observe_maker_lock(ChainProof::new("maker-lock", 3).unwrap())
            .unwrap();
        assert_eq!(swap.phase(), Phase::BothLegsLocked);
        swap.observe_taker_lock_removed("taker-lock").unwrap();
        assert_eq!(swap.phase(), Phase::TakerLockReorged);
        swap.observe_maker_lock(ChainProof::new("maker-lock", 1).unwrap())
            .unwrap();
        assert_eq!(swap.phase(), Phase::TakerLockReorged);
        swap.observe_taker_lock(ChainProof::new("taker-lock", 2).unwrap())
            .unwrap();

        let before = swap.clone();
        assert_eq!(
            swap.observe_revealing_claim(
                swap.first_claimant(),
                ChainProof::new("revealing-claim", 1).unwrap(),
                ClaimEvidence::new([5; 32]),
            ),
            Err(Error::MakerLockNotConfirmed),
            "{pair:?} {direction:?}"
        );
        assert_eq!(swap, before);

        // Once the maker leg is back at policy depth the claim is admitted.
        swap.observe_maker_lock(ChainProof::new("maker-lock", 2).unwrap())
            .unwrap();
        swap.observe_revealing_claim(
            swap.first_claimant(),
            ChainProof::new("revealing-claim", 1).unwrap(),
            ClaimEvidence::new([5; 32]),
        )
        .unwrap();
        assert_eq!(swap.phase(), Phase::ClaimEvidenceAvailable);
    }
}

#[test]
fn maker_lock_returning_during_taker_reorg_restores_claim_authority() {
    for (pair, direction) in PROFILES {
        // The other side of item 1: a maker leg removed and re-observed at depth while the
        // taker leg is the reason for the phase must count once the taker leg returns.
        let mut swap = coordinator(pair, direction, 2);
        swap.observe_taker_lock(ChainProof::new("taker-lock", 2).unwrap())
            .unwrap();
        swap.observe_maker_lock(ChainProof::new("maker-lock", 2).unwrap())
            .unwrap();
        swap.observe_taker_lock_removed("taker-lock").unwrap();
        swap.observe_maker_lock_removed("maker-lock").unwrap();
        assert_eq!(swap.phase(), Phase::TakerLockReorged);
        swap.observe_maker_lock(ChainProof::new("maker-lock", 2).unwrap())
            .unwrap();
        swap.observe_taker_lock(ChainProof::new("taker-lock", 2).unwrap())
            .unwrap();
        assert_eq!(swap.phase(), Phase::BothLegsLocked);
        swap.observe_revealing_claim(
            swap.first_claimant(),
            ChainProof::new("revealing-claim", 1).unwrap(),
            ClaimEvidence::new([5; 32]),
        )
        .unwrap();
        assert_eq!(
            swap.phase(),
            Phase::ClaimEvidenceAvailable,
            "{pair:?} {direction:?}"
        );
    }
}
