#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

export LC_ALL=C
umask 077

gnupg_home=""
cleanup_gpg_agent() {
  local status=$?
  trap - EXIT
  if [[ -n "$gnupg_home" ]]; then
    if [[ ! -d "$gnupg_home" || -L "$gnupg_home" ]]; then
      echo "Monero verification keyring changed before GPG-agent cleanup" >&2
      status=1
    elif ! gpgconf --homedir "$gnupg_home" --kill gpg-agent >/dev/null 2>&1; then
      echo "Monero verification GPG-agent cleanup failed" >&2
      status=1
    fi
    if [[ -d "$gnupg_home" && ! -L "$gnupg_home" ]]; then
      rm -rf -- "$gnupg_home" || status=1
    else
      status=1
    fi
  fi
  exit "$status"
}
trap cleanup_gpg_agent EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

provenance_file="${MONERO_PROVENANCE_FILE:-tests/e2e/monero/provenance.env}"
if [[ ! -f "$provenance_file" || -L "$provenance_file" ]]; then
  echo "missing regular Monero provenance contract: ${provenance_file}" >&2
  exit 1
fi

# shellcheck source=/dev/null
source "$provenance_file"

required_variables=(
  MONERO_VERSION
  MONERO_TAG
  MONERO_SOURCE_URL
  MONERO_SOURCE_TAG_OBJECT
  MONERO_SOURCE_COMMIT
  MONERO_ARCHIVE_NAME
  MONERO_ARCHIVE_URL
  MONERO_ARCHIVE_SHA256
  MONERO_ARCHIVE_SIZE
  MONERO_HASHES_URL
  MONERO_HASHES_SHA256
  MONERO_HASHES_SNAPSHOT
  MONERO_SIGNING_KEY_URL
  MONERO_SIGNING_KEY_SHA256
  MONERO_SIGNING_KEY_SNAPSHOT
  MONERO_SIGNER_FINGERPRINT
)
for variable in "${required_variables[@]}"; do
  if [[ -z "${!variable:-}" ]]; then
    echo "Monero provenance contract is missing ${variable}" >&2
    exit 1
  fi
done

: "${MONERO_CACHE_DIR:?MONERO_CACHE_DIR is required}"
: "${MONERO_BUILD_CONTEXT:?MONERO_BUILD_CONTEXT is required}"
: "${MONERO_PROVENANCE_EVIDENCE:?MONERO_PROVENANCE_EVIDENCE is required}"

