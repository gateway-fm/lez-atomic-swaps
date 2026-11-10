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

- `0001-lez-v0.2.4-indexer-ffi-rpc-call.patch` — upstream
  logos-execution-zone, `lez/indexer/ffi`: `indexer_rpc_call` (one JSON-RPC
  request answered by the running indexer, in process) and
  `indexer_rpc_serve` (indexer_service's own JSON-RPC on a TCP port, served
  from the module process). The module exposes the indexer only through Qt
  calls; the swap Nodes and sidecars read it over its JSON-RPC.
- `0002-lez-indexer-module-rpc.patch` — the module: LEZ v0.2.2 → v0.2.4 in
  the flake, and `rpc(request)` / `serve_rpc(port)` over the two FFI calls.

## logos-execution-zone-module (github.com/logos-blockchain/logos-execution-zone-module)

- `0001-lez-wallet-ffi-sign-public-message-hash.patch` — upstream
  logos-execution-zone, `lez/wallet-ffi`: `wallet_ffi_sign_public_message_hash`,
  a signature over a 32-byte hash with a public account the wallet holds (the
  `lee::Signature` a public transaction's witness carries), so a caller signs
  without the private key leaving the wallet.
- `0002-module-import-and-sign-public-account.patch` — lez_core:
  `import_public_account(private key hex)` over the FFI's own import,
  `sign_public_message_hash`, `ensure_wallet(config, storage, password)` (open
  or create, idempotent) and `serve_signer(socket, account)` (a signer thread in
  the module process answering one JSON line per connection on an owner-only
  Unix socket). Basecamp then holds each role's LEZ signing key.

Our own module (`modules/lez-swap-node`, which launches and supervises a
role's Node inside Basecamp) is not an upstream patch.
