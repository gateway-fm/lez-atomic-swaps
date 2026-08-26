use std::{
    error::Error,
    ffi::OsString,
    fmt::{self, Display, Formatter},
    process::ExitCode,
};

use nssa::{AccountId, PublicKey};
use serde::Serialize;

const SCHEMA: &str = "lez-v0.2-nssa-account-id";
const VERSION: u8 = 1;

#[derive(Debug, PartialEq, Eq)]
enum CliError {
    Usage,
    NonCanonicalPublicKey,
    ZeroPublicKey,
    InvalidPublicKey,
    Serialization,
}

impl Display for CliError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage => formatter.write_str(
                "expected exactly one canonical lowercase 32-byte x-only public-key argument",
            ),
            Self::NonCanonicalPublicKey => formatter
                .write_str("public key must be exactly 64 lowercase hexadecimal characters"),
            Self::ZeroPublicKey => formatter.write_str("the all-zero public key is forbidden"),
            Self::InvalidPublicKey => formatter.write_str("invalid x-only NSSA public key"),
            Self::Serialization => formatter.write_str("could not serialize public result"),
        }
    }
}

impl Error for CliError {}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct PublicAccountMapping {
    schema: &'static str,
    version: u8,
    x_only_public_key: String,
    account_id: String,
}

fn parse_argument(mut arguments: impl Iterator<Item = OsString>) -> Result<String, CliError> {
    let argument = arguments.next().ok_or(CliError::Usage)?;
    if arguments.next().is_some() {
        return Err(CliError::Usage);
    }
    argument
        .into_string()
        .map_err(|_| CliError::NonCanonicalPublicKey)
}

fn derive_mapping(input: &str) -> Result<PublicAccountMapping, CliError> {
    if input.len() != 64
        || !input
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(CliError::NonCanonicalPublicKey);
    }

    let mut key_bytes = [0_u8; 32];
    hex::decode_to_slice(input, &mut key_bytes).map_err(|_| CliError::NonCanonicalPublicKey)?;
    if key_bytes == [0; 32] {
        return Err(CliError::ZeroPublicKey);
    }

    let public_key = PublicKey::try_new(key_bytes).map_err(|_| CliError::InvalidPublicKey)?;
    let account_id = AccountId::from(&public_key);

    Ok(PublicAccountMapping {
        schema: SCHEMA,
        version: VERSION,
        x_only_public_key: input.to_owned(),
        account_id: hex::encode(account_id.into_value()),
    })
}

fn execute(arguments: impl IntoIterator<Item = OsString>) -> Result<String, CliError> {
    let input = parse_argument(arguments.into_iter())?;
    let mapping = derive_mapping(&input)?;
    serde_json::to_string(&mapping).map_err(|_| CliError::Serialization)
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

    const GENERATOR_X: &str = "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
    const INVALID_X: &str = "eefdea4cdb677750a420fee807eacf21eb9898ae79b9768766e4faa04a2d4a34";

    #[test]
    fn emits_one_canonical_public_json_object() {
        let output = execute([OsString::from(GENERATOR_X)]).unwrap();

        assert_eq!(output.lines().count(), 1);
        assert_eq!(
            output,
            concat!(
                r#"{"schema":"lez-v0.2-nssa-account-id","version":1,"#,
                r#""x_only_public_key":"79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798","#,
                r#""account_id":"af31cf2514f1ffb48b310d21703897d0accb8fb77ca0c98742f901881165a26e"}"#
            )
        );
    }

    #[test]
    fn rejects_noncanonical_and_invalid_public_keys() {
        assert_eq!(derive_mapping("00"), Err(CliError::NonCanonicalPublicKey));
        assert_eq!(
            derive_mapping(&GENERATOR_X.to_uppercase()),
            Err(CliError::NonCanonicalPublicKey)
        );
        assert_eq!(
            derive_mapping(&"0".repeat(64)),
            Err(CliError::ZeroPublicKey)
        );
        assert_eq!(derive_mapping(INVALID_X), Err(CliError::InvalidPublicKey));
    }

    #[test]
    fn accepts_exactly_one_argument() {
        assert_eq!(execute([]), Err(CliError::Usage));
        assert_eq!(
            execute([OsString::from(GENERATOR_X), OsString::from(GENERATOR_X)]),
            Err(CliError::Usage)
        );
    }
}