for path in "$MONERO_CACHE_DIR" "$MONERO_BUILD_CONTEXT" "$MONERO_PROVENANCE_EVIDENCE"; do
  if [[ "$path" != /* ]]; then
    echo "Monero cache, build-context, and evidence paths must be absolute" >&2
    exit 1
  fi
  if [[ -L "$path" ]]; then
    echo "Monero cache, build-context, and evidence paths must not be symbolic links" >&2
    exit 1
  fi
done
if [[ -e "$MONERO_BUILD_CONTEXT" || -e "$MONERO_PROVENANCE_EVIDENCE" ]]; then
  echo "refusing to overwrite Monero build context or provenance evidence" >&2
  exit 1
fi

required_commands=(awk chmod cp curl date diff git gpg gpgconf jq mkdir mktemp mv rg rm sha256sum sort stat tar)
for command_name in "${required_commands[@]}"; do
  command -v "$command_name" >/dev/null || {
    echo "missing Monero provenance tool: ${command_name}" >&2
    exit 1
  }
done

mkdir -p "$MONERO_CACHE_DIR"
chmod 0700 "$MONERO_CACHE_DIR"
evidence_parent="${MONERO_PROVENANCE_EVIDENCE%/*}"
mkdir -p "$evidence_parent"
chmod 0700 "$evidence_parent"

sha256_of() {
  local path="$1"
  local result
  result="$(sha256sum "$path")"
  printf '%s\n' "${result%% *}"
}

verify_regular_sha256() {
  local path="$1"
  local expected="$2"
  if [[ ! -f "$path" || -L "$path" ]]; then
    echo "Monero input is not a regular non-symlink file: ${path}" >&2
    return 1
  fi
  local actual
  actual="$(sha256_of "$path")"
  if [[ "$actual" != "$expected" ]]; then
    echo "Monero input checksum mismatch: ${path}" >&2
    echo "expected=${expected} actual=${actual}" >&2
    return 1
  fi
}

fetch_verified() {
  local url="$1"
  local destination="$2"
  local expected_sha256="$3"
  if [[ -e "$destination" ]]; then
    verify_regular_sha256 "$destination" "$expected_sha256"
    return 0
  fi
  local partial="${destination}.partial"
  curl --proto '=https' --tlsv1.2 --fail --show-error --silent --location \
    "$url" --output "$partial"
  verify_regular_sha256 "$partial" "$expected_sha256"
  mv "$partial" "$destination"
  chmod 0600 "$destination"
}

hashes_file="${MONERO_HASHES_PATH:-$MONERO_HASHES_SNAPSHOT}"
signing_key="${MONERO_SIGNING_KEY_PATH:-$MONERO_SIGNING_KEY_SNAPSHOT}"
archive="${MONERO_CACHE_DIR}/${MONERO_ARCHIVE_NAME}"

verify_regular_sha256 "$hashes_file" "$MONERO_HASHES_SHA256"
verify_regular_sha256 "$signing_key" "$MONERO_SIGNING_KEY_SHA256"
if [[ -n "${MONERO_ARCHIVE_PATH:-}" ]]; then
  if [[ "$MONERO_ARCHIVE_PATH" != /* ]]; then
    echo "MONERO_ARCHIVE_PATH must be absolute" >&2
    exit 1
  fi
  verify_regular_sha256 "$MONERO_ARCHIVE_PATH" "$MONERO_ARCHIVE_SHA256"
  if [[ ! -e "$archive" ]]; then
    cp --reflink=auto "$MONERO_ARCHIVE_PATH" "${archive}.partial"
    verify_regular_sha256 "${archive}.partial" "$MONERO_ARCHIVE_SHA256"
    mv "${archive}.partial" "$archive"
    chmod 0600 "$archive"
  fi
fi
fetch_verified "$MONERO_ARCHIVE_URL" "$archive" "$MONERO_ARCHIVE_SHA256"

actual_archive_size="$(stat -c '%s' "$archive")"
if [[ "$actual_archive_size" != "$MONERO_ARCHIVE_SIZE" ]]; then
  echo "Monero archive size mismatch" >&2
  echo "expected=${MONERO_ARCHIVE_SIZE} actual=${actual_archive_size}" >&2
  exit 1
fi
if ! rg -Fxq "$MONERO_ARCHIVE_SHA256  $MONERO_ARCHIVE_NAME" "$hashes_file"; then
  echo "official signed Monero hash manifest does not bind the expected archive" >&2
  exit 1
fi

actual_key_fingerprint="$(
  gpg --batch --show-keys --with-colons "$signing_key" |
    awk -F: '$1 == "fpr" { print $10; exit }'
)"
if [[ "$actual_key_fingerprint" != "$MONERO_SIGNER_FINGERPRINT" ]]; then
  echo "Monero signing-key fingerprint mismatch" >&2
  exit 1
fi

gnupg_home="$(mktemp -d "${TMPDIR:-/tmp}/lez-monero-gpg-${MONERO_VERSION}.XXXXXX")"
chmod 0700 "$gnupg_home"
gpg --homedir "$gnupg_home" --batch --import "$signing_key" >/dev/null 2>&1
gpg_status="${gnupg_home}/verify.status"
gpg_diagnostics="${gnupg_home}/verify.log"
if ! gpg --homedir "$gnupg_home" --batch --status-fd=1 --verify "$hashes_file" \
  >"$gpg_status" 2>"$gpg_diagnostics"; then
  echo "Monero signed hash manifest verification failed" >&2
  exit 1
fi
if rg -q '^\[GNUPG:\] (BADSIG|ERRSIG|NO_PUBKEY|EXPSIG|EXPKEYSIG|REVKEYSIG) ' "$gpg_status"; then
  echo "Monero hash manifest contains an invalid signature status" >&2
  exit 1
fi
actual_valid_signer="$(
  awk '$1 == "[GNUPG:]" && $2 == "VALIDSIG" { print $3 }' "$gpg_status" |
    sort -u
)"
if [[ "$actual_valid_signer" != "$MONERO_SIGNER_FINGERPRINT" ]]; then
  echo "Monero hash manifest signer differs from the pinned fingerprint" >&2
  exit 1
fi

tag_listing="$(git ls-remote "$MONERO_SOURCE_URL" "refs/tags/${MONERO_TAG}" "refs/tags/${MONERO_TAG}^{}")"
actual_tag_object="$(printf '%s\n' "$tag_listing" | awk -v tag="refs/tags/${MONERO_TAG}" '$2 == tag { print $1 }')"
actual_source_commit="$(printf '%s\n' "$tag_listing" | awk -v tag="refs/tags/${MONERO_TAG}^{}" '$2 == tag { print $1 }')"
if [[ "$actual_tag_object" != "$MONERO_SOURCE_TAG_OBJECT" || "$actual_source_commit" != "$MONERO_SOURCE_COMMIT" ]]; then
  echo "Monero source tag identity mismatch" >&2
  exit 1
fi

archive_root="monero-x86_64-linux-gnu-v${MONERO_VERSION}"
archive_members="${MONERO_CACHE_DIR}/archive-members-${MONERO_VERSION}.txt"
tar -tjf "$archive" >"$archive_members"
for binary in monerod monero-wallet-rpc; do
  if ! rg -Fxq "${archive_root}/${binary}" "$archive_members"; then
    echo "Monero archive is missing ${archive_root}/${binary}" >&2
    exit 1
  fi
done

context_partial="${MONERO_BUILD_CONTEXT}.partial"
mkdir -p "${context_partial}/bin"
chmod 0700 "$context_partial" "${context_partial}/bin"
tar -xjf "$archive" -C "${context_partial}/bin" --strip-components=1 \
  "${archive_root}/monerod" "${archive_root}/monero-wallet-rpc"
chmod 0555 "${context_partial}/bin/monerod" "${context_partial}/bin/monero-wallet-rpc"
monerod_sha256="$(sha256_of "${context_partial}/bin/monerod")"
wallet_rpc_sha256="$(sha256_of "${context_partial}/bin/monero-wallet-rpc")"
monerod_version="$("${context_partial}/bin/monerod" --version | awk 'NR == 1')"
wallet_rpc_version="$("${context_partial}/bin/monero-wallet-rpc" --version | awk 'NR == 1')"
expected_version="Monero 'Fluorine Fermi' (v${MONERO_VERSION}-release)"
if [[ "$monerod_version" != "$expected_version" || "$wallet_rpc_version" != "$expected_version" ]]; then
  echo "verified Monero binary version output mismatch" >&2
  exit 1
fi
mv "$context_partial" "$MONERO_BUILD_CONTEXT"

verified_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
evidence_partial="${MONERO_PROVENANCE_EVIDENCE}.partial"
jq -n \
  --arg verified_at "$verified_at" \
  --arg version "$MONERO_VERSION" \
  --arg archive_name "$MONERO_ARCHIVE_NAME" \
  --arg archive_url "$MONERO_ARCHIVE_URL" \
  --arg archive_sha256 "$MONERO_ARCHIVE_SHA256" \
  --argjson archive_size "$actual_archive_size" \
  --arg hashes_url "$MONERO_HASHES_URL" \
  --arg hashes_sha256 "$MONERO_HASHES_SHA256" \
  --arg hashes_snapshot "$MONERO_HASHES_SNAPSHOT" \
  --arg key_url "$MONERO_SIGNING_KEY_URL" \
  --arg key_sha256 "$MONERO_SIGNING_KEY_SHA256" \
  --arg key_snapshot "$MONERO_SIGNING_KEY_SNAPSHOT" \
  --arg signer "$actual_valid_signer" \
  --arg source_repository "$MONERO_SOURCE_URL" \
  --arg source_tag "$MONERO_TAG" \
  --arg source_tag_object "$actual_tag_object" \
  --arg source_commit "$actual_source_commit" \
  --arg monerod_sha256 "$monerod_sha256" \
  --arg wallet_rpc_sha256 "$wallet_rpc_sha256" '
  {
    schema_version: 1,
    result: "passed",
    verified_at: $verified_at,
    release: {
      version: $version,
      source_repository: $source_repository,
      source_tag: $source_tag,
      source_tag_object: $source_tag_object,
      source_commit: $source_commit
    },
    consumed_archive: {
      name: $archive_name,
      url: $archive_url,
      sha256: $archive_sha256,
      size_bytes: $archive_size
    },
    signed_manifest: {
      url: $hashes_url,
      sha256: $hashes_sha256,
      retained_snapshot: $hashes_snapshot,
      signing_key_url: $key_url,
      signing_key_sha256: $key_sha256,
      retained_signing_key: $key_snapshot,
      signer_primary_fingerprint: $signer
    },
    extracted_binaries: {
      monerod_sha256: $monerod_sha256,
      wallet_rpc_sha256: $wallet_rpc_sha256,
      version_output_matched: true
    },
    external_resources: [$archive_url, $hashes_url, $key_url, $source_repository]
  }
' >"$evidence_partial"
chmod 0600 "$evidence_partial"
mv "$evidence_partial" "$MONERO_PROVENANCE_EVIDENCE"

printf 'Monero %s release provenance verified: %s\n' \
  "$MONERO_VERSION" "$MONERO_PROVENANCE_EVIDENCE"
