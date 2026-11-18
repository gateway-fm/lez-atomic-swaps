use lez_swap_core::{Chain, ChainPosition, LezUnixMilliseconds, SwapDirection, UnixSeconds};
use lez_zec_swap_sdk::{
    PUBLIC_TESTNET_FASTEST_BLOCK_MILLIS, PUBLIC_TESTNET_SLOWEST_BLOCK_MILLIS, ProfileError,
    ZecProfileId, ZecRefundProfile, ZecRevealOrder,
};
use zcash_protocol::consensus::{BlockHeight, BranchId, NetworkType};

#[test]
fn named_profiles_match_the_reviewed_immutable_parameters() {
    let local = ZecRefundProfile::for_id(ZecProfileId::DeterministicLocalV1);
    assert_eq!(local.id().as_str(), "deterministic-local-v1");
    assert_eq!(local.zcash_network(), NetworkType::Regtest);
    assert_eq!(local.consensus_branch_id(), BranchId::Nu6_2);
    assert_eq!(local.lez_confirmations(), 1);
    assert_eq!(local.zcash_confirmations(), 1);
    assert_eq!(local.lez_refund_delay(), UnixSeconds::new(60));
    assert_eq!(local.zcash_refund_blocks(), 4);
    assert_eq!(local.required_margin(), UnixSeconds::new(30));
    assert_eq!(local.expiry_delta_blocks(), 40);

    let devnet = ZecRefundProfile::for_id(ZecProfileId::DeterministicLocalDevnetV1);
    assert_eq!(devnet.id().as_str(), "deterministic-local-devnet-v1");
    assert_eq!(devnet.zcash_network(), NetworkType::Regtest);
    assert_eq!(devnet.consensus_branch_id(), BranchId::Nu6_2);
    assert_eq!(devnet.lez_confirmations(), 1);
    assert_eq!(devnet.zcash_confirmations(), 1);
    assert_eq!(devnet.lez_refund_delay(), UnixSeconds::new(1_800));
    assert_eq!(devnet.zcash_refund_blocks(), 4);
    assert_eq!(devnet.required_margin(), UnixSeconds::new(30));
    assert_eq!(devnet.expiry_delta_blocks(), 40);

    let public = ZecRefundProfile::for_id(ZecProfileId::PublicTestnetV1);
    assert_eq!(public.id().as_str(), "public-testnet-v1");
    assert_eq!(public.zcash_network(), NetworkType::Test);
    assert_eq!(public.consensus_branch_id(), BranchId::Nu6_2);
    assert_eq!(public.lez_confirmations(), 2);
    assert_eq!(public.zcash_confirmations(), 10);
    assert_eq!(public.lez_refund_delay(), UnixSeconds::new(7_200));
    assert_eq!(public.zcash_refund_blocks(), 192);
    assert_eq!(public.required_margin(), UnixSeconds::new(7_200));
    assert_eq!(public.expiry_delta_blocks(), 40);
}

#[test]
fn profiles_reject_wrong_network_and_consensus_branch() {
    let local = ZecRefundProfile::for_id(ZecProfileId::DeterministicLocalV1);
    assert_eq!(
        local.validate_consensus(NetworkType::Test, BranchId::Nu6_2),
        Err(ProfileError::NetworkMismatch)
    );
    assert_eq!(
        local.validate_consensus(NetworkType::Regtest, BranchId::Nu6_1),
        Err(ProfileError::ConsensusBranchMismatch)
    );

    let public = ZecRefundProfile::for_id(ZecProfileId::PublicTestnetV1);
    assert_eq!(
        public.validate_consensus(NetworkType::Regtest, BranchId::Nu6_2),
        Err(ProfileError::NetworkMismatch)
    );
    assert!(
        public
            .validate_consensus(NetworkType::Test, BranchId::Nu6_2)
            .is_ok()
    );
}

#[test]
fn deadline_construction_fails_closed_on_timestamp_and_height_overflow() {
    let public = ZecRefundProfile::for_id(ZecProfileId::PublicTestnetV1);
    assert_eq!(
        public.lez_refund_at(UnixSeconds::new(u64::MAX)),
        Err(ProfileError::TimestampOverflow)
    );

    let last_valid = BlockHeight::from_u32(u32::MAX - public.zcash_refund_blocks());
    assert_eq!(
        public.zcash_refund_at(last_valid),
        Ok(BlockHeight::from_u32(u32::MAX))
    );
    assert_eq!(
        public.zcash_refund_at(BlockHeight::from_u32(u32::from(last_valid) + 1)),
        Err(ProfileError::HeightOverflow)
    );
}

