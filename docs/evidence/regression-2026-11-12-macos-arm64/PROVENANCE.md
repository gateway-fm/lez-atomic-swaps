# Provenance — full regression of the release candidate on macOS arm64, 2026-11-12

Everything in this folder was produced on one Mac by the Docker devnet of
`deploy/`: every test of `scripts/regress.sh full` (97: `regress.sh list
full`), the Node scenarios by `scripts/node-e2e.py` and the desk journeys by
`scripts/ui-e2e.sh`, three at a time on three regression lanes. It is the
macOS counterpart of `docs/evidence/regression-2026-11-12/` (the same 97
tests on Linux x86_64). The earlier Mac run of `main`'s code, 93 tests, is
`docs/evidence/regression-m2-2026-11-12-macos-arm64/`.

Nothing here is private. The ZEC exports carry swap views, durable actor
status, zebra heights and transaction ids; the BTC summaries carry timing,
swap ids and claim transaction ids; the logs carry step lines, captions and
the stack's start-up lines. No key, cookie, capability file, `runtime/` or
`market/` content is included. Local paths in the logs are replaced by
`<checkout>`, `<workspace>`, `<home>` and `<scratch>`. Scanned before commit
for key, cookie, password and preimage patterns; the one password in the
logs is the devnet's documented VNC password (`deploy/README.md`).

## Source

Tests ran from `f2f9b980` (branch `docs/release-0.3.0-gates`, PR #127), tree
clean: `logs/regress.log` prints the commit and "tree dirty: 0" at its start,
and `logs/run-summary.log` the same before anything was torn down.

`f2f9b980` is `main` at `af3ac978` plus the three stacked pull requests:

- PR #131 (`fix/103-review-items`): the #103 work. The harness for items 8,
  9, 10, 12 and 14 (the new `late-claim-confirms`, `late-claim-unmined`,
  `early-lock`, `offline-after-lock` and `refund-then-maker-lock`
  assertions); `last_outcome` and `waiting_for` in both Nodes' swap views and
  the desks; the Maker supervisor's refund fix (`9579950b`), the Bitcoin
  actor's late LEZ claim lookup (`48c8b5b6`, `964cb8c8`), the sidecar's
  discovery by finalized locks (`17cfa50a`), and the harness fixes up to
  `62cf95ca`. PR #131 has one later commit, `2e2db2c5` (a unit test of the
  regress runner, `deploy/tests/` only), that is not in `f2f9b980`.
- PR #126 (`feat/darwin-native-packages`): the Node, the store, the BTC and
  ZEC actors, the sidecar and the Node supervisor build for macOS
  (`f892f20c`, `67e94a28`, `c5226692`).
- PR #127 (this branch): `deploy/` script, Compose and Dockerfile fixes
  (`2ab39a39`, `cbeba3a2`, `af39fd9f`, `b7c84cfe`, `adffc106`), a host-neutral
  fix of two deploy unit tests (`f2f9b980`), and documentation.

Unlike the earlier Mac set, crates, sidecar and desks differ from `main`:
outside `docs/`, `git diff --stat af3ac978 f2f9b980` changes 92 files (6,100
insertions, 694 deletions) in `crates/` (actor-supervision,
btc-reference-actor, maker-node, node-common, swap-store, taker-node,
xmr-reference-actor, xmr-release-authority, zec-reference-actor),
`compat/lez-v0_2-sidecar`, `apps/basecamp`, `modules/lez-swap-node`,
`deploy/` and `Cargo.lock`.

## Images

| image | id | created (UTC) | this run |
|---|---|---|---|
| `lez-maker-node:local` | `sha256:40a5ab0c7414375119bc0a22e4f85cfde0388a48967500ab69bd7d0ac81089c9` | 2026-11-12 06:49 | built |
| `lez-taker-node:local` | `sha256:e19cc0cf80012edd5e0072a7ca8852da90f6c2148014ea8971e458e9344d271d` | 2026-11-12 06:49 | built |
| `lez-basecamp-ui:local` | `sha256:4a3cabddb23dd497804de8c3abaffbb116031317b9b5d8c7d144b342722c673e` | 2026-11-12 06:36 | cache |
| `lez-network-module:local` | `sha256:d031fc1d084757df962cd79646b67bad9021ef62c18a7ed0f49322d7e347c6e4` | 2026-11-12 06:36 | cache |
| `lez-services:local` (sequencer) | `sha256:3d27929115178d3d0b0539261385b71519abb7209d86b12fa87a0ac287f0881d` | 2026-11-11 19:12 | cache |
| `lez-zebra:local` (Zebra 7.0.0) | `sha256:45f432d2e46059e96efc610bd9b3126b12fce0ea5df92a1e0e68dd0935bba77b` | 2026-11-11 19:12 | cache |
| `lez-bitcoin-core:local` | `sha256:0eff51035e1f6508b2600be2fab559068d06957cdd49ebf0862694861aab9443` | 2026-11-11 19:12 | cache |
| `lez-btc-miner:local` | `sha256:db9152efc62ff95bbd3d4a09d9142be7026095cc98f879c30e9fce59f6c436c7` | 2026-11-11 19:12 | cache |

