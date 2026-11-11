# Logos-native acceptance and lifecycle checks, 2026-11-11 (Maker and Taker)

The role's Node now travels in a Logos package (`lez_maker_node`,
`lez_taker_node`; `modules/lez-<role>-node`, `deploy/scripts/package-role-node.sh`)
and `lez_swap_node` 0.3.0 is its one lifecycle owner
([packaging note, "What was built"](../../architecture/logos-native-packaging.md)).
This directory holds one run of `deploy/scripts/logos-native-acceptance.sh`
per role against the packages of that landing, the lifecycle checks run on the
live stack during the landing, and the supervisor's host-side harness.

| File | What |
|---|---|
| `maker/`, `taker/` | the acceptance run per role: `transcript.md` (every command, output, exit status, verdict), `summary.tsv`, `inputs.tsv`, `session-layout.txt`, `daemon.log`. **106 steps, 20 FAIL, 1 NOT RUN** each |
| `lifecycle-stack-maker.txt` | the three lifecycle checks on the live devnet stack (`lez-maker-node`), after the landing's smoke run |
| `lifecycle-checks.sh` | the script behind it (fixed afterwards: its two "old group" listings used `ps -g`, which selects by session in procps, and printed nothing in the run; the process lists before and after show the old group's pids gone) |
| `supervisor-harness.txt` | `lez_swap_node_impl.cpp` driven directly on the host with the Logos SDK stubbed and a stand-in Node: kill -9 of the Node; the module host killed and loaded again; a second host of the same instance while the first still runs |

## Inputs

- Code: branch `agent/rolepkg` at `9f0c1dfa` (rebased on `a5e7a2d7`); the
  stack ran the same commit.
- Packages: `lez_core` 0.4.1-sign.4, `lez_swap_node` 0.3.0 (both
  `~/provision/data`), `lez_maker_node` / `lez_taker_node` 0.1.0 (packaged by
  `from-scratch.sh --only stage`), the role desks 0.2.5 (`apps/basecamp#lez-<role>-ui-lgx`,
  `linux-amd64-dev`), chat_module 0.2.2 / delivery_module, `lez_indexer_module`
  1.1.1-rpc.4, `blockchain_module` 0.2.4. All unsigned. sha256s in
  `<role>/inputs.tsv`.
- Tools as in the 2026-11-10 run: logosctl 0.3.2 and 0.3.0, logoscore 0.2.3,
  lm/lgx/lgpm from the module builder's nix store; every step in throwaway
  `debian:trixie-slim` containers on an internal network. Host linux x86_64.

Reproduce:

```
deploy/scripts/logos-native-acceptance.sh --role maker \
  --role-lgx <logos-lez_atomic_swap_maker-module.lgx> \
  --module <lez_core.lgx> --module <lez_swap_node.lgx> --module <lez_maker_node.lgx> \
  --module <chat_module.lgx> --module <delivery_module.lgx> \
  --module <lez_indexer_module.lgx> --module <blockchain_module-0.2.4.lgx> \
  --tools-nix lez-nix-store-arm --out <dir>
```

## What changed against 2026-11-10

Now PASS (step numbers of this run; the 2026-11-10 ones in brackets):

- The installed package graph carries the role's Node and its launcher (S76;
  was S69 FAIL). `call lez_<role>_node launcher` returns the installed
  `bin/node-entrypoint.sh`, `call lez_<role>_node payload` the files and their
  sha256 (S78–S79).
- `start_node` with the package's launcher starts it (S80–S81; was S70–S71
  FAIL with the Compose launcher). Without the role's configuration, which only
  Compose provides, the launcher stops at once (S82:
  `LEZ_SIDECAR_PORT_BASE: parameter null or not set`) and `lez_swap_node`
  restarts it: that is what this bare session can show.
- `module reload lez_swap_node`: the role is supervised again (S89; was S78
  FAIL).
- `kill -9` of the `lez_swap_node` host: no orphaned Node (S93; was S82 FAIL);
  loading the module again and starting the role gives exactly one Node (S95;
  was S84 FAIL: two).
- `package remove lez_core` also removes `lez_<role>_node` (S97–S98).

Still FAIL, and whose:

| Steps | What | Whose |
|---|---|---|
| S08–S22 | no package is signed | ours (packaging note, d1) |
| S56, S102 | the desk's `linux-amd64-dev` variant is refused (re-labelled with lgx to go on, S57) | ours: the builder's dev variant (d1) |
| S65–S66 | chat_module / delivery_module as we build them are `-dev` too | ours, same cause |
| S60, S70 | a file install resolves no dependencies; the -dev modules above stay uninstalled | logosctl (catalog-only resolution), plus the -dev variants |
| S61 | the role package declares no LEZ network | design choice B3 (an explicit remote network), undecided in #115 |
| S69 | a wrong `--root-hash` is not refused for a file install | logosctl |
| S72 | logosctl does not load a `ui_qml` package | logosctl (the desk's backend runs only in Basecamp) |
| S87 | `module stats` shows the module host, not the Node | runtime (the Node is a child process, option A3 not done) |
| S92 | a crashed module host is reported `not_loaded`, not `crashed` | logosctl |
| S105 | the v0.2-era lgpm refuses the portable core packages | era tool |

## Lifecycle checks on the live stack (`lifecycle-stack-maker.txt`)

Run after the landing's smoke run, on `lez-maker-node`:

1. `kill -9` the Maker Node (pid 136): its group (four socat forwards, the ZEC
   sidecar on 19901, forward children 218–220) was gone within 1.5 s; one new
   Node (pid 384, from `lez_maker_node/bin/`) was healthy 7 s after the kill.
2. The `lez_swap_node` host stopped with SIGTERM, which is how liblogos stops a
   module host on unload or Basecamp exit: 3 s later no Node, forward or
   sidecar was left (no orphan); the records (`node-maker.start`, `.group`,
   `.lock`, `signers`) stayed in the module's instance directory. Basecamp
   0.2.0 does not load a stopped or crashed module again by itself, so the
   Node stays down until the module is loaded again.
3. `docker restart lez-maker-node` (Basecamp and all module hosts stop and
   start): the new `lez_swap_node` resumed the signer and the Node from its
   records at 09:52:40 (Basecamp log); the module logged one supervision and
   one Node start, so the desk's own `start_node` found the role supervised;
   exactly one Node, healthy 8 s after the restart.

The landing's smoke run (`LEZ_LANES=3 regress.sh smoke`) passed 5 of 6 on the
first try; `node:zec:zec-happy:TakerSellsForeign` completed both legs but its
log count read 0 projections because a `delivery_module` log line was written
into the middle of the Maker's `maker actor <id>: drive → projected
taker_first_lock` line on the container's shared stdout; the rerun of that one
test passed (108 s). The two ZEC happy tests recreate both Nodes
(`zec-provision.sh`), so they also covered a Node start from the package after
a container recreation.
