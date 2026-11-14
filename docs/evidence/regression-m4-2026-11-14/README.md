# Full regression of `feat/m4-monero`, 2026-11-14

One run of every test of `deploy/scripts/regress.sh full` (152: `regress.sh list full`) on the
Docker devnet with the Monero regtest chain up (`LEZ_MONERO=1`), three lanes at a time
(`LEZ_LANES=3`), on a chain made for the run, without desk recordings. 2026-11-14 13:40:16Z to
18:15:28Z (UTC). It is the first full run that includes Monero: the 143 Bitcoin and Zcash tests
of the [previous full run](../regression-m5-2026-11-12/) plus nine Monero tests (eight
`node:xmr` scenarios and `ui:xmr:xmr-happy`).

**Result: 152 of 152 passed in one run of 275 minutes.** 151 tests passed on their first try;
one, `node:zec:zec-claim-collision:TakerSellsForeign`, was NOT-EXERCISED on its first try and
passed on its second (below). 0 timeouts, 0 frozen tries; 2 new LEZ chains (the two
`zec-balance-refusal` tests make their own); 3 timing-profile switches. By kind: node:btc 53,
node:zec 51, node:xmr 8, ui:btc 17, ui:zec 22, ui:xmr 1; test time summed over lanes 41,597 s
over 153 tries (41,440 s for the passing tries), of which the nine Monero tests 7,578 s.

`results.tsv` is the runner's table (test, try, verdict, seconds, finished at, lane, new chain;
the two tests on a chain of their own have no lane columns); `regress.log` is the runner's log,
every try's start and verdict line carrying the LEZ finalized height and tip. Neither holds a
local path. The per-test logs are not committed; the lines quoted below are from them.

## The one retried test

