use lez_swap_core::{
    Chain, ChainEventProof, ChainPosition, ChainProof, ClaimEvidence, ConfirmationPolicy, Error,
    Pair, Phase, RecoverySchedule, SwapCoordinator, SwapDirection, SwapId, TimelockSafety,
};

#[test]
fn taker_selling_lez_preserves_taker_first_claim_and_refund_safety() {
    for pair in [Pair::Bitcoin, Pair::Monero, Pair::Zcash] {
        let mut happy = coordinator(pair, "reverse-happy");
        assert_eq!(happy.direction(), SwapDirection::TakerSellsLez);
        assert_eq!(
            happy.observe_maker_lock(ChainProof::new("foreign-maker-lock", 1).unwrap()),
            Err(Error::TakerLockNotConfirmed)
        );
        happy
            .observe_taker_lock(ChainProof::new("lez-taker-lock", 2).unwrap())
            .unwrap();
        happy
            .observe_maker_lock(ChainProof::new("foreign-maker-lock", 1).unwrap())
            .unwrap();
        happy
            .observe_maker_claim(ClaimEvidence::new([31; 32]))
            .unwrap();
        happy
            .observe_taker_claim(ChainProof::new("foreign-taker-claim", 1).unwrap())
            .unwrap();
        assert_eq!(happy.phase(), Phase::Completed);

        let mut refund = coordinator(pair, "reverse-refund");
        refund
            .observe_taker_lock(ChainProof::new("lez-taker-lock", 2).unwrap())
            .unwrap();
        refund
            .observe_maker_lock(ChainProof::new("foreign-maker-lock", 1).unwrap())
            .unwrap();
        if pair == Pair::Monero {
            refund
                .refund_taker_leg(ChainPosition::block_height(Chain::Lez, 120))
                .unwrap();
            refund
                .observe_taker_refund_for_maker_recovery(
                    ChainEventProof::new(Chain::Lez, "lez-refund", 2).unwrap(),
                )
                .unwrap();
            refund
                .observe_maker_recovery(ChainProof::new("xmr-recovery", 10).unwrap())
                .unwrap();
        } else {
            refund
                .refund_maker_leg(ChainPosition::block_height(Chain::from(pair), 100))
                .unwrap();
            assert_eq!(refund.phase(), Phase::MakerLegRefunded);
            refund
                .refund_taker_leg(ChainPosition::block_height(Chain::Lez, 120))
                .unwrap();
        }
        assert_eq!(refund.phase(), Phase::Refunded);
    }
}

fn coordinator(pair: Pair, id: &str) -> SwapCoordinator {
    let schedule = if pair == Pair::Monero {
        RecoverySchedule::xmr_lez_first(ChainPosition::block_height(Chain::Lez, 120), 2).unwrap()
    } else {
        RecoverySchedule::new(
            pair,
            SwapDirection::TakerSellsLez,
            ChainPosition::block_height(Chain::from(pair), 100),
            ChainPosition::block_height(Chain::Lez, 120),
            TimelockSafety::new(1_000, 1_200, 100).unwrap(),
        )
        .unwrap()
    };
    SwapCoordinator::new_with_direction(
        SwapId::new(id).unwrap(),
        pair,
        SwapDirection::TakerSellsLez,
        ConfirmationPolicy::new(2).unwrap(),
        schedule,
    )
}

#[test]
fn pre_direction_state_defaults_to_the_original_trade_direction() {
    let original = SwapCoordinator::new(
        SwapId::new("legacy-state").unwrap(),
        Pair::Bitcoin,
        ConfirmationPolicy::new(2).unwrap(),
        RecoverySchedule::new(
            Pair::Bitcoin,
            SwapDirection::TakerSellsForeign,
            ChainPosition::block_height(Chain::Lez, 100),
            ChainPosition::block_height(Chain::Bitcoin, 120),
            TimelockSafety::new(1_000, 1_200, 100).unwrap(),
        )
        .unwrap(),
    );
    let mut encoded = serde_json::to_value(original).unwrap();
    let object = encoded.as_object_mut().unwrap();
    object.remove("direction");
    let schedule = object.remove("recovery_schedule").unwrap();
    object.insert("refund_schedule".to_owned(), schedule);
    let recovered: SwapCoordinator = serde_json::from_value(encoded).unwrap();
    assert_eq!(recovered.direction(), SwapDirection::TakerSellsForeign);
}
