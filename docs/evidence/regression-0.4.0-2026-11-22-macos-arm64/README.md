# Full regression of `7b4e46da`, 2026-11-22, macOS arm64

One run of every test of `deploy/scripts/regress.sh full` (175: `regress.sh list full`) on the
Docker devnet with the Monero regtest chain up (`LEZ_MONERO=1`), three lanes at a time
(`LEZ_LANES=3`), without desk recordings: run `runtime/regress/20261122T054305Z`, 2026-11-22
05:43:05Z to 09:51:50Z (UTC), at `7b4e46da`. One operator ran both parties, Maker and Taker, on
one host: an Apple-silicon Mac (macOS 26.7.3, 10 cores), Docker Desktop 29.9.2, local linux/arm64
dev-profile images, Monero regtest. No second operator and no second host took part.

## What the tree is

`7b4e46da` is `e1fbecea` with one harness fix on top. Against `d4475300`, the commit of the
[2026-11-21 run](../regression-m4-2026-11-21/), `git log --oneline --no-merges d4475300..7b4e46da`
holds 107 commits: the Monero Stagenet work (the Nodes take Monero swaps on Stagenet as well as
regtest, a daemon per role, the two-hour P - R of the PublicStagenet profile, the Stagenet guide),
the Taker and Maker fixes of the M4 review (Monero refunds, punish reads, re-plans, ZEC outpoint
reuse, logins kept off command lines), the custody-credit probe and its six scenarios, the twelve
new Monero scenarios with the Taker Node's devnet test hooks (`LEZ_TEST_HOOKS=1`), and documents.
`packaging` is the tree of `d4475300`; `crates`, `compat`, `apps`, `modules` and `deploy` differ
(PROVENANCE).

The last three commits:

- `e1fbecea`: a test fixture of `crates/maker-node` (the XMR fixture's fake monerod answers
  `get_info`); no shipped code.
- `a5e8e64a`, `84062b80`: documents only.
- `7b4e46da`: `deploy/scripts/node-e2e.py`. `xmr_escrow_program_id()` read
  `market-bootstrap.env` from the lane's runtime directory, which exists for lane 1 only
  (`gen-config.sh` and `bootstrap-market.sh` write it to `runtime/`). On lanes 2 and 3 every
  scenario that runs the Maker's escrow probe failed with `FileNotFoundError`. It now reads the
  stack's manifest on every lane; a deploy unit test covers the read from a lane directory.

**Result: 175 of 175 passed; 174 on their first try, 1 on its second rerun.** 175 tries in the
run (`results.tsv`): 174 PASSED, 1 FAILED, no NOT-EXERCISED try, no timeout, 0 frozen tries; 1 new
LEZ chain (both `zec-balance-refusal` tests at once on it); 3 timing-profile switches. The one
failure, `ui:btc:logos-happy:TakerSellsForeign`, failed again on its first rerun alone and passed
on its second (below). All 25 Monero tests passed on their first try.

| kind | tests | minutes (summed over lanes) | run of `d4475300`: tests | minutes |
|---|---:|---:|---:|---:|
| `node:btc` | 56 | 156.8 | 53 | 186.3 |
| `node:zec` | 55 | 138.2 | 52 | 143.7 |
| `node:xmr` | 22 | 282.1 | 13 | 136.8 |
| `ui:btc` | 17 | 32.7 | 17 | 40.8 |
| `ui:zec` | 22 | 51.8 | 22 | 59.5 |
| `ui:xmr` | 3 | 22.1 | 3 | 22.4 |
| all | 175 | 683.8 | 160 | 589.6 |

(`results.tsv`, column 4 in seconds, every try counted.) The run of `d4475300` was on another
host (Linux 6.8, x86_64, linux/amd64 images), so the minutes compare hosts as well as trees. The
15 tests that run of `d4475300` did not have: the custody-credit scenarios (`btc-custody-credit`
both directions, `btc-custody-credit-refund`, `zec-custody-credit` both directions,
`zec-custody-credit-refund`, `xmr-custody-credit`, `xmr-custody-credit-refund`) and
`xmr-unfunded-punish`, `xmr-punish-replans`, `xmr-taker-cheats`, `xmr-maker-early-punish`,
`xmr-maker-claim-without-authorization`, `xmr-taker-clock-ahead`, `xmr-shared-address-dust`.

