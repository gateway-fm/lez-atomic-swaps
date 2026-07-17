# ADR 0042: Bind witnessed token claims to exact ATAs

Status: Accepted at the checked-guest component boundary. Pushed commit
`66d5e26cd35c6282c0cd420533f70e6ea3e506c9` adds the implementation and
focused evidence. The checked manifest, public IDL, deployer assembly, verifier,
and active M3 runner pins now share the new guest identity. The additive strict
v2 transaction, finalized-classifier, exact-once client, and official sidecar
planner boundaries are also implemented. Sidecar routes/scans, adapter/journal/
actor composition, and actual-node custom-token evidence remain open.

## Context

RFP F7 requires the LEZ side of a BTC swap to support native value and custom
fungible tokens. The existing v0.2 token instructions used SHA-256 preimage
authority, which serves the ZEC construction but cannot consume the aggregate
BIP-340 witness used by the BTC construction. Treating an escrow metadata PDA
as a token holding would also bypass the official asset model: custom-token
custody must be the associated token account derived from the metadata account
and exact fungible definition.

The claim transition changes two kinds of state. Escrow metadata becomes
terminal, while the custody holding moves to the immutable claimant ATA through
the ATA program and its nested Token call. Those changes must succeed or fail
together. A metadata-only `Claimed` result with funded token custody would be
unsafe even if a later retry could repair it.

The public instruction discriminant is declaration-order derived. Inserting a
new instruction among existing declarations would silently change the wire
encoding already used by native and preimage-token integrations.

## Decision

Append `InitializeTokenWitnessed` and `ClaimTokenWitnessed` as wire tags 11 and
12. Keep tags 0 through 10 byte-stable. Reuse one token-term validator and one
token-claim validator for preimage and aggregate-witness authority rather than
creating a second custody interpretation.

Initialization binds all of the following into version-2 escrow metadata:

- swap ID, terms hash, nonzero amount, and exclusive claim/inclusive refund
  boundary;
- depositor and claimant owner accounts;
- one Token-program-owned fungible definition;
- exact depositor and claimant ATAs for that definition;
- exact custody `ATA(metadata, definition)` and the official ATA program;
- a nonzero aggregate x-only key and its exact LEZ v0.2 account derivation;
- an aggregate authority account distinct from the claimant.

Custody creation stays permissionless but can create only the derived custody
ATA. Funding remains depositor-owner signed and transfers the exact amount from
the immutable depositor ATA into empty custody. A witnessed claim requires the
aggregate authority signer, a `Funded` escrow, exact amount in custody, the
same fungible definition in custody and destination holdings, and the immutable
claimant ATA. The existing post-deadline refund stays permissionless and can
transfer only to the immutable depositor ATA.

## Components and authority

```mermaid
flowchart TB
    Depositor["Depositor owner"]
    Claimant["Immutable claimant owner"]
    Aggregate["Two-party aggregate LEZ signer"]
    Caller["Permissionless custody or refund caller"]
    Escrow["Witnessed escrow program"]
    Metadata[("Escrow metadata PDA")]
    Definition[("Fungible token definition")]
    DepositorAta[("ATA depositor and definition")]
    CustodyAta[("ATA metadata and definition")]
    ClaimantAta[("ATA claimant and definition")]
    AtaProgram["Official ATA program"]
    TokenProgram["Official Token program"]

    Depositor -->|"sign initialize and fund"| Escrow
    Claimant -->|"fixed destination only"| Escrow
    Aggregate -->|"sign exact witnessed claim"| Escrow
    Caller -->|"create custody or refund after deadline"| Escrow
    Definition --> Escrow
    DepositorAta --> Escrow
    CustodyAta --> Escrow
    ClaimantAta --> Escrow
    Escrow --> Metadata
    Escrow --> AtaProgram
    AtaProgram --> TokenProgram
    TokenProgram --> DepositorAta
    TokenProgram --> CustodyAta
    TokenProgram --> ClaimantAta
```

