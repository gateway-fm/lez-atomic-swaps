use lez_swap_core::{
    Chain, ChainPosition, ChainProof, ClaimEvidence, ConfirmationPolicy, Error, Pair, Phase,
    RefundSchedule, SwapCoordinator, SwapDirection, SwapId, TimelockSafety,
};

fn coordinator(pair: Pair) -> SwapCoordinator {
    let direction = supported_direction(pair);
    SwapCoordinator::new_with_direction(
        SwapId::new("swap-001").expect("valid swap id"),
        pair,
        direction,
        ConfirmationPolicy::new(2).expect("non-zero confirmation policy"),
        schedule(pair, direction),
    )
}

fn supported_direction(pair: Pair) -> SwapDirection {
    if pair == Pair::Monero {
        SwapDirection::TakerSellsLez
    } else {
        SwapDirection::TakerSellsForeign
    }
}

fn schedule(pair: Pair, direction: SwapDirection) -> RefundSchedule {
    let foreign = Chain::from(pair);
    let (maker_chain, taker_chain) = match direction {
        SwapDirection::TakerSellsForeign => (Chain::Lez, foreign),
        SwapDirection::TakerSellsLez => (foreign, Chain::Lez),
    };
    RefundSchedule::new(
        pair,
        direction,
        ChainPosition::block_height(maker_chain, 100),
        ChainPosition::block_height(taker_chain, 120),
        TimelockSafety::new(1_000, 1_200, 100).unwrap(),
    )
    .unwrap()
}

fn maker_height(value: u64) -> ChainPosition {
    ChainPosition::block_height(Chain::Lez, value)
}

fn taker_height(pair: Pair, value: u64) -> ChainPosition {
    ChainPosition::block_height(Chain::from(pair), value)
}

#[test]
fn happy_path_enforces_taker_first_then_completes_from_on_chain_evidence() {
    for pair in [Pair::Bitcoin, Pair::Monero, Pair::Zcash] {
        let mut swap = coordinator(pair);

        let err = swap
            .observe_maker_lez_lock(ChainProof::new("lez-lock", 1).unwrap())
            .expect_err("maker must not lock before the taker's lock is confirmed");
        assert_eq!(err, Error::TakerLockNotConfirmed);

        swap.observe_taker_foreign_lock(ChainProof::new("foreign-lock", 1).unwrap())
            .expect("unconfirmed lock is tracked");
        assert_eq!(swap.phase(), Phase::AwaitingTakerConfirmations);

        let err = swap
            .observe_maker_lez_lock(ChainProof::new("lez-lock", 1).unwrap())
            .expect_err("one confirmation is below policy");
        assert_eq!(err, Error::TakerLockNotConfirmed);

        swap.observe_taker_foreign_lock(ChainProof::new("foreign-lock", 2).unwrap())
            .expect("the same lock can gain confirmations");
        assert_eq!(swap.phase(), Phase::TakerLockConfirmed);

        swap.observe_maker_lez_lock(ChainProof::new("lez-lock", 1).unwrap())
            .expect("maker may lock only after taker confirmation");
        assert_eq!(swap.phase(), Phase::BothLegsLocked);

        // From the first lock onward every input is durable on-chain evidence. No Delivery,
        // Chat, daemon, or peer handle is needed to reach a terminal state.
        swap.observe_maker_claim(ClaimEvidence::new([7; 32]))
            .expect("maker claim reveals the adaptor secret or HTLC preimage");
        assert_eq!(swap.phase(), Phase::ClaimEvidenceAvailable);

        swap.observe_taker_lez_claim(ChainProof::new("lez-claim", 1).unwrap())
            .expect("taker uses the revealed witness to claim LEZ funds");
        assert_eq!(swap.phase(), Phase::Completed);
    }
}

#[test]
fn timeout_path_refunds_both_legs_without_off_chain_coordination() {
    let mut swap = coordinator(Pair::Zcash);
    swap.observe_taker_foreign_lock(ChainProof::new("zec-lock", 2).unwrap())
        .unwrap();
    swap.observe_maker_lez_lock(ChainProof::new("lez-lock", 1).unwrap())
        .unwrap();

    assert_eq!(
        swap.refund_maker_leg(maker_height(99)),
        Err(Error::TimelockNotExpired)
    );
    swap.refund_maker_leg(maker_height(100))
        .expect("LEZ refund is available at its deadline");
    assert_eq!(swap.phase(), Phase::MakerLegRefunded);

    assert_eq!(
        swap.refund_taker_leg(taker_height(Pair::Zcash, 119)),
        Err(Error::TimelockNotExpired)
    );
    swap.refund_taker_leg(taker_height(Pair::Zcash, 120))
        .expect("foreign refund follows the shorter LEZ deadline");
    assert_eq!(swap.phase(), Phase::Refunded);
}

#[test]
fn taker_recovers_when_maker_never_locks() {
    let mut swap = coordinator(Pair::Bitcoin);
    swap.observe_taker_foreign_lock(ChainProof::new("btc-lock", 2).unwrap())
        .unwrap();

    assert_eq!(
        swap.refund_taker_leg(taker_height(Pair::Bitcoin, 119)),
        Err(Error::TimelockNotExpired)
    );
    swap.refund_taker_leg(taker_height(Pair::Bitcoin, 120))
        .expect("taker can recover without waiting for an absent maker");
    assert_eq!(swap.phase(), Phase::Refunded);
}

