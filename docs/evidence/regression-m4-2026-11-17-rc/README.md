# Full regression of `32c8bfb4`, 2026-11-17

One run of every test of `deploy/scripts/regress.sh full` (159: `regress.sh list full`) on the
Docker devnet with the Monero regtest chain up (`LEZ_MONERO=1`), three lanes at a time
(`LEZ_LANES=3`), without desk recordings: run `runtime/regress/20261117T141249Z`, 2026-11-17
14:12:49Z to 18:14:22Z (UTC), at `32c8bfb4`. It follows the
[run of the same morning](../regression-m4-2026-11-17/) (159 of 159 at `7d09351a`) with the same
159 tests, on a tree that adds the six commits listed below.

**Result: 158 of 159 passed in one run of 241 minutes; one failed,
`node:btc:survivor:TakerSellsForeign` (below).** Every test ran once: 158 passed on their first
try, the failed one timed out on its first try, which is the run's only timeout. No
NOT-EXERCISED try, 0 frozen tries; 1 new LEZ chain (both `zec-balance-refusal` tests at once on
it); 3 timing-profile switches. All 16 Monero tests passed. Test time summed over lanes 38,766 s
over 159 tries (34,932 s in the morning run).

| kind | tests | minutes (summed over lanes) | morning run (`7d09351a`): minutes |
|---|---:|---:|---:|
| `node:btc` | 53 | 237.4 | 196.2 |
| `node:zec` | 51 | 133.8 | 123.3 |
| `node:xmr` | 13 | 145.5 | 141.2 |
| `ui:btc` | 17 | 50.0 | 44.8 |
| `ui:zec` | 22 | 56.0 | 53.8 |
| `ui:xmr` | 3 | 23.4 | 22.9 |
| all | 159 | 646.1 | 582.2 |

(`results.tsv`, column 4 in seconds; both runs have one try per test. `node:btc` includes the
failed try's 1,518 s; without it the group is 212.1 minutes.)

Where the 27 minutes of wall time went, by group (`regress.log`): `fast` 185.2 minutes (160.2 in
the morning run), `zec-v6` 26.6 (25.9), `refund` 23.8 (22.2), the own-chain pair 5.4 (5.5).
The `fast` group holds 25 of them: the failed test held lane 3 for 25.3 minutes where it passed
in 83 s in the morning, and lanes 1 and 2 had no test left and waited for it from 17:11:40Z to
17:18:33Z. The Monero tests took 4.8 minutes more summed over lanes (164.1 to 168.9), as the
Monero windows are longer (+320, +720, +1000 s, from +290, +650, +910 s; `32c8bfb4`): the
tests that wait for the refund or punish time wait that much longer. The 158 other tests took 8%
longer in the median here than in the morning run; this run does not show why (none of the
changes below touches their path, other than `reset-swaps.sh`, whose time is not in a test's
seconds: the gap between a test's start line and its verdict beyond its own seconds was 11 s in
the median here, 9 s in the morning run). The morning run started with a switch to `fast`, this
one on `fast` already, hence 3 switches instead of 4.

`results.tsv` is the runner's table (test, try, verdict, seconds, finished at, lane, LEZ chains
made so far by this run); `regress.log` is the runner's log, every try's start and verdict line
carrying the LEZ finalized height and tip. `logs/` holds the failed test's log and, in
`reruns/`, the runner output, table and Bitcoin test logs of the five reruns (below), as the
runner printed them. None holds a local path. The other per-test logs are not committed; the
lines quoted below are from them.

## The failure: `node:btc:survivor:TakerSellsForeign`

Lane 3, 16:53:01Z to 17:18:33Z. The Maker published an offer at 16:53:17, the Taker took it at
16:53:23 (swap `1dd5df0b16c5`) and locked its Bitcoin (`ad56352c0d45`) in the same second. The
Maker saw that lock confirmed and never locked its LEZ; the test timed out after 1,500 s, before
the step where the scenario stops the Taker:

    [16:53:28]   [4 s] waiting for the Taker's 1dd5df0b16c5 to reach claim_available (Maker funding): taker awaiting_second_lock (gen 1) | maker taker_lock_confirmed | …
    [17:18:26] FAILED: timed out after 1500 s waiting for the Taker's 1dd5df0b16c5 to reach claim_available (Maker funding); last seen: taker awaiting_second_lock (gen 1) | maker taker_lock_confirmed; …

