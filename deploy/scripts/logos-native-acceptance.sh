#!/usr/bin/env bash
# logos-native-acceptance.sh — run the Logos-native acceptance transcript
# (docs/architecture/logos-native-packaging.md) against built .lgx packages and
# keep what every command printed.
#
#   scripts/logos-native-acceptance.sh --role maker --role-lgx <role.lgx> \
#       --module <lez_core.lgx> --module <lez_swap_node.lgx> --module <lez_maker_node.lgx> \
#       [--module <x.lgx>]... [--network-lgx <lez_network.lgx>] \
#       [--tools-nix <docker volume or directory>] [--out <dir>]
#
# The packages are the release set from-scratch.sh --only stage writes
# (deploy/images/basecamp-ui/assets/packages/: lgx-portable builds, our
# packages' dependencies pinned to the set's versions). lez_network is the LEZ
# network's operator package (the LEZ Network desk, which declares
# blockchain_module and lez_indexer_module); a role package declares no network
# module (it reaches the network its configuration names), so a trader using
# the public testnet does not install it.
#
# lez_<role>_node is the role's Node payload package (modules/lez-<role>-node,
# scripts/package-role-node.sh): the Node, its actors, the LEZ sidecar and the
# launcher, which lez_swap_node starts. The Node itself needs the role's
# configuration (chains, identity: Compose provides it); in this bare session
# the launcher starts and stops for want of it, which is what is judged.
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

usage() { sed -n '2,43p' "$self" | sed 's/^# \{0,1\}//'; exit 64; }

