#!/usr/bin/env bash
# Runs one fuzz target under cargo-fuzz for a fixed wall time. The pinned nightly and
# cargo-fuzz only ever execute inside the container; the repository is mounted read-only and copied
# into the work directory (cargo-fuzz writes into its fuzz dir), which also holds the toolchain,
# target dir, working corpus, log and any crash artifacts.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly image="rust:1.96.0-bookworm"
# nightly-2026-07-01 added a built-in `unroll` attribute that makes pasta_curves 0.6.1 (in the
# Zcash SDK's graph, which node-common and swap-store pull in) fail to compile.
readonly nightly="nightly-2026-06-01"
readonly cargo_fuzz_version="0.13.2"
readonly target="${1:-}"
readonly seconds="${2:-}"
jobs="${FUZZ_JOBS:-6}"

case "$target" in
  coordinator) max_len=512 ;;             # one profile byte and the 511 actions the harness reads
  offer_announcement) max_len=32769 ;;    # one byte over the announcement bound
  chat_frame) max_len=16384 ;;            # frames may reach 1 MiB; mutation stays far below it
  zec_agreement_wire) max_len=16385 ;;    # one byte over the agreement record bound
  *) max_len=0 ;;
esac
(( max_len > 0 )) && [[ "$seconds" =~ ^[1-9][0-9]{0,5}$ ]] || {
  echo "usage: $0 coordinator|offer_announcement|chat_frame|zec_agreement_wire <seconds> [work-dir]" >&2
  echo "       (FUZZ_JOBS, default 6, sets the parallel jobs)" >&2
  exit 2
}
[[ "$jobs" =~ ^[1-9][0-9]?$ ]] || {
  echo "FUZZ_JOBS must be an integer from 1 through 99" >&2
  exit 2
}
command -v docker >/dev/null || {
  echo "Docker is required for the fuzz run" >&2
  exit 1
}

work_dir="${3:-$(mktemp -d "${TMPDIR:-/tmp}/lez-$target-fuzz.XXXXXX")}"
mkdir -p "$work_dir/corpus" "$work_dir/artifacts"
work_dir="$(cd "$work_dir" && pwd -P)"
cp -a "fuzz/corpus/$target/." "$work_dir/corpus/"

echo "$target fuzz: ${seconds}s, ${jobs} jobs, work dir $work_dir" >&2
docker run --rm \
  --name "lez-$target-fuzz-$$" \
  --user "$(id -u):$(id -g)" \
  --env HOME=/work \
  --env RUSTUP_HOME=/work/rustup \
  --env CARGO_TARGET_DIR=/work/target \
  --env PATH=/work/cargo/bin:/usr/local/cargo/bin:/usr/local/bin:/usr/bin:/bin \
  --volume "$(pwd -P):/workspace:ro" \
  --volume "$work_dir:/work" \
  --workdir /work \
  "$image" \
  bash -euo pipefail -c '
    rm -rf /work/src && mkdir /work/src
    tar -C /workspace --exclude=./.git --exclude=./target -cf - . | tar -C /work/src -xf -
    cd /work/src
    rustup toolchain install --no-self-update --profile minimal "$1"
    export CARGO_HOME=/work/cargo
    PATH="/work/rustup/toolchains/$1-$(uname -m)-unknown-linux-gnu/bin:$PATH"
    cargo install cargo-fuzz --version "$2" --locked
    rustc --version --verbose
    cargo fuzz --version
    status=0
    cargo fuzz run --fuzz-dir fuzz --target-dir /work/target "$6" /work/corpus -- \
      "-fork=$3" "-max_total_time=$4" "-max_len=$5" -timeout=2 -print_final_stats=1 \
      -artifact_prefix=/work/artifacts/ 2>&1 | tee /work/fuzz.log || status=$?
    if (( status != 0 )); then
      echo "$6 fuzz failed with status $status; artifacts in /work/artifacts" >&2
      exit "$status"
    fi
    rm -rf /work/corpus-min
    cp -a /work/corpus /work/corpus-min
    cargo fuzz cmin --fuzz-dir fuzz --target-dir /work/target "$6" /work/corpus-min \
      -- "-max_len=$5" 2>&1 | tee /work/cmin.log
  ' bash "$nightly" "$cargo_fuzz_version" "$jobs" "$seconds" "$max_len" "$target"
