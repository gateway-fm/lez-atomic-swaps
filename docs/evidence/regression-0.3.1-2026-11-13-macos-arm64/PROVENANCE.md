# Provenance — full regression for v0.3.1 on macOS arm64, 2026-11-13

Everything in this folder was produced on one Mac by the Docker devnet of
`deploy/`: every test of `scripts/regress.sh full` (97: `regress.sh list
full`), the Node scenarios by `scripts/node-e2e.py` and the desk journeys by
`scripts/ui-e2e.sh`, three at a time on three regression lanes. It is the run
on `538530eb`, `main` after PR #143, the code of release v0.3.1. The run on
the v0.3.0 candidate, at `b4e17cf0`, is
`docs/evidence/regression-0.3.0-2026-11-12-macos-arm64/`.

Nothing here is private. The ZEC exports carry swap views, durable actor
status, zebra heights and transaction ids; the BTC summaries carry timing,
swap ids and claim transaction ids; the logs carry step lines, captions and
the stack's start-up lines. No key, cookie, capability file, `runtime/` or
`market/` content is included. Local paths in the logs are replaced by
`<checkout>`, `<workspace>`, `<home>` and `<scratch>`. Scanned before commit
with gitleaks (the repository's `.gitleaks.toml`, no leaks found) and for
key, cookie, password and preimage patterns; the one password in the logs is
the devnet's documented VNC password (`deploy/README.md`).

## Source

Tests ran from `538530eb` (`main`, the merge of PR #143), tree clean:
`logs/regress.log` prints the commit and "tree dirty: 0" at its start, and
`logs/run-summary.log` the same before anything was torn down.

The release commit `8ce16323` (`main`, the merge of PR #144) is not the
tested commit. `git diff --stat 538530eb 8ce16323` changes 34 files (115
insertions, 219 deletions), and nothing in `crates/`, `deploy/scripts/`,
`deploy/images/`, the Compose files or the Dockerfiles:

- The version alignment to 0.3.1 (`b5c4f44e`): both desks and `lez_network`
  (`apps/basecamp/*/metadata.json` and the three `.lidl` files in
  `apps/basecamp/common/deps/`), the role packages and `lez_swap_node`
  (`modules/*/metadata.json` and the two `lez_swap_node.lidl` files), the
  sidecar (`compat/lez-v0_2-sidecar/Cargo.toml` and `Cargo.lock`), and the
  versions two checks expect (`scripts/check-m6-basecamp-package-contract.mjs`,
  `deploy/tests/test_release_bundle.py`).
- Eight advisory exemptions (`2f620981`): RUSTSEC-2026-0525, -0530, -0531,
  -0534, -0535, -0536, -0537 and -0538, added to the four `deny.toml` files
  under `compat/` (the provisional fixture, its escrow deployer, the
  sequencer reproducers and the sidecar), to the lists
  `scripts/verify-lez-v02-provisional.sh` checks, and to the boundary
  `compat/lez-v0_2-sidecar/check-dependency-policy.sh` greps for (`quinn`
  added).
- Documentation (`README.md`, `CONTRIBUTING.md`, `AGENTS.md`,
  `apps/basecamp/README.md`, `compat/lez-v0.2-provisional/README.md`,
  `deploy/README.md`, `docs/issue-closing-notes-2026-11-11.md`,
  `docs/m2-acceptance-checklist.md`, `docs/runtime-build-isolation.md`,
  `docs/submission-115.md`, `submission/`), and `docs/release-notes-0.3.0.md`
  removed.

So the run's packages read 0.3.0, not 0.3.1 (below). Against `35bd85de`, the
commit of the first run (below), `538530eb` changes only
`crates/swap-store/src/maker_actor_process.rs` and
`crates/swap-store/tests/maker_actor_manual_action.rs` (PR #143). Against
`b4e17cf0`, the commit of the v0.3.0 set, `git log --first-parent
b4e17cf0..538530eb` lists the merges of PRs #136, #138, #140, #137 and #143;
in `crates/` only PR #143 changes anything.

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

| image | id | this run |
|---|---|---|
| `lez-maker-node:local` | `sha256:2080bfd7445aa242b434705b28869c1ceeb55f1fc95e4bac9b9ed54e27e7d39b` | built, 2026-11-13 12:46Z |
| `lez-taker-node:local` | `sha256:ce1df45aaa597eb84d003515fd8660a3a54347846dba9bd96cf17ccb8b9a107f` | built, 2026-11-13 12:46Z |
| `lez-basecamp-ui:local` | `sha256:ca840471a2b553f1477b3da1fb0b2ba34302360321ad0ba2c6de2469ac07db07` | cache |
| `lez-network-module:local` | `sha256:4ec3faaa6724268fd07cfe73e764ad6fda0d0e3da05d462842b65d9da4784875` | cache |
| `lez-services:local` (sequencer) | `sha256:3d27929115178d3d0b0539261385b71519abb7209d86b12fa87a0ac287f0881d` | cache |
| `lez-zebra:local` (Zebra 7.0.0) | `sha256:45f432d2e46059e96efc610bd9b3126b12fce0ea5df92a1e0e68dd0935bba77b` | cache |
| `lez-bitcoin-core:local` | `sha256:0eff51035e1f6508b2600be2fab559068d06957cdd49ebf0862694861aab9443` | cache |
| `lez-btc-miner:local` | `sha256:db9152efc62ff95bbd3d4a09d9142be7026095cc98f879c30e9fce59f6c436c7` | cache |

Ids are the ones `up.sh --build` wrote during `from-scratch.sh`
(`logs/from-scratch.log`); the Node images' ids and creation times are also
in `logs/run-summary.log`. All are local `linux/arm64` builds of this host;
nothing was pulled from a registry and no release was built from this
commit.

`from-scratch.sh --workspace <workspace>` ran from 12:43Z to 12:48Z and
staged every payload from `538530eb`. Its stack phase ran `up.sh --build`.
The two Node images were built anew: the steps that copy in the Node package
and the build source ran, the base layers came from the build cache. Every
layer of the other six came from the build cache, so their content is what
this run staged. The Basecamp and network images keep the ids of a build on
this host on the evening of 2026-11-12 and differ from the v0.3.0 set's
(`sha256:0b9e3bbc…` and `sha256:5d7d3c39…`). The sequencer, Zebra, Bitcoin
Core and miner images keep the ids they carry in the v0.3.0 set.

Each Node image carries its build source in
`/usr/local/share/lez/build-source.txt`. `logs/run-summary.log` records the
build source of both images right after the build, and the build source and
installed package source of all six Node containers (three lanes) before
the smoke: `538530eb3500e1c55b1fb1e40f1f6bebd10315a1` everywhere. The
wrapper stops before the run when either image reports another commit.

## The run

- Host: MacBook Pro, Apple M1 Max, 10 cores, 64 GB; macOS 26.7.3; Docker
  Desktop, engine 29.9.2, 10 CPUs and 32 GB given to its VM.
- Beside the run: between about 12:50Z and 13:12Z, during the smoke and the first 18 minutes of the full run, cargo
  builds of the sidecar and a `cargo-deny` install ran on the host for the
  advisory work of PR #144. No test failed or was retried in that span.
- In order (`logs/run-summary.log`): `lanes.sh down 3 --wipe` and `down.sh
  --wipe` (12:43Z); `from-scratch.sh` (12:43Z to 12:48Z); `lanes.sh mint 3`;
  `up.sh --fresh-lez` with LEZ_LANES=3 (`logs/fresh-lez.log`, done at
  12:50Z), which made the chain the run started on; `regress.sh smoke`, 6 of
  6 in 4 min (`logs/smoke-regress.log`, `logs/smoke-results.tsv`); then
  `LEZ_REGRESS_RECORD=1 LEZ_LANES=3 scripts/regress.sh full` from 12:54:16Z
  to 14:33:23Z (99 min).
- Lanes: lane 1 is the stack; lanes 2 and 3 are one more Maker/Taker pair
  each with their own Bitcoin Core, miner and Zebra, on the stack's one LEZ
  network. `results.tsv` names the lane of every test (32, 35 and 28 on
  lanes 1, 2 and 3; the two balance-refusal tests ran on lane 1 alone).
- LEZ chains: the starting chain served every test but two;
  `zec-balance-refusal` in each direction starts a chain of its own (the
  last two tests, `logs/chain-1.log` and `logs/chain-2.log`). No chain was
  replaced for a finality freeze: 0 frozen tries. `chain.tsv` samples the
  finalized height and the tip every 30 s; the widest gap between them was
  16 blocks on the starting chain and 24 on the balance-refusal chains.
- Timing profiles: `fast` from the start, `fast` -> `refund` at 14:09:33Z
  (`logs/profile-1.log`); the run ended on `refund`.
- Before each test that does not start a chain of its own, the runner
  resets the lane's swaps with `reset-swaps.sh --abandon-open`; those
  per-lane reset logs are not in this set.
- The per-test logs (`logs/node-*.log`, `logs/ui-*.log`) print the host's
  local time (CET, UTC+1), as do the `docker compose` warnings (`time=…+01:00`)
  in the start-up and chain logs. The runner's and the wrapper's logs, the
  start-up step lines, `results.tsv` and `chain.tsv` are in UTC.

## The first run, and what it found

The same wrapper ran the same 97 tests earlier that day on `35bd85de`
(`main` after PRs #140 and #137), on Node images built from that commit
(all six containers reported it as their build source), 07:51Z to 09:26Z:
96 of 97 in 95 min, 2 new chains, 1 profile switch, 0 frozen tries.

`node:btc:refund-then-maker-lock:TakerSellsLez` failed on lane 3 after
280 s. The test queued a manual Maker refund while the Maker's actor was
still working on the same swap; the Maker Node exited on `CHECK constraint
failed: updated_at >= created_at`, its supervisor restarted it two seconds
later, and the test's next call to the Node, in the same second as the
request, found its socket closed. The actor supervisor resolved the attempt
with the clock reading taken before it, which was older than the manual
action's `created_at`. Issue #142; fixed by PR #143 (`538530eb`). Run again
alone on `35bd85de` at 09:31Z, the test passed, but that rerun did not hit
the race.

Nor did the run in this set: in its try of the same test, as in the rerun,
the Maker had refunded its Bitcoin lock on its own before the test reached
the manual refund, and the log shows no manual refund request. The full
regression therefore does not by itself exercise the fix; the two store
tests PR #143 added to `crates/swap-store/tests/maker_actor_manual_action.rs`
do.

`first-run/` keeps:

- `results-run1.tsv`, `regress-run1.log` and `summary-run1.log`: the
  runner's table and log and the wrapper's log of the first run;
- `node-btc-refund-then-maker-lock-TakerSellsLez-1.log`: the failed test's
  log (host local time);
- `maker-node-exit.txt`: the Maker Node's lines of the failure, as issue
  #142 quotes them; the container log itself was removed by the wipe that
  preceded this run;
- `results-rerun.tsv` and `summary-rerun.log`: the rerun alone.

The first run's other per-test logs, exports and recordings are kept on the
host, not in git.

## Recordings

Every desk test was recorded on its lane's two Basecamp displays
(`ui-e2e.sh --record`); a recording is a by-product of the test that passed.
32 recordings were made; the two Bitcoin `node-offline` desk tests have no
recorded desk step and produced none. Seven are in `videos/`, the same ZEC
set as the other sets: `zec-happy`, `zec-maker-refund` and `zec-concurrent`
in both directions and `zec-stale-window` selling ZEC (10.4 MB together; the
v0.3.0 set's seven are 10.3 MB). All 32 are kept on the host for the
release attachments, not in git, in `<home>/tmp/lez-regress-main-538530eb/videos/`;
`videos-kept-outside-git.sha256` lists them, these seven among them.

## Not covered by this set

- The release commit `8ce16323` itself: no run was made on it, and the
  packages of this run read 0.3.0. What it changes is listed under Source.
- Any public network: see `docs/m2-acceptance-checklist.md`.
- A released bundle or the package install path: the images are local
  builds, and the Nodes and desks ran from packages staged into the images,
  not installed from a release bundle or a module catalog.
- macOS on Intel, and Linux arm64 outside Docker Desktop's VM.
- Native darwin Nodes: the Nodes ran as `linux/arm64` containers in Docker
  Desktop's VM.
