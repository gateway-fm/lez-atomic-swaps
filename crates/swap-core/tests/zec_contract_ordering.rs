use lez_swap_core::{
    Chain, ChainPosition, ChainProof, ClaimEvidence, ConfirmationPolicy, Error, Pair, Participant,
    Phase, RecoverySchedule, SwapCoordinator, SwapDirection, SwapId, TimelockSafety,
};

#[test]
fn zec_taker_always_reveals_first_on_the_maker_funded_leg_which_refunds_first() {
    // The common flow (AGENTS.md): in both directions the Taker holds the preimage and
    // claims the Maker-funded leg first; that leg's refund opens first.
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        let expected_first_claimant = Participant::Taker;
        let (maker_refund, taker_refund, safety) = match direction {
            SwapDirection::TakerSellsForeign => (
                ChainPosition::timestamp(Chain::Lez, 100),
                ChainPosition::block_height(Chain::Zcash, 120),
                TimelockSafety::between(Chain::Lez, Chain::Zcash, 1_000, 1_400, 300).unwrap(),
            ),
            SwapDirection::TakerSellsLez => (
                ChainPosition::block_height(Chain::Zcash, 120),
                ChainPosition::timestamp(Chain::Lez, 1_400),
                TimelockSafety::between(Chain::Zcash, Chain::Lez, 1_000, 1_400, 300).unwrap(),
            ),
        };
        let schedule =
            RecoverySchedule::new(Pair::Zcash, direction, maker_refund, taker_refund, safety)
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
            ChainProof::new("maker-funded-leg-claim", 1).unwrap(),
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
            ChainProof::new("taker-funded-leg-claim", 1).unwrap(),
        )
        .unwrap();
        assert_eq!(swap.phase(), Phase::Completed);
    }
}

#[test]
fn zec_schedule_rejects_a_taker_funded_leg_that_refunds_first() {
    // TakerSellsLez: main's order (LEZ refund first, Zcash later) let the preimage-holding
    // Maker take the LEZ without funding Zcash. It is refused now.
    let reverse = RecoverySchedule::new(
        Pair::Zcash,
        SwapDirection::TakerSellsLez,
        ChainPosition::block_height(Chain::Zcash, 120),
        ChainPosition::timestamp(Chain::Lez, 100),
        TimelockSafety::between(Chain::Lez, Chain::Zcash, 1_000, 1_400, 300).unwrap(),
    );
    assert_eq!(
        reverse,
        Err(Error::WrongTimelockOrder {
            pair: Pair::Zcash,
            earlier: Chain::Zcash,
            later: Chain::Lez,
        })
    );
    let forward = RecoverySchedule::new(
        Pair::Zcash,
        SwapDirection::TakerSellsForeign,
        ChainPosition::timestamp(Chain::Lez, 100),
        ChainPosition::block_height(Chain::Zcash, 120),
        TimelockSafety::between(Chain::Zcash, Chain::Lez, 1_000, 1_400, 300).unwrap(),
    );
    assert_eq!(
        forward,
        Err(Error::WrongTimelockOrder {
            pair: Pair::Zcash,
            earlier: Chain::Lez,
            later: Chain::Zcash,
        })
    );
}
