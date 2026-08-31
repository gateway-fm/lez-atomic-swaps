#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

readonly unit="packaging/systemd/lez-maker-node.service"
readonly node_config="packaging/systemd/lez-maker-node.json.example"
readonly installer="scripts/install-maker-node-service.sh"
readonly staged_rehearsal="scripts/rehearse-m5-maker-service-install.sh"
readonly transient_rehearsal="scripts/run-m5-maker-systemd-transient.sh"
readonly lifecycle="crates/maker-node/src/daemon_lifecycle.rs"
readonly service_control="crates/node-common/src/service_control.rs"
readonly operator_cli="crates/maker-node/src/bin/lez-maker.rs"
readonly service_cli_test="crates/maker-node/tests/maker_service_cli.rs"
readonly process_test="crates/maker-node/tests/daemon_lifecycle.rs"
readonly daemon="crates/maker-node/src/bin/lez-maker-daemon.rs"
readonly secure_file="crates/node-common/src/secure_file.rs"
readonly manifest="crates/maker-node/Cargo.toml"
readonly runtime_contract_decision="docs/architecture/0212-version-runtime-components.md"
readonly actual_chain_crash_runner="scripts/run-m4-actual-claim-poc.sh"

fail() {
  echo "M5 service lifecycle contract failed: $*" >&2
  exit 1
}

for path in \
  "$unit" "$node_config" "$installer" "$staged_rehearsal" "$transient_rehearsal" \
  "$lifecycle" "$service_control" "$operator_cli" "$service_cli_test" \
  "$process_test" "$runtime_contract_decision" "$actual_chain_crash_runner"; do
  test -f "$path" || fail "missing $path"
done
for token in \
  'M7_XMR_REFUND_PROCESS_KILL_AFTER_SUBMISSION' \
  '--features test-crash-hooks' \
  '--actor-test-pause-operation sweep_monero_refund' \
  'kill -KILL -- "-${crashed_daemon_group}"' \
  'kill -KILL -- "-${crashed_actor_group}"' \
  'start_m5_xmr_application_daemon m7-refund-recovery 1' \
  'recovered_generation > crashed_generation' \
  'post_restart_route:"observe_only_pending"'; do
  rg -Fq -- "$token" "$actual_chain_crash_runner" ||
    fail "actual-chain refund crash recovery is missing $token"
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
  'RuntimeDirectory=lez/maker' \
  'RuntimeDirectoryMode=0700' \
  'StateDirectory=lez/maker' \
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
  'ExecStart=/usr/bin/lez-maker-node --config /etc/lez/maker/node.json'; do
  rg -Fq -- "$directive" "$unit" || fail "unit is missing $directive"
done

for directive in \
  '--actor-supervisor' \
  '/var/lib/lez/maker/authority/zec-maker.json' \
  '/var/lib/lez/maker/actors' \
  '/usr/bin/lez-zec-maker-actor' \
  '--zec-actor-program-sha256' \
  '--ready-file' \
  '/run/lez/maker/ready'; do
  rg -Fq -- "$directive" "$node_config" || fail "Node config is missing $directive"
done

for token in \
  'SOURCE_BIN_DIR' \
  'DESTDIR' \
  'install -D -m 0755' \
  'install -D -m 0644' \
  'lez-zec-maker-actor' \
  'lez-maker-node.json.example' \
  'lez-maker-node.service'; do
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

for token in \
  'const SYSTEMCTL_PROGRAM: &str = "/usr/bin/systemctl"' \
  'const MAKER_UNIT: &str = "lez-maker-node.service"' \
  'const SYSTEMCTL_TIMEOUT: Duration = Duration::from_secs(30)' \
  'Command::new(SYSTEMCTL_PROGRAM)' \
  '.wait_timeout(SYSTEMCTL_TIMEOUT)' \
  'kill_and_reap(&mut child)' \
  '.take((MAXIMUM_STATE_BYTES + 1) as u64)' \
  'SystemctlTimeout' \
  '--no-ask-password' \
  '--property=ActiveState'; do
  rg -Fq -- "$token" "$service_control" ||
    fail "fixed service control is missing $token"
done
for token in 'Command::Start' 'Command::Stop' 'control_service(&command)'; do
  rg -Fq -- "$token" "$operator_cli" || fail "operator CLI is missing $token"
done
for token in \
  'maker_cli_exposes_fixed_service_start_and_stop_without_daemon_rpc' \
  'fixed_system_commands_reach_exact_postconditions' \
  'action_and_state_query_timeouts_report_uncertain_state'; do
  rg -Fq -- "$token" "$service_control" "$service_cli_test" ||
    fail "service-control test is missing $token"
done
if rg -Fq 'MakerServiceScope' "$service_control" "$operator_cli"; then
  fail "operator lifecycle control admits an unpackaged service scope"
fi

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
for token in \
  'm5-systemd-fault-actor' \
  '--actor-test-pause-swap-id' \
  '--actor-test-pause-operation zcash_fund' \
  'paused_after_submitted_before_stdout' \
  'lease_generation == 1' \
  'lease_generation == 2' \
  'effect_identity_preserved == true' \
  'disjoint_peer_progressed == true' \
  'KillMode=control-group'; do
  rg -Fq -- "$token" "$transient_rehearsal" ||
    fail "actual actor crash/restart rehearsal is missing $token"
done
echo "M5 service lifecycle contract passed"
