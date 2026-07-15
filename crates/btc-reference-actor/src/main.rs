use btc_reference_actor::{ActorCli, ActorConfig, execute_actor_command};
use clap::Parser as _;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let cli = ActorCli::parse();
    let config = ActorConfig::load_private(cli.config).unwrap_or_else(|error| exit_with(error));
    let output = execute_actor_command(&config, cli.command)
        .await
        .unwrap_or_else(|error| exit_with(error));
    let json =
        serde_json::to_string(&output).unwrap_or_else(|_| exit_with("actor output is unavailable"));
    println!("{json}");
}

fn exit_with(message: impl std::fmt::Display) -> ! {
    eprintln!("{message}");
    std::process::exit(2);
}
