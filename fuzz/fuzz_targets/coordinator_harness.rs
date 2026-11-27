use lez_swap_core::SwapCoordinator;

use super::coordinator_model::{Invariants, apply_action, coordinator, profile_from_byte};

/// Replays one fuzz input: the first byte picks the profile, each later byte one action.
pub fn run(data: &[u8]) {
    let Some((&profile, actions)) = data.split_first() else {
        return;
    };
    let (pair, direction) = profile_from_byte(profile);
    let mut swap = coordinator("coordinator-fuzz", pair, direction);
    let mut invariants = Invariants::new(&swap);

    for byte in actions.iter().copied().take(511) {
        let before = swap.clone();
        let result = apply_action(&mut swap, byte);
        invariants.check_step(&before, result.is_err(), &swap);

        let encoded = serde_json::to_vec(&swap).expect("coordinator serialization must succeed");
        let restarted: SwapCoordinator = serde_json::from_slice(&encoded)
            .expect("coordinator serialization must round-trip after every action");
        assert_eq!(swap, restarted, "restart changed coordinator state");
        swap = restarted;
    }
}
