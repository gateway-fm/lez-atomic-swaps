//! Drives two swap coordinators through one `SqliteSwapStore` file, with restarts, and checks
//! the store against an in-memory model after every operation.
//!
//! Input: two profile bytes (one per swap, as in `coordinator_model::profile_from_byte`), then
//! operations of two bytes each. The first byte of an operation picks the swap (upper case or
//! bit `0x20` clear: the first; lower case: the second) and what happens after the action; the
//! second byte is the action applied to that swap (`coordinator_model::apply_action`).
//!
//! | Byte | After the action                                                                  |
//! |------|-----------------------------------------------------------------------------------|
//! | `k`  | nothing: the change stays in memory                                               |
//! | `s`  | `save` (the first save of a Zcash swap is `save_with_zcash_binding`)              |
//! | `r`  | save, then drop the store and reopen it                                           |
//! | `x`  | drop the store and reopen it without saving: every unsaved change is lost         |
//! | `o`  | `save` the value the last write replaced over the newer one                       |
//! | `c`  | `commit_zcash_event` at the durable revision, first event                         |
//! | `d`  | `commit_zcash_event` at the durable revision, second event                        |
//! | `p`  | `commit_zcash_event` at the revision before the durable one, first event          |
//! | `q`  | `commit_zcash_event` at the revision before the durable one, second event         |
//! | `n`  | `commit_zcash_event` at the revision after the durable one, first event           |
//! | `m`  | `save_with_zcash_binding` with a binding other than the durable one               |
//!
//! Opening the store is by far the most expensive step, so an input reopens it at most
//! `MAX_RESTARTS` times: after that `r` only saves and `x` leaves the change in memory. Every
//! input ends with one more reopen, after which the whole Zcash event journal is read back.

use std::{
    fs,
    os::unix::fs::DirBuilderExt as _,
    path::{Path, PathBuf},
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

use lez_swap_core::{Chain, Pair, Participant, SwapCoordinator, SwapDirection, SwapId};
use lez_swap_store::{SqliteSwapStore, StoreError};
use lez_zec_swap_sdk::{ZcashObservationEventRecordV1, ZecSwapBinding, ZecSwapBindingRecordV1};

use super::coordinator_model::{Invariants, apply_action, coordinator, profile_from_byte};

/// Swap identifiers in the order `list_swaps` returns them.
const SWAP_IDS: [&str; 2] = ["coordinator-store-fuzz-a", "coordinator-store-fuzz-b"];
/// Operations read from one input; `scripts/run-fuzz.sh` sets `-max_len` to match.
const MAX_OPERATIONS: usize = 32;
/// Reopens one input may ask for before the closing one.
const MAX_RESTARTS: usize = 8;
const DATABASE_FILE: &str = "swaps.sqlite3";

/// Replays one fuzz input against a fresh store in its own temporary directory.
pub fn run(data: &[u8]) {
    let [first, second, operations @ ..] = data else {
        return;
    };
    let scratch = Scratch::create();
    let mut harness = Harness::open(&scratch, [*first, *second]);
    for operation in operations.chunks_exact(2).take(MAX_OPERATIONS) {
        harness.step(operation[0], operation[1]);
    }
    harness.restart();
    harness.check_event_journals();
}

/// What follows the action of one operation.
#[derive(Clone, Copy, Debug)]
enum Persist {
    Keep,
    Save,
    SaveAndRestart,
    Crash,
    SaveSuperseded,
    Commit { revision: Revision, event: usize },
    SaveWithOtherBinding,
}

/// The expected revision a commit passes, relative to the durable one.
#[derive(Clone, Copy, Debug)]
enum Revision {
    Durable,
    Predecessor,
    Successor,
}

fn persist_from_byte(byte: u8) -> Persist {
    let commit = |revision, event| Persist::Commit { revision, event };
    let choice = match byte | 0x20 {
        b'k' => 0,
        b's' => 1,
        b'r' => 2,
        b'x' => 3,
        b'o' => 4,
        b'c' => 5,
        b'd' => 6,
        b'p' => 7,
        b'q' => 8,
        b'n' => 9,
        b'm' => 10,
        _ => (byte & 0x1f) % 11,
    };
    match choice {
        0 => Persist::Keep,
        1 => Persist::Save,
        2 => Persist::SaveAndRestart,
        3 => Persist::Crash,
        4 => Persist::SaveSuperseded,
        5 => commit(Revision::Durable, 0),
        6 => commit(Revision::Durable, 1),
        7 => commit(Revision::Predecessor, 0),
        8 => commit(Revision::Predecessor, 1),
        9 => commit(Revision::Successor, 0),
        _ => Persist::SaveWithOtherBinding,
    }
}

/// Zcash records the store validates on the revision-checked path. They are the values of
/// `crates/swap-store/tests/zec_event_journal.rs`; `crates/swap-store/tests/fuzz_corpus_replay.rs`
/// rebuilds them and compares.
struct ZcashFixtures {
    binding: ZecSwapBinding,
    other_binding: ZecSwapBinding,
    events: [ZcashObservationEventRecordV1; 2],
}

fn zcash_fixtures() -> &'static ZcashFixtures {
    static FIXTURES: OnceLock<ZcashFixtures> = OnceLock::new();
    FIXTURES.get_or_init(|| {
        let binding = |json: &str| {
            serde_json::from_str::<ZecSwapBindingRecordV1>(json)
                .expect("binding fixture must decode")
                .validate()
                .expect("binding fixture must be valid")
        };
        let event = |json: &str| {
            let record: ZcashObservationEventRecordV1 =
                serde_json::from_str(json).expect("event fixture must decode");
            record.validate().expect("event fixture must be valid");
            record
        };
        ZcashFixtures {
            binding: binding(include_str!(
                "../fixtures/coordinator_store/zec-binding.json"
            )),
            other_binding: binding(include_str!(
                "../fixtures/coordinator_store/zec-binding-other.json"
            )),
            events: [
                event(include_str!(
                    "../fixtures/coordinator_store/zec-event-tip-102.json"
                )),
                event(include_str!(
                    "../fixtures/coordinator_store/zec-event-tip-103.json"
                )),
            ],
        }
    })
}