#[test]
fn calibrated_bounds_ceil_lez_milliseconds_and_require_the_exact_margin() {
    let local = ZecRefundProfile::for_id(ZecProfileId::DeterministicLocalV1);
    let lez_deadline = UnixSeconds::new(60);
    let zec_deadline = BlockHeight::from_u32(4);
    let earlier_latest = LezUnixMilliseconds::new(12_001);

    assert_eq!(
        local.recovery_schedule(
            SwapDirection::TakerSellsForeign,
            lez_deadline,
            zec_deadline,
            earlier_latest,
            None,
        ),
        Err(ProfileError::MissingSafetyCalibration)
    );
    assert_eq!(
        local.recovery_schedule(
            SwapDirection::TakerSellsForeign,
            lez_deadline,
            zec_deadline,
            earlier_latest,
            Some(UnixSeconds::new(42)),
        ),
        Err(ProfileError::InsufficientSafetyMargin)
    );
    assert!(
        local
            .recovery_schedule(
                SwapDirection::TakerSellsForeign,
                lez_deadline,
                zec_deadline,
                earlier_latest,
                Some(UnixSeconds::new(43)),
            )
            .is_ok()
    );
}

#[test]
fn each_direction_refunds_the_maker_funded_leg_first_while_mapping_role_deadlines() {
    let lez = ChainPosition::timestamp(Chain::Lez, 60);
    let zec = ChainPosition::block_height(Chain::Zcash, 4);
    // TakerSellsForeign: the Maker's LEZ refund, then the Taker's Zcash.
    let legacy = ZecRefundProfile::for_id(ZecProfileId::DeterministicLocalV1);
    let schedule = legacy
        .recovery_schedule(
            SwapDirection::TakerSellsForeign,
            UnixSeconds::new(60),
            BlockHeight::from_u32(4),
            LezUnixMilliseconds::new(13_000),
            Some(UnixSeconds::new(43)),
        )
        .unwrap();
    assert!(schedule.maker_deadline_reached(lez).unwrap());
    assert!(schedule.taker_refund_reached(zec).unwrap());
    // TakerSellsLez under a LEZ-recipient-first profile is unsafe.
    assert_eq!(
        legacy.recovery_schedule(
            SwapDirection::TakerSellsLez,
            UnixSeconds::new(60),
            BlockHeight::from_u32(4),
            LezUnixMilliseconds::new(13_000),
            Some(UnixSeconds::new(43)),
        ),
        Err(ProfileError::UnsafeRevealOrder)
    );
    // TakerSellsLez: the Maker's Zcash refund opens by 30 s (the earlier
    // one), the Taker's LEZ refund at 60 s, the 30 s margin later.
    let taker_first = ZecRefundProfile::for_id(ZecProfileId::DeterministicLocalV3);
    let schedule = taker_first
        .recovery_schedule(
            SwapDirection::TakerSellsLez,
            UnixSeconds::new(60),
            BlockHeight::from_u32(4),
            LezUnixMilliseconds::new(30_000),
            Some(UnixSeconds::new(60)),
        )
        .unwrap();
    assert!(schedule.maker_deadline_reached(zec).unwrap());
    assert!(schedule.taker_refund_reached(lez).unwrap());
    assert_eq!(
        taker_first.recovery_schedule(
            SwapDirection::TakerSellsLez,
            UnixSeconds::new(60),
            BlockHeight::from_u32(4),
            LezUnixMilliseconds::new(30_001),
            Some(UnixSeconds::new(60)),
        ),
        Err(ProfileError::InsufficientSafetyMargin)
    );
    // The Taker-reveals-first profile is sized for TakerSellsLez only.
    assert_eq!(
        taker_first.recovery_schedule(
            SwapDirection::TakerSellsForeign,
            UnixSeconds::new(60),
            BlockHeight::from_u32(4),
            LezUnixMilliseconds::new(60_000),
            Some(UnixSeconds::new(90)),
        ),
        Err(ProfileError::UnsafeRevealOrder)
    );
}

