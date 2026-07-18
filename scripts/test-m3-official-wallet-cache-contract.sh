#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly helper="scripts/prepare-m3-official-wallet-artifact.sh"

fail() {
  echo "M3 official-wallet cache contract failed: $*" >&2
  exit 1
}

[[ -x "$helper" && -f "$helper" && ! -L "$helper" ]] ||
  fail "cache helper is missing or unsafe"

contract="$("$helper" contract)"
jq -e '
  .schema_version == 1
  and .kind == "m3_official_wallet_artifact_cache_contract"
  and .cache.scope == "immutable_official_wallet_executable_only"
  and .cache.publication == "per_input_lock_and_atomic_ref"
  and .cache.same_uid_is_trusted == true
  and .cache.production_test_mode_forbidden == true
  and .cache.missing_ref_is_miss == true
  and .cache.invalid_ref_or_object_is_fatal == true
  and .input_key.binds_source_archive == true
  and .input_key.binds_lockfile == true
  and .input_key.binds_toolchain_and_target == true
  and .input_key.binds_build_recipe_and_environment == true
  and .input_key.binds_native_libraries == true
  and .input_key.binds_bindgen_include_tree == true
  and .input_key.binds_program_artifacts == true
  and .input_key.binds_cargo_metadata == true
  and .input_key.binds_target_library_tree == true
  and .input_key.binds_validation_policy_and_helper == true
  and .input_key.binds_expected_output_identity == true
  and .object.allowlisted_files == ["manifest.json","wallet"]
  and .object.wallet_mode == "0500"
  and .object.manifest_mode == "0600"
  and .consumption.copy_or_reflink_not_hardlink == true
  and .consumption.hash_source_before_and_after_copy == true
  and .consumption.hash_private_destination == true
  and .consumption.execute_only_private_copy == true
  and .evidence.canonical_input_manifest == true
  and .evidence.object_and_runtime_identity == true
  and .evidence.monotonic_duration_and_artifact_size == true
  and .secrets_or_state_cached == false
' <<<"$contract" >/dev/null || fail "cache contract is incomplete"

root="$(mktemp -d "${TMPDIR:-/tmp}/m3-wallet-cache-contract.XXXXXX")"
trap 'chmod -R u+rwX "$root" 2>/dev/null || true; rm -rf "$root"' EXIT
chmod 0700 "$root"

if env M3_OFFICIAL_WALLET_CACHE_TEST_MODE=0 \
    M3_OFFICIAL_WALLET_TEST_EXPECTED_COMMIT=forbidden \
    "$helper" prepare >"$root/production-test-override.out" 2>&1; then
  fail "production mode accepted a cache test override"
fi

fake_bin="$root/bin"
source_dir="$root/source"
include_dir="$root/include"
native_dir="$root/native"
target_libdir="$root/target-libdir"
mkdir -m 0700 "$fake_bin" "$source_dir" "$include_dir" "$native_dir" \
  "$target_libdir"
printf 'fixture target library\n' >"$target_libdir/libfixture.rlib"

printf 'fixture include v1\n' >"$include_dir/fixture.h"
for library in librapidsnark.a libgmp.a libfq.a libfr.a; do
  printf 'fixture %s\n' "$library" >"$native_dir/$library"
done

cat >"$source_dir/Cargo.toml" <<'EOF'
[workspace]
members = ["lez/wallet"]
resolver = "2"
EOF
cat >"$source_dir/Cargo.lock" <<'EOF'
version = 4
EOF
mkdir -p "$source_dir/lez/wallet" \
  "$source_dir/artifacts/lez/programs"
cat >"$source_dir/lez/wallet/Cargo.toml" <<'EOF'
[package]
name = "wallet"
version = "0.1.0"
edition = "2021"
EOF
printf 'fn main() {}\n' >"$source_dir/lez/wallet/main.rs"
printf 'token fixture\n' >"$source_dir/artifacts/lez/programs/token.bin"
printf 'ata fixture\n' >"$source_dir/artifacts/lez/programs/associated_token_account.bin"

