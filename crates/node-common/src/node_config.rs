//! Shared strict startup envelope for role-fixed Node processes.

use std::{ffi::OsString, fs, path::PathBuf};

use anyhow::{Context as _, ensure};
use clap::Parser;
use serde::Deserialize;

const MAXIMUM_NODE_CONFIG_BYTES: u64 = 64 * 1024;
const MAXIMUM_NODE_CONFIG_ARGUMENTS: usize = 256;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NodeConfigV1 {
    schema_version: u16,
    arguments: Vec<Box<str>>,
}

/// Loads direct process arguments or one exclusive strict `--config` envelope.
///
/// Maker and Taker Nodes deliberately share this parser, size bounds, recursive
/// configuration rejection, and Clap validation behavior.
///
/// # Errors
///
/// Returns an error when the configuration is not a bounded regular file, its
/// schema is invalid, or its expanded arguments fail the role-specific parser.
pub fn load_node_arguments<T>(role: &str) -> anyhow::Result<T>
where
    T: Parser,
{
    let process_arguments = std::env::args_os().collect::<Vec<_>>();
    if process_arguments.get(1).and_then(|value| value.to_str()) != Some("--config") {
        return Ok(T::parse_from(process_arguments));
    }
    ensure!(
        process_arguments.len() == 3,
        "--config is exclusive; put every {role} Node option in the versioned configuration"
    );
    let path = PathBuf::from(&process_arguments[2]);
    let metadata = fs::symlink_metadata(&path)
        .with_context(|| format!("inspect {role} Node configuration"))?;
    ensure!(
        metadata.file_type().is_file() && metadata.len() <= MAXIMUM_NODE_CONFIG_BYTES,
        "{role} Node configuration must be a bounded regular file"
    );
    let bytes = fs::read(&path).with_context(|| format!("read {role} Node configuration"))?;
    parse_node_config(&bytes, process_arguments[0].clone(), role)
}

/// Expands a strict versioned Node configuration into a role-specific parser.
///
/// # Errors
///
/// Returns an error for an unsupported schema, invalid bounds, recursion, or
/// role-specific argument validation failure.
pub fn parse_node_config<T>(bytes: &[u8], executable: OsString, role: &str) -> anyhow::Result<T>
where
    T: Parser,
{
    ensure!(
        bytes.len() <= usize::try_from(MAXIMUM_NODE_CONFIG_BYTES)?,
        "{role} Node configuration is too large"
    );
    let configuration: NodeConfigV1 = serde_json::from_slice(bytes)
        .with_context(|| format!("parse strict {role} Node configuration v1"))?;
    ensure!(
        configuration.schema_version == 1,
        "unsupported {role} Node configuration schema"
    );
    ensure!(
        !configuration.arguments.is_empty()
            && configuration.arguments.len() <= MAXIMUM_NODE_CONFIG_ARGUMENTS,
        "{role} Node configuration argument count is invalid"
    );
    ensure!(
        configuration.arguments.iter().all(|argument| {
            !argument.is_empty() && argument.len() <= 4_096 && argument.as_ref() != "--config"
        }),
        "{role} Node configuration contains an invalid argument"
    );
    let mut arguments = Vec::with_capacity(configuration.arguments.len() + 1);
    arguments.push(executable);
    arguments.extend(
        configuration
            .arguments
            .into_iter()
            .map(|argument| OsString::from(argument.as_ref())),
    );
    T::try_parse_from(arguments).map_err(anyhow::Error::new)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use clap::Parser;

    use super::parse_node_config;

    #[derive(Debug, Parser, PartialEq)]
    struct PairedArguments {
        #[arg(long)]
        socket: String,
    }

    #[test]
    fn one_envelope_contract_serves_both_roles() {
        let bytes = br#"{"schema_version":1,"arguments":["--socket","/run/lez/node.sock"]}"#;
        for role in ["Maker", "Taker"] {
            let parsed: PairedArguments =
                parse_node_config(bytes, OsString::from("lez-node"), role).unwrap();
            assert_eq!(parsed.socket, "/run/lez/node.sock");
        }
    }

    #[test]
    fn envelope_rejects_unknown_fields_and_recursion_for_both_roles() {
        for role in ["Maker", "Taker"] {
            for invalid in [
                br#"{"schema_version":1,"arguments":["--config","again.json"]}"#.as_slice(),
                br#"{"schema_version":1,"arguments":["--socket","/tmp/node.sock"],"extra":true}"#
                    .as_slice(),
            ] {
                assert!(
                    parse_node_config::<PairedArguments>(invalid, OsString::from("lez-node"), role)
                        .is_err()
                );
            }
        }
    }
}
