# Provenance — full regression of `0a42823f` on macOS arm64, 2026-11-16

Everything in this folder was produced on one Mac by the Docker devnet of
`deploy/`: every test of `scripts/regress.sh full` (97: `regress.sh list
full`), the Node scenarios by `scripts/node-e2e.py` and the desk journeys by
`scripts/ui-e2e.sh`, three at a time on three regression lanes. It is the run
on `0a42823f`: the fix for #156 (`f4e5fe02`) and the version 0.3.3
(`0a42823f`) on top of `main` after PR #155. The run on `961b0fe9` two days
before is `docs/evidence/regression-0.3.2-2026-11-14-macos-arm64/`, and is
the comparison in `RESULTS.txt`.

This folder is committed on top of `0a42823f` in a docs-only commit; the
tested commit is `0a42823f`, not the commit that adds this folder.

Nothing here is private. The ZEC exports carry swap views, durable actor
status, zebra heights and transaction ids; the BTC summaries carry timing,
swap ids and claim transaction ids; the logs carry step lines, captions and
the stack's start-up lines. No key, cookie, capability file, `runtime/` or
`market/` content is included. Local paths in the logs are replaced by
`<checkout>`, `<workspace>`, `<home>` and `<scratch>`. Scanned before commit
for local paths, user and host names, and key, cookie, password and preimage
patterns; the one password in the logs is the devnet's documented VNC
password (`deploy/README.md`).

## Source

Tests ran from `0a42823f`, tree clean: `logs/regress.log` prints the commit
and "tree dirty: 0" at its start, and `logs/run-summary.log` the same before
anything was torn down.

`git log --first-parent 961b0fe9..0a42823f` lists the merges of PRs #154
(release 0.3.2: version strings and docs) and #155 (the 0.3.2 regression
evidence), then two commits:

- `f4e5fe02` fixes #156. The Bitcoin adapter's pre-send readiness check
  (`crates/btc-core-adapter`) reads the tip and the txindex again, up to
  three times 200 ms apart, when a synced txindex is at another height than
  the tip just read or a header is ahead of its block; the exact-height rule
  is unchanged. A readiness refusal before anything was offered to Bitcoin
  Core is reported as not attempted. The swap store
  (`crates/swap-store`) returns such a started send or Maker lock step to
  prepared, and the actor (`crates/btc-reference-actor`) gives the send
  authority back for the Bitcoin claim, the Bitcoin refunds and the Maker's
  Bitcoin lock, so the next drive sends once. The Taker Node
  (`crates/taker-node`) keeps a failed Bitcoin claim's admission and drives
  it again while the status still offers the claim.
- `0a42823f` sets every shipped component to 0.3.3 and adds to `AGENTS.md`
  that the version bump and the regression evidence go into the change's
  pull request.

This is the first full regression of the #156 fix. Every Bitcoin test sends
through the readiness check; the per-test logs do not show whether a re-read
or a second send happened, since the Nodes' own logs are not part of this
set.

## Packages

`logs/from-scratch.log` prints the version of every package it staged. This
project's components read 0.3.3: `lez_maker_node` and `lez_taker_node`
(packaged `linux-arm64` for the Node images and for the Basecamp image),
`lez_atomic_swap_maker`, `lez_atomic_swap_taker`, `lez_network` and
`lez_swap_node`. The sidecar's crate reads 0.3.3 at `0a42823f`; its build
line is not in the log (the build prints only its last two lines), and the
Node images carry `0a42823f` as the sidecar's source
(`/usr/local/share/lez/sidecar-source.txt`), which the image build checks
against the checkout's sidecar tree. Upstream components keep their own
versions: `lez_core` 0.4.1-sign.4, `blockchain_module` 0.2.4,
`lez_indexer_module` 1.1.1-rpc.4, `chat_module` 0.2.2, `delivery_module`
0.2.0.

## Images

| image | id | this run |
|---|---|---|
| `lez-maker-node:local` | `sha256:788b2f327bc7919cd2716b8608bbdd071e4dc0efe239886e76bf300898befe2c` | built, 2026-11-16 11:52Z |
| `lez-taker-node:local` | `sha256:e1b416c612731214aeabbf42b95d24a363aa24525e436b8e807594c2edb2baf1` | built, 2026-11-16 11:52Z |
| `lez-basecamp-ui:local` | `sha256:078add252c7a42d1a6a1ad2cf256d11f7a55517bc5ec6d76957c06e2beeea765` | built, 2026-11-16 11:52Z |
| `lez-network-module:local` | `sha256:51547af67163b8cb10545a9f66ccd6daeeb964528346774fa6d1c0981f51d648` | built, 2026-11-16 11:52Z |
| `lez-services:local` (sequencer) | `sha256:3d27929115178d3d0b0539261385b71519abb7209d86b12fa87a0ac287f0881d` | cache |
| `lez-zebra:local` (Zebra 7.0.0) | `sha256:45f432d2e46059e96efc610bd9b3126b12fce0ea5df92a1e0e68dd0935bba77b` | cache |
| `lez-bitcoin-core:local` | `sha256:0eff51035e1f6508b2600be2fab559068d06957cdd49ebf0862694861aab9443` | cache |
| `lez-btc-miner:local` | `sha256:db9152efc62ff95bbd3d4a09d9142be7026095cc98f879c30e9fce59f6c436c7` | cache |

