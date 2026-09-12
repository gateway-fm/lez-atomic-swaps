# Reproducing every scenario by hand and by script

One guide per scenario in the evidence matrix. Each scenario can be run three
ways, on the same Nodes and through the same Node methods:

- **API run**: `deploy/scripts/node-e2e.py <scenario> [--direction TakerSellsLez]`
  drives the two Nodes' owner sockets and writes `deploy/runtime/e2e/<scenario>[-lez].json`.
- **Desk run**: `deploy/scripts/ui-e2e.sh <scenario> [--direction TakerSellsLez] [--record]`
  presses the real Basecamp desks (the APIs are read only to learn the swap id);
  `--record` films it into `deploy/runtime/evidence/videos/<direction>-<scenario>.mp4`.
- **By hand**: the numbered steps below, on the desks over VNC, with the Node-side
  checks to run next to them. This is what the desk run automates.

The steps and the expected desk labels are the ones the harness asserts; the
recordings show exactly these screens.

## Before any scenario

**Stack.** From a checkout on an arm64 host with Docker (or from the release
bundle, whose `scripts/start.sh` does the same):

```sh
cd deploy
LEZ_TIMING_PROFILE=fast bash scripts/up.sh --fresh   # one unattended run; ~15 min the first time
docker compose --env-file runtime/runtime.env ps      # every service healthy
```

The `fast` timing profile is required for the refund scenarios and is what the
evidence was produced with:

| Setting | fast | local |
|---|---|---|
| Maker's lock cutoff after the take | 600 s | 1800 s |
| earlier refund (second locker) | 900 s | 3600 s |
| later refund (first locker) | 1200 s | 7200 s |
| Bitcoin refund CSV | 6 blocks | 144 blocks |
| Bitcoin regtest blocks | one every 120 s (auto-miner) | same |
| LEZ blocks | every 10 s; finality trails by one to three minutes, in bursts | same |

