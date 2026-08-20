# ADR 0011: Deadline and event-gated recovery profiles

Status: Accepted and represented in core — 2026-07-11

```mermaid
flowchart TB
    Terms["Pair + direction + named network profile"] --> Kind{"Maker-funded leg"}
    Kind -->|BTC/ZEC/LEZ| Deadline["Deadline trigger<br/>typed chain + basis + position"]
    Kind -->|Monero| Event["Canonical LEZ refund event<br/>plus confirmation policy"]
    Deadline --> Bounds["Conservative wall-clock safety bounds"]
    Event --> Share["Persisted key-share recovery"]
    Bounds --> Recovery["Role recovery action"]
    Share --> Recovery
    Recovery --> Store["Persist trigger + evidence + action idempotently"]
```

## Context

ADR 0010 correctly prevents numeric comparison of unrelated chain clocks, but
the prototype generalized every recovery as a deadline. The reviewed LEZ-first
COMIT construction has no Monero script or refund timelock. Treating a Monero
height as a maker refund deadline would invent a security property the chain
does not provide.

## Decision

Recovery terms use a tagged trigger:

- `Deadline(ChainPosition)` for LEZ, Bitcoin CSV, and Zcash CLTV paths; or
- `CanonicalEvent { chain, event, confirmations }` for the XMR maker recovery
  unlocked by the taker's canonical LEZ refund and persisted recovery share.

`public-testnet-v1` fixes named confirmation depths and direction-specific
horizons in the M1 parameter profile. Mainnet has no enabled profile until
telemetry, fee/reorg stress tests, value policy, and formal review are complete.

## Consequences

`RecoverySchedule` represents both variants without accepting a Monero deadline.
RED tests assert that XMR cannot recover before canonical LEZ refund evidence,
revokes availability if confirmations regress before maker recovery, can recover
after restart, and rejects XMR-first terms. The maker RPC uses tagged
deadline/XMR-event requests, so the actual CLI does not expose a required fake
Monero deadline.
