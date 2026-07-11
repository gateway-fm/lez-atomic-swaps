use lez_swap_core::{
    ChainProof, ClaimEvidence, ConfirmationPolicy, Pair, Phase, SwapCoordinator, SwapId, Timelocks,
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
        Timelocks::new(100, 120).unwrap(),
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
                } => swap.observe_taker_foreign_lock(
                    ChainProof::new(
                        transaction_id("foreign-lock", "other-foreign-lock", alternate),
                        confirmations,
                    )
                    .unwrap(),
                ),
                Action::RemoveTakerLock { alternate } => swap.observe_taker_lock_removed(
                    transaction_id("foreign-lock", "other-foreign-lock", alternate),
                ),
                Action::MakerLock { alternate } => swap.observe_maker_lez_lock(
                    ChainProof::new(
                        transaction_id("lez-lock", "other-lez-lock", alternate),
                        1,
                    )
                    .unwrap(),
                ),
                Action::MakerClaim { secret_byte } => {
                    swap.observe_maker_claim(ClaimEvidence::new([secret_byte; 32]))
                }
                Action::TakerClaim { alternate } => swap.observe_taker_lez_claim(
                    ChainProof::new(
                        transaction_id("lez-claim", "other-lez-claim", alternate),
                        1,
                    )
                    .unwrap(),
                ),
                Action::RefundMaker { now } => swap.refund_maker_lez_leg(now),
                Action::RefundTaker { now } => swap.refund_taker_foreign_leg(now),
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
                    Phase::BothLegsLocked => {
                        prop_assert!(matches!(before, Phase::TakerLockConfirmed | Phase::TakerLockReorged));
                    }
                    Phase::TakerLockReorged => {
                        prop_assert!(matches!(before, Phase::BothLegsLocked | Phase::ClaimEvidenceAvailable));
                    }
                    Phase::ClaimEvidenceAvailable => {
                        prop_assert!(matches!(before, Phase::BothLegsLocked | Phase::TakerLockReorged));
                    }
                    Phase::Completed => {
                        prop_assert_eq!(before, Phase::ClaimEvidenceAvailable);
                        prop_assert!(swap.claim_evidence().is_some());
                    }
                    Phase::MakerLegRefunded => {
                        prop_assert!(matches!(before, Phase::BothLegsLocked | Phase::TakerLockReorged));
                    }
                    Phase::TakerLegRefunded => {
                        prop_assert!(matches!(before, Phase::BothLegsLocked | Phase::TakerLockReorged));
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
