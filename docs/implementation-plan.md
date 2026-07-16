# Living implementation plan

Last updated: 2026-07-15

This file is the delivery control document. It must change whenever scope,
architecture, sequencing, risks, or acceptance evidence changes.

## Source of truth

1. The live [RFP-003 specification](https://github.com/logos-co/rfp/blob/master/RFPs/RFP-003-atomic-swaps.md).
2. Gateway's accepted replacement
   [proposal #112](https://github.com/logos-co/rfp/issues/112).
3. Actual pinned upstream source and executable behavior, where prose and code
   disagree.

Issue #61 is historical only. The accepted pairs are LEZ-BTC, LEZ-XMR, and
LEZ-ZEC transparent. ETH and shielded ZEC are out of scope.

## Product vision and completion bar

A maker operator runs a headless daemon and controls it through the maker CLI
or maker mini-app. A taker discovers an offer, negotiates, locks first, and
completes or refunds through the taker CLI or mini-app. Once the first lock is
submitted, either participant can recover using persisted local state plus LEZ
and foreign-chain nodes; Delivery and Chat may disappear permanently.

Final acceptance is role-based, not an internal API demonstration:

- `UJ-001`: maker operator and taker complete one swap for each pair;
- `UJ-002`: either party abandons and both funds are recovered after deadlines;
- `UJ-003`: daemon crashes after every durable transition and resumes safely;
- `UJ-004`: independent users run concurrent swaps without state leakage;
- `UJ-005`: one chain dependency is unavailable while other pairs remain usable;
- `UJ-006`: Delivery and Chat disappear after lock without blocking claim/refund;
- `UJ-007`: maker operator configures prices/pairs and performs operational
  actions through CLI-to-daemon RPC;
- `UJ-008`: maker and taker perform the same lifecycle through Basecamp mini-apps.

## Delivery architecture

The system is split at durable and security-relevant boundaries:

1. `swap-core`: explicit per-swap transition model and immutable safety
   parameters. It accepts only validated chain evidence after lock.
2. Per-pair SDKs: common lifecycle vocabulary with BTC, XMR, and ZEC protocol
   implementations. Cryptographic differences remain visible in typed evidence
   and errors.
3. Chain ports/adapters: LEZ sequencer, Bitcoin Core, `monerod` plus wallet RPC,
   and `zebrad` plus locally constructed Zcash transactions.
4. Coordinator/persistence: one transactionally persisted aggregate per swap,
   idempotent event application, restart recovery, and concurrent isolation.
5. Maker daemon: pricing, advertisements, incoming requests, execution,
   monitoring, and authenticated local JSON-RPC.
6. Maker/taker CLIs and Basecamp mini-apps: real actor boundaries used by
   black-box E2E tests.

Off-chain Delivery/Chat adapters participate only in discovery and negotiation.
They are not dependencies of post-lock transition commands.

## Progressive-JPEG delivery ladder

ADR 0027 replaces the previous rule that every new slice must begin RED. Each
milestone first produces the smallest reproducible, actor-realistic happy-path
PoC through its exact isolated local devnets and real signed chain effects. Only
after the repository owner ends that phase does hardening proceed through QA
with RED-GREEN-REFACTOR, chaos engineering, information security, and production
readiness. The owner explicitly controls every phase end and milestone switch;
work does not advance or tag itself.

1. Reconcile milestone scope, actual pinned source, versions, user roles, and
   observable terminal outcome.
2. Assemble a one-command, run-scoped local-devnet PoC with deterministic local
   funds, separated actors, real chain effects/finality, manual repetition, and
   exact no-clash cleanup.
3. On owner direction, QA the working path using RED-GREEN-REFACTOR across
   requirements, invariants, restart, boundaries, concurrency, and regressions.
4. On owner direction, inject and measure process, RPC, node, network, reorg,
   storage, and timing faults.
5. On owner direction, perform the systematic information-security pass while
   retaining continuous CI lint, vulnerability, license, source, secret, and
   image gates.
6. On owner direction, close observability, performance, runbooks, packaging,
   deployment, configuration portability, and release risks.

Existing regression tests and baseline build, provenance, isolation, secret-
safety, and chain-reality checks stay GREEN during the PoC. New PoC feature work
does not need a prewritten RED matrix. Defects and QA work after the PoC do.
Evidence accumulated under the older test-first sequence is retained as carried
evidence, not relabeled as completion of a new hardening phase.

Tests map to hard requirement IDs in `docs/requirements-traceability.md`. Custom
cryptography is prohibited: canonical libraries and published vectors are used,
with dependency license/advisory checks in CI.

The live [milestone delivery scorecard](milestone-metrics.md) records phase
state, reproducibility, user-flow, QA, chaos, security, and production-readiness
measurements without manufacturing percentage-complete estimates.

The living [manual reproduction guide](manual-user-flows.md) records exact
fresh-checkout prerequisites, isolated commands, actor boundaries, expected
evidence, and cleanup for every currently proven operator/chain flow. It must be
updated with the implementation whenever one of those surfaces changes and
must continue to distinguish local fixtures from composed actor and public
testnet evidence. It and the global README also maintain the external-resource
inventory: public/local RPCs, faucets, registries, release assets, mutable
security databases, pins/checksums, availability risks, and fallback policy.

## Current vertical slice

The table below is accumulated implementation and test evidence, much of it
created under the previous test-first strategy. ADR 0027 carries it forward for
later revalidation; it does not imply that the M2 QA, chaos, information-
security, or production-readiness phase has been entered or completed. M2 is
certified at the reproducible local-functional PoC boundary; its QA and later
hardening phases remain inactive, while the separately scoped M3 PoC is active.
The final column is a post-M2 backlog, not
an instruction to start every listed refactor now. Restart, refund, reorg,
concurrency, broad negative testing, and new RED-GREEN-REFACTOR work wait for
the repository owner to enter the applicable phase.

| Slice | RED evidence | GREEN evidence | Next phase/gate, not automatically active |
|---|---|---|---|
| Reference-actor composition prerequisites | The first process-composition audit found no production fresh-client factory, actor-owned request allocator, role-local operation journal, checked Zebra composite, hardened private config, or independent actor boundary | Those prerequisites are GREEN and `activate`/`drive` are live against fresh role bridges and direct Zebra. Run 14o completed `TakerSellsLez` in 25.370 seconds across 39 rounds/78 actor events, and run 14c completed `TakerSellsForeign` in 26.960 seconds across 50 rounds/100 actor events. Schema v3 now composes typed deterministic-local, self-hosted-cookie, and exact Tatum Testnet Zebra routes; activation checks signed runtime/Zebra identities before persistence. The runner live-proved no round cap, 0.10-second polling, fail-closed millisecond timing, KILL-bounded calls, direction-derived effect ownership, exact endpoint-tuple serialization, and the Zcash-fund -> LEZ-reveal -> Zcash-follow-up order | The canonical evidence is synchronized and the exact local repository gate is GREEN. New TDD and broader hardening begin only after the owner enters QA |
| Taker-first happy path for BTC/XMR/ZEC | 2026-07-11 unresolved protocol API imports | 2026-07-11 `cargo test --workspace --all-targets`, 3 passed | Persist every fact and move from direct core calls to role harness |
| Ordered timeout refund | Same acceptance test RED; later primary-source reconciliation exposed that generic maker/taker order contradicted ZEC's fixed chain order | BTC uses maker-funded then taker-funded recovery; ZEC uses LEZ then ZEC recovery in both directions; XMR remains event-gated | Exercise exact pair boundaries and fee/reorg stress on real nodes |
| On-chain-only completion | Core API accepts only `ChainProof`/`ClaimEvidence` after lock | Happy path reaches `Completed` without a peer/transport handle | Prove through CLI/daemon black-box test with Delivery/Chat stopped |
| Restart recovery and user isolation | 2026-07-11 unresolved `SqliteSwapStore` and later missing `claim_evidence`; the schema-v9 RED then exposed plaintext legacy evidence and missing claim journals; the schema-v10 RED exposed missing refund recovery | Schema v10 closes/reopens two independent role stores through both claim directions to `Completed` and both refund directions to `Refunded`. Core aggregate state retains only a SHA-256 evidence marker; preimages and exact submissions use context-bound XChaCha20-Poly1305 envelopes. Commit `5ed04ec` transactionally migrates and scrubs legacy plaintext from SQLite/WAL; `340bf10` proves claim corruption/rollback/replay hardening; `845ff89` adds atomic refund intent/transition/revision commits, exact conflict, forced rollback, corruption, and restart replay. | Replace deterministic LEZ observation/refund ports with isolated actual-node adapters; retain broader process-kill coverage for the M5 coordinator and add rotation-aware production key provisioning |
| Maker abandonment/refund observation order | 2026-07-11 missing `TakerLegRefunded` and no direct taker recovery | 2026-07-11 taker-only refund and foreign-first observation pass | Add model tests over all legal event orderings and chain reorgs |
| At-least-once chain observation replay | 2026-07-11 repeated confirmed lock failed with `InvalidPhase` | 2026-07-11 identical lock/claim events are idempotent; conflicting IDs/evidence are rejected | Extend to persisted outbox/event sequence numbers and refund transaction proofs |
| Generated transition sequences | 2026-07-11 property oracles exposed both confirmation growth and regression cases and were corrected | 512 arbitrary sequences include confirmation regression and explicit removal/replacement; retained minimized reorg seed preserves the discovered case | Add pair-specific recovery triggers and compare against pair reference models |
| Maker operator process boundary | 2026-07-11 acceptance test could not resolve daemon/CLI executables | Actual CLI authenticates through HTTP metadata to actual daemon, creates a swap, daemon is killed/restarted on a new ephemeral port, persisted status remains visible | Move to owner-restricted Unix socket/credential file; add durable request IDs/audit outbox and price configuration |
| Bidirectional role ordering | 2026-07-11 reverse-direction test could not resolve direction or role-neutral transitions; CLI rejected `--direction` | BTC/ZEC both directions and XMR's LEZ-first direction preserve taker-first funding; claim order is now explicit per construction instead of fixed to maker-first | Run every supported real-chain role matrix and retain explicit XMR-first rejection |
| Taker-lock reorg/replacement | 2026-07-11 missing durable reorg phase/removal event; property oracle assumed confirmations only rise | Pre-maker regression/removal revokes permission and permits explicit replacement; post-maker removal pins the committed ID, suspends claims, and preserves refunds; generated model covers events | Add pair-specific reorg depth policies and real-node replacement cases |
| Typed recovery conditions | 2026-07-11 tests could not resolve chain/basis/safety schedule types | BTC/ZEC coordinator, persistence, RPC, and CLI use typed positions; wrong domains and insufficient conservative margins are rejected | Replace the prototype XMR deadline with the accepted canonical-LEZ-refund event trigger |
| Architecture diagrams | 2026-07-11 completeness guard failed on the first ADR without Mermaid; a 2026-07-12 audit then found that the renderer covered architecture/M1 blocks but omitted the manual-flow diagram | All 95 tracked Mermaid blocks pass the conservative GitHub-host policy and render through the exact Mermaid CLI 11.16.0 repository harness. The M3 closure pass also added a regression for the reserved sequence identifier that had escaped the conservative checker | Keep static completeness, host-policy, and exact-render gates synchronized with future diagram changes |
| XMR funding-direction capability | 2026-07-11 source review found COMIT ships scriptable-chain-first only; test lacked `UnsupportedDirection` | Core schedule and actual CLI/daemon reject XMR-first; LEZ-first XMR remains supported and documented in the per-leg flow | Validate exact DLEQ/key-share recovery transcript against vectors and third-party review in M4 |
| XMR event-gated recovery | 2026-07-11 acceptance test could not resolve `RecoverySchedule`, event evidence, or recovery phases; prototype accepted a fake Monero deadline | Core/RPC/CLI use tagged deadline vs canonical-event terms; wrong-chain/low-confirmation evidence is rejected, confirmation regression revokes availability, restart preserves each phase, and real operator CLI creates LEZ-first XMR without maker-deadline flags | Replace the generic 32-byte recovery proof with exact COMIT DLEQ/key-share and Monero transaction evidence in M4 |
| Pinned LEZ execution semantics | Source inspection alone could not prove the mempool/block split or accepted transaction-byte preservation; an initial filtered native command falsely ran zero tests | A clean pinned checkout passes 14 validity cases, the full BIP-340 vector test, and exactly one run each of the repository-owned admission/block reproducer and upstream transaction-equality test | Keep the pinned lane required and use the scheduled current-`dev` lane only as forward-compatibility drift detection |
| RFP F4 ZEC chain ordering | 2026-07-11 M2 source reconciliation found M1's generic role-relative claim/refund prose allowed ZEC-before-LEZ in `TakerSellsLez` | RED required typed participants and chain-ordered bounds; GREEN adds 2 regressions and 23-test workspace pass: LEZ always reveals/refunds before later ZEC in both directions | Reprove all M1 gates, tag corrective commit `m1-complete.1`, then keep these vectors as M2 entry tests |
| CI security and quality gates | 2026-07-11 workflow audit found the advisory scope implicit and a malformed `rzup` install command in the scheduled LEZ lane | CI hard-fails advisories, bans, licenses, sources, Rust format/clippy/test/docs, ShellCheck, traceability, Mermaid, Docker isolation, and SHA-pinned Trivy high/critical scanning of the Zebra image | The exact local equivalents are GREEN. When private Actions status is observable, require every GitHub job on the exact pushed closure commit before tagging. Without API read credentials, record the unavailable remote result in the tag instead of inferring GREEN; any later visible failure requires a corrective commit and tag |
| ZEC full-lifecycle SDK boundary | 2026-07-12 `sdk_lifecycle` could not import discovery, negotiation, concrete agreement, active-swap, recovery-store, or secret types | Independent role-fixed SDKs receive bounded untrusted wire, validate the same dual-signed agreement, persist before activation, reject adversarial resume, expose no raw transport/store handles, drive lock, claim, and fixed LEZ-then-Zcash refund paths in both directions, and replay exact owner/observer transitions after restart. The package has 131 passing checks including one doctest plus the intentional ignored Zebra gate. Refund records are primitive, versioned, deny unknown fields, and reconstruct trusted domain state only after agreement/coordinator/revision revalidation. | Persist the refund contract through the SQLite role journals, then replace deterministic ports with actual-node adapters and independent reference-actor processes |
| Crash-safe ZEC lock, claim, and refund intents | Initial active chain capabilities were inert; a naive RPC-success transition would lose unknown outcomes and combined LEZ initialize/fund into one unsafe effect | Fresh gating, noneligible zero-effect, intervening canonical revisions, stale instances, projection failure/unknown success, and SQLite lock/claim rollback pass. Two independent role-fixed SDK instances with separate stores complete both lock directions and restart at `BothLegsLocked`; claim paths continue to `Completed`. Refund now observes before every rebroadcast, retains exact signed owner bytes, makes unknown submission explicit, requires fresh funding/deadline evidence, forbids observer signing, and reaches `Refunded` in both directions. Schema v10 atomically retains/deletes owner intent, inserts owner/observer transition, and advances one revision; exact replay, changed-payload conflict, forced rollback, reopen, future-version, and unknown-field cases pass. Main LEZ/Zebra validation adapters and the official native-refund sidecar are GREEN. The context-owning LEZ SDK bridge persists caller-owned request IDs/windows by run, role, swap, and logical operation; opens a fresh one-use client per attempt; restores exact ambiguous prepare/refund contexts after SQLite reopen; and preserves initialize-before-fund ordering through the production recovery store. Its 46 adapter checks, including secret-redaction regression coverage, strict Clippy, and rustdoc pass. | Compose the context-owning bridge through independent actor processes and the checked external-node handoff. |
| SQLite SDK-recovery schema | Schema v4 had no role-local accepted agreement, open/closed effect intent, separate active revision, or exact transition slot for the concrete SDK contract | Schema v9 retains the schema-v8 lock journal and adds encrypted claim material, protected exact claim intents, mandatory owner-intent transitions, observation-only counterparty transitions, atomic revision replay, and two-direction close/reopen to `Completed` (`add5d98`). Aggregate `ClaimEvidence` persists only a SHA-256 marker; payload AAD binds agreement, role, claim step, staged revision, and expected submission identity. Commit `5ed04ec` transactionally replaces legacy plaintext evidence with the marker, enables secure deletion, truncates the WAL, and scans for remnants. Commit `340bf10` proves exact observe-before-rebroadcast after an unknown outcome, stale replay, coupled rollback, wrong keys, corrupted/future protected payloads, and malformed unified journals. | Build the actual-node claim adapters and rotation-aware production keyring. Broader operating-system process-kill coverage remains M5. |
| Composed actual-node LEZ/Zebra claim corridor | A direct single-graph composition is impossible because the Zcash and official LEZ cryptographic pins conflict; ADR 0022 therefore requires the separately locked sidecar process | Run `m2poc-corridor-fresh-20260714o` completed `TakerSellsLez`: independent actors reached revision 4 `Completed`; LEZ init/fund/claim finalized in blocks 264/265/266 and the two-confirmation Zebra HTLC was claimed at height 108. Run `m2poc-corridor-reverse-fresh-20260714c` completed `TakerSellsForeign`: LEZ init/fund/claim finalized in blocks 641/642/643 and the Zebra funding at height 113 received two confirmations before its claim at height 115. Both terminal proofs have zero LEZ custody and both role stores at revision 4. Secret-safe evidence is checked in at `docs/evidence/m2-taker-sells-lez-corridor-20260714.json` and `docs/evidence/m2-taker-sells-foreign-corridor-20260714.json` | The two-direction PoC execution, portability/documentation, and exact local repository certification gates are GREEN; later hardening waits for an owner transition |
| Zcash public-testnet route and funding research | M2 required self-hosted/public node, wallet, faucet, privacy, and flakiness guidance but no supported route was selected | Primary sources select self-host Zebra 6.0.0 with loopback cookie RPC and Tatum's documented API-key-authenticated Testnet Zebrad gateway as the public-provider route. Schema v3 now restricts those routes by network and auth kind; the adapter supplies bounded sensitive Basic headers over loopback HTTP or `x-api-key` only to the exact allowlisted Tatum Testnet HTTPS origin, and 70 adapter plus 30 actor tests pass without connecting publicly. Optional Zallet alpha.4 funding and faucet/Discord fallback remain documented. No Zcash Foundation-operated public Zebra RPC was found. The project HTLC signer is locally wired; no live public key, funded TAZ, or broadcast evidence exists. | The cross-document audit and exact local repository gates are GREEN. Clean-host public rehearsal, TAZ, rate-limit behavior, and live evidence are deferred under ADR 0023; U10 remains incomplete |
| LEZ v0.2 public-runtime security route | Fresh 2026-07-12 audit found the official v0.2.0 runtime graph still carries Hickory 0.25 and upstream explicitly ignores RUSTSEC-2026-0118/0119; SPEL PR #238 remains open and unreviewed despite green CI | Exact LEZ v0.2.0 `a58fbce...` plus SPEL PR head `df17acd...` now build the checked guest/generated client and recursive native/two-definition/rollback suite. The exact-once fixed-URL official-RPC deployer binds channel, genesis, built-ins, ELF, ImageID, ProgramId, transaction bytes/hash, and containing block. It domain-separated HMAC-SHA256 authenticates retained dynamic facts with a separate zeroized owner-only 32-byte key. Its offline `provision-identity` path verifies that tag before revalidating the immutable trusted target and bounded retained evidence, records the raw envelope SHA-256, and atomically creates a no-clobber exact runtime identity in a non-shared-writable directory; six native-safe boundary tests cover happy output, no-clobber, eight authenticated mutations, wrong-key/unauthenticated/envelope chain-fact tampering, bounded/non-regular input, and exact owner-only key files without public I/O. Its unavoidable Logos common/libp2p/Hickory path is constrained by four graph-local cargo-deny policies and feature/reachability tests rather than hidden or falsely excluded. | Deploy and exercise public actor calls, feed authenticated retained evidence through the offline handoff, retain ambiguity/no-resubmit evidence, and keep production readiness fail-closed until SPEL is reviewed/merged and Hickory is removed or explicitly accepted. Before third-party-verifiability claims, add separate-UID credential isolation, key rotation/retention, and a pinned public-key signature or anchored chain proof. Per ADR 0018, the exact Logos-owned items are disclosed production blockers but do not stop private M2 certification once repository-controlled local-v0.2 and public-capability contract evidence is green; live public execution is deferred under ADR 0023. |
| Full local LEZ v0.2 service stack | Initial audit found wildcard binds, mutable packaging, implicit native downloads, incorrect topology, and weak health-only readiness | Exact source/binary/image inputs now run on a unique no-masquerade bridge. Run `v02-actors-finalized-20260713b` proves signed key-derived channel onboarding, finalized block 2, indexer ID/hash equality, sequencer Borsh identity, distinct maker/taker owner/Vault pre-Claim states at that exact finalized block, and fail-closed exact cleanup. The checked deployment and both reference-actor directions now cross the same Bedrock, non-standalone sequencer, and indexer topology. Compose is configuration validation only; direct Docker lifecycle is exact-ID scoped | The local two-direction PoC stack, dormant configuration-only node routes, and closure revalidation are GREEN; restart recovery is post-M2 hardening |
| Official-wire LEZ v0.2 sidecar | The final ADR-0023 corridor had only the incompatible v0.1.2/NSSA sidecar; direct `cargo --offline` still let upstream `rust-rapidsnark` attempt an implicit native-library download | The exact role bridges completed direction-derived initialize/fund/revealing-claim/observation/submit sequences in runs 14o and reverse14c. Run 14o retried one payload-free `moving_tip`; reverse14c required no retry. Exact absence remains fixed by `0861117`. The actor-facing listener remains capability-authenticated loopback HTTP. A typed outbound profile now admits either explicit local sequencer/indexer URLs or only the exact `https://testnet.lez.logos.co/` origin for both clients; mixed, generic, credentialed, path, query, and fragment routes fail locally. The 44-test sidecar suite and strict Clippy pass without a public call. | Retain separate indexer finality and disclose that live `getLastFinalizedBlockId` support on the official endpoint is unverified upstream. Existing gates remain enforced; expanded RED-GREEN-REFACTOR begins only after owner transition |
| LEZ v0.2 submission and finality contract | Exact source audit found that `sendTransaction` proves only stateless admission, the volatile mempool exposes no status, stateful rejection has no receipt, and neither transaction query returns a containing block position | Run 14o retained exact public tx/block membership and independent indexer finality for initialize/fund/claim in blocks 264/265/266; reverse14c retained the same proof in blocks 641/642/643. Both include terminal account state, and neither run duplicated a submission. This strengthens the corridor evidence without claiming that bridge readiness itself is finality. Dormant signed public activation and exact official-origin routing are now locally contract-tested. | Broaden finality and ambiguity cases only after owner transition; live official-endpoint execution and method availability remain deferred production evidence |
| Concrete LEZ/ZEC agreement | Initial generic LEZ terms did not bind exact chain identities, custody, transaction destinations/fees, funding inputs, wire bounds, or both actor signatures | Eighteen focused cross-binding tests prove bounded exact decoding, dual low-S signatures, both directions, exact signed public deployment identity, actual LEZ/ZEC deadlines, exact PDA/ATA derivation, accepted-at resume, redacted diagnostics, and agreement-derived funding/claim/refund requests; all 35 lifecycle tests pass. Actor activation independently validates the exact signed LEZ runtime plus Zebra network/branch before any store mutation. A focused RED then found the shared bridge validator still excluded public v0.2; GREEN admits only `PublicTestnetV0_2 + LeeV0_2_0`, retains cross-generation rejection, and the full 63-test bridge suite passes. The dependency-light derivation source still matches pinned upstream v0.2 `lee_core`, SPEL multi-seed, and ATA-core types. | Retain exact runtime recomputation in effect adapters and provision public channel/genesis/program identities only from verified deployment evidence; live public execution remains deferred |
| Whole-system architecture and actor flows | 2026-07-11 ADR-local diagrams passed the old gate but did not provide one composed system, actor, trust-boundary, or lifecycle view | Canonical living architecture diagrams independent maker/taker actors, runtime components, node boundaries, happy/refund/restart flows, and current-versus-planned status; CI requires these views | Keep the status and flows synchronized whenever a slice crosses a new real process or chain boundary |
| Exact BIP-199 contract envelope | 2026-07-11 redeem API was absent; subsequent REDs exposed missing P2SH/scriptSig, refund policy, fetched-prevout validation, V5 epochs, UTXO ownership, dust/change, and Zebra acceptance | Exact script and V5 bytes/txids pass; deterministic selection and actor-only change use canonical builders; pinned Zebra confirms funding/claim/refund and rejects mutated funding/claim signatures plus pre-CLTV refund | Add replacement/reorg/refund-margin stress and composed LEZ↔ZEC roles |
| Arbitrary P2SH transaction signing | Stable source review shows ordinary and PCZT signers/finalizers recognize P2PKH/P2PK/multisig but not BIP-199; transparent builder also defaults every input to final sequence | GREEN uses canonical `TxOut`, `Bundle`, ZIP-244, deterministic secp256k1, and `TransactionData`; exact HTLC scriptSig is the only adapter-owned encoding, interpreter mutation tests pass, and Zebra is the final authority | Retain vectors and extend the node lane to replacement/reorg cases |
| Pinned SPEL/LEZ compatibility | Initial RED had no real fixture; generated client exposed missing signers; a later 11-test custody slice appeared green, but direct v0.1.2 source review invalidated its escrow-owned native users and direct token-holding PDA as real-user/RFP evidence | Native custody now composes canonical `authenticated_transfer`; custom custody is official `ATA(metadata, definition)` for two definitions; generated clients use real owner signers. Exact Risc0 3.0.5 builds both checked guests. The corrected v0.1.2 external runner passes canonical deployment, native/two-definition actors, and recursive costs; the v0.2 lane passes guest/client identity, recursive native/token behavior, rollback, exact-once deployment tests, both independent local actor directions, and four graph audits. | The exact local v0.2 and graph gates are revalidated for the M2 closure; keep the deterministic v0.1.2 handoff as a lower compatibility lane and defer public deployment evidence under ADR 0023 |
| Deployable LEZ guest supply chain | Host-only fixture could not produce an ELF; first real build selected Rust-1.89-only enum crates in the Rust-1.88 builder; first runtime RED admitted deployment but stopped at genesis because `RISC0_DEV_MODE` does not provide `r0vm`; audit then rejected vulnerable `ruint 1.17.0`. The first external-process exact run preserved another RED: `getProgramIds` is only a static built-in map, so it cannot prove custom deployment. | Exact `cargo-risczero`/`r0vm 3.0.5`, digest-pinned builders, checked ELF SHA-256/ImageID/ProgramId, port-zero/fresh-private-state service, clock readiness block, RPC deployment, canonical block inclusion, native/token actor lifecycles, recursive cost evidence, and graph-local dependency audits are reproducible. The corrected v0.1.2 full runner proves exact deployment transaction/containing-block and built-in-only program-map binding. The v0.2 supply chain now canonically builds ELF `c850...c9d2e` and ImageID/ProgramId `5cf8...29c1` through both direct digest-pinned Docker and Docker-backed methods embedding, compiles the generated client, proves recursive atomicity, deploys exactly once to the retained local v0.2 chain, authenticates evidence provisioning, and completes both local actor directions against that deployment. | Keep artifact identity, private readiness, actor-key diagnostics, evidence authentication, and advisory reachability fail-closed; the supply-chain and local actor gates are revalidated for the M2 closure; defer public deployment/CU evidence under ADR 0023 |
| Isolated Zebra consensus E2E | First runner RED found RPC cookie assumptions; the initial capability hardening found an unwritable cache; strict Trivy then rejected both official 5.1.1 and 5.2.0 runtimes with 40 HIGH/2 CRITICAL findings | Two disconnected copies of the official 5.2.0 binary now run in immutable distroless nonroot images that scan at 0 HIGH/CRITICAL; read-only/capability-free NU6.2 Regtest on separate ephemeral ports proves funding, claim, refund, rejection, concurrency, and a three-block detach onto a conflicting four-block branch; exact project cleanup leaves unrelated Docker workloads untouched. Both local corridor directions and dormant route contracts now cross the typed chain boundary. | Fresh isolated Zebra restart/fork-removal and real-key claim/refund E2E plus the fresh-db fail-hard final-image scan are GREEN for closure. Composed refund-margin/reorg hardening waits for owner transition; live public smoke is deferred under ADR 0023 |

## Milestone 1 plan: three weeks

### Week 1 — truth, invariants, and executable skeleton

- [x] Reconcile live RFP with accepted replacement issue #112.
- [x] Establish clean Rust workspace, formatting/lint policy, contribution rules,
  dual-license intent, and test-first acceptance harness.
- [x] Add and pass dependency advisory, license, ban, and source policy checks
  with `cargo-deny 0.19.9`.
- [x] Source-trace current LEZ `dev` at `cac4921581b37e85ae25e940f3a62412cd22308e`.
- [x] Confirm validity interval shape and where block validation occurs in source.
- [x] Complete first protocol RED-GREEN cycle for taker-first and refund ordering.
- [x] Complete second RED-GREEN cycle for basic SQLite restart recovery and
  concurrent swap isolation.
- [x] Make lock and claim observations idempotent under at-least-once delivery
  while rejecting conflicting chain evidence.
- [x] Add pinned upstream LEZ reproducer tests, including mempool-vs-block timing
  and signature-byte preservation.
- [x] Complete the hard-requirement traceability matrix and enforce ID coverage in CI.

### Week 2 — protocol and threat design

- [x] Publish per-leg message/state diagrams and atomicity arguments, including
  the COMIT-derived LEZ-first-only XMR capability.
- [x] Specify the LEZ escrow account model, native/custom token flows, claim and
  refund instructions, and SPEL IDL.
- [x] Complete threat model: adaptor extraction, signature byte stability,
  timelocks/reorgs, XMR key-share recovery, ZEC transparent visibility, local RPC,
  persistence, and concurrency.
- [x] Publish common SDK lifecycle traits plus typed pair-specific evidence/errors.
- [x] Add generated property tests for transition legality, conflicts, and
  absorbing terminal states.
- [x] Extend the model with reorg/replacement events and typed per-chain deadlines.
- [x] Select and justify the Bitcoin refund construction (Taproot script-path CSV)
  with its M3 failure/fee/reorg validation matrix.

### Week 3 — integration contracts and review packet

- [x] Decide persistence direction: SQLite through `rusqlite`, behind a repository
  port and a single writer actor; validate with crash tests before freezing.
- [x] Decide Zcash direction: `zebrad` node, local `librustzcash` transaction
  construction, Zallet only where wallet RPC capabilities fit.
- [x] Specify maker daemon local JSON-RPC, authentication, systemd fallback, and
  Logos Core daemon-mode adapter contract.
- [x] Fix public-testnet per-pair confirmation/recovery profiles with reorg,
  latency, and reaction margins; keep mainnet disabled pending calibration/audit.
- [x] Review all ADRs, test evidence, open questions, and Milestone 2 entry gates.

Milestone 1's original evidence commit is retained at `m1-complete`. M2 source
reconciliation found and corrected an RFP F4 chain-order overgeneralization. The
correction is marked `m1-complete.1` only after the full M1 gate run succeeds;
the original tag is never moved.

## Milestone completion tags

Each milestone is marked by one annotated Git tag, `m1-complete` through
`m7-complete`, on the exact commit whose living plan, review packet, and required
test evidence prove every exit gate. Tags are never created for partial or
aspirational states. A later fix does not move an existing tag; it receives a
new normal commit and, if the milestone evidence was invalidated, a documented
corrective tag such as `m1-complete.1` only after the full gate is rerun.

Tags are created only after verification. The repository owner authorized
frequent pushes on 2026-07-11; proven commits and milestone tags are published
without weakening the tag evidence rule.

## Milestone sequence and entry gates

| Milestone | Outcome | Entry gate |
|---|---|---|
| M1 | Designs, threat model, LEZ verification, SDK surface, persistence/node decisions | Accepted proposal and current source reconciled |
| M2 | ZEC transparent BIP-199/LEZ HTLC end to end | M1 HTLC/timelock design and Zebra test harness approved |
| M3 | BTC Schnorr adaptor/Taproot end to end | Owner transition; ADR 0009 refund accepted; Gateway erratum GW-M3-001 tracked without claiming an accepted substitute |
| M4 | XMR Ed25519 adaptor/cross-curve DLEQ end to end | COMIT vectors and key-share recovery design approved |
| M5 | Persistent coordinator, daemon, CLIs, price plugins, fuzzing | At least one real pair adapter stable; RPC/persistence ADRs accepted |
| M6 | Maker/taker Basecamp mini-apps | Daemon RPC stable and role E2E reusable |
| M7 | Third-party reviews, remediation, readiness packet | All hard-requirement tests and demos green |

These dependencies describe safe planning order only. The repository owner
explicitly chooses when a phase ends and when work switches milestones. A switch
does not imply completion or authorize a completion tag. When directed, M2 and
M3 may overlap after M1; M4 follows M3 for cryptography-lead capacity, M5/M6 may
overlap the tail of M4, and M7 follows all implementation milestones.

## Milestone 2 plan: transparent ZEC end to end

Certified boundary: **reproducible local-functional PoC** under
`m2-complete`. The owner has not entered M2 QA, chaos, information security, or
production readiness; the separately scoped M3 PoC is active. Existing hardening
evidence is carried forward, but no later phase begins without an explicit owner
transition.

The first progressive pass completes one real local LEZ/ZEC happy direction.
Before the M2 PoC gate is offered for owner review, the same run-scoped system
must complete both accepted directions with independent maker/taker processes,
actual local-v0.2 Vault Claims, checked escrow deployment/funding, exact Zebra
Regtest HTLC funding and spend, finalized state/balance evidence, a one-command
runner, manual repetition steps, and exact cleanup. Restart, refund, reorg,
concurrency, corruption, broad negative matrices, and public-route readiness are
subsequent hardening layers unless the owner revises the phase boundary.

### Accepted proposal authority and delivery boundary

The contractual authority is accepted Gateway proposal
[issue #112](https://github.com/logos-co/rfp/issues/112), not superseded
issue #61. The authority was re-fetched on 2026-07-12: issue body SHA-256
`49356263a762307abc0f8dd2863ac5af8fe13d9b17b674f242d025de655f1c87`;
canonical comments JSON SHA-256
`3c596392f7356a29a2d512ffa92ebb9153cab7b97e38848b61e79e4764240980`. The
local `proposal.gateway.md` snapshot is archive-only because it excludes ZEC and
calls ETH M2.

The six accepted proposal outputs are the eventual RFP delivery boundary. ADRs
0023 and 0027 define the narrower private local-functional M2 certification
boundary:

- [ ] Deploy LEZ escrow v1 on testnet 0.2 with the ZEC SHA-256 HTLC and
  validity-window refund.
- [ ] Run BIP-199 transparent transaction construction integration tests against
  Zcash testnet in CI, with credentials, rate limits, retries, and external
  flakiness made explicit.
- [ ] Publish a documented LEZ/ZEC SDK lifecycle covering offer discovery,
  negotiation, escrow creation, claim, and refund. Transaction construction and
  observation primitives alone do not satisfy this item.
- [ ] Publish step-by-step self-hosted and public Zcash-node routes, including
  configuration, transparent wallet creation, and obtaining testnet funds.
- [x] Document transparent-pool visibility and the shield-after-swap user journey.
- [ ] Record happy, refund/timeout, and concurrent-swap demos from passing
  testnet actor suites.

Owner-approved stealth certification policy (ADRs 0023 and 0027): the unchecked
public deployment, public Zcash-testnet, full discovery/negotiation SDK
publication, public funding-guide rehearsal, and public recording outputs above
remain honest proposal #112 delivery gaps, but they do not block the private
local-functional M2 tag. M2 instead requires a fully functional private corridor using one
pinned public-compatible LEZ v0.2 local devnet and one pinned local Zcash
Regtest devnet using the same canonical builders/validators as the future
public route; signed configuration selects each environment's network/branch.
LEZ certification targets the full Bedrock node, indexer, and non-standalone
sequencer. Their exact source/publication/finality topology and locked service
output hashes are now attested. Run `m2poc-vertical-20260714a` additionally
kept the three services live while both owner-authorized Vault Claims finalized,
the checked escrow deployed, and separate maker/taker PoC processes completed a
native initialize/fund/claim lifecycle with exact finalized transaction and
balance evidence. Reference actors now complete both terminal corridors. In
`m2poc-corridor-fresh-20260714o`, both actors are revision 4 `Completed`, LEZ
blocks 264/265/266 are finalized with terminal balances, and the Zcash HTLC
funding/claim are canonical at heights 106/108. In
`m2poc-corridor-reverse-fresh-20260714c`, both actors are likewise revision 4
`Completed`, LEZ blocks 641/642/643 are finalized, and the reverse Zcash HTLC
was funded at height 113, received two confirmations, and was claimed at height
115. One bounded payload-free `moving_tip` retry succeeded in 14o; reverse14c
needed none. Reverse attempts 14a and 14b made distinct effects and are
permanently nonreusable; they exposed a forward-only LEZ observation validator,
now corrected by a focused both-direction regression. Independent clean-host
rebuild reproducibility remains unmeasured. The standalone mock publisher and
existing v0.1.2 lane remain lower-level coverage only.
Independent maker and taker processes must have separate configurations, keys,
funds, stores, journals, sidecars, and process lifecycles. Both happy directions
must cross the actual local nodes; contract doubles do not certify this PoC
boundary. Restart, refund, reorg, concurrency, and broad fault cases follow only
after the owner transitions to hardening. Evidence and recordings stay private
and no public endpoint, faucet, deployment address, or transaction identifier
is required.

Portability remains an M2 gate: local and future public runs must use the same actor
binaries, SDK state machine, ports, builders, and validators. Moving public may
change only signed configuration/provisioning--endpoints/authentication,
chain/genesis/channel/branch identities, confirmation profiles, signer/funding
material, and the public LEZ escrow deployment/program ID. A devnet-only code
path, fake adapter, alternate transaction format, or environment-selected build
does not pass. This gate is locally GREEN: the same actor binary accepts signed
public LEZ deployment identities, validates runtime and Zebra identities before
persistence, keeps the actor-to-role-sidecar boundary on
capability-authenticated loopback, and selects typed
local/self-hosted/Tatum Zebra plus local/exact-official LEZ outbound routes
through configuration. Contract tests construct those routes without public I/O
and reject generic or mixed transports. Public
evidence remains a production-readiness item and the M2
annotated tag must disclose this accepted-scope deviation.

The additional two-direction, daemon/CLI, Delivery/Chat-loss, deep-reorg,
immutable-binding, and alert-outbox gates below are project-strengthened safety
evidence. Accepted issue #112 assigns the complete daemon/CLI/coordinator product
to M5; those stronger checks do not replace or silently redefine the six M2
outputs above.

### Contract and compatibility baseline

- [x] Reconcile RFP-003 F4 and accepted issue #112 against the actual upstream
  implementations; retain fixed LEZ-before-ZEC claim/refund ordering in both
  product directions.
- [x] Select the exact BIP-199 P2PKH shape from `zcash_script 0.4.3` primitives;
  reject its semantically equivalent ready-made helper because that helper
  duplicates the signature tail and is not byte-identical to BIP-199.
- [x] Pin SPEL v0.5.0 with its exact LEZ v0.1.2 compatibility commit and keep
  newer LEZ semantics in a separate drift lane.
- [x] Reject superseded Zebra 4.5.1 and vulnerable 5.1.1/5.2.0 full runtimes;
  pin signed stable Zebra 5.2.0 and copy its exact official-image binary into a
  pinned distroless nonroot runtime that passes the strict vulnerability gate.
- [x] Add RED/GREEN exact redeem-script vector before adapter code, including
  BIP-199's common tail and minimal CLTV encoding.
- [x] Add RED/GREEN claim/refund stack, P2SH, wrong-preimage, and exact CLTV
  boundary vectors through the upstream interpreter before transaction
  orchestration.

### LEZ escrow and generated client

- [x] RED/GREEN a minimal SPEL program and generated IDL against the exact
  pinned compatibility set without hand-written IDL duplication.
- [x] Require the standalone compatibility build and its dedicated advisory,
  license, ban, source, exact-commit, and non-exposure checks in CI.
- [x] RED/GREEN a generated client golden from the evidenced IDL; do not hand
  duplicate the client surface.
- [x] RED/GREEN actual-user native LEZ through v0.1.2's canonical
  `authenticated_transfer`: initialize the escrow PDA custody account under that
  program, fund from a signed user, release with escrow-PDA delegation, and keep
  the immutable-destination refund permissionless.
- [x] RED/GREEN two independent custom-token definitions through official
  `ata_core`/ATA-program derivation and nested token calls. The custody address
  is `ATA(metadata, definition)` and its account owner is the token program;
  direct token holdings at an escrow custody PDA do not satisfy RFP F7. Generated
  clients sign with owner accounts, never ATAs; refund and ATA creation are
  permissionless with immutable destinations.
- [x] Add the canonical SPEL/Risc0 guest build and checked ELF/program-ID
  evidence. Exact Risc0 3.0.5 builds ELF SHA-256
  `a324355c6417f6ac7265ab8ba880287d0976e8c27a672917d293bddd80be7006`
  with ImageID
  `c14c978abbaedeffb54c71aa6a96275d1fdb66fcf79f7343bf6bf7aee04f4483`.
- [x] Run standalone-sequencer native actor tests against exact v0.1.2 using an
  in-process ephemeral-port/temp-state service. The harness proves the mandatory
  clock, deploys the checked guest through public RPC, uses the two genuinely
  funded genesis actors, and observes canonical initialize/fund/claim/refund
  state and balances. RED cases prove wrong preimage, valid-signer wrong role,
  and permissionless early refund rejection without nonce or custody mutation;
  canonical block time then enables the fixed-destination refund.
- [x] Expose that exact checked-guest standalone as a reusable external process
  for later reference actors. The process verifies the repository-tracked ELF
  bytes and ImageID before creating any node state, refuses pre-existing home or
  readiness paths, creates a fresh mode-0700 home, uses an allocated port, and
  publishes a no-clobber mode-0600 private readiness manifest only after health,
  genesis, mandatory-clock progress, exact deployment transaction/containing
  block and ProgramId, the static authenticated-transfer built-in identity, and
  two key-derived funded genesis actors are verified through official RPC. The
  sequencer configuration and readiness handoff use one nonempty deterministic
  channel accepted by the signed SDK agreement validator. Its
  successful and rejection paths keep signing keys out of bounded diagnostics
  and shut down on stdin or Ctrl-C. This is reusable local-node infrastructure,
  not composed actor-corridor or public-v0.2 evidence. The initial exact run
  retained the false custom-`getProgramIds` assumption as RED. The corrected
  earlier full runner passed the process suite, strict Clippy, actual native and
  two-definition actor lifecycles, and byte-identical recursive-cost evidence
  with the schema-v2 transaction/block proof. A focused actor-contract RED then
  exposed the prior empty channel; the corrected three-test locked-graph
  readiness suite is GREEN and the exact full runner is repeated before
  corridor evidence.
- [x] Record native recursive Risc0 costs with the exact guest and production
  v0.1.2 state transition. A single-threaded direct-state replay excludes
  mandatory-clock noise, attributes the escrow root plus authenticated-transfer
  child for each instruction, checks cycle classification/count/budgets, and
  reproduces the checked machine-readable evidence artifact.
- [x] Run the equivalent official-ATA actor lifecycle for two independent token
  definitions. Real owner keys create/fund actor ATAs; metadata custody is the
  exact ATA of metadata and definition. Claim/refund plus wrong-preimage,
  wrong-role, cross-definition destination, and early-deadline negatives pass
  through canonical standalone blocks with exact holding/supply conservation.
- [x] Record recursive costs for every token instruction. The deterministic
  direct-state evidence attributes one escrow session for initialization and the
  escrow, ATA, and nested Token sessions for custody/fund/claim/refund. CI checks
  their order, segments, cycle classification, allocated totals, user budgets,
  and the complete generated escrow evidence JSON.
- [x] Port and rebuild SPEL, guest, generated client, and PDA derivations for LEZ
  v0.2.0 before live-testnet evidence. v0.1.2 `/NSSA/` and v0.2.0 `/LEE/` PDA
  domains are incompatible. Official LEZ v0.2.0 and its public testnet are live,
  but SPEL PR #238 head `df17acd98436be4f09c55877dae1fe2e73cbcdca`
  remains open/unmerged with no submitted maintainer review. A provisional port
  may use that exact head for engineering evidence. A merged/tagged release or
  explicit reviewed exception remains a final production-readiness gate. Under
  ADR 0018 it does not stop M2 certification when the immutable pin,
  compensating controls, repository-owned behavior, and evidence are green and
  the open Logos item is linked from the milestone packet. Open issues #242 and
  #243 still require fail-closed PDA/program-ID handling in repository code.
  Commit `ec683ff` builds the Risc0 v0.2 guest/generated client, binds ELF
  SHA-256 `40c9d37c...8021`, ImageID `f8385049...0fbe`, and ProgramId,
  executes recursive native and two-definition token claim/refund plus full
  child-failure rollback, and tests an exact-once official-RPC deployer. This
  statement is retained as historical evidence for that commit, not the current
  certification artifact.
- [x] Promote the current v0.2 on-chain target to the exact digest-pinned Docker
  build. Direct Docker and Docker-backed methods embedding agree on ELF
  `c85055f6...c9d2e` and ImageID/ProgramId `5cf8c5a4...329c1`; deployment
  transaction `bd16808e...733f` finalized in local block 2582 before both
  canonical actor directions completed. The immutable certification packet
  records why path-sensitive host output was superseded without rewriting the
  earlier evidence.
- [x] Pin and audit a provisional LEZ v0.2 executable engineering lane against official
  tag `v0.2.0` and SPEL PR #238 head
  `df17acd98436be4f09c55877dae1fe2e73cbcdca`. CI proves the LEE
  configuration seam, a single `lee_core` type identity, the `/LEE/` PDA vector,
  and four independently locked root/guest/methods/deployer graphs. CI runs
  format, strict Clippy, tests, rustdoc, exact Docker/host ELF identity checks,
  and graph-local advisory/ban/license/source audits. Official RPC types still
  pull Logos common/libp2p/Hickory advisories RUSTSEC-2026-0118/-0119 into the
  deployer; exact feature/reachability guards and ADR 0018 make this a disclosed
  nonblocking M2 upstream exception and a production-release blocker.
- [x] Source-audit and produce attested locked outputs for the full
  non-standalone local-v0.2
  service boundary. ADR 0024 and the executable contract bind clean LEZ
  `a58fbce2...`, Rust 1.94.0, Bedrock digest plus immutable OCI source revision
  `d8711bbc...`, exact r0vm/Rapisnark/distroless inputs, and the corrected
  sequencer publication to Bedrock and indexer polling from Bedrock. One
  clean-source locked offline build into a fresh target attests sequencer
  SHA-256 `3727e9aa...412f` and indexer SHA-256 `6ed54f04...7442`; a warm
  locked offline rerun performed no rebuild and retained those hashes.
  Restricted distroless CLI smoke passes. Independent clean rebuild reproducibility remains open. Isolated run `v02-actors-finalized-20260713b` additionally executes the pinned Bedrock, sequencer, and indexer with signed channel onboarding, non-genesis finality, and distinct maker/taker owner/Vault pre-Claim state at the exact finalized block.
- [x] Package and execute the exact binaries with Bedrock under `.e2e/{run_id}/lez-v02`. Run `v02-actors-finalized-20260713b` proves cryptarchia advancement, exact missing-channel failure, signed runtime-channel accreditation, sequencer health/channel/built-ins/genesis, finalized ID 2, indexer by-ID/by-hash equality, sequencer Borsh-header identity, exact maker/taker owner/Vault allocations and nonces through `getAccountAtBlock`, channel advancement, and fail-closed exact cleanup.
- [x] Complete the PoC-scoped LEZ v0.2 effect composition. The Vault
  Claim submission slice is GREEN: typed preparation and exact bytes are stored
  in an immutable role/run/runtime/signer-bound SQLite journal,
  `AttemptStarted` commits before the only call, every reopen is observe-only,
  and seventeen focused tests cover forced concurrency, crash windows,
  ambiguity, exact error classification, malicious schema triggers, strict
  revisions, duplicated bytes, actor-binding drift, and filesystem
  substitution. The full package gate has 42 integration tests. The exact
  `lez-v02-bridge-poc` source now adds an explicit nonzero-loopback,
  file-input-only role process with capability/run/runtime/signer/state binding,
  official sequencer and indexer health gates, prepare/observe/revealing-claim/
  submit methods, PREPARE replay, repeatable observation/transient-error
  execution, and unknown-before-I/O submit durability. Refund is typed
  unavailable and indexer finality is not asserted. Existing gates are GREEN;
  no new test was added by the bridge fix. Both role bridges and both actor
  commands are now live-exercised. Partial 14d exposed exact claim-absence
  misclassification; pushed `0861117` fixes it. Fresh attempts 14i and 14k
  through 14n stopped before effects. Attempt 14j made effects but correctly
  refused LEZ reveal after only one Zcash confirmation; its distinct 50000 LEZ
  lock remains and its state is never reused. Successful run
  `m2poc-corridor-fresh-20260714o` completed `TakerSellsLez` in 25.370 seconds
  across 39 rounds/78 actor events with one successful payload-free
  `moving_tip` retry. Exact LEZ initialize/fund/claim finalized in blocks
  264/265/266 with terminal `Claimed`, custody 0, depositor 100000, and
  claimant 150000. The Zcash funding at height 106 received two confirmations
  before reveal and its `:0` output was spent at height 108. Successful reverse
  run `m2poc-corridor-reverse-fresh-20260714c` completed
  `TakerSellsForeign` in 26.960 seconds across 50 rounds/100 actor events with
  no same-run retry. LEZ initialize/fund/claim finalized in blocks 641/642/643;
  Zebra funding at height 113 had two confirmations before reveal and its exact
  `:0` output was claimed at height 115. Attempts reverse14a and reverse14b are
  retained, effect-bearing, and never reused; their direction-specific
  observation failure produced the focused regression for direction-derived
  LEZ depositors. Integrated journal transitions and ambiguous native-effect
  crash reconciliation remain
  queued for owner-triggered hardening. Actual local evidence already proves
  both Vault Claims in finalized blocks 29/30 and the checked native initialize/fund/claim
  transactions in finalized blocks 219/220/223. Those manual indexer reads do
  not retroactively make the CLI's sequencer-inclusion result a finality result.
- [x] Complete the full local PoC runtime tuple. Vault onboarding, checked deployment,
  and the role-separated LEZ native initialize/fund/claim vertical slice are
  GREEN in `m2poc-vertical-20260714a`. The same run now also has a reloadable,
  pair-validated `TakerSellsLez` reference-actor fixture bound to the checked
  LEZ runtime and a stable mature real-Zebra UTXO. Its saved discovery window
  1..256 is stale at later tip 389, so it is evidence but not runnable input.
  The development runner prebuilds, provisions just in time, starts explicit
  run-port role bridges, has no arbitrary round cap, polls every 0.10 seconds to
  a fail-closed millisecond deadline, KILL-bounds calls, and permits at most
  eight exact same-run retries inside the absolute deadline. Runs 14o and
  reverse14c live-prove those controls and terminal evidence for both
  directions. The runner serializes only its exact endpoint tuple with an
  advisory lock and never cleans unrelated Docker or host resources. Restart/
  ambiguous-outcome recovery is deferred until
  the owner starts hardening.
- [ ] Deploy the evidenced escrow to LEZ testnet 0.2 and retain an immutable
  deployment manifest plus public smoke transaction evidence.
- [ ] Re-measure initialize, claim, and refund compute units on the exact deployed
  LEZ testnet 0.2 runtime; the checked v0.1.2 standalone evidence is not a
  substitute for the RFP P1 named-testnet-version report.

### Transparent Zcash adapter

- [x] Implement a typed one-input spend foundation with actual funding `TxOut`
  validation, fee conservation, key ownership, exact claim/refund scriptSigs,
  ZIP-244, secp256k1, and canonical V5 serialization.
- [x] Implement transparent UTXO selection plus dust-aware P2SH funding,
  fee/change policy around that spend foundation.
- [x] Prove exact redeem/P2SH and signed claim/refund bytes/txids locally, with
  real upstream interpreter signature checks and mutation negatives.
- [x] Prove funding/claim/refund acceptance, mutated-signature and pre-CLTV
  rejection, and confirmation through pinned Zebra RPC.
- [x] Exercise wrong preimage/signature, non-final sequence, CLTV edge, and
  fee/dust cases across local and Zebra-authoritative layers.
- [x] Recognize bounded canonical BIP-199 spend evidence with the exact pinned
  Zebra consensus flags, all six ZIP-244 sighash modes, and consensus-valid
  high-S/nonminimal/semantic stack forms. Preserve the revealed preimage and
  complete transaction policy fields, while reporting the SDK's stricter
  low-S/minimal/`SIGHASH_ALL` construction policy separately. Agreement-derived
  funding provenance plus durable spend reorg tracking remain open.
- [x] Exercise two independent funding/spend lifecycles concurrently through
  Zebra, invalidate their shared non-finalized terminal block, detect
  confirmation regression, rebroadcast exact actor transactions, reject a
  conflicting same-output replacement, and reconsider the exact block.
- [x] Exercise an accepted competing-fork replacement and deeper reorg through
  actual nodes. Two disconnected pinned Zebras share an RPC-relayed prefix,
  then mine a claimant-claim three-block branch and a conflicting funder-refund
  four-block branch. Submitting the latter through `submitblock` makes it
  canonical at every detached height and exposes the replacement transaction
  through the primary node.
- [x] Implement immutable network-bound `deterministic-local-v1` and
  `public-testnet-v1` ZEC profiles. Exact reviewed confirmation, LEZ-delay,
  ZEC-height, reaction-margin, branch, and expiry constants pass; wrong
  network/branch, timestamp/height overflow, absent calibration, and a
  one-second-short margin fail closed. Both directions retain LEZ-before-ZEC.
- [x] Add the production-grade ZEC observation validator. It re-decodes one
  canonical transaction with no trailing bytes, recomputes txid/depth/outpoint,
  and binds network/branch, stable canonical block hash/height, exact value,
  redeem/P2SH bytes, explicit output index, and active-chain status before a
  lossy `ChainProof` projection. Malformed, inconsistent, side-chain,
  mismatched, out-of-range, and overflow snapshots fail closed.
- [x] Populate the typed funding observation through a stable actual Zebra RPC
  snapshot. The E2E binds Regtest by exact genesis hash because Zebra reports
  its BIP70 family as `test`, verifies NU6.2, holds the tip stable across raw
  transaction/canonical-height queries, and revalidates the exact
  100,000,000-zatoshi BIP-199 outpoint before core projection.
- [x] Add stable-tip canonicality reconciliation. Positive observations retain
  raw bytes and the exact tip used for depth; removals require a changed
  canonical hash at the prior height and a stable replacement tip. The tracker
  proposes without mutation, suppresses exact replay, and advances only after
  the caller confirms a durable commit. RPC absence/errors emit nothing.
- [x] Encode watcher events as a version-1 primitive persistence DTO rather than
  deserializing trusted canonical types. Round-trip and corruption tests recheck
  known branch, height-derived nonzero depth, raw transaction/txid, outpoint,
  value, and script bindings; loaded records remain historical until fresh RPC
  reconciliation.
- [x] Persist each versioned ZEC event and its swap aggregate revision in one
  immediate SQLite transaction. Schema-v2 tests prove legacy migration, future
  database/payload rejection, forced update-failure rollback, stale revisions,
  exact replay after an unknown successful commit, role isolation, and restart
  loading with record revalidation. Replay is predecessor-revision scoped, so a
  later identical canonical reappearance remains a new event.
- [x] Add participant-aware core funding/removal semantics for both ZEC
  directions. Reverse ZEC maps ZEC to the maker-funded leg; its distinct reorg
  phase pins the exact transaction, suspends both claims, rejects conflicting
  replacement, restores on exact reappearance, and preserves both refunds.
- [x] Add independent immutable maker-leg confirmation policy. Reverse-ZEC tests
  prove below-threshold accumulation, threshold promotion, canonical depth
  regression into claim suspension, and exact depth recovery; a 10→9 public
  profile regression can no longer remain `BothLegsLocked`.
- [x] Add the initial direction-derived runtime composition for canonical and
  removal events. Both directions prove atomic aggregate/event commit, close and
  reopen, exact canonical reappearance, and predecessor-slot replay before core
  mutation after an unknown successful commit.
- [x] Revalidate and replay ordered journal records into the historical tracker
  head on restart. Missing predecessors and changed inclusions without explicit
  replacement fail closed; close/open restores removed and reappeared heads, and
  an identical fresh requery is suppressed without treating history as current
  canonicality.
- [x] Journal removal/replacement after `Completed` or `Refunded` without
  mutating the absorbing lifecycle result, and return
  `TerminalReorgDetected` rather than a normal applied outcome, including on
  exact replay. Durable operator/security alert delivery remains open.
- [x] Commit a post-dependent different-ID replacement as one atomic journal
  revision while retaining the committed participant transaction ID and
  role-specific reorg phase. Both ZEC-funded roles return an explicit
  `ReplacementConflict`; pre-dependent replacement and exact-ID re-mining pass.
- [x] Replay and validate the exact durable watcher head before projecting a new
  event. Same-transaction but stale-inclusion replacement evidence fails before
  aggregate or journal mutation.
- [x] Define a version-1 primitive ZEC binding record for immutable profile,
  expected network/branch/value, BIP-199 source terms, and derived redeem/P2SH
  bytes. Loading reconstructs the contract and rejects profile or script drift.
- [x] Add schema-v3 immutable ZEC binding persistence. Swap plus first binding
  insert commits atomically; exact repeats are idempotent, changed terms fail
  without overwrite, restart revalidates, and legacy databases migrate unbound
  rather than inferring signed terms.
- [x] Enforce immutable bindings before tracker restoration, replay detection,
  projection, and lower store commit/probe boundaries. Missing legacy bindings,
  profile/coordinator confirmation-policy mismatch, and event-envelope mismatch
  fail before revision, journal, or aggregate mutation.
- [x] Add the operator/security alert outbox (introduced in schema v4 and
  retained through schema v9). Conflict warnings and
  terminal critical alerts commit with event+aggregate; Applied emits none;
  forced insertion failure rolls back; replay/restart preserve one cursor and
  acknowledgment without changing protocol state.
- [x] Expose owner-authenticated alert status/list/ack through actual daemon and
  CLI processes. Wrong credentials fail before RPC parsing; restart preserves
  attention; acknowledgment retains evidence and reorg/terminal protocol phase.
- [x] Prove the actual-node persistence boundary: canonical funding, immutable
  binding, close/reopen, unchanged fresh-query suppression, affirmative
  two-Zebra fork removal, second close/reopen, and exact retry pass through the
  maker runtime on schema v9. Daemon-integrated polling remains open.
- [x] Establish the concrete ZEC SDK pre-lock/activation/resume boundary:
  role-fixed async discovery and negotiation treat bytes as untrusted; separate
  role stores persist exact accepted envelopes before activation; exact replay
  is idempotent and changed same-key input conflicts; adversarial resume
  revalidates wire, role, revision, commitment, and swap ID; the post-lock
  `ActiveZecSwap` exposes no discovery, negotiation, raw chain, or store handles;
  claim material and diagnostics are redacted and secrets zeroize.
- [x] Implement the concrete canonical LEZ/ZEC agreement validator and bounded
  wire record. Both actors sign the same profile, roles, digest, LEZ
  environment/channel/genesis/program/asset/amount/custody, BIP-199 binding, exact
  Zcash destinations/fees/input-set commitment/expiry, refund anchors/bounds,
  and negotiation transcript. Negotiation, activation, and resume now use this
  concrete record, though that boundary alone does not satisfy the full SDK
  lifecycle without typed effects and actor execution.
  Public-testnet validation remains fail-closed until a reviewed immutable LEZ
  deployment exists. The provisional lane now compiles the exact
  dependency-light SDK derivation source and proves metadata, native multi-seed
  custody, and ATA bytes against pinned upstream v0.2 `lee_core`, SPEL, and
  ATA-core types; deployed adapters must still recompute selected identities.
- [x] Complete the PoC-scoped typed LEZ and Zcash actions plus atomic
  active-swap transition persistence for escrow creation and both claims; the
  SDK and lower recovery lanes also retain both ordered refunds. The first-lock
  contract now stages exact bytes before effects, observes before
  rebroadcast, separates LEZ initialize/fund, and atomically projects/replays
  confirmed taker evidence through the production role-fixed SQLite adapter.
  Maker-independent observation now selects only the agreement-derived maker
  node route, commits without taker intent, survives SQLite restart, and remains
  non-authorizing: SDK next action is `Wait`. Forward Zcash now requires and
  persists complete canonical evidence. Its ordered schema-v10 journal now
  commits/replays depth changes, atomic replacements, and affirmative removals
  across restarts and rejects discontinuous or history-incompatible rows.
  The distinct fresh pre-second-lock call now replays and re-queries without
  caching authority or changing `next_action`. The maker effect invokes that
  check internally on every drive, persists the direction-fixed opposite-chain
  plan before submission, and atomically commits confirmed Maker evidence.
  Both deterministic-local directions now use separate maker and taker stores:
  each actor learns the other lock through its role-fixed chain-observation
  boundary, reaches `BothLegsLocked`, and independently replays there after
  schema-v10 SQLite close/reopen. The expected maker submission ID is still
  asserted by the contract-double adapter, so production adapters must derive
  it from canonical node evidence. Reverse LEZ now rejects
  primitive transaction-ID/depth assertions and accepts only a stable canonical
  escrow snapshot bound to signed channel/genesis, exact public fund
  program/signer/account order, canonical inclusion and tip, complete SPEL
  metadata, exact native custody, depth, and public-profile finality policy.
  The primitive snapshot is journaled and fully revalidated after SDK and
  SQLite close/reopen. Its schema-aware historical decoder derives the old
  missing instruction kind from the signed asset and losslessly preserves
  `u128` amounts above `u64`; a single-field negative matrix covers chain,
  transaction, metadata, and custody substitutions. Commit `7001198` closes the
  analogous revealing-claim evidence gap: live evidence is constructible only
  from a stable primitive claim snapshot binding the node-reported identity to
  the official-decoder hash, signed claimant, generated account order, exact
  claim instruction and preimage, terminal metadata, empty custody, canonical
  inclusion, and depth. New owner and observer rows use secret-free schema v2;
  replay decrypts the separately protected preimage, reconstructs the snapshot,
  and reruns the full validator. Legacy opaque v1 remains internal read-only
  compatibility and cannot be emitted by a live adapter. The official-wire LEZ
  transaction decoder/bridge, token-custody observation matrix and refunds are
  remaining work. Commit `add5d98` now proves the SDK and
  production SQLite happy path after `BothLegsLocked`: the agreement-derived
  first claimant stages encrypted exact LEZ bytes before submission, both actors
  durably project the canonical reveal, the observer atomically protects the
  extracted preimage, the other actor stages and submits the Zcash follow-up,
  and both separate role stores close/reopen at `Completed` in both directions.
  Owner and observer transitions occupy distinct journal slots; this is
  deterministic contract-double chain evidence, not an actual-node claim.
  Coordinator state retains only a SHA-256 claim marker. Submission AAD binds
  the full agreement context plus claim step, staged revision, and expected
  identity; exact submissions and preimages remain encrypted at rest. Commit
  `5ed04ec` also migrates exact legacy v8 plaintext evidence arrays to tagged
  commitments in one crash-safe v9 transaction and scrubs database/WAL remnants.
  Commit `340bf10` closes the repository-controlled claim persistence hardening
  gate: unknown submission and stale replay observe exact durable bytes before
  any rebroadcast; forced aborts roll back coupled effects; wrong key ID/material,
  ciphertext, nonce, authenticated fingerprint, future payload version, orphan
  rows, duplicate revisions, and active-head drift fail closed. Broader process
  kill coverage remains M5. The public refund SDK now fixes LEZ-before-Zcash
  order in both directions, persists exact owner intent before broadcast,
  observes before rebroadcast, rejects early/unstable/reorged funding, and
  commits distinct owner/observer transitions through a versioned revalidation
  boundary. Commit `845ff89` now persists refund intents/transitions in the
  unified schema-v10 journal. Owner projection atomically retains the exact
  intent, inserts the transition, advances the active revision once, and
  deletes the pending intent; observer rows contain no signing intent. Both
  directions close/reopen at `Refunded`, and exact conflict, forced rollback,
  future-version, unknown-field, and journal-corruption cases fail closed.
  Commit `8b16670` supplies the production Zebra owner/observer refund port
  with exact signer role, outpoint, destination, fee, expiry, branch, bytes,
  maturity, stable-tip, submit-outcome, reorg, and bounded spender-discovery
  validation. Commit `9f3abc9` adds the main LEZ native-refund validation
  adapter: both signed directions expose caller-owned IDs/windows for state,
  prepare, exact/discovery observe, and one-attempt submit; stable clocks,
  complete accounts, exact transaction/instruction facts, the millisecond
  deadline, window, depth, and durable identity are independently checked.
  Uncertain submits remain `Unknown`, never rejection. Official sidecar refund
  handlers, context-owning SDK wiring, and composed actors remain.
  The actual-node implementation follows ADR 0022 because an executable RED
  proved the Zcash `crypto-common = 0.2.0-rc.1` graph cannot coexist with the
  official LEZ graph's stable `crypto-common ^0.2` requirement. Commit
  `b1de754` closes the dependency-light bridge-contract slice with bounded,
  source-correct terms, metadata, account ordering, exact inner transaction
  bytes, discovery windows, and typed absence/ambiguity semantics. The
  separately locked `lez-v0-1-2-sidecar` planner now constructs official native
  initialize/fund messages, reserves one checked consecutive nonce pair under
  a mutex, caches randomized BIP340 signatures for byte-identical retry, and
  accepts only that exact cached pair for submission. Its constructor and every
  request bind the complete runtime descriptor, role, signer, escrow program,
  and authenticated-transfer program before nonce use. Seven locked tests plus
  strict format, Clippy, rustdoc, advisory, license, ban, and source gates pass.
  The official node slice now implements `getAccountsNonces`, exact cached-byte
  `sendTransaction`, and bracketed bounded `getBlockRange` scans through
  upstream generated RPC types. Literal loopback, finite body/time/concurrency
  bounds, recomputed block hashes and links, exact returned IDs, and conservative
  post-submit unknown outcomes pass four additional tests. The authenticated
  server library now binds capability, `RUN_ID`, role, and runtime before JSON
  parsing; restores exact randomized prepare results through the official
  decoder; and writes a durable unknown-submission guard before any node call.
  That checkpoint registered six executable methods while exposing the two
  native-refund calls as the next RED; commit `4e6fdec` subsequently registers
  and proves those final handlers. Official revealing-claim preparation now
  validates the exact claimant role, runtime, signer, agreement terms,
  preimage, and funding binding, restores byte-identical randomized output
  after restart, and restricts submission to the exact cached transaction.
  Commit `f1f98a1` closes native escrow observation and the executable
  sidecar runner. Official transaction/signature/instruction/account decoding,
  linked-block/genesis validation, identical tip brackets, exact cached owner
  lookup, bounded counterparty discovery, ambiguity/moving-tip rejection, and
  conservative full-window absence pass. Maker and taker processes run
  concurrently with distinct private capabilities, signers, runtime
  descriptors, stores, and ephemeral listeners. Commit `e9fc760` adds official
  exact-owner and bounded counterparty revealing-claim observation, canonical
  Risc0 instruction/message/signature/account validation, terminal account
  checks, stable full-window absence, ambiguity/moving-tip rejection, and
  restart-safe native/claim cache coexistence. Commits `3d18819` and `3da31b0`
  add both-direction agreement-bound Zebra funding discovery and the explicit-
  context main revealing-claim adapter. Commit `4e6fdec` completes official
  permissionless native-refund preparation, restoration, exact/discovery
  observation, eight-method server registration, and crash-safe generic submit.
  Context-owning SDK ports now implement first-lock, taker/maker observation,
  revealing-claim, and native-refund SDK traits over a role-local SQLite
  operation journal. They retain caller-owned IDs/windows, use a fresh client
  per attempt, restore exact ambiguous contexts after restart, and reject
  canonical-funding mutations before the sidecar. The production fresh-client
  factory now rereads a bounded regular non-symlink mode-0600 capability file
  on every attempt, detects replacement, retains no secret bytes, zeroizes
  rejected material, and redacts both content and path. Actor processes and the
  composed corridor remain.
  The separately locked standalone compatibility harness now also owns a
  reusable external node binary rather than requiring the future actor runner
  to embed the sequencer. It rejects a tampered guest before state creation,
  refuses and preserves a pre-existing node home, creates a fresh private home,
  deploys the exact checked guest, verifies the dynamic loopback client endpoint
  and official genesis, exact deployment transaction/block, built-in owner, and
  account facts, then publishes a private write-once readiness handoff with the
  two funded deterministic actor keys. Upstream `getProgramIds` is deliberately
  used only for its static authenticated-transfer built-in; custom deployment
  authority is `getTransaction` plus exact containing-block membership.
  The earlier corrected exact full runner closed this provisioning boundary
  with exit `0`, including process rejection paths, native/two-definition actor
  lifecycles, strict Clippy, and byte-identical recursive costs. No SDK actor or
  sidecar consumes that handoff in a composed LEZ/Zebra swap yet. The later
  nonempty-channel correction passes its focused suite; the exact full runner
  is repeated before this handoff is consumed as current corridor evidence.
  Commit `8c92007` closes official revealing-claim preparation and restart
  restoration; the exact post-runner sidecar tree passes 25 all-target tests
  plus strict Clippy, rustdoc, advisory, license, ban, and source gates.
  Commit `cdb732e` supplies the in-process Zebra first-lock half: bounded private
  DTOs, exact lowercase hash/transaction decoding, agreement/network/branch/
  genesis binding, stable-tip canonical snapshots, exact V5 authorization-byte
  checks, observe-before-rebroadcast, byte-exact submission, explicit
  post-submit unknown outcomes, and sensitive cookie authentication on explicit
  loopback HTTP only. The production owner-claim port now derives its request
  from the accepted agreement, delegates only signing to the role-local
  capability, validates the exact retained one-input/one-output claim, observes
  before byte-identical rebroadcast, and never advances from an ambiguous RPC
  result. The adapter's 43 tests use SDK/SQLite-built authentic contexts and
  now include bounded canonical counterparty outpoint-spend discovery plus the
  production refund port from `8b16670`. The refund path enforces role-local
  signing, exact outpoint/destination/fee/expiry/branch/bytes, fresh stable
  funding, CLTV maturity, conservative submit outcomes, and both directions. Zebra
  exposes no direct spender index, so older or unresolved spends remain
  `Unstable` rather than becoming false absence. Actor wiring remains. The
  composed actual-node corridor is still RED. Commit `b0d3b52` closes the
  cross-adapter evidence-loss gap: typed Zebra/LEZ observations now carry the
  exact final submission identity, canonical transaction ID, and observed depth
  through `ReadyForFundingProjection`; the SDK checks step, durable identity,
  and agreement-required depth before returning it, and actor/SQLite restart
  tests project that returned evidence instead of fabricating a replacement.
  The next runtime-identity RED
  proved that the pinned v0.1.2 standalone's `/NSSA/` PDA domain cannot be
  truthfully reported as v0.2 `/LEE/`. GREEN adds the append-only
  `DeterministicLocalV0_1_2Compatibility` agreement environment, selects exact
  metadata/native-custody/token derivations from the signed environment, keeps
  public v0.2 fail-closed, and cross-checks the dependency-light SDK derivation
  byte-for-byte against pinned official `nssa`/`ata_core` v0.1.2 types. This is
  the honest deterministic corridor profile, not production v0.2 evidence.
  The main workspace bridge client now implements all eight
  `lez_bridge.v1.*` calls over literal loopback HTTP with a sensitive
  capability, exact run/role/runtime echoes, one-use request IDs, bounded
  bodies, no redirects/proxies/retries, and nine passing contract tests.
  Commit `a2b01a9` adds the first main-process adapter: it accepts a
  caller-owned request ID, rechecks the signed compatibility environment,
  channel, genesis, escrow program, actor, and signer, then maps the exact
  official initialize/fund result into one SDK first-lock plan. Six tests cover
  the exact mapping, signed/runtime mutations, token rejection, response
  mismatch, and unknown-outcome no-retry behavior. Server-side durable
  idempotency is implemented. Commit `a2697e6` adds agreement-bound exact
  owner and bounded claimant-discovery observation, revalidates full primitive
  initialization/funding facts through the SDK canonical validator, rejects 41
  mutation classes, and preserves caller-owned request identity with one
  transport attempt. Commit `e9fc760` closes the official-sidecar revealing-
  claim observer with exact cached-owner and peer-independent depositor
  discovery paths; 28 all-target tests plus strict Clippy, rustdoc, and
  dependency-policy gates pass. Commit `9f3abc9` closes the main native-refund
  validation boundary with both-direction role tests and 21 primitive mutation
  classes. Commit `3da31b0` adds the analogous explicit-ID/window revealing-
  claim adapter; commit `4e6fdec` completes native-refund sidecar execution and
  observation. Context-owning SDK-port composition now passes production-store
  initialize/fund ordering, pre-sidecar claim-evidence mutation, and ambiguous
  refund-reopen checks; actor composition remains. Pin
  weakening, crypto patches, and hand-copied LEZ wire/RPC types are prohibited.
  The public `validate_runtime_binding` API now owns the reusable comparison
  between accepted signed terms and the described sidecar's compatibility
  environment, channel, genesis, escrow program, and the local participant's
  signed account. The process role is deliberately a separate local
  trust-boundary check: `LezBridgeAdapter::new` also requires the described
  sidecar role to equal the fixed local participant before it creates any port.
  External contract tests cover both roles, mutation rejection, and
  payload-free diagnostics. Canonical runs
  `m2cert-canonical-forward-bb53daf-20260714a` and
  `m2cert-canonical-reverse-bb53daf-20260714a` subsequently close the
  actual-node claim corridor in both directions. Composed actual-node refund and
  recovery remain owner-gated hardening.
- [x] Prove the composed native happy path preserves the ADR 0022 atomicity
  invariants in both directions: separate role processes/stores/keys/sidecars;
  taker-first and maker-after-canonical-evidence ordering; both locks before
  reveal; canonical LEZ reveal before the exact Zcash outpoint spend; durable
  intent before every broadcast; observe before each PoC submission; and no peer
  dependency after both locks. Record the signed refund margin but defer
  close/reopen recovery and public-latency calibration until the owner starts
  hardening rather than treating Regtest timing as production proof. Both
  canonical runs show direction-correct first and second locks, a
  two-confirmation Zcash gate, LEZ reveal before exact-outpoint spend, four
  revision-4 `Completed` stores, and zero terminal LEZ custody.
- [x] Before the LEZ revealing submission, freshly revalidate the exact
  coordinator-pinned Zcash outpoint as canonical, unspent, sufficiently deep,
  and before CLTV. RED/GREEN this after restart in both directions with absent,
  spent, unstable, replaced, under-depth, and expired-tip observations; none may
  release the preimage, while the restored exact output submits the already
  retained LEZ transaction once. Commit `166d3e5` closes this repository-
  controlled gate: the durable claim is observed first and, when absent, the
  exact Zcash funding observation is literally the final awaited port call
  before the byte-identical LEZ submission. The SDK revalidates transaction,
  outpoint, value, script, network, branch, depth, one stable canonical/UTXO tip,
  unspent state, and pre-CLTV height; absent, spent, unstable, replaced,
  under-depth, expired, or mutated facts reveal nothing across restart. The
  local one-confirmation profile requires an additional reorg-distance block;
  the signed production policy remains authoritative when it is stronger.
- [ ] After the owner enters QA hardening, keep observing both locks after
  `BothLegsLocked`: ingest durable removal
  and replacement facts for taker and maker locks instead of treating the
  projected phase as immutable. A stale local phase must never authorize a
  revealing claim.
- [ ] After the owner enters QA hardening, recheck the remaining LEZ/Zcash
  recovery horizon immediately before the maker second-lock effect. Near-expiry or expired accepted terms must produce no
  second-lock broadcast even when identity/depth/finality evidence is valid.
- [ ] After the owner enters QA hardening, complete peer-independent timeout recovery
  through public SDK ports. The
  SDK and versioned durable-record boundary are GREEN: exact LEZ/Zcash owner
  intents precede broadcast, every retry observes first, fresh funding/deadline
  evidence gates resubmission, observer paths cannot sign, and canonical
  transitions replay in LEZ-before-Zcash order. Atomic SQLite journal/revision
  commits plus Zebra and main LEZ refund validation adapters are now GREEN. The
  official refund sidecar and caller-context SDK-port wiring are also GREEN;
  composed independent actors remain before this gate can close.
- [ ] After the owner enters QA hardening, compose cross-chain refund-margin cases through
  actual LEZ and Zebra nodes. The LEZ
  Unix-millisecond/core Unix-second boundary is typed, checked, conservatively
  rounded, and boundary-tested; the composed flow remains.
- [ ] After the owner enters production-readiness hardening, calibrate and publish
  the public-testnet ZEC refund margin against a stated
  worst-case confirmation-latency and operator-reaction envelope. Nominal block
  cadence or local Regtest timing alone does not satisfy RFP F4.
- [x] Keep the stable Zebra/security pin and public-capable configuration under
  M2 contract tests; re-audit releases immediately before the deferred
  public-testnet evidence because the current release horizon ends ahead of NU7.
- [x] Re-audit the deployed SPEL/LEZ guest graph for the M2 tag. All eleven
  independently locked Rust graphs pass the pinned offline advisories, bans,
  licenses, and sources audit; exact Logos-owned advisories and upstream review
  status remain disclosed in the production-blocker register under ADR 0018.
  Re-audit again before public-testnet evidence. Floating pins, undisclosed
  exceptions, or repository-controlled adapter defects still block release.

### Reference-actor delivery and M2 exit

- [x] Publish a living manual reproduction guide for the currently proven maker
  operator, Zebra actor/fork, and LEZ native/token/cost fixtures, with exact
  no-clash and cleanup rules. This does not satisfy the independent composed
  maker/taker local corridor below; live public-testnet items are deferred under
  ADR 0023.
- [x] Harden the unreleased reference-actor configuration as schema v3 after the
  retained schema-v2 lifecycle PoC and before final exact-tree certification. A private raw JSON form cannot
  construct an
  `ActorConfig` without validation; each role binds the exact run, swap,
  sidecar role/signer/endpoint, signed-agreement SHA-256, complete runtime
  descriptor, Zebra network/RPC chain/branch/genesis, finite discovery windows,
  exact candidate outpoints, and isolated state/journal/key paths. Paired
  configs additionally require one preimage owner and one funder, identical
  agreement/runtime/Zebra terms, distinct sidecars and signers, and no
  cross-role path aliases. On Unix, command material must use exact mode `0600`
  and be a single-link, regular non-symlink file; load-time descriptor/version checks,
  agreement hashing, alias detection, and redacted diagnostics reject
  replacement and confused-deputy paths. Offline `status` needs only the
  claim-recovery key and role-store path, and terminal store replay opens no
  chain port. It now opens an existing hardened SQLite store without `CREATE`,
  recovers through an SDK type that cannot perform chain calls, returns
  versioned secret-free state, and reports an absent database as
  `not_activated` without creating it. Create-capable and existing-only store
  opens use `SQLITE_OPEN_NOFOLLOW`, reject non-regular/hardlinked/wrong-mode
  files, and compare device/inode identity before and after mutable setup. The
  `activate` and `drive` compose the exact v0.2 role bridge, SDK, and Zebra port
  in the development runner. Retained schema-v2 run 14o live-proves bounded
  payload-free retry and terminal evidence for `TakerSellsLez`; retained
  schema-v2 reverse14c proves the direction-derived actor/effect mapping for
  `TakerSellsForeign` with no retry. Current-schema certification runs
  `m2cert-schema3-forward-2d09997-20260714a` and
  `m2cert-schema3-reverse-2d09997-20260714a` then repeated both directions
  against the actual local nodes; both independent actors reached `completed`,
  the forward run used 46 drive rounds with no retry, and the reverse used 33
  rounds with two bounded same-run retries.
  The schema additionally separates deterministic-local, self-hosted-cookie,
  and exact Tatum Testnet `x-api-key` Zebra routes, binds matching route kind
  and endpoint across roles without equating secret paths, and keeps every
  bridge endpoint literal-loopback. Public activation validates the signed LEZ
  runtime plus Zebra network/branch before persistence; focused suites pass
  without public I/O.
- [x] Run independent SDK reference maker and taker processes with direction-correct
  keys, transparent funds, LEZ funds, selected node routes, and durable recovery state
  for both supported ZEC directions. Runs 14o and reverse14c prove the two
  direction-derived role mappings and isolated durable stores.
- [x] Pass both happy directions through the public SDK and actual local chain
  boundaries; destroy the test-only pre-lock mailbox after terms persist.
  Production daemon/CLI and Logos Delivery/Chat integration remain M5
  deliverables and are not relabeled as M2.
- [ ] After the owner enters QA hardening, pass abandonment/refund, restart, reorg, and
  concurrent-swap suites through those same actor and chain boundaries.
- [x] Publish exact local LEZ v0.2/Zebra setup and the configuration-only public
  migration matrix, transparent privacy warnings, and shield-after-swap journey.
  Clean-host public funding rehearsal remains deferred under ADR 0023.
- [x] Resolve the yanked `spin 0.9.8` finding in every independently locked
  graph. Exact offline resolution proved `0.9.9` compatible, all ten nested
  lockfiles were updated without other package movement, five temporary
  package exceptions were removed, and all eleven graph-local
  advisory/bans/licenses/sources audits pass with `yanked = "deny"` intact.
  `LOGOS-013` is therefore recorded as a resolved repository finding rather
  than an upstream production blocker.
- [x] Classify a private happy-path recording as an optional external-demo
  artifact rather than an M2 PoC exit gate under ADR 0027. The reproducible
  command, manual actor flow, and immutable machine evidence are the M2
  reproduction contract; generate recordings before an external demonstration,
  and add refund/concurrency recordings only after the owner starts hardening.
- [x] Re-run formatting, strict Clippy, all tests/docs, ShellCheck,
  traceability, advisories, bans, licenses, sources, the full LEZ v0.2 verifier,
  fresh isolated Zebra restart/reorg and real-key claim/refund E2E, dormant
  public-route contracts, all 69 Mermaid renders, and a fresh-db fail-hard
  Trivy scan of the exact Zebra image. All repository-controlled gates are
  GREEN; Trivy found zero HIGH/CRITICAL vulnerabilities. Live public smoke
  remains production-readiness work.
- [x] Bind the annotated `m2-complete` tag to this exact pushed closure commit.
  The tag certifies the local-functional PoC and links the freshly rechecked
  Logos production-blocker register; it does not claim actual-node recovery,
  chaos, public execution, or production readiness. Logos-owned release
  blockers follow ADR 0018 and are final-production gates rather than M2 stops.

## Milestone 3 entry plan: BTC adaptor/Taproot end to end

Status: **progressive private local-devnet PoC complete 2 of 2 through the
public actor and actual nodes at pushed `origin/main` commit `6ded2f9`; later
QA, chaos, infosec, public-Testnet, and production-readiness hardening remain
active**. The canonical countersigned agreement, finalized LEZ funding/claim
adapters, typed Core adapter, and reference-actor revisions zero through four
are GREEN in source, deterministic tests, and run `m3actor-20260716n`. The
exact Core 31.1 release verifier, minimal isolated image fixture, role-aware Regtest boundary,
typed P2TR/CSV transaction library, and one-process public deterministic
two-party MuSig2/adaptor/extraction funding/cooperative-claim composition are
GREEN. CI runs that composition and fail-hard scans its exact image for
HIGH/CRITICAL vulnerabilities. Strict clean pushed-commit evidence on
`f5a9caa66b04b0bec1a86cb732f5a64f63852e6e` closes this cryptographic/Core
fixture sub-slice. Pushed commit `0177151` additionally closes the in-memory
dual-domain signing boundary: distinct role-state objects, fresh OS nonces,
commitment-before-reveal, exact BTC/LEZ message binding, one-use phases, peer
partial verification, adaptation, extraction, and both scalar-reveal orders.
Remote private-CI status remains unobservable without credentials.

The audited packet is
`.e2e/m3actor-20260716n/m3-actor-poc/evidence/m3-actor-local-poc.json`,
completed at `2026-07-16T01:00:30Z` for full commit
`6ded2f9b8ba9ec8e0cfbf06287da92d34256f91a`. Each direction has two unique
confirmed Bitcoin effects and three exact durable LEZ submissions; both maker
and taker terminal statuses are revision 4 `completed` / `complete`. The
Core verifier's exit trap stopped its exact run-owned GnuPG agent, and the
post-run process audit found no agent for the run-n home. This lifecycle fix is
part of certified commit `6ded2f9`; it never performs broad GnuPG cleanup.
historical invocation used `scripts/run-m3-actor-local-poc.sh`, verified guest
target `/tmp/lez-m3-artifact-20260715a`, the exact v0.2 source/service/r0vm
inputs, Rapidsnark directory
`/tmp/lez-atomic-swaps-tools/rapidsnark-v0.0.8/d4133227`, and explicit
`-I/usr/lib/gcc/x86_64-linux-gnu/13/include` bindgen arguments. A reproduction
must use a fresh run ID/root; the complete portable command is maintained in
the M3 operator guide.

Run `m3poc-live2-20260715a` now composes the exact local LEZ v0.2
Bedrock/sequencer/indexer and Bitcoin Core 31.1 Regtest through separate
maker/taker sidecars, restricted Core `rpcauth` users, signing processes, keys,
SQLite journals, and state roots. It completes both happy directions with
actual finalized/confirmed effects. The current tree now has a single
full-lifecycle public BTC reference-actor command through revision four, but the
retained live packet predates that composed command and production signing
authority remains outside the PoC. Its component boundary no longer holds both
signers in one process:
`ca524ff` runs every maker/taker commitment, nonce, partial, and aggregate phase
in fresh role-fixed processes with separate journals; `96f2a31` externalizes
adaptation and scalar recovery; and `f827dad` verifies the resulting external
Bitcoin signature before emitting the exact broadcast-ready transaction.
Pushed `3862dde` also prepares the separate witnessed LEZ initialize/fund
transactions. Pushed `3d7386b` observes that exact pair without overstating
absence/finality; `a3da09e` supplies its typed operator CLI; and `bf5bdbd` uses
official `nssa` for aggregate-account mapping. Live submission, both reveal
orders, and unilateral recovered-scalar completion are now proven at the
operator-composed happy-path boundary. A bounded canonical agreement now binds
both role signatures, exact chain identities and confirmation policy, LEZ
runtime/custody/claim terms, the reconstructed P2TR/CSV contract, funding
outpoint/value, cooperative transaction/sighash, and direction-correct recovery
schedule. The typed finalized witnessed-claim adapter is now GREEN. The
actor-local Bitcoin recovery store is also GREEN through revision four and
offline `Completed` reconstruction. The reference actor now projects exact
revealing and follow-up claims through revisions three and four in both roles
and directions. Its live Bitcoin path now constructs the role- and
revision-owned exact claim, persists complete witness bytes before one send,
never rearms `Started` or `Unknown`, and consumes only the typed Core
finalized-claim observer for projection. Pushed commit `66d352f` composes the
live LEZ path as well: the owner completes and durably journals exact bytes,
classifies bounded presence, submits only after stable `NotFound` plus the
one-winner CAS, and projects only later finalized exact evidence; the other role
uses peerless terms-and-transcript discovery.
The typed Core 31.1 adapter and canonical bounded public evidence
codec are GREEN as components and in run-n's actual-node actor paths. Refund
execution, concurrency, production key custody, and the
accepted proposal's full SDK/demo surface remain pending.

Authority was reread again on 2026-07-15: accepted replacement issue #112 is
closed with the `accepted` label and explicitly supersedes issue #61. The live
RFP repository baseline remains commit `969a76d`
(file blob `d0fa52b`) and accepted issue #112, whose newline-normalized body
SHA-256 remains
`49356263a762307abc0f8dd2863ac5af8fe13d9b17b674f242d025de655f1c87`.
Issue #61 remains superseded.

Accepted issue #112 names six explicit M3-specific outputs:

1. update the LEZ escrow for the BTC adaptor/witness-gated claim;
2. deliver the full-lifecycle LEZ/BTC SDK;
3. supply conformance and swap-specific adaptor vectors;
4. document self-hosted/public Bitcoin testnet, wallet, and funding setup;
5. record happy, refund/timeout, and concurrent BTC demos; and
6. explain the Aumayr and Fournier constructions inline.

These six proposal outputs are not the complete acceptance checklist. The
applicable live RFP contracts remain binding, including F2 and F5–F7, U1/U8,
R1–R7, P1, S1–S8, and D1: native/custom assets, taker-first ordering, post-lock
independence, persistence, concurrency, timelock/refund rationale, compute
evidence, CI, tests for every hard requirement, docs, reference integration,
write-up, SDK API documentation, and all three BTC demos.

The proposal's named DLC `AdaptorSignature.md` Schnorr corpus does not exist;
the live DLC corpus is ECDSA. M3 must not fake literal conformance.
[GW-M3-001](proposal-acceptance-errata.md) proposes official BIP-340/BIP-327
vectors, project-owned adaptor positive/negative fixtures, an independent
implementation cross-check, and Bitcoin Core consensus. That substitute is not
yet accepted, so no tag may present it as literal or accepted DLC conformance.

### Progressive local PoC

The owner entered M3 on 2026-07-14. Implement the smallest complete vertical
slice in this order:

1. source/checksum-pin Bitcoin Core 31.1 and boot an isolated, run-owned Regtest
   node with deterministic local funds, cookie RPC, allocated loopback ports,
   exact cleanup, and immutable version/image evidence;
2. construct the exact aggregate internal key, tapleaf/version, Merkle root,
   BIP-341 tweak/parity, output key `Q`, control block, and CSV refund leaf,
   then prove funding and a one-item cooperative key-path spend through Core;
3. integrate a candidate MuSig2/BIP-340 adaptor library without custom curve
   arithmetic; before funding, complete both exact-message adaptor sessions,
   durably reserve/consume one-use nonces, verify and persist both aggregate
   pre-signatures and refund material, and prove signatures under tweaked `Q`;
4. update and deploy the pinned LEZ v0.2 guest with a distinct two-party
   aggregate witnessed-claim authority in both directions—never an actor-owned
   or direct-`claim_adaptor_secret` bypass;
5. add a public, role-fixed BTC SDK, Core adapter, and independent maker/taker
   reference actors with separate configs, keys, stores, and recovery material;
6. run `TakerSellsForeign`, then `TakerSellsLez`, through actual local LEZ and
   Bitcoin nodes and emit secret-safe evidence from both actors and chains.

Progress on 2026-07-15: steps 1 through 4 are closed at the local PoC boundary.
Step 3's cryptographic/Core
sub-slice, fresh-nonce dual-domain boundary, role-local durable signing journal,
restart-safe SDK bridge, independent one-shot role processes, and external
adaptation/extraction are GREEN. Step 4's guest source, digest-pinned artifact,
exact aggregate-account mapping, recursive state effect, durable witnessed
initialize/fund preparation, durable claim preparation/completion, conservative
pair observation, finalized same-block claim/state evidence, and a typed
operator boundary are GREEN. The exact guest was
deployed in finalized block 405 and used for both live directions.
Step 5 now also has a public one-shot, role-fixed reference actor. It activates
the canonical agreement and composes predecessors zero through three using
typed Core or finalized LEZ funding/claim observation, role-owned exact claim
completion, the actor-local public-effect journal, and lifecycle projection.
Its local-input boundary now includes the fixture-only
generate/prepare/finalize provisioner from pushed `a8688a3`. Generation creates
fresh OS-random maker/taker signing, refund, claim-destination, and adaptor
material in owner-only, create-new files and emits only the public planning
document plus the exact pinned `lez-v02-account-id` helper invocation.
`prepare-funding` consumes an actual rawtr service candidate and raw mode-0600
key file, then constructs and cryptographically verifies one exact v2/RBF
funding transaction offline without putting raw bytes or secrets on stdout.
The harness separately proves the Core UTXO and runs read-only
`testmempoolaccept` on the persisted bytes. Finalization verifies those exact
bytes and BIP-341 authorization again, binds a planned next-block funding
anchor plus LEZ deployment/prepared-claim facts and the recovery schedule, then
countersigns and canonically revalidates the agreement before either effect.
The root graph deliberately does not reimplement the Logos `nssa` account
derivation: the isolated pinned helper derives it, and the live pinned LEZ
prepare path independently rejects a wrong authority account. Eleven all-target
tests cover both directions, genuine rawtr signing, public/private crosswires,
malformed or drifted funding, authority/key and recovery drift, strict JSON,
no-clobber, file modes, unsafe links, and stdout secret scanning; strict Clippy,
formatting, and the workspace advisory/license/source policy pass. The
repository-owned harness now composes preparation, node policy, and
finalization plus the public actor flow. Run `m3actor-20260716n` passed that
composition at `6ded2f9`: both maker and taker reached revision four
`Completed` in both directions, offline status returned `complete`, terminal
replay resubmitted nothing, the packet disclosed no private material, and exact
cleanup targeted no foreign Docker resource.
Pushed `ff352b1` also makes identity bootstrap dynamic: schema version 2 retains
each fresh owner account, its official owner-derived Vault account in base58
and hex, and the x-only public key while keeping signer material private. The
stack accepts an owner and its derived Vault override only as a pair, supplies
the owner to genesis, and uses the derived Vault for readiness and Claim.
Both roles and both directions reach revision two `BothLegsLocked` in focused
tests. Both claim revisions project through terminal `Completed`: revision
three reruns the complete activation-material gate and accepts only the exact
related public signature, while revision four accepts only the
direction-correct finalized/confirmed follow-up. The live Bitcoin and LEZ claim
effects are both composed through actor-owned persist-before-presence and
one-attempt authority. An accepted send alone does not project.
Only `activate` may
insert the agreement acceptance. Strict private config schema 2 now requires
the complete prepared-claim result and distinct Bitcoin/LEZ session IDs plus
role-local journals. Activation rederives both exact contexts from the signed
agreement, opens journals existing-only, verifies local identities, phases, and
presignatures, requires and point-checks a private taker-only adaptor scalar
without creating a signature, forbids that authority in maker configs, and
refuses any state creation on run, claimant, request, message, journal, secret,
or context drift. The actor gate is 34/34 library tests plus seven CLI
integration tests; fresh-process coverage also rejects
an explicit null maker authority and scans stdout plus SQLite/WAL artifacts for
raw or hex-encoded scalar disclosure.
The SDK now reconstructs the Bitcoin and LEZ adaptor contexts from the validated
agreement plus caller-supplied fresh session IDs. Ten agreement tests cover both
directions and both domains. This keeps role-runner session JSON outside actor
authority. The same material gate must run immediately before revision-three or
revision-four use because activation alone cannot prevent later file
replacement.
The next RED-GREEN seam batch is now complete. Signer journals open existing-
only, so a mistyped claim configuration cannot create empty signing authority.
The LEZ bridge publicly reuses its official-domain prepared witnessed-message
validator. A new additive SQLite public-effect journal persists the complete
public Bitcoin or LEZ bytes, agreement commitment, and expected effect ID before
a one-winner `Prepared` to `Started` CAS grants the only fresh send. Ambiguous
`Started` or `Unknown` recovery is observation-only, and exact accepted IDs plus
complete observed bytes are required. A chain result that proves presence but
conflicts with the exact durable bytes now atomically burns still-fresh send
authority to observation-only `Unknown` without a transport call; a later
absence cannot rearm it, while timeout/finality uncertainty remains retryable.
Eight focused LEZ actor tests cover both owned directions, both peerless roles,
deterministic full-field requests and a later window, accepted-without-
projection, finalized-only projection, activation reruns, unavailable and
uncertain observations, `Started`/`Unknown` restart, conflicting bytes or
signature, and an out-of-window containing block.
Seven signer-journal, fourteen public-effect, eleven BTC-recovery, and all 86
store tests pass; the bridge-client gate is 2 unit, 26 integration, and 3
example tests. This is the persistence
boundary for revisions three and four; run-n now supplies its complete
two-direction actual-node PoC evidence.
`status` reports absent or precreated-empty/no-acceptance state as
`not_activated`; `drive` returns `NotActivated`. Status may migrate an existing
database schema but creates no acceptance and performs no RPC. Corrupt or
conflicting existing state fails closed. Pre-funding LEZ finalized-observer
errors remain retryable `ObservationUnavailable`, not false absence. Exact
retries keep the deterministic ID and request; a deliberate window change gets
a distinct ID, and the request remains evidence-bound. Observation
returns before the predecessor CAS; valid concurrent revision-one or
revision-two winners are reconstructed without overwrite and other projection
failures fail closed.
The first full-workspace gate exposed an earlier shared metadata constructor
that relabelled genuine NSSA v0.1.2 schema-1 facts as LEE v0.2 schema-2 facts.
The correction makes metadata construction generation-explicit in both
sidecars, the adapter, and the SDK; cross-generation first-lock and claim facts
now fail closed instead of being silently reinterpreted.
Step 6 is 2 of 2 for operator-composed actual chain execution. The public actor
source path is also proved through revision four for both chain directions by
run `m3actor-20260716n`. Both Bitcoin and LEZ public-effect paths now have
actual-node public-actor evidence at the progressive local PoC boundary.
Pushed commit
`a58ef96` adds the checked-in secret-safe packet, complete operator recipe,
exact cleanup proof, synchronized architecture/traceability, and all 76 rendered
Mermaid diagrams. The countersigned agreement binding the executable Bitcoin
refund plan is now GREEN in seven focused tests. Pushed commit `523c64d`
additionally rejects oversized caller-constructed fields before total Borsh
encoding. Actor activation and both typed funding projections are implemented.
Pushed commit `66d352f` adds the live LEZ completion, bounded presence,
one-attempt submission, peerless observation, canonical evidence, and terminal
projection path. Pushed `a8688a3` then closes the post-confirmation agreement
inversion: exact funding authorization and planned recovery are signed before
either effect. Pushed `d777d35` adds the run-owned outer, per-direction, and LEZ
bootstrap drivers, fresh one-shot actor processes, exact child-resource
ownership, and success/failure cleanup. Commits `2233964` and `650d94e`
respectively correct Core 31.1 spender observation to the required options
object and bound each actor-to-LEZ finalized scan to 30 seconds. Run-n then
closed the local PoC certification work with both terminal packets.
Exact-pinned `bitcoin` 0.32.101 constructs and verifies
the P2TR/CSV transaction boundary. Exact-pinned `musig2` 0.4.1 aggregates the
ordered maker/taker fixture keys, applies the Taproot tweak with matching `Q`
and parity, creates and verifies both adaptor partials and the 65-byte aggregate
presignature, adapts a final 64-byte signature under `Q`, and re-extracts and
point-checks the labeled public Regtest scalar.

`TakerSellsForeign` confirmed taker Bitcoin lock
`ca0ae641...a4c75`, finalized maker LEZ init/fund in blocks 540/544,
finalized taker LEZ reveal claim `ef77099e...2cde3` in block 570, and
confirmed the maker's recovered-scalar Bitcoin claim
`0ee99753...6a5aa`. `TakerSellsLez` finalized taker LEZ init/fund in
blocks 617/620, confirmed maker Bitcoin lock `c5dd0f85...752a3`, confirmed
taker reveal claim `66255398...054f4`, and finalized the maker's
recovered-scalar LEZ claim `834c67e9...d3033` in block 644. Both exact
Bitcoin outpoints are spent once, both key-path witnesses contain exactly the
expected 64-byte signature, both LEZ custody accounts end at zero, and both
indexer scans prove exact-once `Finalized` membership plus equal by-ID/by-hash
blocks.

The exact MuSig2 graph is now locked with package-scoped license exceptions and
exercised through rust-bitcoin verification and Core policy/consensus. It
remains an unaccepted dependency candidate: the crate is beta/unaudited,
maintainer-concentrated, exposes cloneable non-zeroizing internal secret types,
and provides no commitment round. The project wrapper now supplies a
transcript-bound commitment round, fresh OS nonce seeds, one-use in-memory
phases, zeroizing retained key/serialized-nonce bytes, and focused
phase/commitment/message/point negatives. The additive journal durably reserves
and consumes serialized nonces and atomically persists exact partial outboxes
with concurrent/restart replay. The SDK now produces and reconstructs those
canonical bytes, revalidates the full context plus both commitments, checks the
secret/public nonce relation, and verifies partials and aggregate
presignatures. The role-runner now exercises that combined boundary across
fresh processes, separate owner-only journals, restart/replay, canonical public
packets, external adaptation, and point-checked extraction. The journal's
plaintext nonce at rest is an explicit PoC nonclaim. Full reference-actor
recovery, encrypted or hardware-backed secret custody, and review remain. Its
`secp256k1` 0.31 types
remain byte-isolated from rust-bitcoin's 0.29 types.

In the `TakerSellsForeign` Bitcoin-leg fixture, the taker policy-checks and
submits the normal 1 BTC funding transaction, Core mines it at height 102, the
maker observes the exact aggregate-key/CSV output, then submits the adapted
0.99999 BTC tweaked-`Q` one-item claim, which Core mines at height 103. The
taker observes the outpoint spent once. This proves MuSig2/adaptor/extraction
and Core interoperability only for one process using public deterministic
fixture secrets. Commitments are computed but not exchanged; no crash-safe
nonce journal, independent signer processes/stores, production authority, LEZ
effect, complete direction, refund, or atomicity is proven.

The first bisectable implementation slice is Core infrastructure only and adds
zero Rust dependencies. It introduces
`tests/e2e/bitcoin-core/{Dockerfile,compose.yml}`, a provenance contract,
`scripts/check-bitcoin-core-isolation.sh`, and
`scripts/run-bitcoin-core-e2e.sh`; then it extends Compose validation, the CI
hardening policy, and CI with an actual-node smoke plus a fail-hard Trivy scan.
The consumed artifact is the signed/checksum-verified official binary archive;
the recorded source commit is provenance and must not be described as a local
source build.

Local run `m3-core-smoke-20260714f` reached GREEN on 2026-07-14. Its runtime
packet proves Core 31.1 and exact Regtest genesis, height 101 under the fixed
600-second mock-time policy, a mature deterministic 50 BTC Taproot fixture,
separate maker/taker allow-and-deny RPC matrices, zero peers, no public runtime
resource, and complete exact cleanup with a foreign sentinel surviving. This
packet was produced before the runner commit and therefore is validation input,
not retained exact-commit certification. Clean pushed commit `a7393dfb` then
reproduced the smoke and exact cleanup as run `m3-core-exact-a7393df`;
[its secret-safe retained summary](evidence/m3-bitcoin-core-smoke-a7393df-20260714.json)
records the packet hashes and preserves the unobserved remote Trivy boundary.

Clean pushed commit `4f7b6b3e` reproduced the P2TR composition as
`m3-p2tr-exact-4f7b6b3`: taker funding txid `c131b09d...227f1` confirmed at
height 102; maker cooperative txid `97799495...51bce` confirmed at height 103;
exact contract address `bcrt1ptee2...r28qgv`; one 64-byte key-path witness;
zero final mempool entries/peers/public resources; and exact cleanup. The
runner enforced `BITCOIN_CORE_E2E_REQUIRE_CLEAN=1`, hashed all critical
source/policy/actor/block/final-state evidence, and emitted a post-cleanup
attestation binding the runtime, manifest, and cleanup packet.
[The secret-safe retained summary](evidence/m3-bitcoin-core-p2tr-4f7b6b3-20260715.json)
records the exact packet hashes and nonclaims.

Strict clean pushed commit `f5a9caa66b04b0bec1a86cb732f5a64f63852e6e`
then reproduced the one-process two-party fixture as
`m3-musig-exact-f5a9caa`: taker funding `7393db97...54ae3f` confirmed at
height 102 and maker MuSig2/adaptor claim `46ba3858...4300ac` confirmed at
height 103. The helper verified the Taproot-tweaked aggregate key, both adaptor
partials, the 65-byte presignature, the adapted 64-byte signature under `Q`, and
the re-extracted public fixture scalar/point before Core accepted the witness.
The clean-worktree requirement, exact cleanup, foreign-sentinel survival, and
hash-bound attestation passed. The
[retained secret-safe summary](evidence/m3-bitcoin-core-musig2-f5a9caa-20260715.json)
records those facts and the one-process, nonce-exchange, durability, LEZ, and
atomicity nonclaims.

Pushed commit `0177151` adds a separate runnable protocol fixture rather than
rewriting that retained Core evidence. `dual-chain-adaptor-poc` constructs exact
BTC and placeholder LEZ 32-byte message sessions over the same adaptor point,
uses separate maker/taker state objects and OS-random nonces, exchanges and
checks commitments before nonce reveal, verifies both peer partials, and proves
that either completed signature reveals the scalar needed to complete the
other. Focused tests reject reveal-before-commitment, commitment mismatch,
nonce reuse, changed messages, and wrong adaptor secrets. Its output explicitly
reports `actual_lez_submission=false`, `durable_nonce_journal=false`, and
`signer_separation=distinct_state_objects`; it is not actual-node corridor
evidence.

Pushed commit `e3f2938` adds the role-local `SqliteAdaptorSessionJournal`.
Immutable session, role, chain domain, exact message, adaptor point, and ordered
keys are fixed before a nonce commitment can leave the process. Peer
commitment persistence gates public-nonce reveal; an immediate SQLite
transaction invokes the pure signing callback once, clears the 97-byte nonce,
and stores the exact partial before returning it. Restart and concurrent retries
return those same bytes without invoking the signer. The store is owner-only
and secret-free after consumption, but nonce encryption at rest and an
enforceable narrow signer/HSM boundary remain production work.

Pushed commit `8a7ea55` adds the restart-safe SDK half of the durable signing
boundary. It generates canonical BIP-327 nonce material from OS entropy,
domain-separates and binds the complete BTC or LEZ signing context, verifies
the local and peer role-bound nonce commitments after reload, checks that the
retained secret nonce derives the recorded public nonce, creates one partial,
and verifies both partials plus the aggregate presignature. This reuses exact
`musig2` 0.4.1 primitives; it introduces no custom curve arithmetic. The
journal/SDK bytes are compatible; the full reference actor and production key
custody remain pending.

Pushed commit `ca524ff` adds the PoC role-runner boundary. Maker and taker use
separate owner-only SQLite journals and fresh one-shot processes for reserve,
commitment acceptance, nonce reveal, partial signing, restart replay, peer
partial verification, and aggregation. Canonical packets bind role, session,
chain context, exact message, keys, and adaptor point. Four integration
journeys cover both LEZ and Bitcoin contexts and reject role/session/message
cross-wires without exposing secret bytes.

Pushed commit `96f2a31` completes that public signing lifecycle with separate
`adapt-presignature` and `extract-adaptor-secret` actions. Both require the
exact journaled presignature and canonical final packet; scalar input/output is
owner-private, create-new, point-checked, and omitted from stdout/errors.

Pushed commit `6935acd` adds `initialize_native_witnessed` and
`claim_native_witnessed` to the checked LEZ v0.2 guest. The guest derives the
official LEZ account from the MuSig2 x-only aggregate key, requires that exact
aggregate account as the sole claim signer, keeps it distinct from the claimant,
and transfers custody only to the claimant. Recursive execution with the
official `lee` account mapping accepts an exact two-party signature and rejects
one share, a signature for another message, a mismatched authority, and the
legacy preimage path. The digest-pinned build reproduces ELF
`a199c5be...e293` and ProgramId `39b6a4db...4dec`. No live LEZ deployment or
cross-chain submission is claimed by this component test.

Pushed commit `79735dd` adds the official-wire bridge from that checked guest
to external aggregate signing. `prepare_witnessed_claim` durably fixes the
official Borsh `Message`, authority nonce, and official hash before exposure;
`complete_witnessed_claim` accepts only the same reservation and an externally
completed 64-byte aggregate signature, verifies it with pinned official `nssa`,
and durably retains the canonical `PublicTransaction`. The existing
`submit_transaction` remains the sole effect boundary. At that checkpoint,
protocol 17/17, client 16/16, and pinned sidecar 47/47 tests passed, including fresh-process completion,
exact replay, conflicting completion, authority/account-order, and transcript
drift. This slice does not claim live-node inclusion or a complete direction.

Pushed commit `3862dde` adds durable witnessed escrow preparation. The LEZ
depositor alone signs the exact generated `InitializeNativeWitnessed` message
with ordered metadata, custody, depositor, claimant, and aggregate-authority
accounts, followed by the separate `FundNative` transaction. Preparation,
restart recovery, and submission remain distinct boundaries.

Pushed commit `f827dad` externalizes the Bitcoin transaction side. The public
fixture emits the exact P2TR spend plan, BIP-341 sighash, tweak/parity and refund
facts plus a canonical `btc_taproot` role-runner session. It accepts only a
verified external 64-byte signature under `Q` before emitting exact raw
transaction, txid/wtxid, and one-item witness facts.

Pushed commit `3d7386b` adds first-class witnessed escrow observation. Exact
depositor mode binds the persisted initialize/fund IDs; claimant discovery is
explicitly window-bounded. Canonical bytes, account order, depositor signer,
aggregate authority/key, metadata/custody facts, and a stable tip must all
agree. Exact misses remain `unknown_or_pending`, and no observation submits or
claims Bedrock finality.

Pushed commits `a3da09e` and `bf5bdbd` add the public orchestration helpers.
The typed one-shot bridge CLI reads strict request/runtime JSON plus a stable
owner-private capability file and prints only result JSON. The public account
helper derives the official LEZ authority account from the x-only aggregate key
through pinned `nssa`; no custom curve or account arithmetic is introduced.

The finalized-observation slices extend the bridge to fourteen methods and
close typed LEZ funding and revealing-claim evidence. A bounded sequential
indexer scan proceeds only when the complete requested window is finalized,
requires immutable `Finalized` blocks to agree by numeric ID and hash and form
one parent-linked ancestry from window start through the stable tip, returns
the canonical transaction once by exact ID or peerless discovery, and reads
historical metadata and custody at the exact containing `BlockId`. Either
agreement participant may observe with its own role-bound sidecar; neither
path submits. The client rechecks the inclusive window and terminal
terms/accounts; claim observation also verifies the aggregate BIP-340 signature
with exact-pinned `secp256k1` 0.29.1. Protocol 23/23, client 2 unit plus 26
integration and 3 CLI tests, and all 78 pinned-sidecar targets are GREEN under
strict Clippy. The upstream
indexer exposes historical account DTOs without an account proof or atomic
multi-read snapshot token, so stable finalized-tip bracketing and same-block
reads are a disclosed trust compensation rather than a production proof.

The complete operator-composed slice is GREEN only when retained, secret-safe
evidence proves Core 31.1,
Regtest genesis and an advancing tip, zero chain peers and zero public runtime
RPC/faucet/funds dependencies, an
allocated literal-loopback RPC port, provisioner-only cookie/wallet/mining
authority, distinct maker/taker `rpcauth` credentials and method allow/deny
matrices, deterministic descriptor-derived mature local funding, immutable
image identity, and exact labeled cleanup while a foreign sentinel resource
survives success and forced failure. Deterministic means the same derivation,
clock/transaction policy, maturity, values, and confirmation assertions; it
does not promise byte-identical blocks or transaction IDs across run IDs. The
new run additionally requires exact LEZ guest deployment/finality, independent
Vault onboarding, both role views of each lock, both complete presignatures
before the first effect, no scalar use before dual locks, exact finalized or
confirmed reveal bytes, point-checked recovery, opposite-chain completion from
persisted state, spent-once Bitcoin outputs, and zero LEZ custody. This accepts
the local happy-path execution, not U8 public-route execution, refund/concurrent
demos, production authority, or accepted-proposal M3 completion.

The PoC gate requires both actors terminal `Completed`, taker-first canonical
at the negotiated policy before the maker effect, no scalar revelation before
both locks, complete durable pre-lock presignature/recovery evidence, and exact
scalar extraction from either finalized LEZ bytes or Bitcoin bytes canonical at
the negotiated confirmation policy, according to direction. The BIP-341
key-path witness is one signature under tweaked `Q` with no script/control
block; the exact BTC outpoint is spent once, recipients are correct, terminal
LEZ custody is zero, post-lock completion needs no counterparty/Delivery/Chat,
and manual commands reproduce the evidence. The agreement commits the exact CSV
refund tapleaf/control block plus maker-funded-shorter/taker-funded-longer typed
recovery schedule even when the happy PoC does not execute it.

The live chain portion of that gate is met in both directions. Commit `a58ef96`
provides the operator guide, immutable secret-safe evidence, exact cleanup
attestation, and current component/actor diagrams. Fresh official LEZ actor
identity provisioning is now GREEN through an OS-random, owner-only,
no-clobber helper whose public account IDs feed the run-owned genesis before
Docker starts. The canonical countersigned agreement now binds the exact
executable BTC refund plan, chain identity/confirmation policy, and recovery
schedule and reconstructs all derived Bitcoin fields before accepting either
signature. Agreement validation now also derives and retains the one fresh
role-neutral `SwapCoordinator`: canonical swap ID, pair, direction, recovery
schedule, direction-correct funded chains, signed Bitcoin confirmation depth,
one finalized-LEZ policy unit, and empty `Offered` state come from the signed
record rather than actor-local mapping. Typed finalized LEZ claim evidence is
now GREEN. The actor-local
Bitcoin recovery component is GREEN in both directions: separate maker/taker
databases replay the four lock/claim revisions to `Completed`, reject mutated
or rolled-back evidence-chain state, and expose the public revealing witness
without persisting the scalar.
The Core component independently reconstructs and cross-checks exact
funding/claim consensus bytes, confirmation/block/tip facts, and a canonical
scalar-free evidence DTO. The actor calls its exact funding and claim observers
and owns one-attempt Bitcoin submission. Run `m3actor-20260716n` exercised both
direction-correct claim paths against actual Core 31.1. Commit `2233964` uses
the required `gettxspendingprevout` options object with `mempool_only=false` and
`return_spending_tx=true`, and verifies exact spender bytes plus block identity
when confirmed.
Peerless finalized LEZ claim discovery is now GREEN: either
role can discover one unique canonical claim from the signed terms and exact
prepared transcript without receiving a peer transaction ID, while absence,
ambiguity, and a conflicting transcript remain distinct fail-closed results.
Finalized witnessed-funding observation is now GREEN without weakening the
existing live-progress observer. The bounded finalized scan proves canonical
funding plus historical `Funded` metadata and exact custody at the containing
block, making finalized funding the explicit evidence input for the implemented
actor claim gate. Protocol 23/23, client
2 unit plus 26 integration and 3 CLI tests, and sidecar 78/78 are GREEN.
The pinned sidecar remains the official transaction decoder and PDA
validator. The actor now binds returned funding accounts and transaction
evidence to the signed agreement before the applicable predecessor-zero or
predecessor-one CAS and retains the finalized tip. The read-only sidecar still does not retain a
funding prerequisite across independent claim methods, so the actor reruns its
complete signer/preparation authority gate immediately before claim projection.
The full actor boundary is GREEN for revisions three and four, including
taker-side signature reproduction, maker-side scalar extraction and point
checking, one-way claim evidence, signed Bitcoin confirmation policy, finalized
LEZ policy units, predecessor CAS convergence, role-owned exact completion,
persist-before-presence, and bounded exact or peerless observation. The typed
live Core observer and durable one-attempt Bitcoin and LEZ claim paths are
wired. The run-owned orchestrator now invokes the verified
generate/prepare/policy/finalize boundary. Pushed `d777d35` supplies the
combined run-owned harness: it creates the actual-node countersigned agreement,
owns host-sidecar listeners and exact child resources, invokes a fresh public
actor process for each command/revision, executes both directions sequentially,
and attests exact cleanup without targeting foreign Docker resources. Commit
`650d94e` makes every actor LEZ bridge request finite at 30 seconds. Run-n
passed this harness at the same pushed commit: both directions, both roles,
revision four, replay without resubmission, and exact cleanup are audited. The
progressive local PoC has no remaining execution task; the next work is the
owner-selected hardening below.

### Later owner-selected hardening

QA begins only after the working PoC and uses RED-GREEN-REFACTOR for malformed
adaptor inputs, wrong message/key/point, nonce reuse, before/at/after CSV,
wrong-key refund, restart, unknown submission, reorg/replacement, fee policy,
concurrency, custom-token parity, post-lock outage, and every traced invariant.
Chaos, information-security, public Testnet4, production RPC/client replacement,
performance, formal review, and release packaging remain separately measured
phases. Public routes must select Testnet4 explicitly; legacy `testnet` is not
an acceptable ambiguous configuration value.

Accepted dependency groups remain Bitcoin Core 31.1 and `bitcoin` 0.32.101: 2
of 5 entry candidates. The exact `musig2` 0.4.1 graph is locked, package-scoped
for CC0/Unlicense, policy-gated, and exercised through Core plus the
commitment-exchanging dual-domain wrapper, but remains an unaccepted
beta/unaudited candidate until independent-process recovery, stronger secret
handling, and review
pass. `miniscript`
13.1.0 and `corepc-client` 0.16.0 remain deferred. Exact `corepc-types`
0.15.0 is now used only for strict Core v31 response DTOs; it is locked,
license-gated, tested, and exercised by the GREEN cohesive actual-node actor
composition. Formal production dependency acceptance and security review remain
part of later hardening rather than a local-PoC blocker.

The P2TR slice now exercises exact-pinned `bitcoin` and `musig2`. The graph
intentionally contains `secp256k1` 0.29 and 0.31; canonical key and signature
bytes cross that boundary and each side reparses them. Completed signatures
must still verify under tweaked `Q` and Core consensus. No global license
allowance or incompatible Rust curve type crosses the boundary.

ADR 0029 contains the milestone component, actor-flow, isolation, and evidence
diagrams. [ADR 0037](architecture/0037-finalize-exact-bitcoin-funding-before-first-effect.md)
records the exact pre-lock funding, planned-anchor,
cryptographic-versus-node proof, output-recovery, and atomicity flow. This plan
now records the progressive M3 local PoC as complete; it does not by itself
authorize an `m3-complete` tag that describes the accepted proposal or
production readiness as satisfied. GW-M3-001 and the later hardening scope need
explicit disposition for such a claim.

## Docker isolation policy

Docker suites must:

- set a suite-specific `COMPOSE_PROJECT_NAME`; the full v0.2 lane must use
  `lez-atomic-swaps-lez-v02-${RUN_ID}`, while existing suites retain the
  `lez-atomic-swaps-${RUN_ID}` prefix;
- avoid fixed `container_name` values and use ephemeral published ports;
- create only project-scoped networks and volumes with identifying labels;
- place per-run data under `.e2e/${RUN_ID}`; the full v0.2 lane owns only
  `.e2e/${RUN_ID}/lez-v02`;
- clean up only with the exact project name/run ID that created resources; and
- never invoke global prune, stop, kill, or volume removal commands.

CI and local scripts fail if the project name is empty or does not start with
`lez-atomic-swaps-`.

## Active risks and open questions

| Risk/question | Current evidence | Owner action |
|---|---|---|
| LEZ proposal file paths drifted (`nssa` became `lee/state_machine`) | Pinned lightweight and native semantic reproducers pass | Retain path checks only as early diagnostics and keep behavior tests authoritative |
| Signature-byte stability is load-bearing for adaptor extraction | Pinned native transaction-equality test preserves the complete signed transaction through block inclusion | Keep the exact equality reproducer required and rerun on deliberate LEZ pin changes |
| Validity windows are checked at block construction, not RPC admission | Repository-owned native test proves a balance-invalid transaction is admitted then excluded during block construction | Allocate inclusion slack and retain the native admission/block reproducer |
| Zcash node migration is active | `zcashd` halts before NU6.3; Zallet omits raw-tx builder RPCs; Zebra's 5.x support horizon ends ahead of NU7 | Use pinned 5.2.0 consensus plus local canonical Rust construction in the vulnerability-clean minimal runtime; re-audit releases and final image before public-testnet evidence |
| SPEL documentation targets older `nssa` paths | The minimal v0.1.2 and provisional exact-PR v0.2 generated programs pass despite documentation/current-`dev` drift | Keep immutable source pins and behavior/artifact tests authoritative; re-audit on upstream merge or tag changes |
| Pinned SPEL guest cannot run on current LEZ testnet | v0.1.2 uses NSSA ABI and `/NSSA/` PDA domain; the separately built v0.2 guest/client now use LEE and `/LEE/`, while PR #238 remains unreviewed | Retain v0.1.2 as lower deterministic evidence, run the checked public-compatible v0.2 runtime locally for M2, contract-test the dormant public configuration, and defer public deployment plus PR review/merge to production readiness |
| Bedrock local image versus public-runtime parity is not published | The exact local image digest has immutable OCI labels binding repository, Apache-2.0 license, and source revision `d8711bbc...`, which matches the LEZ lock; no evidence yet proves that image is the current public-testnet runtime | Bind and verify the OCI labels for local M2, keep live public execution and parity claims fail-closed despite locally admitted exact signed identities, and retain public-runtime parity as a Logos-owned production-readiness question rather than weakening local execution gates |
| Official LEZ v0.2 RPC/type graph contains upstream Hickory advisories | Four graph-local `cargo-deny` gates permit only exact advisory sets. The executable deployer excludes sequencer and generated-wallet dependencies, fixes one HTTPS endpoint, submits once, starts no libp2p/DNS service, rejects DNSSEC features, and records the Logos-owned exception under ADR 0018 | Use these controls for M2 local-v0.2 and dormant public-adapter contract evidence; defer live public execution and retain a production-release blocker until upstream removes the dependency path or a separate security review explicitly accepts it |
| Pinned LEZ and Zcash integration graphs are resolver-incompatible | The Zcash stack pins `crypto-common = 0.2.0-rc.1`; official LEZ v0.1.2 reaches `chacha20 0.10`/`cipher 0.5.1` and stable `crypto-common ^0.2`, so one Cargo graph cannot preserve both evidenced stacks | Implement ADR 0022's separately locked official-wire LEZ sidecar and bounded typed local protocol; never weaken the pins or duplicate official LEZ wire types in the main workspace |
| Upstream LEZ native sequencer tests compile RocksDB and can contend with host work | Clean native lane passes with two jobs in a unique checkout and no Docker/ports | Keep the two-job cap and do not run the heavy lane alongside detected host compilation |
| Mainnet deadlines remain uncalibrated | `public-testnet-v1` fixes testnet depths/horizons and conservative bounds; mainnet is deliberately absent | Gather chain telemetry and fee/reorg stress evidence, then require formal review before enabling a mainnet profile |
| Spend evidence is not yet durable adapter truth | Funding observations are durable and adapter-grade; bounded spend recognition now preserves consensus-valid claim/refund semantics and policy fields, but expected terms are still caller-supplied and spend reorg history is not journaled | Derive expected spends from the concrete agreement plus canonical funding, then add versioned claim/refund removal/replacement persistence before terminal projection |
| LEZ and core timestamp units differ | Typed `UnixSeconds`/`LezUnixMilliseconds` conversion now checks guest multiplication, floors observations, ceils earlier-latest bounds, and passes boundary/overflow tests | Require named profiles to be the only construction path used by composed refund-margin E2E |
| Final E2E must represent actual users | Operator process harness exists; taker and chain lifecycles still call protocol core directly | Extend the role harness through taker CLI and real chain adapters before labeling tests as full E2E |
| Prototype local RPC still uses loopback HTTP and an environment capability | Tower rejects a Bearer header before JSON parsing and non-loopback binds are refused | Move to an owner-restricted Unix socket and credential file before M5 freeze |
| Daemon prototype serializes SQLite with a mutex on blocking workers | Safe for the two-method operator slice, not chain watcher concurrency | Introduce the ADR-0003 single writer actor and atomic outbox before mutations expand |
| Trade direction was unstated in both contractual sources | ADR 0008 now separates taker-first funding from construction-specific claimant order; ZEC's chain order comes directly from RFP F4 | Keep direction immutable; BTC/ZEC allow both only through their reviewed actor/chain flows, while XMR remains LEZ-first only |
| Primary COMIT implementation does not support XMR-first | Pinned commit `dc6ba84…` explicitly ships scriptable-chain-first only | Reject `TakerSellsForeign` for XMR in core and actual CLI; require a new reviewed construction to supersede ADR 0008 |
| Dependency advisories can appear without a source change | The required `cargo-deny` job runs on every push and pull request and explicitly includes `advisories` | Keep advisories hard-failing; investigate and remediate rather than adding broad ignores |
