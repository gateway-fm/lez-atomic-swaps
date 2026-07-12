# Upstream Logos blockers to production

Last rechecked: 2026-07-12

This register contains live-release blockers owned by Logos upstream projects
or services. Under ADR 0018 they are disclosed exceptions, not substitutes for
repository-controlled milestone work. The final production-readiness milestone
must close or explicitly accept every open item.

| ID | Upstream owner and immutable input | Production impact | Current compensating control | Exit evidence |
|---|---|---|---|---|
| LOGOS-001 | logos-co/spel PR #238, pinned head df17acd98436be4f09c55877dae1fe2e73cbcdca | Official SPEL release does not yet carry the LEZ v0.2 compatibility path; the PR is open and awaiting review | Compile and test only the exact head; record it in every evidence packet; never float to the contributor branch | Maintainer review plus merge and an immutable supported SPEL release, or explicit release-owner acceptance of the audited commit |
| LOGOS-002 | Official LEZ v0.2.0, commit a58fbce2ff48c58b7bb5001b1a27e64b9596ee3a, resolves Hickory 0.25.0-alpha.5 through the full Logos/libp2p runtime graph | RustSec 2026-0118 and 2026-0119 affect that Hickory line; the latter is a broader CPU-exhaustion risk | Do not ship the full graph in the swap runtime; build a narrow official-type deployment/query client; keep exact feature/dependency-tree gates and disclosed compile-only exceptions | Logos releases a graph on a patched Hickory line, supplies a supported narrow client, or the release owner explicitly accepts a reviewed residual risk |
| LOGOS-003 | Official LEZ v0.2 public endpoint https://testnet.lez.logos.co | Rate limits, resets, channel changes, and service availability have no repository-controlled SLA and can invalidate or delay evidence | Bind live evidence to returned channel, transaction, block, ProgramId, ELF, ImageID, and exact commits; retain deterministic local evidence and never turn an outage into a passing result | Documented service contract/fallback or explicit operational risk acceptance backed by a fresh live rehearsal |
| LOGOS-004 | LEZ v0.2 RPC/state contract for escrow observation | Upstream documentation does not yet provide one reviewed canonical recipe covering funded escrow identity and stable finality | Escalate the required field list; our adapter remains fail-closed and maker funding remains non-authorized until it validates the complete snapshot | Upstream confirmation or audited primary-source semantics for channel, deployment, accounts, terms, funded state, transaction, block, and finality |
| LOGOS-005 | Open SPEL issues #242 and #243 referenced by PR #238 | Upstream CLI parsing includes a hardcoded private-PDA identifier path and a fail-open 32-byte ProgramId parser | Do not use those parsers as protocol authority; independently derive and compare identifiers in the SDK and narrow adapter | Upstream fixes with regression tests, or proof that the production integration excludes the affected code paths |

## Milestone use

An M2 evidence packet may list these items as open while certifying the exact
repository-controlled implementation. It may not use them to waive canonical
adapter validation, reorg/restart safety, complete role effects, independent
actor tests, or the composed corridor. M7 production readiness revisits the
entire table.
