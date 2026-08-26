use lez_swap_core::{
    Chain, ChainEventProof, ChainPosition, ChainProof, ConfirmationPolicy, Error,
    MakerRecoveryTrigger, Pair, Phase, RecoverySchedule, SwapCoordinator, SwapDirection, SwapId,
};

fn xmr_swap() -> SwapCoordinator {
    SwapCoordinator::new_with_direction(
        SwapId::new("xmr-event-recovery").unwrap(),
        Pair::Monero,
        SwapDirection::TakerSellsLez,
        ConfirmationPolicy::new(2).unwrap(),
        RecoverySchedule::xmr_lez_first(ChainPosition::timestamp(Chain::Lez, 10_000), 2).unwrap(),
    )
}

#[test]
fn xmr_profile_has_no_monero_deadline() {
    let schedule =
        RecoverySchedule::xmr_lez_first(ChainPosition::timestamp(Chain::Lez, 10_000), 2).unwrap();

    assert_eq!(
        schedule.maker_trigger(),
        MakerRecoveryTrigger::CanonicalTakerRefund {
            chain: Chain::Lez,
            required_confirmations: 2,
        }
    );
    assert_eq!(
        schedule.maker_deadline_reached(ChainPosition::block_height(Chain::Monero, 500)),
        Err(Error::RecoveryRequiresTakerRefundEvent)
    );
}

#[test]
fn maker_recovers_xmr_only_after_canonical_lez_refund_event() {
    let mut swap = xmr_swap();
    swap.observe_taker_lock(ChainProof::new("lez-lock", 2).unwrap())
        .unwrap();
    swap.observe_maker_lock(ChainProof::new("xmr-lock", 10).unwrap())
        .unwrap();

    assert_eq!(
        swap.refund_maker_leg(ChainPosition::block_height(Chain::Monero, u64::MAX)),
        Err(Error::RecoveryRequiresTakerRefundEvent)
    );
    swap.refund_taker_leg(ChainPosition::timestamp(Chain::Lez, 10_000))
        .unwrap();
    assert_eq!(swap.phase(), Phase::TakerLegRefunded);

    assert_eq!(
        swap.observe_taker_refund_for_maker_recovery(
            ChainEventProof::new(Chain::Monero, "wrong-chain", 2).unwrap()
        ),
        Err(Error::WrongRecoveryEventChain {
            expected: Chain::Lez,
            actual: Chain::Monero,
        })
    );
    assert_eq!(
        swap.observe_taker_refund_for_maker_recovery(
            ChainEventProof::new(Chain::Lez, "lez-refund", 1).unwrap()
        ),
        Err(Error::InsufficientRecoveryEventConfirmations {
            required: 2,
            actual: 1,
        })
    );

    let refund = ChainEventProof::new(Chain::Lez, "lez-refund", 2).unwrap();
    swap.observe_taker_refund_for_maker_recovery(refund.clone())
        .unwrap();
    assert_eq!(swap.phase(), Phase::MakerRecoveryAvailable);
    swap.observe_taker_refund_for_maker_recovery(
        ChainEventProof::new(Chain::Lez, "lez-refund", 1).unwrap(),
    )
    .unwrap();
    assert_eq!(swap.phase(), Phase::TakerLegRefunded);
    swap.observe_taker_refund_for_maker_recovery(refund)
        .unwrap();
    assert_eq!(swap.phase(), Phase::MakerRecoveryAvailable);

    let recovery = ChainProof::new("xmr-recovery", 10).unwrap();
    swap.observe_maker_recovery(recovery.clone()).unwrap();
    assert_eq!(swap.phase(), Phase::Refunded);
    swap.observe_maker_recovery(recovery).unwrap();
}
