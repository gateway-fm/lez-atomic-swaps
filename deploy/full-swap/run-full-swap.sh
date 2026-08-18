#!/usr/bin/env bash
# Full M5-mode BTC application swap, native arm64: real daemon, CLIs,
# Bitcoin Core 31.1 regtest, LEZ v0.2 devnet, pinned escrow artifacts.
exec > /tmp/m5-arm.log 2>&1
set -euxo pipefail

export RUN_ID="m5arm-$(date -u +%m%d%H%M)"
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

cd /Users/mandrigin/Desktop/las-logos/runner-work/repo
./scripts/run-m3-actor-local-poc.sh
echo "M5-ARM-RC=$?"
