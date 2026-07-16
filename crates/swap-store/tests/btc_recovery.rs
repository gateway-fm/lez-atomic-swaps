use std::{
    fs::{self, OpenOptions},
    os::unix::fs::OpenOptionsExt as _,
    path::Path,
};

use lez_swap_core::{
    Chain, ChainPosition, ClaimEvidence, ConfirmationPolicy, Pair, Participant, Phase,
    RecoverySchedule, SwapCoordinator, SwapDirection, SwapId, TimelockSafety,
};
use lez_swap_store::{
    BtcAgreementAcceptance, BtcLifecycleEvidenceV1, BtcRecoveryError, BtcTerminalOutcome,
    SqliteBtcRecoveryStore, StoreError,
};
use rusqlite::Connection;
use tempfile::tempdir;

const SCALAR_SENTINEL: [u8; 32] = *b"M3-BTC-SCALAR-MUST-NOT-PERSIST!!";
const PUBLIC_REVEALING_WITNESS: [u8; 64] = [0x5a; 64];
const CORRUPTION_CASES: [&str; 5] = [
    "missing",
    "out-of-order",
    "snapshot",
    "zero-followup",
    "oversized-json",
];

fn coordinator(swap: &str, direction: SwapDirection) -> SwapCoordinator {
    coordinator_with(swap, direction, 1, 1, 0)
}

fn coordinator_with(
    swap: &str,
    direction: SwapDirection,
    taker_confirmations: u32,
    maker_confirmations: u32,
    schedule_offset: u64,
) -> SwapCoordinator {
    let (maker_chain, taker_chain) = match direction {
        SwapDirection::TakerSellsForeign => (Chain::Lez, Chain::Bitcoin),
        SwapDirection::TakerSellsLez => (Chain::Bitcoin, Chain::Lez),
    };
    let maker_deadline = 100 + schedule_offset;
    let taker_deadline = 200 + schedule_offset;
    let safety =
        TimelockSafety::between(maker_chain, taker_chain, maker_deadline, taker_deadline, 10)
            .expect("safe BTC refund ordering");
    let schedule = RecoverySchedule::new(
        Pair::Bitcoin,
        direction,
        ChainPosition::block_height(maker_chain, maker_deadline),
        ChainPosition::block_height(taker_chain, taker_deadline),
        safety,
    )
    .expect("BTC recovery schedule");
    SwapCoordinator::new_with_confirmation_policies(
        SwapId::new(swap).expect("swap id"),
        Pair::Bitcoin,
        direction,
        ConfirmationPolicy::new(taker_confirmations).expect("taker confirmation policy"),
        ConfirmationPolicy::new(maker_confirmations).expect("maker confirmation policy"),
        schedule,
    )
}

fn acceptance(coordinator: &SwapCoordinator, role: Participant) -> BtcAgreementAcceptance {
    let direction = match coordinator.direction() {
        SwapDirection::TakerSellsForeign => "taker_sells_btc",
        SwapDirection::TakerSellsLez => "taker_sells_lez",
    };
    let wire = format!(
        "btc-agreement-v1:{}:{direction}:fixed-contract-and-refund-terms",
        coordinator.id().as_str()
    );
    BtcAgreementAcceptance::new(
        coordinator,
        role,
        wire.into_bytes(),
        [0x42; 32],
        1_785_000_000,
    )
    .expect("bounded accepted agreement")
}

fn happy_path(coordinator: &SwapCoordinator) -> Vec<BtcLifecycleEvidenceV1> {
    let taker_chain = coordinator.funded_chain(Participant::Taker);
    let maker_chain = coordinator.funded_chain(Participant::Maker);
    vec![
        BtcLifecycleEvidenceV1::taker_lock(
            taker_chain,
            "taker-lock-tx",
            1,
            b"btc-chain-proof-v1:taker-lock".to_vec(),
        )
        .expect("taker lock evidence"),
        BtcLifecycleEvidenceV1::maker_lock(
            maker_chain,
            "maker-lock-tx",
            1,
            b"btc-chain-proof-v1:maker-lock".to_vec(),
        )
        .expect("maker lock evidence"),
        BtcLifecycleEvidenceV1::revealing_claim(
            maker_chain,
            "revealing-claim-tx",
            0,
            b"btc-chain-proof-v1:revealing-claim".to_vec(),
            PUBLIC_REVEALING_WITNESS,
            ClaimEvidence::new(SCALAR_SENTINEL),
        )
        .expect("revealing claim evidence"),
        BtcLifecycleEvidenceV1::followup_claim(
            taker_chain,
            "followup-claim-tx",
            1,
            b"btc-chain-proof-v1:followup-claim".to_vec(),
        )
        .expect("follow-up claim evidence"),
    ]
}

