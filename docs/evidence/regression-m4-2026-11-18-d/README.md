# Full regression of `10a785ad`, 2026-11-18

One run of every test of `deploy/scripts/regress.sh full` (159: `regress.sh list full`) on the
Docker devnet with the Monero regtest chain up (`LEZ_MONERO=1`), three lanes at a time
(`LEZ_LANES=3`), without desk recordings: run `runtime/regress/20261118T183839Z`, 2026-11-18
18:38:39Z to 22:05:28Z (UTC), at `10a785ad`.

The tree is `44e39081`, the commit of the full run of the same morning, plus four commits:

- `234c17db` fix(swap-store): the Maker binds an actor's state database by its inode, owner, mode
  and link count instead of also its size and times.
- `ef0f1299` fix(btc-core-adapter): before a caller-authorized Bitcoin broadcast, a readiness
  check that fails because a block landed between two RPCs is checked again.
- `15b1dc84` fix(deploy): the regtest Bitcoin Core of each lane runs with `-checkblockindex=0`.
- `10a785ad` fix(sidecar): a LEZ submission the sidecar refused with `unavailable` before any
  broadcast is no longer replayed from its request journal, so it can be tried again under the
  same request id.

`git log --oneline 44e39081..10a785ad` shows exactly these four and the docs-only evidence commit
`9c958b06` (the run of `15b1dc84`, [regression-m4-2026-11-18-c](../regression-m4-2026-11-18-c/)).
`git diff --stat 15b1dc84 10a785ad`, without that note, touches only
`compat/lez-v0_2-sidecar` (`src/bridge_runtime.rs`, `src/bridge_server.rs` and
`tests/bridge_native_refund.rs`).

**Result: 159 of 159 passed in one run of 206 minutes, every test on its first try.** 159 tries
(`results.tsv`, column 2 is 1 on every row), no NOT-EXERCISED try, no timeout, 0 frozen tries; 1
new LEZ chain (both `zec-balance-refusal` tests at once on it); 3 timing-profile switches. All 16
Monero tests passed. Test time summed over lanes 33,841 s over 159 tries (35,384 s over 161 in
the run of `15b1dc84`). It is the first full run with every test passing since the
[run of `7d09351a`](../regression-m4-2026-11-17/) on 2026-11-17 (`20261117T000855Z`).

| kind | tests | minutes (summed over lanes) | run of `15b1dc84`: minutes |
|---|---:|---:|---:|
| `node:btc` | 53 | 183.4 | 189.6 |
| `node:zec` | 51 | 121.9 | 126.4 |
| `node:xmr` | 13 | 137.6 | 139.4 |
| `ui:btc` | 17 | 43.2 | 42.3 |
| `ui:zec` | 22 | 54.4 | 68.7 |
| `ui:xmr` | 3 | 23.4 | 23.4 |
| all | 159 | 564.0 | 589.7 |

(`results.tsv`, column 4 in seconds. Here every test has one try. The run of `15b1dc84` counts
every try: its `ui:zec` includes its failed try's 1,255 s (47.7 minutes without it), its
`node:zec` two NOT-EXERCISED tries of 75 s each.)

Wall time 206 minutes (221 in the run of `15b1dc84`). By group (`regress.log`): `fast` 155.1
minutes (158.2), `zec-v6` 25.5 (36.8; the failed test held lane 3 for 21 minutes of it there),
`refund` 20.1 (20.5), the own-chain pair 5.6 (5.6). The median test took 114 s (113 s).

`results.tsv` is the runner's table (test, try, verdict, seconds, finished at, lane, LEZ chains
made so far by this run); `regress.log` is the runner's log, every try's start and verdict line
carrying the LEZ finalized height and tip. Both are as the runner wrote them and hold no local
path. No test failed, so the harness kept no Node logs for this run; the per-test logs and the
Maker Nodes' own logs read below are not committed, and the lines quoted are from them.

## The four facts this run was made to test

