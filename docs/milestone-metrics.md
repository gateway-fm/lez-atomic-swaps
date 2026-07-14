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
| M2 | Reproducible PoC | In progress; `TakerSellsLez` is GREEN, 1 of 2 local LEZ/ZEC happy directions | None | Remain in PoC until the owner says to end or switch |
| M3 | Not active | Awaiting owner transition | None | Not requested |
| M4 | Not active | Awaiting owner transition | None | Not requested |
| M5 | Not active | Awaiting owner transition | None | Not requested |
| M6 | Not active | Awaiting owner transition | None | Not requested |
| M7 | Not active | Awaiting owner transition | None | Not requested |

## M2 current scorecard

### PoC

| Metric | Current measurement | Evidence or next measurement point |
|---|---|---|
| Full corridor reproductions | 1 successful direction: `m2poc-corridor-fresh-20260714o`; earlier 14d/14e/14f and fresh 14i through 14n remain failure evidence | Run 14o completed `TakerSellsLez`; the reverse `TakerSellsForeign` direction remains |
| Clean-host reproductions | 0 | Run 14o used fresh isolated devnets and actor state on a host with verified caches; measure a cold clean-host repeat after both directions are GREEN |
| Setup duration | Run 14o entered effects after 400 ms of provisioning; earlier baselines were 6 seconds in 14d, 17 seconds in 14e, and 5590 ms in 14f | Prebuild happens before the protocol clock; run 14o retained 48600 ms at the pre-effect budget check |
| Happy-path execution duration | 25.370 seconds from provisioning through both terminal actor states | The cap is 49 seconds, preserving a true minimum 10-second margin against the 60-second LEZ delay despite whole-second deadline truncation |
| Required local chain environments | 2: pinned LEZ v0.2 and pinned Zebra Regtest | Both were crossed by 14d; 14e crossed LEZ and queried Zebra at tip 105 but performed no Zcash effect |
| LEZ processes in the target environment | 3: Bedrock, non-standalone sequencer, indexer | All three remained live while Vault onboarding, checked deployment, native initialize/fund/claim, same-tip state reads, and manual indexer finality completed |
| Effect-bearing swap actors | 2 independent reference actors and 2 role bridges completed one direction | Run 14o recorded 78 actor events across 39 rounds; maker and taker independently reached revision 4 `Completed` |
| Exact v0.2 PoC role bridge | 1 executable; both role processes completed the full first-direction method sequence | Run 14o crossed initialize, fund, bounded observe, revealing claim, and exact submit. One payload-free `moving_tip` event occurred; no secret or request payload was logged |
| Same-run retry evidence | 1 successful retry in run 14o; maximum is 3 exact retries | Taker round 2 retried `lez_bridge.v1.observe_escrow` once after `moving_tip`, then the same run completed |
| Supported happy directions | 1 of 2 composed | `TakerSellsLez` is GREEN; reverse `TakerSellsForeign` is the remaining PoC direction |
| Actual maker/taker Vault Claims | 2 of 2 finalized on the retained local LEZ run | Maker block 29 and taker block 30 are exact finalized indexer evidence; this onboards the LEZ actors but is not a swap corridor |
| Checked LEZ escrow lifecycle | 1 local v0.2 initialize/fund/claim slice finalized | Checked ProgramId `f8385049...0fbe`; initialize block 219, fund block 220, claim block 223; maker deposited 700 and taker received exactly 700 |
| Zcash/reference-actor fixture readiness | 1 pair provisioned; 0 currently runnable retained pairs | Stable Zebra tip/output proved provisioning; the saved LEZ window 1..256 was stale by audit tip 389, so it must be reprovisioned just in time |
| Actual Zcash HTLC lifecycle | 1 terminal composed funding and claim | Funding `255b991f...dceab:0` entered height 106, had two confirmations before LEZ reveal, and was spent by claim `a2b41c5f...be16e` at height 108 |
| Final state and balance proof | 1 cross-chain terminal proof plus the earlier terminal LEZ-only proof | LEZ initialize/fund/claim finalized in blocks 264/265/266; terminal status `Claimed`, custody 0, depositor 100000, claimant 150000; both actor stores are revision 4 `Completed` |
| Public RPCs, faucets, or public funds used | 0 | Run 14o used only the isolated local LEZ and Zebra endpoints and deterministic local Vault/Regtest funds; cold artifact provisioning remains an external availability dependency |
| Cleanup and retained state | Bridge processes are exact-PID/start-time/executable scoped; failure roots retained; chain funds are not rolled back | Run 14o stopped only its role bridges and retained private evidence. Failed 14j retains 50000 LEZ in a distinct swap; never reuse failed-run actor files, swap, or funding input |
| Manual reproduction path | First-direction one-command runner and expected evidence documented | Requires already-running explicit fresh local nodes; allocate a new run ID, ports, and output root. Reverse-direction command/evidence remains unavailable |

Open PoC work is not an external blocker. Run 14o live-proved the no-round-cap
loop, 0.10-second polling, fail-closed millisecond clock, KILL-bounded calls, one
of at most three exact retries, two-confirmation Zcash reveal gate, exact
claim/follow-up, and terminal LEZ indexer/account evidence inside 25.370
seconds. The runner prebuilds, provisions at a fresh tip, starts run-owned
bridge ports, mines only after a reported Zcash effect, and fails on
deadline/headroom. Fresh attempts 14i and 14k through 14n stopped before any
chain effect; 14j proved the two-confirmation invariant by refusing the reveal
after one block and retains 50000 LEZ in a nonretryable distinct swap. Cross-
chain atomicity remains protocol ordering and recoverability rather than one
database transaction. Implement and reproduce the reverse direction before the
PoC gate can be met. Recovery/refund and ambiguity hardening remain after the
owner ends PoC unless needed to avoid corrupting the happy-path evidence.
Logos-owned production issues
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
