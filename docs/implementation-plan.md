# Living implementation plan

Last updated: 2026-07-12

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

## Test-first delivery ladder

Every slice follows RED, GREEN, REFACTOR and records the command/evidence below.

1. Protocol acceptance tests with deterministic fake chain evidence.
2. Persistence/restart and property tests over every transition/interleaving.
3. Role-oriented black-box tests invoking maker CLI, daemon RPC, and taker CLI.
4. Isolated Docker E2E with real regtest/stagenet-capable chain processes.
5. LEZ standalone-sequencer integration in CI.
6. Public testnet smoke suites, opt-in and credential-isolated.
7. Recorded happy, refund, and concurrency demos generated from passing suites.

Tests map to hard requirement IDs in `docs/requirements-traceability.md`. Custom
cryptography is prohibited: canonical libraries and published vectors are used,
with dependency license/advisory checks in CI.

The living [manual reproduction guide](manual-user-flows.md) records exact
fresh-checkout prerequisites, isolated commands, actor boundaries, expected
evidence, and cleanup for every currently proven operator/chain flow. It must be
updated with the implementation whenever one of those surfaces changes and
must continue to distinguish local fixtures from composed actor and public
testnet evidence. It and the global README also maintain the external-resource
inventory: public/local RPCs, faucets, registries, release assets, mutable
security databases, pins/checksums, availability risks, and fallback policy.

## Current vertical slice