**`actor_deployment_invalid`: 0 for the swaps of this run.** Each lane's Maker `node.log` in its
socket volume was copied out of the container after the run (`docker cp`, lanes 1 and 2 running,
lane 3 stopped), searched and deleted. The file outlives a run and has no timestamps; this run's
part of each was taken from the first line naming the swap of the lane's first test (lane 1
`node:xmr:xmr-kill-taker`, swap `f61524be…`; lane 2 `node:xmr:xmr-kill-maker`, `05c183d9…`;
lane 3 `node:btc:kill-taker:TakerSellsLez`, `503d9f51…`) to the end of the file (2,230, 2,482 and
2,764 lines; each ends with its lane's last test of the run). The whole files hold six
`actor_deployment_invalid` lines, the six the run of `15b1dc84` lists, all for the same older
swaps (lane 1 `a1321f30`, `61123f4e`, `1aee1825`; lane 2 `a5f63bc4`; lane 3 `fd3af315`,
`1dd5df0b`) and all before this run's part. Before `234c17db` the class failed a Bitcoin test in
the run of `44e39081` and in full runs of 2026-11-17 and 2026-11-13.

**The refund retry loop: 0 lines.** The same three parts hold no
`submission_outcome_unknown lez_refund` line, and no `submission_outcome_unknown` line for any
action, so the maximum per swap in this run is 0. In the whole files the line names three swaps,
all before this run's part: lane 3 `zec-60893343` 265 times (the looping swap of the run of
`15b1dc84`); lane 1 `zec-4b79f485` 534 times (run `20261118T161227Z` on the `15b1dc84` images,
the loop reproduced by hand for `10a785ad` by flapping the lane-1 Maker's sequencer route when
its refund was due; stopped by hand, not a full run) and `zec-cc4c7a0b` once (run `20261118T171321Z`
at `10a785ad`, the same test with one failed pre-broadcast lookup injected: one line, then
`refunded`). The Maker Nodes' logs say nothing of whether the sidecar answered `unavailable`
before a broadcast in this run.

**Bitcoin mining of the 421 refund-path blocks: 1 to 2 s.** A Bitcoin refund path mines 421
blocks (the CSV plus one) between the Taker's `claim_available` and the kill walk's point "both
legs locked, refund times running". From the timestamps of the last `claim_available` line
before that kill point and the kill point line of each test's log:

| test | lane | this run | run of `15b1dc84` | test s | run of `15b1dc84`: s |
|---|---|---:|---:|---:|---:|
| `node:btc:kill-taker:TakerSellsLez` | 3 | 1 s | 1 s | 1,222 | 1,197 |
| `node:btc:kill-taker:TakerSellsForeign` | 1 | 2 s | 2 s | 845 | 870 |
| `node:btc:kill-maker:TakerSellsLez` | 3 | 2 s | 1 s | 1,244 | 1,277 |
| `node:btc:kill-maker:TakerSellsForeign` | 2 | 1 s | 1 s | 938 | 915 |

All four kill walks, both `late-claim-unmined` tests (657 and 473 s, TakerSellsLez and
TakerSellsForeign; 673 and 473 s in the run of `15b1dc84`) and both `concurrent-kill-maker` tests
(118 and 134 s) passed on their first try (`results.tsv`).

**`node:zec:zec-claim-collision:TakerSellsForeign` passed on its first try** (lane 3, 59 s): "revealing
LEZ claim superseded (planned nonce 35, live 36); re-plan 1 at nonce 36". In the three completed
full runs before this one it was NOT-EXERCISED first: once at `c6c3431f` (`20261117T205115Z`),
twice at `44e39081` and twice at `15b1dc84`; at `32c8bfb4` and `7d09351a` it passed on its first
try. Whether the collision forms depends on which LEZ transaction a block takes first; this is an
observation, not an effect of these commits.

## History of the same day

Full runs on other commits, none of them evidence for `10a785ad`:

- `44e39081` (`20261118T040324Z`): 158 of 159 in 228 minutes;
  `node:btc:offline-after-second-lock:TakerSellsLez` failed when the Maker's Bitcoin actor
  schedule failed as `actor_deployment_invalid`. Its note is
  `docs/evidence/regression-m4-2026-11-18/` on top of `44e39081`, not in this history.
- `ef0f1299` (`20261118T082111Z`): abandoned after 15 tries; the regtest Cores' per-block index
  check made the 421 refund-path blocks take 5 to 7 minutes (in the note of `15b1dc84`).
- `15b1dc84` (`20261118T101419Z`): 158 of 159 in 221 minutes;
  `ui:zec:zec-taker-refund:TakerSellsForeign` failed in the refund retry loop that `10a785ad`
  addresses ([regression-m4-2026-11-18-c](../regression-m4-2026-11-18-c/)).

## The Monero tests

All 16 passed on their first try. All are TakerSellsLez (the only Monero direction), against
Monero regtest on the devnet (per lane a `monerod` with funding, Maker and Taker
`monero-wallet-rpc`), `fast` profile. Each swap's windows, as the logs print them: "the Maker
funds by +320 s, the Taker refunds from +720 s, the Maker punishes from +1000 s". Lot 1 XMR
against 50,000 LEZ units.

