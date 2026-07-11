# Requirements traceability

Status: initial skeleton. The live RFP is authoritative; this matrix is expanded
before Milestone 1 exit.

| Requirement | Acceptance evidence | Status |
|---|---|---|
| F1 decentralised discovery/coordination | UJ-001 through Delivery/Chat adapters; UJ-006 outage | Planned |
| F2 LEZ-BTC | BTC happy/refund/concurrency black-box suites plus DLC vectors | Planned M3 |
| F3 LEZ-XMR | XMR happy/refund/concurrency suites plus COMIT DLEQ vectors | Planned M4 |
| F4 LEZ-ZEC transparent | ZEC happy/refund/concurrency suite plus BIP-199 vectors | Planned M2 |
| F5 LEZ escrow claim/refund | Escrow instruction tests and standalone sequencer E2E | Planned |
| F6 atomicity | Model tests, happy/refund terminal-state assertions | In progress |
| F7 native/custom LEZ tokens | Parameterized escrow E2E for native token and ATA | Planned |
| F8 price modes | Local-config and Logos-module C API contract tests | Planned M5 |
| F9 headless maker | UJ-007 maker CLI-to-daemon black-box suite | Planned M5 |
| R1 taker-first | `e2e_swap_lifecycle::happy_path…` | Passing core acceptance |
| R2 on-chain-only after lock | Core post-lock API plus UJ-006 with transports stopped | Partial |
| R3 graceful degradation | UJ-005 dependency matrix | Planned M5 |
| R4 persistence | Close/reopen after locks and witness reveal passes; encryption/process-kill matrix remains | Partial |
| R5 concurrency isolation | Two persisted swaps remain independent; full UJ-004 remains | Partial |
| R6 timelock rationale | Pair parameter ADRs and boundary tests | In progress |
| R7 Delivery/Chat outage | UJ-006 | Planned M5 |
| P1 compute units | Per-operation benchmark against named LEZ testnet release | Planned |