| Slice | RED evidence | GREEN evidence | Next refactor/gate |
|---|---|---|---|
| Taker-first happy path for BTC/XMR/ZEC | 2026-07-11 unresolved protocol API imports | 2026-07-11 `cargo test --workspace --all-targets`, 3 passed | Persist every fact and move from direct core calls to role harness |
| Ordered timeout refund | Same acceptance test RED; later primary-source reconciliation exposed that generic maker/taker order contradicted ZEC's fixed chain order | BTC uses maker-funded then taker-funded recovery; ZEC uses LEZ then ZEC recovery in both directions; XMR remains event-gated | Exercise exact pair boundaries and fee/reorg stress on real nodes |
| On-chain-only completion | Core API accepts only `ChainProof`/`ClaimEvidence` after lock | Happy path reaches `Completed` without a peer/transport handle | Prove through CLI/daemon black-box test with Delivery/Chat stopped |
| Restart recovery and user isolation | 2026-07-11 unresolved `SqliteSwapStore` and later missing `claim_evidence` | 2026-07-11 close/reopen after locks and witness reveal, plus two independent swaps, 2 passed | Encrypt secrets at rest; add process-kill/WAL matrix, migrations, and atomic outbox |
| Maker abandonment/refund observation order | 2026-07-11 missing `TakerLegRefunded` and no direct taker recovery | 2026-07-11 taker-only refund and foreign-first observation pass | Add model tests over all legal event orderings and chain reorgs |
| At-least-once chain observation replay | 2026-07-11 repeated confirmed lock failed with `InvalidPhase` | 2026-07-11 identical lock/claim events are idempotent; conflicting IDs/evidence are rejected | Extend to persisted outbox/event sequence numbers and refund transaction proofs |
| Generated transition sequences | 2026-07-11 property oracles exposed both confirmation growth and regression cases and were corrected | 512 arbitrary sequences include confirmation regression and explicit removal/replacement; retained minimized reorg seed preserves the discovered case | Add pair-specific recovery triggers and compare against pair reference models |
| Maker operator process boundary | 2026-07-11 acceptance test could not resolve daemon/CLI executables | Actual CLI authenticates through HTTP metadata to actual daemon, creates a swap, daemon is killed/restarted on a new ephemeral port, persisted status remains visible | Move to owner-restricted Unix socket/credential file; add durable request IDs/audit outbox and price configuration |
| Bidirectional role ordering | 2026-07-11 reverse-direction test could not resolve direction or role-neutral transitions; CLI rejected `--direction` | BTC/ZEC both directions and XMR's LEZ-first direction preserve taker-first funding; claim order is now explicit per construction instead of fixed to maker-first | Run every supported real-chain role matrix and retain explicit XMR-first rejection |
| Taker-lock reorg/replacement | 2026-07-11 missing durable reorg phase/removal event; property oracle assumed confirmations only rise | Pre-maker regression/removal revokes permission and permits explicit replacement; post-maker removal pins the committed ID, suspends claims, and preserves refunds; generated model covers events | Add pair-specific reorg depth policies and real-node replacement cases |
| Typed recovery conditions | 2026-07-11 tests could not resolve chain/basis/safety schedule types | BTC/ZEC coordinator, persistence, RPC, and CLI use typed positions; wrong domains and insufficient conservative margins are rejected | Replace the prototype XMR deadline with the accepted canonical-LEZ-refund event trigger |
| Architecture diagrams | 2026-07-11 completeness guard failed on the first ADR without Mermaid; a 2026-07-12 audit then found that the renderer covered architecture/M1 blocks but omitted the manual-flow diagram | All 46 tracked Markdown Mermaid blocks pass the conservative GitHub-host policy and the exact Mermaid CLI 11.16.0 render gate; the independently hashed live GitHub Viewscreen asset also reported 11.16.0 on 2026-07-12 | Visually verify the rendered diagrams on GitHub after push because GitHub's live Mermaid version and host policy remain controlled externally |
| XMR funding-direction capability | 2026-07-11 source review found COMIT ships scriptable-chain-first only; test lacked `UnsupportedDirection` | Core schedule and actual CLI/daemon reject XMR-first; LEZ-first XMR remains supported and documented in the per-leg flow | Validate exact DLEQ/key-share recovery transcript against vectors and third-party review in M4 |
| XMR event-gated recovery | 2026-07-11 acceptance test could not resolve `RecoverySchedule`, event evidence, or recovery phases; prototype accepted a fake Monero deadline | Core/RPC/CLI use tagged deadline vs canonical-event terms; wrong-chain/low-confirmation evidence is rejected, confirmation regression revokes availability, restart preserves each phase, and real operator CLI creates LEZ-first XMR without maker-deadline flags | Replace the generic 32-byte recovery proof with exact COMIT DLEQ/key-share and Monero transaction evidence in M4 |
| Pinned LEZ execution semantics | Source inspection alone could not prove the mempool/block split or accepted transaction-byte preservation; an initial filtered native command falsely ran zero tests | A clean pinned checkout passes 14 validity cases, the full BIP-340 vector test, and exactly one run each of the repository-owned admission/block reproducer and upstream transaction-equality test | Keep the pinned lane required and use the scheduled current-`dev` lane only as forward-compatibility drift detection |
| RFP F4 ZEC chain ordering | 2026-07-11 M2 source reconciliation found M1's generic role-relative claim/refund prose allowed ZEC-before-LEZ in `TakerSellsLez` | RED required typed participants and chain-ordered bounds; GREEN adds 2 regressions and 23-test workspace pass: LEZ always reveals/refunds before later ZEC in both directions | Reprove all M1 gates, tag corrective commit `m1-complete.1`, then keep these vectors as M2 entry tests |
| CI security and quality gates | 2026-07-11 workflow audit found the advisory scope implicit and a malformed `rzup` install command in the scheduled LEZ lane | CI hard-fails advisories, bans, licenses, sources, Rust format/clippy/test/docs, ShellCheck, traceability, Mermaid, Docker isolation, and SHA-pinned Trivy high/critical scanning of the Zebra image | Run the exact local equivalents, then require every GitHub job before each milestone tag |
| ZEC full-lifecycle SDK boundary | 2026-07-12 `sdk_lifecycle` could not import discovery, negotiation, concrete agreement, active-swap, recovery-store, or secret types | Eight integrated cases prove independent role-fixed SDKs receive bounded untrusted wire, validate the same dual-signed agreement, persist exact accepted envelopes before activation, treat exact retry as idempotent and changed same-key input as conflict, reject adversarial role/revision/wire/swap-ID resume, and expose an `ActiveZecSwap` with no transport, chain, or store handles; full package has 72 passing tests plus the intentional Docker gate | Add typed LEZ/Zcash action ports, atomic transition persistence, both-direction happy/refund/restart/concurrency actors, then actual nodes |
| Crash-safe ZEC first-lock intent and projection | Initial active chain capabilities were inert; a naive RPC-success transition would lose unknown outcomes and combined LEZ initialize/fund into one unsafe effect | Taker staging/retry/projection plus maker-independent direction-selected observation commit before memory changes, probe unknown outcomes, reject corrupt primitive records, and replay from separate role-local stores; forward Zcash rejects primitive ID/depth assertions and durably journals complete canonical evidence, atomic same-tip replacement, same-inclusion depth change, and affirmative removal through `ZcashObservationTracker`; unchanged polls write nothing; the schema-v6 store folds history before append, rejects orphan/holey/duplicate/stale/poison histories, rolls back trigger failures, and catches stale instances up to the durable head; SDK package has 81 ordinary tests plus one doctest and store has 22 tests | Connect the canonical type to the production Zebra port; implement the distinct fresh pre-second-lock eligibility check, canonical LEZ evidence, then later effects |
| SQLite SDK-recovery schema | Schema v4 had no role-local accepted agreement, open/closed effect intent, separate active revision, or exact transition slot for the concrete SDK contract | Schema-v6 strict tables, primitive JSON DTOs, and cloneable role-fixed `SqliteZecRecoveryStore` preserve immutable acceptance separately from active revision, atomically close taker intent, and use a contiguous ordered maker journal; every load revalidates agreement, role, revision, bytes, evidence, history compatibility, and exact row range | Extend the proven adapter schema/transaction pattern to later effects; add process-kill-at-every-boundary and secret encryption in M5 without weakening the M2 first-lock guarantees |
| Zcash public-testnet route and funding research | M2 required self-hosted/public node, wallet, faucet, privacy, and flakiness guidance but no supported route was selected | Primary official sources select self-host Zebra 6.0.0 with loopback cookie RPC; document optional Zallet alpha.4 funding and faucet/Discord fallback; no official public Zebra RPC or Zallet arbitrary HTLC signer was found | Implement project-owned disposable transparent signing, rehearse the guide on a clean host, obtain TAZ, and retain live both-direction evidence; research alone does not satisfy U10 or M2 |
| LEZ v0.2 public-runtime security route | Fresh 2026-07-12 audit found the official v0.2.0 runtime graph still carries Hickory 0.25 and upstream explicitly ignores RUSTSEC-2026-0118/0119; SPEL PR #238 remains open and unreviewed despite green CI | Pin LEZ v0.2.0 at `a58fbce...` and SPEL PR head `df17acd...` only for engineering; port the guest and advisory-free `V03State` semantics first; build a thin official-type deployment/query client that excludes the Logos/libp2p/Hickory path; bind channel, ELF, ImageID, ProgramId, transaction, and block in evidence | Add RED lock-graph gates proving Hickory and LGPL exceptions are absent, port the guest/client, deploy and exercise actor calls, then keep public-profile enablement and the M2 tag fail-closed until SPEL is reviewed/merged or an explicit immutable-head security exception is recorded |
| Concrete LEZ/ZEC agreement | Initial generic LEZ terms did not bind exact chain identities, custody, transaction destinations/fees, funding inputs, wire bounds, or both actor signatures | Seventeen focused tests prove bounded exact decoding, dual low-S signatures, both directions, fail-closed public deployment, actual LEZ/ZEC deadlines, exact PDA/ATA derivation, accepted-at resume, redacted diagnostics, and agreement-derived funding/claim/refund requests; the exact dependency-light derivation source also matches pinned upstream v0.2 `lee_core`, SPEL multi-seed, and ATA-core types | Independently recompute deployed chain-derived terms in effect adapters, retain public deployment fail-closed behavior until reviewed, and compose the accepted agreement through real actors and nodes |
| Whole-system architecture and actor flows | 2026-07-11 ADR-local diagrams passed the old gate but did not provide one composed system, actor, trust-boundary, or lifecycle view | Canonical living architecture diagrams independent maker/taker actors, runtime components, node boundaries, happy/refund/restart flows, and current-versus-planned status; CI requires these views | Keep the status and flows synchronized whenever a slice crosses a new real process or chain boundary |
| Exact BIP-199 contract envelope | 2026-07-11 redeem API was absent; subsequent REDs exposed missing P2SH/scriptSig, refund policy, fetched-prevout validation, V5 epochs, UTXO ownership, dust/change, and Zebra acceptance | Exact script and V5 bytes/txids pass; deterministic selection and actor-only change use canonical builders; pinned Zebra confirms funding/claim/refund and rejects mutated funding/claim signatures plus pre-CLTV refund | Add replacement/reorg/refund-margin stress and composed LEZ↔ZEC roles |
| Arbitrary P2SH transaction signing | Stable source review shows ordinary and PCZT signers/finalizers recognize P2PKH/P2PK/multisig but not BIP-199; transparent builder also defaults every input to final sequence | GREEN uses canonical `TxOut`, `Bundle`, ZIP-244, deterministic secp256k1, and `TransactionData`; exact HTLC scriptSig is the only adapter-owned encoding, interpreter mutation tests pass, and Zebra is the final authority | Retain vectors and extend the node lane to replacement/reorg cases |
| Pinned SPEL/LEZ compatibility | Initial RED had no real fixture; generated client exposed missing signers; a later 11-test custody slice appeared green, but direct v0.1.2 source review invalidated its escrow-owned native users and direct token-holding PDA as real-user/RFP evidence | Native custody now composes canonical `authenticated_transfer`; custom custody is official `ATA(metadata, definition)` for two definitions; generated clients use real owner signers; exact Risc0 3.0.5 builds the checked guest and v0.1.2 standalone RPC includes its deployment in a persisted block after a mandatory-clock readiness block | Execute initialise/fund/claim/refund as actor transactions through standalone, record recursive cycles/segments, port the full lane to v0.2, then run testnet role evidence |
| Deployable LEZ guest supply chain | Host-only fixture could not produce an ELF; first real build selected Rust-1.89-only enum crates in the Rust-1.88 builder; first runtime RED admitted deployment but stopped at genesis because `RISC0_DEV_MODE` does not provide `r0vm`; audit then rejected vulnerable `ruint 1.17.0` | Exact `cargo-risczero`/`r0vm 3.0.5`, pinned builder digest, enum 4.3.0 compatibility pin, fixed `ruint 1.17.1`, checked ELF SHA-256/ImageID, port-zero/temp-state service, clock readiness block, RPC deployment, canonical block inclusion, and three locked-graph dependency audits are reproducible in CI | Extend the deployment harness to actor-signed lifecycle calls and cost evidence; keep the artifact identity and advisory non-exposure checks fail-closed |
| Isolated Zebra consensus E2E | First runner RED found RPC cookie assumptions; the initial capability hardening found an unwritable cache; strict Trivy then rejected both official 5.1.1 and 5.2.0 runtimes with 40 HIGH/2 CRITICAL findings | Two disconnected copies of the official 5.2.0 binary now run in immutable distroless nonroot images that scan at 0 HIGH/CRITICAL; read-only/capability-free NU6.2 Regtest on separate ephemeral ports proves funding, claim, refund, rejection, concurrency, and a three-block detach onto a conflicting four-block branch; exact project cleanup leaves unrelated Docker workloads untouched | Add typed chain evidence, composed refund-margin enforcement, and public-testnet smoke; repeat final-image scan immediately before evidence/tag |

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
| M3 | BTC Schnorr adaptor/Taproot end to end | DLC vectors and refund construction approved |
| M4 | XMR Ed25519 adaptor/cross-curve DLEQ end to end | COMIT vectors and key-share recovery design approved |
| M5 | Persistent coordinator, daemon, CLIs, price plugins, fuzzing | At least one real pair adapter stable; RPC/persistence ADRs accepted |
| M6 | Maker/taker Basecamp mini-apps | Daemon RPC stable and role E2E reusable |
| M7 | Third-party reviews, remediation, readiness packet | All hard-requirement tests and demos green |

