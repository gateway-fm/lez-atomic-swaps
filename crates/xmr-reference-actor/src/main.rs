use clap::Parser as _;
use xmr_reference_actor::{Cli, execute};

fn main() {
    if let Err(error) = execute(Cli::parse()) {
        eprintln!("{error:#}");
        std::process::exit(2);
    }
}
