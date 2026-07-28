use btc_reference_actor::{ActorCli, ActorConfig, execute_actor_command};
use clap::Parser as _;

fn main() {
    let cli = ActorCli::parse();
    let config = match (cli.config.as_ref(), cli.config_fd) {
        (Some(path), None) => {
            ActorConfig::load_private(path).unwrap_or_else(|error| exit_with(error))
        }
        (None, Some(fd)) => ActorConfig::load_private_fd(fd)
            .unwrap_or_else(|_| exit_with("actor configuration is unavailable")),
        (Some(_), Some(_)) | (None, None) => exit_with("actor configuration is unavailable"),
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|_| exit_with("actor runtime is unavailable"));
    let output = runtime
        .block_on(execute_actor_command(&config, cli.command))
        .unwrap_or_else(|error| exit_with(error));
    let json =
        serde_json::to_string(&output).unwrap_or_else(|_| exit_with("actor output is unavailable"));
    println!("{json}");
}

fn exit_with(message: impl std::fmt::Display) -> ! {
    eprintln!("{message}");
    std::process::exit(2);
}
