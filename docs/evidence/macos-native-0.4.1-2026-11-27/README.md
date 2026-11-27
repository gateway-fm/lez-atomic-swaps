# XMR/LEZ swaps with a native macOS role Node, 0.4.1 candidate, 2026-11-27

In 0.4.1 the darwin-arm64 role packages carry the Monero pair, and the Maker's Monero effect custody has a macOS path (PR #204, `2373e287`). This set shows XMR/LEZ swaps settling with one role Node running natively on an Apple-silicon Mac, from the CI-built candidate darwin-arm64 package. The other role ran in its container from the candidate release bundle, on the same Monero regtest, LEZ and Bitcoin chains.

The native Maker ran its own Monero effect tools on macOS against a live `monerod`. It funded the Monero leg, verified it, submitted tag 15 for the LEZ claim, swept the Monero back after the Taker's refund, and submitted tag 17 for the punish. It ran with the official macOS `monero-wallet-rpc`.

Inputs, hashes and image build sources are in [PROVENANCE.md](PROVENANCE.md).

## Host

- An Apple-silicon Mac (arm64, 64 GB) on macOS 26 (Darwin 25.6.0).
- Docker Desktop for the chains and the other role.
- `logosctl` 0.3.2 for macOS as the native role's Logos host.

## Setup

1. **Stack.** The candidate bundle `lez-swap-stack-sha-2373e28735b7-arm64`, started with `LEZ_STACK_PREFIX=lez041n LEZ_MONERO=1 LEZ_TIMING_PROFILE=fast ./scripts/start.sh`. Its desk suites passed. The same stack first ran the bundle's whole XMR regression with both roles in their containers ([../bundle-xmr-0.4.1-2026-11-27/RESULT.md](../bundle-xmr-0.4.1-2026-11-27/RESULT.md)).
2. **One role native** (`scripts/up-native.sh`):
   - **Container stopped.** The role's container is stopped, since the native Node takes the same identity.
   - **`LEZ_NODE_HOME` staged** from the stack's `runtime/` (`scripts/stage-native-home.sh`): `identity/<role>/` with the role's two Monero logins, `market-bootstrap.env`, the Bitcoin and Zebra cookies, all 0600 in 0700 directories. This is the layout of the packaging note ("The Node on macOS").
   - **Wallet-rpc on the Mac.** The official macOS `monero-wallet-rpc` 0.18.5.1 runs natively on 127.0.0.1:18083, the endpoint the Node's Monero role is fixed to. It uses the role's login and talks to the stack's `monerod` on 127.0.0.1:18081. The role's Monero wallet is a new one in that wallet-rpc, created and funded on regtest by the harness. The stack's funding wallet-rpc, which publishes 127.0.0.1:18083 by default, was recreated on 18183.
   - **Logos host.** One `logosctl` session per role, as the role's desk does it headlessly (`scripts/start-native.sh`, `scripts/env-native.sh`):
     - `package install` of `lez_core`, `lez_swap_node` and `lez_<role>_node`, from the candidate packages;
     - `module load lez_<role>_node`;
     - `lez_core create_new`, then `import_public_account` of the role's key;
     - `lez_swap_node serve_signer`, so the Node signs through the signer socket;
     - `lez_<role>_node launcher`, then `lez_swap_node start_node`.
     The daemon ran with `PATH=/usr/bin:/bin:/usr/sbin:/sbin`, so the package's launcher ran under `/bin/bash` 3.2.
3. **Between the native role and the container** (devnet "local" transport). The Taker reads the Maker's Delivery directory and connects to the Maker's Chat socket, which Compose shares through volumes.
   - **Delivery directory:** shared by a bind mount of the native home into the container (`scripts/*-with-native-*.yaml`).
   - **Chat socket:** a container cannot connect to a macOS Unix socket, so the socket crosses as loopback TCP. Two pieces carry it, both copying bytes unchanged: `scripts/chat-bridge.py` on the Mac, and a socat forwarder container `lez041n-maker-chat-forwarder` on the default bridge, in the `maker-socket` volume.
