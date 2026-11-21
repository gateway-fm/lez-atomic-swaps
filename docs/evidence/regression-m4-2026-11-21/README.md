# Full regression of `d4475300`, 2026-11-21

One run of every test of `deploy/scripts/regress.sh full` (160: `regress.sh list full`) on the
Docker devnet with the Monero regtest chain up (`LEZ_MONERO=1`), three lanes at a time
(`LEZ_LANES=3`), without desk recordings: run `runtime/regress/20261121T020133Z`, 2026-11-21
02:01:33Z to 05:42:36Z (UTC), at `d4475300`.

## What the tree is

`d4475300` is a merge: its first parent `dd87b5c6` carries the DLEQ cross-check, its fix and
their documents, its second parent `6799c9f5` the desk wording, captions, recording setup and a
test. Against `86bdb8a4`, the commit of the
[2026-11-20 run](../regression-m4-2026-11-20/), `git log --oneline --no-merges 86bdb8a4..d4475300`
holds 11 commits:

- The DLEQ cross-check and the proof-encoding fix: `35696dcd` (three tests compare the proof's
  two Pedersen generators with COMIT's `cross-curve-dleq` and rebuild the proof system of
  eigenwallet/core, which accepts the vector proofs and makes the same bytes; proofs do not cross
  with the archived xmr-btc-swap's basepoint parameters), `3e2d984b` (`crates/xmr-swap-sdk`: the
  verifier re-encodes a decoded proof and refuses proof bytes that differ, such as a non-minimal
  length prefix; honest proofs and the fixed vectors are unchanged).