M2 and M3 may overlap after M1. M4 follows M3 for cryptography-lead capacity.
M5/M6 may overlap the tail of M4. M7 follows all implementation milestones.

## Milestone 2 plan: transparent ZEC end to end

### Accepted proposal authority and delivery boundary

The contractual authority is accepted Gateway proposal
[issue #112](https://github.com/logos-co/rfp/issues/112), not superseded
issue #61. The authority was re-fetched on 2026-07-12: issue body SHA-256
`49356263a762307abc0f8dd2863ac5af8fe13d9b17b674f242d025de655f1c87`;
canonical comments JSON SHA-256
`3c596392f7356a29a2d512ffa92ebb9153cab7b97e38848b61e79e4764240980`. The
local `proposal.gateway.md` snapshot is archive-only because it excludes ZEC and
calls ETH M2.

The six accepted M2 outputs are the payment/tag boundary:

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
- [ ] Document transparent-pool visibility and the shield-after-swap user journey.
- [ ] Record happy, refund/timeout, and concurrent-swap demos from passing
  testnet actor suites.

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
- [ ] Port and rebuild SPEL, guest, generated client, and PDA derivations for LEZ
  v0.2.0 before live-testnet evidence. v0.1.2 `/NSSA/` and v0.2.0 `/LEE/` PDA
  domains are incompatible. Official LEZ v0.2.0 and its public testnet are live,
  but SPEL PR #238 head `df17acd98436be4f09c55877dae1fe2e73cbcdca`
  remains open/unmerged with no submitted maintainer review. A provisional port
  may use that exact head for engineering evidence; the final M2 tag requires a
  merged/tagged release or an explicit reviewed exception. Open issues #242 and
  #243 also require fail-closed PDA/program-ID handling.
- [x] Pin a provisional LEZ v0.2 compile-only compatibility lane to official
  tag `v0.2.0` and SPEL PR #238 head
  `df17acd98436be4f09c55877dae1fe2e73cbcdca`. CI proves the LEE
  configuration seam, a single `lee_core` type identity, and the `/LEE/` PDA
  vector without polling the constructed sequencer future. The exact upstream
  graph contains Hickory advisories RUSTSEC-2026-0118/-0119, so narrow,
  feature-guarded exceptions apply only to this hash-locked compile fixture.
  This graph is prohibited for runtime or testnet use until upstream removes
  the vulnerable dependency path or a separate security review approves a
  safe replacement.
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
  retained through schema v6). Conflict warnings and
  terminal critical alerts commit with event+aggregate; Applied emits none;
  forced insertion failure rolls back; replay/restart preserve one cursor and
  acknowledgment without changing protocol state.
