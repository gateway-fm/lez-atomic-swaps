# Remote chain RPC (~30–200 ms per call): what breaks and where

Measured on the Linux host with `scripts/rpc-count-proxy.py`. It sits on the chain ports
and logs each JSON-RPC method, the port and the time to the first response byte; the SSH
tunnel was moved behind it (logs: `remote-logs/rpc-calls.log`). Per call through the
tunnel: median 30–120 ms, p90 60–220 ms, max ~650 ms. Locally on the Mac: 1–5 ms.

## 1. ZEC: the "identical tip before and after" check

- **Check:** `crates/zec-swap-sdk/src/lez_observation.rs:447–448` (first-lock and
  claim evidence) and `:648–649` (snapshot chain). Both require
  `tip.before_hash == tip.after_hash && tip.before_height == tip.after_height`, else
  `LezObservationError::UnstableTip`. The adapter wraps it at
  `crates/lez-bridge-adapter/src/lib.rs:506` (`Canonical(...)`).
- **The sidecar side tolerates growth:** `tip_extends`
  (`crates/lez-bridge-adapter/src/lib.rs:455`) and the sidecar's `confirm_extends`
  (`compat/lez-v0_2-sidecar/src/bridge_runtime.rs:2335`) accept a later tip whose block at
  the pinned height is unchanged. The comment there: "a scan that outlives one slot must
  not fail for that alone". The SDK's strict equality is the only place that fails.
- **RPCs between the two tip reads in one sidecar `observe_escrow`**
  (`bridge_runtime.rs:1643`), all sequential:
  1. Tip: `getLastBlockId`, `getBlock`.
  2. `getBlockRange` over the window.
  3. Tip again: `getLastBlockId`, `getBlock` (plus `getBlock` at the pinned height if
     it moved).
  4. Snapshot: account reads (`getAccount` ×2).
  5. Final tip: `getLastBlockId`, `getBlock` (plus one if moved).

  That is about 8–10 calls, plus the `getBlockRange` payload. During the passing 5 s run
  the sequencer saw 133 `getLastBlockId` and 114 `getBlock` for 20 `getBlockRange`, which
  fits ~6 tip/block reads per range scan.
- **Tolerance:** one observation takes D ≈ 8–10 × (30–190 ms) ≈ 0.3–1.9 s. With blocks
  every T seconds, the chance one observation sees no new block is about 1 − D/T.

  | LEZ block interval | chance per attempt | observed |
  |---|---|---|
  | 1 s (devnet default) | ≈ 0 when D > 1 s; fails nearly always | 7 of 7 first-lock attempts and 7 of 7 claim attempts refused |
  | 5 s | ≈ 60–94 % for the first-lock observation | TakerSellsLez passed (1 `UnstableTip`, retried); TakerSellsForeign: the Taker's LEZ **claim** observation was refused 12 of 12 times (it walks finalized blocks, so its D is longer) |
  | 10 s | ≈ 80–97 % | the same TakerSellsForeign claim was refused once more, then went through ~20 s after the switch; swap completed |

  Locally (D ≈ 20–50 ms) a 1 s chain gives ≈ 95–98 %, which is why the Mac runs pass.
- **Fix options (product code unchanged here):**
  - The SDK accepts an "after" tip that extends the "before" tip, i.e. the pinned block is
    still canonical, as the sidecar already does.
  - The observation reads state at a pinned height rather than at the moving tip.
  - The adapter retries the observation a few times immediately instead of reporting
    `actor_drive_unavailable` and waiting for the actor's 30 s retry.

## 2. BTC: Maker actor attempts timing out (`actor_timeout`), and the slow 2fb30c321e4a

- **Measured** (199 s, Nodes idle except one Maker swap in `recover`): the indexer got
  1,475 `getBlockById` + 1,472 `getBlockByHash`. In 10 s bins the summed request time was
  ~9.7 s per 10 s: one sequential walker, busy ~30 s, then a ~10 s gap, repeating. That is
  the Maker actor attempt (deadline 30 s), killed at the deadline (`actor_timeout`), then
  its 10 s backoff. ~560 calls per attempt = ~280 blocks × 2 calls, which is the `fast`
  profile's 300-block LEZ discovery window.
- **Code:** `compat/lez-v0_2-sidecar/src/finalized_refund_observation.rs:270–271` walks
  `window.start_height()..=scan_end` one block at a time. `read_finalized_block`
  (`finalized_claim_observation.rs:317`) makes 2 indexer calls per block (`block_by_id`,
  then `block_by_hash`). Lines 628–629 repeat the walk for descendants. Locally that is
  ~600 × 2 ms ≈ 1.2 s. Through the tunnel it is ~600 × 50 ms ≈ 30 s or more, so it never
  fits the actor's 30 s attempt deadline. The swaps it hit:
  - 6d3e75450ea6: the Maker moved to recovery instead of funding.
  - abf2c6dbdc75: the Maker never reconciled the Taker's refund.
- **2fb30c321e4a (87 s Maker funding, failed claim window):**
  - Phase times from the harness's 3 s polls:
    - Taker LEZ lock → seen locked: ~56 s (passing 142e42bb: ~9 s; Mac: ~6 s).
    - Then the Maker's BTC lock → `claim_available`: ~31 s (142e42bb: ~13 s; Mac: ~6 s).
  - The Node logs have no timestamps and the actor DBs keep only the latest state, so a
    per-call trace of that swap cannot be rebuilt. Not verified call by call.
  - What was measured for the same mechanism: every LEZ observation that walks finalized
    blocks costs ~100 ms per block over the tunnel against ~4 ms locally. Under the
    devnet `refund` profile the discovery window is 130 blocks (~260 calls ≈ 9–30 s per
    walk).
  - When 2fb30c ran, the Taker's sequential observer sweep also carried two dead BTC swaps
    from the permission failures (`awaiting_second_lock`, each with a live sidecar). The
    Maker's actor for 2fb30c was leased 52 times over its life. On clean state
    (142e42bb) the same step took 9 s.
  - **Conclusion:** the tunnel latency is the root cause, multiplied by per-block
    sequential indexer reads. The dead swaps from the earlier failures added sweep load.
    Miner cadence (2 s BTC blocks, same as the Mac) is not a factor. Bitcoin Core calls
    are few: the desks' `getbalances` polling (~1/s) dominates port 18443.
- **Fix options:**
  - Batch or parallelise the finalized-block walk (one `getBlockRange`-style indexer call,
    or a bounded concurrent fetch).
  - Drop the second `block_by_hash` read when `block_by_id` already returned a finalized
    header.
  - Cache finalized blocks across attempts (they cannot change).
  - Make the actor attempt deadline scale with the window.
