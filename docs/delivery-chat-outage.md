# Logos Delivery and Chat outages: what the Nodes do

Requirement R8 (`docs/requirements-traceability.md`): "Graceful Logos Delivery / Chat
unavailability handling, behaviour documented". This page is that behaviour.

In short:

- Delivery carries offers and Chat carries the rounds of a take. Both are needed only until a
  take has returned. Locks, claims and refunds read each Node's own state and the chains.
- While either is unavailable a Node stays up, answers its owner socket with `ready: true` and
  reports `degraded: true` and which of the two is `unavailable`.
- A take that cannot be confirmed or negotiated is refused at once with JSON-RPC code `-32010`
  and a category. Nothing is buffered: the Node does not queue the take and does not send it
  later. The caller repeats the same request with the same request id.
- When the transport returns, both Nodes leave `degraded` without a restart. Offers the Maker
  published keep their state in its store; an offer's expiry time is not extended by an outage.

Everything here is read from the code and the tests named in section 9. Nothing was re-run for
this page.

## 1. Two transports, and which one a statement is about

The role launcher's `LEZ_TRANSPORT` (`modules/lez-role-node/node-entrypoint.sh`,
[deploy/README.md](../deploy/README.md#two-transports)) says how the Taker reaches the Maker.

- `local`, the default. Delivery is a directory the Maker writes signed offer files into and the
  Taker reads (`RunLocalDelivery`, `crates/node-common/src/run_local_delivery.rs`). Chat is the
  Maker Node's Chat Unix socket, which the Taker Node calls once per round
  (`call_local_chat_rpc`, `crates/node-common/src/local_rpc.rs`).
- `logos` (`deploy/compose.logos.yaml`). Offers travel as signed announcements over Logos
  Delivery and the rounds as Logos Chat messages between the two Basecamps. Each role runs a Chat
  gateway (`crates/node-common/src/logos_chat_gateway.rs`) between its Node and its desk's Logos
  Chat bridge. The Maker's gateway hands each request to the Maker Node's Chat socket; the
  Taker's gateway gives the Taker Node a proxy socket in place of the Maker's.

The Node's `delivery` and `chat` health fields, the refusals of sections 3 and 4 and every devnet
scenario of section 9 are about the Node's own boundary: the Delivery directory and the Chat
socket. On `logos` the Taker Node's Delivery source is an empty directory of its own and its Chat
socket is the gateway's proxy socket, so those two fields do not describe the Logos network. A
Logos message that does not arrive is handled by the gateway (section 6).

## 2. Health states

`maker_health` and `taker_health` both answer these fields:

| Field | Values | Meaning |
|---|---|---|
| `ready` | `true` | the Node's owner socket and durable state serve requests |
| `delivery` | `disabled`, `available`, `unavailable` | see below |
| `chat` | `disabled`, `available`, `unavailable` | see below |
| `degraded` | `true`, `false` | `true` when `delivery` or `chat` is `unavailable`, when a route's chain is `unavailable` (`routes`), or when the LEZ chain is degraded (`lez_chain`) |

`disabled` means the dependency is not configured for this Node. It does not set `degraded`.

Where each value is computed:

| Node | Field | Computed in | `available` when |
|---|---|---|---|
| Maker | `delivery` | the `maker_health` method (`register_health_method`, `crates/maker-node/src/lib.rs`), through `RunLocalDelivery::projection_health` | the directory is the owner's, mode 0700, and holds exactly one authentic file for each unexpired active or reserved offer in the store, and no other offer file |
| Maker | `chat` | the same method, `owner_socket_is_available` | the configured Chat socket path is a socket owned by the Node's user with mode 0600 |
| Maker | `degraded` | `MakerHealthV1::ready` and `with_lez_chain` (`crates/maker-node/src/rpc_contracts.rs`) | |
| Taker | `delivery` | `TakerFacadeBackend::health` (`crates/taker-node/src/taker_backend.rs`) | authenticated discovery (`RunLocalDelivery::discover`) succeeds on every configured Delivery source |
| Taker | `chat` | the same method, through `OwnerChatSocketProbe` (`crates/taker-node/src/taker_service_config.rs`) | the configured Chat socket path is a socket owned by the Node's user with mode 0600 |
| Taker | `degraded` | `TakerHealthV1::new`, `with_routes` and `with_lez_chain` (`crates/taker-node/src/taker_facade.rs`) | |

Three properties follow from this code:

- Both `chat` checks read the socket's metadata only. They do not connect, so `available` says
  the socket is there for this user, not that the Maker answered.
- Delivery fails closed. One unreadable, tampered or wrongly signed offer file makes the whole
  discovery fail (`authenticate_listed`), and the Taker then reports `delivery: unavailable`. The
  one exception is an offer file the Maker removed between the listing and the read, which is
  skipped.
- On the Maker, `delivery` also reads `unavailable` while the directory differs from the store
  (`RunLocalDeliveryError::ProjectionDrift`), for example after a publish whose file could not be
  written.

A Node starts during an outage. `RunLocalDelivery::publisher` and `subscriber` accept a
directory its owner cannot use at the moment (`validate_openable_directory`); every later
operation checks it again (`validate_private_directory`). The Maker logs
`maker: Delivery unavailable at start (…); reconciled once it is back` and serves its owner
socket. The Taker's launcher starts with the Maker identity it last used and logs
`taker: Delivery unavailable; starting with the Maker identity it last used`.

The gateway has no part in these fields. Its own `status` reply
(`LogosChatGatewayStatusV1`) carries `session_bound`, `session_count`, `queued_frames`,
`pending_requests` and `discovered_offers`.

## 3. Before a swap is taken: what each role can do

The Taker's refusals carry `error.data.category`. The Maker's errors carry a code and a message
and no `data`.

### Taker

| Action | Delivery unavailable | Chat unavailable (Delivery available) |
|---|---|---|
| Health (`taker_health`) | answers; `delivery: "unavailable"`, `degraded: true` | answers; `chat: "unavailable"`, `degraded: true` |
| Discover (`taker_offer_list_v1`) | refused: `-32010` `Taker dependency unavailable`, category `authenticated_delivery_unavailable` | works |
| Take (`taker_swap_initiate_v1`), a new request id | refused: `-32010`, category `authenticated_delivery_unavailable`. The Node cannot confirm that the offer is still live (`selected_offer_is_live`). Nothing is admitted and no Chat round is sent | refused: `-32010`, category `initiation_execution_unavailable`. The offer is confirmed live, then a Chat round fails |
| Take, a request id the Node already admitted | answered from the Node's registry, which is read before Delivery (`lookup_replay`); `was_replay: true` | answered if the take had finished its rounds; otherwise `-32010` `initiation_execution_unavailable` until Chat is back |
| List, monitor, lock, claim, refund of swaps already taken | work | work |

Two details of the take:

- A request that carries `logos_offer_announcement_base64` (the `logos` transport's signed
  announcement) proves the offer live by that announcement's signature and lease
  (`live_broadcast_offer_matches`); the Delivery directory is not read for it.
- The same request id with other facts (another offer, amount or Maker) is refused with `-32013`,
  category `initiation_conflict`.

### Maker

| Action | Delivery unavailable | Chat unavailable (Delivery available) |
|---|---|---|
| Health (`maker_health`) | answers; `delivery: "unavailable"`, `degraded: true` | answers; `chat: "unavailable"`, `degraded: true` |
| Publish (`maker_offer_publish`) | the offer is committed to the store as `active`, then the reply is an error: `-32603` with the Delivery adapter's message, `Delivery directory must be an owner-owned non-symlink directory with mode 0700` for a directory with another mode, `Delivery filesystem operation failed` for an I/O failure. No Taker can discover the offer yet | works; the offer is discoverable, and a take of it fails on the Taker's side as above |
| Withdraw (`maker_offer_withdraw`) | the offer is withdrawn in the store, then the reply is the same `-32603` error because its file could not be removed | works |
| List offers and swaps (`maker_offer_list`, `swap_history`) | work | work |
| Answer a Taker's Chat round | not reached: the Taker refuses the take before any round | not reached |

The Maker does not refuse a publish or a withdraw up front on Delivery's state. The store is the
authority and the directory is a projection of it (`reconcile_unhealthy_route_offers`), so the
owner's decision is recorded and the projection is repaired later (section 5).

## 4. Who retries

The Node does not buffer a take. `initiate`
(`crates/taker-node/src/taker_service/lifecycle.rs`) returns the refusal in the same call, keeps
no queue of pending takes and has no task that sends one later. The caller retries:

- The CLI user repeats `lez-taker-cli swap take` with the same `--request-id`.
- The devnet harness repeats the same request (`initiate` in `deploy/scripts/node-e2e.py` retries
  a `-32010` answer on a backoff schedule).
- On the Taker desk the user presses Take again. The order book's Take sends each press under a
  new request id (`ui-taker-take-offer-<milliseconds>`, `takeBtcOffer`), so it is a first take
  each time. The desk's Confirm and initiate form derives its request id from the offer's
  envelope digest (`initiate`), so pressing it again repeats the same request; a replayed answer
  shows as `Acceptance recovered`.

Repeating the same request id is safe at every point:

- Refused before admission (Delivery unavailable, or the first Chat round failed): the Taker's
  registry holds nothing under the request id, so the repeat is a first take.
- Admitted, some rounds done: the registry is read first, and the take resumes on its swap
  directory. `crates/taker-node/src/taker_service/btc_dynamic.rs` derives the reservation id from
  the request id (`reservation_id_for`) and each round's request id from the reservation id, so
  the repeat sends the same rounds and the Maker answers the ones it already ran from its record
  (section 6).
- Finished: the repeat is answered with `was_replay: true` and has no second effect.

The Maker's side is the same rule. `maker_offer_publish` repeated with the same request id after
a Delivery failure returns `was_replay: true` and writes the missing file
(`RunLocalDelivery::publish_or_verify`). `RunLocalDelivery::withdraw` treats a file that is
already gone as done, so a repeated withdraw finishes the cleanup.

## 5. What recovers by itself, and what happens to offers

When Delivery returns:

- The Maker repairs its directory on every `maker_health` call: `RunLocalDelivery::reconcile`
  writes the file of each unexpired active or reserved offer that is missing and removes the file
  of each offer that is no longer one (withdrawn, expired, consumed). The devnet's container
  health check calls `maker_health` every 10 s and both desks poll health every 10 s. A Maker
  with `--route-health-config` also runs the same pass on its own timer. A failed pass is tried
  again on the next call; `delivery` stays `unavailable` until the directory matches the store.
- The Taker's next `taker_health` and `taker_offer_list_v1` read the directory again. Nothing is
  cached, so they answer `available` and the offers as soon as discovery succeeds.

When Chat returns, the next round connects. Chat opens one connection for each round, so there is
no session to rebuild on the `local` transport.

Neither Node needs a restart. `assert_restored` in the devnet harness waits for both Nodes to
leave `degraded` after the modes are put back, and fails after 120 s.

Offers during an outage:

- An offer published before the outage keeps its record in the Maker's store: same status, same
  revision. Its file stays in the directory when only access is lost.
- An offer published during a Delivery outage is `active` in the store and reaches the directory
  at the first reconciliation after Delivery returns, if it has not expired by then.
- An offer withdrawn during a Delivery outage is withdrawn in the store at once. Its file is
  removed at the first reconciliation; until then no Taker can read the directory either.
- Expiry is not paused. `expires_at_unix_seconds` is part of the signed offer. The Taker lists an
  offer only while its trusted time is before that moment, and the Maker's reservation round
  refuses an expired offer. An offer whose lifetime ends during the outage is expired when the
  transport returns; the Maker publishes a new one.
- On the `logos` transport an announcement is a 30 s lease that the Maker renews every 10 s
  (`LOGOS_OFFER_ANNOUNCEMENT_TTL_SECONDS_V1`, `LOGOS_OFFER_REBROADCAST_SECONDS_V1`). An offer
  leaves the Taker's order book when three renewals in a row are missed and comes back with the
  next one that arrives.

## 6. Chat rounds are keyed by request digest

A take is a fixed sequence of Chat rounds (for Bitcoin: `btc_reserve_v1`, `btc_chat_propose_v2`,
`btc_chat_complete_v2`, `btc_ceremony_reserve_v1`, `btc_ceremony_nonce_v1`,
`btc_ceremony_partial_v1`; the full list is `LOGOS_CHAT_GATEWAY_METHODS_V1`). The Maker answers
each round once and remembers the answer:

- For each round the Maker stores the SHA-256 of the request and its response in the swap
  directory of the reservation (`RoundRecord`, `request_digest` and `replay` in
  `crates/maker-node/src/btc_lifecycle.rs`; the same in `zec_reserve.rs` and `xmr_reserve.rs`).
- The same request again returns the stored response with `was_replay: true`. The round does not
  run a second time.
- A different request for a round that already ran is refused with `-32009`.

So a round whose answer was lost can be sent again without a second reservation, signature or
nonce.

On the `local` transport the Taker Node sends each round once and waits up to 30 s. A failure
ends the take with `initiation_execution_unavailable` and the owner repeats it (section 4).

On the `logos` transport the Taker's gateway also retries inside one round. A request still
unanswered 1, 2, 4, 8 and 16 s after its first send is queued again, the same frame with the same
id and bytes (`REQUEST_RESEND_AFTER`), inside a 25 s wait (`REQUEST_TIMEOUT`). The Maker's gateway
answers a repeated frame from its in-flight set or its response cache, and the Maker Node's digest
rule covers a repeat that reaches it. If no answer arrives in 25 s the round fails with `-32010`
`Chat peer operation unavailable`; without a bound Chat session it fails at once with `-32010`
`Chat session unavailable`. Either ends the take as above.

## 7. After the take nothing depends on either transport

Every Chat round of a swap runs inside the take. The Taker's lock, both Nodes' observation of the
chains, the claims and the refunds use each Node's durable state and its chain connections; the
Taker Node's lock, claim and refund paths make no Chat call and read no offer.

The devnet scenarios cut both transports at four points and require the swap to end correctly
under the cut (section 9): before the Taker's lock with the Maker stopped
(`taker-refund-offline`), at the first lock (`offline-after-lock`), with both legs locked
(`offline-after-second-lock`) and after the Taker's claim is sent (`offline-after-claim`). At the
last two the counterparty's container is also killed under the cut and must start again without
Chat and Delivery.

A Node restarted during an outage keeps its swaps: the Taker answers a repeated take from its
registry and lists the swap while discovery is refused.

## 8. What the owner sees

### The desks

Both desks (`apps/basecamp/maker/src/qml/Main.qml`, `apps/basecamp/taker/src/qml/Main.qml`) read
health every 10 s in the background and on the Check Node button.

- The header badge (`nodeBadge`) reads `Node degraded` in amber whenever the last health reply has
  `degraded: true`. A suffix names a chain cause when there is one (` · LEZ finality stalled`,
  ` · LEZ indexer unreachable`, or the pairs whose route is unavailable). With only Delivery or
  Chat unavailable there is no suffix: the badge reads `Node degraded` alone.
- Check Node (`health`) sets the status strip to `Node needs attention` when the reply is
  degraded and `Node ready` otherwise. Its detail line names the transports. On the Maker desk:
  `<n> active route(s) · chat <state> · delivery <state>`. On the Taker desk:
  `Offer delivery: <state>`; the Taker's line does not name Chat. A degraded LEZ chain replaces
  the detail line on both desks.
- The other badge texts are `Node connected`, `Node not ready`, `Node unavailable` (the desk
  cannot reach its Node at all), `Checking the Node` and `Desk loading`.

### The CLIs

`lez-maker-cli health` (`crates/maker-node/src/bin/lez-maker-cli.rs`) and `lez-taker-cli health`
(`crates/taker-node/src/bin/lez-taker-cli.rs`) print the health reply as one line of JSON and
exit 0, degraded or not. With both transports unavailable the Maker's line contains

```json
{"schema_version":1,"ready":true,"degraded":true,"delivery":"unavailable","chat":"unavailable","routes":[]}
```

with one entry in `routes` for each configured route, and `lez_chain` on a Node that reads the
chain. The Taker's line has the same five leading fields, then `pair_capabilities` and
`registered_methods`, and `lez_chain` and `routes` where configured.

A refused command prints the error on stderr in the form
`local RPC error <code>: <message> <data>` and exits non-zero: `lez-taker-cli` with status 2,
`lez-maker-cli` with status 1 and the line prefixed `Error: `. `lez-taker-cli swap take` first
lists offers, so with Delivery unavailable it fails there:

```text
local RPC error -32010: Taker dependency unavailable {"category":"authenticated_delivery_unavailable"}
```

`lez-maker-cli publish-offer` during a Delivery outage fails with the `-32603` message of
section 3; the same command again after Delivery is back prints the commit with
`"was_replay":true`.

## 9. Proof

Crate tests:

| Statement | Proof |
|---|---|
| The health states and `degraded` on the Taker | `health_reports_disabled_available_and_degraded_dependencies` (`crates/taker-node/tests/taker_backend.rs`) |
| The Taker's Chat check is metadata only and refuses aliases and other modes | `chat_probe_is_dynamic_payload_free_and_rejects_aliases_or_wrong_modes` (`crates/taker-node/tests/taker_service_config.rs`) |
| `-32010` with category `authenticated_delivery_unavailable`, and no path or endpoint in the error | `backend_failures_map_to_fixed_codes_and_categories_without_private_details` (`crates/taker-node/src/taker_service.rs`) |
| Discovery fails for a missing directory or an unreadable entry; a withdrawn entry is skipped | `a_missing_directory_and_an_unreadable_entry_still_fail_discovery`, `an_offer_withdrawn_after_the_listing_is_skipped` (`crates/node-common/src/run_local_delivery.rs`) |
| A repeated take is answered before Delivery is read | `service_initiation_is_live_atomic_redacted_and_replays_before_delivery` (`crates/taker-node/tests/taker_initiate_rpc.rs`), `configured_initiation_survives_process_restart_without_live_delivery` (`crates/taker-node/tests/taker_service_process.rs`) |
| A Taker restarted while Delivery is unreadable starts, keeps its swap, refuses discovery, and discovers again once Delivery is back | `a_restart_while_delivery_is_unreadable_restores_the_swap_and_refuses_discovery` (`crates/taker-node/tests/taker_service_process.rs`) |
| A Maker starts while Delivery is unreadable and reports `delivery: "unavailable"` | `a_maker_starts_while_its_delivery_is_unreadable_and_reports_it_unavailable` (`crates/maker-node/tests/run_local_delivery.rs`) |
| A publish during a Delivery outage fails, leaves one `active` offer in the store and `ready: true`, `degraded: true`, `delivery: "unavailable"`; the same publish afterwards returns `was_replay: true` and health is clean | `assert_delivery_outage_is_visible_and_exact_retry_recovers`, called by both tests of `crates/maker-node/tests/zec_chat_process.rs` |
| With the Chat socket moved away the Maker reports `chat: "unavailable"`, `degraded: true`, `delivery: "available"`; a Taker Node's take (`taker_swap_initiate_v1`) is refused and writes no agreement, no actor root and no receipt; with Chat back and the Maker restarted the same take completes | `assert_chat_outage_and_restart`, called by `separate_taker_countersigns_and_maker_atomically_accepts_before_response` (the same file) |
| Every round of a take replays from the Maker's record | `a_monero_swap_is_agreed_at_take_time_in_six_rounds_and_every_round_replays` (`crates/maker-node/tests/xmr_reserve_process.rs`) |
| The gateway's resend: a dropped first request is answered after a resend, two delivered copies run once, a lost answer is sent again from the cache, no answer at all is `-32010` at the timeout, and the resends fall inside the timeout | `a_dropped_first_request_is_answered_after_a_resend`, `both_copies_delivered_run_once_and_the_late_response_is_ignored`, `a_dropped_answer_is_sent_again_from_the_cache_on_a_resend`, `no_answer_at_all_is_dependency_unavailable_at_the_timeout`, `production_resends_fall_inside_the_request_timeout`, `a_replayed_xmr_ceremony_round_gets_the_byte_identical_answer` (`crates/node-common/src/logos_chat_gateway.rs`) |

No crate test pins the category `initiation_execution_unavailable` for a take whose Chat round
fails on the Node's own take path; that row of section 3 is from `prepare_dynamic_btc`,
`execute_prepared` and `map_initiation_error`.

Devnet scenarios (`deploy/scripts/node-e2e.py`, listed for `regress.sh full` or its `refunds`
tier in `deploy/scripts/regress.sh`):

| Scenario | Pair | What it asserts |
|---|---|---|
| `outage-before-take` | Bitcoin, both directions | `take_after_outage`, below; then lock, claim, both Nodes completed |
| `zec-outage-before-take` | Zcash, both directions | `take_after_outage`; then the swap completes |
| `xmr-outage-before-take` | Monero | `take_after_outage`; then the swap completes as `xmr-happy` does |
| `offline-after-lock` | Bitcoin | take and lock, then the cut; claim and completion under the cut; `assert_cut_held`; `assert_restored` |
| `offline-after-second-lock` | Bitcoin | the cut with both legs locked; the Taker's container killed under the cut; it claims after its restart |
| `offline-after-claim` | Bitcoin | the cut after the Taker's claim is sent; the Maker's container killed under the cut; it makes its follow-up claim after its restart |
| `taker-refund-offline` | Bitcoin | the cut after the take, then the Maker stopped; the Taker locks and refunds under the cut, its side still refused at the refund |
| `zec-offline-after-lock`, `zec-offline-after-second-lock`, `zec-offline-after-claim` | Zcash | as the three Bitcoin scenarios |
| `zec-taker-refund-offline` | Zcash | the cut once the Maker has observed the Taker's lock, then the Maker stopped; the Taker refunds |
| `xmr-cut-after-lock` | Monero | the cut once the Taker's first LEZ transaction of the swap is on chain; the Maker's Monero funding, the Taker's claim, the Maker's LEZ claim and the Taker's sweep under the cut |

What the two shared helpers do:

`cut_delivery_and_chat` acts on the `local` transport's two inodes, in the Maker's container:
`chmod 0000` on the Chat socket `/run/lez/maker/chat.sock` and on the Delivery directory
`/delivery`. It does not stop, pause or disconnect the Logos Delivery and Chat modules of
`deploy/compose.logos.yaml`; those are not part of these scenarios. It asserts, in order:

1. Before the cut, a probe run as the Taker Node's user in the Taker's container connects to the
   Chat socket and lists the Delivery directory, so the probe can see a difference.
2. After the cut, the same probe gets `EACCES` for both and finds no open file descriptor into
   the Delivery directory in that container.
3. The Maker holds no open connection on its Chat socket (read from its `/proc/net/unix`) once
   a call in flight at the cut has ended, within 60 s.
4. `maker_health` and `taker_health` each report `chat` and `delivery` `unavailable` within
   120 s, and `degraded`.

It returns a watch that samples the Maker's Chat connections every 2 s. `assert_cut_held`, at the
end of what ran under the cut, requires at least one sample, no sample with a connection, and the
Taker-side probe still refused.

`take_after_outage` discovers the offer and builds the take request before the cut, records the
Maker's offer record and swap history, cuts, and then asserts:

1. `lez-maker-cli health` and `lez-taker-cli health` exit 0 and each read `ready: true`,
   `degraded: true`, `delivery: "unavailable"` and `chat: "unavailable"` (`assert_cli_health`).
2. `lez-maker-cli publish-offer` of a second offer on the same route fails with `-32603` and the
   message `Delivery directory must be an owner-owned non-symlink directory with mode 0700`;
   `lez-maker-cli offers` lists that offer once, `active`; `taker_offer_list_v1` for the route is
   refused with `-32010` and category `authenticated_delivery_unavailable`
   (`publish_under_outage`).
3. `taker_swap_initiate_v1` answers no result, code `-32010` and category
   `authenticated_delivery_unavailable`.
4. No swap in `taker_swap_list_v1` names the offer.
5. The Maker's record of the offer under take (`maker_offer_list`) and its `swap_history` are
   equal to what they were before the cut.
6. The cut held for at least two samples of the Maker's Chat socket (`assert_cut_held`).
7. After the modes are restored, `assert_restored`: the probe connects and lists again and
   neither Node is `degraded`.
8. The offer published under the cut is in the Taker's offer list within 120 s, with no second
   publish; its record on the Maker has the status and revision it was committed with; the same
   `publish-offer` request repeated answers `was_replay: true`. The scenario then withdraws it
   (`assert_outage_offer_recovered`).
9. The same take request, with the same request id, takes the offer.

The refusal these scenarios observe is the Delivery one. Chat is cut as well, but the Taker
refuses before it sends a round; the scenarios show that no connection reached the Maker.

## 10. The known upstream race

On the `logos` transport a Taker's first message on a new direct conversation can reach the
Maker's Delivery node before the Maker's Chat client has subscribed to the conversation's topic.
The node then drops it, and neither Delivery nor Chat sends it again. The gateway's resend of
section 6 is the workaround: the take goes on with the next copy that arrives after the
subscription. The race itself remains in the Logos modules.

- [submission/LIMITATIONS.md](../submission/LIMITATIONS.md), "The Logos transport": the mechanism,
  the measurements and what remains upstream.
- [docs/upstream/logos-chat-delivery-report.md](upstream/logos-chat-delivery-report.md): the
  report for the Logos Chat and Delivery modules, with the section "A message before its topic's
  subscription" and the separate finding that direct messages on the public `logos.test` fleet
  arrive minutes late or not at all.
