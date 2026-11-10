# Logos-native packaging: what the official tools say about PR #120, and how to close the gaps

Status: Proposed — 2026-11-10

```mermaid
flowchart LR
    subgraph today["Today: what logosctl installs"]
        Desk["lez_atomic_swap_maker (ui_qml, -dev variant)"] --> Sup["lez_swap_node: start_node(role, launcher, log, hold)"]
        Desk --> Core["lez_core (patched)"]
        Sup --> Core
        Sup -. "absolute launcher" .-> Compose["node-entrypoint.sh + Rust Node: Docker image only"]
    end
    subgraph proposed["Proposed"]
        Desk2["lez_atomic_swap_maker (portable, signed)"] --> Payload["lez_maker_node: plugin + bin/ payload; Node tied to the module's life"]
        Payload --> Core2["lez_core (versioned, signer-pinned)"]
        Desk2 -. "optional" .-> Net["lez_network: blockchain_module + lez_indexer_module (operator)"]
        Payload --> Data["session data/: config, identity, state"]
    end
```

The official Logos path is source → logos-module-builder → module → `lm` →
LGX → `logosctl package install` (dependencies, root hash) → a session
(`modules/ data/ keyring/ logs/`) → `module load|reload`, `call`, `watch`,
`stats`, headless or in Basecamp. This note records what that path does with
PR #120's packages today and proposes how to close the gaps. Every "observed"
claim comes from one retained run of `deploy/scripts/logos-native-acceptance.sh`
(transcript: [docs/evidence/logos-native-2026-11-10](../evidence/logos-native-2026-11-10/README.md);
step numbers `S..` refer to it) or from the spike that preceded it. The rest
is marked as a proposal or a guess.

## What we observed

Packages (all unsigned; S08–S20):

| package | version | manifestVersion | variants | declared dependencies |
|---|---|---|---|---|
| lez_atomic_swap_maker (desk) | 0.2.5 | 0.3.0 | linux-amd64-dev | chat_module, delivery_module, lez_core, lez_swap_node |
| lez_swap_node | 0.2.0 | 0.2.0 | linux-amd64 | lez_core |
| lez_core (patched) | 0.4.1-import.3 | 0.3.0 | linux-amd64 | — |
| lez_indexer_module (patched) | 1.1.1-rpc.3 | 0.2.0 | linux-amd64 | — |
| blockchain_module | 0.2.4 | 0.3.0 | linux-amd64, linux-arm64, darwin-arm64 | — |
| chat_module / delivery_module | 0.2.2 / 0.2.0 | 0.3.0 | linux-amd64-dev | delivery_module / — |

(lez_swap_node was rebuilt with the `lez_core` dependency during the spike;
the earlier build, 0.1.0, declared `dependencies: []`.)

1. **logosctl 0.3.2 and 0.3.0 install and run our core modules.** A clean
   session installs lez_core, lez_swap_node, lez_indexer_module and
   blockchain_module 0.2.4 from their files, loads lez_swap_node together with
   its declared lez_core, and answers `call lez_swap_node node_status`
   (S56–S68, S90). The v0.2-era logoscore 0.2.3 does the same from a modules
   directory (S92–S93). So the v0.2-built core LGX is not the blocker.
