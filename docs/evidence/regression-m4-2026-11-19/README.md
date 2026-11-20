# Full regression of `9a15d94b`, 2026-11-19

A later full run, 160 of 160 at `86bdb8a4` (`main` at `c87497a9` merged, take-time ZEC in
ADR 0216's order and the fixes for #180, #181, #130 and #151), is in
[regression-m4-2026-11-20](../regression-m4-2026-11-20/).

One run of every test of `deploy/scripts/regress.sh full` (159: `regress.sh list full`) on the
Docker devnet with the Monero regtest chain up (`LEZ_MONERO=1`), three lanes at a time
(`LEZ_LANES=3`), without desk recordings: run `runtime/regress/20261119T042557Z`, 2026-11-19
04:25:57Z to 08:01:17Z (UTC), at `9a15d94b`.

## What the tree is

`main` at `8903241b` merged with the Monero delivery work, plus three fixes:

- `f94b5018` merges `main` at `8903241b` (version 0.3.3, the fix for #156, the Node.js 24.22.3
  pin and the explorer images) into the delivery work at `7807ce3b`. The release-version files
  keep 0.4.0, as `54dd9319` set them.
- `5a938e9c` fix(swap-store): the Maker binds an actor's state database by its inode, owner, mode
  and link count instead of also its size and times (the change of `234c17db`).
- `9976f9a3` fix(deploy): the regtest Bitcoin Core of each lane runs with `-checkblockindex=0`
  (the change of `15b1dc84`).
- `8271d630` fix(sidecar): a LEZ submission the sidecar refused with `unavailable` before any
  broadcast is no longer replayed from its request journal, so it can be tried again under the
  same request id (the change of `10a785ad`).
- `347de2de` and `9a15d94b`: the evidence notes
  [regression-m4-2026-11-18-c](../regression-m4-2026-11-18-c/) and
  [regression-m4-2026-11-18-d](../regression-m4-2026-11-18-d/), docs only, unchanged. They
  record the runs that found the three defects above and the runs of the fixes on the earlier
  base; they are not evidence for this tree.

`ef0f1299`, the earlier tree's recheck of Bitcoin Core's readiness before a broadcast, is not in
this tree. `main`'s fix for #156 (`f4e5fe02`) replaces it: `ensure_ready` itself reads the tip
and `txindex` again, up to three times 200 ms apart, when a block lands between its reads; a
readiness refusal before anything was offered to Core is reported as `NotAttempted`, after which
the Bitcoin actor gives its send authority back (the Taker's claim, a Bitcoin refund, the Maker's
lock step) and the next drive sends once; and the Taker Node keeps the admission of a Bitcoin
claim whose first attempt failed and its observer drives the claim while the status still offers
it at the admitted generation.

**Result: 159 of 159 passed in one run of 215 minutes, 0 frozen tries.** 160 tries
(`results.tsv`): 158 tests passed on their first try, `node:zec:zec-claim-collision:TakerSellsForeign`
was NOT-EXERCISED on its first try and passed on its second (below). No FAILED try, no timeout;
1 new LEZ chain (both `zec-balance-refusal` tests at once on it); 3 timing-profile switches. All
16 Monero tests passed on their first try. Test time summed over lanes 35,135 s over 160 tries
(33,841 s over 159 in the run of `10a785ad`).

| kind | tests | minutes (summed over lanes) | run of `10a785ad`: minutes |
|---|---:|---:|---:|
| `node:btc` | 53 | 197.2 | 183.4 |
| `node:zec` | 51 | 129.8 | 121.9 |
| `node:xmr` | 13 | 139.0 | 137.6 |
| `ui:btc` | 17 | 43.5 | 43.2 |
| `ui:zec` | 22 | 53.2 | 54.4 |
| `ui:xmr` | 3 | 22.9 | 23.4 |
| all | 159 | 585.6 | 564.0 |

(`results.tsv`, column 4 in seconds, every try counted: `node:zec` includes the 89 s of the
NOT-EXERCISED try.)

Wall time 215 minutes (206 in the run of `10a785ad`). By group (`regress.log`): `fast` 162.9
minutes (155.1), `zec-v6` 25.9 (25.5), `refund` 20.3 (20.1), the own-chain pair 5.7 (5.6). The
median passing test took 120 s (114 s).

Before the full run, on the same chain and images: `regress.sh smoke`, 6 of 6 in 3 minutes
(`20261119T035504Z`), and one run of eleven targeted tests, 11 of 11 in 27 minutes, each on its
first try (`20261119T035813Z`): `node:btc:restart-maker`, `node:btc:offline-after-claim`,
`node:btc:kill-taker` and `node:btc:survivor` in both directions,
`node:zec:zec-taker-refund` in both directions and `node:xmr:xmr-happy`. Their tables are not
committed.

`results.tsv` is the runner's table (test, try, verdict, seconds, finished at, lane, LEZ chains
made so far by this run); `regress.log` is the runner's log, every try's start and verdict line
carrying the LEZ finalized height and tip. Both are as the runner wrote them and hold no local
path. The per-test logs, the Node logs the harness kept for the NOT-EXERCISED try and the Maker
Nodes' own logs read below are not committed; the lines quoted are from them.