/// One swap: the coordinator that passes through the store, its never-persisted twin, and the
/// model of what the store holds for it.
struct Slot {
    id: SwapId,
    pair: Pair,
    direction: SwapDirection,
    /// The side that funds the Zcash leg, as the Maker Node derives it from the direction.
    zcash_funder: Participant,
    /// Replaced by the loaded value at every restart.
    memory: SwapCoordinator,
    /// Never serialised and never read from the store.
    twin: SwapCoordinator,
    invariants: Invariants,
    /// The value the store must return, taken from the twin at each successful write.
    durable: Option<SwapCoordinator>,
    /// The durable value the last write replaced.
    superseded: Option<SwapCoordinator>,
    bound: bool,
    revision: u64,
    /// The event committed at each revision, as an index into the fixtures.
    events: Vec<usize>,
}

impl Slot {
    fn new(id: &str, profile: u8) -> Self {
        let (pair, direction) = profile_from_byte(profile);
        let memory = coordinator(id, pair, direction);
        let zcash_funder = if memory.funded_chain(Participant::Taker) == Chain::Zcash {
            Participant::Taker
        } else {
            Participant::Maker
        };
        Self {
            id: memory.id().clone(),
            pair,
            direction,
            zcash_funder,
            invariants: Invariants::new(&memory),
            twin: memory.clone(),
            memory,
            durable: None,
            superseded: None,
            bound: false,
            revision: 0,
            events: Vec::new(),
        }
    }

    /// Records a write that made the twin's state durable.
    fn record_write(&mut self) {
        let replaced = self.durable.replace(self.twin.clone());
        if replaced != self.durable {
            self.superseded = replaced.or(self.superseded.take());
        }
    }
}

struct Harness {
    database: PathBuf,
    store: Option<SqliteSwapStore>,
    slots: [Slot; 2],
    restarts_left: usize,
}

impl Harness {
    fn open(scratch: &Scratch, profiles: [u8; 2]) -> Self {
        let database = scratch.directory.join(DATABASE_FILE);
        let harness = Self {
            store: Some(open_store(&database)),
            database,
            slots: [
                Slot::new(SWAP_IDS[0], profiles[0]),
                Slot::new(SWAP_IDS[1], profiles[1]),
            ],
            restarts_left: MAX_RESTARTS,
        };
        harness.check_store();
        harness
    }

    fn store(&mut self) -> &mut SqliteSwapStore {
        self.store
            .as_mut()
            .expect("the store is open between restarts")
    }

    fn step(&mut self, control: u8, action: u8) {
        let index = usize::from(control & 0x20 != 0);
        self.act(index, action);
        match persist_from_byte(control) {
            Persist::Keep => {}
            Persist::Save => self.save(index),
            Persist::SaveAndRestart => {
                self.save(index);
                if self.take_restart() {
                    let saved = self.slots[index].twin.clone();
                    self.restart();
                    assert_eq!(
                        self.slots[index].memory, saved,
                        "the coordinator loaded after save and reopen is not the one in memory"
                    );
                }
            }
            Persist::Crash => {
                if self.take_restart() {
                    self.restart();
                }
            }
            Persist::SaveSuperseded => self.save_superseded(index),
            Persist::Commit { revision, event } => self.commit(index, revision, event),
            Persist::SaveWithOtherBinding => self.save_with_other_binding(index),
        }
        self.check_store();
    }

