#!/usr/bin/env bash
# merge-set.sh — the v0.3.0 catalog set with darwin-arm64 beside linux-arm64.
#
#   merge-set.sh <linux dir> <darwin dir> <checkout> <out dir>
#
# <linux dir>: the packages the v0.3.0 release run built (linux-arm64 only,
# the old dependency form), <name>.lgx. <darwin dir>: build-darwin-packages.sh
# output, <name>-darwin-arm64.lgx. For the six packages a desk installs, the
# Linux file's manifest is replaced by the darwin build's (its own `main` and
# `hashes` kept: the root hash covers the variant's files, not the manifest),
# so the two are identical as `lgx merge` requires; then merged, the set pinned
# once with the checkout's pin-package-dependencies.sh, and every package
# verified. lez_network and lez_indexer_module stay linux-arm64 and are only
# re-pinned. LGX is the module builder's lgx.
set -euo pipefail
linux="${1:?linux dir}" darwin="${2:?darwin dir}" checkout="${3:?checkout}" out="${4:?out dir}"
lgx="${LGX:?LGX: the module builder lgx}"
work="$(mktemp -d "${TMPDIR:-/tmp}/merge-set.XXXXXX")"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/norm" "$work/merged" "$out"

for name in lez_atomic_swap_maker lez_atomic_swap_taker lez_swap_node lez_maker_node lez_taker_node lez_core; do
  python3 -I - "$linux/$name.lgx" "$darwin/$name-darwin-arm64.lgx" "$work/norm/$name-linux-arm64.lgx" <<'PY'
import gzip, io, json, sys, tarfile
src, ref, dest = sys.argv[1:]
def read(path):
    with tarfile.open(path, "r:gz") as t:
        return [(m, t.extractfile(m).read() if m.isfile() else None) for m in t.getmembers()]
members = read(src)
own = json.loads(next(d for m, d in members if m.name == "manifest.json"))
manifest = json.loads(next(d for m, d in read(ref) if m.name == "manifest.json"))
manifest["main"], manifest["hashes"] = own["main"], own["hashes"]
with open(dest, "wb") as raw, gzip.GzipFile(fileobj=raw, mode="wb", mtime=0) as gz, \
        tarfile.open(fileobj=gz, mode="w", format=tarfile.GNU_FORMAT) as t:
    for m, d in members:
        if m.name == "manifest.json":
            d = json.dumps(manifest, indent=2).encode()
            m.size = len(d)
        t.addfile(m, io.BytesIO(d) if d is not None else None)
PY
  "$lgx" verify "$work/norm/$name-linux-arm64.lgx" >/dev/null
  "$lgx" merge "$work/norm/$name-linux-arm64.lgx" "$darwin/$name-darwin-arm64.lgx" -o "$work/merged/$name.lgx" -y >/dev/null
  "$checkout/deploy/scripts/restore-merged-manifest.py" "$work/merged/$name.lgx" \
    "$work/norm/$name-linux-arm64.lgx" "$darwin/$name-darwin-arm64.lgx"
  echo "merged $name: $(tar -xzOf "$work/merged/$name.lgx" manifest.json | jq -c '.main | keys')"
done
for name in lez_network lez_indexer_module blockchain_module chat_module delivery_module; do
  cp "$linux/$name.lgx" "$work/merged/$name.lgx"
done
"$checkout/deploy/scripts/pin-package-dependencies.sh" --out "$work/pinned" "$work/merged"/*.lgx
for name in lez_atomic_swap_maker lez_atomic_swap_taker lez_swap_node lez_maker_node lez_taker_node lez_core lez_network lez_indexer_module; do
  "$lgx" verify "$work/pinned/$name.lgx"
  install -m 0644 "$work/pinned/$name.lgx" "$out/$name.lgx"
done
grep -E '^(name|lez_)' "$work/pinned/packages.tsv" > "$out/packages.tsv"
(cd "$out" && shasum -a 256 -- *.lgx > SHA256SUMS)
