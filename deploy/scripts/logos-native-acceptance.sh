#!/usr/bin/env bash
# logos-native-acceptance.sh — run the Logos-native acceptance transcript
# (docs/architecture/logos-native-packaging.md) against built .lgx packages and
# keep what every command printed.
#
#   scripts/logos-native-acceptance.sh --role maker --role-lgx <role.lgx> \
#       --module <lez_core.lgx> --module <lez_swap_node.lgx> [--module <x.lgx>]... \
#       [--tools-nix <docker volume or directory>] [--out <dir>]
#
# The official Logos path is .lgx -> `logosctl package install` (dependencies,
# root hash) -> a session (modules/ data/ keyring/ logs/) -> `module load`,
# `call`, `stats`, `reload`, `package remove`. This script walks that path with
# the role package and the modules it is given, in a clean session, and judges
# each step against the target contract: a role package that installs alone,
# declares what it needs and starts its Node without anything from Compose.
# A step that fails is evidence; the script reports it and goes on.
#
# Tools. logosctl (LOGOSCTL_VERSION, default 0.3.2; LOGOSCTL_COMPAT, default
# 0.3.0, are also tried) and the v0.2-era logoscore 0.2.3 that our images use
# are downloaded from logos-co/logos-logoscore-cli releases, checked against
# the sha256 pinned here, and only ever run inside throwaway containers on an
# internal network (no catalog, no Logos Storage, no host ports). lm, lgx and
# lgpm have no released binaries: they come from the nix store the module
# builds leave behind (--tools-nix, default the docker volume
# lez-nix-store-arm, mounted read-only). Without them the lm/lgx steps are
# reported NOT RUN.
#
# Output (--out, default deploy/runtime/logos-native/<UTC stamp>): transcript.md (every
# command, its output, exit status and verdict), summary.tsv (one line per
# step), inputs.tsv (sha256, root hash, signature of each package),
# session-layout.txt and daemon.log. Session secrets (client tokens, the
# storage module's key) stay in the container and are thrown away with it.
# Exit status 0 when every judged step passed, 1 otherwise.
set -uo pipefail
caller_dir="$PWD"
self="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/$(basename "${BASH_SOURCE[0]}")"
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

usage() { sed -n '2,33p' "$self" | sed 's/^# \{0,1\}//'; exit 64; }

