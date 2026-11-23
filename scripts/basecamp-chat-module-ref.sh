#!/usr/bin/env bash
set -euo pipefail

# Prints the Chat release that apps/basecamp/flake.lock pins, as a flake
# reference locked by revision. The per-role flakes (apps/basecamp/maker,
# apps/basecamp/taker) carry no lock file and name the Chat release by tag;
# Nix would ask the GitHub API to resolve that tag on every evaluation. The
# aggregate lock pins the same tag, so overriding the per-role input with this
# reference leaves the computed lock, and what is built, unchanged.

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly -a nix=(nix --extra-experimental-features 'nix-command flakes')
# shellcheck disable=SC2016 # a Nix expression, expanded by Nix
lock_expr='
  let
    lock = builtins.fromJSON (builtins.readFile ./apps/basecamp/flake.lock);
  in
    lock.nodes.${lock.nodes.${lock.root}.inputs.chat_module}'
locked_tag="$("${nix[@]}" eval --raw --impure --expr "(${lock_expr}).original.ref")"
reference="$("${nix[@]}" eval --raw --impure --expr \
  "let node = (${lock_expr}).locked; in \"github:\${node.owner}/\${node.repo}/\${node.rev}\"")"
if [[ ! "$reference" =~ ^github:logos-co/logos-chat-module/[0-9a-f]{40}$ ]]; then
  echo "apps/basecamp/flake.lock does not pin the Chat release by revision: ${reference}" >&2
  exit 1
fi
for role in maker taker; do
  flake="apps/basecamp/${role}/flake.nix"
  [[ "$(<"$flake")" =~ chat_module\.url\ =\ \"github:logos-co/logos-chat-module/([^\"]+)\" ]] ||
    { echo "${flake} does not name the Chat release" >&2; exit 1; }
  if [[ "${BASH_REMATCH[1]}" != "$locked_tag" ]]; then
    echo "${flake} names Chat ${BASH_REMATCH[1]}, apps/basecamp/flake.lock pins ${locked_tag}" >&2
    exit 1
  fi
done
printf '%s\n' "$reference"
