# Living implementation plan

Last updated: 2026-08-05

This file is the delivery control document. It must change whenever scope,
architecture, sequencing, risks, or acceptance evidence changes.

## Source of truth

1. The live [RFP-003 specification](https://github.com/logos-co/rfp/blob/master/RFPs/RFP-003-atomic-swaps.md).
2. Gateway's accepted replacement
   [proposal #112](https://github.com/logos-co/rfp/issues/112).
3. Actual pinned upstream source and executable behavior, where prose and code
   disagree.

The 2026-08-04 M7 authority refresh pins the live RFP repository at master
commit `bff4bb291fa59fae70cb5310eb78d4e4d566a9a8` and raw-file SHA-256
`a83d0b87ab32e459235a8fea7766519b7fe85ec99d7bcaf1dfe44d329bc3d498`.
Replacement issue #112 is open, retains the `accepted` and `RFP-003` labels,
continues to supersede issue #61, and retains newline-normalized body SHA-256
`593399d667d0187e591f0cb1814e12533830cd05b7fd50ce67ce5bcf672f7cf4`.
The intervening RFP changes alter legal/responsibility and privacy wording but
do not alter the seven accepted M5 outputs. These
immutable source identities anchor this plan; the issue state remains a mutable
upstream fact that must be reread at milestone closure.

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
security, or production-readiness phase has been entered or completed. M2 and
M3 are certified at their reproducible local-functional PoC boundaries; their
QA and later hardening phases remain inactive. M4, M5, and M6 are certified at
their owner-selected local-functional PoC boundaries. The owner entered M7 on
2026-08-04 and directed completion of every repository-controlled item before
raising the formal external-review dependency.
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
| Zcash public-testnet route and funding research | M2 required self-hosted/public node, wallet, faucet, privacy, and flakiness guidance but no supported route was selected | Primary sources select self-host Zebra 6.0.0 with loopback cookie RPC and Tatum's documented API-key-authenticated Testnet Zebrad gateway as the public-provider route. Schema v3 restricts those routes by network and auth kind; the adapter supplies bounded sensitive Basic headers over loopback HTTP or `x-api-key` only to the exact allowlisted Tatum Testnet HTTPS origin. The project-owned signer and role-fixed actors construct and sign exact funding, claim, and refund transactions from separate mode-0600 keys. Local actual-node and nonconnecting public-route contracts are GREEN. Optional Zallet alpha.4 funding and faucet/Discord fallback remain documented. No Zcash Foundation-operated public Zebra RPC was found. | The guide, signer correction, cross-document audit, and exact local repository gates are GREEN. Clean-host public rehearsal, TAZ, rate-limit behavior, and live public evidence are deliberately deferred and unclaimed under ADR 0023; they are production evidence, not a missing repository capability |
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

M1--M3 retain their annotated `m1-complete` through `m3-complete` tags. Under
the progressive-JPEG strategy, M4 has an explicitly scoped annotated
`m4-poc-complete` tag on the exact commit whose local PoC replay, living plan,
review packet, and required evidence prove the PoC exit gates. A later
`m4-complete` tag is reserved for production/hardening closure; tags are never
created for aspirational states. A later fix does not move an existing tag; it receives a
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

## M7 active work package: independent-review readiness and closure

Entered: 2026-08-04. The owner authorized all repository-controlled M7 work and
excluded only performance of the independent external review itself. No
external blocker is to be raised until self-owned implementation, tests,
hardening, documentation and handoff preparation are exhausted.

The exact official `logos-co/logos-docs` template is pinned at commit
`63ecf397ca5dae4b81de85a578ec839a78fec1c0`, SHA-256
`7f5a8507bd98bb54dfe4e1ab8b9e9e3a9bff8f3b64f1d1bbfa508a62fff4ccee`.
ADR 0148 separates repository review-readiness from the independent S12/S13
attestation so neither can be misreported as the other.

### Progressive closure order

- [x] Reconcile the live RFP, accepted replacement issue #112, terms, exact M7
  proposal text and Logos documentation template.
- [x] Create the S7 mainnet-readiness write-up, S12/S13 review scope, finding
  policy and five locally validated documentation packets.
- [x] Close missing U9 operator documentation with a pinned self-hosted Stagenet
  daemon/wallet/funding journey, explicit untrusted public-node option,
  resource/flakiness inventory and no-public-call CI contract.
- [x] Reconcile the exact F1–F9, U1–U10, R1–R8 and P1 inventory against
  current executable evidence. The 28-row TSV now requires a repository-owned
  executable gate, retained evidence, explicit state and concrete remaining
  work; strict mode rejects every self-owned open row.
- [x] Reconcile S1–S13 and D1 against current executable evidence. Their
  14-row manifest preserves S12/S13 as the only `external-review` verdicts and
  strict self-closure currently exposes four repository-owned grouped gaps.
- [x] Close XMR U1/S8 API parity through ADR 0149: a bounded canonical
  Stage-A/Stage-B envelope, role-fixed full lifecycle, retained restart
  identity, actor-owned durability/effects, structured errors, strict Rust
  gates and an external-wiring example.
- [x] Close the automatic Maker route-health component through ADR 0150:
  strict owner-private configuration, hash-pinned bounded semantic commands,
  nonoverlapping periodic process isolation, fail-closed quote/publication,
  deterministic active-offer CAS withdrawal, reserved-negotiation preservation,
  unrelated-route continuity, real-daemon process proof and manual reproduction.
  The component and literal actual-node F1/R3 journey are both certified by
  the checked clean-run evidence below.
- [x] Implement the reproducible F1/R3 actual-node composition runner without
  changing the existing M5/M6 default path: provision and authenticate a unique
  Bitcoin Core 31.1 Regtest node, stop only its exact run-owned container, bind
  one hash-pinned semantic probe to the stopped Bitcoin route and live Zebra
  route, reject the Bitcoin quote, complete the literal Zcash user/actor path,
  and verify route isolation across Maker restart. Contracts and the clean
  pushed execution `m7outage-2c63218-a` are GREEN; F1/R3 now reference the
  checked terminal certificate.
- [x] Close U7 generated-custody ABI drift through ADR 0151: use one pinned
  v0.2 SPEL IDL for deployment and runtime generation; hash both the raw IDL
  and generated Rust client; bind both to the deployment manifest and artifact
  verifier; assert all native, XMR and token account/signer surfaces; and run a
  fast network-free wiring contract in ordinary CI. The runtime sidecar's
  minimally reconciled lock now includes its existing local SDK-core edge; its
  full locked-offline tests, strict Clippy, warning-fatal Rustdoc, advisories,
  bans, licenses and source policy pass. Public deployment remains separately
  deferred and is not implied by this interface freeze.
- [x] Repair the transitive checked-artifact binding found by the fresh M7
  actual-node rehearsal: update the artifact source guard to the custody-frozen
  deployment manifest, rebind the checked-artifact manifest to the runner, and
  make the fast SPEL contract derive and execute both identities. The observed
  stale proof failed before its sole send; a fresh proof and deployment must be
  generated from the repaired pushed commit.
- [x] Close the first XMR semantic-worker custody prerequisite through ADR
  0152: retain the already validated Stage A/B, own/peer packets, private-role
  manifest and private view key; seal them on FDs 211 through 216 before the
  workflow CAS; preserve zeroizing/redacted handling; and deliberately exclude
  stale mutable-journal bytes. Focused RED failed at the child boundary, then
  the full XMR actor suite, strict Clippy and warning-fatal Rustdoc passed.
  This does not close U3/U4/F9: live journal authority and branch-produced
  signature, finalized-evidence and extracted-scalar inputs remain next.
- [x] Close the second XMR semantic-worker prerequisite through ADR 0153: add
  one bounded canonical secret-free child plan on sealed FD 217, bind role,
  invoke/observe mode, step, immutable identities, selected ABI, original
  sending-plan digest, adaptor journal, evidence root and all validated
  loopback RPC origins, and verify the descriptor's seals again in the child
  loader. RED failed on missing FD 217; focused sender/observer tests and strict
  Clippy are GREEN. This binds the live-journal address but does not yet
  implement semantic journal transitions or branch-produced artifact custody.
- [x] Close the first real XMR semantic sender through ADR 0154: retain the
  cryptographically validated private XMR share and expose it only to Tag16 and
  Monero sending steps on sealed FD 218; make no-argument Tag16 load the exact
  Stage A/B, runtime, capability, view key, plan and live Taker refund journal;
  require the durable presignature to equal Stage B; adapt and verify in memory;
  and prepare, complete and submit exactly once through the authenticated local
  sidecar. RED first exposed missing FD 218, then exposed the incompatible
  on-disk capability policy at the sealed-FD boundary. GREEN preserves the
  strict manual capability-file policy while adding one bounded sealed-client
  path. A read-only `Prepared` check now starts a sealed prepare-only child
  before the one-attempt CAS. Rejected or too-early preparation performs no
  complete, submission, evidence write, or workflow transition; successful
  preparation is followed by repinning, CAS, and the existing one-send child.
  Restart states skip preflight and cannot rearm. Tag16 process tests pass 6 of
  6, effect routing passes 7 of 7, and the literal receipt-v2 refund journey
  passes 1 of 1 in 106.26 seconds, including rejected-preflight retry without
  CAS consumption, one accepted preflight, one invocation, observation,
  process-free Complete, and losing-branch exclusion. Actual-node
  replay, semantic Tag14, Monero sweep workers and finalized observers remain
  open.
- [x] Close the safe Tag14 worker-invocation prerequisite through ADR 0155:
  reuse the established exclusive release preparer and release-only service
  instead of introducing a generic sender that lacks the finalized Monero-lock
  prerequisite. Add a no-argument ABI on sealed FDs 220 through 222 for typed
  invocation, release-only capability, and protection key plus already-open
  owner-private journal directory FD 223; validate all inputs
  before journal or RPC use; and retain the original encrypted-journal,
  post-CAS finalized-clock and no-ambiguous-retry semantics in one shared
  routine. The process proof rejects mutable/unsealed inputs at zero wire calls,
  then admits exactly once and observes only after restart. Receipt-v2 authority
  wiring and replacement of the Tag14 marker remain the next semantic step.
- [x] Close the versioned Tag14 authority prerequisite through ADR 0156:
  preserve schema-1 marker semantics, require schema 2 plus the release-worker
  v2 ABI and a complete Taker-only release profile, separate general and
  release-only capabilities, and validate local/exact-public indexer policy,
  journal/key paths and key identity canonically. The focused Taker authority
  suite is GREEN 8 of 8 and the full XMR actor regression stays GREEN. ADR 0157
  subsequently closes sealed at-use custody, pre-CAS invocation and schema-v2
  marker replacement.
- [x] Close semantic Tag14 preflight and receipt-v2 composition through ADR
  0157: add a schema-v2 `preflight`/`invoke` sealed worker mode; authenticate
  the exact journal, key, run/runtime/terms and release-only client without a
  network call or CAS; rederive the invocation from retained validated Stage
  A/B inside the parent; and grant only FDs 220..223. The parent preflights
  while Prepared, then repins before the workflow CAS and one invocation.
  Real worker process proof is GREEN 1 of 1, effect routing is GREEN 8 of 8,
  and the literal claim journey is GREEN 1 of 1 in 164.85 seconds with rejected
  preflight retry, invoke once, observe/reconcile, process-free Complete and
  losing-branch exclusion. A joined actual-node CLI replay, semantic finalized
  observer, Monero claim sweep, and adverse crash/concurrency cases remain.
- [x] Repair the actual F1/R3 probe-custody mismatch found by fresh run
  `m7outage-5e9d47d-a`. Bitcoin Core provenance, semantic health and exact
  container outage passed; Maker then rejected the probe before readiness
  because its repository parent was group writable. No offer, actor or swap
  submission occurred. RED now requires run-private executable staging; GREEN
  installs the source-hashed probe under the `0700` proof root with mode `0500`,
  verifies byte identity, and retains the fail-closed validator unchanged. A
  fresh clean-commit actual-node replay is still required before closing F1/R3.
- [x] Repair the post-restart projection mismatch found by clean run
  `m7outage-f482acd-a`. Both semantic route states survived restart and the
  Bitcoin quote failed closed; application acceptance completed, but handoff
  stopped before actor execution because the legacy assertion expected only
  the Zcash row. RED requires the intentional disabled Bitcoin row to remain
  visible. GREEN applies that two-row invariant only when route health is
  configured and preserves the exact one-row baseline otherwise. No actor or
  chain submission ran; another fresh clean-commit replay remains required.
- [x] Certify F1/R3 through clean pushed run `m7outage-2c63218-a`. The harness
  authenticated and stopped its exact Bitcoin Core 31.1 Regtest container;
  owner-private hash-pinned probes reported Bitcoin unavailable and Zcash
  available before and after Maker restart; the Bitcoin quote failed closed;
  and the ordinary role-correct Zcash application completed through actual
  local Zebra and LEZ nodes. The 36.920-second corridor used zero same-run
  retries and observed confirmed Zcash funding before the revealing LEZ claim,
  then confirmed Zcash claim. The checked certificate binds commit `2c63218`,
  every retained evidence hash, empty runtime external resources and no public
  RPC, faucet or funds. F1/R3 inventory rows are now GREEN.
  The first full quality replay then caught ShellCheck SC2155 in the new digest
  assignments. Declaration and assignment are split without semantic change,
  and the focused contract now requires both digests to become immutable.
  The next gate exposed an older M5 lock assertion coupled to Cargo dependency
  ordering. It now extracts the exact `lez-xmr-swap-sdk` package block from both
  locked graphs and checks the required runtime edges individually, including
  `async-trait` and `lez-swap-sdk-core`, without changing either dependency set.

- [x] Compose the reproducible actual-local Tag-17 PoC without changing the
  claim/refund defaults. A narrow Maker CLI prepares the exact durable
  punishment without submission, then releases only the transaction-ID-bound
  reservation. The isolated runner adds a configurable 120--600 second,
  whole-second local boundary, proves absence against the pre-boundary
  finalized clock, waits outside the guest, performs one release, and requires
  byte-identical Maker exact-owner and Taker terms-discovery finalized facts.
  The focused driver unit test, new executable runner/CI contract, historical
  M4 runner contract, formatting, whitespace and CI-hardening gates are GREEN.
  No public RPC, peer, faucet, public funds or external finality service is
  introduced; the Monero local stack supplies only agreement identity in this
  protocol-transition PoC and no Monero funding effect is claimed.
- [x] Execute the first pushed actual-node RED rehearsal
  `m7tag17124df10a`. The current checked guest passed all five recursive tests,
  deployed with the exact expected ELF/ImageID, and the isolated Maker/Taker
  onboarding, Monero 0.18.5.1 Regtest identity, agreement, Tag13 preparation,
  pre-boundary absence/uncertainty, durable Tag17 preparation and exactly one
  transaction-ID-bound release all passed. The sequencer accepted transaction
  `02f1ae...4597`, but canonical classification never produced a retained
  finality file because the observer rejected `Punish` and the fixed 64-block
  scan waited for its entire future range. Exact cleanup passed, the sentinel
  survived, and independent post-run Docker queries found no run-labelled
  resource. This is RED chain execution, not F5 certification.
- [x] Close the RED observer defect with protocol-first TDD. The shared
  actor-realistic test now proves Maker exact ownership and Taker terms
  discovery for canonical Tag17 bytes, claimant-only signing, ordered
  metadata/custody/claimant accounts, terminal `Claimed` metadata, zero
  custody, no aggregate signature, and rejection before the inclusive
  `punish_at` boundary. The protocol fact constructor independently enforces
  the same timestamp. Actual-node search now advances through contiguous,
  fully finalized eight-block pages only after a typed `uncertain` result;
  this retains full Bedrock finality, cannot skip a height, and bounds local
  post-inclusion wait without treating page size as confirmation depth.
- [x] Execute `M5_XMR_JOURNEY=punish` from exact pushed classifier-fix commit
  `a23a314` against fresh isolated local nodes. Run `m7tag17a23a314a` rebuilt
  and deployed the current five-of-five guest, retained the pre-boundary
  `uncertain` result, performed one transaction-ID-bound release, and found
  byte-identical Maker exact-owner and Taker terms-discovery facts. Tag17 was
  finalized at height 124, 9.877 seconds after `punish_at`, with terminal
  `Claimed` metadata and zero custody under finalized tip 127. Exact cleanup
  passed and the checked secret-free certificate is
  `docs/evidence/m7-actual-tag17-a23a314-20260804.json`. The complete replay
  took 48 minutes 15 seconds; after release, Maker finality took about 75
  seconds and the independent Taker view another four. F5 is GREEN locally.
  F3 and F6 remain open for joined abandonment economics and adverse recovery
  races rather than for missing Tag17 chain execution.
- [x] Add the first application-owned Tag17 authority boundary through ADR 0159.
  Workflow schema v3 adds a Maker-only Punish branch after reconciled Monero
  funding, makes Claim Refund and Punish mutually exclusive, consumes exactly
  one invocation authority, and reconciles completion only from exact finalized
  LEZ evidence. Focused RED/GREEN covers wrong-role and losing-branch rejection,
  restart ObserveOnly, Unknown recovery, and exact replay. Existing schema-v2
  Claim/Refund journals open without migration and retain user version 2. This
  is a durable composition prerequisite; the effect route, sealed worker, joined
  Monero economics and adverse process cases remain open.
- [x] Version the Maker Tag17 effect authority through ADR 0160. Schema 3
  requires an exact `lez_xmr_tag17_punish_v1` tool, while schema-1 Maker and
  schema-2 Taker canonical profiles retain their prior meaning. Missing tools,
  ABI drift and cross-version injection fail before executable or RPC use. The
  focused authority RED/GREEN is complete; ADRs 0161 and 0162 close route and
  semantic-worker composition.
- [x] Route Maker Tag17 through least-privilege sealed descriptors under ADR
  0161. A real Maker application fixture proves non-mutating preflight,
  pin-before-CAS invocation, exactly one command, restart ObserveOnly, finalized
  LEZ observation with the original sending-plan digest, and explicit absence of
  the private Monero share from sender and observer. The semantic no-argument
  Tag17 child was the next RED/GREEN slice and is closed by ADR 0162.
- [x] Close the semantic Tag17 sender through ADR 0162. The no-argument
  Maker worker rejects private-share FD 218 before parsing or RPC, binds the
  exact Stage A/B application and Maker runtime, performs prepare-only preflight,
  submits only the prepared transaction once with transaction-derived identity,
  and writes create-once secret-free evidence. Focused RED exposed the protocol
  field and fixture-reuse assumptions; GREEN passes all nine sealed XMR process
  tests including rejected preflight and forbidden-share zero-call cases. No
  Docker, node, public RPC, faucet, funds or external service is used by this
  process proof; ADR 0158 remains the separate actual-node finality proof.
- [x] Join the normal Maker supervisor to the schema-3 Tag17 recovery route
  through ADR 0163. An explicit queued Refund overrides only the typed Monero
  pre-effect block; the supervisor transfers its existing actor lock without
  reopening it, the actor acquires a distinct workflow lock, preflights before
  CAS, submits once, and on the next cycle observes finalized evidence without
  resending. The real actor process proof reaches durable pending revision 1 and
  terminal refunded revision 2, completes the manual action, and terminalizes
  the process. The proof is local and networkless; the fresh joined two-devnet
  abandonment corridor and adverse recovery matrix remain open.
- [x] Make Maker recovery branch-aware through ADR 0164. The actor reads only
  the validated durable branch: Refund selects the Monero refund sweep with
  invocation-only private-share FD 218, Punish selects preflight plus Tag17,
  and Claim or an unselected branch fails closed. One real supervisor/actor
  process test proves both routes invoke once and restart into observation
  without resending. The semantic Maker refund worker is subsequently closed
  by ADR 0166; the actual-node join and adverse recovery matrix remain open.
- [x] Seal finalized Tag16 evidence for transcript-bound in-memory refund
  extraction through ADR 0165. The Refund sender alone receives the stable
  owner-private signature packet on FD 219 and Maker share on FD 218, both
  pinned before CAS; the restart observer receives neither. The adaptor runner
  verifies the signature against the exact durable presignature and returns an
  opaque in-memory scalar without accepting or writing a scalar handoff file.
  Focused real-transcript and real-process descriptor tests are GREEN. ADR 0166
  subsequently closes the semantic one-shot wallet-RPC child.
- [x] Submit the semantic Maker Monero refund once without confirmation mining
  through ADR 0166. The no-argument child validates the compiled role/mode/step
  ABI, exact Stage A/B and Maker runtime, durable presignature, FD 219 final
  signature, FD 218 retained share, DLEQ reconstruction, three independent RPC
  authorities and exact unlocked accounting. It reads the Maker destination
  from the role wallet and sends once through the shared wallet. The typed
  adapter returns explicitly non-final evidence and the daemon fixture records
  zero calls; restart finality remains a separate observer. Happy and corrupted
  signature process tests plus all affected strict gates are GREEN. The next
  slice is the joined local-node application recovery and observer replay.
- [x] Implement the restart-only Maker Monero finality observer through ADR
  0167. The existing schema-3 verifier ABI now validates the sealed Observe
  plan, Stage A/B, canonical submission and original sending-plan identity,
  explicitly rejects FDs 218/219, and uses the maintained typed wallet/daemon
  verifier. Only bounded non-final states return Pending; semantic, chain or
  accounting mismatches fail closed. Final evidence uses fsynced staging plus
  no-replace atomic publication and must be re-derived and byte-identical on
  replay. Unit and real-process negative tests are GREEN. The fresh official
  Regtest happy-path observation remains part of the joined local-node slice.
- [x] Expose replay-safe schema-3 effect provisioning through the normal role
  actor CLI. The command requires the existing application manifest, immutable
  effect authority, distinct workflow journal, exact run ID and output
  manifest together; it emits only a canonical secret-free summary. The daemon
  already accepts this schema-3 projection, so the actual runner can register
  it from the first activation instead of mutating a registered legacy actor.
- [x] Activate the schema-3 Maker Refund workflow only from exact evidence.
  ADR 0168 adds a no-branch CLI gate that revalidates the application, one-shot
  funding plus independent receipt, finalized Maker-side Tag16 discovery and
  observed signature packet. It imports funding with stable evidence/plan
  digests, publishes the child packet by no-replace exact replay, selects only
  Refund by durable CAS and prepares the semantic sweep. Parser and strict
  all-target compile gates are GREEN; joined actual-node replay remains.
- [x] Wire the exact-evidence activation into the isolated joined runner. The
  opt-in M7 mode starts the real role sidecars before first schema-3 actor
  registration, admits Refund only through the generation-fenced owner CLI,
  supervises the semantic non-mining sender, mines exactly ten confirmations
  through a separate run-owned Regtest driver, and reconciles through the
  spend-authority-free observer. The machine-readable runner contract and
  static control-flow regression are GREEN; an exact pushed-commit replay is
  the next PoC gate and is not yet claimed.
- [x] Exercise the first exact pushed joined attempt `m7refund-d143123-a`. It
  passed cold source/artifact builds, all five recursive guest tests, fresh LEZ
  deployment, role onboarding, official Monero Regtest topology and real Tag13,
  then failed before schema-3 registration or any refund send because authority
  construction bypassed the already validated exported-manifest parser. Exact
  process and Docker cleanup passed. A focused RED contract now forbids raw
  `manifest_value MONERO_*` reads and requires all thirteen validated map keys;
  the minimal parsed-map fix is GREEN. This attempt is diagnostic, not PoC
  evidence, and its one-shot ID must not be reused.
- [x] Exercise the second exact pushed joined attempt `m7refund-a5fe34b-a`.
  The parsed-map fix removed all thirteen missing-key errors. The run again
  passed cold builds, all five recursive guest tests, fresh LEZ deployment,
  onboarding, official Monero Regtest, Stage A/B and Tag13, then stopped before
  schema-3 registration at replay-safe effect-authority validation. Exact
  cleanup passed. Source inspection against the retained identities and
  manifest proved the authority was emitted by pretty-printing `jq -n`, while
  the Rust boundary intentionally accepts only compact canonical JSON plus one
  newline. A focused RED runner contract now requires canonical emission; the
  minimal `jq -cn` fix is GREEN with the full runner contract, Bash syntax and
  diff hygiene. This diagnostic ID must not be reused. The next gate is a fresh
  exact-commit replay through registration, funding, send, external local
  confirmation mining and restart-only terminal observation.
- [x] Exercise the third exact pushed joined attempt `m7refund-3e513ab-a`.
  Both prior fixes held: schema-3 provision, application replay, real Monero
  funding and verification, the signed refund window, Tag16 submission and
  finalized Maker discovery all passed. Activation then failed before branch
  selection or any refund send while revalidating the immutable Maker
  application. The supervised path had opened the original byte-pinned adaptor
  SQLite journal through the legacy ingestion helper immediately beforehand.
  ADR 0169 splits custody: schema 3 passes the already-created canonical Tag16
  packet to activation, which validates it against finalized facts and places
  it create-new in effect custody; only the legacy M5 route retains journal
  ingestion. The focused RED/GREEN runner contract, Bash syntax, legacy
  ingestion tests and all ten effect-route tests pass. Exact cleanup passed and
  this diagnostic ID must not be reused. A fresh pushed replay remains the PoC
  gate.
- [x] Exercise the fourth exact pushed joined attempt `m7refund-8f836c7-a`.
  It passed all cold source and artifact builds, five recursive guest tests,
  fresh finalized LEZ deployment, both actor onboardings, official Monero
  Regtest, schema-3 registration, real funding and independent verification,
  the signed refund window, finalized Tag16 and evidence-driven activation.
  The semantic Maker child then submitted Monero refund transaction
  `b34c5fcbde4e9f7c8617e6e2286f7aad8230fa8253fd67b50f1f437dcc02ff0e`
  once and published its create-new receipt. The runner did not invoke its
  separate ten-block driver because it required a transient queued supervisor
  state while the 20-millisecond daemon poll exposed leased or backoff states.
  ADR 0170 gates the driver on the durable validated receipt plus the same
  active Refund action across queued, leased or backoff replay-safe states.
  The focused RED/GREEN contract, Bash syntax and diff hygiene pass. The run
  was interrupted only after the race was established; exact cleanup passed
  with source status 130 and no foreign resource targeted. This diagnostic ID
  must not be reused. A fresh pushed replay remains the terminal PoC gate.
- [x] Exercise the fifth exact pushed attempt `m7refund-e7016d8-a`. Durable
  receipt detection admitted the separate driver, one real Maker refund was
  sent, and exactly ten official Regtest blocks were mined. Restart observation
  then failed because the schema-3 manifest treated mutable SQLite page bytes
  as immutable. ADR 0171 keeps that digest as provisioning provenance but
  revalidates complete session semantics on every restart. Its real `VACUUM`
  RED/GREEN, complete XMR actor suite and normal-supervisor process proof pass.
- [x] Exercise the sixth exact pushed attempt `m7refund-d6ebaaf-a`. The joined
  path completed one semantic Maker wallet send, exactly ten local confirmation
  blocks, restart-only wallet and daemon finality, workflow revision 2,
  completed manual Refund, terminal scheduler state, and exit-status-zero exact
  cleanup. This closes the functional PoC defect. Post-run audit found that the
  validated finality receipt was correctly removed with its private effect root
  instead of being retained for review; this run is functional diagnostic
  evidence and its ID must not be reused for certification.
- [x] Close the retained-finality RED under ADR 0172. The runner now requires an
  exact secret-free schema, writes an owner-private `O_EXCL` staging file,
  fsyncs, publishes without replacement, unlinks staging, revalidates one link
  and byte equality, then performs exact cleanup. The focused filesystem test
  proves the happy handoff and replacement rejection; shell syntax and diff
  hygiene pass.
- [x] Execute fresh pushed-commit replay `m7refund-7cd3a9c-a`, retain
  `evidence/monero-refund-finalized.json`, terminal monitor, phase ledger and
  cleanup together, and check in the exact-hash secret-free certificate
  `docs/evidence/m7-actual-maker-refund-7cd3a9c-20260805.json`. The receipt
  survived cleanup as mode `0600` with one link; the private source and every
  exact run-labelled Docker resource were absent afterward.
- [x] Run the complete pinned CI quality suite after certificate publication.
  RED first found ShellCheck SC2034 in the dynamically scoped retention fixture
  and a stale M5 contract that still required one literal 3600-second handoff.
  GREEN exports the fixture root and proves the production 3600-second default,
  isolated M7 one-second override, exact daemon/evidence propagation, and both
  ordered pre-Tag13 and post-sidecar handoffs. ShellCheck, Actionlint, Hadolint,
  Compose validation, all M3/M5/M6/M7 contracts, certificate checks, fuzz,
  SPEL ABI and nonconnecting public-guide route tests pass.
- [ ] Close repository-controlled SDK, application, graceful-degradation,
  restart/concurrency, timelock/fee/reorg and demo gaps found by that audit.
- [ ] Run the post-PoC QA RED-GREEN-REFACTOR matrix, bounded chaos/fault matrix,
  information-security review and production-readiness review.
- [ ] Produce one immutable candidate with source, dependency, artifact,
  evidence, threat/atomicity, command, SBOM/vulnerability and cleanup manifests;
  replay all safe local role journeys from that exact commit.
- [ ] Push and mark the repository-controlled review-ready candidate only after
  every local gate is GREEN. Do not tag M7 complete without external S12/S13.
- [ ] Then raise the first external dependency: mutually agree and engage a
  reputable reviewer. Process subsequent external blockers one at a time.

### Current repository-owned gap classes

1. Traceability contains carried Partial/Planned statements from earlier PoC
   boundaries, including accepted-application actual-chain concurrency,
   crash/reorg/fee matrices, public calibration alternatives and non-BTC demo
   evidence.
2. CI has strong lint, strict Rust, vulnerability, license, source, fuzz,
   architecture and isolation gates, but does not yet produce one review dossier.
3. Public deployment remains intentionally skipped under the stealth/local
   policy. Configuration portability, deployment instructions and honest
   release blockers remain mandatory; public transaction evidence does not.
4. Logos-owned upstream limitations remain nonblocking for milestone
   implementation but release-visible. Repository findings and proposal errata
   are not eligible for that exception.
5. The receipt-v2 XMR process boundary now carries immutable application
   inputs, a canonical execution plan and least-privilege branch material. The
   real Tag16 child derives its signature from the Stage-B-matching live journal
   and submits through an authenticated local sidecar. Literal Tag14 and Tag16
   CLI integration are process-GREEN; Tag17 is semantic-process, actual-node,
   and normal Maker-supervisor GREEN through ADR 0163. ADR 0164 also makes the
   application choose durable Refund versus Punish and proves both one-shot
   process routes. Complete joined claim,
   refund, and abandonment corridors plus the semantic Taker claim sweep, actual-node join, and adverse restart/concurrency remain repository work; ADR 0166 closes the Maker refund sender.

The working ETA will be recalculated after the hard-requirement audit because
the carried matrix mixes completed evidence with historical gaps. The initial
range is 5-10 focused workdays for repository-controlled review readiness,
excluding actual-node cold-source outages and third-party review calendar time.

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
- [x] Publish step-by-step self-hosted and public Zcash-node routes, including
  configuration, transparent wallet creation, and obtaining testnet funds.
- [x] Document transparent-pool visibility and the shield-after-swap user journey.
- [ ] Record happy, refund/timeout, and concurrent-swap demos from passing
  testnet actor suites.

RFP R2/D1 reconciliation during M3 exposed a recovery branch that the M2
happy-path packet and the broad refund-demo checkbox above did not state
precisely:

- [ ] add the corrective ZEC **first-lock-only absent-maker** actor journey in
  both accepted directions. After the taker locks the direction-correct first
  leg and the maker never submits the second lock, the taker must recover that
  first leg from persisted state and canonical LEZ/Zebra evidence alone;
- [ ] bind a direction-correct maker-second-lock cutoff that leaves the reviewed
  finality and reaction margin before first-leg recovery, reobserve the first
  lock as canonical and unspent on every maker attempt, and fail closed across
  the cutoff/refund race.

This is neither the existing two-lock timeout path, where both legs already
exist and refund in the fixed LEZ-before-ZEC order, nor the concurrent-swap
demo. It is a corrective accepted-delivery gap, not a retroactive claim about
the narrower `m2-complete` happy PoC tag, which already excludes actual-node
recovery and later hardening. It must be closed before the complete RFP
delivery is certified.

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
  `fe8ec1166ec886693d1fcd1d1ddc80090f81f6fab941851cce43b5bfb0c739f7`
  with ImageID
  `5421868ee00d213bf083c09f14ed09f303e8581b95b3a17bb9b79f6cb44add62`.
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
  not composed actor-corridor or public-v0.2 evidence. Earlier runs retained the
  false custom-`getProgramIds` assumption and empty-channel contract as RED.
  Exact remediation run `m5-ruint-v012-final-20260731`, using builder
  `r0.1.94.1`, then passed six ordinary tests, two actual deployment/native-plus-
  two-token lifecycle tests, and one recursive cost case with the schema-v2
  transaction/block proof. It reproduced ELF `fe8ec116...c739f7` and ImageID
  `5421868e...add62`. The final stable-cost policy passed the exact generated
  output while preserving immutable identity, topology, totals, budgets, and
  classification arithmetic; the former byte-identical volatile snapshot is
  superseded.
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
deferred until owner transition**. All six issue-#112 M3-specific outputs,
including the literal three-video D1 output, and the underlying actual-node
evidence are implemented at the private functional-PoC boundary. The private
video bundle is GREEN at `7697a27c...f101ba8`; exact closure commit
`f7fb250f...dcbb2` is certified and published under `m3-complete`. The
pushed claim packet
certifies both happy directions. Fresh run
`m3refund-20260716h` now also closes the separate actual-node two-lock
timeout/refund demo in both directions. Run `m3firstlock-20260716h` closes both
refund-side first-lock absent-maker paths. Clean pushed-commit run
`m3survivor-20260716c` closes direct post-reveal continuation in both
directions. Canonical maker-lock containing-time enforcement is GREEN at
`3d202f7`. Pushed commits `4fb6950` and `79d7e68` close the typed Core
unspent/submission port, role-fixed exact-plan facade, current LEZ first-lock
state proof, and durable ordered Maker intent with atomic revision-two close.
Pushed `8870910` closes strict schema-4 typed actor/port integration: direction-
shaped exact material reconstructs the `BtcPairSdk` plan, the actor observes
before each one-attempt send, rechecks cutoff and first-lock eligibility, and
atomically closes exact final observation with revision two. Schema 3 remains
legacy observation-only compatibility with zero attempts. The focused gate is
77 of 77 GREEN (69 library plus 8 CLI integration), with strict Clippy,
rustdoc, formatting, and diff checks GREEN. Pushed commits `5102046`,
`2b2781b`, `f40cf5a`, and `13d048b` close the authenticated stable-current LEZ
clock, exact finalized/current first-lock joins, mutation matrix, and concrete
schema-4 live Maker-lock port. Pushed `6c8e459` and `9b2bce2` move both Maker
second locks under the actor while leaving only the Taker first lock as an
external runner submission. The binary-aware orchestration contract is GREEN.
The secret-safe actual-node schema-4 admission packet is GREEN in both
directions at run `m3schema4-20260717d`. Clean pushed-commit run
`m3overlap-20260717a` now closes the opposite-direction overlapping-swap
execution gate at a simultaneous revision-two barrier. LEZ v0.2 cannot prove pending-level initialization
absence. Pushed `3336b6e` adds a distinct journal
`ExactIdempotentSubmissionSafe` observation that may grant one CAS/send only
for an adapter/node operation bound to the same exact ID and bytes; it is not
absence, never rearms `Started` or `Unknown`, and still requires canonical
evidence for acceptance. Store-focused tests/gates and run D prove the live
exact-idempotence contract. Pushed `11111dd` maps the
distinct observation through `MakerLockStepChainObservationV1`; its actor test
submits once on the first drive and zero times after restart. This is typed
no-rearm evidence, not live adapter/node composition. Pushed `923586b` generalizes
the read-only LEZ current-state proof to the agreement-selected escrow in either
direction and for either role. It proves current `Funded` metadata and complete
custody under one unchanged canonical clock, but explicitly does not prove
finality or exact initialize/fund transaction bytes.

Actual-node attempt `m3schema4-20260716b` passed checked LEZ deployment,
fresh-identity Vault bootstrap, the Taker Bitcoin first lock, one actor-owned
LEZ initialization send, restart without resubmission, and exact finalized
transaction proof. The official sidecar then returned typed `moving_tip` while
classifying that finalized initialization under a live advancing Bedrock tip.
No lifecycle projection or funding send occurred; the Maker remained at
revision one and cleanup attested that every exact-run resource was absent
without targeting foreign resources. Pushed `dc07518` adds a bounded
typed-error-only fresh-actor retry across all five LEZ Maker-lock drive phases.
Each failure must have empty stdout and keep the durable submission count within
that phase's one-send bound; success must reach its exact target count. Any
other error, ambiguous stdout, or excess effect still fails closed.

Actual-node attempt `m3schema4-20260716c` crossed attempt B's boundary,
completed the full actor-owned LEZ Maker pair and the `TakerSellsForeign`
claim direction, then finalized both Taker-owned LEZ first-lock transactions
for `TakerSellsLez`. The Maker's fresh eligibility classification returned
typed `moving_tip` before Bitcoin intent creation or broadcast. Both actors
remained revision one, the Bitcoin Maker-lock intent/step tables remained
empty, the Bitcoin mempool had no Maker effect, LEZ retained exactly its two
Taker effects, and exact-run cleanup again passed without targeting foreign
resources. Pushed `cd93fb9` adds the Bitcoin-specific sibling reconciliation:
each fresh actor attempt requires the LEZ count to remain exactly two and the
mempool to be either empty or exactly the planned funding txid; success and
accepted restart require exactly that single txid. The RED/GREEN matrix covers
pre-send moving-tip, ambiguous post-send convergence, restart, non-typed error,
nonempty failed stdout, foreign mempool content, and LEZ-count drift.

Clean pushed-commit run `m3schema4-20260717d` passes the full schema-4
two-direction claim journey at tested `origin/main` commit `0e7635f`. Only the
Taker first lock is externally submitted; the Maker actor submits the LEZ
initialize/fund pair in `TakerSellsForeign` and the exact Bitcoin funding
transaction in `TakerSellsLez`. Each direction reaches revision four
`Completed` for both roles with exactly 2 confirmed Bitcoin effects and 3
durable LEZ effects. Restart and terminal replay add zero effects. Run D
exercised nine typed moving-tip reconciliations before the Bitcoin Maker send;
the LEZ count stayed exactly two and the mempool remained empty until the one
planned transaction appeared. Cleanup removed every exact-run resource and
targeted no foreign resource. The secret-safe retained packet is
`docs/evidence/m3-schema4-actor-owned-lock-poc-20260717.json`; its contract
explicitly leaves accepted-M3 completion, production readiness, concurrency,
SDK/F7, recordings, documentation closure, and final gates open. Attempts B
and C remain diagnostic evidence, not milestone acceptance evidence. The

Pushed `1e6d5f1` replaces the sequential runner's chained-change Bitcoin
fixture with two direction-private mature coinbase outpoints and adds an
opt-in deterministic overlap schedule. Clean run `m3overlap-20260717a` at that
already-pushed commit passes both directions on the same Core/LEZ topology.
Both controllers and all four actor stores reached revision two
`both_legs_locked` before either settlement was released. The retained packet
binds distinct inputs, agreements, actor databases, signer sessions/journals,
escrows, deadlines, and pairwise-disjoint effect IDs; each role then reached
revision four and replay added zero effects. This closes the accepted
opposite-direction overlap checkpoint without claiming simultaneous RPC
mutation, arbitrary-N scheduling, same-direction LEZ nonce scheduling, public
deployment, or production readiness.

The canonical countersigned agreement, finalized LEZ funding/claim
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
actor-local Bitcoin recovery store is also GREEN through revision four and offline `Completed` reconstruction, plus the alternative ordered maker/taker timeout branch to offline `Refunded`. The reference actor now projects exact
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
codec are GREEN as components and in run-n's actual-node actor paths. The
schema-3 public `recover` command now composes the deterministic Bitcoin and LEZ
refund boundaries in both directions. Run `m3refund-20260716h` now proves those
two-lock timeout paths and `m3firstlock-20260716h` proves both first-lock-only
absent-maker refund paths against actual local nodes. Survivor recovery is
clean GREEN in `m3survivor-20260716c`; concurrency, production key custody,
and the accepted proposal's full SDK/demo
surface remain pending.

The first post-PoC RED-GREEN-REFACTOR loop is now GREEN at the canonical
Bitcoin transaction and agreement boundary. Exact-pinned `bitcoin` 0.32.101
constructs a one-input BIP-342 script-path refund with the agreement CSV
sequence, exact funding prevout/value, default tapleaf sighash, funder-key
verification, and signature/script/control-block witness. The shared
claim/refund value validator rejects null outpoints, MAX_MONEY overflow,
overspend, and zero fee identically. A validated countersigned agreement
derives the refund only to the direction-selected funder's already signed
role-owned destination, using the signed cooperative fee as the deterministic
version-one baseline. Focused tests cover both directions, wrong keys, changed
outputs, and exact witness shape. The typed Core adapter now proves
signed-anchor deadline eligibility and exact one-send/readback semantics. The
actor now requires the agreement-selected Bitcoin funder's lowercase-hex
mode-`0600` refund-key file, forbids it on the other role, and rederives the
countersigned x-only key before activation. Fresh actual-node two-lock timeout
evidence is GREEN in run `m3refund-20260716h`. The first-lock rev1-to-rev2
projection, signed cutoff agreement, finalized LEZ `Found`/`Absent`
classifier, and refund-side live gate are GREEN. Run
`m3firstlock-20260716h` closes actual absent-maker execution; live maker-lock
admission at the cutoff boundary remains the active race-safety slice.

Actual-node attempt `m3firstlock-20260716b` reached checked LEZ deployment,
both finalized Vault Claims, and direction planning, then failed before any lock
broadcast because the stage-two jq program referenced an unbound cutoff
variable. Its exact cleanup attestation is GREEN. A focused contract guard
reproduced the RED and now requires the shell value to be explicitly bound as
`maker_cutoff` before the JSON agreement uses it. This failed run is diagnostic
evidence, not M3 acceptance evidence; the certifying rerun uses a fresh ID.

Attempt `m3firstlock-20260716c` crossed that boundary, finalized the taker
Bitcoin first lock, and then correctly withheld LEZ absence authority because
the current finalized LEZ timestamp had not yet crossed the signed cutoff. Its
exact cleanup attestation is GREEN. The initial RED-GREEN guard required a bounded 240-sample finalized-tip wait,
revalidated exact finalized block identity on each usable sample, and retained
the wait count. RPC uncertainty still could not become absence. This attempt is
also diagnostic rather than acceptance evidence.

Attempt `m3firstlock-20260716d` crossed both prior fixes and confirmed the taker
Bitcoin first lock. Its 60-second finalized-tip wait then failed closed: the
signed cutoff was `1784205779`, while the last usable finalized LEZ block was
height 36 at `1784205765508` milliseconds, 13.492 seconds before the cutoff.
Bedrock continued producing/finalizing, so this measured local finality/indexer
lag invalidated the harness bound, not the signed atomicity invariant. The next
RED-GREEN guard extends only that bound to 1200 quarter-second samples (five
minutes); every usable sample still revalidates exact finalized block identity,
and uncertainty still cannot become absence. Exact cleanup is GREEN. This is
diagnostic evidence, not acceptance evidence.

The same run exposed repeated best-effort pinned-Bedrock UDP NTP attempts to
`pool.ntp.org:123`, all observed as timeouts. No public chain RPC, peer, faucet,
deployment, or public funds participated, and certification does not depend on
NTP success. The contract and final evidence now distinguish an attempted
external time-sync request from an external success dependency and record the
observed timeout count.

Attempt `m3firstlock-20260716e` crossed the new bound, finalized the taker
Bitcoin first lock, proved both fresh LEZ absence windows beyond the signed
cutoff, and reached refund admission. It then failed before refund broadcast
because the native observation path selected JSON numbers from the
agreement-bound LEZ amount, whose canonical full-width `u128` wire form is a
decimal string. The CLI rejected the resulting empty `--amount`; no ambiguous
submission occurred. Exact cleanup is GREEN. A focused RED now requires a
nonzero canonical decimal-string extraction and forbids the lossy numeric
selector; GREEN preserves the signed wire format and delegates the final
`u128` range check to the typed native-escrow CLI. Run E remains diagnostic,
not acceptance evidence, and the certifying rerun uses a fresh ID.

Attempt `m3firstlock-20260716f` completed the actual-node
`TakerSellsForeign` first-lock refund through both revision-two `Refunded`
roles and zero-submission replay. In `TakerSellsLez`, it finalized the taker
LEZ first lock, remained pending until the signed later deadline, crossed that
deadline in finalized LEZ time, and advanced stable Core median time through
the maker cutoff. Its first corroborating admission read then failed before
refund authorization because the runner reused the hashlock-only native escrow
observer against aggregate-witness metadata. Exact cleanup is GREEN and no
ambiguous refund submission occurred. A focused RED now isolates that
hashlock-only absence probe to `TakerSellsForeign`; GREEN reuses the existing
authenticated sidecar's exact, bounded, finalized witnessed-funding observer
for `TakerSellsLez`, binding the persisted funding transaction, original
finalized lock window, aggregate authority, canonical full-width amount, and
historical funded custody. Run F is diagnostic, not combined acceptance
evidence; the certifying rerun uses a fresh ID.

Attempt `m3firstlock-20260716g` again completed the actual-node
`TakerSellsForeign` first-lock refund and reached the independently finalized
`TakerSellsLez` refund deadline with no maker second lock. Its first exact
witnessed-funding admission read failed closed with the sidecar's typed
`moving_tip` result while the local finalized tip advanced. No refund was
submitted, and exact run-owned cleanup is GREEN. This is a read-only
availability race, not absence authority and not acceptance evidence. The
active RED-GREEN slice requires a bounded retry of only that typed observation,
with a fresh request ID per attempt, empty failed stdout, retained diagnostics,
unchanged durable effect count, and immediate rejection of every other error.
The certifying rerun will use a fresh ID.

Run `m3firstlock-20260716h` is the clean pushed-commit GREEN rerun. Both
economic directions used fresh Core 31.1 Regtest and private LEZ v0.2 nodes,
fresh one-shot actors, separate role stores/journals, and a maker that remained
offline after activation through refund finality. The Bitcoin-first direction
retained exactly lock plus refund and no LEZ effect; the LEZ-first direction
retained exactly initialize, fund, and refund and no Bitcoin effect. Both
fresh makers reconstructed terminal revision two `Refunded`, terminal replay
added zero effects, and exact cleanup targeted no foreign resource. Its second
witnessed-funding admission sample converged after three typed `moving_tip`
retries with fresh request IDs and unchanged durable submission count. The
secret-safe retained packet is
`docs/evidence/m3-local-two-direction-first-lock-refund-poc-20260716.json`.

The refund-wire loop is GREEN. The existing native-refund RPC names and
hashlock JSON shape remain unchanged, while strict untagged protocol envelopes
accept either `NativeEscrowTerms`/metadata or the M3
`WitnessedNativeEscrowTerms`/metadata. No variant discriminator is added.
Each inner type retains `deny_unknown_fields`, so mixed secret-digest and
aggregate-authority requests or account facts fail closed. The v0.1.2 sidecar
explicitly accepts only the hashlock variant; all of its targets still compile.

The following v0.2 planner RED-GREEN-REFACTOR loop is now GREEN as well. It
builds the exact official `RefundNative` public transaction with ordered
metadata, custody, and immutable depositor accounts, and with no nonce or
witness. Complete role, runtime, program, destination, and witnessed-authority
bindings are revalidated before an owner-only durable exact-byte reservation is
created. Identical restart replay returns those bytes without a nonce RPC; a
distinct request or any account, instruction, nonce, witness, ID, signer,
program, or aggregate-authority mutation fails closed. The generic submission
boundary admits only the retained reservation through a dedicated unsigned
decoder. ADR 0038 records why preparation alone does not prove deadline
eligibility or authorize a send. Authenticated prepare/restart replay and
finalized witnessed refund observation are now GREEN. The actor-local
recovery store also replays typed maker-refund revision three and taker-refund
revision four evidence to terminal `Refunded`, with an atomic migration that
preserves old happy-path payload bytes. The observer rejects pre-deadline
inclusion, exposes stable
state-only clocks at deadline minus one and at the deadline, accepts only exact
canonical unsigned bytes in fully covered finalized ancestry, proves historical
and tip Refunded state with zero custody, and keeps observation repeatable and
no-submit. The typed Core refund adapter now validates the signed funding anchor, exact next-block CSV boundary, canonical three-item witness, conflict and
early-inclusion cases, one broadcast, exact post-send txid/wtxid readback, and
finalized containing-height evidence. The same readback closes a shared claim
race. The schema-3 actor composes this with the LEZ planner and observer through
public `recover`: rev2 to rev3 refunds the maker-funded leg, rev3 to rev4
refunds the taker-funded leg, and both directions reach `Refunded` in
deterministic role tests. Exact public bytes are durable before the one-winner
CAS and only one attempt; `Started`, `Unknown`, and `Accepted` are
observe-only, and only later exact finalized evidence projects.

Run `m3refund-20260716g` supplied the final fail-closed RED: its first
two-lock direction reached revision four `Refunded` for both actors with zero
replay effects, while the second made and finalized each exact refund once but
failed after 120 bounded nonowner `moving_tip` reads instead of projecting
terminal state. Exact cleanup passed. The correction pins the terminal-only
finalized prefix while still rejecting moving or inconsistent coverage; its 16
of 16 focused cases and complete pinned sidecar suite are GREEN.

Fresh successor run `m3refund-20260716h` closes the actual-node two-lock
timeout/refund gate. Its packet at
`.e2e/m3refund-20260716h/m3-actor-poc/evidence/m3-actor-local-poc.json` reports
`passed` at `2026-07-16T10:22:51Z`. In both `TakerSellsForeign` and
`TakerSellsLez`, independent maker and taker actors reached terminal revision
four `Refunded` and offline `complete`. Each direction has exactly two unique
confirmed Bitcoin effects (funding plus actor-owned script-path refund) and
three exact durable LEZ submissions (initialize, fund, and actor-owned native
refund), with no cooperative claim effect. All four terminal `recover`
invocations returned without changing revision or phase; before/after effect manifests are
byte-identical and the measured replay resubmission count is zero. The terminal
command currently labels that no-op outcome `not_yet_composed`; certification
uses the unchanged terminal status and exact zero-effect comparison rather than
calling that label `complete`.

The same packet binds Bitcoin Core 31.1 Regtest and private local LEZ v0.2.0,
the three executable runner hashes, the two stage-two packet hashes, offline
native-build prerequisites, and the selected local timing profile. It records
no public RPC, faucet, peers, or funds. Exact-ID cleanup removed every run-owned
container, network, volume, image, and secure reservation state without broad
cleanup or a foreign target. The run used base repository commit `ef5f306` with
a dirty worktree; the exact runner scripts are hash-bound, but this is not
described as clean pushed-commit evidence. A checked-in secret-safe summary
must retain these facts and public transaction/block IDs and hashes while
omitting private state, credentials, capabilities, raw signing material, logs,
and full raw transaction packets.

The local stack now makes its timing profile explicit. Happy claim evidence
uses the audited upstream-compatible `1.0`-second LEZ slot duration. Refund
certification uses the runner-owned, tightly allowlisted, manifest-recorded
`3.0`-second local slot duration so a loaded host has a stable observation
interval; it does not weaken deadlines, finality checks, or stable-prefix
validation. Both profiles run the same pinned Bedrock, sequencer, indexer,
guest, sidecars, and actor code, use only isolated loopback endpoints and local
genesis/Regtest funds, and retain the selected duration in evidence. Public LEZ
timing remains deployment configuration and requires fresh proof rather than
assuming local cadence parity.

Active M3 refund critical path:

- [x] derive and verify the agreement-bound canonical Bitcoin CSV refund;
- [x] extend the refund wire without changing legacy hashlock JSON;
- [x] durably prepare and restart-restore exact official LEZ v0.2 refund bytes;
- [x] register authenticated prepare and restore its successful canonical
  request/result before accepting traffic;
- [x] prove stable finalized pre-deadline rejection, deadline eligibility, exact
  `Refunded` metadata, zero custody, immutable depositor, and bounded absence;
- [x] register the authenticated finalized observe method without broadening
  generic submission;
- [x] replay typed maker-refund revision three and taker-refund revision four
  evidence to terminal `Refunded`, preserving exact legacy payloads on migration;
- [x] require affirmative stable refund eligibility for the sole `Prepared` to
  `Started` CAS; absence is invalid for refunds and races yield one winner;
- [x] observe Bitcoin maturity from the signed funding anchor and next-block CSV
  boundary, accept one send only after exact txid/wtxid spender readback, and
  encode finalized evidence at the refund containing height;
- [x] integrate schema-3 actor `recover` effects for the ordered Bitcoin and LEZ
  legs with role-shaped authority, persist-before-send, one `Started` CAS,
  observation-only ambiguous recovery, and finalized-only projection;
- [x] close both direction-correct **two-lock timeout/refund** paths against
  fresh isolated Core 31.1 Regtest and private local LEZ v0.2.0 nodes. Run
  `m3refund-20260716h` leaves both actors at revision four `Refunded` in both
  directions, with exact 2 Bitcoin / 3 LEZ effects per direction, no claim
  effect, byte-identical before/after replay counts, zero resubmission, and
  exact foreign-safe cleanup;
- [x] document the construction-specific atomic flow for every supported pair
  and direction: BTC forward/reverse, transparent ZEC forward/reverse, and the
  supported LEZ-first XMR direction each have a dedicated Mermaid sequence plus
  a direction-specific conditional atomicity argument. Each flow explicitly
  covers late-lock exclusion, pre-reveal and post-reveal abandonment, and the
  rule that a half-completed economic outcome remains nonterminal. Stable
  `atomic-sequence` and `atomicity-argument` identifiers plus the architecture
  guard prevent a direction or property from disappearing; full GitHub
  rendering remains deferred to the milestone-close pass;
- [x] add durable first-lock-only recovery projection: the RED required a
  revision-one `TakerLockConfirmed` store to accept the exact taker-leg refund
  at revision two, reconstruct terminal `Refunded` after reopen, replay
  idempotently, and expose `observe_maker_second_lock_or_recover_taker_leg`;
  GREEN covers maker and taker stores in both BTC directions;
- [x] bind an explicit maker-second-lock cutoff into both signatures on the
  canonical BTC agreement. The RED exposed that the agreement did not enforce
  a safe signed cutoff; GREEN binds and round-trips the exact value and
  validates a nonzero, overflow-checked reaction margin before the earlier
  refund bound. This repository-selected implementation of the RFP-derived taker-first, timeout, race-safety, and
  lossless-recovery obligations; the RFP does not literally prescribe a field
  named `maker_second_lock_cutoff`;
- [x] finish the distinct RFP R2/D1 **first-lock-only absent-maker** BTC journey:
  after the taker’s first lock confirms and the maker never locks, the taker
  must own, prepare, submit, finalize, and replay that first-leg refund without
  maker, Delivery, or Chat participation. Persistence, projection, and live
  refund-side admission are GREEN: two ordinal-bound exact-tip reads, monotonic
  clocks, and exact taker refund evidence persist together. Run
  `m3firstlock-20260716h` proves both actual-node directions, fresh-maker
  reconstruction, zero replay submission, and exact foreign-safe cleanup;
- [ ] after the owner enters QA hardening, run the dedicated adversarial
  maker-second-lock cutoff/refund race at the live chain boundary. The signed
  agreement, refund-side live gate, and timely Maker admission are GREEN,
  but every live maker-lock attempt must still revalidate the canonical unspent
  first lock and cutoff, and the refund branch must use stable canonical
  observations from both chains. The actor consumes the pushed LEZ classifier
  without mapping errors to absence, requires a window ending at the
  cutoff-authorizing finalized tip, and persists both distinct reads. Clean run D
  proves live happy-path Maker admission in both directions. A dedicated
  near-cutoff adversarial race remains open: it must fail closed rather than permit both an accepted
  maker lock and taker recovery under incompatible histories;
  Pushed `3d202f7` binds the maker lock's canonical containing-block time to the
  signed cutoff and exact chain evidence, rejects post-cutoff inclusion in both
  directions, and preserves the existing Bitcoin evidence-v1 wire. This closes
  observation-side false acceptance; the
  same-operation fresh eligibility plus durable SDK intent were then required
  before the actor, rather than the external runner, could submit the maker lock.
  Pushed `4fb6950` closes the Bitcoin node prerequisite: one stable-tip bracket
  now covers the exact agreement funding transaction, containing header, and
  `gettxspendingprevout`, so only sufficiently confirmed and currently unspent
  funding can grant fresh eligibility. Pending exact bytes, spent funding,
  absence, malformed spender bytes, and a moving tip remain non-authorizing.
  The same commit adds caller-authorized Bitcoin funding submission: canonical
  bytes, agreement output, txid/wtxid/vsize, one broadcast, and exact
  `getrawtransaction` readback are checked, while rejection and ambiguity are
  terminal outcomes for already-consumed durable authority. Pushed `8870910`
  wires that contract into the strict schema-4 typed actor seam. Pushed
  `13d048b` supplies the live exact-init/fund admission and current/finalized
  joins. LEZ v0.2 cannot prove pending-level absence. Pushed `3336b6e`
  can authorize only one same-ID/same-bytes journal call and cannot manufacture
  canonical acceptance. Pushed `11111dd` maps that result through the typed
  actor and proves restart does not rearm it. Pushed `923586b` provides the
  generic current-`Funded` state-only proof. Run D proves the composed live
  views, exact transaction joins, node-level idempotence, moving-tip
  reconciliation, and one Maker effect per direction; only the dedicated
  adversarial near-cutoff race remains in this item;
- [x] run the accepted concurrent-swap demo with independent funding inputs,
  agreements, actor stores, effect journals, and overlapping in-flight phases;
  this is not satisfied by sequential swaps, either timeout branch, or a
  two-store unit isolation test. Pushed `1e6d5f1` adds an explicit overlap
  schedule without weakening the sequential runner: it mines one verified
  coinbase-only maturity block, assigns two distinct mature Regtest outpoints,
  pins consecutive anchors 103 and 104, prepares two independent agreements,
  and runs two long-lived controllers whose actual actor commands remain fresh
  one-shot processes. Chain mutations remain deliberately serialized so each
  exact empty/singleton-mempool and finalized-history assertion stays strict.
  Clean already-pushed-commit run `m3overlap-20260717a` proves both swaps were
  simultaneously at revision two `both_legs_locked` before either settlement
  permit. Its isolation packet proves four distinct state databases, eight
  distinct signing journals, two BTC and two LEZ sessions, two agreements,
  two escrow metadata/custody pairs, distinct deadlines, and pairwise-disjoint
  Bitcoin and LEZ effect IDs. Both swaps then reached revision four for both
  roles, replay added zero effects, and exact cleanup targeted no foreign
  resource. The retained secret-safe packet is
  `docs/evidence/m3-overlapping-two-swap-poc-20260717.json`. This checkpoint
  covers one opposite-direction pair; it does not claim arbitrary-N or two
  same-direction swaps sharing one LEZ depositor nonce stream;
- [x] close the survivor-specific nuance: after both locks, the taker publishes
  the direction-correct reveal and is then barred from every harnessed taker
  actor invocation until maker terminality. A fresh maker observes the
  canonical reveal, extracts and
  point-checks the adaptor scalar, commits nonterminal revision 3
  `ClaimEvidenceAvailable`, and exits while the opposite leg remains exact and
  claimable before its signed refund boundary. Another fresh maker resumes from
  its store and the chain, submits the follow-up, and reaches revision 4 before
  the taker returns for observation-only revisions 3 and 4. Clean pushed-commit
  run `m3survivor-20260716c` is GREEN in both directions with exact 2 Bitcoin / 3 LEZ
  effects, zero delayed-catchup or terminal-replay resubmission, no
  caller-supplied secret, and foreign-safe cleanup. Run A first exposed a final
  packet duplicate-key bug (`follower` role versus process evidence); the
  post-PoC RED-GREEN fix separates `follower_role`, and the contract suite also
  caught and fixed an unbound sourced-fixture guard. Independent pre-commit
  review then found that the aggregate merely hashed direction packets while
  restating their conclusions, and that its generic zero-resubmission claim
  retained only a LEZ count. New RED fixtures reject swapped/noncanonical
  direction evidence and a Bitcoin catch-up effect. The GREEN implementation
  validates and derives the aggregate from both recovery/completion packets,
  binds fresh actor outputs plus pre/post Core mempool reads and LEZ durable
  counts, records per-chain successful-resubmission counts, and proves the
  follow-up finalized or confirmed before its signed refund boundary. The
  secret-safe retained packet is
  `docs/evidence/m3-local-two-direction-survivor-claim-poc-20260716.json`;
- [x] implement the accepted proposal’s public LEZ/BTC SDK full lifecycle. A
  shared adapter-independent protocol boundary must expose typed offer
  discovery, negotiation, activation/escrow creation, status/resume, claim, and
  refund with documented public types, errors, and examples. Reuse the existing
  agreement/adaptor/P2TR primitives, coordinator, stores, Core adapter, and LEZ
  bridge; do not wrap the CLI or duplicate their logic. Pushed `ed5cd77` closes
  the shared dependency-light contract: lifecycle, discovery, negotiation,
  structured errors, versioning, explicit claim order, and bounded ordered
  exact-public-effect plans are public and independently tested. The concrete
  full-lifecycle BTC facade, durable post-activation resume, claims/refunds, and
  SDK-owned effects remain open. Pushed `79d7e68` adds the narrower role-fixed
  funding facade: both direction-specific exact plans, offline revision-zero
  activation/status/resume, exact-byte first-lock validation, claim order, and
  typed unsupported capability gaps. Its same-txid/different-witness and
  same-LEZ-ID/different-byte regressions prove that a caller cannot assert a
  plan commitment without the observed bytes. This is a GREEN prerequisite,
  not the accepted full-lifecycle SDK. The 2026-07-17 code/RFP audit confirms
  the remaining gap is a public-boundary/refactor task, not missing protocol
  machinery: stores, Core/LEZ adapters, actor claims/refunds, revisions zero
  through four, and restart behavior already exist. Replace the four public
  `BtcUnsupported*` associated types and the `prepare`,
  `validate_revealing_claim`, `build_followup_claim`, and `recovery_action`
  placeholders with real SDK evidence/material/state/action types. Add public
  chain/store ports plus a lifecycle facade for offer discovery, negotiation,
  activation, status/resume, both locks, both claims, and both refund paths;
  then make the reference actor a thin adapter. External-consumer tests must
  cover both directions, resume at every revision, role/byte substitution,
  replay, and loss of negotiation capability after activation. A compiling
  lifecycle example and API docs are required. Real Delivery/Chat adapters are
  M5 scope; typed ports and realistic in-memory M3 implementations are enough.
  The first claim-lifecycle RED-GREEN slice now replaces the unsupported claim
  evidence/material types with canonical Bitcoin/LEZ revealing evidence,
  redacted zeroizing recovered adaptor material, and deterministic exact
  follow-up plans in both directions. Claim sessions are derived from the
  countersigned agreement and chain domain; both adaptor presignatures and the
  bounded LEZ signature-substitution envelope are verified before claim-ready
  preparation. External-consumer tests reject role, agreement, network,
  finality, byte, presignature, and adaptor-domain substitution and prove
  deterministic replay. The next RED-GREEN slice supplies agreement-bound
  signed Bitcoin and LEZ refund effects before the first lock, so the shared
  `SwapProtocol::prepare` now succeeds only with both claim and recovery
  material. Its pure canonical-state projection validates exact funding and
  refund identities, Bitcoin network/confirmations, LEZ network/finality and
  custody, native deadlines, role ownership, and the direction-dependent
  revealing-leg-first refund order. External-consumer coverage is 42 of 42
  GREEN and strict Clippy, rustdoc, formatting, and diff gates pass. This
  closes the former `PreLockRecovery` and `Recovery` placeholders without
  claiming node, persistence, or submission I/O. The next public lifecycle
  slice composes application-owned discovery and negotiation, validates the
  returned countersigned wire, and drops both capabilities from the active
  type. Complete prepared material then replays exact agreement-bound
  transitions through revisions one to four in both directions and both roles,
  including claims, revealing-leg-first refunds, resume after every revision,
  historical idempotency, and role/agreement/revision/byte substitution. New
  transitions use clone-validate-commit so a failure leaves the coordinator,
  revision, effect log, and restart envelope unchanged. The full BTC SDK now
  passes 15 unit, 16 agreement, 20 external-facade, and 3 example tests plus
  two doctests, strict Clippy, rustdoc, formatting, and diff gates. This proves
  deterministic replay from an application-supplied envelope, not a public
  process-durable store codec, node I/O, or direct actor composition. Pushed
  `0c78f3d` closes those M3 public boundaries with a bounded canonical
  secret-free lifecycle codec, full-range decimal `u128`, exact create/CAS
  store port, role-fixed stored SDK, typed Bitcoin/LEZ runtime, both claim and
  ordered-refund directions, restart after every transition, zero-write replay,
  chain/role/agreement/byte substitution rejection, and a dedicated wiring
  example. Fifteen unit, 32 external-facade, two doctest, and 75 combined
  all-target/all-feature checks are GREEN. Production applications still
  supply the process-durable store and persist-before-send effect journals;
- [x] make the reference actor a thin SDK adapter and move first/second lock
  construction and submission under SDK-owned persist-before-send authority.
  Pushed `79d7e68` adds the dedicated Maker-only revision-one journal with
  complete ordered plan persistence, exact schema validation, observe-before-
  send, one CAS winner, durable accepted-versus-unknown call classification,
  no rearm, exact confirmed step acceptance, and atomic intent close plus
  lifecycle projection. It also forbids the Maker from bypassing that close via
  the generic projector while preserving Taker observation-only projection.
  Pushed `8870910` makes this the schema-4 typed actor path: the Maker requires
  exact direction-shaped material; the Taker forbids it; every step observes
  before one possible send; `Accepted` and `Unknown` stay observation-only;
  exact canonical/finalized evidence alone advances; and the final observation
  ID must equal the final plan ID before atomic close. Schema 3 remains
  observation-only with `attempt_count` zero. Pushed `3336b6e` makes the
  `ExactIdempotentSubmissionSafe` journal path GREEN, and `11111dd` maps it
  through the typed actor with one-send/restart-no-rearm coverage. Neither
  supplies the live same-ID/same-bytes node proof or makes an absence claim.
  Pushed `923586b` supplies generic current-funded state-only evidence, not
  finality or the exact transaction-byte join. Pushed `13d048b`, `6c8e459`, and
  `9b2bce2` compose the missing live ports and move Maker submissions under
  schema-4 actors with exact count/restart checks. `dc07518` and `cd93fb9` close
  the chain-specific transient stable-tip retries found by actual-node attempts
  B and C without weakening effect counts. Clean run D proves both actor-owned
  Maker legs, exact restart/no-rearm behavior, one effect per direction, and
  terminal replay without resubmission;
- [x] implement F7 at the BTC pair boundary. The 2026-07-17 actual RFP,
  accepted Gateway architecture, code, and test audit confirms F7 applies and
  the current witnessed BTC path is native-only. Existing token initialize and
  claim use SHA-preimage authority, while BTC requires the aggregate witnessed
  authority. Reuse both proven implementations to add witnessed-token
  initialize/claim while preserving fixed-destination permissionless refund;
  introduce versioned BTC LEZ asset terms for token program, ATA program,
  definition, owner ATAs, and custody; generalize bridge/client/adapter and the
  funding plan beyond two native steps; regenerate IDL/client; and cover two
  token definitions plus wrong-definition/ATA/authority and rollback cases.
  Certify at least one actual-node custom-token journey in both trade
  directions with exact balances, effects, and restart/no-resubmission. This
  is repository-controlled accepted scope, not a Logos dependency blocker.
  The first RED-GREEN guest slice now appends witnessed-token initialize and
  claim as wire tags 11/12 while proving tags 0-10 unchanged. Shared validators
  compose the existing aggregate BIP-340 authority with exact fungible
  definition, depositor/claimant ATA, custody amount, and deadline checks;
  fixed-destination permissionless refund is unchanged. Host tests cover two
  definitions and substitution negatives, while the recursive checked guest
  proves exact two-party claims and that wrong-definition, wrong-ATA, unrelated
  authority, and one-share attempts leave metadata and custody unchanged. The
  rebuilt guest SHA-256 is
  `bc2ea18eaacb917727934fcf0366dd54c1f9a2b69b61ea53080c926850967fd7` and
  ImageID is
  `f3ead24b95d316ce91980cb3531a70b83a27fd1640f47c1b857757aef26c244e`.
  The next RED-GREEN public-wire slice adds an explicit
  `asset_terms_version: 2` native-or-custom-token envelope without widening
  any existing `lez_bridge.v1.*` method or changing witnessed-native JSON.
  Strict custom-token terms bind the two roles, owners, exact owner/custody
  ATAs, token/ATA programs, fungible definition, amount, deadline, agreement,
  and aggregate authority/key. Thirty protocol tests cover two definitions,
  every field, aliases, zero/malformed/unknown input, exact v1 compatibility,
  and deterministic round trips; Clippy, rustdoc, formatting, and diff gates
  are GREEN. Official ATA/key derivation remains a sidecar duty, and distinct
  v2 request/results plus ordered token effects and observation facts are the
  next wire slice;
  The checked deployment manifest, full verifier, active M3 bootstrap/runner,
  CI pin assertions, and operator guide now consume that exact new ELF/ImageID.
  The generated public IDL is bound by SHA-256
  `994afe1a2fccf285a56070edd520a482d528ef7a85772e12dd7222cf5c80d53f`,
  13 exact append-only instruction names, and tags 11/12. The local-only
  deployer interface and typed initialize/claim assemblers perform zero RPC,
  rederive the metadata PDA plus custody/claimant ATAs, preserve exact IDL
  account order/signer flags, serialize through the official Risc0 codec, and
  bind every output to the checked artifact and IDL. Sixteen deployer tests,
  dependency policy, and the graph-local advisory, ban, license, and source
  audit pass alongside Clippy, rustdoc, formatting, and artifact checks.
  Seven distinct v2 transaction methods now carry strict native or custom-token
  request/results for ordered two-step or three-step preparation, exact current
  observations, witnessed claim reservation/completion/finalized observation,
  and permissionless refund preparation/observation. Their cross-field
  validators reject definition, ATA, program, authority, amount, state,
  instruction-order, and unknown-field drift for two definitions while all v1
  JSON and method strings remain unchanged. Thirty-five protocol tests, strict
  Clippy, rustdoc, formatting, and diff gates pass. An additive separately
  countersigned asset-extension record now binds the unchanged agreement-v1
  commitment to an explicit native or custom-token choice. The custom variant
  covers the programs, definition, owners, independently derived custody and
  owner ATAs, amount, deadline, and aggregate authority/key. Both role
  signatures, bounded canonical wire, exact local asset policy, cross-agreement
  and network substitution, every field, aliases, and the native-byte
  compatibility contract pass 16 agreement tests.
  Four additional finalized classifiers now cover initialization, token-only
  custody-ATA creation, funding, and claim with exact prepared bytes/ID or
  bounded terms discovery. Stable finalized coverage, containing blocks,
  instructions, metadata, custody, two definitions, native parity, and strict
  `Found`/`Absent`/`Uncertain`/`Unavailable` separation pass 44 total protocol
  tests plus Clippy, rustdoc, formatting, and diff gates. This closes the typed
  four-effect restart model without claiming the official sidecar can yet
  produce its complete scans. The exact-once bridge client now maps all eleven
  v2 lifecycle/classifier methods without retries. Its operation-specific role
  matrix permits only the depositor to prepare funding, only the claimant to
  prepare/complete a claim, and either bound participant to read/classify or
  prepare the permissionless fixed-destination refund. Strict response checks
  bind context, Lee v0.2 runtime, terms, target, window, transcript, ordered
  effects, public placement, stable tips/clocks, and exact IDs/bytes. Five unit,
  five external v2, 32 preserved bridge-contract, and four example tests pass,
  together with strict all-target Clippy, rustdoc, doctests, formatting, and
  diff gates. The official v0.2 sidecar planner now rederives the pinned
  Token/ATA programs, exact depositor/claimant/custody ATAs, tags 11/7/8/12/10,
  signer sets, and consecutive depositor nonces. It durably reserves the
  three-effect escrow plan, aggregate claim reservation/completion, and
  permissionless refund in four distinct v2 files before exposing exact bytes;
  restart replays without nonce reads or regeneration. Six focused tests cover
  both roles, two definitions, order/program/ATA/authority drift, conflicts,
  redaction, and two-stage restart. The full sidecar regression, strict
  all-target Clippy, rustdoc, formatting, diff, advisory, ban, license, and
  source gates pass. The main-process adapter now rechecks the
  extension-to-base-agreement commitment and exact caller-owned asset policy,
  maps native or every custom-token field into the v2 terms, and exposes one
  no-submit transport method for each of the eleven client calls. Runtime,
  chain, program, signer, and depositor/claimant/either-participant checks happen
  before I/O; caller-owned IDs, windows, targets, effects, and transcripts pass
  unchanged; transport failures stay distinct from
  `Found`/`Absent`/`Uncertain`/`Unavailable`. Six external F7 tests and 73
  preserved adapter tests pass with strict all-target Clippy, rustdoc,
  doctests, formatting, and diff gates and no dependency change. The additive
  role-fixed F7 SDK now accepts only a complete agreement-bound Bitcoin lock
  plus an explicit native two-step or custom-token three-step LEZ plan. It
  revalidates the countersigned extension, exact Bitcoin transaction/output,
  asset-specific step order, unique IDs/bytes, and adapter-produced finalized
  first-lock facts. Its opaque confirmation binds the base agreement, asset
  commitment, exact Taker plan, and direction before releasing the Maker plan;
  it cannot be constructed from a boolean finality assertion or evidence for a
  different asset. Seven F7 facade cases cover both directions and both asset
  kinds plus plan, byte, output, finality, network, metadata, custody, definition,
  amount, role, and confirmation substitution. All 61 SDK all-target tests,
  strict all-feature Clippy, rustdoc, doctests, formatting, and diff checks pass
  without a dependency or agreement-v1 wire change. A separate
  official-wallet fixture now provisions two independent v0.2 token
  definitions, four exact actor ATAs, and opposite-role `250/0` and `0/250`
  starting balances through eight unique local-chain transactions. Its
  RED-GREEN contract fixes the 20-command role/order surface, literal-loopback
  routes, no-reuse behavior, `0700`/`0600` state, and secret-safe evidence.
  Rehearsal `m3f7fixture20260717a-lez` passed on fresh pinned local nodes through
  finalized tip 17 and exact run-scoped cleanup; its throwaway keys and
  plaintext wallet state were then removed. That rehearsal proves reproducible
  provisioning, not a completed swap or retained certification packet.
  The existing offline Bitcoin PoC provisioner now has an additive
  `finalize-asset-extension` command that reconstructs common roles, amount,
  deadline, aggregate authority, and aggregate key from the already validated
  base agreement; accepts only the locally selected Token/ATA programs,
  definition, owner ATAs, and custody ATA; countersigns the canonical extension
  with the existing separate maker/taker fixture keys; and revalidates the
  encoded wire before create-new `0600` persistence. Its focused test captured
  the missing function RED and is GREEN together with all 12 provisioner tests,
  strict all-target Clippy, rustdoc, and formatting. This supplies the
  reproducible agreement-to-F7 artifact seam for runner composition; it does
  not itself submit or observe a token swap.
  Actor schema 5 now requires that exact countersigned extension for both
  roles, retains its canonical wire and commitment as part of the durable
  agreement acceptance/evidence-chain identity, and preserves schema-3/4
  reopen behavior through an explicit pre-asset-column SQLite migration test.
  A custom-token LEZ maker lock maps the strict v2 preparation result into the
  exact ordered initialize/custody-ATA/fund journal, while the reverse Bitcoin
  maker lock is also bound to the asset rather than only the base agreement.
  Activation stages both directions without send authority; replay is exact;
  duplicate transaction IDs or exact bytes in the untrusted preparation file
  fail before journal creation. All 80 actor and 107 store tests, strict
  all-target Clippy, rustdoc, doctests, formatting, and diff checks pass. Live
  schema-5 taker-first submission, observation, claim, and refund composition
  remains part of the runner work rather than being implied by this durable
  mapping slice.
  The official v0.2 sidecar server now authenticates and registers all eleven
  v2 asset methods, restores the four preparation/completion reservations in
  dependency order, and replays their exact results after process restart.
  Its bounded finalized scanner independently checks block ID/hash equality,
  parent-linked ancestry, canonical public bytes/hash, stateless validity,
  signer and account order, instruction/program/ATA identities, metadata,
  fungible definition, and historical holdings for initialization, custody
  creation, funding, claim, and refund. A pinned-indexer default account is
  treated as authoritative absence while an RPC failure remains unavailable;
  permissionless custody creation requires no fabricated signer. Root review
  reproduced a refund scan/state same-height fork that previously could join
  facts from different finalized views. The RED regression is GREEN after the
  scanner retained one stable witness, read state at that exact tip, rechecked
  its identity, and downgraded movement/history loss to `UnknownOrPending`.
  Official planner bytes pass a complete init/custody/fund finalized journey,
  authenticated real-client route/restart coverage, five focused scanner
  cases, and all 128 current sidecar tests plus 44 protocol
  contracts, strict Clippy,
  rustdoc/doctests, formatting, diff, advisory, ban, license, and source gates.
  Fresh isolated run `m3f7compose20260717l` then proved the checked guest
  deployment, both fresh-identity Vault claims, the eight-transaction official
  Token/ATA fixture, the forward Bitcoin first lock, and actor-owned finalized
  LEZ initialization plus custody creation. Its funding pre-send loop exposed
  a repository-owned liveness defect: bounded snapshot absence was coupled to
  an unrelated twice-frozen live finalized tip, so a continuously advancing
  official indexer could deny send authority forever. The run was stopped
  through its exact cleanup trap before any funding submission. RED-GREEN
  coverage now anchors missing-effect predecessor state to the immutable
  requested-end block, revalidates that exact block by ID and hash after the
  historical account reads, permits later finalized blocks to arrive, and
  still fails closed on requested-end identity drift. Pushed commit `d48e70e`
  carries that requested-end absence fix; all three focused tests and the full
  eight-test sidecar unit set pass. Found effects, refunds, actor
  CAS, deterministic transaction bytes, accepted/unknown journals, nonce
  replay protection, and monotonic escrow transitions keep their stricter
  existing behavior. Subsequent fresh isolated run
  `m3f7compose20260717m` proved the fresh official-wallet build, checked guest
  deployment, both finalized fresh-identity Vault claims, and the official
  eight-transaction Token/ATA fixture. It is not PoC evidence: a transient
  rewrite of the live runner during concurrent terminal-evidence editing
  produced the shell error `eements` and invalidated the run before either
  trade direction started. Its exact cleanup attestation passed and targeted
  no foreign resource. The process correction is mandatory: never edit a
  runner during its live execution; finish, syntax-check, test, and commit the
  complete patch before starting a new run. Fresh isolated run
  `m3f7compose20260717n` then passed the checked deployment, fresh-identity
  Vault bootstrap, official eight-transaction F7 Token/ATA fixture, forward
  Bitcoin first lock, and actor-owned LEZ initialization, custody creation, and
  funding through exact finalized inclusion. It remains bounded RED evidence,
  not a passed PoC: Maker second-lock observation never closed after the exact
  funding transaction `b75f...9e7f` finalized once in block 181. All 120 fresh,
  read-only actor retries failed closed without a duplicate submission. Exact
  cleanup attestation passed without broad cleanup or a foreign resource
  target. Read-only diagnosis found two repository-owned liveness couplings:
  the sidecar's hard 10-second historical-account timeout is shorter than the
  pinned official indexer's checkpoint replay at that height, and a separately
  unrelated latest-finalized-tip freeze rejects an unchanged containing block
  whenever the local chain advances. RED-GREEN work now gives the historical
  read an explicit bounded budget and revalidates the exact containing block
  rather than freezing unrelated later finality. A batched or cached upstream
  historical snapshot remains the production improvement, but it is not an M3
  blocker after the local fail-closed fix. Independent GREEN gates are rustfmt,
  strict Clippy for all targets and features, all 128 current sidecar tests,
  the actor contract, Mermaid compatibility, and diff check; these component
  gates do not promote run N to PoC evidence. Fresh isolated run
  `m3f7compose20260718o` then passed the checked deployment, fresh-identity
  Vault bootstrap, official eight-transaction F7 Token/ATA fixture, forward
  Bitcoin first lock, and actor-owned finalized LEZ initialization, custody
  creation, and funding. It also remains bounded RED evidence, not a passed
  PoC: post-funding Maker observation attempts 1 through 13 each failed closed
  after roughly 20 seconds without a duplicate submission. The run was stopped
  intentionally instead of spending approximately 35 minutes on redundant
  retries. Its exact cleanup attestation passed without broad cleanup or a
  foreign resource target. The local accommodation now gives each sidecar
  historical request a bounded 90-second budget and the single outer actor
  bridge call exactly 120 seconds; it does not add a transport retry or widen
  any durable submission authority. Batched or cached historical snapshots
  remain the upstream production improvement, not a milestone blocker after
  this local bound is GREEN. Fresh isolated run
  `m3f7compose20260718p` started from clean pushed commit `d8d8f6a` and passed
  the checked deployment, both fresh-identity Vault bootstraps, and the official
  eight-transaction F7 Token/ATA fixture. The first actor configuration was
  rejected before any trade effect because
  `actor_lez_bridge_request_timeout_millis=120000` exceeded the actor validator
  maximum of 60000 milliseconds. Exact cleanup passed without broad cleanup or
  a foreign resource target. Run P is bounded RED evidence, not a custom-token
  F7 PoC pass. The current tree locally fixes that exact mismatch by raising the
  enforced actor maximum to 120000 milliseconds. A schema-5 boundary regression
  accepts 120000 and rejects 120001. Fresh isolated run
  `m3f7compose20260718q` then started from clean pushed commit `d2d3bef`,
  repeated the checked deployment, both fresh-identity Vault bootstraps, and the
  official eight-transaction F7 Token/ATA fixture, and accepted the actor
  configuration. Its first Maker-owned LEZ initialization drive still failed
  before an effect with typed `actor adapter configuration is unavailable`:
  the bridge client retained its independent one-minute maximum while the
  runner and actor allowed 120 seconds. Exact cleanup again passed without broad
  cleanup or a foreign resource target. Run Q is bounded RED evidence, not a
  custom-token F7 PoC pass. The current tree now also raises the enforced bridge
  client maximum to two minutes, with a client boundary test accepting 120
  seconds and rejecting 121. The pre-Docker contract extracts all three
  configured limits and rejects future runner/actor/client drift before node
  startup. Fresh isolated run `m3f7compose20260718r` started from clean pushed
  commit `7fd84fa`, passed the checked deployment, both fresh-identity Vault
  bootstraps, official eight-transaction F7 Token/ATA fixture, forward Bitcoin
  first lock, and actor-owned LEZ initialization, permissionless custody-ATA
  creation, and funding through unique finalized inclusion. The Maker then
  projected that custom-token lock to revision two. The Taker did not: all 120
  bounded retries called the legacy native-only
  `lez_bridge.v1.observe_finalized_witnessed_funding`, which cannot recognize
  the four-account token funding transaction. No claim or refund followed;
  exact cleanup removed only the captured run resources and secure state. Run R
  is bounded RED evidence, not an F7 PoC pass. RED-GREEN coverage now proves
  that a schema-5 Taker with no Maker-private prepared material uses the v2
  classifier with `DiscoverByTerms`, projects an exact `Found` token funding to
  revision two, and keeps `Absent`, `Uncertain`, and `Unavailable` pending. The
  live selector retains schema 4 on v1, binds the v2 request to the countersigned
  agreement, asset commitment, run, role, runtime, full terms, fixed target,
  and discovery window, and exposes no submission method. Forward
  Taker-observes-Maker and reverse Maker-observes-Taker transitions are both
  covered. Fresh isolated run `m3f7compose20260718s` started from clean pushed
  commit `ba17e3b`, repeated the checked deployment, both Vault bootstraps, the
  official eight-transaction fixture, the forward Bitcoin lock, and unique
  finalized token initialization, custody creation, and funding. The Maker
  reached revision two through exact observation. The Taker used the intended
  v2 peerless classifier but failed closed with `conflicting_discovery`: its
  funding scan encountered the valid earlier same-swap initialization before
  reaching the valid funding at block 170. No claim/refund followed. Exact
  cleanup attests all captured resources absent, no broad cleanup, and no
  foreign target. Run S is bounded RED evidence, not an F7 PoC pass.
  RED-GREEN coverage now exercises the official three-effect sequence in one
  finalized window. Terms discovery classifies each decoded instruction by
  lifecycle kind, validates every term field encoded by a different same-swap
  step plus its ordered accounts and signers before ignoring it, and preserves conflicts for
  same-kind or malformed substitutions. The regression fails on the old code,
  passes on the fix, and a changed same-swap terms hash remains fail-closed.
  All 128 pinned v0.2 sidecar tests, formatting, and strict all-target/all-feature
  Clippy pass; all 85 actor tests and the M3 pre-Docker actor contract remain
  green from `ba17e3b`. Fresh isolated Run T (`m3f7compose20260718t`) then
  started from clean pushed commit `50db397`, repeated the checked deployment,
  two Vault Claims, official eight-transaction Token/ATA fixture, Bitcoin lock,
  and finalized token initialization/custody/funding at LEZ blocks 120/148/170.
  After two moving-tip fail-closed retries, Maker exact observation reached
  revision two. Taker peerless lifecycle-aware discovery then also reached
  revision two on its first projection attempt, proving the Run S scanner fix
  through the actual node and role boundary. The next evidence-only step failed
  because the embedded `jq` program tried to add custom-token fields directly
  inside an unparenthesized object value. No claim or reverse direction followed;
  exact cleanup removed all captured resources and secure state without a broad
  or foreign target. Run T is bounded RED evidence, not an F7 PoC pass. The
  dual-lock serializer is now a tracked directly executable jq filter; its
  contract compiles both native and custom-token shapes and asserts every common
  field plus the custom custody, asset commitment, and three-step order. It
  publishes only after successful validation through a private partial file.
  The full pre-Docker orchestration contract is GREEN. Run U started from exact
  pushed `65f55c5` but the inherited ten-second custom-token slot spent thirteen
  minutes only reaching bootstrap and had not crossed any new F7 boundary. It
  was deliberately interrupted and its exact cleanup left no run-owned Docker
  resource, process, or secure-state directory. ADR 0047 replaces that quiet-tip
  workaround with a pinned requested-window observer: it reads only the
  countersigned interval, accepts monotonic newer finalized descendants, and
  rechecks both finalized height monotonicity and the pinned end block after
  historical account reads. Behavioral RED-GREEN covers irrelevant descendants;
  existing fork and identity-drift tests remain fail-closed. The complete 128-test
  sidecar suite, five binary/example tests, strict Clippy, and the one-second F7
  orchestration contract are GREEN. Fresh isolated Run V
  (`m3f7compose20260718v`) started from clean pushed `4b55dda` and validated the
  faster cadence through one complete forward direction. It finalized the
  Bitcoin first lock at height 103, the four custom-token LEZ effects, the
  revealing claim, and the Bitcoin follow-up at height 104; both actors reached
  terminal revision four, custody was zero, balances were conserved at
  `175/75/0`, and terminal replay submitted nothing. The reverse direction then
  failed before stage-two finalization because overlap-era allocation had
  preassigned its funding anchor to height 104, now legitimately occupied by
  forward settlement. Its source remained unspent, policy-only preparation had
  no public effect, and exact cleanup passed. Behavioral RED-GREEN now separates
  immutable source allocation from schedule-aware anchor assignment: sequential
  directions atomically reserve `current tip + 1` immediately before their own
  stage two, while overlap reserves consecutive heights before either stage two.
  Reservations require a stable tip and empty mempool and cannot be rebased
  after finalization. Fresh isolated Run W (`m3f7compose20260718w`) started from
  clean pushed `b872b12` and proved that fix against actual nodes: forward used
  anchor 103, its settlement consumed height 104, and reverse then reserved and
  finalized stage two with fresh anchor 105. Forward again reached terminal
  revision four with exact `175/75/0` balances and zero replay submissions.
  Reverse then finalized custom-token initialization, custody creation, and
  funding at blocks 57, 61, and 64 and both actors projected that first lock.
  The run stopped before the reverse Bitcoin second lock because its retry guard
  still assumed the native path's two LEZ effects instead of the custom-token
  path's three. No foreign resource was targeted and exact cleanup passed. This
  bounded RED is now covered by native `2 -> 3` and custom-token `3 -> 4` drift
  rejection; GREEN derives the invariant from the countersigned asset mode and
  preserves the exact count across typed retries. Run W is not a complete F7
  pair. Its private evidence timestamps also establish current iteration costs:
  the cold official-wallet build took 2 minutes 7 seconds, serialized Core and
  LEZ readiness took about 36 and 58 seconds, forward stage two through terminal
  took 5 minutes 32 seconds, and reverse stage two through the typed RED took
  2 minutes 39 seconds. Explicit terminal ATA balance and packet bindings remain
  forward `175/75/0`, reverse `75/175/0`, and conserved total `250`.
  Fresh isolated Run X (`m3f7compose20260718x`) on clean pushed `422c72e`
  closes the functional gate. Both actual-node directions reached revision four
  `completed`, each retained exactly two Bitcoin and four LEZ effects, both
  retained one Maker second-lock effect, replay submitted nothing, and exact
  cleanup removed every captured run resource without targeting a foreign
  resource. The secret-safe packet records no public RPC, faucet, public funds,
  or private-material disclosure. Run X took 20 minutes 52 seconds including a
  2 minute 2 second cold official-wallet build. The owner-requested repeatability
  gate requires at
  least three clean custom-token swaps per direction, another recorded native
  pair, and one clean repeat of the opposite-direction overlap checkpoint before
  the M3 tag. Every repetition must use fresh identities and retain exact
  balance, effect, replay, and cleanup evidence. Synchronized
  documentation, D1, and milestone-wide closure gates follow that passing run;
  the official-wallet cache is now implementation- and contract-GREEN. Policy
  revision 2 binds the clean official source/origin/archive and Cargo metadata,
  lockfile, program artifacts, effective modern/legacy Cargo configs, Rust and
  Cargo binaries/versions/target-library tree, build tools, bindgen include
  tree, native libraries, exact recipe, expected wallet SHA-256, and helper
  hash. Objects contain only the executable and manifest under owner-only
  modes; private consumers are non-hardlinked and triple-rehashed. Production
  test overrides, dirty/ignored source, missing runtime libraries, invalid
  published refs/objects, changed helper bytes, and untracked/dirty actor HEAD
  all fail closed. A hardened production-input miss measured 202.42 seconds
  and 856,824 KiB peak RSS; its exact hit measured 10.35 seconds and 33,844 KiB,
  saving 192.07 seconds (94.9%) and about 804 MiB peak RSS per repeat. The JSON
  retains monotonic duration, artifact size, canonical secret-free input,
  policy/helper, object-manifest, runtime, and wallet identities. This is
  dirty-tree development performance evidence, not exact-head chain evidence.
  Run Y (`m3f7compose20260718y`) intentionally counts as no repetition: an
  operator-selected pre-F7 artifact failed the pinned guest hash before
  deployment in 1 minute 58 seconds, then exact cleanup removed every owned
  resource without foreign targeting. Fresh Run Z (`m3f7compose20260718z`) on
  clean pushed `1555749` used the exact Run-X witnessed-token artifact and
  completed both directions in 19 minutes 10.95 seconds. It certified a
  production-mode 10.32-second cache hit while preserving revision four,
  `2 Bitcoin + 4 LEZ` effects, `175/75/0` and `75/175/0` balances, conserved
  total 250, zero replay, finalized chain evidence, and exact cleanup.
  Fresh Run AA (`m3f7compose20260718aa`) on clean pushed `df7ed86` then
  completed the third pair in 18 minutes 13.61 seconds with the same revisions,
  effects, balances, finality, zero replay, and exact cleanup; its
  production-mode cache hit took 7.81 seconds. F7 is therefore 3 of 3 clean
  repetitions per direction and the requested repeatability gate is closed. A
  follow-on RED proved the outer runner lacked early exact artifact identity.
  GREEN now checks the canonical regular non-symlink guest and deployer hashes
  before prebuild or node startup, while bootstrap independently rechecks the
  guest and deployer through point of use and evidence publication. This avoids
  Run Y's measured 1 minute 58 second late-fail path without relaxing or
  replacing any bootstrap identity gate;
- [x] parallelize independent Core and LEZ node provisioning without weakening
  isolation or cleanup. Run AA measured the old sequential startup at about
  39 seconds Core, 58 seconds LEZ, and 98 seconds including handoff. ADR 0048
  starts both fixed SHA-bound launchers in exact sessions, defers signals across
  registration, waits and reaps both statuses, authenticates exact
  run/scope/component resource sets, and retains individually verified
  resources for exact cleanup even when certification fails. The production
  coordinator behavioral matrix is GREEN. Fresh Run AB on clean pushed
  `74c58d1` proved both concurrent launchers, the exact join, and reconciled
  inventory on actual nodes, then failed closed after 4 minutes 24.69 seconds
  when the first direction received the full tab-separated Core inventory
  record instead of only its Docker ID. It certified no direction and no
  successful-run speedup. RED reproduced exact record-boundary failures;
  GREEN now accepts only one owner-private non-symlink canonical
  `ID<TAB>bitcoin-core` record, exports only the 12- or 64-hex ID, and
  revalidates live run/scope/component labels in both the outer and direction
  processes. Run AB is spent. Run AC on clean pushed `b5bf322` crossed that
  boundary and completed both effect-bearing directions in 17 minutes 10.41
  seconds with the expected revisions, effects, balances, conservation, and
  zero replay. It nevertheless exited nonzero because cleanup parsed valid
  omitted/false actor ownership booleans with `jq -e`, whose shell status for
  JSON false is failure. All label-filtered Docker categories, registered
  processes, listeners, and secure state were absent, but fail-closed policy
  means AC certifies neither another F7 repetition nor a successful benchmark.
  RED now reproduces valid false/omitted and invalid typed flags; GREEN converts
  only validated booleans to strings before exit-status evaluation. AC's
  diagnostic logs show a concurrent 82-second startup window versus AA's
  sequential 98 seconds, a provisional 16-second saving with host contention.
  Run AC is spent. Fresh Run AD on clean pushed `0826dd5` then completed both
  directions and terminal cleanup with exit zero in 16 minutes 6.52 seconds.
  It preserved revision four, `2 Bitcoin + 4 LEZ` effects, one Maker second
  lock, zero replay, zero custody, total 250, exact `175/75/0` and `75/175/0`
  balances, a production 7.370-second wallet-cache hit, and no foreign cleanup
  target. Core took 38 seconds and LEZ 67 seconds in one 67-second concurrent
  window versus AA's 98-second sequential baseline. The measured and certified
  startup saving is therefore 31 seconds. AD's 127.09-second end-to-end
  improvement over AA is retained as context, not attributed wholly to node
  startup without structured per-phase timings;
- [x] publish fail-closed structured phase timings before selecting another
  optimization. ADR 0049 records one outer-run monotonic producer and fixed
  phase sets for custom-token sequential, native sequential, and native
  overlap execution. The owner-private journal and final packet reject
  malformed uptime, unsupported or reordered phases, missing/duplicate
  records, overlaps/regressions, imprecise integers, extra fields, invalid UTC
  dates, symlinks, unsafe modes, and final or partial clobber. The main packet
  independently revalidates the schema, binds the relative path and SHA-256,
  requires exact summary equality, and rehashes immediately before and after
  publication. Focused RED-GREEN, the actor orchestration contract, the node
  coordinator contract, ShellCheck, actionlint, hadolint, Compose validation,
  and CI policy are GREEN. Clean pushed Run AE
  (`m3f7compose20260718ae` at `a82876d`) passed both custom-token
  directions and exact cleanup in 17 minutes 9.57 seconds wall time. The
  pre-publication packet measured 1,023,100 ms with 280 ms unattributed:
  363,660 ms forward direction, 405,810 ms reverse direction, 103,820 ms LEZ
  bootstrap, 75,400 ms F7 fixture, 60,110 ms node startup, and 13,800 ms in all
  other measured phases. Both directions consume 75.2 percent. The next safe
  iteration decomposes direction internals before changing finality, cadence,
  actor scheduling, or evidence semantics; no additional speedup is claimed;
- [x] split each sequential outer direction into unchanged funding-reservation,
  stage-two, child actor-flow, terminal/replay, and custom-token balance
  boundaries. RED required the exact custom-token/native phase matrices and
  dynamic direction names; GREEN preserves command order, omits the balance
  phase for native assets, and retains one overlap window rather than summing
  concurrent work. The broad actor contract and complete pinned CI quality
  suite pass. Add semantic phases inside the child actor flow before spending
  another 17-minute actual-node iteration;
- [x] publish strict child actor-flow timings for both directions. The fixed
  claim plan measures transcript, presign/activation, each lock and revision,
  the dual-lock gate, each claim and revision, and terminal evidence. Survivor,
  two-lock refund, first-lock refund, and overlap use fixed journey-specific
  plans. Each child binds its current actual-effect manifest. The outer runner
  independently validates exact schemas and permissions, requires child
  duration containment by the correct outer actor-flow or overlap phase, binds
  both relative paths and hashes into the main packet, and rehashes both child
  packets plus both effect manifests before and after main publication.
  Focused RED-GREEN and the broad actor/CI-policy contracts pass without
  changing actor order, RPCs, retries, finality, authority, or chain effects;
- [x] execute one clean custom-token actual-node pair from the exact pushed
  child-timing commit. Run `m3f7compose20260718af` on
  `0b54ab68f766ff016741dd6ba2eacade4a1c1e31` passed both directions and
  exact cleanup in 1,007.57 seconds wall time. Its 1,000,170 ms outer packet
  has 510 ms unattributed. The 346,060 ms forward and 386,060 ms reverse
  children fit exact 346,280/386,310 ms parents, bind exact effect manifests,
  and retain revision four, `2 Bitcoin + 4 LEZ` effects, one Maker second
  lock, zero replay/custody, conservation 250, and exact balances. Five
  finalized lock/claim windows dominate; every other child phase is below one
  second. Unrelated host contention makes the 22-second Run-AE wall-time
  difference non-certifiable as a speedup;
- [x] connect schema-5 two-lock timeout recovery to the existing witnessed
  asset v2 refund protocol/client/adapter instead of adding another wire or
  dependency. The actor now binds preparation, observation, and generic
  submission request identities to the base agreement, asset commitment,
  transition, role, runtime, full terms, target, and exact transaction. The
  existing durable public-effect journal remains the sole submit-once
  authority; schema-4 native recovery is unchanged. The locked actor compile,
  all 91 pre-existing actor tests, the focused asset/refund identity test, both
  shell parsers, custom-token refund contract mode, and the complete
  source-only actor orchestration contract are GREEN;
- [x] extend the sequential local runner and secret-safe evidence model for
  two distinct custom-token refunds. Each direction requires three token lock
  effects followed by exactly one v2 refund, terminal revision four
  `Refunded`, zero custody, no replay submission, and direction-correct return
  balances: forward Maker 250/Taker 0 and reverse Maker 0/Taker 250. Claim
  packets retain their prior 175/75 and 75/175 outcomes under generic terminal
  evidence field names;
- [ ] commit and push this component-GREEN slice, then execute the exact clean
  pushed-source two-devnet refund pair. Retain the certificate only after both
  directions, terminal replay, exact cleanup, and foreign-resource isolation
  pass; until then F7 remains open and no actual-node refund claim is made;
- [ ] map those five dominant waits to their exact finalized-observation and
  confirmation policies, then write the next RED around the lowest-risk
  development-only acceleration. Preserve production defaults, chain
  finality, atomicity, actor authority, one-attempt journals, and deadlines;
- [ ] compact or externally reference peerless public transaction facts before
  production. The validated v2 wire can carry transaction bytes larger than
  the recovery store's 64 KiB per-chain-evidence cap. Official PoC transactions
  are well below the cap; an oversized valid `Found` currently fails closed at
  projection and cannot grant authority, but could deny liveness. This is
  repository-owned production hardening, not an M3 functional-PoC blocker;
- [x] create the D1 recordings for BTC happy, refund/timeout, and overlapping
  concurrent journeys. Secret-safe JSON manifests prove machine facts but are
  not recordings and cannot satisfy D1. The repository now supplies a
  private recorder with no new project dependency around the installed util-linux
  `script`/`scriptreplay` pair. Its RED-GREEN contract covers all three exact
  scenario mappings, replayable output/timing files, SHA-256 binding to the
  passing actual-node packet, commit and Core/LEZ version metadata, `0700`/`0600`
  permissions, no-clobber behavior, failed-driver propagation, and a forbidden
  production test-driver override. Test fixtures are marked
  `certification_mode: test_contract`; only a clean-worktree run through the
  fixed actual-node driver may emit `live_actual_nodes`. The contract runs in
  the pinned CI quality gate. A second GREEN verifier accepts exactly one
  happy, refund, and concurrent recording; requires distinct run IDs, the same
  clean repository commit and node versions, exact scenario-to-evidence
  mappings, replayable `0600` output/timing pairs, and byte-identical manifest,
  evidence, output, and timing hashes. It rejects test fixtures in production
  mode, duplicates, tampering, public dependencies, and output overwrite. The
  Three fresh owner-private actual-node recordings now bind clean pushed commit
  `a6eb1ada739f8fcd671feb8fbb41cfc682e5d651`: happy run
  `m3record-happy-20260718ag`, refund run
  `m3record-refund-20260718ag`, and concurrent run
  `m3record-concurrent-20260718ag`. Each is replayable from its mode-`0600`
  terminal output/timing pair, binds its passing evidence packet and exact
  isolated Core/LEZ run identities, records no public RPC/faucet/funds, and
  retains zero terminal replay submissions. The refund recording completes
  both direction-specific earlier/later recovery orders; the overlap recording
  proves both swaps simultaneously revision two with disjoint actor authority;
- [x] seal the three completed D1 recordings into the private bundle after the
  active M3 source slices return to a clean committed checkout. The first live
  bundle attempt exposed a real verifier RED: exact object equality rejected
  legitimate per-run IDs and the deliberate refund `3.0` versus claim `1.0`
  slot profiles. Pushed `eb94f91` makes the contract compare invariant chain
  versions/networks, retains unique topology in each hashed manifest, requires
  all evidence to bind one commit, permits only an ancestor evidence commit
  from a clean verifier checkout, and records both evidence and verifier
  commits. Focused RED-GREEN-REFACTOR, syntax, and whitespace checks pass.
  Verifier commit `946208a887709d9b8422f51f8152a3008c6d745a` sealed the
  three evidence-commit `a6eb1ada739f8fcd671feb8fbb41cfc682e5d651`
  recordings into one mode-`0600`, result-`passed` private bundle with SHA-256
  `3d7d7adc12571a610be21a18b746e68cb17311ea1224191fcdcdf1b39a86c7cc`.
  It binds all manifest/evidence/output/timing hashes, exact run IDs and
  isolated node profiles, and records no public RPC, faucet, funds, or
  external-network certification dependency;
- [x] render the literal three-video D1 output from the retained actual-node
  evidence without rerunning either chain. RED-GREEN contracts first failed on
  source-manifest tamper, aggregate tamper, role-action mutation, out-of-order
  refund confirmation, MP4 tamper, duplicate scenarios, and an unstable
  presentation tail. The digest-pinned MIT VHS renderer and fail-closed bundle
  verifier now regenerate the complete role/effect proof, run with no network
  or Linux capabilities, decode-probe every H.264 1280x720 stream, and bind
  exact source/video bytes. At renderer/verifier commit `846ba56`, happy,
  refund, and concurrent MP4s passed complete decode plus sampled intro,
  both-direction, scenario/atomicity, and stable-tail frames. Their mode-`0600`
  bundle is `7697a27c...f101ba8` and records no public RPC, faucet, public
  funds, or external-network success dependency;
- [x] synchronize retained secret-safe evidence, manual reproduction,
  architecture/atomicity diagrams, traceability, and the milestone packet,
  then run the exact lint, test, vulnerability, license, source, security, and
  Mermaid gates before any M3 completion tag. On 2026-07-19 the pinned quality
  runner, Rust/Node/dependency/security/isolation/traceability gates, all 11
  cargo-deny graphs, conservative GitHub parsing, and exact rendering of all
  150 diagrams passed. The remote Trivy and actual-node jobs remain mandatory
  on the pushed closure commit;
- [x] publish exact closure commit
  `f7fb250f0491b9c33ed56f2ee02cdbc5ea5dcbb2` and annotated
  `m3-complete`. SSH verified `origin/main` and the peeled remote tag at that
  commit. The private Actions API was unavailable because no API identity is
  configured, so the tag annotation records that fact and makes no remote-green
  claim;
- [ ] after the owner enters later hardening, add restart, reorg, fee, and chaos
  cases beyond the reproducible functional PoC boundary.

No repository-controlled private-functional M3 closure item remains. Later
owner-selected QA, chaos, information-security, public execution, and
production-readiness phases remain separate work. The D1 bundle, durable public
lifecycle codec/store/typed-port composition,
official and independent vectors, and self-hosted/exact-HTTPS Testnet4
configuration contract are GREEN in pushed commits `0c78f3d` and `946208a`.
The fresh native happy, refund, and overlap recordings plus their three
verified private MP4 projections are complete. Runs X,
Z, and AA close the requested three exact F7 terminal balance/finality pairs.
The synchronized closure claim, six-output evidence inventory, external
resource boundary, deferred hardening, and exact tag rule are maintained in
[the M3 review packet](milestone-3-review.md).
The official-wallet starting fixture remains a reproducible prerequisite rather
than a substitute for actor-owned escrow/claim/refund effects or finalized
balance evidence. The
revision-one-to-two store/projector, signed
cutoff agreement, finalized LEZ classifier, and refund-side live gate are GREEN components, and run H closes the distinct two-lock timeout/refund item; none
substitutes for the final repository-wide closure gates or later owner-selected
hardening. Logos-owned limitations listed below
remain production-release blockers/nonblocking milestone caveats; they do not
waive any repository-controlled M3 item.

LOGOS-017 records two nonblocking upstream production caveats: the compatible
refund wire does not separately expose the containing-block timestamp that the
pinned sidecar enforces internally, and terms discovery uses a fixed maximum
4096-block window that can age past an old transaction. The actor treats every
miss, timeout, moving tip, or aged window as unknown/pending with no new send
authority. Under the owner policy these Logos limitations do not stop M3
certification, but they remain explicit release work. Run H proves the private
local actual-node two-lock refunds under these disclosed compensations; it does
not establish proof-bearing public LEZ finality. The upstream indexer also
returns finalized blocks and historical account state as RPC DTOs without an
account proof or atomic multi-read snapshot token, and live official-endpoint
`getLastFinalizedBlockId` support remains unverified. The local actor therefore
brackets reads at a stable finalized prefix, checks parent-linked ancestry, and
requires exact same-block terminal state; those are explicit trust
compensations, not proof-equivalent production finality. Under ADR 0018 this
Logos-owned limitation is disclosed for production without weakening the local
M3 evidence gate.

Authority was reread again on 2026-07-18: accepted replacement issue #112 is
open, retains the `accepted` and `RFP-003` labels, and explicitly supersedes
issue #61. The live RFP repository baseline is
master commit
`121da225de1930c5ba693ebbef80ee788d55542a` (file blob `d0fa52b`) and accepted
issue #112, whose newline-normalized body SHA-256 remains
`49356263a762307abc0f8dd2863ac5af8fe13d9b17b674f242d025de655f1c87`.
Issue #61 remains superseded.

Accepted issue #112 names six explicit M3-specific outputs:

1. update the LEZ escrow for the BTC adaptor/witness-gated claim;
2. deliver the full-lifecycle LEZ/BTC SDK;
3. supply conformance and swap-specific adaptor vectors;
4. document self-hosted/public Bitcoin testnet, wallet, and funding setup;
5. record happy, refund/timeout, and concurrent BTC demos; and
6. explain the Aumayr and Fournier constructions inline.

Outputs 1–6 are now repository-complete at the private functional-PoC boundary.
The witnessed escrow path is live; the public lifecycle SDK exposes a bounded
canonical secret-free codec, exact CAS store port, typed Bitcoin/LEZ runtime,
restart/replay coverage, and a dedicated wiring example; immutable official
BIP-340/BIP-327 plus project adaptor vectors pass an independent `k256`
cross-check; Core 31.1 self-hosted loopback and exact HTTPS Testnet4 routes are
profile-bound with wallet/funding/flakiness guidance; all three BTC recordings,
private MP4s, and both sealed source/video bundles exist; and ADR 0050 maps the
exact adaptor operations,
assumptions, and atomicity conditions to the two primary papers without
claiming a transferred two-party proof. Public live deployment remains
deliberately deferred, not represented as missing local output evidence.

These six proposal outputs are not the complete acceptance checklist. The
applicable live RFP contracts remain binding, including F2 and F5–F7, U1/U8,
R1–R7, P1, S1–S8, and D1: native/custom assets, taker-first ordering, post-lock
independence, persistence, concurrency, timelock/refund rationale, compute
evidence, CI, tests for every hard requirement, docs, reference integration,
write-up, SDK API documentation, and all three BTC demos.

The RFP does not literally name a maker-second-lock cutoff field. The signed
cutoff is the selected implementation of its combined taker-first,
timelock-margin, atomic-outcome, missing-counterparty, and concurrency duties
at the race boundary. Agreement validation is necessary but not sufficient:
acceptance now has live two-chain absent-maker execution and still requires the
maker-lock admission side of the boundary race evidence above.

The proposal’s named DLC `AdaptorSignature.md` Schnorr corpus does not exist;
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
   only the Taker fixture may submit the first lock, while the schema-4 Maker
   actor must durably submit and reconcile the direction-specific second lock;
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
Only `activate` may insert the agreement acceptance. Strict private config
schema 3 now requires the complete prepared-claim result, distinct Bitcoin/LEZ
session IDs and role-local journals, plus a role-shaped refund object: only the
agreement-selected Bitcoin funder may name its lowercase-hex mode-`0600`
refund-key file. Activation rederives both exact contexts from the signed
agreement, opens journals existing-only, verifies local identities, phases, and
presignatures, requires and point-checks a private taker-only adaptor scalar
without creating a signature, forbids that authority in maker configs, and
refuses any state creation on run, claimant, request, message, journal, secret,
or context drift. The actor gate is 49/49 library tests plus eight CLI
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
Focused LEZ claim tests cover both owned directions, both peerless roles,
deterministic full-field requests and a later window, accepted-without-
projection, finalized-only projection, activation reruns, unavailable and
uncertain observations, `Started`/`Unknown` restart, conflicting bytes or
signature, and an out-of-window containing block. The refund actor cases add
both ordered directions, owner/nonowner roles, pre-deadline state-only reads,
one attempt, accepted/started/unknown restart suppression, recomputed Bitcoin
txid/wtxid and canonical bytes, exact LEZ finality, and terminal revision four
`Refunded`.
Seven signer-journal, fourteen public-effect, eleven BTC-recovery, and all 86
store tests pass; the bridge-client gate is 2 unit, 26 integration, and 3
example tests. This is the persistence
boundary for revisions three and four; run-n now supplies its complete
two-direction actual-node PoC evidence.
`status` reports absent or precreated-empty/no-acceptance state as
`not_activated`; `drive` and `recover` return `NotActivated`. Status may migrate
an existing
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
Step 6 is 2 of 2 for operator-composed actual-chain happy execution. The public
actor source path is also proved through revision four `Completed` for both
chain directions by run `m3actor-20260716n`. Both Bitcoin and LEZ claim-effect
paths have actual-node public-actor evidence at the progressive local PoC
boundary. Public schema-3 two-lock refund composition and actual-node
execution are GREEN in both directions through run `m3refund-20260716h`.
The first-lock-only projector, persistence, signed cutoff agreement, durable
discovery-window baseline, and live refund-side cross-chain gate are GREEN.
Run `m3firstlock-20260716h` closes both fresh actual-node first-lock journeys.
Run `m3survivor-20260716c` cleanly certifies the survivor nuance; maker-lock
admission at the boundary remains open.

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
the local happy-path execution. That packet alone does not prove U8 public-route
execution, the separately evidenced run-H refund demo, concurrency, production
authority, or accepted-proposal M3 completion.

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
databases replay the four lock/claim revisions to `Completed` or the ordered lock/refund revisions to `Refunded`, reject mutated
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
revision four, replay without resubmission, and exact cleanup are audited. That
historical schema-3 happy-path PoC has no remaining execution task. Run
`m3refund-20260716h` separately closes two-lock timeout/refund, and run
`m3firstlock-20260716h` closes actual-node absent-maker recovery. Clean
post-reveal survivor execution is GREEN in `m3survivor-20260716c`. Clean run
`m3schema4-20260717d` closes live schema-4 Maker-lock composition and
actual-node admission in both directions. Clean run `m3overlap-20260717a`
closes the accepted opposite-direction overlapping-swap execution item with
both swaps simultaneously at revision two before either settlement. The F7
witnessed custom-token path, public lifecycle SDK boundary,
official/independent vector gates, Testnet4 route contract, all three BTC
source recordings, and the private source bundle are GREEN. A literal RFP
audit found that D1 requires three demo videos, not only replayable terminal
captures. The RED-GREEN network-isolated renderer/verifier produced all three
private MP4s from retained actual-node evidence at commit `846ba56`; regenerated
source checks, full decode, sampled scenario/atomicity/tail frames, and sealed
bundle `7697a27c...f101ba8` are GREEN. Synchronized evidence documentation and
fresh exact repository gates passed; exact commit `f7fb250f...dcbb2` and
annotated `m3-complete` are published. The private Actions API was unavailable
and the tag makes no remote-green claim. Broader owner-selected hardening
remains below.

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
cryptographic-versus-node proof, output-recovery, and atomicity flow.
[ADR 0040](architecture/0040-continue-post-reveal-from-canonical-evidence.md)
records the protected revealer-absence interval, fresh follower restart, exact
nonterminal remaining-leg proof, and delayed observation-only catch-up. This plan
records both the historical schema-3 lifecycle PoC and the schema-4 actor-owned
Maker-lock PoC as complete local checkpoints. A narrow `m3-complete` tag is
authorized only after the final synchronized gates and clean push; its claim
must remain private-local and functional. GW-M3-001 and Logos production
dependencies remain disclosed, while public execution and the later
owner-selected hardening phases are explicitly deferred rather than represented
as completed.

## Milestone 4 entry plan: XMR spend-key-share end to end

Active phase: **progressive local-functional PoC**. The authoritative deliverable
set is the retained RFP snapshot plus accepted replacement issue #112 snapshot,
inspected on 2026-07-20. The old ETH-scoped issue #61 is not authoritative.

### Actual entry state

### Certified exact-commit local PoC replay (2026-07-22)

Run `m4cert20260722an` is the clean, isolated replay of commit
`5ec65217424c4b976ced662ffcc590ffd5a1713e`. It passed artifact identity and
negative tests, LEZ v0.2 deployment/readiness, fresh Maker/Taker Vault Claims
onboarding, official Monero 0.18.5.1 provenance and peerless Regtest topology,
canonical Stage A/B and role journals, finalized tag 13/14/15, adaptor extraction,
post-fee Maker-destination receipt verification, the canonical
`lez_v02_m4_claim_cross_chain_binding_v1` binder, and exact cleanup. The retained
ledger records `source_exit_status=0`, `evidence=completed`, `cleanup=passed`,
`exact_run_resources_absent=true`, closed sidecar ports, and a surviving foreign
sentinel; no public RPC, faucet, peer, or fund was used. Evidence is under
`.e2e/m4cert20260722an/m4-actual-claim/evidence/`.

This certifies the progressive local-functional M4 happy-path PoC. It does not
claim production readiness or literal F6/F7/U9/D1 closure; signed refund and
punishment branches, token parity, independent crypto review, chaos, and later
QA/security hardening remain explicitly deferred.

M4 has now crossed its first actual local successful-claim vertical. The
underlying component set includes bounded two-party cross-curve DLEQ proofs,
pair-neutral adaptor signatures, canonical countersigned Stage A and Stage B,
checked guest tags 13 through 17, a strict nine-method bridge boundary, durable
tag-13/tag-14/tag-15 planners, role journals, the exclusive release preparer,
sealed one-shot publisher, role-local finalized classifier, and typed official
Monero funding and sweep effects.

Working-tree run `m4happy-40cbac3-20260721a` joined those pieces through actual
isolated services. LEZ Initialize and Fund finalized at heights 3953 and 3960;
the exact Stage-A address received 1 XMR and reached ten confirmations; the
fresh release journal moved through `Prepared` and `Admitted`; tag 14 finalized
at height 4107; tag 15 finalized at height 4208 with custody zero; and the Taker
reconstructed the spend key only after finalized tag-15 evidence and confirmed
the Monero sweep at tip 130. The
[public checkpoint packet](evidence/m4-actual-claim-poc-20260721.json) records
the exact transaction identities without credentials or
private scalar material. It explicitly omits execution-binary hashes because post-run rebuilds changed the
evidence schemas; clean replay must bind the final binaries.

This is deliberately labeled **working-tree evidence pending exact
committed-tree replay**. The run used later uncommitted sources on top of base
commit `40cbac3`; it is not a clean-pushed-commit certification result. The
retained Docker resources also have no cleanup attestation yet. A replay from
the exact committed source plus scoped cleanup is the next certification gate.

The successful claim branch now has actual conditional-atomicity evidence: the
Maker's finalized tag-15 claim reveals the Maker share needed by the Taker to
spend the already confirmed XMR output. This is not a distributed atomic
transaction and does not execute or certify tag-16 signed refund, tag-17
punishment, literal both-refund conformance, custom-token F7 parity, U9, D1,
repeatability, or later hardening.

The official Monero compatibility audit also found that 0.18.5.1 can omit the
`connections` field when the list is empty. The typed decoder accepts omission
as empty only while `get_info` independently proves zero incoming and outgoing
peers; nonempty connections or nonzero counters still fail closed. The first
two live preparer databases remain quarantined after their bounded failures.
Only a genuinely fresh third database was eligible for publication.

### Dependency and specification gate

- [x] Reconcile the six issue-#112 outputs and actual RFP F3/U9/D1 language.
- [x] Prove that `comit-network/cross-curve-dleq` is archived and unlicensed and
  that `xmr-btc-swap` is GPL-3.0; neither is a runtime or vendored dependency.
- [x] Identify maintained `sigma_fun` 0.9.0 (0BSD), `monero` 0.22.0 (MIT),
  `monero-rpc` 0.5.1 (Apache-2.0), and official Monero 0.18.5.1 as candidates,
  without accepting their production use before graph and behavior review.
- [x] Record GW-M4-001 for the unlicensed literal conformance target and
  GW-M4-002 for the underspecified Ed25519-adaptor/LEZ-witness mapping.
- [x] Record GW-M4-003 for the conflict between literal RFP F6 two-leg refunds
  and the cited COMIT punishment branch when the Taker disappears after Maker
  XMR funding. It is a production-conformance disposition, not permission to
  skip local signed-refund or punishment execution.
- [x] Pin and execute the h4sh3d scalar width, endianness, public points,
  subgroup/identity rejection, proof encoding, and transcript commitment behind
  the pair-specific boundary in ADR 0054.
- [x] Extract nonce commitment, partial signing, aggregation, adaptation,
  extraction, point checking, and final verification into the dependency-leaf
  `lez-adaptor-signature` crate. ADR 0056 preserves BTC top-level API and
  byte-exact hash behavior; leaf, vector, facade, process, and direct-consumer
  regressions are green.
- [x] Pin and execute the exact adaptor pre-signature, adaptation, extraction,
  retained-share addition, and reconstructed Monero spend equations. Both
  DLEQ-bound shares, canonical proof wire, symmetric addition, shared address,
  exact claim and refund adaptation/extraction, official-wallet reconstructed
  spends, and generated tag-15/tag-16/tag-17 messages and builders are GREEN.
  Exact Tag16 plus Maker sweep and exact Tag17 finality are actual-node GREEN.
  Joined abandonment economics and independent cryptographic review remain
  separate gates.
- [x] Lock the current crypto-slice graph and pass strict lint, Rustdoc,
  advisories, bans, licenses, and source policy. The rejected unmaintained
  bincode feature was replaced by pinned postcard rather than allowlisted.
- [ ] Complete independent vector compatibility, focused negatives, unsafe and
  cryptographic reachability review before any production-quality claim.

### Progressive happy-path PoC

- [x] Compose the pair-specific SDK, role-fixed actor inputs, Monero RPC
  effects, strict sidecars, release service, and secret-safe records into one
  actual local native-XMR successful-claim journey.
- [x] Execute the reviewed positive direction: Taker LEZ Initialize/Fund,
  exact confirmed Maker-funded XMR output, dedicated tag-14 release, Maker
  tag-15 claim, Taker extraction, and reconstructed official-wallet sweep.
- [x] Retain one public secret-safe packet with ordered effects, transaction
  and finalized-height identities, role boundaries, zero LEZ custody, no public
  resources, and explicit nonclaims. The current packet is a working-tree
  checkpoint and is not clean-commit certification.
- [x] Bind finalized LEZ Claim evidence to the transcript-verified adaptor
  extraction, reconstructed public spend key, original sweep effect, and an
  independent Monero receipt in one create-new Taker-private record. The
  retained legacy-v1-plus-receipt-v2 record has a null exact fee and a checked
  1808400000-piconero unreceived remainder; the current sweep-v2 path proves
  exact fee conservation in focused tests but was not the retained full CLI
  invocation. Destination ownership remains the explicit owner-private
  Taker-wallet boundary rather than a Stage-A commitment.
- [ ] Replay the exact journey from the final committed source, retain binary
  and input identities, prove scoped cleanup without touching unrelated Docker
  activity, and publish the synchronized clean-push checkpoint.
- [ ] Finish and rehearse the run-scoped orchestrator through every role
  handoff, terminal assertion, evidence publication, and scoped cleanup. The
  manual procedure now includes the cross-chain binder, but
  `scripts/run-m4-actual-claim-poc.sh` remains deliberately partial. Its
  source/contract path now starts the run-scoped official Monero child, composes
  canonical Stage A and countersigned Stage B through separate role journals,
  publishes a durable create-new no-retry latch before the exact one-shot
  tag-13 actor, and intentionally fails after tag-13 finality but before
  swap-specific Monero funding. The exact cleanup ledger pre-registers the
  Monero child, captures exact containers, volumes, network, and image, and
  revalidates each persisted run label immediately before deletion; process
  entries revalidate PID start time and executable identity, foreign-sentinel
  survival is mandatory, and broad cleanup is forbidden. This is
  contract-GREEN, not a clean actual replay from the current commit.
  The agreement receipt calls CLI values `requested_terms` and explicitly
  records that the helper itself does not decode and rebind those terms from
  Stage A; the role actors remain the canonical validation and signing
  boundary. The role-sidecar launcher now adopts the exact existing tag-13 state for Taker,
  requires the fixed owner-private receipt/runtime/terms siblings, rejects
  cross-swap terms and state/output overlap, and passes the original typed runtime
  to the child. The typed tag-13-to-tag-14 exporter and bridge receipt gate are
  source/component-GREEN with adversarial coverage. Parent-runner wiring and an
  actual continuation replay remain next.
  Parent-runner integration must continue to stage each checked actor,
  role-runner, and composer binary as an owner-held, mode-`0700`, single-link
  run artifact; the shared `target/debug` cache is mode `0775` with two links
  and is intentionally rejected by the helper trust boundary.
- [ ] Build and scan the final distributable Monero runtime image fail-hard;
  the current official archive and local runtime remain PoC infrastructure.

The happy-path gate is satisfied only by the causal chain now executed: the
Taker locks LEZ first; the Maker funds the exact shared XMR output; the Taker
publishes the activation-bound partial only after confirmation; the Maker's
canonical LEZ claim reveals `s_a`; and the Taker adds retained `s_b` and sweeps
that exact output. A wallet-to-wallet transfer alone would not satisfy it.

This first progressive image covers the successful claim branch only. The next
functional slice must execute the distinct signed tag-16 refund that reveals
`s_b` for Maker recovery and the tag-17 punishment disposition. Those paths do
not retroactively invalidate the successful claim checkpoint, but they remain
mandatory for M4 closure and literal F6 disposition.

### Actual-node checkpoint and next implementation slice

The first complete local claim journey is GREEN as a working-tree checkpoint:

1. canonical Stage A/B and role journals committed the exact claim, refund, and
   punishment messages before either chain effect;
2. actual LEZ tag-13 Initialize and Fund finalized in order at heights 3953 and
   3960;
3. official Monero transaction `de02209c...a8ef8017` funded the exact shared
   address with 1 XMR and reached ten confirmations;
4. the exclusive preparer proved finalized Fund, authenticated peerless
   topology, exact output, and the completed Taker journal, while the separate
   worker admitted only its sealed tag-14 authorization;
5. Maker-side discovery found finalized tag 14 at height 4107, the Maker adapted
   and submitted exact tag 15, and Taker-side discovery found it finalized at
   height 4208 with custody zero; and
6. the Taker extracted the Maker share only from that canonical final signature,
   reconstructed the Stage-A spend key, and confirmed sweep
   `6c8c7bca...70e8e21a` at Monero tip 130. The Taker actor then produced
   a verified owner-private cross-chain binding over LEZ Claim height 4208 and
   finalized tip 4220, plus Monero receipt height 121 and stable tip 130.

The retained example topology used LEZ Bedrock/sequencer/indexer ports
33145/33146/33147, Maker/Taker sidecars 36967/58993, and Monero daemon,
funding, shared/Maker, and Taker wallet ports 39185/41189/46769/58393. Those
numbers are evidence for this retained run only. Reproduction allocates fresh
dynamic literal-loopback ports from run manifests.

No public RPC, P2P peer, faucet, public funds, stagenet, or external finality
service participated. Loopback was transport to real official local processes,
not a substitute daemon or wallet model. Cold setup can still depend on pinned
Cargo/Git sources, circuits, Risc0 tools, the digest-pinned builder/runtime
images, and the verified Monero archive; availability failures can delay setup
without changing runtime chain facts.

Two bounded preparer attempts exposed the official omitted-empty
`connections` response and are quarantined. The decoder fix preserves strict
zero-peer checks, and only fresh `release3` reached `Prepared` and `Admitted`.
The release and sidecar journals remain separate and are not one atomic
transaction. Same-host/different-UID isolation, rollback anchoring,
definitive-absence recovery, and cancellation-after-CAS remain hardening work.

The next certification order is:

1. finish focused tests, strict Clippy/Rustdoc, dependency policy, traceability,
   one end-of-slice static Mermaid check, and secret/diff hygiene for the runner through tag 13
   and the exclusive state lease;
2. commit and push the exact sources and documentation, then replay the journey
   from that clean commit with fresh run IDs and retain scoped cleanup;
3. replace the working-tree qualifier only if the replay reproduces the ordered
   claim and sweep; and
4. after owner transition, begin the signed-refund/punishment functional slice,
   then QA, chaos, information-security, and production-readiness hardening.

The direct finalized-Claim-to-sweep binder and its retained-run invocation are
complete. The replay runner source/contract now composes the official Monero child,
agreement and separate role journals, durable tag-13 no-retry latch, exact
finalized tag 13, typed four-artifact handoff, both role sidecars through
readiness, and local Monero fund/verify evidence. It has not been cleanly
replayed from the current commit. The runner now generates same-run release configuration and invokes the typed Tag14 preparer; publisher, finalized-Tag14 observation, and the retained
successful-claim tail remain, publish synchronized
evidence, and prove ledgered/label-revalidated scoped cleanup. The remaining work is implementation-owned; no Logos or external dependency is currently blocking it. Remaining
runner/PoC implementation is estimated at 1 to 3 focused hours. After that
implementation is complete,
budget 25 to 45 minutes for one warm exact-commit replay or 1 to 3 hours for a
cold replay, evidence rebinding, and scoped cleanup. Full M4 functional closure,
including actual tag-16/tag-17 recovery, F7 native-plus-two-token parity, U9
guidance/CI, D1 XMR recordings, and synchronized closure gates, remains
estimated at 15 to 27 focused hours from this checkpoint. These ranges exclude
owner-selected post-PoC QA, chaos, information-security, and
production-readiness phases and do not let Logos-owned external gaps block
local certification. No ETA authorizes an `m4-complete` tag before the recorded
gates are true.

### Post-PoC RED-GREEN-REFACTOR hardening required for M4 closure

- [ ] Cryptographic vectors and mutations: wrong message/key/point/share,
  noncanonical scalar, identity/small-order/torsion encodings, endian/domain
  substitutions, forged proof, adapt/extract mismatch, and no-panic fuzzing.
- [ ] Agreement and capability gates: XMR-first zero-wire rejection at every
  boundary, exact address/amount/output/profile binding, both role ownership
  views, and no reveal before both locks plus durable recovery material.
- [ ] Node/RPC evidence: exact network/genesis identity, authentication, finite
  bounds, node-versus-wallet disagreement, scan lag, locked output, confirmation
  regression/reorg, wrong output, ambiguous submission, and idempotent replay.
- [ ] Restart/survivor and partial-loss lifecycle after every durable transition,
  including post-reveal Taker continuation without Maker or Chat.
- [ ] Serialize the bridge journal's request-ID check, execution ownership, and
  final outcome so concurrent different bodies cannot both execute or let a
  later error overwrite success; prove CAS behavior under adversarial races.
- [ ] Both recovery cases: absent Maker XMR lock then Taker LEZ refund; funded XMR
  without reveal then canonical LEZ refund event enables only Maker XMR recovery.
- [ ] RFP F7 native/custom-token parity: execute the complete XMR LEZ lifecycle
  for native value and at least two independent custom-token definitions through
  the exact Token/ATA programs and owner, custody, claimant, and depositor ATAs;
  retain substitution, rollback, replay, and terminal-balance evidence. The
  native-only progressive PoC may land first, but it cannot satisfy this item or
  authorize an `m4-complete` tag.
- [ ] Same-direction concurrent swaps with disjoint addresses, transcripts,
  wallets, stores, request IDs, key images, effects, and no nonce/share reuse.
- [ ] Secret custody, encryption/AAD/schema/key failures, zeroization, owner-only
  modes, SQLite/WAL/log/argv/evidence leak scans, and one-attempt outboxes.
- [ ] Chaos and performance: process/node/wallet restarts, RPC outages, timeouts,
  scan lag, LEZ/XMR reorgs, late-lock/refund race, fee/inclusion stress, phase
  timings, repeat count, flake count, and instrumented E2E coverage.
- [ ] Literal M4 outputs: U9 self-hosted/public stagenet guide and funding notes,
  self-hosted stagenet `monerod` CI lane, happy/refund/concurrent D1 videos, full
  SDK lifecycle docs/examples, traceability/review packet, and all closure gates.

ADR 0053 is the component, RPC, flow, isolation, dependency, and evidence entry
decision; ADR 0054 pins the executable two-proof/share boundary; ADR 0055
corrects claim/refund share ownership, claim-partial sequencing, and the
atomicity nonclaims. The annotated `m4-poc-complete` tag now records the exact local PoC replay;
the separate `m4-complete` production tag remains forbidden until all six
accepted outputs are present, the exact clean pushed commit passes the full
repository and M4 actual-node gates, and the tag states every deferred
production/formal-review item without claiming literal resolution of open
proposal errata.

## M5 active work package: application plane

Entered: 2026-07-23. Authority is the live RFP-003 plus Gateway's accepted
replacement proposal issue #112; superseded issue #61 is not an acceptance
source. The proposal's stale F9/R7 references are interpreted by their semantic
text as current RFP F8/R8.

M5 must deliver:

- a Tokio/Rust maker daemon controlled through the Logos Core daemon-mode seam,
  with a real standalone systemd fallback and installation guide;
- a maker CLI that configures pairs/prices, controls and inspects the service,
  lists history, and requests manual claims/refunds;
- a taker CLI that discovers offers, initiates swaps, monitors progress, and
  requests claim/refund;
- a persistent coordinator with crash recovery and concurrent-swap isolation;
- a pluggable price-source contract with local configuration and a Logos-module
  C-API adapter;
- documented graceful behavior while Logos Delivery or Chat is unavailable; and
- a `cargo-fuzz` or equivalent state-machine fuzz harness.

The current executable baseline is intentionally not called M5-complete. It has
an owner-restricted Unix-socket maker daemon, durable pair and exact-price
configuration, a pluggable local runtime price source, swap/alert history,
SQLite recovery machinery, ZEC watcher reconciliation, and property tests. It
also has a taker CLI with key-pinned discovery and ZEC acceptance, daemon-owned
signed run-local Delivery, durable Chat staging and atomic final acceptance,
validated final-wire actor handoff, a literal fuzz target, and a hardened
systemd/future-Core lifecycle seam. The provisional C-API v1 worker and bounded
parent are GREEN against actual C fixtures. Schema v15 now atomically binds an
external module epoch, monotonic quote, policy snapshot, immutable offer, and
request replay result; daemon selection and signed Delivery replay are now
process-GREEN. The persistent coordinator is local-process GREEN through daemon
startup, abandoned-lease recovery, bounded actor execution, and SIGTERM cleanup;
its actual-node, disjoint-live-process, and systemd actor crash/restart
compositions remain open. Symmetric ZEC Taker provisioning is now component-
GREEN: the same role-aware provisioner validates Taker authority, stages an
owner-private role-only bundle, publishes it with `RENAME_NOREPLACE`, excludes
Maker state, and exact-replays the original inodes and bytes. The real `lez-taker`
now exposes ZEC `monitor`, `claim`, and `refund` directly from that role-fixed
config under the shared per-swap kernel lock, without Delivery or Chat.
Acceptance-receipt wiring and receipt-bound offline lifecycle are process-GREEN.
Actual-node Taker command effects, manual effects, autonomous other-pair execution, and post-PoC hardening are still incomplete. The local
Delivery/Chat outage output
is process-GREEN under ADR 0098; LOGOS-020 remains an upstream production-parity
caveat.

### Progressive PoC gate

The first M5 image is one reproducible local happy path through the actual
application binaries:

1. an operator starts the maker daemon on a mode-0600 Unix socket inside an
   owner-only runtime directory and configures one enabled pair plus a static
   price through the maker CLI;
2. the daemon publishes an authenticated, expiring offer through a run-local
   Delivery-compatible adapter and signs negotiation through a run-local
   Chat-compatible adapter;
3. a separate taker CLI identity discovers and accepts that offer, after which
   both role processes persist the same signed terms;
4. the selected stable pair adapter completes against the already pinned local
   foreign-chain and LEZ devnets, with no internal test-only lifecycle call
   standing in for a maker or taker action;
5. Delivery and Chat are removed after the first lock and the swap still reaches
   an observable terminal state from durable chain evidence;
6. daemon restart retains configuration, offer/swap history, and terminal state;
   and
7. the one-command runner records exact binaries, roles, RPCs, transactions,
   finality, duration, external resources, and scoped cleanup.

The PoC uses the existing local LEZ/ZEC corridor first because it is the
shortest already-certified real-node pair. BTC and XMR application-plane
composition remain required before the literal M5 milestone exit unless the
owner explicitly changes that exit gate. No public RPC, faucet, public funds,
or public deployment is necessary; cold dependency acquisition is recorded
separately from runtime dependencies.

### Implementation order and live status

- [x] Reconcile the live RFP and accepted replacement issue #112.
- [x] Audit the current daemon, CLI, coordinator, persistence, pair adapters,
  tests, CI, and living documentation.
- [x] Record the component, actor, trust, and outage design in ADR 0079.
- [x] Replace loopback HTTP with owner-restricted Unix-socket JSON-RPC while
  retaining `jsonrpsee` as the protocol implementation. The daemon now enforces
  owner/mode/path/body/connection limits, no-clobber readiness, and exact-inode
  cleanup; real daemon/CLI restart and alert journeys pass.
- [x] Complete durable application views. Schema v13 pair, exact local-price,
  offer, and swap history are GREEN with global request replay, CAS, rollback,
  migration, and restart evidence. Reservation is one-winner; consumption
  atomically inserts the matching initial coordinator.
- [x] Complete both price-source adapters. The trait, store-backed local
  adapter, owner-local quote RPC/CLI, and restart journey are GREEN. The
  provisional fixed-width C ABI, one-shot `libloading` worker, actual-C fixture,
  typed missing/unavailable results, freshness validation, malformed-response
  rejection, native-abort containment, and bounded parent are GREEN. Schema v15
  adds a per-route/per-module high-water record and commits policy revalidation,
  revision/time/ratio validation, immutable signed-offer fields, and request
  replay together. Preflight returns an exact durable replay before any source
  call; a fresh final transaction rejects policy races, revision rollback,
  observation rollback, and same-revision equivocation. Daemon configuration,
  source selection outside the store mutex, exact replay-before-effect, black-box signed Delivery replay, failed-source rejection, and restart reconciliation are GREEN.
- [ ] Complete maker CLI commands and the taker CLI. The separate `lez-taker` process now discovers daemon-published key-pinned
  signed offers and initiates ZEC acceptance through the isolated Chat socket. It
  validates the exact maker proposal, countersigns with an owner-private raw key,
  exact-replays both mutations, and publishes the final agreement without
  clobber. Status, claim, refund, other pairs, and corridor composition remain.
- [x] Add the bounded signed run-local Delivery adapter with exact maker identity,
  canonical snapshot validation, half-open expiry, immutable publication, and
  daemon-owned publish/withdraw. Startup reconciles SQLite's exact unexpired active, reserved, or consumed retry set
  before readiness, republishing missing files and pruning authenticated stale
  advertisements; the black-box maker/taker process journey is GREEN.
- [x] Add the maker-first canonical ZEC draft/proposal/countersign contract and
  exact no-rounding offer amount conversion.
- [x] Complete documented graceful Delivery/Chat outage behavior for the local application path and the post-lock cutover rule under ADR 0098. The real process journey proves degraded health, durable-first Delivery failure, exact repair replay, no final output during Chat loss, reserved/consumed-envelope restart reconciliation, deterministic taker replay, and one atomic completion. The proposal and final-acceptance process stages are GREEN: a
  disjoint mode-0600 Chat socket authenticates and cross-binds the exact signed
  Delivery envelope and canonical unsigned draft, signs with the pinned maker
  identity, commits reservation plus proposal before response, exact-replays,
  rejects owner/Chat method crossover, and survives kill/reopen. A separate
  taker role validates and countersigns the proposal; the daemon validates the
  exact final wire using its own clock and daemon-local raw recovery/preimage
  authority, then reuses the atomic final transaction before responding. Schema v13
  now atomically stores the exact bounded maker proposal before transport and
  reserves one offer winner with exact replay/conflict/restart evidence. The
  countersigned agreement, coordinator, immutable ZEC binding, protected maker
  claim material, offer consumption, and replay result also commit together
  with forced-rollback/replay/restart evidence. A separate no-authority preparer
  now rebinds only the authenticated transcript of validated local chain facts,
  and the finalizer exact-compares every other body field, both role keys,
  funder ownership and the hash preimage before emitting fresh isolated actor
  state. The opt-in M5
  runner now composes these steps with fresh final actor state, preserves the
  endpoint-tuple lock and one 49-second provision-to-completion clock, keeps the
  restarted application transports through the first confirmed Zcash lock, and
  removes both before later settlement. Exact pushed-tree run
  `m5app-71dd9cc-20260724a` completed in 26.780 protocol seconds against
  isolated LEZ v0.2 and Zebra Regtest, with both actors at revision 4, exact
  Zebra height 107 to 110, post-lock transport removal, no public resources,
  and exact scoped cleanup. The secret-safe packet is
  `docs/evidence/m5-zec-application-corridor-20260724.json`. The preceding
  fail-closed attempts exposed stale typed assertions and a daemon shutdown
  deadlock; corrections and a real-process regression are GREEN.
- [x] Implement the terminal operator-history seam behind schema v14. A stopped
  Maker actor is replayed with unit chain ports through `resume_all_capable`;
  only an absorbing `Completed` or `Refunded` coordinator can enter a separate
  provenance-bound projection table. One `BEGIN IMMEDIATE` validates the exact
  completed Chat agreement and immutable application aggregate before an
  insert-once projection. Exact replay is idempotent; changed input conflicts.
  `swap_status` and `swap_history` overlay only this read model while ordinary
  `load`/`list_swaps` remain effect-authoritative and unchanged. The source
  terminal journal and target projection cannot share one cross-file
  transaction, but the source is already immutable: a crash before target
  commit leaves the old view and retries safely; a crash after commit exact-
  replays; neither path performs a chain call. Focused RED-GREEN persistence,
  injected rollback, invalid/conflicting provenance, the complete swap-store
  suite, maker-process tests, strict Clippy/Rustdoc, shell syntax, formatting,
  traceability, and diff hygiene are GREEN. Run `m5appee8424520260724a` proved
  the seam through fresh actual local nodes and a real restarted owner daemon.
- [x] Exercise the terminal projection through real local nodes before claiming
  its packet. Exact pushed-tree run `m5appaed757f20260724c` reached both actors
  at revision 4, removed Chat/Delivery, offline-replayed the stopped Maker
  actor, and returned `Completed` through a fresh daemon's real owner history
  and status. The runner compared that RPC enum to lowercase actor-status
  spelling and therefore withheld its terminal receipt and result. A focused
  contract reproduced the failure RED; both comparisons are GREEN with the
  real RPC spelling and the contract is now part of the CI quality gate.
- [x] Connect the application plane to the stable local LEZ/ZEC corridor and
  retain one exact reproducible PoC. The one-command `TakerSellsLez` composition
  is exact-tree corridor GREEN. Exact pushed-tree run
  `m5appee8424520260724a` completed in 33.400 seconds with zero same-run retry,
  both actors at revision 4, Zebra 104 to 107, post-first-lock transport
  removal, fresh owner history/status `Completed`, no public resources, and
  exact scoped cleanup. The secret-safe packet is
  `docs/evidence/m5-zec-application-terminal-projection-20260724.json`. Exact
  replay `m5app6c3bbbe20260724a` repeated the same outcome from its pushed
  packet-bearing commit `6c3bbbe` in 27.860 seconds, 56 rounds, and zero retry;
  both actor revisions, fresh terminal owner projection, no-public-resource
  boundary, and exact cleanup remained GREEN.
- [x] Add the standalone hardened systemd unit/install rehearsal and the tested
  Logos Core lifecycle adapter contract. The same daemon now handles SIGTERM,
  publishes typed health and `sd_notify` readiness, accepts systemd's safe
  mode-0400 runtime credentials, and holds one nonblocking process-lifetime
  database lease. The hardened `Type=notify` unit uses encrypted credentials,
  dedicated state/runtime directories, an owner-only socket, bounded restart,
  and system-call/filesystem/capability restrictions. A staged install passes
  `systemd-analyze verify`; actual run `lez-m5-systemd-1000-1141654-16155`
  survived one exact SIGKILL restart with configuration intact and cleaned its
  runtime on SIGTERM in one second; the process adapter proves bounded
  start/health/stop and lease transfer. ADR
  0097 records the component, sequence, atomicity, and upstream-Core boundary.
- [ ] Complete persistent process coordination. ADR 0099 adds a non-default,
  allowlisted real-actor pause only after a secret-free submitted result exists
  and before stdout; its owner-private no-clobber helper and negative cases are
  GREEN and explicitly run in CI, while a real submitted subprocess remains
  open. Schema v16 then persists pair-bound lexical config/program/state
  identities, stable due order, backoff, and owner/generation-fenced attempts
  without any protocol or effect data. Transactional exact registration,
  two-connection same-row exclusion and distinct-row progress, restart lease
  enumeration, half-open backoff, stale-fence rejection, peer isolation, and no
  time-based lease steal are GREEN under ADR 0100. The held-lock RED/GREEN slice
  adds
  a non-cloneable per-swap held-lock capability: secure `openat2`, exact owner/
  mode/link/inode checks, child-only FD-198 inheritance through exact-pinned
  Apache-2.0 `command-fds` 0.3.3, live-child exclusion, and one atomic
  owner/generation transfer that never exposes a queued/unleased row all pass.
  Unsafe parents, hard links, cross-swap capabilities, stale recovery, and peer
  mutation fail closed; full swap-store tests, strict Clippy/Rustdoc, and the
  advisory/license/source gate are GREEN.

  The physical-artifact RED/GREEN slice then secure-opens stable single-link
  config/program files, verifies trusted ownership, modes, bounds, inode
  identity, and exact manifest SHA-256, and copies the bytes into write-sealed
  Linux memfds. Command construction executes only sealed program FD 197, maps
  sealed config FD 196 and lock FD 198, and rebinds the state database as the
  same private inode or same absent path. Tests replace both deployment paths
  before command construction and still execute/read only the verified bytes;
  wrong hashes, symlinks, hard links, unsafe state mode, and unexpected state
  creation fail closed. Full crate tests, strict Clippy/Rustdoc, and dependency
  policy are GREEN.

  The atomic-acceptance RED/GREEN slice now reuses registration inside the ZEC
  completion transaction. Coordinator, agreement, binding, encrypted claim
  material, offer/negotiation mutation, replay record, and immutable queued
  actor row commit or roll back together. The manifest is exact replay identity;
  a changed manifest conflicts and a missing durable actor row fails closed.
  The legacy method remains only for migration/tests; the production Chat path
  has no unscheduled caller after the daemon-owned slice below.

  The ZEC config-capability RED/GREEN slice now changes the real one-shot actor,
  not a fixture. Its CLI requires exactly one private path or inherited FD 196
  and rejects every other descriptor. The inherited route synchronously checks
  an anonymous euid-owned mode-0600 zero-link regular memfd, the 64-KiB bound,
  and all four immutable seals before Tokio exists, then reuses the existing
  strict schema/path/role/agreement validation. The black-box binary test
  replaces the deployment path after sealing and proves the snapshot wins;
  incomplete seals and an ordinary file fail with no actor JSON. Full actor
  tests, strict Clippy, warning-free Rustdoc, formatting, and diff hygiene are
  GREEN. No chain RPC, node, Docker, faucet, or network participates.

  The BTC config-capability slice is also GREEN in the real binary. RED first
  proved the missing fixed-FD CLI and missing agreement commitment. Schema 6 now
  requires the exact signed-agreement SHA-256 for inherited execution while
  preserving schemas 3 through 5 on the path route. Before Tokio, the actor
  enforces the same FD-196 anonymous/mode/owner/link/size/seal contract, then
  exposes its role, state path, commitment, and agreement-derived swap ID for
  supervisor comparison. The actor rechecks the digest before activation. The
  black-box test proves deployment-path replacement immunity and fail-closed
  incomplete seals, ordinary files, and legacy sealed schemas. All 95 actor
  tests, strict Clippy, and warning-free Rustdoc are GREEN; the refactor removed
  duplicated schema alternatives without lint exceptions.

  The daemon-owned provisioning RED/GREEN slice closes that running handoff.
  RED used the real daemon and taker binaries: without actor deployment inputs,
  final completion failed before acceptance instead of creating an unscheduled
  swap. GREEN supplies an existing owner-private Maker template, mode-0700 actor
  root, exact executable, and SHA-256. The daemon validates the template role,
  private canonical parent, all activation material, and executable policy at
  startup, then retains the loaded config and its file identities; replacement
  after readiness fails closed. On completion it rechecks the final agreement
  against unchanged chain facts, the Maker Zcash key, funder role, and preimage.
  It derives the destination from a domain-separated agreement digest, stages
  only a shared agreement plus Maker config/state paths, syncs files and every
  containing directory bottom-up, and publishes with kernel
  `RENAME_NOREPLACE`. Only kernel `EEXIST` enters replay; all other publication
  or sync failures prevent database acceptance. Existing output is accepted
  only as byte-and-semantic exact replay after mutable-state safety checks and a
  repeated durability barrier; no Taker subtree or authority is read or
  emitted. Only then does the same Chat request atomically commit acceptance and
  one queued schema-v16 manifest. The process test proves one queued ZEC row,
  exact role/swap/state binding, no Taker subtree, and delayed replay retaining
  the same row, manifest, bytes, and config inode. Six direct tests prove
  creation/replay, Taker rejection, corrupt collision, unsafe state/journal
  rejection, and concurrent same-wire publication. A post-publication SQLite
  failure can leave only an inert exact-replayable filesystem bundle; without a
  scheduler row it has no execution authority.

  The expiry-independent replay RED/GREEN slice closes the next lost-response
  boundary. A read-only store preflight returns `None` for an absent or rolled-
  back request and otherwise verifies the exact request operation/version,
  offer and reservation, expected revision, final-wire and protected-preimage
  digests, completed negotiation bytes/state/swap, and the complete immutable
  scheduled ZEC actor row. Only that fully matching committed result bypasses
  current-wall-clock agreement parsing and provisioning; changed wire,
  preimage, revision, reservation, offer, or missing actor fail closed. The
  taker now durably no-clobber-publishes its countersigned agreement before the
  completion RPC. On rerun it reopens and validates that private agreement,
  executable draft, both roles/keys, amount, and swap identity, then retries
  only completion instead of rediscovering Delivery or reproposing. The real
  daemon/taker process proof uses a three-second offer TTL, waits beyond expiry,
  and receives the same committed revision/swap as an exact replay. Its stale
  consumed Delivery envelope intentionally makes health `ready` plus
  `degraded`/Delivery-unavailable until reconciliation; the projection cannot
  authorize or erase the durable completion.

  The systemd syscall-policy RED/GREEN slice is statically closed. The packaged
  unit explicitly permits `memfd_create` alongside `@system-service`, retains
  native-only/EPERM policy and `KillMode=control-group`, and its lifecycle
  contract asserts every relevant directive.

  The authority-registry and actor-bearing systemd slice is now GREEN. Chat's
  CLI group requires complete Delivery, claim, registry, root, program, and
  digest inputs. `--zec-source-maker-config` is repeatable and bounded to 256;
  every config is loaded with all activation material at startup. Duplicate
  swap or role-state identities fail before socket/database creation, and final
  agreement provisioning selects only the exact application-swap template.
  The real Delivery/operator journeys use valid authority rather than dummy
  flags, while a repeated registry member fails in the real daemon process.
  The package installs `zec-reference-actor`, requires an exact digest
  environment, and supplies authority/root/program inputs to the hardened unit.
  Staged install and `systemd-analyze verify` pass. Actual user-systemd run
  `lez-m5-systemd-1000-2947208-15620` reached `READY=1`, persisted one route
  across exact SIGKILL restart, and removed the runtime on SIGTERM in 51 seconds
  from a clean Cargo cache with no external resources. The initial RED rejected Cargo's
  group-writable, multiply linked debug artifact; a single-link mode-0500
  deployment copy passed without relaxing policy. Stripping only disposable
  debug sections reduced two-start warm-cache evidence from more than 34 seconds
  to nine; the clean-cache run records build cost separately from runtime behavior.

  Exact-snapshot pair comparison is now GREEN: the store hashes and opens a
  config once, the BTC/ZEC adapter compares Maker role, application swap, and
  role-state path on those bytes, and the same bytes are sealed into FD 196.
  BTC additionally requires schema 6 and revalidates the agreement-derived
  swap. Wrong swap/state and deployment-path replacement tests pass before
  spawn.

  The persistent daemon supervisor is now local-process GREEN. It is explicitly
  opt-in, opens its own SQLite connection instead of sharing the RPC mutex, and
  creates one nonzero 128-bit OS-CSPRNG lease owner per daemon lifetime. Before
  readiness it scans every abandoned lease. Only successful acquisition of the
  exact per-swap kernel lock authorizes an immediate CAS transfer to that owner
  and generation plus one; the recovered row remains leased continuously, so
  no queued or unleased handoff is visible. A live inherited lock leaves the
  old lease unchanged while a distinct due peer still progresses.

  The same loop claims stable due rows, executes exact sealed `status`, selects
  `activate`, `drive`, or BTC `recover`, and retains the lock through the effect
  process and durable owner/generation-fenced resolution. Every spawned PID plus
  Linux start ticks is recorded before waiting and exact-cleared only after
  kill/reap or normal reap. Each command has its own process group; timeout,
  SIGTERM cancellation, or successful leader exit kills lingering descendants
  before the output reader joins. Finite time and output bounds classify
  transient process failure as durable backoff and malformed output/deployment
  as failed without storing payloads. The packaged systemd unit and transient
  lifecycle rehearsal now enable the supervisor.

  Focused evidence is 12/12 store cases and 12/12 pair-neutral supervisor cases.
  One actual-daemon process E2E starts a leased local actor, proves owner health
  remains responsive through the supervisor's dedicated connection, then
  SIGTERMs the daemon and proves cancellation, process-group reap, durable
  non-leased state, child-identity clear, and socket/readiness cleanup in under
  two seconds. These tests use local process, kernel, filesystem, and SQLite
  primitives only; no node, chain RPC, Docker, faucet, DNS, public network, or
  public funds participate. A cold Cargo build may need the pinned registry
  cache or dependency download.

  A second real-daemon process journey now composes two distinct scheduled
  swaps on the same persistent service. Actor A records its PID and exceeds a
  two-second status bound; the supervisor terminates and reaps its process
  group, exact-clears child identity, and commits 600-second backoff. Actor B
  then reports schema-valid allowlisted `Completed` revision-four fixture status
  and commits terminal from a different config, program, state path, lock,
  manifest, and row. Both have one attempt, owner health stays responsive, and daemon restart preserves the
  exact records; both one-entry invocation logs remain unchanged during the
  300-millisecond post-readiness observation window.
  This historical checkpoint closed node-free sequential composition. ADR 0116
  now supersedes its single-worker limitation with simultaneous worker overlap,
  deterministic peer-failure isolation, restart equality, and no replay.
  Accepted-application/actual-chain execution, unavailable-chain route
  isolation, and all-pair composition remain open. Runtime external resources are none: temporary private
  files, SQLite, Unix sockets, and local processes only.

  The user-systemd scheduler crash slice is now node-free process GREEN. Exact
  run `lez-m5-systemd-1000-3497452-2505` reached an allowlisted submitted-effect
  pause in a feature-gated compiled actor, bound its marker to the durable PID
  and start ticks, hash-verified the sealed program memfd and inherited lock FD
  198, then killed daemon generation 1. Systemd restarted the same daemon in ten
  seconds; generation 2 adopted the continuously leased row, advanced its fence
  from 1 to 2, retained the exact fixture-effect inode and SHA-256, progressed a
  disjoint queued peer from generation 0 to 1, and left no leased or child rows.
  The harness now uses a 30-second actor bound so CI inspection cannot race the
  fault seam. Runtime external resources were none. This proves systemd,
  scheduling, fencing, sealed execution, and local effect replay only; the
  fixture is not a Zcash transaction and `actual_zcash_chain_certified` is
  explicitly false.

  The application handoff now installs a private mode-0500, single-link,
  hash-pinned ZEC actor and passes the complete provisioner group to the daemon.
  A secret-free inspector uses the public store API to require exactly one
  daemon-provisioned queued Maker manifest at generation and attempt zero, with
  exact config/program/state paths and digests and no child identity. Acceptance
  and registration already share one SQLite transaction. The actual-node runner
  intentionally continues to drive its separately finalized Maker actor until
  the next atomic slice reroutes effects; the queued row is registration evidence,
  not supervisor execution evidence.

  The 2026-07-28 current-artifact replay now uses the append-only M4 escrow
  guest `dc370bc3...b7292` / ProgramId `4d659033...2c82`; its exact local
  deployment is finalized at block 264 and the M5 runner binds both the raw
  deployment receipt and canonical indexer-finality proof. Two cold-setup
  attempts stopped before creating a run root or chain effect while completing
  the locked sidecar cache and canonical Zebra height-104 maturity bootstrap.
  The first application-layer attempt then exposed a real startup-bound RED:
  validating the 168,579,992-byte debug actor took 20.741 seconds against the
  ten-second handoff bound. Reusing the already-certified systemd packaging
  pattern—private copy, `strip --strip-debug`, final mode 0500, then hash—reduced
  the deployed actor to 33,690,232 bytes and fresh readiness to 4.446 seconds.
  The focused M5 and CI hardening contracts are GREEN; an actual-node replay of
  this correction remains required before upgrading the supervisor gate.

  The next actual-node replay reached the pre-effect LEZ depositor guard and
  failed closed because the deterministic Taker genesis Vault had not been
  claimed into the owner account; no swap effect was submitted. The repository
  now requires M5 mode to ingest owner-private actor-onboarding evidence tied to
  the same channel, current ProgramId, and exact finalized-deployment evidence.
  It validates one canonical finalized Vault Claim per role, the exact configured
  Maker/Taker accounts, expected 100000/200000 balances at nonce one, empty
  genesis Vaults, no automatic retry, no external resources, and no public RPC
  or faucet. The onboarding summary now carries the public owner and Vault IDs,
  and the final M5 result binds its digest, both claim transaction hashes, and
  their finalized block IDs. Focused RED failures for both missing contracts
  became GREEN without weakening the pre-effect guard.

  A clean rebuild must use fresh OS-random Maker/Taker identities before genesis,
  current escrow deployment, and the two canonical claims. That signer-binding
  slice is now component-GREEN. M5 requires absolute canonical owner-private
  signer files, validates their exact lowercase encoding and secp256k1 scalar,
  derives the pinned LEZ v0.2 public-account domain, exact-matches both provision
  accounts, rejects the historical 01/02 fixtures and inode aliases, and copies
  the same bytes into isolated role roots with create-new mode `0600` writes.
  The deterministic M2 fallback remains explicit and unchanged. Eight focused
  derivation, provisioner, and CLI tests plus the M5 contract and strict Clippy
  are GREEN; no dependency was added. Project Cargo output, failed run roots,
  four owned devnet containers, and unused Docker cache were then retired;
  unrelated `gate55` and `pr127` containers remained running. The next gate is
  one cold fresh-identity stack/deployment/onboarding replay through the daemon
  supervisor.

  ZEC actors now expose a role-fixed `recover` command that calls only the SDK's
  existing ordered `drive_refund` boundary. SDK tests prove LEZ-before-Zcash,
  owner-only submission, non-owner observation, early-deadline zero submission,
  ambiguity handling, and terminal replay. ADR 0102 closes the first actual-node
  actor-boundary recovery checkpoint. Run
  `m5fresh-a390dd8-20260728a-app3` deliberately
  stopped after only the Taker-owned LEZ amount 50000 was locked. After expiry,
  the Taker persisted and submitted refund `3a7ffaa5...16e25` exactly once; the
  indexer proves it in finalized block 608 with equal by-ID and by-hash reads,
  terminal metadata, and zero custody. The Taker reached `Refunded` revision 2.
  A focused RED then showed the Maker could not discover that finalized refund
  until the entire future window closed. GREEN now scans the available finalized
  prefix, returns a unique matching transaction immediately, and keeps partial
  absence non-terminal in the adapter. The Maker observed the same transaction
  and reached `Refunded` revision 2 without submission. The full observer suite
  is 26 of 26 GREEN for exact and discovery, both authority variants, deadlines,
  ancestry, ambiguity, stable tips, custody, old pages, and bounded windows; the
  bridge-adapter integration suite is 47 of 47 GREEN.

  This is intervention-assisted actual-node evidence, not a clean reproducible
  proof. The provisioned window 193 through 448 had aged out before the refund
  finalized at block 608; the retained run manually rotated both actor windows
  to 590 through 845 and manually retired one older active bridge-journal row.
  Neither operation was a supported user flow. The current RED-GREEN slice now
  makes the configured page only an initial seed and page size: the existing
  bridge-operation journal atomically advances validated fully covered misses to
  the next contiguous page, retains partial/ambiguous/typed-error polls, restores
  the active page across SQLite reopen, and fails closed on height overflow. Both
  exact-owner and counterparty-discovery paths prove 10..12 to 13..15 progression
  after restart with unchanged config and fresh request IDs. No schema migration
  was required. Replay from an exact pushed tree through the daemon supervisor
  and application CLI still precedes any upgrade of the historical evidence claim.
  The schema-v17 manual-action foundation is now GREEN. One immediate
  transaction binds the existing global mutation request ID, swap, explicit
  `claim` or `refund`, observed generation, open action, and process wakeup.
  The existing process owner/generation lease attaches the action; resolution
  updates both rows atomically; kernel-locked abandoned recovery retargets both
  rows. Exact replay precedes current-generation validation. Four focused
  tests plus the complete store suite prove restart, global request conflict,
  one-open-action, stale-generation and wrong-owner rejection, nonterminal
  requeue, explicit completion, and crash transfer. ADR 0103 records the
  component, sequence, crash, and atomicity diagrams.

  The explicit ZEC claim and supervisor-routing RED-GREEN slice is now GREEN.
  The role-fixed actor exposes literal `claim`, admits only both-locked,
  claim-evidence, or completed phases, and retains generic `drive` only for
  backward-compatible M2/M3 runners. The supervisor attaches an action only
  after acquiring the exact per-swap kernel lock, validates status first,
  selects claim to `claim` and refund to `recover`, and uses command-specific
  outcome and absorbing-phase allowlists. Twelve supervisor tests prove atomic
  action/process completion plus existing crash, cancellation, timeout, peer,
  and terminal invariants; 34 actor-boundary and eight actor unit tests are
  GREEN. No dependency, endpoint, container, RPC, faucet, or public resource
  was added.

  The schema-v18 actor progress path is now GREEN end to end. One bounded
  secret-free observation per actor stores only kind, source generation,
  observation time, and either `not_activated` or validated
  phase/revision/next-action fields. Active revision zero is valid because both
  real actors use it immediately after activation. The supervisor accepts only
  the actual pair-specific phase and next-action vocabularies, enforces
  phase/action/outcome terminal coherence, and replaces status progress only
  with a validated effect. If an effect exits, times out, or is rejected, the
  last validated status remains the committed observation. Process, attached
  action, and progress resolve in one immediate transaction under the exact
  owner/generation fence.

  BTC effect output now exposes the actor-derived next action through the same
  function as offline status. Focused tests prove revision-zero activation, ZEC
  claim/refund completion, BTC completed claim and refund across both roles and
  directions, terminal status without an effect, invalid or regressing-effect status
  preservation, cross-pair rejection, reopen, actor-kind binding, and stale
  rollback. ADR 0104 records the updated component, sequence, and atomicity
  diagrams. No dependency or runtime resource was added.

  The owner-local Maker `monitor/claim/refund` RED-GREEN slice is now
  process-GREEN. Versioned RPC methods on the existing owner Unix socket expose
  only actor kind, scheduler state, current generation, attempt count, validated
  progress, and latest action state. They never expose actor paths, hashes,
  lease owner, child identity, or private role state. Monitor performs only
  application-SQLite reads and has no actor or chain effect.

  Claim and refund require an explicit expected generation and global request
  ID. Exact payload replay returns the original admission after restart; changed
  payload, stale generation, or a second open action fails closed. ZEC supports
  claim and refund. BTC supports refund only and rejects manual claim. The
  black-box daemon/CLI journey proves generation-zero monitoring, claim
  admission and replay, conflict, durable queued-action visibility, daemon
  restart, identical post-restart view, and missing-actor classification using
  no external runtime resource. ADRs 0103 and 0104 carry the updated component
  and flow diagrams.

  Symmetric role-validated ZEC Taker provisioning, acceptance-receipt binding,
  and direct kernel-locked monitor, claim, and refund commands are process-GREEN.
  Fresh acceptance provisions a role-only Taker bundle before completion and
  publishes the bounded receipt only after the Maker durable commit. The receipt
  pins config bytes, role, swap, state, and agreement from one identified read;
  exact replay preserves agreement/config/receipt bytes and inodes. Persisted
  completion replay works with Delivery removed, and receipt-only monitor works
  after both application transports are absent. Seven lifecycle tests plus the
  real Chat process reject tampering, unknown fields, Maker role, ambiguous
  sources, actor-local receipt placement, and lock contention without exposing
  private paths. Direct `--actor-config` is retained only as an expert component
  and recovery escape hatch. Completion-response loss is now process-GREEN:
  a bounded Unix HTTP fault proxy forwards proposal replay, fully observes the
  Maker's successful non-replay completion response after its atomic SQLite
  commit, and drops it before the Taker receives any response. The failed Taker
  publishes no receipt; the Maker is durably `Completed`, the role-only Taker
  bundle is inert and exact, and direct retry reuses agreement/config inodes,
  exact-replays completion, then publishes a fresh receipt. The composed runner now consumes only the acceptance-provisioned Taker config
  and state. Before activation, a dependency-free inspector reuses the public
  rebound-pair invariant to validate the effect-bearing queued Maker and accepted
  Taker bundles. Every receipt invocation is bracketed by exact mode, owner,
  link-count, size, device/inode, and SHA-256 checks. The runner admits legacy
  drive only from the fixed non-claim phase/action pairs and routes the exact
  `claim_evidence_available` plus `claim_zcash` state through one receipt-bound
  `claim` after confirmed-lock transport cutover. Terminal receipt monitor,
  exactly one submitted Zcash follow-up claim, accepted swap ID, receipt digest,
  and admission/effect traces are mandatory result evidence. The focused
  rebound-pair test, warning-fatal Clippy, Rust formatting, syntax, pinned
  ShellCheck 0.11.0, M5 application contract, and diff hygiene are GREEN. This
  is runner-contract evidence; a fresh isolated actual-node execution has not
  yet exercised the new Taker claim route. Two disjoint live swaps and a fresh
  supervisor replay must still pass. A unified XMR lifecycle actor still
  precedes honest all-pair CLI composition. This progress does not by itself
  make M5 complete.

  Project cleanup has reclaimed about 85 GB cumulatively. The latest 2026-07-28
  passes removed 26.4 GiB of rebuildable Cargo targets around verification,
  reducing the repo to about 129 MB and increasing free disk to about 530 GB
  while preserving source, Git,
  fixtures, and unrelated running stacks. Four running project containers were deliberately preserved. Docker inventory
  and
  pruning calls timed out at containerd; Docker was not restarted because that
  could interrupt active project or unrelated stacks. The clean rebuild exposed
  the documented upstream `unzip` fallback assumption; continued verification
  uses the already pinned four rapidsnark v0.0.8 libraries only after exact
  SHA-256 validation and with Cargo offline.
- [ ] After the working PoC, apply RED-GREEN-REFACTOR to restart, concurrent
  isolation, unavailable-chain, outage, stale price, request replay, and manual
  recovery cases.
- [x] Add and continuously exercise the coordinator fuzz target with retained
  regression inputs and bounded CI smoke execution. The isolated cargo-fuzz
  0.13.2/libfuzzer-sys 0.4.13 graph covers every supported pair/direction
  profile, rejected-transition immutability, absorbing terminal states, claim
  evidence, immutable agreement terms, and an exact JSON restart after every
  generated action. Seven seeds, a disposable mutable corpus, 512-run bounded
  CI job, strict Clippy, and graph-local advisory/license/ban/source audit are
  GREEN; ADR 0096 records the boundary and first local run.
- [ ] Revalidate formatting, Clippy, tests, Rustdoc, dependency
  advisories/licenses/sources, image vulnerability scans, isolation,
  traceability, diagrams, secret safety, and exact cleanup.
- [ ] Certify the clean pushed commit, update evidence/manual docs/metrics, and
  create the annotated M5 completion tag only after every literal output is
  proven.

  The first BTC application slice is now SDK-GREEN. `BtcAgreementDraftV1`
  bounded-decodes a canonical unsigned body and shares every non-signature
  executable invariant with final agreement validation;
  `BtcMakerAgreementProposalV1` verifies the body-selected Maker Schnorr
  signature and delegates Taker completion to the unchanged dual-signature
  `BtcAgreementV1` validator. Policy-pinned entry points reject a different
  Bitcoin genesis or confirmation requirement before either role signs. Both
  directions round-trip and complete; wrong-role signatures plus draft and
  proposal schema, commitment, truncation, trailing, and oversize mutations
  fail closed. Warning-fatal
  Clippy and all BTC SDK targets pass. ADR 0106 records component and sequence
  diagrams, the pre-effect resource boundary, and why this is cryptographic
  all-or-nothing binding rather than the still-pending database transaction.
  BTC durable staging/offer binding, atomic completion and actor registration,
  role provisioning, and daemon/Taker CLI composition remain next; the
  under-specified black-box RED test is deliberately not counted as GREEN.

  The schema-v19 BTC durable negotiation slice is now GREEN. One real SDK draft,
  Maker Schnorr signature, and Taker Schnorr signature drive the focused store
  journey. Staging retains the caller-authenticated Delivery commitment, winning reservation, derived
  swap identity, both role keys, exact Bitcoin and LEZ amounts, proposal body,
  and offer quote before the proposal may be exposed. Completion reparses the
  canonical final wire and commits the agreement-derived coordinator, completed
  negotiation, consumed offer, immutable Bitcoin Maker actor, and global replay
  result in one immediate transaction. A trigger at the last mutation insert
  proves all earlier writes roll back. Exact replay verifies the final wire,
  coordinator bytes, consumed revision, and actor without resetting scheduler
  time. A read-only preflight recovers the exact committed actor manifest before
  filesystem provisioning after a lost response. Stage replay verifies both durable request owners, the original half-open reservation window, proposal direction, exact quote, and proposal/offer rows. The lost-response preflight reparses the completed proposal and final agreement, rebinds the exact staged Maker signature, route, quote, coordinator, consumed revision, and immutable actor before returning provisioning authority.

  The full `lez-swap-store` all-target suite is 142 of 142 GREEN. Schema 18 to
  19 migration retains prior global request rows while adding the two BTC
  mutation operations. Warning-fatal all-target Clippy, Rustdoc, formatting,
  and diff hygiene pass. ADR 0107 records component, sequence, resource, crash,
  and atomicity diagrams and arguments. No chain node, RPC, Docker service,
  faucet, DNS, public network, or public funds participated; this is durable
  application authority, not yet a BTC application swap.

  Symmetric role-fixed BTC provisioning is now GREEN. The public Maker and Taker
  entry points accept only a startup-pinned schema-6 source of their own role,
  reparse a canonical dual-signed final agreement with the exact source body,
  preserve role-private signing and recovery authority, and rebind only the
  trusted acceptance time plus final agreement, digest, and lifecycle paths.
  They write a mode-0700 sibling stage with mode-0600 files, synchronize it, and
  publish at one `RENAME_NOREPLACE` linearization point. Exact replay preserves
  bytes and inodes; cross-role authority and a preseeded destination fail without
  output mutation. The full actor all-target suite is 100 of 100 GREEN, with
  warning-fatal Clippy and Rustdoc also GREEN. ADR 0108 records the component,
  publication sequence, atomicity argument, and resource boundary. No node, RPC,
  Docker service, faucet, DNS, network, or public funds participated.

  The BTC application pre-effect process PoC is now GREEN. One black-box test
  drives the real Maker CLI, maker daemon, and Taker CLI through signed Delivery,
  beginning with a Delivery-only daemon that has no Chat, signing, provisioning,
  or actor authority. The Taker planning command authenticates the exact offer
  and derives its reservation-bound swap ID without private material; the daemon
  then restarts with only the selected BTC authority. A canonical draft exporter
  reparses the finalized fixture under its exact Bitcoin policy and creates one
  owner-private no-clobber draft, eliminating duplicated shell representation of
  executable terms before the real process handoff continues through
  pair-isolated BTC Chat proposal and completion, both Schnorr signatures,
  schema-19 activation, independent Maker/Taker schema-6 provisioning, a durable
  final agreement and receipt, and exact replay after Delivery removal without
  replacing either role artifact. Receipt-only offline monitor then reads the
  Taker actor. The exact test passes 1 of 1 in 0.87 seconds. It uses only
  deterministic owner-private fixtures, Unix sockets, local processes, SQLite,
  and files: no node, RPC, Docker service, faucet, DNS, network, or public funds.
  ADR 0109 records its components, sequences, and atomicity argument. This is a
  reproducible pre-effect handoff, not a BTC chain swap. Actual isolated Bitcoin
  Core 31.1 Regtest plus LEZ v0.2 lifecycle execution remains next.
  The first exact locked/offline rerun exposed a create-before-write readiness
  race. Readiness now stages and synchronizes the full socket path before a
  no-replace publication and parent-directory sync. The formerly failing process
  case then passed ten consecutive locked/offline replays, preserving the
  production readiness contract instead of teaching the test to accept emptiness.

The progressive local ZEC application PoC gate closed on 2026-07-24, and the
exact pushed BTC application corridor is also GREEN. After the verified XMR
schema-v2 semantic-supervisor checkpoint and the source/contract-complete
actual-runner splice described below, that corridor is now clean-certified.
A current literal-output audit keeps M5 at 3 of 7 and corrects the remaining PoC
ETA to 14 to 27 focused implementation hours; the milestone-tag ETA is 24 to 43
focused hours after the route-control and multi-worker checkpoints. Update both ranges on every push. The
PoC range covers Maker CLI service start/stop, Taker XMR lifecycle controls,
one-daemon accepted-application and actual-chain concurrency, and honest unavailable-route
isolation. The tag range additionally includes evidence synchronization,
requirements/manual/README closure, a composite M5 evidence verifier, the final
CI and vulnerability gates, secret scanning, and tag review; those tasks
partially overlap. XMR has a real schema-v2 pre-effect actor and a clean composed
legacy chain-effect handoff, but intentionally has no daemon-owned supervised
chain-effect command yet. Broader hardening remains a follow-up unless it
exposes a milestone-owned regression. Shared containerd timeouts add wall-clock uncertainty but are not
counted as implementation time and will not be worked around by restarting a
daemon that owns unrelated stacks.

The BTC actual-node splice has entered contract-first implementation. Its fixed
wrapper permits only the native sequential claim journey and records the exact
Delivery-to-actor order plus isolated Core/LEZ resource boundary. The RED was an
absent wrapper; the GREEN contract, Bash syntax, and CI invocation now pass.
The outer runner now restricts this opt-in to the single supported forward route
without changing the legacy two-direction contract. The direction driver accepts
only a canonical 32-byte Delivery-derived swap ID in application mode and uses it
as stage-two identity; random identity remains the non-application behavior. The
outer runner now starts a registered process-group-isolated Delivery-only daemon,
uses the real Maker CLI to configure the BTC route, exact 1:1000 price, and bounded
offer, and uses the real Taker CLI to authenticate and plan 1,000,000 sats as
1,000 LEZ. It validates the secret-free envelope/reservation-derived ID, stops and
reaps the daemon, and supplies that ID before stage two. Static order, syntax, and
legacy M3 contracts are GREEN; a fresh isolated execution remains the runtime gate.

The next contract-first RED showed that this opt-in route still advertised and
wrote legacy schema 4 actor authority. It now selects schema 6 only in M5 BTC
application mode, computes the exact finalized agreement SHA-256 once, and binds
that digest into each role source config. The M5 source authority begins with one
bounded 4,096-block LEZ discovery window so the later no-clobber application
bundle does not need a changed accepted config merely to advance a scan cursor;
legacy and custom-token routes retain their schema and initial one-block window.
The focused M5 contract, Bash syntax, diff hygiene, and the complete no-binary
M3 orchestration regression are GREEN. The direction runner now exports the
canonical draft, starts an identity-registered process-group-isolated full Chat
daemon with supervision disabled, and invokes the real Taker CLI to complete
both signatures and provision disjoint Maker/Taker bundles before activation.
All nine actor invocations resolve through those provisioned configs. Later LEZ
window requests must stay within the initial 4,096-block bound and update only
diagnostic evidence; neither published bundle nor source authority is replaced.
The focused real-process BTC handoff remains 1 of 1 GREEN in 0.86 seconds, and
warning-fatal feature-complete Clippy is GREEN. The composed runner's static
contract and full legacy M3 regression are GREEN. A clean pushed isolated-node
execution remains the runtime gate, so this checkpoint does not yet claim a BTC
application chain swap.

Exact pushed replay `m5-btc-app-20260730-65cee8e-m` then passed the two local
nodes, current M4 deployment, fresh LEZ identities, Delivery planning, Chat
acceptance, role-only provisioning, daemon shutdown, offline receipt monitor,
both actor activations, and the confirmed Bitcoin first lock. It failed closed
before the first Maker LEZ submission. Retained state proved both actors at
revision 1, zero Maker-lock intents, and zero Maker-lock steps. The cause was a
schema-routing omission: supervised native schema 6 fell through to the generic
found-only LEZ observer instead of the Maker-owned one-attempt send journal.
A focused RED reproduced `ActivationMaterialUnavailable`; the GREEN treats
schema 6 as a supervision overlay over the validated native shape, validates
prepared Maker authority during activation, and routes revision 1 through the
same SDK plan, cutoff check, durable CAS, and ordered send path as schema 4.
All 90 actor unit tests, 11 actor CLI integration tests, and both M3/M5 shell
contracts are GREEN. The failed replay created no LEZ effect and reached no
secret reveal; a new exact pushed replay remains the runtime gate.

That exact replay then entered the correct Maker-owned classifier but exposed a
second fail-closed boundary: the immutable schema-6 config authorized blocks 18
through 4,113, while the local finalized tip was still near the beginning of the
range. The old sidecar refused to scan any block before the complete range
existed. RED tests now cover initialization and funding found in a strict
finalized prefix, initialization prefix uncertainty, funding prefix
uncertainty, forbidden strict-prefix absence, forward finality, and
scanned-end drift. The GREEN treats the 4,096-block config range as immutable
authorization rather than a mutable cursor, reuses the fixed finalized-window
reader for the available same-start prefix, and independently pins its endpoint.
The client accepts only an in-envelope same-start prefix and rejects every
strict-prefix `Absent`; the composed runtime also prevents a future current-state
absence from bypassing the full-window rule. Exact positive evidence may advance,
but incomplete history never creates duplicate-send or recovery authority.

This additive response shape requires coordinated sidecar/client deployment.
Older clients fail closed on the response, producing availability loss only;
they cannot convert a prefix miss into absence or send/refund authority.

All 23 native finalized-observation integration tests and all 35 bridge-client
contract tests are GREEN. ADR 0110 records the component/RPC flow, sequence, and
atomicity argument. Provisioned config bytes, digest, inode, prepared exact
transactions, and role remain unchanged. The next clean pushed isolated-node
replay must prove both ordered LEZ effects and the downstream claim before this
BTC application chain gate can close.

Exact pushed replay `m5-btc-app-20260730-fe0600c-b` then confirmed the Bitcoin
first lock and produced exactly two durable Maker-owned LEZ submissions. Both
the initialization and funding transactions finalized inside the immutable
window, proving that progressive per-step observation works. The replay stopped
before revision 2 because its final complete-lifecycle read still used the
legacy found-only client, which requires the entire 4,096-block range. Fresh
authenticated read-only calls to both additive classifiers returned exact
`Found` evidence from the same-start prefix; no transaction or reveal was
performed by the diagnostics.

The GREEN routes that final read through the additive funding classifier and
persists evidence schema 2 with the exact `scanned_window` and
`finalized_clock`. Only `Found` can project the Maker-lock lifecycle; `Absent`,
`Uncertain`, transport failure, malformed evidence, and shifted/out-of-envelope
prefixes cannot. The focused evidence test, all 90 actor unit tests, all 11 actor
CLI tests, and actor all-target/all-feature Clippy are GREEN. A new exact pushed
replay remains the runtime gate.

Exact pushed replay `m5-btc-app-20260730-0e77fdb-c` then passed deployment,
fresh-identity bootstrap, Delivery/Chat application negotiation, independent
role provisioning, actor activation, the confirmed Bitcoin first lock, both
ordered finalized LEZ Maker effects, and the dual-lock revision-2 gate. It
failed closed at the first revealing-claim submission: the claim classifier
still required the complete 4,096-block envelope, returned unavailable, and the
actor correctly performed zero claim submissions. No secret was revealed and
no unsafe effect occurred.

The claim classifier now uses the same stable same-start finalized prefix
reader. An exact claim may be returned from the prefix; a strict-prefix miss is
the new structural `PrefixUncertain`, never `NotFound`; full-window absence
remains the only chain-absence authority. The client rejects shifted,
out-of-envelope, prefix-`NotFound`, full-window-uncertain, and out-of-prefix
positive facts.

For an owning role only, `PrefixUncertain` may reach a new payload-bearing
journal observation containing the already persisted LEZ claim ID and complete
exact bytes. Inside one immediate SQLite transaction the journal verifies both
against its durable snapshot, permits only LEZ `Claim`, and consumes
`Prepared` to `Started` once. Funding, refunds, Bitcoin claims, payload drift,
peerless discovery, generic node unavailability, timeouts, and transport
ambiguity cannot use this path. `Started`, `Unknown`, or terminal state never
rearms. Only later finalized `PresentExact` evidence can project lifecycle
progress or expose the revealing signature.

The affected workspace gate is GREEN: 91 actor unit tests, 11 actor CLI tests,
45 protocol contracts, 37 bridge-client contracts, 18 public-effect journal
tests, and their remaining package targets all pass locked/offline. The pinned
LEZ v0.2 sidecar gate is also GREEN with 207 tests and warning-fatal
all-target/all-feature Clippy. Workspace formatting, diff hygiene, warning-fatal
all-target/all-feature Clippy, and warning-fatal Rustdoc are GREEN across every
affected workspace crate. ADRs 0036 and 0110 record the component and claim
sequence flows, exact atomicity boundary, and `LOGOS-022` authoritative-indexer
production limitation. A clean pushed commit and exact isolated BTC replay
remain the runtime gate; no M5 tag is claimed.

Exact pushed replay `m5-btc-app-20260730-836f75b-d` then completed both legs.
It passed the hash-pinned deployment/bootstrap, Delivery and Chat negotiation,
role-only provisioning, both actor activations, confirmed Bitcoin first lock,
one finalized LEZ initialization, one finalized LEZ funding, both revision-2
dual-lock projections, one revealing LEZ claim through the exact
`PrefixUncertain` authority, both revision-3 projections, one confirmed Bitcoin
follow-up claim, and both revision-4 `Completed` terminal states. Retained
submission evidence contains exactly two unique Bitcoin effects and three
unique LEZ effects.

The outer wrapper nevertheless exited nonzero during its post-terminal
replay-only tail because that verifier still opened the obsolete source
configs rather than the no-clobber role-provisioned configs that performed the
swap. Direct offline `status`, `drive`, and second `status` calls using the
retained Maker database manifest and Taker acceptance receipt proved both
actors remained revision-4 `Completed` and the replay composed no effect. The
runner now resolves those exact authorities, independently verifies the
canonical agreement digest, recorded config digest, role/schema/state binding,
owner-root confinement, and regular canonical files, while retaining the
legacy source-config route outside M5. Syntax, both M5 contracts, the complete
no-binary M3 orchestration regression, and diff hygiene are GREEN. One fresh
pushed replay must still make the complete wrapper and evidence packet exit
cleanly before the BTC application runtime gate is closed.

Exact pushed replay `m5-btc-app-20260730-992b6d4-e` closes that BTC gate. Both
role actors reached revision 4 `Completed`; the retained manifest contains two
unique Bitcoin effects and three unique LEZ effects; both terminal `drive`
replays returned `not_yet_composed` without changing any effect ID or count.
The wrapper exited zero and attested that every exact run container, network,
volume, image, and secure reservation root was absent without targeting a
foreign resource. The tracked secret-safe evidence packet binds clean
`origin/main` commit `992b6d4`, stable executable hashes, node versions, effect
IDs, timings, external-resource facts, replay, and cleanup. XMR application
composition and the literal all-pair closure gates are now the critical path.

The first XMR application store slice is now GREEN. Schema v20 adds a strict
Stage-A negotiation row and a domain-separated swap ID derived from the exact
authenticated Delivery commitment plus winning reservation. The store parses
and canonically re-encodes the complete dual-signed XMR agreement before its
write lock, then verifies its LEZ-first direction, both role identities,
piconero and LEZ principals, no-rounding offer quote, acceptance window, and
derived ID. One immediate transaction inserts the exact agreement, CASes the
active offer to reserved, and records the global replay result. Exact replay
also checks the complete offer and negotiation rows; the mutation ledger alone
is never authority. Generic offer consumption rejects any staged XMR row.

The focused RED covered absent public types and stage/load methods. GREEN now
covers exact replay and reopen, request conflict, malformed wire, wrong
signature, unsupported direction, wrong reservation-derived ID, wrong quote,
zero-write failure with reusable request identity, forced final-write rollback,
and concurrent one-winner staging. Schema 19 to 20 preserves existing global
requests. The complete store gate is 148 tests, with warning-fatal all-target
Clippy, formatting, diff hygiene, and warning-fatal Rustdoc GREEN. ADR 0111
records current component, reservation, replay, resource, and atomicity flows.
No Docker service, node, RPC, faucet, DNS, public network, or funds participated.
That schema-v20 checkpoint deliberately creates no coordinator, actor, effect
journal, or chain authority.

Schema v21 now closes the Stage-B store boundary. RED first proved missing
coordinator projection and Monero actor persistence. GREEN derives the exact
lowercase-hex coordinator only from canonical countersigned Stage B, including
the signed LEZ finality, Monero confirmations, and canonical LEZ refund-event
schedule. One immediate transaction inserts the coordinator and immutable
Monero Maker actor, changes the negotiation to activated, consumes the reserved
offer, and records global replay. A forced final insert failure restores the
Stage-A reservation with no coordinator or actor. Restart, exact replay,
changed-request conflict, and schema-20 process/manual-action/progress
preservation are GREEN.

Integration review added a second RED-GREEN cycle. Stage-B acceptance now
allows the exact signed Maker funding cutoff and rejects one second later. It
does not reapply the public advertisement TTL after Stage A has linearized the
reservation. The immutable replay fingerprint includes acceptance time, and
replay reloads canonical Stage A plus the complete durable offer route, quote,
activation, coordinator, actor, and mutation state. Corrupt Stage-A or offer
rows fail closed. The scheduling not-before value is intentionally replay
insensitive because it cannot replace the already committed actor manifest.

ADR 0112 records the updated component, commit/replay sequence, local atomicity argument, private-view-key validation boundary, and resource inventory. The real daemon and Taker CLI pre-effect handoff is now process-GREEN. It uses the M4 role-separated Stage-A/Stage-B material, a bounded daemon-owned Maker agreement key, shared private view key, and Maker-only actor-manifest registry. The daemon semantically validates the canonical role-provision manifest against the exact swap and state database before readiness. Stage A remains reserve-only; Stage B atomically activates the coordinator, consumed offer, one Maker actor, and replay row. The Taker no-clobber publishes only its role bundle and receipt.

The exact process proof covers a crossed-reservation zero-write negative, revision-2 Stage A with no coordinator/actor/effect, revision-3 Stage B, Delivery removal, daemon reopen, Delivery-independent exact replay, and actor/receipt byte and inode stability. It uses temporary Unix sockets and SQLite only: no chain node, RPC, Docker service, faucet, DNS, network, or funds. Its exact locked/offline black-box proof is GREEN 1 of 1 in 307.71 seconds.

The schema-v2 semantic-supervisor checkpoint is now independently GREEN. The
real `xmr-maker-actor` accepts only fully sealed config FD 196 and reconstructs
execution authority by rehashing and semantically validating the pinned Stage A
and Stage B, public packets, Maker private manifest and view key, and an
immutable snapshot of the external role journal. The supervisor requires the
exact `xmr-maker-actor` program identity, `lez_maker_xmr_pre_effect_v1` ABI, and
nine-key status contract. The only accepted result is typed `Blocked`,
`chain_effect_executed:false`, revision 0, and
`xmr_chain_effects_not_yet_composed`; no effect subcommand runs, the row does
not enter failure/backoff, and its next authority observation is delayed at
least 60 seconds. The exact real-process proof is GREEN 1 of 1 in 79.22 seconds.
Optimizing only the four portable XMR cryptography kernels reduced complete
authority replay from 194.75 to 29.02 seconds without changing debug assertions,
validation, ordering, RPC, finality, or effect semantics. Runtime chain resources
remain empty.

The opt-in XMR application-to-chain runner is now clean local PoC GREEN.
`scripts/run-m5-xmr-application-poc.sh execute` validates an exact clean commit
and delegates to the existing M4 actual-claim runner with
`M5_XMR_APPLICATION_MODE=1`. The composed order is Delivery-only publication
and authenticated plan; canonical Stage A/B; Maker provisioning; authorized
daemon; real Taker acceptance and role receipt; typed-Blocked supervisor
observation; removal of the original Delivery tree; restart reconciliation of
the consumed retryable advertisement; real-Taker authentication of the
identical swap and terms; archival into an empty Delivery outage; Delivery-free
exact replay; and a synchronous daemon/process-group/socket/readiness cutoff
immediately before the one-shot legacy tag 13 tail. Exact journal
device/inode/size/digest snapshots, artifact byte/inode snapshots, absent SQLite
sidecars, and one swap ID across plan, agreement, provisioning, acceptance, and
replay are required. Cleanup is ledger-, PID/start-time/binary-, and
Docker-label-scoped; an earlier cleanup error is no longer erased by a later
absence check. The runner uses official Monero 0.18.5.1 Regtest and LEZ v0.2
local nodes, ephemeral loopback RPCs, and deterministic local genesis/Regtest
funds only. No public RPC, faucet, peer, or public funds participate.

Four exact-commit attempts stopped before any node, RPC, Docker chain resource,
or tag-13 latch existed. Run `m5-xmr-app-20260730-a5a5d0e-a` exposed missing
XMR SDK edges in the sidecar lock. Run `m5-xmr-app-20260730-edd5217-b` exposed
the same edges plus the newly reachable swap-store graph in the release-service
lock. Both minimal repairs resolve fully offline, and the focused contract pins
both graphs, the exact command-fds checksum, and all store edges. Run
`m5-xmr-app-20260730-7b8ec43-c` then built every repaired graph and stopped
before nodes because the artifact verifier still pinned the pre-M5 bootstrap
SHA-256. The bootstrap change is the default-preserving M5 BTC mode from
`eea4905`; its exact hash chain is refreshed. Artifact `verify-source` now runs
before all heavy builds, turning future source drift from a roughly 14-minute
discovery into a sub-second preflight. Run
`m5-xmr-app-20260730-5f9cb12-d` passed all repaired build graphs and the exact
LEZ artifact proof, then stopped at LEZ stack startup because the outer runner's
pinned `RISC0_SERVER_PATH` was not handed to the nested stack's
`LEZ_V02_R0VM` input. The existing binary has the required
`36c016a5...15b` SHA-256 and reports version 3.0.5; the handoff is now explicit
and regression-pinned. All four pre-runtime cleanups passed.

The fifth exact attempt, `m5-xmr-app-20260730-58e1ee1-e`, passed the repaired
builds, exact artifact proof, local LEZ stack and finalized deployment, both
fresh Vault Claims, official four-service Monero topology, Delivery planning,
canonical Stage A/B, agreement and role journals, real Maker/Taker acceptance,
and the schema-v2 typed-`Blocked` checkpoint. It stopped before tag 13 because
the runner expected the restart publisher not to reconstruct a consumed offer,
while the production store deliberately keeps consumed offers retryable for
lost-response recovery. Cleanup passed with no tag-13 latch or swap-chain
effect. The corrected contract authenticates the identical reconciled offer,
archives it, creates an empty outage mailbox, and only then performs the
Delivery-free replay and cutoff. A fresh exact isolated replay is the next
gate, followed by new
executable concurrent/all-pair and unavailable-route gates. A read-only audit
confirmed those gates cannot honestly be closed by launching the existing
single-worker wrappers in parallel.

Exact pushed-tree run `m5-xmr-app-20260730-da9be26-f` completed the entire
functional corridor. The application cutoff passed with one swap ID, identical
reconciled signed envelope, empty outage mailbox, unchanged artifact inodes and
journal hashes, no SQLite sidecars, and exact process/socket absence. The
one-shot tail finalized tag 13, tag 14 authorization, and tag 15 Claim; extracted
the adaptor scalar; swept official Monero Regtest; and emitted the cross-chain
binding without claiming a distributed transaction. Claim was included at LEZ
height 141 and finalized by tip 146. The Monero sweep received 998191600000 of
1000000000000 funded piconero with fee 1808400000 and 10 confirmations at tip
130. The source returned zero and every exact run resource is absent, but the
fail-closed cleanup result is `failed`: one removal command returned nonzero and
the v1 cleanup packet could not identify which. The run remains functional
happy-path evidence, not clean certification, and its tag-13-latched ID must not
be reused. The cleanup packet is now schema v2 with stable reason codes while
preserving all earlier-failure semantics.

Exact pushed-tree run `m5-xmr-app-20260730-9067ba3-g` then repeated the entire
functional corridor with source status zero. Claim finalized at LEZ height 140
and tip 143. The Monero sweep again received 998191600000 of
1000000000000 funded piconero with fee 1808400000 and 10 confirmations at tip
130; cross-chain binding completed without a distributed-transaction claim.
Every exact resource is absent and the foreign sentinel survived. Schema v2
identified exactly three `ephemeral_path_boundary_failed` reasons, all for
nested directories below the exact run-owned private namespace. The guard had
admitted the namespace but rejected its children before later removing the
namespace itself. Commit `fb4e279` canonicalizes paths and permits only
descendants of the run-owned private root while regression tests continue to
reject traversal, symlinks, and foreign paths. The cleanup result correctly
remains failed for run G.

Evidence correction 2026-07-30: audit of run H proved a runner-owned role inversion. The provisioner funded, the Taker RPC hosted the shared wallet, and the Maker received the sweep. Preserve H as historical cryptographic, finalized-chain, sweep, binding, and cleanup evidence only; do not count it as role-correct user-flow certification. The runner and focused source contract now require Maker funding and claim mining, a distinct neutral provisioner shared-wallet process, and Taker receipt. A fresh clean exact-commit replay remains required.

Current corrective slice:

- [x] Correct the claim runner to the three-origin Maker to neutral shared wallet to Taker topology.
- [x] Add a fail-closed source contract preventing either economic actor RPC from hosting the shared wallet.
- [x] Run and retain one fresh exact-commit role-correct claim replay.
- [x] Enable authenticated Taker tag-16 preparation and aggregate completion with independent durable replay and zero submission.
- [x] Add transaction-derived one-attempt tag-16 submission and finalized Taker-exact plus Maker-discovery classification.
- [x] Add Maker finalized-signature ingestion, extraction, role-correct reconstructed-key sweep, binding, and exact actual-node evidence.
- [ ] Close final application-owned effects, accepted-application concurrency, unavailable-route composition, and milestone gates. Tag17 is separately actual-node GREEN under ADR 0158.

Exact pushed-tree run `m5-xmr-app-20260730-2c6aec1-h` then repeated the full
corridor from commit `2c6aec1` and passed cleanup schema v2. Swap
`9d627d18...abfeb7c` retained one identity through application cutoff, tag
13/14/15, adaptor extraction, sweep, and binding. Claim transaction
`05cb9052...349fce` was included at LEZ height 139 and observed at finalized tip
142. Monero sweep transaction `37930570...1603c8` received 998191600000 of
1000000000000 funded piconero after a 1808400000 fee and reached 10
confirmations at tip 130. Source status was zero; every exact resource, process,
and port was absent; the foreign sentinel and no-retry latch survived; no
foreign or broad cleanup occurred; and `failure_reasons` was empty. The binder
claims conditional successful-claim atomicity, not a distributed transaction or
future-reorg immunity. ADR 0114 is accepted at this local PoC boundary.

The current RFP/issue audit keeps literal M5 completion at 3 of 7. Fixed
packaged-system-service start/stop is GREEN. Remaining implementation is the fresh role-correct actual claim replay, Maker
tag-16 recovery and Taker XMR claim/refund controls, honest accepted-application
concurrency under one daemon/database, and automatic unavailable-node behavior
including unaffected-pair progress. After the lifecycle-control checkpoint, M5 PoC ETA is 8 to 18 focused hours
and milestone-tag ETA is 18 to 32 focused hours.

### M5 tag-16 one-attempt submission and classification checkpoint (2026-07-30)

RED proved that the generic submission route rejected the completed native-XMR
tag-16 refund and that the protocol accepted a finalized refund one millisecond
before its signed deadline. GREEN admits only the exact durable completed
transaction under its transaction-derived request ID, persists a one-attempt
outcome before restart, and validates Taker-owned or Maker-discovered finalized
refund facts only in `[refund_at, punish_at)`. A deliberately ambiguous
sequencer response performs one lookup and one send; exact replay after sidecar
restart returns the same unknown result with zero additional node calls.

REFACTOR extracts the durable tag-15/tag-16 submission and effect validators,
keeps tag 17 unavailable, and passes 9 protocol cases, 9 authenticated XMR route
cases, 2 finalized-classifier cases, 30 sidecar library cases, and strict
all-target/all-feature Clippy. ADR 0120 records component, sequence, and
conditional-atomicity diagrams. This is a controlled component checkpoint, not
an actual local-devnet refund: Maker ingestion, adaptor extraction, exact
Stage-A key reconstruction, neutral shared-wallet sweep to Maker, Taker-mined
confirmations, cross-chain binding, and fresh role-correct replay remain.
Literal M5 therefore stays 3 of 7. Updated remaining ETA is 8 to 18 focused
hours for M5 PoC and 18 to 32 focused hours for reviewed tag closure.

### M5 explicit route-control checkpoint (2026-07-30)

RED reproduced a real application defect: after committing a disabled Zcash
route with a valid local price, the owner CLI still returned a Zcash quote.
Offer publication already rejected the route. GREEN adds one guard in the
shared quote selector before local or Logos C-API price-source I/O. REFACTOR
extracts the black-box fixture setup into pair-scoped helpers and passes the
complete four-journey operator test, warning-fatal Maker Clippy, and
warning-fatal Maker Rustdoc.

The executable journey disables Zcash, keeps Bitcoin enabled, rejects Zcash
quote and publication with stable JSON-RPC `-32602`, proves the Bitcoin quote is
unaffected, restarts the daemon on the same SQLite database, repeats both
outcomes, then re-enables Zcash with expected revision 1 and obtains its quote.
No chain RPC, Docker service, faucet, DNS lookup, public network, or funds
participate. ADR 0115 records the component, sequence, and local-isolation
argument; Flow 1S gives the operator reproduction.

This closes explicit pre-publication route control only. Automatic unhealthy-
node detection, withdrawal of an already active offer, mid-negotiation policy,
and an actual unaffected-pair application while another node is absent remain
R3 work. Literal M5 therefore remains 3 of 7. Remaining order is: accepted-application and actual-chain overlap, Maker CLI systemd start/stop, receipt-bound
Taker XMR monitor/claim, the missing tag-16 XMR refund execution path, then
composite closure/security/evidence review. Updated ETA after the route-control and multi-worker checkpoints is 14 to 27
focused hours to M5 PoC and 24 to 43 focused hours to the reviewed tag.

### M5 bounded multi-worker checkpoint (2026-07-30)

RED passed `--actor-worker-count 2` to the real daemon and failed at readiness
because production exposed no such option. The first GREEN created bounded
worker runtimes but the overlap assertion exposed scheduler starvation when two
workers raced for the same first due row. Runtime ordering now claims due work
before scanning for an abandoned lease; startup still exhaustively recovers
abandoned leases before publishing readiness.

The daemon accepts 1 through 32 workers only with explicit supervisor opt-in,
opens one WAL SQLite connection per worker, reuses one random daemon lease
identity, and runs exactly N scoped OS threads under one aggregate task. Every
thread retains the existing per-row CAS/generation fence and per-swap kernel
lock. A worker return or panic cancels its peers through a guard; the aggregate
joins every thread before daemon shutdown completes.

REFACTOR replaced timing-dependent sleep/timeout observation with an owner-
private release-gated fixture. One terminal actor completes while a disjoint
actor remains live and Leased with its exact child identity. Releasing the peer
to a nonzero exit changes only that row to Backoff. Both manifests, state paths,
one-attempt generations, child cleanup, responsive owner health, restart
equality, and no replay are asserted. The exact case passed 10 of 10 repetitions
in 0.49 to 0.54 seconds; daemon CLI tests, both process journeys, strict Clippy,
and strict Rustdoc are GREEN.

ADR 0116 and Flow 1H record the component, commands, worker sequence, and local
authority argument. Runtime external resources are none: no node, chain RPC,
Docker service, faucet, DNS, network, or funds. This proves persistent daemon
worker concurrency, not yet two accepted application agreements with distinct
escrows/deadlines and actual chain effects. Literal M5 remains 3 of 7; updated
ETA is 14 to 27 focused hours to PoC and 24 to 43 hours to reviewed tag closure.

### M5 fixed Maker service-control checkpoint (2026-07-30)

RED ran the compiled `lez-maker --help` and proved that literal `start` and
`stop` were absent. The initial GREEN exposed a system-or-user scope, but the
architecture audit found that only the system unit is packaged: the existing
user-systemd fixture owns a uniquely named transient rehearsal unit. REFACTOR
therefore removed the unsupported scope and made both commands target only
`lez-maker-daemon.service` through fixed `/usr/bin/systemctl`.

Both the lifecycle action and exact `ActiveState` query now have a 30-second
deadline. The adapter discards action stdout and all stderr, caps state output at
33 bytes, kills and reaps the exact timed-out child, and reports uncertain state
instead of inferring success or failure. Start requires `active`; stop requires
`inactive`. Unit tests prove exact argument vectors, output modes, JSON,
nonzero-action/query behavior, malformed and oversized states, opposite states,
timeout mapping, and secret-output redaction. The black-box CLI test proves both
subcommands and rejection of caller-selected scope, unit, or socket flags.

ADR 0117 supplies component, sequence, failure, and atomicity diagrams. Flow 1D
now uses the real commands, distinguishes host administration from service-user
RPC, gives exact JSON and timeout/retry behavior, and fixes its pre-existing
broken `useradd` recipe. The focused slice uses no chain node, RPC, Docker,
faucet, funds, DNS, public network, Delivery, Chat, or finality service and does
not touch the host unit during CI. The daemon database lease and existing
per-swap transactions/fences remain the authority, so repeated systemd actions
cannot create a second writer or bypass swap atomicity.

This closes the literal service-control sub-gap but not the full F9/U3 output;
M5 remains 3 of 7. Remaining order is receipt-bound Taker XMR monitor and claim,
the missing tag-16 refund execution path, accepted-application plus actual-chain
overlap, automatic unavailable-node composition, and composite evidence,
security, documentation, and tag review. Updated ETA is 8 to 18 focused hours
to the M5 PoC and 18 to 32 focused hours to reviewed tag closure.

### M5 XMR Taker receipt-only monitor checkpoint (2026-07-30)

RED extended the real Maker/daemon/Taker XMR process journey after accepted
Delivery withdrawal and daemon shutdown. The previously ZEC/BTC-only lifecycle
loader rejected the genuine XMR acceptance receipt, proving the missing U4
monitor route without introducing a fixture-only command.

GREEN adds XMR receipt selection to the real `lez-taker monitor --receipt`
path. The selector bounds and strictly decodes the owner-private receipt, pins
the referenced manifest bytes to its SHA-256, and derives the exact swap and
state-database lock identity. The CLI then acquires the shared per-swap kernel
lock before full semantic validation of Stage A, Stage B, Taker manifest,
packets, private authority, and both presignature-verified role sessions. It
compares the resulting authority back to every receipt-bound path, digest,
commitment, swap, and state field before emitting one fixed secret-free object.
This ordering avoids a validate-then-lock TOCTOU gap and serializes the read with
the actor worker. Monitoring writes no actor or application state.

REFACTOR keeps XMR claim and refund fail-closed with the stable public error
`XMR Taker claim and refund are not yet composed`. Ambiguous or invalid
receipts, held locks, unsafe authority, and changed receipt semantics also map
to bounded stable errors without authority paths or bytes. The real-process
journey proves monitor output after Delivery and Chat disappear, unchanged
accepted artifacts, unsupported-effect rejection, unknown receipt-field and
manifest-digest rejection, and secret-free stderr.

Flow 1T records the reproducible command and exact JSON. Runtime external
resources are none: no Monero or LEZ node, chain RPC, Docker service, faucet,
funds, DNS, public network, Delivery, or Chat participates. The output is
pre-effect application-authority status only; it does not observe or infer
current or enduring chain progress. Inherited ABA hardening for authority paths
that must be reopened during semantic validation remains production work.

This closes only the receipt-bound XMR Taker monitor sub-gap. Literal M5 remains
3 of 7. XMR Taker claim/refund effect composition, tag-16 refund execution,
accepted-application plus actual-chain overlap, automatic unavailable-node
composition, and composite evidence/security/tag review remain. The existing
updated estimate is 8 to 18 focused hours to the PoC and 20 to 36 focused
hours to the reviewed tag.

The complete hash-pinned CI quality gate is also GREEN, including ShellCheck
0.11.0, workflow/Docker/Compose lint, every M3/M5 shell contract, and Testnet4
security contracts. Its extracted legacy timing fixture now declares non-M5
mode explicitly.

Logos Core daemon mode is acknowledged by issue #112 as not yet delivered.
Until Logos publishes that capability, M5 tests the lifecycle contract against
the same daemon binary and records the missing upstream integration in the
production-blocker register. The standalone systemd, local control, persistence,
and restart user flow are now GREEN; only the live upstream Core attachment is
deferred under that exception.

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
| Final E2E must represent actual users | The real taker CLI now owns ZEC discovery, proposal validation, countersigning, completion, replay, and final-wire persistence; chain lifecycle remains separate | Extend the same role boundary through actor activation and real chain adapters before labeling the application flow full E2E |
| Prototype local RPC still uses loopback HTTP and an environment capability | Tower rejects a Bearer header before JSON parsing and non-loopback binds are refused | Move to an owner-restricted Unix socket and credential file before M5 freeze |
| Daemon prototype serializes SQLite with a mutex on blocking workers | Safe for the two-method operator slice, not chain watcher concurrency | Introduce the ADR-0003 single writer actor and atomic outbox before mutations expand |
| Trade direction was unstated in both contractual sources | ADR 0008 now separates taker-first funding from construction-specific claimant order; ZEC's chain order comes directly from RFP F4 | Keep direction immutable; BTC/ZEC allow both only through their reviewed actor/chain flows, while XMR remains LEZ-first only |
| Primary COMIT implementation does not support XMR-first | Pinned commit `dc6ba84…` explicitly ships scriptable-chain-first only | Reject `TakerSellsForeign` for XMR in core and actual CLI; require a new reviewed construction to supersede ADR 0008 |
| Dependency advisories can appear without a source change | The required `cargo-deny` job runs on every push and pull request and explicitly includes `advisories` | Keep advisories hard-failing; investigate and remediate rather than adding broad ignores |

## M5 role-correct tag-16 refund continuation checkpoint

The component boundary after ADR 0120 is GREEN. A real Taker process now loads
the role-fixed Stage A/B material, validates the refund session and aggregate
signature cryptographically, calls authenticated prepare and complete with
distinct request IDs, and submits only under the transaction-derived canonical
identity. Its process proof observes one submission and no retry; crossed role,
session, signature, and request identities fail before sidecar I/O.

The Maker reference actor now ingests only canonical finalized tag-16
`DiscoverByTerms` evidence, re-derives its exact durable refund session, and
writes the observed signature into the existing role-local journal-linked
packet format. The existing Monero sweep engine is now symmetric without
duplicating wallet logic: claim retains Taker share plus Maker scalar and pays
Taker, while explicit refund uses Maker share plus Taker scalar and pays Maker;
the opposite role wallet mines confirmations and the shared-wallet process
remains neutral. Legacy claim evidence stays v2-compatible and refund emits an
honest journey-bound v3 schema. ADR 0121 records current components, RPCs,
sequence, and the conditional atomicity argument; Flow 1V reproduces the tests.

At this historical checkpoint literal M5 remained 3 of 7 and the next order was
runner wiring, the refund/receipt binder, and a fresh role-correct two-devnet
replay before actual-chain concurrency and unavailable-route/operator surfaces.
Those refund steps are superseded by exact run `m5xmrrefund45924caa` below.
Tag 17 remains a separate punishment-path requirement; the historical 6-to-14/
16-to-28-hour estimates are superseded by the current 10-to-20-hour tag ETA.

## M5 reproducible application-refund runner checkpoint

The bounded role-correct refund tail is now working-tree GREEN. It is opt-in
through `M5_XMR_JOURNEY=refund`, requires application mode, keeps the historical
claim path as the unchanged default, and rejects refund deadlines outside the
600000 through 3600000 millisecond local profile. The default schedules refund
900000 milliseconds after agreement composition and leaves a 600000
millisecond punishment window.

The runner obtains time only through the authenticated Maker classifier. It
waits for a finalized clock in `[refund_at, punish_at)`, fixes discovery at the
next height, adapts the Taker refund presignature, submits tag 16 once through
the real Taker process, and requires canonical Maker `DiscoverByTerms`
finality. The Maker then ingests that exact aggregate signature into its
durable refund session, extracts the Taker scalar, reconstructs with its own
Monero share, and sweeps the neutral shared wallet to the Maker wallet while
the Taker wallet remains the independent foreign observer and confirmation
miner.

The new owner-private binder revalidates the Maker Stage A/B material, durable
refund session, finalized tag-16 facts, observed aggregate signature,
extraction proof, refund-v3 sweep, and independent receipt. It requires honest
`taker_refund_signature` and `maker` roles, exact
`funded = received + fee` accounting, isolated Regtest topology, no public RPC,
no faucet, and no automatic retry. Its scope is explicitly successful-refund
conditional atomicity; it claims neither a distributed cross-chain transaction
nor future-reorg immunity.

Focused actor tests, all actor targets, strict Clippy, warning-fatal Rustdoc,
Bash syntax, the preserved M4 claim contract, the M5 application contract, and
diff hygiene are GREEN. ADR 0122 records components/RPCs, sequence, and the
atomicity argument; Flow 1W is the manual clean-replay procedure. At this
historical checkpoint it was not yet actual-node evidence: the next gate was a
fresh exact pushed-commit run on isolated LEZ v0.2 plus official Monero
0.18.5.1 Regtest. Exact run `m5xmrrefund45924caa` below supersedes that open
gate with retained binding and cleanup evidence.

That checkpoint kept literal M5 at 3 of 7 pending the refund replay and accepted
application-output closures. The refund replay is now GREEN; remaining work is
accepted-application actual-chain overlap, complete Maker and Taker lifecycle
surfaces, unavailable-route composition, and the final evidence/security/tag
review. Tag 17 stays recorded as protocol punishment-path work rather than a
separate literal issue-#112 M5 output. Literal M5 still remains 3 of 7, and the
current closure estimate remains 10 to 20 focused implementation hours as
recorded by the final correction below.

## M5 local finalized-clock liveness RED and component repair (2026-07-31)

Clean attempt `m5xmrrefund8c10cd7a` reached application acceptance, finalized
tag 13, and verified Maker-funded Monero output. It then returned the same
authenticated finalized identity at height 120 for more than two minutes. The
local sequencer did not finalize empty blocks, so read-only classifier polling
could not advance the finalized timestamp into the signed refund interval. Host
time eventually passed `punish_at`, but it was never used as refund authority.
The run was stopped and scoped cleanup completed. It is diagnostic RED evidence,
not an actual refund replay or milestone result.

The implemented correction is one local-profile-only liveness effect after two
identical finalized samples. Activated terms seal a one-native-unit
authenticated transfer from the Taker depositor to the Maker claimant. The
Taker sidecar, not the runner, holds the signer. One create-once durable
reservation binds run, runtime, terms, recipient, cutoff, nonce, exact bytes,
and transaction ID. The transaction then crosses canonical
`SubmitTransaction` once under its transaction-derived request identity; an
ambiguous result is sticky and never retried. Read-only post-state must prove
canonical inclusion, Taker balance minus one and nonce plus one, Maker balance
plus one with unchanged nonce, and byte-identical escrow metadata and custody.
The runner then returns to read-only Maker classification. Only a new finalized
classifier result inside `[refund_at, punish_at)` can authorize tag 16.

ADR 0123 records the component/RPC, sequence, liveness, and conditional-atomicity
diagrams. Flow 1W records how the future operator replay will expose and verify
the tick. Runtime remains isolated LEZ v0.2 plus official Monero 0.18.5.1
Regtest on ephemeral loopback endpoints and deterministic local funds; no public
RPC, faucet, peer, DNS, or public funds participate. Cold artifact availability,
host pressure, local finality cadence, the bounded tick-finality wait, and the
signed punishment margin remain explicit flake or failure sources.

Historical checkpoint status was **component-GREEN; corrected actual replay
pending**. The eight-thread reservation RED exposed a partial-publication race
fixed by a narrow planner mutex. All 325 Rust tests, strict Clippy/Rustdoc,
compatibility contracts, and the complete quality/security/vulnerability gate
passed. Exact run `m5xmrrefund45924caa` below supersedes the replay-open status;
this historical checkpoint itself created no output-count change or tag.

Replay `m5xmrrefund827a5d4a` then passed both devnets, checked deployment,
role-correct application handoff, finalized tag 13, and Maker-funded Monero
verification before exposing a pre-effect integration RED at the signed refund
threshold: the clock preparation request ID exceeded the protocol maximum. Zero
clock effects were emitted and scoped cleanup passed. A new TDD regression test
now proves versioned, operation-domain-separated SHA-256 prepare/verify IDs are
deterministic, distinct, safe-grammar, and exactly 64 characters. The sidecar
suite is 215 GREEN and strict Clippy, Rustdoc, runner-contract, formatting, and
diff gates passed. That replay-open status was historical and is superseded by
exact run `m5xmrrefund45924caa` below.

## M5 finalized-tip observation correction (2026-07-31)

Clean run `m5xmrrefund842610ca` admitted exactly one terms-sealed clock effect,
advanced the sequencer from height 193 to 194, and proved one submission, exact
accounting, unchanged escrow state, and ten Bedrock descendants under the
configured security parameter in about 16 seconds. It then failed because the
runner repeatedly classified immutable block 120. The classifier was correct:
its clock describes the requested effect window, not the current finalized tip.
Longer sleeps, more transactions, or lower security parameters cannot fix that.

The focused RED required a distinct authenticated read-only current-finalized-
tip boundary. `lez_bridge.v1.observe_finalized_clock` now returns the stable,
genesis-bound official-indexer head through the existing production reader. The
client enforces exact context/runtime echo and nonzero identity/time, the driver
uses fresh SHA-256 request identities while polling for at most 60 seconds, and
the runner scans exactly the returned finalized height for the effect. The one
terms-sealed transaction and one-attempt submission invariant are unchanged.

Focused GREEN evidence is protocol 46 of 46, client 38 of 38, sidecar current-
clock 3 of 3, clock-driver 1 of 1, and the M5 runner contract. The complete
sidecar and root test suites are GREEN. The default root suite also exposed and
fixed a repository-hygiene defect: Cargo now skips the feature-gated systemd
crash example by default while still compiling it under `test-crash-hooks`.
Strict Clippy and warning-fatal Rustdoc pass across every root and sidecar
target/feature, as do the repository CI/security policy, Docker-isolation,
compatibility, and dependency-policy gates. The refreshed advisory database
found `RUSTSEC-2026-0220` in sidecar-transitive `ruint 1.19.0`, introduced
through the Logos v0.2 RISC Zero graph. A surgical lockfile update to fixed
`ruint 1.20.0` passes the complete sidecar suite, strict Clippy/Rustdoc, and
`cargo deny`; no advisory waiver was added.

Literal M5 remains 3 of 7. Four outputs remain: daemon-owned effect-bearing
accepted applications; complete supported-pair Maker CLI lifecycle; complete
supported-pair Taker CLI lifecycle; and accepted-application actual-chain
concurrency with restart isolation. The concurrency run should also compose an
unavailable XMR route while unaffected BTC/ZEC work. Final evidence, security,
manual-flow, diagram, cleanup, push, and annotated-tag review follow those four.
Logos-owned production blockers stay disclosed but do not block local M5.

Exact pushed-commit run `m5xmrrefund45924caa` closes that gate. It completed
application acceptance, tag 13, Maker-funded Monero verification, exactly one
sealed clock transaction, authenticated finality advance from height 188 to
192, finalized tag 16 in block 198, Maker refund ingestion/extraction, the
Maker-directed Monero sweep `252b922e...d4caf` with ten confirmations, the
conditional cross-chain binding, and cleanup schema v2 with source exit zero.
The clock transaction used one submission attempt, exact `-1/+1` balance and
`+1/0` nonce deltas, and byte-identical escrow metadata/custody. The retained
packet is `docs/evidence/m5-xmr-application-refund-corridor-20260731.json`.

The cold replay took about 61 minutes end to end; approximately 24 minutes 35
seconds were repeated run-private Cargo builds. A read-only audit found a safe
future optimization: content-addressed, hash-verified, read-only artifact
bundles can preserve exact source/lock/toolchain/features/native-input/ELF/
ImageID provenance while avoiding shared writable target directories. Do not
weaken final certification by blindly sharing `CARGO_TARGET_DIR`.

Literal M5 remains 3 of 7 because this refund corridor is a prerequisite, not a
separate issue-#112 output. The next gate is the already-wired daemon-supervisor
ZEC application happy path from a fresh pushed commit. It must prove the daemon
alone owns Maker effects, restart after lock without Delivery/Chat, exact
terminal fenced state, zero duplicate submission, fresh owner history, and
scoped cleanup. Current closure ETA remains 10 to 20 focused implementation
hours.

## M5 repository-wide ruint remediation and artifact re-attestation

The 2026-07-31 advisory refresh found `RUSTSEC-2026-0220` in multiple nested
repository-controlled Cargo graphs, not only the v0.2 sidecar. The RED gate was
reproduced directly in the deployable SPEL guest at `ruint 1.17.1`. Every one
of the ten repository lockfiles that contains `ruint` now resolves `1.20.0`,
the final direct guest pin is exact `=1.20.0`, and all 13 CI dependency graphs
pass advisories, bans, licenses, and sources without a waiver.

Because the lock repair changes guest code, the checked v0.2 artifact identity
also changed. A clean private-Docker build reproduced ELF
`ade4af8426040b7e5c171b559a382a15a3fa72e27531a93fe89742689a1bbcee`
and ImageID
`b7f8727893174a29bd776eacbfdd9773e0510ebdac43102cb7e93ba4fa0b0433`.
All five recursive native/XMR cases passed. The current deployer source,
sidecar, runners, contracts, source-boundary hashes, manual flow, and
architecture references now bind that identity; it has not been freshly
deployed, and historical evidence packets retain the superseded identity.
The secret-safe remediation receipt is
`docs/evidence/m5-ruint-remediation-20260731.json`.

The first exact sidecar-tree replay then found a second RED: the canonical hex
identity had changed while its separately encoded little-endian `[u32; 8]`
constant still represented the superseded guest. Twenty-five tests passed and
five tag-13 handoff tests failed closed with `BindingMismatch`. Updating all
eight words from the same checked ImageID made the seven focused handoff and
identity tests GREEN; the complete sidecar verifier then passed all targets and
features, warning-fatal Clippy, warning-fatal Rustdoc, and dependency policy
offline. Keeping both encodings is an upstream API compatibility constraint,
so the regression test remains the drift guard.

The immutable official external LEZ investigation source still resolves
`ruint 1.17.2`. `LOGOS-023` records that Logos-owned production-release blocker;
under ADR 0018 it does not block private local M5 certification. Next remains
the already-wired daemon-supervisor ZEC accepted-application proof. Literal M5
stays 3 of 7 and the closure ETA remains 10 to 20 focused implementation hours.

The same remediation changed the v0.1.2 compatibility artifact. Its superseded
`r0.1.88.0` builder failed closed because `ruint 1.20.0` requires Rust 1.90 or
newer; the already supply-chain-tracked digest-pinned `r0.1.94.1` builder uses
Rust 1.94.1. Exact run `m5-ruint-v012-final-20260731` reproduced ELF
`fe8ec1166ec886693d1fcd1d1ddc80090f81f6fab941851cce43b5bfb0c739f7`
and ImageID
`5421868ee00d213bf083c09f14ed09f303e8581b95b3a17bb9b79f6cb44add62`.
It passed six ordinary tests, two actual deployment/native-plus-two-token
lifecycle tests, and one recursive cost case.

The first top-level runner exit was `1` only at the final comparison: all build,
identity, deployment, actor-lifecycle, topology, accounting, and budget gates
had passed, but the old byte-identical snapshot policy compared volatile cycle
classification values. The replacement `scripts/check-lez-cost-evidence.sh`
policy is CI-required and passed against the exact generated output. It keeps
artifact identity, operation order, session topology, segment counts, total
cycles, and budgets immutable; it also independently requires every session's
user/paging/reserved classifications to sum to its total and every recursive
user total to stay within budget. Only the internally consistent volatile
classification split and measurement date may vary.

Approved deletion of the local `.e2e` run cache reduced the Risc0 Docker build
context from 6.37 GB to approximately 64 KB. This is measured iteration relief,
not a durable repository fix: the pinned Risc0-generated
`Dockerfile.dockerignore` overrides the root `.dockerignore`, and a future
retained `.e2e` tree can enlarge the context again. A durable optimization must
change that generated build-context boundary without weakening artifact
attestation or deleting another run's data.

## M5 fresh ZEC deployment bootstrap TDD checkpoint

The first fresh replay from pushed commit
`f0dc6297ce4dc1aa8590eb4fab1c7de105d7529f` started isolated local Zebra and
LEZ nodes with fresh Maker and Taker identities. The retained deployer binary
was stale: it submitted the historical `dc370…` / `4d659…` guest once, in
finalized block 83, before the exact deployment-evidence validator rejected
the mismatch. That LEZ chain was quarantined and its exact containers, network,
and image were removed. The still-unspent Zebra run was preserved because it
had no causal relationship to the invalid LEZ submission.

The resulting RED exposed a current-source inconsistency as well as the stale
binary. The generic buildable deployment manifest and public-interface
validator still described the 13-instruction F7 guest, while the embedded
current guest is the 18-instruction M5 artifact with ELF `ade4…`, ImageID
`b7f…`, and the native-XMR instructions. The current manifests, integrity
pins, validators, tests, deployment wrapper, and CI guards now describe that
exact embedded guest. Historical manifests and evidence retain their
historical identities.

Two mock happy paths also used a 100 ms response budget and could fail under
ordinary host scheduling load. Their budgets are now 2 seconds; deliberate
timeout and ambiguous-submission tests remain at 100 ms. The deployer suite
progressed RED 12/20, then 19/20, then GREEN 20/20 in 38.44 seconds. ADR 0124
records the component boundary, replay flow, chain quarantine, and conditional
atomicity argument.

A second isolated LEZ chain with a second fresh Maker/Taker identity pair is
running effect-free for the clean deployment replay. No fresh deployment of
the corrected artifact is claimed by this checkpoint. Literal M5 acceptance
remains 3 of 7. The corrected estimate at that checkpoint was 1 to 3 focused
hours for the ZEC daemon replay; the subsequent live result and hardening work
supersede it below.

## M5 daemon-driven ZEC live replay and deadline TDD checkpoint

Clean isolated LEZ run `m5zecb416lezc` deployed the current 18-instruction
guest and onboarded fresh, separate Maker and Taker identities. The first
application launch used the runner's historical default genesis variable name;
the live identity guard rejected it before provisioning or any chain effect.
The corrected replay invocation uses the exact `LEZ_GENESIS_HASH` input.

Exact application run `m5zecb416appf` then completed both role actors at
revision 4. Zebra advanced from deterministic mature height 104 to 107, the
Maker funding transaction was exact, the transport cutover passed, and Maker
effects were owned only by the daemon supervisor. The runner still rejected
the run because the scheduler persisted `failed/actor_output_invalid`. An
earlier checkpoint inferred that the rejected output was a valid terminal
projection, but the retained packet has no raw child stdout. A later source and
fresh-evidence audit corrected that inference: `lez_revealing_claim` is a valid
nonterminal projection into `claim_evidence_available` at revision 3. Run
`appf` remains diagnostic-only; its exact LEZ and Zebra resources were
quarantined.

Fresh chain `m5zecb626lez1` then deployed the same current guest, finalized two
fresh actor Vault Claims, and paired with a fresh Zebra Regtest at height 104.
Application run `m5zecb626app1` passed post-lock transport cutover and daemon-
only Maker authority. The Maker supervisor advanced the durable actor to
`claim_evidence_available` revision 3, then failed it after 24 attempts with
`actor_output_invalid`; the Taker reached the corresponding claim admission.
The run is retained as a second rejected RED packet; its disposable nodes,
networks, run-tagged images, and node directories were quarantined exactly.

The focused RED now reproduces that exact nonterminal projection. The GREEN
accepts `outcome: projected` only as `lez_revealing_claim` paired with the
Maker's `claim_evidence_available` phase and `wait` action, for both supervised
`drive` and explicit `claim`. Claim projections with missing, unrelated, or
crossed operation/phase/action fields remain rejected, as do projected
terminal claims. Cleanup also progressed RED then GREEN so the effect-bearing
daemon is always cancelled before either sidecar shutdown can consume time.

A second RED proved that a fixed 20-second attempt timeout cannot prevent an
effect attempt beginning near the 49-second corridor cutoff. The GREEN adds one
absolute Linux boot-time cutoff, inherited unchanged by both the full and
supervisor-only daemon incarnations. Effects are rejected before preparation,
after sealed-command construction, immediately before spawn, and during every
child wait. An in-flight process group is killed and reaped at the earlier
cutoff, its fenced child identity is cleared, and the durable actor backs off.
An already-expired supervisor does not claim or mutate a queued actor.

Focused GREEN evidence is 15 of 15 Maker supervisor integration tests, 5 of 5
parser tests, 3 of 3 daemon supervisor CLI tests, and the M5 application shell
contract. Cold optimized builds measured about 6 minutes for the root process
set and 14 minutes 38 seconds for the full LEZ sidecar graph; warm rebuild
checks complete in about 2 seconds and remain outside the protocol clock.

The next replay used pushed source
`7d402dcf7a7fd436621fd1c922babcb0d3ae8a1e`, a wholly fresh local LEZ
deployment, fresh Zebra Regtest, and a fresh Maker/Taker identity pair.
Application run `m5zec7d402app1` passed the revision-3 nonterminal projection
that rejected the preceding replay, and both role actors reached revision 4
`Completed`. The scheduler nevertheless failed closed with
`actor_output_invalid`: the parser rejected the exact terminal projection
`zcash_followup_claim` / `completed` / `complete`. This packet is diagnostic
only, is not milestone certification, and does not change the 3-of-7 score.
The narrow operation/phase/action mapping is being corrected, and the next
certification attempt must again use a wholly fresh chain and fresh keys.

Literal M5 remains 3 of 7 until a fresh-chain replay publishes a terminal
scheduler row, restart projection, duplicate-submission proof, and scoped
cleanup as one accepted evidence packet. That fresh proof is next. Only after
it passes does the score become 4 of 7; Maker CLI, Taker CLI, and coordinator
concurrency/restart isolation then remain. The corrected checkpoint estimate is
7 to 14 focused implementation hours to the M5 tag, subject to measured local
LEZ finality and the required new-chain provisioning cycles.

## M5 daemon-owned accepted ZEC application certification (2026-07-31)

Exact clean pushed commit
`432d1f7dabbb573b9642794155066e37ee95e75d` was replayed against a newly
deployed LEZ v0.2 chain, a new Zebra 5.2.0 Regtest node, and newly generated
Maker and Taker identities. The initial application preflight found Zebra at
height 0 before it created an evidence root, provisioned actors, or attempted a
swap effect. After the documented deterministic local maturity prefix reached
height 104, the same untouched application inputs were admitted; this is
fixture setup rather than a protocol-effect retry.

The completed run `m5zec432dapp1` reached both role actors at revision 4
`completed` in 25,030 milliseconds from provisioning. Zebra advanced exactly
104 to 107 through two funding-confirmation blocks and one follow-up-claim
block. The Maker scheduler resolved as `terminal` at lease generation 24 and
attempt count 24 with no child identity, while `same_run_drive_retries` stayed
zero.

The fresh packet proves Delivery, Chat, and the owner socket remained absent
after the first confirmed lock; the supervisor-only daemon retained the one
absolute effect cutoff and was the only Maker effect authority; the Taker CLI
remained bound to its immutable acceptance receipt. A new owner daemon then
projected revision-4 `completed` history and status from stopped actor state
without either chain RPC. No public RPC, faucet, peer, or public funds were
used.

After preserving the allowlisted manifest hashes, scoped cleanup removed only
the exact four run containers, two networks, two tagged images, private node
and identity roots, application root, and run-owned build target. All four RPC
ports and all run processes were absent afterward; no global prune or foreign
resource selector was used. The secret-safe committed packet is
[`m5-zec-daemon-supervisor-certification-20260731.json`](evidence/m5-zec-daemon-supervisor-certification-20260731.json).

This closes the daemon-owned effect-bearing accepted-application output and
accepts ADR 0125 for the local-functional scope. Literal M5 is now 4 of 7. The
three accepted outputs still open are:

1. complete Maker CLI lifecycle control across every supported pair;
2. complete Taker CLI lifecycle control across every supported pair; and
3. accepted-application actual-chain coordinator concurrency, restart
   isolation, and proof that an unavailable XMR route cannot stall BTC/ZEC.

No M5 tag is justified until those outputs and the final composite gates pass.
Updated ETA is 8 to 16 focused implementation hours to the M5 tag, subject to
measured fresh-node cycles and final review.

## M5 separate XMR effect-workflow authority checkpoint (2026-07-31)

The next Maker/Taker CLI slice began with a real restart-safety RED rather than
relaxing the existing schema-v2 monitor boundary. The new dedicated
SqliteXmrWorkflowJournal binds one exact swap, role, run, agreement,
activation, and future effect-authority digest. It uses exclusive 0600 creation,
an owner-private parent, a dedicated application ID and exact STRICT schema,
one irreversible claim/refund branch CAS, and fixed role-legal steps.

The first RED could not import the journal API. GREEN proves that the only
Prepared-to-Started transaction returns InvokeOnce, while both Started and
Unknown reopen as ObserveOnly and cannot be rearmed. A second RED constructed a
foreign SQLite database with copied application headers and table names; exact
canonical schema comparison now rejects it. Eight concurrent creators produce
exactly one new journal, and eight concurrent authorizers produce one
InvokeOnce plus seven ObserveOnly decisions. The full lez-swap-store
all-target/all-feature suite is GREEN at 156 tests; strict Clippy,
warning-fatal Rustdoc, rustfmt, and diff checks pass.

ADR 0126 records the authority split. The workflow journal coordinates local
branch and process recovery only. Immutable adaptor journals still authorize
Stage A/B, and the existing tag-15/tag-16 sidecar journals remain the actual
one-attempt LEZ send authorities. No chain endpoint, credential, tool, manifest
v3, receipt v2, CLI effect, or actual-node swap is added by this checkpoint.
The manual reproduction guide therefore remains unchanged until a real user
command exists.

Literal M5 remains 4 of 7. The next implementation order is:

1. publish and validate a canonical role-fixed XMR effect-authority v1 through
   a new application manifest v3, while legacy schema v2 remains monitor-only;
2. add acceptance receipt v2 and route Maker/Taker lifecycle commands through
   the separate workflow journal and exact external classifiers;
3. prove claim and refund through fresh isolated LEZ and official Monero local
   nodes, then update the manual flow and evidence packet; and
4. close the complete all-pair Maker CLI, Taker CLI, and accepted-application
   coordinator concurrency/restart outputs.

There is no external blocker to this local-functional work. Logos-owned public
readiness caveats remain tracked separately and do not block milestone
certification. Updated estimate is 7 to 14 focused implementation hours to the
M5 tag, dominated by v3/receipt composition and fresh two-devnet proofs.

### XMR Maker effect-authority loader checkpoint (2026-08-02)

The canonical effect-authority RED is GREEN for the Maker profile. Schema v1
binds the exact role, swap, agreement, activation, run, separate workflow and
adaptor journals, evidence root, LEZ sidecar runtime/capability, four
credential-file-backed literal-loopback Monero RPC classes, and five fixed
program/hash/ABI tool slots. Unknown fields and embedded secrets, legacy
schema-v2 bytes, noncanonical JSON, identity drift, overlapping journal roles,
unsafe paths, non-loopback or missing-port RPCs, and tool digest/ABI drift fail
closed. The implementation reuses the locked url 2.5.8 parser; the lockfile
edge was regenerated offline.

This does not yet publish application manifest v3, enable a CLI effect, or
close a literal M5 output. Taker-profile RED/GREEN is next, followed by v3
provisioning and receipt v2. Literal M5 remains 4 of 7. Updated estimate is 6
to 12 focused implementation hours to the M5 tag.

The symmetric Taker authority profile is now GREEN as well. It requires fixed
tag-14 authorization, finalized classification, Monero claim sweep, Monero
verification, and tag-16 refund slots; Maker and Taker profiles cannot cross or
coexist in one authority. Representative ABI and canonical-hash drift fail
closed. Focused Maker/Taker tests, strict Clippy, warning-fatal Rustdoc, rustfmt,
and diff checks pass. Manifest-v3 publication remains next, so literal M5 stays
4 of 7. Updated estimate is 5 to 10 focused implementation hours to the M5 tag.
### XMR schema-v3 effect-authority publication checkpoint (2026-08-02)

The manifest-v3 cycle is GREEN without reinterpreting or overwriting the
schema-v2 monitor authority. Schema v3 directly adds the run ID, immutable
effect-authority file and SHA-256, and separate workflow-journal path. Its
semantic loader reconstructs v2 only through the original canonical parser,
fully revalidates every pinned Stage A/B, role-material, packet, and adaptor
journal source, validates the exact effect bytes against the fixed role profile,
and then proves the existing workflow database contains the same
swap/role/run/agreement/activation/effect-digest identity.

The workflow journal gained a read-only validate_initialized boundary. It
fails on missing or crossed identity without initializing or mutating an empty
database. The first focused compile RED could not call that API; GREEN accepts
the exact durable row and rejects a crossed run. The schema-v3 publication RED
could not call the create-new publisher; GREEN atomically publishes one 0600
owner-private file and preserves the first bytes on collision.

The existing full role-separated Stage A/B process integration now provisions a
real Maker schema-v2 application, creates and initializes its independent
workflow journal, publishes canonical effect authority and schema v3, and loads
the complete semantic authority. Digest tamper, crossed run, legacy-v2
execution, and output collision fail closed. Focused tests, strict
all-target/all-feature Clippy, and warning-fatal Rustdoc pass. No RPC, node,
faucet, public network, or chain effect participates in this checkpoint.

Literal M5 remains 4 of 7 because this is a prerequisite rather than a complete
Maker/Taker lifecycle. The next implementation order is:

1. publish receipt v2 binding schema v3, effect digest, workflow identity, and
   run for the Taker lifecycle;
2. route the role-legal XMR Maker and Taker CLI actions through the workflow CAS
   and exact external classifiers;
3. repeat claim and refund through fresh isolated LEZ v0.2 and official Monero
   Regtest nodes using the real user commands; and
4. close the all-pair lifecycle and accepted-application concurrency outputs,
   run the composite gates, tag, and push.

Updated estimate is 3 to 7 focused implementation hours to the M5 tag, subject
to the measured fresh-node cycles and final composite review.

### XMR receipt-v2 and locked-monitor checkpoint (2026-08-02)

The current working tree adds the replay-safe Taker handoff without relaxing
the legacy boundary. Acceptance receipt v2 binds the exact schema-v3 manifest,
effect-authority bytes and digest, workflow journal, run, swap, role,
agreement, and activation. Its writer publishes only after schema-v3
provisioning succeeds; its selector rereads and digest-pins every authority
file, then performs full semantic validation while the per-swap and workflow
locks are held. Receipt v1 remains monitor-only.

The effect-capable argument group is all-or-nothing, and locked receipt-v2
monitor is implemented without RPC or chain effects. Claim and refund still
reject. This checkpoint remains under focused verification and does not close
a literal output or add an actual-node execution.

Remaining work is the complete typed tool/RPC execution plan, at-use hashes for
executables and capabilities, all role-legal workflow steps with exact
reconciliation, transfer of lock custody to any effect child, Maker effect
composition, receipt-v2 claim/refund execution, and fresh isolated LEZ v0.2
plus official Monero Regtest runner proof. The complete all-pair Maker CLI,
Taker CLI, and accepted-application concurrency/restart outputs then require
their composite proof and review.

Literal M5 remains 4 of 7. ETA remains 3 to 7 focused implementation hours to
the M5 tag, subject to fresh-node runtime and the final composite gates.

### XMR typed effect plan and sealed-executable checkpoint (2026-08-02)

The next narrow RED/GREEN checkpoint converts the already validated canonical
authority into role-specific Rust views without enabling either lifecycle
route. The typed LEZ view retains its literal-loopback sidecar URL, absolute
runtime-identity path and SHA-256, and absolute capability path. The typed
Monero view retains separate daemon, Maker funding-wallet, neutral
shared-wallet, and local-role-wallet loopback roots; each RPC has separate
absolute username and password file paths. These are endpoint and credential
path authorities only: no socket is opened and no credential is read by this
checkpoint.

The Maker view exposes exactly Monero fund
(`lez_xmr_monero_fund_v2`), LEZ tag-15 claim
(`lez_xmr_tag15_claim_v1`), finalized classifier
(`lez_xmr_finalized_classifier_v1`), Monero refund sweep
(`lez_xmr_monero_refund_sweep_v3`), and Monero verify
(`lez_xmr_monero_verify_v2`). The Taker view exposes exactly tag-14 authorize
(`lez_xmr_tag14_authorize_v1`), finalized classifier
(`lez_xmr_finalized_classifier_v1`), Monero claim sweep
(`lez_xmr_monero_claim_sweep_v2`), Monero verify
(`lez_xmr_monero_verify_v2`), and tag-16 refund
(`lez_xmr_tag16_refund_v1`). Every typed slot retains its normalized absolute
program path and decoded pinned SHA-256; a Maker authority has no Taker tool
view and vice versa.

The reusable `PinnedExecutable` boundary now makes program selection
race-resistant when a future route elects to use it. At use, it securely opens
without symlink traversal, validates the trusted parent and exact opened/named
single-link executable identity, reads at most 512 MiB, revalidates, and checks
the authority SHA-256. It copies those bytes into an immutable mode-0700 sealed
memfd and constructs a command against child FD 197. Replacing or unlinking the
named path afterward cannot change that command's bytes; a fresh verification
of the changed path fails closed. This closes executable snapshot TOCTOU only.
It neither authorizes replay nor calls a route.

The focused Taker authority suite is GREEN at 3 of 3, including exact typed
endpoint/credential/tool projection and replacement, symlink, and writable-mode
failures. The focused Maker plus Taker authority pair is GREEN at 4 of 4. The
full `lez-swap-store --all-targets` and
`xmr-reference-actor --all-targets --all-features` test suites, strict
all-target/all-feature Clippy for both packages, warning-fatal Rustdoc for both
packages, and diff hygiene are GREEN. No node, RPC, Docker service, faucet,
public network, peer, or funds participated.

The remaining XMR order is:

1. securely open and validate the pinned LEZ runtime and capability bytes and
   the RPC credential files at use, with a secret-safe child descriptor plan;
2. expand the fixed workflow to every role-legal LEZ and Monero external
   effect, add evidence-bound exact reconciliation for Started/Unknown, and
   enforce predecessor and branch semantics;
3. transfer both the actor/adaptor-state lock and workflow lock into each
   effect child for its complete lifetime, without colliding with program FD
   197;
4. compose the Maker route and receipt-v2 Taker claim/refund routes through
   that authority; and
5. prove role-correct claim and refund through fresh isolated LEZ v0.2 and
   official Monero Regtest nodes before the all-pair/concurrency closure gates.

No route executes a typed tool at this checkpoint, and claim/refund retain
their existing fail-closed behavior. Literal M5 remains 4 of 7. ETA remains 4
to 8 focused implementation hours to the M5 tag, subject to fresh-node runtime
and the final composite gates.

### XMR workflow-v2 and dual-lock checkpoint (2026-08-02)

The next RED replaces the two-step schema-v1 prototype with a closed schema-v2
external-effect catalog. Existing-open requires version 2 and rejects version 1
rather than migrating or reinterpreting it. The eight fixed entries are Taker
Initialize LEZ tag 13, Taker Fund LEZ tag 13, Maker Fund Monero, Taker
Authorize LEZ tag 14, Maker Claim LEZ tag 15, Taker Sweep Monero Claim, Taker
Refund LEZ tag 16, and Maker Sweep Monero Refund. The first three have Common
scope, the next three Claim scope, and the last two Refund scope. Every stored
row is re-parsed and checked against the fixed role and scope whenever storage
is opened or revalidated.

Preparation enforces role-local succeeded predecessors. Taker LEZ funding
follows initialization; tag 14 and tag 16 follow LEZ funding; Maker tag 15 and
the Maker refund sweep follow Monero funding; and the Taker claim sweep follows
tag 14. Every local Common row must be Prepared or later before the irreversible
Claim/Refund branch CAS. Branch-specific preparation then requires the selected
branch and its predecessor. These are role-local gates, not proof of global or
cross-role order; the route must bind finalized LEZ or confirmed Monero wallet
evidence before satisfying a counterparty-dependent transition.

The only `Prepared -> Started` CAS returns `InvokeOnce`.
`Started` and `Unknown` return `ObserveOnly` after restart or contention
and can never be rearmed. The old evidence-free `mark_succeeded` API now
always rejects. Only `reconcile_succeeded` can move Started or Unknown to
Succeeded, and it atomically binds nonzero canonical effect-evidence SHA-256,
nonzero exact tool-plan SHA-256, and either `lez_finalized_event` or
`monero_wallet_transaction`. Exact replay succeeds without mutation; any
evidence, plan, or source drift fails closed.

The sealed-command boundary now accepts two distinct already-held locks. It
revalidates both named/device/inode identities, rejects lock aliases and
descriptor collisions, and installs the sealed executable as FD 197, the
actor/adaptor-state lock as FD 198, and the workflow lock as FD 199 in one
descriptor mapping. The spawned child retains both kernel locks until it exits
and is reaped, so neither can be acquired through a competing process during
the effect lifetime. Changed, crossed-swap, aliased, or unsafe-root locks fail before spawn.

Focused evidence is GREEN:

- maker-process command and custody suite: 17 of 17;
- concurrent workflow authority: 2 of 2;
- workflow storage hardening: 1 of 1;
- restart/no-rearm regression: 1 of 1; and
- workflow-v2 catalog and reconciliation: 3 of 3.

The full `lez-swap-store --all-targets` suite, strict all-target/all-feature
Clippy, warning-fatal Rustdoc, rustfmt, and diff hygiene are GREEN.

This checkpoint performs no node or RPC operation and does not compose a chain
effect. No Maker or Taker lifecycle route invokes the workflow-v2 executor or
dual-lock command yet. Remaining work is:

1. securely bind the LEZ runtime/capability and RPC credential bytes at use;
2. derive canonical effect evidence and the exact tool-plan digest from the
   role-fixed classifier/wallet observation;
3. compose the Maker and receipt-v2 Taker routes through workflow v2, sealed
   execution, dual-lock child custody, cancellation, and reap;
4. run role-correct claim and refund through fresh isolated LEZ v0.2 and
   official Monero Regtest nodes; and
5. close the all-pair lifecycle and accepted-application concurrency outputs,
   composite gates, review, tag, and push.

Literal M5 remains 4 of 7. ETA remains 3 to 7 focused implementation hours to
the M5 tag, subject to fresh-node runtime and final composite review.

### XMR schema-v3 effect-input custody checkpoint (2026-08-02)

The next focused RED/GREEN closes secure at-use custody for the inputs named by
the schema-v3 effect authority without enabling a lifecycle route.
`pin_effect_inputs_at_use` uses `openat2` with no symlink traversal under
the exact mode-0700 euid-owned parent. Each runtime, capability, username, and
password source must be a mode-0600 euid-owned regular single-link file. Parent
identity plus source device, inode, length, owner, mode, link count, mtime, and
ctime are stable across a bounded read and named-file recheck; cross-source
inode aliases fail closed.

The LEZ runtime is limited to 16 KiB and must match the authority SHA-256. The
LEZ capability and all eight Monero RPC credential files are limited to 256
bytes each. They accept the actual runner's one ASCII-graphic value stored raw,
with one LF, or with one CRLF, preserving the exact original bytes. Empty,
embedded/multiple-newline, stray-CR, NUL, non-graphic, and oversized inputs
fail closed.

Each of the nine secrets is copied into a separate mode-0400 memfd with write,
grow, shrink, and seal seals, then duplicated close-on-exec to a unique
collision-free descriptor at or above 200. The non-Clone custody types expose
only the descriptor path, redacted byte length, and SHA-256; Debug output
redacts values. Named path replacement cannot alter an existing snapshot, while
a fresh pin rejects digest or storage drift. Runtime bytes remain a bounded
hash-checked in-memory snapshot rather than a secret memfd.

The focused Taker effect-authority suite is GREEN at 5 of 5. It covers exact
runtime and nine-secret snapshots, raw/LF/CRLF inputs, descriptor uniqueness,
redaction, replacement isolation, fresh drift, invalid content, size, mode,
parent, symlink, hard-link, and cross-source alias rejection. Strict all-target
Clippy, warning-fatal Rustdoc, rustfmt, and diff hygiene are GREEN. No RPC,
node, Docker service, faucet, peer, public network, or funds participated.

Remaining work is:

1. map the nine secret snapshots to their exact child descriptor numbers
   together with program FD 197 and lock FDs 198/199;
2. compose Maker and receipt-v2 Taker lifecycle routes through the sealed
   program, pinned inputs, workflow-v2 CAS, dual locks, cancellation, and reap;
3. derive evidence-bound reconciliation from finalized LEZ events and confirmed
   Monero wallet history;
4. prove both role-correct branches with fresh isolated LEZ v0.2 and official
   Monero Regtest nodes; and
5. close all-pair lifecycle, concurrency, composite, review, tag, and push
   gates.

No route or child mapping consumes the snapshots yet, and this checkpoint
opens no RPC or node and executes no chain effect. Literal M5 remains 4 of 7.
ETA remains 3 to 7 focused implementation hours to the M5 tag, subject to
fresh-node runtime and final composite review.

### XMR atomic child-exec descriptor checkpoint (2026-08-02)

The next process RED/GREEN consumes the sealed runtime and nine secret snapshots
rather than leaving them as parent-only custody objects. The generic non-Clone
`PinnedChildFdPlan` accepts 1 through 64 owned source descriptors and requires
unique non-aliased sources plus unique child targets in 200 through 1023. Empty,
reserved/out-of-range, duplicate-target, and aliased-source plans fail closed;
redacted Debug exposes only the count.

The XMR specialization fixes the complete child ABI:

| FD | Input |
|---|---|
| 197 | sealed executable |
| 198 | actor/adaptor-state lock |
| 199 | workflow lock |
| 200 | hash-pinned LEZ runtime |
| 201 | LEZ capability |
| 202/203 | Monero daemon username/password |
| 204/205 | funding-wallet username/password |
| 206/207 | shared-wallet username/password |
| 208/209 | role-wallet username/password |

`PinnedXmrEffectInputsV1::into_command` consumes runtime plus all nine secret
snapshots, the pinned executable, and the two exact held locks. It installs all
13 descriptors with one `fd_mappings` call, preventing later mapping calls
from replacing earlier custody. No runtime, capability, username, or password
bytes enter argv or env.

The process proof pins every input and executable, replaces all named sources,
then execs the original sealed program. The child reports the exact original
runtime and nine secret hashes, sees FDs 197 through 209 and no FD 210, and
remains alive after the parent Command and both parent lock handles are dropped.
Competing lock acquisition fails until child exit/reap and succeeds afterward.
The generic negative test covers empty/reserved/duplicate/aliased plans and
redacted Debug.

Full `lez-swap-store` and `xmr-reference-actor` all-target/all-feature
regressions, strict Clippy, warning-fatal Rustdoc, rustfmt, and diff hygiene are
GREEN. No RPC, node, Docker service, faucet, peer, public network, or funds
participated.

Remaining work is route composition: select the role-fixed tool, pin exact
inputs, acquire both locks, enter workflow-v2 InvokeOnce/ObserveOnly, spawn and
reap the one-map command, classify finalized LEZ or confirmed Monero evidence,
and reconcile success. Maker effects, receipt-v2 Taker claim/refund, fresh
two-devnet claim/refund proofs, and all-pair/concurrency/composite/tag gates
remain.

No lifecycle route calls this boundary, and this checkpoint opens no RPC or
node and executes no chain effect. Literal M5 remains 4 of 7. ETA is now 2.5
to 5.5 focused implementation hours to the M5 tag, subject to fresh-node
runtime and final composite review.

### XMR shared-wallet file-password and complete input-map checkpoint (2026-08-02)

The current focused checkpoint closes the last known effect-input schema and
descriptor gap. Canonical effect authority now requires one normalized absolute
`shared_wallet_file_password_file`, distinct from all eight Monero RPC
credential paths. At use it receives the same bounded content, exact
mode-0600/single-link/owner, stable-read, no-symlink, and cross-source alias
checks as the capability and RPC credentials.

The password is the tenth sealed secret and maps to fixed child FD 210. The
complete single-call child map is now program FD 197, actor-state lock FD 198,
workflow lock FD 199, runtime FD 200, capability FD 201, four RPC
username/password pairs on FDs 202 through 209, and shared-wallet file password
FD 210. The process proof consumes all 14 descriptors, verifies exact
pre-replacement runtime plus ten secret snapshots, and requires FD 211 absent.
No secret enters argv or env.

Maker execution authority now retains the semantically validated canonical
published Stage-A and Stage-B paths plus each exact wire SHA-256. The future
route can therefore bind its effect preparation to the already validated public
agreement and activation identities without re-deriving them from unbound
paths.

Focused tests require the file-password field, reject missing, relative,
unsafe, overlapping, and cross-source-aliased paths/files, expose its pinned
redacted snapshot, and include it in the replacement-proof exec hash matrix.
Maker tests retain the validated Stage-A/B paths and digests. This closes the
current authority/input validation, sealed custody, and child descriptor-map
gaps.

Route composition remains: choose the role-fixed tool and branch, acquire both
locks, authorize through workflow v2, execute/reap the prepared command, bind
finalized LEZ or confirmed Monero evidence, and reconcile the exact result.
Maker/Taker lifecycle commands, fresh two-devnet claim/refund, and final
all-pair/concurrency/composite/tag gates remain open.

No lifecycle route, RPC, node, or effect is added by this checkpoint. Literal
M5 remains 4 of 7. ETA remains 2.5 to 5.5 focused implementation hours to the
M5 tag, subject to fresh-node runtime and final composite review.

### XMR role-fixed invocation preparation checkpoint (2026-08-02)

The schema-v3 execution loader now retains the immutable effect-authority
SHA-256 and exact initialized workflow identity with the fully validated
authority. This removes path/digest reconstruction from the future route.

`prepare_effect_invocation` has a closed six-slot sending allowlist:

- Maker: Monero fund, LEZ tag-15 claim, and Monero refund sweep;
- Taker: LEZ tag-14 authorize, Monero claim sweep, and LEZ tag-16 refund.

Classifier, verifier, wrong-role, and other catalog steps cannot consume
invocation authority through this boundary. The method selects the exact tool,
computes a domain-separated plan digest, hash-pins the program, pins the runtime
and ten secrets, validates the actor/adaptor and workflow locks against the
loaded swap and exact state paths, and composes the complete FD 197..210 command
before opening workflow v2 and calling `authorize_once`.

This ordering protects Prepared: any corrupt program/input, wrong role, crossed
lock, or command-composition failure occurs before the only CAS. A Prepared
winner returns `InvokeOnce` with the owned Command and plan digest. Started or
Unknown returns `ObserveOnly`; Succeeded returns `Complete`. Those latter
results drop the already validated local command and expose only the same
digest, so neither can send.

The plan SHA-256 is domain-separated by
`lez-xmr-effect-tool-plan-v1\0` and binds role, stable step name, fixed ABI,
pinned program SHA-256, and exact effect-authority SHA-256. It is intentionally
stable across restart and is suitable for workflow-v2 reconciliation identity;
rotating credential contents are not misrepresented as part of that immutable
tool-plan digest.

The real schema-v3 Taker Tag14 process fixture proves:

- corrupt named program bytes fail without moving Prepared;
- the Maker tag-15 step fails under Taker authority;
- the valid process receives exact FDs 197 through 210;
- exactly one preparation returns InvokeOnce with a Command; and
- reload returns ObserveOnly with no Command and the identical nonzero digest.

This is process and authority evidence only. The fixture worker does not contact
the LEZ sidecar and does not construct, sign, submit, or semantically classify a
real tag-14 transaction.

Remaining work is lifecycle composition around this boundary: spawn/reap the
InvokeOnce command, interpret its bounded typed result, classify finalized LEZ
or confirmed Monero evidence for ObserveOnly, reconcile success, and expose the
Maker/Taker commands. Fresh two-devnet claim/refund plus final all-pair,
concurrency, composite, review, and tag gates remain.

No lifecycle route, RPC, node, or semantic chain effect is added. Literal M5
remains 4 of 7. The current 2 to 5 focused-hour ETA remains subject to
fresh-node runtime and final composite review.

### XMR receipt-v2 Taker Tag14 process-invocation checkpoint (2026-08-02)

The real `lez-taker claim --receipt` path now consumes the schema-v3 boundary
under separate actor/adaptor-state and workflow locks. It selects only Taker
`AuthorizeLezTag14`, pins the program, runtime, and ten secrets, composes FDs
197 through 210, and then enters workflow-v2 authorization.

The first valid call wins the durable Prepared-to-Started CAS, spawns exactly
one hash-pinned child, waits at most 30 seconds, and reaps it. Successful marker
output is schema 3 `invoked_unreconciled`, with the stable nonzero plan digest
and `chain_effect_finalized:false`; the workflow remains Started.

The second claim starts no sender. It pins the role-fixed finalized classifier,
pins the same runtime, secrets, and locks, derives the original sending-plan
identity rather than an observer-plan identity, and exact-compares it before
execution. Only Started or Unknown is observation-eligible; Prepared and
Succeeded reject this boundary. The strict parser bounds output, requires the
parent-selected exact step, accepts only pending without evidence or finalized
with a nonzero canonical evidence digest, and has no source field. Role and
step locally select `lez_finalized_event` or
`monero_wallet_transaction`.

The fixture classifier returns finalized Tag14 marker evidence. The CLI
atomically reconciles the exact evidence digest, original sending-plan digest,
and locally derived source to Succeeded, then emits `complete` with
`chain_effect_finalized:true`. A third claim returns the same Complete state
without sender or observer. Observer spawn, wait, timeout, exit, bounded-read,
parse, plan, evidence, or reconciliation failure makes no journal mutation.
Sending ambiguity remains sticky Unknown and never rearms. Once Claim wins, the
losing Refund branch fails closed before tool invocation.

The exact real Maker-daemon/Delivery/Chat black-box case is GREEN 1 of 1 in
133.16 seconds. It withdraws Delivery and stops Chat with the daemon before the
lifecycle actions, then proves first invocation, second-call observation and
reconciliation, third-call Complete replay, losing refund, digest stability,
marker stability, and unchanged accepted artifacts. The focused effect-route
suite is GREEN 5 of 5; strict Clippy and warning-fatal Rustdoc are GREEN.

This is a process-component checkpoint. Its sender and classifier are fixed
local markers, not semantic Tag14 or finalized-chain workers; no transaction is
constructed or submitted, no RPC or node is opened, and the stored evidence is
not on-chain proof. Remaining M5 work is semantic workers and real finalized
evidence, the other role-fixed Maker/Taker actions, and literal outputs 5/7
Maker lifecycle, 6/7 Taker lifecycle, and 7/7 accepted-application
concurrency/restart/unavailable-XMR isolation, followed by final review/tag
gates. Literal M5 remains 4 of 7.


## M5 progressive-PoC closure candidate checkpoint (2026-08-02)

This checkpoint reconciles the literal current RFP and accepted issue #112,
rather than treating three QA composites as additional deliverables. The seven
outputs are: daemon, Maker CLI, Taker CLI, coordinator
persistence/crash/concurrency, price sources, Delivery/Chat degradation, and
fuzzing. All seven now have reproducible local-functional PoC evidence, so the
milestone state moves from 4/7 to **verified local-functional PoC 7/7**.

New closure evidence:

1. `maker_actor_lifecycle_control_plane_is_pair_safe_replay_safe_and_restart_durable`
   is GREEN 1/1 in 0.64 seconds through the real Maker CLI and daemon. It covers
   claim/refund admission, generation fencing, replay, restart, pair identity,
   and durable manual-action rows for BTC, XMR, and ZEC.
2. The matrix first exposed a Bitcoin claim RED through JSON-RPC `-32602`.
   The production supervisor now maps user-level Bitcoin Claim to its semantic
   Drive command; ZEC/XMR Claim remain Claim, and all Refund intents map to
   Recover. `manual_actions_map_to_pair_semantic_commands` is GREEN 1/1.
3. `receipt_v2_refund_invokes_observes_and_completes_exact_tag16_once` is
   GREEN 1/1 in 106.26 seconds after the M7 preflight integration. It proves
   rejected-preflight retry, prepare-only preflight once, Tag16 sender once,
   restart-only
   observer, exact-plan/evidence reconciliation, third-call Complete, and
   losing-claim exclusion through the real Taker CLI after Delivery/Chat and
   the Maker daemon are removed.
4. `daemon_runs_overlapping_actors_and_isolates_failing_peer_across_restart`
   is GREEN 1/1 in 16.31 seconds. One daemon, database, and three-worker pool
   retain pair-correct BTC/XMR/ZEC coordinators and disjoint manifests/state.
   XMR remains live while BTC and ZEC become Terminal, then fails alone to
   Backoff; health stays responsive, the child is reaped, restart preserves
   exact rows, and no actor replays.

The first and fourth cases use marker actor programs. Together with the Tag16
fixture classifier, they are control-plane/process evidence, not fresh on-chain
evidence. The PoC chain-effect layer remains the retained M2 ZEC, M3 BTC, M4 XMR
local-devnet certifications plus the clean M5 BTC/ZEC/XMR accepted-application
corridors. The evidence layers compose but are not interchangeable.

The exact repository format/test/Clippy/Rustdoc/security/traceability gates,
single final 362-diagram Mermaid render, and candidate diff/evidence review are
GREEN. Tag `m5-poc-complete` binds this verified closure. Semantic receipt-v2 XMR transaction/observer adapters and a fresh
simultaneous accepted-application actual-chain composite move to
post-PoC QA/chaos/infosec/production hardening under the progressive-JPEG
policy. Public deployment remains deferred. No production-ready claim is made.

## M6 active work package: Maker and Taker Basecamp mini-apps

Entered: 2026-08-03 after verified tag `m5-poc-complete`. Authority is the
current RFP-003 plus accepted replacement proposal issue #112. The literal M6
outputs are:

1. signed-off clickable HTML prototypes for both role journeys;
2. a Maker mini-app for pair and price configuration, active monitoring, and
   history;
3. a Taker mini-app for offer browsing, initiation, progress, terminal action,
   and ZEC shield-after-swap guidance; and
4. a Basecamp-loadable repository with assets and reproducible local-build
   instructions.

ADR 0128 governs the implementation. The accepted proposal TypeScript UI
assumption has been superseded by the current official Basecamp 0.2.0
`ui_qml` package shape. The implementation will pin the official tutorial
contract verified at commit `bfc34c451c08da9f78072dd825756a1e071a051d` and
will keep role authority outside the view layer.

Progressive order:

- build deterministic local HTML prototypes with no external runtime calls;
- rehearse both user journeys and record owner sign-off;
- build two current-QML packages with pinned, reproducible definitions;
- bind Maker UI actions to the owner Unix RPC and Taker actions to a typed,
  role-fixed lifecycle facade;
- add actor-real UI E2E, builds, CI gates, manual-flow documentation, assets,
  external-resource/flakiness notes, and final milestone evidence.

The first M6 slice is the visibly clickable happy path. Red-green-refactor,
chaos, infosec, and production hardening follow the reproducible PoC rather
than delaying it.

## M6 clickable-prototype checkpoint (2026-08-03)

Pushed commit `0abdbc2` implements deterministic, dependency-free Maker and
Taker HTML prototypes with original SVG assets and a built-in Node server that
selects an ephemeral literal-loopback port. Maker covers configuration,
monitoring, history, and sample manual intents. Taker covers browsing, exact
review, initiation, receipt-shaped progress, mutually exclusive claim/refund,
and ZEC shield-after-swap guidance. Every screen continuously labels its state
as simulated and opens no daemon, Delivery, Chat, chain, wallet, faucet, DNS,
or public-network boundary.

Post-PoC red-green-refactor added a recursive static resource/effect/CSP
contract and a six-case Puppeteer actor E2E. The static contract, Node syntax,
zero-moderate npm advisory audit, license allowlist, CI hardening contract, and
action pin policy are GREEN. A Docker-isolated RED run exposed unreliable
number-input replacement and a smooth-scroll click race. Commit `53e6cd8`
made those actor journeys deterministic without disabling the Chromium
sandbox. The exact digest-pinned runner at `e48ad9c` is GREEN 6/6 in 16.13
seconds with no network namespace, a read-only repository mount, run-unique
profile and container names, bounded resources, and exact cleanup. Runtime
external resources are empty. An absent image may require a one-time GHCR
pull before the networkless run. Remote Actions status remains unobservable
without credentials, so no remote-green claim is made.

The review record is `docs/m6-prototype-review.md`. Explicit owner sign-off is
the gate before production Basecamp QML work under ADR 0128. This checkpoint
implements the first of four literal M6 outputs but does not claim Basecamp
loadability, backend authority, chain effects, or M6 completion.

Fresh unchanged-input replay on 2026-08-04 revalidated the contract and all six
actor-browser journeys against repository commit `1afc0db`. The sandboxed,
networkless digest-pinned runner completed in 19.34 seconds and was removed
afterward. A direct host attempt executed no journey because Chromium correctly
refused to disable its unavailable host sandbox; it was not counted as product
evidence. The exact packet is
`docs/evidence/m6-prototype-revalidation-20260804.json`. The repository owner
explicitly approved the reviewed prototype on 2026-08-04, releasing the
issue-#112 production-QML gate without treating automation as human approval.

## M6 atomic Maker route backend checkpoint (2026-08-03)

Pushed commit `8c6a7db` removes the non-atomic three-request sequence behind
the prototype's one-click route save. Strict owner RPC
`maker_local_route_save_v1` accepts one request ID, both expected revisions,
one validated local-source policy, and the exact same-route reduced integer
price. Schema v22 commits the policy row, price row, and combined replay result
inside one immediate transaction. Exact replay returns both original revisions;
changed payload reuse conflicts.

Red first established the missing method. GREEN proves fresh enabled-route
creation, restart replay, global request conflict, and rollback of the earlier
pair write when the later price CAS is stale. The owner-RPC integration also
rejects an unknown path-shaped field before dispatch. The complete
`lez-swap-store --all-targets` regression, focused Maker RPC test, strict
two-package Clippy, warning-fatal Rustdoc, formatting, and diff hygiene are
GREEN.

ADR 0129 records the atomicity argument and both success/failure flows. At this
2026-08-03 checkpoint it was an implemented nonvisual prerequisite and did not
cross the then-pending prototype sign-off gate. Approval and the later QML/QtRO
composition are recorded in the M6 checkpoints below.

## M6 typed Taker facade contract checkpoint (2026-08-03)

Pushed commit `3547130` exposes the unchanged hardened secret-file readers as
a reusable library boundary instead of compiling duplicate binary-private
modules. Pushed commit `6161e35` defines the strict, versioned, secret-free
Taker facade DTO contract and exact seven-method allowlist. The caller supplies
reviewed public commitments and opaque IDs, never receipt paths, socket paths,
keys, raw evidence, generic commands, or node endpoints. Claim and refund are
different request types and carry an observed generation.

Six focused contract tests, strict Clippy, formatting, and diff hygiene are
GREEN. Capability reporting preserves pair truth: BTC and ZEC expose their
current receipt-bound lifecycle, while XMR remains effect-checkpoint-only and
cannot be presented as terminal completion from one marker. ADR 0130 records
the component and sequence flows and why the boundary preserves conditional
atomicity without claiming that DTOs themselves perform a swap.

## M6 owner-only read backend checkpoint (2026-08-03)

Pushed commit `270c5ef` extracts the proven HTTP-only jsonrpsee limits,
owner-owned mode-0700 runtime validation, no-replacement mode-0600 Unix bind,
and inode-safe cleanup for reuse by a separate Taker service. Existing Maker
process lifecycle and owner journey regressions remain GREEN.

Pushed commit `1584b76` implements real key-pinned Delivery health and offer
listing with injected trusted time, current-route validation, bounded sources
and results, deterministic identity/offer ordering, exact duplicate collapse,
conflicting immutable duplicate rejection, fixed path-free errors, optional
payload-free Chat health, and redacted diagnostics. Focused tests are GREEN
6/6 with strict Clippy. No mutation method or generic dispatcher is exposed.

ADR 0131 requires a separate owner-only Taker process rather than adding this
authority to the Maker daemon.

## M6 read-only Taker process checkpoint (2026-08-03)

Pushed commits `3121a5d`, `b8b375c`, and `8826836` register only
`taker_health` and `taker_offer_list_v1`, load their dependencies from a
strict owner-private schema-v1 file, and run them through the dedicated
`lez-taker-service` owner Unix socket. The configuration accepts only pinned
Delivery directories and Maker keys, an optional metadata-only Chat socket
probe, and a bounded offer maximum. It has no registry, prepared material,
receipt, actor, wallet, or node field. Commit `0ef38b0` returns configuration
bytes with their same-descriptor device, inode, and length identity, checks
length around the read, reopens the path to reject replacement, zeroizes bytes,
and accepts only an owner-owned single-link regular exact mode 0400 or 0600
file.

Pushed commit `ad088f8` makes that narrow deployment observable rather than
requiring a caller to infer it from pair-level capabilities. Every health
response reports health and offer-list as registered and reports swap-list,
initiate, monitor, claim, and refund as unregistered. Process tests prove the
reported booleans match the actual method-not-found boundary.

The process test proves empty health and offer listing, mode-0700 runtime and
mode-0600 socket custody, Maker and all five unimplemented Taker methods as
method-not-found, SIGTERM cleanup, restart, preservation of a replacement
inode, and failure before bind for missing or invalid configuration and a
relative socket. This is a read-only process boundary and creates no swap.

## M6 standalone Taker initiation registry checkpoint (2026-08-03)

Pushed commits `ca10c13` and `5c6500d` add a separate owner-private
SQLite schema-v1 registry. For the current ZEC `TakerSellsLez` vertical, one
immediate transaction commits exact public initiation facts, service-derived
private authority, and the global request/result replay row. Exact replay
survives restart. Durable `lookup_initiation` revalidates the request, public
projection, and private authority, then returns only public facts without a
live Delivery or trusted-time check. A future service can therefore check
durable replay before an offer expires or disappears. Changed public or private
payloads conflict; a same-swap loser rolls back without consuming its request
ID; schema, row, symlink, curve-point, and file-identity drift fail closed. Ten
focused tests plus the new lookup assertions, strict Clippy, Rustdoc,
formatting, diff hygiene, and the swap-store library regression were GREEN
before push. Pushed commit `9820400` adds two independent SQLite-connection
concurrency proofs: identical contenders converge to one new admission and one
exact replay, while different requests for the same swap produce one winner
and one `SwapConflict` without consuming the losing request ID. The full
registry is GREEN 12/12; the two concurrent cases also passed 40 repeated
invocations, for 80 concurrent-test executions.

ADR 0132 records the components, success, conflict, restart, atomicity, and
limitations. At this standalone checkpoint the registry was not yet connected
to `lez-taker-service`, and no worker, actor, Chat, chain, wallet, claim, or
refund effect existed.

Pushed commit `28006dc` makes the strict optional prepared-ZEC context
component-GREEN. It opens only an existing registry; caps the static catalog at
256; requires unique named sources and fixed swap, offer, reservation, and
output identities; authenticates the retained same-descriptor signed envelope;
cross-binds Maker, ZEC `TakerSellsLez`, offer, exact amount/quote, and SHA-256;
validates immutable-file digests and a real 32-byte secp256k1 signing key; and
keeps paths and private authority out of Debug and fixed errors. Dynamic client
request IDs and caller-supplied route or Maker identity are rejected from the
catalog. The legacy backend loader rejects this optional context rather than
silently discarding it. Focused context 4/4, read/config/backend/RPC/process
18/18, same-FD race 2/2, strict Clippy, Rustdoc, formatting, and diff hygiene
are GREEN.

## M6 replay-first service admission checkpoint (2026-08-03)

Pushed commit `1664c41` switches the executable to the complete validated
service context. Empty configurations remain backward compatible with exactly
health and offer-list. A configuration with prepared authority additionally
registers `taker_swap_initiate_v1`, and health truthfully reports that third
method while swap-list, monitor, claim, and refund remain absent.

The handler validates schema, then performs durable request lookup inside
`spawn_blocking` before it selects prepared authority, samples time, or reads
Delivery. Exact replay returns the original public projection even after
process restart and Delivery removal. Changed reuse is a fixed conflict. A new
request must exactly match the prepared offer, route, Maker, signed-envelope
SHA-256, foreign units, and LEZ units. The service then captures one trusted
timestamp, authenticates Delivery at that timestamp, and rechecks the exact
selection before an immediate SQLite transaction admits public facts, private
authority, and replay. It returns `Initiating` generation zero only after the
commit. The mutex-protected registry/catalog is never held across an async
Delivery call.

Focused admission, read RPC, configuration, and actual-process tests are GREEN
16/16. The process proof starts the real owner-only Unix service, admits through
JSON-RPC, restarts it, removes the live offer file, and receives the exact
durable replay. All-target strict Clippy, warning-fatal Rustdoc, formatting,
and diff hygiene are GREEN. ADR 0134 records the components, sequence, fixed
errors, and atomicity argument. Commit `e7a7e2b` adds service-level concurrent exact-replay
and one-winner conflict evidence.

Commit `0afb6da` then moved the already process-proven real ZEC acceptance,
countersigning, no-clobber agreement and receipt publication, and Taker actor
provisioning into one reusable library module. Commit `b7280ac` retained the
immutable original admission timestamp for expiry-safe replay.

## M6 prepared ZEC service acceptance checkpoint (2026-08-03)

Pushed commit `5536dd0` connects that path to
`taker_swap_initiate_v1` behind explicit
`initiation.execute_prepared_zec: true`. The default remains admission mode.
At this commit any nonempty prepared catalog requires the owner-local Chat
socket, whether or not execution is enabled.

A new request preserves the admission-first ordering from ADR 0134. After the
registry transaction is durable, the service revalidates prepared draft,
signing-key, and source-actor material, performs real bounded Maker Chat
proposal and completion, persists the countersigned agreement without
clobbering, provisions the role-fixed Taker actor, and publishes the completion
receipt. It returns `NotActivated` generation zero only after Maker completion
and the receipt are durable.

Replay reads the stored facts and original admission time before live offer
checks. It then selects the current prepared entry and reuses exact registry
admission to compare every public fact and private authority field with the
durable row. A same-byte signing-key replacement at the same path but a new
inode therefore conflicts. If a valid completion receipt exists, exact replay
succeeds after both the Delivery offer and Chat socket are unavailable and
does not rewrite agreement, actor-config, or receipt bytes or inodes. The
digest-pinned `ActorConfig` object is passed directly into config-based
provisioning instead of being reopened by path.

The real service-connected proof crosses an in-process Taker RPC module, a real
Maker daemon, signed local Delivery, the separate Chat socket, both SQLite
stores, agreement validation and countersigning, Maker atomic completion,
Taker actor provisioning, and receipt publication. Maker negotiation is
`Completed`, exactly one Maker actor is queued, and the Taker bundle is
role-correct. Neither actor starts: Maker and Taker state databases and the LEZ
bridge journal remain absent, so no Zebra or LEZ RPC or wallet effect occurs.
The separate `taker_service_process` suite continues to prove the executable
owner socket and restart boundary.

The affected suites are GREEN 14/14: direct admission/replay 4, configuration
5, service process 3, and real/legacy Chat 2. Strict all-target Clippy,
warning-fatal Rustdoc, formatting, and diff hygiene are GREEN. ADR 0135 records
the component topology, fresh and restart sequences, failure-window recovery,
and the exact pre-effect atomicity argument.

This is synchronous, client-retried acceptance rather than an autonomous
worker. A failure after registry admission returns dependency-unavailable and
requires the exact request to resume, but no chain effect has started. Draft
and signing-key paths are still reread after service preflight; retained-byte
handoff and exact use-time inode enforcement remain production hardening.
Admission-only catalogs retaining execution material and requiring Chat are a
least-authority compatibility item.

The receipt-bound swap-list and monitor projection is now implemented by
`e9393cf`; the next nonvisual work is actor driving and generation-fenced
claim/refund methods. The owner released the production QML and QtRO signoff
gate on 2026-08-04. Actor-real UI composition, Basecamp packages, final
quality/evidence gates, and the M6 tag remain pending.

## M6 receipt-bound list and monitor checkpoint (2026-08-03)

Pushed commit `e9393cf` registers `taker_swap_list_v1` and
`taker_swap_monitor_v1` only when the service has a validated prepared-ZEC
catalog and registry. Health then reports exactly five methods: health, offer
list, swap list, initiate, and monitor. Claim and refund remain unregistered.

The caller supplies only a schema version and, for monitor, a swap ID. The
service resolves the immutable prepared entry, requires its exact durable
private authority, and uses only the prepared receipt and actor root. A
missing receipt projects `Initiating`. A receipt-bound actor is loaded and
cross-checked, its per-swap kernel lock is acquired, and receipt/config
authority is reread before the status-only actor command. Typed actor status
is normalized into the secret-free Taker state, generation, available action,
and privacy-guidance DTO. Unknown swap IDs and offer-ID substitution return
the same fixed `swap_not_found` response. Results are capped at 256 and remain
in stable registry swap-ID order.

The real service acceptance proof now removes Delivery and Chat after
completion, reloads the service, and proves health, one-item list, exact
monitor, unknown/substituted-ID errors, response redaction, and no mutation of
agreement, actor-config, or receipt bytes and inodes. The accepted swap remains
`NotActivated` generation zero with no available action. Status uses unit
ports, starts no actor, writes no role state or journal, and contacts no node,
wallet, faucet, public RPC, Delivery, or Chat endpoint.

ADR 0136 records the components, fresh monitor and restart/offline sequences,
and the read-atomicity argument. The shared lock excludes concurrent actor
progress for one projected swap, but one list is a sequence of per-swap
snapshots rather than a global snapshot. Commit `3307dca` extends that
boundary with a process-incarnation receipt digest/device/inode fence. Same-byte receipt replacement and live actor-lock
contention now return one fixed redacted dependency error; restoring the exact
receipt and releasing the lock restores the same `NotActivated` view without
creating role state, a bridge journal, or chain effects. Durable rollback or
state-incarnation fencing across a service restart remains production
hardening. Pushed `9cf1a34` additionally proves that bound-receipt deletion,
coherent receipt/config cross-tampering, and corrupt role-state storage make
both monitor and the whole list fail closed, while a receipt that never existed
continues to project honest `Initiating` state across process restart. Pushed `c90b21d` seeds the accepted Taker agreement into the real role-state
SQLite store with unit ports and proves `Offered` revision zero projects as
`AwaitingFirstLock`; future payload and malformed agreement rows make monitor
and the whole list fail closed, then exact restoration recovers. Next nonvisual work is actor driving
and generation-fenced claim/refund.

## M6 service-driven ZEC Claim PoC checkpoint (2026-08-03)

Pushed commits `4cadbb0`, `3b7d927`, `6eb9523`, and `0c32200` connect the
prepared `TakerSellsLez` actor to the owner service terminal-action boundary.
The service admits one generation-fenced Claim before invoking the actor,
refreshes custody after status replay under the same per-swap lock, and returns
the same durable authorization to an identical retry. Claim and Refund remain
mutually exclusive in the Taker registry.

Certification run `m6cert20260803164006` is GREEN. At generation three, the
first `taker_swap_claim_v1` response returned `was_replay: false`; Zebra
Regtest moved from an empty mempool to exactly transaction
`6b65cdff60f821717ba1e4cc862cec197ef16b0f7bccff4eb8c7e3d93ed11b70`.
The immediate exact request returned `was_replay: true`, and Zebra retained
that same one-element mempool. The runner mined the transaction and completed
both roles after confirmed ZEC funding and the LEZ revealing claim. Elapsed
provision-to-completion time was 35.100 seconds. Commit `e5b4c32` corrects
future `result.json` summaries to report `owner_taker_service` and explicit M6
mode. The retained certificate predates that reporting-only fix; its dedicated
Claim/replay responses and mempool snapshots are the authoritative service
evidence.

This proof reused the already isolated actual local LEZ v0.2 run
`m6lez20260803155817` and paired it with fresh Zebra Regtest run
`m6zec20260803164006`. Both exposed dynamic literal-loopback RPCs and used
deterministic genesis/Regtest funds and run-private files. No public RPC,
faucet, public funds, or public deployment participated. A separate fresh LEZ
stack was deployed and onboarded successfully afterward but was not used by
this certificate. The one-command role runner is
`scripts/run-m6-zec-taker-service-poc.sh`; it reuses the isolated endpoint lock,
bounded clock, exact evidence, and scoped cleanup of the certified corridor.

Commit `0ed6a59` keeps an already-admitted Refund live across the two
intermediate refunded phases while advanced Claim and terminal Refund replay
remain inert. This completes the nonvisual Claim happy-path slice, not M6.
Equivalent service-driven actual-node Refund, Maker and Taker Basecamp `ui_qml`
packages, their QtRO hosts, actor-real UI E2E, final repository gates, and the
M6 tag remain pending. Owner prototype signoff is complete. Durable
rollback-incarnation anchoring remains deferred production hardening rather than
an accepted issue-#112 PoC gate.

## M6 refund liveness and pinned-finality checkpoint (2026-08-03)

Three retained service-Refund runs closed successive REDs without claiming a
successful journey. The first proved that service availability could precede
the signed LEZ refund timestamp; the runner now waits for a finalized block
whose timestamp reaches the signed deadline. The second proved that a typed
`moving_tip` is retryable only through the same durable service request. The
third retained one admitted Refund and four exact retries but no chain refund.

The third failure is now separated into its actual causes. A local official
LEZ v0.2 `getAccountAtBlock` call measured about 9.78 seconds while generated
ZEC actors allowed 10 seconds for the entire bridge request. Separately,
nonterminal refund observations rejected any forward finalized-height movement
even after reading all facts at one immutable pinned height. The observed
`observe_escrow` moving-tip log belonged to an independent background poll and
was not itself the Refund eligibility failure.

ADR 0138 records the production-correct component fix. Deterministic-local ZEC
actors now use a finite 30-second bridge budget. Refund state, exact misses, and
discovery misses may retain their old pinned clock across a bounded forward
advance only after ID/hash agreement, complete parent-linked descendant proof,
and repeated pin verification. Regression, pin replacement and ABA, broken
ancestry, ID/hash disagreement, and advances beyond the protocol maximum still
fail closed. No devnet cadence was slowed and no finality check was removed.

The focused timeout test progressed RED at 10 seconds to GREEN at 30 seconds.
The complete ZEC reference actor suite and all 26 finalized native refund
observer tests are GREEN. The M6 service runner static contract remains GREEN.
This is a component checkpoint, not an actual-node Refund certificate.

## M6 terminal service timeout checkpoint (2026-08-03)

A fresh isolated run `m6refund407dbb3a` reached both legs locked and
`refund_available` after the signed finalized LEZ deadline. Its first service
Refund call then timed out at 15 seconds because the outer Unix-socket client
budget was shorter than the actor's 30-second bridge budget. The retained run
had exactly the two expected Taker LEZ submissions, Initialize and Fund, before
cleanup; it is quarantined and will not be retried or reused.

ADR 0139 separates 15-second read/query calls from 40-second terminal Claim and
Refund calls. The action budget remains capped by the 130-second monotonic
Refund corridor and strictly dominates the inner bridge while preserving ten
seconds of scheduling and durable-response headroom. Service admission remains
generation-fenced before actor I/O, and actor/sidecar journals retain exact
effect replay and uncertain-send authority; the timeout change cannot mint a
second effect.

The focused runner contract progressed RED before the two budgets and GREEN
after explicit Refund and Claim action wiring. At that checkpoint fresh actual-node Refund proof was next,
followed by post-terminal no-effect replay and a Claim regression run. Both are now GREEN.

## M6 durable terminal-conflict checkpoint (2026-08-03)

Fresh run `m6refund538c629a` returned the first durable Refund service commit
after the timeout correction, proving that the 40-second outer budget worked,
but its deliberate opposite Claim check returned action-unavailable. The actor
was correctly advertising only Refund; the service had consulted that transient
availability before the registry's already durable Refund winner. The run is
quarantined and its swap and funds will not be reused.

ADR 0140 now gives exact replay first precedence, followed by any sole durable
terminal winner, then actor generation/action availability, and finally atomic
new admission. A competing winner between the read and atomic admission maps to
the same fixed action-conflict result. New actions are still never admitted
before actor validation.

The process regression progressed RED at `-32016` to GREEN at `-32017`, proves
the losing request creates no second action row, and retains the original Claim
row byte-for-byte. The atomic race mapping unit regression and the broader 25
library plus two ZEC Chat process tests are GREEN.

Fresh run `m6refund9e84d76a` then returned the durable Refund commit and the
correct opposite-Claim `-32017` envelope on actual isolated nodes. Its runner
stopped before replay because two assertions still compared `error.data` to a
scalar instead of the stable `error.data.category` field. This was an evidence
consumer defect, not a protocol failure. The run is quarantined; the focused
runner contract now locks the object-shaped envelope and is GREEN.

ADR 0141 adds the final Refund certificate wiring before another effect-bearing
run. A mined Zcash refund is accepted only when the generated block hash,
verbosity-one block, expected height, exact-once transaction membership, and
height-to-hash canonical lookup agree. After both role actors report
`refunded`, the exact same service request is replayed and must return
`was_replay:true`; ordered successful LEZ submissions, Zebra tip, and empty
mempool must remain unchanged, and both canonical refund blocks are re-read.
The result binds all new receipts by SHA-256. The focused runner contract
progressed RED on absent terminal-replay evidence and is GREEN after wiring.
Fresh actual-node execution remains next.

Fresh run `m6refund8e0ed10a` proved the durable Refund, fixed opposite-Claim
conflict, in-progress replay, finalized LEZ refund, and isolated Maker recovery,
then exhausted the old 130-second outer corridor before the Zcash refund.
Measured time was consumed by the fixed 60-second refund deadline, two bounded
roughly 15 to 17 second service-to-actor reconciliations, and finalized LEZ
observation. The run is quarantined. The runner contract progressed RED to
GREEN at a 190-second outer ceiling; protocol deadlines, block cadence,
finality, and 15/40-second per-call limits are unchanged, so this is liveness
headroom rather than a slower happy path.

Fresh run `m6refund734db82a` repeated durable service Refund, opposite-Claim
conflict, exact replay, finalized LEZ Refund, and Maker recovery under that
ceiling, but the Taker's durable actor remained `both_legs_locked`. The
service replay file also stopped changing after the first reconciliation.
This proved that the ceiling was not the remaining fault. The main loop
captures `drive_m5_taker` through command substitution; Bash therefore ran
the entire Refund driver in a child shell and discarded its admitted
generation, pinned start tip, finalized transaction, and supervisor process
state on return. The Maker happened to complete work started in that child
while later parent rounds continued to treat Refund as unadmitted.

ADR 0142 moves supervisor startup out of the child and adds one explicit
validated handoff envelope. The parent accepts only an admitted generation and
start tip that cannot be replaced, permits finality only with one lowercase
64-hex transaction ID, rejects finalized-state regression, and starts Maker
recovery at most once. The executable contract progressed RED on the absent
handoff and is GREEN for pending, finalized, exact replay, replaced-generation,
and regressive-finality cases. No protocol timeout, chain cadence, or
190-second fail-safe ceiling changed. Both effect-bearing discovery runs are quarantined; fresh
certificate
`m6refund8f76d87a` subsequently closed the required proof.

Fresh pushed-commit run `m6refund5320572a` used new LEZ genesis allocations,
one new checked deployment, two new finalized Vault claims, and a new Zebra
height-104 Regtest prefix. It proved the ADR 0142 handoff through finalized LEZ
Refund and parent-owned Maker recovery. On the following already-admitted
reconciliation, the sidecar rejected a moving finalized observation tip and
the service returned the fixed `-32010`
`taker_action_execution_unavailable` envelope. The runner treated that
expected retryable actor result as fatal.

ADR 0143 accepts only that exact object-shaped response after durable Refund
admission, records it as a reconciliation transient, emits the same validated
parent state, and lets a later bounded main-loop round retry the same request
ID, swap, action, and generation. Any changed category or scalar envelope
still fails closed, as do a wrong JSON-RPC version or ID and any extra field.
The executable contract progressed RED on the absent classifier and is GREEN
for the accepted exact object plus the rejected variants. Final acceptance
semantically validates every admission and reconciliation transient. No
protocol deadline, per-call timeout, cadence, or finality rule changed. The
190-second outer fail-safe expired as the transient arrived, so it is now 300
seconds to allow a later bounded round; success does not wait for this ceiling.
The effect-bearing run is quarantined; at that checkpoint another fresh-node
certificate was required.

Fresh run `m6refund7be4428a` then proved that the transient retry was no longer
the first failure. It reached durable Refund, opposite-Claim exclusion,
finalized LEZ Refund, and parent recovery handoff, but no Zcash refund appeared.
The Maker daemon had been stopped after its funding transaction was confirmed
and before its own actor projected that lock. The Taker actor durably held both
locks, while the Maker coordinator still held only `taker_lock_confirmed`.
From that incomplete local phase, observing the Taker LEZ refund correctly
terminalized the Maker as if no second lock existed.

ADR 0144 adds one bounded observation-only reconciliation after the two funding
blocks and daemon/transport suppression. It requires exact one-call Maker
`maker_lock` projection to `both_legs_locked`, unchanged Zebra height, and an
empty mempool before and after. Final acceptance revalidates the evidence and
binds its SHA-256 into `result.json`. Thus the already-canonical funding fact
reaches durable Maker state without overlapping daemon authority or adding a
chain effect. The executable contract progressed RED on the absent
reconciliation and is GREEN after exact-call, timeout, changed-tip, dirty-
mempool, live-authority, and certificate-binding regressions. Run
`m6refund7be4428a` is quarantined; at that checkpoint a fresh-node certificate
was still required.

Fresh run `m6refund43f2cbca` proved the ADR 0144 projection on fresh LEZ
and Zebra nodes, then finalized one LEZ Refund and restarted Maker recovery.
The run did not produce the Zcash refund. Its windows, chain state, and actor
state were correct. Measured Logos LEZ v0.2 historical reads instead exposed
two liveness faults: simultaneous Maker discovery and Taker exact replay
oversubscribed the indexer's three historical slots, and the old 20/30/40
second supervisor/bridge/service layers were shorter than the two-phase Maker
and three-phase Taker observations.

ADR 0145 is GREEN at the component boundary. It suppresses only redundant
Taker action reconciliation after finalized LEZ Refund and during active Maker
recovery, preserving the exact parent handoff with zero Taker actor or action
RPC calls. It uses a generated local actor bridge budget of 60 seconds, a
refund-only Maker supervisor attempt of 75 seconds, and a service action caller
of 90 seconds below the unchanged 300-second corridor. It retains both
containing-block and finalized-tip account checks. The effect-bearing run is
quarantined and all funds, identities, and evidence must be fresh again.

Fresh pushed-commit run `m6refund8f76d87a` then completed in 211.530
seconds on wholly fresh LEZ deployment/onboarding `m6lez8f76d87a` and
Zebra `m6refundzec8f76d87b`. It finalized LEZ Refund
`c43df1bb...dcf5ad` exactly once at block 129, then confirmed Maker-owned
Zcash Refund `db066a94...5ab470` exactly once in canonical block 110.
Maker, Taker, and service reached `refunded`; the opposite Claim conflict
and exact replay were durable; the transient log was empty; terminal replay
changed neither chain and revalidated both inclusions. The retained packet is
`docs/evidence/m6-zec-service-refund-certificate-20260804.json`.

Fresh pushed-commit regression `m6claim0ba41aba` then completed the Claim
journey in 33.330 seconds on new LEZ deployment/onboarding
`m6claimlez0ba41aba` and new Zebra `m6claimzec0ba41aba`. LEZ Claim
`f865903e...14d0cc` is finalized exactly once in block 127; exact service
replay preserved the sole Zcash Claim `0da6b4c2...d2abf`, canonical at height
107. Maker, Taker, and service reached `completed`; drive retries and actor
stderr were empty. The retained packet is
`docs/evidence/m6-zec-service-claim-regression-certificate-20260804.json`.

Next in order:

1. build the Maker and Taker Basecamp QML/backend packages, reproducible LGX
   artifacts, and actor-real UI E2E before final M6 gates/tag.

The measured Logos v0.2 historical-account latency, per-account genesis
reconstruction, and lack of a batched multi-account-at-block RPC are recorded
as LOGOS-024. They do not block local milestone
certification under the accepted Logos-owned dependency policy. Shared
concurrency control, caching, and coalescing identical in-flight repeatable
observations remain production hardening.

## M6 Basecamp 0.2 toolchain preflight checkpoint (2026-08-04)

The official Logos tutorial at exact commit
`bfc34c451c08da9f78072dd825756a1e071a051d` was copied into a run-private
temporary directory and its C++/QML calculator was locked against the exact
module-builder 0.2.0 commit
`92ef691ea72844134f6c68fb447d37f855fc9690`. The rehearsal used only the
digest-pinned Linux/amd64 Nix 2.35.1 image
`sha256:d78540374f6a886653cba47d5c3f61c5a41d42e2a8db2607b8d68cb226fd463e`,
a dedicated Nix-store volume, unique container names, literal temporary
mounts, four CPUs, 12 GiB memory, and a 1024-process cap. It did not edit the
repository or touch unrelated Docker resources.

The default package built successfully from the generated consumer lock. Its
2,130,512-byte NAR has hash
`sha256-UoyshKh+zzMVigumE3BhjMgQUEFaM8HsuyFcXvCEdpk=` and explicitly retains
Qt 6.9.2 Qt Remote Objects. The `.#lgx` output then built successfully and
created `logos-calc_ui_cpp-module.lgx` with file SHA-256
`d184c0423dc7dc5bee98e74eb1cf51c4edc3e381ce017ab88a38caf857e13bd5`.
This closes the pre-signoff toolchain feasibility risk without implementing a
production Maker or Taker view.
The retained machine-readable result is
`docs/evidence/m6-basecamp-toolchain-preflight-20260804.json`.

The documented core prerequisite was also packaged into
`logos-calc_module-module-lib.lgx` with SHA-256
`959126dcd54ded28be30a33c63a9c191febf119b7bd7f3c664ae89376e8d8f54`.
Exact `lgpm` 0.2.0 commit `7a1f1cf35b22dc1a3407d6b5cafce333321be584`
built successfully and installed both unsigned official tutorial artifacts into
an owner-private temporary tree. Its JSON inventory resolved `calc_ui_cpp` as
a `ui_qml` package depending on `calc_module` and retained the plugin, replica
factory, and QML view. This proves installation and dependency discovery, not
Basecamp runtime load; unsigned input was allowed only for this local tutorial
rehearsal.

The exact Basecamp 0.2.0 root at
`48b26c0d33573b5dd3695ae5868b04328f79e5c6` was first stopped safely before
host exhaustion. After the disk cleanup, a fresh isolated replay built the
official `smoke-test` output without accepting its untrusted extra cache. The
Basecamp 0.2.0-RC3 binary loaded the capability, package-manager,
package-downloader, and main-UI modules, connected local Qt Remote Objects, and
passed its expected five-second offscreen smoke. The exact smoke output is
`/nix/store/cckzvs6p79ygfd2l0rmw8816nklnyndp-logos-basecamp-smoke-test`, NAR
hash `sha256-lfg55Q/2x84ormtBRzFytP4hMfd1jH0sS7oIkcQN3nI=`, with a
2,749,148,608-byte closure. This certifies the pinned official runtime, not a
Maker or Taker package load.

After evidence capture, exact-name cleanup removed the 5.254 GB dedicated Nix
volume, 988 MB pinned Nix image, and 9.2 MB temporary checkout. Reported host
free space moved from 406 to 411 GiB while an unrelated active Miden build was
simultaneously consuming Docker storage. Its images, 17.1 GB build cache,
anonymous PostgreSQL volume, and `pr127-pg` container were left untouched.

The upstream all-output `nix flake check --no-build` did not pass: evaluation
of its integration-test output referenced a missing Nix-store source path.
The isolated Nix store subsequently passed full existence, link-hash, and
content-hash verification, while the package and LGX outputs remained green.
ADR 0146 therefore requires separate locked package, LGX, repository-owned UI
test, and optional official-harness lanes rather than suppressing the failed
upstream check.

The generated graph is fully revision/NAR locked but large and revision-
duplicative. Five direct Logos sources in the selected graph have no license
file or declared package license. LOGOS-025 records that production-release
finding. The local M6 PoC may proceed under the accepted Logos-owned dependency
exception; distribution and production readiness may not silently inherit a
license grant or skip a realized-closure SBOM, vulnerability, and license
review.

Owner signoff released the issue-#112 gate on 2026-08-04. RED now begins for
the package contract and actor journey; GREEN will implement the two
consumer-locked packages, load them in this proven pinned runtime, and run the
actor-real journey with an explicit isolated disk budget.

## M6 Basecamp role-package and actor-real checkpoint (2026-08-04)

Commits `149cb84`, `0141e60`, and `e3e6907` close that RED. Two independent
consumer-locked `ui_qml` packages now expose six typed Maker slots and seven
typed Taker slots. Each QML view talks through its own process-isolated QtRO
backend to one fixed owner-service method allowlist over an effective-user-owned
mode-0600 Unix socket. The shared client rejects symlinks, non-sockets, wrong
owners and modes, messages above 64 KiB, TCP/web transports, arbitrary method
dispatch, and command execution.

Both module outputs, LGXs, developer-install trees, and official standalone
integration outputs build against module-builder 0.2.0 at exact commit
`92ef691ea72844134f6c68fb447d37f855fc9690`. Both packages load in exact
Basecamp commit `48b26c0d33573b5dd3695ae5868b04328f79e5c6`, internal
`0.2.0-RC3`. Separate user directories and owner sockets preserve roles. The
product container runs with no network and the same effective UID as the
service.

Repository-owned product tests prove both missing-service fail-closed paths.
The Maker package then calls real daemon health, atomically saves one route, and
reads history from isolated SQLite. The Taker baseline calls real service health
and offer list. The stronger prepared rendezvous enters the exact authenticated
offer facts through the Basecamp QML, observes new initiation, repeats the same
fixed UI request as exact replay, lists swaps, and monitors the admitted swap.
After Basecamp exits, the Rust process test opens the actual registry and proves
`taker-ui-initiate-001` durably maps to `m6-process-zec-swap-001`.

This product proof is intentionally layered with the fresh actual-node Claim and
Refund certificates. It proves UI/package/backend/service composition through
admission and monitor, not that the Basecamp click itself emitted their retained
LEZ or Zcash transactions. ADR 0147 records the component schema, Maker and
Taker operation sequences, per-pair terminal sequences, atomicity arguments,
failure behavior, and limits. The machine-readable package evidence is
`docs/evidence/m6-basecamp-role-packages-20260804.json`.

Cold package construction can depend on DNS, immutable GitHub flake inputs, and
`cache.nixos.org`; runtime is networkless and uses no public RPC, faucet, public
funds, or public deployment. A fully network-disabled cold reconstruction was
not proven because the Nix fetcher source cache was incomplete. LOGOS-025 keeps
license, signature, upstream all-output evaluation, realized-closure SBOM and
vulnerability review, graph review, and offline-cold-build work release-blocking
but nonblocking for the private local PoC under the accepted Logos exception.

M6 certification closure:

1. requirements traceability, the exact diff, JSON evidence, architecture
   compatibility, all 429 Mermaid renders, and the package contract are GREEN;
2. formatting, strict all-target/all-feature Clippy, the locked workspace
   all-target test suite, both feature-gated crash seams, warning-fatal Rustdoc,
   root dependency advisories/bans/licenses/sources, Node audit/license, CI
   hardening, and the pinned shell/workflow/Dockerfile/Compose quality wrapper
   are GREEN;
3. the sandboxed, networkless browser prototype is GREEN 6/6; host AppArmor did
   not permit Chromium's namespace sandbox, so neither UI nor Mermaid proof used
   `--no-sandbox` on the host;
4. all seven exact `/tmp/lez-m6-*` paths, the 33 GiB repository Cargo target,
   the dedicated M6 Nix volume, and both unused pinned test images were removed.
   No unrelated Docker resource was pruned, and 354 GiB was available after
   cleanup;
5. `m6-poc-complete` identifies the final clean certification commit once its
   push and annotated tag push succeed. No remote-green claim is made because
   no observable Actions result was used for this local certification.

Certification RED/GREEN update: warning-fatal Rust 1.96 Clippy rejected the
unchanged five-minute external Basecamp rendezvous ceiling when written as
`Duration::from_secs(300)`. The semantics-preserving refactor expresses it as
`Duration::from_mins(5)`. Focused and full workspace all-target/all-feature
Clippy are GREEN; no timeout value or product behavior changed.

The first full workspace test attempt exposed independent shared-host
contention: the two XMR Chat process journeys concurrently secure-hashed large
debug actor binaries and both missed their unchanged 30-second per-daemon
readiness bound. Disk had 324 GiB free. An unchanged sequential diagnostic
passed 2/2 in 255.34 seconds, isolating parallel fixture thrash rather than a
daemon defect. A test-only Tokio mutex now serializes only those two process
journeys. The original default test command is GREEN 2/2 in 250.71 seconds;
production code, RPCs, timeouts, and protocol behavior are unchanged.

The next exact workspace run exposed the same class in the two
daemon-supervisor process journeys: parallel child churn made one isolated test
miss an expected PID observation. The unchanged sequential diagnostic passed
2/2 in 17.64 seconds. A second test-only standard mutex now serializes only
those two daemon process journeys. Their original default command is GREEN 2/2
in 21.86 seconds; production supervisor concurrency coverage remains inside the
three-pair test, and no production scheduling or timeout changed.

The retained authoritative rerun
`cargo test --quiet --locked --workspace --all-targets` is GREEN. Three tests
remain explicitly ignored because they require their pinned Docker/Zebra
actual-node routes; their corresponding fresh terminal certificates are
retained separately rather than falsely counted as part of the workspace run.

Final quality RED/GREEN found four stale static-contract assumptions and one
invalid GitHub Actions context. The M5 contract now recognizes the M6-specific
suppressed-authority status observation, both `Completed` and `Refunded` real
RPC enum projections, and the one receipt-bound CLI authority branch. The XMR
contract follows the current typed loader into its Taker-role semantic
validation instead of requiring a removed redundant method call. The service
lifecycle contract follows the shared secure-file module that accepts only
owner-owned single-link mode-0400-or-0600 bounded regular files. The M6 browser
test's `${{ runner.temp }}` expression moved from invalid job scope to step
scope. The complete maintained quality wrapper is GREEN after these changes.

The one-time diagram gate first rejected `Actor`, a reserved GitHub Mermaid
sequence identifier, in the three pair diagrams. Renaming only that participant
identifier to `PairActor` made all 429 conservative compatibility checks and all
429 isolated SVG renders GREEN without changing the documented flows.

## M7 Tag-17 durable release checkpoint (2026-08-04)

RED first changed the authenticated Maker route contract from
`Unavailable` to an exact official transaction requirement. The focused test
failed at the expected remote `unavailable` response. An unrelated first
compile attempt entered the pinned Rapisnark download fallback because
`RAPIDSNARK_LIB_DIR` was not exported and the host has no `unzip`; rerunning
with the already hash-verified local v0.0.8 library directory reached the
intended RED in 5.56 seconds without installing or downloading dependencies.

GREEN reuses the pinned generated instruction and NSSA transaction types but
does not call the generated submit-on-build helper. The isolated Maker planner
constructs tag 17 with ordered metadata, custody, and claimant accounts, signs
with the claimant key, checks the immutable punishment-message hash, and
persists exact bytes before exposure. The authenticated server restores that
reservation after restart. Generic submission accepts only the transaction-ID-
derived request identity and byte-identical owner-only reservation, then uses
the existing lookup-plus-one-send durable journal.

The focused route test and a separate operator-behavior test are GREEN. The
latter proves an arbitrary release ID causes zero node calls, the exact release
causes one lookup and one send, identical retry causes no node I/O, and restart
replays both preparation and accepted release without consulting a changed
nonce source. ADR 0158 records the component and sequence diagrams, conditional
atomicity argument, resource boundary, and limits.

The first pushed actual-node rehearsal `m7tag17124df10a` then proved the
checked guest build/deployment, actor onboarding, Tag13 prerequisite,
pre-boundary classification, durable Tag17 preparation, and exactly one
accepted transaction-ID-bound release. It failed at canonical evidence because
the observer did not yet classify `Punish` and requested full coverage of a
fixed 64-block range. Cleanup still passed exactly. Focused RED/GREEN now covers
Maker exact-owner and Taker terms-discovery facts, the inclusive boundary,
terminal state and custody, while contiguous eight-block finalized pages remove
the unnecessary future-range wait without weakening Bedrock finality.

Fresh pushed replay `m7tag17a23a314a` closes that RED on commit `a23a314`.
The current five-of-five guest was freshly deployed as ImageID
`b7f87278...b0433`; the pre-boundary finalized clock was below `punish_at`;
one transaction-ID-bound release finalized at height 124; and Maker exact-owner
and Taker discovery retained identical canonical facts, `Claimed` metadata and
zero custody. The eight-block value is contiguous pagination, not confirmation
depth. Exact cleanup and independent absence checks passed. The checked
certificate is `docs/evidence/m7-actual-tag17-a23a314-20260804.json`. F5 is
therefore GREEN at the local-functional boundary. F3 and F6 remain open only
for joined two-devnet abandonment economics, losing-branch proof, and adverse
process/concurrency cases; public deployment remains deliberately deferred.


## M7 branch-aware Maker recovery checkpoint (2026-08-04)

RED first proved that the actor had no durable branch-to-step selector. GREEN
adds one read-only, identity-validated journal query and rejects Claim or an
unselected branch. A second route check caught that only Tag17 supports semantic
preflight; Refund correctly enters its one-attempt CAS directly.

The real supervisor and actor process test then exposed two fixture assumptions:
the Tag17 step name was mechanically corrupted during refactoring, and the
read-only Monero verifier was incorrectly expected to receive private-share FD
218. Focused regression tests fixed the canonical route name and preserved the
least-privilege rule: only the Refund invocation receives FD 218. The final
process run is GREEN in 237.52 seconds. Tag17 executed preflight, invoke and
observe exactly once; Refund executed invoke and observe exactly once; both
completed the durable operator action without a restart send.

ADR 0164, system/deployment component and RPC diagrams, manual Flow 1ZC, the
root README, readiness write-up, traceability and both machine-readable M7
inventories now record the exact boundary. No Docker, node, RPC, faucet, DNS,
peer, public funds or public deployment participated. Inventory remains
honestly 14 hard requirements open, 4 submission groups open, and S12/S13 as
the only two external-review items. ADR 0166 subsequently closes the
semantic no-argument Maker Monero refund worker. The fresh joined actual-node corridor, semantic Taker claim sweep, and
adverse crash/concurrency matrix remain the next repository-owned slices.

## M7 joined Maker refund restart checkpoint (2026-08-05)

Exact pushed run `m7refund-e7016d8-a` closed the ADR 0170 handoff uncertainty:
the semantic sender submitted one real Maker-directed Monero refund, durable
receipt detection survived queued, leased and backoff scheduler states, and the
separate driver mined exactly ten official Monero 0.18.5.1 Regtest blocks. The
wallet and daemon agreed on the exact incoming unlocked output and ten
confirmations. No public RPC, faucet, peer, public funds or public deployment
participated, and exact cleanup passed.

The run then exposed the next restart defect before terminal observation. The
schema-3 manifest treated the provisioning-time SHA-256 of the mutable SQLite
role journal as a permanent runtime invariant. The sender's legitimate database
open/checkpoint changed representation bytes without changing any validated
session state, so every observer cycle failed during authority loading. A
throwaway exact-FD diagnostic reproduced `role journal digest differs from
provision manifest`; its 3.1 GB build directory was removed after diagnosis.

RED adds a real refund-route regression that invokes once, rewrites the same
valid journal with SQLite `VACUUM`, proves its bytes changed and requires the
restart-only observer. GREEN retains the digest as provisioning provenance and
uses the existing stable owner-only, sidecar-free, full semantic journal
validation at restart. Immutable application inputs remain digest-pinned. The
focused regression and complete XMR reference-actor suite are GREEN. ADR 0171
records the component, sequence and atomicity decision.

The real normal-supervisor regression then exposed stale process-fixture
custody: the production refund route requires the finalized Tag16 signature on
invocation-only FD 219, while the fixture still supplied only private-share FD
218. Before the correction the actor failed closed before its worker and the
supervisor recorded `actor_exit_failed`; no effect log existed. GREEN now
supplies both invocation-only inputs, asserts both are absent from the observer,
and completes Tag17 plus Monero Refund invoke/restart/reconcile in 302.10
seconds without relaxing any supervisor assertion.

Exact pushed-commit run `m7refund-7cd3a9c-a` closes that PoC gate. It freshly
built and deployed the five-of-five guest, finalized Tag13 and Tag16, admitted
one generation-fenced owner Refund, retained one semantic Maker Monero send,
mined exactly ten confirmation blocks outside both effect children, and reached
workflow revision 2 with completed manual action and terminal scheduler state.
The retained `monero-refund-finalized.json` is byte-bound in
`docs/evidence/m7-actual-maker-refund-7cd3a9c-20260805.json`; it remained mode
`0600` and single-link after source-status-zero exact cleanup removed the
private source and all run-owned Docker resources. This run proves durable
same-daemon retry/terminalization, not a daemon restart after submission.

The next repository-owned slices are semantic joined Claim and the Taker
Monero claim sweep, abandonment/adverse recovery, accepted-application
actual-chain concurrency, timelock/fee/reorg matrices, demos, and the immutable
candidate security/SBOM/vulnerability dossier. Corrected ETA remains 1 to 2
focused days for repository-controlled candidate closure, excluding independent
S12/S13 review and policy-deferred public deployment.

## M7 receipt-v2 Tag14 join checkpoint (2026-08-05)

- [x] Add an evidence-driven Taker Claim activation command with no operator
  branch selector. It validates schema-2 application authority, exact Stage
  A/B, finalized Tag13 Initialize/Fund and the independent confirmed Monero
  funding pair before importing role-local reconciliations and preparing only
  Tag14.
- [x] Add a sealed read-only Tag14 classifier that derives exact terms from
  Stage A/B, uses fresh read request identities, scans from the retained Tag13
  successor, and atomically publishes one canonical finalized receipt.
- [x] Wire an isolated `M7_XMR_SEMANTIC_CLAIM=1` runner mode through a real
  receipt-v2 upgrade and the literal `lez-taker claim --receipt` user flow.
  Existing legacy claim and refund defaults are unchanged.
- [x] Record the component, user-flow and conditional-atomicity decision in ADR
  0173. Focused provisioning tests, both existing runner contracts, formatting,
  compile checks and strict Clippy are GREEN.
- [x] Commit and push the implementation checkpoint as `aae5c5c`.
- [x] Complete one clean commit-pinned actual LEZ/Monero replay. The first
  replay reached the application handoff and preserved a RED caused by numeric
  jq truthiness before Tag13; exact cleanup passed. The follow-up emits a real
  JSON boolean and retains byte-identical safe activation/finality evidence;
  focused syntax, boolean-regression, M4 and M5 contract checks are GREEN.
  Until the fresh replay succeeds, Tag14 remains implementation-ready rather
  than actual-node certified.
- [x] Replay the direct effect-promotion correction. The `a204cca` replay
  proved the boolean fix, finalized Tag13, confirmed Monero funding and release
  preparation, then correctly rejected full actor reprovisioning after the
  role journal had advanced. The runner now reuses
  `provision-effect-application` on the accepted actor and composes canonical
  receipt v2 from the immutable receipt v1 plus digest-pinned effect provision;
  exact cleanup and focused runner contracts are GREEN.
- [x] Replay the typed Tag13 correction. Exact run `m7claim-2d3c859-a` proved
  the direct effect promotion and both chain prerequisites, then preserved a
  RED because the new activator decoded typed canonical producer bytes through
  an untyped JSON map before comparing them. Cleanup schema v2 passed. The
  activator now decodes the typed schema directly, and the full XMR
  reference-actor suite is GREEN.
- [x] Replay the complete-schema Tag13 correction. Exact run
  `m7claim-987dd32-a` falsified the reduced typed decoder at the same boundary:
  it ignored producer-owned fields before canonical reserialization. Cleanup
  schema v2 again passed. The activator now mirrors the complete producer
  schema and nested field order, reuses the shared typed escrow terms, and
  denies unknown fields; focused library tests are GREEN.
- [x] Replay the producer-exact durable Tag13 reader. Exact run
  `m7claim-7cd0d88-a` proved every prerequisite and the complete schema but
  exposed the final encoding distinction: the producer's stdout is compact,
  while its authoritative durable file is canonical pretty JSON plus newline.
  The activator now reproduces that exact producer encoding without changing
  the generic compact evidence reader; cleanup schema v2 passed.
- [x] Replay newline-free receipt-v2 composition. Exact run
  `m7claim-0c88ec7-a` proved the durable Tag13 reader and completed Taker Claim
  activation, then the literal CLI rejected receipt ambiguity because jq had
  appended a newline to otherwise canonical bytes. The runner now uses
  join-output mode; Bash syntax and M4/M5 runner contracts are GREEN, and exact
  cleanup passed.
- [x] Replay newline-free sealed release-key composition. Exact run
  `m7claim-d297163-a` proved the receipt-v2 correction, completed Taker Claim
  activation, and entered the literal semantic Tag14 route. Its non-sending
  child then failed before eligibility because `openssl rand -hex` had appended
  a newline to the 64 lowercase-hex protection key. The older pathname loader
  tolerated that terminator, while the least-privilege sealed-descriptor worker
  correctly requires exactly 64 bytes. The runner now strips only that output
  terminator before the key is persisted; exact cleanup passed.
- [x] Replay newline-free sealed release-capability composition. Exact run
  `m7claim-fa7e3ec-a` showed that the key correction was necessary but
  incomplete: it reached the same non-sending child, where the copied sidecar
  bearer also retained the launcher's line terminator. The ordinary pathname
  reader explicitly removes one line ending; the sealed worker correctly gives
  raw descriptor bytes to the strict bearer grammar. The dedicated release
  copy is now normalized into a distinct create-new inode and asserted to be
  exactly 64 bytes. The live sidecar credential and strict worker are unchanged;
  exact cleanup passed.
- [x] Replay the bounded Tag14 observer. Exact run `m7claim-5a6606f-a`
  proved both sealed-input corrections, admitted the one semantic Tag14
  publication, and independently located its finalized transaction in block
  135 from scan start 123. The 64-block single-request classifier exceeded its
  20-second transport and 30-second parent bounds, so every observation-only
  retry restarted the same scan. The PoC now uses the existing actual-runner
  16-block bound, which covers this deterministic 12-block interval. Durable
  multi-page cursoring remains explicit production hardening. The nonproductive
  loop was interrupted through its normal trap; cleanup schema v2 passed.
- [x] Replay exact owner-side Tag14 observation. Exact run
  `m7claim-b8aa8a0-a` proved the 16-block bound, admitted one publication and
  retained workflow `attempt_count=1`; a read-only diagnostic found the exact
  transaction at block 136 from scan start 125 in about five seconds. The
  sealed observer nevertheless used discovery-by-terms through the Taker
  sidecar, which correctly rejected that owner route as `InvalidTransaction`:
  contract tests permit owner-exact or counterparty-discovery, never
  owner-discovery. The correction does not lend the Taker a Maker capability.
  After the send CAS, the trusted parent authenticates the encrypted release
  snapshot, decrypts only its now-public prepared transaction, seals canonical
  JSON on fixed FD 224, and the Taker observer classifies that exact transaction.
  Prepared/suppressed records cannot expose observation material. Focused
  all-target compilation is GREEN; the interrupted replay's exact Docker
  containers, networks, volumes and images were verified absent.
- [x] Replay descriptor-native Tag14 capability custody. Exact run
  `m7claim-95876e4-a` proved the release correction itself: the journal was
  admitted once at revision 2, FD 224 contained canonical byte-identical exact
  transaction `98207f30...c885d`, and an authenticated owner-exact diagnostic
  found it at finalized block 135 inside the declared 125..140 window. The
  sealed observer still failed before RPC because the ordinary pathname
  capability factory correctly rejects `/proc/self/fd/201` as a symlink, even
  though FD 201 was a sealed owner-only memfd. The classifier now validates FD
  201 through its descriptor-native sealed-input reader, normalizes at most one
  line ending, zeroizes rejected bytes, and constructs the official bridge
  client directly. The regular-path factory and its symlink protections remain
  unchanged. Focused formatting, all-target compilation, and diff hygiene are
  GREEN; exact normal-trap cleanup left no run-labelled containers.
- [x] Replay owner-exact Tag14 consumption by the Maker actor. Exact run
  `m7claim-194b974-a` proved descriptor-native capability custody and produced
  canonical finalized owner-exact Tag14 evidence through the authenticated
  Taker sidecar: the exact transaction was finalized at block 131 inside the
  bounded 120..135 scan. The release workflow completed, but the Maker actor
  still applied its older counterparty-discovery assertion and rejected the
  Taker-owned exact evidence as the wrong sidecar. The consumer now requires
  the Taker role plus an exact target only for Tag14 claim completion. Tag15,
  refund, and sweep consumers retain their role-local discovery checks. Exact
  cleanup passed and all run-labelled containers were removed.
- [x] Join the receipt-v2 Taker Monero claim sweep. Exact pushed-commit run
  `m7claim-2cff48d-a` completed with source status zero. Owner-exact Tag14
  transaction `6697fb1d...c986b` finalized at block 136 in bounded window
  126..141; role-local Tag15 transaction `3ff01f31...e09f` finalized at block
  150 with custody zero; the extracted Maker share enabled Taker sweep
  `e8209a8a...85f0`, confirmed ten times on official Monero 0.18.5.1 Regtest.
  The owner-private binder checked the finalized signature, reconstructed key,
  exact fee and receipt. No public RPC, peer, faucet, public funds or public
  deployment participated. Cleanup schema v2 passed, preserved the foreign
  sentinel and removed every exact run resource.
- [ ] Post-PoC RED/GREEN contracts: owner-exact/counterparty-discovery source
  separation, release-store states, descriptor/newline bounds, restart and
  adverse concurrency. Then run the QA, chaos, information-security and
  production-readiness matrices before candidate closure.
  The first focused regression now locks Tag14 to Taker owner-exact evidence,
  rejects Taker discovery and Maker exact evidence, and proves a correctly
  sourced but unavailable result remains pending. Existing role-local discovery
  regressions continue to protect Tag15 and refund consumers.
  A second RED proved that a stale PublicationStarted snapshot could still open
  the encrypted transaction after another process durably recorded Suppressed.
  `exact_publication` now authenticates and equality-pins the supplied snapshot
  to the current durable row before its state gate or decryption. A follow-up
  cross-connection RED proved suppression could still commit after that read.
  Opening a Started publication now atomically commits Ambiguous before bytes
  leave the boundary, making disclosure irreversibly observe-only; a later
  suppressor fails its Started CAS. Prepared and Suppressed reject, while
  Admitted and Ambiguous remain readable, and wrong keys authenticate-fail.
  Descriptor-relative open-existing returns Missing without creating a database
  and rejects a pre-existing empty file without initializing it.
  A third RED proved the sealed Tag14 view-key parser accepted an unbounded run
  of trailing CR/LF bytes. It now accepts only raw canonical hex, one LF, or
  one CRLF. Descriptor-native capability custody is capped at the exact
  128-byte bearer plus one CRLF (130 bytes); focused sealed-memfd tests cover
  raw/LF/CRLF, repeated/lone line endings, over-bound input, and invalid UTF-8.
  Deterministic async cancellation now covers both post-CAS suspension seams:
  waiting on the decisive finalized clock and waiting on the node submission.
  Each simulated process loss durably retains PublicationStarted; restart opens
  the exact transaction into Ambiguous and a fresh publication transport sees
  zero clock and zero submission calls.
  A barrier-synchronized two-connection race now executes exact disclosure and
  suppression against the same Started revision. SQLite admits exactly one:
  disclosure yields Ambiguous and rejects suppression, or suppression yields
  Suppressed and rejects disclosure. No schedule can both expose bytes and
  retain a known-no-send state.

## M7 joined abandonment progressive PoC (2026-08-07)

- [x] Start with a RED runner contract requiring an isolated joined-abandonment
  mode, actual Monero funding before Tag17, re-observation of the same Stage-A
  output afterward, and explicit penalty-model rather than literal F6 claims.
- [x] Add `M7_XMR_JOINED_ABANDONMENT=1` only for the protocol-only Punish
  journey. Claim, Refund, ordinary Punish, semantic Claim and supervised Refund
  defaults remain unchanged and mutually exclusive.
- [x] Bind the pre/post observations by transaction, agreement, genesis,
  destination, amount and containing block; require zero peers and no public
  resources. Preserve the documented view-only key-image residual instead of
  claiming independent unspent authority.
- [x] Record the components, RPCs, sequence and conditional economic-safety
  argument in ADR 0174 and manual Flow 1ZG.
- [x] Push the clean implementation checkpoint as `3a25e22` after the full
  pinned CI-quality wrapper passed, including ShellCheck, workflow/container
  policy, milestone contracts, and focused Rust route tests.
- [x] Execute fresh isolated replay `m7abandon-a742c9f-a` from exact pushed
  commit `a742c9f` against LEZ v0.2 plus official Monero 0.18.5.1 Regtest.
  Source status was zero, exact cleanup passed, the foreign sentinel survived,
  and the checked secret-free certificate is
  `docs/evidence/m7-actual-joined-abandonment-a742c9f-20260807.json`.
- [x] After the PoC, add the certificate contract RED, then make it GREEN and
  wire it into both the pinned quality runner and CI-hardening policy.
- [x] Record measured iteration cost: the cold run took about 57 minutes and
  exhaustive finalized deployment-history validation consumed about 13
  minutes despite one-second LEZ slots. Preserve evidence depth; investigate
  safe read batching or parallel validation during hardening.
- [ ] Post-PoC hardening: inject losing Tag14/Tag16 branches, process kills,
  concurrent recovery, fee pressure and reorgs. Do not mark F3/F6 GREEN or tag
  M7 before those repository-owned cases and the remaining global gates close.

## M7 losing Tag16 after Tag17 hardening (2026-08-07)

- [x] Add a RED runner contract for an isolated
  `M7_XMR_LOSING_TAG16_AFTER_TAG17=1` mode.
- [x] Require joined abandonment, complete the existing valid Tag16 signature
  before Tag17, and leave every default journey unchanged.
- [x] After finalized Tag17, invoke the existing Tag16 process exactly once;
  record exact `accepted` admission or admission `unknown` and permit no retry.
- [x] RED then GREEN: bracket the losing process with finalized anchors, require
  its complete interval plus an eight-block finalized tail to contain no Refund,
  and byte-equivalently re-observe the exact winning Tag17 facts.
- [x] Record components, RPCs, sequence, atomicity scope, residuals, and manual
  repetition in ADR 0175 and Flow 1ZH.
- [x] Replay `m7lose16-a720b96-a` through finalized Tag17; preserve its RED
  finding that protocol-only losing mode omitted the Tag16 binary from the
  staged build, then verify exact cleanup passed.
- [x] Add a focused RED build/staging assertion and make losing mode build,
  declare, and stage the exact Tag16 driver independently of application mode.
- [x] Replay `m7lose16-4c891e9-a` through the late attempt; preserve its RED
  finding that LEZ can return transport `accepted` before stateful execution,
  then verify exact cleanup passed.
- [x] RED then GREEN: remove synchronous rejection as an atomicity oracle;
  validate exact accepted evidence or retain local failure as admission unknown
  while requiring finalized Refund absence and unchanged exact Tag17.
- [x] Interrupt exact run `m7lose16-8b91756-a` during build before node
  provisioning after a source audit proved its requested scan-start comparison,
  terminal-absence classifier, and post-attempt anchor were under-specified.
- [x] RED then GREEN: bind the requested scan start, classify Refund absent only
  from terminal Claimed/zero custody at candidate and window end, bracket the
  attempt with authenticated actual finalized-tip clocks, and retain raw and
  canonical evidence hashes plus exact request/transaction identities.
- [x] Push clean checkpoint `930e3b4`, run exact two-devnet replay
  `m7lose16-930e3b4-a`, and retain a checked secret-free certificate only after
  source status zero and exact cleanup passed without touching foreign Docker
  activity.
- [x] Add the certificate contract RED, then make it GREEN and wire it into the
  pinned quality runner and CI-hardening policy.
- [ ] Follow with the opposite ordering, concurrent boundary schedules,
  process-kill recovery, fees, and reorg cases before changing F3/F6 state.

## M7 losing Tag17 after Tag16 hardening (2026-08-07)

- [x] Add a focused RED classifier case for an included valid Punish under
  terminal Refunded metadata and zero custody.
- [x] Generalize finalized-effect exclusion so terminal Claimed excludes
  Refund and terminal Refunded excludes Punish, while all other effects keep
  fail-closed behavior.
- [x] Add a RED runner contract for isolated
  M7_XMR_LOSING_TAG17_AFTER_TAG16=1 and make the manifest contract GREEN.
- [x] Restrict the mode to the application refund journey, make it mutually
  exclusive with other M7 hardening modes, and preserve default behavior.
- [x] Prepare exact Tag17 before Tag16; bind finalized Tag16 to the submitted
  transaction; attempt Tag17 once after punish_at; treat exit failure only as
  admission unknown.
- [x] Bracket the attempt with authenticated actual finalized tips, scan the
  exact Tag17 target through an eight-block tail, and require terminal
  Refunded/zero-custody exclusion.
- [x] Re-observe canonical complete Tag16 facts and require byte-equal hashes.
- [x] Record components, local RPCs, sequence, bounded atomicity argument,
  residuals, and manual reproduction in ADR 0176 and Flow 1ZI.
- [x] Run the full pinned CI-quality wrapper, push clean implementation
  checkpoints 1b60283 and 5b2bb71 to origin/main, and report the corrected
  replay ETA.
- [x] Retain bounded run m7lose17-5b2bb71-a: exact Tag16 finalized Refunded/0,
  one late transport-accepted Tag17 produced no Punish effect through finalized
  heights 221..228, then a single unguarded read-only Tag16 reobservation timed
  out; exact cleanup passed and no foreign resource was targeted. This is
  diagnostic evidence, not a milestone certificate.
- [x] RED then GREEN: require bounded unique-request retries for the final
  read-only Tag16 reobservation while failing immediately on inconsistent
  Found facts.
- [x] Replay the retry fix from its exact pushed commit on isolated LEZ v0.2
  and Monero 0.18.5.1 Regtest; validate source status, proof packet, exact
  cleanup, and foreign-sentinel preservation.
- [x] RED then GREEN: add a checked secret-free certificate and wire it into
  the quality runner and CI hardening policy.
- [ ] Continue with concurrent boundary schedules, process-kill recovery,
  fees, and reorg cases before changing F3/F6 state or tagging M7.

## M7 Monero refund process-kill recovery (2026-08-07)

- [x] Audit the existing schema-3 Maker refund sender, durable workflow,
  supervisor lease, feature-gated submitted-effect pause, and joined
  actual-node refund runner before choosing the crash boundary.
- [x] RED then GREEN: admit `sweep_monero_refund` only for XMR `recover`;
  preserve the existing owner-private no-clobber marker and reject a `drive`
  result for the same operation.
- [x] Inject the hook only for the exact configured swap and matching Recover
  child. Production builds remain unchanged because daemon flags, environment
  injection, actor pause code, and helper exports are feature gated.
- [x] RED/GREEN real-actor process test: pause after the sealed refund worker
  succeeds, kill the exact actor process group before stdout, then prove the
  next generation terminalizes with effect log exactly `invoke, observe` and
  a completed manual Refund action.
- [x] Record components, recovery sequence, conditional atomicity argument,
  and explicit evidence limits in ADR 0177.
- [x] Extend the isolated joined actual-node refund mode to kill the exact
  daemon and actor identities at the marker, restart the same database and
  registry, prove abandoned lease-generation transfer and unchanged
  submission inode/digest/transaction, then mine ten confirmations.
- [x] RED then GREEN runner contract: require the feature-only hook build,
  submitted marker, daemon-then-actor kill order, sealed-memfd actor identity,
  greater recovered generation, unchanged submission identity, restart before
  mining, and no public resources.
- [x] First exact replay reached finalized Tag16 and a durable Monero refund
  send, then proved the crash trigger incorrectly required the actor's
  revision-one stdout projection before killing a process paused before
  stdout. RED/GREEN fixture the submitted-state predicate so crash mode accepts
  durable leased revision zero while normal mode still requires revision one;
  exact run cleanup preserved the foreign Docker project.
- [x] Corrected exact replay reached the daemon-then-actor SIGKILL boundary,
  then exposed an instantaneous actor-group absence check while a non-zombie
  member was still exiting. RED/GREEN require a bounded 200 x 50 ms wait for
  exact actor-group quiescence; the existing liveness helper continues to
  ignore only zombies and fails closed if a live member survives the bound.
- [x] Third exact replay passed finalized deployment, fresh actor onboarding,
  and Monero Regtest startup, then sampled the application replay while its
  second generation was still leased. The typed Blocked payload was retained,
  so RED/GREEN replace the immediate monitor read with a bounded state-aware
  wait for queued lease generation two, attempt two, and progress source
  generation two; daemon exit and timeout remain fail-closed.
- [x] Fourth exact replay passed the corrected handoff, finalized Tag16, one
  durable Monero refund submission, the post-send pause, and ordered daemon
  then actor SIGKILL. Restart transferred the lease without resending, but the
  actual observer repeatedly exhausted its parent budget before publishing the
  expected Pending projection. The nominal recovery loop also exceeded twenty
  minutes because every 50-millisecond poll rehashed the staged daemon binary.
  The interrupted run exited 130 through its normal trap; exact cleanup passed
  and did not target the foreign Docker project.
- [x] RED then GREEN at the actual sealed-observer process boundary: after
  validating the pinned genesis, a missing destination-wallet transaction
  returns non-authorizing Pending before daemon transaction, block, output, or
  stable-tip queries. An in-pool transfer first validates exact transaction,
  direction, destination, amount, and double-spend status, then returns the
  same Pending result. Finalized candidates retain the complete evidence path.
- [x] RED then GREEN the joined watchdog: prove the restarted daemon hash once,
  poll only the same PID/start-tick instance afterward, and enforce a true
  180-second `SECONDS` deadline instead of 3,600 variable-cost iterations.
  The 24-test adapter suite, 14-test sealed process suite, and focused runner
  contract are GREEN.
- [x] Add manual Flow 1ZJ and pin the actual-chain crash seam in the M5 service
  lifecycle policy gate.
- [x] Run the full pinned quality wrapper after the observer and watchdog
  fixes. Vulnerability/license policy, strict linting, CI/container policy,
  milestone contracts, certificate checks, documentation, and repository
  hygiene are GREEN.
- [x] Push verified observer/watchdog checkpoint `0619be1` to `origin/main`.
- [x] Fifth exact replay `m7refundkill-e2702ef-b` passed finalized deployment,
  actor onboarding, Regtest funding, finalized Tag16, one durable refund send,
  the feature-gated pause, and ordered daemon/actor SIGKILL. The restarted
  supervisor advanced through generation seven but retained revision zero and
  failed closed at the real 180-second Pending watchdog. Source status one and
  exact cleanup removed all run-owned resources without touching `wt-016w`.
- [x] RED then GREEN the composed artifact profile: require `lez-maker`,
  `lez-maker-daemon`, `lez-taker`, and `xmr-maker-actor` to be built and staged
  from Cargo release output. The exact crash-hook release build passes; the
  actor is 9,280,096 bytes instead of the 184,025,168-byte debug artifact while
  owner/mode/link, SHA-256, and sealed-memfd verification remain unchanged.
- [x] Sixth exact replay `m7refundkill-8399c00-c` proved all release artifacts,
  finalized deployment/onboarding/Tag16, one durable send, ordered daemon and
  actor SIGKILL, and abandoned-lease transfer through generation eleven, but
  every recovered effect still exited before revision one. Source status one
  and exact cleanup removed only run-owned resources; the foreign sentinel and
  clean worktree survived.
- [x] RED then GREEN the actual Monero wallet wire shape: an exact incoming
  mempool transfer is category `pool`, not `in`. Validate its transaction,
  destination, amount, and double-spend fields, then classify it only as
  non-authorizing Pending before every deep daemon/output/finality query.
  Confirmed `pool` can never finalize. The 25-test adapter and 15-test sealed
  process suites are GREEN.
- [x] Repeat the exact isolated two-devnet process-kill flow from clean pushed
  commit `f8bee63`. Run `m7refundkill-f8bee63-d` transferred abandoned
  generation four to generation six, published revision-one observe-only
  Pending before mining, reused the unchanged transaction without resending,
  then reached Refunded revision two after exactly ten confirmations.
- [x] Source status zero and exact cleanup passed; every run-owned container,
  volume, network, port, and sidecar disappeared while the foreign sentinel
  survived. The checked secret-free certificate is
  `docs/evidence/m7-actual-maker-refund-process-kill-f8bee63-20260808.json`;
  its RED/GREEN verifier is pinned into the quality runner and CI hardening
  policy.
- [ ] Continue with accepted-application concurrency, the other process-kill
  seams, fee stress, and reorg cases. This certificate closes only the joined
  Maker Monero refund crash boundary and does not close F3/F6/R2/R4 wholesale.

## M7 terminal custom-token refund checkpoint (2026-08-08)

- [x] RED then GREEN the schema-5 actor route for witnessed custom-token
  refunds while preserving the schema-4 native route and submit-once journal
  authority; 92 actor tests and strict Clippy pass.
- [x] Extend the two-direction actual-node runner and its source contract to
  admit only sequential custom-token refund journeys, require exactly one
  terminal refund effect per direction, bind the asset commitment, and prove
  terminal owner/custody balances.
- [x] Run the exact LEZ 0.2 verifier from clean commit `0b54ab68` in the
  deterministic `/tmp/lez-f7-artifact-src-0b54ab68` worktree. Root, guest,
  methods, recursive token/refund, deployer, Clippy, Rustdoc, dependency, and
  advisory-policy checks pass.
- [x] Independently retain guest SHA-256 `bc2ea18e...67fd7`, ProgramId
  `f3ead24b...c244e`, and the verified host deployer SHA-256
  `c594ea1e...f5cbd`. Rotate only the host-artifact pin under ADR 0179 because
  debug source-path metadata changed; the on-chain program identity did not.
- [x] Push the verified pin checkpoint `0078df9` to `origin/main`.
- [x] First fresh actual-node replay reached `BothLegsLocked` in both role
  stores, crossed the signed LEZ deadline, and then failed closed because the
  refund submitter retained the old pre-deadline lock-discovery window. Exact
  cleanup passed without targeting foreign resources.
- [x] RED then GREEN the refund baseline refresh: pin a one-block window at the
  fresh post-deadline finalized tip before the owner invocation, retain the
  durable submit-once journal, and require the complete post-baseline finalized
  window before projection. Record the flow and atomicity consequences in ADR
  0180; the focused orchestration contract passes.
- [x] Second fresh replay proved the baseline refresh active, then captured the
  remaining protocol mismatch directly: the sidecar returned definitive
  `Absent` for an exact-ID miss, which the bridge client and actor correctly
  rejected. Exact cleanup again passed without targeting foreign resources.
- [x] RED then GREEN exact-miss semantics under ADR 0181: the sidecar now
  returns `UnknownOrPending` with stable state/clock facts, while the owner
  avoids a moving-latest account read and reaches its durable CAS only from the
  configured exact baseline. All 93 actor tests, 31 sidecar library tests,
  strict actor/sidecar Clippy, and the orchestration contract pass.
- [x] Push exact-miss checkpoint `d8515ea` to `origin/main`.
- [x] Fresh exact run `m7f7refund-d8515ea-e` finalized one Maker-owned LEZ
  custom-token refund on attempt one and projected revision three to both role
  stores. It then failed closed because the post-projection guard still
  required native count three instead of the already-derived custom-token
  count four. No Bitcoin recovery attempt followed; exact cleanup passed and
  targeted no foreign resource. This is bounded RED evidence, not a completed
  swap.
- [x] RED then GREEN the post-projection effect invariant under ADR 0182: use
  asset-aware `expected_after` for both native and custom-token refunds and
  reject a return to the native literal in the orchestration contract.
- [x] Push asset-aware effect-count checkpoint `f279734` to `origin/main`.
- [x] Fresh exact run `m7f7refund-f279734-f` crossed the corrected count gate,
  finalized both ordered refunds in `taker_sells_foreign`, reached revision
  four in both role stores, and replayed with zero resubmission. The terminal
  balance wrapper then rejected canonical actor transition `maker_leg` because
  it expected the noncanonical alias `maker_refund`; every other exact actor
  evidence predicate passed individually. Balance sampling and the reverse
  direction did not follow. Exact cleanup passed without targeting a foreign
  resource, so this is bounded RED evidence rather than a certificate.
- [x] RED then GREEN the terminal-evidence vocabulary under ADR 0183: map the
  forward and reverse refund evidence to canonical `maker_leg` and `taker_leg`
  while retaining all exact asset, role, custody, transaction, and finality
  predicates.
- [ ] Push the canonical-evidence checkpoint, execute a fresh two-direction
  Bitcoin Regtest plus LEZ 0.2 custom-token refund journey, and retain a
  sanitized checked certificate with terminal replay and exact cleanup
  evidence.
- [ ] Close F7 only after the actual-node certificate, quality wrapper, manual
  flow, traceability, and hard-requirement inventory all pass from the same
  pushed commit.
