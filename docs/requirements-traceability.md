# Requirements traceability

Last reconciled: 2026-07-16 against the live RFP-003 and Gateway's accepted
replacement proposal #112. Issue #61 is superseded. IDs below follow the RFP's
Functionality, Usability, Reliability, Performance, Supportability, and Demo
ordering. A row is not acceptance: `Passing` requires the named evidence at the
real actor and chain boundary described by the requirement.

The 2026-07-16 authority audit pins the live RFP repository at master commit
`121da225de1930c5ba693ebbef80ee788d55542a` and RFP-003 file blob `d0fa52b`.
Replacement issue #112 is open/reopened with the `accepted` and `RFP-003`
labels and still supersedes issue #61. The commit/blob are immutable audit
anchors; the issue state is mutable and must be reread before milestone
acceptance.

The executable guard `scripts/check-requirements-traceability.sh` requires every
hard-requirement ID to occur exactly once. RFP Supportability 4 ultimately
requires a corresponding test—not merely a row—for every F, U, R, and P item.

ADR 0023 records an owner-approved stealth M2 certification profile. The M2 tag
requires a fully functional private corridor across one actual
public-compatible local LEZ v0.2 devnet (source-verified immutable Bedrock,
indexer, and non-standalone sequencer pins/wiring, not standalone mock settlement)
and one actual local Zcash Regtest devnet with independent maker/taker
processes, while public deployment/testnet evidence remains visibly incomplete
in this matrix and moves to production readiness. The local and future public
routes must use the same binaries, ports, builders, and validators; only signed
configuration, funding, authentication, confirmation policy, and required LEZ
on-chain deployment may change.

The private happy-path portion of that profile is now GREEN in both accepted
directions. Runs `m2poc-corridor-fresh-20260714o` and
`m2poc-corridor-reverse-fresh-20260714c` used independent maker/taker
processes and stores, the pinned local LEZ v0.2 services, and Zebra Regtest;
both role stores reached revision 4 `Completed`. Their secret-safe evidence is
[`m2-taker-sells-lez-corridor-20260714.json`](evidence/m2-taker-sells-lez-corridor-20260714.json)
and
[`m2-taker-sells-foreign-corridor-20260714.json`](evidence/m2-taker-sells-foreign-corridor-20260714.json).
This meets the two-direction PoC gate. The dormant public-portability contract
is also locally GREEN: exact public-route identity contracts, schema-v3
actor/Zebra routes, and the exact official-public LEZ sidecar route are
contract-tested without public I/O. Two later schema-v3 certification runs also
repeat both accepted directions through the actual local nodes. Final canonical
runs `m2cert-canonical-forward-bb53daf-20260714a` and
`m2cert-canonical-reverse-bb53daf-20260714a` then repeated both directions
against the Docker-built/deployed ProgramId `5cf8c5a4...329c1`: all four actor
stores are terminal, six LEZ effects are finalized, both confirmed Zcash HTLC
outputs are spent after the LEZ reveal, and both LEZ custody accounts are zero.
The immutable evidence is
[`m2-canonical-local-certification-20260714.json`](evidence/m2-canonical-local-certification-20260714.json).
M2 is certified at the local-functional PoC boundary under `m2-complete`.
Actual-node refund/restart/reorg/chaos and live public execution retain their
explicitly deferred status; the tag itself did not imply a transition to QA or
M3. The owner subsequently entered M3 on 2026-07-14.