**Desks.** VNC to `vnc://127.0.0.1:5901` (password `lezswap`). The Basecamp
launcher shows two microapps: **LEZ / BTC Maker** (wallet *Munich Vault 01*, the
Maker Node's identity) and **LEZ / BTC Taker** (wallet *Zurich Wallet 01*, the
Taker Node's identity). Each desk shows **Backend connected** and its Node's
wallet balances (BTC and LEZ) top right; **Check Node** and **Refresh market**
read the Node on demand, and the desk refreshes itself every two seconds.

**Node-side checks.** Two shell helpers, run from `deploy/`:

```sh
set -a; source runtime/runtime.env; set +a
maker() { docker exec lez-maker-node curl -sS --unix-socket /run/lez/maker/node.sock \
  -H 'content-type: application/json' --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$1\",\"params\":[${2:-{\}}]}" http://localhost/ | jq; }
taker() { docker exec lez-taker-node curl -sS --unix-socket /run/lez/taker/node.sock \
  -H 'content-type: application/json' --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$1\",\"params\":[${2:-{\"schema_version\":1\}}]}" http://localhost/ | jq; }
btc() { docker exec lez-bitcoin-core bitcoin-cli -conf=/run-config/bitcoin.conf -datadir=/var/lib/bitcoin "$@"; }
```

- `taker taker_swap_list_v1` lists every swap the Taker Node knows with its
  `state` and `available_action`; `taker taker_swap_monitor_v1 '{"schema_version":1,"swap_id":"<id>"}'`
  adds the countersigned deadlines (`terms`) and the on-chain effects.
- `maker maker_actor_monitor_v1 '{"id":"<id>"}'` reports the Maker actor's phase.
- `btc -rpcwallet=lez-taker listtransactions '*' 3` / `-rpcwallet=lez-maker` show
  the Bitcoin locks, claims and refunds; the LEZ side is visible in the explorer
  at http://127.0.0.1:3003 (latest blocks) and the Bitcoin side at http://127.0.0.1:3002.

**Clean history.** `bash scripts/reset-swaps.sh` forgets every swap on both
Nodes (chains and wallets stay). The recordings were made after a reset before
each scenario so that a desk shows only its own swap.

**Directions.** Every scenario exists in both directions. The desk labels differ
only in which asset is "mine":

| | Taker sells BTC (`TakerSellsForeign`, default) | Taker sells LEZ (`TakerSellsLez`) |
|---|---|---|
| Maker offer | sells 1,000 LEZ for 0.01 BTC | sells 0.01 BTC for 1,000 LEZ |
| Taker's first lock | **Lock 0.01000000 BTC** | **Lock 1,000 LEZ** |
| Maker's second lock (automatic) | *Funding the LEZ escrow* | *Locking Bitcoin* |
| Taker's revealing claim | **Claim 1,000 LEZ** | **Claim 0.01000000 BTC** |
| Maker's follow-up claim (automatic) | *Claiming Bitcoin* | *Claiming LEZ* |
| Taker's refund | **Refund 0.01000000 BTC** | **Refund 1,000 LEZ** |

The Maker desk has a direction selector next to **Publish offer**; the Taker's
order book lists both kinds and shows the direction on each row.

**Reading the Taker desk.** A swap row in *My orders* carries a state label, a
progress bar, the countersigned deadlines as countdowns (*Maker locks by*,
*Maker refunds by*, *your refund from*, *BTC refund height*), and at most one
button. The labels in the order they appear in a completed swap:

`Preparing the swap` → `Your Bitcoin lock is ready` (button) → `Bitcoin lock
confirming` → `Waiting for the Maker's LEZ lock` → `Your LEZ claim is ready`
(button) → `LEZ claim submitted` → `Completed`

and on the Maker desk: `Waiting for the Taker's Bitcoin lock` → `Funding the
LEZ escrow` → `Waiting for the Taker's LEZ claim` → `Claiming Bitcoin` →
`Completed`. (Swap the chain names for the other direction.)

---

## happy

**What it proves.** One complete swap: five public transactions, both desks at
*Completed*, both Nodes at revision 4.

**Scripted.**
```sh
python3 scripts/node-e2e.py happy                       # ~4 min
python3 scripts/node-e2e.py happy --direction TakerSellsLez
bash scripts/ui-e2e.sh happy [--direction TakerSellsLez] [--record]   # ~8 min
```

**By hand.**
1. Maker desk: choose the direction, click **Publish offer** twice (the Taker's
   order book needs at least one pending offer of that kind; the harness keeps
   two). Check: `maker maker_offer_list` lists them `active`.
2. Taker desk: in *Available orders* click **Take offer** on a *Munich Vault 01*
   row of the wanted direction. Within ten seconds the swap appears in *My
   orders* as `Preparing the swap`, then `Your Bitcoin lock is ready` (or `Your
   LEZ lock is ready`). Check: `taker taker_swap_list_v1` → `awaiting_first_lock`;
   `maker maker_offer_list` shows the offer `consumed` with the `swap_id`.
3. Taker desk: click **Lock …**. The row shows `… lock confirming`, then
   `Waiting for the Maker's … lock`. Check: `btc -rpcwallet=lez-taker
   listtransactions '*' 1` (selling BTC) or the LEZ explorer (selling LEZ)
   shows the lock. Selling BTC the lock needs one block (up to two minutes);
   selling LEZ it needs finality (one to three minutes).
4. Nothing to click: the Maker desk goes `Waiting for the Taker's … lock` →
   `Funding the LEZ escrow` / `Locking Bitcoin` → `Waiting for the Taker's …
   claim`. Check: `maker maker_actor_monitor_v1` phase `both_legs_locked`.
5. Taker desk: the row shows `Your … claim is ready`; click **Claim …**. The
   row shows `… claim submitted`.
6. Nothing to click: the Maker desk shows `Claiming …`, then `Completed`; the
   Taker desk shows `Completed`. Check: both `taker_swap_list_v1` state
   `completed` and the Maker monitor `completed`, revision 4.
7. Evidence: `python3 scripts/export-node-evidence.py --swap <id>` writes the
   swap's five transaction identities, confirmed against both chains, to
   `runtime/evidence/<swap id>.json` and the explorer's evidence view.

**Pass criteria.** Both desks at *Completed*; five effects in the export
(Taker's lock, escrow initialization, Maker's lock, revealing claim, follow-up
claim). Typical duration 5 to 8 minutes.

---

## replay (API only)

**What it proves.** Every owner call is idempotent under its request id: a
repeated publish, take, lock or claim returns `was_replay: true` and causes no
second effect.

**Scripted.** `python3 scripts/node-e2e.py replay [--direction TakerSellsLez]` (~4 min).

**By hand** (the desks cannot repeat a request; use the helpers):
1. `maker maker_offer_publish '{"request_id":"r-1","offer_id":"offer-r-1","route":{"pair":"Bitcoin","direction":"TakerSellsForeign"}}'`
   twice: the second reply carries `was_replay: true` and the same offer.
2. `taker taker_swap_initiate_v1` with the same `request_id` twice: the second
   reply is `was_replay: true` with the same `swap_id` (the exact params are in
   `node-e2e.py`, function `take`).
3. `taker taker_swap_lock_v1 '{"schema_version":1,"swap_id":"<id>"}'` twice:
   the same transaction id both times; the wallet shows one lock.
4. After the Maker's lock, `taker taker_swap_claim_v1` twice: `was_replay: true`.

**Pass criteria.** No second offer, swap, lock or claim exists; the swap
completes normally.

---

## wrong-inputs (API only)

**What it proves.** Malformed or stale requests are refused with a category
and change nothing. The nine cases the harness sends:

| Request | Expected |
|---|---|
| reusing a publish request id for another offer | refused |
| withdrawing with a stale revision | refused |
| taking with a mismatched envelope hash | refused |
| taking an off-preset amount | refused |
| taking with a wrong LEZ quote | refused |
| locking an unknown swap | refused, category `lock_swap_unknown` |
| claiming an unknown swap | refused |
| claiming before the Maker funded | refused |
| taking the consumed lot again | refused |

Each refusal is a JSON-RPC `error` with a `data.category`; the harness asserts
the error and, for the unknown-swap lock, the category.

**Scripted.** `python3 scripts/node-e2e.py wrong-inputs [--direction TakerSellsLez]` (~3 min).

**By hand.** Send each request with the helpers (the exact bodies are in
`node-e2e.py`, function `scenario_wrong_inputs`) and confirm the JSON-RPC
`error` with its `data.category`; `taker taker_swap_list_v1` and
`maker maker_offer_list` are unchanged afterwards.

---

## restart-taker

**What it proves.** A swap survives a Taker Node restart between its lock and
its claim: the Node reloads it from its own saved copy and the desk continues.

**Scripted.**
```sh
python3 scripts/node-e2e.py restart-taker [--direction TakerSellsLez]
bash scripts/ui-e2e.sh restart-taker [--direction TakerSellsLez] [--record]
```

**By hand.** Steps 1 to 3 of *happy*, then:
4. `docker restart lez-taker-node`; wait for `docker inspect -f
   '{{.State.Health.Status}}' lez-taker-node` to say `healthy` (about ten
   seconds). The Taker desk shows *Backend connected* again; the swap row is
   still there in the same state. Check: `taker taker_swap_list_v1` still lists
   it (not terminal, not `attention_required`).
5. Continue with steps 4 to 7 of *happy*.

**Pass criteria.** As *happy*, with the restart in the log.

---

## restart-maker

**What it proves.** A Maker Node restarted between the Taker's lock and its
own lock still locks in time and completes the swap.

**Scripted.**
```sh
python3 scripts/node-e2e.py restart-maker [--direction TakerSellsLez]
bash scripts/ui-e2e.sh restart-maker [--direction TakerSellsLez] [--record]
```

**By hand.** Steps 1 to 3 of *happy*, then:
4. `docker restart lez-maker-node`, wait for `healthy`. The Maker desk comes
   back at `Waiting for the Taker's … lock` or `Funding the LEZ escrow` /
   `Locking Bitcoin` and proceeds to `Waiting for the Taker's … claim`.
5. Continue with steps 5 to 7 of *happy*.

**Pass criteria.** As *happy*. The restart must happen before the Maker's
cutoff (600 s after the take); a Maker that comes back later does not lock
(see *taker-refund*).

---

## survivor

**What it proves.** The Maker Node completes the swap alone once the Taker's
revealing claim is on chain, with the Taker Node down; the Taker's desk catches
up on restart.

**Scripted.**
```sh
python3 scripts/node-e2e.py survivor [--direction TakerSellsLez]
bash scripts/ui-e2e.sh survivor [--direction TakerSellsLez] [--record]
```

**By hand.** Steps 1 to 5 of *happy* (click **Claim …**), then immediately:
6. `docker stop lez-taker-node`. The Taker desk shows *Backend unavailable*.
7. Maker desk: `Claiming …`, then `Completed` (one to three minutes). Check:
   `maker maker_actor_monitor_v1` → `completed`.
8. `docker start lez-taker-node`, wait for `healthy`. The Taker desk reconnects
   and the row moves to `Completed`. Check: `taker taker_swap_list_v1` →
   `completed`.

**Pass criteria.** The Maker reached *Completed* while the Taker container was
stopped; the Taker shows *Completed* after its restart.

---

## concurrent

**What it proves.** Two swaps taken, locked and claimed interleaved each keep
their own row, state and outcome.

**Scripted.**
```sh
python3 scripts/node-e2e.py concurrent [--direction TakerSellsLez]
bash scripts/ui-e2e.sh concurrent [--direction TakerSellsLez] [--record]
```
(The harness first makes sure the locking wallet holds two spendable coins;
by hand, `btc -rpcwallet=lez-taker listunspent` must show two outputs of at
least 0.02 BTC when selling BTC. If not: `btc -rpcwallet=lez-taker
sendtoaddress "$(btc -rpcwallet=lez-taker getnewaddress '' bech32m)" 0.05`
and wait for a block.)

**By hand.**
1. Maker desk: **Publish offer** until two offers are pending.
2. Taker desk: **Take offer** on one row, then **Take offer** on the other. Two
   rows appear in *My orders*, each `Your … lock is ready`.
3. Taker desk: **Lock …** on the first row, then **Lock …** on the second.
   Selling LEZ the second lock waits until the first is sequenced (a few
   seconds); the button may read *pressing again* in the harness log if the
   Node answered *unavailable* while it waited.
4. Nothing to click: both Maker rows reach `Waiting for the Taker's … claim`.
5. Taker desk: **Claim …** on each row.
6. Both rows on both desks reach `Completed`. Check: `taker taker_swap_list_v1`
   lists both swaps `completed`; export both.

**Pass criteria.** Two distinct swap ids, each with its own five effects; no
row ever shows the other's state.

---

## taker-refund

**What it proves.** The Maker never locks; after the Maker's cutoff the Taker
desk offers the refund, the Taker Node drives it once its refund time is
reached, and the Maker, when it comes back, reconciles to *Refunded* without
locking late.

**Scripted.**
```sh
python3 scripts/node-e2e.py taker-refund [--direction TakerSellsLez]     # ~23 min
bash scripts/ui-e2e.sh taker-refund [--direction TakerSellsLez] [--record]   # ~22 min
```

**By hand.** Steps 1 and 2 of *happy*, then:
3. `docker stop lez-maker-node` (before the Taker locks, so the Maker cannot
   lock). The Maker desk shows *Backend unavailable*.
4. Taker desk: **Lock …**. The row shows `… lock confirming`, then `Waiting
   for the Maker's … lock`, with *Maker locks by* counting down (600 s after
   the take).
5. When the countdown reaches zero the row shows `Refund available`
   (`The Maker did not lock in time; you may recover your …`) with the button
   **Refund …**. Check: `taker taker_swap_list_v1` → `refund_available`,
   `available_action: refund`.
6. Click **Refund …**. The row shows `Refund submitted · Your Node observes
   the refund`. The Node sends the refund only when both are true: *your
   refund from* has passed (1200 s after the take, the first locker's refund)
   and, selling BTC, the Bitcoin CSV has matured; selling LEZ, the Bitcoin
   chain's median time has also passed the Maker's cutoff (the Node checks
   the Maker's Bitcoin leg is absent at a stable tip; regtest's median time
   trails the wall clock by about ten minutes). Check: `taker
   taker_swap_monitor_v1` shows the `terms`; the refund appears in the wallet
   (`btc -rpcwallet=lez-taker listtransactions '*' 1`) or the LEZ explorer.
7. `docker start lez-maker-node` any time after step 5 (the harness does it
   right after the refund is admitted). The restarted Maker must **not** lock:
   `btc -rpcwallet=lez-maker listtransactions '*' 1` shows no new send. Its
   desk shows `Lock window missed`, then `Refunded` once the Taker's refund is
   final.
8. Taker desk: `Refunded` (`Your … came back`). The wallet balance top right
   is back to what it was before the lock (minus fees on Bitcoin).

**Pass criteria.** Taker desk *Refunded*, Maker desk *Refunded*, no Maker lock
transaction after the cutoff. Duration about 22 minutes under `fast`.

---

## maker-refund

**What it proves.** The Taker never claims; the Maker Node refunds its own
lock after its deadline on its own, then the Taker desk offers and completes
its refund.

**Scripted.**
```sh
python3 scripts/node-e2e.py maker-refund [--direction TakerSellsLez]     # ~25 min
bash scripts/ui-e2e.sh maker-refund [--direction TakerSellsLez] [--record]  # ~24 min
```

**By hand.** Steps 1 to 4 of *happy* (do **not** click Claim), then:
5. Wait. The Taker row shows `Your … claim is ready` until *Maker refunds by*
   (900 s after the take) passes, then `Claim window closed` (`A claim can no
   longer land; your Node waits for the Maker's refund, then offers yours`).
   The Maker desk shows `Claim window closed`, then `Refunded` (`Your … lock
   came back; the Taker's refund follows on its own`). Check: the Maker's
   refund in `btc -rpcwallet=lez-maker listtransactions '*' 1` (selling LEZ)
   or the LEZ explorer (selling BTC); `maker maker_actor_monitor_v1` →
   `maker_leg_refunded`.
6. Taker desk: the row shows `Refund available`; click **Refund …**, then
   `Refund submitted`, then `Refunded` (the first locker's refund opens 1200 s
   after the take). Check: `taker taker_swap_list_v1` → `refunded`.

**Pass criteria.** Maker desk *Refunded* before the Taker acts; Taker desk
*Refunded* after its refund; both wallets back to their pre-lock balances
(minus Bitcoin fees). Duration about 24 minutes under `fast`.

---

## regenerated-config (API)

**What it proves.** A swap taken under one rendering of the live role
configuration survives a maintenance re-render and a Taker Node restart: it
reloads from its own saved copy of the configuration and completes.

**Scripted.** `python3 scripts/node-e2e.py regenerated-config [--direction TakerSellsLez]` (~4 min).

**By hand.**
1. Steps 1 to 3 of *happy* (publish, take, lock), by desk or API.
2. `bash scripts/gen-config.sh runtime` (renders `runtime/` again with the
   same chain and identities; nothing else changes).
3. `docker restart lez-taker-node`, wait for `healthy`.
4. `taker taker_swap_list_v1` still lists the swap, not terminal and not
   `attention_required`.
5. Continue with steps 4 to 7 of *happy*: the swap completes on both Nodes.

---

## tampered-config (API)

**What it proves.** The per-swap copy of the role configuration is
integrity-checked on reload: one altered byte and the swap is refused after
a restart.

**Scripted.** `python3 scripts/node-e2e.py tampered-config` (~30 s).

**By hand.**
1. Steps 1 and 2 of *happy* (publish, take).
2. Find the swap's directory inside the Taker container and append one byte to
   its saved configuration:
   ```sh
   docker exec lez-taker-node python3 -c '
   import glob, json, sys
   for f in glob.glob("/var/lib/lez/taker/btc/swaps/*/taker-swap.json"):
       s = json.load(open(f))
       if bytes(s.get("swap_id") or []).hex() == sys.argv[1]: print(f.rsplit("/", 1)[0])' <swap id>
   docker exec lez-taker-node sh -c "printf '\n' >> <that directory>/taker-role-config.json"
   ```
3. `docker restart lez-taker-node`, wait for `healthy`.
4. `taker taker_swap_list_v1` lists the swap as `attention_required`;
   `taker taker_swap_lock_v1 '{"schema_version":1,"swap_id":"<id>"}'` is
   refused with category `lock_swap_unknown`. On the Taker desk the row shows
   `Needs attention`.

---

## Recording a hands-on run

The harness records by spawning each desk on a private virtual display; a
hands-on run over VNC can be captured the same way from the host:

```sh
# from deploy/, one segment of the Taker desk on the stack's display
docker compose --env-file runtime/runtime.env run --rm --no-deps \
  -v "$PWD/runtime/evidence/videos:/recordings" --entrypoint bash basecamp-ui \
  /ui-tests/record-step.sh taker my-segment
```

or simply screen-record the VNC window. The narrated subtitles in the delivered
videos come from the desk runner's own log lines, one per action.

## When something does not happen

- **A refund or claim is "submitted" but never lands.** Read the actor's trace:
  set `LEZ_BTC_ACTOR_TRACE=1` in `runtime/runtime.env`, recreate the Nodes
  (`bash scripts/up.sh`), and watch `docker logs -f lez-taker-node`: every
  effect decision, submission outcome and uncertain safety read is one JSON
  line (`effect_reconcile`, `effect_submission`, `first_lock_safety_uncertain`,
  `lez_asset_refund_uncertain`, `maker_lock_not_sent`).
- **The desk looks stale.** Click **Refresh market**; the desk's own reads are
  one at a time and a Node with many swaps answers slowly.
- **Finality stops for a few minutes.** The devnet's L1 wins few leader slots in
  some epochs (about one in every few, one to three minutes each); the indexer
  then catches up in one burst. Swaps complete through it.
- **`repair-indexer.sh --check` fails.** The LEZ indexer cannot serve a
  historical read; `bash scripts/repair-indexer.sh` re-indexes (a minute).
- **Keep the evidence before a reset.** `bash scripts/dump-swaps.sh <label>`
  copies both Nodes' owner views, swap directories and container logs to
  `runtime/e2e/dumps/`.