abs() { [[ "$1" == /* ]] && echo "$1" || echo "$caller_dir/$1"; }
role=maker role_lgx="" network_lgx="" tools_nix=lez-nix-store-arm out=""
modules=()
while (($#)); do
  case "$1" in
    --role) role="${2:?}"; shift 2 ;;
    --role-lgx) role_lgx="$(abs "${2:?}")"; shift 2 ;;
    --module) modules+=("$(abs "${2:?}")"); shift 2 ;;
    --network-lgx) network_lgx="$(abs "${2:?}")"; shift 2 ;;
    --tools-nix) tools_nix="${2:?}"; [[ -e "$caller_dir/$tools_nix" && "$tools_nix" != /* ]] && tools_nix="$caller_dir/$tools_nix"; shift 2 ;;
    --out) out="${2:?}"; [[ "$out" == /* ]] || out="$caller_dir/$out"; shift 2 ;;
    -h|--help) usage ;;
    *) echo "unknown argument $1" >&2; usage ;;
  esac
done
[[ "$role" == maker || "$role" == taker ]] || { echo "--role must be maker or taker" >&2; exit 64; }
[[ -f "$role_lgx" ]] || { echo "--role-lgx <role package .lgx> is required" >&2; exit 64; }
for tool in docker jq sha256sum curl tar python3; do
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
network_pkg=""
[[ -n "$network_lgx" ]] && { add_input "$network_lgx"; network_pkg="${names[-1]}"; }

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
     ((.dependencies // []) | if any(type == "object") then map(select(type == "object") | "\(.name) \(.version // "*")")
       else . end | join(",")), .hashes.root] | @tsv' >>"$out/inputs.tsv"
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
step '(( rc == 0 )) && ! grep -q not_installed "$o"' \
  "installing the role package pulled in its dependencies" "$RUN" "$L package deps $role_pkg --recursive"
# Each dependency carries a version range (pin-package-dependencies.sh), so the
# default catalog's other packages of the same names (lez_core 0.5.1,
# chat_module 0.3.0, ...) do not satisfy it. The resolved view (--recursive)
# takes the first entry of a name, the range; without --recursive logosctl
# lists the manifest's entries as written, the bare names the v0.2 runtime
# reads included.
step '(( rc == 0 )) && jq -e "length > 0 and all(.[]; (.requiredVersion // \"\") != \"\")" "$o" >/dev/null' \
  "every dependency $role_pkg declares resolves with a version range" "$RUN" "$L --json package deps $role_pkg --recursive"
# B3: the LEZ network is reached through the endpoints the role is configured
# with (the public testnet's or an operator's); its modules are the operator
# package lez_network's dependencies, not the role's.
step '(( rc == 0 )) && ! grep -Eq "blockchain_module|lez_indexer_module|lez_network" "$o"' \
  "the role package requires no LEZ network module (B3: lez_network is the operator package)" "$RUN" "$L --json package deps $role_pkg --recursive"

section "Install the modules"
for name in "${names[@]:1}"; do
  [[ "$name" == "$network_pkg" ]] && continue
  step "$ok" "install $name with its root hash" "$RUN" "$L package install ${file_of[$name]} --root-hash ${root_of[$name]:-none} -y"
done
if [[ -n "${file_of[lez_swap_node]:-}" ]]; then
  step '(( rc != 0 ))' "a wrong --root-hash is refused" "$RUN" \
    "$L package install ${file_of[lez_swap_node]} --root-hash 0000000000000000000000000000000000000000000000000000000000000000 -y"
fi
step '(( rc == 0 )) && ! grep -Eq "not_installed|version_mismatch" "$o"' "every dependency of $role_pkg is installed, at a version in its range" "$RUN" "$L package deps $role_pkg --recursive"
# The range excludes the default catalog's lez_core 0.5.1: the same lez_core
# with only its manifest version changed to 0.5.1 is reported version_mismatch.
if [[ -n "${file_of[lez_core]:-}" && -n "${file_of[lez_swap_node]:-}" ]]; then
  python3 -I - "$work/in/lez_core.lgx" "$work/x/lez_core-0.5.1.lgx" <<'PY'
import io, json, sys, tarfile
with tarfile.open(sys.argv[1], "r:gz") as t:
    members = [(m, t.extractfile(m).read() if m.isfile() else None) for m in t.getmembers()]
with tarfile.open(sys.argv[2], "w:gz", format=tarfile.GNU_FORMAT) as t:
    for m, d in members:
        if m.name == "manifest.json":
            j = json.loads(d); j["version"] = "0.5.1"; d = json.dumps(j, indent=2).encode(); m.size = len(d)
        t.addfile(m, io.BytesIO(d) if d is not None else None)
PY
  docker cp "$work/x/lez_core-0.5.1.lgx" "$RUN:/tmp/lez_core-0.5.1.lgx" >/dev/null 2>&1
  step '(( rc == 0 )) && jq -e "any(.[]; .name == \"lez_core\" and .status == \"version_mismatch\")" "$o" >/dev/null' \
    "a lez_core 0.5.1 (the default catalog's version) does not satisfy lez_swap_node's range" "$RUN" \
    "$L package install /tmp/lez_core-0.5.1.lgx -y >/dev/null && $L --json package deps lez_swap_node"
  step "$ok" "install lez_core again with its root hash" "$RUN" \
    "$L package install ${file_of[lez_core]} --root-hash ${root_of[lez_core]:-none} -y"
fi
step "$ok" "package ls" "$RUN" "$L package ls"

if [[ -n "$network_pkg" ]]; then
  section "The LEZ network's operator package"
  step "$ok" "install $network_pkg with its root hash" "$RUN" \
    "$L package install ${file_of[$network_pkg]} --root-hash ${root_of[$network_pkg]:-none} -y"
  step '(( rc == 0 )) && jq -e "any(.[]; .name == \"blockchain_module\") and any(.[]; .name == \"lez_indexer_module\") and all(.[]; (.requiredVersion // \"\") != \"\" and .status == \"installed\")" "$o" >/dev/null' \
    "$network_pkg declares blockchain_module and lez_indexer_module with version ranges, both installed in range" "$RUN" \
    "$L --json package deps $network_pkg --recursive"
fi

section "Lifecycle and behaviour"
step "$ok" "module load $role_pkg" "$RUN" "$L module load $role_pkg"
step "$ok" "module load lez_swap_node (with its declared dependencies)" "$RUN" "$L module load lez_swap_node"
step "$ok" "module show lez_swap_node" "$RUN" "$L module show lez_swap_node"
step "$callok" "call lez_swap_node node_status $role" "$RUN" "$L call lez_swap_node node_status $role"
step '[[ -s "$o" ]]' "the installed package graph carries the $role Node and its launcher" "$RUN" \
  "find $S/modules $S/plugins \\( -name node-entrypoint.sh -o -name 'lez-$role-node*' \\) -print"
node_pkg="lez_${role}_node"
step "$ok" "module load $node_pkg (the $role Node package, with its declared dependencies)" "$RUN" "$L module load $node_pkg"
step "$callok"' && [[ "$(result)" == /*/bin/node-entrypoint.sh ]]' "call $node_pkg launcher: the installed launcher" "$RUN" "$L call $node_pkg launcher"
launcher="$(result)"
step "$callok"' && result | jq -e ".files[\"lez-'"$role"'-node\"]" >/dev/null' "call $node_pkg payload: the installed files and their sha256" "$RUN" "$L call $node_pkg payload"
step "$callok"' && [[ "$(result)" == 0 ]]' "start_node with the package's launcher" "$RUN" \
  "$L call lez_swap_node start_node $role ${launcher:-/missing} $S/logs/$role-node.log $S/$role-node.hold"
sleep 3
step "$callok"' && result | jq -e ".starts >= 1" >/dev/null' "lez_swap_node launched the $role Node from the package" "$RUN" "$L call lez_swap_node node_status $role"
step info "the $role Node's output (without the role's configuration, which Compose provides, it stops at once)" "$RUN" "tail -5 $S/logs/$role-node.log"
docker exec "$RUN" bash -c "$L call lez_swap_node stop_node $role" >/dev/null 2>&1

# The supervisor itself, with a stand-in launcher: a short shell script, not a
# Node. It leaves a helper (sleep 299) in its process group, as the real
# launcher leaves its forwards and sidecars, and becomes the stand-in Node (sleep 300).
step info "STUB: a stand-in launcher (sleeps; not a Node)" "$RUN" \
  "printf '#!/bin/sh\necho stub \$1 \$\$\nsleep 299 &\nexec sleep 300\n' > /tmp/stub-launcher && chmod 0755 /tmp/stub-launcher && cat /tmp/stub-launcher"
step "$callok"' && [[ "$(result)" == 0 ]]' "STUB: start_node with the stand-in launcher" "$RUN" \
  "$L call lez_swap_node start_node $role /tmp/stub-launcher /tmp/stub.log /tmp/stub.hold"
sleep 2
step "$callok"' && grep -q running "$o"' "STUB: node_status reports the supervised child" "$RUN" "$L call lez_swap_node node_status $role"
child="$(result | jq -r '.pid // empty' 2>/dev/null)"
step "$ok" "module stats" "$RUN" "$L module stats"
# The runtime samples one pid per module, the logos_host it started
# (liblogos logos_core_get_module_stats -> ProcessStats::getModuleStats over
# ModuleManager::getModuleProcessIds; /proc/<pid>/stat utime+stime and VmRSS of
# that pid only), and a module has no way to add processes to it. So the Node's
# own use comes from lez_swap_node's node_stats, judged against /proc below.
step info "module stats lists lez_swap_node's host, not the $role Node (pid ${child:-?}): the runtime samples one pid per module (upstream request: \"module stats reports only the logos_host process; include (or list) the processes a module host started\")" \
  "$RUN" "$L module stats"
# node_stats_match: the call's result names the Node (the pid node_status
# gave), its group as /proc shows it (the stand-in Node and its helper, nothing
# else), the lez_swap_node host as the Node's parent, and each process's
# memory as its VmRSS (within 5% or 64 kB).
node_stats_match() {
  python3 -I - "$o" "${child:-0}" <<'PY'
import json, sys
lines = open(sys.argv[1]).read().splitlines()
child = int(sys.argv[2])
try:
    call = json.loads(next(l for l in lines if l.startswith("{") and '"method"' in l))
    r = json.loads(call["result"])
    live = {}
    for l in lines:
        f = l.split()
        if len(f) >= 6 and f[0] == "live":
            live[int(f[1])] = {"ppid": int(f[2]), "pgrp": int(f[3]), "rss_kb": int(f[4]), "cmd": " ".join(f[5:])}
    host = [p for p, v in live.items() if "logos_host --name lez_swap_node " in v["cmd"] + " "]
    group = sorted(p for p, v in live.items() if v["pgrp"] == child)
    procs = {p["pid"]: p for p in r["processes"]}
    ok = (call.get("status") == "ok" and child > 0 and r["pid"] == child and r["state"] == "running"
          and r["processes"][0]["pid"] == child and len(host) == 1 and r["host"]["pid"] == host[0]
          and procs[child]["ppid"] == host[0] and len(group) == 2 and sorted(procs) == group
          and r["group"]["processes"] == 2 and r["uptime_seconds"] > 0)
    for p in group:
        q = procs[p]
        ok = ok and q["threads"] >= 1 and q["fds"] >= 1 and \
            abs(q["memory_mb"] * 1024 - live[p]["rss_kb"]) <= max(64, 0.05 * live[p]["rss_kb"])
except (StopIteration, KeyError, IndexError, TypeError, ValueError):
    ok = False
sys.exit(0 if ok else 1)
PY
}
step node_stats_match "call lez_swap_node node_stats $role: the $role Node (pid ${child:-?}) and its process group, as /proc shows them" "$RUN" \
  "$L call lez_swap_node node_stats $role; echo; echo 'pid ppid pgrp VmRSS_kB cmdline, from /proc:'; for p in \$(procs | awk '(\$3 == \"sleep\" && (\$4 == \"299\" || \$4 == \"300\")) || (\$3 ~ /logos_host\$/ && \$5 == \"lez_swap_node\") {print \$1}'); do echo \"live \$p \$(awk '{print \$4, \$5}' /proc/\$p/stat) \$(awk '/^VmRSS/{print \$2}' /proc/\$p/status) \$(tr '\\0' ' ' </proc/\$p/cmdline)\"; done"
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
step '! grep -Eq "lez_core|lez_swap_node|lez_(maker|taker)_node|lez_atomic_swap" "$o"' "nothing of ours is left installed" "$RUN" "$L package ls"
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
  # The module builder's lgpm is the development build (logos-package-manager
  # `cli`), which takes only `<platform>-dev` variants. Logos's own release
  # package is the control: our release packages are judged by whether lgpm
  # treats them as it treats blockchain_module.
  lgpm="mkdir -p /x/lgpm-root/modules /x/lgpm-root/plugins && /x/lgpm --modules-dir /x/lgpm-root/modules --ui-plugins-dir /x/lgpm-root/plugins install --file"
  control_rc=""
  if [[ -n "${file_of[blockchain_module]:-}" ]]; then
    step info "lgpm (v0.2-era, the module builder's development build): install blockchain_module (Logos's release package, the control)" "$TOOLS" \
      "$lgpm ${file_of[blockchain_module]}"
    control_rc=$rc
  fi
  for name in lez_swap_node "$role_pkg"; do
    [[ -n "${file_of[$name]:-}" ]] || continue
    if [[ -n "$control_rc" ]]; then
      step "(( (rc == 0) == (control_rc == 0) ))" "lgpm (v0.2-era development build) treats $name as it treats Logos's release package" "$TOOLS" "$lgpm ${file_of[$name]}"
    else
      step "$ok" "lgpm (v0.2-era, the desks' installer): install $name" "$TOOLS" "$lgpm ${file_of[$name]}"
    fi
  done
fi

{
  printf '\n## Summary\n\n```\n'
  cat "$out/summary.tsv"
  printf '```\n\n%d steps, %d judged FAIL.\n' "$n" "$fails"
} >>"$T"
log "transcript: $T ($fails FAIL of $n steps)"
((fails == 0))