The M3 local happy-path actor gate is now GREEN in both directions. Run
`m3actor-20260716n` at commit
`6ded2f9b8ba9ec8e0cfbf06287da92d34256f91a` used fresh one-shot maker
and taker processes against actual local Bitcoin Core 31.1 Regtest and LEZ
v0.2 services. Both roles reached revision 4 `Completed` in both directions;
replay added zero submissions and exact cleanup targeted no foreign resources.
No public RPC, faucet, or public funds were used. The schema-3 public `recover`
command is deterministic and actual-node GREEN for both ordered LEZ/Bitcoin
two-lock refund directions. Run `m3refund-20260716h` used fresh one-shot
roles, Core 31.1 Regtest, and private-local LEZ v0.2 with 3.0-second slots; all
four role/direction stores reached revision 4 `Refunded`, each direction
retained two Bitcoin and three LEZ effects, and replay added zero submissions.
The run used no public RPC, faucet, or public funds. The revision-one
`TakerLockConfirmed` to revision-two terminal `Refunded` store/projector is
GREEN in both roles and directions. The canonical agreement now also signs and
validates an explicit last-safe maker-second-lock cutoff: RED exposed the
missing safe signed bound, and GREEN binds and round-trips it with a nonzero,
overflow-checked reaction margin before the earlier refund bound. That
field is the selected implementation of RFP-derived race safety, not literal
RFP wording. The exact finalized LEZ maker-lock
presence/absence/uncertainty classifier and refund-side live actor admission are
GREEN. Run `m3firstlock-20260716h` proves the durable baseline and both
refund-side actual-node absent-maker journeys. Live maker-lock admission, two
overlapping swaps, the full-lifecycle public BTC SDK surface, D1 recordings,
process-kill/reorg, chaos, public Testnet4/LEZ deployment, production custody,
and formal review remain open.

Commit `8870910` is a narrower GREEN M3 component result, not new actual-node
evidence. The strict schema-4 actor seam requires complete direction-shaped
Maker material, reconstructs exact lock effects through `BtcPairSdk`, and uses
`SqliteBtcMakerLockJournal` for observe-before-send, one-attempt ordering,
cutoff/first-lock rechecks, exact final observation, and atomic revision-two
close. `Accepted` and `Unknown` never rearm. Schema 3 remains supported only as
observation-only compatibility with zero send attempts. Its focused gate is 73
of 73 GREEN (65 library plus 8 CLI integration), and strict Clippy, rustdoc,
formatting, and diff checks pass. The live schema-4 Maker CLI still fails closed.
LEZ v0.2 cannot prove pending-level initialization absence. Pushed `3336b6e`
adds journal observation `ExactIdempotentSubmissionSafe`, which grants one CAS/send only when
the adapter/node call is bound to the same exact ID and bytes. It does not claim
absence, never rearms `Started` or `Unknown`, and still requires canonical
evidence for acceptance. Its store-focused tests/gates are GREEN, but a live
adapter/node port must still prove the idempotence contract. Pushed `11111dd`
maps this distinct observation through the typed actor; one drive submits once
and a restarted actor submits zero times. That is actor no-rearm evidence, not
live adapter/node composition. Pushed `923586b`
proves the agreement-selected LEZ escrow is currently `Funded` with complete
custody under one unchanged canonical clock for either role/direction. It is
state-only and neither finality nor exact initialize/fund transaction evidence.
The Bitcoin-maker direction still lacks a composed view joining those current
facts with exact bytes and finalized fund evidence. Current runner schema-4
edits are uncommitted and are not evidence. No
actual-node maker-lock admission packet or `m3-complete` tag exists for this
slice.

## Functionality

