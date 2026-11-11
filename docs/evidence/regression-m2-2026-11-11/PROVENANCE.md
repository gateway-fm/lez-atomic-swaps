# Provenance — M2 release-candidate regression on the modules stack, 2026-11-11

Everything in this folder was produced on one host by the Docker devnet of
`deploy/`: every test of `scripts/regress.sh full` (93: `regress.sh list
full`), the Node scenarios by `scripts/node-e2e.py` and the desk journeys by
`scripts/ui-e2e.sh` (the desks driven in each party's own Basecamp through
`ui-tests/verify.mjs`), three at a time on three regression lanes.

Nothing here is private. The ZEC exports carry swap views, durable actor
status, zebra heights and transaction ids; the BTC summaries carry timing,
swap ids and claim transaction ids; the logs carry step lines and captions.
No key, cookie, capability file, `runtime/` or `market/` content is
included. Local paths in the logs are replaced by `<checkout>`, `<home>`
and `<scratch>`. Scanned before commit with the repository's gitleaks
configuration and for key, cookie, password and preimage patterns.

## Source

Tests ran from `023c929e` (branch `feat/split-lez-node-route`, PR #120), tree
clean: `regress.log` prints the commit and "tree dirty: 0" at its start. That
is 307 commits after `27ecee2f` (M2 on the Docker devnet, PR #119, the code of
`regression-m2-2026-11-02`). What changed in that range, by theme
(`docs/submission-115.md` maps it to issue #115):

- the stack as Logos modules: one network Basecamp (the Logos Blockchain node
  and the LEZ indexer), each role's LEZ wallet, Node supervisor and Node (a
  Logos package) in the role's Basecamp; release-variant packages with
  version-ranged dependencies;
- the devnet: 1 s LEZ blocks, the finality-freeze configuration fix, a
  genesis time per chain, timing profiles `fast` and `refund`, regression
  lanes, a stack prefix;
- Zcash: NU6.3 and NU7 signing (librustzcash 0.31), Zebra 7.0.0 regtest with
  NU7 from height 1, devnet profiles `deterministic-local-devnet-v5` / `-v6`;
- fixes for issues #103 (items 1, 3, 4, 5, 16), #112 and #113
  (`docs/issue-closing-notes-2026-11-11.md`).

## Images

Built on 2026-11-11 between 14:57Z and 15:01Z by the landing that preceded
the run (rust, sidecar, nix desks and modules, stage, compose) and not rebuilt
during the run or the reruns:

| image | id |
|---|---|
| `lez-maker-node:local` | `sha256:2a04334a0b9f4bcdf101c2e9c9f73edecd79dc4f338d7046a204eb365f5a1940` |
| `lez-taker-node:local` | `sha256:2204aea8763545f31c920d08508819edc4e50e8c75d4bb9fd1a982dd00bbcbc7` |
| `lez-network-module:local` | `sha256:77b4c3a8622166c41b75810c56905c44afc340586eade5cf9facf4f0a83808d1` |
| `lez-services:local` (sequencer) | `sha256:b00bbe5b33124bbcd593946c54cc160fe05c153d70ea6658991b758694ffcc61` |
| `lez-zebra:local` (Zebra 7.0.0) | `sha256:d20f3f51604c0f9e9842ebe8e16502efd3313331c03df53b0443af667098810d` |
| `lez-bitcoin-core:local` | `sha256:dc9f379d3358c80ab98a304d3a863210be63ba59427d92297d11bb5068a18f1b` |
| `lez-btc-miner:local` | `sha256:89812ace66bed267a05dac86be056ef19e9d0ba62352764fcf03d7f515f71fd5` |

These are local ids of images built on this host (linux/amd64); no image was
pulled from a registry and no release was built from this commit.

Each Node image carries its build source in
`/usr/local/share/lez/build-source.txt`: `165c4e7f75772473c9e733f85c313dac64782ab2`.
That is the commit the landing built at, one amend before `023c929e`; the two
differ only in `deploy/scripts/logos-native-acceptance.sh`
(`git diff 165c4e7f 023c929e`): crates, sidecar, desks, modules, image
contexts and compose files are identical.

## The run

- Host: Linux 6.8 x86_64, 8 cores, Docker 29.1.3. Other activity on the host
  during the run, none of it on the stack: in its first hour a tool build at
  two jobs and a cache copy by a parallel task (stopped at 16:35Z), and
  documentation checks at low priority throughout.
- `up.sh --fresh-lez` (LEZ_LANES=3) at 15:32Z made the chain the run started
  on; `LEZ_REGRESS_RECORD=1 LEZ_LANES=3 scripts/regress.sh full` ran from
  15:34:47Z to 17:48:59Z (134 min).
- Lanes: lane 1 is the stack; lanes 2 and 3 are one more Maker/Taker pair each
  with their own Bitcoin Core, miner and Zebra, on the stack's one LEZ network.
  `results.tsv` names the lane of every test.
- LEZ chains: the starting chain served every test but two;
  `zec-balance-refusal` in each direction starts a chain of its own (the last
  two tests of the run). No chain was replaced for a finality freeze: 0 frozen tries.
- Timing profiles: `fast` -> `refund` at 15:35Z for the refund tests,
  `refund` -> `fast` at 17:06Z (Nodes and miner recreated, the chain kept).
- Swaps are reset between tests by `reset-swaps.sh --abandon-open` on the
  test's lane (`logs/regress.log`).
- The one failure and its two reruns (17:49Z, 17:52Z; single lane, same images
  and chain) are described in `RESULTS.txt`.

## After the run: the collision test's harness fixes

`RESULTS.txt` ends with an addendum: five further runs of `zec-claim-collision`
through `regress.sh` between 18:02Z and 18:44Z, at the harness commits
`d0a626c5` and `2adef99b`, on the same images and the same chain. Those
commits and `34b8a169` change only `deploy/scripts/regress.sh` and
`deploy/scripts/node-e2e.py`; no image was rebuilt.

## Recordings

Every desk test was recorded on its lane's two Basecamp displays
(`ui-e2e.sh --record`); a recording is a by-product of the test that passed.
32 recordings were made; the two Bitcoin `node-offline` desk tests have no
recorded desk step and produced none. Seven are in `videos/`, the ZEC demo
set: `zec-happy`, `zec-maker-refund` and `zec-concurrent` in both directions
and `zec-stale-window` selling ZEC. The other 25 (the Bitcoin set and the
remaining ZEC journeys) are kept for the release's attachments, not in git.

## Not covered by this set

- Any public network: see `docs/m2-acceptance-checklist.md` ("State on
  2026-11-11").
- linux/arm64 and macOS: every image here is linux/amd64.
- The release workflow and a release bundle: none was built from this commit.
- The Logos-native package acceptance: `docs/evidence/logos-native-2026-11-11-b/`
  (same packages as these images).
