# Group-writable directories on Linux (umask 002): what refused, what was changed locally

Host: Ubuntu 24.04 with user-private groups, where the default umask is 002. Files created
by `logosctl` 0.3.2 and by hand there are 0775 (directories) and 0664 (files). macOS
(umask 022) never meets this.

## What refused

1. **The LEZ sidecar's state-directory lease** (both roles, every BTC swap):
   "LEZ v0.2 bridge PoC failed: acquire exclusive bridge sidecar state-directory lease:
   sidecar state directory or lease file is unsafe". The take then answers
   `initiation_execution_unavailable`.
   - Check: `compat/lez-v0_2-sidecar/src/durable_reservation.rs:351`
     `validate_trusted_parent_chain` (the condition is at lines 361–366: every ancestor
     must be owned by root or the user and not group/other-writable unless sticky). It is
     called from `SecureStateDirectory::open` (`:221`) and `revalidate` (`:247`), which
     `StateDirectoryLease::acquire` (`:295`) maps to `UnsafeState`.
   - Path refused: the state dir `$LEZ_NODE_HOME/<role>/btc/swaps/<id>/sidecar-state`.
     The offending ancestor was the working dir `/home/<user>/lez-verify` (0775). The Node
     home itself was 0700.
2. **The Maker's actor supervisor** (every BTC swap on the Maker): actor schedule
   `failed`, `last_failure_class: actor_deployment_invalid`.
   - Check: `crates/swap-store/src/maker_actor_process.rs:2678`
     `validate_program_parent` (condition at `:2685–2689`: the actor program's parent
     directory must not be group/other-writable). The program file itself is checked at
     `:2722`. `crates/maker-node/src/actor_supervisor/runtime.rs:894`
     `classify_deployment` maps the error to `actor_deployment_invalid`.
   - Path refused: `<user dir>/modules/lez_maker_node/bin` (0775, created by
     `logosctl package install` under umask 002).

## What was changed locally (test host only, product code untouched)

- `chmod 755 /home/<user>/lez-verify` (the working dir holding `LEZ_NODE_HOME`).
- `chmod -R go-w <user dir>`: 104 entries under `modules/` and `plugins/` (every
  installed package's directories and files) plus `logs/`. Also on `tools/zec` and the
  harness checkout.
- Nothing else. With these two changes both BTC directions completed.

## Proper fixes (options)

- **Package manager:** install packages with fixed modes (directories 0755, files 0644,
  executables 0755) regardless of the caller's umask. This is upstream
  (`logos-package-manager`) and the cleanest fix. Our packages cannot rely on it today.
- **Launcher (our code, works now):** at start, `node-entrypoint.sh` removes group/other
  write from its own `bin/` and the actor programs (`chmod go-w "$bin" "$bin"/*`). It
  also refuses to start, with a clear message, when any ancestor of `LEZ_NODE_HOME` or of
  the state dir is group/other-writable. Today the Node starts "ready" and fails only at
  the first swap.
- **Checks accepting user-private groups:** treat a group-writable ancestor as safe when
  its group is the user's private group (gid == uid's primary gid, group has no other
  members). That matches Debian/Ubuntu convention but weakens a deliberately strict check
  and needs group-membership lookups. Not recommended over the two above.
- **Docs, now:** "On Linux, run Basecamp/logosctl with `umask 022`, and keep
  `LEZ_NODE_HOME` under directories that only you can write (no group-writable
  ancestors)."