2. **The desks do not install with logosctl.** They are built with the
   builder's development variant tag `linux-amd64-dev`; logosctl takes only
   `linux-amd64` ("Package does not contain variant for platform", S50, S91).
   The era `lgpm` (the desks' nix installer) is the mirror image: it takes
   only `-dev` and refuses the portable core modules (S94). No single
   installer takes the whole graph today; Basecamp works only because
   `stage-basecamp-package.sh` renames the variant in an installed tree, which
   no longer matches any published root hash. chat_module and delivery_module
   as we build them are `-dev` too (S58–S59).
3. **Installing from a file resolves no dependencies.** The (re-labelled) desk
   installs alone and `package deps --recursive` then lists chat_module,
   delivery_module, lez_core and lez_swap_node as `not_installed` (S53–S54).
   Dependency resolution only happens against a catalog — and the default
   catalog (logos-co/logos-modules-release) carries *other* packages under our
   names: lez_core 0.5.1, lez_indexer_module 1.2.1, blockchain_module 0.3.4,
   chat_module 0.3.0, delivery_module 0.3.3. Our plain-string dependencies
   ("any version, any signer") would resolve to them.
4. **`--root-hash` is not checked for a file install.** An all-zero root hash
   is accepted (S62). Integrity of a local file rests on the signature (none)
   or on the caller comparing `package show`'s `rootHash`, which the script does.
5. **logosctl does not load a `ui_qml` package** (`module load
   lez_atomic_swap_maker`: "not found", S65); it installs it into `plugins/`.
   The desk's backend runs only in Basecamp.
6. **The package graph has no Node.** Nothing installed contains
   `node-entrypoint.sh` or `lez-maker-node` (S69). `start_node` with the
   launcher Compose provides returns 1 (S70) and nothing runs (S71).
7. **The supervisor works, and duplicates the runtime.** With a stand-in
   launcher, lez_swap_node starts the child, restarts it after an exit
   (`starts: 2, last_exit: "exit 7"`) and holds it while the hold file
   exists (spike; S73–S74). But:
   - `module stats` shows the module host, not the Node (S76);
   - `module reload lez_swap_node` kills the Node and nothing starts it again:
     the supervision state is gone (S78);
   - when the module host crashes (`kill -9`), the daemon logs "Module process
     crashed" but `module ls` says `not_loaded`, not `crashed` (S81); the Node
     keeps running as an orphan (S82), and loading the module and starting the
     role again — what the desk does — gives **two Nodes** (S84). In Compose
     the container's death hides this; on a user's machine it would not.
     node-entrypoint.sh removes `node.sock` before starting, so a second real
     Node would take over the socket while the first still owns the store.
8. **Session.** Each module host is `logos_host --name <m> --path
   <session>/modules/<m>/<plugin> --instance-persistence-path
   <session>/data/<m>/<instance id>`; the instance directory stayed the same
   across reload and crash. `keyring/` holds trusted *package-signing* keys
   only. `package remove lez_core` takes lez_swap_node and the desk with it,
   and leaves their `data/` directories behind (S86–S88).
9. **Payload files survive packaging.** A probe package with an ELF and a
   shell script under `variants/linux-amd64/bin/` (0755) was built with lgx
   0.1.0, installed by logosctl 0.3.2 with its modes kept, and the ELF ran
   from `modules/<m>/bin/` (spike). The current LGX spec, though, says files
   are written `0644`; a newer lgx may drop the execute bit.
10. **Tools.** logosctl 0.3.x ships for linux x86_64/aarch64, macOS aarch64
    and Windows; logoscore 0.2.3 for linux x86_64/aarch64. `lm` has no
    released binary: the builder's lm 0.1.0 has `metadata|methods|events` but
    no `verify`, so `lm verify --did` was not run (S49), and there is no DID to
    pin anyway. Our core packages carry only the build host's variant; only
    blockchain_module 0.2.4 is multi-platform. Nothing was run on arm64.

## Proposal

### (a) The role's Node enters the package graph

Options:

- **A1 — a payload module per role (recommended now).** `lez_maker_node` and
  `lez_taker_node`, `type: core`, each variant carrying the role's
  executables (`bin/lez-<role>-node`, its chat gateway, actors, CLI,
  healthcheck, the v0.2 sidecar it spawns) and a small plugin that is
  lez_swap_node's supervisor, specialised to one role. The plugin finds its
  payload through `LogosModuleContext::modulePath()` and its state through
  `instancePersistencePath()` (both in the cpp-sdk our builder pins), so
  `start` takes no paths. Depends on `lez_core` (signer). The Maker package
  carries no Taker binary and vice versa: deploy/role-symmetry.json gains the
  module names and a check that lists each LGX (`lgx manifest`, `tar -t`) and
  fails on the other role's executable names. lez_swap_node either becomes
  this plugin's shared source (built twice) or disappears.
- **A2 — both payloads inside lez_swap_node.** Rejected: it breaks the role
  symmetry contract (a role's artifact must not carry the other role's
  binaries).
- **A3 — the Node as a library**, a Rust cdylib the module loads in process,
  as blockchain_module wraps logos-blockchain-c and lez_core wraps
  wallet_ffi. The most native (the runtime then owns the Node's life
  outright), but it changes the Node's process model (it spawns sidecars and
  actors per swap) and touches the frozen M3 engine. A later target, not the
  next step.