What the Maker kept (its swap state, moved to the Node's reset archive when the harness reset
the Nodes after the test; read there, not committed: it holds the swap's keys):

- `maker_actor_processes`: one row, the swap's Bitcoin actor, `desired_state` running,
  `schedule_state` **failed**, `last_failure_class` **`actor_deployment_invalid`**,
  `attempt_count` 2, `lease_generation` 2, registered at 16:53:23 and last updated at 16:53:27.
  A failed schedule is not attempted again until an operator acts, so nothing ran for the swap
  after 16:53:27.
- `maker_actor_progress`: one row, from the first attempt (generation 1, 16:53:23): state
  active, phase `taker_lock_confirmed`, next action
  `observe_maker_second_lock_or_recover_taker_leg`. The second attempt left none.
- The actor's own state database: the swap's aggregate at revision 1, phase
  `TakerLockConfirmed`, no Maker lock transaction; evidence of the Taker's lock with 1
  confirmation; the public-effect journal and the Maker lock intents and steps empty. No LEZ
  lock was ever prepared, so none was submitted and none can have been lost at the sequencer.
  The Node's application record of the swap stayed at phase `Offered`.
- The pinned configuration's SHA-256 equals the configuration file's; the actor program and the
  configuration were not touched after the take.
- The actor's state database was last modified at 17:19:40, 26 minutes after the actor's last
  attempt: something other than the actor wrote it while the actor was not running.

`actor_deployment_invalid` is the class the Maker's actor supervisor gives a check of the
actor's artifacts (configuration, program, state database) that refuses them before the child
runs; it is permanent by design. The Node prints which check refused (`11ca8461`), but on its
own standard error, and that log was lost: the lane-3 Maker container was recreated at 17:45Z
by a later test, before the diagnosis. Which check refused is therefore inferred, not read:

- Of those checks only one compares something that changes between two attempts when nothing
  is reinstalled: the state database's size and modification and change times
  (`validate_named_artifact` in `crates/swap-store/src/maker_actor_process.rs`, the descriptor
  opened before the attempt against the name, twice per attempt). The configuration and the
  program are compared the same way but are not written after the take; the directories are
  compared by inode only.
- The state database is written by others between attempts: for a Bitcoin swap the Node's
  `maker_actor_monitor_v1` reads the actor's effects in place (`btc_reference_actor::actor_effects`),
  and opening the store migrates and checkpoints it. The harness calls `maker_actor_monitor_v1`
  every few seconds while it waits (`maker_phase` in `scripts/node-e2e.py`), and the 17:19:40
  write above is such a read.
- A write that lands between the two `stat` calls makes the comparison fail, and the attempt
  fails as `actor_deployment_invalid`. This is the defect already listed in the morning run's
  "Not covered" (`ui:btc:happy:TakerSellsLez`, 2026-11-13), and the same signature as the failure
  of 2026-11-12 in [darwin-catalog-install-2026-11-12](../darwin-catalog-install-2026-11-12/)
  (`actor_deployment_invalid` within seconds of the swap's registration, hashes unchanged).
  A fix that binds the state database by its inode, owner, mode and link count and keeps the
  full comparison for the configuration and the program is prepared
  (`fix(swap-store): an actor's state database is bound by its inode, not its times`) and is
  not in this tree.

It is not one of this week's changes: none of the commits since `7d09351a` touches
`crates/swap-store`, the Maker's actor supervisor or the Bitcoin crates; the reset's new wait
for Delivery and Chat ran before the test and the test failed before its kill; the offer was
discovered and taken normally. Nor is it the Maker's one-LEZ-transaction-in-flight coordination:
the Maker held this one swap, and an effect held back backs off with a class of its own instead
of failing the schedule. In the fifteen earlier full runs kept on the host since 2026-11-11
(the morning run among them) the test passed every time (80 to 168 s).

### Five reruns on the same images

`LEZ_MONERO=1 LEZ_LANES=3 scripts/regress.sh run node:btc:survivor:TakerSellsForeign
node:btc:survivor:TakerSellsLez node:zec:zec-survivor:TakerSellsForeign`, five times one after
the other (18:21:06Z to 18:34:01Z), same images, no rebuild, on a new LEZ chain (`up.sh
--fresh-lez` after the reset archive had been read). The runner put the failed test on lane 1,
`node:btc:survivor:TakerSellsLez` on lane 2 and the Zcash test on lane 3 each time. All 15
passed on their first try, 0 frozen:

| run | `node:btc:survivor:TakerSellsForeign` s | Taker's lock → Maker at `taker_lock_confirmed` → Taker at `claim_available` | `node:btc:survivor:TakerSellsLez` s | Taker's lock → Maker at `taker_lock_confirmed` → Taker at `claim_available` | `node:zec:zec-survivor:TakerSellsForeign` s |
|---|---:|---|---:|---|---:|
| `20261117T182106Z` | 85 | 18:21:57 → 18:22:02 → 18:22:43 (+46 s) | 75 | 18:22:02 → 18:22:35 → 18:22:35 (+33 s) | 106 |
| `20261117T182340Z` | 87 | 18:24:31 → 18:24:36 → 18:25:18 (+47 s) | 76 | 18:24:34 → 18:24:55 → 18:25:04 (+30 s) | 108 |
| `20261117T182615Z` | 77 | 18:27:02 → 18:27:07 → 18:27:41 (+39 s) | 96 | 18:27:03 → 18:27:35 → 18:27:35 (+32 s) | 107 |
| `20261117T182843Z` | 95 | 18:29:33 → 18:29:37 → 18:30:19 (+46 s) | 87 | 18:29:35 → 18:30:10 → 18:30:10 (+35 s) | 107 |
| `20261117T183118Z` | 99 | 18:32:17 → 18:32:24 → 18:33:06 (+49 s) | 89 | 18:32:17 → 18:32:46 → 18:32:53 (+36 s) | 110 |

(The harness polls and prints a line when a state changes, so the times are its first lines that
saw each state; `claim_available` is the Taker seeing the Maker's lock, LEZ when the Taker sells
Bitcoin and Bitcoin when it sells LEZ. In the failed run the Maker reached
`taker_lock_confirmed` 5 s after the Taker's lock, as here, and never locked.) From the Maker's
`taker_lock_confirmed` to the Taker's `claim_available` took 34 to 42 s selling Bitcoin and 0 to
9 s selling LEZ. Fifteen passes fit a race that strikes now and then; they do not show it is
gone.

## The Monero tests

All 16 passed on their first try. All are TakerSellsLez (the only Monero direction), against
Monero regtest on the devnet (per lane a `monerod` with funding, Maker and Taker
`monero-wallet-rpc`), `fast` profile. Each swap's windows, as the logs print them: "the Maker
funds by +320 s, the Taker refunds from +720 s, the Maker punishes from +1000 s" (+290, +650,
+910 s in the morning run). Lot 1 XMR against 50,000 LEZ units.

| test | lane | s | s in the morning run | what its log shows |
|---|---|---:|---:|---|
| `node:xmr:xmr-happy` | 1 | 200 | 212 | "settled completed: LEZ moved {'maker': 50000, 'taker': -50000} as both Nodes say"; the Taker's wallet received 0.9981844 XMR. |
| `node:xmr:xmr-taker-refund` | 1 | 881 | 801 | "settled refunded: LEZ moved {'maker': 0, 'taker': 0}"; the Maker's wallet swept its Monero back (0.9981808 XMR), the Taker's received nothing. |
| `node:xmr:xmr-maker-never-funds` | 3 | 795 | 719 | "settled refunded", "no Monero moved: the Maker's wallet sent nothing, the Taker's received nothing". |
| `node:xmr:xmr-punish` | 2 | 1,088 | 989 | "the Maker punished; its LEZ owner's nonce moved 13 → 14"; neither wallet received Monero, the funding "stays at the shared address: neither party received it". |
| `node:xmr:xmr-concurrent` | 1 | 243 | 301 | "settled both: LEZ moved {'maker': 100000, 'taker': -100000}", the Taker's wallet received two outputs. |
| `node:xmr:xmr-kill-taker` | 1 | 1,567 | 1,584 | "taker killed at 7 points: after the take → completed; lock in flight → completed; locked → completed; Monero verified → completed; claim authorised → completed; waiting for the Maker's claim → completed; sweeping → completed". |
| `node:xmr:xmr-kill-maker` | 2 | 1,498 | 1,433 | "maker killed at 6 points: after the take → completed; lock observed → completed; funding in flight → completed; funded → completed; claim in flight → completed; claim tool running → completed". |
| `node:xmr:xmr-taker-refund-kill` | 2 | 872 | 831 | The Taker killed right after its refund's admission ("back up: refund_in_flight"); "settled refunded", the Maker's wallet swept its Monero back. |
| `node:xmr:xmr-monerod-down` | 3 | 527 | 462 | Before a take "the Maker withdrew offer … and refuses to publish on Monero: chain dependency is unavailable for route"; mid-swap "monerod was down 87 s (the funding cutoff is 232 s away)"; both swaps completed. |
| `node:xmr:xmr-wallet-restart` | 1 | 390 | 430 | Both swaps "settled completed" with a role's `monero-wallet-rpc` restarted mid-swap. |
| `node:xmr:xmr-outage-before-take` | 2 | 198 | 216 | "the Taker refused the take: Taker dependency unavailable (authenticated_delivery_unavailable)"; restored, the swap completed. |
| `node:xmr:xmr-cut-after-lock` | 1 | 186 | 243 | "the cut held: 48 samples of the Maker's Chat socket, none with a connection"; "settled completed". |
| `node:xmr:mixed-three-pair` | 3 | 283 | 254 | A Monero, a Bitcoin and a Zcash swap of the same Nodes at once: "all three completed; LEZ transactions signed by the owner accounts meanwhile: {'maker': 1, 'taker': 7}". |
| `ui:xmr:xmr-happy` | 3 | 199 | 212 | The Monero swap clicked through both desks, as in the recorded run. |
| `ui:xmr:xmr-taker-refund` | 3 | 959 | 869 | The Taker desk's refund journey: "the Maker Node has swept its Monero back, and both desks show the swap refunded". |
| `ui:xmr:xmr-concurrent` | 2 | 249 | 290 | Two Monero swaps at once on both desks, as in the recorded run. |

The tests that wait for the refund or the punish time took 40 to 100 s longer, as their windows
are; the others are within the spread of a run.

## What changed since the morning run

`git log 7d09351a..32c8bfb4` holds six commits:

- **Monero windows**: a Maker drive counted at 80 s instead of 70 s, so on `fast` C, R, P =
  +320, +720, +1000 s (were +290, +650, +910), on `refund` +320, +720, +955 s, on `local`
  +1820, +2220, +2740 s; Bitcoin and Zcash values unchanged (`32c8bfb4`).
- **Monero take speed**: the shared Monero address is derived from the two cross-curve DLEQ
  proofs without verifying them a second time (they exist only verified); what is computed and
  every check that can fail are unchanged (`aa5fb7ba`, its doc `bcc2b1c6`).
- **Offer discovery** (shared Node code, all three pairs): an offer withdrawn between the
  Delivery listing and its read is skipped instead of failing the whole discovery (`4059e56c`).
- **Runner**: `reset-swaps.sh` returns only when both Nodes report Delivery and Chat available in
  the same health answer, not only ready (`6ef0ecd3`).
- **Docs**: the morning run's evidence (`cb3d1f80`).

The Bitcoin engine's crates (`crates/btc-core-adapter`, `crates/btc-reference-actor`,
`crates/swap-store`) are unchanged since `7d09351a`.

Between the two runs, a full run of `e026fb0c` (`20261117T063300Z`, 06:33:00Z to 10:07:26Z)
passed 158 of 159 in 214 minutes; `node:zec:zec-wrong-inputs:TakerSellsForeign` failed on the
discovery race that `4059e56c` fixes. `e026fb0c` and the commit under it
(`build(dev): optimize the Maker Node crate in development builds`) are not in this history, so
that run is not evidence for `32c8bfb4` and is not committed here.

## Source and images

Tests and images from `32c8bfb4`, tree clean (`regress.log` prints the commit and "tree dirty:
0"). Image ids, creation times and the build-source check are in [PROVENANCE.md](PROVENANCE.md).

## Not covered

- Monero beyond an offline regtest chain: no stagenet or mainnet; x86_64 (linux/amd64) only.
- The Monero windows of the `refund` and `local` profiles changed with `32c8bfb4`; no Monero test
  runs on `refund`, and no run uses `local`.
- The devnet images run development builds. No release package was built or run from this
  commit.
- The systemd VM run with a Monero role (`scripts/run-maker-node-systemd-vm.sh`) was not run; the
  Maker unit's Monero role is checked statically and with stub programs only.
- The Monero punish rows and the Taker's in-doubt rows were not seen on a desk
  ([regression-m4-desks-2026-11-16](../regression-m4-desks-2026-11-16/)).
- Proven by unit tests only, not on the stack: the lost-nonce punish re-plan; the switch of a
  Maker's stopped claim to the refund or the punishment; a claim submitted but not recorded; the
  regtest observer mining a funding or refund sweep left in the pool. Whether the read and mining
  retries fired in this run was not checked: the Node logs are not kept with the run.
- A Monero transfer or sweep that hits the Digest failure is not made again; the swap is left in
  doubt (on regtest the observer mines what reached the pool).
- Owner-account nonces, as the 2026-11-15 note says: the Taker's tag 14 and the Maker's tag 17
  are signed without coordination with the other pairs' transactions.
- Two Bitcoin-adjacent defects are not fixed in this tree: the Maker's check of its actor's
  state database by size and times, which failed `node:btc:survivor:TakerSellsForeign` here
  (above; earlier `ui:btc:happy:TakerSellsLez`, 2026-11-13); and the Maker's Bitcoin claim
  journaled `unknown` after one submission attempt and never sent
  (`node:btc:restart-maker:TakerSellsForeign` on 2026-11-16, see
  [that note](../regression-m4-2026-11-16/); `node:btc:offline-after-claim:TakerSellsForeign` on
  2026-11-13), whose fix rechecks readiness before the broadcast and changes the frozen Bitcoin
  engine. The second did not show here: `node:btc:restart-maker:TakerSellsForeign` passed in
  92 s and `node:btc:offline-after-claim:TakerSellsForeign` in 76 s.
- The Maker's actor supervisor prints the check behind `actor_deployment_invalid` only to the
  Node's log, which the harness does not keep; a failure of that class is diagnosed from the
  reset archive, as here, without the check's name.
- macOS, linux/arm64, and any public network are untested on this head.
