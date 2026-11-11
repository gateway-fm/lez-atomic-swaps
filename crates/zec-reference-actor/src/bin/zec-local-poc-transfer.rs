use std::{fmt, path::PathBuf};

use clap::Parser;
use url::Url;
use zec_reference_actor::{
    TestnetZebraCredential, transfer_local_v0_2_regtest, transfer_v0_2_testnet,
};

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum ZcashNetwork {
    /// The deterministic Regtest fixture (loopback, no credential).
    Regtest,
    /// Zcash Testnet on NU7 (public-testnet-v2): a self-hosted Zebra with
    /// `--zebra-cookie-file`, or Tatum's gateway with `--zebra-api-key-file`.
    Test,
}

#[derive(Clone, Parser)]
#[command(
    about = "Move Regtest or Testnet ZEC between two owner-private keys with one plain transparent transaction"
)]
struct Arguments {
    /// Zebra RPC endpoint: loopback, or Tatum's Testnet origin.
    #[arg(long, value_name = "URL")]
    zebra_endpoint: Url,
    /// The chain the transfer is made on.
    #[arg(long, value_enum, default_value_t = ZcashNetwork::Regtest)]
    zcash_network: ZcashNetwork,
    /// Owner-private `.cookie` of a self-hosted Testnet Zebra.
    #[arg(
        long,
        value_name = "PRIVATE_FILE",
        conflicts_with = "zebra_api_key_file"
    )]
    zebra_cookie_file: Option<PathBuf>,
    /// Owner-private Tatum API key file.
    #[arg(long, value_name = "PRIVATE_FILE")]
    zebra_api_key_file: Option<PathBuf>,
    /// Testnet: the lowest height the sender-output search reads.
    #[arg(long, value_name = "HEIGHT", default_value_t = 0)]
    scan_from_height: u32,
    /// Owner-private sender key (must own a mature P2PKH output).
    #[arg(long, value_name = "PRIVATE_KEY")]
    from_zcash_key_file: PathBuf,
    /// Owner-private recipient key.
    #[arg(long, value_name = "PRIVATE_KEY")]
    to_zcash_key_file: PathBuf,
    /// Value to pay the recipient, zatoshis.
    #[arg(long, value_name = "ZATOSHIS")]
    value_zatoshis: u64,
    /// Mature sender outputs to pass over before spending one.
    #[arg(long, value_name = "N", default_value_t = 0)]
    skip: u32,
}

impl fmt::Debug for Arguments {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Arguments")
            .field("zebra_endpoint", &self.zebra_endpoint)
            .field("zcash_network", &self.zcash_network)
            .field(
                "zebra_credential",
                &(self.zebra_cookie_file.is_some() || self.zebra_api_key_file.is_some())
                    .then_some("[REDACTED]"),
            )
            .field("scan_from_height", &self.scan_from_height)
            .field("from_zcash_key_file", &"[REDACTED]")
            .field("to_zcash_key_file", &"[REDACTED]")
            .field("value_zatoshis", &self.value_zatoshis)
            .field("skip", &self.skip)
            .finish()
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let arguments = Arguments::parse();
    let summary = match arguments.zcash_network {
        ZcashNetwork::Regtest => {
            anyhow::ensure!(
                arguments.zebra_cookie_file.is_none() && arguments.zebra_api_key_file.is_none(),
                "the Regtest fixture takes no Zebra credential"
            );
            transfer_local_v0_2_regtest(
                &arguments.zebra_endpoint,
                &arguments.from_zcash_key_file,
                &arguments.to_zcash_key_file,
                arguments.value_zatoshis,
                arguments.skip,
            )
            .await?
        }
        ZcashNetwork::Test => {
            let credential = match (arguments.zebra_cookie_file, arguments.zebra_api_key_file) {
                (Some(cookie), None) => TestnetZebraCredential::SelfHostedCookie(cookie),
                (None, Some(api_key)) => TestnetZebraCredential::TatumApiKey(api_key),
                _ => anyhow::bail!(
                    "a Testnet transfer takes --zebra-cookie-file or --zebra-api-key-file"
                ),
            };
            transfer_v0_2_testnet(
                &arguments.zebra_endpoint,
                &credential,
                &arguments.from_zcash_key_file,
                &arguments.to_zcash_key_file,
                arguments.value_zatoshis,
                arguments.skip,
                arguments.scan_from_height,
            )
            .await?
        }
    };
    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}
