# Node-driven XMR/LEZ swaps on public Monero Stagenet, 2026-11-22

The Maker and Taker Nodes of `f059d3fa` ran XMR/LEZ swaps (the Taker sells
LEZ) with Monero on public Monero Stagenet and LEZ on the private devnet,
under the `PublicStagenet` windows C, R, P = +10 min, +2 h, +4 h. They
completed one swap, refunded one by the Taker Node's own refund at R and
punished one, and refunded a fourth that the Maker could not fund. Times
are UTC; Monero times are block timestamps; LEZ times are the finalized
block times the Nodes recorded.

One operator ran both parties, Maker and Taker, on one host: an
Apple-silicon Mac with Docker Desktop, local linux/arm64 dev-profile
images. No second operator and no second host took part.

## What ran

- **Code.** Both Nodes built from `f059d3fa`
  (`/usr/local/share/lez/build-source.txt` in both images), whose shipped
  code is that of `7b4e46da`, the commit of the
  [full regression](../regression-0.4.0-2026-11-22-macos-arm64/). Built
  without the devnet test hooks (`LEZ_TEST_HOOKS` unset; the Taker Node
  binary holds no `LEZ_TEST_HOOKS_DIR`).
- **Stack.** The devnet stack `lez-m4-stagenet-dev` on its own fresh LEZ
  chain, with `LEZ_MONERO_NETWORK=stagenet`: both Nodes read
  `profile: public-stagenet` and the windows 600, 7200 and 14400 s in their
  `xmr-role.json`. The Taker Node refunds by itself
  (`xmr_automatic_refund: true`, the default).
- **Daemons.** Public Stagenet nodes, each behind an haproxy forwarder that
  keeps one connection open: the Maker Node's `node2.monerodevs.org:38089`,
  the Taker Node's `node.monerodevs.org:38089`. Each role's
  `monero-wallet-rpc` (Monero 0.18.5.1) reads `stagenet.xmr-tw.org:38081`.
  No self-hosted daemon took part.
- **Coins.** The Maker's role wallet held 0.01576 XMR in three unlocked
  outputs from the run of 2026-11-21; each swap moved 0.003 XMR for 50,000
  LEZ units.

## The swaps

| Swap | Ending | Take (LEZ anchor) | C, R, P | Taker's LEZ lock final | Maker's funding | What followed |
|---|---|---|---|---|---|---|
| A `13711985…` | completed | 10:14:07 | 10:24:07, 12:14:07, 14:14:07 | 10:14:25 (`7de5d07a…`) | `007ccb716a347a830f0ae6d59225aa3d1ea97efc06fb83462115d80d92082f74`, block 2,255,819, 10:20:09 | `claim_available` 10:43:03; owner's claim 10:43:12; tag 14 `634161127ae0…` final 10:44:11 (LEZ block 1377); Maker's tag 15 `8e5682fc9103…` final 10:44:28 (block 1389); Taker's sweep `f2e6b550fb950c3e05885c94040b1d8ece00602997d2052ac1169c30fcfca87f`, block 2,255,838, 10:55:00, 0.0029695 XMR; both Nodes `completed` 11:14:22, 60 minutes after the take |
| B `fc596d6b…` | refunded | 10:13:44 | 10:23:44, 12:13:44, 14:13:44 | 10:14:04 (`9eab1410…`) | `99acf8fd385da05230851ea41b4d7e489454ad2e0a7b3d5e9f216093713e1ff6`, block 2,255,818, 10:16:06 | not claimed; the Taker Node's own refund, tag 16 `85024935e244…` final 12:15:25 (block 3842), 101 s after R; the Maker found it and swept its Monero back in `5b1ea9674331c03e22a9a3478de2ab632eab53c5b9b389aa992fa06cbb9980a3`, block 2,255,869, 12:22:09, 0.00296954 XMR (fee 0.00003046); Maker `refunded` 12:50:47, the sweep at ten confirmations |
| C `5a696c9f…` | refunded, never funded | 10:28:50 | 10:38:50, 12:28:50, 14:28:50 | 10:30:54 (`fae261c3…`) | none: three attempts answered `not enough unlocked money` (the wallet had spent its outputs on A and B); Maker `expired` | the Taker Node's own refund, tag 16 `f61a0c6abe11…` final 12:28:57 (block 4405), 7 s after R |
| C2 `353a4758…` | punished | 10:44:34 | 10:54:34, 12:44:34, 14:44:34 | 10:51:27 (`47d36b4e…`) | `3779bd5b2e9b06ba4569adea461678d2fae95cee1b06afb1e3f5ade3d2fe87d5`, block 2,255,837, 10:53:03 | the Taker Node stopped 12:33:01 to 14:50:08, through [R, P); the Maker's tag 17 `cd923939c0ee…` final 14:44:57 (block 8965), 23 s after P; the Taker Node read `punished` at 14:50:31, after its restart. Its 0.003 XMR stays at the swap's shared address |

Swap C was the punish swap as planned; its funding failed, so C2 took its
place. C stands as a Stagenet run of the unfunded case of TM-0031: the
Maker funded nothing, and the Taker Node refunded its LEZ by itself from R.

## What the review's fixes did here

- **The refund runs before any Monero read.** The Taker Node's tag 16 for B
  and C went out within its first sweep past R, while its Monero steps for
  other swaps were timing out (below). Its view of B read `refunded` at
  12:24:21, nine minutes after tag 16 was final: the record of a sent
  refund is read again only in a later sweep, which the timed-out Monero
  steps of the other swaps made long.
- **The Taker's Monero steps have a budget.** Eighteen of them ended at it
  (`the Monero step took longer than 120s`, `logs/taker-node.log`): twelve
  were C's observation of a funding that never came, four C2's and two A's.
  The view-only wallets restored through public nodes take more than two
  minutes to refresh; the wallet-rpc goes on refreshing after the request
  is dropped, and the next step reads the result. A reached
  `claim_available` 23 minutes after its funding was mined, and C2's funding
  was recorded the same way; on public nodes this budget costs a step, not
  a swap.
- **The funding is read again before tag 14.** A's tag 14 was final 59 s
  after the owner's claim, with that read before it.
- **The daemon is checked before each Taker decision.** No step failed on
  the sync or tip-age check: both public nodes were synchronized with fresh
  tips throughout.
- **The Maker's drive survives a failing funding observation.** C2's
  observer timed out once (`the Monero funding is not reconciled yet: XMR
  Maker observer timed out`, `logs/maker-node.log`); the drive went on and
  recorded the funding later.
- **After P.** C2 had no tag 14, so the Taker Node read it `punished` from P
  (on its restart). An authorised claim past P (`xmr_watching_for_maker_claim`)
  did not occur in this run.

## Files

- `logs/watch.log`: each change of each swap as both Nodes report it
  (`taker_swap_monitor_v1` and the Maker's actor phase), polled from the
  host, UTC; `taker-unreadable` while the Taker Node was stopped.
- `logs/taker-node-stop.log`: when the Taker Node was stopped and started.
- `logs/maker-node.log`: the Maker Node's log lines for these swaps and its
  Monero funding and effect lines (repeated `unchanged` and
  `awaiting_observation` drives left out).
- `logs/taker-node.log`: the Taker Node's log, without the lines of the
  monitor's closed RPC connections.
- `taker-records.jsonl`: the Taker Node's swap facts and progress record of
  each swap, as stored.

The logs hold no logins, wallet passwords or wallet addresses; the
transactions and LEZ effects in them are public on their chains.
