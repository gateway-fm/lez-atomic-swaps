# Accepted proposal errata and evidence substitutions

Last disposition update: 2026-11-21

This register records defects in Gateway's accepted proposal and any replacement
evidence contract. It is separate from
[Logos-owned production blockers](upstream-production-blockers.md): a proposal
error is not a Logos component dependency and cannot use ADR 0018's relaxed
Logos-code treatment.

| ID | Accepted text and verified defect | Proposed evidence contract | Current status | Exit evidence |
|---|---|---|---|---|
| GW-M3-001 | Gateway issue #112 requires conformance to DLC-specs `AdaptorSignature.md`. The live DLC tree `9cd9148938c616690c79d99ec6f330e213c246c5` and the path history checked at that revision contain no such path; published DLC adaptor vectors are ECDSA, not the required BIP-340 Schnorr construction | Official BIP-340/BIP-327 vectors; exact-pinned swap-specific adaptor positive/negative fixtures; independent implementation cross-check; tweak/parity-aware completed signatures verified by the Bitcoin library and Bitcoin Core consensus | Replacement evidence implemented at pushed `0c78f3d`: immutable official corpora, applicable stateful operations, exact swap adaptor positive/negative fixture, independent `k256` verification, rust-bitcoin checks, and prior Core consensus. Logos [accepted the test-reference correction](https://github.com/logos-co/rfp/issues/123#issuecomment-5781178140) on 22 September 2026 after reviewing the v0.2.4 files and CI results | [Recorded decision](https://github.com/logos-co/rfp/issues/123#issuecomment-5781178140) closes the test-reference question only. Other M3 deliverables and the M7 security assessment remain open; no literal DLC-vector conformance or production-readiness claim follows |
| GW-M4-001 | Gateway issue #112 and rfp#124 deliverable D-3 require "DLEQ test-vector conformance against `comit-network/cross-curve-dleq`". At its last commit `6e90b10eca1c2e96678319f93c401859fa31c03d` (2021-05-01, archived; its README redirects to `sigma_fun`'s `dl_secp256k1_ed25519_eq`) the crate publishes no test vectors: its five tests are property tests, and its only fixed bytes are the two Pedersen generators. Its proof is a different construction of the same statement from the maintained `sigma_fun` proof this SDK uses: each bit committed as bᵢ·G + rᵢ·G′ (here rᵢ·G + bᵢ·2ⁱ·H), a weighted sum of the commitments (here a plain one), ECDSA and Ed25519 signatures over SHA-256(X) to tie the sum to the keys (here two DLEQ sigma proofs in one Fiat–Shamir transcript), an untagged per-bit SHA-256 challenge that does not hash X (here a tagged challenge over the whole statement), and a 97,308-byte bincode proof (here 56,611 bytes, inside this SDK's own v1 envelope). No proof of one verifies under the other, and none can be converted without the prover's blinders. The crate is also unlicensed, so `deny.toml` keeps it out of every build | Run COMIT's crate as it stands, outside the repository, and check this SDK against it at every point the two share: (1) COMIT's own tests on its pinned toolchain; (2) the two alternate generators, byte for byte; (3) for the same secret scalar, the secp256k1 and Ed25519 public keys of both libraries, byte for byte; (4) proofs of each fed to the other's verifier, expected to fail; (5) a negative control. In the repository: the self-generated regression vectors and negative cases of `crates/xmr-swap-sdk/tests/cross_curve_vectors.rs`, `alternate_generators_are_the_published_nums_points` (both generators from their published definitions) and `proofs_interoperate_with_the_eigenwallet_parameters` (eigenwallet/core, which continues COMIT's swap, uses this proof and these generators, and accepts the vectors' proofs) | Run on 2026-11-21 against the SDK at `b99a55ad`; COMIT in a Linux container on its pinned `nightly-2020-08-13`, with dependencies held at versions that build there and no COMIT source changed: (1) 5 of 5 passed; (2) both generators the same bytes (`0250929b…803ac0`, `8b655970…9c1f94`); (3) the same keys for 5 of 5 scalars (the vectors `one`, `max` and `mixed`, and two COMIT-random scalars); (4) every proof refused in both directions, at decoding; (5) COMIT's proof for one scalar refused against another scalar's keys. The repository's vector suite: 11 passed, 1 ignored (the regenerator). The comparison harness is not in the repository, since building it brings the unlicensed crate into the tree. Not raised with Logos | Logos's decision on rfp#124 that the replacement answers D-3. No literal COMIT-vector conformance and no proof-level interoperability with COMIT follow; the construction's third-party review stays with M7 |

## Milestone effect

Logos accepted GW-M3-001 as a correction to the named test reference only on
22 September 2026. The replacement does not establish literal DLC-vector
conformance and may not weaken the exact cryptographic, interoperability,
actor, or consensus gates.

The decision does not approve M3, production readiness, or the full security
construction. The other M3 items remain under review, and the full security
assessment remains with M7. Historical evidence retains its original status
and build identity.

GW-M4-001 has no Logos decision. Its replacement evidence answers D-3 only at
the points COMIT's crate and this SDK share, the generators and the keys of a
scalar; it is neither conformance to COMIT's vectors, which do not exist, nor
to its proof, which differs.