- **A4 — the Node as a daemon managed by logosctl** (#115's "Maker stays a
  daemon under logosctl"): logosctl manages modules only; there is no
  non-module daemon to manage. Not available.

Size: the stripped Maker payload is about 146 MB, the Taker's about 230 MB
(it carries the zec-local-poc tools); the LGX decompression cap is 1 GiB per
package, so three platform variants of the Taker (≈ 690 MB) fit, barely. The
ZEC proof-of-concept tools should move out of the role payload.

Mapping onto the session (proposal):

| today (Compose) | in the session |
|---|---|
| `/var/lib/lez/<role>` (state volume) | `data/lez_<role>_node/<id>/state` (0700) |
| `/run/lez/<role>` sockets | a short directory under `$XDG_RUNTIME_DIR` (or `$TMPDIR`) named by the instance id — Unix socket paths are limited to 108 bytes and session paths are long |
| `LEZ_*` environment, `/run-config`, market-bootstrap.env | one config document, set by `configure(json)` and kept in `data/…/config.json`; the desk shows and edits it |
| `/run-lez-identity/identity.json` | created or imported once into `data/…/identity.json` (0600); backup = the session's `data/` |
| LEZ signing key | stays in lez_core's wallet (already: `serve_signer`); not in `keyring/`, which is for package signers |
| socat loopback forwards to bitcoind/sequencer/indexer/zebra | not needed when the endpoints are configured as real addresses; the BTC adapter and sidecars accept only loopback endpoints today — relaxing that is BTC-adjacent and needs its own review |
| Node log via `log_file` | the Node's stdout/stderr to the host's, so logosctl's rotated `logs/daemon.log` carries it (GUESS: the pipe is inherited; not verified) |

### (b) The role package and the LEZ network

- **B1 — hard dependencies on blockchain_module + lez_indexer_module.** Every
  user installs and runs a full LEZ node and indexer (≈ 160 MB of packages,
  chain storage, ports), and the network modules still need a deployment
  file that today only Compose writes. Wrong for a trader's machine.
- **B2 — `optional_dependencies` (manifest 0.6.0+).** The installer offers
  the network modules; the role runs without them against configured
  endpoints. Needs a builder that writes manifest ≥ 0.6.
- **B3 — an explicit remote network (recommended).** The role package
  declares no network module; its config names the sequencer and indexer
  endpoints (the public testnet's, or a self-hosted operator's). lez_network
  stays a separate *operator* package that declares blockchain_module and
  lez_indexer_module (it already does) and is what the devnet harness
  installs. Once the builder writes manifest ≥ 0.6, the desk lists
  `lez_network` under `optional_dependencies`, so the relation is visible in
  `package deps` without forcing it.

Whatever the choice, dependencies must stop being bare names: object entries
with a version range and, once we sign, the signer DID
(`{"name": "lez_core", "version": "0.4.1-import.3", "signer": "did:jwk:…"}`),
so a catalog cannot substitute upstream's lez_core 0.5.1 for our patched
build (observation 3). The patched modules could also take distinct names
(`lez_core_swap`), which avoids the collision without manifest 0.7.

### (c) One lifecycle owner

lez_swap_node re-implements restart and hold, and the runtime cannot see
the result (observation 7). Proposal: the runtime owns the module, the module
owns exactly one Node, tied to its own lifetime.

- Loading `lez_<role>_node` with a valid config starts the Node; unloading or
  reloading stops it (SIGTERM to the process group, SIGKILL after 10 s, as
  now). No `start_node(role, launcher, log, hold)` from the desk.
- The Node dies with its host: `PR_SET_PDEATHSIG` on Linux; on macOS a pipe
  the child watches. The Node takes an exclusive lock on its state directory,
  so a second instance refuses to start instead of taking the socket.
- Restart after a Node exit can stay in the module (bounded back-off), but
  every exit is an event (`logos_events: node_exited(code, starts)`) and
  `status()` reports the Node's readiness, pid, restarts and last exit, so
  `logosctl watch` and Basecamp see it. A module that cannot keep its Node up
  should fail visibly rather than loop silently. The hold file becomes a
  persisted `hold(bool)` call.
- Reload keeps state (it is in `data/`) and restarts the Node; the
  acceptance script's S78/S82/S84 become PASS.
- Upstream ask: logosctl should report a crashed module as `crashed` (S81).

### (d) The v0.2 ↔ v0.3 tooling gap

What already works across the gap: logosctl 0.3.x installs and loads our
v0.2-built core packages (manifestVersion 0.2.0/0.3.0) and dispatches calls.
What does not: the desks' dev variants, the unsigned packages, bare-name
dependencies, no headless desk. Migration path, none of which moves the LEZ
protocol off v0.2.4:

1. Build and publish portable packages for everything, desks included (the
   builder's `lgx-portable`, as the core modules already use), merge platform
   variants with `lgx merge`, sign with `lgx sign`, publish the DID; run
   logos-native-acceptance.sh in CI on the release artifacts.
2. Move the module builder pin to a revision that writes manifest ≥ 0.6
   (versioned/signed dependencies, optional dependencies, `assets/lidl`,
   the 256×256 icon for `ui_qml`). The spec says tooling reads 0.2.x–0.7.x;
   whether Basecamp 0.2.0 reads a 0.7 manifest is not verified.
3. Replace logoscore 0.2.3 in the images with logosctl 0.3.x and move the
   Basecamp pin to its v0.3 release, so the harness runs the same runtime a
   user installs.
4. Moving LEZ itself to v0.3 (lez_core 0.5, indexer 1.2, blockchain_module
   0.3) is a protocol change and its own milestone; the four patches would
   have to be rebased onto it.

## Recommended order

1. Portable, signed packages for the role desks and our modules (d1); the
   acceptance script against them.
2. `lez_maker_node` / `lez_taker_node` payload modules with the single
   lifecycle owner (a + c), config in `data/`, identity onboarding.
3. Role network requirement as B3, with versioned, signer-pinned dependencies.
4. Builder and runtime pins (d2–d3).
