# ADR 0058: Activate XMR swaps in two countersigned stages

Status: Accepted and source-executed for M4. The canonical Stage-A agreement,
Stage-B activation, structural LEZ-lock/cutoff validation, LEZ guest publication
gate, strict additive v3 bridge protocol/client, and exact Monero output
observation pass focused source tests. Trusted finalized LEZ evidence,
Stage-B-bound one-shot release, bridge sidecar/runtime, and independent actors
remain in progress. The fresh local checked guest artifact is complete and is
not a public deployment or actor-runtime claim.

## Context

The claim and refund adaptor session IDs must be derived before either party
creates nonce material or partial signatures. A single agreement commitment
cannot also contain the Taker claim-partial commitment: that partial depends on
the session ID, so including it in the session's own commitment is circular.

The first LEZ lock creates a second constraint. RFP Reliability 2 and Gateway's
accepted submission require every later step to use only local durable state
and chain nodes. The Taker cannot deliver its claim partial to the Maker through
Chat or another off-chain channel after the first lock. Conversely, giving the
Maker that usable partial before funding XMR lets the Maker adapt the claim with
its already known share `s_a` and take LEZ early.

## Decision

M4 separates negotiation from activation.

Stage A is a canonical base agreement countersigned with distinct Maker and
Taker agreement keys. It binds:

- the fixed `TakerSellsLez` direction, swap ID, and exact named parameter
  profile;
- three non-aliased secp256k1 keys per role for agreement, claim, and refund;
- both canonical DLEQ envelopes and their transcript commitments;
- the Monero network/genesis, shared address and public keys, exact amount, and
  confirmation policy;
- the LEZ channel/genesis, escrow and authenticated-transfer programs, exact
  role/accounts/PDAs, amount, and finality policy; and
- the funding cutoff, refund and punishment boundaries, minimum reaction
  margin, and purpose-separated LEZ messages.

Only the Stage A commitment derives the distinct claim and refund session IDs.
BIP-340 x-only parity aliases and any DLEQ adaptor point reused as an agreement,
claim, or refund signing key are rejected.

Stage B is a separate canonical activation record countersigned by both Stage A
agreement keys. It binds the Stage A commitment, both adaptor-context bindings,
both roles' claim and refund nonce commitments and public nonces, the validated
Maker claim partial, a transcript binding plus commitment for the still-hidden
Taker claim partial, and both validated refund partials/presignature. Each role
must locally prove that its retained private Monero view key opens the Stage A
public view key before countersigning activation.

The claim-partial context binding is derived without the hidden partial:

```text
SHA256("logos.gateway.lez-xmr.claim-partial-context.v1\0"
  || base agreement commitment
  || claim context binding
  || both claim nonce commitments
  || both claim public nonces
  || Maker claim partial)
```

The guest stores that binding and
`H("logos.gateway.lez-xmr.claim-partial-commitment.v1\0" || binding || Taker
claim partial)`. The Stage B activation commitment becomes guest `terms_hash`.
A Stage A agreement alone cannot produce a first-lock plan.

```mermaid
flowchart LR
    StageA["Stage A base agreement"] --> Sessions["Claim and refund session IDs"]
    Sessions --> Nonces["Durable purpose-separated nonce transcripts"]
    Nonces --> StageB["Stage B activation record"]
    StageA --> StageB
    ViewKey["Local private view-key match"] --> StageB
    StageB --> TermsHash["LEZ terms_hash"]
    StageB --> CheckedGuest["Checked local M4 guest<br/>ELF dc370bc...b7292"]
    CheckedGuest --> Init["InitializeNativeXmr"]
    Init --> Fund["FundNative"]
    Fund --> LezEvidence["Finalized exact LEZ lock evidence"]
    LezEvidence --> XmrFund["Maker XMR funding authorization"]
    XmrFund --> XmrObservation["Non-cloneable exact XMR observation<br/>component green"]
    XmrObservation -.-> Release["Stage-B-bound one-shot release<br/>pending"]
    Release -.-> Publish["AuthorizeNativeXmrClaim"]
    Publish --> Claim["ClaimNativeXmr"]
    Fund --> Refund["RefundNativeXmr"]
    Fund --> Punish["PunishNativeXmr"]
```

After the first lock, no peer channel is required:

