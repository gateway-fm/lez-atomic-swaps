# The v0.3.0 packages with darwin-arm64, installed from a catalog on macOS, 2026-11-12

The v0.3.0 package set with a `darwin-arm64` variant beside `linux-arm64`,
served from a local HTTPS catalog next to the Logos official catalog, and
installed the way a user installs it: `logosctl package install` of each
desk in a fresh session on an Apple-silicon Mac (macOS 26.7). Both role
Nodes were then started from the installed packages, as the desks start them,
and swapped against the devnet's chains in Docker; the same install ran in a
clean linux-arm64 container. How a Node runs on macOS and what is weaker than
Linux: packaging note,
["The Node on macOS"](../../architecture/logos-native-packaging.md#the-node-on-macos).

## The packages

| package | version | variants | sha256 | root hash |
|---|---|---|---|---|
| lez_atomic_swap_maker | 0.3.0 | darwin-arm64, linux-arm64 | `3a0bd12d5dd96df92181719b0b06cece66ff4eb64b2f552e315b96faa648b5b8` | `e894ea86643a1a4404beb365e2b79264f336319ccf5c6dce0d8a3de1dddb05a7` |
| lez_atomic_swap_taker | 0.3.0 | darwin-arm64, linux-arm64 | `f9bd64ba0db46178245e1b556afd5e5514fcaa66e62cfc42ed4e0030bf6d89e5` | `093034aa6388519ac181af4d36bb3622cc04b5d47a1f3f4b2e07dc1e47121966` |
| lez_swap_node | 0.3.0 | darwin-arm64, linux-arm64 | `653f5a3bf274a6e7845289ee6b008422f054a00716fa9089e3c73c5b745516d7` | `33becb829cfeebe8f4e9168173c01aaa185cc5c6cdb0404dfda16aa7a4499d50` |
| lez_maker_node | 0.3.0 | darwin-arm64, linux-arm64 | `b810ac65eff5c6bf2e28af9fcd38928e11ecbebe57c83ceac636e84dc2f02403` | `5d9a4b1f643a22237beddda0c3fd9c80bb4b660d83c1021f29d1b30f4fc87ce8` |
| lez_taker_node | 0.3.0 | darwin-arm64, linux-arm64 | `dd99c1bffbc3a8b812d5064d7aa2b146b58163123e3246956cacf37f9981155f` | `cfd14f5846e07dc7ab8aec91c96c5fd05d4c09c70095f87dd00d6df47679ed49` |
| lez_core | 0.4.1-sign.4 | darwin-arm64, linux-arm64 | `0978e323e084c1d1674765dade3fd8b4e219793483a13741022b7ee7b348604f` | `c7cae13afe5bf932ddd21cead84b81ac18ba5681243693814b03802c7c1a3e03` |
| lez_network | 0.3.0 | linux-arm64 | `8598a820405a0cc00e88cacd2494334d1eb4e13ab5c949d030b2fa8d6ff6f9af` | `15b63e24a6d5ddaa5c3385d4d4a3756f498752303498997cfaa993b979cf2f72` |
| lez_indexer_module | 1.1.1-rpc.4 | linux-arm64 | `1434c0ff42bb4f51fb37b8ad8103a571a857ab75466cc5506cd030a6864817fb` | `44aedefbe38d6fd021981a2c8fa101441eeb46711867d0b8f784496a6b5edb05` |

Dependencies as declared: `packages.tsv`; `SHA256SUMS` lists the files.
Every package passes `lgx verify` (`build/merge-set.log`).

Where the two variants come from, and what differs in them:

- **linux-arm64**: the packages the v0.3.0 release run built from `14ebc3e5`
  (the role packages' `bin/payload.json` says `build_source` `14ebc3e5`).
  Their files are unchanged; only `manifest.json` was replaced, with the
  darwin build's (keeping their own `main` and `hashes`), so that the two
  manifests are identical as `lgx merge` requires. That changes the role
  packages' description (it named Linux only) and the dependency form; the
  root hash covers the files, not the manifest.
- **darwin-arm64**: built on this Mac from `4bd826e6` with
  `deploy/scripts/build-darwin-packages.sh` (`build/darwin-variants.txt`).
  lez_core is LEZ v0.2.4 with `builder/patches` at the same pins as the
  Linux build, from the same derivation as the earlier darwin build.
- **The code inside.** Between `14ebc3e5` and `4bd826e6` the Rust crates, the
  modules' sources and the desks did not change; the macOS code paths
  (`cfg(target_os = "macos")`: process inspection with libproc, the actor
  artifact copies in place of sealed memfds, the kqueue watcher in
  `lez_swap_node`) were already in `14ebc3e5` and are compiled only into the
  darwin variant. The launcher, `node-entrypoint.sh`, differs: the Linux
  variant carries the `14ebc3e5` launcher, the darwin one the launcher that
  runs under macOS's bash 3.2 and takes per-role settings (identical
  behaviour in the role images: darwin-native-node-2026-11-12-b,
  `linux-equivalence/`). The Linux variant's executables are the images'
  debug builds; the darwin ones are release builds, stripped and signed ad
  hoc, not notarised.
- **Merge.** `scripts/merge-set.sh`: manifests normalised, `lgx merge`
  (logos-package `ab885df3`), the merged manifest restored
  (`restore-merged-manifest.py`: `lgx merge` dropped `display_name`, see
  below), the set pinned once with `pin-package-dependencies.sh`, verified.

lez_network and lez_indexer_module stay linux-arm64 only (re-pinned):
lez_network is the LEZ network operator's desk, not installed in a party's
Basecamp, and its indexer module has not been built or run on macOS. A macOS
install of lez_network fails with the installer's "Package does not contain
variant for platform".

## Results

| Check | Result | Files |
|---|---|---|
| Local catalog beside the official one, fresh `logosctl` 0.3.2 session on macOS | both catalogs resolve, no errors | `install/macos-install.log` |
| `package install lez_atomic_swap_maker -y` | ok; lez_core 0.4.1-sign.4, lez_swap_node 0.3.0, lez_maker_node 0.3.0 and the desk from the local catalog, chat_module 0.2.2, delivery_module, libp2p_module, liblogos_rln_module, liblogos_lez_rln_module from the official one | `install/macos-install.log` |
| `package deps lez_atomic_swap_maker --recursive` | every dependency `installed` except `delivery_module` 0.3.3 `version_mismatch` (required `~0.2.0`) | `install/deps-maker.json` |
| `package install lez_atomic_swap_taker -y`, then `deps` of both desks | ok; the installer moved delivery_module from 0.3.3 to 0.2.1; both desks: every dependency `installed` | `install/deps-taker.json`, `install/deps-maker-after-taker.json` |
| What is on disk | every package `darwin-arm64`; lez_core 0.4.1-sign.4, root `c7cae13a…3e03`; role packages built from `4bd826e6`, signature ad hoc | `install/macos-install.log` (end) |
| The same install in a clean linux-arm64 container (Debian trixie, logosctl 0.3.2 AppImage) | the same resolution; every package `linux-arm64`; lez_core root `c7cae13a…3e03`; role payload from `14ebc3e5` | `install/linux-install.log` |
| The role Dockerfile's variant selection on the merged lez_maker_node, in that container | `aarch64: installs linux-arm64` | `install/dockerfile-variant-check.log` |
| Both Nodes started from the installed packages in that session (`lez_<role>_node launcher`, `lez_swap_node start_node`, signer in lez_core) | both ready; the Node processes run from the session's `modules/lez_<role>_node/bin` | `nodes/` |
| node-e2e `happy`, BTC, TakerSellsForeign | passed, 37 s | `e2e/btc-happy.log` |
| node-e2e `happy`, BTC, TakerSellsLez | passed, 23 s | `e2e/btc-happy-lez.log` |
| node-e2e `zec-happy`, TakerSellsLez | passed, 54 s | `e2e/zec-happy-lez.log` |
| The release Basecamp 0.2.3 on the same packages, desks clicked | pinned manifests: desks fail to load, no Node; a bare name before each pinned entry: both desks load their modules, both Nodes start and report ready | `basecamp-release/` |
| Both desks in Basecamp 0.2.0-RC3 (inspector build) on a copy of the session's `modules/` and `plugins/` | desks fail to load: no core dependency loaded, "Timeout waiting for replica lez_core" | `desks/basecamp-catalog-manifests-excerpt.log` |
| The same with each installed manifest's dependencies as bare names (what the role images do for Basecamp 0.2.0) | both desks open, start their signers and Nodes; `ui-e2e.sh happy` passed, 55 s | `desks/` |

The `delivery_module` result is the accepted one: the official chat_module
0.2.2 declares `delivery_module` without a version, so the first install adds
the newest (0.3.3) beside the 0.2.1 the desk requires, and leaves 0.3.3 on
disk; the second desk's install replaces it with 0.2.1. chat_module 0.2.2 and
delivery_module 0.2.1 and 0.3.3 have darwin-arm64 variants in the official
catalog and installed.

### Basecamp

A `logosctl` session and a Basecamp user directory have the same layout
(`modules/<name>/` and `plugins/<name>/`, each with the variant's files,
`manifest.json` and `variant`). Basecamp 0.2.0 reads only string dependency
entries from an installed manifest; a catalog install keeps the pinned
objects, so Basecamp 0.2.0 loads none of a desk's modules and the desk times
out. The role images write bare names for it; `scripts/bare-dependency-names.sh`
does the same to the copy, and nothing else changed. Basecamp 0.2.0's own
package manager reads manifests with the same parser, so installing through
it was not tried. The release Basecamps: next section.

### Found on the way (first attempt)

- **`lgx merge` dropped `display_name`.** From the first merged set, Basecamp
  listed the desks by package name (`first-attempt/sidebar-without-display-name.png`)
  and lez_core had lost "LEZ Core Module". Fixed by
  `deploy/scripts/restore-merged-manifest.py`, which release-packages.yml now
  runs; every result above is from the fixed set.
- **The workflow's lgx refused pinned manifests.** logos-package `ff93a0df`
  fails `lgx verify` on a versioned dependency ("Invalid dependency entry (not
  a string)"); release-packages.yml now uses `ab885df3`.
- **One TakerSellsLez swap failed on the Maker** in the first session
  (`first-attempt/btc-happy-lez.log`): the Maker's actor supervision recorded
  `actor_deployment_invalid` on the swap's first attempt, in the same second
  the swap was registered and 4 s after the previous swap's actor finished;
  the swap stayed `Offered`. That class is terminal and comes from the
  artifact checks before the actor runs (the per-swap config and program
  hashes, the config's role, swap and state path, the state file's owner,
  mode and link count; a failure to stage the program copy is a retry, not
  this class). Ruled out: a changed actor binary. Nothing was reinstalled
  between the take and the failure, and the swap's record pins the program
  sha256 `cd7bd733…6f91` and the config sha256 `3201104d…48be`, both equal to
  the files on disk afterwards; the actor's own `status` with that config
  succeeds. Not known: which check failed. The Maker logs no line for a
  supervision failure (its store keeps only the class), so the cause cannot
  be read back; a race between the swap's registration and its first
  attempt (the state file appearing between two of the checks) fits the
  timing but is not shown. The next TakerSellsLez swap in that session, and
  the one above, passed.

## Release Basecamp and the manifest form

The question: does a desk installed from the catalog, with the pinned
(object) dependencies, load in the Basecamp releases users run?

**Sources.** Basecamp 0.2.3 is logos-basecamp `aa237766` with
logos-package-manager `202af6fa` (its own build-commit banner,
`basecamp-release/pinned-manifests.log`). Its installer writes the package's
root manifest unchanged into the installed module (`package_manager_lib.cpp`,
"Write the full root manifest.json"), so its own install gives the same
manifest a logosctl install gives. Its scan of installed modules keeps a
dependency only `if (d.is_string())` (`package_manager_lib.cpp` around line
500): an object entry is dropped. `UIPluginManager` hands that list to
`PluginLoader::loadCoreDependencies`, which loads `dep.toString()` for each:
with only objects, no core module is loaded before the desk. The latest
release, Basecamp 0.3.2 (`release/0.3.2`, `f2fae63`, 2026-10-06), reads both
forms: its package manager (`4771a648`) takes `name` from a string or from an
object (`readDeps`), and its `PluginLoader` reads each entry with
`readDependencyEntry` ("Bare name or {name, version, signer}").

**The app, 0.2.3** (`/Applications`, Developer ID, notarised), on its default
profile (backup beside it), modules and plugins replaced by the catalog
session's (the same files its installer writes; installing through its UI
against the private-CA catalog was not done), started with the environment
of `scripts/env-both.sh` (`scripts/start-basecamp-release.sh`), desks opened
by clicking their sidebar buttons through System Events (this host's
Accessibility permission allowed it):

