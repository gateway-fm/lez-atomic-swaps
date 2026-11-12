# Issue #115 submission: the swap stack as Logos modules in Basecamp

Status: the record for release v0.3.0 (Milestone 2), 2026-11-12. Code at
`main` `b6b1c273` (PRs #120, #124, #131, #126 and #127 merged) plus the
release-preparation commits. Every claim below names a file, a commit or an
evidence path in this repository. Where nothing in the repository proves a
claim, the row says **not verified**.

The full regressions in the repository ran on the commits named in
[Evidence in the repository](#evidence-in-the-repository). The release
workflow on the commit to be tagged is under
[Pending gates](#pending-gates).

## Short answer

The LEZ network node, the LEZ indexer, each role's LEZ wallet and each role's
Node supervisor run as Logos modules inside Basecamp processes. Compose still
hosts those Basecamps and supplies the Rust Node binaries, configuration,
identities, volumes and lifecycle. The sequencer, Bitcoin Core, Zebra and the
miner stay plain containers. Six upstream patches are carried as patch files,
one of which changes only the macOS build of `lez_core`; none has been
offered upstream.

So the stack is a working rehearsal of the module topology inside Docker, not
yet "install the Maker or Taker package and nothing else to start" on a
user's machine.

## What runs in Basecamp and what stays in Compose

A component map of the whole stack (which box is a Basecamp, which module is
upstream, patched or ours) is in
[`docs/architecture/deployment-components-and-rpcs.md`](architecture/deployment-components-and-rpcs.md#component-map).

Source: `deploy/compose.yaml`. Seven containers run by default; no init
container exists (commit `b63c958b`). `docker compose config --services`
with the default profiles lists `bitcoin-core btc-miner network sequencer
maker-node taker-node zebra basecamp-ui`, and `basecamp-ui` has `scale: 0`.

| Component | Runs where | Why there |
|---|---|---|
| LEZ network node: `blockchain_module` 0.2.4 | `network` container: a headless Basecamp; its LEZ Network desk (`apps/basecamp/network`) starts it | The released lgx a Basecamp user installs, unpatched, sha256-pinned (`BLOCKCHAIN_MODULE_SHA256` in `deploy/scripts/from-scratch.sh`). The config and deployment paths come from Compose environment (`LEZ_NETWORK_NODE_CONFIG`, `LEZ_NETWORK_NODE_DEPLOYMENT`); without them the desk starts nothing (`apps/basecamp/common/lez_network_host.h`). |
| LEZ indexer: `lez_indexer_module` 1.1.1-rpc.4 | the same `network` Basecamp | Built from source with three patches (below), because the swap Nodes, sidecars and explorer read `indexer_service` JSON-RPC and the module offered only Logos calls (`deploy/builder/patches/README.md`). |
| LEZ Network desk (`lez_network` 0.3.0) | the same `network` Basecamp | Starts both modules and runs Re-index (`deploy/scripts/repair-indexer.sh`). Not a role package; it declares `blockchain_module` and `lez_indexer_module` (`apps/basecamp/network/metadata.json`). |
| LEZ sequencer: `sequencer_service` | plain `sequencer` container | There is no sequencer module (`deploy/compose.yaml`, `deploy/README.md` service table). |
| Role LEZ wallet: `lez_core` 0.4.1-sign.4 | `maker-node` / `taker-node`: each party's headless Basecamp | The desk opens the wallet and imports the role key (`apps/basecamp/common/lez_role_signer.h`); patched to import a public account and sign a message hash. |
| `lez_swap_node` 0.3.0 (our module, `modules/lez-swap-node`) | each party's Basecamp | Serves the role's signer socket (asks `lez_core` for every signature) and is the Node's one lifecycle owner: `start_node`, `stop_node`, `node_status`, `node_stats`, `serve_signer` (`modules/lez-swap-node/src/lez_swap_node_impl.h`). The Node dies with the module's host (`PR_SET_PDEATHSIG`); a Node group a dead host left is ended before the next start; what the desk asked for is resumed when the module is loaded again. |
| `lez_maker_node` / `lez_taker_node` 0.3.0 (our payload packages, `modules/lez-<role>-node`) | installed in each party's Basecamp (`<user dir>/modules`) | Carry the role's Node: the Rust Node, its CLI, its Bitcoin and Zcash actors, the LEZ v0.2 sidecar, the health probe and the launcher (`modules/lez-role-node/node-entrypoint.sh`) under `bin/`, with sha256s in `bin/payload.json`; `launcher()` and `payload()`. Linux only. Packaged by `deploy/scripts/package-role-node.sh`. |
| Maker and Taker desks (`lez_atomic_swap_maker`, `lez_atomic_swap_taker`) | each party's Basecamp, one desk per Basecamp, on VNC 127.0.0.1:5901 / :5902 | Commit `51f8f19b`. The UI tests drive the same desks (`deploy/ui-tests`). |
| Rust Maker / Taker Node and their actors and sidecars | processes inside each party's Basecamp container, children of `lez_swap_node` | Started from the installed role package (`lez_<role>_node`, above); the image only installs the package (`deploy/images/<role>-node/Dockerfile`). The configuration (chains, identity, volumes) still comes from Compose. |
| Loopback forwarders (`socat`) | inside each role container (the package's launcher) | The Bitcoin adapter, wallet client and LEZ sidecars accept only literal-loopback endpoints. Relaxing that is BTC-adjacent and not done. They end with the Node (`setpriv --pdeathsig`). |
| Bitcoin Core 31.1 regtest | plain `bitcoin-core` container | #115 keeps Bitcoin bring-your-own. `logos-bitcoind-module` (9f89d3d, Core 31.2) is not adopted. Built on macOS arm64 on 2026-11-11 and its library run with the module's own start flags on regtest: the wallet the Nodes fund from is compiled out (`getbalances`, `walletcreatefundedpsbt`: Method not found); there is no setting for `txindex`/`txospenderindex` (the library has both: started with our options it served them); the opened RPC port answers 403 for `getindexinfo`, `gettxout`, `gettxspendingprevout`, `testmempoolaccept` and regtest mining; RPC binds the module's loopback only. Also: no released lgx, and a from-source Core in place of the Guix-verified 31.1 that M3 was accepted on (`scripts/verify-bitcoin-core-release.sh`). Not run: the module in a Logos host, Linux. Follow-up: issue #129. |
| Zebra 7.0.0 regtest (NU7) | plain `zebra` container | `logos-zebrad-module` (6fa8663) is not adopted in this release, for two reasons: there is no released lgx, and its `linux-arm64` variant has never been built upstream, while our Basecamps run in Linux containers. The node itself fits: it embeds Zebra 7.0.0, the release this stack runs; built on macOS arm64 on 2026-11-11 and run with the configuration the module writes (Regtest, NU7 at height 1, a miner address, JSON-RPC on TCP with a cookie), its library answered every method our adapter and scripts call, on branch 77190ad9, `generate` included. The cookie it always requires is one our adapter already accepts. It would still be a container exposing TCP JSON-RPC for the shared regtest chain. An earlier version of this row gave a version difference and the cookie as reasons; both were wrong. Not run: the module in a Logos host, Linux, a real swap. Follow-up: issue #128. |
| `btc-miner` | plain container | Regtest harness only. |
| `btc-explorer`, `lez-explorer` | profile `explorers`, off by default | Reviewer and demo aids, not product runtime. |
| `heartbeat` | profile `heartbeat` | Finality watcher; its transfers go through `lez_core` in a headless logoscore, but its host is Compose. |
| `tools` | profile `tools`, one-shot | Escrow deployment, vault claims, identity minting for the market bootstrap. |
| `basecamp-ui` | build only (`scale: 0`) | Base image of the three Basecamps (`deploy/images/basecamp-ui`). |
| Identities, keys, configuration, volumes, restart policy | Compose mounts and `deploy/scripts/gen-config.sh` | No package onboards them yet. |

## Requirement traceability

The #115 requirements below are as restated in this repository's planning
notes and in the reviewer reports of 2026-11-10; the issue text itself is not
in the repository. "On the current code" means checked by a static check or
a test that ran on the module topology `main` carries; the full regressions
of 2026-11-11 and 2026-11-12 (Evidence in the repository) ran on it.

| # | Requirement | Where it is implemented | How it is verified | Evidence | On the current code |
|---|---|---|---|---|---|
| R1 | The LEZ network node is the Logos `blockchain_module` | `network` service; `deploy/images/lez-network-module/Dockerfile`; `apps/basecamp/network`; commits `c703d531`, `4626f8af`, `b63c958b` | `regress.sh` regression; `docker compose config -q`; `scripts/check-m6-basecamp-package-contract.mjs` (covers the LEZ Network desk) | `docs/evidence/regression-m2-2026-11-11/`, `docs/evidence/regression-2026-11-12/` and their macOS arm64 counterparts (one `network` Basecamp hosting node and indexer); earlier `docs/evidence/regression-modules-4c-2026-11-10/` (node and indexer in two Basecamps) | Static checks pass. Runtime: every swap of the full regressions ran on this network. |
| R2 | The indexer is the official `lez_indexer_module` v1.2.0 | Deviation: 1.1.1 source (`5033da44`) rebuilt on LEZ v0.2.4 with three patches, version `1.1.1-rpc.4` | as R1 | as R1 | Deviation stands. Our planning notes record v1.2.0 as built on LEZ v0.3.0, a protocol change; that is **not verified** in this repository. |
| R3 | The LEZ wallet `lez_core` (#115: a later swap-in) | Done (PR #120): the desk imports the role key into patched `lez_core`; `lez_swap_node` serves the signer socket; the sidecar verifies every returned witness against its planned message and account (`compat/lez-v0_2-sidecar/src/message_signer.rs`). Commits `1e2cbc7e`, `228689cc`, `22196d30` | regression as R1 | as R1 (the full regressions run after `22196d30` moved the signer socket into `lez_swap_node`) | Runtime: the swaps of the full regressions signed their LEZ transactions this way. |
| R4 | The Taker Node becomes a Logos module | The Node travels in a Logos package, `lez_taker_node` (`modules/lez-taker-node`, `deploy/scripts/package-role-node.sh`), which the Taker desk depends on; `lez_swap_node` starts it from the installed package and is its one lifecycle owner (packaging note, option A1). The Node is still a process the module launches, not code the module host runs (option A3) | regression; logos-native acceptance run (Maker and Taker) | `docs/evidence/logos-native-2026-11-11/` (the package graph carries the Node; the launcher comes from `call lez_<role>_node launcher`; reload, host crash and recovery leave exactly one Node) | Packaged. Process model unchanged (A3 not done). |
| R5 | The Maker Node stays a daemon under `logosctl` | Deviation: the Maker Node is packaged and supervised like the Taker's (`lez_maker_node`, `lez_swap_node`). `logosctl` manages modules only, so there is no daemon to manage (`docs/architecture/logos-native-packaging.md`, option A4) | as R4 | as R4 | Needs a decision in #115. |
| R6 | The sequencer proxy disappears | Local devnet: there is no proxy; the Nodes forward a loopback port to `sequencer` with `socat`. Public testnet: the launcher renders `sequencer_route: official_public` (`LEZ_LEZ_SEQUENCER_ROUTE`, `compose.testnet.yaml`), so the Nodes, their sidecars and the role signer's `lez_core` reach `https://testnet.lez.logos.co/` directly. The proxy remains only for the one-time escrow deployment (its deployer takes loopback only) | `docker compose -f compose.yaml -f compose.testnet.yaml config`; a read-only JSON-RPC call from the role image to the public sequencer | `docs/testnet-run.md` | Configuration verified; **no testnet swap run** since the change. |
| R7 | The Chat gateway disappears | Not started anywhere; no longer copied into either role image or the role packages (the binaries remain Cargo targets for the Chat tests) | `deploy/images/<role>-node/Dockerfile`, `deploy/scripts/package-role-node.sh` | — | Verified (static). |
| R8 | `desk-sockets.py` disappears | Removed in commit `f1dfe773`; each party's desk is reached in its own Basecamp over VNC | `git ls-files` finds no such file | — | Verified. |
| R9 | SQLite stays | `crates/swap-store` (`rusqlite` 0.41, commit `ddf12e27`) | unit tests in CI | — | Not re-run by this change. |
| R10 | Bitcoin Core stays bring-your-own | `bitcoin-core` plain container; testnet route uses your own Core or a public RPC provider (`docs/testnet-run.md` sections 2 and 9) | `scripts/verify-bitcoin-core-release.sh` in CI | — | Not re-run by this change. |
| R11 | A user installs the Maker or Taker package and has nothing else to start | The role's executables now come with the package graph (desk → `lez_<role>_node` → `lez_swap_node` → `lez_core`); with `LEZ_<ROLE>_NODE_LAUNCHER=package` the desk starts the Node from it. Not met: the role's configuration (chain endpoints, identity, state paths) is still Compose environment and mounts, the launcher needs host tools (bash, socat, jq, curl, python3, setpriv), a file install resolves no dependencies (the packages themselves, desks included, now install with `logosctl`) | logos-native acceptance run | `docs/evidence/logos-native-2026-11-11/` | Executables: packaged. Configuration and install graph: gap stands. |
| R12 | Out of scope: the LEZ 0.4 port (#107) and a public indexer (logos-co/rfp#126) | The stack stays on LEZ v0.2.4 (`LEZ_SOURCE_COMMIT`, `LEZ_MODULES_LEZ_COMMIT` in `from-scratch.sh`); the indexer runs only on the devnet | — | — | Respected. |
| R13 | Reviewer question on #115: use `logos-bitcoind-module` and `logos-zebrad-module` | Not adopted; reasons in the table above | both modules built on macOS arm64 and their libraries run on regtest with the modules' own settings (2026-11-11); neither loaded in a Logos host | issues #128, #129 | Bitcoin: blocked by the module as built. Zcash: the node fits; blocked on a release and a Linux arm64 build. |
| R14 | Platforms: a native role package on darwin-arm64, with a non-Linux store | Builds only (PR #126). The workspace, the LEZ sidecar, `lez_swap_node`, the role packages, the desks and patched `lez_core` build for darwin-arm64, and the product crates pass their tests on macOS; on macOS the store hands actors owner-private copies instead of Linux's sealed memory files, which is weaker against processes of the same user (PR #126, "Weaker than Linux"). Not done: the launcher (`modules/lez-role-node/node-entrypoint.sh`) and `deploy/scripts/package-role-node.sh` are Linux-only, there is no macOS lane in CI or in the release workflow, and nothing has run in a Logos host on macOS. The release builds Linux arm64 images (`.github/workflows/release-images.yml`) | the crates' tests on macOS | PR #126 | No installable darwin package. |
| R15 | Compose as small as possible | Seven default containers, no init containers, explorers optional, `basecamp-ui` build-only (commits `51f8f19b`, `b63c958b`) | `docker compose config --services`; `deploy/tests` | — | Verified (static). |

## Upstream patch register

Six patch files under `deploy/builder/patches/`. The upstream sources stay
clean git submodules under `third_party/` at the commits below;
`from-scratch.sh` applies the patches to a scratch copy before each module's
own flake builds its lgx. Line counts are `wc -l` of the patch files.
`blockchain_module` 0.2.4 is the released lgx and is not patched.

| Patch | Lines | Applies to | What it adds | Security and review notes | Upstream state | Removal condition |
|---|---|---|---|---|---|---|
| `lez-indexer-module/0001-lez-v0.2.4-indexer-breakpoint-interval.patch` | 13 | logos-execution-zone `9edf4a622fe9` (`lez/storage`) | a state snapshot every 10 blocks instead of 100, so `getAccountAtBlock` answers inside the swap actors' ~30 s attempts (commit `89350dcc`) | an index written with the other interval must be rebuilt; upstream's `indexer_core` test `accept_block_snapshots_state_at_breakpoint_interval` fails with this value | not offered | upstream makes the interval configurable or faster |
| `lez-indexer-module/0001-lez-v0.2.4-indexer-ffi-rpc-serve.patch` | 177 | logos-execution-zone `9edf4a622fe9` (`lez/indexer/ffi`, `lez/indexer/service`) | `indexer_rpc_serve(indexer, port)`: `indexer_service`'s own JSON-RPC over the core the FFI runs; serving again replaces the previous server | binds `0.0.0.0:port` inside the module process; isolation comes from the container network (Compose publishes `127.0.0.1:8779` only); the finalized-block subscription is refused | not offered | the module exposes a contract the Nodes and sidecars can use, or upstream ships this |
| `lez-indexer-module/0002-lez-indexer-module-rpc.patch` | 52 | lez-indexer-module `5033da442a7f` | `serve_rpc(port)` over the call above; version `1.1.1-rpc.4`; the build supplies the patched LEZ v0.2.4 source (`--override-input logos-execution-zone`) | same listener as above | not offered | as above |
| `logos-execution-zone-module/0001-lez-wallet-ffi-sign-public-message-hash.patch` | 120 | logos-execution-zone `9edf4a622fe9` (`lez/wallet-ffi`) | `wallet_ffi_sign_public_message_hash`: a signature over a 32-byte hash with a public account the wallet holds | signs an arbitrary 32-byte hash for the one account: no typed transaction preview, purpose, rate limit or audit record. The sidecar checks every witness, so a wrong signer cannot authorize a swap transaction, but a same-user process that reaches the socket can ask for any hash | not offered (related: logos-execution-zone-module#73, message signing) | an official wallet signing API the sidecar can use |
| `logos-execution-zone-module/0001-lez-darwin-metal-toolchain-for-nix-build-users.patch` | 35 | logos-execution-zone `9edf4a622fe9` (`flake.nix`, the darwin-only `metalStub`) | risc0's Metal kernels compile under a Nix build user on macOS: `metal`/`metallib` run from the Metal Toolchain's mount instead of through `xcrun` (PR #126) | build-time only; on Linux the stub is never used and `lez_core.lgx` is byte-identical with and without it | not offered | upstream's flake finds the toolchain for build users |
| `logos-execution-zone-module/0002-module-import-and-sign-public-account.patch` | 111 | logos-execution-zone-module `acf0cd501b26` | `import_public_account(private key hex)` over the FFI's existing import, and `sign_public_message_hash(account, hash)`; version `0.4.1-sign.4` | the key arrives as hex from a plaintext key file the desk reads from a Compose bind mount | not offered | as above, plus a key onboarding and backup story |

Our own modules `modules/lez-swap-node` and the role payload packages
`modules/lez-maker-node`, `modules/lez-taker-node` are not upstream patches.
The signer socket `lez_swap_node` serves is owner-only (created under
`umask 077`, `modules/lez-swap-node/src/lez_swap_node_impl.cpp`) and serves one
account. It records what the desk asked for (role, launcher, log and hold
paths; signer socket and account) and the running Node's process group in its
instance directory (`module_data/lez_swap_node/<id>/`), so a reload resumes
them; no key material is recorded.

## Known limitations

Stated plainly; each one is open unless it says otherwise.

1. **The Node is packaged, its configuration is not.** `lez_maker_node` and
   `lez_taker_node` carry the Node and everything it runs, but the role's
   configuration and identity are still Compose environment and mounts at
   fixed paths (`/var/lib/lez/<role>`, `/run/lez/<role>`, `/run-lez-identity`,
   `/run-market`), and the launcher needs host tools (bash, socat, jq, curl,
   python3, util-linux `setpriv`). The binaries are glibc-linked
   (rust:1.96-bookworm). Each package carries one variant, the build host's
   (`linux-amd64` here; the release builds `linux-arm64`). The Node is a
   process the module launches, not code its host runs (packaging note, A3).
2. **One lifecycle owner, with one gap.** `lez_swap_node` owns the Node: it
   dies with the module's host, a group a dead host left is ended before the
   next start, a new host waits while an old one still supervises, and a
   reload resumes the signer and the Node (logos-native run 2026-11-11, and
   the landing checks in its README). Restart and hold are still the module's,
   not the runtime's. `logosctl module stats` shows the module's host, not
   the Node: the runtime samples one pid per module (the `logos_host` it
   launched; liblogos `logos_core_get_module_stats`, process-stats
   `getProcessStats`) and has no way for a module to add its child processes,
   and running the Node inside the host would change the frozen engine's
   process model (packaging note, A3). So `lez_swap_node` reports the Node's
   use itself: `node_stats(role)` gives pid, uptime, restarts, CPU, RSS,
   threads and open fds of the Node and each process of its group, read from
   `/proc` (packaging note, "The Node's resource use"); the acceptance script
   judges it against `/proc` and records `module stats` as an upstream request.
   Basecamp 0.2.0 does not reload a module whose host crashed: the Node then
   stays down (no orphan) until the module is loaded again (a desk reopened,
   `logosctl module load`, Basecamp restarted); in Compose the role's health
   check goes unhealthy.
3. **Packages install one by one, unsigned.** Every package is a release
   package now (`lgx-portable`, variant `linux-amd64`/`linux-arm64`) and
   `logosctl` 0.3.2 and 0.3.0 install each of them, desks included; the
   Basecamp images are installed from the same files, nothing is renamed. Our
   packages' dependencies carry version ranges (`~<version in the release
   set>`, `deploy/scripts/pin-package-dependencies.sh`), so the default
   catalog's other `lez_core` 0.5.1, `lez_indexer_module` 1.2.1 and
   `chat_module` 0.3.0 do not satisfy them (logos-native run 2026-11-11-b,
   S64, S74, S75). Still: a file install resolves no dependencies (logosctl,
   S63), so a user installs the set package by package; the packages are
   unsigned (owner: out of scope, 2026-11-11); the bare dependency names stay
   in the manifest after the ranges for Basecamp 0.2.0, which reads only string
   entries (packaging note, "Release packages and the LEZ network").
4. **Pinned v0.2 artifacts against the official v0.3 procedure.** The images
   run Basecamp 0.2.0 (`48b26c0`) and the headless heartbeat runs logoscore
   0.2.3; the official build-and-run procedure is Logos Testnet v0.3 with
   `logosctl` 0.3.x. `logosctl` 0.3.2 and 0.3.0 install every package of the release
   set and load our core modules (logos-native run 2026-11-11-b); a file
   install still resolves no dependencies and `logosctl` loads no `ui_qml`
   package (item 3 above). Moving LEZ itself to v0.3 is out of scope (#107).
5. **Packaging evidence.** The latest logos-native runs
   (`docs/evidence/logos-native-2026-11-11-c/`) used the release set of the
   combined landing (`packages.tsv` there: `lez_core` 0.4.1-sign.4,
   `lez_swap_node` 0.3.1, `lez_maker_node` / `lez_taker_node` 0.1.0, the desks
   0.2.5, `lez_network` 0.1.0, Chat 0.2.2, Delivery 0.2.0): 116 steps, 13 FAIL
   per role, 9 of them unsigned packages and 4 `logosctl` behaviours classified
   in the 2026-11-11-b README. The 2026-11-10, 2026-11-11 and 2026-11-11-b runs
   are kept for comparison.
6. **Custody boundary.** The LEZ key is a plaintext file under
   `deploy/runtime/lez/<role>/` (mode 0640 in a 0750 directory on Linux, 0644
   on macOS, `deploy/scripts/gen-config.sh`), bind-mounted into the role's
   Basecamp. The desk imports it into a wallet it creates under `/tmp` in the
   container with a random password it does not keep
   (`apps/basecamp/common/lez_role_signer.h`), and imports the key again on
   every desk start. `/tmp` is not a volume, so a recreated container builds
   a new wallet from the file: the file, not the wallet, is the durable copy. Signing is raw 32-byte hash signing (patch register above).
   On the ZEC devnet corridor, `scripts/zec-provision.sh` also copies each
   role's LEZ key into the corridor material; the corridor sidecar signs
   through the wallet socket when it is up.
7. **The LEZ network is an operator package that needs Compose.** Decided as
   option B3 (packaging note): the role packages declare no network module and
   reach the sequencer and indexer their configuration names; `lez_network`
   (the LEZ Network desk) is the operator package, declaring
   `blockchain_module ~0.2.4` and `lez_indexer_module ~1.1.1-rpc.4`, and installs
   with `logosctl` (run 2026-11-11-b, S78–S79). A user of the public testnet
   does not need it. Its desk still hosts nothing without the config paths
   Compose sets.
8. **The public testnet route has not moved.** It no longer needs the
   sequencer proxy for the Nodes and signers (R6; the escrow deployment still
   uses it), but it uses upstream's `indexer_service` and Logos Blockchain node as
   plain containers instead of the modules, and has not been run since the
   Nodes moved into their Basecamps (`docs/testnet-run.md`). The overlay no
   longer starts the local chain with the Basecamps (commit `107599c8`).
9. **Linux packages only.** See R14. The darwin-arm64 build of every package
   exists since PR #126, but no installable darwin package does, and the
   release ships only Linux packages, each with the build host's variant
   (packaging note, observation 10). On a Mac the stack runs in Docker
   Desktop with `linux-arm64` packages: built from nothing and smoke-tested on
   Apple silicon on 2026-11-11 (`docs/evidence/macos-arm64-smoke-2026-11-11/`,
   six of six smoke tests, two of them through the desks), the full
   regression passes there on `main`'s code
   (`docs/evidence/regression-m2-2026-11-12-macos-arm64/`, 93 of 93), and the
   logos-native acceptance walk gives the amd64 verdicts step for step
   (`docs/evidence/logos-native-2026-11-11-arm64/`). That is not a native
   darwin package.
10. **Finality freezes on the devnet.** The upstream freeze
    (logos-blockchain#3643) is reduced by configuration only. The devnet runs
    1 s LEZ blocks and Bedrock `security_param` 2 for speed; against the
    freeze it has a 10^7 sequencer tip and 10^12 per genesis note (commit
    `f0c88084`), and the
    sequencer paying from 40 notes of its own instead of the block producer's
    (commit `88fcabbf`), both in `gen-config.sh`. `regress.sh` and the
    `heartbeat` detect a freeze and the harness moves to a new chain. Since
    commit `6df0fece` both Nodes' health reports the LEZ chain's progress
    (`lez_chain`: last finalized block, when it advanced, the tip) and turns
    degraded with `lez_finality_stalled` after a threshold (600 s by default,
    60 s on the devnet, the heartbeat's rule), and both desks show a line for
    it (commit `1775049b`; issue #112). How often the current code freezes is
    **not verified** by any committed evidence.
11. **A second stack needs its own checkout.** Every stack-wide name (the
    Compose project, the Docker network, `<prefix>-network`, `-sequencer`,
    `-heartbeat`, the explorers, the lanes' projects and containers) derives
    from `LEZ_STACK_PREFIX` and every published port from
    `LEZ_STACK_PORT_OFFSET` (`deploy/scripts/lane.sh`; default `lez` and 0,
    today's names and ports), so two independent stacks run on one host
    (`deploy/README.md`, "a second stack on the same host"). The runtime
    directory is not a variable: the second stack runs from a deploy directory
    of its own (a worktree) and a market root of its own; `gen-config.sh`
    binds that directory and root to the stack.
12. **DCO: closed.** One commit had an author that its sign-off did not
    name; the owner rewrote and force-pushed the branch on 2026-11-11 (same
    tree). Commit ids quoted in older notes and in evidence recorded before
    that day name the old history.
13. **Reviewer entry point.** `submission/README.md` is still the v0.1.1
    M1 + M3 + M6 pack (deck, walkthrough, manifest); it points to the M2
    milestone map, this file and the v0.3.0 release notes, but its media show
    only the BTC pair.
14. **A stale comment in code.** `deploy/scripts/gen-config.sh` cites
    `FREEZE-INVESTIGATION.md`, which is not in the repository.
15. **Zcash network upgrades.** Public Zcash Testnet runs NU7 (consensus
    branch `77190ad9`). The swap engine signs for NU6.3 and NU7 (checked
    against two mined Testnet transactions,
    `crates/zec-swap-sdk/tests/zcash_testnet_vectors.rs`), with the profile
    `public-testnet-v2` and a Testnet corridor provisioner; **no transaction
    of ours has been broadcast on Testnet**. The regtest devnet, where the
    regression runs, is Zebra 7.0.0 with NU7 from height 1 (profiles
    `deterministic-local-devnet-v5` / `-v6`; the NU6.2 profiles keep their
    ids and are refused on this chain), so the matrix and the public lane
    sign for the same upgrade. The Zcash refund of
    `public-testnet-v2` opens after 3,700 blocks, about a day at Testnet's
    present pace. `docs/zcash-testnet-setup.md` says the live corridor is
    pending, and `scripts/test-zcash-testnet-guide-contract.sh` (not in CI)
    fails on it today.
16. **A transaction the sequencer will never execute.** The LEZ v0.2.4
    sequencer admits any transaction and drops an unexecutable one at block
    building without telling its submitter. For an escrow initialization whose
    account already exists, the sidecar now refuses by name before sending,
    and the swap fails with a readable reason after 5 refusals over 60 s
    (`status_detail` in the Taker's swap view); other steps only report
    "submitted N times, not included". The stop is unit-tested, not exercised
    on the stack. A sequencer patch that returns the execution error at
    submission is written but neither built nor adopted (branch
    `agent/seqfeedback`).
17. **Tests read one thing from a log.** `node-e2e.py` counts the Maker's
    lock attempts from the Node's own log file (`LEZ_NODE_LOG_FILE`), not
    from an API. Logos Delivery writes ~12 MB of log a minute into each
    role's container output with three lanes; nothing limits it.
18. **No upgrade between releases before 1.0.0.** The owner decided that
    upgrading from one release to the next is not supported before 1.0.0.
    An operator drains first: withdraw every offer, let each open swap reach
    `completed` or `refunded`, then install the new release on fresh state
    (`deploy/README.md`). The work to support upgrades is issue #133 and
    starts after 1.0.0.

## Pending gates

Each row says what has run and on which commit, and says "not run yet"
where a result is not in; nothing in this document claims such a result.

| Gate | What closes it | State |
|---|---|---|
| Full regression on the release candidate | `deploy/scripts/regress.sh full` (93 tests: `regress.sh list full`) on one frozen commit, results committed under `docs/evidence/` | done on `023c929e`, 2026-11-11: 92 of 93 passed in one run of 134 minutes on three lanes, 0 frozen tries; the one failure (`zec-claim-collision`, a provoked nonce race that went the other way) passed on its second rerun. `docs/evidence/regression-m2-2026-11-11/`. Run again in full on `fix/103-review-items`, 2026-11-12 (97 tests with the #103 scenarios): 96 of 97 in one run of 137 minutes, 0 frozen tries; the one failure, a desk-test step, passed on its rerun after a change to the step. `docs/evidence/regression-2026-11-12/`. The 93 tests of `main` (`af3ac978` packages) on an Apple-silicon Mac, 2026-11-12: 93 of 93, each on its first try, 86 minutes on three lanes, 0 frozen tries. `docs/evidence/regression-m2-2026-11-12-macos-arm64/`. The 97 tests on an Apple-silicon Mac at `f2f9b980` (`main` with PRs #131, #126 and #127), 2026-11-12: 97 of 97, each on its first try, 98 minutes on three lanes, 0 frozen tries. `docs/evidence/regression-2026-11-12-macos-arm64/`. The 97 tests on the v0.3.0 candidate `b4e17cf0` (`main` after PR #135) on an Apple-silicon Mac, 2026-11-12: 97 of 97, each on its first try, 97 minutes on three lanes, 0 frozen tries. `docs/evidence/regression-0.3.0-2026-11-12-macos-arm64/` |
| Public-network evidence (logos-co/rfp#122) | swaps on public Zcash Testnet and a public LEZ testnet with the released artifacts | Not applicable to this release: the public LEZ testnet has moved to LEZ 0.3, which this LEZ v0.2.4 stack cannot talk to; public-network evidence is postponed to the release that ports to LEZ 0.4 (#107). The engine already signs for Zcash Testnet's NU7 and the regtest devnet runs NU7. A legacy v0.2 LEZ endpoint still answers; exploratory work against it is on a follow-up branch and is not part of v0.3.0 |
| CI and release workflow on the final commit | all required PR checks green (DCO needs item 12), then `.github/workflows/release-images.yml` dispatched on the same commit | The release workflow's run on `af3ac978` (43106618999) built every image and failed in its smoke job (`libpcsclite.so.1`); `start.sh`, which that smoke job and every bundle user run, could not finish there. The four faults were found on an Apple-silicon Mac (`docs/evidence/macos-arm64-release-gates-2026-11-11/RESULTS.txt`, section 5, and its rerun on images built from `af3ac978`) and fixed by PR #127, now on `main` as `b6b1c273`. `public-readiness` failed on `b6b1c273` on trailing whitespace in the committed evidence logs, which `.gitattributes` now exempts. CI is green on `b4e17cf0`: `ci` and `public-readiness` both passed. The release workflow's rehearsal on `b6b1c273` (run 43189324839) built every image and failed in its smoke job, when the bundle's `start.sh` tried to build an image from a folder the bundle does not ship; the fix is PR #136. The release workflow has not run on the commit to be tagged yet |
| Native install proof | `deploy/scripts/logos-native-acceptance.sh` on the final packages, or an accepted waiver | run on the combined landing's release set (`docs/evidence/logos-native-2026-11-11-c/`): 13 FAIL per role, every one unsigned packages (out of scope) or upstream logosctl behaviour, classified in `logos-native-2026-11-11-b/`; `module stats` is INFO with an upstream request and the Node's stats are judged through `node_stats`; on `linux-arm64` packages built on a Mac every step has the verdict of the amd64 run (`docs/evidence/logos-native-2026-11-11-arm64/`). No run of the script on the packages of `b6b1c273` is in the repository |
| Topology decision in #115 | Logos agrees R2, R5, R7, R11, R13, R14 and the network-package question | open |

## Open-issue ledger

Issue states were re-checked on GitHub on 2026-11-12, after PRs #131, #126
and #127 were merged; each row says what was run for it and where the record is.

| Issue | Disposition | Evidence or state |
|---|---|---|
| gateway-fm #115: run the stack as Logos modules | Open. This document is the traceability for it; closing needs the decisions under Pending gates | this file |
| gateway-fm #106: organization-owned catalog | Closed 2026-11-11. The catalog is `gateway-fm/logos-modules-release-base`, transferred with its 14 releases; its index lists 28 package URLs, all under the organization; its release, unpublish and index workflows are vendored so they run under the organization's SHA-pinning policy (only the index rebuild has run). The address under a personal account keeps serving until the next release | `README.md`; the issue's closing comment |
| gateway-fm #112: finality and claim visibility | Open. Both findings fixed (PR #120): Node health reports LEZ finality and turns degraded when it stalls, both desks show it (commits `6df0fece`, `1775049b`); a claim or refund that is landing shows "submitted" and no Retry button, with "Resubmit" only after 15 minutes without progress or a reported failure (commit `1433d7b1`). Checked on the devnet by stopping the sequencer (`ui-e2e.sh lez-finality-stall`). The issue's further asks (chain events in the Activity pane, `btc_height`) are not done, so the issue stays open. Re-run on `main` on an Apple-silicon Mac on 2026-11-11: `lez-finality-stall` passed in 572 s | `docs/issue-closing-notes-2026-11-11.md`; `docs/evidence/macos-arm64-release-gates-2026-11-11/RESULTS.txt`, section 3 |
| gateway-fm #113: wipe, then start, cannot trade | Closed 2026-11-12 by PR #127. Run on 2026-11-11 and 2026-11-12: `down.sh --wipe` retires the chain's escrow deployment and manifest and keeps the identities, the bootstrap refuses a deployment recorded for another genesis, a failed bootstrap prints its output and stops both Nodes, and `start.sh` checks the Maker's routes once its desk has an offer. The issue's steps (`down.sh --wipe`, then `start.sh`, `market/` left in place) end with exit 0 and a BTC and a ZEC trade through the desks. Getting there took four more fixes to `start.sh`'s path (PR #127). Run on local images on a Mac, not from a published bundle; the release workflow's smoke job is the check from a bundle on Linux. A second stack on one host: limitation 11 | `docs/evidence/macos-arm64-release-gates-2026-11-11/RESULTS.txt`, sections 4 and 5; `deploy/tests/test_chain_lifecycle.py` |
| gateway-fm #114: Bedrock pin and finality stall | Closed 2026-11-12. The pin: the node is the released `blockchain_module` 0.2.4, sha256-pinned. The stall: open upstream (logos-blockchain#3643), worked around on LEZ v0.2 by configuration (limitation 10); it is to be tested again on LEZ 0.4 (#107) | `deploy/scripts/from-scratch.sh`, `deploy/scripts/gen-config.sh` |
| gateway-fm #103: Logos v0.2.5 review notes | Closed 2026-11-12 (PR #131). Items 1 and 16 (the latent `swap-core` paths) and the ZEC SDK's refund planning fixed with tests; item 3 (route-health example), item 4 (`release.env` on a bundle) and item 5 (reset keeps keys) fixed. Item 6 (the actor's last outcome and what it waits for, in `last_outcome` on both Nodes' swap views and on the desk rows; no Resubmit while a bounded wait holds) and item 7 (the header badge follows the Node's health) fixed in the Nodes and desks. The test items fixed in the harness: 8 (early-lock selling LEZ), 9 (offline-after-lock: a real cut, proven from both sides), 10 (refund-then-maker-lock: what the Taker had seen, both parties' balances), 12 (every wait logs state, finality and tip), 14 (late-claim scenarios in both directions). The rewritten tests found three defects on the stack, fixed with a failing test before each: a Bitcoin Maker with a refund request open stopped observing the Taker's revealing claim (`9579950b`); neither Node found a LEZ claim final past the swap's discovery window (`964cb8c8`); the Zcash path took a counterparty's LEZ lock from the sequencer without finality (`17cfa50a`). Items 6 and 7 were seen on the built stack, the others in two full runs (`docs/evidence/regression-2026-11-12/`). Items 2 and 13 are release-notes items, 11 and 15 partly answered. Item 2 moved to #133 (no upgrade between releases before 1.0.0, limitation 18); items 11 and 13 and the release-text part of 12 moved to #134, which the owner closed as superseded by the 0.3.x evidence (the corrections are in `docs/release-notes-0.3.0.md`); item 15 is #107. Commits, how each was checked and what stays open: | `docs/issue-closing-notes-2026-11-11.md` |
| gateway-fm #52: offer history | Open. Nothing in v0.3.0 changes or proves it: the state filter shipped in v0.2.4 (`56dea0cd`); the snapshot still grows with consumed offers and swap history, and the long-history desk step is not done | `docs/issue-closing-notes-2026-11-11.md` |
| gateway-fm #107: LEZ 0.4 port | Open. Out of scope; the stack stays on LEZ v0.2.4 | R12 |
| gateway-fm #133: upgrade between releases | Open. Not supported before 1.0.0; an operator drains first | limitation 18 |
| gateway-fm #134: v0.2.5 release record corrections | Closed 2026-11-12 by the owner, superseded by the 0.3.x evidence | `docs/release-notes-0.3.0.md`, "Corrections to the v0.2.5 release notes" |
| gateway-fm #128, #129: adopt `logos-zebrad-module`, `logos-bitcoind-module` | Open | R13 |
| gateway-fm #130: Claim offered one confirmation early | Open. Selling ZEC, the Taker Node can offer Claim one Zcash confirmation before its actor accepts it. On a network that keeps producing blocks this resolves by itself with the actor's retry; it shows only on a devnet that mines on demand, and the test harness mines the missing block | issue #130 |
| logos-co/rfp#122: M2 ZEC/LEZ | Open; v0.3.0 is the submission. The committed ZEC evidence is Zebra 7 regtest (NU7) on the private LEZ devnet; the public-network work is on a follow-up branch and has no corridor yet | Pending gates; limitation 15 |
| logos-co/rfp#126: M6 mini-app GUIs, public indexer hosting | Open. v0.3.0 runs the indexer only inside the devnet's Compose-hosted Basecamp; no hosting decision is recorded | R12 |
| logos-co/rfp#123: M3 BTC/LEZ | Accepted at v0.2.6; v0.3.0 does not reopen it and uses it as the regression baseline | — |
| gateway-fm PR #116 | Closed without merge, superseded by #119 and #120 | — |

## Evidence in the repository

| Path | What | Code it ran on |
|---|---|---|
| `docs/evidence/regression-modules-4c-2026-11-10/` | Module regression: every test of the step-4c set has a passing try; failed tries are classified in `RESULTS.txt` (freezes, one Zebra capacity failure, one actor-observation timeout not reproduced) | `4626f8af`, `0f8228d8`; node and indexer then in two Basecamps; before the patch split, the funding fix and the lanes |
| `docs/evidence/logos-native-2026-11-10/` | One run of `logos-native-acceptance.sh`: 95 steps, 25 FAIL, 1 NOT RUN | packages built 2026-11-10 13:13Z, before `22196d30` |
| `docs/evidence/logos-native-2026-11-11/` | `logos-native-acceptance.sh` for the Maker and the Taker with the role Node packages; the landing's lifecycle checks on the live stack; the supervisor harness | the packages of this landing (`lez_swap_node` 0.3.0, `lez_<role>_node` 0.1.0) |
| `docs/evidence/logos-native-2026-11-11-c/` | `logos-native-acceptance.sh` for both roles (116 steps, 13 FAIL each); `node_stats` through each role's Basecamp compared with `/proc` idle and during swaps; the lifecycle checks on the live Maker and Taker; the supervisor harness | `a3b33884` (`lez_swap_node` 0.3.1) |
| `docs/evidence/regression-m2-2026-11-11/` | Full regression of PR #120's head: 93 tests, 92 passed in one run, the one failure passed on its second rerun | `023c929e` (trees in its `PROVENANCE.md`; squashed into `main` as `4528d46f`) |
| `docs/evidence/regression-2026-11-12/` | Full regression of `fix/103-review-items`: 97 tests, 96 passed in one run, the one failure (a desk-test step) passed on its rerun; the first full run of the branch (93 of 97) and the reruns that followed its three defect fixes; the hand checks of #103 items 6 and 7; the seven ZEC demo recordings | tests `2b6569e0`, images `964cb8c8`, rerun `62cf95ca` (trees in its `PROVENANCE.md`) |
| `docs/evidence/regression-m2-2026-11-12-macos-arm64/` | Full regression on an Apple-silicon Mac (Docker Desktop, `linux-arm64` images): 93 tests, 93 passed, each on its first try; seven ZEC demo recordings | Node packages built from `af3ac978` (`main` then; trees in its `PROVENANCE.md`) |
| `docs/evidence/regression-2026-11-12-macos-arm64/` | Full regression on an Apple-silicon Mac: 97 tests, 97 passed, each on its first try; seven ZEC demo recordings (the full set's checksums in `videos-kept-outside-git.sha256`) | `f2f9b980` (`af3ac978` with PRs #131, #126 and #127; trees in its `PROVENANCE.md`) |
| `docs/evidence/regression-0.3.0-2026-11-12-macos-arm64/` | Full regression of the v0.3.0 candidate on an Apple-silicon Mac: 97 tests, 97 passed, each on its first try; seven ZEC demo recordings (the full set's checksums in `videos-kept-outside-git.sha256`) | `b4e17cf0` (`main` after PR #135; the difference from `f2f9b980` in its `PROVENANCE.md`) |
| `docs/evidence/macos-arm64-release-gates-2026-11-11/` | Release-gate checks on an Apple-silicon Mac: resume and Node restart, smoke tier, LEZ finality stall (#112), wipe then `up.sh` and `start.sh` and trade (#113); `RESULTS.txt` says which sections count (the rerun of 2026-11-12) | rerun on images built from `af3ac978`; the first pass's images were from `8bdf0749` |
| `docs/evidence/logos-native-2026-11-11-b/` | `logos-native-acceptance.sh` per role on release packages with version ranges and the network package: 115 steps, 14 FAIL each, every FAIL classified in its README | packages built from `023c929e` |
| `docs/evidence/logos-native-2026-11-11-arm64/` | The same walk on the `linux-arm64` release set built on an Apple-silicon Mac; every step has the amd64 run's verdict | PR #120's head `c1d6f813` |
| `docs/evidence/testnet-20260918/`, `docs/evidence/testnet3-provider-20260918/` | Public testnet swaps (LEZ testnet, Bitcoin testnet4 / testnet3 provider) | before PR #120 |


## Where to read next

- `deploy/README.md`: the stack, services, timing profiles, freezes, lanes.
- `docs/architecture/deployment-components-and-rpcs.md`: processes, modules,
  sockets and keys of the current stack.
- `docs/architecture/logos-native-packaging.md`: what `logosctl` does with
  these packages, and the proposal for the role Node as a payload module.
- `deploy/builder/patches/README.md`: the patches in detail.
- `docs/testnet-run.md`: the public testnet route.
