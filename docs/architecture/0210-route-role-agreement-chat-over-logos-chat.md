# ADR 0210: Route role-agreement messages over Logos Chat

- Status: accepted for the Basecamp PoC
- Date: 2026-08-25
- Scope: Basecamp Maker/Taker negotiation transport

## Context

The BTC Chat v2 protocol already constructs Maker and Taker contributions in
independent role roots, verifies both signatures, binds direction, assets,
messages, and chain identities, and persists the final agreement in the role
stores. Its process test previously called the Maker's owner-local Chat socket
directly. That proved the application protocol but did not exercise an actual
peer-to-peer Basecamp transport.

Logos Chat `v0.2.2` supplies direct conversations, peer addresses, events, and
end-to-end encrypted message delivery. The Chat module deliberately keeps
identity, conversations, and history only for the module lifetime. Logos
Delivery is a required runtime dependency of Chat. Neither module should gain
authority to create, validate, sign, or persist a swap agreement.

## Decision

Use Logos Chat as the transport for the existing fixed Chat RPC methods, with a
small role-fixed Rust gateway on each installation:

```text
Taker service -> Taker proxy UDS -> Taker gateway -> Basecamp Chat
                                                    | E2EE direct conversation
Maker daemon Chat UDS <- Maker gateway <- Basecamp Chat
```

The Basecamp adapter owns the Chat module instance and event subscriptions.
The gateway owns only bounded session memory and JSON framing. The Maker daemon
and durable role/store layers remain the sole protocol authority.

The accepted Logos inputs are pinned through `apps/basecamp/flake.lock`:

- `logos-chat-module` tag `v0.2.2`;
- the Delivery input followed from Chat;
- the module builder followed from Chat;
- Delivery's LIDL overridden with Chat's compatible
  `rust-lib/deps/delivery_module.lidl`, as required by the official module
  composition contract.

## Frame and session rules

`LogosChatFrameV1` is UTF-8 JSON with schema version 1, sender and recipient
roles, a request or response payload, and a SHA-256 frame identifier computed
over the complete typed message. Requests carry a process-local monotonic
nonce. Responses bind the exact request frame identifier.

Only these application methods can cross the bridge:

- `btc_chat_propose_v1` and `btc_chat_propose_v2`;
- `btc_chat_complete_v1` and `btc_chat_complete_v2`;
- `zec_chat_propose_v1` and `zec_chat_complete_v1`;
- `xmr_chat_stage_a_v1` and `xmr_chat_activate_v1`.

One gateway process pins exactly one direct conversation, one local address,
one peer address, and one role direction. An exact repeated bind is a replay;
a different bind conflicts. The Maker learns an incoming peer address only
from the authenticated `message_received.sender` event. It does not treat
Chat's `peer_label` as an address because Chat `v0.2.2` uses a shortened
conversation label there.

An explicit owner-local reset is accepted only while no Taker request or Maker
daemon call is active. It clears the session, queued frames, and response cache
so an unintended first peer cannot occupy the full app lifetime. It never
changes durable contribution, agreement, replay, or actor state.

Frames and Taker-facing Chat RPC bodies are independently limited to 1 MiB. The
gateway control envelope is limited to 4 MiB because it JSON-escapes the already
encoded frame; this prevents a valid frame from becoming an untransportable
outbox head. The outbox is limited to 64 entries, pending and in-flight requests
to 32 each, and the Maker response replay cache to 128. An oversized Maker
result, including an ambiguous JSON `null`, becomes a bounded dependency error
response. The Taker request wait is
25 seconds, strictly inside the existing 30-second owner-local caller deadline.
Unix control and proxy sockets are absolute, owned by the effective user, and
mode 0600.

## Event and thread rules

The Basecamp adapter subscribes to `delivery_state_changed`,
`conversation_created`, and `message_received` before calling `chat.init`.
Synchronous Chat calls are never made from an event callback: address lookup
and inbound processing are deferred to the next Qt event-loop turn. Outbound
frames are polled from the owner-local gateway and sent only after Chat is
online and the exact conversation is bound.

The Maker control method validates and accepts an inbound request immediately,
then performs the daemon RPC in a bounded asynchronous task. Duplicate frames
while that task is active are acknowledged as replays, and completed responses
are cached before the in-flight marker is removed. This keeps the Qt thread out
of the daemon's request latency without creating duplicate concurrent work.
An abandoned individual JSON-RPC connection is isolated to that connection and
does not terminate the gateway or discard its app-lifetime session.

The default Delivery preset is `logos.test`; `logos.dev` may be selected with
`LEZ_LOGOS_CHAT_PRESET`. Other values fail closed. The app destructor stops its
timer and calls `chat.shutdown()`.

## Lifetime and durability

The Chat identity, conversation, history, gateway binding, pending table, and
outbox exist only while the corresponding apps and endpoints are running.
Operators start one role gateway with one app and stop it when that app exits.
This matches the PoC requirement and prevents a later app instance from
silently inheriting a previous peer binding.

Durable state is intentionally elsewhere:

- signed Maker and Taker contribution wires;
- final countersigned agreement;
- agreement binding and replay records;
- Maker negotiation state and actor authority.

Losing the Chat session can interrupt an in-flight call, but it cannot change a
committed agreement. The existing request identifiers and store replay checks
recover exact retries after a new session is established.

## Offline verification

The production adapter uses the real Logos Chat module. The process E2E replaces
only the external Chat/Delivery network with `local-relay`, which transfers the
same serialized frames between the same two gateway control sockets. It starts
the real Maker daemon, separate Maker and Taker gateways, independent role
roots, and the real Chat v2 proposal/completion calls.
Transient per-cycle transfer failures remain queued and are retried rather than
terminating the relay.

The test is run with both `CARGO_NET_OFFLINE=true` and a Docker container using
`--network none`. Source and dependency cache mounts are read-only; the target
directory is task-private. This proves the E2E itself has no Internet, public
RPC, or live Compose dependency. The Basecamp Maker/Taker Nix outputs are also
built once from the pinned closure and repeated after disconnecting the
task-owned builder from Docker networking.

## Consequences

The PoC now exercises real Logos Chat APIs and E2EE in Basecamp while retaining
the independently signed application protocol. The local relay is test-only;
it cannot become production actor authority. Offer discovery remains the
existing signed Delivery/filesystem mechanism in this change. Chat sessions
are not resumable across app restarts, which is accepted for this PoC and must
be reconsidered before production use.
