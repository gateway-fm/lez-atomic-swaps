# Full regression of `86bdb8a4`, 2026-11-20

One run of every test of `deploy/scripts/regress.sh full` (160: `regress.sh list full`) on the
Docker devnet with the Monero regtest chain up (`LEZ_MONERO=1`), three lanes at a time
(`LEZ_LANES=3`), without desk recordings: run `runtime/regress/20261120T191249Z`, 2026-11-20
19:12:49Z to 22:57:55Z (UTC), at `86bdb8a4`.

## What the tree is

The first full run on a tree that contains `main` at `c87497a9` (version 0.3.4; ADR 0216, ZEC
follows the common flow). Against `9a15d94b`, the commit of the
[2026-11-19 run](../regression-m4-2026-11-19/), `git log --no-merges 9a15d94b..86bdb8a4` holds 87
commits: 68 from `main` and 19 of the delivery work.

What `main` brought, in summary: [ADR 0216](../../architecture/0216-zec-taker-reveals-first-in-both-directions.md)
(the Taker reveals first in both directions, so in ZEC TakerSellsLez the Taker claims Zcash first
and the Maker follows on LEZ with the preimage it reads from that claim), with the swap engine,
SDK, adapters, Nodes, desks and devnet scenarios in that order; each ZEC profile admits only the
direction it was sized for, with the `-v7`/`-v8` twins for TakerSellsLez of `-v5`/`-v6`; the
Maker's LEZ claim admission and queue; a Maker whose LEZ claim was lost refunds its Zcash (#178);
a LEZ claim whose nonce keeps being taken is re-planned while it can land (#172); the new
devnet scenario `zec-maker-early-claim` (the Maker's claim on the Taker's LEZ before the reveal,
TM-0030), the one test this suite has that the 2026-11-19 run did not;
`scripts/check-all.sh` and lint-first CI; version 0.3.4; the RUSTSEC exemptions of 2026-11-18;
and `main`'s own evidence notes. The release-version files of this tree keep 0.4.0.

The delivery work since `9a15d94b`, by theme:

- The merge of `main` (`d12ff032`, `a5228d9f`) and its fix-ups: `323f7486` (the ZEC dealer
  fixture builds without its feature), `7b6b8ad7` (`check-all.sh` keeps the Monero crates in the
  root workspace checks), `dd757bf0` (`main`'s deploy tests read the merged tree), `8ab8ad01` (the
  atomicity property measures Zcash's safety interval Maker leg to Taker leg).
- Take-time ZEC in the new order: `320f2675` (the take-time agreement `zec_reserve_v1` follows
  ADR 0216: the Taker makes the preimage before it reserves, in both directions; each direction
  runs under its profile's twin).
- The fixes of the open v0.4 issues: `b6882609` (#180: the two-lanes regress test asserts the
  overlap, not a time bound), `26efb15c` (the launcher tests make their fixtures under
  umask 022), `f324f445` (#181: a Zcash swap keeps its stored terminal state once its files are
  archived), `34b794d7` (#130: a claim held by the pre-reveal gate reads `queued` with its
  confirmations), `b9855e04` (#151: a Node the launcher refuses to start shows the launcher's
  line on the desk).
- Monero documents: `a2c23e8b`
  ([ADR 0217](../../architecture/0217-xmr-taker-sells-lez-deviates-from-the-common-flow.md),
  XMR selling LEZ keeps its own order, an exception to the common flow), `1ac28f04` (threat-model
  row TM-0031, the Monero punishment of an unfunded swap), `a83bae24` (the Monero windows follow
  `gen-config.sh`'s formula on every profile), `8d65f151` (the systemd drop-in's windows are
  packaging defaults).
- Other documents and files without code: `d052b013`, `18e9aa89` (traceability), the evidence
  note `296783ed` of the 2026-11-19 run, and two commits of `.gitleaksignore` entries
  (`79101141`, `154d9e0f`).

`86bdb8a4` is the merge of the issue fixes, whose tested head was `b9855e04`, into the delivery
work. Its `crates`, `compat`, `apps` and `modules` trees are those of `b9855e04`; it differs from
`b9855e04` in documents, `deploy/README.md` and one comment of
`packaging/systemd/lez-maker-node-monero.conf`.

**Result: 160 of 160 passed in one run of 225 minutes, each on its first try, 0 frozen tries.**
160 tries (`results.tsv`, all PASSED, all try 1): no FAILED or NOT-EXERCISED try, no timeout;
1 new LEZ chain (both `zec-balance-refusal` tests at once on it); 3 timing-profile switches. All
16 Monero tests passed on their first try. Test time summed over lanes 36,188 s over 160 tries
(35,135 s over 160 tries, 159 tests, in the run of `9a15d94b`).

| kind | tests | minutes (summed over lanes) | run of `9a15d94b`: tests | minutes |
|---|---:|---:|---:|---:|
| `node:btc` | 53 | 188.7 | 53 | 197.2 |
| `node:zec` | 52 | 146.3 | 51 | 129.8 |
| `node:xmr` | 13 | 141.0 | 13 | 139.0 |
| `ui:btc` | 17 | 42.3 | 17 | 43.5 |
| `ui:zec` | 22 | 61.4 | 22 | 53.2 |
| `ui:xmr` | 3 | 23.4 | 3 | 22.9 |
| all | 160 | 603.1 | 159 | 585.6 |

(`results.tsv`, column 4 in seconds, every try counted; the 2026-11-19 `node:zec` minutes include
its 89 s NOT-EXERCISED try.) `node:zec` holds the new `zec-maker-early-claim:TakerSellsLez`
(538 s). The ZEC TakerSellsLez refund journeys take longer than in the run of `9a15d94b`: 523 to
557 s each against 286 to 313 s (`node:zec`'s `zec-taker-refund`, `zec-taker-refund-offline`,
`zec-maker-refund`, `zec-refund-then-maker-lock` and `ui:zec`'s `zec-taker-refund` and
`zec-maker-refund`, all TakerSellsLez); `node-e2e.py` provisions these under
`deterministic-local-devnet-v8`, whose LEZ refund delay is 8 minutes (4 under `-v6`). Their
TakerSellsForeign twins took what they took on 2026-11-19 (272 s against 282 s for
`node:zec:zec-taker-refund`).

Wall time 225 minutes (215 in the run of `9a15d94b`). By group (`regress.log`): `fast` 166.1
minutes (162.9), `zec-v6` 32.9 (25.9), `refund` 20.3 (20.3), the own-chain pair 5.3 (5.7). The
median test took 111.5 s (the 80th and 81st of 160 took 111 and 112 s; 120 s in the run of
`9a15d94b`).

No smoke or targeted run preceded the full run on its chain: `up.sh --fresh-lez`, then
`regress.sh full`.

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
`node:xmr:xmr-kill-taker`, swap `5bcc7c0f…`; lane 2 `node:xmr:xmr-kill-maker`, `a0897224…`;
lane 3 `node:btc:kill-taker:TakerSellsLez`, `1b1363f8…`) to the end of the file (2,171, 2,191 and
2,775 lines; each ends with its lane's last test of the run). The whole files hold the same six
`actor_deployment_invalid` lines the earlier notes list, for the same older swaps (lane 1
`a1321f30`, `61123f4e`, `1aee1825`; lane 2 `a5f63bc4`; lane 3 `fd3af315`, `1dd5df0b`), all
before this run's part.

**The refund retry loop: 0 lines.** The same three parts hold no
`submission_outcome_unknown lez_refund` line, and no `submission_outcome_unknown` line for any
action. In the whole files the line names the three older swaps the earlier notes list, all
before this run's part: lane 3 `zec-60893343` 265 times, lane 1 `zec-4b79f485` 534 times and
`zec-cc4c7a0b` once.

**Bitcoin mining of the 421 refund-path blocks: 1 to 3 s.** A Bitcoin refund path mines 421
blocks (the CSV plus one) between the Taker's `claim_available` and the kill walk's point "both
legs locked, refund times running". From the timestamps of the last `claim_available` line
before that kill point and the kill point line of each test's log:

| test | lane | this run | run of `9a15d94b` | test s | run of `9a15d94b`: s |
|---|---|---:|---:|---:|---:|
| `node:btc:kill-taker:TakerSellsLez` | 3 | 2 s | 1 s | 1,226 | 1,207 |
| `node:btc:kill-taker:TakerSellsForeign` | 1 | 3 s | 2 s | 880 | 893 |
| `node:btc:kill-maker:TakerSellsLez` | 3 | 1 s | 1 s | 1,231 | 1,300 |
| `node:btc:kill-maker:TakerSellsForeign` | 2 | 1 s | 1 s | 933 | 936 |

The log lines carry whole seconds, so each figure is good to about a second. All four kill walks,
both `late-claim-unmined` tests (696 and 480 s, TakerSellsLez and TakerSellsForeign; 695 and
473 s in the run of `9a15d94b`) and both `concurrent-kill-maker` tests (128 and 170 s) passed on
their first try (`results.tsv`). In both `kill-taker` walks every claim after a Taker restart was
answered on its first call (`replay=False` on each of the 22 lines; no
`taker_action_execution_unavailable`); the kill walk's replay of a refused claim did not run in
this run.

**`node:zec:zec-claim-collision:TakerSellsForeign` passed on its first try** (lane 1, 74 s) and
formed the collision: "revealing LEZ claim superseded (planned nonce 49, live 50); re-plan 1 at
nonce 50". In the run of `9a15d94b` it was NOT-EXERCISED on its first try and passed on its
second; in the run of `10a785ad` it passed on its first. Whether the collision forms depends on
which LEZ transaction a block takes first.

## ZEC TakerSellsLez profiles

`regress.log` does not name a test's ZEC profile; it names only each group's switch (`zec-v6`:
`deterministic-local-devnet-v6`; `refund` and the own-chain pair: `-v5`). Two other records do:

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

| test | lane | s | s in the run of `9a15d94b` | what its log shows |
|---|---|---:|---:|---|
| `node:xmr:xmr-happy` | 1 | 191 | 174 | "settled completed: LEZ moved {'maker': 50000, 'taker': -50000} as both Nodes say"; the Taker's wallet received 0.9981784 XMR. |
| `node:xmr:xmr-taker-refund` | 1 | 882 | 877 | The Maker's wallet swept its Monero back (0.9981808 XMR), the Taker's received nothing. |
| `node:xmr:xmr-maker-never-funds` | 3 | 814 | 810 | "no Monero moved: the Maker's wallet sent nothing, the Taker's received nothing". |
| `node:xmr:xmr-punish` | 2 | 1,084 | 1,081 | "the Maker punished; its LEZ owner's nonce moved 13 → 14"; the funding "stays at the shared address: neither party received it". |
| `node:xmr:xmr-concurrent` | 1 | 280 | 252 | "settled both: LEZ moved {'maker': 100000, 'taker': -100000}", the Taker's wallet received two outputs. |
| `node:xmr:xmr-kill-taker` | 1 | 1,373 | 1,458 | "taker killed at 7 points", every one "→ completed". |
| `node:xmr:xmr-kill-maker` | 2 | 1,304 | 1,327 | "maker killed at 6 points", every one "→ completed". |
| `node:xmr:xmr-taker-refund-kill` | 2 | 888 | 878 | The Taker killed right after its refund's admission ("back up: refund_in_flight"); the Maker's wallet swept its Monero back. |
| `node:xmr:xmr-monerod-down` | 3 | 550 | 492 | Before a take "the Maker withdrew offer … and refuses to publish on Monero: chain dependency is unavailable for route"; mid-swap "monerod was down 88 s (the funding cutoff is 230 s away)"; "settled completed". |
| `node:xmr:xmr-wallet-restart` | 1 | 419 | 369 | "settled completed" with a role's `monero-wallet-rpc` restarted mid-swap. |
| `node:xmr:xmr-outage-before-take` | 2 | 231 | 221 | "the Taker refused the take: Taker dependency unavailable (authenticated_delivery_unavailable)"; restored, "settled completed". |
| `node:xmr:xmr-cut-after-lock` | 3 | 220 | 197 | "the cut held: 56 samples of the Maker's Chat socket, none with a connection"; "settled completed". |
| `node:xmr:mixed-three-pair` | 1 | 224 | 206 | A Monero, a Bitcoin and a Zcash swap of the same Nodes at once: "all three completed; LEZ transactions signed by the owner accounts meanwhile: {'maker': 1, 'taker': 7}". |
| `ui:xmr:xmr-happy` | 1 | 175 | 177 | The Monero swap clicked through both desks. |
| `ui:xmr:xmr-taker-refund` | 3 | 947 | 959 | "the Maker Node has swept its Monero back, and both desks show the swap refunded". |
| `ui:xmr:xmr-concurrent` | 2 | 282 | 237 | Two Monero swaps at once on both desks. |

The Monero tests took 164.4 minutes summed over lanes (161.9 in the run of `9a15d94b`).

## Lint and unit tests

Not run on `86bdb8a4` as a whole; what was run, and on which commit (the operators' log of the
work):

- On `b9855e04`, the tested head of the issue fixes, whose `crates`, `compat`, `apps` and
  `modules` trees `86bdb8a4` has unchanged: `scripts/check-all.sh` (full), the root workspace's
  tests (1,593 passed), the Node suites with `pair-zec` (204) and with all features (302), the
  deploy unit tests (194) and gitleaks, all passing. Also on its images, on another chain: smoke
  6 of 6 and five targeted tests, 5 of 5. Those images are not this run's.
- On `86bdb8a4`: `scripts/check-all.sh --fast`, the documentation checks and gitleaks, passing.
- Earlier, on `8ab8ad01` (the merge of `main` with take-time ZEC in the new order, before the
  issue fixes): `check-all.sh`, 1,590 root tests, the Node suites, 193 deploy tests and the
  sidecar's verification passed, and smoke 6 of 6 and a targeted run of 22 tests, 22 of 22.

`check-all.sh` skips the sidecar workspace's Clippy and rustdoc unless the rapidsnark libraries
are given (`RAPIDSNARK_LIB_DIR`); whether they were given on `b9855e04` is not recorded.
`scripts/verify-lez-v02-sidecar.sh`, which runs them, passed on `8ab8ad01`; the `compat` tree
(the sidecar) of `86bdb8a4` is that of `8ab8ad01`. CI's own runs are not part of this note.

## History

The run of `9a15d94b` on 2026-11-19 ([regression-m4-2026-11-19](../regression-m4-2026-11-19/),
159 of 159 in 215 minutes, `zec-claim-collision` on its second try) is a run of another commit:
the tree before `main` at `c87497a9`, ADR 0216's order, take-time ZEC in that order and the
issue fixes. It is not evidence for this tree, nor is this run evidence for that one.

## Not covered

- No run uses the `local` profile. No Monero test runs on the `refund` profile. The block
  explorers are not part of any test.
- Monero beyond an offline regtest chain: no stagenet or mainnet. x86_64 (linux/amd64) only;
  macOS and linux/arm64 were not run on this commit.
- The systemd VM run with a Monero role (`scripts/run-maker-node-systemd-vm.sh`) was not run, nor
  was `scripts/run-zebra-e2e.sh`.
- No desk recordings in this run.
- One run without `actor_deployment_invalid` and without the refund retry loop does not show
  either intermittent defect gone.
- Whether these paths ran is not known from the logs read: `main`'s fix for #156 (a readiness
  re-read, a Bitcoin send reported `NotAttempted`, the Taker's observer retrying an admitted
  claim; the Maker Nodes' logs do not say, and the Taker Nodes' `node.log` files hold only lines
  since their last start and were not used); the sidecar's retry of a submission refused before
  any broadcast; and the #181 fallback (a Zcash swap answered from its stored terminal state
  after its files were archived).
- The #130 `queued` path: the Node's side ran once that a log shows. In
  `ui:zec:zec-concurrent:TakerSellsLez` the Taker Node reported one of the two swaps as
  `claim_in_progress (gen 2, claim_awaiting_zcash_confirmations)` at one Zcash confirmation, then
  completed it. No test asserted the desk's "Claim queued" row, and the scenario that does
  (`LEZ_ZEC_CLAIM_AT_ONE_CONFIRMATION=1`) is not part of the suite.
- The desk message of #151 (the launcher's line under "Node unavailable") does not appear on the
  Compose stack: the devnet containers log to their own output, not a log file, so the desks
  keep the generic message.
- Monero's tag 17 (punish) and ZEC claims made from one Maker account never collided on the
  devnet: a lane runs one test at a time, and no test makes a Monero punish and a ZEC claim of
  the same Maker at once.
- The devnet images run development builds. No release package was built or run from this
  commit.