    fn take_restart(&mut self) -> bool {
        let allowed = self.restarts_left > 0;
        self.restarts_left = self.restarts_left.saturating_sub(1);
        allowed
    }

    /// Applies one action to the coordinator that came through the store and to its twin.
    fn act(&mut self, index: usize, action: u8) {
        let slot = &mut self.slots[index];
        let before = slot.memory.clone();
        let result = apply_action(&mut slot.memory, action);
        let twin_result = apply_action(&mut slot.twin, action);
        assert_eq!(
            result, twin_result,
            "an action on the stored coordinator and on its never-persisted twin differ in outcome"
        );
        assert_eq!(
            slot.memory, slot.twin,
            "the stored coordinator and its never-persisted twin diverged"
        );
        slot.invariants
            .check_step(&before, result.is_err(), &slot.memory);
    }

    fn save(&mut self, index: usize) {
        let store = self
            .store
            .as_mut()
            .expect("the store is open between restarts");
        let slot = &mut self.slots[index];
        if slot.pair == Pair::Zcash && !slot.bound {
            store
                .save_with_zcash_binding(&slot.memory, &zcash_fixtures().binding)
                .expect("saving a swap with its first Zcash binding must succeed");
            slot.bound = true;
        } else {
            store.save(&slot.memory).expect("save must succeed");
        }
        slot.record_write();
    }

    /// `save` replaces the aggregate whatever it holds: an older value over a newer one becomes
    /// the durable value, and the revision stays (`check_store` reads both back).
    fn save_superseded(&mut self, index: usize) {
        let Some(older) = self.slots[index].superseded.take() else {
            return;
        };
        self.store()
            .save(&older)
            .expect("save of an older value must succeed");
        let slot = &mut self.slots[index];
        slot.superseded = slot.durable.replace(older);
    }

    /// `commit_zcash_event` writes the aggregate only at the durable revision. The exact event
    /// already durable at that slot is a replay that writes nothing; any other revision is
    /// stale; a swap without a binding is refused.
    fn commit(&mut self, index: usize, revision: Revision, event: usize) {
        let slot = &self.slots[index];
        let expected = match revision {
            Revision::Durable => slot.revision,
            Revision::Predecessor => slot.revision.saturating_sub(1),
            Revision::Successor => slot.revision + 1,
        };
        let committed_at_slot = usize::try_from(expected)
            .ok()
            .and_then(|slot_index| slot.events.get(slot_index).copied());
        let (swap, funded_by, actual, bound) = (
            slot.memory.clone(),
            slot.zcash_funder,
            slot.revision,
            slot.bound,
        );
        let result = self.store().commit_zcash_event(
            expected,
            &swap,
            funded_by,
            &zcash_fixtures().events[event],
        );
        let slot = &mut self.slots[index];
        if !bound {
            assert!(
                matches!(result, Err(StoreError::MissingZcashBinding)),
                "commit without a Zcash binding: {result:?}"
            );
        } else if committed_at_slot == Some(event) {
            let commit = result.expect("replay of the durable event must succeed");
            assert!(commit.was_replay(), "durable event was committed twice");
            assert_eq!(commit.revision(), expected + 1);
        } else if expected == actual {
            let commit = result.expect("commit at the durable revision must succeed");
            assert!(!commit.was_replay(), "new event was reported as a replay");
            assert_eq!(commit.revision(), actual + 1);
            slot.revision += 1;
            slot.events.push(event);
            slot.record_write();
        } else {
            assert!(
                matches!(
                    result,
                    Err(StoreError::StaleRevision { expected: e, actual: a })
                        if e == expected && a == actual
                ),
                "commit at revision {expected} over durable revision {actual}: {result:?}"
            );
        }

        // The probe the Maker Node asks before it reapplies an event: the slot after
        // `expected` holds this event exactly when the model says it was committed there.
        let durable_at_slot = usize::try_from(expected)
            .ok()
            .and_then(|slot_index| slot.events.get(slot_index).copied())
            == Some(event);
        let id = slot.id.clone();
        let probe = self.store().committed_zcash_event(
            expected,
            &id,
            funded_by,
            &zcash_fixtures().events[event],
        );
        if bound {
            let probe = probe.expect("probe of a bound swap must succeed");
            assert_eq!(
                probe.map(|commit| (commit.revision(), commit.was_replay())),
                durable_at_slot.then_some((expected + 1, true)),
                "probe of the slot after revision {expected} disagrees with the commits made"
            );
        } else {
            assert!(
                matches!(probe, Err(StoreError::MissingZcashBinding)),
                "probe without a Zcash binding: {probe:?}"
            );
        }
    }

