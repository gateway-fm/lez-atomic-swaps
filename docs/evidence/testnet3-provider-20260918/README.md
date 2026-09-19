# A swap with Bitcoin served by a public RPC provider, 2026-09-18

One BTC→LEZ swap (`TakerSellsForeign`, 10,000 sat for 10 LEZ) between a Maker Node
and a Taker Node that have **no Bitcoin node and no Bitcoin wallet**: every Bitcoin
read and every broadcast went through a keyless public RPC provider (PublicNode,
**Bitcoin testnet3**), and the Taker's lock was signed in a wallet neither Node can
reach. The LEZ side is the **official LEZ testnet** (v0.2.4). Both Nodes reached
`completed`; the Maker needed no manual action.

This is the recorded run for Usability 08's public-node option (#49): observation
through a provider (#80) and a lock funded from the owner's own wallet (#81).

| Swap | Direction | Size | Outcome |
|---|---|---|---|
| [`b44abcdb…`](swaps/b44abcdb475c.json) | TakerSellsForeign | 10,000 sat / 10 LEZ | **completed on both Nodes, no manual Maker action** |

## What created what

"Node" means the role's Node acting on its own. A **request** is an owner-API call
someone made; locking and claiming are requests by design on the Taker side, and the
Maker side is meant to need none.

| Transaction | Chain | Created by | Trigger |
|---|---|---|---|
| Taker lock [`a7d50933…`](https://mempool.space/testnet/tx/a7d50933dc006d06a171627853920e421ac9f133dc8533f9833647f0ae659eb1) | Bitcoin testnet3, block 5,149,062 | **signed in the owner wallet, outside both Nodes**; broadcast by the Taker Node through the provider | the Taker Node answered `-32018 bitcoin_funding_required` with the contract address and 10,000 sat; [`tools/provider-swap.py`](tools/provider-swap.py) built and signed the transaction in the offline wallet and replayed the same take with it; request `taker_swap_lock_v1` then sent it |
| Escrow init `27a57b49…`, Maker lock (fund) `2452f455…` | LEZ | Maker Node | none — unattended, after it saw the Taker lock confirmed through the provider |
| Revealing claim `dc4e362d…` | LEZ | Taker Node | request `taker_swap_claim_v1`, sent by the driver when the Taker reached `claim_available` |
| Follow-up claim [`d5f15c7e…`](https://mempool.space/testnet/tx/d5f15c7eae2ff6eee9e7bdc81bb874c45c6159f425118d1ba13ef4a9194d8e78) | Bitcoin testnet3, block 5,149,109 | **Maker Node**, built, signed and broadcast through the provider | **none — unattended.** `maker_actor_manual_actions` holds no row for this swap; the claim pays 9,000 sat to the Maker owner's address `tb1qvmj43…` |

Every effect was sent exactly once (`attempt_count = 1` in both actors' journals).
The LEZ balances moved by exactly the trade: Maker 290 → 280, Taker 310 → 320.

### The owner wallet

A Bitcoin Core 31.1 with **no network at all** (`docker --network none`, `-connect=0`,
zero blocks): it holds the key and signs, nothing else. What it spends is looked up on
a block explorer, as a light wallet would, and the transaction is built with
`createrawtransaction` and signed with `signrawtransactionwithwallet`. It was funded
from a testnet3 faucet (169,255 sat, `9a0dab85…`). The lock spends that one native
SegWit output, pays the contract 10,000 sat and returns 158,255 sat, fee 1,000 sat.

## What had to be fixed for this to work, and every intervention

This run found one real defect and it is fixed in the same pull request.

1. **The readiness gate refused the provider (fixed, `15440ff`).** testnet3 nodes
   report `Unknown new rules activated (versionbit 1)` for good — anyone can signal a
   version bit on a test network — and the adapter refused any node whose
   `getblockchaininfo` carried a warning. The lock had been broadcast and confirmed,
   but the Taker's actor answered `ChainNotReady` until the fix was deployed
   (18:47Z); it then observed the lock at once. Warnings now block readiness on
   mainnet only.
2. **Node images replaced mid-swap, and the Maker re-pinned (18:47Z).** Deploying that
   fix changed the Maker's actor binary, so the swap stopped as
   `failed: actor_deployment_invalid` — the behaviour #73 describes. The owner action
   added for it, `maker_actor_repin_v1` (request `repin-b44abcdb475c-stack2`), moved
   the swap to the new actor. It authorises no transaction; it is recorded in the
   Maker's mutation journal.
3. **A first attempt that moved no funds.** Both Nodes had been given the same claim
   address, and an agreement refuses a Maker and a Taker paid at the same destination
   ([`provider-swap-attempt1.txt`](provider-swap-attempt1.txt)). The overlay now takes
   one address per role. Nothing was broadcast.
4. **The driver was restarted twice; the swap was not touched.** Once because a
   `git stash` in this checkout replaced the log file the driver was writing to, once
   because the Nodes were recreated for (1). [`tools/provider-swap-resume.py`](tools/provider-swap-resume.py)
   picks a locked swap up and appends to the same log; it sends nothing to Bitcoin
   and never calls a Maker action. The `RESUMED` lines in
   [`provider-swap-driver.txt`](provider-swap-driver.txt) mark the restarts.

## Two things worth reading in the Maker's trace

- **The late-lock path of #68, on a public network, unattended.** The take was at
  17:37:17Z, so the Maker's second-lock cutoff was 20:37:17Z. Its LEZ funding was
  submitted long before, but LEZ finality put it on the finalized chain at 20:40:14Z,
  after the cutoff. The supervisor therefore ran `recover`: 18 attempts between
  20:37:23Z and 20:39:38Z answered `first_lock_safety_uncertain` and rightly refunded
  nothing, then the lock was projected and the swap went on to completion. Before #68
  this is where the Maker ended in `failed` for good.
- **One unexplained transient.** At 20:39:48Z one actor invocation answered
  `actor agreement binding is invalid`; the next attempt, 26 seconds later, found the
  funding step and succeeded. It needed no intervention and we have not found its
  cause; it is recorded here rather than explained.

## Verification

`bitcoin_verification` in the record looks every Bitcoin transaction up **twice**:
with `getrawtransaction` through the same provider the Nodes used, and on an
independent explorer (mempool.space). They agree on block, outputs and fee. The lock
spends the faucet output with a two-item witness (P2WPKH); the claim spends the lock's
output 0 with a **one-item witness**, a Taproot key-path spend — the adaptor-signature
claim. `lez_verification` looks each LEZ transaction up on the public sequencer
(`testnet.lez.logos.co`, through the local proxy; the SHA-256 of the returned bytes is
kept) and on the finalized indexer: all three are found on both.

## Which software produced this

[`provenance.json`](provenance.json) has the image ids, binary hashes and commits.
The take, the lock and its broadcast ran on images built from `17213b1` (the top of
the v0.2.4 stack at the time); everything from 18:47Z ran on images built from
`15440ff`, which adds only the readiness fix. The provider is PublicNode's keyless
testnet3 endpoint: Bitcoin Core 29.3.0, `txindex` but no `txospenderindex`, wallet
RPCs refused, replies in JSON-RPC 2.0 and 1.x envelopes. The Nodes reach it through
an nginx that terminates TLS on the Docker network
([`provider-proxy-nginx.conf`](provider-proxy-nginx.conf)), because a Node accepts
only literal-loopback HTTP endpoints.

## Files, and what wrote each

| File | Written by |
|---|---|
| `swaps/b44abcdb475c.json` | [`tools/collect.py`](tools/collect.py), read-only: the Taker's view, the Maker's monitor, scheduler and manual-action rows, both actors' evidence kinds, effect journals and lock steps (SQLite opened `mode=ro` inside each Node container), then the Bitcoin lookups. [`tools/verify-lez.py`](tools/verify-lez.py) adds `lez_verification`. No key, nonce, adaptor secret or evidence payload is read |
| `snapshot.json` | the same run of `collect.py`: capture time, the provider's tip, both Nodes' wallet balances (Bitcoin `disabled`: there is no wallet) |
| `maker-actor-trace.txt`, `taker-actor-trace.txt` | `docker logs -t` of each Node container since 18:47Z, filtered to lifecycle lines and the actors' `LEZ_BTC_ACTOR_TRACE` events; identical events are collapsed to their first timestamp (UTC) and a count, with hashes and per-attempt clocks elided. The containers before 18:47Z were replaced, so their `ChainNotReady` lines are not in these files |
| `provider-swap-driver.txt` | stdout of `tools/provider-swap.py` and of its two resumptions (timestamps in it are local, UTC+2) |
| `provider-swap-attempt1.txt` | stdout of the first attempt, refused at the take (intervention 3) |
| `tools/provider-swap.py` | the driver: publishes one offer, takes it, signs the lock in the offline wallet when the Node asks for funding, requests the lock, waits, requests the claim, waits. It imports the helpers of `deploy/scripts/node-e2e.py`, mines nothing and never calls a Maker action |
| `provider-proxy-nginx.conf` | the proxy configuration, verbatim |
| `provenance.json`, `SHA256SUMS` | generated at the end of the capture |
