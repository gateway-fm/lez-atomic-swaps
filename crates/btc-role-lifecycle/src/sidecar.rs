//! One LEZ role sidecar per swap, spawned and supervised by the Node.
//!
//! The sidecar keeps one durable witnessed-escrow and one witnessed-claim
//! reservation per state directory and one active prepare of each kind per
//! process, so a sidecar can serve exactly one swap. Each swap therefore gets
//! its own process on its own loopback port, with its own capability, state
//! directory and log, all under the swap directory; the bridge run id both
//! roles use derives from the reservation id so the peers agree on it without
//! configuration.

use std::{
    fs::{self, OpenOptions},
    net::{Ipv4Addr, SocketAddrV4, TcpStream},
    os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _},
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result, ensure};
use lez_bridge_protocol::{RequestId, RunId};
use serde::{Deserialize, Serialize};

use crate::{
    config::{BtcRoleRuntime, LezSequencerRoute},
    layout::{SwapLayout, read_private, write_private_exact},
    lez::LezSidecar,
};

/// Upper bound for the sidecar log scanned for its ready line.
const MAX_LOG_SCAN_BYTES: usize = 16 * 1024 * 1024;
const READY_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RECORD_BYTES: usize = 16 * 1024;

/// The bridge run id of one swap: derived, so both roles agree on it.
///
/// # Errors
///
/// Fails only if the reservation id cannot form a run id (it always can).
pub fn swap_run_id(reservation_id: &RequestId) -> Result<RunId> {
    let tail: String = reservation_id.as_str().chars().take(58).collect();
    RunId::new(format!("swap-{tail}")).context("swap run id")
}

/// What the Node recorded about a swap's sidecar.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SidecarRecordV1 {
    pub schema_version: u16,
    pub port: u16,
    pub run_id: String,
    pub capability_file: PathBuf,
    pub state_directory: PathBuf,
    pub log_file: PathBuf,
}

/// A running (or restartable) sidecar bound to one swap.
#[derive(Clone, Debug)]
pub struct SwapSidecar {
    record: SidecarRecordV1,
    run_id: RunId,
}

impl SwapSidecar {
    /// Ensures the swap's sidecar is running: reuses the recorded one when it
    /// answers on its port, otherwise spawns it (first time or after a Node
    /// restart; the sidecar's durable store recovers its own reservations).
    ///
    /// # Errors
    ///
    /// Fails when no loopback port is free, the program cannot start, or it
    /// does not report readiness in time.
    pub fn ensure(
        runtime: &BtcRoleRuntime,
        layout: &SwapLayout,
        reservation_id: &RequestId,
    ) -> Result<Self> {
        let run_id = swap_run_id(reservation_id)?;
        let record_file = layout.root().join("sidecar.json");
        let record = if let Ok(bytes) = read_private(&record_file, MAX_RECORD_BYTES) {
            serde_json::from_slice::<SidecarRecordV1>(&bytes).context("sidecar record")?
        } else {
            let lez = &runtime.config().lez;
            record_with_free_port(
                &runtime.config().swaps_root,
                lez.sidecar_port_base,
                lez.sidecar_port_count,
                &record_file,
                |port| {
                    let record = SidecarRecordV1 {
                        schema_version: 1,
                        port,
                        run_id: run_id.as_str().to_owned(),
                        capability_file: layout.root().join("sidecar-capability"),
                        state_directory: layout.root().join("sidecar-state"),
                        log_file: layout.root().join("sidecar.log"),
                    };
                    let mut capability = [0_u8; 32];
                    getrandom::fill(&mut capability)
                        .map_err(|_| anyhow::anyhow!("OS randomness unavailable"))?;
                    write_private_exact(
                        &record.capability_file,
                        format!("{}\n", hex::encode(capability)).as_bytes(),
                    )?;
                    fs::DirBuilder::new()
                        .mode(0o700)
                        .create(&record.state_directory)?;
                    Ok(record)
                },
            )?
        };
        ensure!(
            record.run_id == run_id.as_str(),
            "sidecar record names another run id"
        );
        let sidecar = Self { record, run_id };
        if !sidecar.answers() {
            sidecar.spawn(runtime)?;
        }
        Ok(sidecar)
    }

    /// A handle over a recorded sidecar without spawning or probing it
    /// (configuration and tests that only need its endpoint facts).
    ///
    /// # Errors
    ///
    /// Fails when the recorded run id is not a valid bridge run id.
    pub fn from_record(record: SidecarRecordV1) -> Result<Self> {
        let run_id = RunId::new(record.run_id.clone()).context("recorded run id")?;
        Ok(Self { record, run_id })
    }

    fn answers(&self) -> bool {
        TcpStream::connect_timeout(
            &SocketAddrV4::new(Ipv4Addr::LOCALHOST, self.record.port).into(),
            Duration::from_millis(300),
        )
        .is_ok()
    }

