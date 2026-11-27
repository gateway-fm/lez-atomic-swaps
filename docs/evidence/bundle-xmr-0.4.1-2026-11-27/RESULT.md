# 0.4.1 candidate bundle: every XMR test on release images

Date: 26–27 November 2026, times in CET (logs in UTC).

- **Bundle:** `lez-swap-stack-sha-2373e28735b7-arm64.tar.gz` (sha256 `64cc333e…a2c31d`, run 45887200123). Images `sha-2373e28735b7`, built from `2373e28735b732d2183bb4ce2b494c95ff8d42a1`.
- **Stack:** `LEZ_STACK_PREFIX=lez041n LEZ_MONERO=1 LEZ_TIMING_PROFILE=fast ./scripts/start.sh`, started fresh on an Apple-silicon Mac. The desk suites passed.
- **Tests:** `./scripts/regress.sh list full`, keeping the `node:xmr:*` and `ui:xmr:*` entries. That gave 22 tests: 18 Node and 4 desk.
  - The bundle's list already leaves out the 4 tests that `node-e2e.py release-unavailable` names (`xmr-taker-clock-ahead`, `xmr-punish-replans`, `xmr-taker-cheats`, `xmr-kill-maker`), all of which need the devnet test hooks.
- **Run:** `LEZ_LANES=2 ./scripts/regress.sh run <22 tests>` in a clean environment, from 23:07 to 01:09 CET. It took 122 minutes, with no new chains, no profile switches and no frozen tries.

## Result

**21 of 22 passed on their first try. `node:xmr:xmr-shared-address-dust` failed on its first try and passed when run again alone, in 174 s.**

| Test | First try | Seconds |
|---|---|---|
| node xmr-kill-taker | passed | 1451 |
| node xmr-monerod-down | passed | 411 |
| node mixed-three-pair | passed | 141 |
| node xmr-unfunded-punish | passed | 2208 |
| node xmr-maker-early-punish | passed | 1079 |
| node xmr-punish | passed | 1056 |
| node xmr-maker-claim-without-authorization | passed | 768 |
| ui xmr-punish | passed | 1062 |
| node xmr-custody-credit-refund | passed | 883 |
| node xmr-taker-refund | passed | 838 |
| node xmr-maker-never-funds | passed | 799 |
| node xmr-taker-refund-kill | passed | 833 |
| node xmr-wallet-restart | passed | 355 |
| node xmr-custody-credit | passed | 217 |
| node xmr-happy | passed | 170 |
| node xmr-concurrent | passed | 236 |
| ui xmr-taker-refund | passed | 948 |
| node xmr-outage-before-take | passed | 203 |
| node xmr-cut-after-lock | passed | 177 |
| node xmr-shared-address-dust | **failed** (68 s); passed on the rerun, 06:49–06:52 CET | 174 |
| ui xmr-happy | passed | 154 |
| ui xmr-concurrent | passed | 222 |

## The one first-try failure: a timing race in the harness

- **The failed try:** lane 2, swap `02aa73bb1408…`.
- **What the scenario does:** it waits for the Taker's `claim_available` (`xmr_wait_funded`). Then it reads the Maker's `xmr-effect/evidence/monero-funding.json` straight away, to learn the shared address.
- **What happened:**
  - 01:05:56 CET: the Taker read `claim_available`, having seen the Maker's Monero funding at depth on its own. The Maker was still at `taker_lock_confirmed`, with its funding tool still running.
  - 01:05:56 CET: the harness read the file before it existed and failed with "no Maker funding evidence names the shared address".
  - 01:05:58 CET: the Maker's tool wrote `monero-funding.json` with `attempt_state: confirmed` and funding txid `e5ee54fd…`.
- **Why this is not a product defect:** the swap and its funding were correct. The scenario assumes the Maker's evidence file exists as soon as the Taker sees the funding, which is not guaranteed. The rerun passed. The failed swap's directory is kept in the lane 2 Maker's `.reset-archive/20261127T000602Z`.

Files:
- [PROVENANCE.md](PROVENANCE.md): the bundle and the images' build source.
- `run/`: the full run (`results.tsv`, `chain.tsv`, `regress.log`, `e2e/`, `lanes/`, `logs/`). The failed try's per-Node log tree is not kept here.
- `rerun-dust-1/`: the rerun.