fn refund_path(coordinator: &SwapCoordinator) -> Vec<BtcLifecycleEvidenceV1> {
    let taker_chain = coordinator.funded_chain(Participant::Taker);
    let maker_chain = coordinator.funded_chain(Participant::Maker);
    vec![
        BtcLifecycleEvidenceV1::taker_lock(
            taker_chain,
            "taker-lock-tx",
            1,
            b"btc-chain-proof-v1:taker-lock".to_vec(),
        )
        .expect("taker lock evidence"),
        BtcLifecycleEvidenceV1::maker_lock(
            maker_chain,
            "maker-lock-tx",
            1,
            b"btc-chain-proof-v1:maker-lock".to_vec(),
        )
        .expect("maker lock evidence"),
        BtcLifecycleEvidenceV1::maker_leg_refund(
            maker_chain,
            "maker-refund-tx",
            1,
            b"btc-chain-proof-v1:maker-refund".to_vec(),
            ChainPosition::block_height(maker_chain, 100),
        )
        .expect("maker refund evidence"),
        BtcLifecycleEvidenceV1::taker_leg_refund(
            taker_chain,
            "taker-refund-tx",
            1,
            b"btc-chain-proof-v1:taker-refund".to_vec(),
            ChainPosition::block_height(taker_chain, 200),
        )
        .expect("taker refund evidence"),
    ]
}

fn assert_scalar_absent(path: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let candidate = path.with_file_name(format!(
            "{}{}",
            path.file_name()
                .expect("database filename")
                .to_string_lossy(),
            suffix
        ));
        let Ok(bytes) = fs::read(candidate) else {
            continue;
        };
        assert!(
            !bytes
                .windows(SCALAR_SENTINEL.len())
                .any(|window| window == SCALAR_SENTINEL),
            "raw scalar sentinel reached SQLite storage"
        );
    }
}

#[test]
fn existing_only_absent_store_does_not_create_activation() {
    let directory = tempdir().expect("temporary actor store");
    let path = directory.path().join("absent.sqlite");
    let initial = coordinator("btc-existing-absent", SwapDirection::TakerSellsForeign);
    let accepted = acceptance(&initial, Participant::Maker);

    assert!(matches!(
        SqliteBtcRecoveryStore::open_existing(&path, &accepted, &initial),
        Err(BtcRecoveryError::Store(StoreError::DatabaseFileUnavailable))
    ));
    assert!(!path.exists(), "existing-only open must not create a file");
}

#[test]
fn private_empty_database_is_not_activation_and_first_acceptance_is_not_a_replay() {
    let directory = tempdir().expect("temporary actor store");
    let path = directory.path().join("empty.sqlite");
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .expect("precreate private empty database");
    let initial = coordinator("btc-existing-empty", SwapDirection::TakerSellsForeign);
    let accepted = acceptance(&initial, Participant::Taker);

    assert!(matches!(
        SqliteBtcRecoveryStore::open_existing(&path, &accepted, &initial),
        Err(BtcRecoveryError::MissingAgreementAcceptance)
    ));
    let connection = Connection::open(&path).expect("inspect migrated empty database");
    let acceptances: i64 = connection
        .query_row("SELECT COUNT(*) FROM btc_actor_agreements", [], |row| {
            row.get(0)
        })
        .expect("acceptance count");
    assert_eq!(acceptances, 0, "status-style open must not activate");
    drop(connection);

    let first = SqliteBtcRecoveryStore::open(&path, &accepted, &initial)
        .expect("explicit first activation");
    assert!(!first.acceptance_was_replay());
    drop(first);
    let replay =
        SqliteBtcRecoveryStore::open(&path, &accepted, &initial).expect("exact activation replay");
    assert!(replay.acceptance_was_replay());
}

