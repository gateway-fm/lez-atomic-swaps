# Journey: perform a swap with the Taker CLI

<!-- logos-docs-template-commit: 63ecf397ca5dae4b81de85a578ec839a78fec1c0 -->

## What the user achieves

A Taker authenticates an offer, initiates a prepared swap, monitors its durable
state and requests the role-correct claim or refund without using the GUI.

## Why it matters

The primary funding role can independently verify offer/agreement facts and
retain post-lock recovery authority when Delivery or Chat disappears.

## Key components

- `lez-taker-service`: owner process for offer, acceptance and terminal actions.
- `lez-taker-cli`: fixed user commands over a private Unix socket.
- Signed Delivery and Maker Chat: pre-lock discovery/negotiation only.
- Pair actor and registry: durable chain authority, status and exact replay.

## Repository

https://github.com/mandrigin/lez-atomic-swaps @ `main` (use the reviewed M7 candidate commit when published)

## Runtime target

local

## Prerequisites

Linux x86_64; Rust 1.96.0; Git; a distinct unprivileged Taker account and
private state directory. Pair-specific local nodes are needed for chain effects.

## Commands and expected outputs

```sh
git clone https://github.com/mandrigin/lez-atomic-swaps.git
cd lez-atomic-swaps
cargo test --locked -p lez-maker-node --test taker_lifecycle_process
cargo test --locked -p lez-maker-node --test taker_service_process
```

The real Taker service/CLI process tests authenticate offers, admit and replay a
prepared swap, list/monitor it and preserve owner state across restart without
public network access.

## Success command

`cargo test --locked -p lez-maker-node --test taker_lifecycle_process`

## Expected result

The black-box Taker CLI/service lifecycle finishes with zero failures.

## Configuration details

Use the run-local socket, registry, pinned Maker Delivery key, trusted-time
policy, Chat endpoint and pair actor configuration documented in
`docs/manual-user-flows.md` Flows 1K/1T/1Y. Taker and Maker paths must differ.

## Failure modes and limits

- Unknown/expired offers, key-pin mismatch or changed replay fail before funds.
- Missing pair capability is reported while other configured pairs remain.
- After first lock, loss of Delivery/Chat is expected; recovery uses retained
  local state and authenticated chain nodes.

## GitHub point of contact

@mandrigin

## Discord point of contact

mandrigin.eth

## Existing docs or specs

`README.md` M5/M6 sections, `docs/manual-user-flows.md` Flows 1K/1T/1Y, ADRs
0087/0093/0131-0145 and the system architecture.

## Hardware requirements

Service only: 2 CPU, 8 GB RAM, 20 GB disk; add pair-specific node and temporary
artifact requirements for a local end-to-end swap.

## Estimated time to complete

10-20 minutes warm; node-backed completion follows the selected pair guide.

## Security notes

Verify authenticated offer facts before acceptance. Protect the registry,
receipts, actor config, keys and recovery state; never share them with Maker or
delete them before terminal chain evidence. Treat transparent-chain activity as
public at deployment.

