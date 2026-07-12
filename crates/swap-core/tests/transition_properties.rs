use lez_swap_core::{
    Chain, ChainPosition, ChainProof, ClaimEvidence, ConfirmationPolicy, Pair, Phase,
    RecoverySchedule, SwapCoordinator, SwapDirection, SwapId, TimelockSafety,
};
use proptest::prelude::*;

#[derive(Clone, Debug)]
enum Action {
    TakerLock { confirmations: u32, alternate: bool },
    RemoveTakerLock { alternate: bool },
    MakerLock { alternate: bool },
    MakerClaim { secret_byte: u8 },
    TakerClaim { alternate: bool },
    RefundMaker { now: u64 },
    RefundTaker { now: u64 },
}

fn actions() -> impl Strategy<Value = Vec<Action>> {
    prop::collection::vec(
        prop_oneof![
            (0_u32..5, any::<bool>()).prop_map(|(confirmations, alternate)| Action::TakerLock {
                confirmations,
                alternate,
            }),
            any::<bool>().prop_map(|alternate| Action::RemoveTakerLock { alternate }),
            any::<bool>().prop_map(|alternate| Action::MakerLock { alternate }),
            any::<u8>().prop_map(|secret_byte| Action::MakerClaim { secret_byte }),
            any::<bool>().prop_map(|alternate| Action::TakerClaim { alternate }),
            (0_u64..150).prop_map(|now| Action::RefundMaker { now }),
            (0_u64..150).prop_map(|now| Action::RefundTaker { now }),
        ],
        0..64,
    )
}

fn coordinator() -> SwapCoordinator {
    SwapCoordinator::new(
        SwapId::new("property-swap").unwrap(),
        Pair::Bitcoin,
        ConfirmationPolicy::new(2).unwrap(),
        RecoverySchedule::new(
            Pair::Bitcoin,
            SwapDirection::TakerSellsForeign,
            ChainPosition::block_height(Chain::Lez, 100),
            ChainPosition::block_height(Chain::Bitcoin, 120),
            TimelockSafety::between(Chain::Lez, Chain::Bitcoin, 1_000, 1_200, 100).unwrap(),
        )
        .unwrap(),
    )
}

fn transaction_id(
    primary: &'static str,
    alternate: &'static str,
    use_alternate: bool,
) -> &'static str {
    if use_alternate { alternate } else { primary }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn arbitrary_event_sequences_preserve_atomicity_and_absorbing_terminal_states(
        actions in actions()
    ) {
        let mut swap = coordinator();
        let mut terminal = None;

        for action in actions {
            let before = swap.phase();
            let result = match action {
                Action::TakerLock {
                    confirmations,
                    alternate,
                } => swap.observe_taker_lock(
                    ChainProof::new(
                        transaction_id("foreign-lock", "other-foreign-lock", alternate),
                        confirmations,
                    )
                    .unwrap(),
                ),
                Action::RemoveTakerLock { alternate } => swap.observe_taker_lock_removed(
                    transaction_id("foreign-lock", "other-foreign-lock", alternate),
                ),
                Action::MakerLock { alternate } => swap.observe_maker_lock(
                    ChainProof::new(
                        transaction_id("lez-lock", "other-lez-lock", alternate),
                        1,
                    )
                    .unwrap(),
                ),
                Action::MakerClaim { secret_byte } => {
                    swap.observe_revealing_claim(
                        swap.first_claimant(),
                        ChainProof::new("revealing-claim", 1).unwrap(),
                        ClaimEvidence::new([secret_byte; 32]),
                    )
                }
                Action::TakerClaim { alternate } => swap.observe_followup_claim(
                    swap.first_claimant().other(),
                    ChainProof::new(
                        transaction_id("followup-claim", "other-followup-claim", alternate),
                        1,
                    )
                    .unwrap(),
                ),
                Action::RefundMaker { now } => {
                    swap.refund_maker_leg(ChainPosition::block_height(Chain::Lez, now))
                }
                Action::RefundTaker { now } => {
                    swap.refund_taker_leg(ChainPosition::block_height(Chain::Bitcoin, now))
                }
            };
            let after = swap.phase();

            if let Some(expected_terminal) = terminal {
                prop_assert_eq!(after, expected_terminal);
            }
            if matches!(after, Phase::Completed | Phase::Refunded) {
                terminal = Some(after);
            }

            if result.is_ok() && before != after {
                match after {
                    Phase::AwaitingTakerConfirmations => {
                        prop_assert!(matches!(before, Phase::Offered | Phase::TakerLockConfirmed));
                    }
                    Phase::TakerLockConfirmed => {
                        prop_assert!(matches!(
                            before,
                            Phase::Offered | Phase::AwaitingTakerConfirmations
                        ));
                    }
                    Phase::AwaitingMakerConfirmations => {
                        prop_assert!(matches!(
                            before,
                            Phase::TakerLockConfirmed | Phase::AwaitingMakerConfirmations
                        ));
                    }
                    Phase::BothLegsLocked => {
                        prop_assert!(matches!(
                            before,
                            Phase::TakerLockConfirmed
                                | Phase::TakerLockReorged
                                | Phase::MakerLockReorged
                        ));
                    }
                    Phase::TakerLockReorged => {
                        prop_assert!(matches!(before, Phase::BothLegsLocked | Phase::ClaimEvidenceAvailable));
                    }
                    Phase::MakerLockReorged => {
                        prop_assert!(matches!(before, Phase::BothLegsLocked | Phase::ClaimEvidenceAvailable));
                    }
                    Phase::ClaimEvidenceAvailable => {
                        prop_assert!(matches!(
                            before,
                            Phase::BothLegsLocked
                                | Phase::TakerLockReorged
                                | Phase::MakerLockReorged
                        ));
                    }
                    Phase::Completed => {
                        prop_assert_eq!(before, Phase::ClaimEvidenceAvailable);
                        prop_assert!(swap.claim_evidence().is_some());
                    }
                    Phase::MakerLegRefunded => {
                        prop_assert!(matches!(
                            before,
                            Phase::BothLegsLocked
                                | Phase::TakerLockReorged
                                | Phase::MakerLockReorged
                        ));
                    }
                    Phase::TakerLegRefunded => {
                        prop_assert!(matches!(
                            before,
                            Phase::BothLegsLocked
                                | Phase::TakerLockReorged
                                | Phase::MakerLockReorged
                        ));
                    }
                    Phase::MakerRecoveryAvailable => {
                        prop_assert!(false, "generated BTC model has no event-gated recovery action");
                    }
                    Phase::Refunded => {
                        prop_assert!(matches!(
                            before,
                            Phase::AwaitingTakerConfirmations
                                | Phase::TakerLockConfirmed
                                | Phase::MakerLegRefunded
                                | Phase::TakerLegRefunded
                        ));
                    }
                    Phase::Offered => prop_assert!(matches!(
                        before,
                        Phase::AwaitingTakerConfirmations | Phase::TakerLockConfirmed
                    )),
                }
            }

            prop_assert!(!(after == Phase::Completed && swap.claim_evidence().is_none()));
        }
    }
}
