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

None of the three is reachable in shipped flows: the BTC lifecycle has no
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

The Logos-native acceptance (`docs/evidence/logos-native-2026-11-11-b/`) has
14 failing steps per role left: 9 are unsigned packages (ours and upstream's
alike; signing is not part of this PR), 4 are `logosctl` behaviour with issue
texts in that directory's README, and one is `module stats` showing the module
host instead of the Node it supervises.
