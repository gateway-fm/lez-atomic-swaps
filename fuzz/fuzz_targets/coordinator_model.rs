//! The action alphabet and the invariants of the coordinator fuzz targets, shared by the
//! in-memory harness (`coordinator_harness.rs`) and the persisted one
//! (`coordinator_store_harness.rs`).

use lez_swap_core::{
    Chain, ChainEventProof, ChainPosition, ChainProof, ClaimEvidence, ConfirmationPolicy, Pair,
    Participant, Phase, RecoverySchedule, SwapCoordinator, SwapDirection, SwapId, TimelockSafety,
};

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

/// Maps one profile byte to the pair and direction of a swap; the letters are for seeds.
pub fn profile_from_byte(byte: u8) -> (Pair, SwapDirection) {
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

/// A fresh coordinator with the fixed fuzz schedule and confirmation policies.
pub fn coordinator(id: &str, pair: Pair, direction: SwapDirection) -> SwapCoordinator {
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
            Pair::Bitcoin | Pair::Zcash => [maker_chain, taker_chain],
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
        SwapId::new(id).expect("fixed fuzz swap ID must be valid"),
        pair,
        direction,
        ConfirmationPolicy::new(2).expect("fixed confirmation policy must be valid"),
        ConfirmationPolicy::new(2).expect("fixed confirmation policy must be valid"),
        schedule,
    )
}

/// The coordinator properties that hold after every action, whatever the action was.
#[derive(Debug)]
pub struct Invariants {
    id: String,
    pair: Pair,
    direction: SwapDirection,
    schedule: RecoverySchedule,
    chains: [Chain; 2],
    confirmations: [u32; 2],
    terminal_snapshot: Option<SwapCoordinator>,
}

impl Invariants {
    /// Records the immutable terms of `swap`, and its state when it is already terminal.
    pub fn new(swap: &SwapCoordinator) -> Self {
        Self {
            id: swap.id().as_str().to_owned(),
            pair: swap.pair(),
            direction: swap.direction(),
            schedule: swap.recovery_schedule(),
            chains: funded_chains(swap),
            confirmations: required_confirmations(swap),
            terminal_snapshot: is_terminal(swap).then(|| swap.clone()),
        }
    }

    /// Checks one action that took the coordinator from `before` to `swap`.
    pub fn check_step(&mut self, before: &SwapCoordinator, rejected: bool, swap: &SwapCoordinator) {
        if rejected {
            assert_eq!(swap, before, "rejected transition mutated durable state");
        }
        if let Some(expected) = self.terminal_snapshot.as_ref() {
            assert_eq!(
                swap, expected,
                "absorbing terminal coordinator changed after later input"
            );
        }
        if self.terminal_snapshot.is_none() && is_terminal(swap) {
            self.terminal_snapshot = Some(swap.clone());
        }
        if swap.phase() == Phase::Completed {
            assert!(
                swap.claim_evidence().is_some(),
                "completed coordinator lost public claim evidence"
            );
        }
        // No reachable state is refunded while claim evidence stands: the model has no
        // observation of the revealing claim leaving the chain (issue #103 item 16).
        if matches!(
            swap.phase(),
            Phase::Refunded | Phase::MakerLegRefunded | Phase::TakerLegRefunded
        ) {
            assert!(
                swap.claim_evidence().is_none(),
                "refunded coordinator holds revealing claim evidence"
            );
        }

        assert_eq!(swap.id().as_str(), self.id);
        assert_eq!(swap.pair(), self.pair);
        assert_eq!(swap.direction(), self.direction);
        assert_eq!(swap.recovery_schedule(), self.schedule);
        assert_eq!(funded_chains(swap), self.chains);
        assert_eq!(required_confirmations(swap), self.confirmations);
    }
}

fn is_terminal(swap: &SwapCoordinator) -> bool {
    matches!(swap.phase(), Phase::Completed | Phase::Refunded)
}

fn funded_chains(swap: &SwapCoordinator) -> [Chain; 2] {
    [
        swap.funded_chain(Participant::Maker),
        swap.funded_chain(Participant::Taker),
    ]
}

fn required_confirmations(swap: &SwapCoordinator) -> [u32; 2] {
    [
        swap.required_confirmations(Participant::Maker),
        swap.required_confirmations(Participant::Taker),
    ]
}

/// Applies the observation one action byte selects; the letters are for seeds.
pub fn apply_action(swap: &mut SwapCoordinator, byte: u8) -> Result<(), lez_swap_core::Error> {
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
