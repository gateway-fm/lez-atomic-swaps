# Full regression of `feat/m4-monero`, 2026-11-16

One run of every test of `deploy/scripts/regress.sh full` (159: `regress.sh list full`) on the
Docker devnet with the Monero regtest chain up (`LEZ_MONERO=1`), three lanes at a time
(`LEZ_LANES=3`), without desk recordings: run `runtime/regress/20261116T092710Z`, 2026-11-16
09:27:10Z to 14:57:02Z (UTC), at `39e60679`. It supersedes the
[run of 2026-11-15](../regression-m4-2026-11-15/) (157 of 157 at `305b32e0`): the same 157 tests
plus the two desk journeys `ui:xmr:xmr-taker-refund` and `ui:xmr:xmr-concurrent`, which are in a
full run for the first time (they were recorded on their own in
[regression-m4-desks-2026-11-16](../regression-m4-desks-2026-11-16/)).

**Result: 158 of 159 passed in one run of 329 minutes; one failed,
`node:btc:restart-maker:TakerSellsForeign` (below).** 157 tests passed on their first try;
`node:zec:zec-claim-collision:TakerSellsForeign` was NOT-EXERCISED on tries 1 to 3 and passed on
try 4. 0 frozen tries, 1 timeout (the failure); 2 new LEZ chains (one for each
`zec-balance-refusal` test); 3 timing-profile switches. All 16 Monero tests passed on their first
try. Test time summed over lanes 50,465 s over 162 tries.

| kind | tests | minutes (summed over lanes) |
|---|---:|---:|
| `node:btc` | 53 | 298.8 |
| `node:zec` | 54 | 189.2 |
| `node:xmr` | 13 | 178.3 |
| `ui:btc` | 17 | 62.9 |
| `ui:zec` | 22 | 81.9 |
| `ui:xmr` | 3 | 29.9 |

(`results.tsv`, column 4 in seconds, every try counted: `node:btc` includes the failed try's
1,925 s, `node:zec` the three NOT-EXERCISED tries' 360 s.)

`results.tsv` is the runner's table (test, try, verdict, seconds, finished at, lane, LEZ chains
made so far by this run; the two tests on a chain of their own have no lane columns);
`regress.log` is the runner's log, every try's start and verdict line carrying the LEZ
finalized height and tip. `logs/` holds the failed test's log and the three reruns' runner
output (`reruns/`), as the runner printed them. None holds a local path. The other per-test
logs are not committed; the lines quoted below are from them.

## The failure: `node:btc:restart-maker:TakerSellsForeign`

Lane 1, 13:09:52 to 13:42:30. The Taker locked its Bitcoin (`bb79d9060287`), the harness
restarted `lez-maker-node`, the Maker locked its LEZ, and the Taker claimed the LEZ at 13:12:25.
The Maker then had to claim the Bitcoin with the secret the Taker's claim revealed. It never did;
the test timed out after 1,800 s:

    [13:12:25]   Taker claimed LEZ (replay=False)
    [13:13:26]   [60 s] waiting for the Taker's 990e4f748028 to reach completed (Maker follow-up claim): taker claim_in_progress (gen 3) | maker claim_evidence_available | …
    [13:42:27] FAILED: timed out after 1800 s waiting for the Taker's 990e4f748028 to reach completed (Maker follow-up claim); last seen: taker claim_in_progress (gen 3) | maker claim_evidence_available; …