`node:zec:zec-claim-collision:TakerSellsForeign` needs two Taker swaps whose LEZ transactions
collide on the owner account's nonce. On try 1 (lane 1, 157 s) the collision did not happen and
the scenario said so instead of passing:

    FAILED: NO-COLLISION: A completed without a superseded claim (C's lock was in a block before
    A's claim was planned, or C's lock lost the nonce instead: … try again

The runner recorded it NOT-EXERCISED and ran it again; try 2 (lane 1, 130 s) produced the
collision and passed. This did not occur in the 143-test run, where every test passed on its
first try.

## The Monero tests

All nine are TakerSellsLez (the only Monero direction), against Monero regtest on the devnet
(`lez-monerod`, with funding, Maker and Taker `monero-wallet-rpc`), `fast` profile. Each swap's
windows: the Maker funds by +260 s, the Taker refunds from +360 s, the Maker punishes from
+480 s. Lot 1 XMR against 50,000 LEZ units.

| test | lane | s | what its log shows |
|---|---|---|---|
| `node:xmr:xmr-happy` | 1 | 281 | One swap through both Nodes: "settled completed: LEZ moved {'maker': 50000, 'taker': -50000} as both Nodes say"; the Taker's wallet received 0.998188 XMR (the 1 XMR lot less fees). |
| `node:xmr:xmr-taker-refund` | 1 | 499 | The Maker funds, the Taker does not claim: "Taker refund admitted", "settled refunded: LEZ moved {'maker': 0, 'taker': 0}"; the Taker's LEZ came back and the Maker's wallet swept its Monero back (received 0.9981856 XMR), the Taker's received nothing. |
| `node:xmr:xmr-maker-never-funds` | 2 | 442 | The Maker Node is stopped before its funding; "Taker refund admitted with the Maker down", LEZ back to the Taker, "no Monero moved: the Maker's wallet sent nothing, the Taker's received nothing". |
| `node:xmr:xmr-punish` | 3 | 631 | The Taker Node is stopped after verifying the Maker's Monero and neither claims nor refunds. "the Maker's LEZ owner 9DaY4JJ1ZjXd… has nonce 13 before the swap", then "the Maker punished; its LEZ owner's nonce moved 13 → 14"; LEZ moved {'maker': 50000, 'taker': -50000}; neither wallet received Monero, the funding "stays at the shared address". The Taker, back up, reads `attention_required` (`xmr_refund_window_closed`, actor `punish_open`). |
| `node:xmr:xmr-concurrent` | 1 | 440 | Two offers taken one after the other, swaps `b994e201a484` and `ac9eb059ca73` live together; both completed, "settled both: LEZ moved {'maker': 100000, 'taker': -100000}", the Taker's wallet received two outputs of 0.998 XMR. |
| `node:xmr:xmr-kill-taker` | 1 | 2,248 | The Taker Node SIGKILLed at 7 points, a fresh swap each: "taker killed at 7 points: after the take → completed; lock in flight → completed; locked → completed; Monero verified → completed; claim authorised → completed; waiting for the Maker's claim → completed; sweeping → completed". Each swap: Taker 3 LEZ transactions, one Maker funding, one Taker sweep. |
| `node:xmr:xmr-kill-maker` | 2 | 2,280 | The Maker Node SIGKILLed at 6 points: "maker killed at 6 points: after the take → completed; lock observed → completed; funding in flight → refunded; funded → completed; claim in flight → completed; claim tool running → completed". At "funding in flight" the Maker's funding was never sent, the Maker read `maker_funding_in_doubt` and the Taker refunded at its window. At "claim tool running" the Maker's tag-15 claim tool held its reserved evidence file on the paused sequencer when killed; "the Maker drove its stopped claim again and recorded its one tag 15". One take was answered `initiation_execution_unavailable` and succeeded on its retry 10 s later. |
| `node:xmr:xmr-taker-refund-kill` | 2 | 481 | The Taker Node SIGKILLed right after its refund was admitted; back up at `refund_in_flight`, the refund settled ("LEZ moved {'maker': 0, 'taker': 0}"), the Maker's wallet swept its Monero back (0.9981916 XMR). |
| `ui:xmr:xmr-happy` | 3 | 276 | A Monero swap clicked through both desks: the Maker desk publishes Monero offers from its form; the Taker desk takes one (the Taker Node reserves, plans, signs and activates); the Taker Node locks its LEZ while the Maker desk shows the row funded; the Taker desk performs `claim_xmr`; the Maker Node claims the LEZ and the Taker Node sweeps the Monero; both desks show the swap `completed`. Its evidence export is skipped ("a Monero swap's record is node-e2e.py's"). |

## How the Zcash agreements were made

Of the 73 Zcash tests, 69 formed their agreement at take time (`zec_reserve_v1`: the Maker
publishes, the Taker takes, the two Nodes compose and sign it, each with its own key; their logs
say "take-time agreement"), which is the harness default on this branch
(`LEZ_ZEC_AGREEMENT=take-time`); both tries of `zec-claim-collision:TakerSellsForeign` did. Four
used the test fixture `deploy/tests/fixtures/zec-catalog-fixture.sh`, which holds both parties'
keys: `zec-regenerated-catalog` and `zec-tampered-catalog` in both directions, because they test
the prepared catalog itself (their logs say "take-time agreement not used here").

## Source and images

Tests and images from `82e1b856` (branch `feat/m4-monero`: 88 commits on `feat/m5-gaps` at
`88d00005`, which holds the previous run's `dcbdb4f2`), tree clean (`regress.log` prints the
commit and "tree dirty: 0"). Trees at that commit (`git rev-parse 82e1b856:<directory>`):

| directory | tree |
|---|---|
| `crates` | `c5c0ae6f825cc6d8330d20cc22a5f393b623000d` |
| `compat` | `af1f42a350f4bafda88ec2d211149c868c5543fb` |
| `apps` | `c434c3d87ceaa23293012962d5d7ffe794590e65` |
| `modules` | `b67910671ce3d5215fef93af2eb6a313b3d2e338` |
| `deploy` | `9dd2487f01e7ddef08193f3db2afa961dbbd5329` |
| `packaging` | `9e605b7dfb43be866bf8c4f855a4f7469811c871` |

Built just before the run (the `maker-node`, `taker-node`, `basecamp-ui` and `network` images
at that commit) and not rebuilt during it; local linux/amd64 ids, with each image's creation
time (a build that changes nothing keeps the earlier image):

| image | id | created |
|---|---|---|
| `lez-maker-node:local` | `sha256:ae59e0b4788af6d28fa62e04f091531ab39711962fcc7726d1f367f808740231` | 2026-11-14 12:35Z |
| `lez-taker-node:local` | `sha256:7d1d95e013befd52633047c469e05956f6802d29d4bc764524bc558e2afb0ce8` | 2026-11-14 12:35Z |
| `lez-basecamp-ui:local` | `sha256:c603ff98d621aa724e74ad19a878e1c56e3e08920bccb360718248b338caa893` | 2026-11-14 11:55Z |
| `lez-network-module:local` | `sha256:b57cdf5f7326e0b01650179b4b14a47999e178614ff20998dabda99760c6492e` | 2026-11-14 13:37Z |
| `lez-services:local` | `sha256:021a7928e64602ce785ce464a83df7e9c27588391c24f6fd3d048489f7d52bef` | 2026-11-13 18:33Z |
| `lez-zebra:local` | `sha256:42d64e9a9e85dbd24911f26b2676f4ca07f3c7d910e80dfb99dfb4ca49ecc101` | 2026-11-11 11:50Z |
| `lez-bitcoin-core:local` | `sha256:dadb2fd79b64a6e20612440c90146d6501a1b3d2039408760e93d48c33619cc5` | 2026-11-11 15:01Z |
| `lez-btc-miner:local` | `sha256:4a33449ce116b239641c7932c63845e2d7c9a54e0d00f39d19688e7c3e403473` | 2026-11-11 15:01Z |
| `lez-monerod:local` | `sha256:db4f4b5620a34a5634bd31d4ab9fc8f92b3ce00139f586828287f41e3ce1d9cf` | 2026-11-13 15:13Z |

`lez-services` and `lez-network-module` carry the same write-ahead-log bound patches as in the
previous run (`deploy/builder/patches/README.md`); `lez-services` is the same image.

## Known defects

The two rare Bitcoin-path races described in the [previous run's note](../regression-m5-2026-11-12/README.md#earlier-run-and-two-known-defects)
(a Maker Bitcoin claim journaled as sent and never broadcast in
`node:btc:offline-after-claim:TakerSellsForeign`; `actor_deployment_invalid` in
`ui:btc:happy:TakerSellsLez`) still have no fix in this commit. Both tests passed in this run,
which does not show that the races are gone.

## Not covered

- Monero beyond an offline regtest chain: no stagenet or mainnet; x86_64 (linux/amd64) only.
- Monero outages: no scenario with `monerod` down or a `monero-wallet-rpc` restarted, and no
  three-pair mixed test (Bitcoin, Zcash and Monero swaps at once).
- On a real desk: only the Monero happy journey is clicked through; the refund, punish and
  in-doubt rows were not seen on a desk.
- No systemd packaging, macOS package or desk recordings for Monero.
- Compute units for the five Monero escrow instructions (tags 13–17) were not measured.
- The Maker's funding retry (a funding tool failing before its transfer) was not seen live.
  `maker_funding_in_doubt` was reached once, in `xmr-kill-maker` at "funding in flight", with
  the funding never sent; an in-doubt funding that did reach the chain was not seen.
- Two fixes written after this run are not in it: the Monero route health asking `monerod` only,
  not the role's busy wallet-rpc (`bfb17a0c`), and the Maker-kill scenario counting fundings per
  swap, seeing and mining a funding its kill left in the pool (`2ca0d8b9`). At the time of
  writing both are on the branch `agent/m4-diag2`, being tested, not landed.
- Two hosts: the Logos-transport tests (`logos-happy`, Bitcoin and Zcash) run both Basecamps on
  one host with a private Delivery network; the public `logos.test` fleet did not deliver Chat
  reliably when measured (`deploy/README.md`, "Two transports").
- A swap under the systemd units is a separate run (`docs/evidence/maker-systemd-2026-11-12/`).
- macOS, and the native-Node harness with the take-time paths, are untested on this head.
- Any public network, linux/arm64.
