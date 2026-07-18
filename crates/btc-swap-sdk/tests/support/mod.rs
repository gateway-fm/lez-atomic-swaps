#![allow(dead_code)]

use std::path::PathBuf;

use bitcoin::hex::FromHex as _;
use serde_json::Value;

pub fn vector_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/bitcoin-bips-8c369ac8")
}

pub fn bip327(name: &str) -> Value {
    let path = vector_root().join("bip-0327").join(name);
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()))
}

pub fn hex_vec(value: &str) -> Vec<u8> {
    Vec::<u8>::from_hex(value).unwrap_or_else(|error| panic!("invalid vector hex: {error}"))
}

pub fn hex_array<const N: usize>(value: &str) -> [u8; N] {
    hex_vec(value)
        .try_into()
        .unwrap_or_else(|bytes: Vec<u8>| panic!("expected {N} bytes, got {}", bytes.len()))
}

pub fn strings(value: &Value, key: &str) -> Vec<String> {
    value[key]
        .as_array()
        .unwrap_or_else(|| panic!("{key} must be an array"))
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .unwrap_or_else(|| panic!("{key} entries must be strings"))
                .to_owned()
        })
        .collect()
}

pub fn indexes(value: &Value, key: &str) -> Vec<usize> {
    value[key]
        .as_array()
        .unwrap_or_else(|| panic!("{key} must be an array"))
        .iter()
        .map(|entry| {
            usize::try_from(
                entry
                    .as_u64()
                    .unwrap_or_else(|| panic!("{key} entries must be unsigned integers")),
            )
            .expect("vector index fits usize")
        })
        .collect()
}

pub fn cases<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value[key]
        .as_array()
        .unwrap_or_else(|| panic!("{key} must be an array"))
}