- Pinned manifests as installed: "Loading UI module", no "Loading core
  dependency", "Timeout waiting for replica lez_core", "Failed to load UI
  module ... Timeout waiting for ui-host"; no lez_core, lez_swap_node or role
  module host, no Node process (`basecamp-release/pinned-*`).
- Each pinned entry preceded by its bare name (`scripts/string-then-object.sh`):
  every core dependency loaded for both desks, both Nodes started from the
  profile's `modules/lez_<role>_node/bin` under the Basecamp's `logos_host`,
  both owner sockets answer `ready: true`
  (`basecamp-release/string-then-object*`). The Maker reported
  `lez_finality_stalled`: the LEZ sequencer did not produce blocks after the
  chain containers were started again for this check, so no swap ran here.
- Basecamp 0.3.2 was downloaded but not run: launching it was not permitted
  on this host.

**logosctl 0.3.2** with four forms of one dependency on lez_core
(`manifest-forms/`):

| form | plan | lez_core on disk |
|---|---|---|
| `[{lez_core, 0.4.1-sign.4}]` | ours only | 0.4.1-sign.4 (ours) |
| `["lez_core"]` | official 0.5.1 | 0.5.1 |
| `["lez_core", {lez_core, 0.4.1-sign.4}]` | both: ours and the official 0.5.1 | ours (3 of 3 fresh sessions) |
| `[{lez_core, 0.4.1-sign.4}, "lez_core"]` | both | official 0.5.1, `version_mismatch` |

