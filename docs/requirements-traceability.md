# Requirements traceability

Last reconciled: 2026-07-12 against the live RFP-003 and Gateway's accepted
replacement proposal #112. Issue #61 is superseded. IDs below follow the RFP's
Functionality, Usability, Reliability, Performance, Supportability, and Demo
ordering. A row is not acceptance: `Passing` requires the named evidence at the
real actor and chain boundary described by the requirement.

The executable guard `scripts/check-requirements-traceability.sh` requires every
hard-requirement ID to occur exactly once. RFP Supportability 4 ultimately
requires a corresponding test—not merely a row—for every F, U, R, and P item.

## Functionality

| ID | Contract | Acceptance evidence | Status / milestone |
|---|---|---|---|
| F1 | No central server; Delivery advertisements and Chat coordination | Role E2E discovers and negotiates through real Delivery/Chat adapters; central services absent; UJ-006 repeats with both stopped after lock | Planned M5 |
| F2 | LEZ–BTC via BIP-340 adaptor signatures and Taproot key-path cooperative claim | DLC vector suite; both-direction Bitcoin Core testnet/regtest happy/refund/concurrency role E2E; cooperative claim decoded as key-path spend | Planned M3 |
| F3 | LEZ–XMR via Ed25519 adaptor signatures, cross-curve DLEQ, and spend-key share | LEZ-first COMIT/DLEQ vectors; `monerod`/wallet RPC stagenet happy/refund/concurrency role E2E; spend-key recovery fault case; XMR-first rejected | Design constrained by primary reference; implementation M4 |
| F4 | LEZ-transparent-ZEC via BIP-199 HTLC; LEZ claim/refund precedes ZEC and the ZEC refund strictly outlives LEZ by the documented margin | Exact vectors, profiles, agreement validation, taker recovery, canonical maker observation, and reorg-safe eligibility pass. The maker consumes fresh eligibility internally, persists the signed-direction opposite-chain plan, submits ordered LEZ initialize/fund or Zcash fund, atomically projects confirmed evidence, and replays at `BothLegsLocked` in both directions through schema-v7 SQLite. Actual Zebra fork/restart and local LEZ custody lifecycles pass separately. | Lock happy path and durable replay implemented; official LEZ/Zebra action adapters, maker hardening, claims/refunds, cross-chain margin composition, independent actors, and public-testnet evidence remain M2 work |
| F5 | Risc0 LEZ escrow validates pair proof and supports claim/refund | Per-pair guest instruction tests plus standalone-sequencer initialise/claim/refund E2E | Source-correct instruction semantics, checked deployment, real-key native plus two-definition token claim/refund lifecycles, and recursive cost gates pass; public testnet remains M2 |
| F6 | Atomic outcome: both claim or both refund | Six scenario tests and 512 generated transition sequences pass; terminal ZEC removal/replacement is journaled and explicitly classified without erasing `Completed`/`Refunded`; its critical alert commits atomically and survives replay/restart; authenticated operator surface, pair-specific real-chain reorg tests, and the abandonment matrix remain | Partial |
| F7 | Native and custom LEZ tokens through ATAs | Parameterized guest and sequencer E2E for native token plus two independent token mints/accounts | Canonical authenticated-transfer native lifecycle and official ATA claim/refund lifecycles for two independent definitions pass with real owner keys, definition-bound holdings, supply conservation, and substitution negatives |
| F8 | Pluggable local and Logos-module C-API price sources | Price-port contract suite; config/CLI mutation E2E; fake and real C-ABI adapter tests including stale/unavailable feeds | Planned M5 |
| F9 | Headless maker covers configuration, pricing, advertisement, execution, monitoring, and full CLI operation | UJ-007 actual CLI/daemon suite plus restart, history, manual claim/refund, pricing, and advertisement cases | Partial: authenticated create/status and durable alert status/list/ack survive process restart; execution/pricing/advertisement remain |

## Usability