4. **Harness.** The bundle's own `scripts/node-e2e.py` scenarios ran, unchanged, on the `fast` profile with the agreement's Monero windows (cutoff +320 s, refund +720 s, punish +1000 s).
   - `node-e2e.py` knows "both Nodes in containers" or "both native". `scripts/mixed-e2e.py` loads it as a module and gives only the native role the harness's own native code paths: owner RPC over its socket, its log and hold files.
   - It runs the Maker's CLI from the package (`lez-maker-cli --socket …`).
   - It reaches the native role's wallet-rpc on 127.0.0.1:18083 (`scripts/xmr-lane-mixed.py`, wrapping `monero-lane.py`).
   - Run with `scripts/run-mixed.sh` (`timeout -k`, stdin closed).

## Swaps

LEZ tags are `<tag> <transaction id>…@<block>`: 13 is the Taker's LEZ lock, 14 the claim authorization, 15 the Maker's LEZ claim, 16 the Taker's refund and 17 the Maker's punish. The Monero transactions are on the stack's isolated Monero regtest. Times are CET, from the harness's clock; the Node logs are in UTC.

| Layout, outcome | Swap | Take | End (duration) | Result | Monero on regtest | LEZ tags |
|---|---|---|---|---|---|---|
| Maker native, completed | `3398e57228cd8c3d…` | 06:57:58 | 07:00:24 (183 s) | passed | funding `cef3397fc325b421…` (block 481); Taker sweep `5703f0e1015665a4…` (block 491) | 13 `7d394c217df7…` @22906; 14 `814278897dc7…` @22936; 15 `d007009b75e6…` @22945 |
| Maker native, Taker refunds | `d59a79575685574d…` | 07:00:55 | 07:14:22 (838 s) | passed | funding `3a07223bb1d9cd68…` (block 501); refund sweep `ca40edc566dbdd99…` (block 511) | 13 `fc055969b5c4…` @23048; 16 `fc30359f67ae…` @23611 |
| Maker native, punished | `630e2cd63858cb84…` | 07:14:49 | 07:31:57 (1055 s) | passed | funding `d4b1e8fb07ec2748…` (block 521) | 13 `224ee92dea94…` @23706; 17 `1084b1504601…` @24499 |
| Taker native, completed | `ad8ae32406bf6bb8…` | 07:33:54 | 07:35:46 (146 s) | passed | funding `ff18dba0015c95b3…` (block 531); Taker sweep `f93d113e7d491413…` (block 541) | 13 `6f8ef0d8936c…` @24614; 14 `020ff6f4fabc…` @24639; 15 `237e4faa7e60…` @24646 |

All four passed on their first try. LEZ settled to the unit each time:
- **Completed:** the lot of 50 000 units went to the Maker.
- **Refunded:** nothing moved.
- **Punished:** the lot went to the Maker, and the funded Monero stays at the shared address.

## Files

- `logs/B-*`: this run.
  - `B-start-lez041n.log`: the stack start.
  - `B-up-<role>.log` and `B-down-<role>.log`: bringing each layout up and down.
  - `B-<role>-<scenario>.log`: the harness's output.
  - `B-e2e-*.json`: the harness's summaries.
  - `B-<scenario>-maker-facts.json`: the native Maker's effect evidence, public fields only (`scripts/swap-facts.py`).
  - `B-maker.out.log` and `B-maker.node.log`, `B-taker.out.log` and `B-taker.node.log`: the native Nodes' own logs.
  - `B-provenance.txt`.
- `logs/A-*`: the rehearsal on the 0.4.0 bundle with locally built 0.4.1 darwin packages. The same four scenarios passed there (`RESULT.md`).
- `logs/monero-mac-verify.log`: the macOS Monero release check.
- `scripts/`: everything above.

Paths in the files are written as `$LEZ_NODE_HOME`, `$SESSION`, `$WORK`, `$HOME` and `$USER` (`scripts/sanitize.sh`). The set contains no login, key, cookie or wallet password; `sanitize.sh` scans for the stack's own secrets.
