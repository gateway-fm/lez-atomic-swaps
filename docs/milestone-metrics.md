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
| M2 | Reproducible PoC | Gate met; both local LEZ/ZEC happy directions and the dormant configuration-only public-portability contract are GREEN, while documentation and repository completion gates remain before the milestone tag | None | Await owner review; remain in PoC until the owner says to end or switch |
| M3 | Not active | Awaiting owner transition | None | Not requested |
| M4 | Not active | Awaiting owner transition | None | Not requested |
| M5 | Not active | Awaiting owner transition | None | Not requested |
| M6 | Not active | Awaiting owner transition | None | Not requested |
| M7 | Not active | Awaiting owner transition | None | Not requested |

## M2 current scorecard

### PoC

| Metric | Current measurement | Evidence or next measurement point |
|---|---|---|
| Full corridor reproductions | 2 successful directions: `m2poc-corridor-fresh-20260714o` and `m2poc-corridor-reverse-fresh-20260714c` | Run 14o completed `TakerSellsLez`; reverse run 14c completed `TakerSellsForeign`. The checked-in secret-safe evidence packets retain exact transactions, blocks, actors, and limitations |
| Current-schema exact-tree replay | 2 of 2 actual-node directions GREEN: `m2cert-schema3-forward-2d09997-20260714a` and `m2cert-schema3-reverse-2d09997-20260714a` | Schema-v3 typed local routes crossed the retained pinned LEZ v0.2 and Zebra Regtest nodes. Forward completed in 46 rounds with 0 retries; reverse completed in 33 rounds with 2 bounded retries. Both actors reached `completed`, atomic order was observed, and no public RPC/faucet was used |
| Clean-host reproductions | 0 | Both successes used fresh run-owned actor state and isolated retained devnets on a host with verified caches. A cold clean-host repeat remains not measured and is not inferred from the two successful directions |
| Setup duration | Run 14o entered effects after 400 ms of provisioning; reverse 14c entered effects after 300 ms | Prebuild happens before the protocol clock. Earlier partial baselines were 6 seconds in 14d, 17 seconds in 14e, and 5590 ms in 14f |
| Happy-path execution duration | 25.370 seconds for 14o; 26.960 seconds for reverse 14c, each measured from provisioning through both terminal actor states | The cap is 49 seconds, preserving a true minimum 10-second margin against the 60-second LEZ delay despite whole-second deadline truncation |
| Required local chain environments | 2: pinned LEZ v0.2 and pinned Zebra Regtest | Both successful directions crossed the same retained endpoint tuple serially; the runner now holds an endpoint-tuple advisory lock so effect-bearing corridor runs cannot overlap |
| LEZ processes in the target environment | 3: Bedrock, non-standalone sequencer, indexer | All three remained live while Vault onboarding, checked deployment, native initialize/fund/claim, same-tip state reads, and manual indexer finality completed |
| Effect-bearing swap actors | 2 independent reference actors and 2 role bridges completed each direction | Run 14o recorded 78 actor events across 39 rounds; reverse 14c recorded 100 events across 50 rounds. Maker and taker independently reached revision 4 `Completed` in both runs |
| Exact v0.2 PoC role bridge | 1 executable; both role processes completed the direction-correct method sequence in both runs | Run 14o used taker LEZ deposit then maker reveal; reverse 14c used maker LEZ deposit then taker reveal. Both crossed initialize, fund, bounded observe, revealing claim, and exact submit |
| Same-run retry evidence | 1 successful retry in run 14o and 0 in reverse 14c; maximum is 3 exact retries | Taker round 2 in 14o retried `lez_bridge.v1.observe_escrow` once after payload-free `moving_tip`, then completed. Reverse 14c completed without a same-run retry |
| Supported happy directions | 2 of 2 composed | `TakerSellsLez` and `TakerSellsForeign` are GREEN; this meets the PoC happy-path gate without entering a later phase or creating an M2 tag |
| Actual maker/taker Vault Claims | 2 of 2 finalized on the retained local LEZ run | Maker block 29 and taker block 30 are exact finalized indexer evidence; this onboards the LEZ actors but is not a swap corridor |
| Checked LEZ escrow lifecycles | 2 composed direction-specific initialize/fund/claim lifecycles plus the earlier local-only slice finalized | Forward effects finalized in blocks 264/265/266. Reverse effects finalized in blocks 641/642/643. Both terminal states are `Claimed` with custody 0; the earlier 700-unit slice remains component evidence |
| Zcash/reference-actor fixture readiness | 2 successful just-in-time pairs were provisioned and consumed; 0 retained actor pairs are advertised as reusable | Stable Zebra identity/output checks ran before each corridor. Every repetition must select fresh current inputs and a fresh LEZ window; saved or failed-run files and candidates are never reused |
| Actual Zcash HTLC lifecycle | 2 terminal composed funding and claim lifecycles | Forward funding `255b991f...dceab:0` at height 106 was spent at height 108. Reverse funding `181c4baa...14f0:0` at height 113 was spent at height 115; both had two confirmations before LEZ reveal |
| Final state and balance proof | 2 cross-chain terminal proofs plus the earlier terminal LEZ-only proof | Forward LEZ blocks 264/265/266 ended custody/depositor/claimant at 0/100000/150000. Reverse blocks 641/642/643 ended 0/0/150000. Both pairs of actor stores are revision 4 `Completed` |
| Public RPCs, faucets, or public funds used | 0 | Both successes used only isolated local LEZ and Zebra endpoints and deterministic local Vault/Regtest funds; cold artifact provisioning remains an external availability dependency |
| Dormant public route contract | 5 composed boundaries, 0 public calls | Signed public LEZ agreement activation, actor schema-v3 routes, Zebra HTTPS/API-key transport, the sidecar's exact official-public outbound profile, and the authenticated deployment-evidence-to-runtime-identity handoff pass local executable contract tests. The actor-facing sidecar listener stays loopback-only. Provisioning covers one happy case, no-clobber output, eight authenticated evidence mutations, unauthenticated chain-fact tampering, bounded/non-regular input, and exact owner-only key-file validation. Live LEZ finalized-tip availability and provider rate limits remain unmeasured |
| Cleanup and retained state | Bridge processes are exact-PID/start-time/executable scoped; endpoint tuples are serialized; failure roots are retained; chain funds are not rolled back | Successful runs stopped only their role bridges. Failed 14j and reverse attempts 14a/14b retain effects in distinct nonretryable swaps; never reuse their actor files, swaps, candidates, or funds |
| PoC defect evidence | 1 directionality defect reproduced in 2 effect-bearing reverse attempts, then corrected | Reverse attempts 14a/14b exposed a forward-only canonical LEZ validator. The correction binds validation to the agreement-derived LEZ depositor; its focused regression and all 35 SDK lifecycle tests passed before reverse 14c |
| Manual reproduction path | One direction-aware runner and expected evidence for both directions are documented | Requires already-running explicit fresh local nodes, a unique run ID/output root per attempt, and serialized runs. The retained evidence endpoints and run IDs are examples, never defaults |

