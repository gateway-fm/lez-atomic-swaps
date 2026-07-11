use lez_swap_core::{
    ChainProof, ClaimEvidence, ConfirmationPolicy, Error, Pair, Phase, SwapCoordinator, SwapId,
    Timelocks,
};

fn coordinator(pair: Pair) -> SwapCoordinator {
    SwapCoordinator::new(
        SwapId::new("swap-001").expect("valid swap id"),
        pair,
        ConfirmationPolicy::new(2).expect("non-zero confirmation policy"),
        Timelocks::new(100, 120).expect("foreign refund follows LEZ refund"),
    )
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
        swap.refund_maker_lez_leg(99),
        Err(Error::TimelockNotExpired)
    );
    swap.refund_maker_lez_leg(100)
        .expect("LEZ refund is available at its deadline");
    assert_eq!(swap.phase(), Phase::MakerLegRefunded);

    assert_eq!(
        swap.refund_taker_foreign_leg(119),
        Err(Error::TimelockNotExpired)
    );
    swap.refund_taker_foreign_leg(120)
        .expect("foreign refund follows the shorter LEZ deadline");
    assert_eq!(swap.phase(), Phase::Refunded);
}

#[test]
fn timelocks_reject_unsafe_ordering() {
    assert_eq!(
        Timelocks::new(100, 100),
        Err(Error::ForeignTimelockMustFollowLez)
    );
    assert_eq!(
        Timelocks::new(101, 100),
        Err(Error::ForeignTimelockMustFollowLez)
    );
}