#[test]
fn interrupted_or_incomplete_existing_activation_fails_closed() {
    let directory = tempdir().expect("temporary actor stores");
    let initial = coordinator("btc-existing-orphan", SwapDirection::TakerSellsForeign);
    let accepted = acceptance(&initial, Participant::Maker);
    for orphan_kind in ["aggregate", "other_role_aggregate", "evidence"] {
        let orphan_path = directory
            .path()
            .join(format!("orphan-{orphan_kind}.sqlite"));
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&orphan_path)
            .expect("precreate private database");
        assert!(matches!(
            SqliteBtcRecoveryStore::open_existing(&orphan_path, &accepted, &initial),
            Err(BtcRecoveryError::MissingAgreementAcceptance)
        ));
        let connection = Connection::open(&orphan_path).expect("open migrated database");
        connection
            .pragma_update(None, "foreign_keys", "OFF")
            .expect("permit impossible interrupted fixture");
        match orphan_kind {
            "aggregate" | "other_role_aggregate" => {
                let local_role = if orphan_kind == "aggregate" {
                    "maker"
                } else {
                    "taker"
                };
                connection.execute(
                    "INSERT INTO btc_actor_aggregates (
                        swap_id, local_role, revision, snapshot_version, snapshot_json,
                        evidence_chain_version, evidence_chain_head
                    ) VALUES (?1, ?2, 0, 1, '{}', 1, ?3)",
                    rusqlite::params![initial.id().as_str(), local_role, [1_u8; 32].as_slice()],
                )
            }
            "evidence" => connection.execute(
                "INSERT INTO btc_actor_evidence (
                    swap_id, local_role, aggregate_revision, evidence_kind,
                    payload_version, payload_json
                ) VALUES (?1, 'maker', 1, 'taker_lock', 1, '{}')",
                [initial.id().as_str()],
            ),
            _ => unreachable!("closed orphan fixture set"),
        }
        .expect("simulate interrupted orphan state");
        drop(connection);
        assert!(matches!(
            SqliteBtcRecoveryStore::open_existing(&orphan_path, &accepted, &initial),
            Err(BtcRecoveryError::AgreementConflict)
        ));
    }

    let incomplete_path = directory.path().join("missing-aggregate.sqlite");
    let activated = SqliteBtcRecoveryStore::open(&incomplete_path, &accepted, &initial)
        .expect("create accepted store");
    drop(activated);
    let connection = Connection::open(&incomplete_path).expect("open accepted database");
    connection
        .execute(
            "DELETE FROM btc_actor_aggregates WHERE swap_id = ?1",
            [initial.id().as_str()],
        )
        .expect("simulate interrupted missing aggregate");
    drop(connection);
    let incomplete = SqliteBtcRecoveryStore::open_existing(&incomplete_path, &accepted, &initial);
    assert!(
        matches!(
            incomplete,
            Err(BtcRecoveryError::InvalidSequence { revision: 0 })
        ),
        "unexpected incomplete-store result: {incomplete:?}"
    );
}

#[test]
fn maker_and_taker_reconstruct_both_btc_directions_through_completed_revision_four() {
    let directory = tempdir().expect("temporary actor stores");

    for (direction_index, direction) in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ]
    .into_iter()
    .enumerate()
    {
        let swap = format!("btc-direction-{direction_index}");
        for role in [Participant::Maker, Participant::Taker] {
            let role_name = match role {
                Participant::Maker => "maker",
                Participant::Taker => "taker",
            };
            let initial = coordinator(&swap, direction);
            let accepted = acceptance(&initial, role);
            let path = directory
                .path()
                .join(format!("{direction_index}-{role_name}.sqlite"));
            let evidence = happy_path(&initial);
            let expected_phases = [
                Phase::TakerLockConfirmed,
                Phase::BothLegsLocked,
                Phase::ClaimEvidenceAvailable,
                Phase::Completed,
            ];

            for (index, record) in evidence.iter().enumerate() {
                let mut store = SqliteBtcRecoveryStore::open(&path, &accepted, &initial)
                    .expect("open and reconstruct actor state");
                assert_eq!(
                    store.status().expect("offline status").revision(),
                    index as u64
                );
                let commit = store
                    .project(index as u64, record)
                    .expect("project exact lifecycle evidence");
                assert_eq!(commit.revision(), index as u64 + 1);
                assert!(!commit.was_replay());
                assert_eq!(
                    store.status().expect("updated offline status").phase(),
                    expected_phases[index]
                );
                assert_eq!(
                    store
                        .status()
                        .expect("updated offline witness status")
                        .revealing_public_witness(),
                    (index >= 2).then_some(PUBLIC_REVEALING_WITNESS.as_slice())
                );
            }

            let mut store = SqliteBtcRecoveryStore::open(&path, &accepted, &initial)
                .expect("terminal state reconstructs without either RPC");
            let status = store.status().expect("terminal offline status");
            assert_eq!(status.revision(), 4);
            assert_eq!(status.phase(), Phase::Completed);
            assert_eq!(status.terminal(), Some(BtcTerminalOutcome::Completed));
            assert_eq!(
                status.revealing_public_witness(),
                Some(PUBLIC_REVEALING_WITNESS.as_slice())
            );
            let replay = store
                .project(3, &evidence[3])
                .expect("exact terminal replay is idempotent");
            assert_eq!(replay.revision(), 4);
            assert!(replay.was_replay());
            let mut changed_witness = PUBLIC_REVEALING_WITNESS;
            changed_witness[17] ^= 1;
            let changed_revealing = BtcLifecycleEvidenceV1::revealing_claim(
                initial.funded_chain(Participant::Maker),
                "revealing-claim-tx",
                1,
                b"btc-chain-proof-v1:revealing-claim".to_vec(),
                changed_witness,
                ClaimEvidence::new(SCALAR_SENTINEL),
            )
            .expect("mutated public witness record");
            assert!(matches!(
                store.project(2, &changed_revealing),
                Err(BtcRecoveryError::EvidenceConflict { revision: 3 })
            ));
            drop(store);
            assert_scalar_absent(&path);
        }
    }
}

