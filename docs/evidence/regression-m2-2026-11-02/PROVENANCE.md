# Provenance — M2 final regression (V6) on the Docker devnet, 2026-10-31 → 2026-11-02

Everything in this folder was produced on one box by the Docker devnet of
`deploy/` (local profile): the Node scenarios by `deploy/scripts/node-e2e.py`,
the desk journeys by `deploy/scripts/ui-e2e.sh` (Basecamp desks driven through
`verify.mjs`), the runners by two scratch scripts whose logs are
`logs/final-v6.log` and `logs/rerun-v6.log`.

Nothing here is private. The ZEC exports carry swap views, durable actor
status, zebra heights and transaction ids; the BTC summaries carry timing,
swap ids and claim transaction ids; the logs carry step lines and captions.
No key, cookie, capability file, `runtime/` or `market/` content is included.
Local paths in the logs are replaced by `<checkout>`, `<work>` and `<scratch>`.
The scan used before commit: key, secret, cookie, password, token, preimage
and key-encoding patterns over every file.

## Source

All tests ran on images built from `b1dd0af8` (branch
`feat/m2-zec-on-docker-signed`, PR #119), tree clean. The commits since the
previous evidence set (`regression-m2-2026-10-29`, at `b978e406`):

| commit | what |
|---|---|
| `64aa1df8` | feat(deploy): gen-config mints each role's Zcash key; zebra mines to the Maker's, which is its Delivery identity |
| `204909a5` | fix(basecamp): the ZEC Taker rows say what happens in each direction; the claim text no longer describes the Bitcoin adaptor flow |
| `5deb2512` | fix(deploy): gen-config removes its private key copy on every exit; the README names the 0644 host-copy caveat |
| `512d892b` | fix(basecamp): the ZEC waiting-for-the-Maker's-lock sentence is a QString before it is joined |
| `7d9e8392` | fix(deploy): the heartbeat service is in the runtime profile's component catalog |
| `2a57a883` | fix(deploy): gen-config derives the miner address from an absolute key path |
| `07495d6d` | feat(deploy): the ZEC lane runs in both directions — node-e2e.py, the desk journeys and the provisioner take --direction |
| `60264d60` | fix(zec): the devnet provisioner funds from any mature output of the funder's key, not from coinbase alone |
| `a93494e8` | fix(deploy): the ZEC harness reads the funding outpoint from whichever role funds Zcash |
| `3f8bcb48` | fix(maker-node): a Zcash drive that projects the Taker's revealing LEZ claim is a valid output (TakerSellsForeign) |
| `f9b8b3ad` | feat(deploy): zec-balance-refusal in TakerSellsForeign — the Maker is the short LEZ depositor |
| `c27d4271` | fix(taker-node): selling ZEC, the Taker keeps driving after its revealing LEZ claim until it observes the Maker's Zcash claim |
| `b27adb33` | docs(taker-node): the ZEC supervisor's design note says an open admission is read before the phase table |
| `75a057b3` | fix(deploy): selling ZEC, the harness mines zebra for the Taker's funding only up to the pre-reveal depth |
| `6d9588c0` | fix(deploy): a desk press counts only when the desk shows a reply; ui-e2e.sh keeps the --direction it was given for Zcash |
| `0d157589` | fix(taker-node): selling ZEC, a Taker that does not claim can refund — the claim window admits a refund on the Zcash route |
| `104da65a` | fix(basecamp,deploy): the Taker desk says when it does not send a press; the driver waits for the Node's reply and marks every re-press |
| `8de775ae` | fix(taker-node): selling ZEC, one revealing LEZ claim in flight per Node — a second waits until the first is final |
| `7f551c94` | fix(deploy): the desk driver runs Zcash journeys in both directions |
| `05093f97` | docs(api): pressing Refund in the ZEC claim window waits for the height and gives up the claim |
| `5b769bcd` | feat(sidecar): a revealing claim whose nonce was taken is named Superseded and re-planned at the live nonce |
| `9c318986` | fix(basecamp): two Zcash swaps no longer share the Taker desk's action request id |
| `f8dbd75f` | feat(zec): a superseded revealing claim is re-planned, restaged with its lineage and margin, and resubmitted in the same attempt |
| `297531d7` | fix(nodes): no refund once a revealing claim was submitted; one revealing LEZ claim in flight per Node until it is final, on both Nodes |
| `f6aaa864` | fix(nodes): a revealing claim held near its LEZ refund time names its holder; the Maker ZEC process tests pass again |
| `ccb934f8` | fix(deploy): selling LEZ, node-e2e.py's ZEC flow text no longer calls itself |
| `fc5a7312` | feat(deploy): zec-claim-collision — a revealing LEZ claim superseded by the same account's other LEZ transaction is re-planned and lands, live |
| `083eb750` | feat(deploy): BTC/ZEC parity — three ZEC Node scenarios and six ZEC desk journeys, both directions |
| `396d58c4` | feat(zec): DeterministicLocalDevnetV3 — the devnet profile with a 12-minute LEZ refund delay; a held revealing claim needs attention a third of the delay before its refund time |
| `d637fab4` | fix(deploy): the desk driver never presses again while the desk has a request in flight, and every re-press reaches the log |
| `09222177` | fix(deploy): zec-refund-then-maker-lock asks zebra's blocks whether the Maker's funding is in, not gettxout |
| `7ad25f2c` | fix(deploy): the lone-swap lock-attempt check counts each submitted operation once |
| `129c8bb3` | fix(zec): a revealing LEZ claim is never sent after its window closes; the claimant's row offers its refund instead |
| `955ea518` | feat(zec): selling ZEC, the row after the claim window reads "Claim window closed"; the Taker view names it; drive_claim takes a typed trusted clock |
| `47782164` | fix(zec): a recovery never ends while the party's own submitted lock is unsettled; a lock that confirms late is projected and refunded |
| `b1dd0af8` | fix(zec): selling ZEC, a refund pressed after the Maker refunded reads "refund in progress", and that row names the Maker's refund |

Findings closed in this range (the review's FINDINGS list): 24 (a superseded
revealing claim is re-planned at the live nonce), 25, 26 (no revealing LEZ claim
after its window; the claimant refunds instead) and 27 (a recovery waits for the
party's own submitted lock to settle; a lock that confirms late is projected and
refunded). M3 BTC code is unchanged in this range (scope frozen by the owner);
the commits that touch code BTC shares (Nodes, desks, harness) are listed
for the owner separately.

## Images

Built once from `b1dd0af8` (2026-10-31 19:41–19:45Z: nix, rust, stage, compose)
and not rebuilt during V6 or the reruns. Each Node image carries its build
source in `/usr/local/share/lez/build-source.txt` (`b1dd0af871eedfaa9598118ed55fca11c42ad57f`);
both runners print it and the image id at start.

| image | id |
|---|---|
| `lez-taker-node:local` | `sha256:09d147a3cb99b3bd80eddea8bcd4dc63c1f5fd76cebab93af87d717300919284` |
| `lez-maker-node:local` | `sha256:129b81dbf912804af51604c15a1ea5b5a084f9725c71957cb2ff7cc8a01803ac` |
| `lez-basecamp-ui:local` | `sha256:e25692af641b4cefbdb6280ec1398e6ce5566d5597dcce4ed95f8a93c966fb65` |
| `lez-tools:local` | `sha256:91648ea2b341419a34f565beacb73d54b08180d8c0face65cc2b5a94d4c97960` |

## Profiles

ZEC: `deterministic-local-devnet-v3` (wire tag 4): 720 s LEZ refund delay, so a
refund scenario fits inside a chain's healthy window; the held-claim attention
threshold is a third of the delay (240 s). BTC: the fast timing profile (values
in each `e2e/*.json` under `timing`).

## Chains

`up.sh --fresh-lez` made every chain; a heartbeat service (one transfer every
20 s, `runtime/heartbeat/status.json`) measured finality. The finality freeze
(upstream #3643: blocks and inclusion continue, the finalized height stops)
hit 20–40 min after a chain came up. The V6 runner gave each ZEC refund
scenario its own fresh chain and recreated whenever finality was flat before a
step; the reruns gave every failure its own fresh chain, up to three tries, a
retry only when finality was flat after the failure, and restarted zebra
before each ZEC journey (V6's zebra had filled its mined-block gossip channel
after 46 h). Chain recreations and what each carried:

final-v6.log:

| chain | up | carried |
|---|---|---|
| 400 | 2026-10-31 19:58Z (attempt 1) | rec zec-happy ok |
| 401 | 2026-10-31 20:07Z (attempt 1) | rec zec-maker-refund ok |
| 402 | 2026-10-31 20:27Z (attempt 1) | rec zec-concurrent ok |
| 403 | 2026-10-31 20:50Z (attempt 1) | zec-balance-refusal (TSL) ok |
| 404 | 2026-10-31 21:38Z (attempt 4) | zec-balance-refusal (TSF) ok |
| 405 | 2026-10-31 21:46Z (attempt 1) | zec-taker-refund (TSL) ok |
| 406 | 2026-10-31 22:23Z (attempt 1) | zec-maker-refund (TSL) ok |
| 407 | 2026-10-31 22:49Z (attempt 1) | zec-refund-then-maker-lock (TSL) ok, zec-happy (TSL) ok, zec-replay (TSL) ok, zec-wrong-inputs (TSL) ok, zec-restart-taker (TSL) ok |
| 408 | 2026-10-31 23:25Z (attempt 1) | zec-restart-maker (TSL) ok, zec-survivor (TSL) ok, zec-concurrent (TSL) ok, zec-regenerated-catalog (TSL) ok, zec-tampered-catalog (TSL) ok, zec-early-lock (TSL) ok |
| 409 | 2026-11-01 00:12Z (attempt 1) | zec-offline-after-lock (TSL) ok |
| 410 | 2026-11-01 00:19Z (attempt 1) | zec-taker-refund (TSF) ok |
| 411 | 2026-11-01 00:39Z (attempt 1) | zec-maker-refund (TSF) ok |
| 412 | 2026-11-01 01:05Z (attempt 1) | zec-refund-then-maker-lock (TSF) ok, zec-happy (TSF) ok, zec-replay (TSF) ok, zec-wrong-inputs (TSF) ok, zec-restart-taker (TSF) ok, zec-restart-maker (TSF) ok, zec-survivor (TSF) ok |
| 413 | 2026-11-01 02:13Z (attempt 2) | zec-concurrent (TSF) ok, zec-regenerated-catalog (TSF) ok, zec-tampered-catalog (TSF) ok, zec-early-lock (TSF) ok, zec-offline-after-lock (TSF) ok |
| 414 | 2026-11-01 02:52Z (attempt 1) | zec-claim-collision (TSF) ok |
| 415 | 2026-11-01 03:08Z (attempt 1) | ui zec-taker-refund (TSL) ok, ui zec-concurrent-staggered (TSL) ok, ui zec-restart-taker (TSL) ok, ui zec-restart-maker (TSL) ok, ui zec-survivor (TSL) ok |
| 416 | 2026-11-01 04:03Z (attempt 1) | ui zec-node-offline (TSL) ok |
| 417 | 2026-11-01 04:14Z (attempt 1) | ui zec-taker-refund (TSF) ok |
| 418 | 2026-11-01 04:41Z (attempt 1) | ui zec-maker-refund (TSF) ok |
| 419 | 2026-11-01 05:16Z (attempt 2) | ui zec-stale-window (TSF) ok, ui zec-happy (TSF) ok, ui zec-concurrent (TSF) ok |
| 420 | 2026-11-01 05:50Z (attempt 1) | ui zec-restart-taker (TSF) FAIL, ui zec-restart-maker (TSF) FAIL, ui zec-survivor (TSF) ok, ui zec-node-offline (TSF) ok |
| 421 | 2026-11-01 06:32Z (attempt 2) | btc happy (TSF) ok, btc replay (TSF) ok, btc wrong-inputs (TSF) ok, btc restart-taker (TSF) ok, btc restart-maker (TSF) FAIL |
| 422 | 2026-11-01 07:39Z (attempt 1) | btc survivor (TSF) ok, btc concurrent (TSF) ok, btc early-lock (TSF) ok, btc offline-after-lock (TSF) FAIL |
| 423 | 2026-11-01 08:52Z (attempt 1) | btc taker-refund (TSF) FAIL |
| 424 | 2026-11-01 09:35Z (attempt 1) | btc maker-refund (TSF) ok, btc refund-then-maker-lock (TSF) ok, btc regenerated-config (TSF) FAIL |
| 425 | 2026-11-01 11:32Z (attempt 2) | btc tampered-config (TSF) ok |
| 426 | 2026-11-01 11:37Z (attempt 1) | btc happy (TSL) ok, btc replay (TSL) ok, btc wrong-inputs (TSL) ok, btc restart-taker (TSL) ok, btc restart-maker (TSL) ok, btc survivor (TSL) ok, btc concurrent (TSL) ok, btc early-lock (TSL) FAIL |
| 427 | 2026-11-01 13:07Z (attempt 1) | btc offline-after-lock (TSL) ok, btc taker-refund (TSL) FAIL |
| 428 | 2026-11-01 14:15Z (attempt 1) | btc maker-refund (TSL) FAIL |
| 429 | 2026-11-01 15:26Z (attempt 1) | btc refund-then-maker-lock (TSL) ok, btc regenerated-config (TSL) ok, btc tampered-config (TSL) ok |
| 430 | 2026-11-01 16:10Z (attempt 1) | ui happy (TSF) ok |
| 431 | 2026-11-01 16:35Z (attempt 1) | ui restart-taker (TSF) ok, ui restart-maker (TSF) ok, ui survivor (TSF) FAIL |
| 432 | 2026-11-01 17:35Z (attempt 1) | ui concurrent (TSF) ok, ui taker-refund (TSF) FAIL |
| 433 | 2026-11-01 18:40Z (attempt 1) | ui maker-refund (TSF) ok |
| 434 | 2026-11-01 19:25Z (attempt 1) | ui node-offline (TSF) ok |
| 435 | 2026-11-01 19:33Z (attempt 1) | ui happy (TSL) ok, ui restart-taker (TSL) FAIL |
| 436 | 2026-11-01 20:26Z (attempt 1) | ui restart-maker (TSL) ok, ui survivor (TSL) ok |
| 437 | 2026-11-01 20:53Z (attempt 1) | ui concurrent (TSL) ok, ui taker-refund (TSL) FAIL |
| 438 | 2026-11-01 21:54Z (attempt 1) | ui maker-refund (TSL) ok, ui node-offline (TSL) ok |

rerun-v6.log:

| chain | up | carried |
|---|---|---|
| 440 | 2026-11-02 10:03Z (attempt 1) | btc taker-refund (TSF) ok |
| 441 | 2026-11-02 10:31Z (attempt 1) | btc taker-refund (TSL) ok |
| 442 | 2026-11-02 11:11Z (attempt 1) | btc maker-refund (TSL) ok |
| 443 | 2026-11-02 11:50Z (attempt 1) | ui taker-refund (TSF) ok |
| 444 | 2026-11-02 12:27Z (attempt 1) | ui taker-refund (TSL) FAIL |
| 445 | 2026-11-02 13:14Z (attempt 1) | ui taker-refund (TSL) ok |
| 446 | 2026-11-02 14:06Z (attempt 1) | btc restart-maker (TSF) ok |
| 447 | 2026-11-02 14:25Z (attempt 1) | btc offline-after-lock (TSF) ok |
| 448 | 2026-11-02 14:35Z (attempt 1) | btc regenerated-config (TSF) ok |
| 449 | 2026-11-02 14:45Z (attempt 1) | btc early-lock (TSL) ok |
| 450 | 2026-11-02 14:56Z (attempt 1) | ui survivor (TSF) ok |
| 451 | 2026-11-02 15:17Z (attempt 1) | ui restart-taker (TSL) ok |
| 452 | 2026-11-02 15:28Z (attempt 1) | ui zec-restart-taker (TSF) ok |
| 453 | 2026-11-02 15:37Z (attempt 1) | ui zec-restart-maker (TSF) ok |

(TSF = TakerSellsForeign, the Taker sells BTC or ZEC; TSL = TakerSellsLez.)

## V7: the desk copy fix, on new images

The owner asked (2026-11-03) for the selling-ZEC desk copy seen in the first selling-ZEC
recordings to be fixed on this branch. Commits after `b1dd0af8` that V7 ran on:

| commit | what |
|---|---|
| `31df3352` | fix(basecamp): selling ZEC, the Taker row offers nothing while its revealing LEZ claim lands; its refund rows no longer wait for the Maker's LEZ refund (Zcash selling-ZEC rows only; BTC and selling-LEZ rows byte-identical) |
| `ff958ec8`, `6574eeea`, `055bb204`, `b7251402` | CI: the Zebra regtest gate and the ZEC test runs from PR #116; the Zebra test image's base pin (CI and test files only) |
| `784ee009` | docs |
| `2f69932a` | fix(deploy): the desk driver's Zcash refund caption no longer says the Maker never locked (harness text, mounted read-only into the desk container; applied from the zec-stale-window recording on, and the zec-maker-refund recording was made again after it) |

Images, built once from `31df3352` (2026-11-03 17:19–17:28Z) and not rebuilt during V7:

| image | id |
|---|---|
| `lez-taker-node:local` | `sha256:4c590a795fa6…` (build-source `31df3352cdba`) |
| `lez-maker-node:local` | `sha256:fdf9b0169cab…` (build-source `31df3352cdba`) |
| `lez-basecamp-ui:local` | `sha256:56ddd0d0e6ed…` (the desk plugin carries the new row text) |

The Rust sources are the same as at `b1dd0af8`; the Node images differ only in their
build-source stamp. V7 ran the selling-ZEC desk journeys (the four recordings among them)
and all 16 BTC desk journeys, because the change is in the shared Taker desk. Its chains:

final-v7.log:

| chain | up | carried |
|---|---|---|
| 458 | 2026-11-03 17:35Z (attempt 1) | rec zec-happy (TSF) ok |
| 459 | 2026-11-03 17:50Z (attempt 1) | rec zec-maker-refund (TSF) ok |
| 460 | 2026-11-03 18:14Z (attempt 1) | rec zec-concurrent (TSF) ok |
| 461 | 2026-11-03 18:29Z (attempt 1) | rec zec-stale-window (TSF) ok |
| 462 | 2026-11-03 18:59Z (attempt 1) | ui zec-taker-refund (TSF) ok, ui zec-restart-taker (TSF) ok, ui zec-restart-maker (TSF) ok, ui zec-survivor (TSF) ok, ui zec-node-offline (TSF) ok |
| 463 | 2026-11-03 19:50Z (attempt 1) | ui taker-refund (TSF) ok |
| 464 | 2026-11-03 20:26Z (attempt 1) | ui maker-refund (TSF) ok |
| 465 | 2026-11-03 21:13Z (attempt 1) | ui happy (TSF) ok, ui restart-taker (TSF) ok, ui restart-maker (TSF) ok, ui survivor (TSF) ok, ui concurrent (TSF) ok, ui node-offline (TSF) ok |
| 466 | 2026-11-03 22:20Z (attempt 1) | ui taker-refund (TSL) FAIL |
| 467 | 2026-11-03 23:06Z (attempt 1) | ui taker-refund (TSL) FAIL |
| 468 | 2026-11-03 23:53Z (attempt 1) | ui taker-refund (TSL) FAIL |
| 469 | 2026-11-04 00:40Z (attempt 1) | ui maker-refund (TSL) FAIL |
| 470 | 2026-11-04 01:51Z (attempt 1) | ui maker-refund (TSL) FAIL |
| 471 | 2026-11-04 02:46Z (attempt 1) | ui maker-refund (TSL) ok, ui happy (TSL) ok, ui restart-taker (TSL) FAIL |
| 472 | 2026-11-04 04:02Z (attempt 1) | ui restart-taker (TSL) ok, ui restart-maker (TSL) ok, ui survivor (TSL) FAIL |
| 473 | 2026-11-04 04:57Z (attempt 1) | ui survivor (TSL) ok, ui concurrent (TSL) ok, ui node-offline (TSL) ok |

v7-rerec.log:

| chain | up | carried |
|---|---|---|
| 500 | 2026-11-04 05:27Z (attempt 1) | rec zec-maker-refund (TSF) ok |

v7-followup.log:

| chain | up | carried |
|---|---|---|
| 520 | 2026-11-04 05:48Z (attempt 1) | ui taker-refund (TSL) ok |

## Recordings

`videos/TakerSellsLez-zec-{happy,maker-refund,concurrent}.mp4`: recorded by
`ui-e2e.sh --record` on chains 400–402 during V6; the reviewer checked each one
frame by frame and accepted it (2026-10-31 20:10Z, 20:31Z, 20:50Z).
`videos/TakerSellsForeign-zec-{happy,maker-refund,concurrent,stale-window}.mp4`:
recorded in V7 on the v7 images (above), each on a fresh chain with zebra
restarted first, each on its first try; zec-maker-refund again after `2f69932a`.
The first selling-ZEC recordings (2026-11-03, `b1dd0af8` images,
`logs/record-tsf.log`) are replaced; their logs stay in `logs/`. A recording is kept only when no
press had to be repeated (`RETRIED-PRESS` absent). The desk-copy notes are in
`RESULTS.txt`.

## Changes BTC shares (owner rule: M3 scope frozen, every shared change listed)

The M3 BTC code (`btc-*` crates, the BTC actors) is not changed on this branch.
Code BTC shares with ZEC was changed. The rule used for this list: commits on
this branch since `4b76e2f7` that touch `maker-node`, `taker-node`,
`actor-supervision`, `swap-store`, `lez-bridge-*`, the sidecar, the deploy
entrypoints and bootstrap, the lockfiles, or the shared desk code
`apps/basecamp/common` and the Taker desk. Each is covered by the BTC
regression in this set (Node 28/28, desks 16/16 on the same images).

- 2bde2fd2 test(taker-node): the first-attempt reply rule of 9afd25a8 is a tested helper; the API README names it
- 6648c363 test(swap-store): every bridge operation name inserts under the journal's CHECK
- de8f42a4 fix(maker-node): no Node-wide Zcash claim preimage; the agreement's template is the only source
- 8f01a687 fix(maker-node): a Zcash completion takes the claim preimage from the agreement's own template, not the Node-wide file
- c0510362 chore(taker-node): a failed Zcash take and a failed receipt binding name their cause on stderr
- 9afd25a8 fix(taker-node): a Zcash action's first attempt may be refused after its admission; the supervisor retries it, the caller gets the admission
- b1714423 fix(adapter): a re-plan retires the superseded steps' submit contexts, so the re-planned bytes get request ids of their own (D1)
- 0983e4ed fix(swap-store): the request-context journal admits the supersede operations and rebuilds an older journal on open (D1)
- e9ad978a fix(sidecar): a journaled submission success is decided on the chain again, so a taken nonce is named Superseded on the resubmit (D1)
- ac7739f4 test(swap-store): the Maker lock sqlite tests follow D2 (a deeper view of the same lock keeps the Maker eligible)
- c3091862 feat(zec): a superseded LEZ nonce is re-planned in the same drive attempt, with its lineage (D1)
- cac9476f feat(deploy): the LEZ finality heartbeat (Part C): one unit every 20 s, a stall declared in two minutes
- b04a8ad6 fix(taker-node): the supervisor reloads the actor config before an effect; material failures retry
- 7e63d2c1 feat(taker-node): the Taker Node supervises its Zcash actor itself, from activation to claim_available
- 9ab3132c test(supervision): every failure class a Node emits is a word of the shared alphabet
- 00788cb8 refactor(supervision): the schedule vocabulary both Nodes supervise actors with, lifted from the Maker
- 090c30eb fix(deploy): the wallet allocation lives in one place, written by gen-config and read by the bootstrap
- 25962a61 feat(deploy): LEZ_BOOTSTRAP_DEFER_CLAIM leaves one wallet's allocation in its vault
- d5a86f72 feat(zec): the Maker is refused by name too, and both desks say "LEZ balance too low to fund this swap"
- 92a755df fix(deploy): the market bootstrap's vault claim follows the 2 000 000 LEZ wallet allocation
- 4060a9e3 feat(zec): a LEZ depositor that cannot fund the lot is refused by name and shown on the desk
- d3a734b3 fix(deploy): 2 000 000 LEZ genesis and wallet allocations per role on the devnet
- 7e0f5c95 fix(zec): name a failed bundle load; the Regtest transfer signs for the branch Zebra reports
- 85e1e5c0 fix(taker-node): a swap whose bound bundle fails its checks needs attention; it does not fail the swap list
- 5b3ee3d0 feat(maker-node): the actor supervisor logs each supervised effect and each backoff
- 11f9bbdf fix(taker-node): the ZEC re-driver names the first error per swap; API doc states the replay rule
- 82f6f701 feat(deploy): two prepared ZEC entries side by side — zec-provision.sh --swaps 2, the concurrent fixture
- ee5ba914 feat(taker-node): the Node re-drives an admitted Zcash claim or refund on its own until terminal
- 8dd7d823 fix(taker-node): an admitted action replays whatever generation the desk re-sends it with
- 7aad0571 feat(taker-node): the Taker's revealing LEZ claim is offered in the reverse ZEC direction
- 85ebab27 fix(maker-node): maker_actor_repin_v1 re-pins by the swap's actor kind
- 6f94e6d3 fix(taker-node): a refunded Taker LEZ leg reads refund_in_progress, with Refund still offered
- c584b4dd deps: tokio 1.53.3, rustls-webpki 0.103.16, webpki-roots 1.0.11, ctutils 0.4.6; crypto-bigint 0.5 documented
- b7096233 deps(sidecar): clear nine advisories in the v0.2 sidecar lockfile
- f8b3753c deps: update rustls to 0.23.46 for four advisories
- b90730fc feat(deploy): node-e2e.py zec-taker-refund and zec-maker-refund on the devnet
- fb639a81 test(zec): devnet profile wire round-trip, re-observed first lock, profile doc
- 3322343c feat(deploy): node-e2e.py --pair Zcash zec-happy settles a ZEC/LEZ swap on the devnet
- 4153a71a fix(zec): find the second lock past re-observed first locks; report claim errors
- 74d0131a fix(deploy): one Maker ZEC claim-recovery key for the life of the state volume
- 6a45046b fix(deploy): pin the Maker's Delivery identity to the ZEC corridor key
- 969a573a feat(deploy): zec-provision.sh builds one prepared ZEC swap on the live devnet
- c0498265 feat(deploy): mount a ZEC corridor volume per role and start its LEZ sidecar
- 9a8fa971 fix(deploy): give the stack room on a slower, shared host
- f6aaa864 fix(nodes): a revealing claim held near its LEZ refund time names its holder; the Maker ZEC process tests pass again (REVIEW 18:24Z)
- 297531d7 fix(nodes): no refund once a revealing claim was submitted; one revealing LEZ claim in flight per Node until it is final, on both Nodes (FINDINGS 24)
- f8dbd75f feat(zec): a superseded revealing claim is re-planned, restaged with its lineage and margin, and resubmitted in the same attempt (FINDINGS 24)
- 5b769bcd feat(sidecar): a revealing claim whose nonce was taken is named Superseded and re-planned at the live nonce (FINDINGS 24)
- 8de775ae fix(taker-node): selling ZEC, one revealing LEZ claim in flight per Node — a second waits until the first is final
- 0d157589 fix(taker-node): selling ZEC, a Taker that does not claim can refund — the claim window admits a refund on the Zcash route
- b27adb33 docs(taker-node): the ZEC supervisor's design note says an open admission is read before the phase table
- c27d4271 fix(taker-node): selling ZEC, the Taker keeps driving after its revealing LEZ claim until it observes the Maker's Zcash claim
- 3f8bcb48 fix(maker-node): a Zcash drive that projects the Taker's revealing LEZ claim is a valid output (TakerSellsForeign)
- 512d892b fix(basecamp): the ZEC waiting-for-the-Maker's-lock sentence is a QString before it is joined
- 204909a5 fix(basecamp): the ZEC Taker rows say what happens in each direction; the claim text no longer describes the Bitcoin adaptor flow
- 9c318986 fix(basecamp): two Zcash swaps no longer share the Taker desk's action request id (Taker desk, shared with BTC)
- 104da65a fix(basecamp,deploy): the Taker desk says when it does not send a press; the driver waits for the Node's reply and marks every re-press (Taker desk, shared with BTC)
- b1dd0af8 fix(zec): selling ZEC, a refund pressed after the Maker refunded reads "refund in progress", and that row names the Maker's refund (REVIEW 17:58Z)
- 47782164 fix(zec): a recovery never ends while the party's own submitted lock is unsettled; a lock that confirms late is projected and refunded (FINDINGS 27, REVIEW 12:08Z)
- 955ea518 feat(zec): selling ZEC, the row after the claim window reads "Claim window closed"; the Taker view names it; drive_claim takes a typed trusted clock (REVIEW 08:49Z, 08:54Z, 08:59Z)
- 129c8bb3 fix(zec): a revealing LEZ claim is never sent after its window closes; the claimant's row offers its refund instead (FINDINGS 26, REVIEW 08:22Z)
- 396d58c4 feat(zec): DeterministicLocalDevnetV3 — the devnet profile with a 12-minute LEZ refund delay; a held revealing claim needs attention a third of the delay before its refund time (REVIEW 00:52Z)
- 64aa1df8 feat(deploy): gen-config mints each role's Zcash key; zebra mines to the Maker's, which is its Delivery identity

- 31df3352 fix(basecamp): selling ZEC, the Taker row offers nothing while its revealing LEZ claim lands, and its refund rows no longer wait for the Maker's LEZ refund (shared Taker desk; covered by V7: BTC desks 16/16)

Desk-driver changes in the shared harness (`deploy/scripts/ui-e2e.sh`, `verify.mjs`): `6d9588c0`, `7f551c94`, `d637fab4`, `2f69932a`.

## Findings fixed on this branch and where the run shows them

- **24, a revealing claim that lost its nonce.** A revealing LEZ claim publishes
  the secret; if another transaction of the same account took its nonce, the
  claim could stay unlanded while the counterparty used the secret. Fixed by
  re-planning it at the live nonce in the same attempt, never abandoning it
  (`5b769bcd`, `f8dbd75f`); no refund once a revealing claim was submitted, and
  one revealing claim in flight per Node until it is final (`297531d7`); a
  claim held near its refund time names its holder (`f6aaa864`). In this set:
  `zec-claim-collision` and `zec-concurrent` both ways.
- **25**, a replay after the transcript expired (`f6aaa864`).
- **26, a revealing claim after its window.** The SDK refuses it before
  anything is sent and the claimant refunds instead (`129c8bb3`); the Taker
  view and desk say "Claim window closed" (`955ea518`). In this set:
  `ui zec-stale-window`. Residual: the margin must exceed inclusion latency.
- **27, a late own lock stranded by a recovery.** A recovery first settles the
  party's own submitted lock: a lock that confirmed late is refunded too, and
  the recovery waits while it can still land (`47782164`). In this set:
  `zec-refund-then-maker-lock` both ways. **BTC has the same gap**
  (`project_own_maker_lock_before_recovery`); not changed here: the owner
  recorded it as an M3 limitation (2026-11-03), BTC scope stays frozen.

## Tests not run on this head

- `zebra_runtime_restart.rs` (`#[ignore]`): needs `scripts/run-zebra-e2e.sh`
  and two pinned Docker Zebra nodes.
- `taker_service_process.rs` (`#[ignore]`): needs the external pinned-Basecamp
  product driver.
- CI: the Zebra regtest gate and the ZEC test runs came over from PR #116 on
  2026-11-03 (`ff958ec8`, `6574eeea`, `055bb204`; the Zebra test image's base
  moved to a clean distroless in `b7251402`), after the runs in this set. Each
  new step ran green locally first: clippy, tests and docs of the three ZEC
  crates, the `pair-zec` Node tests, and `run-zebra-e2e.sh` with the Trivy
  scan (0 HIGH/CRITICAL). The touched crates' tests passed earlier (847 at
  `47782164`).

## Security scans

- Aikido on the branch: 23 HIGH / 8 MEDIUM / 21 LOW resolved by the dependency
  bumps (`c584b4dd`, `b7096233`, `f8b3753c`); one new HIGH was reported on
  `5deb2512`, whose text is on the Aikido dashboard (not yet copied here).
- Dependabot reports 2 open alerts (1 high, 1 low) on `main`, the default
  branch; not triaged in this set.
