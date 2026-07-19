# Accepted proposal errata and evidence substitutions

Last rechecked: 2026-07-19

This register records defects in Gateway's accepted proposal and any replacement
evidence contract. It is separate from
[Logos-owned production blockers](upstream-production-blockers.md): a proposal
error is not a Logos component dependency and cannot use ADR 0018's relaxed
Logos-code treatment.

| ID | Accepted text and verified defect | Proposed evidence contract | Current status | Exit evidence |
|---|---|---|---|---|
| GW-M3-001 | Gateway issue #112 requires conformance to DLC-specs `AdaptorSignature.md`. The live DLC tree `9cd9148938c616690c79d99ec6f330e213c246c5` and its full history contain no such path; published DLC adaptor vectors are ECDSA, not the required BIP-340 Schnorr construction | Official BIP-340/BIP-327 vectors; exact-pinned swap-specific adaptor positive/negative fixtures; independent implementation cross-check; tweak/parity-aware completed signatures verified by the Bitcoin library and Bitcoin Core consensus | Replacement evidence implemented at pushed `0c78f3d`: immutable official corpora, applicable stateful operations, exact swap adaptor positive/negative fixture, independent `k256` verification, rust-bitcoin checks, and prior Core consensus. Upstream acceptance remains pending; no corrective issue edit/comment or Logos acceptance URL is retained | Accepted issue amendment/comment or other explicit Logos acceptance remains required for literal conformance/production claims. The repository owner explicitly allows the private local-functional milestone tag to proceed with this deviation disclosed and no literal DLC-conformance claim |
| GW-M4-001 | Gateway issue #112 requires DLEQ test-vector conformance against `comit-network/cross-curve-dleq`. The named repository is archived, self-described as a PoC, publishes no release, declares no Cargo license, has no license file, and redirects to `sigma_fun`; its source and tests cannot be copied into this dual-licensed delivery | Exact-pinned maintained permissive implementation; independently specified h4sh3d-domain positive and negative fixtures; public-key/share equations and proof acceptance cross-checked by a separately provisioned external COMIT oracle only if provenance/legal review permits; mutation/subgroup/endian/domain gates; formal M7 review | Open at M4 entry. `sigma_fun` 0.9.0 is the leading 0BSD PoC candidate, not a production-accepted verifier. No conformance substitute is claimed yet | Accepted issue amendment/comment approving a license-clean vector contract, or explicit permission/license for immutable COMIT fixtures plus independently reproduced conformance evidence |
| GW-M4-002 | The RFP and issue say “Ed25519 adaptor” while also requiring secp256k1↔Ed25519 DLEQ and citing h4sh3d/COMIT. The cited construction adapts the scriptable-chain signature with a scalar tied to a Monero Ed25519 share; current LEZ v0.2 witnessed claims are BIP-340 aggregate signatures. The accepted text does not define an Ed25519 pre-signature wire, equation, or how it is verified by LEZ | Pin the exact LEZ BIP-340 adaptor pre-signature/adapt/extract mapping, bind its secp256k1 witness to the Monero Ed25519 share by DLEQ, retain the exact completed LEZ witness on chain, and prove reconstruction through a real Monero spend. Do not call this literal Ed25519-adaptor conformance until accepted | Open at M4 entry. ADR 0053 uses the technically coherent h4sh3d mapping as the private local-PoC target and keeps the wording discrepancy explicit | Accepted issue/RFP clarification or explicit Logos/Gateway cryptographic sign-off on the exact mapping and vector corpus; M7 independent review remains required for production |
| GW-M4-003 | RFP Functionality F5 says deposited funds are refunded to the depositor after timelock expiry, while F6 requires both legs complete or both refunded and forbids any state where only one party receives funds. The same RFP requires the h4sh3d/COMIT XMR construction. After the Maker funds the unscriptable shared Monero output, an absent Taker cannot be forced to publish the `s_b`-revealing signed LEZ refund. The cited construction therefore uses a later Maker punishment spend of the scriptable leg for economic safety, not literal unconditional F5 depositor refund or F6 two-leg refund. Likewise, a pre-lock hash commitment to a hidden Taker claim partial proves later consistency but not pre-funding validity, so garbage/withholding can force punishment | For the local PoC, execute and disclose the three exclusive LEZ branches from ADR 0055: Maker claim reveals `s_a`, Taker signed refund reveals `s_b`, and later Maker punishment handles Taker abandonment or invalid/withheld claim publication. Do not claim literal F5/F6 conformance. For production, require reviewed verifiable-encryption/ZK validity evidence or explicit acceptance of the penalty model, and either accept punishment as the intended interpretation or approve a replacement that guarantees the accepted refund wording | Open. The focused guest source executes all three disjoint branches and keeps the incompatibility explicit. It is not a reason to weaken the claim or signed-refund paths | RFP/accepted-issue amendment or explicit Logos/Gateway protocol disposition, followed by independent cryptographic review and tests for the selected construction |

## Milestone effect

GW-M3-001 does not prevent building or evaluating the private local M3 PoC.
Every result must state that the proposed substitute is repository evidence, not
literal DLC-vector conformance. It may not weaken the exact cryptographic,
interoperability, actor, or consensus gates.

GW-M4-001, GW-M4-002, and GW-M4-003 likewise do not excuse a synthetic Monero
transfer or a missing cryptographic link. The private local M4 PoC may proceed
only when its exact adaptor extraction, DLEQ relationship, reconstructed real
Monero spend, signed refund, and disclosed punishment disposition are
executable. Literal accepted-text conformance and production readiness remain
open until the appropriate owners accept the evidence contract and independent
review is complete.

An `m3-complete` tag cannot claim literal DLC-vector conformance while this
disposition is unaccepted. The repository owner has directed that external
Logos/proposal issues remain visible but do not block private local-functional
milestone certification; the tag and evidence therefore narrow the claim to
the implemented replacement contract and explicitly retain GW-M3-001. Final
production readiness and any full accepted-submission conformance claim still
require the appropriate acceptance owner to close or accept the discrepancy.
