# Installing the Maker daemon

The Maker daemon is `lez-maker-node`: one long-running process that publishes the Maker's
offers, agrees swaps with Takers and carries each swap to its end without anyone at a screen.
It runs in one of two ways, and this page says how to install each and how they differ.

- **Hosted by Logos core.** The Node is the payload of the Logos package `lez_maker_node`, and
  the Logos module `lez_swap_node` starts it and keeps it running. The host is Logos Basecamp
  with the Maker desk open, or a headless `logosctl` session. The Maker desk controls the Node
  from the same host.
- **Standalone under systemd.** The Node is the main process of the hardened unit
  `lez-maker-node.service`. No Logos runtime is involved. This is the fallback where Logos
  core does not run, and the step-by-step guide is
  [maker-systemd-install.md](maker-systemd-install.md).

Both modes run the same `lez-maker-node` binary through the same role launcher
(`modules/lez-role-node/node-entrypoint.sh`), which renders the Node's configuration from
environment settings and files and then becomes the Node. Both expose the same owner socket,
so [`lez-maker-cli`](cli-reference.md) and the [owner API](api/README.md) work the same in
both.

## How the two modes differ

| | Hosted by Logos core | Standalone under systemd |
|---|---|---|
| Who starts and restarts the Node | `lez_swap_node`: it restarts the Node two seconds after it exits and ends the Node when its host goes | systemd: `Restart=on-failure`, `RestartSec=5s`, at most three starts in 60 seconds |
| What is installed | the packages `lez_maker_node` (the Node, its CLI, its actors, the launcher), `lez_swap_node` and `lez_core`, and for the desk `lez_atomic_swap_maker` | binaries and the launcher in `/usr/bin`, the unit in `/usr/lib/systemd/system/` (`scripts/install-maker-node-service.sh`) |
| Configuration | the environment the host (Basecamp or the `logosctl` daemon) was started with | `/etc/lez/maker/env`, read by the unit |
| The LEZ account key | in Basecamp's LEZ wallet (`lez_core`). `lez_swap_node` answers the Node's signing requests on an owner-only socket; the key does not reach the Node | the encrypted credential `lez-signer.key`, decrypted by systemd at each start and read where it is |
| The Maker's other keys | owner-private files in the role's identity and state directories. The launcher creates the Delivery signing key (`delivery-signing.key` in the state directory) and the Zcash claim-recovery key on its first start when none exists, and never replaces a key it created | encrypted credentials (`delivery-signing.key`, `maker-claim-recovery.key`) |
| Stop and start | the role's hold file, or `stop_node` and `start_node` of `lez_swap_node`; `lez-maker-cli stop` does not apply | `lez-maker-cli stop` and `start` as root, or `systemctl` |
| Logs | the file named by `LEZ_MAKER_NODE_LOG` (in the role container, the container's log) | the journal: `journalctl -u lez-maker-node` |
| Remote control | the Maker desk, and anything else that opens the owner socket | anything that opens the owner socket; no desk |
| Pairs the packaging carries | Bitcoin, Zcash and Monero: the package holds all three actors and the Monero effect tools | Bitcoin and Zcash as installed; Monero through an optional drop-in that was checked statically and with stub programs, not run in the VM |
| Platforms | Linux (amd64, arm64) and macOS on Apple silicon | Linux with systemd 255 or later |
| Sandbox | the host's user, no extra confinement | the unit's hardening (`ProtectSystem=strict`, no capabilities, a system-call filter) |

In both modes a swap survives a stop, a crash or a restart of the Node: its state is in the
Node's SQLite database and the Node resumes it ([swap coordinator](swap-coordinator.md)).

## Hosted by Logos core

### The parts

- **`lez_maker_node`** (`modules/lez-maker-node`) is a payload package. Its plugin has two
  methods and starts nothing: `launcher()` answers the absolute path of the installed launcher
  (`<module>/bin/node-entrypoint.sh`), `payload()` answers `bin/payload.json`, which lists each
  installed file with its sha256. Under `bin/` it carries `lez-maker-node`, `lez-maker-cli`,
  the Chat gateway, the price worker, the Bitcoin, Zcash and Monero actors, the Monero effect
  tools, the LEZ sidecar, the Monero Stage-A planner and the health probe
  (`deploy/scripts/package-role-node.sh`).
- **`lez_swap_node`** (`modules/lez-swap-node/src/lez_swap_node_impl.h`) is the Node's one
  lifecycle owner. Its methods:

  | method | what it does |
  |---|---|
  | `start_node(role, launcher, log_file, hold_file)` | supervises `<launcher> <role>` in its own process group, output appended to `log_file`. Returns 0 when supervised, 1 for invalid arguments (a launcher that is not an absolute executable path, or a log or hold path that is unset or relative). A role already supervised is left as it is |
  | `stop_node(role)` | SIGTERM to the Node's process group, SIGKILL after 10 seconds; stops supervising and forgets the role |
  | `node_status(role)` | `{role, state, pid, starts, last_exit}`; `state` is `running`, `held`, `restarting` or `stopped`. After the launcher refused to start (exit 78), `last_exit` carries the launcher's line that says what to fix |
  | `node_stats(role)` | the Node's process group read from `/proc`: per process and summed CPU, memory, threads and descriptors, and uptime, starts and restarts |
  | `serve_signer(socket_path, account_id_hex)` | answers the Node's LEZ signing requests for that one account on an owner-only socket; every signature is made by `lez_core` |

  A `start_node` or `serve_signer` request is recorded and resumed when the module is loaded
  again. The Node's process group is recorded too, and a group a dead host left behind is
  ended before a Node is started, so there are never two.
- **`lez_core`** is the LEZ wallet as a Logos module; it holds the Maker's LEZ account key.
- **`lez_atomic_swap_maker`** is the Maker desk (`apps/basecamp/maker`), a Basecamp
  application that depends on the three packages above and on the Chat and Delivery modules.

### In Basecamp, with the Maker desk

1. Install the desk from the module catalog. One install brings the desk, `lez_maker_node`,
   `lez_swap_node`, `lez_core` and the Chat and Delivery modules
   ([apps/basecamp/README.md](../apps/basecamp/README.md#install-a-desk-from-the-module-catalog)).
2. Start Basecamp with the Node's settings in its environment (below). An installed desk
   starts a signer and a Node only from that environment; without it the desk opens and shows
   "Node unavailable".
3. Open **LEZ Maker**. When the desk loads it has `lez_core` open the wallet and import the
   Maker's key, has `lez_swap_node.serve_signer` serve the signer socket
   (`apps/basecamp/common/lez_role_signer.h`), asks `lez_maker_node.launcher()` for the
   launcher and calls `lez_swap_node.start_node("maker", launcher, LEZ_MAKER_NODE_LOG,
   LEZ_MAKER_NODE_HOLD)` (`apps/basecamp/common/lez_role_node.h`).

The Node keeps running while the desk's view is closed, for as long as Basecamp runs. In the
Docker devnet each party's container is this setup: `lez-maker-node` is a Basecamp with the
Maker desk, and its Compose environment is the configuration (`deploy/compose.yaml`).

### Headless, with `logosctl`

The same calls without a desk, as `deploy/scripts/logos-native-acceptance.sh` makes them (the
catalog is added as in
[apps/basecamp/README.md](../apps/basecamp/README.md#install-a-desk-from-the-module-catalog)):

```sh
logosctl daemon start --detach                # with the settings below in its environment
logosctl package install lez_maker_node -y    # from the catalog; brings lez_swap_node and lez_core
logosctl module load lez_maker_node
logosctl module load lez_swap_node
logosctl call lez_maker_node launcher         # prints <module>/bin/node-entrypoint.sh
logosctl call lez_swap_node start_node maker <launcher> <log file> <hold file>
logosctl call lez_swap_node node_status maker
logosctl call lez_swap_node node_stats maker
```

Without the desk nothing calls `serve_signer`. Either make that call (`logosctl call
lez_swap_node serve_signer <socket> <account id>`, after the key is in the `lez_core` wallet),
or leave `LEZ_MAKER_WALLET_SIGNER_SOCKET` unset so the Node reads `lez-signer.key` from its
identity directory. `logosctl call` takes its arguments on the command line, so a key imported
that way is briefly visible to other processes of the same user. The acceptance script runs
these steps in a clean session without the role's configuration: there the launcher starts and
stops at once, and what is judged is that the package installs with its dependencies and that
`lez_swap_node` launches the Node from it. A Maker that trades needs the settings below.

### What the launcher needs

The launcher reads the environment of the process that hosts `lez_swap_node`. In the role
container the defaults are the container's mounts; a native host names the paths. `<ROLE>` is
`MAKER`.

| for | settings |
|---|---|
| the supervisor | `LEZ_MAKER_NODE_LAUNCHER=package`, `LEZ_MAKER_NODE_LOG` and `LEZ_MAKER_NODE_HOLD` (absolute paths, both required; the hold file need not exist) |
| the desk | `LEZ_MAKER_RPC_SOCKET` (the Node's owner socket), `LEZ_MAKER_SIGNER_SOCKET`, `LEZ_MAKER_SIGNER_IDENTITY`, `LEZ_WALLET_DIR`, `LEZ_WALLET_SEQUENCER_URL` |
| the Node's files | on macOS `LEZ_NODE_HOME` holds them all; on Linux `LEZ_MAKER_NODE_STATE_DIR`, `LEZ_NODE_RUN_ROOT`, `LEZ_MAKER_NODE_IDENTITY_DIR`, `LEZ_MARKET_MANIFEST`, `LEZ_BTC_RPC_COOKIE_FILE`, `LEZ_DELIVERY_DIR`, `LEZ_ZEC_STATE_DIR` |
| the Node | `LEZ_V02_CHANNEL_ID`, `LEZ_MAKER_SIDECAR_PORT_BASE`, `LEZ_MAKER_WALLET_SIGNER_SOCKET`, `LEZ_MAKER_BTC_WALLET`, `LEZ_BTC_CLAIM_DESTINATION`, the timing profile |
| the chains | `LEZ_SEQUENCER_URL`, `LEZ_INDEXER_URL`, and the chain hosts when they are not on loopback |

The Maker's Zcash key is its Delivery signing key: one key signs its offers,
signs its side of every Zcash agreement and is the key it funds Zcash from when
a Taker sells LEZ. A Maker may supply it (`zcash.key` in its identity
directory, as the devnet does); otherwise the launcher creates it on the
first start and says so in the Node's log. A key the launcher has just
created holds no coins: `lez-maker-cli balances --zcash` prints its address,
and a Zcash offer in which the Taker sells LEZ can be funded only once that
address has been paid. Each Zcash swap is agreed with the Taker's Node when
the offer is taken: nothing is prepared for a swap ahead of the take, and the
Maker never holds the swap's secret before the Taker reveals it by claiming.
The Maker Node is given its Zcash role (`--zec-role-config`, which the
launcher writes) and no actor template: `--zec-source-maker-config` no longer
exists, and a start that names it is refused as an unexpected argument.

Every setting, its default and the platform differences are in
[logos-native-packaging.md](architecture/logos-native-packaging.md) ("The Node on macOS",
"Chains on another host"). The directories that hold the Node's files must not be writable by
their group or by others; the launcher refuses to start otherwise (exit 78) and names the
directory and the fix in the log, in `node_status` and on the desk.

### What the Maker desk controls

The desk talks to the Node over its owner socket (`LEZ_MAKER_RPC_SOCKET`) and to
`lez_swap_node` through Logos core. Its backend
(`apps/basecamp/maker/src/lez_atomic_swap_maker.rep`,
`lez_atomic_swap_maker_backend.cpp`) has eleven slots. The desk's view
(`apps/basecamp/maker/src/qml/Main.qml`) calls seven of them:

| on the desk | backend slot | what reaches the Node |
|---|---|---|
| the header badge, **Check Node**, and a background read | `health` | `maker_health` |
| the offers, orders and balances shown, **Refresh market**, and a background read | `btcMarket` | reads only: `maker_pair_list`, `maker_local_price_list`, `maker_offer_list`, `swap_history`, `maker_actor_monitor_v1`, `maker_wallet_balances_v1` |
| **New offer**, then **Publish offer** | `btcPublishOffer` | `maker_local_route_save_v1` (the route's policy and price in one transaction), then `maker_offer_publish_v1` |
| **Withdraw** on an open offer | `btcWithdrawOffer` | `maker_offer_list`, then `maker_offer_withdraw` |
| the action button of a swap row | `btcSwapAction` | nothing is sent: the slot checks the request and answers a fresh market read. The Node funds, claims and refunds by itself, and the Maker's rows carry no action |
| **Status** in the Chat panel | `chatStatus` | nothing: it reads the desk's own Logos Chat session |
| **Reset** in the Chat panel | `resetChat` | nothing: it resets the desk's own Logos Chat session |

Four slots exist on the backend and are not called by the desk's view: `history`
(`swap_history`), `monitor` (`maker_actor_monitor_v1`), `claim` (`maker_actor_claim_v1`) and
`refund` (`maker_actor_refund_v1`). The desk has no button for a manual claim or refund; use
`lez-maker-cli claim` and `refund` ([CLI reference](cli-reference.md#manual-claims-and-refunds)).

Lifecycle: the desk starts the signer and the Node when it loads (`serve_signer`,
`start_node`) and reads `node_status` to show the launcher's refusal under "Node unavailable".
It has no control that stops or restarts the Node and does not call `stop_node` or
`node_stats`.

### Stop, start and upgrade

- **Hold the Node down, keep Basecamp.** Create the file `LEZ_MAKER_NODE_HOLD` names and end
  the Node's process group; `lez_swap_node` reports `held` and does not start it while the
  file exists. Remove the file and the Node starts within a second. In the Compose stack
  `deploy/scripts/node-hold.sh hold maker` and `release maker` do this.
- **Stop supervising.** `logosctl call lez_swap_node stop_node maker`. The role is forgotten;
  `start_node` supervises it again.
- **Stop everything.** Quit Basecamp or stop the `logosctl` daemon: the Node ends with its
  host. In the Compose stack `docker stop lez-maker-node` stops desk, signer and Node.
- **Upgrade.** Install the new packages and load the modules again. The Node's state is kept.
  Each open swap pins the SHA-256 of the actor program it was created with, so after an
  upgrade that changes the actor an open swap is moved to the new one with
  `maker_actor_repin_v1` ([owner API](api/README.md#maker-method-reference)).

Basecamp does not reload a module host that crashed. After such a crash the Node stays down,
with nothing left running, until the module is loaded again.

## Standalone under systemd

[maker-systemd-install.md](maker-systemd-install.md) is the guide: prerequisites, the install
scripts, the three credentials, the environment file, the optional Monero drop-in, start,
check, stop and upgrade. Every command in it except the Monero section ran in an Ubuntu 24.04
VM with a Bitcoin and a Zcash swap between the Maker and Taker units
(`scripts/run-maker-node-systemd-vm.sh`). In short:

```sh
useradd --system --no-create-home --shell /usr/sbin/nologin lez-swap
SOURCE_BIN_DIR=target/debug SIDECAR_PROGRAM=<sidecar> ./scripts/install-maker-node-service.sh
# credentials, /etc/lez/maker/env: see the guide
systemctl daemon-reload && systemctl start lez-maker-node
runuser -u lez-swap -- lez-maker-cli health
```

The unit (`packaging/systemd/lez-maker-node.service`) is `Type=notify`: systemd counts the
Node as started when it reports its owner socket and database ready. State is in
`/var/lib/lez/maker/`, the owner socket is `/run/lez/maker/node.sock`.
[packaging/systemd/README.md](../packaging/systemd/README.md) says how to make a packaged Maker
report an unreachable chain (route health).

## After either install

```sh
lez-maker-cli health                 # ready, degraded, delivery, chat, routes
lez-maker-cli configure-pair …       # then set-local-price, publish-offer
lez-maker-cli offers
lez-maker-cli history
```

Run the CLI as the Node's user; another user is refused at the socket
([CLI reference](cli-reference.md#what-both-clis-share)). What the Maker can do while Logos
Delivery or Chat is unavailable is in [delivery-chat-outage.md](delivery-chat-outage.md).
