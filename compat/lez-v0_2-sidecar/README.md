# LEZ v0.2 sidecar verification

This crate is the process boundary between the swap actors and the exact
LEZ v0.2 (`LEE`) RPC/type generation. For milestone evidence, run
`scripts/verify-lez-v02-sidecar.sh`; that wrapper is authoritative. Direct
`cargo` commands do not certify this crate because Cargo's offline mode alone
does not stop the upstream `rust-rapidsnark` build script from attempting an
implicit release-asset download.

## Manual prerequisites

Install the pinned Rust 1.96.0 toolchain and `cargo-deny` 0.19.9. The machine also
needs Bash, `awk`, `rg`, `sha256sum`, the C/C++ and libclang prerequisites used
by bindgen, and a complete local Cargo/Git dependency cache. The verifier uses
`--locked`, `--offline`, and `CARGO_NET_OFFLINE=true`; it never downloads or
provisions a missing tool or dependency.

Provide an absolute path to the already-extracted rapidsnark v0.0.8 libraries.
The wrapper attests all four files against the identities in
`../lez-v0.2-provisional/local-stack.toml` before it invokes Cargo:

| File | Required SHA-256 |
| --- | --- |
| `librapidsnark.a` | `d4133227f845ff5bfa3672eb5b9c018a6a086bfa164b176bdaf76949c7d1f423` |
| `libgmp.a` | `0a910b420c3ad603c83c9dc2818c7ae05394c231ca23135c7b873e8e680ea41b` |
| `libfq.a` | `797b5d24bb8e8b088f811bddfff35f33973af9c797fb3812489cd42ba6a957d0` |
| `libfr.a` | `40f809394904682cb5517845cd3c2f936a5eb4609712534b573f552f2811fb82` |

From the repository root, run:

```bash
export RAPIDSNARK_LIB_DIR=/absolute/path/to/verified/rapidsnark-v0.0.8-libraries
export BINDGEN_EXTRA_CLANG_ARGS=-I/usr/lib/gcc/x86_64-linux-gnu/13/include
./scripts/verify-lez-v02-sidecar.sh
```

The wrapper then checks formatting, locked offline tests, strict Clippy,
rustdoc with warnings denied, and `check-dependency-policy.sh`. Missing,
relative, or hash-mismatched native inputs fail before Cargo starts.

## Scope and limitations

This gate verifies the isolated sidecar source and its dependency boundary. It
also verifies the first fail-closed native prepare foundation: the exact v0.2
escrow instruction and account types emitted by the pinned SPEL generator,
official LEE `Message` and `PublicTransaction` bytes, isolated role, runtime,
signer, and program bindings, one owned consecutive nonce pair, canonical byte,
hash, and signature validation, and exact in-memory replay. Tests assert the
generated initialize and fund account order and every instruction field.
Recovered pairs are validated against the complete original request; raw
transaction bytes alone do not prove an authenticated actor role.

The gate also verifies deterministic actor Vault Claim preparation against the
official `programs` and `vault_core` crates from LEZ v0.2.0. The maker fixture
(private-key byte `01`, allocation `100000`) and taker fixture (private-key byte
`02`, allocation `200000`) reproduce the public-key, owner-account, and Vault
PDA snapshots in ADR 0025. Each exact public transaction uses
`programs::vault().id()`, `vault_core::compute_vault_account_id`, ordered
accounts `[owner, owner_vault]`, one owner nonce, one owner witness, and
`vault_core::Instruction::Claim { amount }`. The complete request carries the
node-observed owner nonce; the configured nonce source must independently
confirm that same value before signing. Tests reject role, runtime, key,
allocation, program, account, order, amount, nonce, transaction-ID, canonical
byte, and signature substitutions.

Preparation is atomic at the planner boundary: its per-actor mutex serializes
nonce ownership, and it installs the active reservation only after the complete
official transaction has been constructed, signed, and converted to its exact
bounded representation. A failed preparation exposes and caches no partial
claim. This is local preparation atomicity only; finalized on-chain balance
conservation remains an actor-level end-to-end property.

Exact LEZ v0.2 still exposes a `PrivateKey` whose `Debug` and `Display` reveal
raw material and which does not implement `Zeroize`. This planner never formats
the key and keeps its retained byte copy in `Zeroizing<[u8; 32]>`, but upstream
constructor inputs and transient signing copies cannot be claimed fully
zeroized. LOGOS-008 tracks replacement or an upstream fix before production.

The prepare foundations expose no submission. Each single active reservation
deliberately fails closed for the life of its one-actor signer instance. Durable
per-operation reservation recovery, concurrent-swap partitioning, authenticated
server integration, official node nonce wiring, observation, and exact-byte
submission remain future work. Submission must not be enabled until a restart
can recover the originally signed bytes without reconstructing or re-signing
them.

This gate does not start Bedrock, the LEZ sequencer/indexer, Zcash Regtest, or
Docker; it
does not prove a corridor swap, cross-chain atomicity, public-runtime parity,
or a public deployment. Those claims require the separate local-stack and
actor-level end-to-end gates.

The native archive is outside Cargo license analysis. Upstream metadata
identifies
rapidsnark as LGPL-3.0-or-later and GMP as LGPLv3-or-GPLv2. This local/CI
gate publishes no artifact. Before distributing a statically linked binary or
image, production release review must determine and satisfy the applicable
source, relinking, and notice obligations. This records the upstream metadata
boundary; it is not legal advice.

The original rapidsnark archive is an external GitHub release asset, and Cargo
also normally resolves crates and pinned Git sources from external services.
Acquiring those inputs can be unavailable or flaky. Provision them out of
band, verify them with the local-stack source contract, and retain a trusted
local cache. A cold build can additionally be slow and disk/CPU intensive.
Once the inputs are cached, this wrapper deliberately avoids the network and
fails closed instead of falling back to any upstream download.
