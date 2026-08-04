# Basecamp role packages

This directory builds two independent Logos Basecamp `ui_qml` packages. The
Maker console can configure local routes and operate Maker actors; the Taker
route can browse authenticated offers and drive Taker swaps. Each QML view is
unprivileged. Its process-isolated C++ backend calls a fixed allowlist over an
owner-only Unix socket.

## Build and run

From this directory, with Nix flakes enabled:

```sh
nix build .#maker
nix build .#taker
nix build .#maker-lgx
nix build .#taker-lgx
```

Start the applicable repository service as the same operating-system user,
then set exactly one endpoint before launching its standalone package:

```sh
LEZ_MAKER_RPC_SOCKET=/absolute/path/to/maker.sock nix run .#maker
LEZ_TAKER_RPC_SOCKET=/absolute/path/to/taker.sock nix run .#taker
```

The endpoint must be a real Unix socket owned by the effective user with mode
`0600`. The parent service runtime directory is expected to be mode `0700`.
The UI never accepts an RPC URL, filesystem authority, credential, or arbitrary
method name. Requests and responses are bounded to 64 KiB with fixed timeouts.

## Tests and external resources

```sh
nix build .#maker-integration-test
nix build .#taker-integration-test
```

Those load tests use the official Basecamp standalone/MCP framework and do not
contact public infrastructure. Full repository E2E starts the real Maker daemon
and Taker service plus isolated local LEZ and Zcash development nodes. Test funds
come from deterministic local genesis/regtest outputs. No faucet, public RPC,
explorer, or DNS dependency is used; therefore those external services cannot
make the run flaky. Switching to public endpoints is configuration plus the
required chain deployment, not a UI code change.