#[test]
fn maker_and_taker_reconstruct_both_refund_directions_through_revision_four() {
    let directory = tempdir().expect("temporary actor stores");
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        for role in [Participant::Maker, Participant::Taker] {
            let suffix = format!("{direction:?}-{role:?}").to_lowercase();
            let initial = coordinator(&format!("btc-refund-{suffix}"), direction);
            let accepted = acceptance(&initial, role);
            let path = directory.path().join(format!("refund-{suffix}.sqlite"));
            let mut store = SqliteBtcRecoveryStore::open(&path, &accepted, &initial)
                .expect("activate refund store");
            for (predecessor, evidence) in refund_path(&initial).iter().enumerate() {
                let predecessor = u64::try_from(predecessor).expect("small predecessor");
                let commit = store
                    .project(predecessor, evidence)
                    .expect("project exact refund lifecycle evidence");
                assert_eq!(commit.revision(), predecessor + 1);
                assert!(!commit.was_replay());
            }
            let status = store.status().expect("terminal refund status");
            assert_eq!(status.revision(), 4);
            assert_eq!(status.phase(), Phase::Refunded);
            assert_eq!(status.terminal(), Some(BtcTerminalOutcome::Refunded));
            assert_eq!(status.revealing_public_witness(), None);
            drop(store);

            let mut reopened = SqliteBtcRecoveryStore::open_existing(&path, &accepted, &initial)
                .expect("replay refund path after restart");
            assert_eq!(reopened.status().expect("replayed status"), status);
            let replay = reopened
                .project(3, &refund_path(&initial)[3])
                .expect("exact terminal refund replay");
            assert!(replay.was_replay());
        }
    }
}

#[test]
fn refund_evidence_rejects_early_wrong_chain_zero_confirmation_and_happy_path_mix() {
    let initial = coordinator("btc-refund-invalid", SwapDirection::TakerSellsForeign);
    let maker_chain = initial.funded_chain(Participant::Maker);
    let taker_chain = initial.funded_chain(Participant::Taker);
    assert!(
        BtcLifecycleEvidenceV1::maker_leg_refund(
            taker_chain,
            "wrong-chain-refund",
            1,
            b"wrong-chain".to_vec(),
            ChainPosition::block_height(maker_chain, 100),
        )
        .is_err()
    );
    assert!(
        BtcLifecycleEvidenceV1::maker_leg_refund(
            maker_chain,
            "zero-confirmation-refund",
            0,
            b"zero-confirmation".to_vec(),
            ChainPosition::block_height(maker_chain, 100),
        )
        .is_err()
    );

    let directory = tempdir().expect("temporary actor store");
    let path = directory.path().join("invalid-refund.sqlite");
    let accepted = acceptance(&initial, Participant::Maker);
    let mut store =
        SqliteBtcRecoveryStore::open(&path, &accepted, &initial).expect("activate store");
    let path_evidence = refund_path(&initial);
    let _ = store.project(0, &path_evidence[0]).expect("taker lock");
    assert!(matches!(
        store.project(1, &path_evidence[2]),
        Err(BtcRecoveryError::InvalidSequence { revision: 2 })
    ));
    let _ = store.project(1, &path_evidence[1]).expect("maker lock");

    let early = BtcLifecycleEvidenceV1::maker_leg_refund(
        maker_chain,
        "early-maker-refund",
        1,
        b"early-maker-refund".to_vec(),
        ChainPosition::block_height(maker_chain, 99),
    )
    .expect("well-shaped but early evidence");
    assert!(matches!(
        store.project(2, &early),
        Err(BtcRecoveryError::InvalidEvidence { revision: 3 })
    ));

    let revealing = &happy_path(&initial)[2];
    let _ = store
        .project(2, revealing)
        .expect("happy branch revealing claim");
    assert!(matches!(
        store.project(3, &path_evidence[3]),
        Err(BtcRecoveryError::InvalidEvidence { revision: 4 })
    ));
}

