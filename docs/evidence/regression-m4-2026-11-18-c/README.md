# Full regression of `15b1dc84`, 2026-11-18

A later full run of the same day, 159 of 159 at `10a785ad` (this tree plus the sidecar change
for the refund retry loop below), is in [regression-m4-2026-11-18-d](../regression-m4-2026-11-18-d/).

One run of every test of `deploy/scripts/regress.sh full` (159: `regress.sh list full`) on the
Docker devnet with the Monero regtest chain up (`LEZ_MONERO=1`), three lanes at a time
(`LEZ_LANES=3`), without desk recordings: run `runtime/regress/20261118T101419Z`, 2026-11-18
10:14:19Z to 13:55:56Z (UTC), at `15b1dc84`.

The tree is `44e39081`, the commit of the full run of the same morning (run `20261118T040324Z`,
158 of 159, evidence in `docs/evidence/regression-m4-2026-11-18/` on top of `44e39081`), plus
three commits and nothing else:

- `234c17db` fix(swap-store): the Maker binds an actor's state database by its inode, owner, mode
  and link count instead of also its size and times, so a write in place between two `stat`
  calls no longer fails the actor's schedule as `actor_deployment_invalid`.
- `ef0f1299` fix(btc-core-adapter): before a caller-authorized Bitcoin broadcast, a readiness
  check that fails because a block landed between two RPCs is checked again, at most three times
  250 ms apart, before `testmempoolaccept`.
- `15b1dc84` fix(deploy): the regtest Bitcoin Core of each lane runs with `-checkblockindex=0`.

`git diff --stat 44e39081 15b1dc84` touches `crates/btc-core-adapter` (3 files),
`crates/swap-store/src/maker_actor_process.rs` and `deploy/compose.yaml`, nothing else.

**Result: 158 of 159 passed in one run of 221 minutes; one failed,
`ui:zec:zec-taker-refund:TakerSellsForeign` (below).** 157 tests passed on their first try.
`node:zec:zec-claim-collision:TakerSellsForeign` was NOT-EXERCISED on tries 1 and 2 and passed on
try 3, as in the run of `44e39081`. The failed test timed out on its first try, the run's only
timeout. 0 frozen tries; 1 new LEZ chain (both `zec-balance-refusal` tests at once on it); 3
timing-profile switches. All 16 Monero tests passed on their first try. Test time summed over
lanes 35,384 s over 161 tries (37,953 s over 161 in the run of `44e39081`).

| kind | tests | minutes (summed over lanes) | run of `44e39081`: minutes |
|---|---:|---:|---:|
| `node:btc` | 53 | 189.6 | 232.6 |
| `node:zec` | 51 | 126.4 | 128.8 |
| `node:xmr` | 13 | 139.4 | 143.7 |
| `ui:btc` | 17 | 42.3 | 48.0 |
| `ui:zec` | 22 | 68.7 | 54.9 |
| `ui:xmr` | 3 | 23.4 | 24.7 |
| all | 159 | 589.7 | 632.5 |

(`results.tsv`, column 4 in seconds, every try counted. `ui:zec` includes the failed try's
1,255 s; without it the group is 47.7 minutes. `node:btc` of the run of `44e39081` includes its
failed try's 1,862 s; without it that group was 201.5 minutes. `node:zec` includes the two
NOT-EXERCISED tries' 150 s, lane 3, 75 s each: "NO-COLLISION: A completed without a superseded
claim".)

Wall time 221 minutes (228 in the run of `44e39081`). By group (`regress.log`): `fast` 158.2
minutes (177.7), `zec-v6` 36.8 (22.6; the failed test held lane 3 for 21 minutes of it), `refund`
20.5 (22.5), the own-chain pair 5.6 (5.0). The median try took 113 s (124 s).

`results.tsv` is the runner's table (test, try, verdict, seconds, finished at, lane, LEZ chains
made so far by this run); `regress.log` is the runner's log, every try's start and verdict line
carrying the LEZ finalized height and tip. `logs/` holds the failed test's log and an excerpt of
the lane-3 Maker Node's log (below), as the tools printed them. None holds a local path. The
other per-test logs and the Node logs the harness kept are not committed; the lines quoted below
are from them.

## The two facts this run was made to test

**`actor_deployment_invalid`: 0 for the swaps of this run.** The Makers' own logs were searched
for the class: the three sets of Node logs the harness kept in this run (the failed try and the
two NOT-EXERCISED tries, all lane 3) and each lane's `node.log` in the Maker's socket volume, read
after the run. Those files outlive a run and carry older runs' lines; every hit names a swap of
an earlier run:

| Maker | lines | swaps | their run |
|---|---:|---|---|
| lane 1 | 3 | `a1321f30`, `61123f4e`, `1aee1825` (status attempt, "deployment artifact hash does not match") | 2026-11-14 (their swap ids are in that day's Monero test logs) |
| lane 2 | 1 | `a5f63bc4` (drive attempt, "deployment artifact is unsafe") | `20261118T040324Z`, the run of `44e39081` |
| lane 3 | 2 | `fd3af315`, `1dd5df0b` (status attempt, "deployment artifact is unsafe") | 2026-11-16 and 2026-11-17 (their swap ids are in those runs' Bitcoin test logs) |

The kept lane-3 logs repeat the two lane-3 lines, 5 in all; no other swap is named. The failed
swap's schedule (below) ran 332 attempts with no failure class. Before `234c17db` the class
failed a Bitcoin test in the run of `44e39081` and in full runs of 2026-11-17 and 2026-11-13 (the
evidence note of `44e39081` lists them); here no test failed that way. One run does not show
that the race is gone.

**Bitcoin mining.** A Bitcoin refund path mines 421 blocks (the CSV plus one) between the Taker's
`claim_available` and the kill walk's point "both legs locked, refund times running". In this
run that took 1 to 2 s in all four kill walks:

| test | this run | run `20261118T082111Z` (abandoned, below) | run of `44e39081` |
|---|---:|---:|---:|
| `node:btc:kill-taker:TakerSellsLez` | 1 s | 298 s | 290 s |
| `node:btc:kill-taker:TakerSellsForeign` | 2 s | 381, 398, 403, 390 s, each NOT-EXERCISED | 302 s |
| `node:btc:kill-maker:TakerSellsLez` | 1 s | 340 s | 243 s |
| `node:btc:kill-maker:TakerSellsForeign` | 1 s | 389 s NOT-EXERCISED, then 325 s | 317 s |

(From the timestamps of the last `claim_available` line and the kill point line of each test's
log.) All four kill walks, both `late-claim-unmined` tests and both
`concurrent-kill-maker` tests passed on their first try (`results.tsv`): kill-taker 1,197 and
870 s, kill-maker 1,277 and 915 s, late-claim-unmined 673 and 473 s (TakerSellsLez,
TakerSellsForeign; 1,254, 894, 1,326, 943, 804 and 830 s in the run of `44e39081`). The kill
walks still wait for their refund deadlines on the clock, so they shortened by 24 to 57 s each;
the two `late-claim-unmined` tests by 131 and 357 s.

## The abandoned run of `ef0f1299`

Earlier that morning the tree without `15b1dc84` (`ef0f1299`) was started as a full run
(`20261118T082111Z`, 08:21:11Z) and stopped at 09:57:57Z on the supervisor's order, 15 tries in:
8 passed, 6 NOT-EXERCISED (`node:btc:kill-taker:TakerSellsForeign` five times,
`node:btc:kill-maker:TakerSellsForeign` once: the swap had moved past the kill point while the
421 blocks were mined) and 1 failed (`node:btc:late-claim-unmined:TakerSellsLez`, whose refund
path opened past the LEZ escrow's refund boundary). The lanes' regtest chains were 230,000 to
260,000 blocks long and Bitcoin Core's default regtest `-checkblockindex` walked the whole index
after every block. That run is not evidence for any commit and is not committed.

## The failure: `ui:zec:zec-taker-refund:TakerSellsForeign`

Lane 3, 13:08:43Z to 13:29:49Z. The journey: the Taker locks its Zcash; the Maker's sequencer
route is cut so it sees that lock but cannot lock its LEZ; the Maker Node is stopped; the Taker
refunds its Zcash from the desk; the Maker Node is started again, the harness queues the Maker's
refund (the operator action the Maker desk has no button for), and the step `maker-up-reconcile`
waits up to 1,200 s for the Maker's swap to reach a terminal state.

Everything up to the last step passed: the Taker took the offer at 13:08:55 (swap
`zec-60893343`), the Maker saw the Taker's lock and was stopped at 13:09:17, zebra was mined to
the refund height 663 and the Taker's Zcash refund was in block 664 at 13:09:26, shown on the
desk. The Maker Node was started at 13:09:27; at 13:09:38 the harness queued its refund. The
Maker's actor, now able to reach the sequencer, went on with its plan from where it stood: it
submitted its LEZ initialization and funding (the funding at 13:09:42) and reached
`both_legs_locked`, so its own leg had to come back by a LEZ refund, due at 13:13:01 (the
escrow's `refund_at`). It never came back: from 13:09:58 to the timeout at 13:29:39 the harness
saw "maker both_legs_locked".

The lane-3 Maker Node's log, lines for the swap after the restart
(`logs/ui-zec-zec-taker-refund-TakerSellsForeign-1.maker-node-excerpt.log`: 74 lines; the log
has no timestamps):

    maker actor zec-60893343…: drive → submitted lez_initialize (phase taker_lock_confirmed, revision 2, next wait)
    maker actor zec-60893343…: drive → submitted lez_fund (phase taker_lock_confirmed, revision 2, next wait)
    actor recovery unavailable: LezRefund(Adapter(Transport(Remote(RemoteProtocolError { … code: InvalidTransaction … }))))    (twice)
    maker actor zec-60893343…: recover → awaiting_deadline lez_refund (phase both_legs_locked, revision 3, next wait)    (56 times)
    maker actor zec-60893343…: recover → submission_outcome_unknown lez_refund (phase both_legs_locked, revision 3, next wait)

and the last line repeats to the end of the log, 253 times in all; the container log has it every
3 to 5 s up to 13:29:40.

What the rest of the saved state shows (the lane's reset archive `20261118T133055Z` and the
actor's two databases in the Zcash state volume, copied out, read with `sqlite3`/`json` and
deleted; not committed, they hold the swap's keys):

- The Maker's schedule for the swap was healthy: `maker_actor_processes` at `leased`, 332
  attempts, no failure class, registered 13:09:02, last attempt 13:30:54; the queued refund
  (`maker_actor_manual_actions`, 13:09:38) leased by the same generation.
- The actor's bridge journal (`bridge.sqlite3`, `bridge_operation_contexts`) has one active
  context for `native_refund_submit`, poll 0, request `req-0937b789…`, never completed, beside 322
  completed eligibility observations and 265 completed exact refund observations.
- The swap's LEZ sidecar (`sidecar-state/bridge-requests.v1.json`) holds three
  `submit_transaction` entries: the initialization and the funding `accepted`, and
  `req-0937b789…` persisted as an **error**, code `unavailable` ("required official v0.2
  operation is unavailable"). Its `sidecar.log` has that one `request_error` for
  `submit_transaction`, no other.

So the first submission of the Maker's LEZ refund met a sidecar that answered `unavailable`. In
`compat/lez-v0_2-sidecar/src/bridge_runtime.rs` (`submit_transaction`) that code comes only from
the node lookups before the broadcast (the exact-inclusion lookup, the signer nonce, the escrow
accounts); a failed broadcast is `unknown_submission_outcome`. The refund was not sent. Then:

1. The Zcash LEZ port (`crates/lez-bridge-adapter/src/lib.rs`, `submit_native_refund`) maps any
   transport error to `RefundSubmitOutcomeV1::Unknown`, and `sdk_ports.rs` (`submit_refund`)
   keeps the submission's request context for an ambiguous outcome (`resume_ambiguous`), so the
   next submission of the same refund carries the same request id.
2. The SDK (`crates/zec-swap-sdk/src/sdk.rs`, `drive_owned_refund`) observes the prepared refund
   absent on the next drive and submits again.
3. The sidecar (`compat/lez-v0_2-sidecar/src/bridge_server.rs`, `execute_with_policy`) persists
   a submission's error and replays it for the same request id without running it again ("errors
   and in-flight markers stay single-attempt either way").

Every later attempt got the stored `unavailable` back, became `submission_outcome_unknown`, and
the refund was never sent. Nothing ends the loop: the swap stays at `both_legs_locked` with the
Maker's LEZ in escrow until an operator acts.

What is established and what is not:

- **Established**: the Maker's LEZ refund was never broadcast; its one submission's `unavailable`
  is replayed to every resubmission under the same request id; the swap's schedule, the LEZ
  chain (finalized height moved 6015 → 6792 during the wait; lane 2's Maker got its LEZ
  back meanwhile in `ui:zec:zec-maker-refund:TakerSellsForeign`, at 13:14:49) and zebra did not
  hold it up; no `actor_deployment_invalid`.
- **Not established**: why the node lookup failed at that one moment. The sidecar's log has no
  timestamps and names only the code. Its seven lines are three starts (`ready`), the escrow
  preparation's `unavailable` while the route was cut, two `invalid_transaction` answers to refund
  observations, and the refund submission's `unavailable` followed by a start.
- **Not the candidate's commits**: `-checkblockindex` is Bitcoin Core's; `ef0f1299` is the
  Bitcoin adapter's; `234c17db` changes only whether the Maker accepts an actor's state database
  before a drive, and every drive of this swap ran. The code in the loop (sidecar, LEZ port, Zcash
  SDK) is the same in `44e39081`.
- **The test's expectation is right**: the Maker must end terminal; it did not because of the
  product.

History: this test passed in every earlier run kept on the host since 2026-11-10 that ran it
(25 passes, 277 to 361 s, and one FROZEN try on 2026-11-11), and in the eight committed notes
that hold it (281 to 334 s). `maker-up-reconcile` failed in no earlier test log. The harness keeps
Node logs only for tries that did not pass (since `582d6c47`), so earlier passes left none; no
kept Node log on the host has `submission_outcome_unknown lez_refund` but this try's.
This is the first time the loop was seen; it needs only one transient `unavailable` at the first
submission of a LEZ refund.

A fix is not part of this tree. Two places would end the loop, both outside this commit's scope:
the sidecar could leave a submission that failed before its broadcast with `unavailable`
repeatable rather than persisted (the rule is shared by every pair's LEZ submissions, Bitcoin
included); or the Zcash LEZ port could retire the refund's submission context after an
`unavailable` answer, as it already does for a superseded plan, so the next submission gets a new
request id (Zcash engine).

## Reruns of the failed test

After the run, on a new LEZ chain from the same images and the same three lanes (build source
and Core arguments checked again), `regress.sh run ui:zec:zec-taker-refund:TakerSellsForeign
ui:zec:zec-taker-refund:TakerSellsLez node:zec:zec-taker-refund:TakerSellsForeign` five times,
14:05:15Z to 14:38:38Z. The runner switched to the tests' ZEC profile (`zec-v6`) each time. **15
of 15 passed on their first try**, 0 frozen tries:

| rerun | `ui:…:TakerSellsForeign` (lane 1) | Maker refund queued → Maker terminal | `ui:…:TakerSellsLez` (lane 2) | `node:…:TakerSellsForeign` (lane 3) |
|---|---:|---:|---:|---:|
| 1 | 278 s | 224 s | 314 s | 50 s |
| 2 | 283 s | 236 s | 302 s | 281 s |
| 3 | 287 s | 230 s | 311 s | 277 s |
| 4 | 288 s | 230 s | 314 s | 285 s |
| 5 | 284 s | 236 s | 300 s | 283 s |

In each of the five desk journeys selling Zcash, the restarted Maker locked its LEZ leg
(`both_legs_locked`) and then got it back by its LEZ refund, the path that looped in the run; the
middle column is the time from the "Maker refund queued" line to "the Maker ended refunded". In
the Node scenario selling Zcash the Maker did the same in reruns 2 to 5 (236 to 242 s); in
rerun 1 it saw the Taker's Zcash refund before it locked and ended `refunded` 3 s after the
refund was queued. The reruns do not show the loop gone: they show the path passes when the
refund's first submission is not answered `unavailable`. The lanes' Maker `node.log` files had no
new `submission_outcome_unknown` or `actor_deployment_invalid` line after the reruns.

## Lint and unit tests

Not run on this commit. The crates are those of `ef0f1299` (`15b1dc84` changes only
`deploy/compose.yaml`).

## Source and images

Tests and images from `15b1dc84`, tree clean (`regress.log` prints the commit and "tree dirty:
0"). Image ids, creation times, the build-source check and the Cores' arguments are in
[PROVENANCE.md](PROVENANCE.md).

## Not covered

- Monero beyond an offline regtest chain: no stagenet or mainnet; x86_64 (linux/amd64) only.
- No Monero test runs on the `refund` profile, and no run uses `local`.
- The devnet images run development builds. No release package was built or run from this
  commit.
- `ef0f1299`'s rechecked readiness: whether a broadcast in this run needed a second check is not
  known; the adapter does not log it.
- `234c17db`: one run without `actor_deployment_invalid` does not show the race is gone; the
  class was seen about once per full run before.
- The Maker's LEZ refund that loops on a replayed `unavailable` (above) is not fixed.
- macOS, linux/arm64, and any public network are untested on this head.