git -C "$source_dir" init --quiet
git -C "$source_dir" config user.email cache-contract@example.invalid
git -C "$source_dir" config user.name cache-contract
git -C "$source_dir" add .
git -C "$source_dir" commit --quiet -m fixture
git -C "$source_dir" tag v0.2.0
git -C "$source_dir" remote add origin \
  https://github.com/logos-blockchain/cache-contract-fixture.git
source_commit="$(git -C "$source_dir" rev-parse HEAD)"

cat >"$fake_bin/rustc" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
case "${1:-}" in
  +fixture) shift ;;
esac
if [[ "${1:-}" == "-vV" ]]; then
  cat <<'OUT'
rustc 1.96.0 (fixture 2026-01-01)
binary: rustc
commit-hash: fixture
commit-date: 2026-01-01
host: x86_64-unknown-linux-gnu
release: 1.96.0
LLVM version: fixture
OUT
elif [[ "${1:-}" == "--print" && "${2:-}" == "target-libdir" ]]; then
  printf '%s\n' "${FAKE_TARGET_LIBDIR:?}"
else
  echo "unexpected fake rustc invocation: $*" >&2
  exit 2
fi
EOF

cat >"$fake_bin/cargo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
case "${1:-}" in
  +fixture) shift ;;
esac
case "${1:-}" in
  -V)
    printf 'cargo 1.96.0 (fixture 2026-01-01)\n'
    ;;
  metadata)
    [[ "${FAKE_METADATA_FAIL:-0}" != 1 ]] || {
      echo "forced metadata fingerprint failure" >&2
      exit 93
    }
    printf '{"packages":[{"name":"wallet","version":"0.1.0"}],"workspace_members":["wallet 0.1.0"],"resolve":null}\n'
    ;;
  build)
    [[ "${FAKE_FORBID_BUILD:-0}" != 1 ]] || {
      echo "fake wallet builder was unexpectedly invoked" >&2
      exit 91
    }
    target=""
    while (( $# > 0 )); do
      if [[ "$1" == "--target-dir" ]]; then
        target="$2"
        shift 2
      else
        shift
      fi
    done
    [[ -n "$target" ]] || {
      echo "fake wallet builder did not receive --target-dir" >&2
      exit 92
    }
    exec 9>>"${FAKE_BUILD_COUNT:?}"
    flock 9
    count=0
    [[ ! -s "$FAKE_BUILD_COUNT" ]] || read -r count <"$FAKE_BUILD_COUNT"
    printf '%s\n' "$((count + 1))" >"$FAKE_BUILD_COUNT"
    flock -u 9
    if [[ -n "${FAKE_BUILD_DELAY_SECONDS:-}" ]]; then
      sleep "$FAKE_BUILD_DELAY_SECONDS"
    fi
    mkdir -p "$target/debug/build/fixture/out/lez/programs"
    cat >"$target/debug/wallet" <<'WALLET'
#!/usr/bin/env bash
printf 'deterministic official wallet fixture\n'
WALLET
    chmod 0755 "$target/debug/wallet"
    cat >"$target/debug/build/fixture/out/lez/programs/mod.rs" <<'REGISTRY'
pub const TOKEN_ID: [u32; 8] = [2282739141, 348907455, 1046946228, 3735699860, 585462133, 3426087150, 772528164, 2090518099];
pub const ASSOCIATED_TOKEN_ACCOUNT_ID: [u32; 8] = [3357312149, 3615960253, 3351583505, 2234166003, 4153433811, 2743238177, 2886052503, 4160755157];
REGISTRY
    ;;
  *)
    echo "unexpected fake cargo invocation: $*" >&2
    exit 2
    ;;
esac
EOF

cat >"$fake_bin/rustup" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[[ "${1:-}" == "which" && "${2:-}" == "--toolchain" &&
   "${3:-}" == "fixture" ]] || exit 2
fake_dir="$(cd "$(dirname "$0")" && pwd)"
case "${4:-}" in
  rustc) printf '%s/rustc\n' "$fake_dir" ;;
  cargo) printf '%s/cargo\n' "$fake_dir" ;;
  *) exit 2 ;;
