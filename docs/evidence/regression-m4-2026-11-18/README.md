# Full regression of `44e39081`, 2026-11-18

One run of every test of `deploy/scripts/regress.sh full` (159: `regress.sh list full`) on the
Docker devnet with the Monero regtest chain up (`LEZ_MONERO=1`), three lanes at a time
(`LEZ_LANES=3`), without desk recordings: run `runtime/regress/20261118T040324Z`, 2026-11-18
04:03:24Z to 07:51:43Z (UTC), at `44e39081`. It follows the
[run of `32c8bfb4`](../regression-m4-2026-11-17-rc/) (158 of 159) with the same 159 tests, on a
tree that adds the three code commits listed below. It is the first full run in which the harness
keeps both Nodes' logs for a try that did not pass (`582d6c47`).

**Result: 158 of 159 passed in one run of 228 minutes; one failed,
`node:btc:offline-after-second-lock:TakerSellsLez` (below).** 157 tests passed on their first
try. `node:zec:zec-claim-collision:TakerSellsForeign` was NOT-EXERCISED on tries 1 and 2 and
passed on try 3 (below). The failed test timed out on its first try, the run's only timeout.
0 frozen tries; 1 new LEZ chain (both `zec-balance-refusal` tests at once on it); 3
timing-profile switches. All 16 Monero tests passed on their first try. Test time summed over
lanes 37,953 s over 161 tries (38,766 s over 159 in the run of `32c8bfb4`).

| kind | tests | minutes (summed over lanes) | run of `32c8bfb4`: minutes |
|---|---:|---:|---:|
| `node:btc` | 53 | 232.6 | 237.4 |
| `node:zec` | 51 | 128.8 | 133.8 |
| `node:xmr` | 13 | 143.7 | 145.5 |
| `ui:btc` | 17 | 48.0 | 50.0 |
| `ui:zec` | 22 | 54.9 | 56.0 |
| `ui:xmr` | 3 | 24.7 | 23.4 |
| all | 159 | 632.5 | 646.1 |

(`results.tsv`, column 4 in seconds, every try counted. `node:btc` includes the failed try's
1,862 s; without it the group is 201.5 minutes (212.1 in the run of `32c8bfb4` without its
failed try). `node:zec` includes the two NOT-EXERCISED tries' 187 s.)

Wall time by group (`regress.log`): `fast` 177.7 minutes (185.2 in the run of `32c8bfb4`),
`zec-v6` 22.6 (26.6), `refund` 22.5 (23.8), the own-chain pair 5.0 (5.4). The failed test held
lane 2 for 31.3 minutes where it passed in 61 s in the run of `32c8bfb4`; it ended at 06:56:43Z,
before the last tests of the `fast` group, so no lane stood idle waiting for it. The median test
took 124 s (134 s in the run of `32c8bfb4`).

`results.tsv` is the runner's table (test, try, verdict, seconds, finished at, lane, LEZ chains
made so far by this run); `regress.log` is the runner's log, every try's start and verdict line
carrying the LEZ finalized height and tip. `logs/` holds the failed test's log and an excerpt of
the lane-2 Maker Node's log (below), as the tools printed them. None holds a local path. The
other per-test logs and the Node logs the harness kept are not committed; the lines quoted below
are from them.

## The failure: `node:btc:offline-after-second-lock:TakerSellsLez`

