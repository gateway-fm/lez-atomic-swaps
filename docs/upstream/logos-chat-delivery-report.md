# chat_module on `logos.test`: direct-conversation messages arrive minutes late or not at all

Draft of an issue for logos-co/logos-chat-module (with a request for logos-delivery-module).
**The measurements are from one host on one day (2026-11-12, UTC), 13 messages in five sessions;
they were not independently re-measured.**

## Summary

Two Basecamp instances on one host, each running chat_module with the `logos.test` preset,
exchanged direct-conversation messages of 0.4-16 KB: of 13 messages, 5 arrived in 1-4 s, 2 after
109 s and 258 s, and 6 had not arrived 20 s to 6 min later. In the same sessions, broadcasts on a
Delivery content topic between the same two nodes arrived reliably. The same message exchange
between the same two applications, with the two Delivery nodes peered directly on a private
cluster (through a 66-line chat_module patch), delivered every message in 0.1-0.3 s.

## Environment

- chat_module `logos-co/logos-chat-module` v0.2.2, commit `dfe8ccf3eff3`; delivery_module
  `logos-co/logos-delivery-module` v0.2.0, commit `3258cdb0132e`; logos-delivery `f8b036594ea2`;
  logos-module-builder 0.2.5 `8e4ea1c1d0e5` (pins in `apps/basecamp/flake.lock`).
- Preset `logos.test` (cluster 2, 8 shards). Chat pubsub topic `/waku/2/rs/2/0`, content topic
  `/logos-chat/1/<conversation>/proto`. chat_module creates its own Delivery node from the preset
  (`start_delivery_bootstrap`: `{mode: Core, preset, messagingOverrides: {logLevel}}`).
- Two Basecamp instances (a "Maker" M and a "Taker" T), each in its own Docker container on one
  Linux host, sharing no directory or socket. Their Delivery nodes listened on 127.0.0.1 inside
  their containers and did not peer with each other; in the morning session both were connected
  to fleet nodes node-01 and node-02 gc-us-central1-a.
- The application is this repository's Basecamp desk plugin, which drives chat_module through
  `apps/basecamp/common/logos_chat_bridge.{h,cpp}`: `init({delivery_preset: "logos.test",
  log_level: "info"})`, `get_address()`, `create_conversation(peer address)`,
  `send_message(conversation id, content)`, and the events `delivery_state_changed`,
  `conversation_created`, `message_received`. The same plugin broadcasts small JSON offer
  announcements with delivery_module `send`/`subscribe` on `/lez-atomic-swaps/1/offers/json`
  every 10 s.

## What we expected

Messages in an established direct conversation between two online nodes delivered within a few
seconds, in both directions. Our application waits up to 25 s for each reply
(`REQUEST_TIMEOUT`, `crates/node-common/src/logos_chat_gateway.rs:49`, inside a 30 s caller
deadline) and needs about six request/reply rounds to complete one exchange.

## What happened

Times are each side's chat_module log lines ("sent N bytes to <conversation>" on the sender,
"received N bytes in <conversation>" on the receiver). Both containers share the host clock.

| Session | Direction | Size | Sent | Received | Delay |
|---|---|---|---|---|---|
| Morning, conversation 8385edd3… (created 10:55:05.951; M subscribed 10:56:10.492) | T→M | 9965 B | 10:55:25.791 | 10:57:14.270 | 108.5 s |
| | M→T | 2315 B | 10:57:14.324 | not by 11:03:10 | lost |
| | T→M | 9958 B | 10:56:16.629 | 11:00:34.245 | 257.6 s |
| | M→T | 364 B | 11:00:34.275 | not by 11:03:10 | lost |
| | T→M | 9947 B | 10:59:43.926 | not by 11:03:10 | lost |
| Run 1 | T→M | 9974 B | 11:58:41.379 | not by 11:59:07 | lost |
| Run 2 | T→M | 9941 B | 12:03:47.632 | 12:03:50.750 | 3.1 s |
| | M→T | 2303 B | 12:03:50.831 | not by 12:04:13 | lost |
| Run 3 | T→M | 9978 B | 12:06:10.929 | 12:06:12.644 | 1.7 s |
| | M→T | 2304 B | 12:06:12.727 | 12:06:13.817 | 1.1 s |
| Run 4 | T→M | 9947 B | 12:08:48.677 | 12:08:50.549 | 1.9 s |
| | M→T | 2296 B | 12:08:50.625 | 12:08:54.061 | 3.4 s |
| | T→M | 15770 B | 12:08:54.432 | not by 12:09:21 | lost |

13 messages: 5 in 1-4 s, 2 after 109 s and 258 s, 6 not within the 20 s to 6 min watched. "Lost"
means not received by the time given; we did not watch longer. No exchange completed.

In contrast, the offer announcements broadcast on the Delivery content topic every 10 s reached
the other node reliably in every session.

## What we ruled out

- **Our own blocking call.** In the morning session our Taker desk made a 10 s synchronous call
  on the thread that also serves chat_module's events. That was fixed before runs 1-4 (commit
  17cbcb60, "Confirm and initiate waits for its take without stopping the Chat bridge"); runs 1-4
  still lost messages. A 10 s stall also does not explain delays of 109 s and 258 s.
- **Our timeouts.** The 25 s per-reply budget is ours and was not raised. It explains why an
  exchange failed; it does not affect whether chat_module delivered: the receive times above are
  chat_module's own log lines, and messages counted as lost had not been logged as received
  minutes later (morning session) or by the end of the run (runs 1-4).