/// The Taker-reveals-first profiles: new ids for a new meaning, every older
/// id unchanged and admitted only for `TakerSellsForeign`.
#[test]
fn taker_reveals_first_profiles_match_the_reviewed_parameters() {
    for (id, spelling, network, delay, blocks, margin, confirmations, reveal_margin, slowest) in [
        (
            ZecProfileId::DeterministicLocalV3,
            "deterministic-local-v3",
            NetworkType::Regtest,
            120,
            8,
            30,
            (1, 1),
            2,
            None,
        ),
        (
            ZecProfileId::DeterministicLocalDevnetV7,
            "deterministic-local-devnet-v7",
            NetworkType::Regtest,
            720,
            16,
            120,
            (1, 1),
            3,
            None,
        ),
        (
            ZecProfileId::DeterministicLocalDevnetV8,
            "deterministic-local-devnet-v8",
            NetworkType::Regtest,
            480,
            16,
            120,
            (1, 1),
            3,
            None,
        ),
        (
            ZecProfileId::PublicTestnetV3,
            "public-testnet-v3",
            NetworkType::Test,
            14_400,
            95,
            7_200,
            (2, 10),
            10,
            Some(PUBLIC_TESTNET_SLOWEST_BLOCK_MILLIS),
        ),
    ] {
        let profile = ZecRefundProfile::for_id(id);
        assert_eq!(profile.id().as_str(), spelling);
        assert_eq!(profile.zcash_network(), network);
        assert_eq!(profile.consensus_branch_id(), BranchId::Nu7);
        assert_eq!(profile.lez_refund_delay(), UnixSeconds::new(delay));
        assert_eq!(profile.zcash_refund_blocks(), blocks);
        assert_eq!(profile.required_margin(), UnixSeconds::new(margin));
        assert_eq!(
            (profile.lez_confirmations(), profile.zcash_confirmations()),
            confirmations
        );
        assert_eq!(profile.zcash_reveal_margin_blocks(), reveal_margin);
        assert_eq!(profile.zcash_slowest_block_millis(), slowest);
        assert_eq!(profile.reveal_order(), ZecRevealOrder::TakerFirst);
        assert!(profile.admits(SwapDirection::TakerSellsLez));
        assert!(!profile.admits(SwapDirection::TakerSellsForeign));
    }
    // public-testnet-v3: the anchor is the tip plus one, so the Maker's
    // Zcash refund is at most 96 blocks away: 96 x 75 s = 7 200 s, and the
    // Taker's LEZ refund is the margin after that.
    let public = ZecRefundProfile::for_id(ZecProfileId::PublicTestnetV3);
    assert_eq!(
        (u64::from(public.zcash_refund_blocks()) + 1) * PUBLIC_TESTNET_SLOWEST_BLOCK_MILLIS
            + public.required_margin().value() * 1_000,
        public.lez_refund_delay().value() * 1_000
    );
    for legacy in [
        ZecProfileId::DeterministicLocalV1,
        ZecProfileId::DeterministicLocalDevnetV1,
        ZecProfileId::DeterministicLocalDevnetV2,
        ZecProfileId::DeterministicLocalDevnetV3,
        ZecProfileId::DeterministicLocalDevnetV4,
        ZecProfileId::PublicTestnetV1,
        ZecProfileId::PublicTestnetV2,
        ZecProfileId::DeterministicLocalV2,
        ZecProfileId::DeterministicLocalDevnetV5,
        ZecProfileId::DeterministicLocalDevnetV6,
    ] {
        let profile = ZecRefundProfile::for_id(legacy);
        assert_eq!(profile.reveal_order(), ZecRevealOrder::LezRecipientFirst);
        assert!(!profile.admits(SwapDirection::TakerSellsLez), "{legacy:?}");
        assert!(
            profile.admits(SwapDirection::TakerSellsForeign),
            "{legacy:?}"
        );
    }
}

