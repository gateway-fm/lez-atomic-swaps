use std::{fmt, path::PathBuf};

use clap::Parser;
use zec_reference_actor::{
    LocalPocLezSignerFiles, LocalPocZcashKeyFiles, provision_local_v0_2_corridor_with_material,
};

#[derive(Clone, Parser)]
#[command(about = "Provision one private deterministic LEZ-v0.2/Zebra corridor fixture")]
struct Arguments {
    /// Owner-private JSON containing non-secret, manifest-derived runtime facts.
    #[arg(long, value_name = "PRIVATE_JSON")]
    spec_file: PathBuf,
    /// New absolute directory that will contain the isolated actor inputs.
    #[arg(long, value_name = "NEW_PRIVATE_DIRECTORY")]
    output_root: PathBuf,
    /// Canonical owner-private Maker signer created before the fresh LEZ genesis.
    #[arg(
        long,
        value_name = "PRIVATE_KEY",
        requires = "taker_lez_signer_key_file"
    )]
    maker_lez_signer_key_file: Option<PathBuf>,
    /// Canonical owner-private Taker signer created before the fresh LEZ genesis.
    #[arg(
        long,
        value_name = "PRIVATE_KEY",
        requires = "maker_lez_signer_key_file"
    )]
    taker_lez_signer_key_file: Option<PathBuf>,
    /// Owner-private Maker Zcash key (32 bytes or 64 hex); the test-lane key when absent.
    #[arg(long, value_name = "PRIVATE_KEY", requires = "taker_zcash_key_file")]
    maker_zcash_key_file: Option<PathBuf>,
    /// Owner-private Taker Zcash key (32 bytes or 64 hex); the test-lane key when absent.
    #[arg(long, value_name = "PRIVATE_KEY", requires = "maker_zcash_key_file")]
    taker_zcash_key_file: Option<PathBuf>,
}

impl fmt::Debug for Arguments {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Arguments")
            .field("spec_file", &"[REDACTED]")
            .field("output_root", &"[REDACTED]")
            .field("maker_lez_signer_key_file", &"[REDACTED]")
            .field("taker_lez_signer_key_file", &"[REDACTED]")
            .field("maker_zcash_key_file", &"[REDACTED]")
            .field("taker_zcash_key_file", &"[REDACTED]")
            .finish()
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let arguments = Arguments::parse();
    let signer_files = match (
        arguments.maker_lez_signer_key_file,
        arguments.taker_lez_signer_key_file,
    ) {
        (Some(maker), Some(taker)) => Some(LocalPocLezSignerFiles::new(maker, taker)),
        (None, None) => None,
        _ => unreachable!("clap requires a complete LEZ signer pair"),
    };
    let zcash_key_files = match (
        arguments.maker_zcash_key_file,
        arguments.taker_zcash_key_file,
    ) {
        (Some(maker), Some(taker)) => Some(LocalPocZcashKeyFiles::new(maker, taker)),
        (None, None) => None,
        _ => unreachable!("clap requires a complete Zcash key pair"),
    };
    let summary = provision_local_v0_2_corridor_with_material(
        &arguments.spec_file,
        &arguments.output_root,
        signer_files.as_ref(),
        zcash_key_files.as_ref(),
    )
    .await?;
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser as _;

    use super::Arguments;

    #[test]
    fn fresh_signer_cli_requires_a_complete_pair() {
        assert!(
            Arguments::try_parse_from([
                "zec-local-poc-provision",
                "--spec-file",
                "/private/spec.json",
                "--output-root",
                "/private/output",
                "--maker-lez-signer-key-file",
                "/private/maker.key",
            ])
            .is_err()
        );
    }

    #[test]
    fn fresh_signer_cli_debug_redacts_private_paths() {
        let arguments = Arguments::try_parse_from([
            "zec-local-poc-provision",
            "--spec-file",
            "/private/spec.json",
            "--output-root",
            "/private/output",
            "--maker-lez-signer-key-file",
            "/private/maker.key",
            "--taker-lez-signer-key-file",
            "/private/taker.key",
            "--maker-zcash-key-file",
            "/private/maker-zcash.key",
            "--taker-zcash-key-file",
            "/private/taker-zcash.key",
        ])
        .unwrap();
        let debug = format!("{arguments:?}");
        for secret_path in [
            "/private/spec.json",
            "/private/output",
            "/private/maker.key",
            "/private/taker.key",
            "/private/maker-zcash.key",
            "/private/taker-zcash.key",
        ] {
            assert!(!debug.contains(secret_path));
        }
    }
}
