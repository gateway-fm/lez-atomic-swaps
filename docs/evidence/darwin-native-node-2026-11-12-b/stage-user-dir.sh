#!/opt/homebrew/bin/bash
# stage-user-dir.sh <user dir> — a Basecamp user directory with both desks and
# everything they depend on, darwin-arm64, laid out as Basecamp's package
# manager installs a package (the variant's files, manifest.json, `variant`):
# desks under plugins/, modules under modules/.
set -euo pipefail
user="${1:?user dir}"
v=darwin-arm64
M=$MODULES
O=$OUT
pkgs=(
  "plugins:$M/desk-maker/logos-lez_atomic_swap_maker-module.lgx"
  "plugins:$M/desk-taker/logos-lez_atomic_swap_taker-module.lgx"
  "modules:$CORE/core/logos-lez_core-module-lib.lgx"
  "modules:$M/swap-node/logos-lez_swap_node-module-lib.lgx"
  "modules:$O/lez_maker_node.lgx"
  "modules:$O/lez_taker_node.lgx"
  "modules:$(ls /nix/store/sa9r0kdq6pihzq2lg6791z99s34b7m7h-logos-chat_module-module-lib-lgx-0.2.2/*.lgx)"
  "modules:$(ls /nix/store/x28vmcdays769g6iz3ncs4c23xdg7lys-logos-delivery_module-module-lib-lgx-0.2.0/*.lgx)"
)
mkdir -p "$user/modules" "$user/plugins"
for entry in "${pkgs[@]}"; do
  kind="${entry%%:*}"; lgx="${entry#*:}"
  tmp="$(mktemp -d $TMPDIR/stage.XXXXXX)"
  tar -xzf "$lgx" -C "$tmp"
  name="$(jq -r .name "$tmp/manifest.json")"
  jq -e --arg v "$v" '.main[$v] != null' "$tmp/manifest.json" >/dev/null || { echo "$lgx has no $v" >&2; exit 65; }
  rm -rf "${user:?}/$kind/$name"; mkdir -p "$user/$kind/$name"
  cp -R "$tmp/variants/$v"/. "$user/$kind/$name"/
  cp "$tmp/manifest.json" "$user/$kind/$name/"
  printf '%s\n' "$v" >"$user/$kind/$name/variant"
  chmod -R u+w "$user/$kind/$name"
  rm -rf "$tmp"
  echo "staged $kind/$name $(jq -r .version "$user/$kind/$name/manifest.json")"
done
