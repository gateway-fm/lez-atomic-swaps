# Accepted proposal errata and evidence substitutions

Last disposition update: 2026-09-22

This register records defects in Gateway's accepted proposal and any replacement
evidence contract. It is separate from
[Logos-owned production blockers](upstream-production-blockers.md): a proposal
error is not a Logos component dependency and cannot use ADR 0018's relaxed
Logos-code treatment.

| ID | Accepted text and verified defect | Proposed evidence contract | Current status | Exit evidence |
|---|---|---|---|---|
| GW-M3-001 | Gateway issue #112 requires conformance to DLC-specs `AdaptorSignature.md`. The live DLC tree `9cd9148938c616690c79d99ec6f330e213c246c5` and the path history checked at that revision contain no such path; published DLC adaptor vectors are ECDSA, not the required BIP-340 Schnorr construction | Official BIP-340/BIP-327 vectors; exact-pinned swap-specific adaptor positive/negative fixtures; independent implementation cross-check; tweak/parity-aware completed signatures verified by the Bitcoin library and Bitcoin Core consensus | Replacement evidence implemented at pushed `0c78f3d`: immutable official corpora, applicable stateful operations, exact swap adaptor positive/negative fixture, independent `k256` verification, rust-bitcoin checks, and prior Core consensus. Logos [accepted the test-reference correction](https://github.com/logos-co/rfp/issues/123#issuecomment-5781178140) on 22 September 2026 after reviewing the v0.2.4 files and CI results | [Recorded decision](https://github.com/logos-co/rfp/issues/123#issuecomment-5781178140) closes the test-reference question only. Other M3 deliverables and the M7 security assessment remain open; no literal DLC-vector conformance or production-readiness claim follows |

## Milestone effect

Logos accepted GW-M3-001 as a correction to the named test reference only on
22 September 2026. The replacement does not establish literal DLC-vector
conformance and may not weaken the exact cryptographic, interoperability,
actor, or consensus gates.

The decision does not approve M3, production readiness, or the full security
construction. The other M3 items remain under review, and the full security
assessment remains with M7. Historical evidence retains its original status
and build identity.