| ID | Contract | Acceptance evidence | Status / milestone |
|---|---|---|---|
| F1 | No central server; Delivery advertisements and Chat coordination | Role E2E discovers and negotiates through real Delivery/Chat adapters; central services absent; UJ-006 repeats with both stopped after lock | Planned M5 |
| F2 | LEZ–BTC via BIP-340 adaptor signatures and Taproot key-path cooperative claim | Proposed GW-M3-001 evidence contract: official BIP-340/BIP-327 vectors, swap-specific adaptor fixtures, independent cross-check, tweak/parity-aware signatures under the P2TR output key, Core consensus, and both-direction actual-node happy/refund/concurrency role E2E. The nonexistent DLC `AdaptorSignature.md` is a nonblocking Gateway/Logos production-review erratum | Happy path is actual-node GREEN in both directions in `m3actor-20260716n`. Run `m3refund-20260716h` executes both ordered two-lock timeouts, clean pushed-commit run `m3firstlock-20260716h` executes both absent-maker first-lock refunds, and clean pushed-commit run `m3survivor-20260716c` executes both post-reveal maker-survivor continuations with protected taker absence, fresh-maker revision 3/restart/terminality, per-chain zero catch-up resubmission, and zero terminal replay. Live maker-lock admission at the cutoff boundary, two overlapping swaps, the full-lifecycle public BTC SDK, D1 artifacts, process-kill/reorg, production authority, Testnet4, formal review, and GW-M3-001 replacement acceptance remain open |
| F3 | LEZ–XMR via Ed25519 adaptor signatures, cross-curve DLEQ, and spend-key share | LEZ-first COMIT/DLEQ vectors; `monerod`/wallet RPC stagenet happy/refund/concurrency role E2E; spend-key recovery fault case; XMR-first rejected | Design constrained by primary reference; implementation M4 |
| F4 | LEZ-transparent-ZEC via BIP-199 HTLC; LEZ claim/refund precedes ZEC and the ZEC refund strictly outlives LEZ by the documented margin | Exact vectors, profiles, agreement validation, canonical funding observations, and fresh maker eligibility pass. Separate role-fixed maker/taker SDKs and schema-v10 SQLite stores complete and reopen the direction-fixed claim or refund lifecycles. The authenticated eight-method client/server protocol, official native/revealing-claim/refund preparation and observation, executable role-isolated sidecars, main escrow/claim/refund validation adapters, role-correct both-direction Zcash funding submission and unknown-ID discovery, and production Zebra claim/refund boundaries pass isolated suites. The context-owning LEZ SDK ports persist caller-owned IDs/windows and share one role-local journal. The fresh-client factory rereads a private capability without retaining it, and the actor context source generates fresh OS-random IDs with only protocol-required bounded windows. Reopened SQLite now derives the exact LEZ claim funding in both directions without primitive caller IDs; reverse direction additionally refuses until the durable Zcash second lock exists. The zeroizing production Zcash signer produces byte-identical canonical funding/claim/refund transactions and rejects foreign roles or keys. The agreement-committed exact-outpoint planner validates every candidate and stable Zebra identity before producing the only durable funding plan. A one-shot maker/taker CLI and private-config boundary now proves separate roles, runs, stores, journals, capabilities, keys, and config files without path aliasing. The local v0.2/Zebra corridor now completes both claim directions through independent role processes: run 14o finalized LEZ blocks 264/265/266 and Zcash heights 106/108; reverse 14c finalized LEZ blocks 641/642/643 and Zcash heights 113/115. In both runs Zcash funding had two confirmations before the LEZ reveal, the exact `:0` HTLC output was spent afterward, and both stores reached revision 4 `Completed`. Signed public LEZ identities, schema-v3 Zebra routes, exact official-public sidecar routing, and pre-persistence runtime checks pass local nonconnecting contract suites with the same binaries and validators. Current-schema runs `m2cert-schema3-forward-2d09997-20260714a` and `m2cert-schema3-reverse-2d09997-20260714a` repeat both role/effect orderings through actual local nodes. Canonical Docker-target runs `m2cert-canonical-forward-bb53daf-20260714a` and `m2cert-canonical-reverse-bb53daf-20260714a` then bind that same order to ProgramId `5cf8...29c1`, finalized LEZ blocks 2594/2595/2596 and 2605/2606/2607, Zcash heights 122/124 and 125/127, zero terminal LEZ custody, and four revision-4 `Completed` actor stores. | Partial: the private actual-node happy-claim and configuration-portability gates are GREEN in both directions, including direction-correct funding and LEZ-before-Zcash reveal order. Lower lanes retain ordered refund, restart, and fault evidence, but their composed actual-node repetition and the remaining post-lock/margin matrix are owner-gated hardening. The M2 local-functional PoC is certified under `m2-complete`; composed actual-node recovery and the post-lock/margin hardening matrix remain owner-gated, and public-testnet execution is deferred to production readiness under ADR 0023 |
| F5 | Risc0 LEZ escrow validates pair proof and supports claim/refund | Per-pair guest instruction tests plus standalone-sequencer initialise/claim/refund E2E | v0.1.2 source-correct semantics, checked deployment, real-key native plus two-definition token claim/refund lifecycles, recursive cost gates, and corrected external schema-v2 handoff pass. The provisional v0.2 lane canonically builds ELF `c85055f6...c9d2e`/ImageID and ProgramId `5cf8c5a4...329c1` through both direct digest-pinned Docker and Docker-backed methods embedding, compiles the generated typed client, executes recursive native and two-definition token claim/refund, proves child-transfer rollback, and tests an exact-once fail-closed deployer. That exact artifact was deployed in local finalized block 2582 before both independent actor directions completed against it. Its offline deployment-evidence handoff verifies a domain-separated owner-keyed HMAC before trusting dynamic facts and is runtime-tested for happy, no-clobber, eight authenticated mutations, wrong-key plus unauthenticated semantic/envelope chain-fact tampering, bounded/non-regular input, and exact key-file cases; it emits the exact chain/channel/genesis/program/transaction/inclusion identity without RPC. Five independently locked v0.2 graphs have graph-local cargo-deny CI audits; the official-wire sidecar has 78 tests covering exact native initialize/fund, deterministic maker/taker Vault Claim preparation, hardened exact-byte restart, ADR 0026's durable submission slice, typed local/exact-official outbound node routes, and authenticated finalized witnessed-funding and witnessed-claim observation. The pinned local v0.2 Bedrock/sequencer/indexer environment proves signed channel onboarding, finalized maker/taker Vault Claims, checked escrow deployment, and effect-bearing independent actor claims in both directions. Historical indexer audits bind the earlier six corridor transactions to finalized blocks 264/265/266 and 641/642/643. Canonical audits separately bind all six ProgramId `5cf8...29c1` transactions to finalized blocks 2594/2595/2596 and 2605/2606/2607, with terminal `Claimed` metadata and zero custody; the corresponding exact Zcash HTLC outputs are spent only after reveal. Composed actual-node refund/restart recovery remains owner-gated hardening. Public deployment and deployed-runtime CU evidence are deferred under ADR 0023. Logos-owned SPEL/Hickory disclosures do not block M2 certification under ADR 0018 but remain production-release blockers. |
| F6 | Atomic outcome: both claim or both refund | Six scenario tests and 512 generated transition sequences pass. M3 run `m3actor-20260716n` retained exact funding/agreement/signer ordering, both canonical locks before reveal, actor-owned claims through revision four, and unchanged effect counts after replay. Run `m3refund-20260716h` executed both ordered two-lock refunds. Run `m3firstlock-20260716h` proved both absent-maker first-lock refunds. Clean run `m3survivor-20260716c` proved that canonical reveal leaves revision 3 nonterminal with the opposite leg claimable, then a fresh maker can complete after taker disappearance and its own process restart; delayed taker catch-up and terminal replay add zero effects. This proves conditional protocol recovery, not one distributed commit across Core, LEZ, and SQLite. Live maker-lock admission at the cutoff boundary, reorg, process-kill, and overlapping concurrency remain | Partial |
| F7 | Native and custom LEZ tokens through ATAs | Parameterized guest and sequencer E2E for native token plus two independent token mints/accounts | Shared LEZ guest/runtime evidence is GREEN for native plus two independent custom definitions with real owner keys, conservation, and substitution negatives. The BTC witnessed escrow terms and actual-node path appear native-only; M3 must obtain an accepted shared-support interpretation or add and prove the pair-specific witnessed custom-token path |
| F8 | Pluggable local and Logos-module C-API price sources | Price-port contract suite; config/CLI mutation E2E; fake and real C-ABI adapter tests including stale/unavailable feeds | Planned M5 |
| F9 | Headless maker covers configuration, pricing, advertisement, execution, monitoring, and full CLI operation | UJ-007 actual CLI/daemon suite plus restart, history, manual claim/refund, pricing, and advertisement cases | Partial: authenticated create/status and durable alert status/list/ack survive process restart; execution/pricing/advertisement remain |

