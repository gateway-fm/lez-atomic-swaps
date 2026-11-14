#!/opt/homebrew/bin/bash
# probe-ranges.sh — how logosctl 0.3.2 matches requirements against a version
# that exists only as a pre-release (probe_dep 0.2.2-delivery.1): one fresh
# session per requiring package, the probe catalog beside the official one.
set -u
L=$HOME/tmp/logosctl-0.3.2-macos/x/logosctl-aarch64-macos/bin/logosctl
CA=${SCRATCH:?}/previous/tls/bundle.pem
for p in probe_exact probe_tilde probe_tilde22 probe_bare; do
  S=${SCRATCH:?}/sess/probe-$p; rm -rf "$S"; mkdir -p "$S"
  lc() { env -i HOME="$HOME" USER="$USER" TMPDIR="$TMPDIR" PATH=/usr/bin:/bin SSL_CERT_FILE="$CA" "$L" --config-dir "$S" "$@"; }
  lc daemon start --detach >/dev/null; lc catalog add https://localhost:8474/logos-repo.json >/dev/null; lc catalog source http >/dev/null; lc catalog refresh >/dev/null
  req=$(tar -xzOf ${SCRATCH:?}/probe/pkgs/$p-1.0.0.lgx manifest.json | jq -c .dependencies)
  echo "## $p requires $req"
  out=$(lc package install $p -y 2>&1); rc=$?; echo "install exit $rc: $(jq -c 'if .changes then [.changes[] | "\(.name) \(.toVersion)"] else . end' <<<"$out" 2>/dev/null || echo "$out" | head -3)"
  lc --json package deps $p --recursive 2>&1 | head -3
  lc daemon stop >/dev/null
done
