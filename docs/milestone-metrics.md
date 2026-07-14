# Milestone delivery metrics

Last updated: 2026-07-14

This is the live evidence scorecard for ADR 0027's progressive milestone
delivery. It tracks measurements and explicit unknowns; it does not infer a
percentage from task counts. Update it whenever a phase changes, a reproducible
run is recorded, or an open item is closed or invalidated.

Status vocabulary:

- `in progress`: the owner-selected active phase;
- `awaiting owner transition`: work is intentionally not the active phase;
- `carried evidence`: relevant evidence created before the phase was selected,
  retained for later revalidation; and
- `gate met`: all documented exit evidence exists, subject to owner review.

## Milestone phase register

| Milestone | Active phase | Phase status | Completion tag | Owner transition |
|---|---|---|---|---|
| M1 | Historical completed milestone | Historical evidence predates ADR 0027; not retroactively reclassified | `m1-complete` and corrective tag `m1-complete.1` | No transition requested |
| M2 | Reproducible PoC | In progress; composed local LEZ/ZEC happy path is not yet complete | None | Remain in PoC until the owner says to end or switch |
| M3 | Not active | Awaiting owner transition | None | Not requested |
| M4 | Not active | Awaiting owner transition | None | Not requested |
| M5 | Not active | Awaiting owner transition | None | Not requested |
| M6 | Not active | Awaiting owner transition | None | Not requested |
| M7 | Not active | Awaiting owner transition | None | Not requested |

## M2 current scorecard

### PoC

| Metric | Current measurement | Evidence or next measurement point |
|---|---|---|
| Full corridor reproductions | 0 successful; failed or partial historical attempts not baselined | No single command yet composes both real local networks and both actors |
| Clean-host reproductions | 0 | Measure after the first composed runner is GREEN |
| Setup duration | Not measured for the composed corridor | Record wall time and cache state for every PoC run |
| Happy-path execution duration | Not measured | Record actor start through finalized terminal state |
| Required local chain environments | 2: pinned LEZ v0.2 and pinned Zebra Regtest | Each has independent real-node evidence; composed use is 0 runs |
| LEZ processes in the target environment | 3: Bedrock, non-standalone sequencer, indexer | Service readiness and finalized cross-RPC block identity are GREEN separately |
| Effect-bearing swap actors | 0 of 2 in a composed run | Target is distinct maker and taker processes with separate keys, configs, funds, stores, journals, and sidecars |
| Supported happy directions | 0 of 2 composed | First visible pass is one direction; PoC gate is both directions unless owner revises it |
| Actual maker/taker Vault Claims | 0 of 2 finalized in the composed flow | Preparation and at-most-once library behavior are carried evidence; actual-node inclusion/finality remains |
| Checked LEZ escrow lifecycle | 0 composed deployments/funding/claims | Provisional/v0.1.2 execution is lower-lane evidence, not the v0.2 corridor |
| Actual Zcash HTLC lifecycle | 0 composed funding/claim sequences | Zebra Regtest construction and consensus suites are carried evidence |
| Final state and balance proof | 0 composed terminal proofs | Capture transaction identities, containing/finalized blocks, exact account/UTXO state, and role terminal states |
| Public RPCs, faucets, or public funds used | 0 planned for PoC; full run absent | Local deterministic genesis/Regtest funding is required; cold artifact provisioning remains an external availability dependency |
| Cleanup leaks | Not measured for composed runner | Existing LEZ/Zebra runners assert scoped cleanup separately; composed run must report exact containers, network, image, state, and leftovers |
| Manual reproduction path | Not available for the full corridor | Add one command plus role/config/evidence inspection steps to the manual guide and README |

Open PoC implementation work is not an external blocker: compose the exact local
networks, actual Vault Claims and finality, checked escrow deploy/init/fund,
independent actor commands, one happy direction, the second direction, terminal
evidence, manual repetition, and exact cleanup. Logos-owned production issues
remain nonblocking for this local phase and stay in the upstream register.

### QA

Status: awaiting owner transition. Extensive unit, property, persistence,
adapter, role-boundary, and real-node regression evidence already exists and is
carried forward. It is not a claim that the M2 QA phase is complete.