- [x] Expose owner-authenticated alert status/list/ack through actual daemon and
  CLI processes. Wrong credentials fail before RPC parsing; restart preserves
  attention; acknowledgment retains evidence and reorg/terminal protocol phase.
- [x] Prove the actual-node persistence boundary: canonical funding, immutable
  binding, close/reopen, unchanged fresh-query suppression, affirmative
  two-Zebra fork removal, second close/reopen, and exact retry pass through the
  maker runtime on schema v6. Daemon-integrated polling remains open.
- [x] Establish the concrete ZEC SDK pre-lock/activation/resume boundary:
  role-fixed async discovery and negotiation treat bytes as untrusted; separate
  role stores persist exact accepted envelopes before activation; exact replay
  is idempotent and changed same-key input conflicts; adversarial resume
  revalidates wire, role, revision, commitment, and swap ID; the post-lock
  `ActiveZecSwap` exposes no discovery, negotiation, raw chain, or store handles;
  claim material and diagnostics are redacted and secrets zeroize.
- [x] Implement the concrete canonical LEZ/ZEC agreement validator and bounded
  wire record. Both actors sign the same profile, roles, digest, LEZ
  environment/genesis/program/asset/amount/custody, BIP-199 binding, exact
  Zcash destinations/fees/input-set commitment/expiry, refund anchors/bounds,
  and negotiation transcript. Negotiation, activation, and resume now use this
  concrete record, though that boundary alone does not satisfy the full SDK
  lifecycle without typed effects and actor execution.
  Public-testnet validation remains fail-closed until a reviewed immutable LEZ
  deployment exists. The provisional lane now compiles the exact
  dependency-light SDK derivation source and proves metadata, native multi-seed
  custody, and ATA bytes against pinned upstream v0.2 `lee_core`, SPEL, and
  ATA-core types; deployed adapters must still recompute selected identities.
