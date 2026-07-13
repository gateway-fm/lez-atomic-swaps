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
does not start Bedrock, the LEZ sequencer/indexer, Zcash Regtest, or Docker; it
does not prove a corridor swap, cross-chain atomicity, public-runtime parity,
or a public deployment. Those claims require the separate local-stack and
actor-level end-to-end gates.

The native archive is outside Cargo license analysis. Upstream metadata identifies
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
