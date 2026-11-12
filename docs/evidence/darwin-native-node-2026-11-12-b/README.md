# Native role Nodes on macOS, 2026-11-12 (b): one host, ZEC, refunds, desks

Both role Nodes run natively on an Apple-silicon Mac (macOS 26) from their
darwin-arm64 Logos packages, both in one Logos host; the chains (LEZ
sequencer and indexer, Bitcoin Core, Zebra) are the devnet's Docker
containers on their published loopback ports. How it works and what is
weaker than Linux: packaging note,
["The Node on macOS"](../../architecture/logos-native-packaging.md#the-node-on-macos).
The first set, [darwin-native-node-2026-11-12](../darwin-native-node-2026-11-12/README.md),
ran one Logos session per role.

## Inputs

- Code: the role packages were built from `307c16f2`
  (`bin/payload.json` `build_source` `307c16f2a3a70253ededb4eae192cdd230fa49cf`,
  sidecar tree `aead716920300ecc8c4b5d01779ff8022c664b27`; `packages.txt`)
  with `deploy/scripts/package-role-node-darwin.sh`, on the darwin-arm64
  plugin packages the module builder built from `14ebc3e5`. `lez_swap_node`
  0.3.0, `lez_core` 0.4.1-sign.4, `chat_module` 0.2.2 and `delivery_module`
  0.2.0 are the module builder's darwin-arm64 `lgx-portable` builds; the
  desks 0.3.0 likewise. The scenario harness ran from `307c16f2`, the desk
  steps (`ui-e2e.sh`, `verify.mjs`) from `83199361`.
- Hosts: `logosctl` 0.3.2 for macOS, one session for both roles
  (`session-both.sh`, environment `env-both.sh`), and Basecamp 0.2.0-RC3
  with the QML inspector, built on this Mac from the basecamp flake's
  `bin-bundle-dir-inspector` (the version the devnet's images run), both
  desks in one process (`start-basecamp-both.sh`, user directory staged by
  `stage-user-dir.sh`). Hosts started with `PATH=/usr/bin:/bin:...`: the
  launcher ran under `/bin/bash` 3.2.
- Chains: the v0.3.0 stack's chain containers; its Node containers stayed
  stopped. Identities `maker-munich-01`, `taker-zurich-01` and the market
  bootstrap manifest from the stack's `runtime/`.

Paths are replaced by `$LEZ_NODE_HOME`, `$SESSIONS`, `$BASECAMP`, `$OUT`,
`$MODULES`, `$CORE`, `$LOGOSCTL`, `$DEPLOY`, `$CHECKOUT`, `$LOGS`, `$TMPDIR`
and `$HOME`. The wallet's `create_new` result is redacted; keys are never
printed (scanned before committing).

## Results

| Check | Result | Files |
|---|---|---|
| Both Nodes in one `logosctl` session, one `lez_swap_node`, one `lez_core` wallet with both accounts | both ready, not degraded; Maker ports from 19000, Taker from 19500 | `one-session/start.txt` |
| BTC `happy` in that session, both directions | passed, 35 s and 27 s | `one-session/` |
| BTC `happy`, `restart-taker`, `restart-maker`, `survivor`, `taker-refund`, `maker-refund` (fast profile) | all passed (43, 39, 44, 60, 423, 461 s); the Node restarted by ending it under its supervisor, stopped with its hold file | `scenarios/btc-*.log`, `scenarios/*.json` |
| ZEC corridor for native Nodes (`zec-provision.sh --native-home`) and `zec-happy`, both directions | passed (65 s, 54 s); funding and claim found in Zebra blocks; the Maker still reports the swap completed after a restart | `scenarios/zec-happy-*.log` |
| `zec-restart-taker`, `zec-restart-maker`, `zec-survivor`, `zec-maker-refund` | all passed (79, 65, 85, 271 s) | `scenarios/zec-*.log` |
| Both desks in one native Basecamp, Nodes started by the desks (signer in `lez_core`, Node through `lez_swap_node`) | both desks "Node connected", balances, orders; both signer status files "serving" | `desks/desk-launched-*.png` |
| BTC swap driven through the two native desks (`ui-e2e.sh happy`: Maker desk publishes, Taker desk takes, locks, claims; both desks show completed) | passed in both directions (46 s, 57 s) | `desks/ui-happy*.log` |
| The role keys in any process's arguments while the desks opened, imported the keys and started the Nodes | 0 of 2149 samples of every process's arguments (the same scanner found the key in 22 of 57 samples when a process carried it: `keyscan-control.txt`) | `desks/keyscan-*.txt` |
| Basecamp quit (SIGTERM) | both Nodes and their sidecars gone within 2 s | `desks/basecamp-quit.txt` |
| The release Basecamp 0.2.3 (`/Applications`) with the same packages in its profile | it lists both desks and opens them (the Maker desk talks to Delivery and its Node); it has no QML inspector and this host gave no screen-capture permission, so nothing was driven or captured there | `desks/attach-*.png` are the RC3 build attached to the same Nodes |

Container behaviour unchanged (`linux-equivalence/`):

- `launcher/`: the launcher of `14ebc3e5` (`old-*`) and of this set
  (`new-*`) in `lez-maker-node:local` and `lez-taker-node:local`, with the
  Node, its CLI, `curl` and the ZEC sidecar faked, both roles, plain, with a
  signer socket and log file, and with a prepared ZEC corridor: identical.
- `zec-provision/`: `zec-provision.sh` of `14ebc3e5` and of this set with
  fakes of `docker`, `curl`, `date` and Zebra, both directions, with and
  without the restart: the same calls, output and files; all exit 0.
- `ui-e2e/`: `ui-e2e.sh happy` of `14ebc3e5` and of this set with fake
  `docker` and `date`, both directions: identical.

Not run: the scenarios that cut a loopback forward or the Chat socket in a
Node container (`late-claim-*`, `early-lock`, `offline-after-lock`,
`refund-then-maker-lock` and their ZEC twins, `zec-taker-refund`): a native
Node has no forward to cut; `concurrent`, `replay`/`wrong-inputs` for ZEC;
the desk journeys other than `happy`; anything in the release Basecamp
beyond opening the desks.

Seen: the Taker desk lists ZEC swaps whose corridor a later provisioning
archived as "Needs attention" (`desks/desk-launched-taker.png`), while the
Maker desk shows the same swaps completed or refunded
(`desks/desk-launched-maker.png`).