What the Maker kept (its swap state, moved to the Node's reset archive when the harness reset
the Nodes after the test; read there, not committed: it holds the swap's keys):

- The Maker's actor supervisor ran the swap's Bitcoin actor 176 times between 13:10:27 and
  13:42:25 (`maker_actor_processes`: `attempt_count` 176, `schedule_state` leased, no failure
  class at the end). It was the only actor process on the Maker; nothing else held the Maker's
  one actor worker. Its last progress row: phase `claim_evidence_available`, revision 3,
  next action `observe_followup_claim`.
- The actor's public-effect journal holds one Bitcoin claim, transaction `ee99634688440cfe…`
  spending `bb79d9060287…:0`, in state `unknown` after **1** submission attempt. A claim in
  `unknown` is never sent again (the journal's rule); the actor only watches for it from then on,
  which is the 175 later runs.
- That claim never reached Bitcoin Core: Core's log for 13:10 to 13:43 has no block with a
  transaction after the Taker's lock (every template "txs: 0"), `getrawtransaction` does not
  know it, and the lock's output `bb79d9060287…:0` is still unspent. Its stored bytes are valid:
  `testmempoolaccept` on them, read afterwards, answers `allowed: true` (locktime 0, so it was
  valid at 13:12 as well).

The Node's and the actor's own log lines are lost: the Maker container was recreated by later
tests before the diagnosis, and its log goes with it. So which call inside the one submission
failed is not seen directly. What the record shows is the signature of a defect already found:
on 2026-11-13 `node:btc:offline-after-claim:TakerSellsForeign` failed the same way (the Maker's
claim journaled `unknown` after one attempt, never in the mempool, the bytes valid). Its
diagnosis: the Bitcoin Core adapter checks readiness (`ensure_ready`) inside the one broadcast,
after the send authority is spent, by comparing the chain height from `getblockchaininfo` with
the transaction index height from `getindexinfo`; at 2-second devnet blocks a block can land
between the two calls, and the error is recorded as a permanent `unknown`. The other ways to
`unknown` without a send (a transport failure of `testmempoolaccept` or `sendrawtransaction` to
the container's local Core) are possible but have no sign in Core's log. A fix that rechecks
readiness up to three times, 250 ms apart, before `testmempoolaccept`, with at most one
`sendrawtransaction` as before, is prepared (`fix(btc-core-adapter): recheck readiness before a
broadcast that straddles a block`) and not in this run: it changes the frozen Bitcoin engine and
waits for the owner. It is listed in the 2026-11-15 note's "Not covered".

The restart is not shown to be the cause: it was at 13:10:28, the Maker Node answered again by
13:11:15, and the claim attempt fell between 13:12:25 and 13:13:26, after the Maker had locked
its LEZ in the meantime. Nor is the Maker's one actor worker: the swap's actor was the only one
and ran on average every 11 s. It is not this week's changes:
nothing on the Bitcoin Maker's path changed between `305b32e0` and `39e60679` (no change in
`crates/btc-core-adapter`, `crates/btc-reference-actor`, `crates/swap-store` or the Maker's
actor supervisor; `node-entrypoint.sh` changed in the Monero role's configuration only). In the
twelve earlier runs of this test kept on the host since 2026-11-12 and in the committed runs
since 2026-11-11 it passed every time (63 to 149 s); its one earlier failure, 2026-11-01
([regression-m2-2026-11-02](../regression-m2-2026-11-02/RESULTS.txt)), was the LEZ finality
freeze, a different cause.

### Three reruns on the same images

`LEZ_MONERO=1 LEZ_LANES=3 scripts/regress.sh run node:btc:restart-maker:TakerSellsForeign`, three
times one after the other, same images, no rebuild, on lane 1, on the chain the run ended on
(the sequencer restarted at 15:03Z). All three passed on their first try, 0 frozen:

| run | seconds | Taker's LEZ claim | Taker reads `completed` (the Maker's Bitcoin claim seen) | both Nodes `completed` |
|---|---:|---|---|---|
| `20261116T150357Z` | 98 | 15:06:15 | 15:06:37 (+22 s) | 15:06:44 (+29 s) |
| `20261116T150645Z` | 112 | 15:09:14 | 15:09:42 (+28 s) | 15:09:46 (+32 s) |
| `20261116T150946Z` | 103 | 15:12:06 | 15:12:33 (+27 s) | 15:12:37 (+31 s) |

(The harness polls; the times are its first lines that saw each state. `logs/reruns/` holds each
rerun's runner log, table and test log.) Three passes fit a race that strikes now and then; they
do not show it is gone.

## The NOT-EXERCISED tries

`node:zec:zec-claim-collision:TakerSellsForeign` needs two Taker swaps whose LEZ transactions
collide on the owner account's nonce: A's revealing claim must lose its nonce to C's lock. On
tries 1 to 3 (lanes 3, 1 and 1; 119, 109 and 132 s) the race went the other way, C's lock lost
the nonce and was re-planned ("re-planned [1 lez_fund nonce 86→87 …]"), and the scenario said so
instead of passing:

    FAILED: NO-COLLISION: A completed without a superseded claim (C's lock was in a block before
    A's claim was planned, or C's lock lost the nonce instead: … try again

The runner recorded each NOT-EXERCISED and ran it again; try 4 (lane 1, 129 s) produced the
collision ("revealing LEZ claim superseded (planned nonce 105, live 106); re-plan 1 at nonce
106") and passed. The test cannot force its race; the 2026-11-14 and 2026-11-15 runs needed a
second try as well.

## The Monero tests

All 16 passed on their first try. All are TakerSellsLez (the only Monero direction), against
Monero regtest on the devnet (per lane a `monerod` with funding, Maker and Taker
`monero-wallet-rpc`), `fast` profile. Each swap's windows, as the logs print them: "the Maker
funds by +350 s, the Taker refunds from +790 s, the Maker punishes from +1090 s". Lot 1 XMR
against 50,000 LEZ units.

| test | lane | s | what its log shows |
|---|---|---:|---|
| `node:xmr:xmr-happy` | 2 | 239 | "settled completed: LEZ moved {'maker': 50000, 'taker': -50000} as both Nodes say"; the Taker's wallet received 0.9981808 XMR. |
| `node:xmr:xmr-taker-refund` | 3 | 1,000 | "settled refunded: LEZ moved {'maker': 0, 'taker': 0}"; the Maker's wallet swept its Monero back (0.9981844 XMR), the Taker's received nothing. |
| `node:xmr:xmr-maker-never-funds` | 2 | 886 | "settled refunded", "no Monero moved: the Maker's wallet sent nothing, the Taker's received nothing". |
| `node:xmr:xmr-punish` | 3 | 1,206 | "the Maker punished; its LEZ owner's nonce moved 3 → 4"; neither wallet received Monero, the funding "stays at the shared address: neither party received it". |
| `node:xmr:xmr-concurrent` | 2 | 350 | "settled both: LEZ moved {'maker': 100000, 'taker': -100000}", the Taker's wallet received two outputs. |
| `node:xmr:xmr-kill-taker` | 1 | 1,970 | "taker killed at 7 points: after the take → completed; lock in flight → completed; locked → completed; Monero verified → completed; claim authorised → completed; waiting for the Maker's claim → completed; sweeping → completed". |
| `node:xmr:xmr-kill-maker` | 2 | 2,014 | "maker killed at 6 points: after the take → completed; lock observed → completed; funding in flight → completed; funded → completed; claim in flight → completed; claim tool running → completed". The claim tool is held on the Maker container's own sequencer forwarder (`97605261`), not by pausing the shared sequencer; no try froze. |
| `node:xmr:xmr-taker-refund-kill` | 1 | 993 | The Taker killed right after its refund's admission; "settled refunded", the Maker's wallet swept its Monero back. |
| `node:xmr:xmr-monerod-down` | 2 | 604 | Before a take "the Maker withdrew offer … and refuses to publish on Monero: chain dependency is unavailable for route"; mid-swap "monerod was down 95 s (the funding cutoff is 253 s away)"; both swaps completed. |
| `node:xmr:xmr-wallet-restart` | 2 | 507 | Both swaps "settled completed" with a role's `monero-wallet-rpc` restarted mid-swap. |
| `node:xmr:xmr-outage-before-take` | 1 | 262 | "the Taker refused the take: Taker dependency unavailable (authenticated_delivery_unavailable)"; restored, the swap completed. |
| `node:xmr:xmr-cut-after-lock` | 3 | 282 | "the cut held: 73 samples of the Maker's Chat socket, none with a connection"; "settled completed". |
| `node:xmr:mixed-three-pair` | 2 | 388 | A Monero, a Bitcoin and a Zcash swap of the same Nodes at once: "all three completed; LEZ transactions signed by the owner accounts meanwhile: {'maker': 1, 'taker': 7}". |
| `ui:xmr:xmr-happy` | 2 | 266 | The Monero swap clicked through both desks, as in the recorded run. |
| `ui:xmr:xmr-taker-refund` | 3 | 1,058 | The Taker desk's refund journey, as in the recorded run. In a full run for the first time. |
| `ui:xmr:xmr-concurrent` | 1 | 470 | Two Monero swaps at once on both desks, as in the recorded run. In a full run for the first time. |

## What changed since the 2026-11-15 run

The branch was rebased after the 2026-11-15 run: `305b32e0` is `b660dcc6` in this history (the
same `crates`, `apps`, `modules`, `deploy` and `packaging` trees; `compat` differs by the Zcash
escrow cycle measurement that came in from `feat/m5-gaps` with `df2db648`, `e65626bc` and
`e3b8faed`, test and docs only). `git log b660dcc6..39e60679` holds 23 commits:

- **Monero fixes** (from the desk runs of 2026-11-16, described in
  [regression-m4-desks-2026-11-16](../regression-m4-desks-2026-11-16/)): the verifier's block
  read and the topology reads made once more (`a1c0b915`); the Monero crypto kernels optimized
  in the sidecar's dev build (`ebf98df5`); the Monero funding cutoff on `fast` moved to +350 s,
  R and P with it, Bitcoin and Zcash values unchanged (`fede8171`); a Monero take's Chat round
  sent once more when the Maker does not answer in time (`363df3cf`).
- **Monero Maker speed**: the Monero actor validates Stage A once per process (`495d755c`),
  starts the search for a swap's LEZ lock at its activation (`ecfe1400`), scans at most 128
  finalized LEZ blocks per drive (`7b6261b0`), and opens the role wallet once for a funding
  (`39e60679`).
- **Tests**: `xmr-kill-maker` holds the claim tool on the Maker's own sequencer forwarder instead
  of pausing the shared sequencer (`97605261`); the desk journeys `ui:xmr:xmr-taker-refund` and
  `ui:xmr:xmr-concurrent`, now in the full tier (`20f8972a`), and their captions (`05e46694`);
  executor cycles of the Monero escrow instructions (`f1ce4ebf`); the systemd VM run with a
  Monero role (`244a0089`).
- **Packaging**: the Maker's systemd unit can take a Monero role (`8f9788fa`), and a Maker built
  without Monero installs as before (`b10f32bb`).
- **Docs**: the 2026-11-15 evidence and its follow-up (`00d71155`, `639e4497`, `d32ba728`), the
  Monero compute units (`abbf41ac`), the install guide's Monero role (`fdb2d8d8`), the desk
  recordings (`a10dff02`, `044e42b9`), the clocks that gate the Maker's Monero funding
  (`737468e9`).

None of them changes a Bitcoin or Zcash value or code path; `node-entrypoint.sh`'s two changes
are the Monero logins' directory for systemd and a refusal of a Monero role in a package without
the Monero programs.

## Source and images

Tests and images from `39e60679` (branch `feat/m4-monero`), tree clean (`regress.log` prints the
commit and "tree dirty: 0"). Image ids, creation times and the build-source check are in
[PROVENANCE.md](PROVENANCE.md).

## Not covered

- Monero beyond an offline regtest chain: no stagenet or mainnet; x86_64 (linux/amd64) only.
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
- Two Bitcoin-adjacent defects have fixes prepared but not landed, so they are not in this run:
  the Maker's Bitcoin claim journaled `unknown` before its broadcast (this run's failure, and
  `node:btc:offline-after-claim:TakerSellsForeign` on 2026-11-13), and the Maker's check of its
  Bitcoin actor's state database by size and times (`actor_deployment_invalid`,
  `ui:btc:happy:TakerSellsLez`, 2026-11-13). The second did not recur here, which does not show
  the race is gone.
- macOS, linux/arm64, and any public network are untested on this head.