The PoC happy-path gate is met, but the owner has not ended the PoC phase and no
M2 tag exists. Run 14o and reverse 14c live-prove the no-round-cap loop,
0.10-second polling, fail-closed millisecond clock, KILL-bounded calls,
maximum-three exact retry policy, direction-derived effect owners,
two-confirmation Zcash reveal gate, exact claim/follow-up order, and terminal
LEZ indexer/account evidence. The runner prebuilds, provisions at a fresh tip,
starts run-owned bridge ports, mines only after a reported Zcash effect, locks
the exact shared endpoint tuple against concurrent corridor use, and fails on
deadline/headroom. Forward failures 14i and 14k through 14n made no effect;
14j and reverse 14a/14b retain effects in distinct nonretryable swaps. Cross-
chain atomicity remains protocol ordering and recoverability rather than one
database transaction. The configuration-portability contract is now locally
GREEN without public I/O. Documentation and repository completion gates remain
before an M2 tag. Recovery/refund, restart, reorg,
ambiguity, concurrency, and broader hardening wait for owner transition unless
needed to protect correctness. Logos-owned production issues remain nonblocking
for this local phase and stay in the upstream register.

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
| Repository-controlled critical/high vulnerabilities | All eleven independently checked-in Rust lockfiles resolve non-yanked `spin 0.9.9`; exact advisory/bans/licenses/sources audits are GREEN for the root and all ten nested graphs with no `spin` exception. Final-image scanning remains in the exact-commit certification pass. | 0 unresolved |
| Logos-owned advisory exceptions | Present and enumerated in the upstream production-blocker register | Exact, narrow, reviewed, and non-expanding for local evidence |
| Threat-model findings | Not rebaselined for the composed corridor | Count by severity with disposition and regression evidence |
| Secret exposure findings | No composed-run measurement | 0; logs, evidence, configs, stores, and process arguments included |
| License/source-policy violations | Prior gates carried; fresh phase result pending | 0 undisclosed violations or license bombs |
| Lint/static-analysis/image gates | CI gates exist; fresh phase result pending | All required jobs GREEN on the exact evidence commit |

### Production readiness

Status: awaiting owner transition.

| Metric | Current measurement | Production-readiness target |
|---|---|---|
| Public configuration portability | Locally contract-proven, including authenticated offline evidence provisioning and no-clobber exact identity output; not live-exercised | Same actor binaries, SDK, builders, and validators; route changes through signed configuration, credentials, funding, and verified LEZ deployment provisioning only |
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