/// Profiles are immutable per id: the devnet's wider Zcash window is V2, V1
/// keeps its four blocks so every agreement signed under it still validates.
#[test]
fn devnet_v2_widens_the_zcash_refund_window_and_v1_keeps_its_four_blocks() {
    let v1 = ZecRefundProfile::for_id(ZecProfileId::DeterministicLocalDevnetV1);
    let v2 = ZecRefundProfile::for_id(ZecProfileId::DeterministicLocalDevnetV2);
    assert_eq!(v2.id().as_str(), "deterministic-local-devnet-v2");
    assert_eq!(v1.zcash_refund_blocks(), 4);
    assert_eq!(v2.zcash_refund_blocks(), 8);
    assert_eq!(v1.lez_refund_delay(), v2.lez_refund_delay());
    assert_eq!(v1.required_margin(), v2.required_margin());
    assert_eq!(v1.lez_confirmations(), v2.lez_confirmations());
    assert_eq!(v1.zcash_confirmations(), v2.zcash_confirmations());
    assert_eq!(v1.zcash_network(), v2.zcash_network());
}

/// V3 is V2 with a 12-minute LEZ refund delay (the devnet chain's finality
/// lives ~27 min on the review host, upstream #3643); V2 keeps its 30 minutes.
#[test]
fn devnet_v3_shortens_the_lez_refund_delay_and_keeps_every_other_v2_parameter() {
    let v2 = ZecRefundProfile::for_id(ZecProfileId::DeterministicLocalDevnetV2);
    let v3 = ZecRefundProfile::for_id(ZecProfileId::DeterministicLocalDevnetV3);
    assert_eq!(v3.id().as_str(), "deterministic-local-devnet-v3");
    assert_eq!(v2.lez_refund_delay().value(), 1_800);
    assert_eq!(v3.lez_refund_delay().value(), 720);
    assert_eq!(v2.zcash_refund_blocks(), v3.zcash_refund_blocks());
    assert_eq!(v2.required_margin(), v3.required_margin());
    assert_eq!(v2.lez_confirmations(), v3.lez_confirmations());
    assert_eq!(v2.zcash_confirmations(), v3.zcash_confirmations());
    assert_eq!(v2.zcash_network(), v3.zcash_network());
}

/// V4 is V3 with a 4-minute LEZ refund delay, for the devnet's refund journeys
/// (they never claim); V3 keeps its 12 minutes for every swap that claims.
#[test]
fn devnet_v4_shortens_the_lez_refund_delay_and_keeps_every_other_v3_parameter() {
    let v3 = ZecRefundProfile::for_id(ZecProfileId::DeterministicLocalDevnetV3);
    let v4 = ZecRefundProfile::for_id(ZecProfileId::DeterministicLocalDevnetV4);
    assert_eq!(v4.id().as_str(), "deterministic-local-devnet-v4");
    assert_eq!(v3.lez_refund_delay().value(), 720);
    assert_eq!(v4.lez_refund_delay().value(), 240);
    assert_eq!(v3.zcash_refund_blocks(), v4.zcash_refund_blocks());
    assert_eq!(v3.required_margin(), v4.required_margin());
    assert_eq!(v3.lez_confirmations(), v4.lez_confirmations());
    assert_eq!(v3.zcash_confirmations(), v4.zcash_confirmations());
    assert_eq!(v3.zcash_network(), v4.zcash_network());
    assert_eq!(v3.consensus_branch_id(), v4.consensus_branch_id());
    assert_eq!(v3.expiry_delta_blocks(), v4.expiry_delta_blocks());
}

