# Basecamp role agreements over Logos Chat — implementation report

Date: 2026-08-26

Delivery branch: `main`

Scope: independent role-agreement implementation, real Basecamp Logos Chat
transport, and signed Logos Delivery offer discovery

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
`2f7869bab70aee9a7ddef1ff75324418d3754250`. Its history remains reachable, and
its Rust source is now present directly in the default branch workspace.

The Logos Chat source implementation is exactly
`7ecd8d3ebe16f84c5a187908902fc3de33be523f`. Its history remains reachable, and
the gateway, Maker/Taker integration, tests, and manifests are checked in at
their normal paths.

The signed Delivery discovery implementation is exactly
`e67522c8d27c274bcb94b92f90420983c83d0f5b`. Its history remains reachable and
the default branch combines the implementation with the richer M3 Basecamp UI,
ADRs, report, deployment, and diagram presentation.

Remote policy at completion: Gateway's `main` is the real public submission and
open-source branch. The personal repository remains a separate upstream source
of future implementation work; the public branch contains ordinary source
files and does not depend on that remote to build.

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
- richer M3+ Maker and Taker Nix packages — built offline;
- direct-source workspace and package manifests — verified from a clean
  checkout without a reconstruction step.

A full all-target Clippy run still reports two unrelated pre-existing warnings:
an unused `provision_maker_claim` in `maker_xmr_tag17_supervisor` and an
unnecessary wrapper in the existing `lez-taker-cli.rs`. They are outside this
change; the changed library and new binary are warning-clean.

## Review disposition

OpenCode with `zai-coding-plan/glm-5.3` reviewed the source and delivery diffs.
Its substantive findings were repaired as follows:

- missing published source material: checked in directly and verified;
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

The final focused reviews of source commit `7ecd8d3` and its staged publication
returned `CLEAN` from both Claude Code Fable and OpenCode GLM-5.3. Each
independently confirmed the six final closures. The implementation is now
reviewable directly in the default branch rather than through publication
indirection.

## Operating the live PoC

Start one gateway beside each application and stop it with that application.
The Maker gateway points to the existing Maker daemon `--chat-socket`; the Taker
service points to the Taker gateway proxy. Set the same owner-local control
socket path in `LEZ_LOGOS_CHAT_GATEWAY_SOCKET` for the corresponding Basecamp
process. `LEZ_LOGOS_CHAT_PRESET` accepts only `logos.test` (default) or
`logos.dev`.

Once both Basecamp apps are open, the Maker publishes signed offer announcements
containing its current Chat address. The Taker browses that authenticated
in-memory index and automatically connects to the selected Maker. Manual address
copy/paste remains a diagnostic control, not the production discovery path.
Closing either app calls `chat.shutdown()`; stopping its paired endpoint
guarantees the next launch has no inherited peer binding. Exact commands are in
`apps/basecamp/README.md`.

## Accepted PoC limitations

- Live messaging requires the configured Logos Delivery network; the production
  E2EE network itself is intentionally not contacted by offline tests.
- The offline Nix proof is a post-warm-up build from the pinned local closure,
  not a claim that a cold machine can fetch dependencies without Internet.
- A Taker gateway supports one direct peer per application lifetime; a Maker
  gateway supports up to 32 concurrent direct peers. Neither resumes sessions
  after restart.
- The first structurally valid gateway request received by an unbound Maker
  establishes its peer; the full frame hash, role, method, signatures, agreement
  facts, chain identities, and replay remain validated downstream.
- The live offer index is in memory and rebuilt from Maker rebroadcasts after an
  app restart. The filesystem adapter remains only for legacy CLI and isolated
  test compatibility, not Basecamp production discovery.

The architectural rationale and invariants are recorded in
`docs/architecture/0210-route-role-agreement-chat-over-logos-chat.md`.

## 2026-08-26 follow-up: signed Delivery offer broadcasts

### Outcome

