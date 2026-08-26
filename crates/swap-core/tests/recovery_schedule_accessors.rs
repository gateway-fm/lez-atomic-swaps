use lez_swap_core::{
    Chain, ChainPosition, ConfirmationPolicy, Pair, RecoverySchedule, SwapCoordinator,
    SwapDirection, SwapId, TimelockSafety,
};

#[test]
fn zec_deadlines_are_addressable_by_chain_in_both_directions() {
    let lez = ChainPosition::timestamp(Chain::Lez, 1_000);
    let zec = ChainPosition::block_height(Chain::Zcash, 2_000);
    let safety = TimelockSafety::between(Chain::Lez, Chain::Zcash, 1_000, 1_200, 100)
        .expect("valid LEZ-before-ZEC safety interval");

    for (direction, maker, taker) in [
        (SwapDirection::TakerSellsForeign, lez, zec),
        (SwapDirection::TakerSellsLez, zec, lez),
    ] {
        let schedule = RecoverySchedule::new(Pair::Zcash, direction, maker, taker, safety)
            .expect("direction-correct ZEC schedule");

        assert_eq!(schedule.deadline_for_chain(Chain::Lez), Some(lez));
        assert_eq!(schedule.deadline_for_chain(Chain::Zcash), Some(zec));
        assert_eq!(schedule.deadline_for_chain(Chain::Bitcoin), None);
        assert_eq!(schedule.required_safety_margin_seconds(), Some(100));

        let coordinator = SwapCoordinator::new_with_direction(
            SwapId::new("schedule-accessors").unwrap(),
            Pair::Zcash,
            direction,
            ConfirmationPolicy::new(2).unwrap(),
            schedule,
        );
        assert_eq!(coordinator.recovery_schedule(), schedule);
    }
}

#[test]
fn xmr_event_gated_recovery_has_no_monero_deadline_or_numeric_safety_margin() {
    let lez_refund = ChainPosition::timestamp(Chain::Lez, 10_000);
    let schedule =
        RecoverySchedule::xmr_lez_first(lez_refund, 2).expect("valid event-gated XMR schedule");

    assert_eq!(schedule.deadline_for_chain(Chain::Lez), Some(lez_refund));
    assert_eq!(schedule.deadline_for_chain(Chain::Monero), None);
    assert_eq!(schedule.required_safety_margin_seconds(), None);
}