/// V2 is V1 on NU7 with a Zcash refund window that still opens no earlier than
/// the LEZ refund delay plus the margin at Testnet's fastest measured pace.
#[test]
fn public_testnet_v2_signs_for_nu7_and_sizes_its_window_for_the_fastest_pace() {
    let v1 = ZecRefundProfile::for_id(ZecProfileId::PublicTestnetV1);
    let v2 = ZecRefundProfile::for_id(ZecProfileId::PublicTestnetV2);
    assert_eq!(v2.id().as_str(), "public-testnet-v2");
    assert_eq!(v2.zcash_network(), NetworkType::Test);
    assert_eq!(v2.consensus_branch_id(), BranchId::Nu7);
    assert_eq!(u32::from(v2.consensus_branch_id()), 0x7719_0ad9);
    assert_eq!(v2.lez_confirmations(), 2);
    assert_eq!(v2.zcash_confirmations(), 10);
    assert_eq!(v2.lez_refund_delay(), UnixSeconds::new(7_200));
    assert_eq!(v2.zcash_refund_blocks(), 3_700);
    assert_eq!(v2.required_margin(), UnixSeconds::new(7_200));
    assert_eq!(v2.expiry_delta_blocks(), 120);
    assert_eq!(v1.lez_refund_delay(), v2.lez_refund_delay());
    assert_eq!(v1.required_margin(), v2.required_margin());
    // V1 is unchanged: NU6.2, 192 blocks.
    assert_eq!(v1.consensus_branch_id(), BranchId::Nu6_2);
    assert_eq!(v1.zcash_refund_blocks(), 192);

    let needed_millis = (v2.lez_refund_delay().value() + v2.required_margin().value()) * 1_000;
    let fastest_window_millis =
        u64::from(v2.zcash_refund_blocks()) * PUBLIC_TESTNET_FASTEST_BLOCK_MILLIS;
    assert!(fastest_window_millis >= needed_millis);
    assert!(
        u64::from(v1.zcash_refund_blocks()) * PUBLIC_TESTNET_FASTEST_BLOCK_MILLIS < needed_millis
    );
    assert!(
        v2.validate_consensus(NetworkType::Test, BranchId::Nu7)
            .is_ok()
    );
    assert_eq!(
        v2.validate_consensus(NetworkType::Test, BranchId::Nu6_2),
        Err(ProfileError::ConsensusBranchMismatch)
    );
    assert_eq!(
        v2.validate_consensus(NetworkType::Test, BranchId::Nu6_3),
        Err(ProfileError::ConsensusBranchMismatch)
    );
}

/// The NU7 Regtest profiles are their NU6.2 namesakes on NU7, parameter for
/// parameter: local-v2 is local-v1's, devnet-v5 is v3's, devnet-v6 is v4's.
/// The NU6.2 ones keep their meaning under their ids.
#[test]
fn nu7_regtest_profiles_are_their_nu6_2_namesakes_on_nu7() {
    for (old, new, spelling) in [
        (
            ZecProfileId::DeterministicLocalV1,
            ZecProfileId::DeterministicLocalV2,
            "deterministic-local-v2",
        ),
        (
            ZecProfileId::DeterministicLocalDevnetV3,
            ZecProfileId::DeterministicLocalDevnetV5,
            "deterministic-local-devnet-v5",
        ),
        (
            ZecProfileId::DeterministicLocalDevnetV4,
            ZecProfileId::DeterministicLocalDevnetV6,
            "deterministic-local-devnet-v6",
        ),
    ] {
        let old = ZecRefundProfile::for_id(old);
        let new = ZecRefundProfile::for_id(new);
        assert_eq!(new.id().as_str(), spelling);
        assert_eq!(old.consensus_branch_id(), BranchId::Nu6_2);
        assert_eq!(new.consensus_branch_id(), BranchId::Nu7);
        assert_eq!(new.zcash_network(), NetworkType::Regtest);
        assert_eq!(old.zcash_network(), new.zcash_network());
        assert_eq!(old.lez_confirmations(), new.lez_confirmations());
        assert_eq!(old.zcash_confirmations(), new.zcash_confirmations());
        assert_eq!(old.lez_refund_delay(), new.lez_refund_delay());
        assert_eq!(old.zcash_refund_blocks(), new.zcash_refund_blocks());
        assert_eq!(old.required_margin(), new.required_margin());
        assert_eq!(old.expiry_delta_blocks(), new.expiry_delta_blocks());
        assert!(
            new.validate_consensus(NetworkType::Regtest, BranchId::Nu7)
                .is_ok()
        );
        assert_eq!(
            new.validate_consensus(NetworkType::Regtest, BranchId::Nu6_2),
            Err(ProfileError::ConsensusBranchMismatch)
        );
        assert_eq!(
            old.validate_consensus(NetworkType::Regtest, BranchId::Nu7),
            Err(ProfileError::ConsensusBranchMismatch)
        );
    }
    assert_eq!(
        ZecRefundProfile::for_id(ZecProfileId::DeterministicLocalDevnetV5)
            .lez_refund_delay()
            .value(),
        720
    );
    assert_eq!(
        ZecRefundProfile::for_id(ZecProfileId::DeterministicLocalDevnetV6)
            .lez_refund_delay()
            .value(),
        240
    );
}