Wall time 248 minutes (221 in the run of `d4475300`). By group (`regress.log`): `fast` 185.9
minutes, `zec-v6` 37.6, `refund` 20.7, the own-chain pair 3.9. The median test took 77 s (the
88th of 175). Test time summed over lanes 41,026 s over 175 tries.

No smoke or targeted run preceded the full run on its chain: the images were built, the stack
started on a wiped state (`down.sh --wipe`, `lanes.sh down 3 --wipe`, then `from-scratch.sh`'s
`rust`, `build`, `stage` and `stack` phases with `LEZ_MONERO=1 LEZ_TEST_HOOKS=1`, then `up.sh`),
then `regress.sh full`.

`results.tsv` is the runner's table (test, try, verdict, seconds, finished at, lane, LEZ chains
made so far by this run); `regress.log` is the runner's log, every try's start and verdict line
carrying the LEZ finalized height and tip. `reruns/1/` and `reruns/2/` hold the same two files of
each rerun, and `diagnostics/` the test log of each of the three tries of
`ui:btc:logos-happy:TakerSellsForeign` (the run's and both reruns'). All are as the runner wrote
them and hold no local path. The other per-test logs and the Nodes' own logs read below are not
committed; the lines quoted are from them.

## The failure and its reruns

`ui:btc:logos-happy:TakerSellsForeign` is the desk journey on the Logos transport
(`compose.logos.yaml`): the Taker reaches the Maker only over Logos Delivery and Chat, on a
private two-node Delivery network. Three tries on lane 1:

| try | when (UTC) | verdict | s | file |
|---|---|---|---:|---|
| the run, try 1 | 08:09:48 to 08:10:46 | FAILED | 58 | `diagnostics/full-run-try-1.log` |
| rerun 1, alone (`regress.sh run`) | 09:52:10 to 09:53:10 | FAILED | 60 | `reruns/1/`, `diagnostics/rerun-1.log` |
| rerun 2, alone (`regress.sh run`) | 09:54:00 to 09:55:19 | PASSED | 79 | `reruns/2/`, `diagnostics/rerun-2.log` |

Both failures are the same step: the Maker desk had published its offers, the Taker desk saw them
in its order book, and the take was refused, "take rejected by the Taker Node:
initiation_execution_unavailable", with the Taker Node's last line "taker BTC reservation failed:
btc_reserve_v1: local RPC error -32010: Chat peer operation unavailable". The Chat modules' lines
the test prints show why: the Taker's Chat module created the direct conversation and "sent 9915
bytes" (rerun 1: 9916), its `btc_reserve_v1` request, 3 s after its Delivery node was online; the
Maker's Chat module logged nothing after "delivery is online", so the request never reached it.
The Taker's Chat gateway waits 25 s for an answer (`REQUEST_TIMEOUT`,
`crates/node-common/src/logos_chat_gateway.rs:49`, the wait at line 862) and then answers
`DependencyUnavailable`, which it sends as -32010 "Chat peer operation unavailable" (line 1459).
In rerun 2 the timing was the same (the Taker sent 9935 bytes 3 s after its node was online), and
the Maker received it 0.1 s later; the six Chat rounds of the take took under 1 s and the swap
completed.

