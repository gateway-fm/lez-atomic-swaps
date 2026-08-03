# ADR 0130: expose a strict role-fixed Taker facade

- Status: Accepted; typed contract and authenticated read backend implemented through `1584b76`
- Date: 2026-08-03
- Scope: M6 nonvisual backend boundary

## Context

The Taker UI must browse authenticated offers, initiate one reviewed swap, recover
progress after restart, and request one admissible terminal action. Existing
pair actors already own private receipts, sealed files, keys, node adapters,
effect journals, and per-swap locks. Giving QML a receipt path, socket path,
generic command, raw wire, key, or node endpoint would move authority into the
least trusted process and would bypass the role contracts proven in M2 through
M5.

The current pairs are not semantically identical. Bitcoin and Zcash have
receipt-bound lifecycle commands. The current Monero receipt-v2 work exposes
role-fixed tag checkpoints, but one tag marker is not proof of terminal
cross-chain completion. The facade must report this difference instead of
normalizing it away.

## Decision

The first facade contract is an exact seven-method allowlist:

- `taker_health`;
- `taker_offer_list_v1`;
- `taker_swap_list_v1`;
- `taker_swap_initiate_v1`;
- `taker_swap_monitor_v1`;
- `taker_swap_claim_v1`; and
- `taker_swap_refund_v1`.

Every request is schema-versioned and rejects unknown fields. Initiation carries
only the selected offer ID, exact route, validated compressed Maker identity,
signed-envelope commitment, and reviewed integer amounts. Monitor and terminal
requests carry an opaque swap ID. Claim and refund are different request types,
each with a global request ID and observed generation; the caller cannot choose
a generic action string.

Private material is resolved by the role-fixed service from owner-controlled
configuration and prepared slots. It is never supplied by QML. Health reports
Bitcoin and Zcash terminal routes as full lifecycle and Monero routes as effect
checkpoints only until semantic terminal workers exist.

```mermaid
flowchart LR
    Qml["Taker QML replica<br/>no private authority"]
    Host["Process-isolated Taker UI host<br/>typed allowlist"]
    Facade["Role-fixed Taker facade<br/>seven methods"]
    Offers["Authenticated Delivery offer projection"]
    Receipts[("Private receipt index")]
    Actors["Pair-specific Taker actors"]
    Journals[("Generation fences and effect journals")]
    Nodes["Role-fixed node adapters"]

    Qml -.-> Host
    Host -.-> Facade
    Facade -.-> Offers
    Facade -.-> Receipts
    Facade -.-> Actors
    Actors --> Journals
    Actors --> Nodes
```

Dashed edges remain planned service and host composition. The typed contract is
implemented; it grants no transport or effect authority by itself.

## Initiation and terminal flows

```mermaid
sequenceDiagram
    actor U as Taker user
    participant Q as Taker QML
    participant F as Role-fixed facade
    participant D as Authenticated Delivery
    participant R as Private receipt store
    participant A as Pair actor
    U->>Q: Review exact offer and confirm
    Q->>F: initiate with request ID and public commitments
    F->>D: Resolve and revalidate exact signed offer
    F->>R: Resolve prepared private material
    F->>A: Negotiate and provision role-fixed actor
    A->>R: Persist acceptance and replay result
    R-->>F: Opaque swap projection
    F-->>Q: Initiation commit
```

```mermaid
sequenceDiagram
    actor U as Taker user
    participant Q as Taker QML
    participant F as Role-fixed facade
    participant R as Private receipt and action journal
    participant A as Pair actor
    U->>Q: Confirm claim or refund
    Q->>F: Method-fixed request ID, swap ID, generation
    F->>R: Resolve receipt and compare generation
    alt Exact replay
        R-->>F: Original action commit
    else Stale generation or wrong action
        R-->>F: Reject without effect
    else Fresh admissible action
        F->>A: Enter per-swap lock and admit one action
        A->>R: Persist request and effect state
        R-->>F: Durable action commit
    end
    F-->>Q: Secret-free projection
```

## Atomicity argument

The DTO layer is not itself an atomic-swap protocol and makes no chain-effect
claim. It preserves conditional atomicity by refusing to create a second
authority path. A future service must reuse the existing pair validation,
persist-before-response acceptance, global replay ledger, per-swap kernel lock,
generation fence, and one-attempt effect journal. Exact request replay returns
the original commit; changed request reuse, stale generation, wrong pair,
wrong direction, or unavailable terminal action fails before a new effect.

For Monero, checkpoint-only capability prevents a tag marker from being shown as
`Completed` or `Refunded`. Production semantic workers remain an upstream
readiness item, not a reason to overstate this M6 contract.

## Consequences

- The UI surface is small, typed, versioned, and auditable.
- Pair and direction substitution cannot be hidden in a generic command.
- QML cannot select filesystem, socket, executable, evidence, or key material.
- Prepared private material is an explicit current initiation limitation.
- Service transport, private resolution, Basecamp host wiring, and actor-real
  E2E remain M6 work after prototype sign-off.