#[test]
fn old_happy_path_rows_migrate_without_changing_exact_payloads() {
    let directory = tempdir().expect("temporary actor store");
    let path = directory.path().join("old-evidence-schema.sqlite");
    let initial = coordinator("btc-old-evidence-schema", SwapDirection::TakerSellsForeign);
    let accepted = acceptance(&initial, Participant::Maker);
    let mut store =
        SqliteBtcRecoveryStore::open(&path, &accepted, &initial).expect("activate store");
    let happy = happy_path(&initial);
    assert!(
        !serde_json::to_string(&happy[0])
            .expect("encode old-shape evidence")
            .contains("refund_position")
    );
    let _ = store.project(0, &happy[0]).expect("project taker lock");
    let _ = store.project(1, &happy[1]).expect("project maker lock");
    drop(store);

    let connection = Connection::open(&path).expect("open migration fixture");
    let exact_before: String = connection
        .query_row(
            "SELECT group_concat(payload_json, char(10)) FROM btc_actor_evidence ORDER BY aggregate_revision",
            [],
            |row| row.get(0),
        )
        .expect("exact pre-migration payloads");
    connection
        .execute_batch(
            "
            PRAGMA foreign_keys = OFF;
            ALTER TABLE btc_actor_evidence RENAME TO btc_actor_evidence_newer;
            CREATE TABLE btc_actor_evidence (
                swap_id TEXT NOT NULL,
                local_role TEXT NOT NULL CHECK (local_role IN ('maker', 'taker')),
                aggregate_revision INTEGER NOT NULL CHECK (aggregate_revision BETWEEN 1 AND 4),
                evidence_kind TEXT NOT NULL CHECK (evidence_kind IN (
                    'taker_lock', 'maker_lock', 'revealing_claim', 'followup_claim'
                )),
                payload_version INTEGER NOT NULL,
                payload_json TEXT NOT NULL CHECK (
                    length(CAST(payload_json AS BLOB)) BETWEEN 1 AND 327680
                ),
                PRIMARY KEY (swap_id, local_role, aggregate_revision),
                UNIQUE (swap_id, local_role, evidence_kind),
                FOREIGN KEY (swap_id, local_role)
                    REFERENCES btc_actor_aggregates(swap_id, local_role) ON DELETE RESTRICT
            );
            INSERT INTO btc_actor_evidence
            SELECT * FROM btc_actor_evidence_newer;
            DROP TABLE btc_actor_evidence_newer;
            PRAGMA foreign_keys = ON;
            ",
        )
        .expect("recreate old evidence constraint");
    drop(connection);

    let reopened = SqliteBtcRecoveryStore::open_existing(&path, &accepted, &initial)
        .expect("migrate old happy rows");
    assert_eq!(
        reopened
            .status()
            .expect("status after migration")
            .revision(),
        2
    );
    drop(reopened);
    let connection = Connection::open(&path).expect("inspect migrated store");
    let exact_after: String = connection
        .query_row(
            "SELECT group_concat(payload_json, char(10)) FROM btc_actor_evidence ORDER BY aggregate_revision",
            [],
            |row| row.get(0),
        )
        .expect("exact post-migration payloads");
    let schema: String = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'btc_actor_evidence'",
            [],
            |row| row.get(0),
        )
        .expect("migrated evidence schema");
    assert_eq!(exact_after, exact_before);
    assert!(schema.contains("'maker_refund'"));
}

#[test]
fn immutable_agreement_role_alias_replay_and_cas_conflicts_fail_closed() {
    let directory = tempdir().expect("temporary actor store");
    let path = directory.path().join("conflicts.sqlite");
    let initial = coordinator("btc-conflicts", SwapDirection::TakerSellsForeign);
    let accepted = acceptance(&initial, Participant::Maker);
    let evidence = happy_path(&initial);
    let mut store =
        SqliteBtcRecoveryStore::open(&path, &accepted, &initial).expect("create actor store");
    let _ = store.project(0, &evidence[0]).expect("first projection");

    assert!(
        store
            .project(0, &evidence[0])
            .expect("exact replay")
            .was_replay()
    );
    let changed = BtcLifecycleEvidenceV1::taker_lock(
        initial.funded_chain(Participant::Taker),
        "changed-taker-lock",
        1,
        b"btc-chain-proof-v1:taker-lock".to_vec(),
    )
    .expect("changed evidence");
    assert!(matches!(
        store.project(0, &changed),
        Err(BtcRecoveryError::EvidenceConflict { revision: 1 })
    ));
    assert!(matches!(
        store.project(9, &evidence[1]),
        Err(BtcRecoveryError::StalePredecessor {
            expected: 9,
            actual: 1
        })
    ));
    assert!(matches!(
        store.project(1, &evidence[3]),
        Err(BtcRecoveryError::InvalidSequence { revision: 2 })
    ));
    drop(store);

    let changed_agreement = BtcAgreementAcceptance::new(
        &initial,
        Participant::Maker,
        b"different-exact-wire".to_vec(),
        [0x42; 32],
        1_785_000_000,
    )
    .expect("different bounded agreement");
    assert!(matches!(
        SqliteBtcRecoveryStore::open(&path, &changed_agreement, &initial),
        Err(BtcRecoveryError::AgreementConflict)
    ));

    let role_alias = acceptance(&initial, Participant::Taker);
    assert!(matches!(
        SqliteBtcRecoveryStore::open(&path, &role_alias, &initial),
        Err(BtcRecoveryError::RolePathAlias)
    ));
}

