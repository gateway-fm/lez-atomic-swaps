# Node-driven XMR/LEZ swaps on Monero Stagenet through a self-hosted monerod, 2026-11-26

The Maker and Taker Nodes of `f259e791` ran XMR/LEZ swaps (the Taker sells
LEZ) with Monero on public Monero Stagenet and LEZ on the private devnet,
under the `PublicStagenet` windows C, R, P = +10 min, +2 h, +4 h. Both Nodes
and both roles' `monero-wallet-rpc` read one `monerod` the operator ran
himself, synchronized at the Stagenet tip, the self-hosted node of the
[Stagenet guide](../../monero-stagenet-guide.md#route-1-a-self-hosted-monerod);
no public node and no forwarder was configured. They completed one swap,
refunded one by the Taker Node's own refund at R and punished one. Times are
UTC; Monero times are block timestamps; LEZ times are the finalized block
times the Nodes recorded.

One operator ran both parties, Maker and Taker, and the `monerod`, on one
host: an Apple-silicon Mac with Docker Desktop, local linux/arm64 images. No
second operator and no second host took part.

## What ran

- **Code.** Both Nodes built from `f259e791` without the devnet test hooks
  ([PROVENANCE.md](PROVENANCE.md): `build-source.txt` of both images, and no
  hook string in any Node binary or sidecar).
- **Stack.** The devnet stack `lez-stagenet040-dev` on its own fresh LEZ
  chain, with `LEZ_MONERO_NETWORK=stagenet`: both Nodes read
  `profile: public-stagenet` and the windows 600, 7200 and 14400 s in their
  `xmr-role.json`. The Taker Node refunds by itself (the default). Both Nodes'
  health read `degraded: false` before the first take.
- **The daemon.** `monerod` 0.18.5.1, pruned, in the container
  `lez-stagenet040-monerod` (Docker network `lez-stagenet040-net`, alias
  `monerod`), synced from public Stagenet peers to the tip from 2026-11-24
  17:46 to 2026-11-25 19:28: height 2,258,356, 6.4 GiB of database
  (`node-route/tip-proof-at-tip.txt`). At the start of the run its top
  block, height 2,258,580, had the hash three public nodes gave
  (`node-route/tip-proof-start.txt`), and so had its top block at the end,
  height 2,258,725 (`node-route/tip-proof-end.txt`). Its RPC access log was on from 01:51:46
  (`daemon.rpc:INFO`, `net.http:DEBUG`, `txpool:DEBUG`).
- **The Nodes' Monero endpoints**, as Docker ran them
  (`node-route/node-daemon-addresses.txt`): both Nodes'
  `LEZ_MONEROD_HOST=lez-stagenet040-monerod:38081`; the Maker's
  `LEZ_XMR_WALLET_HOST=lez-stagenet040-wallet-maker:38083`, the Taker's
  `lez-stagenet040-wallet-taker:38083`. Both wallet-rpcs have
  `daemon-address=lez-stagenet040-monerod:38081` and `trusted-daemon=1`
  (`node-route/configs.txt`, logins removed). No container on the host ran a
  forwarder (`node-route/forwarders.txt` is empty).
- **Coins.** The Maker's role wallet held 0.00286476, 0.00379024 and
  0.00296954 XMR unlocked; before the run it received 0.1 XMR from the
  XMR-TW faucet (`94a738290da9df90efd4395403ea628cf15c9042b9a80bb615e55a0bf98d9e5e`,
  block 2,258,576). Each swap moved 0.003 XMR for 50,000 LEZ units. The
  swaps were taken one after another, each once the Maker had an unlocked
  output for it: A on the 0.00379 output, B on the faucet's, C on B's change.

## The swaps

| Swap | Ending | Take (LEZ anchor) | C, R, P | Taker's LEZ lock final | Maker's funding | What followed |
|---|---|---|---|---|---|---|
| A `b5fc18b7…` | completed | 01:55:27 (01:55:14) | 02:05:14, 03:55:14, 05:55:14 | 01:55:33 (`b109dd87…`, LEZ block 156) | `3ff40f7986894e068304c38eed5447ec984afc1599b7fcbfb1a91743d918ab2d`, block 2,258,583, 01:58:36 | `claim_available` 02:08:59; owner's claim 02:09:02; tag 14 `c7ba4851c3e6…` final 02:09:43 (LEZ block 822); Maker's tag 15 `3fc005618aa6…` final 02:10:42 (block 870); Taker's sweep `5b182e8c75c68fc937db3f99677c795a6c7f6bc9ea16a8a2dc3d8a88874306bf`, block 2,258,594, 02:13:19, 0.00296962 XMR; both Nodes `completed` 02:29:39, 34 minutes after the take |
| B `3d4c2915…` | refunded | 02:00:36 (02:00:26) | 02:10:26, 04:00:26, 06:00:26 | 02:01:11 (`81f62b82…`, block 406) | `a722e1e43be96cd83888fbbbe4d2c3107d4349ca9e3b1d2ed401688bc215fe13`, block 2,258,588, 02:03:51 | not claimed; the Taker Node's own refund, tag 16 `5dadd0ecab82…` final 04:00:32 (block 6235), 6 s after R; the Maker found it and swept its Monero back in `7dcb913175f2e3f6c3c99446f11581f2dba0ac831a5dbbde4b772057de2f0f0a`, block 2,258,649, 04:09:30, 0.00296956 XMR; Maker `refunded` 04:21:23, the sweep at ten confirmations |
| C `9d17b588…` | punished | 02:20:26 (02:20:17) | 02:30:17, 04:20:17, 06:20:17 | 02:20:58 (`d7e16f77…`, block 1371) | `2bc4938409ff547c3d210465a63ab3cd03c81e10b1462f2a56e45f1b217991b7`, block 2,258,599, 02:22:11 | the Taker Node stopped 04:10:10 to 06:26:16, through [R, P); the Maker's tag 17 `53b53a78b7c0…` final 06:20:32 (block 13074), 15 s after P; the Taker Node read `punished` at 06:26:25, after its restart. Its 0.003 XMR stays at the swap's shared address |

Each funding waited for an unlocked output of the Maker's: A took the
0.00379 XMR output, B the faucet's 0.1 XMR once it had unlocked, C B's change
once its ten blocks had passed.

## What the node shows

- Every funding and sweep of the swaps reached the node from a role's
  wallet-rpc: `POST /sendrawtransaction` from that wallet-rpc's address on
  `lez-stagenet040-net` (`node-route/network-map.txt`), then `Transaction
  added to pool: txid <…>`; each was then mined in a block the node had
  verified, at the height `get_transactions` on the node gives
  (`node-route/transactions.txt`).
- The Nodes read it themselves: each Node's launcher forwards its loopback
  18081 to `lez-stagenet040-monerod:38081` and 18083 to its own wallet-rpc
  (`node-route/node-loopback-forwards.txt`), and the node logged requests
  from both Nodes' and both wallet-rpcs' addresses throughout
  (`logs/monerod-callers.txt`).

## Files

- `PROVENANCE.md`: the tested commit, the images, the Monero release.
- `logs/watch.log`: each change of each swap as both Nodes report it
  (`taker_swap_monitor_v1` and the Maker's actor phase), polled from the
  host, UTC; `taker-unreadable` while the Taker Node was stopped.
- `logs/taker-node-stop.log`: when the Taker Node was stopped and started.
- `logs/maker-node.log`: the Maker Node's log, without its repeated
  `unchanged` and `awaiting_observation` drives. The Taker Node's log file is
  on a tmpfs and began again at its restart; after the restart it held only
  the monitor's closed RPC connections, and it is not kept.
- `logs/monerod.log`: the node's lines of the run (01:51:46 to 06:27) for
  each transaction sent to it (`POST /sendrawtransaction` and the caller's
  address) and each one its pool took; `logs/monerod-events.log`: all its
  other lines of the run but the requests; `logs/monerod-callers.txt`: the
  requests counted by caller address and method. The callers are the
  addresses of `node-route/network-map.txt`, and `172.30.0.1`, the host
  (the tip proofs and the progress log's `get_info`).
- `node-route/`: the tip proofs, the Nodes' daemon addresses and their
  launchers' loopback forwards to the node and the wallet-rpcs
  (`node-loopback-forwards.txt`), the network map, the containers and
  configs of the node and the wallet-rpcs, the forwarder check, each swap
  transaction as the node holds it with the node's log lines for it.
- `taker-records.jsonl`: the Taker Node's swap facts and progress record of
  each swap, as stored.

The logs hold no logins, wallet passwords or wallet addresses; the
transactions and LEZ effects in them are public on their chains.