Lane 2, 06:25:28Z to 06:56:43Z. The scenario cuts Logos Chat and Delivery once both legs are
locked, kills the Taker, starts it again with the cut still in place, has the Taker claim, and
waits for the Maker's follow-up claim. The Maker published an offer at 06:25:43, the Taker took
it at 06:25:47 (swap `a5f63bc405f6`) and locked its LEZ (`b7e2bb5125d5`) at 06:25:48. At 06:26:15
the Taker was at `claim_available` and the Maker at `taker_lock_confirmed`; the cut went in, the
Taker was killed at 06:26:17 and back at 06:26:28, when the harness saw the Maker at
`both_legs_locked`. The Taker claimed its Bitcoin at 06:26:29 (`replay=False`). From then on the
harness saw, every minute for 30 minutes, "taker claim_in_progress (gen 3) | maker
both_legs_locked", and the test timed out:

    [06:26:29]   [0 s] waiting for the Taker's a5f63bc405f6 to reach completed (Maker follow-up claim): taker claim_in_progress (gen 2) | maker both_legs_locked | …
    [06:56:29] FAILED: timed out after 1800 s waiting for the Taker's a5f63bc405f6 to reach completed (Maker follow-up claim); last seen: taker claim_in_progress (gen 3) | maker both_legs_locked; …

The harness then saved both Nodes' logs (06:56:42). The lane-2 Maker Node's own log (its standard
error, the last 20,000 lines) holds eleven lines for the swap, its last eleven
(`logs/node-btc-offline-after-second-lock-TakerSellsLez-1.maker-node-excerpt.log`, the two JSON
lines being the Bitcoin actor's trace):

    maker actor a5f63bc4…: drive → awaiting_observation - (phase offered, revision 0, next observe_taker_first_lock)    (three times)
    maker actor a5f63bc4…: drive → observed_then_projected - (phase taker_lock_confirmed, revision 1, next observe_maker_second_lock_or_recover_taker_leg)
    {"event":"bitcoin_maker_step_observation","detail":"absent"}
    maker actor a5f63bc4…: drive → awaiting_observation - (phase taker_lock_confirmed, revision 1, …)
    {"event":"bitcoin_maker_step_observation","detail":"unspent"}
    maker actor a5f63bc4…: drive → observed_then_projected - (phase both_legs_locked, revision 2, next observe_revealing_claim)
    maker actor a5f63bc4…: drive → awaiting_observation - (phase both_legs_locked, revision 2, next observe_revealing_claim)    (twice)
    maker actor a5f63bc4…: drive attempt fails (actor_deployment_invalid: maker actor deployment artifact is unsafe)

So the Maker's Bitcoin actor drove normally through eight attempts: it waited for the Taker's
LEZ lock, saw it final, observed its own Bitcoin lock step absent and then unspent (the Taker saw
that lock and was at `claim_available` at 06:26:15), reached `both_legs_locked`, and twice found
no revealing claim yet. The ninth attempt failed before the
actor ran, as `actor_deployment_invalid`. Nothing follows that line: it is the last line the
Maker Node wrote in the 30 minutes up to the capture, for this swap or any other (the lane held
no other swap). The Node log has no timestamps; the swap's row in the Maker's database, read in
the lane's reset archive `20261118T065643Z` (copied out, read with `sqlite3` and deleted; not
committed, it holds the swap's keys), dates the failure: `maker_actor_processes` at
`schedule_state` **failed**, `last_failure_class` `actor_deployment_invalid`, `attempt_count` 9,
`lease_generation` 9, registered at 06:25:46 and last updated at 06:26:26; `maker_actor_progress`
from generation 9 at 06:26:26, phase `both_legs_locked`, next action `observe_revealing_claim`. A
failed schedule is not attempted again until an operator acts, so the actor never saw the
Taker's revealing claim of 06:26:29 and the Maker never made its follow-up LEZ claim. In the
same archive the actor's configuration and its other files were last written at 06:25:47 or
earlier, while its state database (`actor/state.sqlite3`) was last modified at 06:56:42, 30
minutes after the actor's last attempt, when no actor ran.

What this establishes, beside the [run of `32c8bfb4`](../regression-m4-2026-11-17-rc/), whose
failure (`node:btc:survivor:TakerSellsForeign`) was the same class read from the reset archive
alone:

- **Established**: the failure is the Maker's Bitcoin actor schedule failed permanently as
  `actor_deployment_invalid`, with the Node's own reason "maker actor deployment artifact is
  unsafe", on its ninth attempt after eight normal ones, three seconds before the Taker's claim;
  the schedule did not run again. This is the error `UnsafeArtifact` of
  `crates/swap-store/src/maker_actor_process.rs`, which the checks of the actor's configuration,
  program, state database and their directories all return.
- **Not established**: which artifact and which check refused. The line does not name them.
  That it was the state database's comparison of size and times (`validate_named_artifact` and
  `same_artifact`), raced by a write between the descriptor's and the name's `stat`, is the
  reading of the code given in the run of `32c8bfb4`: of those checks only that one compares
  something that changes between attempts when nothing is reinstalled, and the state database is
  written while the actor does not run (the 06:56:42 write above). This run adds the Node's
  wording and the time; it does not add the check.