Basecamp no longer calls `taker_offer_list_v1` for production discovery. The
Maker periodically broadcasts a short-lived, independently verifiable view of
each relevant durable offer through the Delivery node already owned by Chat;
the Taker verifies and indexes the exact received bytes, reviews the terms, and
resolves the selected offer to the Maker's signed current Chat address. Delivery
is discovery transport only: offer reservation, application agreement,
countersignature, replay, and actor authority remain in the existing Rust
store/protocol boundary.

Source implementation commit:
`e67522c8d27c274bcb94b92f90420983c83d0f5b`.

Public delivery branch: Gateway `main`. The personal origin remains the source
feed and is not the public submission remote.

### Wire format and trust boundary

The exact content topic is
`/lez-atomic-swaps/1/offers/json`. Every message is canonical JSON, at most
32 KiB, and contains:

- the existing exact signed immutable offer envelope;
- the compressed Maker secp256k1 public identity;
- the Maker's current app-lifetime Chat address;
- the durable offer-local revision and projected status;
- the announcement time and an exclusive short lease boundary;
- a low-S secp256k1 signature over every field above.

The Taker rejects malformed or noncanonical JSON, invalid nested or outer
signatures, identity mismatch, address substitution, oversized input, expired
leases, excessive future clock skew, inconsistent active-offer expiry, and
same-revision equivocation. The immutable offer signature is still the
negotiation commitment; the outer signature adds live address and lifecycle
binding without granting Delivery any signing or reservation authority.

### Runtime steps

1. Basecamp registers Delivery event handlers before `chat.init()`.
2. Chat creates and owns the one shared Delivery node; the consumer does not
   call Delivery `createNode`, `start`, or `stop`.
3. When that node is online, both roles subscribe to the exact offer topic.
4. Every 10 seconds, and once immediately after subscription, the Maker asks its
   owner-only daemon RPC for cursor-paged snapshots signed at trusted local time;
   each page keeps its encoded announcement payload below 48 KiB so the complete
   JSON-RPC response stays within the daemon's 64 KiB transport boundary. A
   compile-time assertion couples the 32 KiB canonical-record limit to the
   worst-case standard-Base64 record plus page framing.
5. Basecamp decodes each exact Base64 record and passes the bytes unchanged to
   Delivery `send(topic, payload)`. It processes one snapshot page per Qt event
   turn, retains the lexicographic cursor between turns, skips an individually
   malformed or temporarily unsendable record, and retries omissions on the next
   full sweep instead of starving every later offer.
6. A Taker forwards each `messageReceived` payload unchanged to its owner-only
   Rust gateway through a bounded 64-item deferred queue; the gateway verifies
   it and updates a stable 1,024-entry index with a 128-entry per-Maker quota;
   a full index preserves live ordering state and rejects unrelated new keys
   until a lease expires rather than allowing a later Sybil burst to evict it.
7. **Browse authenticated offers** reads that live index, filters the requested
   route, returns at most the newest 16 entries plus an explicit omitted count,
   fills the reviewed fields, selects a signed current Chat address, and retains
   the exact public announcement proof inside the 4 MiB gateway response bound.
8. Basecamp creates the E2EE direct conversation automatically; negotiation then
   follows the previously implemented Chat request/response path.
9. **Confirm and initiate** refreshes the proof from the selected live index
   entry, then supplies it to the Taker owner service, which verifies its
   signature, lease, route, amounts, Maker identity, and envelope commitment
   again instead of consulting the filesystem offer index. The reviewed terms
   remain pinned, while a human review may safely outlive the original
   30-second lease. A failed refresh stops initiation; the backend never falls
   back to its browse-time proof.

Announcements carry 30-second leases and are refreshed every 10 seconds. An
active lease is additionally clamped to the immutable offer expiry. Snapshots
contain only retryable unexpired active/reserved/consumed records, never the
Maker's unbounded lifetime history, and split at deterministic offer-ID cursors
rather than overflowing the owner socket. A page contains at most 128 records;
the Basecamp bridge retains the page cursor while yielding after every page, so
the retryable set may be arbitrarily larger than one event-loop turn without
making its tail unreachable or blocking the UI through an entire sweep. No
reservation identifier, swap identifier, private actor material, or signing key
is broadcast. Delivery `storeQuery` is not used because the pinned API does not
provide a stable application contract for it.