- [ ] Complete typed LEZ and Zcash actions plus atomic active-swap transition
  persistence for escrow creation, both claims, and both refunds. The first-lock
  contract now stages exact bytes before effects, observes before
  rebroadcast, separates LEZ initialize/fund, and atomically projects/replays
  confirmed taker evidence through the production role-fixed SQLite adapter.
  Maker-independent observation now selects only the agreement-derived maker
  node route, commits without taker intent, survives SQLite restart, and remains
  non-authorizing: SDK next action is `Wait`. Forward Zcash now requires and
  persists complete canonical evidence. Its ordered schema-v6 journal now
  commits/replays depth changes, atomic replacements, and affirmative removals
  across restarts and rejects discontinuous or history-incompatible rows.
  The production RPC port, canonical LEZ adapter, distinct fresh
  pre-second-lock check, second lock, claims, and refunds are remaining work,
  so this item is still open.
- [ ] Compose cross-chain refund-margin cases through actual LEZ and Zebra
  nodes. The LEZ
  Unix-millisecond/core Unix-second boundary is typed, checked, conservatively
  rounded, and boundary-tested; the composed flow remains.
- [ ] Calibrate and publish the public-testnet ZEC refund margin against a stated
  worst-case confirmation-latency and operator-reaction envelope. Nominal block
  cadence or local Regtest timing alone does not satisfy RFP F4.