| ID | Contract | Acceptance evidence | Status / milestone |
|---|---|---|---|
| U1 | Dedicated full-lifecycle SDK for each pair | Shared SDK contract suite instantiated for BTC/XMR/ZEC; public API doctests cover discovery through refund | Partial: ZEC role-fixed discovery, bounded concrete negotiation, persistence-before-activation, and adversarial resume are integrated; typed active chain actions, production adapters, and actor E2E remain M2 |
| U2 | Long-running autonomous maker daemon plus systemd unit/install guide | Packaged daemon runs under hardened systemd in an isolated VM, restarts, advertises, prices, and executes without GUI | Partial daemon seam; packaging M5 |
| U3 | Maker CLI configures pairs/prices, controls daemon, queries history, and triggers claim/refund over IPC/RPC | Black-box CLI-to-daemon command matrix under owner/wrong-owner roles | Partial: create/status/auth/restart pass |
| U4 | Taker CLI covers discovery, initiation, monitoring, claim, and refund | Actual taker CLI drives each pair's happy and abandoned-counterparty journey | Planned M5 |
| U5 | Basecamp maker mini-app configures and monitors | Playwright actor E2E against the same daemon API; clean local build/load from documented repo | Planned M6 |
| U6 | Basecamp taker mini-app browses and executes swaps | Playwright actor E2E for each pair including refund and ZEC shield-after-swap guidance | Planned M6 |
| U7 | SPEL IDL for LEZ escrow program(s) | IDL generation/validation test against pinned compatible SPEL/LEZ versions | Generated IDL/client and signer roles pass; custody ABI is being regenerated around authenticated transfer and official ATAs |
| U8 | Bitcoin Core testnet setup guide, self-hosted and public | Fresh-machine documentation test reaches funded wallet and SDK connectivity for both routes | Planned M3 |
| U9 | Monero stagenet and wallet-RPC setup guide, self-hosted and public | Fresh-machine documentation test reaches funded wallet and SDK connectivity for both routes | Planned M4 |
| U10 | Zebra/Zcash testnet transparent-wallet guide, self-hosted and public | Fresh-machine documentation test reaches funded transparent wallet and SDK connectivity for both routes | Partial M2: primary-source guide selects self-host Zebra 6.0.0, optional Zallet funding, and faucet/Discord fallback; no official public Zebra RPC or project HTLC signer exists, and no clean-machine funded rehearsal has passed |

## Reliability

| ID | Contract | Acceptance evidence | Status / milestone |
|---|---|---|---|
| R1 | Taker locks first; maker waits for required confirmations | Core rejects early lock and uses independent immutable policies for each funded leg; reverse ZEC proves maker-funded ZEC below-threshold accumulation, promotion, 10→9-style depth regression suspension, exact depth recovery, removal pinning, conflict rejection, and preserved refunds; watcher tests require stable affirmative evidence; runtime requires both role policies to match the immutable named profile and commits conflicting chain replacements without substituting the protocol-pinned ID; composed watcher evidence remains | Passing core and ZEC reconciliation/persistence boundaries; composed adapter evidence M2–M4 |
| R2 | After first submission, only local state and chain nodes are required | Core happy/refund paths have no peer handle; UJ-006 kills Delivery/Chat and counterparty after each durable transition | Partial |
| R3 | Missing chain dependency does not disable other pairs | Dependency matrix starts daemon with each node absent/unhealthy and completes unaffected-pair swaps with clear CLI/GUI status | Planned M5 |
| R4 | Persisted state survives crash/restart without fund loss | Schema-v7 SQLite retains agreement, canonical history, separate taker/maker intents, exact transitions, revision, bindings, and alerts. Fourteen SDK-recovery tests cover rollback, torn/corrupt/future rows, stale catch-up, mixed revisions, exact replay, and both-direction unknown-submission reopen without rebroadcast; union replay rejects holes/duplicates. Actual daemon/two-Zebra restart also passes. Actual-node maker transport/reorg faults, later-effect kill points, encryption, and the M5 outbox remain. | Partial |
| R5 | Concurrent swaps have independent state, escrow, and deadlines | Two-store isolation test passes; multi-user multi-pair process/chain concurrency and fault injection remain | Partial |
| R6 | Timelocks cover variance, congestion, and clock drift per chain | Typed pair-specific domains and ordering pass; LEZ seconds/milliseconds conversion is explicit, conservatively rounded, overflow-checked, and boundary-tested; named runtime profiles and public calibration remain | Partial: typed enforcement passes; M2 profile/node composition remains |
| R7 | Bitcoin refund construction is chosen and justified with failure analysis | ADR 0009 selects script-path CSV; Bitcoin Core tests cover exact boundary, key, reorg, current fees, and RBF/CPFP | Design accepted; executable evidence M3 |
| R8 | Delivery/Chat outage is retried/buffered/degraded and documented | Pre-lock outage/recovery matrix plus post-lock UJ-006 proves on-chain independence and user-visible degraded state | Planned M5 |