    fn spawn(&self, runtime: &BtcRoleRuntime) -> Result<()> {
        let lez = &runtime.config().lez;
        let runtime_file = self.record.state_directory.join("runtime.json");
        let descriptor = serde_json::to_vec_pretty(&runtime.runtime_descriptor())?;
        if runtime_file.exists() {
            fs::write(&runtime_file, &descriptor)?;
        } else {
            write_private_exact(&runtime_file, &descriptor)?;
        }
        let log = OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .open(&self.record.log_file)?;
        let started = log.metadata()?.len();
        let mut command = Command::new(&lez.sidecar_program);
        // The official origin serves no indexer, so whenever the sequencer is
        // not loopback the indexer is pinned back to the loopback profile.
        if lez.sequencer_route != LezSequencerRoute::Local {
            command.args(["--indexer-profile", "local"]);
        }
        let child = command
            .args([
                "--listen-address",
                &format!("127.0.0.1:{}", self.record.port),
                "--node-profile",
                lez.sequencer_route.as_sidecar_profile(),
                "--sequencer-url",
                runtime.lez_sequencer().as_str(),
                "--indexer-url",
                runtime.lez_indexer().as_str(),
                "--run-id",
                self.run_id.as_str(),
                "--runtime-file",
                &runtime_file.to_string_lossy(),
                "--capability-file",
                &self.record.capability_file.to_string_lossy(),
                "--state-directory",
                &self.record.state_directory.to_string_lossy(),
                "--authenticated-transfer-program-id",
                &lez.authenticated_transfer_program_id,
            ])
            // The role's signer: its key file, or the LEZ wallet module holding it.
            .args(match (&lez.signer_key_file, &lez.wallet_signer) {
                (Some(key), _) => vec![
                    "--private-key-file".to_owned(),
                    key.to_string_lossy().into_owned(),
                ],
                (None, Some(wallet)) => vec![
                    "--wallet-signer-socket".to_owned(),
                    wallet.socket.to_string_lossy().into_owned(),
                    "--signer-account-id".to_owned(),
                    wallet.account_id.to_ascii_lowercase(),
                ],
                (None, None) => Vec::new(),
            })
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log))
            .spawn()
            .with_context(|| format!("spawn {}", lez.sidecar_program.display()))?;
        // The pid lets the Node retire this sidecar (`retire`). Recording it
        // is best effort: without it the sidecar only cannot be retired.
        let _ = fs::write(self.pid_file(), format!("{}\n", child.id()));
        // The child is intentionally not waited on: it outlives this call and
        // is reaped by the Node's init (tini) if it exits.
        drop(child);
        let deadline = Instant::now() + READY_TIMEOUT;
        while Instant::now() < deadline {
            if let Ok(bytes) = crate::layout::read_vetted(&self.record.log_file, MAX_LOG_SCAN_BYTES)
            {
                let fresh = &bytes[usize::try_from(started).unwrap_or(0).min(bytes.len())..];
                if fresh
                    .windows(15)
                    .any(|window| window == b"\"event\":\"ready\"")
                    && self.answers()
                {
                    return Ok(());
                }
                if fresh.windows(6).any(|window| window == b"failed") {
                    anyhow::bail!(
                        "sidecar failed to start: {}",
                        String::from_utf8_lossy(fresh).lines().last().unwrap_or("")
                    );
                }
            }
            thread::sleep(Duration::from_millis(100));
        }
        anyhow::bail!("sidecar did not report readiness in time")
    }

    fn pid_file(&self) -> PathBuf {
        self.record.state_directory.join("sidecar.pid")
    }

    /// Stops the swap's sidecar, when one is running that this Node spawned
    /// for this swap: the recorded pid must still run this sidecar program
    /// with this swap's run id and state directory, so a reused pid is
    /// never signalled. The record stays, so `ensure` can start it again.
    ///
    /// Returns whether a sidecar was signalled.
    ///
    /// # Errors
    ///
    /// Fails when the swap's sidecar record cannot be read.
    pub fn retire(layout: &SwapLayout) -> Result<bool> {
        let record_file = layout.root().join("sidecar.json");
        let Ok(bytes) = read_private(&record_file, MAX_RECORD_BYTES) else {
            return Ok(false);
        };
        let sidecar = Self::from_record(
            serde_json::from_slice::<SidecarRecordV1>(&bytes).context("sidecar record")?,
        )?;
        let Some(pid) = fs::read_to_string(sidecar.pid_file())
            .ok()
            .and_then(|text| text.trim().parse::<i32>().ok())
            .and_then(rustix::process::Pid::from_raw)
        else {
            return Ok(false);
        };
        let Ok(output) = Command::new("ps")
            .args(["-o", "command=", "-p", &pid.as_raw_nonzero().to_string()])
            .stdin(Stdio::null())
            .output()
        else {
            return Ok(false);
        };
        let command = String::from_utf8_lossy(&output.stdout);
        let state = sidecar.record.state_directory.to_string_lossy();
        if !command.contains(&format!("--run-id {}", sidecar.run_id.as_str()))
            || !command.contains(&format!("--state-directory {state}"))
        {
            return Ok(false);
        }
        let signalled = rustix::process::kill_process(pid, rustix::process::Signal::TERM).is_ok();
        let _ = fs::remove_file(sidecar.pid_file());
        Ok(signalled)
    }

    #[must_use]
    pub fn endpoint(&self) -> String {
        format!("http://127.0.0.1:{}/", self.record.port)
    }

    pub const fn run_id(&self) -> &RunId {
        &self.run_id
    }

    #[must_use]
    pub fn capability_file(&self) -> &std::path::Path {
        &self.record.capability_file
    }

    /// A client bound to this sidecar.
    ///
    /// # Errors
    ///
    /// Fails when the capability file is unreadable.
    pub fn client(&self, runtime: &BtcRoleRuntime) -> Result<LezSidecar> {
        LezSidecar::connect_to(
            runtime,
            &self.endpoint(),
            &self.record.capability_file,
            self.run_id.clone(),
        )
    }
}

