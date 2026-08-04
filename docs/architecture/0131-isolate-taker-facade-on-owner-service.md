# ADR 0131: isolate the Taker facade on an owner-only service socket

- Status: Accepted; read and conditional admission process through `1664c41`
- Date: 2026-08-03
- Scope: M6 Taker application deployment

## Context

The Maker daemon owns Maker configuration, offers, negotiation, and Maker actor
state. The Taker application owns different private receipts, signing material,
prepared drafts, pair actors, and recovery authority. Adding Taker UI methods
to the Maker daemon would merge compromise domains and make the Maker owner
socket a generic route into Taker authority. Letting QML call Maker Chat or the
existing CLI would expose raw paths, commands, or protocol material.

The repository already uses jsonrpsee over Tokio Unix HTTP/1, strict typed
parameters, an end-to-end bounded client, owner-owned mode-0700 runtime
directories, mode-0600 sockets, bounded bodies and connections, disabled
batches, and inode-safe cleanup. Reusing that boundary is smaller and easier to
audit than introducing TCP, WebSocket, bearer tokens, or another RPC framework.

## Decision

The dedicated `lez-taker-service` now runs on a distinct owner-only Unix
socket and reuses the library `owner_rpc_server` boundary. It never registers
Maker, Chat, generic command, raw payload, or path-selected methods.

The executable registers only methods backed by real logic. Health and
offer-list are always present. ADR 0134 conditionally registers initiation when
the prepared-ZEC context loads; swap-list, monitor, claim, and refund remain
method-not-found, not dishonest success or placeholders.

Health reports this exact deployment boundary: two methods for an empty/read
configuration and three for prepared admission. Pair-level capability metadata
describes reusable actor support and is not presented as evidence that an RPC
or effect worker exists.

Startup configuration is an owner-private, maximum-512-KiB, strict schema-v1
JSON file. The secure loader accepts exact mode 0400 or 0600 on an owner-owned
single-link regular file. It binds zeroizing bytes to the same descriptor's
device, inode, and length, revalidates length around the read, and reopens the
path to reject replacement. The schema accepts zero to 32 pinned Delivery
sources, an optional Chat metadata probe, `maximum_offers` from 1 through
1024, and an optional strict prepared-ZEC context with one existing registry.
Prepared private files and output paths remain service-owned; no receipt
selector, executable, wallet credential, or node endpoint becomes caller
authority. None of the paths, identities, parser details, or adapter errors
cross the RPC response.

```mermaid
flowchart LR
    Qml["Taker QML<br/>planned secret-free replica"]
    Host["Taker UI host<br/>planned typed QtRO adapter"]
    Config["Owner-private service configuration"]
    Socket["Owner-only Taker Unix socket<br/>mode 0600"]
    Service["Running lez-taker-service<br/>HTTP-only jsonrpsee"]
    Delivery["Pinned authenticated Delivery sources"]
    Chat["Optional Chat socket metadata probe"]
    Prepared["Prepared-ZEC authority catalog<br/>service-wired at 1664c41"]
    Registry[("Initiation registry<br/>replay and atomic admission")]
    Worker["Future bounded mutation worker"]
    Maker["Maker daemon and Maker owner socket"]

    Qml -.-> Host
    Host -.-> Socket
    Config --> Service
    Socket --> Service
    Service --> Delivery
    Service --> Chat
    Prepared --> Service
    Prepared --> Registry
    Service --> Registry
    Registry -.-> Worker
```

The absence of an edge between the Taker service and Maker denotes separation:
the Taker service does not enter the Maker owner socket. Solid edges are
implemented components, including prepared authority and registry admission.
The QML, QtRO, mutation worker, and actor edges remain planned. Negotiation
continues only through existing authenticated Delivery and role-fixed Chat
protocol boundaries; admission itself does not call Chat.

## Read-only request flow

```mermaid
sequenceDiagram
    actor U as Taker user
    participant Q as Taker UI
    participant S as Owner-only Taker service
    participant D as Pinned Delivery sources
    U->>Q: Browse supported route
    Q->>S: taker_offer_list_v1 with schema and optional route
    S->>S: Validate schema, route, trusted time, and result bounds
    S->>D: Discover authenticated unexpired offers
    D-->>S: Validated public offers and envelope commitments
    S->>S: Deterministic deduplication and conflict check
    S-->>Q: Secret-free bounded offer list
```

```mermaid
flowchart TD
    Request["Read-only request"] --> Schema{"Schema and route valid"}
    Schema -->|No| Invalid["Fixed invalid-request error"]
    Schema -->|Yes| Dependencies{"Trusted time and pinned Delivery available"}
    Dependencies -->|No| Unavailable["Fixed path-free unavailable error"]
    Dependencies -->|Yes| Merge["Authenticate, bound, sort, and deduplicate"]
    Merge --> Conflict{"Conflicting identity and offer facts"}
    Conflict -->|Yes| Reject["Fail closed without response data"]
    Conflict -->|No| Response["Return secret-free projection"]
```

## Transport and isolation rules

- Socket paths are absolute and their real parent directory is euid-owned mode
  0700.
- Existing endpoint paths are never replaced.
- The bound endpoint is rechecked as an euid-owned mode-0600 Unix socket.
- Cleanup removes only the captured device and inode.
- The server is HTTP-only with batches disabled, at most 16 connections, and a
  64 KiB request/response bound for this secret-free control surface.
- The typed client has one 30-second end-to-end timeout and aborts its Hyper
  connection driver on timeout.
- There is no TCP, WebSocket, bearer-token, generic JSON, CLI shell-out, or
  direct UI-to-Chat/node fallback.

## Atomicity argument

Health and offer listing are read-only and perform no swap or wallet effect.
They fail before returning a partial or unauthenticated offer set when a pinned
source fails, a result cap is exceeded, or duplicate immutable facts conflict.
This does not prove cross-chain atomicity.

ADR 0134 wires ADR 0132 admission into this process. Durable lookup precedes
catalog, clock, and Delivery. A new request must match prepared and
live-authenticated facts before one transaction commits exact public facts,
private authority, and replay; the RPC returns only after durability. This is
local admission atomicity and creates no external effect. A later bounded
worker must enter the existing receipt validator, per-swap lock, generation
fence, and one-attempt pair effect journal. Swap-list, monitor, claim, and
refund remain absent until their real paths exist.

## Consequences

- Maker and Taker compromise domains stay process- and socket-separated.
- The actual service, private startup loader, health, offer list, conditional
  initiation, SIGTERM cleanup, restart replay, and inode-safe cleanup are GREEN.
- The implementation reuses maintained dependencies and already tested custody.
- Initiation returns only durable `Initiating` generation-zero admission.
- At this ADR's original read-only checkpoint, effect workers, durable ZEC
  execution, terminal RPCs, Basecamp host, QML, actor-real UI test, and owner
  prototype sign-off remained M6 work. ADRs 0132 through 0147 now record their
  implemented local-PoC progression and evidence.
