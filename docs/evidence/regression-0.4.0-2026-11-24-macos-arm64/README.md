# Full regression of `957c8016`, 2026-11-24/25, macOS arm64

This is every test of `deploy/scripts/regress.sh full` (179: `regress.sh list full`), run on
the Docker devnet. The Monero regtest chain was up (`LEZ_MONERO=1`) and the devnet test hooks
were built in (`LEZ_TEST_HOOKS=1`). Tests ran two at a time on two lanes (`LEZ_LANES=2`),
without desk recordings, from 2026-11-24 21:54:13Z to 2026-11-25 04:05:12Z (UTC), at
`957c8016`.

One operator ran both parties, Maker and Taker, on one host: an Apple-silicon Mac (macOS
26.7.3, 10 cores), Docker Desktop 29.9.2, local linux/arm64 dev-profile images, Monero regtest.
No second operator and no second host took part.

**Result: 179 of 179 passed, each on its first try.** 179 tries in all (`results.tsv`): 179
PASSED, no FAILED or NOT-EXERCISED try, no timeout, 0 frozen tries. No test was rerun.

The tier ran in two runner invocations on the same stack and images:

| invocation | command | UTC | tests | result |
|---|---|---|---:|---|
| 1 (`run-1-full/`) | `regress.sh full` | 21:54:13Z to 02:41:37Z (287 min) | 150 | 150 passed, then the runner stopped |
| 2 (`run-2-remaining/`) | `regress.sh run <the 29 left>` | 02:44:44Z to 04:05:12Z (80 min) | 29 | 29 passed |

Invocation 1 stopped at its group `zec-v6`, on its first new chain. The runner had been
started without `LEZ_WORKSPACE` and `LEZ_PROVISION`, the workspace the stack was built in. So
`bootstrap-market.sh` looked for the escrow deployer under a default directory, did not find
it, and `up.sh` reported each new chain as one that "does not finalize"
(`run-1-full/logs/profile-1.log`, `chain-1.log`, `chain-2.log`). No test had started on those
chains. With those two variables set, `up.sh --fresh-lez` made a new chain with the escrow
deployed again (`run-2-remaining/recovery-up-fresh-lez.log`, 02:43:11Z to 02:44:31Z). Invocation
2 then ran the 29 tests that had no try yet; `results.tsv` is the two runners' tables one after
the other. Wall time from the first start to the last verdict: 371 minutes.

| kind | tests | minutes (summed over lanes) |
|---|---:|---:|
| `node:btc` | 56 | 156.6 |
| `node:zec` | 59 | 142.8 |
| `node:xmr` | 22 | 287.9 |
| `ui:btc` | 17 | 33.8 |
| `ui:zec` | 22 | 48.2 |
| `ui:xmr` | 3 | 21.8 |
| all | 179 | 691.0 |

The median test took 84 s. The tests ran on 3 LEZ chains: the stack's, the one made after
invocation 1, and one of their own for both `zec-balance-refusal` tests, which ran at once on
lanes 1 and 2. There were 3 timing-profile switches in invocation 2: to `zec-v6` at 02:45:02Z,
to `refund` at 03:32:32Z, and back to `fast` for the own-chain pair at 04:01:36Z.

## The tree

`957c8016` is `27ea6231` plus the harness fix that came out of the stopped first attempt
(below), one new test, an offline SDK example, CI and documents.
`git diff --name-only 27ea6231 957c8016` lists 18 files (`diff-27ea6231-957c8016.txt`). None is
under a shipped crate's `src/`, under `compat/*/src` or under `apps/`. The test-and-harness
files are `deploy/scripts/{up.sh,regress.sh,node-e2e.py}` and their tests under
`deploy/tests/`. The SDK files are `crates/xmr-swap-sdk/{Cargo.toml,README.md,examples/,tests/}`.
The rest are `.github/workflows/ci.yml`, `scripts/test-ci-hardening-policy.sh` and five
documents.

## The first attempt at `27ea6231`

`first-attempt-27ea6231/` holds the stopped run of `regress.sh full` at `27ea6231` (178 tests,
2026-11-24 18:34:22Z to 21:28:42Z): 9 passed, 2 failed for reasons outside the Nodes, 2 tries
voided by a LEZ freeze, 167 not run. Its README gives the causes. The runner stopped after the
freeze: on the restart of `lez-network` for the new chain, Docker Desktop dropped the indexer's
host port, and `up.sh` ended at the first refused read instead of polling. `957c8016` fixes
`up.sh`. Both tests that failed there passed here on their first try:
`node:xmr:xmr-maker-early-punish` (1,079 s) and `node:xmr:xmr-custody-credit-refund` (854 s).
None of the first attempt's results are counted above.

## The new tests

**`node:zec:zec-activate-after-expiry:TakerSellsLez`**, first run on a devnet: passed, lane 2,
111 s. Its log shows:

- the offer was published with a 60 s TTL and taken 0 s later;
- "the Maker's activation of zec-c0e8c117 is held (test_hook_activation_held); taker
  awaiting_first_lock; stopping the Maker Node";
- the offer expired with the Maker down, the Maker was started again with the hold still set,
  and the hold was cleared 18 s after the expiry;
- "the Maker activated zec-c0e8c117 28 s after the offer expired";
- the Taker claimed the Zcash, the Maker submitted `['lez_followup_claim']`, and both Nodes
  report the swap completed, the Maker still after its restart;
- "Maker lock attempt count 2 (projected the Taker's lock 1 time(s); 10 supervisor attempts over
  the swap); Taker LEZ re-plan count 0".

The delayed activation kept `zec_happy_after_take`'s assertions: 2 lock attempts against the
allowed 1 to 2, and exactly 1 projection of the Taker's lock.

**`node:zec:zec-claim-at-one-confirmation:TakerSellsForeign`**, first run on a devnet: passed,
lane 2, 44 s. "the Node reports the claim queued: {'kind': 'zcash_funding_confirmations',
'chain': 'zcash', 'confirmations': 1, 'required_confirmations': 2} (last failure class None);
mining the second confirmation".

## The kill and adversarial tests

All passed on their first try (`results.tsv`; each log under `run-1-full/logs/` or
`run-2-remaining/logs/`):

- `node:zec:zec-kill-maker-claim:TakerSellsLez`, 157 s: "the Maker killed at claim preparation
  held → completed; claim submission in flight → completed".
- `node:xmr:xmr-kill-maker:TakerSellsLez`, 1,601 s: "maker killed at 7 points: after the take →
  completed; lock observed → completed; funding in flight → completed; funded → completed; claim
  in flight → completed; claim preparation held → completed; claim tool running → completed".
- `node:xmr:xmr-kill-taker` 1,394 s, `xmr-taker-refund-kill` 854 s, `xmr-unfunded-punish`
  2,263 s, `xmr-taker-cheats` 1,637 s, `xmr-maker-early-punish` 1,079 s, `xmr-punish-replans`
  1,090 s, `xmr-punish` 1,069 s, `xmr-taker-clock-ahead` 845 s.
- `node:btc:kill-maker` (1,131 and 738 s) and `node:btc:kill-taker` (1,077 and 696 s), both
  directions; `node:btc:concurrent-kill-maker` and `node:zec:zec-concurrent-kill-maker`, both
  directions; `node:zec:zec-kill-maker` and `zec-kill-taker`, both directions.
- The custody credits (TM-0032): `btc-custody-credit` both directions, `btc-custody-credit-refund`
  (304 s), `zec-custody-credit` both directions, `zec-custody-credit-refund` (553 s),
  `xmr-custody-credit` (196 s), `xmr-custody-credit-refund` (854 s).
- `node:zec:zec-slow-funding:TakerSellsLez`, 125 s.

## Files

- `results.tsv`: the two runners' tables one after the other (test, try, verdict, seconds,
  finished at, lane, LEZ chains made so far by that invocation).
- `run-1-full/`, `run-2-remaining/`: each invocation's output as the runner wrote it:
  `results.tsv`, `regress.log`, `chain.tsv`, `logs/` (every try's log, the profile and chain
  logs, the lane logs), `e2e/` (the scenarios' summaries) and `zec/` (the ZEC swap exports).
  `e2e/` and `zec/` are the stack's evidence directory at the end of each invocation, so
  invocation 2's also holds invocation 1's files.
- `first-attempt-27ea6231/`: the stopped run at `27ea6231`, with its own README.
- `diff-27ea6231-957c8016.txt`: `git diff --name-only 27ea6231 957c8016`.
- `OUTSIDE-GIT.sha256`: the SHA-256 of the two archives of the first attempt's kept Node logs,
  which are not committed. No desk recordings were made, so there are no videos.
- `PROVENANCE.md`: build source, images, the stack, privacy and times.
- `RESULTS.txt`: the count in short.
- `SHA256SUMS`: every file here but itself.

`results.tsv`, `chain.tsv` and the `regress.log` files are in UTC. The per-test logs of
`node-e2e.py` and `ui-e2e.sh` print the host's local time (CET, UTC+1).

## Not covered

- Lint, unit tests, the CI quality gates and gitleaks over the repository were not run on this
  host for this set. The set itself was scanned (PROVENANCE).
- No run uses the `local` profile. No Monero test runs on the `refund` profile. The block
  explorers are not part of any test.
- Monero beyond an offline regtest chain. Monero Stagenet is not part of `regress.sh full`.
- arm64 (linux/arm64 under Docker Desktop on macOS) only. Two operators on two hosts: one
  operator ran both parties on one host.
- The devnet images run development builds with the devnet test hooks. No release package was
  built or run from this commit.
