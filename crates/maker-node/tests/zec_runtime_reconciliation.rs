use lez_maker_node::{
    ZcashFundingProjectionOutcome, apply_zcash_funding_event, load_zcash_observation_tracker,
};
use lez_swap_core::{
    Chain, ChainPosition, ChainProof, ClaimEvidence, ConfirmationPolicy, Pair, Participant, Phase,
    RecoverySchedule, SwapCoordinator, SwapDirection, SwapId, TimelockSafety,
};
use lez_swap_store::SqliteSwapStore;
use lez_zec_swap_sdk::{
    Bip199Contract, CanonicalZcashOutputObservation, CanonicalZcashOutputRemoval,
    ExpectedBip199Output, TransparentFundingRequest, TransparentUtxo, ZcashNodeRemovalSnapshot,
    ZcashNodeSnapshot, ZcashObservationEvent, ZcashObservationReconciliation, ZcashStableTip,
    build_funding_transaction,
};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use tempfile::tempdir;
use zcash_primitives::block::BlockHash;
use zcash_protocol::{
    consensus::{BlockHeight, BranchId, NetworkType},
    value::Zatoshis,
};
use zcash_transparent::{
    address::{Script, TransparentAddress},
    bundle::{OutPoint, TxOut},
};

fn zatoshis(value: u64) -> Zatoshis {
    Zatoshis::from_u64(value).unwrap()
}

fn canonical_observation() -> CanonicalZcashOutputObservation {
    let key = SecretKey::from_slice(&[7; 32]).unwrap();
    let public_key = PublicKey::from_secret_key(&Secp256k1::new(), &key);
    let owner_script: Script = TransparentAddress::from_pubkey(&public_key).script().into();
    let contract = Bip199Contract::new(500_000, [0x11; 20], [0x22; 32], [0x33; 20]);
    let request = TransparentFundingRequest::new(
        vec![TransparentUtxo::new(
            OutPoint::new([9; 32], 0),
            TxOut::new(zatoshis(120_000), owner_script),
        )],
        public_key,
        zatoshis(100_000),
        zatoshis(10_000),
        zatoshis(1_000),
        BlockHeight::from_u32(4_100_000),
        BranchId::Nu6_2,
    )
    .unwrap();
    let transaction = build_funding_transaction(&contract, &request, &key).unwrap();
    let mut raw = vec![];
    transaction.write(&mut raw).unwrap();
    CanonicalZcashOutputObservation::validate(
        &ExpectedBip199Output::new(
            NetworkType::Regtest,
            BranchId::Nu6_2,
            zatoshis(100_000),
            contract,
        ),
        &ZcashNodeSnapshot::new(
            NetworkType::Regtest,
            BranchId::Nu6_2,
            true,
            BlockHash([0x44; 32]),
            BlockHash([0x44; 32]),
            BlockHeight::from_u32(100),
            ZcashStableTip::new(
                BlockHash([0xaa; 32]),
                BlockHeight::from_u32(102),
                BlockHash([0xaa; 32]),
                BlockHeight::from_u32(102),
            ),
            transaction.txid(),
            raw,
            0,
            3,
        ),
    )
    .unwrap()
}

fn removal(previous: &CanonicalZcashOutputObservation) -> CanonicalZcashOutputRemoval {
    CanonicalZcashOutputRemoval::validate(
        previous,
        &ZcashNodeRemovalSnapshot::new(
            NetworkType::Regtest,
            BranchId::Nu6_2,
            BlockHash([0x55; 32]),
            ZcashStableTip::new(
                BlockHash([0xbb; 32]),
                BlockHeight::from_u32(104),
                BlockHash([0xbb; 32]),
                BlockHeight::from_u32(104),
            ),
        ),
    )
    .unwrap()
}

fn swap(id: &str, direction: SwapDirection) -> SwapCoordinator {
    let (maker_chain, taker_chain) = match direction {
        SwapDirection::TakerSellsForeign => (Chain::Lez, Chain::Zcash),
        SwapDirection::TakerSellsLez => (Chain::Zcash, Chain::Lez),
    };
    SwapCoordinator::new_with_direction(
        SwapId::new(id).unwrap(),
        Pair::Zcash,
        direction,
        ConfirmationPolicy::new(1).unwrap(),
        RecoverySchedule::new(
            Pair::Zcash,
            direction,
            ChainPosition::block_height(maker_chain, 100),
            ChainPosition::block_height(taker_chain, 120),
            TimelockSafety::between(Chain::Lez, Chain::Zcash, 1_000, 1_200, 100).unwrap(),
        )
        .unwrap(),
    )
}

