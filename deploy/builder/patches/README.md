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

- `0001-lez-v0.2.4-indexer-ffi-rpc-serve.patch` — upstream
  logos-execution-zone, `lez/indexer/ffi` and `lez/indexer/service`:
  `indexer_rpc_serve(indexer, port)` serves `indexer_service`'s own JSON-RPC
  implementation over the indexer core the FFI already runs
  (`IndexerService::from_core`, no second ingestion; the finalized-block
  subscription is refused). No method is reimplemented.
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
- `0002-module-import-and-sign-public-account.patch` — lez_core:
  `import_public_account(private key hex)` over the FFI's existing import and
  `sign_public_message_hash(account, hash)` over the call above (version
  `0.4.1-sign.4`). Nothing else: the wallet is opened with lez_core's own
  `open`/`create_new`, and the signer socket the Node's sidecars talk to is
  served by our own module, which asks lez_core for each signature.

Our own module (`modules/lez-swap-node`: launches and supervises a role's Node
inside Basecamp and serves that role's signer socket) is not an upstream patch.

| patch | lines | applies to | upstream state |
|---|---|---|---|
| lez-indexer-module/0001 | 176 | logos-execution-zone `9edf4a622fe9` | not yet offered |
| lez-indexer-module/0002 | 52 | lez-indexer-module `5033da442a7f` | not yet offered |
| logos-execution-zone-module/0001 | 120 | logos-execution-zone `9edf4a622fe9` | not yet offered (related: logos-execution-zone-module#73, message signing) |
| logos-execution-zone-module/0002 | 111 | logos-execution-zone-module `acf0cd501b26` | not yet offered |