- The Taker side is not involved: the Taker Node's saved log names `actor_deployment_invalid`
  only for `zec-devnet-swap-179498248402`, the swap of `node:zec:zec-tampered-catalog:TakerSellsLez`
  (06:14:40 to 06:15:54 on the same lane), whose configuration that test tampers with on purpose.
- A prepared fix that binds the state database by its inode, owner, mode and link count is not
  part of this commit.

In the 15 earlier runs kept on the host since 2026-11-13 that include this test, it passed 14
times (61 to 120 s); the 15th, on 2026-11-13, failed in 227 s because the Taker container did not
become healthy within 180 s, a different cause. `node:btc:offline-after-second-lock:TakerSellsForeign`
passed here in 84 s on lane 1, and every other Bitcoin test passed on its first try.

## The NOT-EXERCISED tries

`node:zec:zec-claim-collision:TakerSellsForeign` needs two Taker swaps whose LEZ transactions
collide on the owner account's nonce: A's revealing claim must lose its nonce to C's lock. On
tries 1 and 2 (lane 1, 98 and 89 s) C's lock lost the nonce instead and was re-planned ("re-planned
[1 lez_fund nonce 43→44 …]", then 46→47), and the scenario said so:

    FAILED: NO-COLLISION: A completed without a superseded claim (C's lock was in a block before
    A's claim was planned, or C's lock lost the nonce instead: … try again

The runner recorded each NOT-EXERCISED and ran it again; try 3 (lane 1, 85 s) produced the
collision ("revealing LEZ claim superseded (planned nonce 48, live 49); re-plan 1 at nonce 49")
and passed. The harness kept both Nodes' logs for each NOT-EXERCISED try; none was needed.

## The two tests fixed since the run of `32c8bfb4`

Both failed once in a full run of 2026-11-17 evening (`20261117T205115Z`, not committed) and are
fixed by commits of this tree; both passed here on their first try:

- `node:xmr:xmr-monerod-down:TakerSellsLez` (lane 3, 523 s): both swaps "settled completed"; the
  second with "monerod was down 81 s (the funding cutoff is 237 s away); at its end maker
  taker_lock_confirmed, taker taker_lock_confirmed" (`44e39081`).
- `node:btc:kill-taker:TakerSellsForeign` (lane 1, 894 s): every kill point completed or
  refunded as the scenario expects. No claim after a restart was refused here, so the new replay
  (`2f0fb725`) did not fire: its log line, "claim after the restart answered
  taker_action_execution_unavailable", is in none of the four kill-taker tests' logs.

## The Monero tests

All 16 passed on their first try. All are TakerSellsLez (the only Monero direction), against
Monero regtest on the devnet (per lane a `monerod` with funding, Maker and Taker
`monero-wallet-rpc`), `fast` profile. Each swap's windows, as the logs print them: "the Maker
funds by +320 s, the Taker refunds from +720 s, the Maker punishes from +1000 s", as in the run of
`32c8bfb4`. Lot 1 XMR against 50,000 LEZ units.

| test | lane | s | s in the run of `32c8bfb4` | what its log shows |
|---|---|---:|---:|---|
| `node:xmr:xmr-happy` | 1 | 214 | 200 | "settled completed: LEZ moved {'maker': 50000, 'taker': -50000} as both Nodes say"; the Taker's wallet received 0.9981832 XMR. |
| `node:xmr:xmr-taker-refund` | 1 | 870 | 881 | "settled refunded: LEZ moved {'maker': 0, 'taker': 0}"; the Maker's wallet swept its Monero back (0.9981784 XMR), the Taker's received nothing. |
| `node:xmr:xmr-maker-never-funds` | 3 | 794 | 795 | "settled refunded", "no Monero moved: the Maker's wallet sent nothing, the Taker's received nothing". |
| `node:xmr:xmr-punish` | 2 | 1,088 | 1,088 | "the Maker punished; its LEZ owner's nonce moved 13 → 14"; neither wallet received Monero, the funding "stays at the shared address: neither party received it". |
| `node:xmr:xmr-concurrent` | 2 | 312 | 243 | "settled both: LEZ moved {'maker': 100000, 'taker': -100000}", the Taker's wallet received two outputs. |
| `node:xmr:xmr-kill-taker` | 1 | 1,499 | 1,567 | "taker killed at 7 points: after the take → completed; lock in flight → completed; locked → completed; Monero verified → completed; claim authorised → completed; waiting for the Maker's claim → completed; sweeping → completed". |
| `node:xmr:xmr-kill-maker` | 2 | 1,388 | 1,498 | "maker killed at 6 points: after the take → completed; lock observed → completed; funding in flight → completed; funded → completed; claim in flight → completed; claim tool running → completed". |
| `node:xmr:xmr-taker-refund-kill` | 2 | 872 | 872 | The Taker killed right after its refund's admission ("back up: refund_in_flight"); "settled refunded", the Maker's wallet swept its Monero back. |
| `node:xmr:xmr-monerod-down` | 3 | 523 | 527 | Before a take "the Maker withdrew offer … and refuses to publish on Monero: chain dependency is unavailable for route"; mid-swap "monerod was down 81 s (the funding cutoff is 237 s away)"; both swaps completed. |
| `node:xmr:xmr-wallet-restart` | 1 | 354 | 390 | Both swaps "settled completed" with a role's `monero-wallet-rpc` restarted mid-swap. |
| `node:xmr:xmr-outage-before-take` | 1 | 240 | 198 | "the Taker refused the take: Taker dependency unavailable (authenticated_delivery_unavailable)"; restored, the swap completed. |
| `node:xmr:xmr-cut-after-lock` | 3 | 232 | 186 | "the cut held: 53 samples of the Maker's Chat socket, none with a connection"; "settled completed". |
| `node:xmr:mixed-three-pair` | 3 | 237 | 283 | A Monero, a Bitcoin and a Zcash swap of the same Nodes at once: "all three completed; LEZ transactions signed by the owner accounts meanwhile: {'maker': 1, 'taker': 7}". |
| `ui:xmr:xmr-happy` | 1 | 218 | 199 | The Monero swap clicked through both desks, as in the recorded run. |
| `ui:xmr:xmr-taker-refund` | 3 | 968 | 959 | The Taker desk's refund journey: "the Maker Node has swept its Monero back, and both desks show the swap refunded". |
| `ui:xmr:xmr-concurrent` | 2 | 297 | 249 | Two Monero swaps at once on both desks, as in the recorded run. |

The Monero tests took 168.4 minutes summed over lanes (168.9 in the run of `32c8bfb4`).

## What changed since the run of `32c8bfb4`

`git log --oneline 32c8bfb4..44e39081` holds four commits:

- **Monero funding during a `monerod` outage** (Maker Node, Monero only): before it spends the
  swap's one funding invocation the Maker's Monero actor asks `monerod` (one `get_info`) and,
  while it does not answer, changes nothing and tries on a later drive, never at or after the
  funding cutoff; a funding that has started keeps its rules (`44e39081`).
- **Runner, Node logs**: a try that ends FAILED, TIMEOUT or NOT-EXERCISED keeps both Nodes'
  container log, own log, newest Basecamp log and owner health answer under
  `logs/<try>.nodes`, with Basecamp's Delivery dumps dropped and secret-shaped values redacted;
  the Bitcoin actors' trace (`LEZ_BTC_ACTOR_TRACE`) is on in the devnet's `runtime.env`
  (`582d6c47`).
- **Runner, kill walk**: a Taker claim made right after a Taker restart and refused as
  `taker_action_execution_unavailable` is replayed under the same request id, at most six times
  over 60 s; any other refusal still fails (`2f0fb725`).
- **Docs**: the run of `32c8bfb4`'s evidence (`51a93b3f`).

The Bitcoin engine's crates (`crates/btc-core-adapter`, `crates/btc-reference-actor`,
`crates/swap-store`) are unchanged since `32c8bfb4`. The Bitcoin-adjacent changes are the
runner's: the actor trace in the Node logs and the kill walk's replay.

