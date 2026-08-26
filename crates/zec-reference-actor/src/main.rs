use clap::Parser as _;
#[cfg(feature = "test-crash-hooks")]
use zec_reference_actor::ActorRole;
#[cfg(feature = "test-crash-hooks")]
use zec_reference_actor::arm_test_crash_hook;
use zec_reference_actor::{ActorCli, ActorConfig, execute_actor_command};

fn main() {
    let cli = ActorCli::parse();
    let config = match (cli.config.as_ref(), cli.config_fd) {
        (Some(path), None) => ActorConfig::load_private(path),
        (None, Some(fd)) => ActorConfig::load_private_fd(fd),
        (Some(_), Some(_)) | (None, None) => exit_with("actor configuration is unavailable"),
    }
    .unwrap_or_else(|_| exit_with("actor configuration is unavailable"));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|_| exit_with("actor runtime is unavailable"));
    let output = runtime
        .block_on(execute_actor_command(&config, cli.command))
        .unwrap_or_else(|error| exit_with(error));
    let json =
        serde_json::to_string(&output).unwrap_or_else(|_| exit_with("actor output is unavailable"));
    pause_after_submitted_effect_if_armed(&config, &json);
    println!("{json}");
}

#[cfg(feature = "test-crash-hooks")]
fn pause_after_submitted_effect_if_armed(config: &ActorConfig, output: &str) {
    let (Some(operation), Some(marker)) = (
        std::env::var_os("LEZ_ACTOR_TEST_PAUSE_AFTER_SUBMITTED"),
        std::env::var_os("LEZ_ACTOR_TEST_PAUSE_MARKER"),
    ) else {
        return;
    };
    let operation = operation
        .to_str()
        .unwrap_or_else(|| exit_with("test crash hook is unavailable"));
    let role = match config.role() {
        ActorRole::Maker => "maker",
        ActorRole::Taker => "taker",
    };
    if arm_test_crash_hook(
        operation,
        marker.as_ref(),
        config.swap_id().as_str(),
        role,
        output,
    )
    .unwrap_or_else(|_| exit_with("test crash hook is unavailable"))
    {
        loop {
            std::thread::park();
        }
    }
}

#[cfg(not(feature = "test-crash-hooks"))]
fn pause_after_submitted_effect_if_armed(_config: &ActorConfig, _output: &str) {}

fn exit_with(message: impl std::fmt::Display) -> ! {
    eprintln!("{message}");
    std::process::exit(2);
}
