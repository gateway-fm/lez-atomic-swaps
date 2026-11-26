# Provenance of the Stagenet run of 2026-11-26

## The tested commit

Both Nodes were built from `f259e791b47f9cda1ee64344790bf977950babcc`,
without the devnet test hooks (`LEZ_TEST_HOOKS` unset, `LEZ_MONERO=1`), by
`deploy/scripts/from-scratch.sh --only nix`, `rust`, `build` and `stage` in a
clean checkout of that commit, then `docker compose build` under the image
prefix `lez-stagenet040` (the
[guide's recipe](../../monero-stagenet-guide.md#the-stack)).

| Image | Id | `build-source.txt` | `sidecar-source.txt` |
|---|---|---|---|
| `lez-stagenet040-maker-node:local` | `sha256:efe7c6286c6fc4effa01f37bb99b1d59f166b4196979cdd3ca2931636df8abff` | `f259e791b47f9cda1ee64344790bf977950babcc` | `f259e791b47f9cda1ee64344790bf977950babcc` |
| `lez-stagenet040-taker-node:local` | `sha256:e36a624c73b61e72b33d1aaa513e264ca3706881e2c17320836739a5d27b3ebe` | `f259e791b47f9cda1ee64344790bf977950babcc` | `f259e791b47f9cda1ee64344790bf977950babcc` |

No Node binary or sidecar in either image holds the string `LEZ_TEST_HOOKS`,
`test_hooks` or `test-hooks` ([node-route/images.txt](node-route/images.txt),
one line per binary); the devnet's images built with `LEZ_TEST_HOOKS=1` hold
one or two in the Node binary and the sidecar.

`8f0c638c`, the head of the branch the run was made for, differs from
`f259e791` only by the evidence set
`docs/evidence/regression-0.4.0-2026-11-25-macos-arm64-zebra-module/` and
lines of `.gitleaksignore` (`git diff --name-only f259e791 8f0c638c`).

## Monero

The pinned Monero 0.18.5.1 (`tests/e2e/monero/provenance.env`):
`monero-linux-armv8-v0.18.5.1.tar.bz2`, SHA-256
`c0caf042cb7c7b760f5ad6be188084b59352440b32990a78b8051497b9398dbc`, checked
against the repository's signed `hashes-v0.18.5.1.txt` (good signature of
binaryFate, `81AC591FE9C4B65C5806AFC3F0AF4D462A0BDF92`), in the image
`lez-stagenet040-monero:0.18.5.1` built from `tests/e2e/monero/Dockerfile`
(`sha256:f7d4fe6a75b2ba29110f3b70e54b2fd44d59c783dbc8cc47bd8722ed075b6897`).
The same image ran `monerod` and both wallet-rpcs.

## The host

One Apple-silicon Mac with Docker Desktop (linux/arm64), one operator for both
roles.