/// Serializes port allocation in this Node: the free-port probe and the
/// write of the record that reserves the port are one step for every pair's
/// swaps (BTC, ZEC and XMR take their own round locks), so two activations
/// never record the same port.
static PORT_ALLOCATION: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Picks a free port and writes the sidecar record that reserves it, under
/// [`PORT_ALLOCATION`]. `make` builds the record for the port (and the files
/// it names).
fn record_with_free_port(
    swaps_root: &std::path::Path,
    port_base: u16,
    port_count: u16,
    record_file: &std::path::Path,
    make: impl FnOnce(u16) -> Result<SidecarRecordV1>,
) -> Result<SidecarRecordV1> {
    let _allocating = PORT_ALLOCATION
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let record = make(free_port(swaps_root, port_base, port_count)?)?;
    write_private_exact(record_file, &serde_json::to_vec_pretty(&record)?)?;
    Ok(record)
}

/// A loopback port in the role's range that no other swap recorded and nothing
/// listens on. Recorded ports stay reserved for their swap across restarts,
/// so a respawned sidecar never lands on a port another swap's actor expects.
fn free_port(swaps_root: &std::path::Path, port_base: u16, port_count: u16) -> Result<u16> {
    let recorded = recorded_ports(swaps_root);
    for offset in 0..port_count {
        let port = port_base.saturating_add(offset);
        if recorded.contains(&port) {
            continue;
        }
        if std::net::TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)).is_ok() {
            return Ok(port);
        }
    }
    anyhow::bail!("no free loopback port for a swap sidecar")
}

fn recorded_ports(swaps_root: &std::path::Path) -> Vec<u16> {
    let Ok(entries) = fs::read_dir(swaps_root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let bytes = read_private(&entry.path().join("sidecar.json"), MAX_RECORD_BYTES).ok()?;
            serde_json::from_slice::<SidecarRecordV1>(&bytes)
                .ok()
                .map(|record| record.port)
        })
        .collect()
}

/// Every swap directory under `swaps_root` that recorded a sidecar, for
/// keep-alive loops after a Node restart.
///
/// # Errors
///
/// Fails when the swaps root cannot be read.
pub fn recorded_swaps(swaps_root: &std::path::Path) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    for entry in fs::read_dir(swaps_root)?.flatten() {
        if entry.path().join("sidecar.json").is_file() {
            found.push(entry.path());
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt as _, sync::Barrier};

    use super::{SidecarRecordV1, record_with_free_port};

    /// Two activations of different pairs used to probe the same free port
    /// and record it both: each pair takes its own round lock, and the
    /// probe and the record write were separate steps. The pause stands for
    /// the capability and state directory written in between.
    #[test]
    fn concurrent_activations_never_record_the_same_port() {
        const SWAPS: usize = 6;
        let run = tempfile::tempdir().unwrap();
        let swaps = fs::canonicalize(run.path()).unwrap();
        let swaps = &swaps;
        let base = 30_000 + u16::try_from(std::process::id() % 1_000).unwrap() * 20;
        let barrier = Barrier::new(SWAPS);
        let ports: Vec<u16> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..SWAPS)
                .map(|index| {
                    let barrier = &barrier;
                    scope.spawn(move || {
                        let swap = swaps.join(format!("swap-reservation-{index:02}"));
                        fs::create_dir(&swap).unwrap();
                        fs::set_permissions(&swap, fs::Permissions::from_mode(0o700)).unwrap();
                        barrier.wait();
                        record_with_free_port(swaps, base, 16, &swap.join("sidecar.json"), |port| {
                            std::thread::sleep(std::time::Duration::from_millis(50));
                            Ok(SidecarRecordV1 {
                                schema_version: 1,
                                port,
                                run_id: format!("swap-{index}"),
                                capability_file: swap.join("sidecar-capability"),
                                state_directory: swap.join("sidecar-state"),
                                log_file: swap.join("sidecar.log"),
                            })
                        })
                        .unwrap()
                        .port
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect()
        });
        let mut distinct = ports.clone();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(distinct.len(), SWAPS, "{ports:?}");
    }
}
