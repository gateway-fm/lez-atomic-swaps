# ADR 0019: Canonical LEZ funded-escrow observation

Status: accepted for the M2 SDK boundary; production RPC adapter and exact-head
reconciliation remain open.

~~~mermaid
flowchart LR
    Agreement["Dual-signed agreement<br/>channel, genesis, program, roles, asset, amount"]
    RPC["LEZ v0.2 RPC adapter<br/>tip before and after"]
    Tx["Public fund transaction<br/>signer, program, ordered accounts"]
    Block["Canonical inclusion block<br/>height, hash, finality"]
    Accounts["Metadata and custody accounts<br/>owner, decoded state, exact balance"]
    Validator["Canonical LEZ validator"]
    Journal["Maker schema-v6 ordered journal<br/>primitive snapshot"]
    Replay["Restart revalidation"]

    Agreement --> Validator
    RPC --> Tx
    RPC --> Block
    RPC --> Accounts
    Tx --> Validator
    Block --> Validator
    Accounts --> Validator
    Validator --> Journal
    Journal --> Replay
    Agreement --> Replay
~~~

## Context

A transaction ID and confirmation count cannot prove a LEZ escrow is the
agreement-selected taker lock. The maker must distinguish a real funded SPEL
escrow from the wrong execution channel, program, actor, instruction accounts,
metadata, asset, custody, amount, or fork. A trusted in-memory verdict is also
insufficient because every restart must reconstruct trust from primitive data.

LEZ v0.2 exposes channel, block, transaction, account, and block-status RPCs.
Its RPC does not expose a sequencer verification key or a separately
verifiable Bedrock finality proof. The resulting evidence is therefore
consistency evidence against the selected authoritative node, not a trustless
light-client proof.

## Decision

The dual-signed agreement binds the execution environment, nonzero v0.2 channel
ID, nonzero genesis block hash, escrow ProgramId, role accounts, derived
metadata/custody accounts, asset programs and definition, amount, terms hash,
secret digest, and refund deadline.

The observation adapter brackets all reads with the same tip and returns
primitive transaction, block, decoded metadata, and custody facts. The SDK
accepts only the reverse direction and checks:

- exact channel/genesis and a stable bracketing tip;
- a public, validly signed fund transaction under the escrow program, signed by
  the taker/depositor, using the exact generated FundNative or FundToken kind,
  on-chain swap ID, and generated-client account order;
- canonical inclusion at or below the stable tip and recomputed nonzero depth;
- metadata ownership plus exact version, roles, terms hash, digest, custody,
  programs, definition, amount, deadline, and Funded status;
- exact queried custody account address plus native or token owner, definition,
  and balance exactly equal to the signed account and amount; and
- exact upstream Pending, Safe, or Finalized status. Structural validation
  accepts every nonzero stable depth; the later funding-eligibility boundary
  applies the signed threshold and requires Finalized on public v0.2.

The ordered maker journal stores the complete untrusted snapshot. Replay calls
the same agreement validator and then checks byte-for-byte record
reconstruction. It never deserializes a trusted verdict.

Adding the channel to the signed body introduced agreement schema 2. The
schema-aware decoder recognizes legacy schema-1 layout but rejects it with a
typed error. Because the missing channel was never signed, old agreements are
not migrated; both actors must renegotiate and re-sign.

## Consequences

Primitive reverse-LEZ ID/depth assertions fail closed. SDK and SQLite
close/reopen preserve and revalidate canonical funded evidence. Channel or
genesis changes are identity failures, never reorg replacements.

The dependency-free two-phase `LezObservationTrackerV1` now suppresses exact
duplicates, journals same-inclusion depth and monotonic
Pending-to-Safe-to-Finalized updates, requires affirmative same-tip atomic
replacement evidence for changed inclusion, rejects stale evidence, stable-tip
regression, and finality regression, and treats any finalized removal as an
operator-fatal violation. Proposal never mutates the head; only an exact
committed event does.

The next slice must use official LEZ wire types to decode and hash the public
transaction, integrate the tracker into the ordered SDK/SQLite journal,
distinguish a still-canonical fund whose
escrow has become Claimed or Refunded, and add native plus token standalone
actor evidence. A finalized block changing is an operator-fatal finality
violation, not a routine reorg.

The missing independently verifiable sequencer/finality proof is recorded as
LOGOS-004 under ADR 0018 and does not waive repository-controlled M2 work.
