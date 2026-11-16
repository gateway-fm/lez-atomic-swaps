#!/usr/bin/env bash
# package-chat-module.sh — the Chat module our catalog publishes: Logos's
# released chat_module 0.2.2 with Delivery pinned to 0.2.x.
#
#   scripts/package-chat-module.sh --out <file.lgx> [--from <chat_module-0.2.2.lgx>]
#   scripts/package-chat-module.sh --print <version|delivery-requirement|source-sha256>
#
# Logos's chat_module 0.2.2 declares a bare `delivery_module`. logosctl 0.3.x
# and Basecamp 0.3.x resolve each dependency entry of a plan on its own, so
# that bare name plans the newest Delivery (0.3.3) beside the 0.2.x a desk
# requires, both land in the one modules/delivery_module directory, and the
# copy written last is what runs (issue #147). Chat 0.2.2 cannot read
# Delivery 0.3.x's `messageReceived` (0.3.x adds a `source` argument before
# `timestamp`), so Delivery must stay on 0.2.x while the desks run Chat 0.2.2.
#
# This takes the official release asset (or --from, a copy of it), checks it
# against its pinned sha256, and writes it again with two manifest fields
# changed: `version` (CHAT_MODULE_VERSION below) and `dependencies`
# (`delivery_module` with DELIVERY_REQUIREMENT, the requirement the desks give
# their own `delivery_module` entry, so both entries resolve to one Delivery).
# Every other member of the package — each variant's files, their modes and
# order — is copied byte for byte; the manifest's hashes cover those files
# only, so `lgx verify` holds with the hashes the release carries. The output
# is gzip-compressed without a timestamp or file name: the same input gives
# the same bytes.
# The release asset is unsigned; the catalog signs what it publishes.
#
# The version is a pre-release of the release it is cut from: it sorts below
# the official 0.2.2, so no range an official package declares picks it over
# an official release, and no official release equals it. Our desks require
# it exactly (pin-package-dependencies.sh: a pre-release is required exactly).
set -euo pipefail

readonly CHAT_MODULE_SOURCE_VERSION=0.2.2
readonly CHAT_MODULE_SOURCE_SHA256=ab55abc49771f9bc52d689c463b22228241ffdb98150cfc3d45071d5d373dac1
readonly CHAT_MODULE_SOURCE_URL="https://github.com/logos-co/logos-modules-release/releases/download/chat_module-v$CHAT_MODULE_SOURCE_VERSION/chat_module-$CHAT_MODULE_SOURCE_VERSION.lgx"
readonly CHAT_MODULE_VERSION=0.2.2-delivery.1
readonly DELIVERY_REQUIREMENT='~0.2.0'

out="" from=""
while (($#)); do
  case "$1" in
    --out) out="${2:?}"; shift 2 ;;
    --from) from="${2:?}"; shift 2 ;;
    --print)
      case "${2:-}" in
        version) echo "$CHAT_MODULE_VERSION" ;;
        delivery-requirement) echo "$DELIVERY_REQUIREMENT" ;;
        source-sha256) echo "$CHAT_MODULE_SOURCE_SHA256" ;;
        *) echo "--print takes version, delivery-requirement or source-sha256" >&2; exit 64 ;;
      esac
      exit 0 ;;
    -h|--help) sed -n '2,31p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 64 ;;
  esac
done
[[ -n "$out" ]] || { echo "usage: $0 --out <file.lgx> [--from <chat_module-$CHAT_MODULE_SOURCE_VERSION.lgx>]" >&2; exit 64; }
command -v python3 >/dev/null || { echo "python3 is required" >&2; exit 66; }

sha256() { if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -c1-64; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
if [[ -z "$from" ]]; then
  from="$work/chat_module-$CHAT_MODULE_SOURCE_VERSION.lgx"
  curl -fsSL --retry 3 -o "$from" "$CHAT_MODULE_SOURCE_URL"
fi
[[ -f "$from" ]] || { echo "$from is not a file" >&2; exit 65; }
actual="$(sha256 "$from")"
[[ "$actual" == "$CHAT_MODULE_SOURCE_SHA256" ]] \
  || { echo "$from: sha256 $actual, not the chat_module $CHAT_MODULE_SOURCE_VERSION release ($CHAT_MODULE_SOURCE_SHA256)" >&2; exit 65; }

mkdir -p "$(dirname "$out")"
python3 -I - "$from" "$out" "$CHAT_MODULE_SOURCE_VERSION" "$CHAT_MODULE_VERSION" "$DELIVERY_REQUIREMENT" <<'PY'
import gzip, io, json, os, sys, tarfile

src, dst, source_version, version, delivery = sys.argv[1:6]
with tarfile.open(src, "r:gz") as t:
    members = [(m, t.extractfile(m).read() if m.isfile() else None) for m in t.getmembers()]
names = [m.name for m, _ in members]
if names.count("manifest.json") != 1:
    sys.exit(f"{src}: expected one manifest.json, found {names.count('manifest.json')}")
manifest = json.loads(next(d for m, d in members if m.name == "manifest.json"))
deps = manifest.get("dependencies")
if manifest.get("name") != "chat_module" or manifest.get("version") != source_version or deps != ["delivery_module"]:
    sys.exit(f"{src}: not chat_module {source_version} with a bare delivery_module "
             f"({manifest.get('name')} {manifest.get('version')}, dependencies {deps})")
manifest["version"] = version
manifest["dependencies"] = [{"name": "delivery_module", "version": delivery}]
tmp = dst + ".part"
with open(tmp, "wb") as raw, gzip.GzipFile(filename="", fileobj=raw, mode="wb", mtime=0) as gz, \
        tarfile.open(fileobj=gz, mode="w", format=tarfile.GNU_FORMAT) as t:
    for m, d in members:
        if m.name == "manifest.json":
            d = json.dumps(manifest, indent=2).encode()
            m.size = len(d)
        t.addfile(m, io.BytesIO(d) if d is not None else None)
os.chmod(tmp, 0o644)
os.replace(tmp, dst)
print(f"chat_module {version} ({', '.join(sorted(manifest['main']))}; "
      f"delivery_module {delivery}; root {manifest['hashes']['root']}) -> {dst}")
PY
echo "sha256 $(sha256 "$out")"
