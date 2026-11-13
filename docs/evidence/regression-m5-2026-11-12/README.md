# Full regression of `feat/m5-gaps`, 2026-11-13

One run of every test of `deploy/scripts/regress.sh full` (143: `regress.sh list full`) on the
Docker devnet, three lanes at a time (`LEZ_LANES=3`), on a chain made for the run, without desk
recordings. 2026-11-13 19:40:36Z to 22:56:55Z (UTC). (The directory keeps its first run's date:
other documents link here.)

**Result: 143 of 143 passed in one run of 196 minutes, every test on its first try.** 0 timeouts,
0 NOT-EXERCISED, 0 frozen tries; 2 new LEZ chains (the two `zec-balance-refusal` tests make their
own); 3 timing-profile switches. By kind: node:btc 53, node:zec 51, ui:btc 17, ui:zec 22; test
time summed over lanes 30,914 s.

`results.tsv` is the runner's table (test, try, verdict, seconds, finished at, lane, new chain;
the two tests on a chain of their own have no lane columns); `regress.log` is the runner's log,
every try's start and verdict line carrying the LEZ finalized height and tip. Neither holds a
local path. The per-test logs are not committed.

## How the Zcash agreements were made

Of the 73 Zcash tests, 69 formed their agreement at take time (`zec_reserve_v1`: the Maker
publishes, the Taker takes, the two Nodes compose and sign it, each with its own key; their logs
say "take-time agreement"), which is the harness default on this branch
(`LEZ_ZEC_AGREEMENT=take-time`). Four used the test fixture
`deploy/tests/fixtures/zec-catalog-fixture.sh`, which holds both parties' keys:
`zec-regenerated-catalog` and `zec-tampered-catalog` in both directions, because they test the
prepared catalog itself (their logs say "take-time agreement not used here").

## Source and images

Tests and images from `dcbdb4f2` (branch `feat/m5-gaps`: PR #127's branch merged with `main` at
`538530eb`, and the work since), tree clean (`regress.log` prints the commit and "tree dirty:
0"). Trees at that commit (`git rev-parse dcbdb4f2:<directory>`):

| directory | tree |
|---|---|
| `crates` | `e0001fb48b2d0f221cb9f2e1cb6847e1b2241ef8` |
| `compat` | `84a3f4f691f45b166c4accae6e173e8fda6fc3e8` |
| `apps` | `4435d3885d29b95500a4239fd0e36023977a14b9` |
| `modules` | `31c942a69b73d154affa301b3fb366d217e304e9` |
| `deploy` | `db85c1c97fdcb3615d87b36e6747d9f6229d15a6` |
| `packaging` | `9e605b7dfb43be866bf8c4f855a4f7469811c871` |

Built before the run (desk packages, sidecar, Rust, stage, then the `network`, `maker-node`,
`taker-node` and `basecamp-ui` images at that commit) and not rebuilt during it; local
linux/amd64 ids, with each image's creation time (a build that changes nothing keeps the
earlier image):

| image | id | created |
|---|---|---|
| `lez-maker-node:local` | `sha256:39e9c693ee2a10eb25d8feed6124a6fe1a92906c3c032bd759a7e93fcdc82513` | 2026-11-13 19:37Z |
| `lez-taker-node:local` | `sha256:f14021573d65304ded0deeae5a27b1d6fb8fd6e8b5631425ea9edaba24851832` | 2026-11-13 19:37Z |
| `lez-basecamp-ui:local` | `sha256:a7769e32b667ada27df125696ba888c49d39b4f857db1a9f7ca8770ac943bdc7` | 2026-11-13 19:12Z |
| `lez-network-module:local` | `sha256:3466edb28b658b15eb64c82d9647aa0ee98a7094acf218f8d465c0da70f4aa78` | 2026-11-13 19:12Z |
| `lez-services:local` | `sha256:021a7928e64602ce785ce464a83df7e9c27588391c24f6fd3d048489f7d52bef` | 2026-11-13 18:33Z |
| `lez-zebra:local` | `sha256:42d64e9a9e85dbd24911f26b2676f4ca07f3c7d910e80dfb99dfb4ca49ecc101` | 2026-11-11 11:50Z |
| `lez-bitcoin-core:local` | `sha256:dadb2fd79b64a6e20612440c90146d6501a1b3d2039408760e93d48c33619cc5` | 2026-11-11 15:01Z |
| `lez-btc-miner:local` | `sha256:4a33449ce116b239641c7932c63845e2d7c9a54e0d00f39d19688e7c3e403473` | 2026-11-11 15:01Z |

The sequencer (`lez-services`) and the indexer (`lez_indexer_module`, in `lez-network-module`)
carry the write-ahead-log bound patches (`deploy/builder/patches/README.md`:
`0001-lez-v0.2.4-sequencer-bound-wal.patch`, `0001-lez-v0.2.4-indexer-bound-wal.patch`): the
source-patch stamps of both builds match this commit's patch sets. Without them a devnet chain's
directory grew by gigabytes of write-ahead log within half an hour on this host; with them the
chain stayed small: the host's free disk did not fall over the 196 minutes (17 GB before the
run's chain was made, 21 GB after the last test).

## Earlier run and two known defects

- The previous full run, on `09bed69f` (run `20261113T112837Z`, 222 min), passed 142 of 143.
  `node:btc:offline-after-claim:TakerSellsForeign` timed out after 1,800 s with the Taker in
  `claim_in_progress` and the Maker at `claim_evidence_available`: the Maker's Bitcoin claim was
  journaled as sent with its outcome unknown, was never broadcast, and is never sent again. The
  inferred cause, not proven, is a transient readiness mismatch just before the broadcast. The
  same test passed in this run.
- In a smoke run on the merged head (`12e2e154`), `ui:btc:happy:TakerSellsLez` hung once: the
  Maker's actor failed permanently with `actor_deployment_invalid`. The inferred cause: the state
  database's times change under the Maker monitor's reads while the supervisor validates it. The
  test passed on its rerun and in this run.

Both are rare races in code older than this branch, and neither fix is in this commit. This run
passing does not show that they are gone.

## Not covered

- Two hosts: the Logos-transport tests (`logos-happy`, Bitcoin and Zcash) run both Basecamps on
  one host with a private Delivery network; the public `logos.test` fleet did not deliver Chat
  reliably when measured (`deploy/README.md`, "Two transports").
- A swap under the systemd units is a separate run (`docs/evidence/maker-systemd-2026-11-12/`).
- macOS, and the native-Node harness with the take-time paths, are untested on this head.
- Any public network, linux/arm64, Monero.
