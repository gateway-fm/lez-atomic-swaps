#!/usr/bin/env bash
# Full M5-mode BTC application swap, native arm64: real daemon, CLIs,
# Bitcoin Core 31.1 regtest, LEZ v0.2 devnet, pinned escrow artifacts.
set -euxo pipefail

export RUN_ID="${LEZ_M3_RUN_ID:-m5arm-$(date -u +%m%d%H%M%S)}"
exec > "/tmp/${RUN_ID}.log" 2>&1
export DOCKER_BUILDKIT=1
export LEZ_V02_SOURCE_DIR=/Users/mandrigin/Desktop/las-logos/runner-work/lez-source
export LEZ_V02_SERVICES_DIR=/tmp/lez-v02-services-a58fbce2-20260713/release
export LEZ_V02_R0VM=/provision/tools-arm/bin/r0vm
export LEZ_V02_ARTIFACT_TARGET_DIR=/tmp/lez-m3-artifact-arm
export RAPIDSNARK_LIB_DIR=/provision/rapidsnark-arm
export BINDGEN_EXTRA_CLANG_ARGS=-I/usr/lib/gcc/aarch64-linux-gnu/13/include
export M5_LEZ_DEPLOYER_SHA256="$(sha256sum "$LEZ_V02_ARTIFACT_TARGET_DIR/debug/lez-zec-escrow-v02-deployer" | cut -d' ' -f1)"
export M5_BTC_APPLICATION_MODE=1
export M3_ACTOR_POC_JOURNEY=claim
export M3_ACTOR_POC_ASSET_MODE=native
export M3_ACTOR_POC_SCHEDULE=sequential

# Attach mode: swaps execute on the long-standing settlement chains with the
# persistent wallet identities — the mainnet-shaped path.
if [[ "${LEZ_M3_ATTACH:-0}" == 1 ]]; then
  market_root=/Users/mandrigin/Desktop/las-logos/runner-work/market
  export LEZ_ATTACH_BTC_RUN="${LEZ_ATTACH_BTC_RUN:-market-btc-0001}"
  export LEZ_ATTACH_LEZ_RUN="${LEZ_ATTACH_LEZ_RUN:-market-lez-0001}"
  export LEZ_ATTACH_MAKER_IDENTITY_DIR="${market_root}/identities/${LEZ_INTERACTIVE_MAKER_WALLET:?attach requires the maker wallet}"
  export LEZ_ATTACH_TAKER_IDENTITY_DIR="${market_root}/identities/${LEZ_INTERACTIVE_TAKER_WALLET:?attach requires the taker wallet}"
  export LEZ_ATTACH_BOOTSTRAP_MANIFEST="${market_root}/market-bootstrap.env"
fi

cd /Users/mandrigin/Desktop/las-logos/runner-work/repo
if [[ "${LEZ_M3_INTERACTIVE:-0}" == 1 ]]; then
  /tmp/lez-interactive-m3-outer.sh
else
  ./scripts/run-m3-actor-local-poc.sh
fi
echo "M5-ARM-RC=$?"
