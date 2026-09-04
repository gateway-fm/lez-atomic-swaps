//! Exercises the ZEC reference actor; compiled only with `pair-zec`.
#![cfg(feature = "pair-zec")]

use lez_maker_node::{
    ZcashFundingProjectionOutcome, apply_zcash_funding_event, load_zcash_observation_tracker,
};
use lez_swap_core::{
    Chain, ChainPosition, ChainProof, ClaimEvidence, ConfirmationPolicy, Pair, Participant, Phase,
    RecoverySchedule, SwapCoordinator, SwapDirection, SwapId, TimelockSafety,
};
use lez_swap_store::{OperatorAlertKind, OperatorAlertSeverity, SqliteSwapStore, StoreError};
use lez_zec_swap_sdk::{
    Bip199Contract, CanonicalZcashOutputObservation, CanonicalZcashOutputRemoval,
    ExpectedBip199Output, TransparentFundingRequest, TransparentUtxo, ZcashNodeRemovalSnapshot,
    ZcashNodeSnapshot, ZcashObservationEvent, ZcashObservationReconciliation, ZcashStableTip,
    ZecProfileId, ZecSwapBinding, build_funding_transaction,
};
use rusqlite::Connection;
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use std::path::Path;
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
    canonical_observation_for(7, [0x44; 32], 100, [0xaa; 32], 102)
}

fn canonical_observation_for(
    seed: u8,
    inclusion_hash: [u8; 32],
    inclusion_height: u32,
    tip_hash: [u8; 32],
    tip_height: u32,
) -> CanonicalZcashOutputObservation {
    let key = SecretKey::from_slice(&[seed; 32]).unwrap();
    let public_key = PublicKey::from_secret_key(&Secp256k1::new(), &key);
    let owner_script: Script = TransparentAddress::from_pubkey(&public_key).script().into();
    let contract = Bip199Contract::new(500_000, [0x11; 20], [0x22; 32], [0x33; 20]);
    let request = TransparentFundingRequest::new(
        vec![TransparentUtxo::new(
            OutPoint::new([seed.wrapping_add(2); 32], 0),
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
            BlockHash(inclusion_hash),
            BlockHash(inclusion_hash),
            BlockHeight::from_u32(inclusion_height),
            ZcashStableTip::new(
                BlockHash(tip_hash),
                BlockHeight::from_u32(tip_height),
                BlockHash(tip_hash),
                BlockHeight::from_u32(tip_height),
            ),
            transaction.txid(),
            raw,
            0,
            tip_height - inclusion_height + 1,
        ),
    )
    .unwrap()
}

fn replacement_observation() -> CanonicalZcashOutputObservation {
    canonical_observation_for(8, [0x66; 32], 101, [0xcc; 32], 104)
}

fn binding() -> ZecSwapBinding {
    binding_with_value(100_000)
}

fn binding_with_value(value: u64) -> ZecSwapBinding {
    ZecSwapBinding::new(
        ZecProfileId::DeterministicLocalV1,
        ExpectedBip199Output::new(
            NetworkType::Regtest,
            BranchId::Nu6_2,
            zatoshis(value),
            Bip199Contract::new(500_000, [0x11; 20], [0x22; 32], [0x33; 20]),
        ),
    )
    .unwrap()
}

fn assert_single_alert(
    store: &SqliteSwapStore,
    id: &SwapId,
    revision: u64,
    kind: OperatorAlertKind,
    severity: OperatorAlertSeverity,
    funded_by: Participant,
) -> u64 {
    let alerts = store.list_operator_alerts(id, 0, true).unwrap();
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].aggregate_revision(), revision);
    assert_eq!(alerts[0].record().kind(), kind);
    assert_eq!(alerts[0].record().severity(), severity);
    assert_eq!(alerts[0].record().funded_by(), funded_by);
    assert!(!alerts[0].acknowledged());
    alerts[0].sequence()
}

fn assert_alert_state(store: &SqliteSwapStore, id: &SwapId, sequence: u64, acknowledged: bool) {
    let alerts = store.list_operator_alerts(id, 0, true).unwrap();
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].sequence(), sequence);
    assert_eq!(alerts[0].acknowledged(), acknowledged);
}