## Usability

| ID | Contract | Acceptance evidence | Status / milestone |
|---|---|---|---|
| U1 | Dedicated full-lifecycle SDK for each pair | Shared SDK contract suite instantiated for BTC/XMR/ZEC; public API doctests cover discovery through refund | Partial: pushed `ed5cd77` supplies the adapter-independent `lez-swap-sdk-core` lifecycle, discovery, negotiation, structured errors, explicit claim order, versioning, and bounded ordered exact-public-effect plans without actor/adapter/store/SQLite/CLI coupling. ZEC role-fixed discovery, negotiation, separate schema-v10 persistence, both lock effects/observations, agreement-directed claims to `Completed`, and ordered refunds to `Refunded` are integrated in both directions. The same ZEC public SDK boundary drives independent maker/taker processes through both private local actual-node happy paths. Signed public deployment activation and typed local/self-hosted/provider routes reuse that SDK and actor binary; their local nonconnecting contract suites pass. Protected claim/refund replay remains proven in lower lanes; composed actual-node refund/restart faults remain owner-gated. The BTC reference actor and lower typed components do not yet constitute the accepted full-lifecycle public BTC SDK with concrete activation/status/resume, SDK-owned escrow creation, pair errors, examples, and lifecycle documentation. Current actual-node scripts submit both locks outside the actor/SDK. |
| U2 | Long-running autonomous maker daemon plus systemd unit/install guide | Packaged daemon runs under hardened systemd in an isolated VM, restarts, advertises, prices, and executes without GUI | Partial daemon seam; packaging M5 |
| U3 | Maker CLI configures pairs/prices, controls daemon, queries history, and triggers claim/refund over IPC/RPC | Black-box CLI-to-daemon command matrix under owner/wrong-owner roles | Partial: create/status/auth/restart pass |
| U4 | Taker CLI covers discovery, initiation, monitoring, claim, and refund | Actual taker CLI drives each pair's happy and abandoned-counterparty journey | Planned M5 |
| U5 | Basecamp maker mini-app configures and monitors | Playwright actor E2E against the same daemon API; clean local build/load from documented repo | Planned M6 |
| U6 | Basecamp taker mini-app browses and executes swaps | Playwright actor E2E for each pair including refund and ZEC shield-after-swap guidance | Planned M6 |
| U7 | SPEL IDL for LEZ escrow program(s) | IDL generation/validation test against pinned compatible SPEL/LEZ versions | Generated IDL/client and signer roles pass; custody ABI is being regenerated around authenticated transfer and official ATAs |
| U8 | Bitcoin Core testnet setup guide, self-hosted and public | Fresh-machine documentation test reaches funded wallet and SDK connectivity for both routes | Partial M3: the private repository-owned happy and two-lock timeout/refund workflows are actual-node GREEN in both directions. The guide documents a fresh-ID `M3_ACTOR_POC_JOURNEY=refund` command, Core 31.1 Regtest and LEZ v0.2 local services, terminal/effect/replay/cleanup checks, deliberate deadline runtime, and local flakiness. Self-hosted/public Testnet4 routes, cold clean-host funded rehearsal, and public flakiness evidence remain |
| U9 | Monero stagenet and wallet-RPC setup guide, self-hosted and public | Fresh-machine documentation test reaches funded wallet and SDK connectivity for both routes | Planned M4 |
| U10 | Zebra/Zcash testnet transparent-wallet guide, self-hosted and public | Fresh-machine documentation test reaches funded transparent wallet and SDK connectivity for both routes | Deferred public-production evidence under ADR 0023: the guide selects self-host Zebra 6.0.0 and Tatum's API-key-authenticated Testnet Zebrad gateway, with schema-v3 configuration and local nonconnecting adapter tests for both routes. Optional Zallet funding and faucet/Discord fallback remain documented. No Zcash Foundation-operated public Zebra RPC was found; the project signer is locally wired, but no live public signer key, funded TAZ, or broadcast evidence exists. Live exact-method smoke and the clean-machine funded rehearsal remain. |

