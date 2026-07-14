# Accepted proposal errata and evidence substitutions

Last rechecked: 2026-07-14

This register records defects in Gateway's accepted proposal and any replacement
evidence contract. It is separate from
[Logos-owned production blockers](upstream-production-blockers.md): a proposal
error is not a Logos component dependency and cannot use ADR 0018's relaxed
Logos-code treatment.

| ID | Accepted text and verified defect | Proposed evidence contract | Current status | Exit evidence |
|---|---|---|---|---|
| GW-M3-001 | Gateway issue #112 requires conformance to DLC-specs `AdaptorSignature.md`. The live DLC tree `9cd9148938c616690c79d99ec6f330e213c246c5` and its full history contain no such path; published DLC adaptor vectors are ECDSA, not the required BIP-340 Schnorr construction | Official BIP-340/BIP-327 vectors; exact-pinned swap-specific adaptor positive/negative fixtures; independent implementation cross-check; tweak/parity-aware completed signatures verified by the Bitcoin library and Bitcoin Core consensus | Verified defect; acceptance pending. No corrective issue edit/comment or Logos acceptance URL has been posted or retained | Accepted issue amendment/comment, or another explicit Logos acceptance of this replacement contract, bound into the M3 review and evidence packet |

## Milestone effect

GW-M3-001 does not prevent building or evaluating the private local M3 PoC.
Every result must state that the proposed substitute is repository evidence, not
literal DLC-vector conformance. It may not weaken the exact cryptographic,
interoperability, actor, or consensus gates.

An `m3-complete` tag cannot claim full conformance to the accepted submission
while this disposition is unaccepted. Before tagging, retain the accepted
clarification URL and immutable body/comment hash, or record a repository-owner
decision that explicitly narrows the certification claim without presenting the
deviation as accepted RFP compliance. Final production readiness still requires
the appropriate acceptance owner to close or accept the discrepancy.