esac
EOF
chmod 0555 "$fake_bin/rustc" "$fake_bin/cargo" "$fake_bin/rustup"

native_sha_args=()
for library in librapidsnark.a libgmp.a libfq.a libfr.a; do
  variable="M3_OFFICIAL_WALLET_TEST_${library^^}_SHA256"
  variable="${variable//./_}"
  native_sha_args+=("$variable=$(sha256sum "$native_dir/$library" | sed 's/ .*//')")
done
expected_wallet="$root/expected-wallet"
cat >"$expected_wallet" <<'EOF'
#!/usr/bin/env bash
printf 'deterministic official wallet fixture\n'
EOF
expected_wallet_sha="$(sha256sum "$expected_wallet" | sed 's/ .*//')"

build_count="$root/build.count"
: >"$build_count"

prepare() {
  local cache="$1" destination="$2" evidence="$3"
  shift 3
  env PATH="$fake_bin:$PATH" \
    M3_OFFICIAL_WALLET_CACHE_TEST_MODE=1 \
    M3_OFFICIAL_WALLET_TEST_EXPECTED_COMMIT="$source_commit" \
    M3_OFFICIAL_WALLET_TEST_EXPECTED_ORIGIN=https://github.com/logos-blockchain/cache-contract-fixture.git \
    M3_OFFICIAL_WALLET_TEST_WALLET_SHA256="$expected_wallet_sha" \
    M3_OFFICIAL_WALLET_CACHE_ROOT="$cache" \
    M3_OFFICIAL_WALLET_DESTINATION="$destination" \
    LEZ_V02_SOURCE_DIR="$source_dir" \
    M3_RUST_TOOLCHAIN=fixture \
    RAPIDSNARK_LIB_DIR="$native_dir" \
    BINDGEN_EXTRA_CLANG_ARGS="-I$include_dir" \
    FAKE_TARGET_LIBDIR="$target_libdir" \
    FAKE_BUILD_COUNT="$build_count" \
    "${native_sha_args[@]}" "$@" "$helper" prepare >"$evidence"
}

cache="$root/cache"
destination_one="$root/run-one/debug/wallet"
evidence_one="$root/evidence-one.json"
prepare "$cache" "$destination_one" "$evidence_one"
jq -e '
  .result == "prepared"
  and .cache_hit == false
  and .test_mode == true
  and .validation_policy_revision == 2
  and (.publisher_helper_sha256 | test("^[0-9a-f]{64}$"))
  and .input.validation_policy_revision == 2
  and .input.publisher_helper_sha256 == .publisher_helper_sha256
  and .input.build.expected_wallet_sha256 == .wallet_sha256
  and (.input.toolchain.target_libdir_sha256 | test("^[0-9a-f]{64}$"))
  and (.object_manifest_sha256 | test("^[0-9a-f]{64}$"))
  and (.runtime_fingerprint_sha256 | test("^[0-9a-f]{64}$"))
  and (.duration_ms | numbers) >= 0
  and (.artifact_bytes | numbers) > 0
' \
  "$evidence_one" >/dev/null ||
  fail "first preparation was not a cache miss"
[[ "$(<"$build_count")" == 1 ]] || fail "cache miss did not build exactly once"

input_key="$(jq -er '.input_key' "$evidence_one")"
wallet_sha="$(jq -er '.wallet_sha256' "$evidence_one")"
cached_wallet="$cache/objects/sha256/$wallet_sha/wallet"
[[ -x "$destination_one" && -f "$cached_wallet" && ! -L "$cached_wallet" ]] ||
  fail "cache miss did not publish executable and private copy"
