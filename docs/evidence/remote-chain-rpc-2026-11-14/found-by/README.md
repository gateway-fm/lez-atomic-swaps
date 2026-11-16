# Native Linux x86_64 Nodes and desks, chains over an SSH tunnel, 2026-11-14

Both role Nodes and both desks ran natively on a Linux x86_64 host (`<x86_64-host>`:
Ubuntu 24.04, kernel 6.8, 8 cores, no Docker, no root) in one official Basecamp 0.3.2
x86_64. The published, signed 0.3.1 packages came from the real catalog. The chains
(LEZ sequencer and indexer, Bitcoin Core with its miner, Zebra, the finality heartbeat)
were the devnet's lane-1 containers on an Apple-silicon Mac. The Mac served as a
third-party RPC provider: each chain RPC port reached the Linux host's loopback through
one `ssh -R` tunnel. All times are UTC.

## Inputs

- Hosts: Basecamp `LogosBasecamp-Desktop-v0.3.2-f2fae6-x86_64.AppImage` (sha256
  `983de0c7…c3ade012`) and logosctl 0.3.2 x86_64 (sha256 `294fbcc6…dc6762`). Both match
  the official release digests and were extracted with `--appimage-extract`. The display
  is Xvfb `:77` with a software Qt backend; Phase 1 (`../README.txt`) has the details.
- Catalog: `gateway-fm/logos-modules-release-base` `logos-repo.json` beside the official
  one. The index is `generatedAt` 2026-11-14T10:18:11Z: the signed 0.3.1 set, with
  `trustedSigners` pinning the release DID by now.
- Install: one clean dir (logosctl `--config-dir` = Basecamp `--user-dir`). The steps
  were `package install` Maker, then Taker, Maker and Taker again (`scripts/install-both.sh`).
  `package deps` of both desks is clean: `delivery_module` 0.2.1, `chat_module` 0.2.2,
  `lez_core` 0.4.1-sign.4, `lez_swap_node`/`lez_maker_node`/`lez_taker_node` 0.3.1, all
  `linux-amd64`. The role packages' `payload.json` `build_source` is `976f5f72`.
- Harness: `deploy/` of `976f5f72` (`git archive`), `node-e2e.py` in native mode
  (`LEZ_E2E_NATIVE_RUN_ROOT`). It had one Linux fix (`remote-logs/node-e2e-linux-native-bin.patch`,
  below).
- ZEC provisioning tools: `zec-local-poc-{provision,chat-draft,transfer}` cross-built from
  `976f5f72` for `x86_64-unknown-linux-gnu`, in a throwaway `rust:1.96.0-bookworm`
  container on the Mac. The Linux host has no C compiler and the role packages do not ship
  these tools. `lez-maker-cli` is the one from the installed Maker package.
- Identities `maker-munich-01` and `taker-zurich-01`, the market bootstrap manifest and the
  Bitcoin RPC cookie came from the stack's `runtime/`. They were copied 0600 into the
  Linux host's `LEZ_NODE_HOME` and removed at the end. The stack's own Node containers
  stayed stopped.
- One Basecamp with both desks. The Maker desk was opened first and the Taker desk second,
  as in the macOS run. The environment is `scripts/env-linux.sh`.

## Endpoints

| what | Mac (container port) | Linux host | how | result |
|---|---|---|---|---|
| LEZ sequencer RPC | 127.0.0.1:3040 | 127.0.0.1:3040 | `ssh -R` | clean |
| LEZ indexer RPC (`lez-network`) | 127.0.0.1:8779 | 127.0.0.1:8779 | `ssh -R` | clean |
| Bitcoin Core RPC | 127.0.0.1:18443 | 127.0.0.1:18443 | `ssh -R` | clean (cookie auth) |
| Zebra RPC | 127.0.0.1:18232 | 127.0.0.1:18232 | `ssh -R` | clean |
| Bitcoin miner, finality heartbeat | inside the stack | — | not needed by the Nodes | the heartbeat's `status.json` is mirrored every 10 s for the harness preflight (`scripts/heartbeat-mirror.sh`) |
| Bitcoin ZMQ, explorers, price source | not used | — | — | the Nodes reach none |
| Delivery directory, Maker Chat socket, signer sockets, owner sockets | — | `$LEZ_NODE_HOME/{delivery,run,wallet}` | local files and unix sockets | both desks are on this host |
| harness `docker exec … bitcoin-cli` / `docker inspect` | — | `scripts/docker-shim.py` | JSON-RPC through the tunnel | `remote-logs/*/docker-shim.log`: every call |

Each RPC round trip through the tunnel took 100–190 ms, against 1–5 ms on the Mac.
Because the chain hosts are `127.0.0.1`, the launcher starts no `socat` forward.

## Results

