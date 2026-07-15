use clap::Parser as _;
use lez_adaptor_role_runner::{Cli, execute};

fn main() {
    let cli = Cli::parse();
    if let Err(error) = execute(&cli) {
        eprintln!("{error}");
        std::process::exit(2);
    }
}