### Conflict and convergence behavior

The Maker store is the sole one-winner compare-and-set boundary. Multiple
Takers may legitimately discover the same active announcement and open
different conversations; the Maker gateway therefore supports 32 bounded
conversation/peer bindings and keys in-flight/replay work by both conversation
and frame identity.

| Event | Winning Taker | Losing Taker | Other listeners |
|---|---|---|---|
| Concurrent proposal | atomic reservation succeeds | correlated `-32018` unavailable or `-32009` conflict | may still show the current active lease briefly |
| Local result | continues the agreement | suppresses that indexed offer immediately | unchanged until broadcast |
| Next Maker poll | receives signed revision/status | receives signed revision/status | receives signed reserved/consumed status |
| Missing update | existing state remains until lease end | local suppression remains | stale active entry expires after at most 30 seconds |

The index orders updates by `(offer_revision, announced_at)`, rejects immutable
term changes, treats exact repeats as replay, ignores older announcements, and
rejects non-active-to-active resurrection while the newer signed state is
retained. A strictly newer signed Active rebroadcast clears an earlier local
loser marker even when the durable revision is unchanged; a fresh Active insert
after the old lease leaves the index clears it too. This gives immediate loser
feedback and bounded eventual convergence for observers without claiming that
broadcast delivery itself serializes negotiation.

Conflict suppression is correlated to the exact `(Maker identity, offer ID)`
chosen from the authenticated index and retained by the Taker gateway. It is
not reconstructed from the Maker Chat address: another valid identity can reuse
the same address and local offer-ID string without hiding the selected offer's
loser marker. Marker storage is pruned against the bounded live index before an
insert and can never exceed the 1,024-entry index bound.

### Session and failure behavior

The Taker retains one selected direct session. Browsing an offer at a different
signed Maker address returns `session_busy`; the user must invoke the explicit
idle-only reset before switching, so a pause between negotiation RPCs cannot
silently discard the current peer. The Maker accepts up to 32 authenticated
`message_received.sender` bindings. Outbox records carry their exact
conversation identifier, response caches and in-flight keys are
conversation-scoped, a failed Chat send defers only that head behind other
conversation work, and an unwind guard releases every in-flight Maker key. The
offline relay forwards the same target rather than using a fixture conversation
constant.

All indexes, Chat identities, bindings, conversations, and message queues remain
app-lifetime as requested. Durable offer states, signed contributions,
countersigned agreements, replay journals, and actor roots survive independently
in their existing stores. Snapshot pages and list responses are transport
bounded; index pressure preserves every live entry and its local marker until
signed advancement or lease expiry. Permissionless identities can still occupy
capacity if they arrive first; preventing that requires an admission policy that
this PoC deliberately does not invent.

### Product harness and presentation

The prepared Taker Basecamp product test no longer merely types filesystem
fixture values into QML. Service mode requires a fresh
`logos_offer_announcement_base64`, sends it over the owner-only gateway Unix
socket with a five-second timeout, requires the isolated live index to contain
exactly that one signed entry with no omissions, and only then exercises Browse,
confirm/initiate, exact replay, list, and monitor. The test is local-network-only
and does not require Internet access.

The requested presentation now includes the diagram slide **Broadcast to
discover. Chat to negotiate.** in the basics section. Its public lane shows a
signed 30-second offer rebroadcast every 10 seconds through Logos Delivery to
multiple Takers; its private lane shows two E2EE Chat conversations converging
at one atomic Maker reservation, with one countersigned winner and one busy
loser. The diagram lives in the editable presentation source as well as the
generated standalone, so rebuilding no longer deletes it. The 22-slide
standalone, manifest, README, source hashes, and checksum entry were regenerated
together and rendered at 1600×900. The older 2026-08-20 stack slide is explicitly
labelled as an earlier capture where Chat was disabled, while this follow-up's
offline verification is dated 2026-08-26. The exact user-requested local deck at
`lez-atomic-swaps/media/lez-btc-m1-m3-m6-submission.html` was recovered separately
as a diagram and was intentionally not staged from that already-dirty worktree.

