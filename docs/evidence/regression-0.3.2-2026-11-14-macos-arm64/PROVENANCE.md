# Provenance — full regression of `961b0fe9` on macOS arm64, 2026-11-14

Everything in this folder was produced on one Mac by the Docker devnet of
`deploy/`: every test of `scripts/regress.sh full` (97: `regress.sh list
full`), the Node scenarios by `scripts/node-e2e.py` and the desk journeys by
`scripts/ui-e2e.sh`, three at a time on three regression lanes. It is the run
on `961b0fe9`, `main` after PRs #148 and #153. The run of the day before, on
`538530eb` (the code of v0.3.1), is
`docs/evidence/regression-0.3.1-2026-11-13-macos-arm64/`, and is the
comparison in `RESULTS.txt`.

Nothing here is private. The ZEC exports carry swap views, durable actor
status, zebra heights and transaction ids; the BTC summaries carry timing,
swap ids and claim transaction ids; the logs carry step lines, captions and
the stack's start-up lines. No key, cookie, capability file, `runtime/` or
`market/` content is included. Local paths in the logs are replaced by
`<checkout>`, `<workspace>`, `<home>` and `<scratch>`. Scanned before commit
for key, cookie, password and preimage patterns; the one password in the
logs is the devnet's documented VNC password (`deploy/README.md`). The
repository's `public-readiness` check runs gitleaks with `.gitleaks.toml`
over the history that carries this folder.

## Source

Tests ran from `961b0fe9` (`main`, the merge of PR #153), tree clean:
`logs/regress.log` prints the commit and "tree dirty: 0" at its start, and
`logs/run-summary.log` the same before anything was torn down.

`git log --first-parent 538530eb..961b0fe9` lists the merges of PRs #144
(release 0.3.1: version strings, advisory exemptions, docs), #146 (the
0.3.1 regression evidence), #148 and #153. What the two last change:

- PR #148 (`ae5056a3`) packages Chat for the module catalog as
  `chat_module` 0.2.2-delivery.1, Logos's 0.2.2 with its Delivery
  requirement pinned to `~0.2.0` (`deploy/scripts/package-chat-module.sh`,
  `pin-package-dependencies.sh`, `release-packages.yml`), with its evidence
  `docs/evidence/catalog-chat-module-2026-11-14/`. The Docker stack's
  installed set is unchanged: `logs/from-scratch.log` stages `chat_module`
  0.2.2 and `delivery_module` 0.2.0, as in the 0.3.1 set.
