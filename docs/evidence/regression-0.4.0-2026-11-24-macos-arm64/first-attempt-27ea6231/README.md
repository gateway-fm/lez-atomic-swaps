# The stopped first attempt at `27ea6231`, 2026-11-24

The first run of `regress.sh full` for this set, on `27ea6231` (178 tests), two
lanes, Monero regtest up, the devnet test hooks built in. It started at
18:34:22Z and the runner stopped itself at 21:28:42Z (174 min). These are its
files as the runner wrote them (`runtime/regress/20261124T183422Z`), with local
paths replaced. Its results are not counted in the run of `957c8016` that the
set's README reports.

## What it did

- 9 tests passed, each on its first try: `node:xmr:xmr-unfunded-punish`,
  `xmr-kill-maker`, `xmr-taker-cheats`, `xmr-kill-taker`, `xmr-punish-replans`,
  `xmr-punish`, `ui:xmr:xmr-taker-refund` (all TakerSellsLez), and
  `node:btc:kill-maker` and `node:btc:kill-taker`, both TakerSellsLez.
- 2 tests failed, both for reasons outside the Nodes:
  - `node:xmr:xmr-maker-early-punish:TakerSellsLez` (lane 2, 1,185 s). The
    Nodes did what the scenario asks. The Maker's tag 17 before P was accepted
    by the sequencer but not included, and the escrow kept its custody. The
    Taker Node then refunded by itself from R. After that, the test-only
    `lez-v02-escrow-claim-probe` (a tag 17 after the refund, past P) ended
    with "could not read an escrow account: Request timeout". That is its
    10 s read timeout, hit while the sequencer took 14 s between blocks 2418
    and 2419 (20:28:54Z to 20:29:08Z).
  - `node:xmr:xmr-custody-credit-refund:TakerSellsLez` (lane 1, 556 s). At
    21:07:28Z the harness's `docker exec` for the Taker's
    `taker_swap_monitor_v1` was answered "Cannot connect to the Docker daemon".
    The swap stood at `awaiting_second_lock` at the time. Docker Desktop
    rotated its backend log in the same second (21:07:29.872Z).
- 2 tries were voided by a LEZ freeze: `node:xmr:xmr-taker-refund-kill` and
  `node:xmr:xmr-taker-clock-ahead`. Finality stood at 2,923 for 60 s. Blocks had
  slowed to 7 to 9 s, and at 21:26:04Z the sequencer logged "zone-sdk publish
  failed" and "Main loop exited unexpectedly".
- 167 tests were not run.

## Why the runner stopped

The runner made a new chain (`up.sh --fresh-lez`), and `up.sh` exited at once
at its stage "the new chain finalizes" (`logs/chain-1.log`). Two things met
there:

- Docker Desktop dropped the host forward `127.0.0.1:8779` of the restarted
  `lez-network` container (the indexer). The backend log for the restart at
  21:28:12Z has an `exposer.Add` with an empty port list, then the forward
  removed and the container "unregistered" at 21:28:17Z to 21:28:23Z
  (`docker-desktop-backend-excerpt.txt`). Inside the container the indexer
  listened on 8779; on the host the port refused. Every other published port
  answered.
- `up.sh` read the indexer under `set -euo pipefail`. The first refused `curl`
  ended the script, so its 300 s poll and its retries never ran. `957c8016`
  fixes this (a refused read is an empty answer; test `FreshChainFinalityRead`
  in `deploy/tests/test_chain_lifecycle.py`).

A third thing would have stopped this new chain as well. The runner was started
without `LEZ_WORKSPACE` and `LEZ_PROVISION`, so `bootstrap-market.sh` would have
looked for the escrow deployer outside the workspace the stack was built in. The
run of `957c8016` hit this at its first new chain (its README).

## Files

`results.tsv`, `regress.log`, `chain.tsv`, `logs/` (each try's log, `chain-1.log`
and the lane logs), `e2e/` (the scenarios' summaries) and `lanes/` (the runner's
per-lane files) are as the runner wrote them. The Node logs the runner kept for
the two failed tries (`logs/<try>.nodes/`, 31 MB) are not committed. They are kept
outside git as two archives, whose SHA-256 sums are listed in the set's
`OUTSIDE-GIT.sha256`. `docker-desktop-backend-excerpt.txt` holds the Docker
Desktop backend log lines quoted above.
