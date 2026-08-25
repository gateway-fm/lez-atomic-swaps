# Basecamp role agreements over Logos Chat — implementation report

Date: 2026-08-25

Delivery branch: `m3-plus`

Scope: independent role-agreement implementation plus real Basecamp Logos Chat transport

## Outcome

The Basecamp PoC now uses the real Logos Chat `v0.2.2` module and its pinned
Logos Delivery runtime for live peer-to-peer, end-to-end encrypted negotiation
transport. The transport does not become swap authority: the Maker and Taker
still construct independent signed contributions, and the Rust protocol/store
layer still validates direction, assets, messages, chain identities, replay,
and the final countersigned agreement.

The formerly fixture-only process path is now an offline test adapter, not the
production transport. Production Basecamp packages call the generated Chat API;
the E2E suite replaces only Chat/Delivery's external network with a Unix-only
relay that transfers the exact same serialized gateway frames. This makes the
full local E2E runnable with no Internet or public RPC while retaining a real
Chat-backed application path.

## Commit and branch correction

The earlier independent-role implementation is exactly
`2f7869bab70aee9a7ddef1ff75324418d3754250`. It is represented on the delivery
branch by merge commit `d0c75e65b42fcc5a273691eff6a037757aa1f43e`, whose
second parent is that exact implementation commit and whose first-parent tree
adds runner patch `0035` plus the reproducibility metadata.

The Logos Chat source implementation is exactly
`7ecd8d3ebe16f84c5a187908902fc3de33be523f`. It is represented by runner patch
`0036`, which applies directly after patch `0035`. The final `m3-plus` delivery
commit has `d0c75e6` as first parent and `7ecd8d3` as second parent, while its
tree also contains the richer M3 Basecamp UI port and this report.

Remote policy at completion:

- `gateway/m3-plus` stops at `d0c75e6`, so it receives the requested earlier
  implementation but not the new Chat integration;
- `origin/m3-plus` contains both delivery commits;
- no per-implementation delivery branch is required.

The patch series was reconstructed from base `5c384a5`. Applying patches
`0001` through `0036` produces source tree
`eec5693c31520fa2a35762d2f730c02e3b5b56a2`, exactly matching the tree of
source commit `7ecd8d3`.

## Why the old path was fixture-only

The previous process test called the Maker daemon's owner-local Chat JSON-RPC
socket directly. That was valuable because it proved independent role roots,
signatures, store replay, actor handoff, and countersigned agreement equality,
but it bypassed Basecamp and did not instantiate Logos Chat, establish a direct
conversation, or send an E2EE Chat message. It therefore tested the application
protocol without testing the intended peer transport.

The fix separates two concerns explicitly:

1. live applications use the real Chat and Delivery modules;
2. offline tests use a local relay only where a public Delivery network would
   otherwise be required.

The relay has no keys, signing authority, role roots, chain clients, or store
access. It only peeks, transfers, ingests, and acknowledges the same
content-addressed frames through owner-only Unix sockets.

## Implemented data flow

```text
Taker service
  -> owner-only Taker proxy socket
  -> role-fixed Taker gateway and bounded outbox
  -> Basecamp Chat send_message
  -> Logos Delivery + E2EE direct conversation
  -> Maker Basecamp message_received event
  -> role-fixed Maker gateway
  -> owner-only Maker daemon Chat socket
  -> signed contribution/agreement validation and durable store
  -> response follows the same path in reverse
```

The bridge allows only these existing protocol methods:

- `btc_chat_propose_v1`, `btc_chat_propose_v2`;
- `btc_chat_complete_v1`, `btc_chat_complete_v2`;
- `zec_chat_propose_v1`, `zec_chat_complete_v1`;
- `xmr_chat_stage_a_v1`, `xmr_chat_activate_v1`.

Every frame carries schema version, fixed sender/recipient roles, request or
response type, a request nonce or exact request-frame reference, and a SHA-256
identifier over the complete typed message. A gateway binds one conversation,
one local address, and one authenticated peer address for its process lifetime.

## Detailed fixes

### 1. Real Chat module composition

Both isolated Basecamp packages now depend only on `chat_module` and its
`delivery_module` runtime. The consumer flake pins Chat tag `v0.2.2`, follows
Chat's module-builder and Delivery inputs, and overrides the Delivery LIDL with
Chat's compatible `rust-lib/deps/delivery_module.lidl`. The lockfile pins Chat
commit `dfe8ccf3eff3e95da0ba54043577270474a216ae` and Delivery commit
`3258cdb0132e37228aa2519e0c01c0e7429a20dd` with checked NAR hashes.

### 2. Basecamp Chat adapter

The shared C++ `LogosChatBridge` subscribes before `chat.init()` to
`delivery_state_changed`, `conversation_created`, and `message_received`.
Generated API calls cover `init`, `get_address`, `create_conversation`,
`send_message`, and `shutdown`.

Chat `v0.2.2` forbids synchronous module calls from an event callback, so
address lookup, conversation creation after an online transition, and inbound
gateway submission are deferred to the next Qt event-loop turn. A Taker address
entered before Chat becomes online is retained and retried after the online
event. Unrelated direct-conversation notifications no longer pin or poison the
Maker session; the Maker waits for the first structurally valid inbound gateway
frame and takes the peer from authenticated `message_received.sender`.