| Metric | Current classification | QA-phase measurement |
|---|---|---|
| Requirement/invariant coverage | Carried evidence exists; composed matrix not measured | Map every M2 happy and negative behavior to executable actor-level evidence |
| RED-GREEN-REFACTOR cases | Historical cases exist; no count assigned to the new phase | Count new failing cases, fixes, and refactors from QA entry |
| Restart, boundary, reorg, refund, concurrency cases | Proven in multiple lower lanes; 0 composed phase cases | Revalidate around the completed PoC using real roles and required nodes |
| Pass/fail/ignored totals | Not baselined for the phase | Record exact commands, totals, and justified ignores on phase entry and exit |
| Flake rate | Not measured | Repeated isolated runs must report attempts, intermittent failures, and causes |
| Open QA defects | Not baselined | Maintain severity, owner, reproduction, and disposition |

### Chaos

Status: awaiting owner transition. Zebra fork/reorg, restart, ambiguous effect,
and store recovery tests are carried evidence, not a composed chaos campaign.

| Metric | Current measurement | Chaos-phase target |
|---|---|---|
| Composed fault cases injected | 0 | Catalogue process, RPC, node, network, reorg, storage, and timing faults |
| Successful recoveries | 0 composed | Record result and observed recovery time per fault |
| Duplicate external effects | Not measured in a composed run | 0 unexplained duplicates |
| Lost funds or state corruption | Not measured in a composed run | 0 |
| Run-owned resource leaks | Not measured in a composed run | 0 after exact cleanup |

### Information security

Status: awaiting owner transition. Security is still a continuous baseline:
formatting, strict linters, tests/docs, RustSec, dependency bans/licenses/sources,
ShellCheck, traceability, Mermaid policy, and pinned image scanning remain CI
requirements. Prior green results are carried evidence and must be freshly
recorded when this phase is active.

| Metric | Current measurement | Information-security phase target |
|---|---|---|
| Repository-controlled critical/high vulnerabilities | Not freshly baselined; no known open critical/high item is recorded | 0 unresolved |
| Logos-owned advisory exceptions | Present and enumerated in the upstream production-blocker register | Exact, narrow, reviewed, and non-expanding for local evidence |
| Threat-model findings | Not rebaselined for the composed corridor | Count by severity with disposition and regression evidence |
| Secret exposure findings | No composed-run measurement | 0; logs, evidence, configs, stores, and process arguments included |
| License/source-policy violations | Prior gates carried; fresh phase result pending | 0 undisclosed violations or license bombs |
| Lint/static-analysis/image gates | CI gates exist; fresh phase result pending | All required jobs GREEN on the exact evidence commit |

### Production readiness

Status: awaiting owner transition.

| Metric | Current measurement | Production-readiness target |
|---|---|---|
| Public configuration portability | Open | Same actor binaries, SDK, builders, and validators; route changes through signed configuration and provisioning only |
| Public deployment/execution | Intentionally absent under ADR 0023 | Remains explicit until owner authorizes evidence or scope changes |
| Latency/resource envelope | Not measured for composed corridor | Report setup/runtime latency, CPU, memory, storage, chain compute/fees, and concurrency envelope |
| Availability/recovery objectives | Not defined | Define and verify operator-facing objectives |
| Observability and alert coverage | Partial lower-layer diagnostics; not measured | Every external effect, wait, recovery, and terminal failure is diagnosable without exposing secrets |
| Operator runbooks | Partial manual component flows | Clean setup, normal operation, recovery, upgrade, backup/restore, and teardown paths complete |
| Release artifacts and provenance | Partial pinned inputs | Reproducible package, SBOM/provenance, signatures, scans, and release notes complete |
| Upstream production blockers | Living register exists | Every Logos-owned dependency risk has owner/status/impact/workaround and release disposition |

## Update rules

1. Record the exact commit, command, run ID, cache/network assumptions, and
   result behind every numeric improvement.
2. Never count a lower-lane unit or contract test as a composed PoC run.
3. Never count carried evidence as phase completion until it is revalidated
   against the working vertical path.
4. Record failed attempts and flakes as well as successes.
5. Update the implementation plan, manual guide, architecture diagrams, and
   external-resource inventory in the same change when their facts change.