- Desk wording and captions: `bb4273bd` (the Maker desk's two punish rows say the Maker's Monero
  stays at the swap's shared address for good, `apps/basecamp/common/node_market.cpp`),
  `c77298db` (the Zcash desk journeys' captions name the take-time agreement when it runs).
- The recording setup: `6320be53` (`ui-e2e.sh` withdraws every open offer of the lane's Maker
  before a journey it records, through `node-e2e.py withdraw-offers`; only when recording).
- The barrier test: `6799c9f5` (the deploy unit test of the two `zec-balance-refusal` tests
  asserts their overlap through the fake scenario's barrier, not from timestamps).
- Documents: `f1e84893` (what the DLEQ cross-check with COMIT shows), `dd87b5c6` (what the DLEQ
  verifier checks), `8a5fd2f6` (traceability: the 2026-11-16 desk run was of `363df3cf`).
- The two evidence notes of `86bdb8a4`: `8c0bf049` (its full regression) and `9d3304b1` (its
  desk recordings).

Code outside tests and the harness that changed: `crates/xmr-swap-sdk/src/cross_curve.rs` and
`apps/basecamp/common/node_market.cpp`. `compat`, `modules` and `packaging` are those of
`86bdb8a4`.

**Result: 160 of 160 passed in one run of 221 minutes, each on its first try, 0 frozen tries.**
160 tries (`results.tsv`, all PASSED, all try 1): no FAILED or NOT-EXERCISED try, no timeout;
1 new LEZ chain (both `zec-balance-refusal` tests at once on it); 3 timing-profile switches. All
16 Monero tests passed on their first try. The test names are those of the run of `86bdb8a4`.
Test time summed over lanes 35,374 s over 160 tries (36,188 s in the run of `86bdb8a4`).

| kind | tests | minutes (summed over lanes) | run of `86bdb8a4`: tests | minutes |
|---|---:|---:|---:|---:|
| `node:btc` | 53 | 186.3 | 53 | 188.7 |
| `node:zec` | 52 | 143.7 | 52 | 146.3 |
| `node:xmr` | 13 | 136.8 | 13 | 141.0 |
| `ui:btc` | 17 | 40.8 | 17 | 42.3 |
| `ui:zec` | 22 | 59.5 | 22 | 61.4 |
| `ui:xmr` | 3 | 22.4 | 3 | 23.4 |
| all | 160 | 589.6 | 160 | 603.1 |

(`results.tsv`, column 4 in seconds, every try counted.) The ZEC TakerSellsLez refund journeys
took 519 to 555 s each (523 to 557 s in the run of `86bdb8a4`), under
`deterministic-local-devnet-v8`; their TakerSellsForeign twins about half that (268 s for
`node:zec:zec-taker-refund`).

Wall time 221 minutes (225 in the run of `86bdb8a4`). By group (`regress.log`): `fast` 161.0
minutes (166.1), `zec-v6` 33.6 (32.9), `refund` 20.2 (20.3), the own-chain pair 5.5 (5.3). The
median test took 108.5 s (the 80th and 81st of 160 took 107 and 110 s; 111.5 s in the run of
`86bdb8a4`).

The images were built for the smoke run on the same commit, on another chain (smoke 6 of 6 and
five Monero tests 5 of 5, below in PROVENANCE). No smoke or targeted run preceded the full run on
its chain: `up.sh --fresh-lez`, then `regress.sh full`.

`results.tsv` is the runner's table (test, try, verdict, seconds, finished at, lane, LEZ chains
made so far by this run); `regress.log` is the runner's log, every try's start and verdict line
carrying the LEZ finalized height and tip. Both are as the runner wrote them and hold no local
path. The per-test logs and the Maker Nodes' own logs read below are not committed; the lines
quoted are from them.

## The four facts carried over from the earlier runs

**`actor_deployment_invalid`: 0 for the swaps of this run.** Each lane's Maker `node.log` in its
socket volume was copied out of the container after the run (`docker cp`, lanes 1 and 2 running,
lane 3 stopped), searched and deleted. The file outlives a run and has no timestamps; this run's
part of each was taken from the first line naming the swap of the lane's first test (lane 1
`node:xmr:xmr-kill-taker`, swap `16aa135f…`; lane 2 `node:xmr:xmr-kill-maker`, `a41a031c…`;
lane 3 `node:btc:kill-taker:TakerSellsLez`, `58041e5a…`) to the end of the file (2,229, 2,476 and
2,705 lines; each ends with its lane's last test of the run). The whole files hold the same six
`actor_deployment_invalid` lines the earlier notes list, for the same older swaps (lane 1
`a1321f30`, `61123f4e`, `1aee1825`; lane 2 `a5f63bc4`; lane 3 `fd3af315`, `1dd5df0b`), all
before this run's part.

**The refund retry loop: 0 lines.** The same three parts hold no
`submission_outcome_unknown lez_refund` line, and no `submission_outcome_unknown` line for any
action. In the whole files the line names the three older swaps the earlier notes list, all
before this run's part: lane 3 `zec-60893343` 265 times, lane 1 `zec-4b79f485` 534 times and
`zec-cc4c7a0b` once.

**Bitcoin mining of the 421 refund-path blocks: 1 to 2 s.** A Bitcoin refund path mines 421
blocks (the CSV plus one) between the Taker's `claim_available` and the kill walk's point "both
legs locked, refund times running". From the timestamps of the last `claim_available` line
before that kill point and the kill point line of each test's log:

| test | lane | this run | run of `86bdb8a4` | test s | run of `86bdb8a4`: s |
|---|---|---:|---:|---:|---:|
| `node:btc:kill-taker:TakerSellsLez` | 3 | 2 s | 2 s | 1,210 | 1,226 |
| `node:btc:kill-taker:TakerSellsForeign` | 1 | 1 s | 3 s | 882 | 880 |
| `node:btc:kill-maker:TakerSellsLez` | 3 | 1 s | 1 s | 1,273 | 1,231 |
| `node:btc:kill-maker:TakerSellsForeign` | 2 | 2 s | 1 s | 881 | 933 |

The log lines carry whole seconds, so each figure is good to about a second. All four kill walks,
both `late-claim-unmined` tests (678 and 475 s, TakerSellsLez and TakerSellsForeign; 696 and
480 s in the run of `86bdb8a4`) and both `concurrent-kill-maker` tests (135 and 177 s) passed on
their first try (`results.tsv`). In both `kill-taker` walks every claim after a Taker restart was
answered on its first call (`replay=False` on each of the 22 lines; no
`taker_action_execution_unavailable`); the kill walk's replay of a refused claim did not run in
this run.

**`node:zec:zec-claim-collision:TakerSellsForeign` passed on its first try** (lane 2, 69 s) and
formed the collision: "revealing LEZ claim superseded (planned nonce 25, live 26); re-plan 1 at
nonce 26". In the runs of `86bdb8a4` and `10a785ad` it passed on its first try; in the run of
`9a15d94b` it was NOT-EXERCISED on its first try and passed on its second. Whether the collision
forms depends on which LEZ transaction a block takes first.

## ZEC TakerSellsLez profiles

`regress.log` does not name a test's ZEC profile; it names only each group's switch (`zec-v6`:
`deterministic-local-devnet-v6`; `refund` and the own-chain pair: `-v5`). Two other records do,
and both read as in the run of `86bdb8a4`:

- The swap records `ui-e2e.sh` exported under the run's `zec/` (23 files, each naming its
  profile, direction and offer, matched to a test log by the offer id): the ZEC TakerSellsLez
  desk journeys `zec-happy`, `zec-concurrent` (two swaps), `zec-concurrent-staggered` (two),
  `zec-node-offline`, `zec-restart-maker`, `zec-restart-taker` and `zec-survivor` ran under
  `-v7`, and `zec-maker-refund` under `-v8`; their TakerSellsForeign twins under `-v5`, and
  `zec-maker-refund` and `zec-stale-window` TakerSellsForeign under `-v6`. Four records (two
  `-v5` TakerSellsForeign, two `-v7` TakerSellsLez) name an offer no test log names.
- The Node scenarios' corridor line, printed by the scenarios that read the dealer's corridor
  layout: `zec-tampered-catalog` and `zec-regenerated-catalog` TakerSellsLez under `-v7` (their
  TakerSellsForeign runs under `-v5`), and `zec-maker-early-claim` under `-v8`.

For the other ZEC TakerSellsLez tests the logs do not print the profile; `node-e2e.py` provisions
the refund journeys under `-v8` and every other ZEC TakerSellsLez scenario under `-v7`.

## The Monero tests

All 16 passed on their first try. All are TakerSellsLez (the only Monero direction), against
Monero regtest on the devnet (per lane a `monerod` with funding, Maker and Taker
`monero-wallet-rpc`), `fast` profile. Lot 1 XMR against 50,000 LEZ units.

| test | lane | s | s in the run of `86bdb8a4` | what its log shows |
|---|---|---:|---:|---|
| `node:xmr:xmr-happy` | 2 | 176 | 191 | "settled completed: LEZ moved {'maker': 50000, 'taker': -50000} as both Nodes say"; the Taker's wallet received 0.998182 XMR. |
| `node:xmr:xmr-taker-refund` | 3 | 872 | 882 | "settled refunded"; the Maker's wallet swept its Monero back (0.9981784 XMR), the Taker's received nothing. |
| `node:xmr:xmr-maker-never-funds` | 1 | 804 | 814 | "no Monero moved: the Maker's wallet sent nothing, the Taker's received nothing". |
| `node:xmr:xmr-punish` | 2 | 1,073 | 1,084 | "the Maker punished; its LEZ owner's nonce moved 13 → 14"; the funding "stays at the shared address: neither party received it". |
| `node:xmr:xmr-concurrent` | 2 | 224 | 280 | "settled both: LEZ moved {'maker': 100000, 'taker': -100000}", the Taker's wallet received two outputs. |
| `node:xmr:xmr-kill-taker` | 1 | 1,431 | 1,373 | "taker killed at 7 points", every one "→ completed". |
| `node:xmr:xmr-kill-maker` | 2 | 1,344 | 1,304 | "maker killed at 6 points", every one "→ completed", "claim tool running" among them. |
| `node:xmr:xmr-taker-refund-kill` | 2 | 869 | 888 | The Taker killed right after its refund's admission ("back up: refund_in_flight"); the Maker's wallet swept its Monero back. |
| `node:xmr:xmr-monerod-down` | 3 | 444 | 550 | Before a take "the Maker withdrew offer … and refuses to publish on Monero: chain dependency is unavailable for route"; mid-swap "monerod was down 83 s (the funding cutoff is 236 s away)"; "settled completed". |
| `node:xmr:xmr-wallet-restart` | 3 | 386 | 419 | "settled completed" with a role's `monero-wallet-rpc` restarted mid-swap. |
| `node:xmr:xmr-outage-before-take` | 3 | 200 | 231 | "the Taker refused the take: Taker dependency unavailable (authenticated_delivery_unavailable)"; restored, "settled completed". |
| `node:xmr:xmr-cut-after-lock` | 2 | 181 | 220 | "the cut held: 43 samples of the Maker's Chat socket, none with a connection"; "settled completed". |
| `node:xmr:mixed-three-pair` | 1 | 205 | 224 | A Monero, a Bitcoin and a Zcash swap of the same Nodes at once: "all three completed; LEZ transactions signed by the owner accounts meanwhile: {'maker': 1, 'taker': 7}". |
| `ui:xmr:xmr-happy` | 1 | 151 | 175 | The Monero swap clicked through both desks. |
| `ui:xmr:xmr-taker-refund` | 1 | 941 | 947 | "the Maker Node has swept its Monero back, and both desks show the swap refunded". |
| `ui:xmr:xmr-concurrent` | 3 | 253 | 282 | Two Monero swaps at once on both desks. |

The Monero tests took 159.2 minutes summed over lanes (164.4 in the run of `86bdb8a4`). The desk
journeys ran without recordings; no test reaches the Maker desk's punish rows, whose wording
`bb4273bd` changed.

## Lint and unit tests

Unlike the run of `86bdb8a4`, the checks `AGENTS.md` asks for before a push that changes code
ran on the tested commit itself: on 2026-11-21 from 05:45Z to 06:57Z, on this host, in a clean
checkout detached at `d4475300`, after the run had ended and with the devnet idle. Each with its
result:

- `scripts/check-all.sh` (full, with the checksum-matching rapidsnark libraries given as
  `RAPIDSNARK_LIB_DIR`): passed, 37 checks, the sidecar workspace's Clippy and rustdoc among
  them; its own shellcheck is the host's 0.9.0 (CI pins 0.11.0, which the quality gates below
  ran).
