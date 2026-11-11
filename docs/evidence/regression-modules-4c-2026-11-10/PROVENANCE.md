# Provenance — step 4c regression (LEZ network node and indexer in Basecamps)

- Code: `4626f8af` (step 4c) for the full run; `0f8228d8` (4c + the faster harness,
  test code unchanged) for the remaining tests and the concurrent rerun. Both on
  feat/lez-basecamp-modules, merged into PR #120 as `ec2dd379`.
- Images, built once from 4626f8af on 2026-11-08 21:50Z and used for every run
  (the image store's manifest-list ids change on each cached rebuild; the creation
  times and build sources do not):

  | image | created (UTC) | Node build source |
  |---|---|---|
  | lez-maker-node:local | 2026-11-08T21:50:19 | 1e2cbc7e90dc |
  | lez-taker-node:local | 2026-11-08T21:50:20 | 1e2cbc7e90dc |
  | lez-basecamp-ui:local | 2026-11-08T21:50:04 | — |
  | lez-network-module:local | 2026-11-08T21:50:05 | — (blockchain_module 0.2.4, sha256 31819f06…) |
  | lez-indexer-module:local | 2026-11-08T21:50:05 | — (lez_indexer_module 1.1.1 + deploy/builder/patches) |

- Stack: the local devnet (LEZ v0.2.4, Bitcoin regtest, Zebra regtest), `fast`
  timing profile (600/900/1800 s in the full run, 360/600/1200 s after 0f8228d8).
- Chains: 35 recreations in the full run, 3 for the remaining tests, 1 for the
  concurrent rerun, each a fresh LEZ chain (`up.sh --fresh-lez`).
- Contents: `RESULTS.txt` (every try), `e2e/` (Node scenario evidence),
  `zec/` (ZEC corridor evidence), `logs/` (each try's harness output).
  Public chain data only; no keys, cookies or capability files.