## Lint and unit tests on the same commit

Right before the run (03:53Z), in a checkout of `44e39081`: the workspace's clippy exited 0, the
clippy of the Maker and Taker Node crates with all features exited 0, and the Monero Maker
actor's unit tests (`crates/maker-node/src/bin/xmr-maker-actor.rs`) passed 19 of 19.

## Source and images

Tests and images from `44e39081`, tree clean (`regress.log` prints the commit and "tree dirty:
0"). Image ids, creation times and the build-source check are in [PROVENANCE.md](PROVENANCE.md).

## Not covered

- Monero beyond an offline regtest chain: no stagenet or mainnet; x86_64 (linux/amd64) only.
- No Monero test runs on the `refund` profile, and no run uses `local`.
- The devnet images run development builds. No release package was built or run from this
  commit.
- The systemd VM run with a Monero role (`scripts/run-maker-node-systemd-vm.sh`) was not run; the
  Maker unit's Monero role is checked statically and with stub programs only.
- The Monero punish rows and the Taker's in-doubt rows were not seen on a desk
  ([regression-m4-desks-2026-11-16](../regression-m4-desks-2026-11-16/)).
- Proven by unit tests only, not on the stack: the lost-nonce punish re-plan; the switch of a
  Maker's stopped claim to the refund or the punishment; a claim submitted but not recorded; the
  regtest observer mining a funding or refund sweep left in the pool; the Monero funding's wait
  for `monerod` (`44e39081`), whose Node log line was not read here: the harness keeps Node logs
  only for a try that did not pass.
- A Monero transfer or sweep that hits the Digest failure is not made again; the swap is left in
  doubt (on regtest the observer mines what reached the pool).
- Owner-account nonces, as the 2026-11-15 note says: the Taker's tag 14 and the Maker's tag 17
  are signed without coordination with the other pairs' transactions.
- Two Bitcoin-adjacent defects are not fixed in this tree: the Maker's check of its actor's
  artifacts that fails a schedule permanently as `actor_deployment_invalid` ("maker actor
  deployment artifact is unsafe"), which failed `node:btc:offline-after-second-lock:TakerSellsLez`
  here (above; `node:btc:survivor:TakerSellsForeign` on 2026-11-17, `ui:btc:happy:TakerSellsLez`
  on 2026-11-13); and the Maker's Bitcoin claim journaled `unknown` after one submission attempt
  and never sent (`node:btc:restart-maker:TakerSellsForeign` on 2026-11-16, see
  [that note](../regression-m4-2026-11-16/); `node:btc:offline-after-claim:TakerSellsForeign` on
  2026-11-13), whose fix rechecks readiness before the broadcast and changes the frozen Bitcoin
  engine. The second did not show here: `node:btc:restart-maker:TakerSellsForeign` passed in
  70 s and `node:btc:offline-after-claim:TakerSellsForeign` in 83 s.
- The Maker Node's line for `actor_deployment_invalid` names the class and "deployment artifact
  is unsafe", not the artifact or the check; which one refused is inferred from the code.
- macOS, linux/arm64, and any public network are untested on this head.