The logs kept do not say why the first message was lost: the Delivery nodes' own lines of the
Logos phase are not in the kept files (the journey recreates both Basecamps on the local
transport before the harness collects the containers' logs). The README of `deploy/` records 6
passing runs of this journey on the devnet (2026-11-12), and the run of `d4475300` passed it on
its first try (169 s, x86_64). On this host it failed 2 of 3 tries. Both `ui:zec:logos-happy`
journeys of the run (TakerSellsLez on lane 3, TakerSellsForeign on lane 1 right after the failure)
passed on their first try. Whether the loss
is in the Delivery network's start (the Taker's node dialling the Maker's) or elsewhere is not
established by these logs; no code was changed for it.

## The facts carried over from the earlier runs

**`actor_deployment_invalid`: 0.** Each lane's Maker `node.log` in its socket volume was copied
out of the container after the reruns (`docker cp`; lanes 1 and 2 running, lane 3 stopped),
searched and deleted. The stack and all three lanes started from wiped volumes for this run, so
the whole files (4,476, 3,481 and 4,351 lines) are this run's, with the two reruns' in lane 1's.

**The refund retry loop: 0 lines.** The same files hold no `submission_outcome_unknown` line, for
`lez_refund` or any other action.

**Bitcoin mining of the 421 refund-path blocks: 0 to 1 s.** From the last `claim_available` line
before the kill walk's point "both legs locked, refund times running" to that line:

| test | lane | this run | test s | run of `d4475300`: s |
|---|---|---:|---:|---:|
| `node:btc:kill-taker:TakerSellsLez` | 3 | 1 s | 1,056 | 1,210 |
| `node:btc:kill-taker:TakerSellsForeign` | 3 | 0 s | 711 | 882 |
| `node:btc:kill-maker:TakerSellsLez` | 1 | 1 s | 1,105 | 1,273 |
| `node:btc:kill-maker:TakerSellsForeign` | 1 | 0 s | 719 | 881 |

The log lines carry whole seconds. All four kill walks, both `late-claim-unmined` tests (568 and
464 s, TakerSellsLez and TakerSellsForeign) and both `concurrent-kill-maker` tests (61 and 104 s)
passed on their first try. In both `kill-taker` walks every claim after a Taker restart was
answered on its first call (`replay=False` on each of the 22 lines; no
`taker_action_execution_unavailable`).

**`node:zec:zec-claim-collision:TakerSellsForeign` passed on its first try** (lane 2, 50 s) and
formed the collision: "revealing LEZ claim superseded (planned nonce 48, live 49); re-plan 1 at
nonce 49".

## ZEC TakerSellsLez profiles

`regress.log` names each group's switch (`zec-v6`: `deterministic-local-devnet-v6`; `refund` and
the own-chain pair: `-v5`). The 23 swap records `ui-e2e.sh` exported under the run's `zec/` name
`-v5` 9 times, `-v6` twice, `-v7` 11 times and `-v8` once, the same split as in the run of
`d4475300` (desk journeys TakerSellsLez under `-v7`, `zec-maker-refund` TakerSellsLez under `-v8`,
TakerSellsForeign under `-v5` and `-v6`). No test log of this run shows a claim held by the
pre-reveal gate (`claim_awaiting_zcash_confirmations`).

## The Monero tests

All 25 passed on their first try, all TakerSellsLez, against Monero regtest on the devnet (per
lane a `monerod` with funding, Maker and Taker `monero-wallet-rpc`), `fast` profile, lot 1 XMR
against 50,000 LEZ units.

| test | lane | s | s at `d4475300` | what its log shows |
|---|---|---:|---:|---|
| `node:xmr:xmr-happy` | 3 | 170 | 176 | "settled completed: LEZ moved {'maker': 50000, 'taker': -50000} as both Nodes say". |
| `node:xmr:xmr-taker-refund` | 1 | 856 | 872 | "settled refunded". |
| `node:xmr:xmr-taker-refund-kill` | 3 | 857 | 869 | "settled refunded". |
| `node:xmr:xmr-maker-never-funds` | 2 | 806 | 804 | "no Monero moved: the Maker's wallet sent nothing, the Taker's received nothing". |
| `node:xmr:xmr-punish` | 1 | 1,065 | 1,073 | "the Taker, back up, reads punished (actor punish_open)". |
| `node:xmr:xmr-concurrent` | 2 | 278 | 224 | "settled both: LEZ moved {'maker': 100000, 'taker': -100000}". |
| `node:xmr:xmr-kill-taker` | 1 | 1,346 | 1,431 | "settled completed" after each kill point. |
| `node:xmr:xmr-kill-maker` | 2 | 1,389 | 1,344 | "maker killed at 6 points", every one "→ completed", "claim tool running" among them. |
| `node:xmr:xmr-monerod-down` | 3 | 483 | 444 | "settled completed". |
| `node:xmr:xmr-wallet-restart` | 2 | 353 | 386 | "settled completed". |
| `node:xmr:xmr-outage-before-take` | 3 | 203 | 200 | "settled completed". |
| `node:xmr:xmr-cut-after-lock` | 2 | 185 | 181 | "settled completed". |
| `node:xmr:mixed-three-pair` | 2 | 186 | 205 | "all three completed; LEZ transactions signed by the owner accounts meanwhile: {'maker': 1, 'taker': 7}". |
| `node:xmr:xmr-unfunded-punish` | 3 | 2,150 | not run | "xmr-punish from P, after the Node's refund: sequencer accepted, not included; escrow Refunded → Refunded, custody 0 → 0". |
| `node:xmr:xmr-punish-replans` | 1 | 1,091 | not run | "6 punish re-plan slots reserved; the Maker's owner signed 7 transactions". |
| `node:xmr:xmr-maker-early-punish` | 2 | 1,090 | not run | the probe's punish after the Taker's refund past P: "escrow Refunded → Refunded"; "settled refunded". |
| `node:xmr:xmr-maker-claim-without-authorization` | 1 | 772 | not run | "xmr-claim without tag 14 or Monero: sequencer accepted, not included; escrow Funded → Funded"; "settled refunded". |
| `node:xmr:xmr-taker-cheats` | 2 | 1,591 | not run | "the refund sent after the Maker's claim was refused: the escrow stays Claimed". |
| `node:xmr:xmr-taker-clock-ahead` | 2 | 826 | not run | "the Taker Node refunds by itself: true"; "settled refunded". |
| `node:xmr:xmr-shared-address-dust` | 1 | 177 | not run | "settled completed"; the Taker's wallet received one output of 0.9981916 XMR. |
| `node:xmr:xmr-custody-credit` | 2 | 211 | not run | "settled completed: LEZ moved {'maker': 50001, 'taker': -50000}". |
| `node:xmr:xmr-custody-credit-refund` | 3 | 844 | not run | "settled refunded: LEZ moved {'maker': 0, 'taker': 1}". |
| `ui:xmr:xmr-happy` | 3 | 163 | 151 | The Monero swap clicked through both desks. |
| `ui:xmr:xmr-taker-refund` | 1 | 929 | 941 | "the Maker Node has swept its Monero back, and both desks show the swap refunded". |
| `ui:xmr:xmr-concurrent` | 2 | 234 | 253 | Two Monero swaps at once on both desks. |