A string entry is a name only (logos-package `manifest.cpp`: "Legacy
plain-string form: equivalent to { name }"); there is no exact-version string
form. So no single form gives all three consumers what they need: the object
form is exact for logosctl and Basecamp 0.3.x and loads nothing in Basecamp
0.2.x; the bare name loads in every Basecamp and installs the official
lez_core; a bare name before the object loads in Basecamp 0.2.3 and leaves
our lez_core on disk with logosctl 0.3.2, but the installer also fetches and
installs the official 0.5.1, and which one stays depends on entry order. The
role images (Basecamp 0.2.0) do not depend on the published form: they write
bare names when the image is built.

To watch the 0.2.3 profile by hand (desks opened by a click on "LEZ Maker"
and "LEZ Taker"):

```sh
grep -E 'Loading core dependency|Failed to load UI module' "$PROFILE"/logs/basecamp_*.log | tail
pgrep -fl 'lez-(maker|taker)-node'
curl -s --unix-socket "$LEZ_NODE_HOME/run/maker/node.sock" -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"maker_health","params":[{}]}' http://x/
curl -s --unix-socket "$LEZ_NODE_HOME/run/taker/node.sock" -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"taker_health","params":[{"schema_version":1}]}' http://x/
tail "$LEZ_NODE_HOME"/maker.out.log "$LEZ_NODE_HOME"/taker.out.log
```

## Inputs

- Hosts: logosctl 0.3.2 for macOS (`96e704df`) with a session of its own;
  logosctl 0.3.2 AppImage (linux-arm64) in a throwaway container; Basecamp
  0.2.0-RC3 with the QML inspector, built on this Mac from the basecamp flake.
- Catalog: `scripts/serve-catalog.py` on `https://localhost:8443`, index
  built with the catalog tool's `index.py build` (`catalog/`), private CA.
  The daemon got `SSL_CERT_FILE` = the system roots plus that CA; a published
  catalog needs no such setting. The container reached it through a `socat`
  forward to the host.
- Chains: the v0.3.0 stack's chain containers (lane 1); its Node containers
  stayed stopped. Identities `maker-munich-01`, `taker-zurich-01` and the
  market bootstrap manifest copied from the stack's `runtime/`.
- Harness: `node-e2e.py` and `ui-e2e.sh` from this checkout with
  `LEZ_E2E_NATIVE_RUN_ROOT` / `LEZ_UI_NATIVE_RUN_ROOT` (`scripts/run-suite.sh`);
  the ZEC provisioning tools from a host build of the same sources.

Paths are replaced by `$WORK` (this run's directory: session, Node home,
catalog), `$LOGOSCTL`, `$CHECKOUT`, `$DEPLOY` (the stack's checkout),
`$BASECAMP_BUILD`, `$MERGED`, `$RELEASE_PACKAGES`, `$TOOLS`, `$PROFILE` (the release Basecamp's profile), `$TMPDIR` and
`$HOME`. The scripts read `LOGOSCTL`, `SESSION`, `LEZ_NODE_HOME`, `DEPLOY`,
`CA_BUNDLE`, `BASECAMP` and `USER_DIR` from the environment. Keys are never
printed (scanned before committing).

Not run: anything on linux-amd64 (no variant yet); Basecamp 0.3.2; an install
through Basecamp's own package manager UI; the Node scenarios other than `happy` and
`zec-happy`; a run of release-packages.yml on GitHub's runners.
