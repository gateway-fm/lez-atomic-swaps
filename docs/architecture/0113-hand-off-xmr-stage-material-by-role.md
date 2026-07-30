# ADR 0113: Hand off XMR stage material without crossing role authority

- Status: Accepted for the M5 application process boundary
- Date: 2026-07-30
- Milestone: M5 progressive local-functional PoC

## Context

ADR 0112 makes Stage-B activation one atomic application-store transition, but
its component tests call the store directly. M5 also needs the path an actual
operator uses: authenticated offer discovery in `lez-taker`, an isolated Chat
socket on `lez-maker-daemon`, and separate Maker and Taker role material.

The existing M4 processes already produce canonical dual-signed Stage A,
canonical dual-signed Stage B, separate public role packets, and separate
role-local adaptor journals. Reimplementing that cryptography inside Chat would
duplicate a proven wheel and would expand the daemon's custody. The first M5
application PoC therefore consumes those reproducible artifacts while retaining
their role boundary.

## Decision

1. The Taker discovers and authenticates a signed run-local Delivery offer,
   validates the exact `Monero` plus `TakerSellsLez` route and no-rounding quote,
   and derives the public swap ID from the offer commitment and reservation.
2. Chat accepts only the signed Delivery envelope, reservation, public
   principal, and canonical public Stage-A or Stage-B wire. Private roots,
   signing keys, Monero shares, the shared view-key file, adaptor journals, and
   the unpublished Taker claim partial never cross Chat.
3. The Maker daemon starts with a bounded registry of Maker-only authority. It
   pins the Maker agreement identity, private Monero view key, and immutable
   actor manifest for each derived swap ID. The Taker cannot supply any of
   those paths through an RPC request.
4. Stage A authenticates Delivery and rederives the route, quote, swap ID,
   agreement identity, and shared-view public key before reserving the offer.
   An exact durable replay remains valid after public advertisement expiry;
   only a fresh reservation applies the half-open Delivery TTL.
5. Stage B reloads Stage A from SQLite, validates the activation with the
   daemon-owned private view key, derives the coordinator in the XMR SDK,
   selects the daemon-owned actor manifest by that derived ID, and invokes the
   single transaction defined by ADR 0112.
6. Each role publishes a no-clobber application bundle. It copies only the
   canonical public stage wires and stores a role-fixed manifest containing
   digests of the original private root, role packet pair, view-key material,
   and role journal. Private authority and the journal remain at their original
   owner-only paths rather than being duplicated.
7. The Taker publishes its digest-bound acceptance receipt only after the Maker
   returns a durable Stage-B commit. Exact replay revalidates all sources,
   preserves published inodes, needs no Delivery advertisement, and creates no
   second coordinator or actor.
8. Owner-control RPC keeps its smaller request bound. The isolated Chat service
   has a separate bounded body limit large enough for the SDK's canonical XMR
   wire maxima plus JSON encoding overhead.

## Components and authority

```mermaid
flowchart LR
    subgraph TakerHost["Taker owner boundary"]
        TakerCli["lez-taker"]
        TakerRoot["Taker private role root"]
        TakerJournal[("Taker adaptor journal")]
        TakerBundle["No-clobber Taker bundle"]
        Receipt["Acceptance receipt"]
    end

    subgraph PublicExchange["Public authenticated material"]
        Delivery["Signed run-local Delivery"]
        StageA["Dual-signed Stage A"]
        StageB["Dual-signed Stage B"]
    end

    subgraph MakerHost["Maker daemon owner boundary"]
        Chat["Isolated Chat Unix socket"]
        MakerAuthority["Maker identity, view key, actor registry"]
        MakerStore[("Application SQLite")]
        MakerBundle["No-clobber Maker bundle"]
    end

    Delivery --> TakerCli
    TakerRoot --> TakerCli
    TakerJournal --> TakerCli
    StageA --> TakerCli
    StageB --> TakerCli
    TakerCli -->|"envelope, terms, Stage A or B only"| Chat
    MakerAuthority --> Chat
    Chat --> MakerStore
    TakerCli --> TakerBundle
    TakerBundle --> Receipt
    MakerAuthority --> MakerBundle
```

