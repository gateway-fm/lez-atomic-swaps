# Full regression of `feat/m5-gaps`, 2026-11-12

One run of every test of `deploy/scripts/regress.sh full` (115: `regress.sh list full`) on the
Docker devnet, three lanes at a time (`LEZ_LANES=3`), on a chain made for the run, without desk
recordings. 12:34:05Z to 14:56:09Z (UTC).

**Result: 115 of 115 passed in one run of 142 minutes.** 0 failed, 0 timeouts, 0 NOT-EXERCISED,
0 frozen tries; 2 new LEZ chains (the two `zec-balance-refusal` tests make their own); 1
timing-profile switch. By kind: node:btc 42, node:zec 39, ui:btc 16, ui:zec 18; test time summed
over lanes 21,941 s.

`results.tsv` is the runner's table (test, try, verdict, seconds, finished at, lane, new chain);
`regress.log` is the runner's log, with every try's start and verdict line carrying the LEZ
finalized height and tip (local paths replaced by `<checkout>` and `<home>`). The per-test logs
are not committed.

The 18 tests this branch adds to the 97 of `regression-2026-11-12`, all in this run:
`kill-maker` / `kill-taker` and `zec-kill-maker` / `zec-kill-taker` (both directions),
`zebra-down` and `zec-bitcoin-down` (both directions), `outage-before-take` and
`zec-outage-before-take`, `maker-cli`, `taker-cli`, `zec-taker-cli`, `taker-cli-refund`.

## Source and images

Tests and images from `cbd0ce0a`, the head of `feat/m5-gaps` when it sat on PR #127's then head
`f2f9b980`; tree clean (`regress.log` prints the commit and "tree dirty: 0"). The branch was then
rebased onto PR #127's later head `f12c95f6`, so `cbd0ce0a` is not in its history; the trees below
name the same content. After the rebase `crates`, `compat`, `apps` and `modules` are identical;
`deploy` differs only in what PR #127 added meanwhile (a macOS build patch for `lez_core` and its
README lines, `deploy/README.md`, one unit test in `deploy/tests`), none of it in the Linux
images or the harness the run used. Trees at the tested commit:

| directory | tree |
|---|---|
| `crates` | `bbd18386e5a04256c33f4cd2527bfae0b438a302` |
| `compat` | `ec178b583e405a8ea4bd93446a7e0f0ebf0b9a92` |
| `apps` | `35798988918fda8339d196b09010ab37b2d75ed3` |
| `modules` | `ae9d633b29d2c85f23b89b3cb8085bb12fcea67b` |
| `deploy` | `67a121bc4b8c0e8ec1c4f68f994353a410fddf09` |

Built before the run at that commit (desk packages, Rust, stage, Node images) and not rebuilt
during it; local linux/amd64 ids:

| image | id |
|---|---|
| `lez-maker-node:local` | `sha256:672bde59c812abb1eaede10886b63430875ab438e5e7f77eecf75b26e9ade93c` |
| `lez-taker-node:local` | `sha256:cf8bf7b02151135a0b283d6d709508d246d03b849cf04c8ff6b339625eb9f12f` |
| `lez-basecamp-ui:local` | `sha256:0e96f33ae5f8ce12efcc8cd58e0bb9a42c82c7087f588f026dd2bcfbd619ea2b` |
| `lez-network-module:local` | `sha256:51a018ec2bceeacb4123b8607d4b2f280377b52729a0f254d251ccbeaf668449` |
| `lez-services:local` | `sha256:ce3eb7aeaa5ab280a0be3bcba875e502d349f0c1b57b8788101e937e4d3fa3a8` |
| `lez-zebra:local` | `sha256:8093a13ad9cfd754315da4d5b152865c6e35c6dea868abb7b753fa55f5c0180c` |
| `lez-bitcoin-core:local` | `sha256:a2fc0c4660aba5b8a2dfd044c1d5f751d17f36b82a79912a4acf253d699ca4cd` |
| `lez-btc-miner:local` | `sha256:6d20393814e0e4c5244df85f21279fe024f54c10b4440e1c618c037c7ee61fe6` |

## Not covered

- The Logos Chat and Delivery modules: every swap here uses the run-local directory and the
  Maker's Chat socket shared between the two containers (`docs/requirements-traceability.md`, F1).
- Zcash agreements formed by the two Nodes: each corridor is provisioned by
  `deploy/scripts/zec-provision.sh`, which holds both parties' keys.
- A swap under the systemd units (`docs/evidence/maker-systemd-2026-11-12/`), any public
  network, linux/arm64, macOS, Monero.