`xmr-unfunded-punish` (lane 3), `xmr-maker-early-punish` (lane 2) and
`xmr-maker-claim-without-authorization` run the Maker's escrow probe; the first two ran on lanes
other than lane 1, the path `7b4e46da` fixed. The Monero tests took 304.2 minutes summed over
lanes.

## Lint and unit tests

On `7b4e46da`, on this host: `python3 -m unittest discover -s deploy/tests` passed (245 tests,
5 skipped); `scripts/check-requirements-traceability.sh`, `scripts/check-architecture-diagrams.sh`
and `scripts/check-threat-model.py` passed. The new unit test
(`test_the_escrow_probe_reads_the_stacks_manifest_on_every_lane`) fails on `e1fbecea` with the
`FileNotFoundError` of the run's first attempt and passes on `7b4e46da`. The Rust suites,
`scripts/check-all.sh`, the CI quality gates and gitleaks over the range were not run on this
host for this note; CI's own runs are not part of it.

## History

- A first full run at `ee66628d` (2026-11-22 04:15:51Z, same host, three lanes) was stopped
  after its first verdict: `node:xmr:xmr-unfunded-punish:TakerSellsLez` failed on lane 3 after
  1,092 s with `FileNotFoundError: .../runtime/lanes/3/market-bootstrap.env` at
  `deploy/scripts/node-e2e.py:7108`. Its partial results are not committed and are not evidence
  for any commit.
- The fix was then shown on that stack (images of `ee66628d`, the fixed harness): run alone with
  the lane pinned (`LEZ_LANE`), `xmr-unfunded-punish` passed on lane 2 (2,083 s) and lane 3
  (2,094 s), and `xmr-punish-replans` on lane 2 (1,087 s) and lane 3 (1,083 s). Those runs' files
  are not committed.
- The run of `d4475300` on 2026-11-21 ([regression-m4-2026-11-21](../regression-m4-2026-11-21/),
  160 of 160 in 221 minutes, on x86_64) is a run of another commit and host. It is not evidence
  for this tree, nor is this run evidence for that one.

## Not covered

- No run uses the `local` profile. No Monero test runs on the `refund` profile. The block
  explorers are not part of any test.
- Monero beyond an offline regtest chain: no Monero Stagenet test is part of `regress.sh full`.
- arm64 (linux/arm64 under Docker Desktop on macOS) only; x86_64 was not run on this commit.
- Two operators on two hosts: one operator ran both parties on one host.
- The systemd VM run with a Monero role (`scripts/run-maker-node-systemd-vm.sh`) and
  `scripts/run-zebra-e2e.sh` were not run.
- No desk recordings were made at `7b4e46da`.
- Why the Taker's first Chat message was lost in two tries of
  `ui:btc:logos-happy:TakerSellsForeign` is not established (above).
- One run without `actor_deployment_invalid` and without the refund retry loop does not show
  either intermittent defect gone.
- The devnet images run development builds with the devnet test hooks. No release package was
  built or run from this commit.
