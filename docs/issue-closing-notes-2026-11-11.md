# Notes for the open issues, 2026-11-11

What PR #120 changed for each open issue Logos filed, with the commits and how
each was checked. These are drafts for the issues; nothing here has been
posted. Commit ids are those of `feat/split-lez-node-route`.

## #103, items 1 and 16: latent paths in `swap-core`

Item 16 (Logos QA "CODE-5") is fixed in `crates/swap-core/src/lib.rs` (commit
`f6443ec2`). `SwapCoordinator::refund_maker_leg` and `refund_taker_leg` refuse
with their existing `Error::InvalidPhase` whenever `claim_evidence` is set,
before the phase whitelist is consulted: after a revealing claim is recorded no
refund is admitted while that evidence stands. The model has no observation of
a revealing claim leaving the chain, so the guard is strict; a guard was chosen
over a post-reveal phase so that no phase table changes. It also covers the
second route into the same state, a shallower re-observation of a lock after
the reveal. Tests, each over BTC and ZEC in both directions, in
`crates/swap-core/tests/reorg_after_reveal.rs`:
`maker_lock_removal_after_revealing_claim_admits_no_refund`,
`taker_lock_removal_after_revealing_claim_admits_no_refund`,
`depth_regression_after_revealing_claim_admits_no_refund`, and
`lock_removal_before_any_claim_still_refunds_both_legs` (refunds without a
claim are unchanged). The property
`no_refund_after_reveal_and_no_claim_below_maker_policy` in
`transition_properties.rs` checks that no refunded phase holds claim evidence;
the coordinator fuzz target asserts the same, with the two sequences as seeds.

Item 1 was still present and is fixed in `observe_maker_lock_impl` (same
commit): a maker-leg observation in `TakerLockReorged` took the replay early
return without updating `maker_funding_confirmed`. That phase now records the
maker leg's depth; the phase itself does not change. The reported sequence is
`shallow_maker_lock_during_taker_reorg_blocks_the_claim`: the revealing claim
is refused with `MakerLockNotConfirmed` and admitted once the maker leg is back
at policy depth; `maker_lock_returning_during_taker_reorg_restores_claim_authority`
covers the converse.

A third finding of the same kind, found while fuzzing the fix, is closed in
commit `c75576ab`: the lock-removal observers cleared a funding flag even in
`Completed` and `Refunded`, so a settled coordinator changed after later input
(the checked-in seed `fuzz/corpus/coordinator/reorg-and-conflict` failed on
it) and a replayed follow-up claim returned `TakerLockNotConfirmed`. Both
observers now refuse a removal in a settled phase and change nothing. Tests:
`lock_removal_after_completion_is_refused_and_changes_nothing`,
`lock_removal_after_both_refunds_is_refused_and_changes_nothing`; the property
also checks that a settled coordinator is unchanged by any later input, and
the whole fuzz corpus passes.

The ZEC SDK applies the same rule before it plans a refund, so it never stages
or broadcasts a refund the coordinator would refuse to project (commit
`37f706cd`). `next_refund_step` (`crates/zec-swap-sdk/src/sdk.rs`) and
`RefundStepV1::validate_active_phase` (`src/refund.rs`) admitted the LEZ refund
in `TakerLockReorged` and `MakerLockReorged` from the phase alone. Now
`ActiveZecSwap::drive_refund` picks its step through `admitted_refund_step`,
which offers none while claim evidence is recorded, and
`RefundIntentV1::from_active`, `RefundTransitionV1::from_observed` and
`RefundTransitionV1::apply_to` validate through `RefundStepV1::validate_active`,
which refuses with `RefundError::ClaimEvidenceRecorded`. Tests, in both
directions and both reorg phases:
`no_refund_step_is_offered_in_a_reorg_phase_holding_claim_evidence`,
`the_lez_refund_step_is_still_offered_in_a_reorg_phase_without_claim_evidence`,
`refund_validation_refuses_a_reorg_phase_holding_claim_evidence` and
`refund_validation_still_admits_a_reorg_phase_without_claim_evidence`. The BTC
SDK needs no change: its planner treats the reorg phases as a wait, and
`apply_recovery_transition` refunds only from phases that cannot hold claim
evidence.