- The root workspace's tests as `ci.yml` runs them (`cargo test --locked --workspace
  --all-targets`; `ci.yml` excludes no crate): 1,596 passed, 1 failed, 9 ignored, run twice with
  the same result (once with `--no-fail-fast`). The failure is
  `typed_unavailability_is_not_substituted_with_a_local_or_zero_quote` in
  `crates/maker-node/tests/logos_price_process.rs` ("assertion failed: matches!(source.quote(route(),
  NOW), Err(PriceSourceError::UnavailableQuote))"); the test and its 500 ms worker budget are
  those of `main` at `c87497a9`. Run alone it passed 3 of 3 times through Cargo and 20 of 20
  directly; with eight busy loops on the host's eight cores it failed 1 of 20; it passed in both
  Node suites below. The host's load average was 10 to 16 during the workspace runs.
- The Node suites with `--features pair-zec` (`-p lez-maker-node -p lez-taker-node
  --all-targets`): 204 passed, 3 ignored; the two files gated on `pair-zec,pair-xmr`: 38 passed;
  the four `pair-xmr` files: 8 passed, 6 ignored; both Nodes with `--all-features
  --all-targets`: 302 passed, 9 ignored; their Clippy with `--all-features`: clean.
- `zec-reference-actor`: `crash_hook` with `test-crash-hooks`, 5 passed; `dealer-fixture`
  Clippy clean and tests 107 passed.
- The M3 vectors (`scripts/check-m3-cryptographic-vectors.sh`, the three vector files: 9 passed),
  `scripts/test-bitcoin-testnet4-route-contract.sh`, `cargo doc --workspace --no-deps` with
  `RUSTDOCFLAGS=-D warnings`: passed.
- `cargo deny --all-features check advisories bans licenses sources` for the root workspace:
  ok in all four, with duplicate-crate and unused-exception warnings only.
- `scripts/verify-lez-v02-sidecar.sh` with the rapidsnark libraries: "LEZ v0.2 sidecar
  verification: ok (native inputs attested; Cargo remained offline)", 263 tests passed, its
  `cargo deny` ok; the local advisory database did not need a fetch.
- `python3 -m unittest discover -s deploy/tests`: 196 tests OK under umask 022 and under this
  host's 0002.
- `scripts/test-check-all-workspaces.sh` (15 workspaces), `scripts/test-ci-hardening-policy.sh`,
  `scripts/check-runtime-profiles.py` (2 profiles, 60 Cargo targets),
  `scripts/check-requirements-traceability.sh`, `scripts/check-architecture-diagrams.sh`,
  `scripts/check-threat-model.py` (passed; 28 pinned source proofs not resolved without
  `--source-repo`), `scripts/check-desk-views-generated.sh`, and the other scripts of CI's
  quality job (`test-lez-cost-evidence.sh`, `test-spin-lock-remediation.sh`,
  `check-spin-lock-remediation.sh`, `check-github-action-pins.sh`,
  `check-bitcoin-core-isolation.sh`, `check-lez-v02-docker-isolation.sh`): passed.
- `scripts/run-ci-quality-gates.sh`: passed. It fetches its own checksum-pinned actionlint
  1.7.12, hadolint 2.14.0, Docker Compose 5.3.1 and shellcheck 0.11.0, so the workflow,
  Dockerfile and Compose checks that `check-all.sh` skips (none of these tools is installed on
  the host) ran here through it.
- gitleaks 8.30.1 over `c87497a9..d4475300` with the repository's configuration: 336 commits,
  no leaks.
- `scripts/check-mermaid-github-compatibility.sh` (no CI job runs it): failed on `d4475300` at
  `docs/architecture/system-architecture.md:2140`, a semicolon in a note of the Monero sequence
  diagram, added on the delivery work in `a5175375`; it passes on `main` at `c87497a9`.
  `24875fae`, on top of `d4475300`, replaces the semicolon with a comma and changes nothing
  else; on `24875fae` the script passes (191 diagrams).

Not run on this machine: `check-all.sh`'s Clippy of `compat/lez-standalone-e2e` (it needs the
logos-blockchain-circuits release, which is not on the host); the workspaces `check-all.sh`
formats only, as CI does; `scripts/run-zebra-e2e.sh`, the Bitcoin Core and Zebra end-to-end
jobs, the provisional verifier `scripts/verify-lez-v02-provisional.sh` with its guest build, the
Basecamp desk build in nix of CI, the npm and owner-API checks of CI's rust job, and the
container-security job. CI's own runs are not part of this note.

## History

The run of `86bdb8a4` on 2026-11-20 ([regression-m4-2026-11-20](../regression-m4-2026-11-20/),
160 of 160 in 225 minutes, each on its first try) is a run of another commit: the tree before the
DLEQ fix `3e2d984b`, the desk wording and captions, the recording setup and the barrier test. It
is not evidence for this tree, nor is this run evidence for that one. The desk recordings at
`86bdb8a4` ([regression-m4-desks-2026-11-20](../regression-m4-desks-2026-11-20/)) are of that
commit too and predate the caption and wording changes.

## Not covered

- No run uses the `local` profile. No Monero test runs on the `refund` profile. The block
  explorers are not part of any test.
- Monero beyond an offline regtest chain: no stagenet or mainnet. x86_64 (linux/amd64) only;
  macOS and linux/arm64 were not run on this commit.
- The systemd VM run with a Monero role (`scripts/run-maker-node-systemd-vm.sh`) was not run, nor
  was `scripts/run-zebra-e2e.sh`.
- No desk recordings were made at `d4475300`. The recordings of
  [regression-m4-desks-2026-11-20](../regression-m4-desks-2026-11-20/) are of `86bdb8a4`: they
  predate `c77298db`'s Zcash captions, `bb4273bd`'s punish wording and `6320be53`'s withdrawal
  of open offers before a recorded journey.
- The Maker desk's punish rows were not seen on a desk: no desk journey punishes, and
  `node:xmr:xmr-punish` runs without the desks.
- One run without `actor_deployment_invalid` and without the refund retry loop does not show
  either intermittent defect gone.
- Whether these paths ran is not known from the logs read: `main`'s fix for #156 (a readiness
  re-read, a Bitcoin send reported `NotAttempted`, the Taker's observer retrying an admitted
  claim; the Maker Nodes' logs do not say, and the Taker Nodes' `node.log` files hold only lines
  since their last start and were not used); the sidecar's retry of a submission refused before
  any broadcast; and the #181 fallback (a Zcash swap answered from its stored terminal state
  after its files were archived).
- The #130 `queued` path: no test log of this run shows a claim held by the pre-reveal gate
  (`claim_awaiting_zcash_confirmations`; the run of `86bdb8a4` showed it once, in
  `ui:zec:zec-concurrent:TakerSellsLez`). The desk's "Claim queued" row was not seen on a desk:
  no test asserts it, and the scenario that does (`LEZ_ZEC_CLAIM_AT_ONE_CONFIRMATION=1`) is not
  part of the suite.
- The desk message of #151 (the launcher's line under "Node unavailable") does not appear on the
  Compose stack: the devnet containers log to their own output, not a log file, so the desks
  keep the generic message.
- Monero's tag 17 (punish) and ZEC claims made from one Maker account never collided on the
  devnet: a lane runs one test at a time, and no test makes a Monero punish and a ZEC claim of
  the same Maker at once.
- The devnet images run development builds. No release package was built or run from this
  commit.
