#![no_main]

use lez_swap_core::{
    Chain, ChainEventProof, ChainPosition, ChainProof, ClaimEvidence, ConfirmationPolicy, Pair,
    Participant, Phase, RecoverySchedule, SwapCoordinator, SwapDirection, SwapId, TimelockSafety,
};
use libfuzzer_sys::fuzz_target;

const PRIMARY_TAKER_LOCK: &str = "fuzz-taker-lock";
const ALTERNATE_TAKER_LOCK: &str = "fuzz-taker-lock-alternate";
const PRIMARY_MAKER_LOCK: &str = "fuzz-maker-lock";
const ALTERNATE_MAKER_LOCK: &str = "fuzz-maker-lock-alternate";
const PRIMARY_REVEAL: &str = "fuzz-revealing-claim";
const ALTERNATE_REVEAL: &str = "fuzz-revealing-claim-alternate";
const PRIMARY_FOLLOWUP: &str = "fuzz-followup-claim";
const ALTERNATE_FOLLOWUP: &str = "fuzz-followup-claim-alternate";
const PRIMARY_REFUND_EVENT: &str = "fuzz-taker-refund";
const ALTERNATE_REFUND_EVENT: &str = "fuzz-taker-refund-alternate";
const PRIMARY_RECOVERY: &str = "fuzz-maker-recovery";
const ALTERNATE_RECOVERY: &str = "fuzz-maker-recovery-alternate";

fuzz_target!(|data: &[u8]| {
    let Some((&profile, actions)) = data.split_first() else {
        return;
    };
    let (pair, direction) = profile_from_byte(profile);
    let mut swap = coordinator(pair, direction);
    let immutable_id = swap.id().as_str().to_owned();
    let immutable_schedule = swap.recovery_schedule();
    let immutable_chains = [
        swap.funded_chain(Participant::Maker),
        swap.funded_chain(Participant::Taker),
    ];
    let immutable_confirmations = [
        swap.required_confirmations(Participant::Maker),
        swap.required_confirmations(Participant::Taker),
    ];
    let mut terminal_snapshot = None;

    for byte in actions.iter().copied().take(511) {
        let before = swap.clone();
        let result = apply_action(&mut swap, byte);

        if result.is_err() {
            assert_eq!(swap, before, "rejected transition mutated durable state");
        }
        if let Some(expected) = terminal_snapshot.as_ref() {
            assert_eq!(
                &swap, expected,
                "absorbing terminal coordinator changed after later input"
            );
        }
        if terminal_snapshot.is_none() && matches!(swap.phase(), Phase::Completed | Phase::Refunded)
        {
            terminal_snapshot = Some(swap.clone());
        }
        if swap.phase() == Phase::Completed {
            assert!(
                swap.claim_evidence().is_some(),
                "completed coordinator lost public claim evidence"
            );
        }

        assert_eq!(swap.id().as_str(), immutable_id);
        assert_eq!(swap.pair(), pair);
        assert_eq!(swap.direction(), direction);
        assert_eq!(swap.recovery_schedule(), immutable_schedule);
        assert_eq!(
            [
                swap.funded_chain(Participant::Maker),
                swap.funded_chain(Participant::Taker),
            ],
            immutable_chains
        );
        assert_eq!(
            [
                swap.required_confirmations(Participant::Maker),
                swap.required_confirmations(Participant::Taker),
            ],
            immutable_confirmations
        );

        let encoded = serde_json::to_vec(&swap).expect("coordinator serialization must succeed");
        let restarted: SwapCoordinator = serde_json::from_slice(&encoded)
            .expect("coordinator serialization must round-trip after every action");
        assert_eq!(swap, restarted, "restart changed coordinator state");
        swap = restarted;
    }
});

fn profile_from_byte(byte: u8) -> (Pair, SwapDirection) {
    match byte {
        b'B' => (Pair::Bitcoin, SwapDirection::TakerSellsForeign),
        b'b' => (Pair::Bitcoin, SwapDirection::TakerSellsLez),
        b'M' | b'm' => (Pair::Monero, SwapDirection::TakerSellsLez),
        b'Z' => (Pair::Zcash, SwapDirection::TakerSellsForeign),
        b'z' => (Pair::Zcash, SwapDirection::TakerSellsLez),
        _ => match byte % 5 {
            0 => (Pair::Bitcoin, SwapDirection::TakerSellsForeign),
            1 => (Pair::Bitcoin, SwapDirection::TakerSellsLez),
            2 => (Pair::Monero, SwapDirection::TakerSellsLez),
            3 => (Pair::Zcash, SwapDirection::TakerSellsForeign),
            _ => (Pair::Zcash, SwapDirection::TakerSellsLez),
        },
    }
}

