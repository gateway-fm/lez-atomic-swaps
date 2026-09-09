# Maker and taker APIs for independent applications

Use these owner APIs to build your own maker policies, taker automation, or
operator tools. The nodes own negotiation, signing, settlement, persistence,
and recovery. Your application supplies decisions and observes their results.
Start with the [scope proposal and answers for ecosystem #176](proposal.md).

This reference describes the API implemented by this PR. Pin an integrating
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
  LEZ, receives BTC). Route enum strings are case-sensitive.
- All amounts are **atomic-unit integers**. BTC uses satoshis. LEZ amounts use
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
| `maker_health` | `{}` | `{schema_version, ready, degraded, delivery, chat, routes}`; dependency states are `disabled`, `available`, `unavailable`. `ready` alone does not prove a route or wallet is usable. |
| `maker_pair_list` | `{}` | Array of `{revision, value}`; `value` is the route configuration below. |
| `maker_local_price_list` | `{}` | Array of `{revision, value}`; `value` is the local ratio below. |
| `maker_price_quote` | `{route}` | `{price, source_revision, observed_at_unix_seconds}` from the selected source; this is a price observation, not a strategy decision or funds reservation. |
| `maker_local_route_save_v1` | `{request_id, expected_pair_revision, expected_price_revision, configuration, price}` | `{pair_revision, price_revision, was_replay}`. Saves both rows atomically; use returned revisions to publish. `null` means insert-only for that row; otherwise supply its current revision. |
| `maker_offer_publish_v1` **new** | `{schema_version:1, request_id, offer_id, route, expected_pair_revision, expected_price_revision}` | `{revision, was_replay}`. Publishes exactly those positive local-policy and price revisions, or conflicts; it never substitutes newer terms. |
| `maker_offer_list` | `{}` | Array of `{revision, status, offer, reservation_id, swap_id}`; statuses are `active`, `expired`, `reserved`, `consumed`, `withdrawn`. History is durable; lists are not a transactional balance snapshot. |
| `maker_offer_withdraw` | `{request_id, offer_id, expected_revision}` | `{revision, was_replay}`. Withdraws an unreserved offer. A reservation that wins the race prevents withdrawal. |
| `swap_history` | `{}` | Array of `{id, pair, direction, phase, requires_attention, pending_alerts, highest_alert_severity}`. Use each opaque `id` to monitor its actor. |
| `maker_actor_monitor_v1` | `{id}` | Actor snapshot with `schema_version`, `swap_id`, `actor_kind`, `lease_generation`, `schedule_state`, `attempt_count`, `progress`, `manual_action` and, once the Node holds the countersigned BTC agreement, `terms` (see below). See the linked DTO for complete fields. This method can reconcile a terminal actor into the operator projection. |
| `maker_actor_claim_v1`, `maker_actor_refund_v1` | `{request_id, id, expected_generation}` | `{schema_version, swap_id, action, requested_after_generation, was_replay}`. Use the monitor’s `lease_generation` as `expected_generation`. Admission is generation-fenced; it requests node-owned actor work, not arbitrary transaction construction. |

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
reads the currently selected local or Logos C API price source. It has no
reviewed-revision guarantee. `maker_pair_configure` and `maker_local_price_set`
remain available for separate configuration changes; external strategies
should prefer the atomic route-save operation and guarded publication above.
To apply a spread to reference data, obtain the reference in your application
and submit the resulting local ratio. Guarded publication does not run a Logos
feed and a strategy transform inside the node.

## Taker method reference

| Method | Request object | Result / behavior |
|---|---|---|
| `taker_health` | `{schema_version:1}` | Health, registered methods and per-route capability rows. Check them before depending on initiation/monitoring/actions. |
| `taker_offer_list_v1` | `{schema_version:1, route:null}` or an exact route | `{schema_version:1, offers:[{offer, maker_identity, signed_envelope_sha256}]}` from the node's authenticated discovery source. |
| `taker_swap_initiate_v1` | `{schema_version:1, request_id, offer_id, route, maker_identity, signed_envelope_sha256, foreign_units, expected_lez_units}`; optional `logos_offer_announcement_base64` | `{schema_version:1, swap, was_replay}`. Revalidates selected offer and exact amounts; BTC dynamic configuration drives reservation, preparation, ceremony and actor activation. It can reserve capital/prepare signing state; it is not a dry-run quote. |
| `taker_swap_list_v1` | `{schema_version:1}` | `{schema_version:1, swaps:[...]}`; includes recoverable persisted swaps. |
| `taker_swap_monitor_v1` | `{schema_version:1, swap_id}` | One `swap` projection as defined below. |
| `taker_swap_lock_v1` | `{schema_version:1, swap_id}` | `{schema_version:1, swap_id, chain, transaction_id, was_replay}`. Executes the role's first lock. This method uses the per-swap durable lock state; it takes **no request ID or generation**. |
| `taker_swap_claim_v1`, `taker_swap_refund_v1` | `{schema_version:1, request_id, swap_id, expected_generation}` | `{schema_version:1, swap_id, action, requested_after_generation, was_replay}`. Requests only the currently admissible action. Poll for terminal settlement after admission. |

`swap` contains `schema_version`, `swap_id`, `offer_id`, `route`,
`foreign_units`, `lez_units`, `progress_generation`, `state`,
`available_action` (`claim`, `refund`, or `null`), `privacy_guidance` and,
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
States include `initiating`, `not_activated`, `awaiting_first_lock`,
`awaiting_second_lock`, `both_legs_locked`, `claim_available`,
`refund_available`, `claim_in_progress`, `refund_in_progress`, `completed`,
`refunded`, `attention_required`. `both_legs_locked` with no
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
| Taker structured error | Inspect `error.code` and `error.data.category`; examples include `taker_action_conflict`, `taker_action_unavailable`, `authenticated_delivery_unavailable`. Treat unknown categories conservatively. |

The taker error taxonomy is distinct from the maker's; do not assign a global
meaning to the same numeric code on both roles. Human error text is not a stable
machine discriminator. Nodes bound requests and results; the example client
also caps bodies at 64 KiB. The history lists have no durable event cursor or
general pagination contract. A limit/oversized result is an explicit failure,
not an empty list. Poll with bounded backoff; do not infer exactly-once event
delivery, an atomic multi-method snapshot, or unlimited history export.

There is currently no strategy-facing wallet balance API, fee-estimation API,
atomic quote replacement, capital reservation across offers, multi-venue
hedging call, simulator, or policy scheduler. Those facts matter when deciding
which independent applications these primitives can support. See the
[proposal](proposal.md) for the responsibilities and possible follow-up APIs.
