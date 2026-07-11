use lez_swap_core::{
    Chain, ChainPosition, ClockBasis, Error, Pair, RecoverySchedule, SwapDirection, TimelockSafety,
};

#[test]
fn reverse_direction_maps_role_deadlines_to_the_correct_chains() {
    let schedule = RecoverySchedule::new(
        Pair::Bitcoin,
        SwapDirection::TakerSellsLez,
        ChainPosition::block_height(Chain::Bitcoin, 500),
        ChainPosition::block_height(Chain::Lez, 700),
        TimelockSafety::between(Chain::Bitcoin, Chain::Lez, 1_000, 1_400, 300).unwrap(),
    )
    .unwrap();

    assert!(
        !schedule
            .maker_deadline_reached(ChainPosition::block_height(Chain::Bitcoin, 499))
            .unwrap()
    );
    assert!(
        schedule
            .maker_deadline_reached(ChainPosition::block_height(Chain::Bitcoin, 500))
            .unwrap()
    );
    assert!(
        schedule
            .taker_refund_reached(ChainPosition::block_height(Chain::Lez, 700))
            .unwrap()
    );
    assert_eq!(
        schedule.maker_deadline_reached(ChainPosition::block_height(Chain::Lez, 500)),
        Err(Error::WrongDeadlineClock {
            expected_chain: Chain::Bitcoin,
            expected_basis: ClockBasis::BlockHeight,
            actual_chain: Chain::Lez,
            actual_basis: ClockBasis::BlockHeight,
        })
    );
}

#[test]
fn schedule_rejects_wrong_role_chain_and_insufficient_cross_chain_margin() {
    let safe = TimelockSafety::between(Chain::Lez, Chain::Zcash, 1_000, 1_400, 300).unwrap();
    assert_eq!(
        RecoverySchedule::new(
            Pair::Zcash,
            SwapDirection::TakerSellsForeign,
            ChainPosition::block_height(Chain::Zcash, 100),
            ChainPosition::block_height(Chain::Lez, 120),
            safe,
        ),
        Err(Error::WrongRefundChain {
            role: "maker",
            expected: Chain::Lez,
            actual: Chain::Zcash,
        })
    );
    assert_eq!(
        TimelockSafety::between(Chain::Lez, Chain::Zcash, 1_000, 1_250, 300),
        Err(Error::InsufficientTimelockMargin)
    );
}

#[test]
fn block_height_and_timestamp_are_never_compared_as_raw_numbers() {
    let schedule = RecoverySchedule::new(
        Pair::Bitcoin,
        SwapDirection::TakerSellsForeign,
        ChainPosition::timestamp(Chain::Lez, 1_800_000_000),
        ChainPosition::block_height(Chain::Bitcoin, 850_000),
        TimelockSafety::between(
            Chain::Lez,
            Chain::Bitcoin,
            1_800_000_000,
            1_800_003_600,
            1_800,
        )
        .unwrap(),
    )
    .unwrap();

    assert_eq!(
        schedule.maker_deadline_reached(ChainPosition::block_height(Chain::Lez, 1_800_000_000)),
        Err(Error::WrongDeadlineClock {
            expected_chain: Chain::Lez,
            expected_basis: ClockBasis::Timestamp,
            actual_chain: Chain::Lez,
            actual_basis: ClockBasis::BlockHeight,
        })
    );
}

#[test]
fn monero_first_is_rejected_until_a_reviewed_construction_exists() {
    assert_eq!(
        RecoverySchedule::new(
            Pair::Monero,
            SwapDirection::TakerSellsForeign,
            ChainPosition::block_height(Chain::Lez, 500),
            ChainPosition::block_height(Chain::Monero, 700),
            TimelockSafety::between(Chain::Lez, Chain::Monero, 1_000, 1_400, 300).unwrap(),
        ),
        Err(Error::UnsupportedDirection {
            pair: Pair::Monero,
            direction: SwapDirection::TakerSellsForeign,
        })
    );
}
