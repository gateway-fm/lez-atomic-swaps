# ADR 0092: Let the taker own validation and countersigning

- Status: Accepted; real taker acceptance and post-expiry completion retry GREEN
- Date: 2026-07-24
- Milestone: M5 progressive local-functional PoC

## Context

ADRs 0090 and 0091 prove both maker-side Chat mutations, but their first
process proof still performed the taker operations inside the Rust test. That
does not emulate how a user accepts an offer and leaves ambiguity about which
process reads the taker key, validates changed terms, and persists the result.

## Decision

Extend the real `lez-taker` binary with one explicit ZEC acceptance mode while
preserving its discovery-only invocation. The taker receives a pinned maker
public key, exact offer and reservation IDs, selected amount, owner-private
unsigned draft, raw owner-private taker key, Chat socket, and a new agreement
output path.

The CLI independently authenticates Delivery, selects the exact live
`Zcash/TakerSellsLez` offer, validates the unsigned draft at its trusted local
time, and proves that the draft identities match the local taker key and pinned
maker. It sends the authenticated envelope and draft to Chat, then validates
the returned maker signature and byte-exact body before signing. The taker
never accepts a maker-changed amount, identity, transcript, chain binding,
deadline, destination, or executable policy.

Mutation request IDs are deterministic hashes of the reservation plus a
domain-separated stage label. Retrying the same user command therefore reaches
the maker's existing exact-replay records. Before requesting final completion,
the CLI publishes the countersigned wire through a mode-0600 temporary file and
`persist_noclobber`, syncs the file and directory, and accepts an existing file
only when its bounded, descriptor-revalidated bytes are identical. A lost RPC
response therefore cannot lose the only countersigned recovery artifact.

When that file already exists, the CLI skips Delivery discovery and proposal.
It reopens and validates the private agreement against the executable unsigned
draft, pinned Maker identity, local Taker key/role, amount, signatures, and swap
identity, then retries only the deterministic completion request. The daemon
returns an expired agreement's original result only after SQLite exact-matches
the full committed request, negotiation, and immutable scheduled actor row.

The daemon and taker share one hardened private-file implementation. It opens
without following symlinks, checks effective ownership, mode 0600, one link,
regular type and bounds, and rechecks device, inode and length after reading.
No secret appears in the versioned JSON output.

## Components and authority

```mermaid
flowchart LR
    User[Taker user] --> CLI[lez-taker process]
    Delivery[Signed Delivery directory] --> CLI
    MakerPin[Pinned maker public key] --> CLI
    Draft[Unsigned public terms mode 0600] --> CLI
    TakerKey[Raw taker key mode 0600] --> Loader[Hardened bounded loader]
    Loader --> CLI
    CLI -->|Proposal request| Chat[Maker Chat socket]
    Chat -->|Maker-signed proposal after commit| CLI
    CLI --> Publish[No-clobber exact-wire publisher]
    Publish --> Agreement[Final agreement mode 0600]
    Agreement -->|Validated persisted wire| CLI
    CLI -->|Completion request| Chat
    Chat -->|Revision 3 or exact committed replay| CLI
```

The maker never receives the taker secret key. The taker never receives maker
claim or recovery authority. Delivery and Chat carry only authenticated public
terms, signatures, and bounded identifiers.

## User acceptance sequence

```mermaid
sequenceDiagram
    actor U as Taker user
    participant T as lez-taker
    participant D as Delivery
    participant C as Maker Chat
    participant S as Maker SQLite
    participant F as Taker agreement file

    U->>T: Accept exact offer with draft and local key paths
    T->>D: Discover exact route at trusted time
    D-->>T: Signed envelope and immutable offer
    T->>T: Validate draft identities, amount, expiry and executable terms
    T->>C: Proposal request with exact envelope and draft
    C->>S: Reserve offer and stage maker proposal atomically
    S-->>C: Commit revision 2
    C-->>T: Exact maker-signed proposal
    T->>T: Verify signature and exact-body equality
    T->>T: Countersign exact commitment with taker key
    T->>F: Sync temporary and persist without replacement
    T->>C: Complete request with dual-signed wire
    C->>S: Accept agreement, protect claim material and consume offer atomically
    S-->>C: Commit revision 3
    C-->>T: Agreement-derived swap ID
    T-->>U: Secret-free versioned result

    U->>T: Repeat exact command after expiry or lost response
    T->>F: Open bounded private agreement
    T->>T: Revalidate draft roles amount signatures and swap
    T->>C: Retry only deterministic completion request
    C->>S: Preflight exact request negotiation and scheduled actor
    S-->>C: Original committed revision and swap
    C-->>T: Exact replay without current-time acceptance
    T-->>U: Secret-free replay result
```

## Atomicity argument

This flow does not claim a distributed transaction across the taker file and
maker database. Its safety comes from ordering and replay:

1. the maker proposal is not returned before reservation and proposal bytes
   commit together;
2. the final completion response is not returned before agreement,
   coordinator, binding, encrypted maker claim material, offer consumption,
   and replay result commit together;
3. the taker durably writes only the exact dual-signed wire it locally validated
   before attempting final completion;
4. if the RPC is lost before or after maker commit, that persisted artifact
   drives the same deterministic request; an exact committed result replays
   after expiry, while an uncommitted expired agreement remains rejected; and
5. an existing different output is never overwritten.

Thus a crash can leave a recoverable private agreement awaiting completion, or
that same agreement fully committed at the maker; it cannot create conflicting
accepted bytes or a partially consumed maker offer. Cross-chain atomicity still
begins only after both role actors activate these terms and execute the reviewed
HTLC lifecycle; this pre-lock decision does not itself move funds.

## Evidence and limitations

The `zec_chat_process` test now launches the actual maker daemon and actual
`lez-taker` binary. The CLI authenticates the offer, reads its own raw key,
validates and countersigns, persists the final wire, receives revision 3, waits
past the three-second offer/agreement TTL, then repeats the identical command.
The second process reads the private wire, makes no Delivery/proposal request,
and exact-replays completion. Store tests separately prove reopen, rollback,
changed-input conflicts, and missing-actor failure.

The process test uses no node, chain RPC, Docker, faucet, public funds, DNS,
public finality source, or Logos service. It proves the application negotiation
boundary and acceptance-to-scheduler handoff, not actor execution or a
cross-chain effect. A per-swap authority registry, pair-neutral supervisor,
actor-bearing systemd execution, actual local-node crash composition, and
status/claim/refund taker commands remain.
