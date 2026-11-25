use std::{fmt, path::PathBuf};

use clap::Parser;
use url::Url;
use zec_reference_actor::{
    TestnetZebraCredential, transfer_local_v0_2_regtest, transfer_v0_2_testnet,
};

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum ZcashNetwork {
    /// The deterministic Regtest fixture (loopback; its cookie when its
    /// JSON-RPC requires one).
    Regtest,
    /// Zcash Testnet on NU7 (public-testnet-v2): a self-hosted Zebra with
    /// `--zebra-cookie-file`, or Tatum's gateway with `--zebra-api-key-file`.
    Test,
}

#[derive(Clone, Parser)]
#[command(
    about = "TEST-ONLY: move Regtest or Testnet ZEC between two owner-private keys with one plain transparent transaction",
    long_about = "TEST-ONLY, for the devnet and Zcash Testnet runs (feature test-payer): move Regtest or \
Testnet ZEC between two owner-private keys with one plain transparent transaction. The devnet harness \
uses it to fund the Taker's Zcash key before a swap in which the Taker sells ZEC. It pays on the \
deterministic Regtest fixture or Zcash Testnet only: there is no mainnet network, and a Zebra that \
reports another chain is refused before anything is signed. It is no part of a Node or its packages."
)]
struct Arguments {
    /// Zebra RPC endpoint: loopback, or Tatum's Testnet origin.
    #[arg(long, value_name = "URL")]
    zebra_endpoint: Url,
    /// The chain the transfer is made on.
    #[arg(long, value_enum, default_value_t = ZcashNetwork::Regtest)]
    zcash_network: ZcashNetwork,
    /// Owner-private `.cookie` of the Zebra: a self-hosted Testnet Zebra's, or
    /// the devnet's Regtest one where its JSON-RPC requires a cookie.
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
                arguments.zebra_api_key_file.is_none(),
                "the Regtest fixture takes no Zebra API key"
            );
            transfer_local_v0_2_regtest(
                &arguments.zebra_endpoint,
                arguments.zebra_cookie_file.as_deref(),
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

#[cfg(test)]
mod tests {
    use clap::{CommandFactory as _, Parser as _};

    use super::Arguments;

    const KEYS: [&str; 4] = [
        "--from-zcash-key-file",
        "/k/from",
        "--to-zcash-key-file",
        "/k/to",
    ];

    fn parse(extra: &[&str]) -> Result<Arguments, clap::Error> {
        let mut argv = vec![
            "zec-local-poc-transfer",
            "--zebra-endpoint",
            "http://127.0.0.1:18232/",
        ];
        argv.extend(KEYS);
        argv.extend(extra);
        Arguments::try_parse_from(argv)
    }

    #[test]
    fn help_says_it_is_a_test_tool_for_regtest_and_testnet_only() {
        let help = Arguments::command().render_long_help().to_string();
        assert!(help.contains("TEST-ONLY"), "{help}");
        assert!(help.contains("no mainnet network"), "{help}");
        let networks = help.split("--zcash-network").nth(1).unwrap();
        let networks = networks.split("--zebra-cookie-file").next().unwrap();
        assert!(
            networks.contains("- regtest:") && networks.contains("- test:"),
            "{networks}"
        );
        assert!(!networks.contains("main"), "{networks}");
    }

    #[test]
    fn regtest_is_the_default_and_testnet_is_the_other_network() {
        let arguments = parse(&["--value-zatoshis", "100020000"]).unwrap();
        assert!(matches!(
            arguments.zcash_network,
            super::ZcashNetwork::Regtest
        ));
        assert_eq!(arguments.value_zatoshis, 100_020_000);
        let arguments = parse(&["--value-zatoshis", "1", "--zcash-network", "test"]).unwrap();
        assert!(matches!(arguments.zcash_network, super::ZcashNetwork::Test));
    }

    #[test]
    fn mainnet_is_not_a_network_it_takes() {
        for network in ["main", "mainnet"] {
            let error = parse(&["--value-zatoshis", "1", "--zcash-network", network]).unwrap_err();
            assert_eq!(
                error.kind(),
                clap::error::ErrorKind::InvalidValue,
                "{network}"
            );
        }
    }

    #[test]
    fn a_value_both_keys_and_the_endpoint_are_required() {
        assert!(parse(&[]).is_err());
        let error = Arguments::try_parse_from([
            "zec-local-poc-transfer",
            "--zebra-endpoint",
            "http://127.0.0.1:18232/",
            "--value-zatoshis",
            "1",
        ])
        .unwrap_err();
        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::MissingRequiredArgument
        );
        assert!(parse(&["--value-zatoshis", "-1"]).is_err());
    }

    #[test]
    fn one_credential_at_most_and_the_debug_form_redacts_them() {
        let error = parse(&[
            "--value-zatoshis",
            "1",
            "--zebra-cookie-file",
            "/c",
            "--zebra-api-key-file",
            "/a",
        ])
        .unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::ArgumentConflict);
        let shown = format!(
            "{:?}",
            parse(&["--value-zatoshis", "1", "--zebra-cookie-file", "/c"]).unwrap()
        );
        assert!(
            !shown.contains("/k/from") && !shown.contains("/c\""),
            "{shown}"
        );
        assert!(shown.contains("[REDACTED]"), "{shown}");
    }
}
