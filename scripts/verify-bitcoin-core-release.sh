#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

export LC_ALL=C
umask 077

provenance_file="${BITCOIN_CORE_PROVENANCE_FILE:-tests/e2e/bitcoin-core/provenance.env}"
if [[ ! -f "$provenance_file" || -L "$provenance_file" ]]; then
  echo "missing regular Bitcoin Core provenance contract: ${provenance_file}" >&2
  exit 1
fi

# shellcheck source=/dev/null
source "$provenance_file"

required_variables=(
  BITCOIN_CORE_VERSION
  BITCOIN_CORE_TAG
  BITCOIN_CORE_SOURCE_URL
  BITCOIN_CORE_SOURCE_TAG_OBJECT
  BITCOIN_CORE_SOURCE_COMMIT
  BITCOIN_CORE_ARCHIVE_NAME
  BITCOIN_CORE_ARCHIVE_URL
  BITCOIN_CORE_ARCHIVE_SHA256
  BITCOIN_CORE_ARCHIVE_SIZE
  BITCOIN_CORE_RELEASE_BASE_URL
  BITCOIN_CORE_SHA256SUMS_SHA256
  BITCOIN_CORE_SHA256SUMS_ASC_SHA256
  BITCOIN_CORE_GUIX_SIGS_URL
  BITCOIN_CORE_GUIX_SIGS_COMMIT
  BITCOIN_CORE_GUIX_SIGS_RELEASE
  BITCOIN_CORE_RELEASE_SIGNER_FINGERPRINTS
  BITCOIN_CORE_GUIX_BUILDERS
)
for variable in "${required_variables[@]}"; do
  if [[ -z "${!variable:-}" ]]; then
    echo "Bitcoin Core provenance contract is missing ${variable}" >&2
    exit 1
  fi
done

: "${BITCOIN_CORE_CACHE_DIR:?BITCOIN_CORE_CACHE_DIR is required}"
: "${BITCOIN_CORE_PROVENANCE_EVIDENCE:?BITCOIN_CORE_PROVENANCE_EVIDENCE is required}"