## The four facts carried over from the run of `10a785ad`

**`actor_deployment_invalid`: 0 for the swaps of this run.** Each lane's Maker `node.log` in its
socket volume was copied out of the container after the run (`docker cp`, lanes 1 and 2 running,
lane 3 stopped), searched and deleted. The file outlives a run and has no timestamps; this run's
part of each was taken from the first line naming the swap of the lane's first test (lane 1
`node:xmr:xmr-kill-taker`, swap `3fbf0916…`; lane 2 `node:xmr:xmr-kill-maker`, `cce2ea6d…`;
lane 3 `node:btc:kill-taker:TakerSellsLez`, `a9890e85…`) to the end of the file (2,262, 2,231 and
2,308 lines; each ends with its lane's last test of the run). The whole files hold the same six
`actor_deployment_invalid` lines the run of `10a785ad` lists, for the same older swaps (lane 1
`a1321f30`, `61123f4e`, `1aee1825`; lane 2 `a5f63bc4`; lane 3 `fd3af315`, `1dd5df0b`), all
before this run's part.

**The refund retry loop: 0 lines.** The same three parts hold no
`submission_outcome_unknown lez_refund` line, and no `submission_outcome_unknown` line for any
action. In the whole files the line names the three older swaps the run of `10a785ad` lists,
all before this run's part: lane 3 `zec-60893343` 265 times, lane 1 `zec-4b79f485` 534 times and
`zec-cc4c7a0b` once.

**Bitcoin mining of the 421 refund-path blocks: 1 to 2 s.** A Bitcoin refund path mines 421
blocks (the CSV plus one) between the Taker's `claim_available` and the kill walk's point "both
legs locked, refund times running". From the timestamps of the last `claim_available` line
before that kill point and the kill point line of each test's log:

| test | lane | this run | run of `10a785ad` | test s | run of `10a785ad`: s |
|---|---|---:|---:|---:|---:|
| `node:btc:kill-taker:TakerSellsLez` | 3 | 1 s | 1 s | 1,207 | 1,222 |
| `node:btc:kill-taker:TakerSellsForeign` | 1 | 2 s | 2 s | 893 | 845 |
| `node:btc:kill-maker:TakerSellsLez` | 3 | 1 s | 2 s | 1,300 | 1,244 |
| `node:btc:kill-maker:TakerSellsForeign` | 2 | 1 s | 1 s | 936 | 938 |

All four kill walks, both `late-claim-unmined` tests (695 and 473 s, TakerSellsLez and
TakerSellsForeign; 657 and 473 s in the run of `10a785ad`) and both `concurrent-kill-maker`
tests (148 and 205 s) passed on their first try (`results.tsv`). In both `kill-taker` walks every
claim after a Taker restart was answered on its first call (`replay=False` on each); the kill
walk's replay of a claim refused as `taker_action_execution_unavailable` after a restart did not
run in this run.

**`node:zec:zec-claim-collision:TakerSellsForeign` passed on its second try** (lane 2). Try 1
(89 s) was NOT-EXERCISED: "A completed without a superseded claim (C's lock was in a block before
A's claim was planned, or C's lock lost the nonce instead …); try again". Try 2 (76 s) formed the
collision: "revealing LEZ claim superseded (planned nonce 29, live 30); re-plan 1 at nonce 30".
In the run of `10a785ad` it passed on its first try; in the three full runs before that it was
NOT-EXERCISED first. Whether the collision forms depends on which LEZ transaction a block takes
first.

## The Monero tests

All 16 passed on their first try. All are TakerSellsLez (the only Monero direction), against
Monero regtest on the devnet (per lane a `monerod` with funding, Maker and Taker
`monero-wallet-rpc`), `fast` profile. Lot 1 XMR against 50,000 LEZ units.

