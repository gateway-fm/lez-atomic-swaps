use std::{
    error::Error,
    ffi::OsString,
    fmt::{self, Display, Formatter},
    process::ExitCode,
};

use nssa::AccountId;
use serde::Serialize;

const SCHEMA: &str = "lez-v0.2-account-id-codec";
const VERSION: u8 = 1;

#[derive(Debug, Eq, PartialEq)]
enum CliError {
    Usage,
    NonCanonicalAccountId,
    ZeroAccountId,
    Serialization,
}

impl Display for CliError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Usage => "expected exactly one canonical Base58 LEZ account ID",
            Self::NonCanonicalAccountId => "account ID is not canonical 32-byte Base58",
            Self::ZeroAccountId => "the all-zero account ID is forbidden",
            Self::Serialization => "could not serialize public result",
        })
    }
}

impl Error for CliError {}

#[derive(Debug, Eq, PartialEq, Serialize)]
struct PublicAccountEncoding {
    schema: &'static str,
    version: u8,
    account_id_base58: String,
    account_id_hex: String,
}

fn parse_argument(mut arguments: impl Iterator<Item = OsString>) -> Result<String, CliError> {
    let value = arguments.next().ok_or(CliError::Usage)?;
    if arguments.next().is_some() {
        return Err(CliError::Usage);
    }
    value
        .into_string()
        .map_err(|_| CliError::NonCanonicalAccountId)
}

fn encode_account(input: &str) -> Result<PublicAccountEncoding, CliError> {
    let account = input
        .parse::<AccountId>()
        .map_err(|_| CliError::NonCanonicalAccountId)?;
    if account.to_string() != input {
        return Err(CliError::NonCanonicalAccountId);
    }
    if account.value() == &[0; 32] {
        return Err(CliError::ZeroAccountId);
    }
    Ok(PublicAccountEncoding {
        schema: SCHEMA,
        version: VERSION,
        account_id_base58: input.to_owned(),
        account_id_hex: hex::encode(account.value()),
    })
}

fn execute(arguments: impl IntoIterator<Item = OsString>) -> Result<String, CliError> {
    let input = parse_argument(arguments.into_iter())?;
    serde_json::to_string(&encode_account(&input)?).map_err(|_| CliError::Serialization)
}

fn main() -> ExitCode {
    match execute(std::env::args_os().skip(1)) {
        Ok(output) => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn official_parser_round_trips_one_nonzero_account_to_exact_hex() {
        let account = AccountId::new([1; 32]);
        let output = execute([OsString::from(account.to_string())]).expect("canonical account");
        assert_eq!(
            output,
            format!(
                "{{\"schema\":\"{SCHEMA}\",\"version\":1,\"account_id_base58\":\"{}\",\"account_id_hex\":\"{}\"}}",
                account,
                "01".repeat(32),
            )
        );
    }

    #[test]
    fn zero_malformed_non_utf8_and_argument_count_fail_closed() {
        let zero = AccountId::new([0; 32]).to_string();
        assert_eq!(
            execute([OsString::from(zero)]),
            Err(CliError::ZeroAccountId)
        );
        assert_eq!(
            execute([OsString::from("not-an-account")]),
            Err(CliError::NonCanonicalAccountId)
        );
        assert_eq!(execute([]), Err(CliError::Usage));
        assert_eq!(
            execute([OsString::from("one"), OsString::from("two")]),
            Err(CliError::Usage)
        );
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt as _;
            assert_eq!(
                execute([OsString::from_vec(vec![0xff])]),
                Err(CliError::NonCanonicalAccountId)
            );
        }
    }
}
