# Full regression of `7d09351a`, 2026-11-17

One run of every test of `deploy/scripts/regress.sh full` (159: `regress.sh list full`) on the
Docker devnet with the Monero regtest chain up (`LEZ_MONERO=1`), three lanes at a time
(`LEZ_LANES=3`), without desk recordings: run `runtime/regress/20261117T000855Z`, 2026-11-17
00:08:55Z to 03:43:21Z (UTC), at `7d09351a`. It supersedes the
[run of 2026-11-16](../regression-m4-2026-11-16/) (158 of 159 at `39e60679`) with the same 159
tests. It is the first full run on a tree that carries `main` (`6d87d958`, the 0.3.2 release with
PRs #148 and #153), the release version 0.4.0 and the regression speed changes listed below.

**Result: 159 of 159 passed in one run of 214 minutes, every test on its first try.** 159 tries,
no NOT-EXERCISED try, no timeout, 0 frozen tries; 1 new LEZ chain (both `zec-balance-refusal`
tests at once on it); 4 timing-profile switches. All 16 Monero tests passed. Test time summed
over lanes 34,932 s over 159 tries (50,465 s over 162 tries on 2026-11-16).

| kind | tests | minutes (summed over lanes) | 2026-11-16: tries | 2026-11-16: minutes |
|---|---:|---:|---:|---:|
| `node:btc` | 53 | 196.2 | 53 | 298.8 |
| `node:zec` | 51 | 123.3 | 54 | 189.2 |
| `node:xmr` | 13 | 141.2 | 13 | 178.3 |
| `ui:btc` | 17 | 44.8 | 17 | 62.9 |
| `ui:zec` | 22 | 53.8 | 22 | 81.9 |
| `ui:xmr` | 3 | 22.9 | 3 | 29.9 |
| all | 159 | 582.2 | 162 | 841.0 |

(`results.tsv`, column 4 in seconds. Here every test has one try. The 2026-11-16 columns are that
note's table, which counts tries: its `node:zec` holds 51 tests and three NOT-EXERCISED tries of
360 s, its `node:btc` the failed try's 1,925 s.) Wall time 329 to 214 minutes (-35%), summed
test time -31%.

`results.tsv` is the runner's table (test, try, verdict, seconds, finished at, lane, LEZ chains
made so far by this run); `regress.log` is the runner's log, every try's start and verdict line
carrying the LEZ finalized height and tip. `logs/` holds the four Taker-refund-offline tests'
logs of this run and, in `before/`, the runner output and logs of the same four tests run just
before it (below), as the runner printed them. None holds a local path. The other per-test logs
are not committed; the lines quoted below are from them.

## The four Taker-refund-offline tests before the run

`7d09351a` changes the cut check of the Taker-refund-offline scenarios (below). Right before the
full run, on the same images and the same chain, the four tests that use it ran on their own:
run `20261116T235559Z`, 23:55:59Z to 00:08:55Z, 4 of 4 on their first try in 12 minutes, 0
frozen tries (`logs/before/`). In three of the four (`node:btc:taker-refund-offline:TakerSellsLez`
and `node:zec:zec-taker-refund-offline` in both directions) the stopped Maker had removed its
Chat socket by the Taker's second check ("the Taker's side is still refused: connect ENOENT, list
Delivery EACCES"), the case the change accepts; in the fourth the socket was still there
(EACCES). The full run shows the same split in the same three tests (`logs/`).

## History since the 2026-11-16 run

- **The full run of `406a140a`, 2026-11-16 evening** (`20261116T192438Z`, 19:24:38Z to
  23:07:16Z): 157 of 159 passed in 222 minutes, 1 new chain, 2 profile switches, 0 frozen tries;
  `node:zec:zec-claim-collision:TakerSellsForeign` passed on its third try after two
  NOT-EXERCISED. Both failures were `node:zec:zec-taker-refund-offline` (TakerSellsLez and
  TakerSellsForeign), each with "FAILED: the cut does not hold on the Taker's side: {'chat':
  'ENOENT', 'delivery': 'EACCES', 'delivery_fds': 0, 'uid': 4713}". The scenario stops the Maker
  and checks from the Taker's container that Chat and Delivery are still refused; the check
  accepted only EACCES on the Maker's Chat socket. With the faster development build a stopped
  Maker Node finishes its shutdown inside the stop grace period and removes its sockets, so the
  Taker's connect found no socket (ENOENT). This is a run of an earlier commit, not evidence for
  `7d09351a`, and is not committed here. `406a140a` is not in this history: its series also held
  `fix(regress): the Zcash refund journeys run in group refund` (`10b62b37`), which was dropped;
  the other three commits came back as `8f85c905`, `6e9133ed` and `c0b9861e`.
- **At `c0b9861e`** (without the dropped commit) the two tests failed again the same way
  (`20261116T233024Z`, 0 of 2, ENOENT), so the cause was the check, not the grouping.