#[test]
fn acceptance_binds_every_immutable_initial_coordinator_field() {
    let directory = tempdir().expect("temporary agreement-bound store");
    let path = directory.path().join("initial-digest.sqlite");
    let swap = "btc-initial-digest";
    let initial = coordinator(swap, SwapDirection::TakerSellsForeign);
    let accepted = acceptance(&initial, Participant::Maker);
    drop(
        SqliteBtcRecoveryStore::open(&path, &accepted, &initial)
            .expect("persist exact initial coordinator digest"),
    );

    let changed_initials = [
        coordinator_with(swap, SwapDirection::TakerSellsLez, 1, 1, 0),
        coordinator_with(swap, SwapDirection::TakerSellsForeign, 2, 1, 0),
        coordinator_with(swap, SwapDirection::TakerSellsForeign, 1, 2, 0),
        coordinator_with(swap, SwapDirection::TakerSellsForeign, 1, 1, 1),
    ];
    for changed in &changed_initials {
        assert!(matches!(
            SqliteBtcRecoveryStore::open(&path, &accepted, changed),
            Err(BtcRecoveryError::InitialCoordinatorMismatch)
        ));
    }
}

#[test]
fn agreement_chain_evidence_and_terminal_confirmation_bounds_fail_closed() {
    let directory = tempdir().expect("temporary boundary store");
    let path = directory.path().join("boundaries.sqlite");
    let initial = coordinator("btc-boundaries", SwapDirection::TakerSellsForeign);
    let exact_max = BtcAgreementAcceptance::new(
        &initial,
        Participant::Taker,
        vec![0xa5; 16 * 1024],
        [0x42; 32],
        0,
    )
    .expect("16 KiB agreement wire boundary");
    let mut store = SqliteBtcRecoveryStore::open(&path, &exact_max, &initial)
        .expect("persist 16 KiB agreement wire boundary");
    assert!(matches!(
        BtcAgreementAcceptance::new(
            &initial,
            Participant::Taker,
            vec![0xa5; 16 * 1024 + 1],
            [0x42; 32],
            0,
        ),
        Err(BtcRecoveryError::InvalidAgreementAcceptance)
    ));
    assert!(matches!(
        BtcAgreementAcceptance::new(
            &initial,
            Participant::Taker,
            b"agreement".to_vec(),
            [0; 32],
            0,
        ),
        Err(BtcRecoveryError::InvalidAgreementAcceptance)
    ));

    let maximum_evidence = BtcLifecycleEvidenceV1::taker_lock(
        initial.funded_chain(Participant::Taker),
        "maximum-chain-evidence",
        1,
        vec![u8::MAX; 64 * 1024],
    )
    .expect("64 KiB exact chain evidence boundary");
    let _ = store
        .project(0, &maximum_evidence)
        .expect("encoded evidence remains inside durable JSON bound");
    assert!(matches!(
        BtcLifecycleEvidenceV1::taker_lock(
            initial.funded_chain(Participant::Taker),
            "oversized-chain-evidence",
            1,
            vec![u8::MAX; 64 * 1024 + 1],
        ),
        Err(BtcRecoveryError::InvalidEvidence { revision: 0 })
    ));
    assert!(matches!(
        BtcLifecycleEvidenceV1::followup_claim(
            initial.funded_chain(Participant::Taker),
            "zero-confirmation-followup",
            0,
            b"btc-chain-proof-v1:zero-confirmation-followup".to_vec(),
        ),
        Err(BtcRecoveryError::InvalidEvidence { revision: 0 })
    ));
}

