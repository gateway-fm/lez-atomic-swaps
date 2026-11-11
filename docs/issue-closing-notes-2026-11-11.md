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

Items 2 and 6 to 15 were not re-checked for this PR.

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
