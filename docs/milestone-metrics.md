# Milestone delivery metrics

Last updated: 2026-07-16

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
| M2 | Certified local-functional PoC | Canonical Docker-built/deployed artifact, both local LEZ/ZEC directions, and the exact-tree repository gates are GREEN; later hardening is deferred | `m2-complete` | M2 completion/tag directed; no QA or M3 transition requested |
| M3 | Progressive local PoC | PoC gate met by `m3actor-20260716n` at commit `6ded2f9`: the repository-owned role-fixed actor completed both directions against fresh actual local Core/LEZ services, with both roles revision 4 `Completed`, zero replay resubmissions, and exact cleanup. No cross-system atomic commit or production-hardening completion is claimed | None | Owner entered M3 on 2026-07-14; PoC evidence and local closure gates are GREEN, while the owner phase transition and any milestone tag remain pending |
| M4 | Not active | Awaiting owner transition | None | Not requested |
| M5 | Not active | Awaiting owner transition | None | Not requested |
| M6 | Not active | Awaiting owner transition | None | Not requested |
| M7 | Not active | Awaiting owner transition | None | Not requested |

## M3 PoC scorecard

Status: progressive local PoC evidence gate met. Counts below deliberately measure
evidence instead of assigning a percentage to unlike work items.

| Metric | Current measurement | Evidence or next measurement point |
|---|---|---|
| Live authorities reconciled | 2 of 2 | RFP master commit `121da225...5542a` / blob `d0fa52b`; open/reopened accepted issue #112 body SHA-256 `49356263...f1c87`; issue #61 excluded. Re-fetched 2026-07-16 |
| Executable BTC-specific crates/components | 3 BTC crates, 1 actual-node runner, 4 fixture examples/CLIs, 1 role-local journal, 1 role-runner crate, and 2 of 2 live actor directions GREEN | Source tests remain GREEN. Run `m3actor-20260716n` binds commit `6ded2f9`, certified script hashes, fresh one-shot actor processes, four terminal role stores, and exact replay |
| Typed Bitcoin Core adapter | 18 of 18 adapter tests plus 2 of 2 actual-node actor integrations GREEN | Exact Core 31.1 readiness, funding, claim, canonical evidence, and one-attempt submission are covered. The live fix sends `gettxspendingprevout` flags as Core 31.1's single options object. Testnet4 remains production-portability work |
| LEZ BTC witnessed path | Fresh checked deployment/onboarding and 2 of 2 actor directions GREEN | Deployment finalized in block 6; maker/taker Vault Claims in 9/12. Foreign init/fund/claim finalized in 16/19/25; LEZ-direction init/fund/claim in 31/34/42. Bounded scans and finite 30-second bridge reads are live-proven; upstream historical-account proof/snapshot limits remain |
| Durable signer, public-effect, and BTC recovery-state boundaries | Existing focused suites plus 2 fresh full-lifecycle actor integrations GREEN | Both directions retained two Bitcoin and three LEZ effects, both roles revision 4, and unchanged counts after replay. Process-kill timing and malicious database-owner authentication remain pending; no chain/database atomic commit is claimed |
| Dependency groups accepted | 2 of 5 entry candidates | Core 31.1 and exact-pinned `bitcoin` 0.32.101 graphs passed their acceptance gates. The exact `musig2` 0.4.1 graph is locked, policy-gated, exercised through Core, the real LEZ guest, and independent crash-safe role processes, but remains an unaccepted beta/unaudited candidate pending stronger secret handling and review; `miniscript` and `corepc` also remain unaccepted |
| Fresh identity, guest, and pre-lock orchestration | Actual-node effect-bearing run GREEN | `m3actor-20260716n` generated fresh owners/Vaults, deployed exact guest `a199c5be...e293` / ProgramId `39b6a4db...4dec`, finalized onboarding, pre-admitted exact Bitcoin funding, finalized agreement and journals before effects, and hit planned anchors 102/104 |
| Actual local M3 node compositions | Repository-owned actor: 2 of 2 fresh directions on 1 isolated Core/LEZ tuple | Run used Core `32913`, Bedrock `32914`, sequencer `32915`, indexer `32916`, and dynamic role sidecars. No public RPC/faucet/funds; exact cleanup attestation passed without targeting foreign resources |
| Supported happy directions completed | Fresh repo-owned actor composition: 2 of 2 | `TakerSellsForeign` and `TakerSellsLez` both reached revision 4 for maker and taker. Each Bitcoin contract outpoint was spent once and each LEZ custody account ended zero |
| Runnable manual BTC flows | Repository-owned workflow is implemented and actual-node evidenced | The maintained guide mirrors the successful bootstrap, pre-lock funding/admission/finalization, role journals, revisions zero through four, terminal/replay checks, and local-only inventory. Cold-cache setup remains a prerequisite, not a runtime RPC |
| Gateway proposal acceptance errata | 1 nonblocking upstream production/review item | GW-M3-001 records the nonexistent DLC Schnorr adaptor-vector path and the proposed replacement evidence contract. It does not block local milestone certification under the owner policy, but remains visible for Logos/Gateway review and production readiness |
| QA / chaos / information security / production phases | Not active | Each phase begins only after its owner transition; continuous CI/security baselines remain enforced |
| M3 completion tag | None | Created only on the exact pushed commit after every selected M3 gate is evidenced |