#[test]
fn all_zero_revealing_witness_fails_in_construction_and_historical_replay() {
    let directory = tempdir().expect("temporary zero-witness store");
    let path = directory.path().join("zero-witness.sqlite");
    let initial = coordinator("btc-zero-witness", SwapDirection::TakerSellsLez);
    assert!(matches!(
        BtcLifecycleEvidenceV1::revealing_claim(
            initial.funded_chain(Participant::Maker),
            "zero-revealing-witness",
            0,
            b"btc-chain-proof-v1:zero-revealing-witness".to_vec(),
            [0; 64],
            ClaimEvidence::new(SCALAR_SENTINEL),
        ),
        Err(BtcRecoveryError::InvalidEvidence { revision: 0 })
    ));

    let accepted = acceptance(&initial, Participant::Maker);
    let evidence = happy_path(&initial);
    let mut store =
        SqliteBtcRecoveryStore::open(&path, &accepted, &initial).expect("create actor store");
    for (index, record) in evidence.iter().take(3).enumerate() {
        let _ = store
            .project(index as u64, record)
            .expect("advance through revealing claim");
    }
    drop(store);

    let raw = Connection::open(&path).expect("open corruption connection");
    let payload: String = raw
        .query_row(
            "SELECT payload_json FROM btc_actor_evidence WHERE aggregate_revision = 3",
            [],
            |row| row.get(0),
        )
        .expect("load revealing payload");
    let mut payload: serde_json::Value = serde_json::from_str(&payload).expect("valid payload");
    payload["revealing_public_witness"] = serde_json::json!(vec![0_u8; 64]);
    raw.execute(
        "UPDATE btc_actor_evidence SET payload_json = ?1 WHERE aggregate_revision = 3",
        [serde_json::to_string(&payload).expect("encode corrupt payload")],
    )
    .expect("inject all-zero historical witness");
    drop(raw);

    assert!(matches!(
        SqliteBtcRecoveryStore::open(&path, &accepted, &initial),
        Err(BtcRecoveryError::InvalidEvidence { revision: 3 })
    ));
}

#[test]
fn evidence_hash_chain_rejects_nonzero_witness_chain_bytes_and_typed_field_mutations() {
    let directory = tempdir().expect("temporary evidence-chain stores");

    for mutation in ["witness", "chain-bytes", "confirmations"] {
        let path = directory.path().join(format!("{mutation}.sqlite"));
        let initial = coordinator(
            &format!("btc-evidence-chain-{mutation}"),
            SwapDirection::TakerSellsForeign,
        );
        let accepted = acceptance(&initial, Participant::Taker);
        let evidence = happy_path(&initial);
        let mut store =
            SqliteBtcRecoveryStore::open(&path, &accepted, &initial).expect("create actor store");
        for (index, record) in evidence.iter().enumerate() {
            let _ = store
                .project(index as u64, record)
                .expect("advance complete evidence chain");
        }
        drop(store);

        let revision = match mutation {
            "witness" => 3,
            "chain-bytes" => 2,
            "confirmations" => 4,
            _ => unreachable!("fixed mutation fixture"),
        };
        let raw = Connection::open(&path).expect("open corruption connection");
        let payload: String = raw
            .query_row(
                "SELECT payload_json FROM btc_actor_evidence WHERE aggregate_revision = ?1",
                [revision],
                |row| row.get(0),
            )
            .expect("load exact evidence payload");
        let mut payload: serde_json::Value = serde_json::from_str(&payload).expect("valid payload");
        match mutation {
            "witness" => payload["revealing_public_witness"][0] = serde_json::json!(0x5b),
            "chain-bytes" => payload["chain_evidence"][0] = serde_json::json!(0x63),
            "confirmations" => payload["proof"]["confirmations"] = serde_json::json!(2),
            _ => unreachable!("fixed mutation fixture"),
        }
        raw.execute(
            "UPDATE btc_actor_evidence SET payload_json = ?1 WHERE aggregate_revision = ?2",
            rusqlite::params![
                serde_json::to_string(&payload).expect("encode mutated payload"),
                revision
            ],
        )
        .expect("inject one-byte semantic mutation");
        drop(raw);

        assert!(matches!(
            SqliteBtcRecoveryStore::open(&path, &accepted, &initial),
            Err(BtcRecoveryError::EvidenceChainMismatch)
        ));
    }
}