The bridge member is destroyed before the backend's `LogosUiPluginContext`
base, so the captured generated Chat wrapper remains alive while the bridge
stops its timer and calls `chat.shutdown()`.

### 3. Role-fixed Rust gateways

`lez-logos-chat-gateway endpoint` exposes one owner-only control socket for each
role and, only for the Taker role, one owner-only proxy socket. Socket paths are
absolute, owned by the effective user, mode `0600`, and protected by bounded
HTTP/JSON-RPC framing. Maker mode cannot expose the proxy; Taker mode cannot be
configured with a Maker daemon socket.

The shared C++ local-RPC client also distinguishes invalid constructor limits
from an oversized request: non-positive size or timeout configuration returns
`invalid_configuration`, while only an actual body overflow returns
`request_too_large`.

State is deliberately in memory and bounded:

- 1 direct session binding;
- 64 queued outbox frames;
- 32 pending Taker calls;
- 32 in-flight Maker calls;
- 128 cached Maker responses;
- 1 MiB maximum Chat frame and Taker proxy body.

Gateway control bodies are separately capped at 4 MiB because a control request
or response JSON-escapes an already encoded frame. This repair prevents a valid
1 MiB frame from becoming an outbox head that can never be peeked or ingested.
An oversized Maker result or ambiguous JSON `null` result is converted to a
small dependency-error response so the Taker receives a correlated failure
instead of waiting indefinitely.
Structured Maker-daemon errors retain their original numeric code and a
validated message of at most 4 KiB across the response frame; transport,
malformed, and oversized errors remain the generic bounded dependency failure.

Maker ingest now validates and acknowledges a frame quickly, then calls the
Maker daemon in a bounded asynchronous task. A duplicate content-addressed
frame received while that call is active is acknowledged as a replay rather
than starting duplicate work. The response is cached before the in-flight mark
is removed, closing the race between completion and replay.

The Taker peer wait is 25 seconds, strictly shorter than the existing 30-second
owner-local RPC deadline, so pending state is released and a correlated error
can be returned before the caller abandons its connection.
An abandoned individual RPC connection is reaped without terminating the
gateway; listener failures still follow the graceful process-shutdown path.

### 4. JSON-RPC null-result correctness

The local Rust client previously deserialized `"result": null` into the same
`Option` state as a missing result field. That rejected a valid RPC response
when the expected type was `Option<Value>`, which is exactly how an empty
outbox is represented. A custom presence-aware `RpcResultField` now
distinguishes a missing field from a present JSON null. The regression test
`present_null_result_is_not_treated_as_a_missing_result` covers this case.

### 5. UI and lifetime behavior

The richer M3 Maker and Taker screens retain all existing market, evidence, and
settlement controls and add a private negotiation Chat panel. The Maker can
refresh and copy its app-lifetime address. The Taker can paste that address,
connect, and inspect the bound-session status. Failure envelopes pass through
the existing QML decoder, which rejects `ok:false` before any success state is
displayed.

Both panels expose **Reset Chat**. Reset is owner-local and succeeds only while
the pending and in-flight tables are empty; it clears the session, outbox, and
response cache so an unintended first peer does not occupy the entire app
lifetime. A failed initial gateway bind is retried once per second while Chat
remains online and the conversation is known.

The Taker never takes authority from a `conversation_created` event: its exact
conversation identifier comes only from the return value of its own
`create_conversation` call. The Maker continues to pin its peer only from the
authenticated sender of the first valid gateway frame. Failed Chat sends
preserve the outbox head and use bounded exponential retry spacing from one to
five seconds, avoiding a synchronous retry on every 50 ms poll.

A failed outbox peek marks the local session unbound and enters the same
once-per-second rebind path; a missing conversation at that point retries
creation. This recovers a transient creation failure or a gateway restart
instead of leaving the UI in a false connected state. All owner-local gateway
I/O shares one 500 ms total deadline, and event callbacks defer binding and
ingest to the next Qt event-loop turn.

Chat identity, conversation, history, binding, pending requests, and outbox are
discarded when the applications and their paired gateways stop. Signed
contributions, the final countersigned agreement, replay records, and actor
authority remain durable in Rust stores. This implements the accepted PoC
requirement that Chat sessions need live only while the apps are live.

### 6. Offline process E2E

`lez-logos-chat-gateway local-relay` binds two task-owned endpoints and transfers
their exact serialized outbox frames through Unix-domain sockets. The process
test launches a real Maker daemon, Maker gateway, Taker gateway, independent
role roots, and the relay; the real proposal/completion clients point at the
Taker proxy. No fixture actor owns both roles.
Per-cycle relay failures leave the source outbox head unacknowledged and are
retried instead of terminating the relay.

The harness verifies owner-only socket type/mode, waits for both role bindings,
performs proposal and completion, checks replay after Delivery input is moved
offline, and terminates only the children it started.

## Verification evidence

