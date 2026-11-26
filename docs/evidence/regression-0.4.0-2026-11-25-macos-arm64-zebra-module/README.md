# Full regression of `23e54486`, Zebra as a Logos module, 2026-11-25/26, macOS arm64

This is the second full regression of release 0.4.0. The first,
[regression-0.4.0-2026-11-24-macos-arm64](../regression-0.4.0-2026-11-24-macos-arm64/), ran on
`957c8016`, whose devnet runs Zebra 7.0.0 as a plain container. One more code change went in
before the tag. In this set's code, the devnet's Zcash Regtest chain runs as the upstream
`zebrad_module` (Zebra 7.1.0) inside a Basecamp with a Zcash Node desk, and the Nodes read
Zebra's cookie live from a read-only mount of its data volume. The same change carries two
Maker fixes found under load: the abandoned-lease pass leaves leases of its own owner alone, and
a lease accessor. This set is the run on that code.

Every test of `deploy/scripts/regress.sh full` (180: `regress.sh list full`) ran on the Docker
devnet, with the Monero regtest chain up (`LEZ_MONERO=1`) and the devnet test hooks built in
(`LEZ_TEST_HOOKS=1`). They ran four at a time on four lanes (`LEZ_LANES=4`), without desk
recordings. One operator ran both parties, Maker and Taker, on one host: an Apple-silicon Mac
(macOS 26.7.3, 10 cores), Docker Desktop 29.9.2, local linux/arm64 dev-profile images. No second
operator and no second host took part.

**Result:**

- **On `23e54486`:** 177 of 180 passed, each on its first try, in one run of 195 minutes
  (2026-11-25 22:10:05Z to 2026-11-26 01:24:43Z). There were 0 frozen tries, 0 NOT-EXERCISED
  and no timeout.
- **The 3 failures:** the three tests that run on the Logos transport overlay
  (`compose.logos.yaml`), which dropped the Taker's Zebra cookie mount (below).
- **On `f259e791`:** those three were run again, in a separate runner invocation labelled as
  such. `f259e791` differs from `23e54486` only in that overlay and its test. All three passed
  on their first try.
- **Nothing else was rerun.**

| invocation | head | command | UTC | tests | result |
|---|---|---|---|---:|---|
| 1 (`run-23e54486-full/`) | `23e54486` | `regress.sh full` | 22:10:05Z to 01:24:43Z (195 min) | 180 | 177 passed, 3 failed |
| 2 (`run-f259e791-logos/`) | `f259e791` | `regress.sh run` with the three Logos-transport tests | 01:28:07Z to 01:30:03Z (2 min) | 3 | 3 passed |

| kind | tests | minutes in invocation 1 (summed over lanes) |
|---|---:|---:|
| `node:btc` | 56 | 161.0 |
| `node:zec` | 59 | 145.7 |
| `node:xmr` | 22 | 287.9 |
| `ui:btc` | 17 | 34.9 |
| `ui:zec` | 22 | 52.1 |
| `ui:xmr` | 4 | 39.7 |
| all | 180 | 721.2 |

The median test took 86 s. Invocation 1 had 3 timing-profile switches: `zec-v6` at 00:37:22Z,
`refund` at 01:04:59Z, and back to `fast` for the own-chain pair at 01:21:05Z. It made one new
LEZ chain, for both `zec-balance-refusal` tests (lanes 1 and 2 at once). Before invocation 2,
`up.sh --fresh-lez` with `LEZ_LANES=4` made a funded chain again (01:26Z), because the last
chain had been that pair's own, which leaves two wallets unfunded.

`results.tsv` holds the two invocations' tables one after the other (183 rows: 180, then 3).

## The Logos-transport tests

`ui:btc:logos-happy:TakerSellsForeign`, `ui:zec:logos-happy:TakerSellsForeign` and
`ui:zec:logos-happy:TakerSellsLez` are the only tests of the tier that use the overlay: only
`ui-e2e.sh`'s `logos-happy` calls `transport logos`.

**Why they failed on `23e54486`.** On `23e54486`, the overlay's Taker volume list
(`volumes: !override`) left out `zebra_module_data:/var/lib/zebra:ro`. A Taker recreated on the
overlay still had `LEZ_ZEBRA_COOKIE_FILE` set, but no cookie at that path. The failures were:

- `ui:zec:logos-happy:TakerSellsForeign` (lane 3, 32 s): "paying the Taker's Zcash key failed:
  Error: Zebra credential file is unavailable or unsafe".
- `ui:zec:logos-happy:TakerSellsLez` (lane 3, 130 s): the Taker desk's "real Node health" step,
  then "take rejected by the Taker Node: … chain_unavailable".
- `ui:btc:logos-happy:TakerSellsForeign` (lane 4, 132 s): the Taker desk's "real Node health"
  step read "Node needs attention" (`degraded: true`). The offer was still taken.

**The degraded route.** The test log cuts the desk's output before the routes. After the run,
the lane-1 Taker was recreated on the overlay of `23e54486` once more, and its health was read
with `lez-taker-cli health` (`run-f259e791-logos/repro-taker-health-23e54486-overlay.json`).
It reads `degraded: true`, with both Zcash routes `unavailable` and both Bitcoin routes and the
Monero route `available`. The container had no cookie. The degraded routes in the BTC journey
were therefore the Zcash probes. The Taker was then recreated on `compose.yaml` again (healthy,
every route available).

