#!/usr/bin/env bash
# The three lifecycle checks of the role-package landing, on the live stack:
#   lifecycle-checks.sh [container (lez-maker-node)] [role (maker)]
#  1. kill -9 the Node: its group (forwards, sidecars) ends, one new Node starts, health returns
#  2. the lez_swap_node module host goes the way an unload/reload ends it (SIGTERM, as liblogos
#     stops a host): the Node and its helpers end with it (no orphan); records stay for the resume
#  3. Basecamp restarts (docker restart: Basecamp and every module host stop, Basecamp starts again):
#     exactly one Node, from the package, healthy; the lez_swap_node records were resumed
set -uo pipefail
c="${1:-lez-maker-node}"; role="${2:-maker}"
ts() { date -u +%H:%M:%SZ; }
procs() { docker exec "$c" sh -c 'ps -eo pid,ppid,pgid,etimes,args' | awk -v r="$role" '$5 ~ ("lez-" r "-node$") || $5 ~ /lez-v02-bridge-poc$/ || $5 == "socat" || $6 ~ /node-entrypoint.sh$/ || /--name lez_swap_node/ {print substr($0, 1, 170)}'; }
health() { docker exec "$c" lez-runtime-healthcheck uds "/run/lez/$role/node.sock" "${role}_health" '[{}]' >/dev/null 2>&1 && echo healthy || echo unhealthy; }
nodes() { docker exec "$c" pgrep -x "lez-$role-node" | wc -l; }
wait_healthy() { for _ in $(seq 1 "${1:-180}"); do [[ "$(health)" == healthy ]] && return 0; sleep 1; done; return 1; }

echo "== [$(ts)] before"; procs; echo "Nodes: $(nodes); $(health)"
docker exec "$c" sh -c 'ls -l /proc/$(pgrep -x lez-'"$role"'-node)/exe'

echo; echo "== [$(ts)] 1. kill -9 the $role Node"
node="$(docker exec "$c" pgrep -x "lez-$role-node")"
group_members="$(docker exec "$c" ps -eo pid=,pgid= | awk -v g="$node" '$2 == g {printf "%s ", $1}')"
echo "Node pid $node; its group: $group_members"
docker exec "$c" kill -9 "$node"
sleep 1.5
echo "[$(ts)] 1.5 s later, members of the old group still alive:"
docker exec "$c" ps -eo pid=,pgid=,args= | awk -v g="$node" '$2 == g' 
if wait_healthy 240; then echo "[$(ts)] healthy again"; else echo "[$(ts)] NOT healthy after 240 s"; fi
procs; echo "Nodes: $(nodes)"

echo; echo "== [$(ts)] 2. the lez_swap_node host is stopped as liblogos stops a module host (SIGTERM)"
host="$(docker exec "$c" pgrep -f -- '--name lez_swap_node ')"
echo "host pid(s): $host"
docker exec "$c" kill -TERM $host
sleep 3
echo "[$(ts)] 3 s later:"; procs; echo "Nodes: $(nodes); $(health)"
docker exec "$c" sh -c 'ls /var/lez/basecamp-*-user/module_data/lez_swap_node/*/'

echo; echo "== [$(ts)] 3. restart Basecamp (docker restart $c)"
docker restart "$c" >/dev/null
if wait_healthy 400; then echo "[$(ts)] healthy"; else echo "[$(ts)] NOT healthy after 400 s"; fi
sleep 5; procs; echo "Nodes: $(nodes)"
docker exec "$c" sh -c 'grep -h "lez_swap_node" /var/lez/basecamp-*-user/logs/*.log | grep -E "resum|supervising|Node started|ending|waiting" | tail -8' | cut -c1-200
