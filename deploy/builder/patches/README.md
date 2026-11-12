# Patches for upstream

What this repository adds to upstream Logos/LEZ sources, one directory per
upstream module, each patch ready to offer upstream as it is. The sources stay
clean git submodules at their exact upstream commits (`third_party/`, never
edited); `from-scratch.sh` applies these to a scratch copy before the module's
own flake builds its `.lgx` (`build_logos_module`). A patch named `0001-lez-*`
applies to logos-execution-zone, the others to the module itself.

Pins (`deploy/scripts/from-scratch.sh`): logos-execution-zone v0.2.4 at
`9edf4a622fe9`, lez-indexer-module `5033da442a7f` (1.1.1),
logos-execution-zone-module `acf0cd501b26` (lez_core 0.4.1). blockchain_module
0.2.4 is the released lgx, unpatched.

## lez-indexer-module (github.com/logos-blockchain/lez-indexer-module)

Why: the module answers queries only as Logos calls, and it has no historical
account read; the swap Nodes, their sidecars and the explorer read the indexer
as `indexer_service` JSON-RPC (`getAccountAtBlock` among it).

- `0001-lez-v0.2.4-indexer-breakpoint-interval.patch` — upstream
  logos-execution-zone, `lez/storage`: a state snapshot every 10 blocks
  instead of every 100 (`BREAKPOINT_INTERVAL`, used only by the indexer's
  store). A historical read (`getAccountAtBlock`) replays every transaction
  since the last snapshot, ~0.3 s each: 10–18 s late in a 100-block window,
  past the ~30 s attempts of the swap actors that make these reads
  (`actor_timeout`). Snapshot ids are block / interval, so an index written
  with the other interval must be rebuilt (a fresh chain, or
  `scripts/repair-indexer.sh`). Upstream: make it configurable. Their
  `indexer_core` test `accept_block_snapshots_state_at_breakpoint_interval`
  is written for 100 and fails with this value; the storage tests follow
  the constant.
- `0001-lez-v0.2.4-indexer-ffi-rpc-serve.patch` — upstream
  logos-execution-zone, `lez/indexer/ffi` and `lez/indexer/service`:
  `indexer_rpc_serve(indexer, port)` serves `indexer_service`'s own JSON-RPC
  implementation over the indexer core the FFI already runs
  (`IndexerService::from_core`, no second ingestion; the finalized-block
  subscription is refused). No method is reimplemented. Serving again
  (an indexer restarted with `stop_indexer`/`start_indexer`) replaces the
  previous server.
- `0002-lez-indexer-module-rpc.patch` — the module: `serve_rpc(port)` over
  that call (version `1.1.1-rpc.4`). The module's flake still names LEZ
  v0.2.2; the build supplies the patched v0.2.4 source in its place
  (`--override-input logos-execution-zone`), as it does for lez_core.

## logos-execution-zone-module (github.com/logos-blockchain/logos-execution-zone-module)

Why: the swap sidecar plans the exact bytes of every LEZ transaction and must
not hold the role's key; the wallet has no call that signs for a held account
without building and submitting the transaction itself, and lez_core does not
wrap the wallet FFI's own key import.

- `0001-lez-wallet-ffi-sign-public-message-hash.patch` — upstream
  logos-execution-zone, `lez/wallet-ffi`: `wallet_ffi_sign_public_message_hash`,
  a signature over a 32-byte hash with a public account the wallet holds (the
  `lee::Signature` a public transaction's witness carries).
- `0001-lez-darwin-metal-toolchain-for-nix-build-users.patch` — upstream
  logos-execution-zone, `flake.nix`, macOS only (inside the existing
  `metalStub`, used only when `stdenv.isDarwin`): risc0 compiles its Metal
  kernels with `xcrun metal`, and xcrun finds Xcode's downloadable Metal
  Toolchain through a per-user mapping that a nix build user (`_nixbldN`,
  HOME `/var/empty`) does not have, so the build fails with "missing Metal
  Toolchain" although it is installed. The stub now runs `metal`/`metallib`
  from the toolchain's cryptex mount, with xcrun's SDK as `-isysroot`, and
  falls back to xcrun when there is no mount. On Linux the stub is never used;
  the flake source changes, so lez_core rebuilds once, into a byte-identical
  `lez_core.lgx`.
- `0002-module-import-and-sign-public-account.patch` — lez_core:
  `import_public_account(private key hex)` over the FFI's existing import and
  `sign_public_message_hash(account, hash)` over the call above (version
  `0.4.1-sign.4`). Nothing else: the wallet is opened with lez_core's own
  `open`/`create_new`, and the signer socket the Node's sidecars talk to is
  served by our own module, which asks lez_core for each signature.

## logos-chat-module (github.com/logos-co/logos-chat-module)

Why: chat_module creates the Delivery node itself, from a preset name only
(`start_delivery_bootstrap`: `{mode: Core, preset, messagingOverrides:
{logLevel}}`), so its user can join only a public fleet (`logos.test`,
`logos.dev`). On that fleet a take's Chat rounds were lost or arrived minutes
late (deploy/README.md, "Two transports"); two parties' Basecamps need a
Delivery network of their own: a private cluster id, a listen address, the
other party's node as entry node.

- `0001-chat-module-delivery-config.patch` — `ChatConfig.delivery_config`:
  delivery_module's whole `createNode` JSON, used as it is in place of the
  preset's; absent or empty, nothing changes. The desks' Logos Chat bridge
  passes `LEZ_LOGOS_DELIVERY_CONFIG` (compose.logos.yaml). `from-scratch.sh
  --only nix` fetches the pinned commit (apps/basecamp/flake.lock) into a
  scratch copy, applies this and builds the module's own `lgx-portable`.
  Upstream should instead let the host own Delivery's lifecycle, as their own
  TODO in `start_delivery_bootstrap` says: chat_module would use a node the
  host configured and started.

Our own module (`modules/lez-swap-node`: launches and supervises a role's Node
inside Basecamp and serves that role's signer socket) is not an upstream patch.

| patch | lines | applies to | upstream state |
|---|---|---|---|
| lez-indexer-module/0001 breakpoint-interval | 13 | logos-execution-zone `9edf4a622fe9` | not offered (upstream: make the interval configurable) |
| lez-indexer-module/0001 ffi-rpc-serve | 177 | logos-execution-zone `9edf4a622fe9` | not yet offered |
| lez-indexer-module/0002 | 52 | lez-indexer-module `5033da442a7f` | not yet offered |
| logos-execution-zone-module/0001 wallet-ffi-sign | 120 | logos-execution-zone `9edf4a622fe9` | not yet offered (related: logos-execution-zone-module#73, message signing) |
| logos-execution-zone-module/0001 darwin-metal-toolchain | 35 | logos-execution-zone `9edf4a622fe9` | not yet offered |
| logos-execution-zone-module/0002 | 111 | logos-execution-zone-module `acf0cd501b26` | not yet offered |
| logos-chat-module/0001 delivery-config | 66 | logos-chat-module `dfe8ccf3eff3` | not yet offered (upstream: the host owns Delivery's lifecycle) |
