# ADR 0127: Layer M5 PoC evidence and map Bitcoin claim intent to Drive

Status: Accepted and verified for the M5 local-functional PoC on 2026-08-02;
bound by `m5-poc-complete`

## Context

The current accepted RFP-003 issue #112 names seven M5 deliverables: daemon,
Maker CLI, Taker CLI, coordinator persistence/crash/concurrency, price sources,
Delivery/Chat degradation, and fuzzing. Earlier scorecards treated fresh
all-pair lifecycle and simultaneous actual-chain composites as additional
literal outputs. Under the progressive-JPEG policy, those are valuable
hardening gates but must not silently redefine the accepted milestone.

The real all-pair Maker CLI matrix also exposed a Bitcoin manual Claim failure
at JSON-RPC code `-32602`. The user intent is valid, but the Bitcoin actor
protocol advances claims through its semantic `drive` command rather than a
generic `claim` command.

## Decision

Count the seven literal outputs only when each has reproducible process or
retained chain evidence. Keep evidence types explicit and composable:

```mermaid
flowchart TB
    RFP["RFP 003 issue 112 seven outputs"] --> Candidate["M5 PoC verified 7 of 7"]
    Control["Current control plane evidence"] --> Candidate
    Chain["Retained local devnet chain evidence"] --> Candidate
    Control --> Maker["Maker CLI all pair matrix"]
    Control --> Taker["Taker Tag14 and Tag16 receipt routes"]
    Control --> Concurrent["One daemon three pair isolation"]
    Chain --> M2["M2 ZEC local devnets"]
    Chain --> M3["M3 Bitcoin Core and LEZ"]
    Chain --> M4["M4 Monero and LEZ"]
    Chain --> M5["M5 accepted application corridors"]
    Candidate --> Gates["Final gates green and exact tag bound"]
    Candidate -.-> Hardening["Post PoC semantic XMR workers and simultaneous actual chain composite"]
    Candidate -.-> Production["Production readiness and public deployment"]
```

Marker-backed matrices prove the real CLI, daemon, RPC, SQLite, scheduler,
fencing, child custody, failure isolation, and replay contracts. They do not
prove chain submission or finality. Chain-effect claims remain attached to the
retained isolated-node evidence.

Translate manual actions at the supervisor boundary by pair semantics:

```mermaid
sequenceDiagram
    actor U as Maker operator
    participant C as lez-maker CLI
    participant D as Maker daemon
    participant S as Supervisor
    participant B as Bitcoin actor
    U->>C: claim with request ID and generation
    C->>D: Persist user Claim intent
    D->>S: Lease exact manual action
    S->>S: Map Bitcoin Claim to Drive
    S->>B: drive under held actor lock
    B-->>S: Pair-semantic status or effect result
    S->>D: Fenced durable resolution
    D-->>C: Current action state
    C-->>U: Stable user-level claim result
```

For ZEC and XMR, Claim maps to Claim. Refund maps to Recover for every pair.
The durable row continues to store the user intent, not the internal actor verb.

## Consequences

The M5 PoC score is verified 7/7 after the final repository gates and is bound by
tag `m5-poc-complete`. This is not production readiness and does not claim
public deployment. Semantic receipt-v2 XMR adapters and a fresh simultaneous
accepted-application actual-chain composite remain QA, chaos, security, and
production-hardening work after PoC.
