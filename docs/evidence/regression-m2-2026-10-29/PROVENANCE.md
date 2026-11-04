# Provenance — M2 regression on the Docker devnet, 2026-10-29

Everything in this folder was produced on one box by the Docker devnet of
`deploy/` (local profile), read from the two Nodes and the chains by
`deploy/scripts/node-e2e.py`. Nothing here is private: the exports carry
swap views, durable actor status, zebra facts and transaction ids; the Node
logs carry the actors' effect lines (`deploy/scripts/node-e2e.py` captures
`docker logs` since each scenario's start, filtered to `taker actor` /
`maker actor` lines).

## Source

| commit | what |
|---|---|
| `c3091862` | D1: a superseded LEZ nonce is re-planned in the same drive attempt, with its lineage (protocol `Superseded`, sidecar supersession slots, SDK/store restage, adapter per-index contexts, actor re-plan loop, Node class `lez_first_lock_replan_exhausted`) |
| `e9ad978a` | sidecar: a journaled submission success is decided on the chain again (lineage transactions only) |
| `0983e4ed` | swap-store: the request-context journal admits the supersede operations, rebuilds older journals |
| `b1714423` | adapter: a re-plan retires the superseded steps' submit contexts |
| `4f077eb4` | devnet profile `zcash_refund_blocks` 4 → 8; `zec-export` never provisions; the Taker's Maker-lock observation names its cause |
| `9afd25a8` | taker-node: an admitted Zcash action's refused first attempt is the supervisor's to retry; harness measures the Maker's funding attempts from its effect lines; provisioner withdraw ids; reset clears the Maker's ZEC recovery rows |
| `9baa9cd9`, `0abae8b1` | fixture: one claim secret per provisioned swap (from the OS RNG, owner-private) |
| `c0510362` | taker-node: a failed take / receipt binding names its cause |
| `8f01a687`, `de8f42a4` | maker-node: a Zcash completion takes the claim preimage from the agreement's own template; no Node-wide secret (`--maker-claim-preimage-file` deprecated, refused for a catalog of more than one template) |
| `7264bf6c` | zec-sdk: `deterministic-local-devnet-v2` carries the eight-block Zcash refund window; `-v1` keeps its four (profiles are immutable per id); the devnet provisioner and the harness ask for v2 |
| `6648c363`, `2bde2fd2`, `b978e406` | swap-store: every bridge operation name inserts under the journal's CHECK; taker-node: the first-attempt reply rule is a tested helper; sidecar: a submission decided again keeps its earlier decisions in the journal |
| `bdb34862` | deploy README: the finality heartbeat is the liveness detector, not a keep-alive |

HEAD at the final build: `b978e406` (branch `feat/m2-zec-on-docker-signed`, PR #119).

## Images

Built by `deploy/scripts/from-scratch.sh --only build:sidecar`, `--only rust`,
`--only stage`, then `docker compose build tools maker-node taker-node`
(22:46Z). Both Node images check the staged sidecar's tree stamp against the
one their binaries expect.

| image | id | digest |
|---|---|---|
| `lez-taker-node:local` | `5992f60d52bc` | `sha256:5992f60d52bc35f2e9039cf225a93f3c116793ddc89af09a5e316a731ed4dd87` |
| `lez-maker-node:local` | `0b6adca1d97a` | `sha256:0b6adca1d97a633c37870a6d60f8e476099e9edfa6859fa8da145bf91ee0889d` |
| `lez-tools:local` | `bb62c44bcadd` | `sha256:bb62c44bcaddb0625ed8cb5d5b5c1f6954ef055166af70372824312735f24150` |

Sidecar tree stamp: `93e4194519db1a1911db140f1d2c4e6004dda869`.

The ZEC Node matrix (RESULTS.txt, second table) ran on the images of
`8f01a687` under the mutated v1 profile (anchor + 9, the same window v2 now
carries under its own id); the commits after it change the fixture's secret
source, remove the Maker's Node-wide secret, introduce the v2 id and keep the
sidecar's journal history, none of which a single-template scenario
exercises. The acceptance pair was rerun on the final images and the v2
profile (first table); those exports are the deliverable evidence.

## Chains

The devnet LEZ chain (pinned Bedrock, local profile) was recreated by
`up.sh --fresh-lez` whenever finality froze; a heartbeat service (one
authenticated transfer every 20 s, `runtime/heartbeat/status.json`) measured
each chain. Today's chains and the scenarios they carried:

| chain | up (Z) | finality froze | carried |
|---|---|---|---|
| 37 | 18:37 | ~19:33 (56 min) | zec-happy PASSED 19:06 (154 s) |
| 38 | 19:38 | ~20:24 (46 min) | zec-concurrent PASSED 20:15 (`8f01a687`), zec-happy PASSED 19:56 (228 s), zec-replay, zec-wrong-inputs |
| 39 | 20:29 | ~21:05 (36 min) | zec-restart-taker, zec-restart-maker, zec-survivor, zec-regenerated-catalog, zec-tampered-catalog, zec-concurrent-staggered |
| 40 | 21:39 | alive at 21:56, left | zec-concurrent and zec-happy on de8f42a4 under the mutated v1 profile (mechanism evidence) |
| 41 | 22:03 | alive at 22:41 (38 min), left | reverse desk taker-refund: its steps passed, the Maker's reconciliation never came (FINDINGS 19) |
| 42 | 23:09 (second genesis: the first froze during the bootstrap's vault sweeps) | alive at 23:26 (finalized 115, flat 0 s) | **zec-concurrent and zec-happy on the final images and profile v2; the exports in `zec/`** |

The freezes are the known Bedrock behaviour (#3643): blocks keep being
published and transactions keep being included while the finalized height
stops; the heartbeat showed it does not depend on quiescence. The local
profile gates Zcash swaps on inclusion depth, so a frozen chain lets a swap in
flight finish but not a new chain-dependent step.

## Corridors per scenario

| scenario | corridor run | chain |
|---|---|---|
| zec-concurrent (final, v2) | `zec-devnet-1793315427` (entries 0 and 1, zebra anchor 322, Zcash refund height 331) | 42 |
| zec-happy (final, v2) | `zec-devnet-1793316189` | 42 |
| zec-concurrent (`8f01a687`) | `zec-devnet-1793304752` | 38 |
| zec-replay | `zec-devnet-1793304997` | 38 |
| zec-wrong-inputs | `zec-devnet-1793305200` | 38 |
| zec-restart-taker | `zec-devnet-1793305796` | 39 |
| zec-restart-maker | `zec-devnet-1793305988` | 39 |
| zec-survivor | `zec-devnet-1793306610` | 39 |
| zec-regenerated-catalog | `zec-devnet-1793306837` | 39 |
| zec-tampered-catalog | `zec-devnet-1793307021` | 39 |
| zec-concurrent-staggered | `zec-devnet-1793307131` and `zec-devnet-1793307258` (the `-b` corridor beside it) | 39 |

## Profile in force

`deterministic-local-devnet-v2`: LEZ confirmations 1, Zcash confirmations 1,
LEZ refund delay 1 800 s, Zcash refund window 8 blocks, required margin 30 s.
It is `deterministic-local-devnet-v1` (four blocks, unchanged) under a new id:
profiles are immutable per id, and two swaps taken together at one Zcash
anchor need the first swap's four blocks plus the second swap's. The earlier
runs of 2026-10-29 that say `-v1` with anchor + 9 ran a build in which v1's
window had been widened in place (4f077eb4, reverted by 7264bf6c); they are
mechanism evidence, not shipped. Allocations 2 000 000 LEZ per
role. Zebra regtest is mined by the harness one block per reason.
