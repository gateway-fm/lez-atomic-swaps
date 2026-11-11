# Logos-native acceptance, 2026-11-11 (c): the Node's resource use (node_stats)

One run of `deploy/scripts/logos-native-acceptance.sh` per role against the
release set of the combined landing (nodestats + code5), the live checks of
`lez_swap_node.node_stats` on the stack, and the supervisor harness. Why
`logosctl module stats` cannot show the Node, and what `node_stats` reports
instead: packaging note,
["The Node's resource use"](../../architecture/logos-native-packaging.md#the-nodes-resource-use-2026-11-11-c).

| File | What |
|---|---|
| `maker/`, `taker/` | the run per role: `transcript.md`, `summary.tsv`, `inputs.tsv`, `session-layout.txt`, `daemon.log`. **116 steps: 94 PASS, 13 FAIL, 8 INFO, 1 NOT RUN** each |
| `packages.tsv` | the release set (`lez_swap_node` 0.3.1; every dependency on it is `~0.3.1`) |
| `live-maker-idle.txt`, `live-taker-idle.txt` | `node_stats` asked through each role's own Basecamp on the stack (`deploy/ui-tests/node-stats.mjs`: the desk's QML bridge, Basecamp's LogosAPI, the real `logos_host`), compared with `ps`, `/proc/<pid>/fd` and `docker top` in the role container |
| `live-during-swaps.txt` | the same on lanes 1 and 2 while the ZEC refund tests ran |
| `lifecycle-stack-maker.txt`, `lifecycle-stack-taker.txt` | the three lifecycle checks on the live Maker and Taker, each followed by `node_stats` |
| `supervisor-harness.txt` | `lez_swap_node_impl.cpp` driven on the host with the Logos SDK stubbed: the three lifecycle cases, `node_stats` against `ps`, and the zombie wait fixed in `a3b33884` |

## Inputs

- Code: `a3b33884` (agent/nodestats on the integration head `37f706cd`, which
  carries code5's ZEC SDK refund guard). Built with `from-scratch.sh --only
  rust`, `--only build:swap-node-module`, `--only build:role-node-modules`,
  `--only nix`, `--only stage` (the sidecar's source stamp was unchanged), the
  compose images, `up.sh --fresh-lez` with `LEZ_LANES=3`.
- Tools and command: as in [../logos-native-2026-11-11-b](../logos-native-2026-11-11-b/README.md) ("Reproduce").

## Before and after

| | 2026-11-11-b (`023c929e`) | this run (`a3b33884`) |
|---|---|---|
| steps | 115 | 116 |
| PASS / FAIL / INFO / NOT RUN | 93 / 14 / 7 / 1 | 94 / 13 / 8 / 1 |
| `module stats` shows the Node | S95 FAIL | S95 INFO, with the upstream request |
| the Node's stats, judged against `/proc` | — | S96 PASS (`call lez_swap_node node_stats <role>`) |

Every other verdict is the same (S96–S115 of the earlier run are S97–S116
here). The 13 FAILs are the 9 unsigned packages (S08–S24, out of scope) and
four logosctl behaviours with issue texts in the earlier README: S63 (a file
install pulls in no dependency), S73 (a wrong `--root-hash` is accepted), S80
(`module load` of a `ui_qml` package), S101 (a crashed host reported
`not_loaded`).

A first run on `2de14642` (before `a3b33884`) had 14 FAILs: S104, "exactly one
Node after recovery", found no Node 2 s after the role was started again. The
stand-in launcher now leaves a helper in its group; after the host crash that
helper was orphaned to the container's pid 1 (`sleep infinity`, which never
reaps), and the next host's `end_group` waited the whole 10 s grace for the
zombie. `a3b33884` counts only members that still run (harness case 5:
10.3 s before, 0.34 s after).

S96, taker:

```
{"role":"taker","state":"running","pid":397,"uptime_seconds":2.7,"starts":1,"restarts":0,"last_exit":"",
 "processes":[{"pid":397,"ppid":294,"name":"sleep","state":"S","cpu_percent":0.0,"cpu_time_seconds":0.00,"memory_mb":1.879,"threads":1,"fds":3},
              {"pid":398,"ppid":397,"name":"sleep","state":"S","cpu_percent":0.0,"cpu_time_seconds":0.00,"memory_mb":1.789,"threads":1,"fds":3}],
 "group":{"processes":2,"cpu_percent":0.0,"cpu_time_seconds":0.00,"memory_mb":3.668,"threads":2,"fds":6},
 "host":{"pid":294,"ppid":22,"name":"logos_host","state":"R","cpu_percent":0.3,"cpu_time_seconds":0.02,"memory_mb":22.828,"threads":3,"fds":13}}
pid ppid pgrp VmRSS_kB cmdline, from /proc:
live 294 22 294 23376 /opt/logosctl/usr/bin/logos_host --name lez_swap_node ...
live 397 294 397 1924 sleep 300
live 398 397 397 1832 sleep 299
```

`module stats` in the same session lists `lez_swap_node` at the host's pid
only (S94, S95).

## On the stack

- Idle, both roles (`live-*-idle.txt`): the Node and its 12 group members
  (four loopback forwards and their per-connection forks, two LEZ sidecars,
  the two log `tee`s) with pid, ppid, threads, RSS and open fds equal to
  `ps` and `/proc` in the container; the host is `lez_swap_node`'s
  `logos_host` and the Node's parent. MATCH for both.
- During swaps (`live-during-swaps.txt`, lanes 1 and 2, ZEC refund tests):
  MATCH on all four; the Maker Node at 80% CPU and 62 MB RSS mid-swap. One
  earlier sample is a MISMATCH by a race: a per-connection socat fork listed by
  the call had ended before `ps` ran.
- Lifecycle, Maker and Taker (`lifecycle-stack-*.txt`): kill -9 the Node →
  healthy again in 6 s, `node_stats` shows the new pid, `starts` 2,
  `restarts` 1, `last_exit` "signal 9", no member of the old group left;
  SIGTERM to the `lez_swap_node` host → no Node; `docker restart` → one Node,
  started by the new host from its records, `node_stats` from that host. MATCH after
  each step.
- `LEZ_LANES=3 regress.sh smoke` on these images: 6 of 6 passed in 5 min (two
  of them desk tests, so both desks work with `lez_swap_node` 0.3.1).
- The combined landing's targeted run (code5's ZEC SDK refund guard), same
  images: `LEZ_LANES=3 regress.sh run` of node and desk `zec-maker-refund`,
  `zec-taker-refund` (both directions), node `zec-refund-then-maker-lock`
  (both), `zec-claim-collision:TakerSellsForeign`, node `zec-happy` (both),
  `node:btc:happy:TakerSellsLez`, `node:btc:taker-refund:TakerSellsForeign`:
  15 of 15 passed in 26 min, every one on its first try, 0 frozen tries.
