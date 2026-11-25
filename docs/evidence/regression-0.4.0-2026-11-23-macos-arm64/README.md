# Full regression of `27526e2f`, 2026-11-22/23, macOS arm64

One run of every test of `deploy/scripts/regress.sh full` (176: `regress.sh list full`) on the
Docker devnet with the Monero regtest chain up (`LEZ_MONERO=1`) and the devnet test hooks built in
(`LEZ_TEST_HOOKS=1`), three lanes at a time (`LEZ_LANES=3`), without desk recordings: run
`runtime/regress/20261122T211901Z`, 2026-11-22 21:19:02Z to 2026-11-23 01:42:08Z (UTC), at
`27526e2f`. One operator ran both parties, Maker and Taker, on one host: an Apple-silicon Mac
(macOS 26.7.3, 10 cores), Docker Desktop 29.9.2, local linux/arm64 dev-profile images, Monero
regtest. No second operator and no second host took part.

## What the tree is

`27526e2f` is `5a7688a9` with `m4/node-stagenet` merged (`5c9694fd`, `0d8d17af`: the Monero
Stagenet evidence set and documents). Against `7b4e46da`, the commit of the
[2026-11-22 run](../regression-0.4.0-2026-11-22-macos-arm64/),
`git log --oneline --no-merges 7b4e46da..27526e2f` holds 22 commits:

- The ZEC second-lock lookup: `f1c8a25d` (`crates/lez-bridge-adapter`, `crates/swap-store`).
  Selling LEZ, the Maker re-observes the Taker's LEZ lock on every drive while its Zcash funding
  confirms and commits a revision each time; its LEZ claim looked for its Zcash second lock at
  predecessor revisions 1 to 16 only, so a funding slower than about 16 drives left the claim
  at `SecondLockUnavailable`. The store now finds the transition by its row.