| scenario | direction | result | swap | transactions | time |
|---|---|---|---|---|---|
| BTC `happy` | TakerSellsForeign | passed (devnet `refund` timing) | `425c4de2c2a5…` | BTC claim `9eb54c7b4193…` | 214 s |
| BTC `happy` | TakerSellsLez | failed (devnet `refund` timing): the claim window had passed (below) | `2fb30c321e4a…` | the Maker later refunded its leg | 103 s |
| BTC `happy` | TakerSellsForeign | **passed** (`fast` timing, clean Node state) | `ee05175755c1550db3ba6c3a4c272bcad13456fbe86fe9846252f2e67c3400f0` | Taker BTC lock `f6fb97a76c09…`, BTC claim `51246734ce3571f977ce7ddacd86b48750fd1f0923121204eb0bf12f0fe1e82c` (Taproot key-path) | 170 s |
| BTC `happy` | TakerSellsLez | **passed** (`fast` timing, clean Node state) | `142e42bb1f1c45ecb7d9c7a218fbf69975d8c044695faf081465305c3264480f` | Taker LEZ lock `4a9ce3c8a37b…`, BTC claim `154739ad5a18…` (Taproot key-path) | 107 s |
| ZEC `zec-happy` | TakerSellsLez | **failed** (LEZ 1 s blocks): Taker LEZ first lock never observed (`UnstableTip`) | `zec-devnet-swap-1794656552` | corridor provisioned, offer taken | — |
| ZEC `zec-happy` | TakerSellsForeign | **failed** (LEZ 1 s blocks): both legs locked, Taker LEZ claim never observed (`UnstableTip`) | `zec-devnet-swap-1794656831` | Taker ZEC funding `9997bb86b770…` in Zebra block 707; Maker `lez_initialize` + `lez_fund` submitted | — |
| BTC `taker-refund` | TakerSellsForeign | Taker side **passed** (BTC refunded at 12:20:23); Maker reconcile **failed** (`actor_timeout` ×17) | `abf2c6dbdc75…` | Taker BTC lock `57251c06bc6c…` refunded | 552 s to the refund |
| ZEC `zec-happy` | TakerSellsLez | **passed** (LEZ **5 s** blocks, clean Node state) | `zec-devnet-swap-1794659925` | Maker ZEC funding `404601c0e397…` (Zebra block 710), Taker ZEC claim `7e2b10150073…` (block 713) | 147 s |
| ZEC `zec-happy` | TakerSellsForeign | **passed** (LEZ 5 s blocks, then **10 s** from 12:48:34) | `zec-devnet-swap-1794660073` | Taker ZEC funding `2d07319f1613…` (block 714), Maker ZEC claim `821297c271fd…` (block 716) | 470 s |

The two ZEC passes ran with the devnet sequencer's `block_create_timeout` raised from 1 s
to 5 s (`runtime/config/sequencer_config.json`, only `lez-sequencer` restarted). In the
second one the Taker's LEZ claim was still refused 12 times at 5 s, so the interval went
to 10 s while that swap waited, and the claim went through ~20 s later. The original 1 s
was restored afterwards (file byte-identical to its backup) before the chains were
stopped. Swap-by-swap log, including every failed attempt: `results.tsv`.

Both desks showed `Node connected` and DONE 2 with both BTC swaps completed
(`shots/p2-D1b-maker-done-tab.png`, `shots/p2-D2b-taker-done-tab.png`). After the ZEC
runs the Taker desk lists `zec-devnet-swap-1794660073` Completed and
`zec-devnet-swap-1794659925` as "Needs attention" (`shots/p2-D4-taker-after-zec.png`),
although the harness and both Nodes reported it completed. The later provisioning
archived its corridor. The macOS evidence README notes the same desk behaviour. The Nodes,
actors and sidecars are x86-64 ELF processes under Basecamp's `lez_swap_node` on the
Linux host (`remote-logs/evidence/processes.txt`). The chain ports there are held by
`sshd`, the tunnel's end (`remote-logs/evidence/tunnel-sockets.txt`).

## What did not work, and why

