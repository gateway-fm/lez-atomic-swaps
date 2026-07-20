# ADR 0071: Durably prepare and complete the exact XMR tag-15 claim before submission

- Status: Accepted as a component checkpoint; actual-chain composition pending
- Date: 2026-07-20
- Milestone: M4 progressive local-functional PoC

## Context

The checked M4 guest accepts `ClaimNativeXmr` as tag 15 only when the claim
authority supplies the aggregate BIP340 witness for the exact generated
message. Stage A commits `claim_message_hash`, so a host that chooses another
nonce, account order, instruction, or program cannot repair the mismatch after
either lock. Returning unsigned or completed bytes without durable ownership
would also make restart regenerate or replay a different effect.

ADR 0063 durably prepares tag 14, ADR 0067 gives tag 14 a dedicated one-attempt
submission path, and ADR 0070 prevents Fund before exact finalized Initialize.
Those decisions do not construct the Maker-owned tag-15 claim. Conversely,
building tag 15 does not prove that tag 14 is finalized or authorize a node
send.

## Decision

The Maker sidecar implements the existing strict
`prepare_native_xmr_claim_v3` and `complete_native_xmr_claim_v3` protocol
methods. No parallel wire type or alternate transaction library is introduced.

Preparation:

1. requires Maker role and the exact signed runtime and XMR terms;
2. derives the aggregate authority account from the committed x-only public key;
3. reads that account's nonce once;
4. builds generated tag 15 with ordered
   `[metadata, custody, claimant, claim_authority]` accounts;
5. requires the resulting message hash to equal the immutable Stage-A
   `claim_message_hash`; and
6. owner-only persists the exact unsigned message before returning it.

Completion reloads and revalidates that exact reservation, verifies the
aggregate BIP340 signature against the exact message and authority, constructs
one canonical public transaction, and owner-only persists it in a separate
completion record. Neither method submits. The bridge request journal retains
both request bodies and rederives both successful results during startup, so a
missing, corrupt, or inconsistent planner record prevents cached-success replay.

```mermaid
flowchart LR
    Terms["Signed Stage A and Stage B terms"] --> Prepare["Maker tag-15 prepare"]
    Authority["Aggregate authority nonce"] --> Prepare
    Prepare --> Check["Generated ABI accounts and immutable hash check"]
    Check --> Prepared[("Owner-only unsigned reservation")]
    Prepared --> Complete["Aggregate BIP340 completion"]
    Signature["Aggregate adaptor witness"] --> Complete
    Complete --> Verify["Signature and canonical-byte verification"]
    Verify --> Completed[("Owner-only completed transaction")]
    Completed -.-> Submit["Exact tag-15 submission pending"]
    Tag14Finality["Exact finalized tag 14 pending"] -.-> Prepare
```

Solid edges are component-GREEN and perform zero sequencer sends. Dotted edges
belong to the actual-local actor composition.

## Component flow and restart rule

```mermaid
sequenceDiagram
    actor Maker
    participant Client as Ordinary BridgeClient
    participant Server as Maker sidecar server
    participant Journal as Bridge request journal
    participant Planner as Durable tag-15 planner
    participant Nonce as LEZ nonce source

    Maker->>Client: Prepare exact tag 15
    Client->>Server: Authenticated Maker request
    Server->>Planner: Validate role runtime terms and authority
    Planner->>Nonce: Read aggregate-authority nonce once
    Nonce-->>Planner: Exact nonce
    Planner->>Planner: Build generated ABI and check committed hash
    Planner->>Planner: Persist unsigned reservation before return
    Planner-->>Server: Exact unsigned message
    Server->>Journal: Persist request and success
    Server-->>Maker: Prepared claim
    Maker->>Client: Complete with aggregate BIP340 signature
    Client->>Server: Authenticated completion request
    Server->>Planner: Reload exact preparation and verify signature
    Planner->>Planner: Persist canonical transaction before return
    Planner-->>Server: Exact completed transaction
    Server->>Journal: Persist request and success
    Server-->>Maker: Completed claim with zero submission
    Maker->>Server: Restart with the same journal and planner root
    Server->>Journal: Restore both request bodies
    Server->>Planner: Rederive and revalidate prepare then complete
    Planner-->>Server: Byte-identical results with no new nonce read
```

## Atomicity argument

This decision preserves only the host-side preconditions needed by the XMR
economic construction; it is not a cross-chain atomic commit:

- Tag 15 cannot be silently rebound after locks because the exact nonce-dependent
  message hash is already committed in Stage A.
- Completion cannot attach an arbitrary witness because the pinned BIP340
  verifier checks the aggregate signature for that exact message and authority.
- Persist-before-return plus startup rederivation prevents a successful cached
  response from surviving the loss or corruption of its owned planner record.
- Zero-send preparation keeps construction separate from publication authority.
- The actor must still prove exact finalized tag 14 before preparation and must
  submit and finalize the exact completed tag 15 before treating Maker's share
  as canonically revealed.

Under the supported `TakerSellsLez` flow, canonical tag-15 execution reveals
Maker share `s_a`; the Taker combines it with retained `s_b` to spend the
Monero output. If no canonical claim occurs, the signed-refund/punishment
branches described in ADR 0055 remain the recovery mechanism. This component
does not make those branches or an actual swap GREEN.

## External resources and flakiness

The component tests use authenticated in-process literal-loopback sidecars, an
official-type sequencer fixture for the earlier tag-14 admission, a deterministic
nonce source, and owner-only temporary directories. They use no Docker, actual
chain node, public RPC, peer, faucet, public funds, or external finality service.
After locked dependencies and pinned Rapisnark libraries are cached, runtime
external resources are empty. Cold dependency acquisition can still fail due
to registry, Git, or pinned native-library availability; that is setup
flakiness, not chain-finality evidence.

## Consequences and residuals

- Four of seven transaction-building routes are now functional; refund prepare,
  refund complete, and punishment prepare remain fail-closed `Unavailable`.
- Actual-local tag-14 finality, tag-15 submission/finality, adaptor extraction,
  and fresh Maker/Taker actor ownership remain required for the happy PoC.
- Stage A construction must coordinate or pre-reserve the aggregate-authority
  nonce before the immutable claim-message hash is signed.
- The bridge journal's inherited same-request-ID concurrent overwrite race is a
  post-PoC hardening item; the certified progressive path remains one actor and
  one in-flight request per swap.
- Generic tag-14 submission stays closed. Tag-15 publication must gain its own
  exact-byte, one-attempt, finalized observation path rather than broadening
  authorization publication authority.

## Verification

The actor-realistic regression first submits the exact durable tag-14
authorization through the dedicated route, then prepares and completes tag 15.
It checks exact ABI/accounts/nonce/hash, aggregate signature, zero tag-15 sends,
wrong reservation/terms/signature/role/runtime failures, and exact replay across
a fresh server/planner. Missing or corrupt durable state fails closed rather
than returning a cached success. The complete pinned sidecar suite, strict
Clippy, warning-fatal Rustdoc, formatting, dependency policy, and diff hygiene
remain milestone gates.