abs() { [[ "$1" == /* ]] && echo "$1" || echo "$caller_dir/$1"; }
role=maker role_lgx="" tools_nix=lez-nix-store-arm out=""
modules=()
while (($#)); do
  case "$1" in
    --role) role="${2:?}"; shift 2 ;;
    --role-lgx) role_lgx="$(abs "${2:?}")"; shift 2 ;;
    --module) modules+=("$(abs "${2:?}")"); shift 2 ;;
    --tools-nix) tools_nix="${2:?}"; [[ -e "$caller_dir/$tools_nix" && "$tools_nix" != /* ]] && tools_nix="$caller_dir/$tools_nix"; shift 2 ;;
    --out) out="${2:?}"; [[ "$out" == /* ]] || out="$caller_dir/$out"; shift 2 ;;
    -h|--help) usage ;;
    *) echo "unknown argument $1" >&2; usage ;;
  esac
done
[[ "$role" == maker || "$role" == taker ]] || { echo "--role must be maker or taker" >&2; exit 64; }
[[ -f "$role_lgx" ]] || { echo "--role-lgx <role package .lgx> is required" >&2; exit 64; }
for tool in docker jq sha256sum curl tar; do
  command -v "$tool" >/dev/null || { echo "$tool is required" >&2; exit 66; }
done

LOGOSCTL_VERSION="${LOGOSCTL_VERSION:-0.3.2}"
read -r -a compat <<<"${LOGOSCTL_COMPAT-0.3.0}"
LOGOSCORE_VERSION=0.2.3
# sha256 of the release tarballs (GitHub's asset digests; logoscore 0.2.3 as in
# images/lez-wallet-module/Dockerfile).
declare -A PIN=(
  [logosctl-0.3.2-x86_64]=294fbcc6409a8851b7471582bdad2dbfaf681b5d1c67dfffec38ada925dc6762
  [logosctl-0.3.2-aarch64]=e77ccd9c1a7e673c1a5533f2bcda03f84fd04e80f415ac414c378a897e5135fe
  [logosctl-0.3.0-x86_64]=04d078ebea2e2f247e0c9bb2a6f48526fea05aab2b0dc48db2ce3235fd5bc911
  [logosctl-0.3.0-aarch64]=a6eec63fe27d9b389a6dd94fa8774078fe71bc5ca56158949ea0f40c23eb4ade
  [logoscore-0.2.3-x86_64]=2e6eb1b7c971ceb35979d814ad1f16fc298f8f5d78a9e814546befbce871bbbb
  [logoscore-0.2.3-aarch64]=cecb818c338fe98b6de3265126fa617738a588bf89caa3691066fb1d6a50cdd1
)
BASE_IMAGE="debian:trixie-slim@sha256:f6b08b09a05725f9431502d3b5714f5c4576e82c1a84fa1debe0c24a7e0c3ec4"
case "$(uname -m)" in
  x86_64|amd64) arch=x86_64 ;;
  aarch64|arm64) arch=aarch64 ;;
  *) echo "unsupported host architecture $(uname -m)" >&2; exit 66 ;;
esac

stamp="$(date -u +%Y%m%dT%H%M%SZ)"
out="${out:-runtime/logos-native/$stamp}"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
cache="${LOGOS_NATIVE_CACHE:-$HOME/.cache/lez-logos-native}"
mkdir -p "$cache"
work="$(mktemp -d)"
prefix="logos-native-acc-$$"
net="$prefix-net"
ctrs=()
cleanup() {
  local c
  for c in "${ctrs[@]}"; do docker rm -f "$c" >/dev/null 2>&1; done
  docker network rm "$net" >/dev/null 2>&1
  chmod -R u+w "$work" 2>/dev/null; rm -rf "$work"
}
trap cleanup EXIT

log() { printf '[logos-native] %s\n' "$*" >&2; }

# ---- tools ---------------------------------------------------------------------
# fetch_cli <logosctl|logoscore> <version>: the release's AppImage, unpacked by
# the AppImage itself inside a container without network; prints the directory.
fetch_cli() {
  local tool="$1" version="$2" key dir tarball url
  key="$tool-$version-$arch"
  dir="$cache/$key"
  if [[ ! -x "$dir/squashfs-root/AppRun" ]]; then
    [[ -n "${PIN[$key]:-}" ]] || { log "no sha256 pinned for $key"; return 1; }
    rm -rf "$dir"; mkdir -p "$dir"
    tarball="$tool-$arch-linux.tar.gz"
    url="https://github.com/logos-co/logos-logoscore-cli/releases/download/$version/$tarball"
    curl -fsSL -o "$dir/$tarball" "$url" || { log "download failed: $url"; return 1; }
    echo "${PIN[$key]}  $dir/$tarball" | sha256sum -c - >/dev/null || { log "$tarball ($version) does not match its pinned sha256"; return 1; }
    tar -xzf "$dir/$tarball" -C "$dir"
    docker run --rm --network none --user "$(id -u):$(id -g)" -v "$dir:/w" -w /w "$BASE_IMAGE" \
      sh -c "./$tool-$arch.AppImage --appimage-extract >/dev/null" || { log "could not unpack $tool $version"; return 1; }
  fi
  echo "$dir/squashfs-root"
}

logosctl_dir="$(fetch_cli logosctl "$LOGOSCTL_VERSION")" || exit 1
logoscore_dir="$(fetch_cli logoscore "$LOGOSCORE_VERSION")" || logoscore_dir=""

# Inputs: copies, so the containers never see the host's provision tree.
mkdir -p "$work/in" "$work/bin" "$work/x"
declare -A file_of=()
names=()
add_input() { # add_input <path.lgx>: stage it under its manifest name
  local name
  name="$(tar -xzOf "$1" manifest.json | jq -r .name)" || { log "$1 is not an LGX package"; exit 65; }
  install -m 0644 "$1" "$work/in/$name.lgx"
  file_of[$name]="/in/$name.lgx"
  names+=("$name")
}
add_input "$role_lgx"
role_pkg="${names[0]}"
for m in "${modules[@]}"; do add_input "$m"; done

# Wrappers so the transcript reads `logosctl …`, `lm …` as the docs write them.
cat >"$work/bin/logosctl" <<'EOF'
#!/bin/sh
exec /opt/logosctl/AppRun "$@"
EOF
cat >"$work/bin/logoscore" <<'EOF'
#!/bin/sh
exec /opt/logoscore/AppRun "$@"
EOF
# procs: pid, parent pid and command line of every process (the image has no ps).
cat >"$work/bin/procs" <<'EOF'
#!/bin/sh
for p in /proc/[0-9]*; do
  c="$(tr '\0' ' ' <"$p/cmdline" 2>/dev/null)"; [ -n "$c" ] || continue
  echo "${p#/proc/} $(awk '/^PPid/{print $2}' "$p/status") $c"
done
EOF
chmod 0755 "$work/bin"/*

docker network create --internal "$net" >/dev/null || exit 1
start_ctr() { # start_ctr <name> <docker run args...>
  local name="$prefix-$1"; shift
  docker run -d --name "$name" --network "$net" --cpus 1 --memory 2g --user "$(id -u):$(id -g)" \
    -e HOME=/tmp -e QT_QPA_PLATFORM=offscreen -e LANG=C.UTF-8 -e PATH=/opt/bin:/usr/bin:/bin \
    -v "$work/bin:/opt/bin:ro" -v "$work/in:/in:ro" "$@" "$BASE_IMAGE" sleep infinity >/dev/null || return 1
  ctrs+=("$name")
}
start_ctr run -v "$logosctl_dir:/opt/logosctl:ro" || exit 1
RUN="$prefix-run"

# The tools container: lm, lgx, lgpm from the nix store, the era logoscore.
TOOLS=""
tools_args=(-v "$work/x:/x")
if [[ -d "$tools_nix" ]]; then tools_args+=(-v "$(cd "$tools_nix" && pwd)/nix:/nix:ro")
elif docker volume inspect "$tools_nix" >/dev/null 2>&1; then tools_args+=(-v "$tools_nix:/nix:ro")
else tools_nix=""; fi
[[ -n "$logoscore_dir" ]] && tools_args+=(-v "$logoscore_dir:/opt/logoscore:ro")
if start_ctr tools "${tools_args[@]}"; then
  TOOLS="$prefix-tools"
  for t in logos-module:lm lgx:lgx logos-package-manager-cli:lgpm; do
    path="$(docker exec "$TOOLS" sh -c "ls -d /nix/store/*-${t%%:*}-[0-9]*/bin/${t#*:} 2>/dev/null | head -1")"
    [[ -n "$path" ]] && docker exec "$TOOLS" sh -c "ln -sf $path /x/${t#*:}"
  done
fi
has_tool() { [[ -n "$TOOLS" ]] && docker exec "$TOOLS" test -e "/x/$1"; }

# ---- the transcript ------------------------------------------------------------
T="$out/transcript.md"
: >"$out/summary.tsv"
n=0 fails=0
{
  echo "# Logos-native acceptance — $role ($stamp)"
  echo
  echo "Verdicts: PASS / FAIL against the target contract; INFO records a fact; NOT RUN says why not."
  echo "Every command ran inside a throwaway container ($BASE_IMAGE) on an internal network."
} >"$T"

section() { printf '\n## %s\n' "$1" >>"$T"; }
record() { # record <status> <title> <container label> <command> <rc>
  local id
  n=$((n + 1)); id="$(printf 'S%02d' "$n")"
  [[ "$1" == FAIL ]] && fails=$((fails + 1))
  printf '%s\t%s\t%s\n' "$id" "$1" "$2" >>"$out/summary.tsv"
  {
    printf '\n### %s · %s · %s\n\n```\n' "$id" "$1" "$2"
    [[ -n "$4" ]] && printf '[%s] $ %s\n' "$3" "$4"
    # The storage module logs its peer chatter on stderr; it is noise here.
    grep -v '^\[[0-9-]* [0-9:.]*\] \[' "$work/out" | head -200
    [[ -n "$4" ]] && printf '[exit %s]\n' "$5"
    printf '```\n'
  } >>"$T"
  log "$id $1 $2"
}
# step <check> <title> <container> <command>: <check> is evaluated with $rc and
# $o (the output file); "info" records without judging.
step() {
  local check="$1" title="$2" ctr="$3" cmd="$4" label status
  label="${ctr#"$prefix"-}"
  docker exec "$ctr" bash -c "$cmd" >"$work/out" 2>&1
  rc=$?
  o="$work/out"
  if [[ "$check" == info ]]; then status=INFO
  elif eval "$check"; then status=PASS
  else status=FAIL; fi
  record "$status" "$title" "$label" "$cmd" "$rc"
}
notrun() { # notrun <title> <reason>
  echo "$2" >"$work/out"
  record "NOT RUN" "$1" "" "" ""
}
ok='(( rc == 0 ))'
callok='(( rc == 0 )) && grep -q "\"status\":\"ok\"" "$o"'
result() { jq -r '.result // empty' "$o" 2>/dev/null; }

S=/tmp/acceptance
L="logosctl --config-dir $S"

section "Tools"
step "$ok" "logosctl version" "$RUN" "logosctl --version"
if [[ -n "$TOOLS" ]]; then
  for t in lm lgx lgpm; do
    if has_tool "$t"; then step "$ok" "$t version (v0.2-era, from the module builder's nix store)" "$TOOLS" "/x/$t --version"
    else notrun "$t version" "no $t in the nix store ${tools_nix:-(none given)}"; fi
  done
  if [[ -n "$logoscore_dir" ]]; then step "$ok" "logoscore version (the v0.2-era runtime in our images)" "$TOOLS" "logoscore --version"; fi
fi
{
  printf '\nHost architecture: %s. Base image: %s.\n' "$(uname -m)" "$BASE_IMAGE"
  printf 'logosctl %s is published for linux x86_64/aarch64, macOS aarch64 and Windows x86_64; ' "$LOGOSCTL_VERSION"
  printf 'lm/lgx/lgpm have no released binaries.\n'
} >>"$T"

section "Inputs"
printf 'name\tfile sha256\tversion\tmanifestVersion\ttype\tvariants\tdependencies\troot hash\n' >"$out/inputs.tsv"
for name in "${names[@]}"; do
  f="$work/in/$name.lgx"
  tar -xzOf "$f" manifest.json | jq -r --arg sha "$(sha256sum <"$f" | cut -c1-64)" '
    [.name, $sha, .version, .manifestVersion, .type, (.main | keys | join(",")),
     ((.dependencies // []) | map(if type == "string" then . else .name end) | join(",")), .hashes.root] | @tsv' >>"$out/inputs.tsv"
done
{ printf '\n```\n'; column -t -s $'\t' "$out/inputs.tsv" 2>/dev/null || cat "$out/inputs.tsv"; printf '```\n'; } >>"$T"

section "Package truth (before installing)"
step "$ok" "start a clean session daemon" "$RUN" "$L daemon start --detach"
declare -A root_of=()
for name in "${names[@]}"; do
  step "$ok" "package show $name" "$RUN" "$L --json package show ${file_of[$name]}"
  root_of[$name]="$(jq -r '.rootHash // empty' "$o" 2>/dev/null)"
  sig="$(jq -r '.signatureStatus // "unknown"' "$o" 2>/dev/null)"
  echo "signatureStatus: $sig; rootHash: ${root_of[$name]:-?}" >"$work/out"
  if [[ "$sig" == signed ]]; then record PASS "$name is signed (signer DID in the output)" "" "" ""
  else record FAIL "$name is signed by a signer that can be pinned" "" "" ""; fi
done
for name in "${names[@]}"; do
  if has_tool lgx; then
    step "$ok" "lgx verify $name (structure, Merkle root)" "$TOOLS" "/x/lgx verify ${file_of[$name]}"
  fi
done
# lm reads one platform build: this host's variant, or its -dev tag. A
# portable plugin finds Qt (RemoteObjects) only beside a runtime, so lm loads
# it with the era runtime's libraries.
lm_env=""
[[ -n "$logoscore_dir" ]] && lm_env="LD_LIBRARY_PATH=/opt/logoscore/usr/lib "
for name in "${names[@]}"; do
  has_tool lm || break
  f="$work/in/$name.lgx"
  variant="$(tar -xzOf "$f" manifest.json | jq -r --arg a "linux-$([[ $arch == x86_64 ]] && echo amd64 || echo arm64)" \
    '.main | if has($a) then $a elif has($a + "-dev") then $a + "-dev" else empty end')"
  plugin="$(tar -xzOf "$f" manifest.json | jq -r --arg v "$variant" '.main[$v] // empty')"
  [[ -n "$plugin" ]] || { notrun "lm on $name" "no build for this host in the package"; continue; }
  docker exec "$TOOLS" sh -c "mkdir -p /x/$name && cd /x/$name && tar -xzf ${file_of[$name]}"
  for c in metadata methods events; do
    step "$ok" "lm $c $name" "$TOOLS" "$lm_env/x/lm $c /x/$name/variants/$variant/$plugin"
  done
done
notrun "lm verify <installed dir> --did <pinned signer>" \
  "lm 0.1.0 (the module builder's) has no verify; logos-module 0.3.0, which has it, publishes no binary (a nix build). The packages are unsigned, so there is no DID to pin either."

section "Install the role package alone"
step "$ok" "install $role_pkg as built" "$RUN" "$L package install ${file_of[$role_pkg]} --root-hash ${root_of[$role_pkg]:-none} -y"
if ! (( rc == 0 )) && grep -q 'does not contain variant' "$o" && has_tool lgx; then
  # The desks are built with the builder's dev variant tag (linux-amd64-dev);
  # logosctl takes only the portable name. Rebuild the package under the plain
  # name with lgx (stage-basecamp-package.sh does the same to Basecamp's tree).
  dev="$(tar -xzOf "$work/in/$role_pkg.lgx" manifest.json | jq -r '.main | keys[0]')"
  plain="${dev%-dev}"
  view="$(tar -xzOf "$work/in/$role_pkg.lgx" manifest.json | jq -r '.view')"
  main="$(tar -xzOf "$work/in/$role_pkg.lgx" manifest.json | jq -r --arg v "$dev" '.main[$v]')"
  step info "DERIVED: $role_pkg with variant $dev renamed $plain (lgx extract/add/remove)" "$TOOLS" \
    "cp ${file_of[$role_pkg]} /x/$role_pkg-$plain.lgx && chmod u+w /x/$role_pkg-$plain.lgx && /x/lgx extract /x/$role_pkg-$plain.lgx -v $dev -o /x/relabel && /x/lgx add /x/$role_pkg-$plain.lgx -v $plain -f /x/relabel/$dev --main $main --view $view -y && /x/lgx remove /x/$role_pkg-$plain.lgx -v $dev -y && /x/lgx manifest /x/$role_pkg-$plain.lgx"
  docker cp "$work/x/$role_pkg-$plain.lgx" "$RUN:/tmp/$role_pkg-$plain.lgx" >/dev/null 2>&1
  role_file="/tmp/$role_pkg-$plain.lgx"
  step "$ok" "package show (derived)" "$RUN" "$L --json package show $role_file"
  role_root="$(jq -r '.rootHash // empty' "$o" 2>/dev/null)"
  step "$ok" "install $role_pkg (derived) with its root hash" "$RUN" "$L package install $role_file --root-hash $role_root -y"
fi
step '(( rc == 0 )) && ! grep -q not_installed "$o"' \
  "installing the role package pulled in its dependencies" "$RUN" "$L package deps $role_pkg --recursive"
step 'grep -Eq "blockchain_module|lez_indexer_module|lez_network" "$o"' \
  "the role package declares its LEZ network requirement" "$RUN" "$L package deps $role_pkg --recursive"

section "Install the modules"
for name in "${names[@]:1}"; do
  step "$ok" "install $name with its root hash" "$RUN" "$L package install ${file_of[$name]} --root-hash ${root_of[$name]:-none} -y"
done
if [[ -n "${file_of[lez_swap_node]:-}" ]]; then
  step '(( rc != 0 ))' "a wrong --root-hash is refused" "$RUN" \
    "$L package install ${file_of[lez_swap_node]} --root-hash 0000000000000000000000000000000000000000000000000000000000000000 -y"
fi
step '(( rc == 0 )) && ! grep -q not_installed "$o"' "every dependency of $role_pkg is installed" "$RUN" "$L package deps $role_pkg --recursive"
step "$ok" "package ls" "$RUN" "$L package ls"

section "Lifecycle and behaviour"
step "$ok" "module load $role_pkg" "$RUN" "$L module load $role_pkg"
step "$ok" "module load lez_swap_node (with its declared dependencies)" "$RUN" "$L module load lez_swap_node"
step "$ok" "module show lez_swap_node" "$RUN" "$L module show lez_swap_node"
step "$callok" "call lez_swap_node node_status $role" "$RUN" "$L call lez_swap_node node_status $role"
step '[[ -s "$o" ]]' "the installed package graph carries the $role Node and its launcher" "$RUN" \
  "find $S/modules $S/plugins \\( -name node-entrypoint.sh -o -name 'lez-$role-node*' \\) -print"
step "$callok"' && [[ "$(result)" == 0 ]]' "start_node with the launcher Compose provides" "$RUN" \
  "$L call lez_swap_node start_node $role /usr/local/bin/node-entrypoint.sh $S/logs/$role-node.log $S/$role-node.hold"
step "$callok"' && grep -q running "$o"' "the $role Node is running" "$RUN" "$L call lez_swap_node node_status $role"

# The supervisor itself, with a stand-in launcher: a two-line shell script, not a Node.
step info "STUB: a stand-in launcher (sleeps; not a Node)" "$RUN" \
  "printf '#!/bin/sh\necho stub \$1 \$\$\nexec sleep 300\n' > /tmp/stub-launcher && chmod 0755 /tmp/stub-launcher && cat /tmp/stub-launcher"
step "$callok"' && [[ "$(result)" == 0 ]]' "STUB: start_node with the stand-in launcher" "$RUN" \
  "$L call lez_swap_node start_node $role /tmp/stub-launcher /tmp/stub.log /tmp/stub.hold"
sleep 2
step "$callok"' && grep -q running "$o"' "STUB: node_status reports the supervised child" "$RUN" "$L call lez_swap_node node_status $role"
child="$(result | jq -r '.pid // empty' 2>/dev/null)"
step "$ok" "module stats" "$RUN" "$L module stats"
step "grep -qw '${child:-none}' \"\$o\"" "module stats shows the $role Node (pid ${child:-?}) itself" "$RUN" "$L module stats"
step "$ok" "module reload lez_swap_node" "$RUN" "$L module reload lez_swap_node"
sleep 2
step "$callok"' && grep -q running "$o"' "the $role Node is supervised again after the reload" "$RUN" "$L call lez_swap_node node_status $role"
step info "processes after the reload" "$RUN" "procs | grep -E '^[0-9]+ [0-9]+ [^ ]*logos_host --name lez_swap_node|^[0-9]+ [0-9]+ sleep 300' || true"

# Crash the module host the way a fault would.
docker exec "$RUN" bash -c "$L call lez_swap_node start_node $role /tmp/stub-launcher /tmp/stub.log /tmp/stub.hold" >/dev/null 2>&1
sleep 2
step info "kill -9 the lez_swap_node host process" "$RUN" \
  "pid=\$(procs | awk '\$3 ~ /logos_host\$/ && \$5 == \"lez_swap_node\" {print \$1}'); echo host \$pid; kill -9 \$pid"
sleep 3
step 'grep -E "lez_swap_node" "$o" | grep -q crashed' "the runtime reports lez_swap_node as crashed" "$RUN" "$L module ls"
step '! grep -Eq "^[0-9]+ 1 sleep 300" "$o"' "no orphaned $role Node after the host crash" "$RUN" "procs | grep -E '^[0-9]+ [0-9]+ sleep 300' || true"
step info "load lez_swap_node again and start the role again (what a desk does)" "$RUN" \
  "$L module load lez_swap_node && $L call lez_swap_node start_node $role /tmp/stub-launcher /tmp/stub.log /tmp/stub.hold"
sleep 2
step '[[ "$(grep -c "sleep 300" "$o")" == 1 ]]' "exactly one $role Node after recovery" "$RUN" "procs | grep -E '^[0-9]+ [0-9]+ sleep 300' || true"
docker exec "$RUN" bash -c "$L call lez_swap_node stop_node $role; for p in \$(procs | awk '\$3 == \"sleep\" && \$4 == \"300\" {print \$1}'); do kill \$p; done" >/dev/null 2>&1

section "Remove"
step "$ok" "package remove $role_pkg" "$RUN" "$L package remove $role_pkg -y"
step "$ok" "package remove lez_core (takes its dependents)" "$RUN" "$L package remove lez_core -y"
step '! grep -Eq "lez_core|lez_swap_node|lez_atomic_swap" "$o"' "nothing of ours is left installed" "$RUN" "$L package ls"
step info "what remove leaves in the session" "$RUN" "find $S/modules $S/plugins $S/data -maxdepth 2 | sort"
docker exec "$RUN" bash -c "find $S -maxdepth 3 | sort; echo; ls -la $S/keyring" >"$out/session-layout.txt" 2>&1
docker exec "$RUN" bash -c "cat $S/logs/daemon.log" >"$out/daemon.log" 2>&1
step "$ok" "daemon stop" "$RUN" "$L daemon stop"

section "Which tools take our packages"
for v in "${compat[@]}"; do
  dir="$(fetch_cli logosctl "$v")" || { notrun "logosctl $v" "could not fetch it"; continue; }
  start_ctr "compat-$v" -v "$dir:/opt/logosctl:ro" || continue
  C="$prefix-compat-$v"
  cmd="$L daemon start --detach >/dev/null && $L --version | head -1"
  for name in lez_core lez_swap_node; do [[ -n "${file_of[$name]:-}" ]] && cmd+=" && $L package install ${file_of[$name]} -y"; done
  cmd+=" && $L module load lez_swap_node && $L call lez_swap_node node_status $role"
  step "$callok" "logosctl $v: install, load lez_swap_node, call node_status" "$C" "$cmd"
  step "$ok" "logosctl $v: install $role_pkg as built" "$C" "$L package install ${file_of[$role_pkg]} -y"
done
if [[ -n "$TOOLS" && -n "$logoscore_dir" ]]; then
  cmd="mkdir -p /x/era/modules"
  for name in lez_core lez_swap_node; do
    [[ -n "${file_of[$name]:-}" ]] || continue
    cmd+=" && mkdir -p /x/era/$name.x /x/era/modules/$name && tar -xzf ${file_of[$name]} -C /x/era/$name.x && cp /x/era/$name.x/manifest.json /x/era/modules/$name/ && cp -r /x/era/$name.x/variants/linux-*/. /x/era/modules/$name/ && ls /x/era/$name.x/variants > /x/era/modules/$name/variant"
  done
  step "$ok" "logoscore $LOGOSCORE_VERSION: modules laid out as the images do (no installer in this release)" "$TOOLS" "$cmd && ls /x/era/modules"
  docker exec -d "$TOOLS" sh -c "logoscore --config-dir /x/era/cfg -m /x/era/modules --persistence-path /x/era/data -D >/x/era/daemon.log 2>&1"
  sleep 4
  step "$callok" "logoscore $LOGOSCORE_VERSION: load-module lez_swap_node, call node_status" "$TOOLS" \
    "logoscore --config-dir /x/era/cfg load-module lez_swap_node && logoscore --config-dir /x/era/cfg call lez_swap_node node_status $role"
fi
if has_tool lgpm; then
  for name in lez_swap_node "$role_pkg"; do
    [[ -n "${file_of[$name]:-}" ]] || continue
    step "$ok" "lgpm (v0.2-era, the desks' installer): install $name" "$TOOLS" \
      "mkdir -p /x/lgpm-root/modules /x/lgpm-root/plugins && /x/lgpm --modules-dir /x/lgpm-root/modules --ui-plugins-dir /x/lgpm-root/plugins install --file ${file_of[$name]}"
  done
fi

{
  printf '\n## Summary\n\n```\n'
  cat "$out/summary.tsv"
  printf '```\n\n%d steps, %d judged FAIL.\n' "$n" "$fails"
} >>"$T"
log "transcript: $T ($fails FAIL of $n steps)"
((fails == 0))
