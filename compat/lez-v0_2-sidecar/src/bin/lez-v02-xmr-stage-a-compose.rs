//! Actual-local, read-only M4 unsigned Stage-A composer.

#![forbid(unsafe_code)]

#[cfg(not(target_os = "linux"))]
compile_error!("the M4 Stage-A composer requires Linux file-safety semantics");

use std::path::PathBuf;

use anyhow::{Context as _, Result, ensure};
use clap::Parser;
use lez_v0_2_sidecar::{
    ActualLocalM4StageAConfig, M4StageAParameters, compose_m4_stage_a_actual_local,
};

/// Compose one canonical unsigned M4 Stage A without submitting to either chain.
#[derive(Clone, Debug, Parser)]
#[command(version, about)]
struct Arguments {
    /// Literal-loopback official LEZ v0.2 sequencer root URL.
    #[arg(long)]
    sequencer_url: String,
    /// Literal-loopback official LEZ v0.2 finalized-indexer root URL.
    #[arg(long)]
    indexer_url: String,
    /// Literal-loopback Digest-authenticated official monerod root URL.
    #[arg(long)]
    monero_daemon_url: String,
    /// Owner-only file containing the Monero RPC username.
    #[arg(long)]
    monero_rpc_username_file: PathBuf,
    /// Owner-only file containing the Monero RPC password.
    #[arg(long)]
    monero_rpc_password_file: PathBuf,
    /// Canonical public Maker role packet.
    #[arg(long)]
    maker_public_packet: PathBuf,
    /// Canonical public Taker role packet.
    #[arg(long)]
    taker_public_packet: PathBuf,
    /// New canonical unsigned Stage-A wire; never overwritten.
    #[arg(long)]
    output_unsigned_stage_a: PathBuf,
    /// Exact lowercase-hex 32-byte swap identity.
    #[arg(long)]
    swap_id: String,
    /// Exact nonzero Monero principal in piconero.
    #[arg(long)]
    monero_amount_piconero: u64,
    /// Exact nonzero LEZ principal.
    #[arg(long)]
    lez_amount: u128,
    /// Latest whole-second LEZ consensus timestamp when Maker may fund Monero.
    #[arg(long)]
    maker_xmr_funding_cutoff_ms: u64,
    /// Earliest whole-second signed-refund LEZ consensus timestamp.
    #[arg(long)]
    refund_at_ms: u64,
    /// Earliest whole-second punishment LEZ consensus timestamp.
    #[arg(long)]
    punish_at_ms: u64,
}

#[tokio::main]
async fn main() {
    if let Err(error) = execute(Arguments::parse()).await {
        eprintln!("M4 unsigned Stage-A composition failed: {error:#}");
        std::process::exit(1);
    }
}

async fn execute(arguments: Arguments) -> Result<()> {
    let swap_id = decode_hex32(&arguments.swap_id).context("swap ID is invalid")?;
    ensure!(swap_id != [0; 32], "swap ID must not be zero");
    let config = ActualLocalM4StageAConfig {
        sequencer_url: arguments.sequencer_url,
        indexer_url: arguments.indexer_url,
        monero_daemon_url: arguments.monero_daemon_url,
        monero_rpc_username_file: arguments.monero_rpc_username_file,
        monero_rpc_password_file: arguments.monero_rpc_password_file,
        maker_public_packet: arguments.maker_public_packet,
        taker_public_packet: arguments.taker_public_packet,
        output_unsigned_stage_a: arguments.output_unsigned_stage_a,
        parameters: M4StageAParameters::new(
            swap_id,
            arguments.monero_amount_piconero,
            arguments.lez_amount,
            arguments.maker_xmr_funding_cutoff_ms,
            arguments.refund_at_ms,
            arguments.punish_at_ms,
        ),
    };
    let output = config.output_unsigned_stage_a.clone();
    let receipt = compose_m4_stage_a_actual_local(&config).await?;
    println!(
        "{{\"agreement_commitment\":\"{}\",\"monero_genesis_hash\":\"{}\",\"lez_genesis_hash\":\"{}\",\"lez_channel_id\":\"{}\",\"lez_finalized_block_hash\":\"{}\",\"lez_finalized_height\":{},\"wire_bytes\":{}}}",
        hex::encode(receipt.agreement_commitment()),
        hex::encode(receipt.monero_genesis_hash()),
        hex::encode(receipt.lez_genesis_hash()),
        hex::encode(receipt.lez_channel_id()),
        hex::encode(receipt.lez_finalized_block_hash()),
        receipt.lez_finalized_height(),
        receipt.wire_bytes(),
    );
    eprintln!(
        "canonical unsigned Stage A written create-new to {}",
        output.display()
    );
    Ok(())
}

fn decode_hex32(value: &str) -> Result<[u8; 32]> {
    ensure!(
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "expected exactly 64 lowercase hexadecimal characters"
    );
    let bytes = hex::decode(value).context("decode lowercase hexadecimal value")?;
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("expected 32 bytes"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swap_id_is_exact_lowercase_hex32() {
        assert_eq!(decode_hex32(&"ab".repeat(32)).unwrap(), [0xab; 32]);
        assert!(decode_hex32(&"AB".repeat(32)).is_err());
        assert!(decode_hex32("00").is_err());
    }
}