All network-denial checks used task-owned containers that were disconnected
from Docker networking before execution. The source mount and Cargo registry
mount were read-only, the Cargo target was task-private, and no live Compose
service was started, stopped, restarted, or inspected for mutation.

Completed checks:

- `cargo fmt --all -- --check` — green;
- `git diff --check` in both implementation and delivery worktrees — green;
- Maker-node unit suite — 34/34 green, including null-result, structured-error,
  bounded-forwarding, frame-integrity,
  session-conflict, and duplicate-in-flight replay regressions;
- `btc_chat_process` — 3/3 green, including the independent-role gateway/relay
  negotiation and the existing forward/reverse handoff cases;
- strict Clippy for `lez-maker-node --lib` and `lez-logos-chat-gateway` with
  `-D warnings` — green;
- Basecamp package contract — green: 2 `ui_qml` packages, 18 typed slots,
  pinned E2EE Chat plus owner-local gateway;
- source Basecamp Maker and Taker Nix packages — built offline;
- richer `m3-plus` Maker and Taker Nix packages — built offline;
- ordered patch reconstruction — patches `0001`–`0036` reproduce tree
  `eec5693c31520fa2a35762d2f730c02e3b5b56a2` exactly.

A full all-target Clippy run still reports two unrelated pre-existing warnings:
an unused `provision_maker_claim` in `maker_xmr_tag17_supervisor` and an
unnecessary wrapper in the existing `lez-taker.rs`. They are outside this
change; the changed library and new binary are warning-clean.

## Review disposition

OpenCode with `zai-coding-plan/glm-5.3` reviewed the source and delivery diffs.
Its substantive findings were repaired as follows:

- missing patch `0036` and untracked delivery sources: packaged and verified;
- 1 MiB frame/RPC envelope mismatch: separate 4 MiB control envelope with the
  1 MiB frame/proxy authority retained;
- 500 ms Qt ingest versus long Maker call: fast accept plus bounded asynchronous
  Maker work;
- implicit Tokio sync feature: explicitly enabled;
- unrelated Maker conversation pinning: unrelated events ignored, authenticated
  first valid frame required;
- connect-before-online loss: peer retained and conversation creation retried;
- QML false-success concern: existing decoder confirmed to reject failure
  envelopes;
- teardown lifetime: C++ destruction ordering documented next to the closure.

Claude Code `fable` found no P0/P1 issue and identified six edge cases. They
were repaired: ambiguous null results now fail in-band; abandoned connections
no longer kill the endpoint; failed binds retry; relay failures retry; an
idle-only reset recovers an unintended peer binding; and invalid proxy input is
classified as invalid input.

The next parallel pass found the remaining boundary cases: both reviewers
identified the Taker's post-configuration `conversation_created` race; GLM also
identified false connected state after gateway loss, missing conversation-create
retry, and flattened Maker diagnostics; Fable identified per-read rather than
total Qt I/O timeout and synchronous gateway work in the Chat callback. Those
were repaired by removing Taker event authority, adding create/rebind recovery,
forwarding only validated bounded daemon errors, sharing one 500 ms deadline,
and deferring binding plus ingest to the next event-loop turn.

The final focused reviews of source commit `7ecd8d3`, its staged delivery, and
the regenerated patch returned `CLEAN` from both Claude Code Fable and OpenCode
GLM-5.3. Each independently confirmed the six final closures; GLM additionally
reconstructed all 36 patches to tree `eec5693c31520fa2a35762d2f730c02e3b5b56a2`.
Fable's topology check is satisfied by forming the delivery commit with
`commit-tree`, first parent `d0c75e6` and second parent `7ecd8d3`, rather than a
single-parent commit of the staged index.

## Operating the live PoC

Start one gateway beside each application and stop it with that application.
The Maker gateway points to the existing Maker daemon `--chat-socket`; the Taker
service points to the Taker gateway proxy. Set the same owner-local control
socket path in `LEZ_LOGOS_CHAT_GATEWAY_SOCKET` for the corresponding Basecamp
process. `LEZ_LOGOS_CHAT_PRESET` accepts only `logos.test` (default) or
`logos.dev`.

Once both Basecamp apps are open, refresh the Maker Chat status, copy its
current address, paste it into the Taker Chat panel, and connect. Closing either
app calls `chat.shutdown()`; stopping its paired endpoint guarantees the next
launch has no inherited peer binding. Exact commands are in
`apps/basecamp/README.md`.

## Accepted PoC limitations

- Live messaging requires the configured Logos Delivery network; the production
  E2EE network itself is intentionally not contacted by offline tests.
- The offline Nix proof is a post-warm-up build from the pinned local closure,
  not a claim that a cold machine can fetch dependencies without Internet.
- A gateway supports one direct peer per application lifetime and has no session
  resumption after restart.
- The first structurally valid gateway request received by an unbound Maker
  establishes its peer; the full frame hash, role, method, signatures, agreement
  facts, chain identities, and replay remain validated downstream.
- Offer discovery remains on the existing signed Delivery/filesystem path; this
  change moves negotiation messages, not the offer-index architecture.

The architectural rationale and invariants are recorded in
`docs/architecture/0210-route-role-agreement-chat-over-logos-chat.md`.
