# ADR 0150: Withdraw only unhealthy-route advertisements

- Status: Accepted for the automatic Maker route-health control plane
- Date: 2026-08-04

## Context

ADR 0115 made explicit route disablement pair-scoped, but a stopped chain node
did not automatically affect advertisements. A Maker could therefore continue
offering a route whose LEZ or foreign-chain dependency was unavailable. The
repair must not revoke already accepted terms, expose authority to a probe, run
an arbitrary shell, or stall other pairs while a node is slow.

## Decision

The daemon accepts an owner-private strict JSON map from exact routes to one or
more semantic health commands. Operators normally configure one command for the
LEZ dependency and one for the foreign node. Each executable is absolute,
owner/root controlled, non-writable, single-linked, SHA-256 pinned, and checked
before and after each invocation. Arguments are bounded. The daemon clears the
environment, supplies no stdin, discards output, invokes no shell, enforces a
maximum five-second command timeout, kills and reaps timeouts, and treats every
nonzero exit or validation failure as unavailable.

The daemon samples on a bounded periodic cadence in one `spawn_blocking` task.
Missed ticks are skipped and another probe cannot overlap it, so a slow node
cannot accumulate work or block the async owner/Chat accept loop. Quote and new
publication also consult the selected route synchronously and fail before price
or Delivery I/O when it is unavailable.

```mermaid
flowchart LR
    Config[Owner private route health JSON] --> Daemon[Maker daemon]
    Daemon --> Worker[Hash pinned semantic commands]
    Worker --> LezRpc[LEZ RPC or CLI]
    Worker --> BtcRpc[Bitcoin Core RPC or CLI]
    Worker --> XmrRpc[Monero daemon or wallet RPC]
    Worker --> ZecRpc[Zebra RPC or CLI]
    Daemon --> Store[(Maker SQLite)]
    Store --> Delivery[Delivery advertisements]
    Daemon --> OwnerRpc[Owner Unix RPC]
    Daemon --> ChatRpc[Taker Chat Unix RPC]
```

## Automatic withdrawal flow

Only unexpired `Active` offers are candidates. For an unavailable route, the
daemon derives a stable 64-character SHA-256 request ID from the operation
domain, offer ID, and expected revision, then uses the existing SQLite
compare-and-swap withdrawal. The durable withdrawal precedes best-effort
Delivery cleanup. A concurrent reservation wins by advancing the offer
revision; the stale withdrawal is ignored and the reserved negotiation stays
valid. Reserved and consumed records are never selected for withdrawal.

```mermaid
sequenceDiagram
    participant Timer as Daemon timer
    participant Probe as Semantic probe
    participant Store as Maker SQLite
    participant Delivery as Delivery
    participant Chat as Accepted Chat or actor
    Timer->>Probe: Check exact route commands
    Probe-->>Timer: Unavailable
    Timer->>Store: List active unexpired offers
    Timer->>Store: CAS withdraw offer at revision
    alt Withdrawal wins
        Store-->>Timer: Withdrawn at next revision
        Timer->>Delivery: Remove advertisement
    else Reservation won first
        Store-->>Timer: Stale or unavailable
        Timer-->>Chat: Preserve reserved negotiation
    end
```

## Pair isolation and atomicity argument

This control plane creates no chain effect and does not claim distributed
cross-chain atomicity. Its atomic property is the SQLite offer-state
linearization point. An offer can transition from `Active` to exactly one of
`Reserved` or `Withdrawn`; compare-and-swap revisions prevent both from winning.
Once reserved, expiry or later dependency loss cannot revoke the signed terms.
Post-lock actors retain their independent journals and chain authority and do
not depend on Delivery, Chat, or this health map.

Route keys contain pair plus direction. Health loss for one key neither mutates
another key nor disables its RPC path. The focused contract proves an unhealthy
Zcash offer is withdrawn, a reserved Zcash negotiation survives, an independent
Bitcoin offer and quote remain active, and new Zcash quote/publication fail
closed. The process contract proves the periodic daemon performs withdrawal
without a health request.

```mermaid
flowchart TD
    Active[Active offer at revision N] --> Race{First durable CAS}
    Race -->|Taker reservation| Reserved[Reserved at revision N plus 1]
    Race -->|Health withdrawal| Withdrawn[Withdrawn at revision N plus 1]
    Reserved --> Actor[Accepted swap actor continues]
    Withdrawn --> NoNew[No new negotiation]
    BadZec[Unavailable ZEC route] -. no mutation .-> GoodBtc[Available BTC route]
```

