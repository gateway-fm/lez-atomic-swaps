# CLI reference: `lez-maker-cli` and `lez-taker-cli`

Each role has one operator CLI. `lez-maker-cli`
(`crates/maker-node/src/bin/lez-maker-cli.rs`) is the Maker's, `lez-taker-cli`
(`crates/taker-node/src/bin/lez-taker-cli.rs`) the Taker's. Both ship in the role's Node
package (`deploy/scripts/package-role-node.sh`) and are installed to `/usr/bin` by the systemd
install scripts. This page lists every command of both, grouped as the RFP words them: for the
Maker (U3) configure pairs and prices, start and stop, query history, manual claims and
refunds; for the Taker (U4) offer discovery, swap initiation, progress monitoring, claim and
refund. `deploy/tests/test_cli_reference.py` fails when a subcommand or flag of either CLI is
not named here.

The methods the commands call, their parameters and their results are in the
[owner API reference](api/README.md); this page says which command calls which method and what
the CLI adds.

## What both CLIs share

**One command, one call.** A command that talks to the Node opens the Node's owner socket,
sends one JSON-RPC request, prints the result and exits. The only exception is
`lez-taker-cli swap take`, which first reads the Node's offer list to fill the request. The CLI
keeps no state of its own and never retries.

**The owner socket.** `--socket <path>` names it and goes before the command
(`lez-maker-cli --socket /tmp/m/node.sock health`). The defaults are `/run/lez/maker/node.sock`
and `/run/lez/taker/node.sock`, the paths the systemd units and the role containers use. No
environment variable is read. A Node started by the role launcher outside a container has its
socket at `$LEZ_NODE_RUN_ROOT/<role>/node.sock` (`modules/lez-role-node/node-entrypoint.sh`);
pass that path.

**Who may call.** The Node creates the socket with mode `0600` in a directory it requires to
be its own with mode `0700`, and checks both after binding (`bind_owner_socket`,
`crates/node-common/src/owner_rpc_server.rs`). There is no token and no per-command
permission: whoever can open the socket has the owner's full control, including the
fund-moving commands. Another user's `connect` is refused by the kernel, so no request
reaches the Node, and the CLI prints the cause and exits non-zero:

```text
$ lez-taker-cli health          # as another uid
connect local RPC socket /run/lez/taker/node.sock: Permission denied (os error 13)
```

`lez-maker-cli` prints the same failure in its own format (`Error: connect local RPC socket
/run/lez/maker/node.sock`, then `Caused by:` and `Permission denied (os error 13)`). Run the
CLI as the Node's user: `runuser -u lez-swap -- lez-maker-cli health` under systemd,
`docker exec lez-maker-node lez-maker-cli health` in the Compose stack. The devnet scenarios
`maker-cli` and `taker-cli` run every owner-RPC command of each CLI as uid 65534 and require
exactly this refusal, and that nothing in the Node changed (`wrong_owner_matrix`,
`deploy/scripts/node-e2e.py`).

**Output.** The result is one line of compact JSON on stdout, exactly what the Node answered.
Errors go to stderr. `lez-taker-cli` also prints plain words on stderr for Monero rows
(offers, swaps, balances); stdout stays the Node's JSON, so a script reads stdout and a person
reads both.

**Exit codes.**

| | `lez-maker-cli` | `lez-taker-cli` |
|---|---|---|
| success | 0 | 0 |
| the command failed (socket, Node error, refused input, systemd) | 1, stderr starts with `Error:` | 2 |
| the command line does not parse | 2 | 2 |
| `--help` | 0 | 0 |

