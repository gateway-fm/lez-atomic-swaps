use lez_swap_core::{
    Chain, ChainEventProof, ChainPosition, ChainProof, ClaimEvidence, ConfirmationPolicy, Pair,
    Phase, RecoverySchedule, SwapCoordinator, SwapDirection, SwapId, TimelockSafety,
};
use lez_swap_store::SqliteSwapStore;
use tempfile::tempdir;

fn swap(id: &str, pair: Pair) -> SwapCoordinator {
    let direction = if pair == Pair::Monero {
        SwapDirection::TakerSellsLez
    } else {
        SwapDirection::TakerSellsForeign
    };
    let foreign = Chain::from(pair);
    let role_chains = match direction {
        SwapDirection::TakerSellsForeign => [Chain::Lez, foreign],
        SwapDirection::TakerSellsLez => [foreign, Chain::Lez],
    };
    let schedule = if pair == Pair::Monero {
        RecoverySchedule::xmr_lez_first(ChainPosition::block_height(Chain::Lez, 120), 2).unwrap()
    } else {
        RecoverySchedule::new(
            pair,
            direction,
            ChainPosition::block_height(role_chains[0], 100),
            ChainPosition::block_height(role_chains[1], 120),
            TimelockSafety::new(1_000, 1_200, 100).unwrap(),
        )
        .unwrap()
    };
    SwapCoordinator::new_with_direction(
        SwapId::new(id).unwrap(),
        pair,
        direction,
        ConfirmationPolicy::new(2).unwrap(),
        schedule,
    )
}

#[test]
fn xmr_event_gated_recovery_survives_each_restart() {
    let data_dir = tempdir().unwrap();
    let database = data_dir.path().join("xmr-recovery.sqlite3");
    let mut current = swap("xmr-restart-recovery", Pair::Monero);
    current
        .observe_taker_lock(ChainProof::new("lez-lock", 2).unwrap())
        .unwrap();
    current
        .observe_maker_lock(ChainProof::new("xmr-lock", 10).unwrap())
        .unwrap();
    current
        .refund_taker_leg(ChainPosition::block_height(Chain::Lez, 120))
        .unwrap();
    save_and_reload(&database, &mut current);
    assert_eq!(current.phase(), Phase::TakerLegRefunded);

    current
        .observe_taker_refund_for_maker_recovery(
            ChainEventProof::new(Chain::Lez, "lez-refund", 2).unwrap(),
        )
        .unwrap();
    save_and_reload(&database, &mut current);
    assert_eq!(current.phase(), Phase::MakerRecoveryAvailable);

    current
        .observe_maker_recovery(ChainProof::new("xmr-recovery", 10).unwrap())
        .unwrap();
    save_and_reload(&database, &mut current);
    assert_eq!(current.phase(), Phase::Refunded);
}

fn save_and_reload(database: &std::path::Path, swap: &mut SwapCoordinator) {
    SqliteSwapStore::open(database).unwrap().save(swap).unwrap();
    *swap = SqliteSwapStore::open(database)
        .unwrap()
        .load(swap.id())
        .unwrap()
        .expect("swap survives restart");
}

#[test]
fn maker_daemon_restarts_after_each_durable_step_and_taker_can_complete() {
    let data_dir = tempdir().unwrap();
    let database = data_dir.path().join("maker.sqlite3");
    let mut expected = swap("restart-journey", Pair::Bitcoin);

    {
        let store = SqliteSwapStore::open(&database).unwrap();
        store.save(&expected).unwrap();
    }

    let mut recovered = SqliteSwapStore::open(&database)
        .unwrap()
        .load(expected.id())
        .unwrap()
        .expect("offered swap survives restart");
    assert_eq!(recovered, expected);

    recovered
        .observe_taker_foreign_lock(ChainProof::new("btc-lock", 2).unwrap())
        .unwrap();
    expected = recovered.clone();
    {
        let store = SqliteSwapStore::open(&database).unwrap();
        store.save(&expected).unwrap();
    }

    let mut recovered = SqliteSwapStore::open(&database)
        .unwrap()
        .load(expected.id())
        .unwrap()
        .expect("confirmed taker lock survives restart");
    assert_eq!(recovered, expected);
    recovered
        .observe_maker_lez_lock(ChainProof::new("lez-lock", 1).unwrap())
        .unwrap();

    {
        let store = SqliteSwapStore::open(&database).unwrap();
        store.save(&recovered).unwrap();
    }
    let mut recovered = SqliteSwapStore::open(&database)
        .unwrap()
        .load(expected.id())
        .unwrap()
        .expect("both locks survive restart");

    let claim_evidence = ClaimEvidence::new([9; 32]);
    recovered
        .observe_maker_claim(claim_evidence.clone())
        .unwrap();
    {
        let store = SqliteSwapStore::open(&database).unwrap();
        store.save(&recovered).unwrap();
    }

    let mut recovered = SqliteSwapStore::open(&database)
        .unwrap()
        .load(expected.id())
        .unwrap()
        .expect("claim witness survives restart");
    assert_eq!(recovered.claim_evidence(), Some(&claim_evidence));
    recovered
        .observe_taker_lez_claim(ChainProof::new("lez-claim", 1).unwrap())
        .unwrap();
    assert_eq!(recovered.phase(), Phase::Completed);
}

#[test]
fn concurrent_maker_users_are_persisted_as_isolated_swaps() {
    let data_dir = tempdir().unwrap();
    let store = SqliteSwapStore::open(data_dir.path().join("maker.sqlite3")).unwrap();
    let mut bitcoin = swap("alice-btc", Pair::Bitcoin);
    let monero = swap("bob-xmr", Pair::Monero);

    bitcoin
        .observe_taker_foreign_lock(ChainProof::new("alice-lock", 2).unwrap())
        .unwrap();
    store.save(&bitcoin).unwrap();
    store.save(&monero).unwrap();

    assert_eq!(
        store.load(bitcoin.id()).unwrap().unwrap().phase(),
        Phase::TakerLockConfirmed
    );
    assert_eq!(
        store.load(monero.id()).unwrap().unwrap().phase(),
        Phase::Offered
    );
}