None of the four is reachable in shipped flows: the BTC lifecycle has no
lock-removal or depth-regression transition; the ZEC SDK applies removals only
before the maker lock and refuses a lock depth below policy; maker-node's
`apply_zcash_funding_event` has no non-test caller. No existing test was
edited; the workspace and pair tests pass.

## #103, items 3, 4 and 5

- Item 3: the route-health example lists Bitcoin TakerSellsForeign and
  TakerSellsLez beside the Zcash routes, with a `bitcoin-cli` probe (commit
  `8725b06a`).
- Item 4: `deploy/scripts/lane.sh`, which every harness script sources, takes
  `LEZ_IMAGE_PREFIX`, `LEZ_IMAGE_TAG`, `LEZ_ESCROW_PROGRAM_ID` and
  `LEZ_RELEASE_COMMIT` from `release.env` when it sits beside `compose.yaml`
  and the environment leaves them unset, and never builds on a bundle (commit
  `5a0301a8`). Checked by `deploy/tests/test_release_bundle.py`, which lays a
  bundle out as `package-dist.sh` does. Not run from a real release bundle.
- Item 5: `reset-swaps.sh` refuses to run with open swaps and, with
  `--abandon-open`, archives the per-swap directories and stores instead of
  deleting them (commit `90dd2e21`).

Items 2 and 6 to 15 are re-checked item by item in the last section of these
notes, by reading the code at `18dd9dd8`; nothing there was re-run.

## #115: reply to the three questions of 28 October, and what follows PR #120

PR #120 runs the stack as Logos modules: the Logos Blockchain node
(`blockchain_module` 0.2.4, the released package) and the LEZ indexer in one
network Basecamp; each role's LEZ wallet (`lez_core`), the Node supervisor
(`lez_swap_node`) and the Node itself, carried by a Logos package
(`lez_maker_node` / `lez_taker_node`), in that role's Basecamp. The sequencer
proxy for the Nodes, the chat gateway and `desk-sockets.py` are gone.
`docs/submission-115.md` maps each line of this issue to the code, and lists
where the result differs from it.

1. **Bitcoin as a module (`logos-bitcoind-module`).** Not in PR #120 and not
   planned for M2. Bitcoin Core stays bring-your-own, as this issue first
   scoped it; the assessment is in `docs/submission-115.md` (source reading
   only, no spike was run).
2. **Zebra as a module (`logos-zebrad-module`).** Not in PR #120 and not
   planned for M2: there is no released package of it, and the stack's Zebra
   is a plain container (Zebra 7.0.0, NU7).
3. **macOS.** The role packages are Linux only today: `lez-swap-store` keeps
   its Linux-only hardening. On a Mac the stack runs in Docker, with each
   desk inside its role's Basecamp container and reached over VNC; native
   macOS desks talking to Nodes in Docker are no longer a path. A
   `darwin-arm64` role package is follow-up work.

Also different from the issue's text, and staying so for M2: the indexer is
the 1.1.1 source rebuilt on LEZ v0.2.4 with three patches rather than the
official 1.2.0, which is built on LEZ 0.3 and comes with the LEZ 0.4 port
(#107); the Maker's Node runs in the Maker's Basecamp under `lez_swap_node`,
like the Taker's, rather than as a daemon under `logosctl`.

The Node's resource use: `logosctl module stats` samples one process per
module, the `logos_host` the runtime started, and a module has no way to add
the processes it starts, so it shows the supervisor and not the Node. PR #120
adds `lez_swap_node.node_stats(role)` (module 0.3.1): pid, uptime, restarts,
CPU, memory, threads and open files of the Node and of every process in its
group (forwards, sidecars), read from `/proc`; the acceptance script judges it
against `/proc`. `module stats` itself still shows only the host; upstream
request: "`module stats` reports only the `logos_host` process; include (or
list) the processes a module host started".

Follow-ups after PR #120: the role's configuration and identity inside the
package; the `darwin-arm64` package.

## #112: finality and claim visibility

(1) `maker_health` and `taker_health` include `lez_chain`: the last finalized
block id, when it advanced, the sequencer tip, the threshold, and `degraded`
with reason `lez_finality_stalled` or `lez_indexer_unreachable`; a degraded
chain sets the Node's `degraded` (commit `6df0fece`). The rule is the devnet
heartbeat's: finality flat for the threshold while the tip moves. The
threshold is `--lez-finality-stall-seconds`, 600 s by default and 60 s on the
devnet. Both desks show "LEZ finality has not advanced for N min — swaps wait
until it does" above the swaps (commit `1775049b`).

