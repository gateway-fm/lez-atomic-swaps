# shellcheck shell=bash
# release-env.sh — sourced: the release images a release bundle names.
#
# A release bundle (scripts/package-dist.sh) carries release.env beside
# compose.yaml, naming the published images and the escrow program, and no
# images/ build contexts. Each name the environment does not already give is
# taken from it and exported, so every command the sourcing script starts
# (node-e2e.py, ui-e2e.sh, up.sh, gen-config.sh, the fixtures) resolves the
# bundle's images and escrow program: LEZ_IMAGE_PREFIX, LEZ_IMAGE_TAG,
# LEZ_ESCROW_PROGRAM_ID, LEZ_RELEASE_COMMIT, and start.sh's LEZ_MARKET_ROOT and
# LEZ_WALLET_IDENTITIES. A bundle never builds: unless
# LEZ_IMAGES says otherwise it is start.sh's choice, `present` for a
# developer's own `local` images and `pull` for a release's. LEZ_RELEASE_BUNDLE
# is 1 in a bundle. A checkout has no release.env and is unchanged.
#
# scripts/lane.sh sources this (every script that runs the stack sources
# lane.sh); regress.sh and gen-config.sh source it themselves, as they source
# lane.sh only in a subshell or only for a lane >= 2.
release_env_dir="$(dirname "${BASH_SOURCE[0]}")/.."
if [[ -f "$release_env_dir/release.env" && -f "$release_env_dir/compose.yaml" ]]; then
  for release_env_key in LEZ_IMAGE_PREFIX LEZ_IMAGE_TAG LEZ_ESCROW_PROGRAM_ID LEZ_RELEASE_COMMIT; do
    if [[ -z "${!release_env_key:-}" ]]; then
      release_env_value="$(sed -n "s/^$release_env_key=//p" "$release_env_dir/release.env" | head -1)"
      if [[ -n "$release_env_value" ]]; then
        printf -v "$release_env_key" '%s' "$release_env_value"
        export "${release_env_key?}"
      fi
    fi
  done
  if [[ -z "${LEZ_IMAGES:-}" && -n "${LEZ_IMAGE_TAG:-}" ]]; then
    if [[ "$LEZ_IMAGE_TAG" == local ]]; then LEZ_IMAGES=present; else LEZ_IMAGES=pull; fi
    export LEZ_IMAGES
  fi
  LEZ_RELEASE_BUNDLE=1
  export LEZ_RELEASE_BUNDLE
  # The market root start.sh chose (wallet identities, the escrow deployment):
  # the one runtime.env records, else start.sh's default, market/ beside
  # compose.yaml. gen-config.sh looks for a checkout's market root outside
  # deploy/, which a bundle has not, so a later up.sh, regress.sh or fixture
  # would find none (or render the genesis for the recorded wallet ids).
  if [[ -z "${LEZ_MARKET_ROOT:-}" ]]; then
    if [[ -f "$release_env_dir/runtime/runtime.env" ]]; then
      LEZ_MARKET_ROOT="$(sed -n 's/^LEZ_MARKET_ROOT=//p' "$release_env_dir/runtime/runtime.env" | head -1)"
    fi
    LEZ_MARKET_ROOT="${LEZ_MARKET_ROOT:-$(cd "$release_env_dir" && pwd)/market}"
    export LEZ_MARKET_ROOT
  fi
  LEZ_WALLET_IDENTITIES="${LEZ_WALLET_IDENTITIES:-$LEZ_MARKET_ROOT/identities}"
  export LEZ_WALLET_IDENTITIES
  unset release_env_key release_env_value
fi
unset release_env_dir
