# LEZ primitive verification

Status: source trace complete; pinned executable reproducers in progress —
2026-07-11

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

## Protocol answers, pending reproducer gate

- Validity is lower-inclusive and upper-exclusive.
- Enforcement that determines inclusion occurs at block construction/validation,
  not initial RPC/mempool admission. Deadlines must include uncertain mempool
  residence and next-block timestamp/height.
- BIP-340 does not offer the ECDSA-style arbitrary `s -> -s` alternative assumed
  by the original question. The relevant invariant is exact accepted-byte
  preservation for extracting `t = s - s'`. Source supports this, but a
  submission-to-inclusion byte equality test remains required.

## Required executable reproducers

- exact lower and upper block/timestamp boundaries for public and private paths;
- transaction admitted before its window but included only when valid;
- transaction still in/at mempool at exclusive upper bound is rejected;
- valid completed signature bytes are identical before submission and in the
  included block;
- a scalar-malleated/non-canonical signature is rejected and never rewritten.

These tests run against the pinned commit in required CI and current `dev` in a
scheduled compatibility lane.

The initial source/path guard and upstream unit-test runner is
`scripts/verify-lez-primitives.sh`. Sequencer-level custom cases above are still
open; the script does not yet satisfy the full reproducer gate.

Observed locally: source guards passed and the BIP-340 verification vectors
passed with `RISC0_SKIP_BUILD=1`. Guest-backed validity tests require the
separate RISC Zero Rust toolchain and fail without it; run them with
`LEZ_VERIFY_GUESTS=1` after `rzup install rust`. This prerequisite is not
silently installed by the project.
