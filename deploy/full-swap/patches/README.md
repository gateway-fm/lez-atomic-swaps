# Runner patch series

The demo stack drives the certified M3 runner from the implementation history.
This directory holds its complete local delta as an ordered `git format-patch`
series: no runner change exists only in an uncommitted developer worktree. The
exact base commit is still fetched from the implementation repository.

## Reproducing the runner

```sh
workspace_root="$(pwd -P)"
git clone https://github.com/gateway-fm/lez-atomic-swaps.git submission
git -C submission switch m3-plus
git clone https://github.com/mandrigin/lez-atomic-swaps.git runner-work/repo
git -C runner-work/repo checkout 5c384a5
git -C runner-work/repo am \
  "$workspace_root"/submission/deploy/full-swap/patches/*.patch
```

Applying all 34 patches to `5c384a5` reproduces tree
`c3bc49ad802328455ef7c8843d3c7a4ee81bade9` exactly — the same tree the demo
runs against.

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
| **0027-0034** | **Bidirectional runner**: one selected LEZ/BTC direction flows through funding discovery, application replay, terminal assertions, and exported journey evidence |

Patches 0024-0026 make the demo persistent-chain-shaped: without them the
runner provisions and destroys a chain pair per swap. This is still a local
regtest/devnet lane, not a mainnet-readiness claim.

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
