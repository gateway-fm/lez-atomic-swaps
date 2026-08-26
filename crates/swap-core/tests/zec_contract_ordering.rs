use lez_swap_core::{
    Chain, ChainPosition, ChainProof, ClaimEvidence, ConfirmationPolicy, Error, Pair, Participant,
    Phase, RecoverySchedule, SwapCoordinator, SwapDirection, SwapId, TimelockSafety,
};

#[test]
fn zec_refund_always_outlives_lez_and_lez_claim_always_reveals_first() {
    for (direction, expected_first_claimant) in [
        (SwapDirection::TakerSellsForeign, Participant::Taker),
        (SwapDirection::TakerSellsLez, Participant::Maker),
    ] {
        let (maker_refund, taker_refund) = match direction {
            SwapDirection::TakerSellsForeign => (
                ChainPosition::timestamp(Chain::Lez, 100),
                ChainPosition::block_height(Chain::Zcash, 120),
            ),
            SwapDirection::TakerSellsLez => (
                ChainPosition::block_height(Chain::Zcash, 120),
                ChainPosition::timestamp(Chain::Lez, 100),
            ),
        };
        let schedule = RecoverySchedule::new(
            Pair::Zcash,
            direction,
            maker_refund,
            taker_refund,
            TimelockSafety::between(Chain::Lez, Chain::Zcash, 1_000, 1_400, 300).unwrap(),
        )
        .unwrap();
        let mut swap = SwapCoordinator::new_with_direction(
            SwapId::new(format!("zec-{direction:?}")).unwrap(),
            Pair::Zcash,
            direction,
            ConfirmationPolicy::new(1).unwrap(),
            schedule,
        );
        swap.observe_taker_lock(ChainProof::new("taker-lock", 1).unwrap())
            .unwrap();
        swap.observe_maker_lock(ChainProof::new("maker-lock", 1).unwrap())
            .unwrap();

        let wrong_first_claimant = expected_first_claimant.other();
        assert_eq!(
            swap.observe_revealing_claim(
                wrong_first_claimant,
                ChainProof::new("wrong-chain-claim", 1).unwrap(),
                ClaimEvidence::new([42; 32]),
            ),
            Err(Error::UnexpectedClaimant {
                expected: expected_first_claimant,
                actual: wrong_first_claimant,
            })
        );
        swap.observe_revealing_claim(
            expected_first_claimant,
            ChainProof::new("lez-claim", 1).unwrap(),
            ClaimEvidence::new([42; 32]),
        )
        .unwrap();
        assert_eq!(swap.phase(), Phase::ClaimEvidenceAvailable);

        assert_eq!(
            swap.observe_followup_claim(
                expected_first_claimant,
                ChainProof::new("wrong-followup", 1).unwrap(),
            ),
            Err(Error::UnexpectedClaimant {
                expected: wrong_first_claimant,
                actual: expected_first_claimant,
            })
        );
        swap.observe_followup_claim(
            wrong_first_claimant,
            ChainProof::new("zec-claim", 1).unwrap(),
        )
        .unwrap();
        assert_eq!(swap.phase(), Phase::Completed);
    }
}

#[test]
fn zec_schedule_rejects_role_valid_but_contract_reversed_chain_order() {
    let result = RecoverySchedule::new(
        Pair::Zcash,
        SwapDirection::TakerSellsLez,
        ChainPosition::block_height(Chain::Zcash, 100),
        ChainPosition::timestamp(Chain::Lez, 120),
        TimelockSafety::between(Chain::Zcash, Chain::Lez, 1_000, 1_400, 300).unwrap(),
    );

    assert_eq!(
        result,
        Err(Error::WrongTimelockOrder {
            pair: Pair::Zcash,
            earlier: Chain::Lez,
            later: Chain::Zcash,
        })
    );
}