1. **Group-writable directories (Ubuntu's umask 002).** `logosctl package install`
   creates `modules/<pkg>/bin` as 0775 under a 002 umask. Two components refuse that:
   - The Maker's actor supervisor refuses an actor whose parent directory is group- or
     other-writable (`swap-store/src/maker_actor_process.rs` `validate_program_parent`).
     Every BTC swap fails on the Maker with `actor_deployment_invalid`.
   - The LEZ sidecar refuses a state directory with any group-writable ancestor
     (`durable_reservation.rs` `validate_trusted_parent_chain`): "sidecar state directory
     or lease file is unsafe".

   Fixed here with `chmod -R go-w` on the user dir and `chmod 755` on the working dir. A
   macOS user (umask 022) never meets this.
2. **The devnet's `refund` timing profile.** It sets a Maker second-lock cutoff of 100 s
   and an earlier refund at 115 s. Over a 100–190 ms RPC path the Maker's funding took
   87 s (21 s on the Mac), so the TakerSellsLez claim came after its window. With the
   stack's `fast` profile (cutoff 240 s, refunds at 420/840 s, the same 2 s blocks) both
   directions passed.
3. **ZEC with remote chains on a 1 s LEZ chain.** The ZEC SDK's canonical LEZ
   observation requires the LEZ tip to be identical before and after the observation
   (`zec-swap-sdk/src/lez_observation.rs`: `before_hash != after_hash` →
   `UnstableTip`). The devnet makes a LEZ block every second. An observation made of
   several sequential RPCs over a 100–190 ms link nearly always spans a block, so every
   attempt is refused: the first lock in one direction, the claim in the other. BTC has
   no such check and is not affected. This is not a packaging or Linux defect. Slowing
   the devnet's LEZ blocks made both directions pass: 5 s for TakerSellsLez, and 10 s for
   the TakerSellsForeign claim, which was still refused 12 times at 5 s.
4. Harness, Linux only: `node-e2e.py` `native_bin()` takes the program path from
   `ps -o comm=`, which is the full path on macOS but the bare, truncated name on Linux.
   It then fails with `./lez-zec-taker-actor` not found. The patch reads
   `/proc/<pid>/exe`.
5. Harness: `run-suite.sh`'s `echo "[$(date …)] $label exit $?"` logs `exit 0` for a
   failed scenario, because the `$(date)` substitution resets `$?`. The macOS
   `results.txt` files have the same bug; check the scenario logs for `passed`.
   `scripts/run-suite-linux.sh` saves `rc=$?` first.
6. Signing: with the release DID pinned in the catalog's `trustedSigners` (shown by
   `catalog list` as `trustedSignerDids`), logosctl 0.3.2 still logs "Package is signed by
   a key no trusted anchor validates" for all seven LEZ packages. Only `logosctl key add`
   silences it (Phase 1, step K).

7. **Per-block sequential LEZ reads vs the Maker actor's 30 s attempt deadline.** A
   logging proxy on the chain ports (`scripts/rpc-count-proxy.py`,
   `remote-logs/rpc-calls.log.gz`) showed each Maker actor attempt walking the LEZ
   discovery window one block at a time on the indexer: `getBlockById` +
   `getBlockByHash` per block, ~560 calls at 30–120 ms. That takes ≥ 30 s, so the attempt
   is killed (`actor_timeout`). This is why a Maker under recovery never finishes over
   the tunnel (6d3e75450ea6, abf2c6dbdc75), and it slowed 2fb30c321e4a.
   Details, file:line and fix options: `fixes/latency-analysis.md`.

## Changes made only on the test host (for repo fixes)

- `fixes/directory-permissions.md`: which paths and checks refused, the exact local
  `chmod`s, and fix options.
- `fixes/node-e2e-native-bin-linux.patch`: the harness `native_bin()` fix.
- `fixes/harness-docker-shim.patch`: the RPC-only `docker` stand-in (bitcoin-cli and
  `inspect … State.Running`) the harness needs when the chains are remote.
- `fixes/run-suite-exit-code.diff`: the runner's exit-code capture.
- `fixes/latency-analysis.md`: the ZEC `UnstableTip` check (tolerance per block
  interval, RPCs per observation, file:line) and the BTC per-block walk / `actor_timeout`.

## Files

- `scripts/`: `install-both.sh`, `env-linux.sh`, `bc-both.sh`, `run-suite-linux.sh`,
  `docker-shim.py`, `heartbeat-mirror.sh` and `patch-native-bin.py`. The Phase 1 scripts
  (`../scripts/`: `env.sh`, `verify.sh`, `bc-start.sh`, …) are used too.
- `remote-logs/install-both.log`: the install, plans and `package deps`.
- `results.tsv`: one line per swap attempt and per diagnosis, in time order.
- `remote-logs/e2e-btc/`: the passing BTC pair. `remote-logs/e2e-zec-5s/`: the passing
  ZEC pair (5 s, then 10 s LEZ blocks). `remote-logs/e2e-refund/`: `taker-refund`. The other `e2e-*` folders are the earlier
  attempts, named after what stopped them. `e2e-summaries/` holds the harness's JSON.
- `remote-logs/node-logs/`: the Nodes' own logs (`attempt1/` = before the state reset).
- `remote-logs/S*-basecamp.log.gz`: Basecamp sessions S1 (refund timing), S2 (fast), S3
  (fast, clean state) and S4 (fast, clean state, slow LEZ blocks).
- `remote-logs/node-logs/`, `remote-logs/node-logs-final/`: the Nodes' own logs per state
  generation (attempt1: before the first reset; state-attempt2: BTC pair and refund;
  final: the ZEC pair).
- `shots/`: desks connected (`p2-S1-*`, `p2-S2-*`, `p2-S3-*`) and completed (`p2-D*`).

Paths are written as `/home/<user>` and `<home>`. Keys, the RPC cookie and the RPC
password appear in no file here (scanned).