Ids are the ones `up.sh --build` wrote during `from-scratch.sh`
(`logs/from-scratch.log`); the Node images' ids and creation times are also
in `logs/run-summary.log`. All are local `linux/arm64` builds of this host;
nothing was pulled from a registry and no release was built from this
commit.

`from-scratch.sh --workspace <workspace>` ran from 11:47:06Z to 11:54:53Z
and staged every payload from `0a42823f`. Its stack phase ran `up.sh
--build`. The Basecamp and network images' base layers came from the build
cache; the Node images ran their `apt-get install` step again. The
sequencer, Zebra, Bitcoin Core and miner images came from the build cache
and keep the ids they carry in the 0.3.2 set.

Each Node image carries its build source in
`/usr/local/share/lez/build-source.txt`. `logs/run-summary.log` records the
build source of both images right after the build, and the build source and
installed package source of all six Node containers (three lanes) before
the smoke: `0a42823f6526899a5de06c130f8ecd311d889b70` everywhere. The
wrapper stops before the run when either image reports another commit.

## The run

- Host: MacBook Pro, Apple M1 Max, 10 cores, 64 GB; macOS 26.7.3; Docker
  Desktop, engine 29.9.2, 10 CPUs and 32 GB given to its VM. The same host
  as the 0.3.2 run.
- In order (`logs/run-summary.log`): `lanes.sh down 3 --wipe` and `down.sh
  --wipe` (11:46Z to 11:47Z); `from-scratch.sh` (11:47Z to 11:54Z);
  `lanes.sh mint 3`; `up.sh --fresh-lez` with LEZ_LANES=3
  (`logs/fresh-lez.log`, done at 11:56Z), which made the chain the run
  started on; `regress.sh smoke`, 6 of 6 in 3 min
  (`logs/smoke-regress.log`, `logs/smoke-results.tsv`); then
  `LEZ_REGRESS_RECORD=1 LEZ_LANES=3 scripts/regress.sh full` from 12:00:51Z
  to 13:39:30Z (98 min).
- Lanes: lane 1 is the stack; lanes 2 and 3 are one more Maker/Taker pair
  each with their own Bitcoin Core, miner and Zebra, on the stack's one LEZ
  network. `results.tsv` names the lane of every test (30, 35 and 30 on
  lanes 1, 2 and 3; the two balance-refusal tests ran on lane 1 alone).
- LEZ chains: the starting chain served every test but two;
  `zec-balance-refusal` in each direction starts a chain of its own (the
  last two tests, `logs/chain-1.log` and `logs/chain-2.log`). No chain was
  replaced for a finality freeze: 0 frozen tries. `chain.tsv` samples the
  finalized height and the tip every 30 s; the widest gap between them was
  14 blocks on the starting chain and 18 on the balance-refusal chains.
- Timing profiles: `fast` from the start, `fast` -> `refund` at 13:15:45Z
  (`logs/profile-1.log`); the run ended on `refund`.
- No test failed and none was rerun: every test passed on its first try.
- Before each test that does not start a chain of its own, the runner
  resets the lane's swaps with `reset-swaps.sh --abandon-open`; those
  per-lane reset logs are not in this set.
- The per-test logs (`logs/node-*.log`, `logs/ui-*.log`) print the host's
  local time (CET, UTC+1), as do the `docker compose` warnings (`time=…+01:00`)
  in the start-up, chain and profile logs. The runner's and the wrapper's
  logs, the start-up step lines, `results.tsv` and `chain.tsv` are in UTC.
- The `docker compose` warnings are the same ten volume-label warnings in
  each of `logs/fresh-lez.log`, `logs/chain-1.log`, `logs/chain-2.log` and
  `logs/profile-1.log` as in the 0.3.2 set.

## Recordings

Every desk test was recorded on its lane's two Basecamp displays
(`ui-e2e.sh --record`); a recording is a by-product of the test that passed.
32 recordings were made; the two Bitcoin `node-offline` desk tests have no
recorded desk step and produced none. Seven are in `videos/`, the same ZEC
set as the other sets: `zec-happy`, `zec-maker-refund` and `zec-concurrent`
in both directions and `zec-stale-window` selling ZEC. All 32 are kept on the
host, not in git, in `<home>/tmp/lez-regress-pr157-0a42823f/videos/`;
`videos-kept-outside-git.sha256` lists them, these seven among them.

## Not covered by this set

- Latency to the chains: the chains ran on the same host as the Nodes, so a
  slow link, which widens the window #156 is about, is not exercised here.
- Any public network: see `docs/m2-acceptance-checklist.md`.
- Linux x86_64: no regression of this commit ran there.
- A released bundle or the package install path: the images are local
  builds, and the Nodes and desks ran from packages staged into the images,
  not installed from a release bundle or a module catalog.
- macOS on Intel, and Linux arm64 outside Docker Desktop's VM.
- Native darwin Nodes: the Nodes ran as `linux/arm64` containers in Docker
  Desktop's VM.
