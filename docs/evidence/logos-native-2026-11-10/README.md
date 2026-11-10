# Logos-native acceptance run, 2026-11-10 (Maker)

One run of `deploy/scripts/logos-native-acceptance.sh` against the packages
PR #120 builds, judged against the target contract in
[docs/architecture/logos-native-packaging.md](../../architecture/logos-native-packaging.md),
which also holds the design proposal. Result: **95 steps, 25 FAIL, 1 NOT RUN**
(`summary.tsv`). The failures are the findings; nothing was tuned to pass.

Files: `transcript.md` (every command, its output, exit status, verdict),
`summary.tsv`, `inputs.tsv` (sha256, version, manifestVersion, variants,
dependencies and root hash of each package), `session-layout.txt` (the
logosctl session directory after the run), `daemon.log` (the session's
daemon log). Session tokens and the storage module's key stayed in the
container.

## Inputs

- Host linux x86_64. Every tool ran in throwaway `debian:trixie-slim`
  containers on an internal Docker network (no catalog, no Logos Storage,
  no host ports).
- logosctl 0.3.2 (commit 96e704df) and 0.3.0 (d9eb3ba8), logoscore 0.2.3
  (a09b2d1f): release tarballs from logos-co/logos-logoscore-cli, sha256
  pinned in the script.
- lm 0.1.0, lgx 0.1.0, lgpm 1.0.0: the module builder's, from the nix store
  of our builds (no released binaries exist).
- Packages: `~/provision/data` (lez_core, lez_swap_node 0.2.0 as rebuilt at
  13:13Z, lez_indexer_module, blockchain_module 0.2.4), the Maker desk LGX
  that `from-scratch.sh --only nix` built (the nix store output behind the
  staged `maker-install`), and the chat_module / delivery_module LGX it
  staged. All unsigned.

Reproduce (the paths are this host's):

```
deploy/scripts/logos-native-acceptance.sh --role maker \
  --role-lgx <logos-lez_atomic_swap_maker-module.lgx> \
  --module <lez_core.lgx> --module <lez_swap_node.lgx> \
  --module <chat_module.lgx> --module <delivery_module.lgx> \
  --module <lez_indexer_module.lgx> --module <blockchain_module-0.2.4.lgx> \
  --tools-nix lez-nix-store-arm --out <dir>
```

## What works

- logosctl 0.3.2 and 0.3.0 install our portable core packages from file and
  load them: `module load lez_swap_node` brings lez_core with it,
  `call lez_swap_node node_status maker` answers `"status":"ok"` (S56–S68,
  S90). logoscore 0.2.3 does the same from a modules directory (S93).
- `lm metadata|methods|events` read every plugin, including the methods'
  doc comments (S28–S48); `lgx verify` passes every package (S21–S27).
- The supervisor's mechanics, with a stand-in launcher: start, restart after
  exit, status (S73–S74); `package remove lez_core` takes its dependents
  (S86–S87).

## What does not

- The desk is built with variant `linux-amd64-dev`; logosctl refuses it
  (S50, S91); the era lgpm refuses the portable core packages (S94). The
  desk was re-labelled with lgx to go on (S51, marked DERIVED).
- A file install resolves no dependencies (S54, S63); the desk declares no
  LEZ network requirement (S55); chat_module/delivery_module are `-dev` too
  (S58–S59).
- A wrong `--root-hash` is accepted for a file (S62). Nothing is signed.
- logosctl does not load a `ui_qml` package (S65).
- No Node in the package graph (S69); `start_node` with the Compose launcher
  returns 1 (S70).
- The Node is invisible to `module stats` (S76); `module reload` stops it for
  good (S78); a host crash is reported `not_loaded`, not `crashed` (S81),
  leaves the Node orphaned (S82), and restarting gives two Nodes (S84).
- `lm verify --did` not run (S49): lm 0.1.0 lacks it, lm 0.3.0 has no binary,
  and there is no signer DID.

## Not covered

arm64 (no run), macOS, Basecamp's own package manager, a catalog install,
the Taker desk (same build path as the Maker; run with `--role taker`), and a
real Node under any of these runtimes.
