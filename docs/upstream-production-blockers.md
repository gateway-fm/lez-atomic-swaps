# Upstream Logos blockers to production

Last rechecked: 2026-07-13

This register contains live-release blockers owned by Logos upstream projects
or services. Under ADR 0018 they are disclosed exceptions, not substitutes for
repository-controlled milestone work. The final production-readiness milestone
must close or explicitly accept every open item.

| ID | Upstream owner and immutable input | Production impact | Current compensating control | Exit evidence |
|---|---|---|---|---|
| LOGOS-001 | logos-co/spel PR #238, pinned head df17acd98436be4f09c55877dae1fe2e73cbcdca | Official SPEL release does not yet carry the LEZ v0.2 compatibility path; the PR is open and awaiting review | Compile and test only the exact head; record it in every evidence packet; never float to the contributor branch | Maintainer review plus merge and an immutable supported SPEL release, or explicit release-owner acceptance of the audited commit |
| LOGOS-002 | Official LEZ v0.2.0, commit a58fbce2ff48c58b7bb5001b1a27e64b9596ee3a, resolves Hickory 0.25.0-alpha.5 through the full Logos/libp2p runtime graph | RustSec 2026-0118 and 2026-0119 affect that Hickory line; the latter is a broader CPU-exhaustion risk | Do not ship the full graph in the swap runtime; build a narrow official-type deployment/query client; keep exact feature/dependency-tree gates and disclosed compile-only exceptions | Logos releases a graph on a patched Hickory line, supplies a supported narrow client, or the release owner explicitly accepts a reviewed residual risk |
| LOGOS-003 | Official LEZ v0.2 public endpoint https://testnet.lez.logos.co | Rate limits, resets, channel changes, and service availability have no repository-controlled SLA and can invalidate or delay evidence | Bind live evidence to returned channel, transaction, block, ProgramId, ELF, ImageID, and exact commits; retain deterministic local evidence and never turn an outage into a passing result | Documented service contract/fallback or explicit operational risk acceptance backed by a fresh live rehearsal |
| LOGOS-004 | LEZ v0.2 RPC/state contract for escrow observation | The RPC exposes transaction, block, account, channel, and Pending/Safe/Finalized state, but no sequencer verification key or independently verifiable Bedrock finality proof; upstream documentation provides no reviewed canonical recipe | The SDK now validates internal cross-RPC consistency against an authoritative node, binds the signed channel/genesis and complete escrow snapshot, requires Finalized for public policy, and keeps maker funding non-authorized; the thin official-wire adapter remains fail-closed | Upstream confirmation plus a reviewed canonical observation recipe and verifiable finality/key material, or explicit release-owner acceptance of the authoritative-node trust model |
| LOGOS-005 | Open SPEL issues #242 and #243 referenced by PR #238 | Upstream CLI parsing includes a hardcoded private-PDA identifier path and a fail-open 32-byte ProgramId parser | Do not use those parsers as protocol authority; independently derive and compare identifiers in the SDK and narrow adapter | Upstream fixes with regression tests, or proof that the production integration excludes the affected code paths |
| LOGOS-006 | Official LEZ v0.1.2 standalone sequencer, tag v0.1.2 / cf3639d8252040d13b3d4e933feb19b42c76e14a | The server binds `0.0.0.0`, exposes no bind-address setting, and mock settlement leaves blocks `Pending` | Run it only inside a unique isolated Compose network with no host node port; map each authenticated role sidecar to an ephemeral loopback host port; accept depth-qualified `Pending` only in the explicitly named deterministic v0.1.2 compatibility environment | A supported runtime with configurable loopback/Unix binding and deterministic Safe/Finalized progression, or retirement of this compatibility lane after v0.2 local execution is available |
| LOGOS-007 | Official LEZ v0.1.2 sequencer RPC | It exposes no channel query, transaction-to-inclusion lookup, or mempool lookup, so a missing transaction cannot be called stably absent and canonical discovery requires scanning | Bind a nonzero run-owned channel in the signed agreement and sidecar config; use bounded recent-block scanning followed by fresh canonical block/transaction/account reads; report `UnknownOrPending` when absence is ambiguous | Supported channel, inclusion, and mempool RPCs with reviewed semantics, or a reviewed upstream indexing recipe |
| LOGOS-008 | Official LEZ v0.1.2 `PrivateKey` | Upstream `Debug`/`Display` reveal raw key material and the type does not implement `Zeroize` | Never format the type; keep it behind a role-owned sidecar wrapper and capability boundary; prohibit secret-bearing diagnostics and retain leak tests | Upstream redacted formatting plus zeroization, or migration to a supported signer/HSM interface that never exposes raw key material |

## Milestone use

An M2 evidence packet may list these items as open while certifying the exact
repository-controlled implementation. It may not use them to waive canonical
adapter validation, reorg/restart safety, complete role effects, independent
actor tests, or the composed corridor. M7 production readiness revisits the
entire table.