The claimant owner is an immutable destination, not the witnessed claim
signer. The aggregate LEZ account is the transaction authority and is derived
from the exact x-only key committed at initialization. The escrow metadata PDA
authorizes only the nested custody transfer through its exact PDA seed.

## Recursive atomic transition

```mermaid
sequenceDiagram
    participant D as Depositor owner
    participant P as Permissionless caller
    participant E as Escrow guest
    participant M as Metadata PDA
    participant A as ATA program
    participant T as Token program
    participant C as Custody ATA
    participant R as Claimant ATA
    participant W as Aggregate witness

    D->>E: Initialize witnessed token terms
    E->>E: Validate definition, ATAs, amount, deadline, and authority
    E->>M: Produce Empty metadata
    P->>E: Create only the exact derived custody ATA
    E->>A: Recursive create with metadata and definition
    A->>T: Initialize the empty fungible holding
    D->>E: Fund exact amount
    E->>A: Recursive transfer depositor ATA to custody ATA
    A->>T: Validate and apply fungible holding transfer
    E->>M: Produce Funded metadata in the same recursive output
    W->>E: Claim with exact aggregate account signature
    E->>E: Revalidate definition, custody amount, claimant ATA, and authority
    E->>A: Recursive transfer custody ATA to claimant ATA
    A->>T: Validate exact definition and amount
    E->>M: Produce Claimed metadata
    alt every recursive program accepts
        E-->>W: Commit metadata and token holdings atomically
    else wrong definition, ATA, authority, witness, or amount
        E-->>W: Reject and commit no metadata or token state
    end
```

One instruction returns one `SpelOutput` containing the metadata post-state and
the chained ATA transfer. LEZ recursively validates the escrow, ATA, and Token
sessions before committing the output. Therefore a failed nested transfer
cannot leave terminal metadata, and a rejected witness cannot move custody.
Initialization, custody creation, funding, and claim are still separate LEZ
transactions; the decision does not pretend that the whole escrow lifecycle is
one transaction.

## Atomicity boundary

This decision gives the LEZ claim or refund transaction atomic state change
across the escrow metadata, ATA, and nested fungible-token holding state. It
does not create an atomic transaction with Bitcoin, a role-local database, or
an RPC submission journal. Cross-chain atomicity remains conditional on the
existing protocol: both adaptor sessions and recovery material are durable
before funding, the Taker locks first, both locks are canonical before witness
release, and the opposite presignature can be completed from the revealed
scalar. Canonical observation, persist-before-send authority, and timelocks
remain outside this guest transition.

## Evidence

Commit `66d5e26` retains these exact component facts:

- witnessed-token instructions occupy tags 11 and 12 while a regression check
  proves tags 0 through 10 unchanged;
- host guest tests exercise two independent fungible definitions and reject
  wrong definition, wrong claimant ATA, and unrelated aggregate authority in
  `witnessed_token_claims_bind_two_definitions_exact_atas_and_aggregate_authority`
  and
  `witnessed_token_paths_reject_wrong_definition_ata_and_aggregate_authority`;
- the recursive checked-guest test
  `checked_guest_witnessed_token_claims_require_exact_definition_ata_authority_and_witness`
  additionally rejects one-share authority and proves rejected attempts leave
  metadata `Funded` and custody unchanged;
- exact two-party aggregate witnesses claim both definitions to their fixed
  claimant ATAs;
- the rebuilt guest ELF SHA-256 is
  `bc2ea18eaacb917727934fcf0366dd54c1f9a2b69b61ea53080c926850967fd7` and
  its ImageID is
  `f3ead24b95d316ce91980cb3531a70b83a27fd1640f47c1b857757aef26c244e`.

## Consequences and remaining integration

The checked manifest now binds the new ELF/ImageID, generated public IDL
SHA-256, 13 exact append-only instruction names, and witnessed-token tags 11
and 12. A local-only deployer command emits that artifact-bound IDL, and typed
initialize/claim assemblers rederive the metadata PDA and custody/claimant ATAs,
preserve exact IDL account order and signer flags, and serialize through the
official Risc0 codec without RPC access. The full verifier, CI pin assertions,
active M3 bootstrap/runner, and operator guide use the same identity. This is
configuration and deterministic assembly evidence, not an actual deployment or
chain effect. The deployer graph's advisory, duplicate, license, and source
policy also passes with exact SPEL source and hash-checked license
clarifications rather than a broad source or license exception.

