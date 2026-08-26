#!/usr/bin/env bash
set -euo pipefail
umask 077

fail() {
  echo "M3 official-wallet artifact preparation failed: $*" >&2
  exit 1
}

sha_file() {
  sha256sum "$1" | sed 's/ .*//'
}

require_owner_mode() {
  local path="$1" type="$2" mode="$3"
  [[ ! -L "$path" ]] || fail "unsafe symlink at $path"
  case "$type" in
    file) [[ -f "$path" ]] || fail "required file is missing: $path" ;;
    dir) [[ -d "$path" ]] || fail "required directory is missing: $path" ;;
    *) fail "internal owner-mode type is invalid" ;;
  esac
  [[ "$(stat -c %u "$path")" == "$(id -u)" ]] ||
    fail "cache path is not owned by the current uid: $path"
  [[ "$(stat -c %a "$path")" == "$mode" ]] ||
    fail "cache path has unsafe mode: $path"
}

emit_contract() {
  jq -n '{
    schema_version:1,
    kind:"m3_official_wallet_artifact_cache_contract",
    cache:{
      scope:"immutable_official_wallet_executable_only",
      publication:"per_input_lock_and_atomic_ref",
      same_uid_is_trusted:true,
      production_test_mode_forbidden:true,
      missing_ref_is_miss:true,
      invalid_ref_or_object_is_fatal:true},
    input_key:{
      binds_source_archive:true,
      binds_lockfile:true,
      binds_toolchain_and_target:true,
      binds_build_recipe_and_environment:true,
      binds_native_libraries:true,
      binds_bindgen_include_tree:true,
      binds_program_artifacts:true,
      binds_cargo_metadata:true,
      binds_target_library_tree:true,
      binds_validation_policy_and_helper:true,
      binds_expected_output_identity:true},
    object:{
      allowlisted_files:["manifest.json","wallet"],
      wallet_mode:"0500",
      manifest_mode:"0600"},
    consumption:{
      copy_or_reflink_not_hardlink:true,
      hash_source_before_and_after_copy:true,
      hash_private_destination:true,
      execute_only_private_copy:true},
    evidence:{canonical_input_manifest:true,object_and_runtime_identity:true,
      monotonic_duration_and_artifact_size:true},
    secrets_or_state_cached:false
  }'
}

[[ $# == 1 ]] || fail "usage: $0 contract|prepare"
case "$1" in
  contract)
    emit_contract
    exit 0
    ;;
  prepare) ;;
  *) fail "usage: $0 contract|prepare" ;;
esac

for command in awk cargo cp dirname find flock git id jq ldd mktemp mv readelf \
  readlink rg rustc rustup sed sha256sum sort stat; do
  command -v "$command" >/dev/null ||
    fail "required command is unavailable: $command"
done

readonly schema_version=1
readonly validation_policy_revision=2
readonly production_commit="a58fbce2ff48c58b7bb5001b1a27e64b9596ee3a"
readonly production_wallet_sha="28245d5fe1dc2a36a2ec80e9e865f10fa671b2ded8f08d82f4f07445cb9f96e6"
readonly production_rapidsnark_sha="d4133227f845ff5bfa3672eb5b9c018a6a086bfa164b176bdaf76949c7d1f423"
readonly production_gmp_sha="0a910b420c3ad603c83c9dc2818c7ae05394c231ca23135c7b873e8e680ea41b"
readonly production_fq_sha="797b5d24bb8e8b088f811bddfff35f33973af9c797fb3812489cd42ba6a957d0"
readonly production_fr_sha="40f809394904682cb5517845cd3c2f936a5eb4609712534b573f552f2811fb82"
readonly token_declaration='pub const TOKEN_ID: [u32; 8] = [2282739141, 348907455, 1046946228, 3735699860, 585462133, 3426087150, 772528164, 2090518099];'
readonly ata_declaration='pub const ASSOCIATED_TOKEN_ACCOUNT_ID: [u32; 8] = [3357312149, 3615960253, 3351583505, 2234166003, 4153433811, 2743238177, 2886052503, 4160755157];'
readonly build_recipe='cd PINNED_SOURCE && cargo +TOOLCHAIN build --manifest-path Cargo.toml --locked --offline -p wallet --target-dir PRIVATE_STAGING_TARGET'
helper_path="$(readlink -f "${BASH_SOURCE[0]}")" || fail "cache helper path resolution failed"
readonly helper_path
[[ -f "$helper_path" && ! -L "$helper_path" ]] || fail "cache helper is unsafe"
helper_sha="$(sha_file "$helper_path")" || fail "cache helper fingerprint failed"
readonly helper_sha
started_ms="$(awk '{printf "%.0f\n", $1 * 1000}' /proc/uptime)" ||
  fail "monotonic preparation clock is unavailable"
readonly started_ms

readonly test_mode="${M3_OFFICIAL_WALLET_CACHE_TEST_MODE:-0}"
[[ "$test_mode" == 0 || "$test_mode" == 1 ]] ||
  fail "cache test mode must be zero or one"
