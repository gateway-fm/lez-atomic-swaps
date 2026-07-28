#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly unit="packaging/systemd/lez-maker-daemon.service"
readonly installer="scripts/install-m5-maker-service.sh"
readonly staged_rehearsal="scripts/rehearse-m5-maker-service-install.sh"
readonly transient_rehearsal="scripts/run-m5-maker-systemd-transient.sh"
readonly lifecycle="crates/maker-node/src/daemon_lifecycle.rs"
readonly process_test="crates/maker-node/tests/daemon_lifecycle.rs"
readonly daemon="crates/maker-node/src/bin/lez-maker-daemon.rs"
readonly secure_file="crates/maker-node/src/bin/support/secure_file.rs"
readonly manifest="crates/maker-node/Cargo.toml"
readonly manual="docs/manual-user-flows.md"
readonly decision="docs/architecture/0097-supervise-one-maker-daemon-lifecycle.md"

fail() {
  echo "M5 service lifecycle contract failed: $*" >&2
  exit 1
}

for path in \
  "$unit" "$installer" "$staged_rehearsal" "$transient_rehearsal" \
  "$lifecycle" "$process_test" "$decision"; do
  test -f "$path" || fail "missing $path"
done
for script in "$installer" "$staged_rehearsal" "$transient_rehearsal"; do
  test -x "$script" || fail "$script is not executable"
  bash -n "$script"
done

for directive in \
  'Type=notify' \
  'NotifyAccess=main' \
  'User=lez-swap' \
  'Group=lez-swap' \
  'RuntimeDirectory=lez-atomic-swaps' \
  'RuntimeDirectoryMode=0700' \
  'StateDirectory=lez-atomic-swaps' \
  'StateDirectoryMode=0700' \
  'KillMode=control-group' \
  'MemoryDenyWriteExecute=yes' \
  'SystemCallArchitectures=native' \
  'SystemCallFilter=@system-service memfd_create' \
  'SystemCallErrorNumber=EPERM' \
  'ProtectProc=invisible' \
  'UMask=0077' \
  'Restart=on-failure' \
  'TimeoutStopSec=30s' \
  'KillSignal=SIGTERM' \
  'NoNewPrivileges=yes' \
  'PrivateTmp=yes' \
  'ProtectSystem=strict' \
  'ProtectHome=yes' \
  'RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6' \
  'CapabilityBoundingSet=' \
  'LoadCredentialEncrypted=delivery-signing.key:' \
  'LoadCredentialEncrypted=maker-claim-recovery.key:' \
  'LoadCredentialEncrypted=maker-claim-preimage.key:' \
  'EnvironmentFile=/etc/lez-atomic-swaps/zec-actor.env' \
  '--actor-supervisor' \
  '--zec-source-maker-config /var/lib/lez-atomic-swaps/authority/zec-maker.json' \
  '--zec-maker-actor-root /var/lib/lez-atomic-swaps/actors' \
  '--zec-actor-program /usr/bin/zec-reference-actor' \
  '--zec-actor-program-sha256 ${ZEC_ACTOR_PROGRAM_SHA256}' \
  '--ready-file /run/lez-atomic-swaps/ready'; do
  rg -Fq -- "$directive" "$unit" || fail "unit is missing $directive"
done

for token in \
  'SOURCE_BIN_DIR' \
  'DESTDIR' \
  'install -D -m 0755' \
  'install -D -m 0644' \
  'zec-reference-actor' \
  'lez-maker-daemon.env.example' \
  'lez-maker-daemon.service'; do
  rg -Fq -- "$token" "$installer" || fail "installer is missing $token"
done

for token in \
  'pub trait MakerDaemonLifecycle' \
  'async fn start' \
  'fn endpoint' \
  'async fn health' \
  'async fn stop' \
  'ProcessMakerDaemon' \
  'maker_health' \
  'Signal::TERM' \
  'tokio::time::timeout'; do
  rg -Fq -- "$token" "$lifecycle" || fail "lifecycle adapter is missing $token"
done

rg -Fq 'SignalKind::terminate()' "$daemon" || fail "daemon does not handle SIGTERM"
rg -Fq 'NonBlockingLockExclusive' "$daemon" ||
  fail "daemon does not acquire a nonblocking process-lifetime state lease"
rg -Fq 'NotifyState::Ready' "$daemon" || fail "daemon does not notify readiness"
rg -Fq 'NotifyState::Stopping' "$daemon" || fail "daemon does not notify stopping"
rg -Fq 'MakerActorLeaseOwner::random()' "$daemon" ||
  fail "daemon does not generate a collision-resistant coordinator owner"
rg -Fq 'supervise_one_abandoned_maker_actor' "$daemon" ||
  fail "daemon does not recover abandoned actor leases"
rg -Fq 'spawn_blocking' "$daemon" || fail "actor supervisor does not use an isolated store task"
rg -Fq 'matches!(mode, 0o400 | 0o600)' "$secure_file" ||
  fail "systemd runtime credential mode 0400 is not accepted"
rg -Fq 'sd-notify = { version = "=0.5.0"' "$manifest" ||
  fail "sd-notify is not exact-pinned with default features disabled"
rg -Fq 'actual_process_lifecycle_is_ready_healthy_and_gracefully_stopped' "$process_test" ||
  fail "actual-process lifecycle test is missing"
rg -Fq 'one_database_has_one_process_lifetime_writer_lease' "$process_test" ||
  fail "single-writer process lease test is missing"
rg -Fq 'systemd-analyze verify' "$staged_rehearsal" ||
  fail "staged systemd verification is missing"
rg -Fq 'Type=notify' "$transient_rehearsal" ||
  fail "actual transient notification rehearsal is missing"
rg -Fq 'systemctl --user kill --kill-whom=main --signal=SIGKILL' "$transient_rehearsal" ||
  fail "actual crash/restart rehearsal is missing"
rg -Fq 'systemd-analyze verify' "$manual" || fail "manual install verification is missing"
rg -Fq 'Logos Core daemon mode' "$manual" || fail "manual upstream boundary is missing"
rg -Fq 'systemd-creds encrypt' "$manual" || fail "encrypted credential provisioning is missing"
rg -Fq 'sudo -u lez-swap' "$manual" || fail "service-user CLI flow is missing"

echo "M5 service lifecycle contract passed"
