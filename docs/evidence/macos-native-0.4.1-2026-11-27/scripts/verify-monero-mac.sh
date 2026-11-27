#!/usr/bin/env bash
# verify-monero-mac.sh <repo checkout> <archive> <extract dir>
# The macOS (mac-armv8) Monero 0.18.5.1 archive checked the way
# scripts/verify-monero-release.sh checks the Linux ones: the repository's
# snapshots of the signed hash manifest and of the signer's key, each at its
# pinned sha256 (tests/e2e/monero/provenance.env); the key's fingerprint; a
# VALIDSIG of the pinned signer over the manifest in a throwaway keyring; the
# archive's sha256 on the manifest's line. Then only monerod and
# monero-wallet-rpc are kept from the archive, and their --version printed.
set -euo pipefail
export LC_ALL=C
umask 077
repo="${1:?repo}"; archive="${2:?archive}"; out="${3:?extract dir}"
# shellcheck source=/dev/null
source "$repo/tests/e2e/monero/provenance.env"
name=monero-mac-armv8-v${MONERO_VERSION}.tar.bz2
hashes="$repo/$MONERO_HASHES_SNAPSHOT"; key="$repo/$MONERO_SIGNING_KEY_SNAPSHOT"
sha() { shasum -a 256 "$1" | awk '{print $1}'; }
[[ "$(sha "$hashes")" == "$MONERO_HASHES_SHA256" ]] || { echo "hash manifest snapshot differs from its pin" >&2; exit 1; }
[[ "$(sha "$key")" == "$MONERO_SIGNING_KEY_SHA256" ]] || { echo "signing key snapshot differs from its pin" >&2; exit 1; }
fpr="$(gpg --batch --show-keys --with-colons "$key" | awk -F: '$1=="fpr"{print $10; exit}')"
[[ "$fpr" == "$MONERO_SIGNER_FINGERPRINT" ]] || { echo "signer fingerprint mismatch" >&2; exit 1; }
g="$(mktemp -d "${TMPDIR:-/tmp}/lez-monero-gpg.XXXXXX")"
trap 'gpgconf --homedir "$g" --kill gpg-agent >/dev/null 2>&1 || true; rm -rf "$g"' EXIT
gpg --homedir "$g" --batch --import "$key" >/dev/null 2>&1
gpg --homedir "$g" --batch --status-fd=1 --verify "$hashes" >"$g/status" 2>/dev/null
! grep -Eq '^\[GNUPG:\] (BADSIG|ERRSIG|NO_PUBKEY|EXPSIG|EXPKEYSIG|REVKEYSIG) ' "$g/status"
signer="$(awk '$1=="[GNUPG:]" && $2=="VALIDSIG" {print $NF}' "$g/status")"
[[ "$signer" == "$MONERO_SIGNER_FINGERPRINT" ]] || { echo "manifest signer differs from the pin ($signer)" >&2; exit 1; }
line="$(grep -E "^[0-9a-f]{64}  $name\$" "$hashes")"
want="${line%% *}"; got="$(sha "$archive")"
[[ "$got" == "$want" ]] || { echo "archive sha256 $got, manifest $want" >&2; exit 1; }
echo "manifest snapshot sha256 $MONERO_HASHES_SHA256 (pinned), VALIDSIG $signer (pinned signer)"
echo "archive $name sha256 $got, size $(stat -f %z "$archive") bytes: on the signed manifest"
mkdir -p "$out"
root="$(tar -tjf "$archive" | head -1 | cut -d/ -f1)"
tar -xjf "$archive" -C "$out" "$root/monerod" "$root/monero-wallet-rpc"
for b in monerod monero-wallet-rpc; do
  echo "$b sha256 $(sha "$out/$root/$b") ($(file -b "$out/$root/$b" | cut -c1-60))"
  "$out/$root/$b" --version | tail -1
done
echo "root $root"