fn assert_ack_is_swap_scoped_and_survives_restart(
    store: SqliteSwapStore,
    path: &Path,
    id: &SwapId,
    sequence: u64,
) {
    assert!(matches!(
        store.acknowledge_operator_alert(&SwapId::new("different-swap").unwrap(), sequence),
        Err(StoreError::MissingOperatorAlert)
    ));
    drop(store);
    let reopened = SqliteSwapStore::open(path).unwrap();
    assert_alert_state(&reopened, id, sequence, true);
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
    store.save_with_zcash_binding(&swap, &binding()).unwrap();
    let canonical = canonical_observation();
    let canonical_event = ZcashObservationEvent::Canonical(canonical.clone());

    let applied = apply_zcash_funding_event(&mut store, 0, swap.id(), &canonical_event).unwrap();
    assert_eq!(applied.swap().phase(), Phase::TakerLockConfirmed);
    assert_eq!(applied.commit().revision(), 1);
    assert_eq!(applied.alert_sequence(), None);
    assert!(
        store
            .list_operator_alerts(swap.id(), 0, true)
            .unwrap()
            .is_empty()
    );
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
    store.save_with_zcash_binding(&swap, &binding()).unwrap();
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
fn terminal_reorg_is_journaled_and_reported_without_erasing_the_outcome() {
    for terminal in [Phase::Completed, Phase::Refunded] {
        for is_replacement in [false, true] {
            let data = tempdir().unwrap();
            let mut store = SqliteSwapStore::open(data.path().join("terminal.sqlite3")).unwrap();
            let mut swap = swap("terminal-runtime", SwapDirection::TakerSellsForeign);
            store.save_with_zcash_binding(&swap, &binding()).unwrap();
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
            store.save_with_zcash_binding(&swap, &binding()).unwrap();

            let replacement = replacement_observation();
            let reorg_event = if is_replacement {
                ZcashObservationEvent::Replaced {
                    removed: Box::new(removal(&canonical)),
                    canonical: Box::new(replacement.clone()),
                }
            } else {
                ZcashObservationEvent::Removed(removal(&canonical))
            };
            let applied =
                apply_zcash_funding_event(&mut store, 1, swap.id(), &reorg_event).unwrap();
            assert_eq!(applied.swap().phase(), terminal);
            assert_eq!(
                applied.outcome(),
                ZcashFundingProjectionOutcome::TerminalReorgDetected {
                    terminal_phase: terminal,
                    funded_by: Participant::Taker,
                }
            );
            let alert_sequence = assert_single_alert(
                &store,
                swap.id(),
                2,
                OperatorAlertKind::ZcashTerminalReorg,
                OperatorAlertSeverity::Critical,
                Participant::Taker,
            );
            assert_eq!(applied.alert_sequence(), Some(alert_sequence));
            assert_eq!(
                load_zcash_observation_tracker(&store, swap.id())
                    .unwrap()
                    .current(),
                is_replacement.then_some(&replacement)
            );
            assert_eq!(
                store
                    .load_zcash_events(swap.id(), Participant::Taker)
                    .unwrap()
                    .len(),
                2
            );

            let replay = apply_zcash_funding_event(&mut store, 1, swap.id(), &reorg_event).unwrap();
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
            assert_alert_state(&store, swap.id(), alert_sequence, false);
        }
    }
}

#[test]
fn post_dependent_replacement_conflict_is_atomic_and_replayable_for_both_roles() {
    for (direction, funded_by, reorg_phase) in [
        (
            SwapDirection::TakerSellsForeign,
            Participant::Taker,
            Phase::TakerLockReorged,
        ),
        (
            SwapDirection::TakerSellsLez,
            Participant::Maker,
            Phase::MakerLockReorged,
        ),
    ] {
        let data = tempdir().unwrap();
        let path = data.path().join("conflict.sqlite3");
        let mut store = SqliteSwapStore::open(&path).unwrap();
        let mut swap = swap("replacement-conflict", direction);
        if funded_by == Participant::Maker {
            swap.observe_funding(
                Participant::Taker,
                ChainProof::new("lez-taker-lock", 1).unwrap(),
            )
            .unwrap();
        }
        store.save_with_zcash_binding(&swap, &binding()).unwrap();
        let original = canonical_observation();
        apply_zcash_funding_event(
            &mut store,
            0,
            swap.id(),
            &ZcashObservationEvent::Canonical(original.clone()),
        )
        .unwrap();
        if funded_by == Participant::Taker {
            swap = store.load(swap.id()).unwrap().unwrap();
            swap.observe_funding(
                Participant::Maker,
                ChainProof::new("lez-maker-lock", 1).unwrap(),
            )
            .unwrap();
            store.save_with_zcash_binding(&swap, &binding()).unwrap();
        }

        let replacement = replacement_observation();
        let event = ZcashObservationEvent::Replaced {
            removed: Box::new(removal(&original)),
            canonical: Box::new(replacement.clone()),
        };
        let applied = apply_zcash_funding_event(&mut store, 1, swap.id(), &event).unwrap();
        assert_eq!(applied.swap().phase(), reorg_phase);
        assert_eq!(
            applied.outcome(),
            ZcashFundingProjectionOutcome::ReplacementConflict { funded_by }
        );
        assert_eq!(applied.commit().revision(), 2);
        let alert_sequence = assert_single_alert(
            &store,
            swap.id(),
            2,
            OperatorAlertKind::ZcashReplacementConflict,
            OperatorAlertSeverity::Warning,
            funded_by,
        );
        store
            .acknowledge_operator_alert(swap.id(), alert_sequence)
            .unwrap();
        assert!(
            store
                .list_operator_alerts(swap.id(), 0, false)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store.load_zcash_events(swap.id(), funded_by).unwrap().len(),
            2
        );
        assert_eq!(
            load_zcash_observation_tracker(&store, swap.id())
                .unwrap()
                .current(),
            Some(&replacement),
            "tracker follows chain truth while the core keeps the committed ID pinned"
        );

        let replay = apply_zcash_funding_event(&mut store, 1, swap.id(), &event).unwrap();
        assert!(replay.commit().was_replay());
        assert_eq!(replay.swap().phase(), reorg_phase);
        assert_eq!(
            replay.outcome(),
            ZcashFundingProjectionOutcome::ReplacementConflict { funded_by }
        );
        assert_eq!(
            store.load_zcash_events(swap.id(), funded_by).unwrap().len(),
            2
        );
        assert_alert_state(&store, swap.id(), alert_sequence, true);
        drop(replay);
        assert_ack_is_swap_scoped_and_survives_restart(store, &path, swap.id(), alert_sequence);
    }
}

#[test]
fn pre_dependent_taker_replacement_commits_the_new_chain_transaction() {
    let data = tempdir().unwrap();
    let mut store = SqliteSwapStore::open(data.path().join("pre-dependent.sqlite3")).unwrap();
    let swap = swap("pre-dependent", SwapDirection::TakerSellsForeign);
    store.save_with_zcash_binding(&swap, &binding()).unwrap();
    let original = canonical_observation();
    apply_zcash_funding_event(
        &mut store,
        0,
        swap.id(),
        &ZcashObservationEvent::Canonical(original.clone()),
    )
    .unwrap();

    let replacement = replacement_observation();
    let event = ZcashObservationEvent::Replaced {
        removed: Box::new(removal(&original)),
        canonical: Box::new(replacement.clone()),
    };
    let applied = apply_zcash_funding_event(&mut store, 1, swap.id(), &event).unwrap();
    assert_eq!(applied.outcome(), ZcashFundingProjectionOutcome::Applied);
    assert_eq!(applied.swap().phase(), Phase::TakerLockConfirmed);
    assert_eq!(
        applied.swap().funding_transaction_id(Participant::Taker),
        Some(replacement.transaction_id().to_string().as_str())
    );
    assert_eq!(
        load_zcash_observation_tracker(&store, swap.id())
            .unwrap()
            .current(),
        Some(&replacement)
    );
    let replay = apply_zcash_funding_event(&mut store, 1, swap.id(), &event).unwrap();
    assert!(replay.commit().was_replay());
    assert_eq!(replay.outcome(), ZcashFundingProjectionOutcome::Applied);
}

#[test]
fn same_transaction_remined_restores_both_funded_roles() {
    for (direction, funded_by) in [
        (SwapDirection::TakerSellsForeign, Participant::Taker),
        (SwapDirection::TakerSellsLez, Participant::Maker),
    ] {
        let data = tempdir().unwrap();
        let mut store = SqliteSwapStore::open(data.path().join("remined.sqlite3")).unwrap();
        let mut swap = swap("same-transaction-remined", direction);
        if funded_by == Participant::Maker {
            swap.observe_funding(
                Participant::Taker,
                ChainProof::new("lez-taker-lock", 1).unwrap(),
            )
            .unwrap();
        }
        store.save_with_zcash_binding(&swap, &binding()).unwrap();
        let original = canonical_observation();
        apply_zcash_funding_event(
            &mut store,
            0,
            swap.id(),
            &ZcashObservationEvent::Canonical(original.clone()),
        )
        .unwrap();
        if funded_by == Participant::Taker {
            swap = store.load(swap.id()).unwrap().unwrap();
            swap.observe_funding(
                Participant::Maker,
                ChainProof::new("lez-maker-lock", 1).unwrap(),
            )
            .unwrap();
            store.save_with_zcash_binding(&swap, &binding()).unwrap();
        }

        let remined = canonical_observation_for(7, [0x77; 32], 101, [0xdd; 32], 104);
        assert_eq!(remined.transaction_id(), original.transaction_id());
        let event = ZcashObservationEvent::Replaced {
            removed: Box::new(removal(&original)),
            canonical: Box::new(remined.clone()),
        };
        let applied = apply_zcash_funding_event(&mut store, 1, swap.id(), &event).unwrap();
        assert_eq!(applied.outcome(), ZcashFundingProjectionOutcome::Applied);
        assert_eq!(applied.swap().phase(), Phase::BothLegsLocked);
        assert_eq!(
            load_zcash_observation_tracker(&store, swap.id())
                .unwrap()
                .current(),
            Some(&remined)
        );
    }
}

#[test]
fn replacement_must_match_the_exact_durable_tracker_head_before_projection() {
    let data = tempdir().unwrap();
    let mut store = SqliteSwapStore::open(data.path().join("stale-head.sqlite3")).unwrap();
    let swap = swap("stale-head", SwapDirection::TakerSellsForeign);
    store.save_with_zcash_binding(&swap, &binding()).unwrap();
    let original = canonical_observation();
    apply_zcash_funding_event(
        &mut store,
        0,
        swap.id(),
        &ZcashObservationEvent::Canonical(original),
    )
    .unwrap();

    let stale_same_transaction = canonical_observation_for(7, [0x99; 32], 99, [0xee; 32], 102);
    let event = ZcashObservationEvent::Replaced {
        removed: Box::new(removal(&stale_same_transaction)),
        canonical: Box::new(replacement_observation()),
    };
    assert!(apply_zcash_funding_event(&mut store, 1, swap.id(), &event).is_err());
    assert_eq!(store.revision(swap.id()).unwrap(), Some(1));
    assert_eq!(
        store
            .load_zcash_events(swap.id(), Participant::Taker)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store.load(swap.id()).unwrap().unwrap().phase(),
        Phase::TakerLockConfirmed
    );
}

#[test]
fn unbound_legacy_zcash_swap_fails_before_replay_or_projection() {
    let data = tempdir().unwrap();
    let mut store = SqliteSwapStore::open(data.path().join("unbound.sqlite3")).unwrap();
    let swap = swap("unbound", SwapDirection::TakerSellsForeign);
    store.save(&swap).unwrap();
    let event = ZcashObservationEvent::Canonical(canonical_observation());

    assert!(apply_zcash_funding_event(&mut store, 0, swap.id(), &event).is_err());
    assert!(load_zcash_observation_tracker(&store, swap.id()).is_err());
    assert_eq!(store.revision(swap.id()).unwrap(), Some(0));
    assert!(
        store
            .load_zcash_events(swap.id(), Participant::Taker)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store.load(swap.id()).unwrap().unwrap().phase(),
        Phase::Offered
    );
}

#[test]
fn event_must_match_the_durable_expected_output_before_replay_or_projection() {
    let data = tempdir().unwrap();
    let mut store = SqliteSwapStore::open(data.path().join("mismatched-binding.sqlite3")).unwrap();
    let swap = swap("mismatched-binding", SwapDirection::TakerSellsForeign);
    store
        .save_with_zcash_binding(&swap, &binding_with_value(99_000))
        .unwrap();
    let event = ZcashObservationEvent::Canonical(canonical_observation());

    assert!(apply_zcash_funding_event(&mut store, 0, swap.id(), &event).is_err());
    assert_eq!(store.revision(swap.id()).unwrap(), Some(0));
    assert!(
        store
            .load_zcash_events(swap.id(), Participant::Taker)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store.load(swap.id()).unwrap().unwrap().phase(),
        Phase::Offered
    );
}

#[test]
fn durable_profile_confirmations_must_match_both_coordinator_leg_policies() {
    let data = tempdir().unwrap();
    let mut store = SqliteSwapStore::open(data.path().join("mismatched-policy.sqlite3")).unwrap();
    let direction = SwapDirection::TakerSellsForeign;
    let swap = SwapCoordinator::new_with_confirmation_policies(
        SwapId::new("mismatched-policy").unwrap(),
        Pair::Zcash,
        direction,
        ConfirmationPolicy::new(2).unwrap(),
        ConfirmationPolicy::new(1).unwrap(),
        RecoverySchedule::new(
            Pair::Zcash,
            direction,
            ChainPosition::block_height(Chain::Lez, 100),
            ChainPosition::block_height(Chain::Zcash, 120),
            TimelockSafety::between(Chain::Lez, Chain::Zcash, 1_000, 1_200, 100).unwrap(),
        )
        .unwrap(),
    );
    store.save_with_zcash_binding(&swap, &binding()).unwrap();

    assert!(
        apply_zcash_funding_event(
            &mut store,
            0,
            swap.id(),
            &ZcashObservationEvent::Canonical(canonical_observation()),
        )
        .is_err()
    );
    assert_eq!(store.revision(swap.id()).unwrap(), Some(0));
}

#[test]
fn alert_insert_failure_rolls_back_event_revision_and_reorg_projection() {
    let data = tempdir().unwrap();
    let path = data.path().join("alert-rollback.sqlite3");
    let mut store = SqliteSwapStore::open(&path).unwrap();
    let mut swap = swap("alert-rollback", SwapDirection::TakerSellsForeign);
    store.save_with_zcash_binding(&swap, &binding()).unwrap();
    let original = canonical_observation();
    apply_zcash_funding_event(
        &mut store,
        0,
        swap.id(),
        &ZcashObservationEvent::Canonical(original.clone()),
    )
    .unwrap();
    swap = store.load(swap.id()).unwrap().unwrap();
    swap.observe_funding(
        Participant::Maker,
        ChainProof::new("lez-maker-lock", 1).unwrap(),
    )
    .unwrap();
    store.save_with_zcash_binding(&swap, &binding()).unwrap();
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "
            CREATE TRIGGER reject_operator_alert
            BEFORE INSERT ON operator_alert_outbox
            BEGIN
                SELECT RAISE(ABORT, 'forced alert failure');
            END;
            ",
        )
        .unwrap();
    drop(connection);

    let replacement = ZcashObservationEvent::Replaced {
        removed: Box::new(removal(&original)),
        canonical: Box::new(replacement_observation()),
    };
    assert!(apply_zcash_funding_event(&mut store, 1, swap.id(), &replacement).is_err());
    assert_eq!(store.revision(swap.id()).unwrap(), Some(1));
    assert_eq!(
        store.load(swap.id()).unwrap().unwrap().phase(),
        Phase::BothLegsLocked
    );
    assert_eq!(
        store
            .load_zcash_events(swap.id(), Participant::Taker)
            .unwrap()
            .len(),
        1
    );
    assert!(
        store
            .list_operator_alerts(swap.id(), 0, true)
            .unwrap()
            .is_empty()
    );
}
