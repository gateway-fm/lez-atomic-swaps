# Maker and taker APIs for independent applications

Use these owner APIs to build your own maker policies, taker automation, or
operator tools. The nodes own negotiation, signing, settlement, persistence,
and recovery. Your application supplies decisions and observes their results.
Start with the [scope proposal and answers for ecosystem #176](proposal.md).

This reference describes the API as implemented on `main`. Pin an integrating
application to a release containing `maker_offer_publish_v1`; v0.2.0 has the
other methods described here but not that new method. The current product
target is BTC↔LEZ on private local chains. For executable setup, see the
[deployment guide](../../deploy/README.md); do not infer public/testnet readiness
or another pair's lifecycle support from the existence of a DTO.

## Connect from any language

Transport is HTTP POST `/`, JSON-RPC 2.0, over the configured owner Unix-domain
socket. **`params` is an array containing exactly one object.** It is not the
object itself. Always supply a JSON-RPC `id` and inspect `error` even when HTTP
returns 200. The JSON-RPC ID correlates one response; the separate `request_id`
inside a mutation is its durable idempotency identity.

In the default Compose stack, call inside the relevant container:

```sh
docker exec lez-maker-node curl --fail-with-body --silent --show-error \
  --max-time 120 --unix-socket /run/lez/maker/node.sock \
  -H 'Content-Type: application/json' --data \
  '{"jsonrpc":"2.0","id":1,"method":"maker_health","params":[{}]}' \
  http://localhost/

docker exec lez-taker-node curl --fail-with-body --silent --show-error \
  --max-time 120 --unix-socket /run/lez/taker/node.sock \
  -H 'Content-Type: application/json' --data \
  '{"jsonrpc":"2.0","id":1,"method":"taker_health","params":[{"schema_version":1}]}' \
  http://localhost/
```

For a standalone deployment, use its configured socket path and execute as
the node owner. The socket and its directory are owner-restricted. This is a
publicly documented API, **not an unauthenticated public-network API**. An
operator granting a third-party program socket access grants owner-level
control, including fund-moving methods. There are no per-strategy scopes or
remote API keys. Keep each role's credentials and sockets separate.

The [Python 3.10+ example](../../examples/owner-api/owner_rpc.py) uses only the
standard library and supports both nodes:

```python
from owner_rpc import OwnerRpc, RpcError

maker = OwnerRpc("/run/lez/maker/node.sock")
taker = OwnerRpc("/run/lez/taker/node.sock")
health = taker.call("taker_health", {"schema_version": 1})
offers = taker.call("taker_offer_list_v1", {"schema_version": 1, "route": None})
```

Put `examples/owner-api` on `PYTHONPATH` or copy the module into your application
under this repository's MIT/Apache-2.0 licensing. The example has a 64 KiB
request/response budget, exact integer decoding, and no automatic retries. It
is a wire example, not a strategy SDK. Long initiation or claim operations can
outlive a client timeout. Closing a connection does not cancel durable work.

## Compatibility and value conventions

- Methods below form the documented extension subset. Other owner/debug/Chat
  methods are not an invitation to orchestrate private signing steps yourself.
  Existing names without `_v1` are retained as part of this baseline; the
  suffix is not retroactively added to them.
- Breaking semantics require a new method version and a documented migration.
  Additive response fields and new capability/state values may appear. Ignore
  unknown response fields, and stop dependent actions on unknown states or
  capabilities. Do not silently fall back to unguarded publication if a node
  lacks `maker_offer_publish_v1`.
- Supply `schema_version: 1` only where shown. New guarded publication and
  taker request types reject unknown fields. Legacy maker methods do not all
  reject extra fields; send only the fields in this contract.
- A BTC route is `{"pair":"Bitcoin","direction":"TakerSellsForeign"}`
  (taker pays BTC, receives LEZ) or direction `"TakerSellsLez"` (taker pays
  LEZ, receives BTC). Route enum strings are case-sensitive. A Monero route
  exists in one direction only, `{"pair":"Monero","direction":"TakerSellsLez"}`
  (taker pays LEZ, receives XMR; [A Monero swap](#a-monero-swap)).
- All amounts are **atomic-unit integers**. BTC uses satoshis, ZEC zatoshis,
  XMR piconero (10^-12 XMR, so 1 XMR is `1000000000000`). LEZ amounts use
  the configured asset's atomic units, not a presumed display denomination.
  `foreign_units` is u64; `expected_lez_units`/`lez_units` are u128 JSON numbers.
  Use an integer-capable JSON encoder/decoder. JavaScript `Number` is not safe
  above 2^53−1; do not stringify amounts into JSON strings or round them.
- Price is a reduced positive integer ratio:
  `lez_units = foreign_units * lez_units_per_lot / foreign_units_per_lot`.
  Lot components must be coprime and no greater than 2^63−1. Selected amounts
  must divide exactly and lie inside the signed foreign-unit bounds; the
  node rejects nonintegral quotes. Price orientation does not flip with route
  direction. The application decides what spread this ratio represents.
- Use durable unique ASCII `request_id` values (8–64 characters; letters,
  digits, `.`, `_`, `-`) and `offer_id` values (8–64 with that grammar).
  Keep IDs and original mutation bodies in your application's durable state.
  Treat returned swap IDs as opaque strings.
- Maker identity and SHA-256 commitments are JSON byte arrays (33 and 32 bytes),
  not hex strings. Copy authenticated values intact from discovery.

## Maker method reference

Each request column describes the single object inside `params`. Fields are
required unless marked optional. `null` revisions on insertion are intentional.

| Method | Request | Result / behavior |
|---|---|---|
| `maker_health` | `{}` | `{schema_version, ready, degraded, delivery, chat, routes}`, plus `lez_chain` on a Node with the Node-owned lifecycle ([LEZ chain progress](#lez-chain-progress-in-health)); dependency states are `disabled`, `available`, `unavailable`. `ready` alone does not prove a route or wallet is usable; `degraded` is true while a configured dependency is unavailable or `lez_chain.degraded` is. |
| `maker_wallet_balances_v1` | `{}`; optional `zcash:true` | What this Maker's own wallets hold, read-only: `bitcoin` (`state`, `wallet`, `network`, `trusted_sat`, `untrusted_pending_sat`, `immature_sat` from its Core wallet's `getbalances`) and `lez` (`state`, `owner_account_hex`, `owner_account_base58`, `balance_atomic_units`, `nonce` from the indexer). `state` is `available`, `unavailable` or `disabled`; amounts are exact integers and zero when not available. Coins locked by a planned, not yet broadcast funding are not trusted. With `zcash:true` on a Node whose Zcash role is configured, also `zcash` ([what a Zcash funder holds](#what-a-zcash-funder-holds)). It has no Monero field: the Maker's Monero role wallet is read with `monero-wallet-rpc` itself (on the devnet `scripts/monero-lane.py balance --role maker`). |
| `maker_pair_list` | `{}` | Array of `{revision, value}`; `value` is the route configuration below. |
| `maker_local_price_list` | `{}` | Array of `{revision, value}`; `value` is the local ratio below. |
| `maker_price_quote` | `{route}` | `{price, source_revision, observed_at_unix_seconds}` from the selected source; this is a price observation, not a strategy decision or funds reservation. |
| `maker_local_route_save_v1` | `{request_id, expected_pair_revision, expected_price_revision, configuration, price}` | `{pair_revision, price_revision, was_replay}`. Saves both rows atomically; use returned revisions to publish. `null` means insert-only for that row; otherwise supply its current revision. |
| `maker_offer_publish_v1` **new** | `{schema_version:1, request_id, offer_id, route, expected_pair_revision, expected_price_revision}` | `{revision, was_replay}`. Publishes exactly those positive local-policy and price revisions, or conflicts; it never substitutes newer terms. |
| `maker_offer_list` | `{}` or `{states:[…]}` | Array of `{revision, status, offer, reservation_id, swap_id}`; statuses are `active`, `expired`, `reserved`, `consumed`, `withdrawn`. `states` keeps only offers in those statuses, judged after expiry is projected; a Node keeps every offer it ever published, so a caller that renders only open offers should ask for them (the desks ask for `active`, `reserved`, `consumed`). History is durable; lists are not a transactional balance snapshot. |
| `maker_offer_withdraw` | `{request_id, offer_id, expected_revision}` | `{revision, was_replay}`. Withdraws an unreserved offer. A reservation that wins the race prevents withdrawal. |
| `swap_history` | `{}` | Array of `{id, pair, direction, phase, requires_attention, pending_alerts, highest_alert_severity}`. Use each opaque `id` to monitor its actor. |
| `maker_actor_monitor_v1` | `{id}` | Actor snapshot with `schema_version`, `swap_id`, `actor_kind`, `lease_generation`, `schedule_state`, `attempt_count`, `last_failure_class` (why the supervisor last backed off or gave up; a `failed` actor is never polled again until an action is queued), `last_outcome` (the supervisor's last attempt and what it waits for, as on the Taker's view: [The actor's last outcome](#the-actors-last-outcome)), `attention_reason` (why the row needs the owner, when the actor said so by name: `lez_depositor_balance_insufficient` while the Node's account holds less LEZ than a Zcash swap's lot; the next attempt funds the escrow by itself once the account is funded; or `lez_revealing_claim_held_near_refund` (a stable identifier; the held claim is the Maker's follow-up LEZ claim of a swap in which the Taker sells LEZ, [ADR 0216](../architecture/0216-zec-taker-reveals-first-in-both-directions.md)) with `revealing_claim_held_by` naming the swap whose LEZ claim is in flight: the Node sends one LEZ claim at a time until it is final, the earliest LEZ refund first, and a held swap within a third of its LEZ refund delay of its own refund time (4 minutes on the devnet profile, 10 under a 30-minute delay) says so; it claims by itself once the holder's claim is final. To find the actor's marker the Node re-reads and re-hashes the actor's pinned configuration on every poll of a Zcash swap; fine on a devnet, a cached read on a busy Node), `progress`, `manual_action` and, once the Node holds the countersigned BTC agreement, `terms` (see below). See the linked DTO for complete fields. This method can reconcile a terminal actor into the operator projection. |
| `maker_actor_claim_v1`, `maker_actor_refund_v1` | `{request_id, id, expected_generation}` | `{schema_version, swap_id, action, requested_after_generation, was_replay}`. Use the monitor’s `lease_generation` as `expected_generation`. Admission is generation-fenced; it requests node-owned actor work, not arbitrary transaction construction. |
| `maker_actor_repin_v1` | `{request_id, id, expected_generation}` | `{schema_version, swap_id, actor_program_sha256, was_replay}`. Each swap pins the SHA-256 of the actor executable it was created with, so a changed binary never acts on it unnoticed; after a Node upgrade an open swap therefore stops as `failed: actor_deployment_invalid`. This is the owner's explicit decision to move that one swap to the actor the Node runs now, and it queues the swap again. Refused while the actor is leased or terminal. Recorded in the Maker's mutation journal under its `request_id`. |

`configuration` has `route`, `enabled` (boolean), `price_source` (`"local"`
for strategy decisions), `minimum_foreign_units`, `maximum_foreign_units`,
`offer_ttl_seconds` (1–86400). Bounds are positive and maximum ≥ minimum.
`price` has `route`, `lez_units_per_lot`, `foreign_units_per_lot`.
Both routes must match. Disabling a route prevents new publications; it does
not atomically cancel already published or reserved offers.

An `offer` contains immutable `id`, `pair_configuration`, `price`,
`pair_configuration_revision`, `price_source_revision`,
`price_observed_at_unix_seconds`, optional `price_source_identity_sha256`,
`created_at_unix_seconds`, and `expires_at_unix_seconds`.
The signed offer represents one reservation, even when it permits a range of
amounts; it is not a reusable order with partial-fill accounting.

Complete Rust wire definitions:
[maker requests, views and registrations](../../crates/maker-node/src/lib.rs),
[guarded publication](../../crates/maker-node/src/extension_api.rs),
[health](../../crates/maker-node/src/rpc_contracts.rs),
[configuration](../../crates/swap-store/src/maker_application.rs),
[offers](../../crates/swap-store/src/maker_offer.rs).

### Publish your application's exact decision

1. Read `maker_pair_list` and `maker_local_price_list`, matching **both** pair
   and direction. Use current revisions, or `null` for absent rows.
2. Submit the final terms your application computed. For a fresh route:

```json
{
  "jsonrpc": "2.0", "id": 1, "method": "maker_local_route_save_v1",
  "params": [{
    "request_id": "example-route-001",
    "expected_pair_revision": null, "expected_price_revision": null,
    "configuration": {
      "route": {"pair":"Bitcoin","direction":"TakerSellsForeign"},
      "enabled": true, "price_source": "local",
      "minimum_foreign_units": 1000, "maximum_foreign_units": 1000000,
      "offer_ttl_seconds": 300
    },
    "price": {
      "route": {"pair":"Bitcoin","direction":"TakerSellsForeign"},
      "lez_units_per_lot": 1, "foreign_units_per_lot": 1000
    }
  }]
}
```

3. If the result is `{"pair_revision":1,"price_revision":1,"was_replay":false}`,
   use those revisions, not a new read or a guess:

```json
{
  "jsonrpc": "2.0", "id": 2, "method": "maker_offer_publish_v1",
  "params": [{
    "schema_version": 1, "request_id": "example-publish-001",
    "offer_id": "example-offer-001",
    "route": {"pair":"Bitcoin","direction":"TakerSellsForeign"},
    "expected_pair_revision": 1, "expected_price_revision": 1
  }]
}
```

Result: `{"revision":1,"was_replay":false}`. A concurrent edit to either
configuration row produces conflict `-32009` without creating an offer. Reread
and reevaluate your decision before making a new request. Saving configuration
and publishing are two commits: a failed publish leaves the saved settings.
Coordinate applications using the same route; there is no per-strategy namespace.

To withdraw that still-unreserved offer:

```json
{
  "jsonrpc":"2.0", "id":3, "method":"maker_offer_withdraw",
  "params":[{"request_id":"example-withdraw-001","offer_id":"example-offer-001","expected_revision":1}]
}
```

For replacement, reconcile withdrawal before publishing a new offer ID. This
creates a possible liquidity gap and is not an atomic replace. If acceptance
already won, manage that swap; do not assume its capital is available again.
Updating configuration never reprices an existing signed offer. Reserved and
consumed offers cannot be withdrawn to undo a swap. Keep recovery running when
your strategy stops quoting.

The legacy `maker_offer_publish` accepts `{request_id, offer_id, route}` and
reads the currently selected local or Logos C API price source
([writing a Logos price module](../price-module-guide.md)). It has no
reviewed-revision guarantee. `maker_pair_configure` and `maker_local_price_set`
remain available for separate configuration changes; external strategies
should prefer the atomic route-save operation and guarded publication above.
To apply a spread to reference data, obtain the reference in your application
and submit the resulting local ratio. Guarded publication does not run a Logos
feed and a strategy transform inside the node.

### LEZ chain progress in health

Both health methods carry `lez_chain` once the Node runs the Node-owned
lifecycle, read from the indexer and sequencer its sidecars use (`lez.indexer_url`,
`lez.sequencer_url` of the role configuration). It is an additive field of
schema 1: absent on a Node without the lifecycle and on older Nodes, and a
client that does not know it ignores it.

```text
"lez_chain": {
  "degraded": true,
  "reason": "lez_finality_stalled",
  "finalized_block_id": 84,
  "finalized_advanced_at_unix_seconds": 1760413500,
  "finalized_flat_seconds": 104400,
  "tip_block_id": 1286,
  "stall_threshold_seconds": 600,
  "observed_at_unix_seconds": 1760517900
}
```

- `finalized_block_id`: the indexer's `getLastFinalizedBlockId`, the last one
  known while the indexer does not answer; `null` before its first answer.
- `finalized_advanced_at_unix_seconds`: when this Node first saw that id;
  `finalized_flat_seconds`: how long ago that was at `observed_at_unix_seconds`.
- `tip_block_id`: the sequencer's `getLastBlockId`, `null` when it did not answer.
- `degraded` and `reason`: `lez_finality_stalled` once the finalized id has not
  changed for `stall_threshold_seconds` while the tip moved past where it was
  when finality last moved, or could not be read (the rule of the devnet's
  finality heartbeat; a chain whose tip stands still too is idle, not stalled);
  `lez_indexer_unreachable` while the indexer does not answer. `reason` is
  absent while the chain moves. Reasons are additive `snake_case` strings:
  tolerate one you do not know.

Every lock, claim and refund waits on LEZ finality, so a degraded chain
degrades the Node: `degraded` is true and the desks show one line above the
swaps ("LEZ finality has not advanced for N min — swaps wait until it does").
The threshold is `--lez-finality-stall-seconds` on `lez-maker-node` and
`lez-taker-node` (10–86400, default 600: ten blocks of the public testnet); the
devnet passes its heartbeat's `LEZ_HEARTBEAT_STALL_SECONDS` (60 s) through
`LEZ_NODE_FINALITY_STALL_SECONDS`. Nothing polls in the background: a health
call samples the chain (two reads, side by side, 3 s timeout each) at most once
every 5 s and otherwise answers from the last sample, so "when it advanced" is
as precise as health is asked (the desks ask every 10 s), and a Node started on
a chain that is already frozen reports the stall one threshold after its first
sample.

### What a Zcash funder holds

The role that funds Zcash (the Maker when the Taker sells LEZ, the Taker when it
sells ZEC) needs a mature transparent output paying its own key, or its take is
refused with a message naming the address. `zcash` (`lez-maker-cli balances
--zcash`, `lez-taker-cli balances --zcash`; an additive field of schema 1,
present only when asked for and the Zcash role is configured; it costs one pass
over the blocks the funder search reads) is `{state, address, tip_height,
scan_from_height, min_confirmations, outputs}`: `state` `available` or
`unavailable`, `address` the role's P2PKH address to pay, and `outputs` at most
eight outputs paying it from `scan_from_height` up, each `{transaction_id,
output_index, value_zatoshis, confirmations, mature, committed}` (`mature`: deep
enough to fund a reservation; `committed`: a reservation or swap of this Node
holds it already).

### The actor's last outcome

A step the actor holds on purpose used to read like a dead Node: a Taker's
Bitcoin refund held until finalized LEZ time passed the Maker's lock cutoff
showed `refund_in_progress` and nothing else for hours (#103 item 6). The
Taker's swap view and `maker_actor_monitor_v1` therefore carry `last_outcome`,
an additive field of schema 1 (absent on older Nodes; a client that does not
know it ignores it):

```text
"last_outcome": {
  "at_unix_seconds": 1762876700,
  "step": "recover",
  "outcome": "awaiting_observation",
  "revision": 1,
  "waiting_for": {
    "kind": "lez_finality_past_cutoff",
    "chain": "lez",
    "until_unix_seconds": 1762877100,
    "chain_unix_seconds": 1762876680
  }
}
```

- `at_unix_seconds`: when the Node got the outcome; `step`: the actor step
  that ran (`activate`, `drive`, `claim`, `recover`, or `status` when the
  actor could not be read); `revision`: the actor's revision after it.
- `outcome`: the actor's own word (`awaiting_observation`,
  `observed_then_projected`, `converged_on_existing_projection`, `submitted`,
  `awaiting_deadline`, …), or `failed` with `failure_class` (the shared
  failure classes; a Bitcoin actor's are `actor_observation_unavailable`,
  `actor_agreement_binding_invalid`, `actor_store_unavailable`,
  `actor_projection_unavailable`, `actor_not_activated`,
  `actor_material_unavailable`, `actor_configuration_unavailable`,
  `actor_agreement_unavailable`). `operation` names the step a Zcash outcome
  is about (`lez_refund`, `zcash_refund`, …).
- `waiting_for`, when the actor held a step for a reason it can name: `kind`,
  the `chain` whose progress ends the wait, and the bounds it read. Every
  `chain_*` value is the chain's own reading at that attempt (finalized LEZ
  time, Bitcoin median time or stable-tip height), never the Node's clock.
  Kinds: `lez_finality_past_cutoff` (a first-lock refund is held until
  finalized LEZ time passes the Maker's lock cutoff `until_unix_seconds`, the
  only proof its LEZ lock can no longer land; `chain_unix_seconds` is the
  finalized LEZ time), `bitcoin_time_past_cutoff` (the same on Bitcoin, by
  the stable tip's median time, with `chain_height`),
  `bitcoin_lock_confirmations` (the Maker's Bitcoin lock is seen with
  `confirmations` of `required_confirmations`), `bitcoin_refund_maturity` (the
  Bitcoin refund's relative timelock: first valid height `until_height`, stable
  tip `chain_height`), `bitcoin_refund_confirmations` (the refund is sent and
  confirming), `lez_finality_past_refund_time` (a LEZ refund held until
  finalized LEZ time passes the agreement's LEZ refund time), `maker_lock` (the
  Taker waits for the Maker's lock until its cutoff), `counterparty_claim` (the
  other party's claim, until the claim window closes when the agreement bounds
  it), and for Zcash `lez_refund_time` and `zcash_refund_height` (no bound:
  the actor does not report the height). Kinds are additive `snake_case`
  strings: show one you do not know by its bounds.

The facts are the actor's: the Bitcoin actor names the wait beside the
decision that holds the step (its effect output gains an optional `wait`),
and the Zcash supervisor bounds `awaiting_deadline` by the LEZ refund time in
the actor's status. Nothing decides on them; the Node only reports them. The
Node keeps the last outcome per swap in memory from its own attempts (the
Taker's Bitcoin observer every `--observer-sweep-seconds`, the Zcash
supervisor every 5 s, the Maker's supervisor on each attempt, and an owner's
claim or refund), so it is absent after a restart until the next attempt, on a
finished swap, and once the actor's revision moved past the attempt. The
desks show the wait in plain words ("Waiting for LEZ finality to pass the lock
cutoff 16:05 (now 15:58)", "Refund matures at Bitcoin height 1234 (now
1229)") and, while the outcome is under 5 minutes old and a bound it names is
not reached, offer no Resubmit.

## Taker method reference

| Method | Request object | Result / behavior |
|---|---|---|
| `taker_health` | `{schema_version:1}` | Health, registered methods and per-route capability rows, plus `lez_chain` with the Node-owned lifecycle ([LEZ chain progress](#lez-chain-progress-in-health)), which also sets `degraded`, and with `--route-health-config` (the Maker's format) `routes`: `[{route, state}]`, `available` or `unavailable` per probed route; an unavailable one sets `degraded` and a new take on it answers -32010 `chain_unavailable` with its `pair`. Check them before depending on initiation/monitoring/actions. |
| `taker_wallet_balances_v1` | `{schema_version:1}`; optional `zcash:true`, `monero:true` | What this Taker's own wallets hold, the same shape as `maker_wallet_balances_v1`: its Core wallet (`bitcoin`) and its LEZ owner account (`lez`), and with `zcash:true` its Zcash role's `zcash`; with `monero:true`, on a Node whose Monero role is configured, its Monero role wallet's `monero`: `{state:"available", address, balance_piconero, unlocked_piconero, daemon_height}` or `{state:"unavailable"}` while its wallet-rpc or monerod does not answer (absent without a Monero role). Registered with the BTC lifecycle. |
| `taker_offer_list_v1` | `{schema_version:1, route:null}` or an exact route | `{schema_version:1, offers:[{offer, maker_identity, signed_envelope_sha256}]}` from the node's authenticated discovery source. |
| `taker_swap_initiate_v1` | `{schema_version:1, request_id, offer_id, route, maker_identity, signed_envelope_sha256, foreign_units, expected_lez_units}`; optional `logos_offer_announcement_base64`, `funding_transaction_hex` | `{schema_version:1, swap, was_replay}`. Revalidates selected offer and exact amounts; BTC dynamic configuration drives reservation, preparation, ceremony and actor activation. It can reserve capital/prepare signing state; it is not a dry-run quote. |
| `taker_swap_list_v1` | `{schema_version:1}` | `{schema_version:1, swaps:[...]}`; includes recoverable persisted swaps. A swap whose own state cannot be read while others' can (its monitor answers `taker_monitor_unavailable`) is listed as `attention_required` with that `attention_reason` instead of failing the list. |
| `taker_swap_monitor_v1` | `{schema_version:1, swap_id}` | One `swap` projection as defined below. |
| `taker_swap_lock_v1` | `{schema_version:1, swap_id}` | `{schema_version:1, swap_id, chain, transaction_id, was_replay}`. Executes the role's first lock: selling Bitcoin (`TakerSellsForeign`) it broadcasts the funding transaction planned at reservation (`chain: "bitcoin"`); selling LEZ (`TakerSellsLez`) it has the swap's sidecar prepare the escrow and submits its initialization and funding at once (`chain: "lez"`, the funding transaction id). Both Bitcoin directions are capability rows of `taker_health`. This method uses the per-swap durable lock state; it takes **no request ID or generation**. |
| `taker_swap_claim_v1`, `taker_swap_refund_v1` | `{schema_version:1, request_id, swap_id, expected_generation}` | `{schema_version:1, swap_id, action, requested_after_generation, was_replay}`. Requests only the currently admissible action; `expected_generation` fences a *new* request against a stale view. A replay of an admitted `request_id` (same swap, same action) is served with the admitted generation whatever `expected_generation` it is re-sent with: a desk's "Retry" is that replay. For a Zcash swap the reply is the admission even when the Node's first attempt at the action was refused (the funding observation reads unstable while a block lands): the Node's supervisor retries it and `swap.attention_reason` names the class meanwhile; a Bitcoin action's first attempt is the reply. Poll for terminal settlement after admission: a Zcash swap's admitted action is run by the Node's actor supervisor on its next attempt (within 5 s; after a failure the row backs off 30 s and names the class in `last_failure_class`), so one press is enough and a Retry is the same replay. |

`swap` contains `schema_version`, `swap_id`, `offer_id`, `route`,
`foreign_units`, `lez_units`, `progress_generation`, `state`,
`available_action` (`claim`, `refund`, or `null`), `attention_reason` (`lez_depositor_balance_insufficient` while the account cannot fund a Zcash swap's LEZ lot, the supervisor's next attempt locks by itself once it can; or the failure class of a Zcash swap whose supervision stopped, `actor_deployment_invalid`, `lez_first_lock_replan_exhausted` (the LEZ lock's signer nonce was taken by another swap of the same account five re-plans in a row; nothing is on the chain) or `lez_escrow_already_initialized` (the swap's LEZ escrow account already exists, initialized by a transaction that is not this swap's, e.g. another swap with the same swap id: the escrow initialization can never execute, and once the sidecar refused it five times over at least 60 s it is no longer submitted; also the Maker's `last_failure_class` when it is the LEZ depositor) or `lez_revealing_claim_replan_exhausted` (reported only by an actor built before 0.3.4; a backoff since then, and the claim is re-planned again), until a claim or refund is queued; or `lez_revealing_claim_held_near_refund` with `revealing_claim_held_by` naming the holding swap, while this swap's revealing LEZ claim waits behind that swap's (one in flight per Node until it is final) within a third of its LEZ refund delay of its own refund time (4 minutes on the devnet profile); the supervisor still attempts it every 5 s and claims once the holder's claim is final), `last_failure_class` (why the Node's supervisor last backed off on a Zcash swap's actor; for a Bitcoin swap, the class of the actor's last attempt when it failed, e.g. `actor_observation_unavailable`), `last_outcome` (what the actor last did or why it did not act, and what it waits for: see [The actor's last outcome](#the-actors-last-outcome)), `status_detail` (a Zcash swap: one human-readable sentence on what holds its LEZ lock back, when the Node knows: the escrow initialization that can never execute, with `lez_escrow_already_initialized`; or a LEZ lock step submitted at least 10 times over at least 120 s and not included yet, for no reason the chain shows, while the Node keeps submitting it), `actor_phase` (a Zcash swap's Taker actor phase, e.g. `both_legs_locked`), `claim_window_closed` (selling ZEC: `true` once the Taker's revealing LEZ claim window closed with nothing sent; the row then offers `refund` and never the claim, and a desk names the state "Claim window closed"), `observed_at` (present only when the row was answered from the supervisor's last status snapshot because the actor was busy; a snapshot older than 60 s is not served), `privacy_guidance` and,
once the swap's BTC agreement is bound, `terms`: the countersigned schedule
and amounts, so an application can show deadlines without parsing the
agreement itself. `terms` is absent (not `null`) before the agreement exists
and on other pairs. Its fields are `bitcoin_value_sat` (u64), `lez_amount`
(u128), `required_bitcoin_confirmations`, `bitcoin_refund_height` (the height
at which the Bitcoin refund script path opens), and three unix-second
instants: `maker_second_lock_cutoff_unix_seconds` (the Maker may not place its
second lock after this), `earlier_refund_latest_unix_seconds` (the Maker's
leg, locked second, must be refunded by this) and
`later_refund_earliest_unix_seconds` (the Taker's leg may be refunded from
this). The same object appears on `maker_actor_monitor_v1`.

The Taker's view carries `bitcoin_lock_fee_sat` when this role funds the Bitcoin lock: what the planned lock pays the miners, known from the take on so it can be shown before the lock is sent (a lock can never be fee-bumped).

Both views also carry `effects` once something landed: the actor's durable
evidence in order, each `{revision, kind, chain, transaction_id,
confirmations}` with `kind` one of `taker_lock`, `maker_lock`,
`revealing_claim`, `followup_claim`, `maker_refund`, `taker_refund`, and on
the Maker additionally its LEZ lock steps `lez_initialize` and `lez_fund`
(`revision` 0) when it funds the escrow. `chain` is `Bitcoin` or `Lez`; `confirmations` is what the
actor observed when it recorded the evidence and stays 0 for LEZ. The array
is omitted while empty. Nothing in it is secret; it is what a block
explorer shows. A claim or refund this role has **sent** but not yet observed
final is listed too, with `pending: true` and `revision` 0: on a public network
that wait is an hour of LEZ finality or however long Bitcoin takes to confirm,
and without the entry a role that is waiting reads the same as one that never
acted. It turns into an ordinary entry once final.
States include `initiating`, `not_activated`, `awaiting_first_lock`,
`awaiting_second_lock`, `both_legs_locked`, `claim_available`,
`refund_available`, `claim_in_progress`, `refund_in_progress`, `completed`,
`refunded`, `attention_required`. `awaiting_second_lock` carries
`available_action: "refund"` only once the Maker's second-lock cutoff
(`terms.maker_second_lock_cutoff_unix_seconds`) has passed; before it the
Maker's lock is still awaited and no action is offered. `both_legs_locked` with no
`available_action` means the claim window has closed (the earlier refund's
deadline passed): a claim can no longer be included, the Node follows the
Maker's refund and then offers `refund`; an earlier admitted `claim` no longer
hides that. A successful action response is not proof
of `completed` or `refunded`. Unknown state or `attention_required` requires
reconciliation; do not guess a terminal action.

Lifecycle registration depends on node configuration. A read-only node may
answer health/discovery but return method-not-found for lifecycle calls.
`owner_cli_or_demo`, `not_on_this_node`, or an unfamiliar capability is not
full lifecycle support. `taker_swap_lock_v1` is registered with BTC lifecycle;
check the route capability as well as method availability.

Complete DTOs: [taker facade](../../crates/taker-node/src/taker_facade.rs).
Registration and errors:
[service](../../crates/taker-node/src/taker_service.rs),
[lifecycle](../../crates/taker-node/src/taker_service/lifecycle.rs).

Taker CLI: `lez-taker-cli [--socket /run/lez/taker/node.sock]` calls one method
per command, prints its JSON result, and on an error prints its code, message and
`data`. `swap take` fills the offer's route, identity, digest and exact quote from
the Node's listing. A timed-out step is safe to repeat with the same arguments.

```sh
lez-taker-cli swap offers --pair bitcoin --direction taker-sells-foreign   # or --pair zcash
lez-taker-cli swap take --request-id take-1 --offer-id OFFER --foreign-units 100000
lez-taker-cli swap lock --swap-id SWAP     # Bitcoin only; a Zcash or Monero Node locks by itself
until lez-taker-cli swap monitor --swap-id SWAP | grep -q '"state":"claim_available"'; do sleep 10; done
lez-taker-cli swap claim --swap-id SWAP --request-id claim-1 --expected-generation GEN
```

SWAP is the take reply's `swap.swap_id`, GEN the last monitor reply's
`progress_generation`; `swap refund` takes the claim's flags. A Node without a Bitcoin wallet answers the take with -32018 and the
address and amount: repeat the take with `--funding-transaction-hex` set to your
signed, unsent payment. `swap list` and `balances` read the Node.

### Accept an authenticated offer

The application chooses an entry returned by `taker_offer_list_v1` and an exact
in-bounds amount. This is a wire example after that choice, not a selection
strategy or an automatic trading loop:

```python
view = selected_offer  # retain the complete authenticated discovery entry
offer = view["offer"]
price = offer["price"]
numerator = foreign_units * price["lez_units_per_lot"]
lez_units, remainder = divmod(numerator, price["foreign_units_per_lot"])
if remainder:
    raise ValueError("selected amount is not an exact lot")

request = {
    "schema_version": 1, "request_id": "example-take-001",
    "offer_id": offer["id"], "route": offer["pair_configuration"]["route"],
    "maker_identity": view["maker_identity"],
    "signed_envelope_sha256": view["signed_envelope_sha256"],
    "foreign_units": foreign_units, "expected_lez_units": lez_units,
}
# Persist this exact request before sending it.
commit = taker.call("taker_swap_initiate_v1", request)
swap_id = commit["swap"]["swap_id"]
```

The service normally uses its configured authenticated Delivery view. A client
already connected to the live Logos announcement path may supply the exact
signed `logos_offer_announcement_base64`; the node verifies it and its binding
to the reviewed offer. Do not fabricate or reserialize a signed envelope.
This optional field does not configure a discovery feed for the node.

When the owner/application authorizes the first lock:

```python
locked = taker.call("taker_swap_lock_v1", {"schema_version": 1, "swap_id": swap_id})
```

Monitor with `taker_swap_monitor_v1`. If it reports `available_action: "claim"`
and your application authorizes revealing the claim, send:

```json
{
  "jsonrpc":"2.0", "id":4, "method":"taker_swap_claim_v1",
  "params":[{"schema_version":1,"request_id":"example-claim-001","swap_id":"RETURNED_SWAP_ID","expected_generation":4}]
}
```

Replace the placeholder swap ID and generation with the observed values.
For a reported refund action use `taker_swap_refund_v1` and a separate durable
request ID. Poll after admission: canonical chain observations and recovery
deadlines determine when funds actually settle. The maker's corresponding
settlement actions are normally driven by its supervisor.

A Zcash swap in which the Taker sells ZEC reports one action, the claim, while
both legs are locked (`claim_available`). A Taker that does not claim may still
send `taker_swap_refund_v1` at that generation: the Node admits it and runs the
agreement-ordered recovery, which waits for the Maker's LEZ refund and the
Zcash refund height before it submits the Taker's HTLC refund; before that
height it waits (`awaiting_deadline`) and broadcasts nothing. One action is
admitted per swap, so pressing Refund in the claim window gives up the claim:
a later claim request at that generation is refused as a conflict. Once the
Node has submitted the swap's revealing LEZ claim the refund is refused: the
claim has revealed the secret, so the Node pursues it (re-planned at the live
nonce each time another transaction took its nonce, at most once per attempt,
for as long as it can land; past five re-plans the actor reports the wait
`lez_claim_nonce_contention`) and never gives it up for a refund.
The claim window closes at the agreement's LEZ refund time minus the profile's
required margin. After it, a revealing claim that is not already on LEZ is
never prepared or sent: the row reports `refund_available` with `refund` as
its action instead of the claim, a claim request is refused, and a claim
admitted inside the window but never sent runs as the swap's recovery
(`lez_claim_window_closed` names a claim the actor refused). A claim sent
inside the window is still followed; it must still be included before the LEZ refund time, so the profile's required margin has to exceed the network's inclusion latency (30 s on the devnet's 10 s blocks, a parameter to size per network). When the Taker sells ZEC
it is the LEZ claimant and the Zcash funder, so its settlement after the
window is its Zcash refund.

A Zcash swap in which the Taker sells LEZ follows the same rule with the
chains swapped ([ADR 0216](../architecture/0216-zec-taker-reveals-first-in-both-directions.md)):
the Taker holds the preimage and its claim is the revealing claim of the
Maker-funded Zcash output; the Maker's actor reads the preimage from that
claim on first sight, in the mempool or a block, and claims the Taker's LEZ
by itself. The claim is offered only after the
Maker's funding reaches the profile's Zcash confirmations, and its window
closes when no more than the profile's reveal margin of blocks is left
before the Zcash refund height, or at the LEZ refund time minus the
required margin, whichever comes first. After it, the Taker's settlement is its
LEZ refund at the LEZ refund time; it does not wait for the Maker's Zcash
refund, which opens earlier and only the Maker can sign. A refund request
before the LEZ refund time waits (`awaiting_deadline`) and broadcasts
nothing.
Bitcoin swaps admit only the reported action.

### A Monero swap

Monero runs on the Docker devnet only, against Monero regtest (the role
configuration accepts no other profile; [deployment guide](../../deploy/README.md#monero-lez_monero1)).
The Taker sells LEZ and locks first; the protocol, its windows C, R and P and
the punish branch are in [protocol-design.md](../milestone-1/protocol-design.md#xmrlez).
`taker_health` lists the Monero row with `supported_direction:
"TakerSellsLez"`; once the Node has its Monero role, `initiation`
`prepared_private_material`, `monitoring` `receipt_bound` and `claim`/`refund`
`full_lifecycle`.

- **Take.** `taker_swap_initiate_v1` with the Monero offer and `foreign_units`
  in piconero agrees the swap with the Maker over Chat (the six take-time
  rounds); nothing is on either chain before the sixth.
- **Lock.** The Node locks the LEZ by itself right after the take (the escrow's
  tag 13 and its funding). `taker_swap_lock_v1` is not how a Monero swap
  locks: a call for one is refused.
- **Claim.** `available_action: "claim"` (state `claim_available`) appears
  once the Maker's Monero output is seen at 10 confirmations and unlocked,
  before R. `taker_swap_claim_v1` admits it; the Node's supervisor then sends
  the claim authorisation (tag 14), waits for the Maker's LEZ claim (tag 15),
  rebuilds the Monero spend key from it and sweeps the output into the role
  wallet.
- **Refund.** The Node refunds by itself: from R by the finalized LEZ time,
  while the Maker's LEZ claim (tag 15) has not landed, its supervisor sends
  the LEZ refund (tag 16) with no owner's command, and sends it again until
  it is final; it is valid only before P, and from P the Maker can take the
  LEZ (tag 17) and the Monero stays locked for good. `available_action:
  "refund"` is offered from R as well, and `taker_swap_refund_v1` admits it
  for the supervisor's next pass. Setting `initiation.xmr_automatic_refund`
  to `false` in the Node's configuration leaves the refund to that command.

An admitted Monero claim or refund is run by the Node's supervisor on its next
pass (`--observer-sweep-seconds`, 3 s on the devnet). The `swap` view of a
Monero swap carries `actor_phase`, and `attention_reason` and `status_detail`
when the owner is needed (`crates/taker-node/src/taker_service/xmr_supervisor.rs`):

| `actor_phase` | `state` | `available_action` | Meaning |
|---|---|---|---|
| `lez_locking` | `awaiting_first_lock` | — | the Node is locking the LEZ |
| `progress_unreadable` | `attention_required` (`xmr_progress_unreadable`) | — | the swap's progress record (`xmr-taker-progress.json`, named in `status_detail`) cannot be read; the Node does not drive the swap until it is repaired. The Node writes it whole (a temporary file, synced and renamed), so its own crash cannot cut it short |
| `lez_lock_unconfirmed` | `attention_required` (`xmr_lez_lock_unconfirmed`) | — | C passed with a lock prepared and not seen final: the Node sends nothing more and keeps looking for it on chain; a lock found (final before or after C) turns the row locked, and is refunded from R |
| `lez_lock_window_closed` | `attention_required` (`xmr_lez_lock_window_closed`) | — | no lock was prepared by C, or the finalized LEZ time reached R with nothing in the escrow: nothing is locked, the take is over |
| `taker_lock_confirmed` | `awaiting_second_lock` | — | LEZ locked; waiting for the Maker's Monero at 10 confirmations |
| `both_legs_locked` | `claim_available` | `claim` | both legs locked, before R |
| `claim_authorizing` | `claim_in_progress` | — | tag 14 sent, not yet final |
| `awaiting_maker_claim` | `claim_in_progress` | — | tag 14 final; waiting for the Maker's tag 15 |
| `sweeping` | `claim_in_progress`, or `attention_required` (`xmr_claim_sweep_in_doubt`) | — | the Maker claimed; the Monero is being swept |
| `completed` | `completed` | — | the swept output is in the role wallet |
| `refund_available` | `refund_available`, or `attention_required` (`xmr_maker_did_not_claim` from R after a tag 14 the Maker did not answer; `refund_window_closing` from R + (P − R)/2) | `refund` | the refund window [R, P) is open |
| `refund_in_flight` | `refund_in_progress` | — | tag 16 sent, not yet final; sent again, the same bytes, while the finalized LEZ time is before P |
| `refund_not_final` | `attention_required` (`xmr_refund_not_final_by_punish_time`) | — | P passed with tag 16 sent and not seen final: one that landed before P still turns the row `refunded`; otherwise the Maker can take the LEZ |
| `refunded` | `refunded` | — | the LEZ is back |
| `punish_open` | `attention_required` (`xmr_refund_window_closed`) | — | P passed without a refund: the Maker can take the LEZ; the Monero stays locked |

`xmr_claim_sweep_in_doubt` means a sweep was started and its outcome not
recorded; it is **never resent**. Its `status_detail` names the wallet file
(`xmr-<swap id>-claim-<attempt>` on the role's `monero-wallet-rpc`) that holds
the shared spend key; recovering the Monero from it is by hand (its password is
derived from the Taker's own share, `claim_wallet_password` in the same file),
no tool in the tree does it, and it has not been exercised.
`xmr_maker_did_not_claim` carries the `status_detail` "the claim was
authorised on LEZ (tag 14) and the Maker did not claim before the refund time,
so it no longer can: refund (tag 16) before the punish time, or the Maker takes
the LEZ and the Monero stays locked".

On the Maker, `maker_actor_monitor_v1`'s `progress.observation.phase` is the
Monero actor's phase: `offered`, `taker_lock_confirmed` (funding Monero),
`maker_funding_in_doubt` (a funding past C the observer has not found; the
tool's failure is in the Node log; on regtest the observer mines a funding it
finds in the pool or short of its confirmations, so this means none was
seen), `both_legs_locked`,
`claim_evidence_available` (claiming LEZ), `maker_recovery_available`
(sweeping its Monero back after a refund, or punishing after P), and the
terminal `completed`, `refunded`, `punished` and `expired` (nothing funded,
nothing at stake) (`crates/maker-node/src/bin/xmr-maker-actor.rs`). The Maker
runs every step itself; it has no Monero claim or refund to press.

The CLIs (`pair-xmr` builds). The Maker publishes a Monero offer in its one
direction; `--direction taker-sells-lez` must be given to `configure-pair`,
`set-local-price`, `quote` and `publish-offer`, whose default
`taker-sells-foreign` is refused for Monero. A price needs its route saved and an enabled route needs
its price, so a Maker that never had Monero saves the route disabled first:

```sh
lez-maker-cli configure-pair --request-id xmr-pair-1 --pair monero --direction taker-sells-lez \
  --enabled false --price-source local --minimum-foreign-units 1000000000000 \
  --maximum-foreign-units 1000000000000 --offer-ttl-seconds 3600
lez-maker-cli set-local-price --request-id xmr-price-1 --pair monero --direction taker-sells-lez \
  --lez-units-per-lot 1 --foreign-units-per-lot 20000000
lez-maker-cli configure-pair --request-id xmr-pair-2 --expected-revision REV --pair monero \
  --direction taker-sells-lez --enabled true --price-source local \
  --minimum-foreign-units 1000000000000 --maximum-foreign-units 1000000000000 --offer-ttl-seconds 3600
lez-maker-cli publish-offer --request-id xmr-publish-1 --offer-id OFFER --pair monero --direction taker-sells-lez
```

REV is the route's revision from `lez-maker-cli pairs`. The Taker:

```sh
lez-taker-cli balances --monero
lez-taker-cli swap offers --pair monero --direction taker-sells-lez
lez-taker-cli swap take --request-id take-1 --offer-id OFFER --foreign-units 1000000000000
lez-taker-cli swap monitor --swap-id SWAP
lez-taker-cli swap claim --swap-id SWAP --request-id claim-1 --expected-generation GEN
lez-taker-cli swap refund --swap-id SWAP --request-id refund-1 --expected-generation GEN
```

Each prints the Node's JSON answer on **stdout**, unchanged, and for a Monero
row also plain words on **stderr**, so a script reads stdout and a person
reads both. The rows look like this (the format of `lez-taker-cli.rs`; the
values are examples):

```text
offer OFFER: Monero, you sell LEZ for 1.000000000000 XMR to 1.000000000000 XMR (1000000000000 to 1000000000000 piconero), 1 LEZ units per 0.000020000000 XMR (20000000 piconero); take with `swap take --offer-id OFFER --foreign-units <piconero> --request-id <new id>`
swap SWAP: Monero, 1.000000000000 XMR for 50000 LEZ units: both legs are locked: you can claim the Monero
  next: swap claim --swap-id SWAP --request-id <new id> --expected-generation 3
monero: 2.000000000001 XMR (2000000000001 piconero), unlocked 2.000000000001 XMR (2000000000001 piconero), address 4…, monerod height 412
```

A swap past half its refund window prints `needs you: the refund window is
half over: refund now, or from the punish time the Maker takes your LEZ and the
Monero stays locked`, and `detail: …` when the Node gives one. `swap lock` on a
Monero swap fails and says that the Taker Node locks the LEZ by itself right
after the take. The CLIs' Monero rows are covered by unit tests in
`lez-taker-cli.rs`; the devnet scenarios publish through `lez-maker-cli` and
drive the Taker over its owner RPC, not through `lez-taker-cli`.

## Retries, errors, and observation limits

Never treat timeout, connection loss, or an HTTP error as proof of no mutation.
An offer can be durable before Delivery reports success. Retry a mutation with
its original `request_id` and unchanged body; exact replay returns the original
result with `was_replay:true`. Changing terms/guards under that ID conflicts.
Current dependencies may still block an RPC retry; reconcile the persisted
offer/swap instead of creating a fresh ID. Replaying publication after expiry,
withdrawal, or consumption does not reactivate the offer or extend its TTL.

Claim/refund requests bind the observed generation. Reusing their original
request is different from creating a new request against a stale generation.
The lock method instead resolves its recorded per-swap effect. On ambiguous
outcomes, inspect the swap and retry only according to that method's contract.
Do not loop a fund-moving action merely because an HTTP request succeeded.

| Failure | Client response |
|---|---|
| `-32601` method not found | Wrong node configuration/version; inspect health and upgrade/configure deliberately. |
| `-32602` invalid params | Fix schema, route, exact amounts, or bounds. Do not blindly retry. |
| Maker `-32009` conflict | Reused ID with different payload, duplicate offer ID, or stale revision; reconcile before a new decision. |
| Maker `-32018` unavailable, `-32004` not found | Offer/quote is no longer usable or absent; rediscover/reconcile. |
| Dependency/internal/transport failure | Outcome may be ambiguous; retain the original request and inspect state. |
| Taker `-32018` `bitcoin_funding_required` | Not a failure: this Node has no Bitcoin wallet. `error.data` carries `address` and `amount_sat`; sign (do not send) a transaction paying exactly that from native SegWit inputs, then replay the **same** take with `funding_transaction_hex`. See [Funding the lock from your own wallet](#funding-the-lock-from-your-own-wallet). |
| Taker structured error | Inspect `error.code` and `error.data.category`; examples include `taker_action_conflict`, `taker_action_unavailable`, `authenticated_delivery_unavailable`. Treat unknown categories conservatively. |

The taker error taxonomy is distinct from the maker's; do not assign a global
meaning to the same numeric code on both roles. Human error text is not a stable
machine discriminator. Nodes bound requests and results; the example client
also caps bodies at 64 KiB. The history lists have no durable event cursor or
general pagination contract. A limit/oversized result is an explicit failure,
not an empty list. Poll with bounded backoff; do not infer exactly-once event
delivery, an atomic multi-method snapshot, or unlimited history export.

### Funding the lock from your own wallet

A Taker Node whose `bitcoin.wallet` is unset — a Node pointed at a public RPC
provider, which serves no wallet — cannot fund the Bitcoin lock itself. The
contract address exists only once the Maker has reserved, so the take is two
calls with one `request_id`:

1. `taker_swap_initiate_v1` answers `-32018` with the `address` and `amount_sat`.
2. Sign a transaction paying exactly that in your wallet **without sending it**,
   and replay the same take with `funding_transaction_hex`.

The Node checks that the transaction pays the contract once and exactly, that
the mempool would accept it, that its fee passes the Node's `lock_fee` policy,
and that every input is a native SegWit spend: the refund and the claim are
signed against the lock's id before it is sent, so an id a third party can
change would strand the coins. From there the swap is the usual one —
`taker_swap_lock_v1` sends your transaction. The reservation is held only until
the offer expires, and the swap's deadlines run from the first call, so sign
promptly. Spending the inputs elsewhere before locking just fails the lock.

A Maker that funds Bitcoin (`TakerSellsLez`) still needs a wallet.

There is currently no strategy-facing wallet balance API, fee-estimation API,
atomic quote replacement, capital reservation across offers, multi-venue
hedging call, simulator, or policy scheduler. Those facts matter when deciding
which independent applications these primitives can support. See the
[proposal](proposal.md) for the responsibilities and possible follow-up APIs.
