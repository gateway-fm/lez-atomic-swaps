use std::{path::PathBuf, process::ExitCode};

use clap::Parser;
use lez_v0_2_sidecar::actor_identity::provision_local_actor_identity;

#[derive(Debug, Parser)]
#[command(about = "Provision a fresh local-only official LEZ v0.2 actor identity")]
struct Arguments {
    #[arg(long, value_name = "NEW_DIRECTORY")]
    output_directory: PathBuf,
}

fn main() -> ExitCode {
    let arguments = Arguments::parse();
    match provision_local_actor_identity(&arguments.output_directory) {
        Ok(identity) => {
            let Ok(public_json) = serde_json::to_string(&identity) else {
                eprintln!("error: could not serialize public actor identity");
                return ExitCode::from(2);
            };
            println!("{public_json}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(2)
        }
    }
}
