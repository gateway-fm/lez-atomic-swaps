#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

build_dir="${LEZ_PREVIEW_BUILD_DIR:-$PWD/apps/preview/build}"

if [[ "$(uname -s)" == "Darwin" ]]; then
    qt_prefix="${CMAKE_PREFIX_PATH:-/opt/homebrew/opt/qt}"
else
    qt_prefix="${CMAKE_PREFIX_PATH:-}"
fi

cmake_args=()
if [[ -n "$qt_prefix" ]]; then
    cmake_args+=(-DCMAKE_PREFIX_PATH="$qt_prefix")
fi

if [[ ! -f "$build_dir/CMakeCache.txt" ]]; then
    cmake -S apps/preview -B "$build_dir" "${cmake_args[@]}"
fi

cmake --build "$build_dir" --config Release

binary="$build_dir/lez-swap-preview"
if [[ "$(uname -s)" == "Darwin" ]]; then
    app_bundle="$build_dir/lez-swap-preview.app"
    if [[ -d "$app_bundle" ]]; then
        binary="$app_bundle/Contents/MacOS/lez-swap-preview"
    fi
fi

exec "$binary"
