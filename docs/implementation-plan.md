# Living implementation plan

Last updated: 2026-07-11

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

## Current vertical slice

| Slice | RED evidence | GREEN evidence | Next refactor/gate |
|---|---|---|---|
| Taker-first happy path for BTC/XMR/ZEC | 2026-07-11 unresolved protocol API imports | 2026-07-11 `cargo test --workspace --all-targets`, 3 passed | Persist every fact and move from direct core calls to role harness |
| Ordered timeout refund | Same acceptance test RED | LEZ deadline 100, foreign deadline 120 passes; equal/reversed rejected | Use pair-specific block/time domains and safety margins |
| On-chain-only completion | Core API accepts only `ChainProof`/`ClaimEvidence` after lock | Happy path reaches `Completed` without a peer/transport handle | Prove through CLI/daemon black-box test with Delivery/Chat stopped |
| Restart recovery and user isolation | 2026-07-11 unresolved `SqliteSwapStore` and later missing `claim_evidence` | 2026-07-11 close/reopen after locks and witness reveal, plus two independent swaps, 2 passed | Encrypt secrets at rest; add process-kill/WAL matrix, migrations, and atomic outbox |
| Maker abandonment/refund observation order | 2026-07-11 missing `TakerLegRefunded` and no direct taker recovery | 2026-07-11 taker-only refund and foreign-first observation pass | Add model tests over all legal event orderings and chain reorgs |
| At-least-once chain observation replay | 2026-07-11 repeated confirmed lock failed with `InvalidPhase` | 2026-07-11 identical lock/claim events are idempotent; conflicting IDs/evidence are rejected | Extend to persisted outbox/event sequence numbers and refund transaction proofs |
| Generated transition sequences | 2026-07-11 property oracles exposed both confirmation growth and regression cases and were corrected | 512 arbitrary sequences include confirmation regression and explicit removal/replacement; retained minimized reorg seed preserves the discovered case | Add typed per-chain deadlines and compare against pair reference models |
| Maker operator process boundary | 2026-07-11 acceptance test could not resolve daemon/CLI executables | Actual CLI authenticates through HTTP metadata to actual daemon, creates a swap, daemon is killed/restarted on a new ephemeral port, persisted status remains visible | Move to owner-restricted Unix socket/credential file; add durable request IDs/audit outbox and price configuration |
| Bidirectional role ordering | 2026-07-11 reverse-direction test could not resolve direction or role-neutral transitions; CLI rejected `--direction` | Both directions preserve taker-first and maker-before-taker refunds for BTC/XMR/ZEC; actual CLI/daemon persists reverse direction across kill/restart | Replace normalized time with typed chain deadlines and run every real-chain role matrix in both directions |
| Taker-lock reorg/replacement | 2026-07-11 missing durable reorg phase/removal event; property oracle assumed confirmations only rise | Pre-maker regression/removal revokes permission and permits explicit replacement; post-maker removal pins the committed ID, suspends claims, and preserves refunds; generated model covers events | Add pair-specific reorg depth policies and real-node replacement cases |
| Typed refund clocks | 2026-07-11 tests could not resolve chain/basis/safety schedule types | Typed block-height/timestamp positions reject cross-domain comparison; role-chain mapping and conservative wall-clock margin pass in both directions | Replace coordinator/CLI normalized `Timelocks`, then calibrate named per-network parameters |

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
- [ ] Add pinned upstream LEZ reproducer tests, including mempool-vs-block timing
  and signature-byte preservation.
- [x] Complete the hard-requirement traceability matrix and enforce ID coverage in CI.

### Week 2 — protocol and threat design

- [ ] Publish per-leg message/state diagrams and atomicity arguments.
- [ ] Specify the LEZ escrow account model, native/custom token flows, claim and
  refund instructions, and SPEL IDL.
- [ ] Complete threat model: adaptor extraction, signature byte stability,
  timelocks/reorgs, XMR key-share recovery, ZEC transparent visibility, local RPC,
  persistence, and concurrency.
- [ ] Publish common SDK lifecycle traits plus typed pair-specific evidence/errors.
- [x] Add generated property tests for transition legality, conflicts, and
  absorbing terminal states.
- [ ] Extend the model with reorg/replacement events and typed per-chain deadlines.
- [x] Select and justify the Bitcoin refund construction (Taproot script-path CSV)
  with its M3 failure/fee/reorg validation matrix.

### Week 3 — integration contracts and review packet

- [x] Decide persistence direction: SQLite through `rusqlite`, behind a repository
  port and a single writer actor; validate with crash tests before freezing.
- [x] Decide Zcash direction: `zebrad` node, local `librustzcash` transaction
  construction, Zallet only where wallet RPC capabilities fit.
- [x] Specify maker daemon local JSON-RPC, authentication, systemd fallback, and
  Logos Core daemon-mode adapter contract.
- [ ] Fix per-pair confirmation/timelock parameters with reorg and latency margins.
- [ ] Review all ADRs, test evidence, open questions, and Milestone 2 entry gates.

Milestone 1 exits only when every unchecked deliverable above is completed and
reviewable. The first code slice does not by itself complete Milestone 1.

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
| LEZ proposal file paths drifted (`nssa` became `lee/state_machine`) | Source at pinned `dev` commit | Pin and automate semantic reproducers, not path assertions alone |
| Signature-byte stability is load-bearing for adaptor extraction | `Signature.value` is stored and `k256` verifies it directly; no normalizer found | Add a byte-preservation test through sequencer block inclusion |
| Validity windows are checked at block construction, not RPC admission | RPC pushes to mempool; state validation uses new block height/time | Add sequencer-level boundary reproducer and allocate inclusion slack |
| Zcash node migration is active | `zcashd` halts before NU6.3; Zallet omits raw-tx builder RPCs | Use Zebra plus local canonical Rust transaction construction |
| SPEL documentation targets older `nssa` paths | SPEL v0.5 docs and current LEZ `dev` disagree | Build a minimal generated program against one pinned compatibility set before escrow implementation |
| Upstream LEZ validity tests require RISC Zero Rust | Source guards and BIP-340 vectors pass; guest tests fail without `rzup install rust` | Provide an isolated pinned toolchain lane and keep full reproducer gate open |
| Timelocks currently share a normalized `u64` in the skeleton | Not sufficient for mixed height/timestamp chains | Introduce typed pair-specific deadlines before M2 implementation |
| Final E2E must represent actual users | Operator process harness exists; taker and chain lifecycles still call protocol core directly | Extend the role harness through taker CLI and real chain adapters before labeling tests as full E2E |
| Prototype local RPC still uses loopback HTTP and an environment capability | Tower rejects a Bearer header before JSON parsing and non-loopback binds are refused | Move to an owner-restricted Unix socket and credential file before M5 freeze |
| Daemon prototype serializes SQLite with a mutex on blocking workers | Safe for the two-method operator slice, not chain watcher concurrency | Introduce the ADR-0003 single writer actor and atomic outbox before mutations expand |
| Trade direction was unstated in both contractual sources | No one-way limitation exists; ordinary “between” swaps imply either asset can be sold | ADR 0008 supports both directions and makes direction immutable negotiated state |