(2) A claim or refund that is landing shows "… submitted" with no button;
"Resubmit claim/refund (safe to repeat)" appears only after 15 minutes without
a state change, or when the Node reports a failure class (commit `1433d7b1`).

Checked by unit and contract tests and on the devnet: with the sequencer
stopped both Nodes reported `lez_finality_stalled` 73 s later, both desks
showed the line, and both recovered within 36 s of its restart
(`ui-e2e.sh lez-finality-stall`); a BTC desk journey showed the claiming row
without a button. Not exercised live: the 15-minute "Resubmit" path, the
`lez_indexer_unreachable` reason, the 600 s default on a public network. The
issue's further asks (chain events in the Activity pane, `btc_height`) are not
implemented.

## #113: wipe, then start

`down.sh --wipe` retires the wiped chain's escrow deployment and bootstrap
manifest, keeping the identities, and the bootstrap refuses a deployment
recorded for another genesis (commit `18c49392`); `start.sh`'s ready wait now
also needs LEZ finality to be moving (#112). Not re-run from a release bundle.

## #114: Bedrock pin and the finality stall

The node is the released `blockchain_module` 0.2.4, pinned by sha256, hosted
in Basecamp. On the devnet the stall no longer occurred in today's runs after
the funding change (the sequencer pays from notes of its own, `88fcabbf`, with
a larger genesis and tip, `f0c88084`); the upstream cause
(logos-blockchain#3643) is not fixed by us.

## Packages: what `logosctl` still refuses (upstream)

The Logos-native acceptance (`docs/evidence/logos-native-2026-11-11-c/`) has
13 failing steps per role left: 9 are unsigned packages (ours and upstream's
alike; signing is not part of this PR) and 4 are `logosctl` behaviour with
issue texts in `docs/evidence/logos-native-2026-11-11-b/README.md`. `module
stats` showing the module host instead of the Node is recorded as an upstream
request; the Node's own figures are judged through `node_stats`.

## #103, items 2 and 6 to 15, and #52: item by item

Checked by reading the code and the evidence at `18dd9dd8`, against `v0.2.5`
(`c99ff8ce`) and `v0.2.6` (`4b76e2f7`, documentation only over `v0.2.5`).
Nothing below was run for this check.

**Item 2, upgrade with an open swap: unchanged in kind, not tested.** From
`v0.2.6` to this head `SwapCoordinator` gained no field (`git diff
v0.2.6..HEAD -- crates/swap-core/src` adds only guards) and
`crates/swap-store/src/btc_recovery.rs`, which compares the stored
`initial_snapshot_digest` and raises `AgreementConflict`, is unchanged; the
escrow program id pinned in `scripts/verify-lez-v02-provisional.sh` is the
same (`c22d61fc…`), and the Nodes keep the same state volumes
(`maker_state`, `taker_state`). The store changes are additive: new
`CREATE TABLE IF NOT EXISTS` tables, a separate `taker_actor_schedule` file,
and a tested migration of `bridge_operation_contexts` to the widened
operation set. ZEC profiles are additive too (`deterministic-local-devnet-v1`
to `-v6`, `public-testnet-v2`, `deterministic-local-v2`); the two ids
`v0.2.6` knew keep their parameters. So by the code a BTC swap store written
by `v0.2.6` should open at this head, but no test opens a store written by an
older release, and the Maker's actor pin still stops each open swap as
`failed: actor_deployment_invalid` until `maker_actor_repin_v1`
(`docs/api/README.md`). In practice an open swap does not survive this
upgrade on the devnet: this head runs a new LEZ chain (the Logos Blockchain
node as a module, a new genesis, `f0c88084`), so the old chain's escrows are
not there, and `v0.2.6` shipped no ZEC lane at all. Release-notes line, and
the drain procedure: withdraw every offer; let each open swap reach
`completed` or `refunded` (a refund waits for its refund time); check that
`taker_swap_list_v1` and `swap_history` show nothing open, which
`reset-swaps.sh` also refuses to pass (`90dd2e21`); then upgrade. No
migration exists or is planned for M2.

**Item 6, a held refund looks like a dead node: partly.** Done: both Nodes
report `lez_chain` in their health with `lez_finality_stalled` (`6df0fece`),
both desks show "LEZ finality has not advanced for N min — swaps wait until it
does" (`1775049b`), and a refund in flight no longer offers an immediate
button (`1433d7b1`). What a BTC Taker row shows while its refund is held for
LEZ finality past the cutoff: "Refund submitted", "Your Node follows the
refund; it lands once its timelock allows" (`node_market.cpp` `takerRow`,
`refund_in_progress`), then, 15 minutes after the agreement's refund time,
"No change for N min; Resubmit refund is safe to repeat". The actor's last
outcome is still not in the API or the row: a BTC Taker view never sets
`last_failure_class` (only the ZEC supervisor's schedule fills it,
`zec_supervisor.rs` `overlay_schedule`), and a failed BTC refund step goes to
stderr only (`lifecycle.rs` `report_actor_failure`); `status_detail` carries
only the ZEC first-lock attempts. No field names the wait or its time
("waiting for LEZ finality past 16:05"), and the finality line appears only
when finality is flat while the tip moves, not when it is merely slow.
Smallest close (engine Rust plus desk): record the last effect outcome of a
BTC swap beside its state, as the ZEC schedule does, expose it as
`last_failure_class` plus a `waiting_for` sentence with the cutoff and the
finalized LEZ time, and have the row print it.

**Item 7, green "Backend connected" with the Node down: still open.** The
badge is bound to `root.ready`, which is set when the view module loads
(`apps/basecamp/taker/src/qml/Main.qml:574-580`, `:640`; Maker `:558`;
generated by `generate-desks.py:452`), not to the Node. `43cbedf2` (#95)
made the status strip follow the Node; it was already in `v0.2.5`, which is
the build Logos saw the green badge on. Smallest close (desk): bind the
badge to the last health result ("Node connected" / "Node unavailable") or
label it "Desk loaded".

**Item 8, early-lock selling LEZ: still holds for BTC.**
`scenario_early_lock_reverse` (`deploy/scripts/node-e2e.py`) is unchanged
since `v0.2.5`: it cuts the Maker's indexer forwarder, so the Maker cannot
*see* the Taker's lock (not: sees it unfinalized), and it looks for the
Maker's Bitcoin lock with `bitcoin_effects`, which reads the *Taker's* view.
The ZEC twin (`scenario_zec_early_lock`, new) checks the chain itself: the
Maker's funding outpoint unspent and zebra's mempool empty. Smallest close
(harness): in the BTC reverse case fail on `maker_lock_recorded(swap_id)` or
a non-empty `getrawmempool`, as the forward case and the ZEC twin do. "Seen
but unconfirmed" stays out of reach on LEZ, which has no mempool to hold a
transaction in.

**Item 9, offline-after-lock: still holds.** `cut_delivery_and_chat` still
renames `/run/lez/maker/chat.sock` and runs `chmod 0755 /delivery` in the
Maker's container (the Maker's whole Basecamp now). The Maker's health turns
`chat` and `delivery` to `unavailable` because it stats the path
(`owner_socket_is_available`) and validates the directory mode
(`validate_private_directory`): a policy trip, not a cut. The listening
socket and any accepted connection stay open, the Taker's read-only
`/delivery` stays readable, and only the Maker's health is asserted; the
ZEC twin uses the same cut. Smallest close (harness): show that nothing
off-chain was used after the cut (the Maker's chat and delivery request
counts flat from the cut to completion, or a connect from the Taker's
container refused), and cut delivery with `chmod 0000` rather than 0755.

**Item 10, refund-then-maker-lock selling the foreign coin: still holds.**
`scenario_refund_admitted_forward` only checks that the Taker is not
`completed` at the cutoff; "(maker lock unobserved)" is a log line, not an
assertion. Both directions record `taker_balance_before` and `_after` and
compare nothing, and the reverse case logs the maker lock as `timely` or
`late` by median time without failing on `late`. The ZEC twin does assert
the Taker's actor stayed at `taker_lock_confirmed`, but passes when the
blind refund was not admitted (`blind_admission` false). Smallest close
(harness): assert the Taker's view has no maker lock effect and is before
`both_legs_locked` at admission, assert the balance delta (the lock amount
back, less fees), and fail a reverse run whose lock was late by median.

**Item 11, property test: the model gained the depth, the note was not
corrected.** The new property
`no_refund_after_reveal_and_no_claim_below_maker_policy` (`f6443ec2`,
`c75576ab`) gives `MakerLock` a depth of 1 or 2 against a policy of 2, adds
taker-lock removal, claims and refunds, and checks claim authority, refunds
after a reveal and settled coordinators. The original property still has
`MakerLock { alternate }` without a depth, and the checked-in seed still
shrinks to the replayed claim on a completed swap, which `c75576ab` now
refuses at the lock removal. What the new property shrinks to was not
checked here, and the `v0.2.5` release text ("shrinks to exactly the
reported shape") is unedited (release last updated 2026-09-28).

**Item 12, regression evidence: partly.** Done: the runner is committed
(`deploy/scripts/regress.sh`; a test that fails while finality moves is not
retried, a frozen chain voids the try), and the M2 release-candidate run
records its image ids and source (`docs/evidence/regression-m2-2026-11-11/
PROVENANCE.md`, "Images"; 0 frozen tries). Not done: `regress.sh` samples
the finalized height every 30 s but logs it only when it declares a freeze,
and never the sequencer tip; the wait loops of `refund-then-maker-lock` log
neither swap state nor finality and still fail with "the admitted-refund
short-circuit is unfixed"; the `v0.2.5` evidence's "Chain recreations during
the run" is still the placeholder comment; the `v0.2.5` release text still
says "43 of 43" beside the "45 of 45" paragraph. Smallest close: one log line
per wait iteration (state, finalized and tip) in `node-e2e.py`, the same
pair at each `watched()` sample (harness), and the two text corrections
(docs and release).

**Item 13, videos: no longer applies to this release, not fixed for
`v0.2.5`.** The recordings of the M2 run are by-products of passing tests at
a stated source (`023c929e`) on recorded image ids; seven ZEC ones are in
`videos/`, the other 25, the Bitcoin set included, are kept outside git for
release attachments, and no release was built. The `v0.2.5` recordings were
neither dated nor re-recorded. Whether the Taker's "Completed" is on screen
in the happy-path recordings was not checked here.

**Item 14, late-claim scenarios: still open.** `node-e2e.py` has no scenario
in which the revealing claim is sent but never mined, in either direction.
`survivor`, which `regress.sh full` runs in both directions, stops the Taker
right after its claim is admitted and asserts the Maker completes, then the
Taker; it does not hold the claim unconfirmed across the stop. Smallest close
(harness): selling LEZ, hold the Taker's Bitcoin claim in the mempool (the
miner stopped) across a Taker stop; selling Bitcoin, the LEZ claim has no
mempool, so cut the Taker's sequencer forwarder before the claim, as #101
did for the late locks.

**Item 15, LEZ v0.3: partly.** #107 (2026-10-12) assesses the port against
LEZ 0.4, with which the project agreed to port once rather than through 0.3:
the escrow program a rewrite, the sidecar a port, the Node and desks little
change, about 25 pins. It asks Logos five questions and an estimate, and has
no reply. The repository states the port out of scope for M2
(`docs/submission-115.md` R12) and the public-network evidence postponed to
it (`docs/m2-acceptance-checklist.md`). No Gateway estimate is recorded and
no milestone owns the port.

**#52, offer list growth: partly.** `maker_offer_list` takes `{states:[…]}`
(`56dea0cd`, #77, in `v0.2.4`; `docs/api/README.md`), and the Maker desk
asks for `active`, `reserved` and `consumed` (`node_market.cpp`); the 4 MiB
snapshot budget stays as a ceiling (`local_json_rpc_client.h`). The snapshot
still grows with history: every taken offer stays `consumed`, `swap_history`
returns every swap with no filter, and the desk calls `maker_actor_monitor_v1`
once per swap per snapshot. The third box, a desk step on a Node with a long
history proving the snapshot stays small, is not done; regression lanes reset
swaps between tests, so no run builds such a history. Smallest close: the
desk step (harness), and to bound the growth, a `states` or `since` filter
on `swap_history` and consumed offers fetched only for the swaps shown
(engine Rust and desk).
