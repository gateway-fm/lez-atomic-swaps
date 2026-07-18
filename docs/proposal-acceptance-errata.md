# Accepted proposal errata and evidence substitutions

Last rechecked: 2026-07-18

This register records defects in Gateway's accepted proposal and any replacement
evidence contract. It is separate from
[Logos-owned production blockers](upstream-production-blockers.md): a proposal
error is not a Logos component dependency and cannot use ADR 0018's relaxed
Logos-code treatment.

| ID | Accepted text and verified defect | Proposed evidence contract | Current status | Exit evidence |
|---|---|---|---|---|
| GW-M3-001 | Gateway issue #112 requires conformance to DLC-specs `AdaptorSignature.md`. The live DLC tree `9cd9148938c616690c79d99ec6f330e213c246c5` and its full history contain no such path; published DLC adaptor vectors are ECDSA, not the required BIP-340 Schnorr construction | Official BIP-340/BIP-327 vectors; exact-pinned swap-specific adaptor positive/negative fixtures; independent implementation cross-check; tweak/parity-aware completed signatures verified by the Bitcoin library and Bitcoin Core consensus | Replacement evidence implemented at pushed `0c78f3d`: immutable official corpora, applicable stateful operations, exact swap adaptor positive/negative fixture, independent `k256` verification, rust-bitcoin checks, and prior Core consensus. Upstream acceptance remains pending; no corrective issue edit/comment or Logos acceptance URL is retained | Accepted issue amendment/comment or other explicit Logos acceptance remains required for literal conformance/production claims. The repository owner explicitly allows the private local-functional milestone tag to proceed with this deviation disclosed and no literal DLC-conformance claim |

## Milestone effect

GW-M3-001 does not prevent building or evaluating the private local M3 PoC.
Every result must state that the proposed substitute is repository evidence, not
literal DLC-vector conformance. It may not weaken the exact cryptographic,
interoperability, actor, or consensus gates.

An `m3-complete` tag cannot claim literal DLC-vector conformance while this
disposition is unaccepted. The repository owner has directed that external
Logos/proposal issues remain visible but do not block private local-functional
milestone certification; the tag and evidence therefore narrow the claim to
the implemented replacement contract and explicitly retain GW-M3-001. Final
production readiness and any full accepted-submission conformance claim still
require the appropriate acceptance owner to close or accept the discrepancy.