[[ "$(stat -c %a "$destination_one")" == 500 &&
   "$(stat -c %a "$cached_wallet")" == 500 ]] ||
  fail "wallet copies are not mode 0500"
[[ "$(stat -c %i "$destination_one")" != "$(stat -c %i "$cached_wallet")" ]] ||
  fail "private wallet is a hardlink to the cache object"
[[ "$(sha256sum "$destination_one" | sed 's/ .*//')" == "$wallet_sha" ]] ||
  fail "private wallet hash differs from evidence"

mapfile -t object_files < <(
  find "$cache/objects/sha256/$wallet_sha" -mindepth 1 -maxdepth 1 \
    -printf '%f\n' | sort
)
[[ "${object_files[*]}" == "manifest.json wallet" ]] ||
  fail "cache object contains files beyond the strict allowlist"
if find "$cache" -type f \( -name wallet_config.json -o -name storage.json \
    -o -name '*.key' -o -name '*.db' \) -print -quit | grep -q .; then
  fail "cache retained wallet state, a key, or a database"
fi

destination_two="$root/run-two/debug/wallet"
evidence_two="$root/evidence-two.json"
prepare "$cache" "$destination_two" "$evidence_two" FAKE_FORBID_BUILD=1
jq -e --arg key "$input_key" --arg sha "$wallet_sha" '
  .result == "prepared" and .cache_hit == true
  and .input_key == $key and .wallet_sha256 == $sha
' "$evidence_two" >/dev/null || fail "second preparation was not an exact hit"
[[ "$(<"$build_count")" == 1 ]] || fail "cache hit invoked the builder"
[[ "$(stat -c %i "$destination_two")" != "$(stat -c %i "$cached_wallet")" ]] ||
  fail "cache hit hardlinked the private copy"

mkdir -m 0700 "$root/existing"
printf 'do not overwrite\n' >"$root/existing/wallet"
if prepare "$cache" "$root/existing/wallet" "$root/existing-evidence.json" \
    FAKE_FORBID_BUILD=1; then
  fail "cache helper overwrote a preexisting destination"
fi
[[ "$(<"$root/existing/wallet")" == "do not overwrite" ]] ||
  fail "preexisting destination bytes changed"

printf 'fixture include v2\n' >"$include_dir/fixture.h"
prepare "$cache" "$root/run-key-change/debug/wallet" \
  "$root/evidence-key-change.json"
changed_key="$(jq -er '.input_key' "$root/evidence-key-change.json")"
[[ "$changed_key" != "$input_key" && "$(<"$build_count")" == 2 ]] ||
  fail "bindgen include-tree change did not invalidate the input key"
printf 'fixture include v1\n' >"$include_dir/fixture.h"

mkdir -m 0700 "$root/.cargo"
printf '[build]\nincremental = false\n' >"$root/.cargo/config"
prepare "$cache" "$root/run-legacy-config/debug/wallet" \
  "$root/evidence-legacy-config.json"
legacy_config_key="$(jq -er '.input_key' "$root/evidence-legacy-config.json")"
[[ "$legacy_config_key" != "$input_key" && "$(<"$build_count")" == 3 ]] ||
  fail "effective legacy Cargo config did not invalidate the input key"
rm "$root/.cargo/config"
rmdir "$root/.cargo"

printf 'dirty\n' >"$source_dir/untracked"
if prepare "$root/dirty-cache" "$root/run-dirty/debug/wallet" \
    "$root/evidence-dirty.json"; then
  fail "dirty source checkout was accepted"
fi
rm "$source_dir/untracked"

before_fingerprint_failure="$(<"$build_count")"
if prepare "$root/metadata-failure-cache" \
    "$root/metadata-failure-run/debug/wallet" \
    "$root/metadata-failure-evidence.json" FAKE_METADATA_FAIL=1; then
  fail "failed Cargo metadata fingerprint was accepted"
fi
[[ "$(<"$build_count")" == "$before_fingerprint_failure" ]] ||
  fail "fingerprint failure fell through to the wallet builder"