| test | lane | s | s in the run of `10a785ad` | what its log shows |
|---|---|---:|---:|---|
| `node:xmr:xmr-happy` | 2 | 174 | 170 | "settled completed: LEZ moved {'maker': 50000, 'taker': -50000} as both Nodes say"; the Taker's wallet received 0.9981784 XMR. |
| `node:xmr:xmr-taker-refund` | 1 | 877 | 872 | The Maker's wallet swept its Monero back (0.9981832 XMR), the Taker's received nothing. |
| `node:xmr:xmr-maker-never-funds` | 3 | 810 | 786 | "no Monero moved: the Maker's wallet sent nothing, the Taker's received nothing". |
| `node:xmr:xmr-punish` | 2 | 1,081 | 1,073 | "the Maker punished; its LEZ owner's nonce moved 31 → 32"; the funding "stays at the shared address: neither party received it". |
| `node:xmr:xmr-concurrent` | 2 | 252 | 294 | "settled both: LEZ moved {'maker': 100000, 'taker': -100000}", the Taker's wallet received two outputs. |
| `node:xmr:xmr-kill-taker` | 1 | 1,458 | 1,431 | "taker killed at 7 points", every one "→ completed". |
| `node:xmr:xmr-kill-maker` | 2 | 1,327 | 1,300 | "maker killed at 6 points", every one "→ completed". |
| `node:xmr:xmr-taker-refund-kill` | 2 | 878 | 882 | The Taker killed right after its refund's admission ("back up: refund_in_flight"); the Maker's wallet swept its Monero back. |
| `node:xmr:xmr-monerod-down` | 3 | 492 | 422 | Before a take "the Maker withdrew offer … and refuses to publish on Monero: chain dependency is unavailable for route"; mid-swap "monerod was down 89 s (the funding cutoff is 230 s away)"; "settled completed". |
| `node:xmr:xmr-wallet-restart` | 3 | 369 | 389 | "settled completed" with a role's `monero-wallet-rpc` restarted mid-swap. |
| `node:xmr:xmr-outage-before-take` | 3 | 221 | 202 | "the Taker refused the take: Taker dependency unavailable (authenticated_delivery_unavailable)"; restored, "settled completed". |
| `node:xmr:xmr-cut-after-lock` | 2 | 197 | 224 | "the cut held: 43 samples of the Maker's Chat socket, none with a connection"; "settled completed". |
| `node:xmr:mixed-three-pair` | 1 | 206 | 212 | A Monero, a Bitcoin and a Zcash swap of the same Nodes at once: "all three completed; LEZ transactions signed by the owner accounts meanwhile: {'maker': 1, 'taker': 7}". |
| `ui:xmr:xmr-happy` | 1 | 177 | 171 | The Monero swap clicked through both desks. |
| `ui:xmr:xmr-taker-refund` | 1 | 959 | 960 | "the Maker Node has swept its Monero back, and both desks show the swap refunded". |
| `ui:xmr:xmr-concurrent` | 3 | 237 | 274 | Two Monero swaps at once on both desks. |

The Monero tests took 161.9 minutes summed over lanes (161.0 in the run of `10a785ad`).

## Lint and unit tests on the same code

On `9a15d94b`, in a separate checkout, before the images were built, with
`CARGO_INCREMENTAL=0` and debug info off: every Rust step of CI's Rust job exited 0 —
`cargo fmt --all --check`; the three clippy lines (the workspace's, the Maker and Taker Node
crates' with all features, and `zec-reference-actor` with `dealer-fixture`);
`cargo test --locked --workspace --all-targets` (1,516 passed, 0 failed, 5 ignored); the
`pair-zec` Node test files (189 passed, 3 ignored), the files gated on `pair-zec,pair-xmr` (20),
the `pair-xmr` Maker files (8 passed, 6 ignored), the ZEC actor's crash-hook file (4), the
`dealer-fixture` tests (82); the M3 vector script and vector tests (9); the Bitcoin Testnet4
route contract; `cargo doc` with `-D warnings`; the traceability and architecture-diagram
checks. Also `scripts/verify-lez-v02-sidecar.sh`, `scripts/run-ci-quality-gates.sh`, the
CI-policy, spin-lock, action-pin, Bitcoin Core and LEZ v0.2 isolation and runtime-profile
checks, and the deploy unit tests under `umask 022` (175 tests, OK).

## Not covered

- One run without `actor_deployment_invalid` and without the refund retry loop does not show
  either intermittent defect gone: before the store fix the first was seen about once per full
  run, and the second was seen once.
- `main`'s fix for #156: whether a readiness check in this run needed a re-read, whether a
  Bitcoin send was reported `NotAttempted`, and whether the Taker's observer retried an admitted
  claim are not known; the Maker Nodes' logs do not say, and the Taker Nodes' `node.log` files
  hold only lines since their last start and were not used.
- The sidecar change: whether the sidecar answered `unavailable` before a broadcast in this run,
  so whether the changed path ran, is not known from the Maker Nodes' logs.
- No desk recordings in this run.
- Monero beyond an offline regtest chain: no stagenet or mainnet; x86_64 (linux/amd64) only. No
  Monero test runs on the `refund` profile.
- The systemd VM run with a Monero role (`scripts/run-maker-node-systemd-vm.sh`) was not run.
- No run uses the `local` profile. The block explorers are not part of any test.
- The devnet images run development builds. No release package was built or run from this
  commit.
- macOS, linux/arm64, and any public network are untested on this head.
