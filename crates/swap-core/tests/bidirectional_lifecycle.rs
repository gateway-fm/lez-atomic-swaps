use lez_swap_core::{
    ChainProof, ClaimEvidence, ConfirmationPolicy, Error, Pair, Phase, SwapCoordinator,
    SwapDirection, SwapId, Timelocks,
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
        refund.refund_maker_leg(100).unwrap();
        assert_eq!(refund.phase(), Phase::MakerLegRefunded);
        refund.refund_taker_leg(120).unwrap();
        assert_eq!(refund.phase(), Phase::Refunded);
    }
}

fn coordinator(pair: Pair, id: &str) -> SwapCoordinator {
    SwapCoordinator::new_with_direction(
        SwapId::new(id).unwrap(),
        pair,
        SwapDirection::TakerSellsLez,
        ConfirmationPolicy::new(2).unwrap(),
        Timelocks::new(100, 120).unwrap(),
    )
}

#[test]
fn pre_direction_state_defaults_to_the_original_trade_direction() {
    let original = SwapCoordinator::new(
        SwapId::new("legacy-state").unwrap(),
        Pair::Bitcoin,
        ConfirmationPolicy::new(2).unwrap(),
        Timelocks::new(100, 120).unwrap(),
    );
    let mut encoded = serde_json::to_value(original).unwrap();
    encoded.as_object_mut().unwrap().remove("direction");
    let timelocks = encoded["timelocks"].as_object_mut().unwrap();
    let maker = timelocks.remove("maker_refund_at").unwrap();
    let taker = timelocks.remove("taker_refund_at").unwrap();
    timelocks.insert("lez_refund_at".into(), maker);
    timelocks.insert("foreign_refund_at".into(), taker);

    let recovered: SwapCoordinator = serde_json::from_value(encoded).unwrap();
    assert_eq!(recovered.direction(), SwapDirection::TakerSellsForeign);
}