- **`7d09351a`** (`test(node-e2e): a stopped Maker's missing Chat socket also counts as cut`):
  no socket is no Chat either, so ENOENT with the Maker stopped counts as cut; the check after a
  Maker restart under the cut (`kill_under_cut`) stays strict. Then the four tests above, then
  this run.

## The Monero tests

All 16 passed on their first try. All are TakerSellsLez (the only Monero direction), against
Monero regtest on the devnet (per lane a `monerod` with funding, Maker and Taker
`monero-wallet-rpc`), `fast` profile. Each swap's windows, as the logs print them: "the Maker
funds by +290 s, the Taker refunds from +650 s, the Maker punishes from +910 s" (+350, +790,
+1090 s on 2026-11-16; `c0b9861e`). Lot 1 XMR against 50,000 LEZ units.

| test | lane | s | s on 2026-11-16 | what its log shows |
|---|---|---:|---:|---|
| `node:xmr:xmr-happy` | 3 | 212 | 239 | "settled completed: LEZ moved {'maker': 50000, 'taker': -50000} as both Nodes say"; the Taker's wallet received 0.9981844 XMR. |
| `node:xmr:xmr-taker-refund` | 1 | 801 | 1,000 | "settled refunded: LEZ moved {'maker': 0, 'taker': 0}"; the Maker's wallet swept its Monero back (0.9981844 XMR), the Taker's received nothing. |
| `node:xmr:xmr-maker-never-funds` | 2 | 719 | 886 | "settled refunded", "no Monero moved: the Maker's wallet sent nothing, the Taker's received nothing". |
| `node:xmr:xmr-punish` | 2 | 989 | 1,206 | "the Maker punished; its LEZ owner's nonce moved 15 → 16"; neither wallet received Monero, the funding "stays at the shared address: neither party received it". |
| `node:xmr:xmr-concurrent` | 1 | 301 | 350 | "settled both: LEZ moved {'maker': 100000, 'taker': -100000}", the Taker's wallet received two outputs. |
| `node:xmr:xmr-kill-taker` | 1 | 1,584 | 1,970 | "taker killed at 7 points: after the take → completed; lock in flight → completed; locked → completed; Monero verified → completed; claim authorised → completed; waiting for the Maker's claim → completed; sweeping → completed". |
| `node:xmr:xmr-kill-maker` | 2 | 1,433 | 2,014 | "maker killed at 6 points: after the take → completed; lock observed → completed; funding in flight → completed; funded → completed; claim in flight → completed; claim tool running → completed". |
| `node:xmr:xmr-taker-refund-kill` | 3 | 831 | 993 | The Taker killed right after its refund's admission ("back up: refund_in_flight"); "settled refunded", the Maker's wallet swept its Monero back. |
| `node:xmr:xmr-monerod-down` | 3 | 462 | 604 | Before a take "the Maker withdrew offer … and refuses to publish on Monero: chain dependency is unavailable for route"; mid-swap "monerod was down 91 s (the funding cutoff is 197 s away)"; both swaps completed. |
| `node:xmr:xmr-wallet-restart` | 1 | 430 | 507 | Both swaps "settled completed" with a role's `monero-wallet-rpc` restarted mid-swap. |
| `node:xmr:xmr-outage-before-take` | 3 | 216 | 262 | "the Taker refused the take: Taker dependency unavailable (authenticated_delivery_unavailable)"; restored, the swap completed. |
| `node:xmr:xmr-cut-after-lock` | 1 | 243 | 282 | "the cut held: 60 samples of the Maker's Chat socket, none with a connection"; "settled completed". |
| `node:xmr:mixed-three-pair` | 3 | 254 | 388 | A Monero, a Bitcoin and a Zcash swap of the same Nodes at once: "all three completed; LEZ transactions signed by the owner accounts meanwhile: {'maker': 1, 'taker': 7}". |
| `ui:xmr:xmr-happy` | 3 | 212 | 266 | The Monero swap clicked through both desks, as in the recorded run. |
| `ui:xmr:xmr-taker-refund` | 2 | 869 | 1,058 | The Taker desk's refund journey: "the Maker Node has swept its Monero back, and both desks show the swap refunded". |
| `ui:xmr:xmr-concurrent` | 2 | 290 | 470 | Two Monero swaps at once on both desks, as in the recorded run. |

## What changed since the 2026-11-16 run

`git log --no-merges 39e60679..7d09351a` holds 27 commits; the merges are `fdaf8264` (`main` at
`6d87d958`) and `4a4a7fe6` (the 2026-11-16 evidence, `4432ad38`).

