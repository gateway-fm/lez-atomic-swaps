# Journey: operate swaps with the Maker CLI

<!-- logos-docs-template-commit: 63ecf397ca5dae4b81de85a578ec839a78fec1c0 -->

## What the user achieves

A Maker operator installs the headless service, configures routes/prices,
advertises liquidity, monitors history and requests role-authorized recovery.

## Why it matters

Every operational action remains available without a GUI through an
owner-authenticated local boundary and durable replay semantics.

## Key components

- `lez-maker-daemon`: persistent coordinator, pricing, offers and actors.
- `lez-maker-cli`: fixed operator command surface over the owner Unix socket.
- SQLite role store: configuration, history, replay and recovery authority.
- Optional systemd unit: hardened standalone service lifecycle.

## Repository

https://github.com/mandrigin/lez-atomic-swaps @ `main` (use the reviewed M7 candidate commit when published)

## Runtime target

local

## Prerequisites

Linux x86_64; Rust 1.96.0; Git. systemd is optional for the install rehearsal.
Use a dedicated unprivileged operator account and private state directory.

## Commands and expected outputs

```sh
git clone https://github.com/mandrigin/lez-atomic-swaps.git
cd lez-atomic-swaps
cargo test --locked -p lez-maker-node --test operator_journey
cargo test --locked -p lez-maker-node --test maker_service_cli
./scripts/rehearse-m5-maker-service-install.sh
```

The process journey configures the daemon through the real CLI, restarts it and
reads the same durable state. The install rehearsal validates the unit and
private paths without modifying the host system service manager.

## Success command

`cargo test --locked -p lez-maker-node --test operator_journey`

## Expected result

The black-box CLI/daemon operator journey finishes with zero failures.

## Configuration details

Use the generated owner-only daemon configuration, SQLite path, socket path and
credential files described in `docs/manual-user-flows.md` Flow 1/1D. Pair routes
and exact prices are saved atomically. The external price worker is optional and
fails closed without making static local prices ambiguous.

## Failure modes and limits

- Wrong socket ownership/mode or token fails before an RPC request.
- Missing chain routes stay unavailable without disabling healthy pairs.
- Delivery/Chat outage blocks new negotiation but not post-lock recovery from
  role-local state and canonical nodes.

## GitHub point of contact

@mandrigin

## Discord point of contact

mandrigin.eth

## Existing docs or specs

`README.md` M5 section, `docs/manual-user-flows.md` Flows 1/1D/1J, ADRs
0079/0086/0091/0106 and the deployment architecture.

## Hardware requirements

Daemon only: 2 CPU, 8 GB RAM, 20 GB disk; add the per-chain node requirements
for every enabled local route.

## Estimated time to complete

10-20 minutes warm; node-backed swaps require the pair-specific setup time.

## Security notes

Do not run as root, expose the owner socket, log credentials/secrets, share
role databases or delete an incomplete swap. Stop new offers before maintenance
and retain recovery workers until every funded swap is terminal.

