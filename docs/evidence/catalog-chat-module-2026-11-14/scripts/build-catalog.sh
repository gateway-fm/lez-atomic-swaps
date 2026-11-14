#!/opt/homebrew/bin/bash
# build-catalog.sh <pkg dir> <srv dir> <host:port> — a local HTTPS test catalog of
# the packages in <pkg dir>: <name>-<version>.lgx, index.json from the catalog
# tool's index.py (lgx semver ordering), logos-repo.json naming <host:port>.
set -euo pipefail
P="${1:?pkg dir}" SRV="${2:?srv dir}" HP="${3:?host:port}"
TOOL=$HOME/tmp/lez-030-multiplatform/up/logos-modules-release-tool/index.py
export PATH="$HOME/tmp/lez-031-signed/src/build-HEAD:$PATH"
rm -rf "$SRV"; mkdir -p "$SRV"; urls="$SRV.urls.txt"; : > "$urls"
for f in "$P"/*.lgx; do
  m="$(tar -xzOf "$f" manifest.json)"; n="$(jq -r .name <<<"$m")"; v="$(jq -r .version <<<"$m")"
  cp -L "$f" "$SRV/$n-$v.lgx"
  echo "https://$HP/$n-$v.lgx $SRV/$n-$v.lgx" >> "$urls"
done
python3 -I "$TOOL" build "$urls" -o "$SRV/index.json" --name lez-atomic-swaps --fetch none
cat > "$SRV/logos-repo.json" <<J
{
 "schemaVersion": 1,
 "name": "lez-atomic-swaps",
 "displayName": "LEZ Atomic Swaps (local test)",
 "description": "local test catalog",
 "homepage": "https://github.com/gateway-fm/lez-atomic-swaps",
 "indexUrl": "https://$HP/index.json",
 "trustedSigners": []
}
J
python3 -I "$TOOL" list "$SRV/index.json"
