use std::{
    fs::{self, File, OpenOptions},
    io::{Read as _, Write as _},
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _},
    path::Path,
};

use zeroize::Zeroizing;

use crate::RunnerError;

pub(crate) const MAX_PUBLIC_JSON_BYTES: u64 = 8 * 1024;
const MAX_SECRET_KEY_FILE_BYTES: u64 = 65;

pub(crate) fn read_public(path: &Path) -> Result<Vec<u8>, RunnerError> {
    read_bounded_regular(path, MAX_PUBLIC_JSON_BYTES, false)
}

pub(crate) fn read_secret_key(path: &Path) -> Result<Zeroizing<[u8; 32]>, RunnerError> {
    let bytes = read_bounded_regular(path, MAX_SECRET_KEY_FILE_BYTES, true)?;
    let encoded = bytes.strip_suffix(b"\n").unwrap_or(bytes.as_slice());
    if encoded.len() != 64 || !encoded.iter().all(u8::is_ascii_hexdigit) {
        return Err(RunnerError::InvalidSecretKeyFile);
    }
    let encoded = std::str::from_utf8(encoded).map_err(|_| RunnerError::InvalidSecretKeyFile)?;
    if encoded.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return Err(RunnerError::InvalidSecretKeyFile);
    }
    let decoded = hex::decode(encoded).map_err(|_| RunnerError::InvalidSecretKeyFile)?;
    let key: [u8; 32] = decoded
        .try_into()
        .map_err(|_| RunnerError::InvalidSecretKeyFile)?;
    Ok(Zeroizing::new(key))
}

fn read_bounded_regular(
    path: &Path,
    maximum: u64,
    require_owner_private: bool,
) -> Result<Vec<u8>, RunnerError> {
    let path_metadata = fs::symlink_metadata(path).map_err(RunnerError::InputIo)?;
    if path_metadata.file_type().is_symlink() || !path_metadata.is_file() {
        return Err(RunnerError::UnsafeInputFile);
    }
    let file = File::open(path).map_err(RunnerError::InputIo)?;
    let metadata = file.metadata().map_err(RunnerError::InputIo)?;
    if !metadata.is_file()
        || metadata.dev() != path_metadata.dev()
        || metadata.ino() != path_metadata.ino()
        || metadata.len() > maximum
    {
        return Err(RunnerError::UnsafeInputFile);
    }
    if require_owner_private && (metadata.mode() & 0o077 != 0 || metadata.nlink() != 1) {
        return Err(RunnerError::UnsafeSecretKeyFile);
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    file.take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(RunnerError::InputIo)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > maximum {
        return Err(RunnerError::InputTooLarge);
    }
    Ok(bytes)
}

pub(crate) fn write_public_new(path: &Path, bytes: &[u8]) -> Result<(), RunnerError> {
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_PUBLIC_JSON_BYTES {
        return Err(RunnerError::OutputTooLarge);
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let mut file = options.open(path).map_err(RunnerError::OutputIo)?;
    file.write_all(bytes).map_err(RunnerError::OutputIo)?;
    file.sync_all().map_err(RunnerError::OutputIo)
}
