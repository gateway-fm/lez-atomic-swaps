# The swap coordinator: state machine, persistence, crash recovery, isolation

This page is the map of the swap-coordinator state machine: what it is, where each part of a
swap's state is stored, how a restarted Node continues a swap, and how swaps running at the same
time are kept apart. It covers requirements R4 ("Persisted state survives crash/restart without
fund loss") and R5 ("Concurrent swaps have independent state, escrow, and deadlines") of
[`requirements-traceability.md`](requirements-traceability.md). Everything here is what the code
in this tree does; each statement names the crate, type, test or scenario it comes from. The
decisions behind it are in the ADRs under [`architecture/`](architecture/), linked where they
apply.

Verified by reading the code and the tests named below. No test or devnet scenario was run for
this page; the recorded runs are in `requirements-traceability.md`, rows R4 and R5.

| Crate | Path | What it holds |
| --- | --- | --- |
| `lez-swap-core` | [`crates/swap-core`](../crates/swap-core) | `SwapCoordinator`, the state machine of one swap, with its phases, typed deadlines and errors. No I/O. |
| `lez-swap-store` | [`crates/swap-store`](../crates/swap-store) | `SqliteSwapStore` and the journals: every durable record a Node keeps for a swap. |

## 1. The state machine

`lez_swap_core::SwapCoordinator` ([`crates/swap-core/src/lib.rs`](../crates/swap-core/src/lib.rs))
is the state of one swap. It is created from terms that never change afterwards: a `SwapId`, a
`Pair` (`Bitcoin`, `Monero`, `Zcash`), a `SwapDirection` (`TakerSellsForeign`, `TakerSellsLez`),
one `ConfirmationPolicy` per funded leg and a `RecoverySchedule`. It does no I/O and holds no
secret: a caller reports what it observed on a chain, and the coordinator accepts the observation
and moves, or refuses it with an `Error` and stays as it was.

### Phases

`Phase` has thirteen values.

| Phase | Meaning |
| --- | --- |
| `Offered` | Terms are fixed; neither leg has a known lock transaction. |
| `AwaitingTakerConfirmations` | The Taker's lock is seen, below its confirmation policy. |
| `TakerLockConfirmed` | The Taker's lock meets its policy; the Maker may lock. |
| `AwaitingMakerConfirmations` | The Maker's lock is seen, below its own policy. |
| `BothLegsLocked` | Both legs are locked. |
| `TakerLockReorged` | The Taker's funding fell below policy, or left the chain, after the Maker locked. |
| `MakerLockReorged` | The Maker's committed funding left the chain or fell below policy. |
| `ClaimEvidenceAvailable` | The first claimant's claim is recorded, with a commitment to the secret it revealed. |
| `Completed` | Both parties claimed. Terminal. |
| `MakerLegRefunded` | The Maker-funded leg is refunded; the Taker-funded leg is not resolved. |
| `TakerLegRefunded` | The Taker-funded leg is refunded; the Maker-funded leg is not resolved. |
| `MakerRecoveryAvailable` | A canonical Taker refund gave the Maker what it needs to recover (the Monero profile). |
| `Refunded` | Both parties have their own funds back. Terminal. |

The phase is one variable for two legs, so the coordinator also keeps, per leg, whether its
funding meets its policy right now (`taker_funding_confirmed`, `maker_funding_confirmed`), the
transaction id pinned to each leg, the `ClaimEvidence`, and the ids of the revealing claim, the
follow-up claim, the Taker refund event and the Maker recovery. `ClaimEvidence` keeps the SHA-256
of the revealed 32 bytes, never the bytes
(`serialized_claim_evidence_never_contains_the_plaintext_preimage`,
[`tests/claim_secret_at_rest.rs`](../crates/swap-core/tests/claim_secret_at_rest.rs)).

### Events

Each event is one method of `SwapCoordinator`.

| Method | What it records | Effect on the phase |
| --- | --- | --- |
| `observe_taker_lock`, `observe_funding(Participant::Taker, …)` | The Taker's lock with its depth | `Offered` to `AwaitingTakerConfirmations` or `TakerLockConfirmed`; a depth below policy after the Maker locked gives `TakerLockReorged`, and the same transaction back at policy restores the earlier phase |
| `observe_maker_lock`, `observe_funding(Participant::Maker, …)` | The Maker's lock with its depth | `TakerLockConfirmed` to `AwaitingMakerConfirmations` or `BothLegsLocked`; before that, `Error::TakerLockNotConfirmed` |
| `observe_taker_lock_removed`, `observe_maker_lock_removed`, `observe_funding_removed` | The pinned funding transaction left the canonical chain | Before the Maker funded, the Taker's observation is cleared (`Offered`); afterwards the id stays pinned and the phase is `TakerLockReorged` or `MakerLockReorged` |
| `observe_revealing_claim` | The claim of `first_claimant()` and its `ClaimEvidence` | `BothLegsLocked` to `ClaimEvidenceAvailable` |
| `observe_followup_claim` | The other party's claim | `ClaimEvidenceAvailable` to `Completed` |
| `refund_maker_leg`, `refund_taker_leg` | A refund at a typed `ChainPosition` | To `MakerLegRefunded`, `TakerLegRefunded` or `Refunded`; `Error::TimelockNotExpired` before the deadline |
| `refund_maker_leg_after_lost_followup` | ZEC `TakerSellsLez` only: the Maker's refund after its follow-up claim was lost and the Taker refunded | `ClaimEvidenceAvailable` to `Refunded` |
| `observe_taker_refund_for_maker_recovery`, `observe_maker_recovery` | The Monero profile's event-gated recovery ([ADR 0011](architecture/0011-event-gated-recovery.md)) | `TakerLegRefunded` to `MakerRecoveryAvailable` to `Refunded` |

`first_claimant()` is the Taker for Bitcoin and Zcash and the Maker for Monero. A second lock
transaction for a leg that already has one is refused (`Error::ConflictingTakerLock`,
`Error::ConflictingMakerLock`), as is different claim evidence
(`Error::ConflictingClaimEvidence`). Repeating an observation already recorded is accepted and
changes nothing (`replayed_chain_observations_are_idempotent`,
[`tests/e2e_swap_lifecycle.rs`](../crates/swap-core/tests/e2e_swap_lifecycle.rs)). A deadline is
a `ChainPosition` with its chain and clock basis, and an observation on another chain or basis
is `Error::WrongDeadlineClock`
([ADR 0010](architecture/0010-typed-cross-chain-deadlines.md)).

### Invariants the property tests assert

[`crates/swap-core/tests/transition_properties.rs`](../crates/swap-core/tests/transition_properties.rs)
holds two `proptest` properties over the Bitcoin and Zcash profiles in both directions.

`arbitrary_event_sequences_preserve_atomicity_and_absorbing_terminal_states` (512 cases, up to 64
events each: locks at depths 0 to 4, removals, both claims, both refunds, with alternate
transaction ids) asserts:

- a claim is accepted only while both legs are funded at policy, as tracked by a model that
  follows the accepted events and not the phase;
- once the phase is `Completed` or `Refunded` it never changes;
- every phase change comes from one of the phases allowed to precede it (for example
  `Completed` only from `ClaimEvidenceAvailable`, `Refunded` only from
  `AwaitingTakerConfirmations`, `TakerLockConfirmed`, `MakerLegRefunded` or `TakerLegRefunded`);
- `Completed` is never reached without claim evidence, and `MakerRecoveryAvailable` is never
  reached in these profiles.

`no_refund_after_reveal_and_no_claim_below_maker_policy` (4096 cases; the Maker leg has a policy
of two, so it can sit below policy while the Taker leg is the reason for the phase) asserts:

- a refused event leaves the coordinator equal to what it was;
- a claim is admitted only while both legs are at their own policy depth;
- after a revealing claim is recorded no refund is admitted, so no reachable state is refunded
  with claim evidence set;
- a settled coordinator is unchanged by any later event, accepted or not.

The failing cases found so far are kept in `transition_properties.proptest-regressions` beside
the test. `coordinator_fuzz_corpus_replays_on_stable`
([`tests/fuzz_corpus_replay.rs`](../crates/swap-core/tests/fuzz_corpus_replay.rs)) replays every
seed of `fuzz/corpus/coordinator` through the fuzz target's harness in the ordinary test run.
The example tests beside them pin single cases:
[`tests/reorg_after_reveal.rs`](../crates/swap-core/tests/reorg_after_reveal.rs) (for example
`maker_lock_removal_after_revealing_claim_admits_no_refund`),
[`tests/xmr_event_gated_recovery.rs`](../crates/swap-core/tests/xmr_event_gated_recovery.rs)
(`maker_recovers_xmr_only_after_canonical_lez_refund_event`) and
[`tests/zec_contract_ordering.rs`](../crates/swap-core/tests/zec_contract_ordering.rs).

## 2. Persistence

The decision for SQLite through `rusqlite` is
[ADR 0003](architecture/0003-sqlite-persistence.md). All stores are in
[`crates/swap-store/src/`](../crates/swap-store/src).

### `SqliteSwapStore` and the common configuration

`lez_swap_store::SqliteSwapStore` ([`lib.rs`](../crates/swap-store/src/lib.rs)) stores one
`SwapCoordinator` per row of the `swaps` table (`id` primary key, `schema_version`,
`state_json`, `revision`). `save` replaces one aggregate atomically; `load` and `list_swaps`
refuse a row whose payload version is not the supported one
(`StoreError::UnsupportedPayloadVersion`). Zcash observations are committed with the aggregate
and its revision in one transaction (`commit_zcash_event`, `commit_zcash_transition`; table
`chain_events`), with the immutable `zcash_swap_bindings` row and the `operator_alert_outbox`.

Every database opened through `open_configured_connection` (the swap store and the journals
marked "common" below) gets the same treatment:

- The file must be one owner-private regular file (mode 0600, one link); a symlink at the final
  path component is refused, and the file's device and inode are compared before and after the
  open (`StoreError::UnsafeDatabaseFile`). The code comment in `open_configured_connection_with_mode`
  states the limit: SQLite opens its `-wal` and `-shm` files by path, so the parent directory
  must be private to the owner.
- `journal_mode = WAL`, `synchronous = FULL`, `foreign_keys = ON`, `secure_delete = ON`, and a
  busy timeout of 5 s.
- `migrate` runs in one `IMMEDIATE` transaction and then the WAL is checkpointed with
  `wal_checkpoint(TRUNCATE)`; a checkpoint that a reader blocks fails the open
  (`StoreError::LegacyMigrationCheckpointBusy`).

### Schema versions and migration

The schema version is SQLite's `user_version`; `DATABASE_SCHEMA_VERSION` is 22. `migrate`
creates what is missing (`CREATE TABLE IF NOT EXISTS`), alters or rebuilds tables an older
version created, and writes the current version last, all in the one transaction, so a database
is either migrated completely or left as it was. A database whose version is higher than the
binary's is refused (`StoreError::UnsupportedDatabaseVersion`). Below version 10
(`LEGACY_CLAIM_MIGRATION_VERSION`) a stored plaintext claim preimage is replaced by its SHA-256
commitment (`migrate_legacy_claim_evidence`); `secure_delete` and the truncating checkpoint are
what remove the old bytes from the file and the WAL.

| Test | File | What it pins |
| --- | --- | --- |
| `schema_v8_plaintext_claim_evidence_migrates_to_current_schema_and_is_scrubbed` | [`tests/restart_recovery.rs`](../crates/swap-store/tests/restart_recovery.rs) | A version-8 database migrates, and the preimage is gone from the files |
| `legacy_v1_table_migrates_and_future_versions_fail_explicitly` | [`tests/zec_event_journal.rs`](../crates/swap-store/tests/zec_event_journal.rs) | The first schema migrates; a newer one is refused |
| `schema_20_actor_kind_checks_widen_to_monero_without_losing_existing_rows` | [`tests/maker_actor_process.rs`](../crates/swap-store/tests/maker_actor_process.rs) | A version-20 actor table is rebuilt with its rows |
| `legacy_acceptance_table_migrates_with_exact_evidence_and_null_asset_columns`, `old_happy_path_rows_migrate_without_changing_exact_payloads` | [`tests/btc_recovery.rs`](../crates/swap-store/tests/btc_recovery.rs) | Older Bitcoin actor stores migrate with their evidence unchanged |
| `a_journal_from_before_the_supersede_operations_is_rebuilt_on_open`, `a_journal_from_before_the_claim_replans_is_rebuilt_on_open` | [`tests/bridge_operation_journal.rs`](../crates/swap-store/tests/bridge_operation_journal.rs) | An older bridge journal is rebuilt on open |

Three stores are standalone files with their own `application_id` and `user_version` and do not
use the common migration: the XMR workflow journal (schema 3; a version-2 file is read as it is,
`exact_schema_v2_remains_readable_and_unmigrated`), the Taker facade registry (schema 1) and the
Taker actor schedule (schema 1). Each refuses a file with another application id or a newer
schema, and each sets WAL and `synchronous = FULL`.

### The journals

| Module | Type | Tables | Open | What it records |
| --- | --- | --- | --- | --- |
| [`btc_recovery.rs`](../crates/swap-store/src/btc_recovery.rs) | `SqliteBtcRecoveryStore` | `btc_actor_agreements`, `btc_actor_aggregates`, `btc_actor_evidence` | common | One Bitcoin swap of one role: the countersigned agreement, and one evidence row per revision 1 to 4 (`taker_lock`, `maker_lock`, `revealing_claim`, `followup_claim`, `maker_refund`, `taker_refund`) under a hash chain |
| [`btc_maker_lock_journal.rs`](../crates/swap-store/src/btc_maker_lock_journal.rs) | `SqliteBtcMakerLockJournal` | `btc_maker_lock_intents`, `btc_maker_lock_steps` | common | The Maker's second lock of a Bitcoin swap: the exact bytes of each ordered step and its one send authority |
| [`zec_recovery.rs`](../crates/swap-store/src/zec_recovery.rs) | `SqliteZecRecoveryStore` | the `zec_sdk_*` tables (agreements, first-lock, maker-lock, claim and refund intents and transitions) | common | One role's Zcash swaps as the ZEC SDK's recovery port: an intent before each effect, a transition after its evidence. `open_claim_capable` takes a `ProtectedClaimKey` that stays in process memory; claim material is stored as an authenticated envelope |
| [`xmr_effect_workflow_journal.rs`](../crates/swap-store/src/xmr_effect_workflow_journal.rs) | `SqliteXmrWorkflowJournal` | `xmr_workflow_identity`, `xmr_workflow_steps` | standalone | One Monero swap of one role: its identity, the selected `XmrWorkflowBranch` (`Claim`, `Refund`, `Punish`) and one row per `XmrWorkflowStep` with its one invocation authority |
| [`public_effect_journal.rs`](../crates/swap-store/src/public_effect_journal.rs) | `SqlitePublicEffectJournal` | `public_effect_journal` | common | A complete public Bitcoin or LEZ transaction of a Bitcoin swap, stored before it is sent, with its one send authority |
| [`adaptor_session_journal.rs`](../crates/swap-store/src/adaptor_session_journal.rs) | `SqliteAdaptorSessionJournal` | `adaptor_sessions` | common | One MuSig2 adaptor signing session per row: transcript, the one-use secret nonce until it is consumed, the partial signatures and the presignature |
| [`bridge_operation_journal.rs`](../crates/swap-store/src/bridge_operation_journal.rs) | `SqliteBridgeOperationJournal` | `bridge_operation_contexts` | common | The request id and scan window of each request to the LEZ sidecar, per `BridgeOperationKind`, so an ambiguous request is resumed as the same request |
| [`maker_actor_process.rs`](../crates/swap-store/src/maker_actor_process.rs) | methods of `SqliteSwapStore`; `MakerActorLeaseV1`, `ActorHeldLock` | `maker_actor_processes`, `maker_actor_manual_actions`, `maker_actor_progress` | in the swap store | The Maker supervisor's schedule: one row per accepted swap (`MakerActorScheduleState`: `Queued`, `Leased`, `Backoff`, `Terminal`, `Failed`), its lease and its last progress |
| [`taker_facade_registry.rs`](../crates/swap-store/src/taker_facade_registry.rs) | `SqliteTakerFacadeStore` | `taker_facade_swaps`, `taker_facade_authorities`, `taker_facade_requests` | standalone | The Taker Node's admissions: one row per taken swap, and one per owner request (`initiate`, `claim`, `refund`) with its result |
| [`taker_actor_schedule.rs`](../crates/swap-store/src/taker_actor_schedule.rs) | `SqliteTakerActorSchedule`, `TakerActorLeaseV1` | `taker_actor_schedule` | standalone | The Taker supervisor's schedule for its Zcash actors, in the Maker schedule's shape |

Two more modules of the same crate hold the Maker's application state in the swap store and are
outside this page: [`maker_application.rs`](../crates/swap-store/src/maker_application.rs)
(routes and local prices) and [`maker_offer.rs`](../crates/swap-store/src/maker_offer.rs)
(offers and the negotiations that consume them).

Each swap has its own directory on each Node, with its own actor store and signing journals;
the layout is in
[`architecture/deployment-components-and-rpcs.md`](architecture/deployment-components-and-rpcs.md).

### The adaptor nonce at rest

A MuSig2 secret nonce must be used once. `SqliteAdaptorSessionJournal` stores it when the
session is reserved and removes it in the transaction that stores the partial signature made
with it (`AdaptorSessionPhase::PartialPersisted` has no nonce), so a restart replays the stored
partial and never signs again. A fingerprint of every nonce stays unique across sessions.

While it is stored, the nonce is sealed: XChaCha20-Poly1305 under an `AdaptorNonceSealingKey`
derived with HKDF-SHA256 from the role's signing secret, with the session id as associated
data. The module's own statement of what this does and does not protect:

- It protects a copy of the database and its WAL taken without the signing key file (a backup,
  a disk image of the state directory alone).
- It does not protect against a reader of the signing key file: that reader holds the signing
  key already and can derive the sealing key.
- A plaintext record written by older code is still read and signed; a journal file whose
  schema admits only plaintext refuses new reservations.

| Test ([`tests/adaptor_session_journal.rs`](../crates/swap-store/tests/adaptor_session_journal.rs)) | What it pins |
| --- | --- |
| `sealed_nonce_round_trips_and_never_reaches_the_database_or_wal_in_clear` | The nonce bytes are in neither file |
| `wrong_key_or_tampered_sealed_nonce_fails_closed_without_a_new_nonce` | Another key or altered bytes stop the session; no new nonce is made |
| `plaintext_record_from_older_code_is_read_and_its_file_refuses_new_reservations` | The compatibility rule above |
| `restart_recovers_exact_transcript_and_never_signs_a_persisted_partial_again` | Reopen replays the stored partial |
| `secret_nonce_fingerprint_is_one_use_across_session_ids` | One nonce cannot serve two sessions |
| `existing_only_open_never_creates_a_missing_signer_database` | A missing journal is an error, not an empty journal |

## 3. Recovery after a restart

No Node keeps a swap's progress in memory only. After a restart each kind of state is read back
as follows; the last column names the ADR, or the module, that has the argument and the limits.

| What was in flight | What is replayed from disk | What is observed from the chain | Where it is argued |
| --- | --- | --- | --- |
| The phase of a Bitcoin swap | `SqliteBtcRecoveryStore::open` derives a fresh coordinator from the agreement and applies the evidence rows in order; the stored snapshot is checked against the result and is never the authority. Missing or out-of-order evidence fails the open | A new revision is written only from exact chain evidence of the effect, never from a node's acceptance of a submission | [ADR 0031](architecture/0031-one-shot-btc-actor-observe-before-project.md), [ADR 0046](architecture/0046-replay-btc-sdk-lifecycle-from-exact-transitions.md) |
| The phase of a Zcash swap | `SqliteZecRecoveryStore` reloads the agreement, intents and transitions and revalidates each against the agreement; a record that fails is an error, not a state | Lock, claim and refund observations of both chains | [ADR 0216](architecture/0216-zec-taker-reveals-first-in-both-directions.md), [SDK guide](sdk-lez-zec.md) |
| A step of a Monero swap | `SqliteXmrWorkflowJournal::authorize_once` answers `InvokeOnce` once per step; a step found started or unknown answers `ObserveOnly`, a reconciled one `Complete` | The finalized LEZ event or the Monero wallet transaction that proves the step (`XmrWorkflowReconciliationSource`) | [ADR 0011](architecture/0011-event-gated-recovery.md), [ADR 0217](architecture/0217-xmr-taker-sells-lez-deviates-from-the-common-flow.md) |
| A public Bitcoin or LEZ transaction of a Bitcoin swap | The exact bytes and the row's `PublicEffectState` | Whether those exact bytes are on chain (the rule below) | [ADR 0033](architecture/0033-persist-public-effects-before-submission.md), [ADR 0036](architecture/0036-prove-bounded-lez-claim-absence-before-first-send.md) |
| A request to the LEZ sidecar | `SqliteBridgeOperationJournal::resume_after_ambiguous` returns the same request id and window | The request is asked again as the same request | [ADR 0036](architecture/0036-prove-bounded-lez-claim-absence-before-first-send.md) |
| A signing session | The transcript and, once made, the partial signature; the nonce is gone with the partial | Nothing | section 2 |
| The follow-up claim after the counterparty's reveal | The claimant's own presignature and state | The canonical revealing claim, from which it takes the secret; the revealer need not be running | [ADR 0040](architecture/0040-continue-post-reveal-from-canonical-evidence.md) |
| A Maker actor that was running | `recover_abandoned_maker_actor` moves the lease to the new supervisor only while it holds the swap's `ActorHeldLock`, a kernel lock that is free only once the old process and its children have exited; elapsed time is never the signal | What the actor then observes | the comments of `recover_abandoned_maker_actor` and `ActorHeldLock` in [`maker_actor_process.rs`](../crates/swap-store/src/maker_actor_process.rs) |
| A Taker Zcash actor that was running | `requeue_leased_on_start` requeues every row left leased with the class `coordinator_restarted` and a new lease generation | What the actor then observes | the module comment of [`taker_actor_schedule.rs`](../crates/swap-store/src/taker_actor_schedule.rs) |
| An owner request to the Taker (take, claim, refund) | `SqliteTakerFacadeStore` returns the stored result for the same request id; a changed request under that id conflicts | Nothing | [ADR 0132](architecture/0132-persist-taker-initiation-admission-separately.md), [ADR 0137](architecture/0137-authorize-one-taker-terminal-action-before-effects.md) |

### One send per public effect, and no rearm

`SqlitePublicEffectJournal` separates having a transaction from being allowed to send it. The
order is: `record_prepared` stores the exact bytes (`Prepared`); the caller observes the chain;
`reconcile` compares the observation with the row. The states are `Prepared`, `Started`,
`Accepted`, `Rejected` and `Unknown`, and the rules of `reconcile` are:

- Only a row that is `Prepared` can give `PublicEffectDecision::SubmitOnce`, and it does so by
  committing `Started` first. Of several processes that ask, one gets it
  (`competing_processes_receive_exactly_one_submit_once_decision`).
- What permits the send depends on the operation. A funding or a claim needs
  `PublicEffectObservation::Absent`, which for a LEZ claim is the bounded, finalized `NotFound`
  of ADR 0036; a LEZ claim may instead be admitted by
  `ExactIdempotentLezClaimSubmissionSafe`, which does not assert absence. A refund needs
  `EligibleToAttempt`.
- A row that is `Started` or `Unknown` is never rearmed by an observation: `reconcile` answers
  `ObserveOnly`. This is the state a crash between the authorization and the recorded result
  leaves.
- `Uncertain` (a timeout, a moving tip, missing history) changes nothing and is always
  `ObserveOnly`.
- `ConflictingPresence` (the chain shows the effect with other bytes) burns a still-fresh
  authority without a send.
- `PresentExact` moves `Prepared`, `Started` or `Unknown` to `Accepted`.

`release_not_attempted` is the one way back from `Started` to `Prepared`: it is for the caller
that holds the swap's actor lock and whose single call was refused before anything was offered
to the node. A `Started` row left by a crash does not qualify.

An accepted submission is not lifecycle evidence. The phase advances only when the effect is
observed on chain and its evidence is projected into the actor store, as ADR 0036 states.
`SqliteBtcMakerLockJournal` applies the same rule to each step of the Maker's second lock
(`started_or_unknown_step_never_rearms_after_restart`), and `SqliteXmrWorkflowJournal` to each
Monero workflow step.

These rules are about the journal row. What an actor may do for a step whose row is observe-only
is set by ADR 0033, ADR 0036 and, after a reveal, ADR 0040.

### State that cannot be read

A store that fails its checks is an error for its swap and is not rewritten: a Bitcoin actor
store with missing or reordered evidence
(`reopen_rejects_missing_or_out_of_order_evidence_instead_of_trusting_snapshot`), a public
effect row with a wrong digest (`malformed_digest_and_transition_shape_fail_closed_as_corrupt`),
a sealed nonce under another key, a protected Zcash claim payload that does not authenticate
(`corrupt_or_future_protected_claim_payloads_fail_closed_on_reopen`).

On the Taker Node a Zcash swap whose role state is not a readable database is answered by
`taker_swap_monitor_v1` with the error `-32010`, category `taker_monitor_unavailable`, and never
with a state. `taker_swap_list_v1` still lists the Node's other swaps and lists that swap as
`attention_required` with `attention_reason` `taker_monitor_unavailable`; if no swap of the Node
can be read, the list answers the error (`list_rows` in
[`crates/taker-node/src/taker_service/lifecycle.rs`](../crates/taker-node/src/taker_service/lifecycle.rs),
test `a_swap_whose_state_cannot_be_read_does_not_hide_the_others`).

## 4. Concurrent swaps

Swaps share nothing that one of them could change for another.

- **One aggregate per swap.** `swaps.id` is the primary key, and every journal row carries the
  swap id in its key: `PublicEffectKey` is swap, local role, chain, operation and predecessor
  revision; `BridgeOperationKey` is run, swap, role and operation; an adaptor session has its
  own 32-byte session id; an XMR workflow journal is one file for one swap and role, created
  exclusively (`create_new`). The same chain-native effect id cannot be attached to a second
  key (`one_chain_effect_id_cannot_be_crosswired_to_another_authority_key`).
- **One directory per swap.** Each swap's actor store and signing journals are files of its own
  under the Node's `swaps/<reservation id>/`, so a damaged file stops one swap
  (`zec-corrupt-state` below).
- **One actor per swap at a time.** `ActorHeldLock` is a non-cloneable proof that this process
  holds the swap's kernel lock; an actor command, an owner action and the monitor take it for
  the swap they touch and for no other (`pinned_executable_rejects_cross_swap_lock_composition`).
  The Maker's schedule row is claimed with an owner and a generation
  (`claim_maker_actor`, `MakerActorLeaseV1`), and a write from an older generation is refused;
  the Taker's schedule does the same with `TakerActorLeaseV1`
  (`TakerActorScheduleError::LeaseConflict`). Two supervisors can lease two different swaps at
  once (`competing_connections_can_claim_distinct_swaps_independently`).
- **Deadlines per swap.** The `RecoverySchedule` is part of each coordinator and is fixed at
  creation; no deadline is shared.

### The LEZ account nonce

The one thing two swaps of the same Node do share on chain is the role's LEZ owner account,
whose transactions are ordered by its nonce. [ADR 0041](architecture/0041-interleave-overlapping-swaps-with-exact-chain-barriers.md)
records the first overlapping run and states its limit: the two swaps had different LEZ
depositors, and that run established no scheduler for two swaps on one depositor's nonce
stream. What the Nodes do for swaps that share an account is in the later ADRs:

- **Maker, escrow preparation.** The sidecar reserves the depositor's next nonce pair when an
  escrow is prepared, so the Maker Node prepares one escrow at a time, for the oldest swap
  whose Taker has locked, and only while no other prepared escrow is in flight
  ([ADR 0213](architecture/0213-nodes-own-the-btc-lifecycle.md)).
- **Maker, LEZ claims.** The Maker's LEZ claims go one at a time on its one LEZ account, and a
  claim whose nonce was taken by another transaction of the account is planned again at the
  live nonce; a collision costs a re-plan and never funds
  ([ADR 0216](architecture/0216-zec-taker-reveals-first-in-both-directions.md)).
- **Maker, Monero punishment.** The punishment is planned again at the live nonce whenever
  another transaction of the owner took its own
  ([ADR 0217](architecture/0217-xmr-taker-sells-lez-deviates-from-the-common-flow.md)).
- **Taker, LEZ locks.** The Taker Node holds one LEZ escrow submission in flight:
  `LEZ_LOCK_GATE` in
  [`crates/taker-node/src/taker_service/btc_dynamic.rs`](../crates/taker-node/src/taker_service/btc_dynamic.rs)
  keeps the nonce the account must reach before the next escrow is prepared, and a lock that
  waits past the bound reports the dependency unavailable so the owner retries. The Bitcoin
  lock and the Monero lock are planned under this gate. A Zcash lock whose nonce pair was taken
  is planned again (`BridgeOperationKind::NativeEscrowSupersede1` to `NativeEscrowSupersede5`).
- **Refunds.** A LEZ refund is permissionless and carries no signer nonce
  ([ADR 0038](architecture/0038-durable-permissionless-lez-refund.md)), so it does not take
  part in this ordering.

A lane of the devnet has one Maker identity and one Taker identity, so the scenarios below run
several swaps of the same two parties; no scenario runs several Takers against one Maker.

## 5. Property, tests, scenarios

Crate tests are under `crates/`; `swap-store/tests/x.rs` stands for
`crates/swap-store/tests/x.rs`. Devnet scenarios are the names accepted by
[`deploy/scripts/node-e2e.py`](../deploy/scripts/node-e2e.py); scenarios without a prefix are
the Bitcoin ones, `zec-` and `xmr-` the Zcash and Monero ones. Which of them ran, on which
commit, is recorded in `requirements-traceability.md` and under `docs/evidence/`.

| Property | Crate tests | Devnet scenarios and what each asserts |
| --- | --- | --- |
| Transitions are atomic and terminal phases absorb | `swap-core/tests/transition_properties.rs`: `arbitrary_event_sequences_preserve_atomicity_and_absorbing_terminal_states`, `no_refund_after_reveal_and_no_claim_below_maker_policy`; `swap-core/tests/fuzz_corpus_replay.rs`: `coordinator_fuzz_corpus_replays_on_stable` | Every scenario ends with both Nodes at the same terminal state |
| A reopened store continues the swap | `swap-store/tests/restart_recovery.rs`: `maker_daemon_restarts_after_each_durable_step_and_taker_can_complete`, `xmr_event_gated_recovery_survives_each_restart`; `swap-store/tests/btc_recovery.rs`: `maker_and_taker_reconstruct_both_btc_directions_through_completed_revision_four`, `maker_and_taker_reconstruct_both_refund_directions_through_revision_four`; `swap-store/tests/zec_sdk_recovery.rs`: `first_lock_intent_transition_and_closed_recovery_survive_reopen`, `maker_second_lock_happy_path_survives_sqlite_reopen_in_both_directions` | `restart-taker`, `restart-maker`: the role's Node restarted (SIGTERM) between the lock and the claim; the swap completes. `zec-restart-taker`, `zec-restart-maker`: the role's Node restarted at one of its recovery points; the Nodes alone carry the swap to completed, and a Taker row left leased reads `coordinator_restarted` until its next attempt. `zec-activate-after-expiry`: the Maker Node stopped between the take and its actor's activation and started after the offer expired; the swap completes on both Nodes |
| A Node killed at any step settles the swap, and its one send authority per effect survives the kill | `swap-store/tests/public_effect_journal.rs`: `absent_observation_commits_started_before_granting_the_only_send_authorization`, `exact_present_bytes_reconcile_prepared_started_or_unknown_to_accepted`, `only_a_started_row_gives_its_authority_back_and_then_rearms_exactly_once`; `swap-store/tests/btc_maker_lock_journal.rs`: `started_or_unknown_step_never_rearms_after_restart`; `swap-store/tests/bridge_operation_journal.rs`: `ambiguous_prepare_and_submit_resume_exact_caller_context_after_restart`; `swap-store/tests/xmr_effect_workflow_v2.rs`: `started_and_unknown_reconcile_once_with_exact_persisted_v2_evidence` | `kill-taker`, `kill-maker`: the role's container killed with SIGKILL at each of `BTC_KILL_POINTS` (taken, first lock sent, first lock final, both legs locked, claim sent), a fresh swap per point, then twice on one refund swap; every swap settles and the balances agree with the state both Nodes report. `zec-kill-taker`, `zec-kill-maker`: the same at each of `ZEC_KILL_POINTS` (taken, first lock final, both legs locked, claim sent); each swap completes and its LEZ leg moves the lot. `zec-kill-maker-claim`: the Maker killed while its LEZ follow-up claim is held in its sidecar, at each of `ZEC_CLAIM_HOLDS`; the swap completes on both Nodes. `xmr-kill-taker`, `xmr-kill-maker`: the role killed at each of `XMR_KILL_POINTS`, a fresh swap per point; LEZ to the unit, the owner accounts' transaction counts, the Maker's one funding, and the Monero swept to the Taker or, refunded, back to the Maker or never sent. `xmr-taker-refund-kill`: the Taker killed right after its refund is admitted and before it is submitted; started again, it submits the refund |
| The swap completes without the party that revealed | `zec-swap-sdk/tests/sdk_lifecycle.rs`: `maker_restart_after_reading_the_zcash_reveal_claims_lez_with_the_stored_preimage` | `survivor`: the Taker Node stopped right after its revealing claim; the Maker reaches `completed` alone, and the Taker, started again, completes. `zec-survivor`: the Taker Node stopped once its Zcash claim is broadcast; the Maker completes alone, the claim's confirmation while the Taker is down is asserted, and the Taker completes after its restart |
| A repeated request has one effect | `swap-store/tests/taker_facade_registry.rs`: `initiation_is_atomic_and_exactly_replays_after_restart`, `action_admission_is_generation_fenced_and_exactly_replays_after_restart`, `concurrent_exact_initiation_converges_to_one_admission_and_one_replay`; `swap-store/tests/public_effect_journal.rs`: `prepared_effect_is_exactly_replayable_and_conflicts_on_any_immutable_drift` | `replay`: the offer publication, the take, the lock and the claim each sent twice with the same request; the second answers `was_replay`, the lock returns the same transaction id, and the swap is listed once as `completed`. `zec-replay`: the same for the publication, the take and the Zcash claim |
| The adaptor nonce is used once and sealed at rest | `swap-store/tests/adaptor_session_journal.rs`: the six tests of section 2, and `concurrent_signing_uses_one_callback_and_one_exact_outbox` | The kill scenarios above cross the signing ceremony's storage on every Bitcoin swap; none inspects the journal file |
| Unreadable state fails closed for its swap only | `taker-node/src/taker_service/lifecycle.rs`: `a_swap_whose_state_cannot_be_read_does_not_hide_the_others`; `swap-store/tests/btc_recovery.rs`: `reopen_rejects_missing_or_out_of_order_evidence_instead_of_trusting_snapshot`; `swap-store/tests/zec_sdk_recovery.rs`: `corrupt_or_future_protected_claim_payloads_fail_closed_on_reopen`, `corrupt_maker_intent_or_transition_fails_closed_after_reopen`; `swap-store/tests/public_effect_journal.rs`: `malformed_digest_and_transition_shape_fail_closed_as_corrupt` | `zec-corrupt-state`: of two Zcash swaps at `claim_available`, one swap's role state file is replaced, with the Taker Node stopped, by bytes that are not SQLite. After the start the swap's monitor answers `-32010` `taker_monitor_unavailable` on every read and the file keeps its inode and bytes; the swap list shows that swap as `attention_required` with reason `taker_monitor_unavailable` and the other swap in its own state; the other swap completes on both Nodes |
| A swap keeps the configuration it was taken under | No crate test covers this property; the two devnet scenarios are its proof | `regenerated-config`: the live role configuration is rendered again and the Taker Node restarted; the swap reloads against its saved copy, is listed non-terminal, and completes. `tampered-config`: one byte appended to the swap's saved copy; after a restart the lock is refused and the swap lists as `attention_required` |
| Concurrent swaps have their own state, authority and leases | `swap-store/tests/restart_recovery.rs`: `concurrent_maker_users_are_persisted_as_isolated_swaps`; `swap-store/tests/public_effect_journal.rs`: `composite_identity_isolates_every_authority_dimension`, `one_chain_effect_id_cannot_be_crosswired_to_another_authority_key`; `swap-store/tests/bridge_operation_journal.rs`: `journal_keys_isolate_run_role_swap_and_operation`; `swap-store/tests/maker_actor_process.rs`: `restart_preserves_distinct_leases_while_one_swap_has_one_fenced_owner`, `competing_connections_can_claim_distinct_swaps_independently`, `pinned_executable_rejects_cross_swap_lock_composition`; `swap-store/tests/xmr_effect_workflow_concurrency.rs`: `concurrent_authorization_returns_exactly_one_invoke_once`; `maker-node/tests/daemon_actor_supervisor_process.rs`: `daemon_runs_overlapping_actors_and_isolates_failing_peer_across_restart`, `daemon_leases_two_accepted_xmr_applications_concurrently_across_restart` | `concurrent`: two Bitcoin swaps interleaved end to end; both complete. `concurrent-kill-maker`: the same with the Maker killed with SIGKILL once both swaps have both legs locked; both complete. `zec-concurrent`, `zec-concurrent-kill-maker`: the same for two Zcash swaps taken together, their LEZ steps interleaving on the same role accounts. `xmr-concurrent`: two Monero swaps of the same Nodes, the second taken while the first's lock and funding run; LEZ moved twice the lot, two fundings, two sweeps, both completed on both Nodes. `mixed-pair`: a Bitcoin and a Zcash swap in flight at once on the same Nodes; both non-terminal when both are taken and again when the Bitcoin claim is sent; both complete. `mixed-three-pair`: a Monero, a Bitcoin and a Zcash swap at once, all `TakerSellsLez`, so the Taker's one LEZ owner account signs each lock while the others are in flight; all three non-terminal once taken, all three complete, and the owner accounts' LEZ transactions are recorded |
