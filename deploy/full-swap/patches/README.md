# Runner patch series

The demo stack drives the certified M3 runner from the implementation repo.
This directory holds the complete local delta that runner carries, as an
ordered `git format-patch` series, so this branch is self-sufficient: nothing
the demo needs lives only on a developer's machine.

## Reproducing the runner

```sh
git clone git@github.com:mandrigin/lez-atomic-swaps.git runner-work/repo
cd runner-work/repo
git checkout 5c384a5                       # upstream main, tag m7-functional-complete
git am /path/to/deploy/full-swap/patches/*.patch
```

Applying all 26 patches to `5c384a5` reproduces tree `1e5fdfd8` exactly — the
same tree the demo runs against. The same commits are also pushed as the
branch `m3-attach-mode` for anyone who prefers a branch to a series.

## What the series contains

| Patches | What they do |
|---|---|
| 0001-0007 | Native arm64 build lane: LEZ service/r0vm pins, `LEZ_NATIVE_TOOLS`, stub `RISC0_HOME`, aarch64 rapidsnark libraries |
| 0008-0009 | GPG keyring in a per-run tmp home (virtiofs cannot host gpg-agent locks) |
| 0010-0013 | Create Core and LEZ containers through compose with `mode: host` ports |
| 0014-0016 | Sidecar drift diagnostics and slow mount-backed exec tolerance |
| 0017-0021 | Deterministic ZEC corridor fixture for UI-initiated swaps |
| 0022-0023 | UI ZEC fixture taker authority isolation and offer-TTL validity |
| **0024-0026** | **Attach mode**: swaps run on the long-standing settlement chains — injectable chain run ids, no chain launch or teardown, funding discovered from unspent mature coinbases, persistent wallet identities, one-time bootstrap reuse, cumulative opening balances, and shared-chain tolerance in the Bitcoin lock confirmation |

Patches 0024-0026 are the ones that make the demo mainnet-shaped: without them
the runner provisions and destroys a chain pair per swap.

## Keeping it current

Regenerate after any further runner change:

```sh
cd runner-work/repo
git format-patch 5c384a5..HEAD -o /path/to/deploy/full-swap/patches --no-signature
```

Verify the series still reconstructs the tree before committing:

```sh
git worktree add --detach /tmp/patch-verify 5c384a5
cd /tmp/patch-verify && git am /path/to/deploy/full-swap/patches/*.patch
[ "$(git rev-parse HEAD^{tree})" = "$(git -C ../repo rev-parse HEAD^{tree})" ] && echo identical
```
