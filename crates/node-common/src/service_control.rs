use std::{
    ffi::OsStr,
    io::{self, Read as _},
    process::{Child, Command, Output, Stdio},
    thread,
    time::Duration,
};

use serde::Serialize;
use thiserror::Error;
use wait_timeout::ChildExt as _;

const SYSTEMCTL_PROGRAM: &str = "/usr/bin/systemctl";
const MAKER_UNIT: &str = "lez-maker-node.service";
const TAKER_UNIT: &str = "lez-taker-node.service";
const MAXIMUM_STATE_BYTES: usize = 32;
const SYSTEMCTL_TIMEOUT: Duration = Duration::from_secs(30);

/// Fixed lifecycle action exposed symmetrically by both operator CLIs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeServiceAction {
    /// Start the packaged Maker service.
    Start,
    /// Stop the packaged Maker service.
    Stop,
}

impl NodeServiceAction {
    const fn name(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
        }
    }

    const fn expected_state(self) -> &'static str {
        match self {
            Self::Start => "active",
            Self::Stop => "inactive",
        }
    }
}

/// Secret-free result after systemd confirms the requested stable state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct NodeServiceControlV1 {
    schema_version: u16,
    action: &'static str,
    unit: &'static str,
    active_state: Box<str>,
}

/// Fail-closed service-manager error without raw subprocess output.
#[derive(Debug, Error)]
pub enum NodeServiceControlError {
    /// The fixed systemctl executable could not be launched.
    #[error("cannot execute the fixed systemctl program")]
    SystemctlUnavailable(#[source] io::Error),
    /// The fixed lifecycle operation did not complete within its deadline.
    #[error("systemctl {operation} timed out; service state is uncertain")]
    SystemctlTimeout {
        /// Fixed action or state-query operation.
        operation: &'static str,
    },
    /// systemd rejected the requested lifecycle action.
    #[error("systemctl {action} failed with exit status {status}")]
    ActionFailed {
        /// Fixed action name.
        action: &'static str,
        /// Numeric child exit status, or -1 when terminated by signal.
        status: i32,
    },
    /// systemd could not report the unit state after the action.
    #[error("systemctl state query failed with exit status {status}")]
    StateQueryFailed {
        /// Numeric child exit status, or -1 when terminated by signal.
        status: i32,
    },
    /// `ActiveState` output was not one bounded lowercase token.
    #[error("systemctl returned an invalid active state")]
    InvalidState,
    /// The action completed but the fixed unit did not reach its exact state.
    #[error("systemctl {action} completed without reaching {expected}")]
    UnexpectedState {
        /// Fixed action name.
        action: &'static str,
        /// Required postcondition.
        expected: &'static str,
    },
}

/// Starts or stops only the packaged Maker Node and verifies its stable state.
///
/// This function invokes one absolute executable directly. It never uses a
/// shell, embedded privilege elevation, a caller-selected unit, or the Maker
/// daemon RPC socket.
///
/// # Errors
///
/// Fails without emitting raw subprocess output when systemctl is unavailable,
/// rejects the action, returns malformed state, or misses the exact
/// action-specific postcondition.
pub fn control_maker_service(
    action: NodeServiceAction,
) -> Result<NodeServiceControlV1, NodeServiceControlError> {
    control_maker_service_with(&ProcessSystemctl, action)
}

/// Starts or stops only the packaged Taker Node and verifies its stable state.
///
/// # Errors
///
/// Fails without emitting raw subprocess output when systemctl is unavailable,
/// rejects the action, returns malformed state, or misses the exact
/// action-specific postcondition.
pub fn control_taker_service(
    action: NodeServiceAction,
) -> Result<NodeServiceControlV1, NodeServiceControlError> {
    control_node_service_with(&ProcessSystemctl, action, TAKER_UNIT)
}

trait SystemctlRunner {
    fn output(&self, arguments: &[&OsStr], capture_stdout: bool) -> io::Result<Output>;
}

struct ProcessSystemctl;

impl SystemctlRunner for ProcessSystemctl {
    fn output(&self, arguments: &[&OsStr], capture_stdout: bool) -> io::Result<Output> {
        let mut command = Command::new(SYSTEMCTL_PROGRAM);
        command
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(if capture_stdout {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stderr(Stdio::null());
        let mut child = command.spawn()?;
        let reader = child.stdout.take().map(|stdout| {
            thread::spawn(move || {
                let mut bytes = Vec::with_capacity(MAXIMUM_STATE_BYTES + 1);
                stdout
                    .take((MAXIMUM_STATE_BYTES + 1) as u64)
                    .read_to_end(&mut bytes)?;
                Ok::<_, io::Error>(bytes)
            })
        });
        let status = match child.wait_timeout(SYSTEMCTL_TIMEOUT) {
            Ok(Some(status)) => Ok(status),
            Ok(None) => {
                kill_and_reap(&mut child);
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "fixed systemctl process timed out",
                ))
            }
            Err(source) => {
                kill_and_reap(&mut child);
                Err(source)
            }
        };
        let stdout = match reader {
            Some(reader) => reader
                .join()
                .map_err(|_| io::Error::other("systemctl output reader panicked"))??,
            None => Vec::new(),
        };
        Ok(Output {
            status: status?,
            stdout,
            stderr: Vec::new(),
        })
    }
}

fn kill_and_reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn control_maker_service_with(
    runner: &impl SystemctlRunner,
    action: NodeServiceAction,
) -> Result<NodeServiceControlV1, NodeServiceControlError> {
    control_node_service_with(runner, action, MAKER_UNIT)
}

fn control_node_service_with(
    runner: &impl SystemctlRunner,
    action: NodeServiceAction,
    unit: &'static str,
) -> Result<NodeServiceControlV1, NodeServiceControlError> {
    let action_arguments = [
        OsStr::new("--no-pager"),
        OsStr::new("--no-ask-password"),
        OsStr::new(action.name()),
        OsStr::new(unit),
    ];
    let action_output = runner
        .output(&action_arguments, false)
        .map_err(|error| map_runner_error(error, action.name()))?;
    if !action_output.status.success() {
        return Err(NodeServiceControlError::ActionFailed {
            action: action.name(),
            status: action_output.status.code().unwrap_or(-1),
        });
    }

    let state_arguments = [
        OsStr::new("--no-pager"),
        OsStr::new("--no-ask-password"),
        OsStr::new("show"),
        OsStr::new(unit),
        OsStr::new("--property=ActiveState"),
        OsStr::new("--value"),
    ];
    let state_output = runner
        .output(&state_arguments, true)
        .map_err(|error| map_runner_error(error, "show"))?;
    if !state_output.status.success() {
        return Err(NodeServiceControlError::StateQueryFailed {
            status: state_output.status.code().unwrap_or(-1),
        });
    }
    let state = parse_active_state(&state_output.stdout)?;
    if state != action.expected_state() {
        return Err(NodeServiceControlError::UnexpectedState {
            action: action.name(),
            expected: action.expected_state(),
        });
    }
    Ok(NodeServiceControlV1 {
        schema_version: 1,
        action: action.name(),
        unit,
        active_state: state.into(),
    })
}

fn map_runner_error(error: io::Error, operation: &'static str) -> NodeServiceControlError {
    if error.kind() == io::ErrorKind::TimedOut {
        NodeServiceControlError::SystemctlTimeout { operation }
    } else {
        NodeServiceControlError::SystemctlUnavailable(error)
    }
}

fn parse_active_state(bytes: &[u8]) -> Result<&str, NodeServiceControlError> {
    let value = std::str::from_utf8(bytes)
        .map_err(|_| NodeServiceControlError::InvalidState)?
        .trim();
    if value.is_empty()
        || value.len() > MAXIMUM_STATE_BYTES
        || !value.bytes().all(|byte| byte.is_ascii_lowercase())
    {
        return Err(NodeServiceControlError::InvalidState);
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, collections::VecDeque, os::unix::process::ExitStatusExt};

    use super::*;

    type RecordedCall = (Vec<String>, bool);

    struct FakeSystemctl {
        outputs: RefCell<VecDeque<io::Result<Output>>>,
        calls: RefCell<Vec<RecordedCall>>,
    }

    impl FakeSystemctl {
        fn new(outputs: impl IntoIterator<Item = Output>) -> Self {
            Self::with_results(outputs.into_iter().map(Ok))
        }

        fn with_results(outputs: impl IntoIterator<Item = io::Result<Output>>) -> Self {
            Self {
                outputs: RefCell::new(outputs.into_iter().collect()),
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl SystemctlRunner for FakeSystemctl {
        fn output(&self, arguments: &[&OsStr], capture_stdout: bool) -> io::Result<Output> {
            self.calls.borrow_mut().push((
                arguments
                    .iter()
                    .map(|argument| argument.to_string_lossy().into_owned())
                    .collect(),
                capture_stdout,
            ));
            self.outputs.borrow_mut().pop_front().unwrap()
        }
    }

    fn output(status: i32, stdout: &[u8], stderr: &[u8]) -> Output {
        Output {
            status: std::process::ExitStatus::from_raw(status << 8),
            stdout: stdout.to_vec(),
            stderr: stderr.to_vec(),
        }
    }

    #[test]
    fn fixed_system_commands_reach_exact_postconditions() {
        for unit in [MAKER_UNIT, TAKER_UNIT] {
            for (action, state, expected_action) in [
                (NodeServiceAction::Start, b"active\n".as_slice(), "start"),
                (NodeServiceAction::Stop, b"inactive\n".as_slice(), "stop"),
            ] {
                let runner = FakeSystemctl::new([output(0, b"", b""), output(0, state, b"")]);
                let result = control_node_service_with(&runner, action, unit).unwrap();
                assert_eq!(result.active_state.as_ref(), action.expected_state());
                assert_eq!(
                    serde_json::to_value(result).unwrap(),
                    serde_json::json!({
                        "schema_version": 1,
                        "action": expected_action,
                        "unit": unit,
                        "active_state": action.expected_state(),
                    })
                );
                assert_eq!(
                    runner.calls.into_inner(),
                    vec![
                        (
                            vec![
                                "--no-pager".into(),
                                "--no-ask-password".into(),
                                expected_action.into(),
                                unit.into(),
                            ],
                            false,
                        ),
                        (
                            vec![
                                "--no-pager".into(),
                                "--no-ask-password".into(),
                                "show".into(),
                                unit.into(),
                                "--property=ActiveState".into(),
                                "--value".into(),
                            ],
                            true,
                        ),
                    ]
                );
            }
        }
    }

    #[test]
    fn failures_are_bounded_fail_closed_and_never_expose_output() {
        let runner = FakeSystemctl::new([output(7, b"secret stdout", b"secret stderr")]);
        let error = control_maker_service_with(&runner, NodeServiceAction::Start).unwrap_err();
        let display = error.to_string();
        assert!(display.contains("exit status 7"));
        assert!(!display.contains("secret"));

        let runner = FakeSystemctl::new([
            output(0, b"", b""),
            output(9, b"secret state", b"secret query"),
        ]);
        let error = control_maker_service_with(&runner, NodeServiceAction::Stop).unwrap_err();
        assert!(error.to_string().contains("exit status 9"));
        assert!(!error.to_string().contains("secret"));

        let oversized_state = vec![b'a'; MAXIMUM_STATE_BYTES + 1];
        for state in [
            b"activating\n".as_slice(),
            b"active pending\n".as_slice(),
            b"ACTIVE\n".as_slice(),
            oversized_state.as_slice(),
            b"\xff\n".as_slice(),
        ] {
            let runner = FakeSystemctl::new([output(0, b"", b""), output(0, state, b"")]);
            assert!(control_maker_service_with(&runner, NodeServiceAction::Start).is_err());
        }
    }

    #[test]
    fn action_and_state_query_timeouts_report_uncertain_state() {
        let runner = FakeSystemctl::with_results([Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "secret action detail",
        ))]);
        let error = control_maker_service_with(&runner, NodeServiceAction::Start).unwrap_err();
        assert!(matches!(
            error,
            NodeServiceControlError::SystemctlTimeout { operation: "start" }
        ));
        assert!(!error.to_string().contains("secret"));

        let runner = FakeSystemctl::with_results([
            Ok(output(0, b"", b"")),
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "secret query detail",
            )),
        ]);
        let error = control_maker_service_with(&runner, NodeServiceAction::Stop).unwrap_err();
        assert!(matches!(
            error,
            NodeServiceControlError::SystemctlTimeout { operation: "show" }
        ));
        assert!(!error.to_string().contains("secret"));
    }
}
