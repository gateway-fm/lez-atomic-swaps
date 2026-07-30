//! One-shot supervised Maker entrypoint for the validated XMR pre-effect boundary.

use clap::{Parser as _, Subcommand};
use lez_swap_store::MAKER_ACTOR_CONFIG_FD;
use serde::Serialize;
use xmr_reference_actor::{
    XMR_MAKER_ACTOR_ABI_V1, XMR_MAKER_ACTOR_NEXT_ACTION, XMR_MAKER_ACTOR_PROGRAM_ID,
    load_validated_xmr_maker_authority_fd,
};

#[derive(Debug, clap::Parser)]
#[command(about = "One-shot supervised LEZ/XMR Maker authority validator")]
struct Arguments {
    #[arg(long, value_name = "FD", value_parser = parse_config_fd)]
    config_fd: i32,
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, Debug, Subcommand)]
enum Command {
    /// Validate the complete immutable pre-effect authority and report its blocked state.
    Status,
}

#[derive(Serialize)]
struct StatusOutput {
    schema_version: u16,
    actor_program: &'static str,
    actor_abi: &'static str,
    role: &'static str,
    state: &'static str,
    phase: &'static str,
    revision: u64,
    next_action: &'static str,
    chain_effect_executed: bool,
}

fn main() {
    let arguments = Arguments::parse();
    let result = match arguments.command {
        Command::Status => status(arguments.config_fd),
    };
    match result {
        Ok(output) => match serde_json::to_string(&output) {
            Ok(json) => println!("{json}"),
            Err(_) => exit_with("XMR Maker actor output is unavailable"),
        },
        Err(()) => exit_with("XMR Maker actor authority is unavailable"),
    }
}

fn status(config_fd: i32) -> Result<StatusOutput, ()> {
    let authority = load_validated_xmr_maker_authority_fd(config_fd).map_err(|_| ())?;
    let _ = (
        authority.swap_id(),
        authority.agreement_commitment(),
        authority.activation_commitment(),
        authority.state_database(),
    );
    Ok(StatusOutput {
        schema_version: 1,
        actor_program: XMR_MAKER_ACTOR_PROGRAM_ID,
        actor_abi: XMR_MAKER_ACTOR_ABI_V1,
        role: "maker",
        state: "active",
        phase: "offered",
        revision: 0,
        next_action: XMR_MAKER_ACTOR_NEXT_ACTION,
        chain_effect_executed: false,
    })
}

fn parse_config_fd(value: &str) -> Result<i32, String> {
    let fd = value
        .parse::<i32>()
        .map_err(|_| "invalid config descriptor".to_owned())?;
    if fd == MAKER_ACTOR_CONFIG_FD {
        Ok(fd)
    } else {
        Err(format!("config descriptor must be {MAKER_ACTOR_CONFIG_FD}"))
    }
}

fn exit_with(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(2);
}
