# Provenance — full regression of the v0.3.0 candidate on macOS arm64, 2026-11-12

Everything in this folder was produced on one Mac by the Docker devnet of
`deploy/`: every test of `scripts/regress.sh full` (97: `regress.sh list
full`), the Node scenarios by `scripts/node-e2e.py` and the desk journeys by
`scripts/ui-e2e.sh`, three at a time on three regression lanes. It is the run
on the v0.3.0 candidate, `main` at `b4e17cf0`. The earlier Mac run of the
same 97 tests, at `f2f9b980`, is `docs/evidence/regression-2026-11-12-macos-arm64/`.

Nothing here is private. The ZEC exports carry swap views, durable actor
status, zebra heights and transaction ids; the BTC summaries carry timing,
swap ids and claim transaction ids; the logs carry step lines, captions and
the stack's start-up lines. No key, cookie, capability file, `runtime/` or
`market/` content is included. Local paths in the logs are replaced by
`<checkout>`, `<workspace>`, `<home>` and `<scratch>`. Scanned before commit
for key, cookie, password and preimage patterns; the one password in the
logs is the devnet's documented VNC password (`deploy/README.md`).

## Source

Tests ran from `b4e17cf0` (`main`, the merge of PR #135), tree clean:
`logs/regress.log` prints the commit and "tree dirty: 0" at its start, and
`logs/run-summary.log` the same before anything was torn down.

Against `f2f9b980`, the commit of the earlier Mac set, `git diff --stat
f2f9b980 b4e17cf0` outside `docs/` changes 24 files (171 insertions, 33
deletions), and nothing in `crates/`, `deploy/scripts/`, the Compose files or
the Dockerfiles:

- The version alignment to 0.3.0 (`378289b6`): both desks and `lez_network`
  (`apps/basecamp/*/metadata.json` and the `.lidl` files that name the
  Nodes), the role packages and `lez_swap_node` (`modules/*/metadata.json`
  and their `.lidl` files; `lez_swap_node` was 0.3.1), the sidecar
  (`compat/lez-v0_2-sidecar/Cargo.toml` and `Cargo.lock`, was 0.0.0), and the
  version the package-contract check expects.
- The darwin Metal patch for the LEZ module flake (`33f40565`,
  `deploy/builder/patches/`): it changes the macOS-only Metal stub of
  `lez_core`'s flake; the run's packages are `linux-arm64`.
- The regress runner's unit-test fake (`2e2db2c5`,
  `deploy/tests/test_regress_lanes.py`), the commit the earlier Mac set names
  as not part of its source.
- Documentation (`README.md`, `apps/basecamp/README.md`, `deploy/README.md`,
  `deploy/builder/patches/README.md`, `submission/`), `AGENTS.md` and
  `.gitattributes`.

`git log --oneline f2f9b980..b4e17cf0` lists these commits, the merges of
PRs #131, #126, #127 and #135, and the PR #126 commits whose content
`f2f9b980` already carried.

## Packages

`logs/from-scratch.log` prints the version of every package it staged. This
project's components read 0.3.0: `lez_maker_node` and `lez_taker_node`
(packaged `linux-arm64` for the Node images and for the Basecamp image),
`lez_atomic_swap_maker`, `lez_atomic_swap_taker`, `lez_network` and
`lez_swap_node`; the sidecar compiled as `lez-v0-2-sidecar v0.3.0`. Upstream
components keep their own versions: `lez_core` 0.4.1-sign.4,
`blockchain_module` 0.2.4, `lez_indexer_module` 1.1.1-rpc.4, `chat_module`
0.2.2, `delivery_module` 0.2.0.

## Images

| image | id | created (UTC) | this run |
|---|---|---|---|
| `lez-maker-node:local` | `sha256:34b9eca21e644b5713e95e56c2b9cce060fe9bdaf8f560ee6332eda7ea30d3dd` | 2026-11-12 09:56 | built |
| `lez-taker-node:local` | `sha256:5da502c3a46af2abec52cb7eeadddbbbf07b35888e39c6143881580bceeae685` | 2026-11-12 09:56 | built |
| `lez-basecamp-ui:local` | `sha256:0b9e3bbc584397f258bbcf4031ba620034c8242b21c4e1bfff14c36eb87b560f` | 2026-11-12 09:43 | cache |
| `lez-network-module:local` | `sha256:5d7d3c397b7c0d0079517b0ea0de638a38ace86974e76cca40263767789da380` | 2026-11-12 09:43 | cache |
| `lez-services:local` (sequencer) | `sha256:3d27929115178d3d0b0539261385b71519abb7209d86b12fa87a0ac287f0881d` | 2026-11-11 19:12 | cache |
| `lez-zebra:local` (Zebra 7.0.0) | `sha256:45f432d2e46059e96efc610bd9b3126b12fce0ea5df92a1e0e68dd0935bba77b` | 2026-11-11 19:12 | cache |
| `lez-bitcoin-core:local` | `sha256:0eff51035e1f6508b2600be2fab559068d06957cdd49ebf0862694861aab9443` | 2026-11-11 19:12 | cache |
| `lez-btc-miner:local` | `sha256:db9152efc62ff95bbd3d4a09d9142be7026095cc98f879c30e9fce59f6c436c7` | 2026-11-11 19:12 | cache |

