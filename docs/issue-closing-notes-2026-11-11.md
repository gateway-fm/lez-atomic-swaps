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
notes, by reading the code at `18dd9dd8`; nothing there was re-run. Items 6
and 7 were fixed afterwards; their paragraphs there say how.

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
Nothing below was run for this check. Items 8, 9, 10, 12 and 14 were
rewritten after their harness fixes (later the same day); each says what the
test asserted before and asserts now, checked by unit tests offline
(`deploy/tests`), not yet by a run on the stack.

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

**Item 6, a held refund looks like a dead node: closed in code, to be seen
on the stack.** Before: both Nodes report `lez_chain` with
`lez_finality_stalled` (`6df0fece`), both desks show the finality line
(`1775049b`), and a refund in flight offers no immediate button (`1433d7b1`).
Now the actor's last outcome is in the API and on the row. The Taker's swap
view and `maker_actor_monitor_v1` carry `last_outcome`, an additive field of
schema 1 (`docs/api/README.md`, "The actor's last outcome"): the step that
ran, the actor's own outcome word or `failed` with its class, when, the
revision, and `waiting_for` with the bound the actor read. The Bitcoin actor
names the wait beside the decision that already holds the step, from facts it
already read and used to send only to its trace: finalized LEZ time against
the Maker's lock cutoff (`lez_finality_past_cutoff`, Logos' case), Bitcoin
median time against the cutoff, the Maker's Bitcoin lock confirmations, the
refund's CSV maturity height against the stable tip, the refund's
confirmations, finalized LEZ time against the LEZ refund time; with none
named, the phase's wait from the signed schedule (the Maker's lock until its
cutoff, the other party's claim). Its decisions are unchanged; the effect
output gains an optional `wait`. A failed Bitcoin attempt now names its class
in the view's `last_failure_class` (it went to stderr only). Zcash: the
supervisor keeps the actor's last effect or failure, and bounds
`awaiting_deadline` for the LEZ refund by the agreement's LEZ refund time.
The desks print the wait ("Waiting for LEZ finality to pass the lock cutoff
16:05 (now 15:58)", "Refund matures at Bitcoin height 1234 (now 1229)") and,
while its bound is not reached and the outcome is under 5 minutes old, offer
no Resubmit; the 15-minute and failure-class Resubmit of `1433d7b1` is
otherwise unchanged. Limits: the outcome lives in the Node's memory (absent
after a restart until the next attempt, seconds later); the Zcash refund
height is not reported by the actor, so that wait has no bound; the finality
line still appears only when finality is flat while the tip moves. Unit and
contract tests pass off the stack; the held-refund row needs the landing (a
refund admitted while LEZ finality is frozen).

**Item 7, green "Backend connected" with the Node down: closed in the
desks, to be seen built.** The badge (`generate-desks.py`, both desks) reads
the channel state the status strip reads (`nodeLink`, moved by every reply and
transport failure; the 10 s health poll's own failure now moves it too) and
the last health reply: "Node connected" (green) only while the Node answers
and is not degraded, "Node degraded · LEZ finality stalled" / "· LEZ indexer
unreachable" (amber, from `lez_chain.reason`), "Node unavailable" (red, at
the same moment the strip says so), "Checking the Node" until the first health
reply, "Desk loading" before the view module is up. The Basecamp product test
(`apps/basecamp/tests/basecamp-role-product.mjs`) waits for the new words
instead of "Backend connected". Not built here; the landing builds the desk
packages and stops a Node with a desk open.

**Item 8, early-lock selling LEZ: fixed in `240c778e` (harness), not yet
run on the stack.** Before, `scenario_early_lock_reverse` cut the Maker's
indexer forwarder, so it tested a Maker that could not *see* the Taker's LEZ
lock, and looked for the Maker's Bitcoin lock with `bitcoin_effects`, which
reads the *Taker's* view. Now the Maker loses only its indexer forwarder, the
one source that says a LEZ block is final (the sidecar observes LEZ in the
indexer's finalized window), and keeps its sequencer forwarder; the Taker's
lock is read back over the Maker's own route (`getTransaction` on its
`127.0.0.1:3040`) as included, so the lock is in front of the Maker and
nothing it can read says it is final. That is the closest LEZ has to "seen,
unconfirmed": it has no mempool, and its finality cannot be held back without
freezing the chain every lane shares. For the Maker's attention span (five
rounds of its requeue plus backoff from the profile, at least 90 s) each
poll asserts no phase that implies an observed first leg, no `maker_lock` in
the Maker's own record (`maker_lock_recorded`), and no send from the Maker's
Core wallet since the hold began, mined or in the mempool (`listsinceblock`
with the mempool); once the view is back, the Maker's lock must appear as
such a send, so the chain check is shown able to see one. Checked the
others: forward (selling Bitcoin) already held the lock in the mempool and
read the Maker's own record; it now also asserts 0 of the agreement's
required confirmations and the Maker's LEZ balance unchanged once finality
has passed the sequencer tip at the end of the hold. The ZEC twin selling
LEZ cut both forwarders (the same blindness) and now cuts the indexer alone;
selling ZEC it checked phases only and now checks the Maker's LEZ balance
too. Upgrade once r103engine's view fields land: assert that the Maker's
`waiting_for` names the Taker lock's finality during the hold.