if [[ "$test_mode" == 1 ]]; then
  readonly expected_commit="${M3_OFFICIAL_WALLET_TEST_EXPECTED_COMMIT:-}"
  readonly expected_origin="${M3_OFFICIAL_WALLET_TEST_EXPECTED_ORIGIN:-}"
  readonly expected_wallet_sha="${M3_OFFICIAL_WALLET_TEST_WALLET_SHA256:-}"
  readonly expected_rapidsnark_sha="${M3_OFFICIAL_WALLET_TEST_LIBRAPIDSNARK_A_SHA256:-}"
  readonly expected_gmp_sha="${M3_OFFICIAL_WALLET_TEST_LIBGMP_A_SHA256:-}"
  readonly expected_fq_sha="${M3_OFFICIAL_WALLET_TEST_LIBFQ_A_SHA256:-}"
  readonly expected_fr_sha="${M3_OFFICIAL_WALLET_TEST_LIBFR_A_SHA256:-}"
else
  for variable in M3_OFFICIAL_WALLET_TEST_EXPECTED_COMMIT \
    M3_OFFICIAL_WALLET_TEST_EXPECTED_ORIGIN \
    M3_OFFICIAL_WALLET_TEST_WALLET_SHA256 \
    M3_OFFICIAL_WALLET_TEST_LIBRAPIDSNARK_A_SHA256 \
    M3_OFFICIAL_WALLET_TEST_LIBGMP_A_SHA256 \
    M3_OFFICIAL_WALLET_TEST_LIBFQ_A_SHA256 \
    M3_OFFICIAL_WALLET_TEST_LIBFR_A_SHA256; do
    [[ ! -v "$variable" ]] ||
      fail "test-only override is forbidden outside contract tests: $variable"
  done
  readonly expected_commit="$production_commit"
  readonly expected_origin="https://github.com/logos-blockchain/logos-execution-zone.git"
  readonly expected_wallet_sha="$production_wallet_sha"
  readonly expected_rapidsnark_sha="$production_rapidsnark_sha"
  readonly expected_gmp_sha="$production_gmp_sha"
  readonly expected_fq_sha="$production_fq_sha"
  readonly expected_fr_sha="$production_fr_sha"
fi
for expected in "$expected_commit" "$expected_rapidsnark_sha" \
  "$expected_gmp_sha" "$expected_fq_sha" "$expected_fr_sha"; do
  [[ "$expected" =~ ^[0-9a-f]{40,64}$ ]] ||
    fail "expected source or native-library identity is invalid"
done
[[ "$expected_wallet_sha" =~ ^[0-9a-f]{64}$ ]] ||
  fail "expected official-wallet identity is invalid"
