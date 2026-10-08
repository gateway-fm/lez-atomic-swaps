# Requirements traceability

Last reconciled: 2026-07-11 against the live RFP-003 and Gateway's accepted
replacement proposal #112. Rows U1, U8, S8, and D1 were reconciled again on
2026-10-08 for the milestone-3 acceptance items; the remaining rows still carry
their 2026-07-11 status. Issue #61 is superseded. IDs below follow the RFP's
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
| F2 | LEZ–BTC via BIP-340 adaptor signatures and Taproot key-path cooperative claim | BIP-340/BIP-327 and exact-pinned swap-specific adaptor replacement suite; [GW-M3-001 test-reference correction accepted](https://github.com/logos-co/rfp/issues/123#issuecomment-5781178140) (not M3 or security approval); both-direction Bitcoin Core testnet/regtest happy/refund/concurrency role E2E; cooperative claim decoded as key-path spend | Planned M3 |
| F3 | LEZ–XMR via Ed25519 adaptor signatures, cross-curve DLEQ, and spend-key share | LEZ-first COMIT/DLEQ vectors; `monerod`/wallet RPC stagenet happy/refund/concurrency role E2E; spend-key recovery fault case; XMR-first rejected | Design constrained by primary reference; implementation M4 |
| F4 | LEZ–transparent-ZEC via BIP-199 HTLC; LEZ claim/refund precedes ZEC and the ZEC refund strictly outlives LEZ by the documented margin | Direction-aware LEZ-recipient/preimage tests pass; BIP-199 vectors and both-direction Zebra testnet happy/refund/concurrency role E2E remain | Core contract corrected; chain implementation M2 |
| F5 | Risc0 LEZ escrow validates pair proof and supports claim/refund | Per-pair guest instruction tests plus standalone-sequencer initialise/claim/refund E2E | Design in M1; implementation M2–M4 |
| F6 | Atomic outcome: both claim or both refund | Six scenario tests and 512 generated transition sequences; pair-specific model/reorg tests and real-chain abandonment matrix remain | Partial |
| F7 | Native and custom LEZ tokens through ATAs | Parameterized guest and sequencer E2E for native token plus two independent token mints/accounts | Planned M2–M4 |
| F8 | Pluggable local and Logos-module C-API price sources | Price-port contract suite; config/CLI mutation E2E; fake and real C-ABI adapter tests including stale/unavailable feeds | Planned M5 |
| F9 | Headless maker covers configuration, pricing, advertisement, execution, monitoring, and full CLI operation | UJ-007 actual CLI/daemon suite plus restart, history, manual claim/refund, pricing, and advertisement cases | Partial: authenticated create/status and process-kill recovery pass |

## Usability

| ID | Contract | Acceptance evidence | Status / milestone |
|---|---|---|---|
| U1 | Dedicated full-lifecycle SDK for each pair | BTC: [`lez-btc-swap-sdk` with `lez-swap-sdk-core`](sdk-lez-btc.md); [`examples/full-lifecycle.rs`](../crates/btc-swap-sdk/examples/full-lifecycle.rs) walks offer discovery, negotiation, escrow creation, claim, and refund, and the named `sdk_facade.rs` tests cover each stage in both directions | BTC done at v0.2.6; XMR/ZEC planned M2–M4 |
| U2 | Long-running autonomous maker daemon plus systemd unit/install guide | Packaged daemon runs under hardened systemd in an isolated VM, restarts, advertises, prices, and executes without GUI | Partial daemon seam; packaging M5 |
| U3 | Maker CLI configures pairs/prices, controls daemon, queries history, and triggers claim/refund over IPC/RPC | Black-box CLI-to-daemon command matrix under owner/wrong-owner roles | Partial: create/status/auth/restart pass |
| U4 | Taker CLI covers discovery, initiation, monitoring, claim, and refund | Actual taker CLI drives each pair's happy and abandoned-counterparty journey | Planned M5 |
| U5 | Basecamp maker mini-app configures and monitors | Playwright actor E2E against the same daemon API; clean local build/load from documented repo | Planned M6 |
| U6 | Basecamp taker mini-app browses and executes swaps | Playwright actor E2E for each pair including refund and ZEC shield-after-swap guidance | BTC↔LEZ delivered in M6; the other pairs and the ZEC guidance are not ([reconciliation](m6-zec-reconciliation.md)) |
| U7 | SPEL IDL for LEZ escrow program(s) | IDL generation/validation test against pinned compatible SPEL/LEZ versions | Sketch in M1; executable M2 |
| U8 | Bitcoin Core testnet setup guide, self-hosted and public | [Guide](bitcoin-testnet4-setup.md) covers Route A self-hosted Testnet4, Route B exact-HTTPS gateway, and Route C keyless public provider on Testnet3, each with wallet creation, funding, and the swap directions it can serve; recorded runs in [testnet4](evidence/testnet4-20260919/README.md) and [testnet3 provider](evidence/testnet3-provider-20260918/README.md) | Done at v0.2.6 |
| U9 | Monero stagenet and wallet-RPC setup guide, self-hosted and public | Fresh-machine documentation test reaches funded wallet and SDK connectivity for both routes | Planned M4 |
| U10 | Zebra/Zcash testnet transparent-wallet guide, self-hosted and public | Fresh-machine documentation test reaches funded transparent wallet and SDK connectivity for both routes | Planned M2 |

## Reliability

| ID | Contract | Acceptance evidence | Status / milestone |
|---|---|---|---|
| R1 | Taker locks first; maker waits for required confirmations | Core rejects early lock and revokes permission on confirmation regression for all pairs; real chain adapters repeat around reorg boundaries | Passing core; adapter evidence M2–M4 |
| R2 | After first submission, only local state and chain nodes are required | Core happy/refund paths have no peer handle; UJ-006 kills Delivery/Chat and counterparty after each durable transition | Partial |
| R3 | Missing chain dependency does not disable other pairs | Dependency matrix starts daemon with each node absent/unhealthy and completes unaffected-pair swaps with clear CLI/GUI status | Planned M5 |
| R4 | Persisted state survives crash/restart without fund loss | SQLite reopen tests plus actual daemon kill/restart pass; kill-at-every-transition, corruption, migration, encryption, and outbox matrix remain | Partial |
| R5 | Concurrent swaps have independent state, escrow, and deadlines | Two-store isolation test passes; multi-user multi-pair process/chain concurrency and fault injection remain | Partial |
| R6 | Timelocks cover variance, congestion, and clock drift per chain | Parameter ADR plus boundary/property tests against pair-specific height/time domains and named network assumptions | In progress M1 |
| R7 | Bitcoin refund construction is chosen and justified with failure analysis | ADR 0009 selects script-path CSV; Bitcoin Core tests cover exact boundary, key, reorg, current fees, and RBF/CPFP | Design accepted; executable evidence M3 |
| R8 | Delivery/Chat outage is retried/buffered/degraded and documented | Pre-lock outage/recovery matrix plus post-lock UJ-006 proves on-chain independence and user-visible degraded state | Planned M5 |

## Performance

| ID | Contract | Acceptance evidence | Status / milestone |
|---|---|---|---|
| P1 | Compute units documented for initialise, claim, and refund against a named LEZ testnet version | Executor cycles for the BTC pair's `InitializeNativeWitnessed`/`FundNative`/`ClaimNativeWitnessed`/`RefundNative` measured against the deployed v0.2.4 escrow (ImageID `c22d61fc…`) in `escrow_cycles.rs`, which fails if the binary drifts; recorded in [docs/lez-compute-units.md](lez-compute-units.md) | Done for the BTC pair; XMR/ZEC variants M4 |

## Supportability

| ID | Contract | Acceptance evidence | Status / milestone |
|---|---|---|---|
| S1 | LEZ escrow deployed/tested on testnet 0.2 | Version-pinned deployment manifest and public smoke-test transaction evidence | Planned M2–M4 |
| S2 | Standalone LEZ sequencer E2E is included in CI | Isolated CI job boots sequencer on ephemeral resources and runs guest lifecycle suites | Planned; primitive compatibility lane exists |
| S3 | Default-branch CI is green | Required checks run format, strict Clippy, workspace tests, traceability, dependencies, and isolated E2E | Local workflow/gates green; remote branch pending |
| S4 | Every F/U/R/P hard requirement has a corresponding test | Traceability completeness guard plus test-report manifest rejects missing or skipped requirement IDs | Matrix guard passing; acceptance tests incomplete |
| S5 | Complete reference integration for every chain | UJ-001/UJ-002/UJ-004 role E2E and runnable reference package for BTC, XMR, and ZEC | Planned M2–M5 |
| S6 | README covers deployment, addresses, prerequisites, and maker/taker CLI/mini-app use | Fresh-machine documentation tests plus link/command validation | Planned incrementally through M6 |
| S7 | Write-up covers protocols, escrow, atomicity, timelocks, assumptions, limitations | M1 design packet reviewed, then updated from executable evidence and audit findings | In progress M1; final M7 |
| S8 | Every pair SDK public API has docs/errors/examples for full lifecycle | BTC: [SDK map](sdk-lez-btc.md), [crate README](../crates/btc-swap-sdk/README.md), and a runnable full-lifecycle example; `cargo doc` stays warning-free and the example compiles under `--all-targets` with pedantic Clippy denied in CI | BTC done at v0.2.6; XMR/ZEC planned M2–M4 |
| S9 | Logos doc packet for each pair SDK | Template validation and reviewer acceptance for BTC, XMR, and ZEC packets | Planned M7 |
| S10 | Separate maker-CLI and taker-CLI doc packets | Template validation plus clean operator/user journey rehearsals | Planned M7 |
| S11 | Figma designs or equivalent for both mini-apps | Signed-off clickable HTML prototypes cover maker and taker role journeys | Planned M6 |
| S12 | Third-party review of all on-chain programs/scripts and remediation | Agreed reputable reviewer report covers LEZ, BTC, ZEC, and other locking logic; findings tracked to closure | Planned M7 |
| S13 | Third-party review of protocol implementation and remediation | Reviewer report covers atomicity, taker-first, timelocks, adaptor/HTLC constructions, with findings tracked | Planned M7 |

## Demos

| ID | Contract | Acceptance evidence | Status / milestone |
|---|---|---|---|
| D1 | Happy, abandonment/refund, and concurrent-swap recording for each pair | BTC: three recordings attached to the [v0.2.5 release](https://github.com/gateway-fm/lez-atomic-swaps/releases/tag/v0.2.5) — `TakerSellsForeign-happy.mp4`, `TakerSellsForeign-maker-refund.mp4`, `TakerSellsForeign-concurrent.mp4` — each a by-product of a scenario that passed while being recorded, driven through the real desk | BTC done at v0.2.5; XMR/ZEC planned M2–M4; regenerated at M7 |
