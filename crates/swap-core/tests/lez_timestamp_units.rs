use lez_swap_core::{
    Chain, ChainPosition, Error, LezUnixMilliseconds, RecoverySchedule, SwapDirection,
    TimelockSafety, UnixSeconds,
};

#[test]
fn whole_second_terms_keep_exact_lez_millisecond_boundaries() {
    let deadline = UnixSeconds::new(1_700_000_000);
    let guest_deadline = deadline
        .checked_to_lez_milliseconds()
        .expect("reviewed Unix deadline fits the LEZ clock");
    let schedule = RecoverySchedule::new(
        lez_swap_core::Pair::Zcash,
        SwapDirection::TakerSellsForeign,
        ChainPosition::timestamp(Chain::Lez, deadline.value()),
        ChainPosition::block_height(Chain::Zcash, 192),
        TimelockSafety::between(Chain::Lez, Chain::Zcash, 7_200, 14_400, 7_200).unwrap(),
    )
    .unwrap();

    assert!(
        !schedule
            .maker_deadline_reached(ChainPosition::lez_timestamp_from_milliseconds_floor(
                LezUnixMilliseconds::new(guest_deadline.value() - 1),
            ))
            .unwrap()
    );
    assert!(
        schedule
            .maker_deadline_reached(ChainPosition::lez_timestamp_from_milliseconds_floor(
                guest_deadline,
            ))
            .unwrap()
    );
}

#[test]
fn millisecond_projections_name_floor_and_ceil_rounding() {
    let exact = LezUnixMilliseconds::new(12_000);
    assert_eq!(exact.to_unix_seconds_floor(), UnixSeconds::new(12));
    assert_eq!(exact.to_unix_seconds_ceil(), UnixSeconds::new(12));

    let partial = LezUnixMilliseconds::new(12_001);
    assert_eq!(partial.to_unix_seconds_floor(), UnixSeconds::new(12));
    assert_eq!(partial.to_unix_seconds_ceil(), UnixSeconds::new(13));

    let subsecond = LezUnixMilliseconds::new(999);
    assert_eq!(subsecond.to_unix_seconds_floor(), UnixSeconds::new(0));
    assert_eq!(subsecond.to_unix_seconds_ceil(), UnixSeconds::new(1));
}

#[test]
fn seconds_to_guest_milliseconds_fails_closed_on_overflow() {
    assert_eq!(
        UnixSeconds::new(u64::MAX).checked_to_lez_milliseconds(),
        Err(Error::TimestampConversionOverflow)
    );
    assert_eq!(
        UnixSeconds::new(u64::MAX / 1_000).checked_to_lez_milliseconds(),
        Ok(LezUnixMilliseconds::new((u64::MAX / 1_000) * 1_000))
    );
}