#[test]
fn delayed_observation_of_lez_refund_does_not_block_foreign_refund() {
    let mut swap = coordinator(Pair::Zcash);
    swap.observe_taker_foreign_lock(ChainProof::new("zec-lock", 2).unwrap())
        .unwrap();
    swap.observe_maker_lez_lock(ChainProof::new("lez-lock", 1).unwrap())
        .unwrap();

    // Both deadlines have passed, but the taker observes its own refund first.
    swap.refund_taker_leg(taker_height(Pair::Zcash, 120))
        .expect("refund safety must not depend on observation order");
    assert_eq!(swap.phase(), Phase::TakerLegRefunded);

    swap.refund_maker_leg(maker_height(120))
        .expect("maker refund completes the independently observed recovery");
    assert_eq!(swap.phase(), Phase::Refunded);
}

#[test]
fn replayed_chain_observations_are_idempotent() {
    let mut swap = coordinator(Pair::Monero);
    let foreign_lock = ChainProof::new("xmr-lock", 2).unwrap();
    let lez_lock = ChainProof::new("lez-lock", 1).unwrap();
    let claim_evidence = ClaimEvidence::new([11; 32]);
    let lez_claim = ChainProof::new("lez-claim", 1).unwrap();

    swap.observe_taker_foreign_lock(foreign_lock.clone())
        .unwrap();
    swap.observe_taker_foreign_lock(foreign_lock)
        .expect("replayed foreign lock is harmless");

    swap.observe_maker_lez_lock(lez_lock.clone()).unwrap();
    swap.observe_maker_lez_lock(lez_lock)
        .expect("replayed LEZ lock is harmless");

    swap.observe_maker_claim(claim_evidence.clone()).unwrap();
    swap.observe_maker_claim(claim_evidence)
        .expect("replayed claim evidence is harmless");

    swap.observe_taker_lez_claim(lez_claim.clone()).unwrap();
    swap.observe_taker_lez_claim(lez_claim)
        .expect("replayed LEZ claim is harmless");
    assert_eq!(swap.phase(), Phase::Completed);

    assert_eq!(
        swap.observe_taker_foreign_lock(ChainProof::new("other-xmr-lock", 2).unwrap()),
        Err(Error::ConflictingTakerLock)
    );
    assert_eq!(
        swap.observe_maker_lez_lock(ChainProof::new("other-lez-lock", 1).unwrap()),
        Err(Error::ConflictingMakerLock)
    );
    assert_eq!(
        swap.observe_maker_claim(ClaimEvidence::new([12; 32])),
        Err(Error::ConflictingClaimEvidence)
    );
    assert_eq!(
        swap.observe_taker_lez_claim(ChainProof::new("other-lez-claim", 1).unwrap()),
        Err(Error::ConflictingTakerClaim)
    );
}

#[test]
fn timelocks_reject_unsafe_ordering() {
    assert_eq!(
        TimelockSafety::new(1_000, 1_099, 100),
        Err(Error::InsufficientTimelockMargin)
    );
}

#[test]
fn taker_lock_reorg_revokes_claim_authority_but_preserves_refunds() {
    let mut before_maker = coordinator(Pair::Bitcoin);
    before_maker
        .observe_taker_lock(ChainProof::new("btc-lock", 2).unwrap())
        .unwrap();
    assert_eq!(before_maker.phase(), Phase::TakerLockConfirmed);
    before_maker
        .observe_taker_lock(ChainProof::new("btc-lock", 1).unwrap())
        .unwrap();
    assert_eq!(before_maker.phase(), Phase::AwaitingTakerConfirmations);
    assert_eq!(
        before_maker.observe_maker_lock(ChainProof::new("lez-lock", 1).unwrap()),
        Err(Error::TakerLockNotConfirmed)
    );

    let mut after_maker = coordinator(Pair::Bitcoin);
    after_maker
        .observe_taker_lock(ChainProof::new("btc-lock", 2).unwrap())
        .unwrap();
    after_maker
        .observe_maker_lock(ChainProof::new("lez-lock", 1).unwrap())
        .unwrap();
    after_maker
        .observe_taker_lock(ChainProof::new("btc-lock", 1).unwrap())
        .expect("a canonicality regression is a durable observation");
    assert_eq!(after_maker.phase(), Phase::TakerLockReorged);
    assert_eq!(
        after_maker.observe_maker_claim(ClaimEvidence::new([9; 32])),
        Err(Error::TakerLockNotConfirmed)
    );

    after_maker.refund_maker_leg(maker_height(100)).unwrap();
    after_maker
        .refund_taker_leg(taker_height(Pair::Bitcoin, 120))
        .unwrap();
    assert_eq!(after_maker.phase(), Phase::Refunded);
}

#[test]
fn removed_uncommitted_taker_lock_can_be_replaced_but_committed_lock_cannot() {
    let mut before_maker = coordinator(Pair::Zcash);
    before_maker
        .observe_taker_lock(ChainProof::new("zec-lock-a", 1).unwrap())
        .unwrap();
    before_maker
        .observe_taker_lock_removed("zec-lock-a")
        .unwrap();
    assert_eq!(before_maker.phase(), Phase::Offered);
    before_maker
        .observe_taker_lock(ChainProof::new("zec-lock-b", 2).unwrap())
        .expect("a removed pre-maker transaction may be replaced explicitly");

    let mut after_maker = coordinator(Pair::Zcash);
    after_maker
        .observe_taker_lock(ChainProof::new("zec-lock-a", 2).unwrap())
        .unwrap();
    after_maker
        .observe_maker_lock(ChainProof::new("lez-lock", 1).unwrap())
        .unwrap();
    after_maker
        .observe_taker_lock_removed("zec-lock-a")
        .unwrap();
    assert_eq!(after_maker.phase(), Phase::TakerLockReorged);
    assert_eq!(
        after_maker.observe_taker_lock(ChainProof::new("zec-lock-b", 2).unwrap()),
        Err(Error::ConflictingTakerLock)
    );
}