```mermaid
sequenceDiagram
    actor Taker
    actor Maker
    participant Lez as LEZ node and indexer
    participant Xmr as monerod and wallet RPC

    Taker->>Maker: Countersign Stage A base agreement
    Maker->>Taker: Exchange both purpose-separated nonce transcripts
    Taker->>Maker: Countersign Stage B activation with hidden-partial commitment
    Note over Taker,Maker: Both roles persist activation and recovery material before first lock
    Taker->>Lez: InitializeNativeXmr and FundNative from Stage B
    Lez-->>Maker: Finalized exact version-3 metadata and custody
    Maker->>Xmr: Fund the exact shared address
    Xmr-->>Taker: Trusted canonical unlocked output reaches profile depth
    Taker->>Lez: AuthorizeNativeXmrClaim publishes the matching partial
    Lez-->>Maker: Finalized canonical publication bytes
    Maker->>Lez: ClaimNativeXmr with s_a-adapted aggregate signature
    Lez-->>Taker: Finalized claim signature reveals s_a
    Taker->>Xmr: Reconstruct s_a plus retained s_b and spend

    alt No claim and Taker acts in the refund window
        Taker->>Lez: RefundNativeXmr with s_b-adapted aggregate signature
        Lez-->>Maker: Finalized refund signature reveals s_b
        Maker->>Xmr: Reconstruct retained s_a plus s_b and recover
    else Taker abandons through punish_at
        Maker->>Lez: PunishNativeXmr
        Note over Taker,Maker: Disclosed penalty fallback and not literal RFP F5/F6 refund conformance
    end
```

The target chain-evidence boundary is capability-bearing, not a caller-set
status enum. The next bridge runtime/adapter slice must mint exact finalized
version-3 LEZ metadata/custody evidence before Maker XMR funding, consume the
now-green non-cloneable Monero observation into a durable Stage-B-bound
one-shot capability before Taker publication, and mint finalized matching
publication evidence before Maker claim. The observation cannot prove
old-output unspent state from a view-only wallet, and configured credentials
alone do not prove server-side authentication; the local run must bind its
peerless topology and cross-credential rejection. The current SDK's
structurally validated candidate and public raw adaptor bindings are not
lifecycle authority.

## Consequences

- Activation adds one countersigned pre-lock round but removes the commitment
  cycle and binds every first-lock input.
- Claim-partial publication is canonical LEZ data, so recovery survives loss of
  Chat and fresh-process restart.
- A hidden-partial hash proves later consistency, not pre-funding validity. A
  malicious Taker can commit invalid bytes or withhold publication and force
  punishment. GW-M4-003 retains the required production disposition and
  verifiable-encryption/ZK review.
- Named accelerated Regtest may shorten wall time without weakening the exact
  two-chain order or ten-confirmation XMR policy. Stagenet uses the reviewed
  public-testnet profile.
- M4's fresh local checked guest is ELF
  `dc370bc34b432317730c51b49342760dbc675fca700e300b30b5fadefe5b7292`
  with ImageID
  `4d6590332948743c2db88a183755815354ef92560550cd206ac27bddeea12c82`.
  Certified M2/M3 artifacts and hashes remain immutable; M4 deployment evidence
  still requires a fresh on-chain deployment of this distinct identity.
- Native LEZ is the progressive happy-path corridor. RFP F7 custom-token XMR
  parity remains mandatory before literal M4 closure.

## Executable evidence

The Stage-A/Stage-B SDK checkpoint has six passing package tests. They cover
canonical wire round trips, exact activation-to-guest initialization fields,
structural version-3 LEZ-lock and inclusive funding-cutoff validation, private
view-key mismatch, agreement/activation field mutations, trailing wire
rejection, and hidden-partial commitment consistency. Strict Clippy and
Rustdoc pass. The lock candidate and its validated projection are explicitly
unauthenticated caller data, not Monero-funding authority; the checked bridge
runtime/adapter must supply canonical evidence in the next slice. The recursive
guest claim/refund/punishment cases now execute against a fresh digest-pinned
M4 ELF: two builds reproduced the exact ELF/ImageID and passed all five serial
tests, comprising one native aggregate-witness compatibility case and four XMR
branch/rollback cases. Runtime external resources are empty. Cold setup may
fetch pinned circuits, Cargo/Git sources, the Docker builder, and Risc0 tools;
those availability inputs are recorded separately from checked execution.
Default scoped cleanup removed about 3.49 GiB of run-owned build/tool state
while retaining the small evidence ELF. This is checked recursive evidence,
not a public deployment, trusted chain observation, bridge runtime, role actor,
or swap. The certified M2/M3 ELF predates tags 13 through 17 and is deliberately
not reused. The additive bridge client maps all eight v3 methods once and
passes 51 package targets, including four new role/echo/coverage/timeout
contracts. The Monero adapter passes seven focused tests over exact
network/genesis/transaction/address/amount, decoded canonical block membership,
ten confirmations, and stable-tip bracketing. Its result is deliberately
non-cloneable observation data, not hidden-partial release authority; upstream
response bounds, header trust flags, malformed-block panic behavior, and
view-only spent status remain explicit residuals.