- The sidecar's in-doubt submissions: `c2fafcc7`, `b7d105bd`, `e8450af1`
  (`compat/lez-v0_2-sidecar`). A tag-15 claim, or another submission, journaled in flight when
  its process was killed is looked up on the chain again and sent once more when absent, rather
  than replayed as `UnknownSubmissionOutcome` until the refund time (issue #187).
- The Taker gateway's resend: `7eebe1fc` (`crates/node-common`). An unanswered Chat request is
  queued again, the same frame, at 1, 2, 4, 8 and 16 s inside its 25 s wait; a first request
  dropped by the Maker's Delivery node before its conversation subscription is answered on a
  resend.
- `url` 2.5.9 for RUSTSEC-2026-0592 in every Cargo graph: `4e152830`.
- Harness: `zec-slow-funding` (`4c51470c`, `c4c4ac5a`, `a7131e0e`); `xmr-kill-maker`'s two claim
  points held in the sidecar by its test hooks (`3ba3e25b`, `5a7688a9`); the Logos phase of
  `logos-happy` kept before the desks go local (`941c1a0e`, `5002e330`); take-time ZEC reads
  through `node_command` (`023b0d2f`).
- Documents: `16780f9b`, `3403044a`, `facf41a4`, `f4530f88`, `0d8d17af`, the Stagenet evidence
  `5c9694fd`, and the two notes of `7b4e46da` (`67e13ca1`, `f059d3fa`).

Code outside tests and the harness that changed: `crates/lez-bridge-adapter`, `crates/swap-store`,
`crates/node-common`, `compat/lez-v0_2-sidecar`, and the `url` pins in seven manifests. `apps`,
`modules` and `packaging` are those of `7b4e46da`.

**Result: 176 of 176 passed in one run of 263 minutes, each on its first try, 0 frozen tries.**
176 tries (`results.tsv`, all PASSED, all try 1): no FAILED or NOT-EXERCISED try, no timeout;
1 new LEZ chain (both `zec-balance-refusal` tests at once on it); 3 timing-profile switches. All
25 Monero tests passed on their first try. No rerun was needed.

| kind | tests | minutes (summed over lanes) | run of `7b4e46da`: tests | minutes |
|---|---:|---:|---:|---:|
| `node:btc` | 56 | 167.6 | 56 | 156.8 |
| `node:zec` | 56 | 144.9 | 55 | 138.2 |
| `node:xmr` | 22 | 300.6 | 22 | 282.1 |
| `ui:btc` | 17 | 35.4 | 17 | 32.7 |
| `ui:zec` | 22 | 53.7 | 22 | 51.8 |
| `ui:xmr` | 3 | 23.4 | 3 | 22.1 |
| all | 176 | 725.7 | 175 | 683.8 |

(`results.tsv`, column 4 in seconds, every try counted.) The one test the run of `7b4e46da` did
not have is `node:zec:zec-slow-funding:TakerSellsLez`.

Wall time 263 minutes (248 in the run of `7b4e46da`). By group (`regress.log`): `fast` 199.2
minutes, `zec-v6` 37.8, `refund` 21.3, the own-chain pair 4.0. The median test took 86.5 s (the
88th and 89th of 176 took 86 and 87 s). Test time summed over lanes 43,540 s over 176 tries.

No smoke or targeted run preceded the full run on its chain: the stack and its lanes were wiped
(`down.sh --wipe`, `lanes.sh down 3 --wipe`), the stack started on a new chain
(`from-scratch.sh`'s `rust`, `build`, `stage` and `stack` phases with `LEZ_MONERO=1
LEZ_TEST_HOOKS=1`, then `up.sh`), then `regress.sh full`. The runs that showed the changes before
it, on other chains, are under "History".

`results.tsv` is the runner's table (test, try, verdict, seconds, finished at, lane, LEZ chains
made so far by this run); `regress.log` is the runner's log, every try's start and verdict line
carrying the LEZ finalized height and tip. Both are as the runner wrote them and hold no local
path. The per-test logs and the Nodes' own logs read below are not committed; the lines quoted
are from them.

## The changed paths in this run

**`node:xmr:xmr-kill-maker:TakerSellsLez`** (lane 2, 1,810 s): "maker killed at 7 points: after
the take → completed; lock observed → completed; funding in flight → completed; funded →
completed; claim in flight → completed; claim preparation held → completed; claim tool running →
completed". At the two claim points the Maker's claim tool was parked by its sidecar's
`hold-claim-preparations` and `hold-claim-submissions` hooks and the Maker container killed;
after each restart "the Maker drove its held claim again and recorded its one tag 15". In the
run of `f4530f88` (History) the same walk, then with its claim point held on the sequencer
forwarder, ended `attention_required (xmr_maker_did_not_claim)` with the sidecar answering
`UnknownSubmissionOutcome` to every redrive.

**`node:zec:zec-slow-funding:TakerSellsLez`** (lane 1, 127 s): the Maker's Zcash funding was held
unmined in zebra's mempool for 75 s; "the hold carried the Maker's journal from revision 3 to 44";
then "the Maker claimed the LEZ after its journal passed revision 44 (now 48):
['lez_followup_claim']; both Nodes completed".

**The three `logos-happy` journeys** passed on their first try (`ui:btc` TakerSellsForeign 134 s
on lane 1; `ui:zec` TakerSellsLez 104 s on lane 2, TakerSellsForeign 95 s on lane 3). In each the
Maker's Chat module received the Taker's first request at its first copy, the Maker having
subscribed to the conversation's topic 52, 33 and 998 ms before it reached its relay
(`scripts/logos-evidence.py` on the Logos phase `ui-e2e.sh` kept); no resend was needed.

## The facts carried over from the earlier runs

**`actor_deployment_invalid`: 0.** Each lane's Maker `node.log` in its socket volume was copied
out of the container after the run (`docker cp`; lanes 1 and 2 running, lane 3 stopped), searched
and deleted. The stack and the lanes started from wiped volumes, so the whole files (4,540, 3,416
and 4,206 lines) are this run's.

**The refund retry loop and in-doubt submissions: 0 lines.** The same files hold no
`submission_outcome_unknown` and no `UnknownSubmissionOutcome` line.

**Bitcoin mining of the 421 refund-path blocks: 0 to 1 s.** From the last `claim_available` line
before the kill walk's point "both legs locked, refund times running" to that line:

| test | lane | this run | test s | run of `7b4e46da`: s |
|---|---|---:|---:|---:|
| `node:btc:kill-taker:TakerSellsLez` | 3 | 1 s | 1,106 | 1,056 |
| `node:btc:kill-taker:TakerSellsForeign` | 2 | 1 s | 732 | 711 |
| `node:btc:kill-maker:TakerSellsLez` | 1 | 0 s | 1,137 | 1,105 |
| `node:btc:kill-maker:TakerSellsForeign` | 1 | 1 s | 763 | 719 |

The log lines carry whole seconds. Both `late-claim-unmined` tests (602 and 468 s) and both
`concurrent-kill-maker` tests (76 and 115 s) passed on their first try. In both `kill-taker`
walks every claim after a Taker restart was answered on its first call (`replay=False` on each of
the 22 lines).

**`node:zec:zec-claim-collision:TakerSellsForeign` passed on its first try** (lane 2, 67 s) and
formed the collision: "revealing LEZ claim superseded (planned nonce 64, live 65); re-plan 1 at
nonce 65".

## ZEC TakerSellsLez profiles

`regress.log` names each group's switch (`zec-v6`: `deterministic-local-devnet-v6`; `refund` and
the own-chain pair: `-v5`). The 23 swap records `ui-e2e.sh` exported under the run's `zec/` name
`-v5` 9 times, `-v6` twice, `-v7` 11 times and `-v8` once, the split of the run of `7b4e46da`. No
test log shows a claim held by the pre-reveal gate (`claim_awaiting_zcash_confirmations`).

## The Monero tests

All 25 passed on their first try, all TakerSellsLez, against Monero regtest on the devnet (per
lane a `monerod` with funding, Maker and Taker `monero-wallet-rpc`), `fast` profile, lot 1 XMR
against 50,000 LEZ units. Seconds, with the run of `7b4e46da` beside them:

| test | lane | s | s at `7b4e46da` |
|---|---|---:|---:|
| `node:xmr:xmr-happy` | 2 | 214 | 170 |
| `node:xmr:xmr-taker-refund` | 3 | 846 | 856 |
| `node:xmr:xmr-taker-refund-kill` | 2 | 867 | 857 |
| `node:xmr:xmr-maker-never-funds` | 1 | 812 | 806 |
| `node:xmr:xmr-punish` | 1 | 1,068 | 1,065 |
| `node:xmr:xmr-concurrent` | 3 | 304 | 278 |
| `node:xmr:xmr-kill-taker` | 1 | 1,533 | 1,346 |
| `node:xmr:xmr-kill-maker` | 2 | 1,810 | 1,389 (6 points) |
| `node:xmr:xmr-monerod-down` | 2 | 598 | 483 |
| `node:xmr:xmr-wallet-restart` | 3 | 355 | 353 |
| `node:xmr:xmr-outage-before-take` | 2 | 233 | 203 |
| `node:xmr:xmr-cut-after-lock` | 1 | 214 | 185 |
| `node:xmr:mixed-three-pair` | 3 | 207 | 186 |
| `node:xmr:xmr-unfunded-punish` | 3 | 2,137 | 2,150 |
| `node:xmr:xmr-punish-replans` | 1 | 1,100 | 1,091 |
| `node:xmr:xmr-maker-early-punish` | 3 | 1,105 | 1,090 |
| `node:xmr:xmr-maker-claim-without-authorization` | 3 | 793 | 772 |
| `node:xmr:xmr-taker-cheats` | 2 | 1,642 | 1,591 |
| `node:xmr:xmr-taker-clock-ahead` | 1 | 869 | 826 |
| `node:xmr:xmr-shared-address-dust` | 2 | 204 | 177 |
| `node:xmr:xmr-custody-credit` | 2 | 211 | 211 |
| `node:xmr:xmr-custody-credit-refund` | 2 | 917 | 844 |
| `ui:xmr:xmr-happy` | 3 | 195 | 163 |
| `ui:xmr:xmr-taker-refund` | 1 | 941 | 929 |
| `ui:xmr:xmr-concurrent` | 1 | 266 | 234 |

The Monero tests took 324.0 minutes summed over lanes. `xmr-unfunded-punish` (lane 3),
`xmr-maker-early-punish` (lane 3) and `xmr-maker-claim-without-authorization` (lane 3) ran the
Maker's escrow probe on a lane other than lane 1.

## Lint and unit tests

On `27526e2f`, on this host, before the run: `scripts/check-all.sh --fast` passed;
`python3 -m unittest discover -s deploy/tests` passed (5 skipped);
`scripts/check-requirements-traceability.sh`, `scripts/check-architecture-diagrams.sh` and
`scripts/check-threat-model.py` passed. On `f4530f88` (the same Rust tree but for the sidecar
commits): `cargo test -p lez-bridge-adapter --test sqlite_canonical_lez_funding_contract` 7
passed, `cargo test -p lez-swap-store` 225 passed. The gateway resend's tests (`7eebe1fc`) passed
natively with the Node suites (`--all-features` 323 passed, `--features pair-zec` 213 passed) and
in a Linux container one crate at a time. The full root-workspace tests, the CI quality gates and
gitleaks were not run on this host for this note; CI's own runs are not part of it.

## History

- The run of `f4530f88` (2026-11-22 14:56Z to 19:29Z, same host, three lanes): 175 of 176, the one
  failure `node:xmr:xmr-kill-maker:TakerSellsLez` at its claim point, the sidecar answering
  `UnknownSubmissionOutcome` to every redrive of the in-flight tag-15 claim until the Taker ended
  `attention_required (xmr_maker_did_not_claim)` (#187, fixed by `c2fafcc7`); not rerun. It is not
  evidence for this tree; its files are not committed.
- On `27526e2f`, before this run, on another chain: `xmr-kill-maker` (7 points), `zec-kill-maker`
  and `zec-kill-taker` both directions, `zec-taker-refund` both directions,
  `node:btc:taker-refund:TakerSellsLez`, `xmr-kill-taker` and `zec-slow-funding`, 10 of 10, each on
  its first try. Those files are not committed.
- `zec-slow-funding` on the images of `3403044a` (without `f1c8a25d`): failed. The Maker's journal
  went from revision 2 to 43 during the hold, the Taker claimed the Zcash, and the Maker logged
  "actor claim unavailable: LezClaim(Recovery(SecondLockUnavailable))" until the test timed out
  at the claim's inclusion. On the images of `a7131e0e` (with it): passed 3 of 3.
- The run of `7b4e46da` ([regression-0.4.0-2026-11-22-macos-arm64](../regression-0.4.0-2026-11-22-macos-arm64/),
  175 of 175, 174 on the first try) and the runs before it are runs of other commits; they are not
  evidence for this tree, nor is this run evidence for them.

## Not covered

- No run uses the `local` profile. No Monero test runs on the `refund` profile. The block
  explorers are not part of any test.
- Monero beyond an offline regtest chain: the Stagenet swaps are in their own set
  ([stagenet-0.4.0-2026-11-22](../stagenet-0.4.0-2026-11-22/)), not in `regress.sh full`.
- arm64 (linux/arm64 under Docker Desktop on macOS) only; x86_64 was not run on this commit.
- Two operators on two hosts: one operator ran both parties on one host.
- The systemd VM run with a Monero role (`scripts/run-maker-node-systemd-vm.sh`) and
  `scripts/run-zebra-e2e.sh` were not run.
- No desk recordings were made at `27526e2f`.
- The gateway's resend did not fire in this run: no `logos-happy` journey lost its first request.
  It was shown on the devnet with the race provoked
  ([LIMITATIONS](../../../submission/LIMITATIONS.md), "The Logos transport").
- One run without `actor_deployment_invalid`, the refund retry loop or an in-doubt submission does
  not show any of them gone.
- The devnet images run development builds with the devnet test hooks. No release package was
  built or run from this commit.