before_wallet_mismatch="$(<"$build_count")"
if prepare "$root/wallet-mismatch-cache" \
    "$root/wallet-mismatch-run/debug/wallet" \
    "$root/wallet-mismatch-evidence.json" \
    M3_OFFICIAL_WALLET_TEST_WALLET_SHA256="$(printf '0%.0s' {1..64})"; then
  fail "unexpected official-wallet output hash was accepted"
fi
[[ "$(<"$build_count")" == "$((before_wallet_mismatch + 1))" ]] ||
  fail "wallet-output mismatch was not rejected immediately after one build"

seed_cache() {
  local name="$1"
  local seeded_cache="$root/${name}-cache"
  local seeded_evidence="$root/${name}-seed.json"
  prepare "$seeded_cache" "$root/${name}-seed/debug/wallet" "$seeded_evidence"
  printf '%s\n%s\n%s\n' "$seeded_cache" \
    "$(jq -er '.input_key' "$seeded_evidence")" \
    "$(jq -er '.wallet_sha256' "$seeded_evidence")"
}

mapfile -t seeded < <(seed_cache symlink-ref)
rm "${seeded[0]}/refs/${seeded[1]}.json"
ln -s /dev/null "${seeded[0]}/refs/${seeded[1]}.json"
if prepare "${seeded[0]}" "$root/symlink-ref-run/debug/wallet" \
    "$root/symlink-ref-evidence.json" FAKE_FORBID_BUILD=1; then
  fail "symlinked cache reference was accepted"
fi

mapfile -t seeded < <(seed_cache tampered-wallet)
chmod 0700 "${seeded[0]}/objects/sha256/${seeded[2]}/wallet"
printf 'tampered\n' >>"${seeded[0]}/objects/sha256/${seeded[2]}/wallet"
chmod 0500 "${seeded[0]}/objects/sha256/${seeded[2]}/wallet"
if prepare "${seeded[0]}" "$root/tampered-wallet-run/debug/wallet" \
    "$root/tampered-wallet-evidence.json" FAKE_FORBID_BUILD=1; then
  fail "tampered cached wallet was accepted"
fi

mapfile -t seeded < <(seed_cache wrong-mode)
chmod 0644 "${seeded[0]}/objects/sha256/${seeded[2]}/manifest.json"
if prepare "${seeded[0]}" "$root/wrong-mode-run/debug/wallet" \
    "$root/wrong-mode-evidence.json" FAKE_FORBID_BUILD=1; then
  fail "wrong-mode object manifest was accepted"
fi

mapfile -t seeded < <(seed_cache missing-object)
rm -rf "${seeded[0]}/objects/sha256/${seeded[2]}"
if prepare "${seeded[0]}" "$root/missing-object-run/debug/wallet" \
    "$root/missing-object-evidence.json" FAKE_FORBID_BUILD=1; then
  fail "published reference with a missing object was treated as a miss"
fi

: >"$build_count"
concurrent_cache="$root/concurrent-cache"
prepare "$concurrent_cache" "$root/concurrent-one/debug/wallet" \
  "$root/concurrent-one.json" FAKE_BUILD_DELAY_SECONDS=1 &
pid_one=$!
prepare "$concurrent_cache" "$root/concurrent-two/debug/wallet" \
  "$root/concurrent-two.json" FAKE_BUILD_DELAY_SECONDS=1 &
pid_two=$!
wait "$pid_one"
wait "$pid_two"
[[ "$(<"$build_count")" == 1 ]] ||
  fail "two same-key callers did not serialize to one build"
hit_sum="$(jq -s '[.[].cache_hit | if . then 1 else 0 end] | add' \
  "$root/concurrent-one.json" "$root/concurrent-two.json")"
[[ "$hit_sum" == 1 ]] ||
  fail "same-key concurrency did not produce exactly one miss and one hit"

echo "M3 official-wallet cache contract passed"