## Performance

| ID | Contract | Acceptance evidence | Status / milestone |
|---|---|---|---|
| P1 | Compute units documented for initialise, claim, and refund against a named LEZ testnet version | Reproducible benchmark records each operation/pair and fails CI thresholds for the pinned release | Partial: exact v0.1.2 native/authenticated-transfer and token/ATA/Token cycles, segments, invariants, and CI budgets are recorded; named public-testnet rerun remains M2 |

## Supportability

| ID | Contract | Acceptance evidence | Status / milestone |
|---|---|---|---|
| S1 | LEZ escrow deployed/tested on testnet 0.2 | Version-pinned deployment manifest and public smoke-test transaction evidence | Base ZEC-capable v1 deployment planned M2; BTC/XMR guest updates follow in M3–M4 |
| S2 | Standalone LEZ sequencer E2E is included in CI | Isolated CI job boots sequencer on ephemeral resources and runs guest lifecycle suites | Passing locally: isolated CI builds/checks/deploys the guest, runs native and two-definition token real-role happy/refund/negative lifecycles through canonical blocks, and gates deterministic recursive costs for both custody paths |
| S3 | Default-branch CI is green | Required checks run format, strict Clippy, workspace tests/docs, traceability, dependencies, strict final-image vulnerability scanning, and isolated E2E | Local workflow/gates green; remote branch pending |
| S4 | Every F/U/R/P hard requirement has a corresponding test | Traceability completeness guard plus test-report manifest rejects missing or skipped requirement IDs | Matrix guard passing; acceptance tests incomplete |
| S5 | Complete reference integration for every chain | UJ-001/UJ-002/UJ-004 role E2E and runnable reference package for BTC, XMR, and ZEC | Planned M2–M5 |
| S6 | README covers deployment, addresses, prerequisites, and maker/taker CLI/mini-app use | Fresh-machine documentation tests plus link/command validation | Planned incrementally through M6 |
| S7 | Write-up covers protocols, escrow, atomicity, timelocks, assumptions, limitations | M1 design packet reviewed, then updated from executable evidence and audit findings | In progress M1; final M7 |
| S8 | Every pair SDK public API has docs/errors/examples for full lifecycle | `cargo doc -D warnings`, doctests, and public-API coverage check for all three SDKs | Planned M2–M4 |
| S9 | Logos doc packet for each pair SDK | Template validation and reviewer acceptance for BTC, XMR, and ZEC packets | Planned M7 |
| S10 | Separate maker-CLI and taker-CLI doc packets | Template validation plus clean operator/user journey rehearsals | Planned M7 |
| S11 | Figma designs or equivalent for both mini-apps | Signed-off clickable HTML prototypes cover maker and taker role journeys | Planned M6 |
| S12 | Third-party review of all on-chain programs/scripts and remediation | Agreed reputable reviewer report covers LEZ, BTC, ZEC, and other locking logic; findings tracked to closure | Planned M7 |
| S13 | Third-party review of protocol implementation and remediation | Reviewer report covers atomicity, taker-first, timelocks, adaptor/HTLC constructions, with findings tracked | Planned M7 |

## Demos

| ID | Contract | Acceptance evidence | Status / milestone |
|---|---|---|---|
| D1 | Happy, abandonment/refund, and concurrent-swap recording for each pair | Nine recordings generated from passing role E2E runs, with commit/testnet/version metadata | Planned M2–M4; regenerated at M7 |