Ids and creation times read with `docker image inspect` right after the run;
they match the ids `up.sh --build` wrote during `from-scratch.sh`
(`logs/from-scratch.log`). All are local `linux/arm64` builds of this host;
nothing was pulled from a registry and no release was built from this
commit.

`from-scratch.sh --workspace <workspace>` ran from 09:53Z to 09:58Z and
staged every payload from `b4e17cf0`: the Nix-built Basecamp bundle and
release packages and the Node binaries were current in their caches, the
sidecar was recompiled. Its stack phase ran `up.sh --build`. The two Node
images were built anew, as in the earlier Mac set. Every layer of the other
six came from the build cache, so their content is what this run staged.
The Basecamp and network images keep the ids of a build at 09:43Z that day
and differ from the earlier Mac set's (`sha256:4a3cabdd…` and
`sha256:d031fc1d…`). The sequencer, Zebra, Bitcoin Core and miner images
keep the ids they carry in the earlier Mac set.

Each Node image carries its build source in
`/usr/local/share/lez/build-source.txt`. `logs/run-summary.log` records the
build source of both images right after the build, and the build source and
installed package source of all six Node containers (three lanes) before
the first test: `b4e17cf0e3ddc559827f91d891d293e243e0576b` everywhere. The
wrapper stops before the run when either image reports another commit.

## The run

- Host: MacBook Pro, Apple M1 Max, 10 cores, 64 GB; macOS 26.7.3; Docker
  Desktop, engine 29.9.2, 10 CPUs and 32 GB given to its VM.
- Beside the run: no build and no other container workload. Between about
  11:15Z and 11:30Z the unit tests of `deploy/tests` ran a handful of times
  on the host, with a fake `docker`, about 90 s each; other sessions edited
  files on the host. No test failed or was retried in that span.
- In order (`logs/run-summary.log`): `lanes.sh down 3 --wipe` and `down.sh
  --wipe` (09:53Z); `from-scratch.sh` (09:53Z to 09:58Z); `lanes.sh mint 3`;
  `up.sh --fresh-lez` with LEZ_LANES=3 (`logs/fresh-lez.log`, done at
  10:00Z), which made the chain the run started on; `regress.sh smoke`, 6 of
  6 in 2 min (`logs/smoke-regress.log`, `logs/smoke-results.tsv`); then
  `LEZ_REGRESS_RECORD=1 LEZ_LANES=3 scripts/regress.sh full` from 10:03:02Z
  to 11:40:22Z (97 min).
- Lanes: lane 1 is the stack; lanes 2 and 3 are one more Maker/Taker pair
  each with their own Bitcoin Core, miner and Zebra, on the stack's one LEZ
  network. `results.tsv` names the lane of every test (33, 33 and 29 on
  lanes 1, 2 and 3; the two balance-refusal tests ran on lane 1 alone).
- LEZ chains: the starting chain served every test but two;
  `zec-balance-refusal` in each direction starts a chain of its own (the
  last two tests, `logs/chain-1.log` and `logs/chain-2.log`). No chain was
  replaced for a finality freeze: 0 frozen tries. `chain.tsv` samples the
  finalized height and the tip every 30 s; the widest gap between them was
  12 blocks on the starting chain and 18 on the balance-refusal chains.
- Timing profiles: `fast` from the start, `fast` -> `refund` at 11:16:17Z
  (`logs/profile-1.log`); the run ended on `refund`.
- Before each test that does not start a chain of its own, the runner
  resets the lane's swaps with `reset-swaps.sh --abandon-open`; those
  per-lane reset logs are not in this set.
- The per-test logs (`logs/node-*.log`, `logs/ui-*.log`) print the host's
  local time (CET, UTC+1), as do the `docker compose` warnings (`time=…+01:00`)
  in the start-up and chain logs. The runner's and the wrapper's logs, the
  start-up step lines, `results.tsv` and `chain.tsv` are in UTC.

## Recordings

Every desk test was recorded on its lane's two Basecamp displays
(`ui-e2e.sh --record`); a recording is a by-product of the test that passed.
32 recordings were made; the two Bitcoin `node-offline` desk tests have no
recorded desk step and produced none. Seven are in `videos/`, the same ZEC
set as the other sets: `zec-happy`, `zec-maker-refund` and `zec-concurrent`
in both directions and `zec-stale-window` selling ZEC (10.3 MB together; the
earlier Mac set's seven are 10.5 MB). All 32 are kept on the host for the
release attachments, not in git, in
`<scratch>/gate6/results/20261112T100302Z/videos/`;
`videos-kept-outside-git.sha256` lists them, these seven among them.

## Not covered by this set

- Any public network: see `docs/m2-acceptance-checklist.md`.
- A released bundle: the images are local builds; no release was built from
  this commit. The release workflow's rehearsal on `b6b1c273` (run
  43189324839) built every image and failed in its smoke job, when the
  bundle's `start.sh` tried to build an image from a folder the bundle does
  not ship. The fix is PR #136; it changes deploy scripts used on a bundle
  and is not in `b4e17cf0`.
- macOS on Intel, and Linux arm64 outside Docker Desktop's VM.
- Native darwin Nodes: the Nodes ran as `linux/arm64` containers in Docker
  Desktop's VM, not as the darwin packages PR #126 builds.