## Reliability

| ID | Contract | Acceptance evidence | Status / milestone |
|---|---|---|---|
| R1 | Taker locks first; maker waits for required confirmations | Core rejects early lock and uses independent immutable policies for each funded leg; reverse ZEC proves maker-funded ZEC below-threshold accumulation, promotion, 10→9-style depth regression suspension, exact depth recovery, removal pinning, conflict rejection, and preserved refunds. Both actual-node happy directions now show the direction-derived taker effect first and prohibit the LEZ reveal until the Zcash lock has two confirmations. Watcher tests still require stable affirmative evidence; runtime requires both role policies to match the immutable named profile and commits conflicting chain replacements without substituting the protocol-pinned ID. | Passing core/reconciliation boundaries and both private local composed happy directions; actual-process reorg/replacement and refund hardening remain owner-gated |
| R2 | After first submission, only local state and chain nodes are required | Core happy/refund paths have no peer handle. Runs `m3actor-20260716n`, `m3refund-20260716h`, and `m3firstlock-20260716h` reconstructed claims, ordered refunds, or the sole funded-leg refund from countersigned agreement, role-local journals, and Core/LEZ observations. In clean run `m3survivor-20260716c`, the taker disappeared after public reveal; fresh maker processes reconstructed revision 3 and the terminal follow-up from maker-only state plus canonical Core/LEZ evidence before delayed taker catch-up. No Delivery or Chat, public RPC, faucet, or public funds participated | Partial: actual-node happy, both-lock refund, absent-maker first-lock, and clean post-reveal on-chain-only continuation are proven. Outage behavior and process-kill evidence remain |
| R3 | Missing chain dependency does not disable other pairs | Dependency matrix starts daemon with each node absent/unhealthy and completes unaffected-pair swaps with clear CLI/GUI status | Planned M5 |
| R4 | Persisted state survives crash/restart without fund loss | Schema v9 retains the schema-v8 lock journal and adds protected claim material, encrypted exact claim intents, owner/observer transitions, and two-direction replay to `Completed` (`add5d98`). Coordinator snapshots retain only a SHA-256 claim marker; AAD binds agreement/role/step/revision/expected identity. The crash-safe v8→v9 migration rewrites exact legacy plaintext arrays, enables secure deletion, truncates WAL, and reopens the marker (`5ed04ec`). Existing lock rollback/corruption/unknown-submission coverage remains green. | Partial: claim happy-path restart and legacy scrub proven; claim-specific unknown submission, atomic rollback, wrong-key/AAD/ciphertext/future-version/orphan corruption, process-kill, and actual-node restart gates remain |
| R5 | Concurrent swaps have independent state, escrow, and deadlines | Two-store isolation test passes; acceptance still requires two swaps with independent inputs, agreements, stores, and journals to remain simultaneously in flight at overlapping phases. Sequential runs do not satisfy this gate; multi-pair concurrency and fault injection remain | Partial |
| R6 | Timelocks cover variance, congestion, and clock drift per chain | Typed pair-specific domains and ordering pass; LEZ seconds/milliseconds conversion is explicit, conservatively rounded, overflow-checked, and boundary-tested; named runtime profiles and public calibration remain | Partial: `m3refund-20260716h` executed both ordered recovery mappings, exact Bitcoin next-block CSV eligibility, and finalized LEZ timestamps with private-local 3.0-second slots. `m3firstlock-20260716h` additionally enforced the signed maker cutoff before two fresh admission reads and waited for the chain-specific sole-leg refund boundary in both directions. Margin validation and the refund-side live gate are GREEN. Commit `3d202f7` now also requires canonical Bitcoin containing-block median time or finalized LEZ containing-block time no later than the cutoff, with late presence quarantined as uncertain. SDK-owned same-action admission, public calibration, cutoff/race stress, congestion, and actual reorg margins remain. |
| R7 | Bitcoin refund construction is chosen and justified with failure analysis | ADR 0009 selects script-path CSV; Bitcoin Core tests cover exact boundary, key, reorg, current fees, and RBF/CPFP | Partial M3: the canonical agreement commits the exact CSV leaf, control block, destination, funding outpoint/value, fee, and direction-correct schedule. `m3refund-20260716h` exercised the two-lock Bitcoin refund in each direction. In `m3firstlock-20260716h`, `TakerSellsForeign` additionally executed the sole Bitcoin first-lock refund at exact signed height 246 with the countersigned three-item witness and txid/wtxid readback. Maker-lock admission, fee stress, bounded RBF/CPFP, and reorg remain |
| R8 | Delivery/Chat outage is retried/buffered/degraded and documented | Pre-lock outage/recovery matrix plus post-lock UJ-006 proves on-chain independence and user-visible degraded state | Planned M5 |