fn coordinator(pair: Pair, direction: SwapDirection) -> SwapCoordinator {
    let schedule = if pair == Pair::Monero {
        RecoverySchedule::xmr_lez_first(ChainPosition::block_height(Chain::Lez, 120), 2)
            .expect("fixed XMR fuzz schedule must be valid")
    } else {
        let foreign = Chain::from(pair);
        let maker_chain = match direction {
            SwapDirection::TakerSellsForeign => Chain::Lez,
            SwapDirection::TakerSellsLez => foreign,
        };
        let taker_chain = match direction {
            SwapDirection::TakerSellsForeign => foreign,
            SwapDirection::TakerSellsLez => Chain::Lez,
        };
        let safety_chains = match pair {
            Pair::Bitcoin => [maker_chain, taker_chain],
            Pair::Zcash => [Chain::Lez, Chain::Zcash],
            Pair::Monero => unreachable!("Monero uses event-gated recovery"),
        };
        RecoverySchedule::new(
            pair,
            direction,
            ChainPosition::block_height(maker_chain, 100),
            ChainPosition::block_height(taker_chain, 120),
            TimelockSafety::between(safety_chains[0], safety_chains[1], 1_000, 1_200, 100)
                .expect("fixed fuzz safety interval must be valid"),
        )
        .expect("fixed pair/direction fuzz schedule must be valid")
    };

    SwapCoordinator::new_with_confirmation_policies(
        SwapId::new("coordinator-fuzz").expect("fixed fuzz swap ID must be valid"),
        pair,
        direction,
        ConfirmationPolicy::new(2).expect("fixed confirmation policy must be valid"),
        ConfirmationPolicy::new(2).expect("fixed confirmation policy must be valid"),
        schedule,
    )
}

fn apply_action(swap: &mut SwapCoordinator, byte: u8) -> Result<(), lez_swap_core::Error> {
    let alternate = byte & 0x20 != 0;
    let confirmations = u32::from(byte & 0x03);
    let action = match byte {
        b'f' => 0,
        b'l' => 1,
        b'e' => 4,
        b'c' => 6,
        b'm' => 8,
        b't' => 9,
        b'u' => 10,
        b'x' => 11,
        b'r' => 2,
        b'R' => 3,
        b'w' => 5,
        _ => byte % 13,
    };

    match action {
        0 => swap.observe_taker_lock(
            ChainProof::new(
                select(PRIMARY_TAKER_LOCK, ALTERNATE_TAKER_LOCK, alternate),
                2,
            )
            .expect("fixed taker proof must be valid"),
        ),
        1 => swap.observe_maker_lock(
            ChainProof::new(
                select(PRIMARY_MAKER_LOCK, ALTERNATE_MAKER_LOCK, alternate),
                2,
            )
            .expect("fixed maker proof must be valid"),
        ),
        2 => swap.observe_taker_lock_removed(select(
            PRIMARY_TAKER_LOCK,
            ALTERNATE_TAKER_LOCK,
            alternate,
        )),
        3 => swap.observe_maker_lock_removed(select(
            PRIMARY_MAKER_LOCK,
            ALTERNATE_MAKER_LOCK,
            alternate,
        )),
        4 => swap.observe_revealing_claim(
            swap.first_claimant(),
            ChainProof::new(select(PRIMARY_REVEAL, ALTERNATE_REVEAL, alternate), 1)
                .expect("fixed revealing proof must be valid"),
            ClaimEvidence::new([byte; 32]),
        ),
        5 => swap.observe_revealing_claim(
            swap.first_claimant().other(),
            ChainProof::new(select(PRIMARY_REVEAL, ALTERNATE_REVEAL, alternate), 1)
                .expect("fixed wrong-claimant proof must be valid"),
            ClaimEvidence::new([byte; 32]),
        ),
        6 => swap.observe_followup_claim(
            swap.first_claimant().other(),
            ChainProof::new(select(PRIMARY_FOLLOWUP, ALTERNATE_FOLLOWUP, alternate), 1)
                .expect("fixed follow-up proof must be valid"),
        ),
        7 => swap.observe_followup_claim(
            swap.first_claimant(),
            ChainProof::new(select(PRIMARY_FOLLOWUP, ALTERNATE_FOLLOWUP, alternate), 1)
                .expect("fixed wrong follow-up proof must be valid"),
        ),
        8 => swap.refund_maker_leg(ChainPosition::block_height(
            swap.funded_chain(Participant::Maker),
            80 + u64::from(byte),
        )),
        9 => swap.refund_taker_leg(ChainPosition::block_height(
            swap.funded_chain(Participant::Taker),
            80 + u64::from(byte),
        )),
        10 => swap.observe_taker_refund_for_maker_recovery(
            ChainEventProof::new(
                if byte == b'u' || !alternate {
                    Chain::Lez
                } else {
                    Chain::Monero
                },
                select(PRIMARY_REFUND_EVENT, ALTERNATE_REFUND_EVENT, alternate),
                confirmations,
            )
            .expect("fixed recovery event must be valid"),
        ),
        11 => swap.observe_maker_recovery(
            ChainProof::new(
                select(PRIMARY_RECOVERY, ALTERNATE_RECOVERY, alternate),
                confirmations,
            )
            .expect("fixed recovery proof must be valid"),
        ),
        _ => swap.observe_taker_lock(
            ChainProof::new(
                select(PRIMARY_TAKER_LOCK, ALTERNATE_TAKER_LOCK, alternate),
                confirmations,
            )
            .expect("fixed varied taker proof must be valid"),
        ),
    }
}

const fn select(
    primary: &'static str,
    alternate: &'static str,
    use_alternate: bool,
) -> &'static str {
    if use_alternate { alternate } else { primary }
}
