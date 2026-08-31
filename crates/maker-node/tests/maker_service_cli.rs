use std::process::Command;

#[path = "support/cross_role_binary.rs"]
mod cross_role;

#[test]
fn maker_cli_exposes_fixed_service_start_and_stop_without_daemon_rpc() {
    let help = maker(&["--help"]);
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    assert!(help.contains("start"));
    assert!(help.contains("stop"));

    for action in ["start", "stop"] {
        let action_help = maker(&[action, "--help"]);
        assert!(action_help.status.success(), "{action} help failed");
        let stdout = String::from_utf8(action_help.stdout).unwrap();
        assert!(stdout.contains(&format!("Usage: lez-maker-cli {action}")));
        assert!(!stdout.contains("--scope"));
        assert!(!stdout.contains("--unit"));
        assert!(!stdout.contains("sudo"));
        assert!(!stdout.contains("--socket"));

        for forbidden in ["--scope", "--unit", "--socket"] {
            let rejected = maker(&[action, forbidden, "attacker-selected"]);
            assert_eq!(rejected.status.code(), Some(2), "{forbidden} was accepted");
        }
    }
}

fn maker(arguments: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_lez-maker-cli"))
        .args(arguments)
        .output()
        .expect("run Maker CLI")
}

#[test]
fn role_clis_share_socket_health_and_fixed_service_controls() {
    for (role, executable, socket) in [
        (
            "Maker",
            std::path::PathBuf::from(env!("CARGO_BIN_EXE_lez-maker-cli")),
            "/run/lez/maker/node.sock",
        ),
        (
            "Taker",
            cross_role::workspace_binary("lez-taker-cli"),
            "/run/lez/taker/node.sock",
        ),
    ] {
        let help = Command::new(&executable).arg("--help").output().unwrap();
        assert!(help.status.success(), "{role} CLI help failed");
        let help = String::from_utf8(help.stdout).unwrap();
        for token in ["--socket", socket, "health", "start", "stop"] {
            assert!(help.contains(token), "{role} CLI help omits {token}");
        }
        for action in ["health", "start", "stop"] {
            let action_help = Command::new(&executable)
                .args([action, "--help"])
                .output()
                .unwrap();
            assert!(action_help.status.success(), "{role} {action} help failed");
        }
    }
}