## Performance

| ID | Contract | Acceptance evidence | Status / milestone |
|---|---|---|---|
| P1 | Compute units documented for initialise, claim, and refund against a named LEZ testnet version | Reproducible benchmark records each operation/pair and fails CI thresholds for the pinned release | Partial: exact local v0.1.2 native/authenticated-transfer and token/ATA/Token cycles, segments, invariants, and CI budgets are recorded. v0.2 recursive behavior is executable, but deployed-runtime CU evidence against the named public testnet is deferred to production readiness under ADR 0023 |

## Supportability

| ID | Contract | Acceptance evidence | Status / milestone |
|---|---|---|---|
| S1 | LEZ escrow deployed/tested on testnet 0.2 | Version-pinned deployment manifest and public smoke-test transaction evidence | Partial local deployment GREEN: checked Docker v0.2 ELF `c85055f6...c9d2e`, ImageID/ProgramId `5cf8c5a4...329c1`, generated client, recursive native/token/rollback suites, and fail-closed deployment observation tests pass. Local deployment transaction `bd16808e...733f` is present in finalized block 2582, and both canonical corridor directions used that target. Retained evidence now includes channel, genesis, program, transaction, and containing-block identities; the authorized deployer authenticates those dynamic facts with a separate owner-only HMAC-SHA256 key, and the offline provisioner verifies the tag plus immutable compiled target before atomically emitting one exact runtime identity. The manifest deliberately retains pending public transaction/block fields: the proved local deployment is recorded separately and is not substituted for public evidence. No public deployment evidence exists to provision, and public deployment/smoke actors are not GREEN. Public deployment/smoke evidence is deferred under ADR 0023; Logos-owned upstream disclosures remain production-blocking under ADR 0018. |
| S2 | Standalone LEZ sequencer E2E is included in CI | Isolated CI job boots sequencer on ephemeral resources and runs guest lifecycle suites | Partial local GREEN: isolated CI builds/checks/deploys the v0.1.2 guest, runs native and two-definition token real-role happy/refund/negative lifecycles through canonical blocks, and gates deterministic recursive costs. The local v0.2 lane separately runs exact Bedrock, non-standalone sequencer, and indexer services; retained evidence proves signed channel onboarding, finalized Vault Claims, checked deployment, and both independent-actor claim directions. Those two corridor runs are private local execution evidence, not proof that the full v0.2 corridor is included in CI. Public deployment remains deferred under ADR 0023. |
| S3 | Default-branch CI is green | Required checks run format, strict Clippy, workspace tests/docs, traceability, dependencies, strict final-image vulnerability scanning, and isolated E2E | The exact local closure tree and pushed `main` pass the repository gates. This environment has SSH push access but no private Actions API credential, so the remote result is unavailable and not claimed; the tag records that limitation, and any later visible failure requires a corrective commit/tag |
| S4 | Every F/U/R/P hard requirement has a corresponding test | Traceability completeness guard plus test-report manifest rejects missing or skipped requirement IDs | Matrix guard passing; acceptance tests incomplete |
| S5 | Complete reference integration for every chain | UJ-001/UJ-002/UJ-004 role E2E and runnable reference package for BTC, XMR, and ZEC | Partial: ZEC and BTC each have private local happy-path references in both directions. BTC runs `m3actor-20260716n`, `m3refund-20260716h`, `m3firstlock-20260716h`, and clean `m3survivor-20260716c` cover happy, ordered two-lock refunds, both absent-maker first-lock refunds, and both post-reveal maker-survivor continuations with exact replay/cleanup. Live maker-lock admission at the cutoff boundary, two overlapping swaps, the full public BTC SDK surface, process-kill/reorg, and XMR M4 work remain |
| S6 | README covers deployment, addresses, prerequisites, and maker/taker CLI/mini-app use | Fresh-machine documentation tests plus link/command validation | Planned incrementally through M6 |
| S7 | Write-up covers protocols, escrow, atomicity, timelocks, assumptions, limitations | M1 design packet reviewed, then updated from executable evidence and audit findings | Living architecture now contains dedicated Mermaid sequences for both BTC directions, both transparent-ZEC directions, and supported LEZ-first XMR, with a conditional atomicity argument and limitations for each pair. Stable markers are CI-guarded; evidence status continues to update per milestone and receives final review in M7 |
| S8 | Every pair SDK public API has docs/errors/examples for full lifecycle | `cargo doc -D warnings`, doctests, and public-API coverage check for all three SDKs | Partial: ZEC has the mature role-fixed surface; BTC exposes cryptographic and agreement primitives but lacks the accepted public lifecycle facade, errors, full example, and SDK-owned lock creation. XMR is M4 |
| S9 | Logos doc packet for each pair SDK | Template validation and reviewer acceptance for BTC, XMR, and ZEC packets | Planned M7 |
| S10 | Separate maker-CLI and taker-CLI doc packets | Template validation plus clean operator/user journey rehearsals | Planned M7 |
| S11 | Figma designs or equivalent for both mini-apps | Signed-off clickable HTML prototypes cover maker and taker role journeys | Planned M6 |
| S12 | Third-party review of all on-chain programs/scripts and remediation | Agreed reputable reviewer report covers LEZ, BTC, ZEC, and other locking logic; findings tracked to closure | Planned M7 |
| S13 | Third-party review of protocol implementation and remediation | Reviewer report covers atomicity, taker-first, timelocks, adaptor/HTLC constructions, with findings tracked | Planned M7 |

## Demos

| ID | Contract | Acceptance evidence | Status / milestone |
|---|---|---|---|
| D1 | Happy, abandonment/refund, and concurrent-swap recording for each pair | Nine recordings generated from passing role E2E runs, with commit/testnet/version metadata | Secret-safe private local ZEC and BTC JSON evidence exists for both directions, and BTC also has two-lock refund JSON evidence, but a manifest is not a D1 recording. BTC happy, timeout/refund, and genuinely overlapping-concurrency recordings remain; ZEC recordings remain; public execution is deferred under ADR 0023 and all final recordings are regenerated at M7 |
