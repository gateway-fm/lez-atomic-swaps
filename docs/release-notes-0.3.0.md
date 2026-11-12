# v0.3.0

Draft of the release text. Every `TBD(release)` is filled from the green
release workflow run and the runs still in progress before the tag is made.

Milestone 2 ([logos-co/rfp#122](https://github.com/logos-co/rfp/issues/122)):
ZEC/LEZ swaps through the Maker and Taker desks and Nodes, on the stack that
runs as Logos modules in Basecamp (issue #115). The Zcash leg is a BIP-199
HTLC on the transparent pool; the LEZ leg is the same LEZ v0.2 escrow the
Bitcoin pair settles on. Both run on private local chains: Zebra 7.0.0
regtest (NU7), Bitcoin Core 31.1 regtest and a LEZ v0.2.4 devnet.

The BTC/LEZ pair (M3, accepted at v0.2.6) behaves as in v0.2.6 except for
the fixes listed below.

## What is in it

- **The ZEC/LEZ pair.** The SDK is `lez-zec-swap-sdk` on `lez-swap-sdk-core`;
  `docs/sdk-lez-zec.md` maps discovery, negotiation, escrow creation, claim and
  refund to the public API and the test that drives each, and
  `cargo run -p lez-zec-swap-sdk --example full-lifecycle` walks all five
  offline. Node setup: `docs/zcash-testnet-setup.md`. Privacy posture:
  `docs/sdk-lez-zec.md#privacy-posture`; the Taker desk shows the
  shield-after-swap guidance on a completed ZEC row. CI funds, claims and
  refunds a real HTLC through Zebra on every push (`zebra-regtest`).
- **The stack as Logos modules.** The LEZ network node (`blockchain_module`
  0.2.4) and indexer run in one network Basecamp; each party's Basecamp holds
  its desk, its LEZ wallet (`lez_core`) and `lez_swap_node`, which starts the
  role's Node from the role package `lez_maker_node` / `lez_taker_node`.
  Compose still hosts the Basecamps and supplies configuration, identities and
  volumes. What runs where, the six upstream patches and the gaps:
  `docs/submission-115.md`.

## Defects found and fixed by the #103 scenarios

The scenarios rewritten or added for issue #103 found three defects on the
stack; each was fixed with a failing test first.

- A Bitcoin Maker with a refund request open stopped observing the Taker's
  revealing claim (`9579950b`).
- Neither Node found a LEZ claim final once it was past the swap's discovery
  window (`964cb8c8`).
- The Zcash path took a counterparty's LEZ lock from the sequencer without
  waiting for finality (`17cfa50a`).

Other fixes since v0.2.6: the latent `swap-core` refund and claim paths
(#103 items 1 and 16); the actor's last outcome and what it waits for on both
Nodes' swap views and the desk rows, and the header badge following the
Node's health (#103 items 6 and 7); LEZ finality in Node health and on both
desks, and a landing claim or refund shown as submitted (#112); `down.sh
--wipe` then `start.sh` trades again, and `start.sh` completes on a host with
nothing (#113, PR #127). Details and commits:
`docs/issue-closing-notes-2026-11-11.md`.

## Images

Published to `ghcr.io/gateway-fm/lez-atomic-swaps/` at tag `v0.3.0`, from
release-images run TBD(release) (run id and its result, smoke job included).
All are `linux/arm64`.

| Image | Digest at `v0.3.0` |
|---|---|
| `lez-maker-node` | TBD(release) |
| `lez-taker-node` | TBD(release) |
| `lez-network-module` | TBD(release) |
| `lez-basecamp-ui` | TBD(release) |
| `lez-services` | TBD(release) |
| `lez-wallet-module` | TBD(release) |
| `lez-bitcoin-core` | TBD(release) |
| `lez-zebra` | TBD(release) |
| `lez-btc-miner` | TBD(release) |
| `lez-btc-explorer` | TBD(release) |
| `lez-explorer` | TBD(release) |

The bundle is `lez-swap-stack-v0.3.0-arm64.tar.gz`; `./scripts/start.sh`
starts the stack from it.

Our packages in this release are all at 0.3.0: the desks
`lez_atomic_swap_maker` and `lez_atomic_swap_taker`, the LEZ Network desk
`lez_network`, `lez_swap_node`, and the role packages `lez_maker_node` and
`lez_taker_node`. The upstream ones keep their own versions (`lez_core`
0.4.1-sign.4, `lez_indexer_module` 1.1.1-rpc.4, `blockchain_module` 0.2.4).

## Regression evidence

Each set is under `docs/evidence/`, with `PROVENANCE.md` and `RESULTS.txt`.

| Set | Host | Commit | Result |
|---|---|---|---|
| `regression-m2-2026-11-11` | Linux x86_64 | `023c929e` | 93 tests: 92 passed in one run, the one failure passed on its rerun |
| `regression-2026-11-12` | Linux x86_64 | tests `2b6569e0`, images `964cb8c8`, rerun `62cf95ca` | 97 tests: 96 passed in one run, the one failure passed on its rerun |
| `regression-m2-2026-11-12-macos-arm64` | Apple silicon, Docker Desktop | `af3ac978` | 93 tests: 93 passed |
| `regression-2026-11-12-macos-arm64` | Apple silicon, Docker Desktop | `f2f9b980` | 97 tests: 97 passed |
| TBD(release) | Apple silicon, Docker Desktop | `b6b1c273` | TBD(release) |

Release gates on an Apple-silicon Mac (resume and Node restart, smoke tier,
LEZ finality stall, wipe then start then trade):
`macos-arm64-release-gates-2026-11-11`, rerun on images built from
`af3ac978`.

## Recordings

Seven ZEC recordings are in git, in
`docs/evidence/regression-2026-11-12/videos/`: `TakerSellsForeign-zec-happy`,
`TakerSellsForeign-zec-maker-refund`, `TakerSellsForeign-zec-concurrent`,
`TakerSellsForeign-zec-stale-window`, `TakerSellsLez-zec-happy`,
`TakerSellsLez-zec-maker-refund` and `TakerSellsLez-zec-concurrent`. Each is a
by-product of a desk test that passed while being recorded. The full set,
Bitcoin included, is attached to this release: TBD(release) (attachment
names).

## Upgrading

Upgrading from one release to the next is not supported before 1.0.0
(#133). Drain first: withdraw every offer, let each open swap reach
`completed` or `refunded`, then install v0.3.0 on fresh state.

## Public networks

The public LEZ testnet runs LEZ 0.3, which this LEZ v0.2.4 stack cannot talk
to. Public-network evidence for the pair is postponed to the release that
ports the stack to LEZ 0.4 (#107). The swap engine signs for Zcash Testnet's
NU7; no transaction of ours has been broadcast on Zcash Testnet.

## Known limitations

- #130: selling ZEC, the Taker Node can offer Claim one Zcash confirmation
  before its actor accepts it. On a network that keeps producing blocks this
  resolves by itself with the actor's retry; it shows only on a devnet that
  mines on demand, and the test harness mines the missing block.
- #112: chain events in the Activity pane and `btc_height` are not done.
- #52: the Maker's offer snapshot still grows with consumed offers and swap
  history.
- Native package acceptance (`docs/evidence/logos-native-2026-11-11-c/`):
  116 steps per role, 13 FAIL each; 9 are unsigned packages, out of scope for
  this release, and 4 are `logosctl` behaviour, classified in
  `logos-native-2026-11-11-b/README.md`. A file install resolves no
  dependencies, so packages install one by one. The Node's configuration and
  identity still come from Compose.
- The LEZ finality freeze upstream (logos-blockchain#3643) is worked around on
  the devnet by configuration; it is to be tested again on LEZ 0.4.
- The full list: `docs/submission-115.md`, "Known limitations".

## macOS

Runs: the Docker stack on Apple silicon (Docker Desktop, `linux-arm64`
images), built from nothing, with the full regression above and the release
gates. Does not run yet: a native darwin package. Since PR #126 the Nodes,
the actors, the LEZ sidecar and every module build for `darwin-arm64` and
the product crates pass their tests on macOS, but the launcher and the
packaging are Linux-only and nothing has run in a Logos host on macOS.

## Corrections to the v0.2.5 release notes

Issue #103 items 11, 12 and 13 (issue #134, closed as superseded by this
evidence).

- **Property test.** The v0.2.5 text says proptest "shrinks to exactly the
  reported shape". The checked-in seed shrinks to a replayed claim on a
  completed swap, which `c75576ab` now refuses at the lock removal. The model
  has since gained a maker-lock depth and a new property,
  `no_refund_after_reveal_and_no_claim_below_maker_policy`; what that
  property shrinks to has not been checked.
- **Regression count.** The v0.2.5 text gives both "43 of 43" and "45 of 45".
  43 of 43 was the run of the pre-merge branch head; 45 of 45 is the run of
  merged `main` (`4b8631b`), the evidence for the tag, committed under
  `docs/evidence/regression-v0.2.5-20260928/`. Its `PROVENANCE.md` lists the
  four chain recreations that run's log records and says what the log does
  not hold.
- **Recordings.** The three v0.2.5 recordings carry offer ids from before the
  v0.2.5 merges and state no build, so nothing shows they were made on the
  tag. Whether
  the Taker's "Completed" is on screen in the happy-path recording has not
  been checked. From this release on each recording is a by-product of a
  passing test at a stated source on recorded image ids.