**On `f259e791`** the overlay mounts the volume again. In each Taker recreated on the overlay in
invocation 2, `/var/lib/zebra/regtest/.cookie` was readable as uid 4713
(`run-f259e791-logos/overlay-cookie-check.log`). All three tests passed (82, 88 and 90 s).
`git diff --name-only 23e54486 f259e791` lists `deploy/compose.logos.yaml` and
`deploy/tests/test_image_contexts.py` only (`diff-23e54486-f259e791.txt`). No image, binary or
other script differs, so the images of `23e54486` ran invocation 2.

## What the Zebra module and the fixes touched

All of these passed on their first try in invocation 1:

- **The dealer path** (the corridor the test dealer provisions for both roles, now with one
  shared Zebra cookie): `node:zec:zec-maker-early-claim:TakerSellsLez` (524 s),
  `zec-tampered-catalog` and `zec-regenerated-catalog` in both directions,
  `ui:zec:zec-stale-window:TakerSellsForeign`.
- **Zebra stopped and started under live swaps** (a new cookie at its start):
  `node:btc:zebra-down`, both directions. The restart scenarios of both pairs,
  `restart-maker`, `restart-taker`, `zec-restart-maker` and `zec-restart-taker`, ran as Node
  and desk tests.
- **The Maker kill and restart scenarios** (the supervisor's lease pass):
  - `node:btc:kill-maker`, both directions (1,110 and 753 s);
  - `node:btc:concurrent-kill-maker` and `node:zec:zec-concurrent-kill-maker`, both
    directions;
  - `node:zec:zec-kill-maker`, both directions;
  - `node:zec:zec-kill-maker-claim` (150 s);
  - `node:xmr:xmr-kill-maker` (1,471 s);
  - the `restart-maker` and `zec-restart-maker` tests;
  - every `offline-after-*` test.
- **The ZEC tests that came with 0.4.0:** `zec-activate-after-expiry` (108 s),
  `zec-claim-at-one-confirmation` (55 s), `zec-slow-funding` (123 s).

No test pressed the desk's Reset chain in this run. No chain was on another consensus branch,
and none was worn.

## The first attempt at `83b89df4`

`first-attempt-83b89df4/` holds the run of `regress.sh full` on `83b89df4`, stopped at a test
boundary. Its README gives the details:

- 33 tests passed, all on their first try.
- `node:zec:zec-maker-early-claim` failed: "provisioned actor pair is not isolated". The
  pair-isolation rule refused the two roles' shared Zebra cookie; `23e54486` fixes it.
- 146 tests did not run.
- It also records how the four lanes were first brought up: Docker's address pools were
  exhausted by other projects' stopped stacks, and lane 2's bind sources were created as
  directories.

None of its results are counted above.

## The four lanes

Lanes 3 and 4 settle as market wallets of their own (`lanes.sh mint 4`, then a new chain funds
them). With four lanes idle, the measurements were:

- this devnet's containers used about 1.1 to 1.25 GiB per lane and 0.9 GiB of shared services
  (`docker stats`);
- the Docker VM had 21.9 GB of 32.3 GB available;
- the host was not swapping (`vm_stat`, no swap-ins or swap-outs over 20 s).

After the run the VM had 21.7 GB available. LEZ finality never froze, and no test failed for
timing. In the first attempt's first 30 minutes on the same four lanes, the sequencer produced a
block about every 1.4 s and the VM had 21.8 GB available. A
Monero Stagenet node of this project (`lez-stagenet040-monerod`, about 1.9 GiB) ran beside the
devnet. Other projects' stacks were mostly stopped.

## Files

- `results.tsv`: both invocations' tables (test, try, verdict, seconds, finished at, lane,
  LEZ chains made so far by that invocation).
- `run-23e54486-full/`, `run-f259e791-logos/`: each invocation's output as the runner wrote it:
  `results.tsv`, `regress.log`, `chain.tsv`, `logs/`, `e2e/` and `zec/`. The `e2e/` and `zec/`
  folders are the stack's evidence directory at the end of each invocation.
  `run-f259e791-logos/` also holds `overlay-cookie-check.log` and
  `repro-taker-health-23e54486-overlay.json`.
- `first-attempt-83b89df4/`: the stopped run at `83b89df4`, with its own README.
- `diff-23e54486-f259e791.txt`, `diff-83b89df4-23e54486.txt`: `git diff --name-only` between
  the heads.
- `OUTSIDE-GIT.sha256`: the SHA-256 of the archives of the Node logs the runner kept for the
  four failed tries (three here, one in the first attempt), which are not committed.
- `PROVENANCE.md`: build source, images, the stack, privacy and times.
- `RESULTS.txt`: the count in short.
- `SHA256SUMS`: every file here but itself.

`results.tsv`, `chain.tsv` and the `regress.log` files are in UTC. The per-test logs of
`node-e2e.py` and `ui-e2e.sh` print the host's local time (CET, UTC+1).

## Not covered

- Lint, unit tests and the CI quality gates were not run on this host for this set. The set
  itself was scanned (PROVENANCE).
- The desk's Reset chain was not pressed by any test of this run.
- No run uses the `local` profile. No Monero test runs on the `refund` profile. The block
  explorers are not part of any test.
- Monero beyond an offline regtest chain.
- arm64 (linux/arm64 under Docker Desktop on macOS) only. Two operators on two hosts: one
  operator ran both parties on one host.
- The devnet images run development builds with the devnet test hooks. No release package was
  built or run from this commit.