#[test]
fn forward_zec_runtime_commits_canonical_and_pre_maker_removal_across_restart() {
    let data = tempdir().unwrap();
    let path = data.path().join("forward.sqlite3");
    let mut store = SqliteSwapStore::open(&path).unwrap();
    let swap = swap("forward-runtime", SwapDirection::TakerSellsForeign);
    store.save(&swap).unwrap();
    let canonical = canonical_observation();
    let canonical_event = ZcashObservationEvent::Canonical(canonical.clone());

    let applied = apply_zcash_funding_event(&mut store, 0, swap.id(), &canonical_event).unwrap();
    assert_eq!(applied.swap().phase(), Phase::TakerLockConfirmed);
    assert_eq!(applied.commit().revision(), 1);
    let replay = apply_zcash_funding_event(&mut store, 0, swap.id(), &canonical_event).unwrap();
    assert!(replay.commit().was_replay());

    let removed = ZcashObservationEvent::Removed(removal(&canonical));
    apply_zcash_funding_event(&mut store, 1, swap.id(), &removed).unwrap();
    drop(store);
    let store = SqliteSwapStore::open(path).unwrap();
    assert_eq!(
        store.load(swap.id()).unwrap().unwrap().phase(),
        Phase::Offered
    );
    assert_eq!(
        load_zcash_observation_tracker(&store, swap.id())
            .unwrap()
            .current(),
        None
    );
}

#[test]
fn reverse_zec_runtime_replays_removal_before_core_and_restores_exact_reappearance() {
    let data = tempdir().unwrap();
    let mut store = SqliteSwapStore::open(data.path().join("reverse.sqlite3")).unwrap();
    let mut swap = swap("reverse-runtime", SwapDirection::TakerSellsLez);
    swap.observe_funding(
        Participant::Taker,
        ChainProof::new("lez-taker-lock", 1).unwrap(),
    )
    .unwrap();
    store.save(&swap).unwrap();
    let canonical = canonical_observation();
    let canonical_event = ZcashObservationEvent::Canonical(canonical.clone());
    apply_zcash_funding_event(&mut store, 0, swap.id(), &canonical_event).unwrap();
    let removed_event = ZcashObservationEvent::Removed(removal(&canonical));
    let removed = apply_zcash_funding_event(&mut store, 1, swap.id(), &removed_event).unwrap();
    assert_eq!(removed.swap().phase(), Phase::MakerLockReorged);
    let replay = apply_zcash_funding_event(&mut store, 1, swap.id(), &removed_event).unwrap();
    assert!(replay.commit().was_replay());
    let restored = apply_zcash_funding_event(&mut store, 2, swap.id(), &canonical_event).unwrap();
    assert_eq!(restored.swap().phase(), Phase::BothLegsLocked);
    drop(restored);
    let tracker = load_zcash_observation_tracker(&store, swap.id()).unwrap();
    assert_eq!(tracker.current(), Some(&canonical));
    assert_eq!(
        tracker
            .propose(&ZcashObservationReconciliation::Canonical(canonical))
            .unwrap(),
        None,
        "fresh exact requery is known after historical replay"
    );
}

#[test]
fn terminal_removal_is_journaled_and_reported_without_erasing_the_outcome() {
    for terminal in [Phase::Completed, Phase::Refunded] {
        let data = tempdir().unwrap();
        let mut store = SqliteSwapStore::open(data.path().join("terminal.sqlite3")).unwrap();
        let mut swap = swap("terminal-runtime", SwapDirection::TakerSellsForeign);
        store.save(&swap).unwrap();
        let canonical = canonical_observation();
        let canonical_event = ZcashObservationEvent::Canonical(canonical.clone());
        apply_zcash_funding_event(&mut store, 0, swap.id(), &canonical_event).unwrap();
        swap = store.load(swap.id()).unwrap().unwrap();
        swap.observe_funding(
            Participant::Maker,
            ChainProof::new("lez-maker-lock", 1).unwrap(),
        )
        .unwrap();
        match terminal {
            Phase::Completed => {
                let first = swap.first_claimant();
                swap.observe_revealing_claim(
                    first,
                    ChainProof::new("revealing-claim", 1).unwrap(),
                    ClaimEvidence::new([9; 32]),
                )
                .unwrap();
                swap.observe_followup_claim(
                    first.other(),
                    ChainProof::new("followup-claim", 1).unwrap(),
                )
                .unwrap();
            }
            Phase::Refunded => {
                swap.refund_maker_leg(ChainPosition::block_height(Chain::Lez, 100))
                    .unwrap();
                swap.refund_taker_leg(ChainPosition::block_height(Chain::Zcash, 120))
                    .unwrap();
            }
            _ => unreachable!(),
        }
        store.save(&swap).unwrap();

        let removed_event = ZcashObservationEvent::Removed(removal(&canonical));
        let applied = apply_zcash_funding_event(&mut store, 1, swap.id(), &removed_event).unwrap();
        assert_eq!(applied.swap().phase(), terminal);
        assert_eq!(
            applied.outcome(),
            ZcashFundingProjectionOutcome::TerminalReorgDetected {
                terminal_phase: terminal,
                funded_by: Participant::Taker,
            }
        );
        assert_eq!(applied.commit().revision(), 2);
        assert_eq!(
            store
                .load_zcash_events(swap.id(), Participant::Taker)
                .unwrap()
                .len(),
            2
        );

        let replay = apply_zcash_funding_event(&mut store, 1, swap.id(), &removed_event).unwrap();
        assert!(replay.commit().was_replay());
        assert_eq!(replay.swap().phase(), terminal);
        assert_eq!(
            replay.outcome(),
            ZcashFundingProjectionOutcome::TerminalReorgDetected {
                terminal_phase: terminal,
                funded_by: Participant::Taker,
            }
        );
        assert_eq!(
            store
                .load_zcash_events(swap.id(), Participant::Taker)
                .unwrap()
                .len(),
            2
        );
    }
}
