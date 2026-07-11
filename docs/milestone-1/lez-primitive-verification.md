# LEZ primitive verification

Status: source trace and lightweight executable vectors complete; pinned native
sequencer lane in review —
2026-07-11

```mermaid
flowchart TB
    Pin["Pin LEZ dev commit"] --> Trace["Source-trace RPC, mempool, builder, state"]
    Trace --> Unit["Run upstream BIP-340 + validity unit vectors"]
    Unit --> Sequencer["Pinned native sequencer test, 2 build jobs"]
    Sequencer --> Admission["RPC enqueue then block-time state validation"]
    Sequencer --> Bytes["Transaction equality from mempool to block"]
    Unit --> Canonical["BIP-340 invalid/non-canonical vectors"]
    Admission --> Answer["Public protocol semantics"]
    Bytes --> Answer
    Canonical --> Answer
    Answer --> Required["Pinned required CI"]
    Answer --> Current["Scheduled current-dev compatibility lane"]
```

## Pinned source

Repository: `logos-blockchain/logos-execution-zone`

Branch/commit inspected: `dev` /
`cac4921581b37e85ae25e940f3a62412cd22308e`

## Findings

1. `ValidityWindow::is_valid_for` implements `from <= value < to`.
2. Both public and privacy-preserving state tests cover lower/upper boundaries.
3. Sequencer RPC authentication pushes user transactions into the mempool; it
   does not evaluate validity windows at admission.
4. Block construction validates against the new block height and timestamp and
   skips invalid transactions.
5. LEZ `Signature` contains the submitted 64-byte value. Verification parses
   those bytes as a BIP-340 `k256` signature. No normalization assignment was
   found between authenticated transaction decoding and block inclusion.

## Protocol answers

- Validity is lower-inclusive and upper-exclusive.
- Enforcement that determines inclusion occurs at block construction/validation,
  not initial RPC/mempool admission. Deadlines must include uncertain mempool
  residence and next-block timestamp/height.
- BIP-340 does not offer the ECDSA-style arbitrary `s -> -s` alternative assumed
  by the original question. The relevant invariant is exact accepted-byte
  preservation for extracting `t = s - s'`. The pinned sequencer test compares
  the complete submitted transaction, including its `[u8; 64]` signature, with
  the included transaction.

## Executable evidence

- `lee_core` guest-free tests exercise the shared validity type at its inclusive
  lower and exclusive upper bounds. The same type represents block-height and
  timestamp windows.
- `lee` public and privacy-preserving transaction tables exercise block-height
  and timestamp windows before, inside, and at both bounds. These remain in the
  optional guest-toolchain lane.
- the pinned sequencer test enqueues two identical signed transactions and
  asserts that the block contains exactly one transaction equal to the original,
  plus the clock transaction. This proves block-time rejection and accepted-byte
  preservation on the real builder path.
- the embedded official BIP-340 verification vectors include invalid field
  elements and an `s` scalar equal to the curve order; the pinned verifier rejects
  them rather than accepting or rewriting them.

The scheduled/manual workflow has two isolated lanes: the pinned commit runs the
native sequencer test with at most two Cargo build jobs; current `dev` runs the
lightweight semantic and source-drift checks. The verifier clones into a unique
temporary directory and never starts Docker or binds a port.

The executable runner is `scripts/verify-lez-primitives.sh`. Source checks are
early drift diagnostics; the tests, not those string matches, are the behavioral
evidence.

Observed locally at the pinned commit: the guest-free validity boundary suite
and BIP-340 verification vectors pass with `RISC0_SKIP_BUILD=1`. The native
sequencer build was deliberately stopped when unrelated host compilation was
detected; CI is its isolated executable gate. Guest-backed validity tests require
the separate RISC Zero Rust toolchain; run them with `LEZ_VERIFY_GUESTS=1` after
`rzup install rust`. The project does not silently install that prerequisite.