**Item 9, offline-after-lock: fixed in `98e60713` (harness), not yet run on
the stack.** Before, the cut renamed `chat.sock` (an open connection
survives) and set `/delivery` to 0755 (looser, still readable by the
Taker), and only the Maker's health, which turned `unavailable` because it
stats the path and checks the mode, was asserted. The transport is two
shared volumes: the Maker's Chat socket on `maker_socket`, mounted into the
Taker's Basecamp, and `delivery_dir`, read-only for the Taker; both Nodes run
as uid 4713 without capabilities. The cut now sets both inodes to mode 0000,
so the kernel refuses every non-root process on both sides a connect or a
read; no image change is needed. Asserted: before the cut, a probe in the
Taker's container as the Node's user connects and lists (so it is not blind);
after it both fail with `EACCES` and no descriptor there points into
`/delivery`; Chat opens one connection per call, so the scenario waits until
the Maker holds no accepted connection on `chat.sock` (its
`/proc/net/unix`), then samples that every 2 s until the swap completes and
fails on any connection; both Nodes report chat and delivery `unavailable`
and are degraded (the Taker's delivery state is its own failed discovery);
the Taker's side is still refused at completion; the restore (in `finally`)
puts back 0600 and 0700 and asserts the probe connects and lists again and
neither Node is degraded. The Maker's log has no per-request Chat line, so
"no request reached the Maker" rests on the kernel refusal plus the
connection samples, not on a counter. The ZEC twin uses the same cut.

**Item 10, refund-then-maker-lock selling the foreign coin: fixed in
`8af3e80d` (harness), not yet run on the stack.** Before, "(maker lock
unobserved)" was a log line, balances were recorded and never compared, the
reverse case logged "late" by median time and went on, and the ZEC twin
passed with `blind_admission` false. Now the refund is asked for until
admitted (within ten observer sweeps), and the Taker's view is asserted
when it asks and right after admission: a state before the second lock and
no `maker_lock` effect (forward: none at all; reversed: none with the
agreement's confirmations, since the Taker may see the held lock in the
mempool); a Node that refuses fails the scenario. Both directions then let
the Maker refund its own leg and compare both parties on both chains, read
from the chains (each Node's Core wallet, trusted + pending + immature, and
its LEZ account's finalized balance): LEZ to the unit, the party that locked
Bitcoin back less at most 20,000 sat of fees (4,100 observed for a lock and
its refund), the other party to the satoshi. A reverse lock confirmed with a
median time past the cutoff ends the try as `NotExercised` (exit 98, retried
by `regress.sh`), since it is not the case the scenario tests. The ZEC twin
selling ZEC now fails when the blind refund is not admitted (every recorded
run admitted it), asserts the same at admission, and requires both LEZ
accounts to end where they started. Upgrade once r103engine's fields land:
assert the Taker's `waiting_for` at admission names the maker lock.

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

**Item 12, regression evidence: the harness part fixed in `a4f1a48d`, the
v0.2.5 provenance filled from its log in the last commit of this set; not
yet run on the stack.** Before, `regress.sh` sampled the finalized height
every 30 s but logged it only when it declared a freeze and never read the
tip, and the wait loops logged no finality and failed with the generic
"short-circuit is unfixed". Now every long wait in `node-e2e.py` logs its
state, `LEZ finalized N, tip M` (from the heartbeat's status when fresh,
else the host's indexer and sequencer ports, never a Node forwarder a
scenario may have cut) on each change and at least every 60 s, and a
timeout reads "timed out after N s waiting for <what>; last seen: <state>;
LEZ finalized N, tip M". `regress.sh` writes both heights on each try's start
and verdict line, each 30 s sample into `chain.tsv`, and the tip into the
freeze line. The `v0.2.5` "Chain recreations" section now lists the four
recreations `RESULTS.txt` records (tests, times) and says plainly what that
log does not hold: no finality or tip reading, no classifier, nothing on
what a stopped try left open. Still open: the `v0.2.5` release text's "43 of
43" (a release edit, outside the repository).

**Item 13, videos: no longer applies to this release, not fixed for
`v0.2.5`.** The recordings of the M2 run are by-products of passing tests at
a stated source (`023c929e`) on recorded image ids; seven ZEC ones are in
`videos/`, the other 25, the Bitcoin set included, are kept outside git for
release attachments, and no release was built. The `v0.2.5` recordings were
neither dated nor re-recorded. Whether the Taker's "Completed" is on screen
in the happy-path recordings was not checked here.

**Item 14, late-claim scenarios: fixed in `11f322b7` (harness), not yet run
on the stack.** Before, no scenario held a revealing claim sent but not on
chain, and `survivor` did not hold it unconfirmed across the stop. Two new
scenarios, both directions, in `regress.sh full` on the `fast` profile:
`late-claim-confirms` (a) sends the claim, stops the Taker before it has
seen it confirmed and confirms it while the Taker is down (selling LEZ: the
lane's miner stopped, the claim found in the mempool as the spend of the
Maker's lock, mined with the Taker stopped; selling Bitcoin: the Taker's
indexer forwarder cut the moment its LEZ claim is out, its view required to
show the claim pending or the try is `NotExercised`, the Node stopped, the
claim waited final); the Maker completes alone, the Taker on restart, and
the balances show the lots traded. `late-claim-unmined` (b) keeps the claim
off chain with the Taker stopped until the refund path is open (selling LEZ:
empty blocks at the profile's pace mature the Maker's Bitcoin refund while
the claim stays in the mempool; selling Bitcoin: the Taker's sequencer
forwarder cut before the claim, so it is never included, asserted on the
Maker's route, through the claim window, the Maker asserted not to reach
claim evidence); then the Maker asks for its refund, the Taker returns and
both Nodes settle on their own. Either outcome passes, but both parties'
balances on both chains must fit a completed or a refunded swap and agree
with both Nodes: no party with both lots or neither. For the ZEC pair,
`zec-survivor` selling LEZ now holds (a): nothing but the harness mines
zebra, so a claim found in its mempool before the stop must be confirmed
before the Taker returns. A ZEC (b) is not added: zebra's `generate` takes
the mempool with it, so the refund height cannot be reached around a held
claim without another mining path.

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
