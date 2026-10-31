use lez_swap_core::{Chain, ChainPosition, LezUnixMilliseconds, SwapDirection, UnixSeconds};
use lez_zec_swap_sdk::{ProfileError, ZecProfileId, ZecRefundProfile};
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
fn both_directions_keep_lez_before_zcash_while_mapping_role_deadlines() {
    let local = ZecRefundProfile::for_id(ZecProfileId::DeterministicLocalV1);
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        let schedule = local
            .recovery_schedule(
                direction,
                UnixSeconds::new(60),
                BlockHeight::from_u32(4),
                LezUnixMilliseconds::new(13_000),
                Some(UnixSeconds::new(43)),
            )
            .unwrap();
        let lez = ChainPosition::timestamp(Chain::Lez, 60);
        let zec = ChainPosition::block_height(Chain::Zcash, 4);
        match direction {
            SwapDirection::TakerSellsForeign => {
                assert!(schedule.maker_deadline_reached(lez).unwrap());
                assert!(schedule.taker_refund_reached(zec).unwrap());
            }
            SwapDirection::TakerSellsLez => {
                assert!(schedule.maker_deadline_reached(zec).unwrap());
                assert!(schedule.taker_refund_reached(lez).unwrap());
            }
        }
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
