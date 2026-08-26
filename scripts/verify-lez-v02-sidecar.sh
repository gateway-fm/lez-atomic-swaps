#!/usr/bin/env bash
set -euo pipefail

# Keep this check ahead of every Cargo invocation. rust-rapidsnark's build.rs
# can otherwise attempt an implicit release-asset download even when Cargo is
# offline.
if [[ -z "${RAPIDSNARK_LIB_DIR:-}" ]]; then
  echo 'RAPIDSNARK_LIB_DIR is required; refusing to start Cargo before native libraries are verified' >&2
  exit 2
fi
if [[ -z "${BINDGEN_EXTRA_CLANG_ARGS:-}" ]]; then
  echo 'BINDGEN_EXTRA_CLANG_ARGS is required; refusing to start Cargo before bindgen input is verified' >&2
  exit 2
fi

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly repository_root
readonly crate_dir="${repository_root}/compat/lez-v0_2-sidecar"
readonly manifest="${crate_dir}/Cargo.toml"
readonly dependency_policy="${crate_dir}/check-dependency-policy.sh"
readonly contract="${repository_root}/compat/lez-v0.2-provisional/local-stack.toml"
readonly contracted_bindgen_args='-I/usr/lib/gcc/x86_64-linux-gnu/13/include'

if [[ "${RAPIDSNARK_LIB_DIR}" != /* ]]; then
  echo 'RAPIDSNARK_LIB_DIR must be an absolute path' >&2
  exit 2
fi
if [[ "${BINDGEN_EXTRA_CLANG_ARGS}" != "${contracted_bindgen_args}" ]]; then
  echo "BINDGEN_EXTRA_CLANG_ARGS must equal ${contracted_bindgen_args}" >&2
  exit 2
fi
if [[ ! -d "${RAPIDSNARK_LIB_DIR}" ]]; then
  echo "RAPIDSNARK_LIB_DIR is not a directory: ${RAPIDSNARK_LIB_DIR}" >&2
  exit 2
fi

for required_file in "${manifest}" "${dependency_policy}" "${contract}"; do
  if [[ ! -f "${required_file}" ]]; then
    echo "missing sidecar verification input: ${required_file}" >&2
    exit 2
  fi
done

for required_command in awk cargo cargo-deny rg sha256sum; do
  if ! command -v "${required_command}" >/dev/null; then
    echo "${required_command} is required by the LEZ v0.2 sidecar verifier" >&2
    exit 2
  fi
done

# This is intentionally an exact second binding of the four identities in the
# local-stack contract. If either file drifts, certification stops instead of
# trusting newly substituted native code.
readonly expected_contract_libraries='"librapidsnark.a" = "d4133227f845ff5bfa3672eb5b9c018a6a086bfa164b176bdaf76949c7d1f423"
"libgmp.a" = "0a910b420c3ad603c83c9dc2818c7ae05394c231ca23135c7b873e8e680ea41b"
"libfq.a" = "797b5d24bb8e8b088f811bddfff35f33973af9c797fb3812489cd42ba6a957d0"
"libfr.a" = "40f809394904682cb5517845cd3c2f936a5eb4609712534b573f552f2811fb82"'
actual_contract_libraries="$({
  awk '
    $0 == "[packaging.rapidsnark.library_sha256]" { in_section = 1; next }
    in_section && /^\[/ { exit }
    in_section && /^"/ { print }
  ' "${contract}"
})"
if [[ "${actual_contract_libraries}" != "${expected_contract_libraries}" ]]; then
  echo 'LEZ v0.2 local-stack contract no longer contains the exact four native-library identities' >&2
  exit 2
fi
if ! rg -Fqx 'bindgen_extra_clang_args = "-I/usr/lib/gcc/x86_64-linux-gnu/13/include"' "${contract}"; then
  echo 'LEZ v0.2 local-stack contract no longer binds the required bindgen include' >&2
  exit 2
fi

verify_native_library() {
  local filename="$1"
  local expected_sha256="$2"
  local library_path="${RAPIDSNARK_LIB_DIR}/${filename}"
  local actual_sha256

  if [[ ! -f "${library_path}" ]]; then
    echo "missing contracted native library: ${library_path}" >&2
    exit 2
  fi
  actual_sha256="$(sha256sum -- "${library_path}")"
  actual_sha256="${actual_sha256%% *}"
  if [[ "${actual_sha256}" != "${expected_sha256}" ]]; then
    echo "native-library identity drift for ${filename}: expected ${expected_sha256}, got ${actual_sha256}" >&2
    exit 2
  fi
}

verify_native_library \
  'librapidsnark.a' \
  'd4133227f845ff5bfa3672eb5b9c018a6a086bfa164b176bdaf76949c7d1f423'
verify_native_library \
  'libgmp.a' \
  '0a910b420c3ad603c83c9dc2818c7ae05394c231ca23135c7b873e8e680ea41b'
verify_native_library \
  'libfq.a' \
  '797b5d24bb8e8b088f811bddfff35f33973af9c797fb3812489cd42ba6a957d0'
verify_native_library \
  'libfr.a' \
  '40f809394904682cb5517845cd3c2f936a5eb4609712534b573f552f2811fb82'

export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
export CARGO_NET_OFFLINE=true

echo 'LEZ v0.2 sidecar native inputs verified; starting locked offline Cargo gates'

cargo +1.96.0 fmt \
  --manifest-path "${manifest}" \
  --all \
  -- \
  --check
cargo +1.96.0 test \
  --manifest-path "${manifest}" \
  --locked \
  --offline \
  --all-targets \
  --all-features
cargo +1.96.0 clippy \
  --manifest-path "${manifest}" \
  --locked \
  --offline \
  --all-targets \
  --all-features \
  -- \
  -D warnings
cargo +1.96.0 rustdoc \
  --manifest-path "${manifest}" \
  --locked \
  --offline \
  --all-features \
  --lib \
  -- \
  -D warnings
bash "${dependency_policy}"

echo 'LEZ v0.2 sidecar verification: ok (native inputs attested; Cargo remained offline)'
