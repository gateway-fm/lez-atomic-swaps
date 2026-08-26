use std::process::Command;

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
        assert!(stdout.contains(&format!("Usage: lez-maker {action}")));
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
    Command::new(env!("CARGO_BIN_EXE_lez-maker"))
        .args(arguments)
        .output()
        .expect("run Maker CLI")
}
