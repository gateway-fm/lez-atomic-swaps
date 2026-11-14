# Provenance

## The run that found the defects

[`found-by/`](found-by/README.md) is the record of the native Linux x86_64 run
of 2026-11-14 (UTC). It used the published, signed 0.3.1 packages, whose
`build_source` is `976f5f72`, in Basecamp 0.3.2, and the devnet's chains on an
Apple-silicon Mac reached through an SSH tunnel. The files are copies of that
run's README, swap log (`results.tsv`), analyses, scripts, scenario logs, Node
logs and RPC call log. Paths are replaced (`/home/<user>`, `<home>`,
`<scratch>`, `<checkout>`, `<workspace>`, `<x86_64-host>`); nothing else is
changed. The run's screenshots and Basecamp session logs are not copied.

## The fixes

Branch `fix/remote-rpc-robustness`, on `976f5f72`:

| commit | issue |
|---|---|
| `b854585a` fix(zec-sdk): accept a LEZ tip that only grew during an observation | #149 |
| `7a09242f` fix(sidecar): read finalized LEZ runs in batches, not two calls per block | #150 |
| `c77849b2` fix(launcher): fix the package's own modes, name an unsafe state ancestor | #151 |
| `870e91e8` fix(harness): native Nodes on Linux, chains reached over RPC only | #152 |
| `2e69dde2` test(deploy): rpc-delay-proxy.py, chain RPC as if on another host | — |

The commit under test is `2e69dde2`. This folder and the documentation
commit sit on top of it.

## What ran, on an Apple-silicon Mac (macOS 26), 2026-11-14

- Workspace: `cargo fmt --all --check`; `cargo clippy --locked --workspace
  --all-targets` with CI's exclusions, `-D warnings`: clean. `cargo test
  --locked --workspace --all-targets` with CI's exclusions, and the
  `pair-zec` Node tests: passed. `cargo doc` with `-D warnings` for
  `lez-zec-swap-sdk` and `lez-bridge-adapter`: clean. On macOS a few tests
  that create Unix sockets or secure directories need `TMPDIR` to be a short
  directory without a symlinked ancestor; they ran with one.
- Sidecar (`compat/lez-v0_2-sidecar`), in the builder image
  (`lez-builder:local`, Linux arm64): `cargo fmt --check`; `cargo clippy
  --all-targets --all-features -D warnings`: clean. `cargo test --all-targets
  --all-features`: 249 passed in 41 test binaries, 0 failed. That run was on
  the sidecar commit before its last amendment, which changes one doc comment:
  `cargo rustdoc -D warnings` failed there on a link to a private function
  and is clean on the commit under test (run on macOS). The dependency policy
  script was not run: it needs `cargo-deny`, and no dependency changed.
- `python3 -m unittest discover -s deploy/tests`: 167 tests, passed
  (1 skipped). `shellcheck --severity=warning` on
  `modules/lez-role-node/node-entrypoint.sh` and `found-by/scripts/`: clean.
- The finalized-window measurement of the README:
  `deploy/scripts/rpc-delay-proxy.py --delay-ms 100` in front of the test
  indexer on `127.0.0.1:18779`, and `cargo test --lib -- --ignored --nocapture
  a_finalized_window_over_a_delayed_link` in `compat/lez-v0_2-sidecar`.
  Outputs: [`measured/`](measured/window-timing.txt).
- `gitleaks` 8.30.1 with the repository's configuration over this folder,
  archives included: no leaks.

## What did not run

- No swap ran with the fixes. The planned proof used the devnet on this Mac,
  lane 1's LEZ network and lane 2's Bitcoin Core and Zebra, with every chain
  RPC of both Nodes through `rpc-delay-proxy.py` at 100 ms. It would have run
  ZEC `zec-happy` in both directions at 1 s LEZ blocks, BTC `happy` in both
  directions and `taker-refund`, each before (images of `538530eb`, the code
  of `main`) and after. Starting the devnet's stopped containers was
  refused by this session's permission check, so nothing was started.
- Node images for that run were built (`lez-maker-node:rpcfix`,
  `lez-taker-node:rpcfix`, build source `9485a350`: the commits above
  before the last two test and rustdoc amendments of the sidecar commit,
  with the same product code). They were not run. Rebuilding them at
  `2e69dde2` stopped in `package-role-node.sh`, whose container could not
  install `jq` (the Debian security archive's signature check failed).
- The fixed build has not been run on the x86_64 host. Nothing in this
  folder shows a swap over a remote link with the fixes.
- The desk does not show the launcher's message for #151; it is in the
  Node's log.