### Offline and non-disruptive verification

The full application protocol E2E runs with independent Maker/Taker role roots,
real gateway processes, exact Chat frames, Unix-domain relay transport, and no
fixture actor authority. Network-denial checks set Cargo offline mode and run in
uniquely named, auto-removed Docker containers with `--network none`,
read-only source/Cargo mounts, and a task-private target directory. No existing
Compose service, container, network, volume, or port is started, stopped,
restarted, or modified.

Evidence for this follow-up:

- full Maker-node library/integration suite: green under
  `cargo test -p lez-maker-node --lib --tests` in a network-disabled container;
  40/40 library tests and every non-external process/integration test passed,
  with exactly two declared external-infrastructure tests ignored (the pinned
  Basecamp product driver and the two-Zebra restart case);
- independent-role Chat-v2 process E2E: green;
- signed announcement/index/conflict regression tests: green;
- exact selected-identity collision and future-created Active skip regressions:
  green;
- Basecamp static package contract: green, 2 packages and 19 typed slots;
- Node syntax check for the product harness: green;
- generated standalone presentation count/source parity: green, 22/22;
- `cargo fmt --all -- --check`: green;
- `git diff --check` in both worktrees: green;
- Basecamp/Nix compile: unavailable on this host because the Nix CLI/closure is
  absent; no networked cold build was substituted for the offline claim.

### Review disposition

Claude Code Fable and OpenCode GLM-5.3 reviewed the source and integration diffs
in parallel through repeated read-only passes. The first review found and drove
the following material corrections:

- C++ session binding now replays the owner-gateway bind after a gateway restart
  instead of trusting only a cached local flag;
- index pressure never evicts unrelated live signed ordering state, preventing a
  stale Active resurrection after a Sybil burst;
- canonical announcements are limited to 32 KiB and Chat addresses to 1 KiB,
  standard Base64 is bounded before decode and round-trip canonicalized, and a
  compile-time invariant proves one maximum record fits the 48 KiB page budget;
- snapshot and gateway clocks have production-default/test-only seams, future
  Active rows are skipped rather than aborting a page, and lease-expiry tests no
  longer depend on a 30-second wall-clock race;
- the Maker bridge carries its cursor across pages, yields after each page, and
  continues after an individual invalid/failed announcement instead of wedging
  the tail;
- a Taker's loser marker is tied to the exact selected Maker identity and offer
  ID, so a second identity cannot suppress it by choosing the same Chat address;
- the live index and the local loser-marker subset are bounded and preserve
  state until signed advancement or lease expiry;
- the official product journey now requires a fresh signed announcement,
  injects it through the real gateway Unix socket with a five-second timeout,
  proves the isolated index contains exactly that entry, and only then performs
  Browse/initiate/replay/list/monitor;
- ADR 0211, the authoritative source-tree ADR index, the editable presentation
  source, standalone hashes, slide count, and dated capture qualification were
  corrected together.

After those changes, Fable returned **CLEAN** for both the source and integration
trees. GLM returned **CLEAN** for all implementation focus areas, requested one
missing ADR-0209 source-index row, and returned **CLEAN** after that row was added;
its separate curated-integration link audit also returned **CLEAN**. The final
accepted reviewer observations are non-blocking only: app-lifetime state is an
explicit PoC choice, first-arrival permissionless identities can consume bounded
index capacity, the checked-in silent MP4 intentionally predates this slide, and
the unrelated UI-demo MP4 checksum mismatch already existed at the baseline.

ADR 0211 records this design and its rejected alternatives; ADR 0210 now points
to it as the production discovery successor.

### Direct-source provenance

The signed Delivery implementation remains attributable to source commit
`e67522c8d27c274bcb94b92f90420983c83d0f5b`. Gateway `main` now carries that
implementation and the rest of the buildable workspace as ordinary files,
alongside the Basecamp UI, ADRs, report, deployment, and presentation.
