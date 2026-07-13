use clap::Parser as _;
use zec_reference_actor::{ActorCli, ActorConfig};

fn main() {
    let cli = ActorCli::parse();
    if let Err(error) = ActorConfig::load_private(cli.config) {
        eprintln!("{error}");
        std::process::exit(2);
    }
    println!("reference actor boundary validated for {:?}", cli.command);
}
