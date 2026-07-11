# ADR 0008: Product direction is constrained by reviewed pair capability

Status: Accepted — 2026-07-11

```mermaid
sequenceDiagram
    participant T as Taker (first funder)
    participant TC as Taker-leg chain
    participant M as Maker (second funder)
    participant MC as Maker-leg chain
    Note over T,M: BTC/ZEC support either asset first; XMR requires LEZ first
    T->>TC: lock with longer refund schedule
    TC-->>M: canonical confirmations reach policy
    M->>MC: lock with shorter refund schedule
    alt cooperative completion
        M->>TC: claim and reveal pair evidence
        TC-->>T: adaptor witness / preimage becomes observable
        T->>MC: claim maker-funded leg
    else timeout
        M->>MC: refund at earlier maker deadline
        T->>TC: refund at later taker deadline
    end
```

## Context

The live RFP requires swaps “between LEZ and” BTC, XMR, and transparent ZEC and
requires the taker to act on-chain before the maker. Neither contractual source
states asset direction. The initial skeleton silently assumed the taker always
sold the foreign asset.

Source review then found an important protocol constraint: COMIT's pinned
`xmr-btc-swap` reference (`dc6ba84bbb1fe5ecc69581fec7dd8529567c4e32`) ships only
the scriptable-chain-first direction and states that XMR-first remains blocked.
Generic role symmetry is therefore not evidence of a safe pair construction.

## Decision

Represent both `TakerSellsForeign` and `TakerSellsLez`, but accept only directions
with a reviewed pair construction. Direction is immutable, signed negotiated
data and is durably stored before any lock. BTC and ZEC currently permit both.
XMR permits only `TakerSellsLez` (LEZ, the scriptable leg, funds first); XMR-first
is rejected in core term validation and at the actual CLI/daemon boundary.

Generic coordination remains role-relative:

1. the taker-funded leg is submitted and reaches its confirmation policy;
2. the maker-funded leg is submitted;
3. the maker claims the taker-funded leg and exposes pair-specific claim evidence;
4. the taker uses that evidence to claim the maker-funded leg.

The second-locking maker's refund matures first. The first-locking taker's refund
matures later by the pair-specific safety margin. Pair adapters map these roles
to LEZ or the foreign chain using direction; the core does not infer safety from
chain names.

## Evidence and consequences

`bidirectional_lifecycle` exercises the LEZ-first direction for all three pairs
and verifies persisted direction. `typed_refund_schedule` rejects XMR-first;
`operator_journey` proves the actual CLI/daemon rejects it while persisting a
supported reverse-direction swap across process kill/restart.

Final BTC/ZEC happy/refund/concurrency matrices run in both directions. XMR runs
LEZ-first unless a separately reviewed XMR-first construction supersedes this
ADR. Typed chain deadlines are implemented; calibrated pair margins remain open.