## Consequences and evidence boundary

- The health executable is an adapter, not a new chain implementation; official
  node CLIs or small reviewed semantic adapters can be used per deployment.
- Program and argument changes are configuration changes, while executable
  identity drift fails closed.
- A route absent from a configured map is unavailable and fails closed. When no
  probe map is configured at all, routes report `disabled` for legacy behavior;
  the additive health response defaults missing route rows for rolling clients.
- A failed Delivery cleanup cannot reactivate the durable offer. Delivery health
  exposes stale projection state and startup reconciliation removes it.
- `run-m7-unaffected-pair-outage-poc.sh` composes the literal proof boundary. It
  provisions an isolated Bitcoin Core 31.1 Regtest service, authenticates its
  network and genesis, stops that exact labelled container, and then runs the
  ordinary Zcash application corridor. The same Maker daemon probes both the
  unavailable Bitcoin route and surviving Zebra route through one hash-pinned
  adapter. A fresh retained successful execution is still required before F1
  or R3 changes from `open` to `green`.
- Fresh run `m7outage-5e9d47d-a` proved the stopped Bitcoin node was
  semantically unavailable, then failed before daemon readiness and before any
  swap submission because the checked-out `scripts/` directory was group
  writable and therefore correctly rejected by the executable-custody policy.
  The harness now copies the source-hashed probe into its owner-private `0700`
  proof directory with mode `0500`, checks the staged digest is identical, and
  passes only that immutable path to the daemon. The retained run is RED
  diagnostic evidence, not an F1/R3 certificate.

- Clean run `m7outage-f482acd-a` passed immutable probe custody, reported the
  stopped Bitcoin route unavailable and live Zcash route available before and
  after Maker restart, rejected the Bitcoin quote, and atomically accepted the
  Zcash application. It then stopped before actor handoff because the baseline
  restart assertion expected only one configured route while the M7 harness
  intentionally retains the disabled Bitcoin route for operator visibility.
  The assertion is now mode-aware: normal M5 still requires exactly one Zcash
  route, while M7 requires that route plus the exact disabled Bitcoin route.
  No role actor or chain submission ran; this remains bounded RED evidence.

```mermaid
flowchart LR
    BtcStart[Bitcoin Core Regtest healthy] --> BtcStop[Stop exact run container]
    BtcStop --> BtcProbe[Bitcoin semantic probe unavailable]
    Zebra[Zebra Regtest healthy] --> ZecProbe[Zcash semantic probe available]
    BtcProbe --> Maker[Maker daemon]
    ZecProbe --> Maker
    Maker --> Reject[Bitcoin quote rejected]
    Maker --> ZecSwap[Zcash Maker and Taker corridor]
    ZecSwap --> Lez[LEZ finalized claim]
    ZecSwap --> ZecClaim[Zcash confirmed claim]
```

```mermaid
sequenceDiagram
    participant Harness as M7 run harness
    participant Bitcoin as Bitcoin Core Regtest
    participant Maker as Maker daemon
    participant Zebra as Zebra Regtest
    participant Users as Maker CLI and Taker CLI
    participant Actors as Maker and Taker actors
    Harness->>Bitcoin: Validate regtest genesis and height
    Harness->>Bitcoin: Stop exact labelled container
    Harness->>Maker: Start with hash-pinned two-route probe map
    Maker->>Bitcoin: Semantic chain and genesis probe
    Bitcoin--xMaker: Connection unavailable
    Maker->>Zebra: Height and genesis probe
    Zebra-->>Maker: Expected Regtest identity
    Users->>Maker: Configure and publish Zcash offer
    Users->>Maker: Attempt Bitcoin quote
    Maker--xUsers: Reject unavailable route
    Users->>Maker: Discover, reserve, and countersign Zcash swap
    Actors->>Zebra: Fund then claim after LEZ revelation
    Actors-->>Users: Both roles completed
    Maker->>Bitcoin: Probe remains unavailable after restart
    Maker->>Zebra: Probe remains available after restart
```

The outage harness changes no swap atomicity argument. The Zcash corridor keeps
the signed foreign-first ordering: confirmed Zcash funding precedes the
revealing LEZ claim, and only that revelation enables the Zcash claimant. The
route-health control has no chain authority and cannot make a partial swap; it
only gates new offers and quotes before acceptance. Once accepted, the
role-separated actor journals continue independently of route health, Delivery,
and Chat.