## M2 current scorecard

### PoC

| Metric | Current measurement | Evidence or next measurement point |
|---|---|---|
| Full corridor reproductions | 2 successful directions: `m2poc-corridor-fresh-20260714o` and `m2poc-corridor-reverse-fresh-20260714c` | Run 14o completed `TakerSellsLez`; reverse run 14c completed `TakerSellsForeign`. The checked-in secret-safe evidence packets retain exact transactions, blocks, actors, and limitations |
| Current-schema exact-tree replay | 2 of 2 actual-node directions GREEN: `m2cert-schema3-forward-2d09997-20260714a` and `m2cert-schema3-reverse-2d09997-20260714a` | Schema-v3 typed local routes crossed the retained pinned LEZ v0.2 and Zebra Regtest nodes. Forward completed in 46 rounds with 0 retries; reverse completed in 33 rounds with 2 bounded retries. Both actors reached `completed`, atomic order was observed, and no public RPC/faucet was used |
| Canonical Docker artifact and corridor replay | 2 of 2 actual-node directions GREEN after exact local deployment | Direct Docker and Docker-backed methods builds agree on ELF `c85055f6...c9d2e` and ImageID/ProgramId `5cf8c5a4...329c1`. Deployment transaction `bd16808e...733f` finalized in local LEZ block 2582. Canonical forward/reverse runs completed in 38/47 rounds with 2/0 bounded retries and no public resources; see `m2-canonical-local-certification-20260714.json` |
| Clean-host reproductions | 0 | Both successes used fresh run-owned actor state and isolated retained devnets on a host with verified caches. A cold clean-host repeat remains not measured and is not inferred from the two successful directions |
| Setup duration | Run 14o entered effects after 400 ms of provisioning; reverse 14c entered effects after 300 ms | Prebuild happens before the protocol clock. Earlier partial baselines were 6 seconds in 14d, 17 seconds in 14e, and 5590 ms in 14f |
| Happy-path execution duration | 25.370 seconds for 14o; 26.960 seconds for reverse 14c, each measured from provisioning through both terminal actor states | The cap is 49 seconds, preserving a true minimum 10-second margin against the 60-second LEZ delay despite whole-second deadline truncation |
| Required local chain environments | 2: pinned LEZ v0.2 and pinned Zebra Regtest | Both successful directions crossed the same retained endpoint tuple serially; the runner now holds an endpoint-tuple advisory lock so effect-bearing corridor runs cannot overlap |
| LEZ processes in the target environment | 3: Bedrock, non-standalone sequencer, indexer | All three remained live while Vault onboarding, checked deployment, native initialize/fund/claim, same-tip state reads, and manual indexer finality completed |
| Effect-bearing swap actors | 2 independent reference actors and 2 role bridges completed each direction | Run 14o recorded 78 actor events across 39 rounds; reverse 14c recorded 100 events across 50 rounds. Maker and taker independently reached revision 4 `Completed` in both runs |
| Exact v0.2 PoC role bridge | 1 executable; both role processes completed the direction-correct method sequence in both runs | Run 14o used taker LEZ deposit then maker reveal; reverse 14c used maker LEZ deposit then taker reveal. Both crossed initialize, fund, bounded observe, revealing claim, and exact submit |
| Same-run retry evidence | Retained schema-v2 runs: 1 successful retry in 14o and 0 in reverse 14c; current-schema runs: 0 in forward and 2 in reverse; configured ceiling is 8 exact same-run retries within the unchanged absolute deadline | Taker round 2 in 14o retried `lez_bridge.v1.observe_escrow` once after payload-free `moving_tip`, then completed. Reverse 14c completed without a same-run retry. Current-schema forward completed without retries; current-schema reverse completed after two bounded same-run retries |
| Supported happy directions | 2 of 2 composed | `TakerSellsLez` and `TakerSellsForeign` are GREEN; `m2-complete` binds this PoC boundary without entering a later phase |
| Actual maker/taker Vault Claims | 2 of 2 finalized on the retained local LEZ run | Maker block 29 and taker block 30 are exact finalized indexer evidence; this onboards the LEZ actors but is not a swap corridor |
| Checked LEZ escrow lifecycles | 2 canonical plus 2 retained historical composed initialize/fund/claim lifecycles, and the earlier local-only slice | Canonical forward effects finalized in blocks 2594/2595/2596; canonical reverse effects finalized in 2605/2606/2607, all under ProgramId `5cf8...29c1`. Both ended `Claimed` with custody 0. Blocks 264/265/266 and 641/642/643 remain immutable pre-canonical behavior evidence |
| Zcash/reference-actor fixture readiness | 2 successful just-in-time pairs were provisioned and consumed; 0 retained actor pairs are advertised as reusable | Stable Zebra identity/output checks ran before each corridor. Every repetition must select fresh current inputs and a fresh LEZ window; saved or failed-run files and candidates are never reused |
| Actual Zcash HTLC lifecycle | 2 canonical terminal composed funding and claim lifecycles plus retained historical evidence | Canonical forward funding `0d041be6...b64c:0` at height 122 was spent by `8555c3d7...77d7` at 124. Canonical reverse funding `1cbb5923...4785:0` at 125 was spent by `bfbd4379...9b2a` at 127. Both had a second confirmation before LEZ reveal; the older height 106/108 and 113/115 runs remain historical |
| Final state and balance proof | 2 canonical cross-chain terminal proofs plus retained historical and LEZ-only proofs | Canonical forward block 2596 ended custody/depositor/claimant at 0/100000/50000 from 0/150000/0; canonical reverse block 2607 ended 0/0/150000 from 0/50000/100000. Each conserves 150000 LEZ and both pairs of actor stores are revision 4 `Completed` |
| Public RPCs, faucets, or public funds used | 0 | Both successes used only isolated local LEZ and Zebra endpoints and deterministic local Vault/Regtest funds; cold artifact provisioning remains an external availability dependency |
| Dormant public route contract | 5 composed boundaries, 0 public calls | Signed public LEZ agreement activation, actor schema-v3 routes, Zebra HTTPS/API-key transport, the sidecar's exact official-public outbound profile, and the authenticated deployment-evidence-to-runtime-identity handoff pass local executable contract tests. The actor-facing sidecar listener stays loopback-only. Provisioning uses domain-separated HMAC-SHA256 and covers one happy case, no-clobber output, eight authenticated evidence mutations, wrong-key plus unauthenticated semantic/envelope chain-fact tampering, bounded/non-regular input, and exact owner-only key-file validation. Live LEZ finalized-tip availability and provider rate limits remain unmeasured |
| Cleanup and retained state | Bridge processes are exact-PID/start-time/executable scoped; endpoint tuples are serialized; failure roots are retained; chain funds are not rolled back | Successful runs stopped only their role bridges. Failed 14j and reverse attempts 14a/14b retain effects in distinct nonretryable swaps; never reuse their actor files, swaps, candidates, or funds |
| PoC defect evidence | 1 directionality defect reproduced in 2 effect-bearing reverse attempts, then corrected | Reverse attempts 14a/14b exposed a forward-only canonical LEZ validator. The correction binds validation to the agreement-derived LEZ depositor; its focused regression and all 35 SDK lifecycle tests passed before reverse 14c |
| Manual reproduction path | One direction-aware runner and expected evidence for both directions are documented | Requires already-running explicit fresh local nodes, a unique run ID/output root per attempt, and serialized runs. The retained evidence endpoints and run IDs are examples, never defaults |
| Exact LEZ v0.2 closure verifier | GREEN | Root compatibility, escrow and local-stack tests; strict Clippy and rustdoc; canonical Docker guest artifact/ProgramId equality; recursive native/refund/rollback/two-definition suites; deployer tests; and dependency source/feature checks all passed |
| Fresh Zebra closure E2E | 2 of 2 GREEN | Isolated `m2cert-final-bc31373-zebra-20260714b` passed restart/requery/actual-fork removal and real actor-key fund/claim/refund through Zebra consensus; the schema-v10 expectation fix followed a RED-to-GREEN defect audit |
| Supply-chain and image vulnerability closure | GREEN | All 11 Rust dependency graphs pass advisories/bans/licenses/sources, npm audit reports zero vulnerabilities, and fail-hard Trivy 0.70.0 with a fresh database reports zero HIGH/CRITICAL findings in the exact Zebra image |
| Architecture and repository-policy closure | GREEN | All 95 tracked Mermaid diagrams render with the repository harness; traceability, CI hardening, formatting, strict Clippy, tests, and docs gates pass. Remote-hosted CI status is not inferred from this checked-in local evidence |

The local-functional PoC boundary is certified under `m2-complete`; the owner
has not entered QA or M3. Run 14o and reverse 14c live-prove the no-round-cap loop,
0.10-second polling, fail-closed millisecond clock, KILL-bounded calls,
maximum-eight exact same-run retry policy, direction-derived effect owners,
two-confirmation Zcash reveal gate, exact claim/follow-up order, and terminal
LEZ indexer/account evidence. The runner prebuilds, provisions at a fresh tip,
starts run-owned bridge ports, mines only after a reported Zcash effect, locks
the exact shared endpoint tuple against concurrent corridor use, and fails on
deadline/headroom. Forward failures 14i and 14k through 14n made no effect;
14j and reverse 14a/14b retain effects in distinct nonretryable swaps. Cross-
chain atomicity remains protocol ordering and recoverability rather than one
database transaction. The configuration-portability contract is locally GREEN without public I/O,
and the exact local repository closure gates are GREEN. Recovery/refund,
restart, reorg,
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