The apparent public exchange does not imply public networking in the local
PoC. Delivery is a signed run-local directory and Chat is an owner-controlled
Unix socket. The diagram distinguishes disclosure class and role custody.

## Process and replay flow

```mermaid
sequenceDiagram
    actor User
    participant Taker as lez-taker
    participant Delivery as Signed Delivery
    participant Chat as Maker Chat socket
    participant SDK as XMR SDK
    participant Store as Maker SQLite
    participant Bundle as Role bundle and receipt

    User->>Taker: Select offer and role-separated Stage A and B
    Taker->>Delivery: Authenticate exact envelope and quote
    Delivery-->>Taker: Offer commitment
    Taker->>Taker: Derive swap ID and validate Stage A
    Taker->>Chat: Stage-A request with public wire
    Chat->>SDK: Canonically validate Stage A and authority
    Chat->>Store: Reserve exact offer
    Store-->>Chat: Durable revision 2
    Chat-->>Taker: Stage-A receipt
    Taker->>Taker: Validate Stage B and publish Taker bundle
    Taker->>Chat: Stage-B request with public activation wire
    Chat->>Store: Reload exact durable Stage A
    Chat->>SDK: Validate Stage B with Maker-owned view key
    SDK-->>Chat: Derived initial coordinator
    Chat->>Store: Activate with daemon-owned actor manifest
    Store-->>Chat: Atomic durable revision 3
    Chat-->>Taker: Activation receipt
    Taker->>Bundle: Publish acceptance receipt no-clobber

    opt Exact replay after Delivery removal
        User->>Taker: Repeat acceptance
        Taker->>Chat: Exact Stage-B request
        Chat->>Store: Revalidate replay record and all durable rows
        Store-->>Chat: Original revision 3, replay
        Chat-->>Taker: Identical activation receipt
        Taker->>Bundle: Revalidate bytes, digests, and inodes
    end
```

## Why the handoff remains atomic

```mermaid
flowchart TD
    A["Stage A request"] --> R["SQLite offer reservation"]
    R --> P["No executable coordinator or actor"]
    P --> B["Stage B request"]
    B --> V["Validate with daemon-owned Maker authority"]
    V --> D["Derive coordinator and select actor by swap ID"]
    D --> T["One SQLite immediate transaction"]
    T --> C{"Commit succeeds?"}
    C -->|"yes"| E["Offer consumed, coordinator and one actor visible"]
    C -->|"no"| N["Stage A remains reserved, no executable authority"]
    E --> Q["Taker receipt published"]
    N --> X["No success receipt"]
```

Local atomicity is the indivisible Stage-B SQLite commit. Filesystem publication
does not participate in that transaction: Maker actor material must already be
present and digest-pinned, and a Taker receipt is only an owner-local projection
published after the commit. A crash before the commit exposes no executable
coordinator; a crash after it is recovered through exact completion replay.

This still is not a distributed transaction across LEZ and Monero. Cross-chain
conditional atomicity comes from the signed protocol: Stage B fixes the claim
and refund sessions, LEZ locks first, Monero funding is admitted only after the
exact finalized LEZ condition, and the successful or timeout branch reveals
only the share needed by its corresponding spend.

## Consequences and remaining work

- The first process slice reuses actual M4 role processes and does not yet turn
  Chat into an interactive nonce or partial-signature exchange.
- Both roles intentionally retain the shared Monero view key established by the
  M4 provisioning handoff; spend and agreement keys remain role-private.
- Canonical Stage A can exceed the owner-control RPC body limit, so Chat and
  control listeners must not share the same size policy.
- The semantic XMR supervisor adapter and exact isolated LEZ plus Monero
  application replay remain the next gates before M5 certification.
