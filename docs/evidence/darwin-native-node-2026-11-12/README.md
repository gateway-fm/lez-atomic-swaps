# Native role Nodes on macOS, 2026-11-12

Both role Nodes started natively on an Apple-silicon Mac from their
darwin-arm64 Logos packages, by `lez_swap_node` in a `logosctl` session, with
only the chains (LEZ sequencer and indexer, Bitcoin Core, Zebra) in the
devnet's Docker containers, reached on their published loopback ports. How
it works and what is weaker than Linux: packaging note,
["The Node on macOS"](../../architecture/logos-native-packaging.md#the-node-on-macos).

## Inputs

- Code: the role packages were built from `aa240744` (`bin/payload.json`:
  `build_source` `aa24074467b04561d9243e0ae924174b4f852f41`, sidecar tree
  `aead716920300ecc8c4b5d01779ff8022c664b27`) with
  `deploy/scripts/package-role-node-darwin.sh`, on the darwin-arm64 plugin
  packages the module builder built from `14ebc3e5`. The harness ran from
  `be034cac` (`LEZ_E2E_NATIVE_RUN_ROOT`). `lez_swap_node` 0.3.0 and
  `lez_core` 0.4.1-sign.4 are the module builder's darwin-arm64 `lgx-portable`
  builds of `14ebc3e5` and of the patched LEZ.
- Host: macOS 26 on Apple silicon, `logosctl` 0.3.2 for macOS, one session
  per role (`session.sh`), the daemon's `PATH` `/usr/bin:/bin:/usr/sbin:/sbin`
  (the launcher ran under `/bin/bash` 3.2).
- Chains: the v0.3.0 devnet stack's chain containers (`lez-bitcoin-core`,
  `lez-btc-miner`, `lez-zebra`, `lez-network`, `lez-sequencer`,
  `lez-heartbeat`), started after being stopped; its Node containers stayed
  stopped. The market bootstrap manifest and the identities of
  `maker-munich-01` and `taker-zurich-01` came from the stack's `runtime/`.

Paths are replaced by `$LEZ_NODE_HOME` (the launcher's directory for both
roles), `$SESSIONS` (the logosctl sessions), `$OUT`, `$MODULES`, `$CORE`,
`$LOGOSCTL`, `$DEPLOY`, `$CHECKOUT` and `$HOME`. The wallet's `create_new`
result is redacted; keys are never printed.

## Files

| File | What |
|---|---|
| `packages.txt` | the two darwin-arm64 role packages and their sha256 |
| `session.sh`, `start-role.sh` | the session per role and the desk's calls (signer, then Node) |
| `maker-start.txt`, `taker-start.txt` | install, load, the desk's calls, `maker_health` / `taker_health` over the owner socket, `node_stats`, the Node's process group |
| `taker-host-killed.txt` | the Taker's `lez_swap_node` host killed: the Node's group ends; `module load lez_swap_node` resumes signer and Node |
| `e2e/*.log`, `e2e/*.json` | `node-e2e.py` against the native Nodes. `happy.json` is the run after the reload (`happy-after-reload.log`); the first run's log is `happy.log` |
| `linux-equivalence/` | the launcher before (`old-*`) and after (`new-*`) the change, run in `lez-maker-node:local` with the Node, its CLI and curl faked (`run.sh`): the same output for both roles |

## Results

| Check | Result |
|---|---|
| Maker Node from `lez_maker_node` (darwin-arm64) | `maker_health`: ready true, degraded false, Delivery and Chat available |
| Taker Node from `lez_taker_node` (darwin-arm64) | `taker_health`: ready true, degraded false, Delivery and Chat available; the Maker's identity read from the shared Delivery directory |
| `node_stats` | the Node first, its parent the `lez_swap_node` `logos_host` |
| BTC `happy`, Taker sells BTC | passed, 36 s; the Bitcoin claim a key-path spend |
| BTC `happy`, Taker sells LEZ | passed, 30 s |
| `replay` | passed, 36 s |
| `wrong-inputs` | passed, 36 s |
| host killed (SIGKILL) | the Node and its four sidecars gone within 2 s |
| `module load lez_swap_node` after that | signer and Node resumed, Taker ready; `happy` passed again, 34 s |
| LEZ signing | through each role's `lez_core` (`serve_signer`); `signer_key_file` null in both `btc-role.json` |

Not run: any ZEC scenario (its corridor is provisioned only into the Node
containers' volumes), the refund, restart and off-chain scenarios (the
harness refuses them with native Nodes), the desks in Basecamp.