    /// A changed binding is refused and the swap written in the same transaction rolls back.
    fn save_with_other_binding(&mut self, index: usize) {
        if !self.slots[index].bound {
            return;
        }
        let swap = self.slots[index].memory.clone();
        let result = self
            .store()
            .save_with_zcash_binding(&swap, &zcash_fixtures().other_binding);
        assert!(
            matches!(result, Err(StoreError::ImmutableZcashBindingMismatch)),
            "changed Zcash binding: {result:?}"
        );
    }

    /// Drops the store and opens the file again; each coordinator continues from what the
    /// store returns, and its twin from the model of the durable value.
    fn restart(&mut self) {
        drop(self.store.take());
        let store = open_store(&self.database);
        for slot in &mut self.slots {
            let loaded = store
                .load(&slot.id)
                .expect("load after reopen must succeed");
            assert_eq!(
                loaded, slot.durable,
                "reopened store does not hold the last written coordinator"
            );
            let fresh = || coordinator(slot.id.as_str(), slot.pair, slot.direction);
            slot.memory = loaded.unwrap_or_else(fresh);
            slot.twin = slot.durable.clone().unwrap_or_else(fresh);
            slot.invariants = Invariants::new(&slot.twin);
        }
        self.store = Some(store);
        self.check_store();
    }

    /// Reads everything but the event journals back: each swap alone, the listings, revisions
    /// and bindings.
    fn check_store(&self) {
        let store = self
            .store
            .as_ref()
            .expect("the store is open between restarts");
        let fixtures = zcash_fixtures();
        for slot in &self.slots {
            assert_eq!(
                store.load(&slot.id).expect("load must succeed"),
                slot.durable,
                "stored coordinator differs from the last value written for this swap"
            );
            assert_eq!(
                store
                    .load_operator_swap(&slot.id)
                    .expect("operator load must succeed"),
                slot.durable,
                "operator view differs from the last value written for this swap"
            );
            assert_eq!(
                store.revision(&slot.id).expect("revision must be readable"),
                slot.durable.as_ref().map(|_| slot.revision),
                "durable revision differs from the commits made for this swap"
            );
            assert_eq!(
                store
                    .load_zcash_binding(&slot.id)
                    .expect("binding must be readable"),
                slot.bound.then(|| fixtures.binding.clone()),
                "durable Zcash binding changed"
            );
        }
        let saved: Vec<_> = self
            .slots
            .iter()
            .filter_map(|slot| slot.durable.clone())
            .collect();
        assert_eq!(
            store.list_swaps().expect("list must succeed"),
            saved,
            "list_swaps is not exactly the saved swaps"
        );
        assert_eq!(
            store
                .list_operator_swaps()
                .expect("operator list must succeed"),
            saved,
            "list_operator_swaps is not exactly the saved swaps"
        );
    }

    /// Reads both roles' event journals of each swap back. The store revalidates every event
    /// it returns, so this runs once per input, after the closing reopen.
    fn check_event_journals(&self) {
        let store = self
            .store
            .as_ref()
            .expect("the store is open between restarts");
        let fixtures = zcash_fixtures();
        for slot in &self.slots {
            let committed: Vec<_> = slot
                .events
                .iter()
                .map(|&event| fixtures.events[event].clone())
                .collect();
            assert_eq!(
                store
                    .load_zcash_events(&slot.id, slot.zcash_funder)
                    .expect("events must be readable"),
                committed,
                "event journal differs from the commits made for this swap"
            );
            assert!(
                store
                    .load_zcash_events(&slot.id, slot.zcash_funder.other())
                    .expect("events must be readable")
                    .is_empty(),
                "event journal of the other role is not empty"
            );
        }
    }
}

fn open_store(database: &Path) -> SqliteSwapStore {
    SqliteSwapStore::open(database).expect("the store must open")
}

/// A private directory for one input, on tmpfs when the host has `/dev/shm`; removed on drop.
struct Scratch {
    directory: PathBuf,
}

impl Scratch {
    fn create() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let name = format!(
            "lez-coordinator-store-fuzz-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let create = |base: &Path| {
            let directory = base.join(&name);
            // A crashed process with the same id may have left its directory behind.
            let _ = fs::remove_dir_all(&directory);
            fs::DirBuilder::new()
                .mode(0o700)
                .create(&directory)
                .map(|()| directory)
        };
        let directory = create(Path::new("/dev/shm"))
            .or_else(|_| create(&std::env::temp_dir()))
            .expect("a temporary directory must be creatable");
        Self { directory }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}