| test | lane | s | s in the run of `15b1dc84` | what its log shows |
|---|---|---:|---:|---|
| `node:xmr:xmr-happy` | 3 | 170 | 184 | "settled completed: LEZ moved {'maker': 50000, 'taker': -50000} as both Nodes say"; the Taker's wallet received 0.9981844 XMR. |
| `node:xmr:xmr-taker-refund` | 2 | 872 | 855 | "settled refunded: LEZ moved {'maker': 0, 'taker': 0}"; the Maker's wallet swept its Monero back (0.998182 XMR), the Taker's received nothing. |
| `node:xmr:xmr-maker-never-funds` | 1 | 786 | 791 | "settled refunded", "no Monero moved: the Maker's wallet sent nothing, the Taker's received nothing". |
| `node:xmr:xmr-punish` | 3 | 1,073 | 1,083 | "the Maker punished; its LEZ owner's nonce moved 1 → 2"; neither wallet received Monero, the funding "stays at the shared address: neither party received it". |
| `node:xmr:xmr-concurrent` | 3 | 294 | 262 | "settled both: LEZ moved {'maker': 100000, 'taker': -100000}", the Taker's wallet received two outputs. |
| `node:xmr:xmr-kill-taker` | 1 | 1,431 | 1,541 | "taker killed at 7 points: after the take → completed; lock in flight → completed; locked → completed; Monero verified → completed; claim authorised → completed; waiting for the Maker's claim → completed; sweeping → completed". |
| `node:xmr:xmr-kill-maker` | 2 | 1,300 | 1,365 | "maker killed at 6 points: after the take → completed; lock observed → completed; funding in flight → completed; funded → completed; claim in flight → completed; claim tool running → completed". |
| `node:xmr:xmr-taker-refund-kill` | 3 | 882 | 856 | The Taker killed right after its refund's admission ("back up: refund_in_flight"); "settled refunded", the Maker's wallet swept its Monero back. |
| `node:xmr:xmr-monerod-down` | 3 | 422 | 456 | Before a take "the Maker withdrew offer … and refuses to publish on Monero: chain dependency is unavailable for route"; mid-swap "monerod was down 83 s (the funding cutoff is 235 s away)"; both swaps completed. |
| `node:xmr:xmr-wallet-restart` | 1 | 389 | 365 | Both swaps "settled completed" with a role's `monero-wallet-rpc` restarted mid-swap. |
| `node:xmr:xmr-outage-before-take` | 1 | 202 | 220 | "the Taker refused the take: Taker dependency unavailable (authenticated_delivery_unavailable)"; restored, the swap completed. |
| `node:xmr:xmr-cut-after-lock` | 3 | 224 | 183 | "the cut held: 53 samples of the Maker's Chat socket, none with a connection"; "settled completed". |
| `node:xmr:mixed-three-pair` | 1 | 212 | 205 | A Monero, a Bitcoin and a Zcash swap of the same Nodes at once: "all three completed; LEZ transactions signed by the owner accounts meanwhile: {'maker': 1, 'taker': 7}". |
| `ui:xmr:xmr-happy` | 2 | 171 | 183 | The Monero swap clicked through both desks, as in the recorded run. |
| `ui:xmr:xmr-taker-refund` | 2 | 960 | 954 | The Taker desk's refund journey: "the Maker Node has swept its Monero back, and both desks show the swap refunded". |
| `ui:xmr:xmr-concurrent` | 1 | 274 | 265 | Two Monero swaps at once on both desks, as in the recorded run. |

The Monero tests took 161.0 minutes summed over lanes (162.8 in the run of `15b1dc84`).

## Lint and unit tests on the same commit

Before the images were built, in a checkout of `10a785ad`: the LEZ v0.2 sidecar's
`scripts/verify-lez-v02-sidecar.sh` exited 0; the root workspace's tests passed (1,505 tests);
two of the three clippy lines of CI's Rust job (the workspace's, and the Maker and Taker Node
crates' with all features) exited 0. CI's third clippy line (`zec-reference-actor` with
`dealer-fixture`) was not run on this commit. These are the results the commit's author recorded;
they were not run again for this note.

## Source and images

Tests and images from `10a785ad`, tree clean (`regress.log` prints the commit and "tree dirty:
0"). Image ids, creation times, the build-source check and the Cores' arguments are in
[PROVENANCE.md](PROVENANCE.md).

## Not covered

- One run without `actor_deployment_invalid` and without the refund retry loop does not show
  either intermittent defect gone: before `234c17db` the first was seen about once per full run,
  and the second was seen once, in the run of `15b1dc84`.
- `ef0f1299`'s rechecked readiness: whether a broadcast in this run needed a second check is not
  known; the adapter does not log it.
- `10a785ad`: whether the sidecar answered `unavailable` before a broadcast in this run, so
  whether the changed path ran, is not known from the Maker Nodes' logs. The change applies to
  every pair's LEZ submissions; its effect on the Monero and Zcash first-lock submissions was
  checked by reading the code, not on the stack. The Taker Nodes' `node.log` files hold only
  lines since their last start and were not used.
- No desk recordings in this run.
- Monero beyond an offline regtest chain: no stagenet or mainnet; x86_64 (linux/amd64) only. No
  Monero test runs on the `refund` profile.
- The systemd VM run with a Monero role (`scripts/run-maker-node-systemd-vm.sh`) was not run.
- No run uses the `local` profile.
- CI's `zec-reference-actor` `dealer-fixture` clippy line was not run on this commit.
- The devnet images run development builds. No release package was built or run from this
  commit.
- macOS, linux/arm64, and any public network are untested on this head.