Ids and creation times read with `docker image inspect` after the run; they
match the ids `up.sh --build` wrote during `from-scratch.sh`
(`logs/from-scratch.log`). All are local `linux/arm64` builds of this host;
nothing was pulled from a registry and no release was built from this
commit.

`from-scratch.sh --workspace <workspace>` ran from 06:47Z to 06:51Z and
staged every payload from `f2f9b980`: the Nix-built Basecamp bundle and
release packages and the Node binaries were current in their caches, the
sidecar was recompiled. Its stack phase ran `up.sh --build`. The two Node
images were built anew. Every layer of the other six came from the build
cache, so their content is what this run staged: the Basecamp and network
images keep the ids of a build at 06:36Z that day, and the sequencer,
Zebra, Bitcoin Core and miner images keep the ids they carry in the earlier
Mac set. The network image differs from the earlier Mac set's
(`sha256:1cb8b50c…`).

Each Node image carries its build source in
`/usr/local/share/lez/build-source.txt` and its sidecar source in
`sidecar-source.txt`: `f2f9b980478a161df77f59b6b61ac292abb3d4e6` for both
(read again after the run). `logs/run-summary.log` records the build source
of both images right after the build, and the build source and installed
package source of all six Node containers (three lanes) before the first
test: `f2f9b980` everywhere. The wrapper refuses to start the run otherwise.

## The run

- Host: MacBook Pro, Apple M1 Max, 10 cores, 64 GB; macOS 26.7.3; Docker
  Desktop, engine 29.9.2, 10 CPUs and 32 GB given to its VM.
- Beside the run, from about 07:07Z to 07:45Z, a Nix build of `lez_core` for
  darwin ran on the host, limited to two cores; the host's load average was
  about 13 to 16 while it ran. The host was not idle for that part of the
  run. Nothing else ran on the stack.
- In order (`logs/run-summary.log`): `lanes.sh down 3 --wipe` and `down.sh
  --wipe` at 06:46Z; `from-scratch.sh` (06:47Z to 06:51Z); `lanes.sh mint
  3`; `up.sh --fresh-lez` with LEZ_LANES=3 (`logs/fresh-lez.log`, done at
  06:53Z), which made the chain the run started on; `regress.sh smoke`, 6 of
  6 in 3 min (`logs/smoke-regress.log`, `logs/smoke-results.tsv`); then
  `LEZ_REGRESS_RECORD=1 LEZ_LANES=3 scripts/regress.sh full` from 06:56:41Z
  to 08:35:16Z (98 min).
- Lanes: lane 1 is the stack; lanes 2 and 3 are one more Maker/Taker pair
  each with their own Bitcoin Core, miner and Zebra, on the stack's one LEZ
  network. `results.tsv` names the lane of every test (28, 32 and 35 on
  lanes 1, 2 and 3; the two balance-refusal tests ran on lane 1 alone).
- LEZ chains: the starting chain served every test but two;
  `zec-balance-refusal` in each direction starts a chain of its own (the
  last two tests, `logs/chain-1.log` and `logs/chain-2.log`). No chain was
  replaced for a finality freeze: 0 frozen tries. `chain.tsv` samples the
  finalized height and the tip every 30 s; the widest gap between them was
  19 blocks.
- Timing profiles: `fast` from the start, `fast` -> `refund` at 08:11:13Z
  (`logs/profile-1.log`); the run ended on `refund`.
- Swaps are reset between tests by `reset-swaps.sh --abandon-open` on the
  test's lane (`logs/regress.log`).

## Recordings

Every desk test was recorded on its lane's two Basecamp displays
(`ui-e2e.sh --record`); a recording is a by-product of the test that passed.
32 recordings were made; the two Bitcoin `node-offline` desk tests have no
recorded desk step and produced none. Seven are in `videos/`, the same ZEC
set as the other two sets: `zec-happy`, `zec-maker-refund` and
`zec-concurrent` in both directions and `zec-stale-window` selling ZEC
(10.5 MB together; the earlier Mac set's seven are 10.4 MB, the Linux set's
13.2 MB). All 32 are kept on the host in the run's directory
(`deploy/runtime/regress/20261112T065641Z/videos/`), not in git;
`videos-kept-outside-git.sha256` lists them, these seven among them.

## Not covered by this set

- Any public network: see `docs/m2-acceptance-checklist.md`.
- A released bundle: the images are local builds; no release was built from
  this commit.
- macOS on Intel, and Linux arm64 outside Docker Desktop's VM.
- Native darwin Nodes: the Nodes ran as `linux/arm64` containers in Docker
  Desktop's VM, not as the darwin packages PR #126 builds.
- `2e2db2c5`, the last commit of PR #131: a unit test of the regress
  runner, not part of this run's source.
