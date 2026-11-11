# Logos-native acceptance, 2026-11-11 (b): release packages, version ranges, the network package

One run of `deploy/scripts/logos-native-acceptance.sh` per role against the
release set of the combined landing (code5 + issue112 + pkgfix). What changed
is described in the packaging note,
["Release packages and the LEZ network"](../../architecture/logos-native-packaging.md#release-packages-and-the-lez-network-2026-11-11-b).

| File | What |
|---|---|
| `maker/`, `taker/` | the run per role: `transcript.md` (every command, output, exit status, verdict), `summary.tsv`, `inputs.tsv`, `session-layout.txt`, `daemon.log`. **115 steps: 93 PASS, 14 FAIL, 7 INFO, 1 NOT RUN** each |
| `packages.tsv` | the release set (name, version, variant, dependency ranges, sha256, root hash), as `from-scratch.sh --only stage` wrote it to `deploy/images/basecamp-ui/assets/packages/` |

## Inputs

- Code: `023c929e` (integration head `c75576ab` plus pkgfix's two commits);
  the acceptance script with the two fixes committed after it (the range
  check reads the resolved `--recursive` view; lez_network is installed once).
  Packages built from `023c929e`: `from-scratch.sh --only rust`,
  `--only build:sidecar`, `--only nix`, `--only stage`.
- Packages: `packages.tsv`. Every one has a release variant (`linux-amd64`;
  `blockchain_module` 0.2.4 also `linux-arm64`, `darwin-arm64`), none is
  signed. Our packages' dependencies are `{name, version: "~<version in the
  set>"}` followed by the bare names.
- Tools: logosctl 0.3.2 and 0.3.0, logoscore 0.2.3, lm/lgx/lgpm from the
  module builder's nix store; throwaway `debian:trixie-slim` containers on an
  internal network; host linux x86_64.
- The landing's smoke run on the same images: `LEZ_LANES=3 regress.sh smoke`
  at `023c929e`, 6 of 6 passed in 7 min, no frozen try (two of the six are
  desk tests, so the release desks run in Basecamp 0.2.0).

Reproduce (P = `deploy/images/basecamp-ui/assets/packages`):

```
deploy/scripts/logos-native-acceptance.sh --role maker \
  --role-lgx $P/lez_atomic_swap_maker.lgx \
  --module $P/lez_core.lgx --module $P/lez_swap_node.lgx --module $P/lez_maker_node.lgx \
  --module $P/chat_module.lgx --module $P/delivery_module.lgx \
  --module $P/lez_indexer_module.lgx --module $P/blockchain_module.lgx \
  --network-lgx $P/lez_network.lgx --tools-nix lez-nix-store-arm --out <dir>
```

## Before and after

The 2026-11-11 run ([../logos-native-2026-11-11](../logos-native-2026-11-11/README.md))
had 106 steps, 20 FAIL per role; a run on that day's release-candidate packages
gave the same. Step numbers below: that run → this one.

| Class | Before (FAIL) | After (FAIL) |
|---|---|---|
| ours, fixed | 6: S56, S102 (desk `-dev`), S65, S66 (Chat, Delivery `-dev`), S70 (desk dependencies not all installed), S61 (no network decision) | 0 |
| upstream era tool, judged against a control | 1: S105 (lgpm refuses portable `lez_swap_node`) | 0 (S113 INFO: lgpm refuses Logos's own `blockchain_module` the same way; S114–S115 PASS) |
| not in scope (owner, 2026-11-11): unsigned | 8: S08–S22 | 9: S08–S24 (lez_network added) |
| upstream | 5: S60, S69, S72, S87, S92 | 5: S63, S73, S80, S95, S100 |

Fixed, step by step:

- S56 → S62, S102 → S110: the desk installs as built, with logosctl 0.3.2
  and 0.3.0 (release variant; the DERIVED relabel step is gone).
- S65, S66 → S69, S70: Chat and Delivery install.
- S70 → S74: every dependency of the desk installed, at a version in its range.
- S61 → S64, S65, S75, S78, S79: the old step looked for a network module among
  the role's dependencies (option B1). With B3 decided, the role's dependencies
  all resolve with a version range (S64), none is a LEZ network module (S65), a
  `lez_core` whose manifest says 0.5.1 (the default catalog's version) is
  reported `version_mismatch` against `lez_swap_node`'s range (S75), and the
  operator package `lez_network` installs and declares `blockchain_module` and
  `lez_indexer_module` with ranges, both installed in range (S78, S79).
- S105 → S114, S115: see the class above. The module builder's lgpm is the
  development build of logos-package-manager (`cli`), which takes only
  `<platform>-dev`; its `cli-portable` build is the one for release packages.

Still FAIL:

| Step (this run) | What | Whose | Issue text |
|---|---|---|---|
| S08–S24 | unsigned | not in scope (owner, 2026-11-11). Nothing upstream blocks it: lgx `keygen`/`sign`/`verify --keyring-dir`, logosctl `key` | — |
| S63 | a file install pulls in no dependency | logosctl (logos-logoscore-cli) | "`package install <file.lgx>` installs the package alone: its declared dependencies are neither resolved from the catalog nor reported as an error, so `package deps` lists them `not_installed`" |
| S73 | a wrong `--root-hash` is accepted for a file install | logosctl | "`package install <file.lgx> --root-hash <hash>` does not compare the given hash with the package's root hash; an all-zero hash installs" |
| S80 | `module load` of the desk fails | logosctl / runtime (a `ui_qml` package's backend runs only in Basecamp) | "`module load` of an installed `ui_qml` package fails with MODULE_LOAD_FAILED; there is no headless host for a `ui_qml` backend, and the error does not say so" |
| S95 | `module stats` shows the module host, not the Node | ours by design (the Node is a child process; option A3, the Node as a library, not done) and a runtime feature | "`module stats` reports only the `logos_host` process; include (or list) the processes a module host started" |
| S100 | a crashed module host is reported `not_loaded` | logosctl | "after a module host is killed, the daemon logs 'Module process crashed' but `module ls` reports the module `not_loaded`, not `crashed`" |

S61 NOT RUN (`lm verify --did`): the builder's lm 0.1.0 has no `verify`, and
the packages are unsigned.