- [ ] Re-audit the stable Zebra/security pin immediately before public-testnet
  evidence because the current release horizon ends ahead of NU7.
- [ ] Re-audit the deployed SPEL/LEZ guest graph before testnet evidence and the
  M2 tag. Under ADR 0018, exact Logos-owned advisories and upstream review
  status are disclosed in the production-blocker register and do not block M2;
  floating pins, undisclosed exceptions, or repository-controlled adapter
  defects still block it.

### Reference-actor delivery and M2 exit

- [x] Publish a living manual reproduction guide for the currently proven maker
  operator, Zebra actor/fork, and LEZ native/token/cost fixtures, with exact
  no-clash and cleanup rules. This does not satisfy the independent composed
  maker/taker or public-testnet items below.
- [ ] Run independent SDK reference maker and taker processes with direction-correct
  keys, transparent funds, LEZ funds, selected node routes, and durable recovery state
  for both supported ZEC directions.
- [ ] Pass happy, abandonment/refund, restart, and concurrent swap suites through
  the public SDK and actual chain boundaries; destroy the test-only pre-lock
  mailbox after terms persist. Production daemon/CLI and Logos Delivery/Chat
  integration remain M5 deliverables and are not relabeled as M2.
- [ ] Publish self-hosted and public Zebra connection/funding guides, transparent
  privacy warnings, and the shield-after-swap journey.
- [ ] Generate happy/refund/concurrency recordings only from passing actor suites.
- [ ] Re-run formatting, strict Clippy, all tests/docs, ShellCheck, traceability,
  Mermaid, advisories, bans, licenses, sources, isolated E2E, and testnet smoke.
- [ ] Mark `m2-complete` only on the exact pushed commit whose evidence proves
  every repository-controlled item above and links the freshly rechecked Logos
  production-blocker register. Logos-owned release blockers follow ADR 0018 and
  are final-production gates rather than M2 stops.

## Docker isolation policy

Docker suites must:

- set `COMPOSE_PROJECT_NAME=lez-atomic-swaps-${RUN_ID}`;
- avoid fixed `container_name` values and use ephemeral published ports;
- create only project-scoped networks and volumes with identifying labels;
- place per-run data under `.e2e/${RUN_ID}`;
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
| SPEL documentation targets older `nssa` paths | SPEL v0.5 docs and current LEZ `dev` disagree | Build a minimal generated program against one pinned compatibility set before escrow implementation |
| Pinned SPEL guest cannot run on current LEZ testnet | v0.1.2 uses NSSA ABI and `/NSSA/` PDA domain; v0.2.0 uses LEE ABI and `/LEE/`; upstream issues #234/#237 record live signature rejection | Prove locally on exact v0.1.2 standalone, then rebuild the full guest/client/PDA lane on a reviewed v0.2 SPEL release or exact approved successor to PR #238 |
| Provisional LEZ v0.2 compile graph contains upstream Hickory advisories | The hash-locked fixture is compile-only and non-polled, DNSSEC features are absent, and `cargo-deny` permits only two narrow advisory exceptions | Prohibit this graph from runtime/testnet use; require a safe upstream dependency path, DNS removal, or an explicit security review before an executable lane |
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