- PR #153 (`961b0fe9`) fixes the four defects found with the chains on
  another host (`docs/evidence/remote-chain-rpc-2026-11-14/`): the ZEC LEZ
  observation is pinned at the tip read before it
  (`crates/zec-swap-sdk/src/lez_observation.rs`, #149); the sidecar's
  finalized observers read their window in batches of 64 blocks
  (`compat/lez-v0_2-sidecar`, #150); the role Node launcher clears group and
  other write on its own package and checks the directories above the
  swaps' state (`modules/lez-role-node/node-entrypoint.sh`, #151); and
  harness fixes in `deploy/scripts/node-e2e.py` and `zec-provision.sh`
  (#152), with the test tool `deploy/scripts/rpc-delay-proxy.py`.

This is the first full regression of the PR #153 code. PR #153's own runs
were tests and a measurement of the finalized window; no swap ran with its
fixes there (`docs/evidence/remote-chain-rpc-2026-11-14/README.md`, "Not
run").

## Packages

`logs/from-scratch.log` prints the version of every package it staged. This
project's components read 0.3.1: `lez_maker_node` and `lez_taker_node`
(packaged `linux-arm64` for the Node images and for the Basecamp image),
`lez_atomic_swap_maker`, `lez_atomic_swap_taker`, `lez_network` and
`lez_swap_node`; the sidecar compiled as `lez-v0-2-sidecar v0.3.1`. The
version alignment to 0.3.2 (PR #154) is not part of `961b0fe9`. Upstream components keep their own versions: `lez_core` 0.4.1-sign.4,
`blockchain_module` 0.2.4, `lez_indexer_module` 1.1.1-rpc.4, `chat_module`
0.2.2, `delivery_module` 0.2.0.

## Images

| image | id | this run |
|---|---|---|
| `lez-maker-node:local` | `sha256:01cf64c1dee5e8d09e2955a81b7f3d803fe5bdaea7ca0b897f14331d4f35767a` | built, 2026-11-14 18:41Z |
| `lez-taker-node:local` | `sha256:f01fe4e182f7bacd8db25ed41ddd3b5828d6e37af50aad5821b92a50dfada457` | built, 2026-11-14 18:41Z |
| `lez-basecamp-ui:local` | `sha256:489f2e8510f89a9879f55ccb2ac23fed9635ac753cae7eb42763512c2609ac9b` | built, 2026-11-14 18:41Z |
| `lez-network-module:local` | `sha256:1dd63decc3fc521730f08fef294242a4d960d66979313f7444e25748ab1be66d` | built, 2026-11-14 18:41Z |
| `lez-services:local` (sequencer) | `sha256:3d27929115178d3d0b0539261385b71519abb7209d86b12fa87a0ac287f0881d` | cache |
| `lez-zebra:local` (Zebra 7.0.0) | `sha256:45f432d2e46059e96efc610bd9b3126b12fce0ea5df92a1e0e68dd0935bba77b` | cache |
| `lez-bitcoin-core:local` | `sha256:0eff51035e1f6508b2600be2fab559068d06957cdd49ebf0862694861aab9443` | cache |
| `lez-btc-miner:local` | `sha256:db9152efc62ff95bbd3d4a09d9142be7026095cc98f879c30e9fce59f6c436c7` | cache |

Ids are the ones `up.sh --build` wrote during `from-scratch.sh`
(`logs/from-scratch.log`); the Node images' ids and creation times are also
in `logs/run-summary.log`. All are local `linux/arm64` builds of this host;
nothing was pulled from a registry and no release was built from this
commit.

`from-scratch.sh --workspace <workspace>` ran from 18:34:45Z to 18:43:52Z
and staged every payload from `961b0fe9`. Its stack phase ran `up.sh
--build`. Unlike the 0.3.1 run, the build cache did not hold the Basecamp
and Node images' base layers, and the `nixos/nix` image the package builds
run in was not on the host: it was pulled while `lez_swap_node` was built,
and the Basecamp, network and both Node images were built from their base
images, the `apt-get install` steps included. The pull's progress lines and
that install are most of `logs/from-scratch.log`. The sequencer, Zebra,
Bitcoin Core and miner images came from the build cache and keep the ids
they carry in the 0.3.1 and 0.3.0 sets.

Each Node image carries its build source in
`/usr/local/share/lez/build-source.txt`. `logs/run-summary.log` records the
build source of both images right after the build, and the build source and
installed package source of all six Node containers (three lanes) before
the smoke: `961b0fe92d0fe4f8122148e866f0def52ebd3eae` everywhere. The
wrapper stops before the run when either image reports another commit.

## The run

- Host: MacBook Pro, Apple M1 Max, 10 cores, 64 GB; macOS 26.7.3; Docker
  Desktop, engine 29.9.2, 10 CPUs and 32 GB given to its VM. The same host
  as the 0.3.1 run.
- In order (`logs/run-summary.log`): `lanes.sh down 3 --wipe` and `down.sh
  --wipe` (18:34Z); `from-scratch.sh` (18:34Z to 18:43Z); `lanes.sh mint 3`;
  `up.sh --fresh-lez` with LEZ_LANES=3 (`logs/fresh-lez.log`, done at
  18:45Z), which made the chain the run started on; `regress.sh smoke`, 6 of
  6 in 3 min (`logs/smoke-regress.log`, `logs/smoke-results.tsv`); then
  `LEZ_REGRESS_RECORD=1 LEZ_LANES=3 scripts/regress.sh full` from 18:49:17Z
  to 20:26:40Z (97 min).
- Lanes: lane 1 is the stack; lanes 2 and 3 are one more Maker/Taker pair
  each with their own Bitcoin Core, miner and Zebra, on the stack's one LEZ
  network. `results.tsv` names the lane of every test (30, 32 and 33 on
  lanes 1, 2 and 3; the two balance-refusal tests ran on lane 1 alone).
- LEZ chains: the starting chain served every test but two;
  `zec-balance-refusal` in each direction starts a chain of its own (the
  last two tests, `logs/chain-1.log` and `logs/chain-2.log`). No chain was
  replaced for a finality freeze: 0 frozen tries. `chain.tsv` samples the
  finalized height and the tip every 30 s; the widest gap between them was
  18 blocks, on the starting chain and on the balance-refusal chains alike.
- Timing profiles: `fast` from the start, `fast` -> `refund` at 20:03:06Z
  (`logs/profile-1.log`); the run ended on `refund`.
- Before each test that does not start a chain of its own, the runner
  resets the lane's swaps with `reset-swaps.sh --abandon-open`; those
  per-lane reset logs are not in this set.
- The per-test logs (`logs/node-*.log`, `logs/ui-*.log`) print the host's
  local time (CET, UTC+1), as do the `docker compose` warnings (`time=…+01:00`)
  in the start-up, chain and profile logs. The runner's and the wrapper's
  logs, the start-up step lines, `results.tsv` and `chain.tsv` are in UTC.
- The `docker compose` warnings are the same ten volume-label warnings in
  each of `logs/fresh-lez.log`, `logs/chain-1.log`, `logs/chain-2.log` and
  `logs/profile-1.log` as in the 0.3.1 set.

## Recordings

Every desk test was recorded on its lane's two Basecamp displays
(`ui-e2e.sh --record`); a recording is a by-product of the test that passed.
32 recordings were made; the two Bitcoin `node-offline` desk tests have no
recorded desk step and produced none. Seven are in `videos/`, the same ZEC
set as the other sets: `zec-happy`, `zec-maker-refund` and `zec-concurrent`
in both directions and `zec-stale-window` selling ZEC (10.2 MB together; the
0.3.1 set's seven are 10.4 MB). All 32 are kept on the host, not in git, in
`<home>/tmp/lez-regress-main-961b0fe9/videos/`;
`videos-kept-outside-git.sha256` lists them, these seven among them.

## Not covered by this set

- Latency to the chains: the chains ran on the same host as the Nodes, so
  the remote-link conditions PR #153 fixes (30–200 ms per RPC) are not
  exercised here. They are the subject of
  `docs/evidence/remote-chain-rpc-2026-11-14/`.
- Any public network: see `docs/m2-acceptance-checklist.md`.
- Linux x86_64: no regression of this commit ran there.
- A released bundle or the package install path: the images are local
  builds, and the Nodes and desks ran from packages staged into the images,
  not installed from a release bundle or a module catalog. Native installs
  from a catalog, with the Chat module of PR #148, are in
  `docs/evidence/catalog-chat-module-2026-11-14/` (on `477bcb06`).
- The version alignment to 0.3.2 (PR #154): it is not in `961b0fe9`, and
  the packages of this run read 0.3.1.
- macOS on Intel, and Linux arm64 outside Docker Desktop's VM.
- Native darwin Nodes: the Nodes ran as `linux/arm64` containers in Docker
  Desktop's VM.