#[test]
fn evidence_and_snapshot_advance_in_one_rollback_safe_transaction() {
    let directory = tempdir().expect("temporary actor store");
    let path = directory.path().join("rollback.sqlite");
    let initial = coordinator("btc-rollback", SwapDirection::TakerSellsLez);
    let accepted = acceptance(&initial, Participant::Taker);
    let evidence = happy_path(&initial);
    let mut store =
        SqliteBtcRecoveryStore::open(&path, &accepted, &initial).expect("create actor store");

    let raw = Connection::open(&path).expect("open trigger connection");
    let head_before: Vec<u8> = raw
        .query_row(
            "SELECT evidence_chain_head FROM btc_actor_aggregates",
            [],
            |row| row.get(0),
        )
        .expect("read genesis evidence-chain head");
    raw.execute_batch(
        "
        CREATE TRIGGER btc_force_aggregate_failure
        BEFORE UPDATE OF revision ON btc_actor_aggregates
        BEGIN SELECT RAISE(ABORT, 'forced aggregate failure'); END;
        ",
    )
    .expect("install rollback trigger");
    drop(raw);
    assert!(matches!(
        store.project(0, &evidence[0]),
        Err(BtcRecoveryError::Store(_))
    ));

    let raw = Connection::open(&path).expect("inspect rollback");
    let count: i64 = raw
        .query_row("SELECT COUNT(*) FROM btc_actor_evidence", [], |row| {
            row.get(0)
        })
        .expect("count rolled-back evidence");
    let revision: i64 = raw
        .query_row("SELECT revision FROM btc_actor_aggregates", [], |row| {
            row.get(0)
        })
        .expect("read rolled-back revision");
    let head_after: Vec<u8> = raw
        .query_row(
            "SELECT evidence_chain_head FROM btc_actor_aggregates",
            [],
            |row| row.get(0),
        )
        .expect("read rolled-back evidence-chain head");
    assert_eq!((count, revision), (0, 0));
    assert_eq!(head_after, head_before);
    raw.execute_batch("DROP TRIGGER btc_force_aggregate_failure")
        .expect("remove rollback trigger");
    drop(raw);

    assert_eq!(
        store
            .project(0, &evidence[0])
            .expect("retry after rollback")
            .revision(),
        1
    );
}

#[test]
fn reopen_rejects_missing_or_out_of_order_evidence_instead_of_trusting_snapshot() {
    let directory = tempdir().expect("temporary actor stores");

    for corruption in CORRUPTION_CASES {
        let path = directory.path().join(format!("{corruption}.sqlite"));
        let initial = coordinator(
            &format!("btc-corrupt-{corruption}"),
            SwapDirection::TakerSellsForeign,
        );
        let accepted = acceptance(&initial, Participant::Maker);
        let evidence = happy_path(&initial);
        let mut store =
            SqliteBtcRecoveryStore::open(&path, &accepted, &initial).expect("create actor store");
        for (index, record) in evidence.iter().enumerate() {
            let _ = store
                .project(index as u64, record)
                .expect("advance before corruption");
        }
        drop(store);

        let raw = Connection::open(&path).expect("open corruption connection");
        if corruption == "missing" {
            raw.execute(
                "DELETE FROM btc_actor_evidence WHERE aggregate_revision = 2",
                [],
            )
            .expect("remove middle evidence");
        } else if corruption == "out-of-order" {
            raw.execute(
                "UPDATE btc_actor_evidence
                 SET payload_json = replace(
                    payload_json,
                    '\"kind\":\"revealing_claim\"',
                    '\"kind\":\"followup_claim\"'
                 ) WHERE aggregate_revision = 3",
                [],
            )
            .expect("change evidence sequence");
        } else if corruption == "snapshot" {
            raw.execute(
                "UPDATE btc_actor_aggregates
                 SET snapshot_json = replace(
                    snapshot_json,
                    '\"phase\":\"Completed\"',
                    '\"phase\":\"Offered\"'
                 )",
                [],
            )
            .expect("corrupt serialized aggregate phase");
        } else if corruption == "zero-followup" {
            raw.execute(
                "UPDATE btc_actor_evidence
                 SET payload_json = replace(
                    payload_json,
                    '\"confirmations\":1',
                    '\"confirmations\":0'
                 ) WHERE aggregate_revision = 4",
                [],
            )
            .expect("corrupt follow-up claim canonicality");
        } else {
            let oversized = "x".repeat(320 * 1024 + 1);
            assert!(
                raw.execute(
                    "UPDATE btc_actor_evidence SET payload_json = ?1
                     WHERE aggregate_revision = 1",
                    [&oversized],
                )
                .is_err(),
                "SQL encoded-evidence bound must reject oversized payloads"
            );
            raw.pragma_update(None, "ignore_check_constraints", "ON")
                .expect("enable deliberate corruption fixture");
            raw.execute(
                "UPDATE btc_actor_evidence SET payload_json = ?1
                 WHERE aggregate_revision = 1",
                [&oversized],
            )
            .expect("inject deliberate over-bound historical payload");
        }
        drop(raw);

        let reopened = SqliteBtcRecoveryStore::open(&path, &accepted, &initial);
        match corruption {
            "snapshot" => assert!(matches!(reopened, Err(BtcRecoveryError::SnapshotMismatch))),
            "zero-followup" => assert!(matches!(
                reopened,
                Err(BtcRecoveryError::InvalidEvidence { revision: 4 })
            )),
            "oversized-json" => assert!(matches!(
                reopened,
                Err(BtcRecoveryError::InvalidEvidence { revision: 1 })
            )),
            _ => assert!(matches!(
                reopened,
                Err(BtcRecoveryError::InvalidSequence { .. })
            )),
        }
    }
}
