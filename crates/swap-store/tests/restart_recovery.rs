use lez_swap_core::{
    Chain, ChainPosition, ChainProof, ClaimEvidence, ConfirmationPolicy, Pair, Phase,
    RefundSchedule, SwapCoordinator, SwapDirection, SwapId, TimelockSafety,
};
use lez_swap_store::SqliteSwapStore;
use tempfile::tempdir;

fn swap(id: &str, pair: Pair) -> SwapCoordinator {
    SwapCoordinator::new(
        SwapId::new(id).unwrap(),
        pair,
        ConfirmationPolicy::new(2).unwrap(),
        RefundSchedule::new(
            pair,
            SwapDirection::TakerSellsForeign,
            ChainPosition::block_height(Chain::Lez, 100),
            ChainPosition::block_height(Chain::from(pair), 120),
            TimelockSafety::new(1_000, 1_200, 100).unwrap(),
        )
        .unwrap(),
    )
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