- **The merge of `main`** (`fdaf8264`, with `079b07dc` fixing a harness note the merge carried
  into the Zcash catalog fixture): the 0.3.1 and 0.3.2 releases (`b5c4f44e`, `01de187d`); Chat
  0.2.2 published with Delivery pinned to 0.2.x (`477bcb06`, PR #148); the remote-RPC fixes of
  PR #153: the LEZ v0.2 sidecar reads finalized LEZ runs in batches instead of two calls per block
  (`7a09242f`), the Zcash SDK accepts a LEZ tip that only grew during an observation
  (`b854585a`), native Nodes on Linux reach the chains over RPC only (`870e91e8`), the launcher
  fixes its package's modes (`c77849b2`), a chain-RPC delay proxy for tests (`2e69dde2`); eight
  dependency advisories on the upstream-pinned libp2p stack exempted (`2f620981`); `main`'s
  evidence and docs (`f033ff7d`, `f3550428`, `a78248f4`, `3b684e4e`, `47e01289`, `fb8d2480`,
  `addb2566`, `4b947889`, `21f68467`).
- **Version 0.4.0**: every shipped component reads 0.4.0 (`54dd9319`); a doc link CI refused
  (`76e824eb`).
- **Development builds**: `sha2`, `zeroize` and `lez-swap-store` built optimized in development
  builds, which the devnet images run (`732a5f87`); release builds and what the code computes are
  unchanged. Its message records, on twelve tests before and after: a Maker Node restart from
  31 s to 5 s, `reset-swaps.sh` from 27-30 s to 9-10 s, the Maker's actor attempts a median
  15.6 s apart for Bitcoin before and 3.5 s after.
- **Runner**: `zec-claim-collision` starts early in its group so its retries overlap other lanes'
  work (`8f85c905`); the two `zec-balance-refusal` tests share one new chain and run at once on
  lanes 1 and 2 (`6e9133ed`).
- **Monero windows**: a Maker drive counted at 70 s instead of 90 s, so on `fast` C, R, P =
  +290, +650, +910 s (were +350, +790, +1090), on `refund` +290, +650, +865 s, on `local` +1820,
  +2180, +2680 s; Bitcoin and Zcash values unchanged (`c0b9861e`).
- **Tests**: the Taker-refund-offline cut check accepts a missing Chat socket with the Maker
  stopped (`7d09351a`).
- **Docs**: the 2026-11-16 evidence (`4432ad38`).

The run does not separate what each change saved. The Bitcoin engine's crates
(`crates/btc-core-adapter`, `crates/btc-reference-actor`, `crates/swap-store`) are unchanged
since `39e60679`; the Zcash SDK and the LEZ v0.2 sidecar changed with `main`.

## Source and images

Tests and images from `7d09351a`, tree clean (`regress.log` prints the commit and "tree dirty:
0"). Image ids, creation times and the build-source check are in [PROVENANCE.md](PROVENANCE.md).

## Not covered

- Monero beyond an offline regtest chain: no stagenet or mainnet; x86_64 (linux/amd64) only.
- The Monero windows of the `refund` and `local` profiles changed with `c0b9861e`; no Monero test
  runs on `refund`, and no run uses `local`.
- The devnet images run development builds; the optimization of `732a5f87` is in development
  builds only. No release package was built or run from this commit.
- The systemd VM run with a Monero role (`scripts/run-maker-node-systemd-vm.sh`) was not run; the
  Maker unit's Monero role is checked statically and with stub programs only.
- The Monero punish rows and the Taker's in-doubt rows were not seen on a desk
  ([regression-m4-desks-2026-11-16](../regression-m4-desks-2026-11-16/)).
- Proven by unit tests only, not on the stack: the lost-nonce punish re-plan; the switch of a
  Maker's stopped claim to the refund or the punishment; a claim submitted but not recorded; the
  regtest observer mining a funding or refund sweep left in the pool. Whether the read and mining
  retries fired in this run was not checked: the Node logs are not kept with the run.
- A Monero transfer or sweep that hits the Digest failure is not made again; the swap is left in
  doubt (on regtest the observer mines what reached the pool).
- Owner-account nonces, as the 2026-11-15 note says: the Taker's tag 14 and the Maker's tag 17
  are signed without coordination with the other pairs' transactions.
- Two Bitcoin-adjacent defects are not fixed in this tree; neither showed in this run, which does
  not show they are gone: the Maker's Bitcoin claim journaled `unknown` after one submission
  attempt and never sent (`node:btc:restart-maker:TakerSellsForeign` on 2026-11-16, see
  [that note](../regression-m4-2026-11-16/); `node:btc:offline-after-claim:TakerSellsForeign` on
  2026-11-13; a fix that rechecks readiness before the broadcast is prepared and not landed: it
  changes the frozen Bitcoin engine), and the Maker's check of its Bitcoin actor's state database
  by size and times (`actor_deployment_invalid`, `ui:btc:happy:TakerSellsLez`, 2026-11-13). Here
  `node:btc:restart-maker:TakerSellsForeign` passed in 82 s and `ui:btc:happy:TakerSellsLez` in
  74 s.
- macOS, linux/arm64, and any public network are untested on this head.
