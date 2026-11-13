# Full regression of `feat/m5-gaps`, 2026-11-12/13

One run of every test of `deploy/scripts/regress.sh full` (123: `regress.sh list full`) on the
Docker devnet, three lanes at a time (`LEZ_LANES=3`), on a chain made for the run, without desk
recordings. 2026-11-12 23:16:31Z to 2026-11-13 01:30:00Z (UTC).

**Result: 122 of 123 passed in one run of 133 minutes; the one failure passed on its rerun after
a change to its test fixture.** 0 timeouts, 0 NOT-EXERCISED, 0 frozen tries; 2 new LEZ chains (the
two `zec-balance-refusal` tests make their own); 3 timing-profile switches. By kind: node:btc 43,
node:zec 41, ui:btc 17, ui:zec 22; test time summed over lanes 19,982 s.

`results.tsv` is the runner's table (test, try, verdict, seconds, finished at, lane, new chain);
`regress.log` is the runner's log, every try's start and verdict line carrying the LEZ finalized
height and tip (local paths replaced by `<checkout>` and `<home>`); `rerun-*` are the rerun's.
The per-test logs are not committed.

## How the Zcash agreements were made

Of the 63 Zcash tests, 59 formed their agreement at take time (`zec_reserve_v1`: the Maker
publishes, the Taker takes, the two Nodes compose and sign it, each with its own key; their logs
say "take-time agreement"), which is the harness default on this branch
(`LEZ_ZEC_AGREEMENT=take-time`). Four used the dealer script `zec-provision.sh`, which holds both
parties' keys: `zec-regenerated-catalog` and `zec-tampered-catalog` in both directions, because
they test the prepared catalog itself (their logs say so).

## The failure

`node:btc:maker-price-module:TakerSellsForeign` failed after 33 s: the Maker refused to publish
with "maker price source revision conflicts with durable quote history". The repository's TEST
price module reported revision 7 for every observation while observing "now − 5 s"; the Maker
keeps one quote head per route and per source identity and refuses the same revision with a
different observation. On a Maker that had recorded revision 7 before (this lane's, after an
earlier run of the test), the next publish was refused. The rule is right for a real source; the
fixture was wrong. `e0204908` gives the fixture a mode in which the revision moves with each
observation. Rerun at `e0204908` on the lane as the full run left it (no new chain, no rebuild:
the change is the fixture and `node-e2e.py`): PASSED in 121 s (`rerun-results.tsv`).

## Source and images

Tests and images from `f741f11d` (branch `feat/m5-gaps`, on PR #127's head `f12c95f6`), tree
clean (`regress.log` prints the commit and "tree dirty: 0"). Trees at that commit
(`git rev-parse f741f11d:<directory>`):

| directory | tree |
|---|---|
| `crates` | `30320fa6b08e7d42663163289e45579a454d093b` |
| `compat` | `b990faaf0903ec0f29abd92f9b4ae80b72bbc2db` |
| `apps` | `995d0a913795937e58593baaabf5c39d77c77b5b` |
| `modules` | `39b11c9c2c919ae45d252d99d5e96d839a7a03bb` |
| `deploy` | `d13790059bae8160c46f99a5abae8c0bba222430` |
| `packaging` | `b17a88e673b5f7427d297ef609c6da3d77da4566` |

Built before the run at that commit (desk packages, sidecar, Rust, stage, images) and not rebuilt
during it or the rerun; local linux/amd64 ids:

| image | id |
|---|---|
| `lez-maker-node:local` | `sha256:69c7c28bb560bc38db3ab83ec3a5ca252e4aaecfc3095b35aca68ce3e463e50d` |
| `lez-taker-node:local` | `sha256:105ca414d71d8d51e0783e74df3750a823804920b6218553cf1094f076af7f7d` |
| `lez-basecamp-ui:local` | `sha256:20a8598b2d4df0b1285f1a39e6596bdb630db9e3427505c90130b1fd7bc4c175` |
| `lez-network-module:local` | `sha256:51a018ec2bceeacb4123b8607d4b2f280377b52729a0f254d251ccbeaf668449` |
| `lez-services:local` | `sha256:ce3eb7aeaa5ab280a0be3bcba875e502d349f0c1b57b8788101e937e4d3fa3a8` |
| `lez-zebra:local` | `sha256:8093a13ad9cfd754315da4d5b152865c6e35c6dea868abb7b753fa55f5c0180c` |
| `lez-bitcoin-core:local` | `sha256:a2fc0c4660aba5b8a2dfd044c1d5f751d17f36b82a79912a4acf253d699ca4cd` |
| `lez-btc-miner:local` | `sha256:6d20393814e0e4c5244df85f21279fe024f54c10b4440e1c618c037c7ee61fe6` |

## Not covered

- Two hosts: the Logos-transport tests (`logos-happy`, Bitcoin and Zcash) run both Basecamps on
  one host with a private Delivery network; the public `logos.test` fleet did not deliver Chat
  reliably when measured (`deploy/README.md`, "Two transports").
- A swap under the systemd units is a separate run (`docs/evidence/maker-systemd-2026-11-12/`).
- Any public network, linux/arm64, macOS, Monero.
