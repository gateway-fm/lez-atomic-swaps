# ADR 0007: Maker local RPC and process ownership

Status: Accepted for the operator vertical slice; production transport pending —
2026-07-11

## Context

The RFP requires a headless maker that can run standalone or under Logos Core.
The CLI and mini-app must not open the database or hold the daemon's chain keys.
Local process boundaries are still security boundaries: another local account or
compromised application must not change offers or trigger fund-moving actions.

## Decision

Use typed JSON-RPC methods implemented with `jsonrpsee`. The maker daemon is the
only database writer and the only component that owns maker execution keys. CLI,
mini-app, systemd, and Logos Core are clients or supervisors, never alternate
database writers.

The current executable slice uses HTTP on an ephemeral or configured loopback
address and refuses non-loopback binds. Every request carries an owner capability
of at least 24 bytes, supplied to both processes through the environment rather
than command-line arguments; comparisons use `subtle`'s constant-time primitive.
RPC request and context `Debug` implementations redact the capability. This is an
integration adapter, not the production transport approval.

Before M5 production freeze:

- Unix deployments use a Unix-domain socket inside a mode-0700 runtime directory;
  the socket is mode 0600 and owned by the maker account. Other platforms need an
  equivalent owner-restricted local transport.
- Generate a random 256-bit capability into an owner-readable credential file.
  Authenticate in transport metadata, not JSON parameters, and redact it from
  traces, errors, audit records, crash reports, and process arguments.
- Version the RPC surface and classify methods as read-only, idempotent mutation,
  or fund-moving mutation. Mutations receive request IDs and durable audit/outbox
  records so retries cannot duplicate effects.
- Enforce request/body/concurrency limits and expose health separately from the
  authenticated control surface.

The `--ready-file` option is service-manager/test handoff only. It contains the
loopback URL, never the capability. Tests bind port zero and kill only the child
process they created, so they cannot collide with another developer's daemon.

## Standalone and Logos Core lifecycle

The standalone systemd unit will use `RuntimeDirectory`, `StateDirectory`, and
`LoadCredential` for socket, database, and capability ownership. Its hardening
baseline includes `NoNewPrivileges`, `PrivateTmp`, `ProtectSystem=strict`, a
private home, an explicit writable state path, bounded restart policy, and no
network address families beyond those required by configured chain adapters.

The Logos Core daemon-mode adapter owns only lifecycle and presentation. Its
contract is `start(config, credential_handle)`, `endpoint()`, `health()`, and
`stop(grace_period)`. It must use the same daemon binary and RPC contract as the
standalone mode, pass credentials through an OS-protected handle, wait for the
readiness/health contract, and never deserialize wallet keys or bypass RPC by
opening SQLite. Unexpected Core or UI termination leaves the daemon governed by
the configured ownership mode; it cannot strand a post-lock recovery workflow.

## Evidence and consequences

`operator_journey` launches the actual daemon and CLI binaries, proves an invalid
capability is rejected, kills the daemon, restarts it on a new ephemeral port,
and reads the persisted swap through the CLI. It covers the first UJ-007 control
seam, not pricing, chain actions, a taker role, or a complete swap.

The prototype serializes SQLite access with a mutex on `jsonrpsee` blocking
workers. Replace this with the dedicated persistence actor and atomic outbox
before chain watchers or concurrent mutations are added.