if [[ "$BITCOIN_CORE_CACHE_DIR" != /* || "$BITCOIN_CORE_PROVENANCE_EVIDENCE" != /* ]]; then
  echo "Bitcoin Core cache and evidence paths must be absolute" >&2
  exit 1
fi
if [[ -L "$BITCOIN_CORE_CACHE_DIR" || -L "$BITCOIN_CORE_PROVENANCE_EVIDENCE" ]]; then
  echo "Bitcoin Core cache/evidence paths must not be symbolic links" >&2
  exit 1
fi
if [[ -e "$BITCOIN_CORE_PROVENANCE_EVIDENCE" ]]; then
  echo "refusing to overwrite Bitcoin Core provenance evidence: ${BITCOIN_CORE_PROVENANCE_EVIDENCE}" >&2
  exit 1
fi

required_commands=(awk chmod cp curl date diff git gpg jq mkdir mv rg sha256sum sort stat tar tr)
for command_name in "${required_commands[@]}"; do
  command -v "$command_name" >/dev/null || {
    echo "missing Bitcoin Core provenance tool: ${command_name}" >&2
    exit 1
  }
done

mkdir -p "$BITCOIN_CORE_CACHE_DIR"
chmod 0700 "$BITCOIN_CORE_CACHE_DIR"
evidence_parent="${BITCOIN_CORE_PROVENANCE_EVIDENCE%/*}"
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
    echo "Bitcoin Core input is not a regular non-symlink file: ${path}" >&2
    return 1
  fi
  local actual
  actual="$(sha256_of "$path")"
  if [[ "$actual" != "$expected" ]]; then
    echo "Bitcoin Core input checksum mismatch: ${path}" >&2
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
  curl --proto '=https' --tlsv1.2 --fail --show-error --silent --location "$url" --output "$partial"
  verify_regular_sha256 "$partial" "$expected_sha256"
  mv "$partial" "$destination"
  chmod 0600 "$destination"
}

sha256sums="${BITCOIN_CORE_CACHE_DIR}/SHA256SUMS"
sha256sums_asc="${BITCOIN_CORE_CACHE_DIR}/SHA256SUMS.asc"
archive="${BITCOIN_CORE_CACHE_DIR}/${BITCOIN_CORE_ARCHIVE_NAME}"

fetch_verified "${BITCOIN_CORE_RELEASE_BASE_URL}/SHA256SUMS" "$sha256sums" "$BITCOIN_CORE_SHA256SUMS_SHA256"
fetch_verified "${BITCOIN_CORE_RELEASE_BASE_URL}/SHA256SUMS.asc" "$sha256sums_asc" "$BITCOIN_CORE_SHA256SUMS_ASC_SHA256"

if [[ -n "${BITCOIN_CORE_ARCHIVE_PATH:-}" ]]; then
  if [[ "$BITCOIN_CORE_ARCHIVE_PATH" != /* ]]; then
    echo "BITCOIN_CORE_ARCHIVE_PATH must be absolute" >&2
    exit 1
  fi
  verify_regular_sha256 "$BITCOIN_CORE_ARCHIVE_PATH" "$BITCOIN_CORE_ARCHIVE_SHA256"
  if [[ ! -e "$archive" ]]; then
    cp --reflink=auto "$BITCOIN_CORE_ARCHIVE_PATH" "${archive}.partial"
    verify_regular_sha256 "${archive}.partial" "$BITCOIN_CORE_ARCHIVE_SHA256"
    mv "${archive}.partial" "$archive"
    chmod 0600 "$archive"
  fi
fi
fetch_verified "$BITCOIN_CORE_ARCHIVE_URL" "$archive" "$BITCOIN_CORE_ARCHIVE_SHA256"

actual_archive_size="$(stat -c '%s' "$archive")"
if [[ "$actual_archive_size" != "$BITCOIN_CORE_ARCHIVE_SIZE" ]]; then
  echo "Bitcoin Core archive size mismatch" >&2
  echo "expected=${BITCOIN_CORE_ARCHIVE_SIZE} actual=${actual_archive_size}" >&2
  exit 1
fi
if ! rg -Fxq "${BITCOIN_CORE_ARCHIVE_SHA256}  ${BITCOIN_CORE_ARCHIVE_NAME}" "$sha256sums"; then
  echo "official SHA256SUMS does not bind the expected Bitcoin Core archive" >&2
  exit 1
fi

tag_listing="$(git ls-remote "$BITCOIN_CORE_SOURCE_URL" "refs/tags/${BITCOIN_CORE_TAG}" "refs/tags/${BITCOIN_CORE_TAG}^{}")"
actual_tag_object="$(printf '%s\n' "$tag_listing" | awk -v tag="refs/tags/${BITCOIN_CORE_TAG}" '$2 == tag { print $1 }')"
actual_source_commit="$(printf '%s\n' "$tag_listing" | awk -v tag="refs/tags/${BITCOIN_CORE_TAG}^{}" '$2 == tag { print $1 }')"
if [[ "$actual_tag_object" != "$BITCOIN_CORE_SOURCE_TAG_OBJECT" || "$actual_source_commit" != "$BITCOIN_CORE_SOURCE_COMMIT" ]]; then
  echo "Bitcoin Core source tag identity mismatch" >&2
  exit 1
fi

guix_dir="${BITCOIN_CORE_CACHE_DIR}/guix.sigs"
if [[ -e "$guix_dir" && ! -d "${guix_dir}/.git" ]]; then
  echo "invalid cached guix.sigs repository" >&2
  exit 1
fi
if [[ ! -e "$guix_dir" ]]; then
  mkdir "$guix_dir"
  git -C "$guix_dir" init --quiet
  git -C "$guix_dir" remote add origin "$BITCOIN_CORE_GUIX_SIGS_URL"
  git -C "$guix_dir" fetch --quiet --depth 1 origin "$BITCOIN_CORE_GUIX_SIGS_COMMIT"
  git -C "$guix_dir" checkout --quiet --detach FETCH_HEAD
fi
if [[ "$(git -C "$guix_dir" rev-parse HEAD)" != "$BITCOIN_CORE_GUIX_SIGS_COMMIT" || "$(git -C "$guix_dir" remote get-url origin)" != "$BITCOIN_CORE_GUIX_SIGS_URL" ]]; then
  echo "cached guix.sigs repository identity mismatch" >&2
  exit 1
fi

gnupg_home="${BITCOIN_CORE_CACHE_DIR}/gnupg"
if [[ -e "$gnupg_home" ]]; then
  echo "refusing to reuse Bitcoin Core verification keyring: ${gnupg_home}" >&2
  exit 1
fi
mkdir -m 0700 "$gnupg_home"
gpg --homedir "$gnupg_home" --batch --import "${guix_dir}"/builder-keys/*.gpg >/dev/null 2>&1

gpg_status="${BITCOIN_CORE_CACHE_DIR}/release-signatures.status"
gpg_diagnostics="${BITCOIN_CORE_CACHE_DIR}/release-signatures.log"
if ! gpg --homedir "$gnupg_home" --batch --status-fd=1 --verify "$sha256sums_asc" "$sha256sums" >"$gpg_status" 2>"$gpg_diagnostics"; then
  echo "Bitcoin Core SHA256SUMS signature verification failed" >&2
  exit 1
fi
if rg -q '^\[GNUPG:\] (BADSIG|ERRSIG|NO_PUBKEY|EXPSIG|EXPKEYSIG|REVKEYSIG) ' "$gpg_status"; then
  echo "Bitcoin Core SHA256SUMS contains an invalid signature status" >&2
  exit 1
fi

actual_signers="${BITCOIN_CORE_CACHE_DIR}/release-signers.actual"
expected_signers="${BITCOIN_CORE_CACHE_DIR}/release-signers.expected"
awk '$1 == "[GNUPG:]" && $2 == "VALIDSIG" { print $NF }' "$gpg_status" | sort -u >"$actual_signers"
printf '%s\n' "$BITCOIN_CORE_RELEASE_SIGNER_FINGERPRINTS" | tr ' ' '\n' | awk 'NF' | sort -u >"$expected_signers"
if ! diff -u "$expected_signers" "$actual_signers"; then
  echo "Bitcoin Core release signer set differs from the pinned contract" >&2
  exit 1
fi

read -r -a guix_builders <<<"$BITCOIN_CORE_GUIX_BUILDERS"
if (( ${#guix_builders[@]} != 15 )); then
  echo "Bitcoin Core provenance requires the exact 15-builder attestation set" >&2
  exit 1
fi
for builder in "${guix_builders[@]}"; do
  builder_dir="${guix_dir}/${BITCOIN_CORE_GUIX_SIGS_RELEASE}/${builder}"
  builder_sums="${builder_dir}/all.SHA256SUMS"
  builder_signature="${builder_sums}.asc"
  if [[ ! -f "$builder_sums" || ! -f "$builder_signature" || -L "$builder_sums" || -L "$builder_signature" ]]; then
    echo "missing regular Guix attestation for builder ${builder}" >&2
    exit 1
  fi
  if ! rg -Fxq "${BITCOIN_CORE_ARCHIVE_SHA256}  ${BITCOIN_CORE_ARCHIVE_NAME}" "$builder_sums"; then
    echo "Guix builder ${builder} disagrees on the Bitcoin Core archive" >&2
    exit 1
  fi
  if ! gpg --homedir "$gnupg_home" --batch --verify "$builder_signature" "$builder_sums" >/dev/null 2>&1; then
    echo "invalid Guix attestation signature for builder ${builder}" >&2
    exit 1
  fi
done

archive_members="${BITCOIN_CORE_CACHE_DIR}/archive-members.txt"
tar -tzf "$archive" >"$archive_members"
for required_member in "bitcoin-${BITCOIN_CORE_VERSION}/bin/bitcoind" "bitcoin-${BITCOIN_CORE_VERSION}/bin/bitcoin-cli" "bitcoin-${BITCOIN_CORE_VERSION}/share/rpcauth/rpcauth.py"; do
  if ! rg -Fxq "$required_member" "$archive_members"; then
    echo "Bitcoin Core archive is missing ${required_member}" >&2
    exit 1
  fi
done

release_signers_json="$(jq -Rsc 'split("\n") | map(select(length > 0))' "$actual_signers")"
guix_builders_json="$(printf '%s\n' "${guix_builders[@]}" | sort -u | jq -Rsc 'split("\n") | map(select(length > 0))')"
verified_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
evidence_partial="${BITCOIN_CORE_PROVENANCE_EVIDENCE}.partial"
jq_arguments=(
  --arg verified_at "$verified_at"
  --arg version "$BITCOIN_CORE_VERSION"
  --arg archive_name "$BITCOIN_CORE_ARCHIVE_NAME"
  --arg archive_url "$BITCOIN_CORE_ARCHIVE_URL"
  --arg archive_sha256 "$BITCOIN_CORE_ARCHIVE_SHA256"
  --argjson archive_size "$actual_archive_size"
  --arg sha256sums_sha256 "$BITCOIN_CORE_SHA256SUMS_SHA256"
  --arg sha256sums_asc_sha256 "$BITCOIN_CORE_SHA256SUMS_ASC_SHA256"
  --arg source_repository "$BITCOIN_CORE_SOURCE_URL"
  --arg source_tag "$BITCOIN_CORE_TAG"
  --arg source_tag_object "$BITCOIN_CORE_SOURCE_TAG_OBJECT"
  --arg source_commit "$BITCOIN_CORE_SOURCE_COMMIT"
  --arg guix_sigs_repository "$BITCOIN_CORE_GUIX_SIGS_URL"
  --arg guix_sigs_commit "$BITCOIN_CORE_GUIX_SIGS_COMMIT"
  --argjson release_signers "$release_signers_json"
  --argjson guix_builders "$guix_builders_json"
)
jq -n "${jq_arguments[@]}" '{
  schema_version: 1,
  result: "passed",
  verified_at: $verified_at,
  core: {
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
  release_manifest: {
    sha256: $sha256sums_sha256,
    signatures_sha256: $sha256sums_asc_sha256,
    valid_signer_primary_fingerprints: $release_signers
  },
  reproducible_build_attestations: {
    repository: $guix_sigs_repository,
    commit: $guix_sigs_commit,
    agreeing_builders: $guix_builders
  },
  external_resources: [
    $archive_url,
    $source_repository,
    $guix_sigs_repository
  ]
}' >"$evidence_partial"
chmod 0600 "$evidence_partial"
mv "$evidence_partial" "$BITCOIN_CORE_PROVENANCE_EVIDENCE"

printf 'Bitcoin Core %s release provenance verified: %s\n' "$BITCOIN_CORE_VERSION" "$BITCOIN_CORE_PROVENANCE_EVIDENCE"