An additive `asset_terms_version: 2` bridge-protocol envelope also preserves
all v1 native JSON and method strings while binding strict native or
custom-token terms. Seven distinct `lez_bridge.v2.*` methods now model ordered
native two-effect or custom-token three-effect preparation, current-state
observation, witnessed claim reservation/completion/finalized observation, and
permissionless refund preparation/observation. Cross-field constructors reject
definition, ATA, program, authority, amount, state, instruction-order, and
unknown-field drift. Thirty-five protocol tests plus strict Clippy, rustdoc,
formatting, and diff gates pass.

```mermaid
flowchart LR
    Terms["Versioned countersigned asset terms"] --> Wire["Strict LEZ bridge v2 messages"]
    Wire --> Prepare["Ordered prepare effects"]
    Wire --> Current["Stable current-state observation"]
    Wire --> Claim["Witnessed claim lifecycle"]
    Wire --> Refund["Permissionless refund lifecycle"]
    Client["Exact-once bridge client GREEN"] --> Wire
    Sidecar["Official v0.2 planner GREEN<br/>tags 11, 7, 8, 12, and 10<br/>four durable v2 reservations"] --> Wire
    Routes["Sidecar routes and finalized token scans open"] -.-> Sidecar
    Finality["Finalized four-effect classifiers"] --> Wire
```

The current-state escrow observation alone cannot certify finalized funding or
an exact absent-versus-unknown submission outcome. Four additional v2 methods
therefore classify initialization, token-only permissionless custody creation,
funding, and claim. Each uses exact prepared bytes and ID or terms discovery,
stable finalized window coverage, containing-block identity, instruction
accounts, metadata, and custody. The nonoverlapping `Found`, `Absent`,
`Uncertain`, and `Unavailable` outcomes prevent a moving tip, incomplete
history, unavailable finality, conflict, or possible pending transaction from
becoming send authority. Native initialization, funding, and claim parity plus
two custom definitions pass; custody creation rejects native terms.

The bounded loopback client now exposes all seven v2 lifecycle methods and all
four finalized classifiers without internal retries. It consumes each request
ID once, validates the exact Lee v0.2 runtime and response echoes, and enforces
depositor-only preparation, claimant-only claim preparation/completion, and
either-bound-participant observation/classification and permissionless refund
preparation. Stable-tip/current-clock checks, exact/discovery windows, public
placements, ordered effects, exact IDs/bytes, and claimant/outsider negatives
pass without weakening any v1 method. Five unit, five external v2, 32 preserved
bridge-contract, and four example tests are GREEN, as are strict all-target
Clippy, rustdoc, doctests, formatting, and diff gates.

The official v0.2 sidecar planner rederives the pinned Token and ATA programs,
definition-specific depositor, claimant, and custody ATAs, exact guest account
order, tags 11/7/8/12/10, and signer sets. Signed initialization and funding use
consecutive nonces around a nonce-free permissionless custody transaction.
Escrow, claim reservation, claim completion, and refund bytes occupy separate
v2 durable files and replay after restart without nonce rereads or
regeneration. Six focused tests cover both roles, two definitions, conflicts,
program/ATA/authority/order substitution, redaction, and two-stage restart.

Sidecar routes and finalized token scans, adapter/SDK composition, journal
mapping, and role-owned actual-node execution remain open. In particular, the
planner alone does not prove that the official node can provide the required
complete scans.

This ADR does not certify an actual-node custom-token swap in either trade
direction, exact composed balances/effects, restart/no-resubmission, public
deployment, production custody, or a cryptographic/security review. It supports
fungible definitions only. The accepted F7 integration gate remains open until
the new guest is deployed and exercised through the same actor, adapter,
finality, journal, and cleanup boundaries as the native corridor.