[[ "$expected_origin" =~ ^https://github.com/[A-Za-z0-9._/-]+\.git$ ]] ||
  fail "expected source origin is invalid"

readonly cache_root="${M3_OFFICIAL_WALLET_CACHE_ROOT:-/tmp/lez-atomic-swaps-cache-$(id -u)/m3-official-wallet-v1}"
readonly destination="${M3_OFFICIAL_WALLET_DESTINATION:-}"
readonly source_dir="${LEZ_V02_SOURCE_DIR:-}"
readonly toolchain="${M3_RUST_TOOLCHAIN:-1.96.0}"
readonly native_dir="${RAPIDSNARK_LIB_DIR:-}"
readonly bindgen_args="${BINDGEN_EXTRA_CLANG_ARGS:-}"

[[ "$cache_root" == /* && "$destination" == /* && "$source_dir" == /* &&
   "$native_dir" == /* ]] || fail "cache, destination, source, and native paths must be absolute"
[[ "$toolchain" =~ ^[A-Za-z0-9._-]{1,64}$ ]] ||
  fail "Rust toolchain identifier is invalid"
[[ ! -e "$destination" && ! -L "$destination" ]] ||
  fail "refusing to overwrite official-wallet destination"

for variable in RUSTFLAGS CARGO_ENCODED_RUSTFLAGS CARGO_TARGET_DIR \
  CARGO_BUILD_TARGET RUSTC RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER \
  CARGO_HOME CARGO_INCREMENTAL CC CXX AR LD CFLAGS CXXFLAGS CPPFLAGS LDFLAGS \
  CPATH C_INCLUDE_PATH CPLUS_INCLUDE_PATH LIBRARY_PATH LD_LIBRARY_PATH \
  LIBCLANG_PATH CLANG_PATH LLVM_CONFIG_PATH PKG_CONFIG PKG_CONFIG_PATH \
  PKG_CONFIG_LIBDIR PKG_CONFIG_SYSROOT_DIR CMAKE CMAKE_PREFIX_PATH MAKE \
  MAKEFLAGS NUM_JOBS CARGO_PROFILE_DEV_DEBUG \
  PYO3_CONFIG_FILE PYO3_CROSS PYO3_PYTHON PYTHON_SYS_EXECUTABLE VIRTUAL_ENV \
  SOURCE_DATE_EPOCH; do
  [[ ! -v "$variable" ]] ||
    fail "unsupported build-influencing environment override: $variable"
done
mapfile -t environment_names < <(compgen -e)
for variable in "${environment_names[@]}"; do
  case "$variable" in
    CARGO_PROFILE_*|CARGO_TARGET_*|PYO3_*|CC_*|CXX_*|AR_*|CFLAGS_*|\
      CXXFLAGS_*|CPPFLAGS_*|LDFLAGS_*|TARGET_CC|TARGET_CXX|TARGET_AR|\
      TARGET_CFLAGS|TARGET_CXXFLAGS|TARGET_CPPFLAGS|TARGET_LDFLAGS|\
      HOST_CC|HOST_CXX|HOST_AR|HOST_CFLAGS|HOST_CXXFLAGS|HOST_CPPFLAGS|\
      HOST_LDFLAGS)
      fail "unsupported build-influencing environment override: $variable"
      ;;
  esac
done

[[ -d "$source_dir/.git" && ! -L "$source_dir" &&
   "$(readlink -f "$source_dir")" == "$source_dir" ]] ||
  fail "LEZ source must be one canonical Git checkout"
[[ -d "$native_dir" && ! -L "$native_dir" &&
   "$(readlink -f "$native_dir")" == "$native_dir" ]] ||
  fail "native library directory must be canonical and non-symlink"
[[ "$bindgen_args" =~ ^-I/[^[:space:]]+$ ]] ||
  fail "BINDGEN_EXTRA_CLANG_ARGS must be one canonical absolute include directory"
readonly include_dir="${bindgen_args#-I}"
[[ -d "$include_dir" && ! -L "$include_dir" &&
   "$(readlink -f "$include_dir")" == "$include_dir" ]] ||
  fail "bindgen include directory must be canonical and non-symlink"
include_symlink="$(find "$include_dir" -type l -print -quit)" ||
  fail "bindgen include-tree symlink scan failed"
[[ -z "$include_symlink" ]] ||
  fail "bindgen include tree contains a symlink"

mkdir -p "$cache_root"
[[ ! -L "$cache_root" && "$(readlink -f "$cache_root")" == "$cache_root" ]] ||
  fail "cache root must be canonical and non-symlink"
chmod 0700 "$cache_root"
require_owner_mode "$cache_root" dir 700
for directory in refs locks objects objects/sha256; do
  mkdir -p "$cache_root/$directory"
  chmod 0700 "$cache_root/$directory"
  require_owner_mode "$cache_root/$directory" dir 700
done

declare -a temporary_paths=()
cleanup() {
  local path
  for path in "${temporary_paths[@]:-}"; do
    [[ -n "$path" ]] || continue
    chmod -R u+rwX "$path" 2>/dev/null || true
    rm -rf -- "$path"
  done
}
trap cleanup EXIT
work_root="$(mktemp -d "$cache_root/.prepare.XXXXXX")"
readonly work_root
chmod 0700 "$work_root"
temporary_paths+=("$work_root")

hash_tree() {
  local root="$1" listing unsorted file relative file_sha
  listing="$(mktemp "$work_root/tree.XXXXXX")"
  unsorted="$(mktemp "$work_root/tree-unsorted.XXXXXX")"
  temporary_paths+=("$listing" "$unsorted")
  find "$root" -type f -print0 >"$unsorted" ||
    fail "hashed-tree file discovery failed"
  sort -z "$unsorted" >"$listing.files" || fail "hashed-tree sorting failed"
  temporary_paths+=("$listing.files")
  : >"$listing"
  while IFS= read -r -d '' file; do
    [[ -f "$file" && ! -L "$file" ]] || fail "unsafe file in hashed tree"
    relative="${file#"$root"/}"
    [[ "$relative" != *$'\n'* && "$relative" != *$'\r'* ]] ||
      fail "newline-bearing path is unsupported in hashed tree"
    file_sha="$(sha_file "$file")" || fail "hashed-tree file fingerprint failed"
    printf '%s  %s\n' "$file_sha" "$relative" >>"$listing"
  done <"$listing.files"
  [[ -s "$listing" ]] || fail "hashed tree contains no regular files"
  sha_file "$listing"
}

native_library_json() {
  local name expected file actual record_json
  local -a records=()
  for record in \
    "librapidsnark.a:$expected_rapidsnark_sha" \
    "libgmp.a:$expected_gmp_sha" \
    "libfq.a:$expected_fq_sha" \
    "libfr.a:$expected_fr_sha"; do
    name="${record%%:*}"
    expected="${record#*:}"
    file="$native_dir/$name"
    [[ -f "$file" && ! -L "$file" ]] ||
      fail "verified native library is missing or unsafe: $name"
    actual="$(sha_file "$file")"
    [[ "$actual" == "$expected" ]] ||
      fail "verified native-library hash mismatch: $name"
    record_json="$(jq -nc --arg name "$name" --arg sha "$actual" \
      '{name:$name,sha256:$sha}')" || fail "native-library manifest creation failed"
    records+=("$record_json")
  done
  printf '%s\n' "${records[@]}" | jq -cs 'sort_by(.name)'
}

program_artifact_fingerprint() {
  local listing files unsorted file relative file_sha count=0
  listing="$(mktemp "$work_root/programs.XXXXXX")"
  files="$(mktemp "$work_root/program-files.XXXXXX")"
  unsorted="$(mktemp "$work_root/program-files-unsorted.XXXXXX")"
  temporary_paths+=("$listing" "$files" "$unsorted")
  find "$source_dir/artifacts/lez/programs" -type f -name '*.bin' -print0 \
    >"$unsorted" || fail "program-artifact discovery failed"
  sort -z "$unsorted" >"$files" || fail "program-artifact sorting failed"
  while IFS= read -r -d '' file; do
    [[ -f "$file" && ! -L "$file" ]] || fail "unsafe program artifact"
    relative="${file#"$source_dir"/}"
    [[ "$relative" != *$'\n'* && "$relative" != *$'\r'* ]] ||
      fail "newline-bearing program artifact path is unsupported"
    file_sha="$(sha_file "$file")" || fail "program-artifact fingerprint failed"
    printf '%s  %s\n' "$file_sha" "$relative" >>"$listing"
    count=$((count + 1))
  done <"$files"
  (( count >= 2 )) || fail "expected at least the official Token and ATA program artifacts"
  sha_file "$listing"
}

tool_fingerprint() {
  local listing libraries unsorted command path canonical library file_sha count=0
  local -a library_roots=()
  listing="$(mktemp "$work_root/tools.XXXXXX")"
  temporary_paths+=("$listing")
  if [[ "$test_mode" == 1 ]]; then
    printf 'contract-test-tool-surface\n' >"$listing"
  else
    for command in cc c++ ar ld python3; do
      path="$(command -v "$command" 2>/dev/null || true)"
      [[ -n "$path" ]] || fail "build tool is unavailable: $command"
      canonical="$(readlink -f "$path")"
      [[ -f "$canonical" && ! -L "$canonical" ]] ||
        fail "build tool is unsafe: $command"
      file_sha="$(sha_file "$canonical")" || fail "build-tool fingerprint failed: $command"
      printf '%s  %s  %s\n' "$command" "$canonical" "$file_sha" \
        >>"$listing"
    done
    python3 --version >>"$listing" 2>&1
    if command -v clang >/dev/null; then
      path="$(readlink -f "$(command -v clang)")"
      file_sha="$(sha_file "$path")" || fail "clang fingerprint failed"
      printf '%s  %s  %s\n' clang "$path" "$file_sha" >>"$listing"
      clang --version >>"$listing" 2>&1
    else
      printf 'clang_cli  absent\n' >>"$listing"
    fi
    libraries="$(mktemp "$work_root/libclang-files.XXXXXX")"
    unsorted="$(mktemp "$work_root/libclang-files-unsorted.XXXXXX")"
    temporary_paths+=("$libraries" "$unsorted")
    [[ ! -d /usr/lib ]] || library_roots+=(/usr/lib)
    [[ ! -d /usr/lib64 ]] || library_roots+=(/usr/lib64)
    (( ${#library_roots[@]} >= 1 )) || fail "no system library root is available"
    find "${library_roots[@]}" -type f -name 'libclang*.so*' -print >"$unsorted" ||
      fail "libclang runtime discovery failed"
    sort -u "$unsorted" >"$libraries" || fail "libclang runtime sorting failed"
    while IFS= read -r library; do
      [[ -n "$library" ]] || continue
      canonical="$(readlink -f "$library")"
      [[ -f "$canonical" && ! -L "$canonical" ]] ||
        fail "libclang runtime is unsafe: $library"
      file_sha="$(sha_file "$canonical")" || fail "libclang runtime fingerprint failed"
      printf '%s  %s\n' "$file_sha" "$canonical" >>"$listing"
      count=$((count + 1))
    done <"$libraries"
    (( count >= 1 )) || fail "no libclang runtime was found for bindgen"
  fi
  sha_file "$listing"
}

cargo_config_fingerprint() {
  local listing config current parent config_sha
  local -A seen=()
  listing="$(mktemp "$work_root/cargo-config.XXXXXX")"
  temporary_paths+=("$listing")
  current="$source_dir"
  while :; do
    for config in "$current/.cargo/config.toml" "$current/.cargo/config"; do
      [[ -z "${seen[$config]:-}" ]] || continue
      seen["$config"]=1
      if [[ -e "$config" || -L "$config" ]]; then
        [[ -f "$config" && ! -L "$config" ]] ||
          fail "Cargo configuration is unsafe: $config"
        config_sha="$(sha_file "$config")" || fail "Cargo configuration fingerprint failed"
        printf '%s  %s\n' "$config_sha" "$(readlink -f "$config")" \
          >>"$listing"
      fi
    done
    [[ "$current" != / ]] || break
    parent="$(dirname "$current")"
    [[ "$parent" != "$current" ]] || break
    current="$parent"
  done
  for config in "$HOME/.cargo/config.toml" "$HOME/.cargo/config"; do
    [[ -z "${seen[$config]:-}" ]] || continue
    seen["$config"]=1
    if [[ -e "$config" || -L "$config" ]]; then
      [[ -f "$config" && ! -L "$config" ]] ||
        fail "Cargo configuration is unsafe: $config"
      config_sha="$(sha_file "$config")" || fail "Cargo configuration fingerprint failed"
      printf '%s  %s\n' "$config_sha" "$(readlink -f "$config")" \
        >>"$listing"
    fi
  done
  [[ -s "$listing" ]] || printf 'none\n' >"$listing"
  sha_file "$listing"
}

compute_input_manifest() {
  local source_archive_sha lockfile_sha include_sha programs_sha native_json
  local rustc_path cargo_path rustc_sha cargo_sha rustc_version cargo_version
  local target_libdir target target_libdir_sha cargo_metadata_sha tools_sha cargo_config_sha
  local metadata_file source_origin target_symlink
  local source_status
  source_status="$(git -C "$source_dir" status --porcelain --untracked-files=all \
    --ignored=matching)" ||
    fail "LEZ source status query failed"
  [[ -z "$source_status" ]] ||
    fail "LEZ source checkout is dirty"
  [[ "$(git -C "$source_dir" rev-parse HEAD)" == "$expected_commit" ]] ||
    fail "LEZ source checkout is not the expected commit"
  [[ "$(git -C "$source_dir" rev-parse 'refs/tags/v0.2.0^{}')" == \
     "$expected_commit" ]] || fail "LEZ v0.2.0 tag does not match the expected commit"
  source_origin="$(git -C "$source_dir" remote get-url origin)" ||
    fail "LEZ source origin query failed"
  [[ "$source_origin" == "$expected_origin" ]] || fail "LEZ source origin is unexpected"
  [[ -f "$source_dir/Cargo.lock" && ! -L "$source_dir/Cargo.lock" ]] ||
    fail "source Cargo.lock is missing or unsafe"

  source_archive_sha="$(git -C "$source_dir" archive --format=tar HEAD |
    sha256sum | sed 's/ .*//')" ||
    fail "tracked source archive fingerprint failed"
  lockfile_sha="$(sha_file "$source_dir/Cargo.lock")" ||
    fail "Cargo.lock fingerprint failed"
  include_sha="$(hash_tree "$include_dir")" ||
    fail "bindgen include-tree fingerprint failed"
  programs_sha="$(program_artifact_fingerprint)" ||
    fail "program-artifact fingerprint failed"
  native_json="$(native_library_json)" ||
    fail "native-library fingerprint failed"

  rustc_path="$(rustup which --toolchain "$toolchain" rustc)" ||
    fail "Rust compiler resolution failed"
  cargo_path="$(rustup which --toolchain "$toolchain" cargo)" ||
    fail "Cargo resolution failed"
  rustc_path="$(readlink -f "$rustc_path")"
  cargo_path="$(readlink -f "$cargo_path")"
  [[ -f "$rustc_path" && -f "$cargo_path" ]] ||
    fail "resolved Rust toolchain binaries are unavailable"
  rustc_sha="$(sha_file "$rustc_path")" ||
    fail "Rust compiler fingerprint failed"
  cargo_sha="$(sha_file "$cargo_path")" || fail "Cargo fingerprint failed"
  rustc_version="$(rustc +"$toolchain" -vV)" ||
    fail "Rust compiler version query failed"
  cargo_version="$(cargo +"$toolchain" -V)" ||
    fail "Cargo version query failed"
  target="$(sed -n 's/^host: //p' <<<"$rustc_version")"
  [[ -n "$target" ]] || fail "Rust toolchain did not report its host target"
  target_libdir="$(rustc +"$toolchain" --print target-libdir)" ||
    fail "Rust target library query failed"
  if [[ "$test_mode" == 0 ]]; then
    [[ -d "$target_libdir" ]] || fail "Rust target library directory is unavailable"
  fi
  target_symlink="$(find "$target_libdir" -type l -print -quit)" ||
    fail "Rust target library symlink scan failed"
  [[ -z "$target_symlink" ]] || fail "Rust target library tree contains a symlink"
  target_libdir_sha="$(hash_tree "$target_libdir")" ||
    fail "Rust target library-tree fingerprint failed"

  metadata_file="$(mktemp "$work_root/metadata.XXXXXX")"
  temporary_paths+=("$metadata_file")
  (
    cd "$source_dir"
    cargo +"$toolchain" metadata --locked --offline --no-deps \
      --manifest-path Cargo.toml --format-version 1
  ) | jq -cS . >"$metadata_file" ||
    fail "offline Cargo metadata fingerprint failed"
  cargo_metadata_sha="$(sha_file "$metadata_file")" ||
    fail "Cargo metadata hash failed"
  tools_sha="$(tool_fingerprint)" || fail "build-tool fingerprint failed"
  cargo_config_sha="$(cargo_config_fingerprint)" ||
    fail "Cargo configuration fingerprint failed"

  jq -ncS \
    --argjson schema "$schema_version" \
    --arg source_origin "$source_origin" \
    --arg source_dir "$source_dir" \
    --arg source_commit "$expected_commit" \
    --arg source_tag "v0.2.0" \
    --arg source_archive_sha256 "$source_archive_sha" \
    --arg cargo_lock_sha256 "$lockfile_sha" \
    --arg cargo_metadata_sha256 "$cargo_metadata_sha" \
    --arg program_artifacts_sha256 "$programs_sha" \
    --arg toolchain "$toolchain" \
    --arg rustc_sha256 "$rustc_sha" \
    --arg cargo_sha256 "$cargo_sha" \
    --arg rustc_version "$rustc_version" \
    --arg cargo_version "$cargo_version" \
    --arg target "$target" \
    --arg target_libdir "$target_libdir" \
    --arg target_libdir_sha256 "$target_libdir_sha" \
    --argjson validation_policy_revision "$validation_policy_revision" \
    --arg helper_sha256 "$helper_sha" \
    --arg recipe "$build_recipe" \
    --arg expected_wallet_sha256 "$expected_wallet_sha" \
    --arg build_environment "unapproved_overrides_rejected" \
    --arg bindgen_args "$bindgen_args" \
    --arg bindgen_include_sha256 "$include_sha" \
    --arg build_tools_sha256 "$tools_sha" \
    --arg cargo_config_sha256 "$cargo_config_sha" \
    --argjson native_libraries "$native_json" \
    '{
      schema_version:$schema,
      kind:"m3_official_wallet_build_inputs",
      validation_policy_revision:$validation_policy_revision,
      publisher_helper_sha256:$helper_sha256,
      source:{
        origin:$source_origin,
        canonical_path:$source_dir,
        commit:$source_commit,
        tag:$source_tag,
        tracked_archive_sha256:$source_archive_sha256,
        cargo_lock_sha256:$cargo_lock_sha256,
        cargo_metadata_sha256:$cargo_metadata_sha256,
        program_artifacts_sha256:$program_artifacts_sha256},
      toolchain:{
        name:$toolchain,
        target:$target,
        target_libdir:$target_libdir,
        target_libdir_sha256:$target_libdir_sha256,
        rustc_sha256:$rustc_sha256,
        cargo_sha256:$cargo_sha256,
        rustc_version:$rustc_version,
        cargo_version:$cargo_version},
      build:{
        recipe:$recipe,
        profile:"dev",
        expected_wallet_sha256:$expected_wallet_sha256,
        wallet_features:[],
        environment_policy:$build_environment,
        build_tools_sha256:$build_tools_sha256,
        cargo_config_sha256:$cargo_config_sha256},
      bindgen:{
        args:$bindgen_args,
        include_tree_sha256:$bindgen_include_sha256},
      native_libraries:$native_libraries
    }' || fail "canonical build-input manifest creation failed"
}

runtime_fingerprint() {
  local wallet="$1" listing ldd_output libraries library canonical file_sha
  listing="$(mktemp "$work_root/runtime.XXXXXX")"
  temporary_paths+=("$listing")
  if [[ "$test_mode" == 1 ]]; then
    printf 'contract-test-runtime-v1\n' >"$listing"
  else
    readelf --file-header --program-headers --dynamic "$wallet" |
      sed -E 's/0x[0-9a-fA-F]+/HEX/g' >"$listing" ||
      fail "official-wallet ELF inspection failed"
    ldd_output="$(ldd "$wallet" 2>&1)" || fail "official-wallet runtime linking failed"
    [[ "$ldd_output" != *"not found"* ]] ||
      fail "official-wallet runtime dependency is unresolved"
    sed -E 's/0x[0-9a-fA-F]+/HEX/g' <<<"$ldd_output" >>"$listing"
    libraries="$(mktemp "$work_root/runtime-libraries.XXXXXX")"
    temporary_paths+=("$libraries")
    sed -nE \
      -e 's/.*=> (\/[^ ]+).*/\1/p' \
      -e 's/^[[:space:]]*(\/[^ ]+).*/\1/p' <<<"$ldd_output" |
      sort -u >"$libraries" || fail "runtime-library discovery failed"
    while IFS= read -r library; do
      [[ -n "$library" ]] || continue
      canonical="$(readlink -f "$library")"
      [[ -f "$canonical" && ! -L "$canonical" ]] ||
        fail "runtime dependency is missing or unsafe: $library"
      file_sha="$(sha_file "$canonical")" || fail "runtime-library fingerprint failed"
      printf '%s  %s\n' "$file_sha" "$canonical" >>"$listing"
    done <"$libraries"
  fi
  sha_file "$listing"
}

validate_object() {
  local wallet_sha="$1" expected_manifest_sha="$2"
  local object_dir="$cache_root/objects/sha256/$wallet_sha"
  local wallet="$object_dir/wallet" manifest="$object_dir/manifest.json"
  local runtime names_file wallet_actual manifest_actual
  local -a names=()
  [[ "$wallet_sha" =~ ^[0-9a-f]{64}$ ]] || fail "cache reference has invalid wallet hash"
  [[ "$wallet_sha" == "$expected_wallet_sha" ]] ||
    fail "cache reference does not name the expected official wallet"
  require_owner_mode "$object_dir" dir 700
  names_file="$(mktemp "$work_root/object-names.XXXXXX")"
  temporary_paths+=("$names_file")
  find "$object_dir" -mindepth 1 -maxdepth 1 -printf '%f\n' |
    sort >"$names_file" || fail "cache-object allowlist scan failed"
  mapfile -t names <"$names_file"
  [[ "${names[*]}" == "manifest.json wallet" ]] ||
    fail "cache object contains a non-allowlisted entry"
  require_owner_mode "$wallet" file 500
  require_owner_mode "$manifest" file 600
  wallet_actual="$(sha_file "$wallet")" || fail "cached wallet fingerprint failed"
  [[ "$wallet_actual" == "$wallet_sha" ]] ||
    fail "cached wallet hash mismatch"
  manifest_actual="$(sha_file "$manifest")" || fail "cache manifest fingerprint failed"
  [[ -z "$expected_manifest_sha" || "$manifest_actual" == "$expected_manifest_sha" ]] ||
    fail "cache object manifest hash mismatch"
  runtime="$(runtime_fingerprint "$wallet")" ||
    fail "cached wallet runtime fingerprint failed"
  jq -e --arg wallet_sha "$wallet_sha" \
    --arg runtime "$runtime" '
      .schema_version == 1
      and .kind == "m3_official_wallet_cache_object"
      and .wallet_sha256 == $wallet_sha
      and .runtime_fingerprint_sha256 == $runtime
      and .allowlisted_files == ["manifest.json","wallet"]
    ' "$manifest" >/dev/null || fail "cache object manifest is invalid"
}

input_json="$(compute_input_manifest)" ||
  fail "official-wallet build-input fingerprint failed"
readonly input_json
input_key="$(sha256sum <<<"$input_json" | sed 's/ .*//')"
readonly input_key
readonly lock_path="$cache_root/locks/$input_key.lock"
exec {input_lock_fd}>"$lock_path"
chmod 0600 "$lock_path"
require_owner_mode "$lock_path" file 600
flock -x "$input_lock_fd"

readonly ref_path="$cache_root/refs/$input_key.json"
cache_hit=false
wallet_sha=""
object_manifest_sha=""

if [[ -e "$ref_path" || -L "$ref_path" ]]; then
  require_owner_mode "$ref_path" file 600
  jq -e --arg key "$input_key" --argjson input "$input_json" '
    .schema_version == 1
    and .kind == "m3_official_wallet_cache_ref"
    and .input_key == $key
    and .input == $input
    and .validation_policy_revision == $input.validation_policy_revision
    and .publisher_helper_sha256 == $input.publisher_helper_sha256
    and (.wallet_sha256 | strings | test("^[0-9a-f]{64}$"))
    and (.object_manifest_sha256 | strings | test("^[0-9a-f]{64}$"))
  ' "$ref_path" >/dev/null || fail "published cache reference is invalid"
  wallet_sha="$(jq -er '.wallet_sha256' "$ref_path")"
  object_manifest_sha="$(jq -er '.object_manifest_sha256' "$ref_path")"
  validate_object "$wallet_sha" "$object_manifest_sha"
  cache_hit=true
else
  build_root="$(mktemp -d "$cache_root/.build-$input_key.XXXXXX")"
  temporary_paths+=("$build_root")
  chmod 0700 "$build_root"
  build_target="$build_root/target"
  (
    cd "$source_dir"
    cargo +"$toolchain" build --manifest-path Cargo.toml \
      --locked --offline -p wallet --target-dir "$build_target"
  ) ||
    fail "offline official-wallet build failed"
  built_wallet="$build_target/debug/wallet"
  [[ -x "$built_wallet" && -f "$built_wallet" && ! -L "$built_wallet" ]] ||
    fail "official-wallet build did not produce one safe executable"

  registry_list="$(mktemp "$work_root/program-registries.XXXXXX")"
  temporary_paths+=("$registry_list")
  find "$build_target/debug/build" -path '*/out/lez/programs/mod.rs' \
    -type f -print >"$registry_list" || fail "generated-program registry discovery failed"
  mapfile -t registries <"$registry_list"
  (( ${#registries[@]} >= 1 )) ||
    fail "official-wallet build did not retain its generated program registry"
  for registry in "${registries[@]}"; do
    [[ ! -L "$registry" ]] || fail "official program registry became a symlink"
    rg -Fqx "$token_declaration" "$registry" ||
      fail "official Token program ID differs from the verified v0.2 value"
    rg -Fqx "$ata_declaration" "$registry" ||
      fail "official ATA program ID differs from the verified v0.2 value"
  done

  post_build_input="$(compute_input_manifest)" ||
    fail "post-build input fingerprint failed"
  [[ "$post_build_input" == "$input_json" ]] ||
    fail "official-wallet build inputs changed during the build"
  wallet_sha="$(sha_file "$built_wallet")" || fail "built wallet fingerprint failed"
  [[ "$wallet_sha" == "$expected_wallet_sha" ]] ||
    fail "built official-wallet hash differs from the pinned identity"
  object_stage="$(mktemp -d "$cache_root/objects/sha256/.$wallet_sha.XXXXXX")"
  temporary_paths+=("$object_stage")
  chmod 0700 "$object_stage"
  cp --reflink=auto -- "$built_wallet" "$object_stage/wallet"
  chmod 0500 "$object_stage/wallet"
  staged_wallet_sha="$(sha_file "$object_stage/wallet")" ||
    fail "staged cache wallet fingerprint failed"
  [[ "$staged_wallet_sha" == "$wallet_sha" ]] ||
    fail "staged cache wallet hash mismatch"
  runtime_sha="$(runtime_fingerprint "$object_stage/wallet")" ||
    fail "built wallet runtime fingerprint failed"
  jq -ncS --arg wallet_sha "$wallet_sha" --arg runtime "$runtime_sha" '{
      schema_version:1,
      kind:"m3_official_wallet_cache_object",
      wallet_sha256:$wallet_sha,
      runtime_fingerprint_sha256:$runtime,
      allowlisted_files:["manifest.json","wallet"]
    }' >"$object_stage/manifest.json"
  chmod 0600 "$object_stage/manifest.json"
  object_manifest_sha="$(sha_file "$object_stage/manifest.json")" ||
    fail "cache object-manifest fingerprint failed"

  object_lock="$cache_root/locks/object-$wallet_sha.lock"
  exec {object_lock_fd}>"$object_lock"
  chmod 0600 "$object_lock"
  require_owner_mode "$object_lock" file 600
  flock -x "$object_lock_fd"
  object_dir="$cache_root/objects/sha256/$wallet_sha"
  if [[ -e "$object_dir" || -L "$object_dir" ]]; then
    validate_object "$wallet_sha" "$object_manifest_sha"
  else
    mv -Tn -- "$object_stage" "$object_dir"
    [[ ! -e "$object_stage" ]] ||
      fail "atomic cache-object publication lost a race"
    validate_object "$wallet_sha" "$object_manifest_sha"
  fi
  flock -u "$object_lock_fd"

  ref_stage="$(mktemp "$cache_root/refs/.$input_key.XXXXXX")"
  temporary_paths+=("$ref_stage")
  jq -ncS --arg key "$input_key" --argjson input "$input_json" \
    --arg wallet_sha "$wallet_sha" --arg manifest_sha "$object_manifest_sha" \
    --argjson policy "$validation_policy_revision" --arg helper_sha "$helper_sha" '{
      schema_version:1,
      kind:"m3_official_wallet_cache_ref",
      input_key:$key,
      input:$input,
      validation_policy_revision:$policy,
      publisher_helper_sha256:$helper_sha,
      wallet_sha256:$wallet_sha,
      object_manifest_sha256:$manifest_sha
    }' >"$ref_stage"
  chmod 0600 "$ref_stage"
  current_helper_sha="$(sha_file "$helper_path")" || fail "cache helper recheck failed"
  [[ "$current_helper_sha" == "$helper_sha" ]] ||
    fail "cache helper changed before reference publication"
  mv -Tn -- "$ref_stage" "$ref_path"
  [[ ! -e "$ref_stage" ]] ||
    fail "atomic cache-reference publication lost a race"
  require_owner_mode "$ref_path" file 600
fi

readonly cached_wallet="$cache_root/objects/sha256/$wallet_sha/wallet"
validate_object "$wallet_sha" "$object_manifest_sha"

destination_parent="$(dirname "$destination")"
mkdir -p "$destination_parent"
chmod 0700 "$destination_parent"
[[ ! -L "$destination_parent" &&
   "$(readlink -f "$destination_parent")" == "$destination_parent" ]] ||
  fail "official-wallet destination parent is unsafe"
require_owner_mode "$destination_parent" dir 700
[[ ! -e "$destination" && ! -L "$destination" ]] ||
  fail "refusing to overwrite official-wallet destination"

private_stage="$(mktemp "$destination_parent/.wallet.XXXXXX")"
temporary_paths+=("$private_stage")
source_hash_before="$(sha_file "$cached_wallet")" || fail "cache source fingerprint failed"
cp --reflink=auto -- "$cached_wallet" "$private_stage"
chmod 0500 "$private_stage"
source_hash_after="$(sha_file "$cached_wallet")" || fail "cache source recheck failed"
private_hash="$(sha_file "$private_stage")" || fail "private wallet fingerprint failed"
[[ "$source_hash_before" == "$wallet_sha" &&
   "$source_hash_after" == "$wallet_sha" &&
   "$private_hash" == "$wallet_sha" ]] ||
  fail "wallet cache copy changed during verified consumption"
[[ "$(stat -c '%d:%i' "$cached_wallet")" != \
   "$(stat -c '%d:%i' "$private_stage")" ]] ||
  fail "private official wallet must not hardlink the cache object"
final_input_json="$(compute_input_manifest)" ||
  fail "post-copy input fingerprint failed"
[[ "$final_input_json" == "$input_json" ]] ||
  fail "official-wallet build inputs changed during cache consumption"
current_helper_sha="$(sha_file "$helper_path")" || fail "cache helper final recheck failed"
[[ "$current_helper_sha" == "$helper_sha" ]] ||
  fail "cache helper changed during artifact preparation"
mv -Tn -- "$private_stage" "$destination"
[[ ! -e "$private_stage" ]] ||
  fail "official-wallet destination appeared during atomic publication"
require_owner_mode "$destination" file 500
destination_hash="$(sha_file "$destination")" || fail "published wallet fingerprint failed"
[[ "$destination_hash" == "$wallet_sha" ]] ||
  fail "private official-wallet hash changed after publication"

runtime_sha="$(jq -er '.runtime_fingerprint_sha256' \
  "$cache_root/objects/sha256/$wallet_sha/manifest.json")" ||
  fail "published runtime fingerprint is unavailable"
completed_ms="$(awk '{printf "%.0f\n", $1 * 1000}' /proc/uptime)" ||
  fail "monotonic completion clock is unavailable"
(( completed_ms >= started_ms )) || fail "monotonic preparation clock regressed"
duration_ms=$((completed_ms - started_ms))
artifact_bytes="$(stat -c %s "$destination")" || fail "artifact-size query failed"
jq -ncS --arg key "$input_key" --arg sha "$wallet_sha" \
  --argjson hit "$cache_hit" --argjson test_mode "$test_mode" \
  --argjson input "$input_json" --arg manifest_sha "$object_manifest_sha" \
  --arg runtime_sha "$runtime_sha" --arg helper_sha "$helper_sha" \
  --argjson policy "$validation_policy_revision" --argjson duration_ms "$duration_ms" \
  --argjson artifact_bytes "$artifact_bytes" '{
    schema_version:1,
    kind:"m3_official_wallet_artifact_preparation",
    result:"prepared",
    cache_hit:$hit,
    test_mode:($test_mode == 1),
    input_key:$key,
    input:$input,
    validation_policy_revision:$policy,
    publisher_helper_sha256:$helper_sha,
    wallet_sha256:$sha,
    object_manifest_sha256:$manifest_sha,
    runtime_fingerprint_sha256:$runtime_sha,
    duration_ms:$duration_ms,
    artifact_bytes:$artifact_bytes,
    private_copy:true,
    hardlink:false,
    source_rehashed_after_copy:true,
    destination_rehashed:true,
    secrets_or_state_cached:false
  }'
