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
git clone https://github.com/gateway-fm/lez-atomic-swaps.git runner-work/repo
git -C runner-work/repo checkout 5c384a5
git -C runner-work/repo am \
  "$workspace_root"/submission/deploy/full-swap/patches/*.patch
```

Applying all 38 patches to `5c384a5` reproduces tree
`c747dafbdf39ed4615d92b005e63552a32bb60bf` exactly. Patches 0001–0037
reproduce milestone source tree `ae28b574b925e2555b47dd255e6d8565f8cbd7d9`;
patch 0038 applies the publication-time dependency update. This is the implementation
tree the Logos-Chat-and-Delivery-enabled role-agreement demo
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
| **0035** | **Independent role agreements**: Maker and Taker construct separately signed contributions, bind messages and assets to chain identities and direction, and finalize the same agreement without a fixture actor holding both roles |
| **0036** | **Real Logos Chat transport**: Basecamp Chat v0.2.2 and its Delivery runtime carry content-addressed role-agreement frames through bounded role-fixed gateways while the signed Rust stores remain authoritative; includes a Unix-only offline E2E relay |
| **0037** | **Signed Delivery discovery**: Makers broadcast short-lived signed offer projections over the Chat-owned Delivery node; Takers verify a bounded live index, resolve the signed Chat address, and converge competing negotiations through the store-authoritative one-winner reservation |
| **0038** | **Dependency advisory closure**: update `h2` from 0.4.15 to 0.4.16 in all seven affected lock graphs; the Cargo 1.96 refresh also records already-declared path dependencies missing from the stale isolated XMR lock and normalizes target-specific `windows-sys`/`socket2` selections, with no manifest or application-code change |

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