## Observations that may help

These are observations, not conclusions.

- In the morning session the Maker's chat_module subscribed to the new conversation's content
  topic 65 s after the conversation was created (10:55:05.951 → 10:56:10.492); the first message
  had been sent at 10:55:25.791 and arrived at 10:57:14.270, after the subscription. *Guess:*
  messages published before the receiver subscribes are only recovered later, if at all.
- Both chat_modules alternated between "delivery failed" and "online" every few minutes.
- Reported by the operator during the morning session but not kept in the retained logs: the
  late arrivals were logged as relay messages, and older messages appeared (store backfill) only
  after a reconnect. *Guess:* the receiving node relies on relay and fetches from store only on
  reconnection, so a message missed on relay waits for the next reconnect.
- In runs 1-3 our Taker created a new direct conversation about every 20 s before one carried a
  message (run 2: ten conversations between 12:00:15 and 12:03:37). That is our client retrying;
  we have not established why its first attempts did not proceed.

## The same exchange on a private two-node network

With chat_module patched to accept a Delivery configuration (below), the two Delivery nodes used
cluster 7341 (8 shards), the Maker's listening on TCP 60000 and the Taker's naming it as its only
entry node (`deploy/compose.logos.yaml`), on the same host, same application, messages of similar sizes.
Six runs on 2026-11-12 (about 17:55-18:24 UTC), six of six exchanges completed. One run's log
(18:15, conversation e6312ef6…):

| T sent | M received | M sent | T received |
|---|---|---|---|
| 23.215 (9988 B) | 23.378 | 23.621 (2307 B) | 23.723 |
| 24.032 (15800 B) | 24.312 | 24.430 (6080 B) | 24.551 |
| 24.626 (5497 B) | 24.762 | 24.971 (476 B) | 25.073 |
| 25.176 (5401 B) | 25.305 | 25.578 (2474 B) | 25.682 |
| 25.726 (2925 B) | 25.858 | 25.977 (4957 B) | 26.163 |
| 26.276 (2530 B) | 26.431 | 26.967 (2887 B) | 27.088 |

Every hop took 0.10-0.28 s; the six rounds took 3.9 s. Combined with the reliable topic
broadcasts on `logos.test`, this points to the public fleet, or to relay/store behaviour between
fleet peers for direct-conversation topics, rather than to chat_module's message handling or to
our application.

## Reproduction

Without our stack (this is what our bridge does; we have not run it as a standalone program):

1. Start two hosts (or two processes with separate instance directories) that load
   delivery_module and chat_module at the pins above.
2. On each: `chat_module.init({"delivery_preset": "logos.test", "log_level": "info"})`; wait for
   `delivery_state_changed` with `online`; `B = get_address()` on the second.
3. On the first: `create_conversation(B)`; on the second wait for `conversation_created`.
4. Alternately send N messages of about 10 KB each way with `send_message`, each after the
   previous one arrived or after 30 s; log the send time and the `message_received` time.
5. Compare with the same steps on a private cluster (needs the patch below).

With this repository: build the stack (`deploy/README.md`), remove the two
`LEZ_LOGOS_DELIVERY_CONFIG` lines from `deploy/compose.logos.yaml` (the desks then use the
`logos.test` preset) and run `deploy/scripts/regress.sh run ui:btc:logos-happy:TakerSellsForeign`;
the scenario prints both chat_modules' send/receive lines. The public-fleet runs were made
from branch `agent/m5-logos-lane` (runs 1-4 at bf030345); run logs are kept locally, not in the repository.

## Requests

**(a) Investigate Chat delivery latency and loss on `logos.test`** for direct conversations:
messages of 0.4-16 KB between two online nodes taking 100-260 s or not arriving, while topic
broadcasts between the same nodes arrive.

**(b) Let the host configure chat_module's Delivery node.** chat_module creates its Delivery
node itself from a preset name only, so an application cannot give it a cluster, entry nodes, a
listen address or a node key, although delivery_module's `createNode` accepts them. A second
`createNode` by the application is not an option (one node per instance). The module's own
comment above `start_delivery_bootstrap` says: "chat_module can't coexist with another
delivery_module consumer today. Drop these calls once the host bootstraps delivery_module and
exposes it ready-to-use." We carry a 66-line patch,
[`deploy/builder/patches/logos-chat-module/0001-chat-module-delivery-config.patch`](../../deploy/builder/patches/logos-chat-module/0001-chat-module-delivery-config.patch),
against `dfe8ccf3eff3`: an optional `ChatConfig.delivery_config` (delivery_module's whole
`createNode` JSON as text) used as it is in place of the preset's; absent or empty, nothing
changes (`chat_module.lidl`, `src/actions.rs`, `src/lib.rs`). It is what made the private-network
runs above possible. We would rather see the host own Delivery's lifecycle as the comment
describes, with chat_module using a node the host configured and started; the patch is offered as
a stop-gap or as a reference.

## Also noted

- Documented in this repository (`deploy/README.md`, "Two transports") and in our operations
  log, without a retained Delivery log: a `/dns4/<docker service name>/tcp/60000/p2p/…` entry
  node did not resolve inside the Delivery node, while the same peer as `/ip4/<address>/…`
  worked; we use addresses. *Guess:* the node's resolver does not use the container's DNS
  resolver.