A Node error is printed as `local RPC error <code>: <message>` followed by the error's `data`
as JSON when the Node sent one (its `category`, and for some errors what the caller needs,
such as a funding address). The codes are the Node's; see
[Retries, errors, and observation limits](api/README.md#retries-errors-and-observation-limits).

**Timeouts.** `lez-maker-cli` waits 30 seconds for every reply. `lez-taker-cli` waits 30
seconds for reads, 7 minutes for `swap take`, 2 minutes for `swap lock` and 5 minutes for
`swap claim` and `swap refund`. A timeout does not cancel the Node's work. Repeating the same
command is safe: the same `--request-id` replays or resumes a take, claim or refund, and a
lock repeats as the swap's one recorded lock.

**Request ids.** Every command that changes something takes `--request-id`. It is the
mutation's durable identity: the same id with the same arguments returns the first result
with `was_replay: true`; the same id with other arguments is a conflict. Use a new id for a
new decision.

**Pairs and directions.** `--pair` is `bitcoin`, `zcash` or `monero`. `--direction` is
`taker-sells-foreign` (the Taker pays the foreign asset and receives LEZ) or `taker-sells-lez`
(the Taker pays LEZ and receives the foreign asset). Bitcoin and Zcash have both directions.
Monero has `taker-sells-lez` only
([ADR 0217](architecture/0217-xmr-taker-sells-lez-deviates-from-the-common-flow.md)). The CLIs
accept all three pairs in every build; whether a pair works depends on the Node
(`pair-zec` and `pair-xmr` builds, and the role's configuration), which
`lez-taker-cli health` reports in `pair_capabilities`.

### Start and stop are systemd only

`start` and `stop` of both CLIs do not use the socket and work with or without a running Node.
They run `/usr/bin/systemctl --no-pager --no-ask-password start|stop` on the one packaged unit
(`lez-maker-node.service` or `lez-taker-node.service`), then read the unit's `ActiveState` and
require `active` after a start and `inactive` after a stop
(`crates/node-common/src/service_control.rs`). The unit name is fixed, no shell is used and
`systemctl`'s own output is discarded. Each `systemctl` call has 30 seconds. On success the
CLI prints:

```json
{"schema_version":1,"action":"stop","unit":"lez-maker-node.service","active_state":"inactive"}
```

Failures: `cannot execute the fixed systemctl program` (no systemd), `systemctl stop failed
with exit status N` (not permitted, or no such unit: run it as root), `systemctl stop timed
out; service state is uncertain`, `systemctl stop completed without reaching inactive`.

A Node hosted by Logos core (in Basecamp or under `logosctl`) is not a systemd unit, and these
two commands do not reach it. There the `lez_swap_node` module supervises the Node
(`modules/lez-swap-node/src/lez_swap_node_impl.h`):

- **Hold file.** The supervisor does not start the Node while the role's hold file exists
  (`LEZ_MAKER_NODE_HOLD`, `LEZ_TAKER_NODE_HOLD`; `/tmp/lez-node-hold` in the role containers).
  To stop: create the file, then end the Node's process group. To start: remove the file; the
  supervisor looks once a second. `deploy/scripts/node-hold.sh hold|release maker|taker` does
  both steps in the Compose stack. Basecamp, the desk and the LEZ signer keep running.
- **Module calls.** `logosctl call lez_swap_node stop_node maker` ends the Node's process
  group (SIGTERM, SIGKILL after 10 seconds) and forgets the role, so a module reload does not
  start it again. `logosctl call lez_swap_node start_node maker <launcher> <log file> <hold
  file>` supervises it again; `logosctl call lez_maker_node launcher` prints the launcher's
  path. `node_status maker` answers `{role, state, pid, starts, last_exit}` with `state` one
  of `running`, `held`, `restarting`, `stopped`.

Both modes are described in the [Maker installation guide](maker-install.md).

## Maker (U3): `lez-maker-cli`

`lez-maker-cli [--socket <path>] <command>`. Every command below needs the running Maker Node
except `delivery-identity`, `start` and `stop`.

| RFP wording | Commands |
|---|---|
| configure pairs and prices | `configure-pair`, `set-local-price`, `pairs`, `prices`, `quote`, `publish-offer`, `offers`, `withdraw-offer` |
| start/stop | `start`, `stop`, `health` |
| query history | `history`, `status`, `monitor`, `alerts`, `acknowledge-alert`, `balances` |
| manual claims and refunds | `claim`, `refund` |
| other | `delivery-identity`, `create-swap` |

### Configure pairs and prices

A route is a pair and a direction. A route needs a pair configuration and, for the `local`
price source, a local price. A price needs its route saved first, and an enabled route needs
its price, so a new route is saved disabled, priced, then enabled:

```sh
lez-maker-cli configure-pair --request-id pair-1 --pair bitcoin --direction taker-sells-foreign \
  --enabled false --price-source local --minimum-foreign-units 100000 \
  --maximum-foreign-units 200000 --offer-ttl-seconds 3600
lez-maker-cli set-local-price --request-id price-1 --pair bitcoin --direction taker-sells-foreign \
  --lez-units-per-lot 5 --foreign-units-per-lot 2
lez-maker-cli configure-pair --request-id pair-2 --expected-revision 1 --pair bitcoin \
  --direction taker-sells-foreign --enabled true --price-source local \
  --minimum-foreign-units 100000 --maximum-foreign-units 200000 --offer-ttl-seconds 3600
lez-maker-cli publish-offer --request-id publish-1 --offer-id offer-0001 --pair bitcoin \
  --direction taker-sells-foreign
```

In `configure-pair`, `set-local-price`, `quote` and `publish-offer`, `--direction` defaults to
`taker-sells-foreign`. With `--pair monero` that default is refused by the CLI before any
call: "Monero supports only --direction taker-sells-lez (the Taker locks LEZ first, the Maker
locks XMR); the default taker-sells-foreign is refused". Name `--direction taker-sells-lez`.

#### `lez-maker-cli configure-pair`

Saves one route's policy. Calls `maker_pair_configure`. Prints `{revision, was_replay}`.

| flag | meaning | default |
|---|---|---|
| `--request-id` | the mutation's id | required |
| `--expected-revision` | the route's current revision from `pairs`; omit to create the route | none (insert only) |
| `--pair` | `bitcoin`, `zcash`, `monero` | required |
| `--direction` | `taker-sells-foreign`, `taker-sells-lez` | `taker-sells-foreign` |
| `--enabled` | `true` or `false` | required |
| `--price-source` | `local` or `logos-c-api` ([price module guide](price-module-guide.md)) | `local` |
| `--minimum-foreign-units`, `--maximum-foreign-units` | the offer's bounds in foreign atomic units: sats, zatoshis or piconero | required |
| `--offer-ttl-seconds` | lifetime of an offer published on this route | required |

A stale `--expected-revision` and a reused request id with other terms answer -32009. Saving a
route never reprices or withdraws an offer already published.

#### `lez-maker-cli set-local-price`

Saves the local price of one route as a ratio of integers. Calls `maker_local_price_set`.
Prints `{revision, was_replay}`.

| flag | meaning | default |
|---|---|---|
| `--request-id` | the mutation's id | required |
| `--expected-revision` | the price's current revision from `prices`; omit to create it | none (insert only) |
| `--pair`, `--direction` | the route | `--direction taker-sells-foreign` |
| `--lez-units-per-lot` | LEZ atomic units in one lot | required |
| `--foreign-units-per-lot` | foreign atomic units in one lot | required |

The ratio must be reduced (greatest common divisor 1) and both values nonzero; the CLI refuses
others before any call.

#### `lez-maker-cli pairs`

Lists the saved pair configurations in route order. Calls `maker_pair_list`. Prints an array
of `{revision, value}`. No flags.

#### `lez-maker-cli prices`

Lists the saved local prices in route order. Calls `maker_local_price_list`. Prints an array
of `{revision, value}`. No flags.

#### `lez-maker-cli quote`

Reads the price the route's selected source gives now. Calls `maker_price_quote`. Prints
`{price, source_revision, observed_at_unix_seconds}`. It records nothing and reserves nothing.
Flags: `--pair` (required), `--direction` (default `taker-sells-foreign`).

#### `lez-maker-cli publish-offer`

Publishes one expiring offer on an enabled route at the price its source gives at that
moment. Calls `maker_offer_publish`. Prints `{revision, was_replay}`.

| flag | meaning | default |
|---|---|---|
| `--request-id` | the mutation's id | required |
| `--offer-id` | the new offer's id: 8 to 64 characters of `A-Z a-z 0-9 . _ -` | required |
| `--pair`, `--direction` | the route | `--direction taker-sells-foreign` |

The Node records the signed offer, then hands it to Delivery. With Delivery unavailable the
command fails with -32603 although the offer is already recorded; `offers` shows it. Once
Delivery is back the Node hands it over at its next `health` call, and the same request
repeated answers `was_replay: true`
([Delivery and Chat outages](delivery-chat-outage.md)). An offer id that exists
answers -32009; a route whose chain the Node reads as down is refused. Replaying the request
after the offer expired, was withdrawn or was taken does not bring it back.

#### `lez-maker-cli offers`

Lists every offer the Node has recorded, in id order. Calls `maker_offer_list`. Prints an
array of `{revision, status, offer, reservation_id, swap_id}`. No flags.

#### `lez-maker-cli withdraw-offer`

Withdraws one active offer that no Taker has reserved. Calls `maker_offer_withdraw`. Prints
`{revision, was_replay}`. Flags: `--request-id`, `--offer-id`, `--expected-revision` (the
offer's revision from `offers`), all required.

### Start, stop and health

#### `lez-maker-cli start`

Starts `lez-maker-node.service` through systemd. No flags, no socket. See
[Start and stop are systemd only](#start-and-stop-are-systemd-only).

#### `lez-maker-cli stop`

Stops `lez-maker-node.service` through systemd. No flags, no socket. Stopping the Node does
not cancel a swap: its state is in the Node's database and the Node resumes it at the next
start ([swap coordinator](swap-coordinator.md)).

#### `lez-maker-cli health`

Asks the running Node whether it is ready. Calls `maker_health`. Prints `{schema_version,
ready, degraded, delivery, chat, routes}` and, on a Node that runs the swap lifecycle,
`lez_chain`. `delivery` and `chat` are each `disabled`, `available` or `unavailable`;
`routes` lists each configured route with the same three states for its foreign chain.
`degraded` is true while Delivery, Chat or a route is `unavailable` or the LEZ chain is
stalled. The command exits 0 whenever the Node answered, also when `degraded` is true: a
script reads the fields. No flags.

### Query history

#### `lez-maker-cli history`

Lists the Node's swaps in id order. Calls `swap_history`. Prints an array of `{id, pair,
direction, phase, requires_attention, pending_alerts, highest_alert_severity}`. No flags. Use
an `id` with `monitor`, `status`, `alerts`, `claim` and `refund`.

#### `lez-maker-cli status`

Reads one swap's summary. Calls `swap_status`. Prints one row of the `history` shape. Flag:
`--id` (required). An unknown id answers -32004.

#### `lez-maker-cli monitor`

Reads one swap's actor: what the Node's supervisor last did for it and what it waits for.
Calls `maker_actor_monitor_v1`. Prints `{schema_version, swap_id, actor_kind, schedule_state,
lease_generation, attempt_count, …}` with `progress`, `manual_action`, `terms` and `effects`
when the Node has them. Flag: `--id` (required). `lease_generation` is the value `claim` and
`refund` take as `--expected-generation`.

#### `lez-maker-cli alerts`

Lists a swap's operator alerts (a lock transaction that was replaced or reorganised after the
Node counted it). Calls `swap_alerts`. Prints an array of `{sequence, aggregate_revision,
kind, severity, funded_by, observed_event, previous_transaction_id, canonical_transaction_id,
terminal_phase, acknowledged}`.

| flag | meaning | default |
|---|---|---|
| `--id` | the swap | required |
| `--after` | list alerts with a sequence above this | `0` |
| `--all` | include acknowledged alerts | off |

#### `lez-maker-cli acknowledge-alert`

Marks one alert as seen. Calls `swap_alert_acknowledge`. Prints the swap's summary row. Flags:
`--id` and `--alert` (the alert's `sequence`), both required.

#### `lez-maker-cli balances`

Reads what the Maker's own Bitcoin Core wallet and LEZ account hold. Calls
`maker_wallet_balances_v1`. Prints the Node's JSON. Flag: `--zcash` also lists what the
Maker's Zcash role can fund from (the Node reads its Zebra).

### Manual claims and refunds

The Maker Node claims and refunds by itself: its supervisor runs each swap's actor when the
chains allow the next step. `claim` and `refund` are for a swap whose actor stopped
(`schedule_state` `failed` or `backoff` in `monitor`) or that the operator wants driven now.
Each queues one request for the swap's actor and returns at once; the actor then does only
what the agreement and the chains admit. Neither command builds a transaction from its
arguments. Follow the result with `monitor`.

```sh
lez-maker-cli monitor --id SWAP          # read lease_generation
lez-maker-cli claim --id SWAP --request-id claim-1 --expected-generation GEN
lez-maker-cli refund --id SWAP --request-id refund-1 --expected-generation GEN
```

#### `lez-maker-cli claim`

Queues a claim for the swap's actor. Calls `maker_actor_claim_v1`. Prints `{schema_version,
swap_id, action, requested_after_generation, was_replay}`. Flags: `--id`, `--request-id`,
`--expected-generation`, all required.

#### `lez-maker-cli refund`

Queues the Maker's timeout recovery for the swap's actor. Calls `maker_actor_refund_v1`. Same
flags and output as `claim`.

Both accept a swap of any pair that has an actor on this Node. Refusals: -32004 `maker actor
not found`; -32009 when `--expected-generation` is not the actor's current `lease_generation`,
when another manual action is still pending, or when the request id was used with other
arguments; -32602 when the actor is terminal.

### Other commands

#### `lez-maker-cli delivery-identity`

Prints the public Delivery identity of a signing key file, the key a Taker pins as its Maker:
`{schema_version, public_key}` with the compressed secp256k1 key in hex. Local: it reads the
file and contacts nothing. Flag: `--signing-key-file <PRIVATE_KEY>` (required), an
owner-private file holding the raw or hexadecimal key.

#### `lez-maker-cli create-swap`

Records one swap in the Node's coordinator store from explicit recovery terms. Calls
`swap_create`. Prints the swap's summary row. The Node creates the records of the swaps it
agrees over Chat itself; this command is the direct entry to the coordinator, used by the
crate tests and the operator journey test.

| flag | meaning | default |
|---|---|---|
| `--id` | the swap's id | required |
| `--pair`, `--direction` | the route | `--direction taker-sells-foreign` |
| `--confirmations` | confirmations a lock needs | required |
| `--maker-refund-basis`, `--taker-refund-basis` | `block-height` or `timestamp` | `block-height` |
| `--maker-refund-at`, `--taker-refund-at` | each refund's height or time | required (`--maker-refund-at` not for Monero) |
| `--earlier-refund-latest`, `--later-refund-earliest`, `--required-margin` | the bounds between the two refunds | required, not for Monero |
| `--xmr-refund-event-confirmations` | Monero only: confirmations of the refund event | `2` |

For Monero the CLI accepts `taker-sells-lez` only ("Monero does not support direction …") and
refuses the Maker deadline flags ("Monero recovery is event-gated and does not accept maker
deadline fields"). For the other pairs it refuses `--xmr-refund-event-confirmations`. An id
that exists answers -32009.

## Taker (U4): `lez-taker-cli`

`lez-taker-cli [--socket <path>] <command>`.

| RFP wording | Commands |
|---|---|
| offer discovery | `swap offers` |
| swap initiation | `swap take`, `swap lock` |
| progress monitoring | `swap monitor`, `swap list`, `health`, `balances` |
| claim and refund | `swap claim`, `swap refund` |
| start/stop | `start`, `stop` |
| without the Node | `monitor`, `claim`, `refund`, and the discovery and acceptance flags |

A swap through the running Node:

```sh
lez-taker-cli swap offers --pair bitcoin --direction taker-sells-foreign
lez-taker-cli swap take --request-id take-1 --offer-id OFFER --foreign-units 100000
lez-taker-cli swap lock --swap-id SWAP     # Bitcoin only
lez-taker-cli swap monitor --swap-id SWAP
lez-taker-cli swap claim --swap-id SWAP --request-id claim-1 --expected-generation GEN
```

SWAP is the take reply's `swap.swap_id`; GEN is the last monitor reply's
`progress_generation`.

### `lez-taker-cli swap`

The group of commands that drive a swap through the running Taker Node. Each needs the Node
and calls one owner method, named with it below.

### Offer discovery

#### `lez-taker-cli swap offers`

Lists the offers the Node has authenticated from its Makers. Calls `taker_offer_list_v1`.
Prints `{schema_version, offers}`; each row carries the `offer`, its `maker_identity` and its
`signed_envelope_sha256`. For each Monero offer one line of words follows on stderr with its
bounds and price in XMR and the take command.

| flag | meaning | default |
|---|---|---|
| `--pair` | list one pair; needs `--direction` | all routes |
| `--direction` | list one direction; needs `--pair` | all routes |

`--pair monero --direction taker-sells-foreign` is refused before any call ("the selected pair
does not support this swap direction"). With Delivery unavailable the Node refuses the list
with -32010 and category `authenticated_delivery_unavailable`
([Delivery and Chat outages](delivery-chat-outage.md)).

### Swap initiation

#### `lez-taker-cli swap take`

Takes one listed offer for an exact amount. The CLI reads `taker_offer_list_v1`, finds the
offer, computes the exact LEZ amount from its price, and calls `taker_swap_initiate_v1` with
the offer's route, Maker identity and signed-envelope digest as the Node listed them. Prints
`{schema_version, was_replay, swap}`.

| flag | meaning | default |
|---|---|---|
| `--request-id` | the take's id; repeat the command with the same id to replay or resume it | required |
| `--offer-id` | the offer to take | required |
| `--foreign-units` | the amount in foreign atomic units (sats, zatoshis or piconero), within the offer's bounds | required |
| `--maker-public-key` | the Maker's compressed public key in hex | needed only when two Makers list the same offer id |
| `--funding-transaction-hex` | for a Node without a Bitcoin wallet: the signed, unsent lock | none |

The CLI refuses, before the take is sent: an offer the Node does not list ("the Taker Node
lists no live offer …"); an offer id two Makers share without `--maker-public-key`; an amount
outside the bounds or one the price cannot represent exactly (the message names the bounds
and the price).

The Node reserves the offer with the Maker, agrees the swap over Chat and starts the swap's
actor before it answers, which is why this command waits up to 7 minutes. A Bitcoin
`taker-sells-foreign` take on a Node without a Bitcoin wallet answers -32018 with category
`bitcoin_funding_required` and the `address` and `amount_sat` to pay: sign that payment
without sending it and repeat the same take with `--funding-transaction-hex`
([Funding the lock from your own wallet](api/README.md#funding-the-lock-from-your-own-wallet)).
A take while Delivery or Chat is down is refused and not queued; repeat it with the same
request id when `health` reads available again
([Delivery and Chat outages](delivery-chat-outage.md)).

#### `lez-taker-cli swap lock`

Sends the Taker's first lock of a Bitcoin swap. Calls `taker_swap_lock_v1`. Prints the swap's
view. Flag: `--swap-id` (required). Repeating it returns the swap's one recorded lock; it does
not lock twice. A Zcash or Monero swap needs no `swap lock`: the Node sends the Taker's LEZ
lock itself after the take. On a Monero swap the command fails and the CLI adds why: "a Monero
swap has no `swap lock`: the Taker Node locks your LEZ by itself right after the take".

### Progress monitoring

#### `lez-taker-cli swap monitor`

Reads one swap. Calls `taker_swap_monitor_v1`. Prints the swap's view: its `state`,
`progress_generation`, `available_action` (`claim`, `refund` or absent), `terms`, effects and
the actor's last outcome. Flag: `--swap-id` (required). For a Monero swap one or more lines of
words follow on stderr: what the state means, the next command when there is one, and `needs
you: …` when the refund window is half over.

#### `lez-taker-cli swap list`

Lists the Node's swaps. Calls `taker_swap_list_v1`. Prints `{schema_version, swaps}`, each row
a swap view. No flags. Monero rows get the same words as in `swap monitor`.

#### `lez-taker-cli health`

Asks the running Node whether it is ready. Calls `taker_health`. Prints `{schema_version,
ready, degraded, delivery, chat, pair_capabilities, registered_methods}` and, when the Node
has them, `lez_chain` and `routes`. `delivery` and `chat` are each `disabled`, `available` or
`unavailable`. The command exits 0 whenever the Node answered. No flags.

#### `lez-taker-cli balances`

Reads what the Taker's own Bitcoin Core wallet and LEZ account hold. Calls
`taker_wallet_balances_v1`. Prints the Node's JSON.

| flag | meaning |
|---|---|
| `--zcash` | also list what the Taker's Zcash role can fund from (the Node reads its Zebra) |
| `--monero` | also read the Taker's Monero wallet, in piconero; one line of words follows on stderr with the balance in XMR, the address and the `monerod` height |

### Claim and refund

The Taker Node does not claim or refund on its own initiative: the owner asks, the Node
checks that the action is admissible now, and the swap's actor performs it. `swap monitor`
names the admissible action in `available_action`.

#### `lez-taker-cli swap claim`

Requests the claim. Calls `taker_swap_claim_v1`. Prints `{schema_version, was_replay, swap}`.

| flag | meaning |
|---|---|
| `--swap-id` | the swap |
| `--request-id` | the action's id; repeat with the same id to replay or resume it |
| `--expected-generation` | the `progress_generation` of the monitor reply the decision was made on |

All three are required. A stale generation, or a request while the other action was admitted,
is refused with the Node's `taker_action_conflict` or `taker_action_unavailable` category. A
successful reply means the action was admitted, not that the swap is `completed`: read
`swap monitor` until it is.

#### `lez-taker-cli swap refund`

Requests the refund. Calls `taker_swap_refund_v1`. Same flags and output as `swap claim`. The
Node offers `refund` only once the swap's terms allow it.

For which pair claims what: on Bitcoin and Zcash the Taker's claim takes the Maker's lock and
reveals the secret; on Monero (`taker-sells-lez`) `swap claim` authorises the Maker's LEZ
claim and then sweeps the Monero, and `swap refund` takes the Taker's LEZ back in the refund
window ([A Monero swap](api/README.md#a-monero-swap)).

### Start and stop

#### `lez-taker-cli start`

Starts `lez-taker-node.service` through systemd. No flags, no socket. See
[Start and stop are systemd only](#start-and-stop-are-systemd-only).

#### `lez-taker-cli stop`

Stops `lez-taker-node.service` through systemd. No flags, no socket.

### Commands that do not use the Node

These work on one swap's actor files directly. They exist for a Taker actor that no running
Node supervises: each takes the actor's lock and fails with "… actor is already running or
unsafe" while a Node (or another command) holds it. A swap taken with `swap take` belongs to
the Node; use the `swap` commands for it.

Each takes exactly one of:

| flag | meaning |
|---|---|
| `--actor-config <PRIVATE_JSON>` | the owner-private Taker actor configuration (Bitcoin or Zcash) |
| `--receipt <PRIVATE_JSON>` | the owner-private acceptance receipt that selects the actor (Bitcoin, Zcash or Monero) |

#### `lez-taker-cli monitor`

Prints the actor's durable status from its own files, without chain or transport access.

#### `lez-taker-cli claim`

Runs the actor's claim step against the chains its configuration names.

#### `lez-taker-cli refund`

Runs the actor's timeout recovery against the chains its configuration names.

The output is the actor's own JSON (for Bitcoin with `"pair":"bitcoin"` added). A Monero
receipt without effect authority supports `monitor` only.

### Discovery and acceptance flags (no subcommand)

Run without a subcommand, `lez-taker-cli` reads a Maker's offers from a run-local Delivery
directory itself, without the Node, and can accept a Bitcoin or Monero offer over the Maker's
Chat socket into owner-private files. The Node's own take (`swap take`) does all of this inside
the Node; these flags are the file-based lane the crate tests and the actor fixtures use. Every
path is an owner-private file or directory.

A Zcash offer is not accepted in this lane: a Zcash swap is agreed by the two Nodes at take time
(`swap take`), each with its own key, and no draft made ahead of the take is accepted. The
flags `--accept-zec-offer`, `--zec-source-taker-config`, `--zec-taker-actor-root` and
`--zec-acceptance-receipt` no longer exist and are refused as unexpected arguments;
the acceptance flags below given without `--accept-btc-offer` or `--accept-xmr-offer` are refused
with a message naming `lez-taker-cli swap take`. The last line of `lez-taker-cli --help` names
the pairs the binary was built with (`Pairs in this build: bitcoin, zcash.`).

Discovery needs `--delivery-directory`, `--maker-public-key` (the Maker identity to trust) and
`--now-unix-seconds` (the time offers are judged live at), and prints `{schema_version,
offers}` with each offer's `maker_public_key` and `signed_envelope_sha256`. `--pair` filters
by pair and `--direction` by direction (`--direction` needs `--pair`).

| mode | selected by | further flags |
|---|---|---|
| plan a Bitcoin take | `--plan-btc-offer <id>` | `--reservation-id`, `--foreign-units`; prints `{offer_id, reservation_id, signed_envelope_sha256, swap_id, foreign_units, lez_units, private_material_disclosed}` and accepts no Chat, signing or actor flag |
| accept a Bitcoin offer | `--accept-btc-offer <id>` | `--chat-socket`, `--reservation-id`, `--foreign-units`, `--unsigned-draft-file`, `--taker-signing-key-file`, `--agreement-output-file`, `--btc-source-taker-config`, `--btc-taker-actor-root`, `--btc-acceptance-receipt`, and for the contribution-bound Chat round `--maker-contribution-file`, `--taker-contribution-file`, `--btc-role-root` |
| accept a Monero offer (`pair-xmr` builds) | `--accept-xmr-offer <id>` | `--chat-socket`, `--reservation-id`, `--xmr-stage-a-file`, `--xmr-activation-file`, `--xmr-source-taker-root`, `--xmr-taker-public-packet`, `--xmr-maker-public-packet`, `--xmr-taker-role-journal`, `--xmr-taker-actor-root`, `--xmr-acceptance-receipt`, and for chain effects `--xmr-effect-authority-file`, `--xmr-effect-manifest-file`, `--xmr-workflow-journal`, `--xmr-run-id` |

Only one acceptance, or the plan, may be asked for in one run. A Bitcoin acceptance defaults
to `taker-sells-foreign`; a Monero acceptance takes `monero` and `taker-sells-lez` only. An acceptance whose agreement file (for Monero, actor
root) already exists resumes from it without reading Delivery again. Each prints its
acceptance result as one JSON line.

## Where the commands are exercised

| what | where |
|---|---|
| every Maker call of a Bitcoin swap made by `lez-maker-cli`, its `health` and `balances` read as the owner, and the wrong-uid refusal of every owner-RPC command | devnet scenario `maker-cli` (`scenario_maker_cli`) |
| `lez-maker-cli refund` on a swap whose Taker never claims, followed with `monitor` and `history` | `maker-cli-refund` (Bitcoin), `zec-maker-cli-refund` (Zcash, the Taker selling ZEC) |
| `lez-maker-cli claim` asked while the Maker's own LEZ claim is held by a devnet test hook | `zec-maker-cli-claim` (`LEZ_TEST_HOOKS=1`) |
| a Bitcoin swap with the Taker driven by `lez-taker-cli`, in both directions, its `swap list`, `health` and `balances` read as the owner, and the wrong-uid refusal | `taker-cli` (`scenario_taker_cli`) |
| a Bitcoin refund driven by `lez-taker-cli` | `taker-cli-refund` |
| Zcash swaps driven by `lez-taker-cli` | `zec-taker-cli`, `zec-dynamic-happy`; a Zcash refund, `zec-taker-cli-refund` |
| a Monero swap driven by `lez-taker-cli` (`balances --monero`, `swap take`, `swap claim`) | `xmr-taker-cli` |
| `health` of both CLIs and `lez-maker-cli publish-offer` while Delivery and Chat are unavailable | `outage-before-take`, `zec-outage-before-take`, `xmr-outage-before-take` |
| Monero offers configured and published through `lez-maker-cli` | the `xmr-*` scenarios; except in `xmr-taker-cli` their Taker side uses the owner RPC |
| the Maker's full CLI journey against a real daemon (configuration, pricing, offers, history, manual claim and refund, `start`/`stop` refusals) | `crates/maker-node/tests/operator_journey.rs` |
| `start` and `stop` on the real units | `scripts/run-maker-node-systemd-vm.sh` ([systemd guide](maker-systemd-install.md)) |
| the words the Taker CLI prints for Monero rows | unit tests in `lez-taker-cli.rs` |

The scenarios are run with `bash deploy/scripts/regress.sh run node:<pair>:<scenario>:<direction>`
([deploy/README.md](../deploy/README.md)).
