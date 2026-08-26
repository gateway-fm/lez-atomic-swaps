//! Local-only provisioning for an official LEZ v0.2 public actor identity.

use std::{
    fs::{self, DirBuilder, File, OpenOptions, Permissions},
    io::{self, Write as _},
    os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::Path,
};

use nssa::{AccountId, PrivateKey, PublicKey};
use serde::Serialize;
use thiserror::Error;
use zeroize::Zeroizing;

/// Name of the owner-private official LEZ signer key file.
pub const PRIVATE_KEY_FILE_NAME: &str = "lez-signer.key";
/// Name of the owner-private public identity descriptor.
pub const IDENTITY_FILE_NAME: &str = "identity.json";

const SCHEMA: &str = "lez-v0.2-local-actor-identity";
const VERSION: u8 = 2;

/// Public output produced for a newly provisioned local LEZ actor.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PublicActorIdentity {
    schema: &'static str,
    version: u8,
    account_id: String,
    account_id_hex: String,
    vault_account_id: String,
    vault_account_id_hex: String,
    x_only_public_key: String,
}

impl PublicActorIdentity {
    /// Returns the canonical base58 public account identifier accepted by LEZ genesis.
    #[must_use]
    pub fn account_id(&self) -> &str {
        &self.account_id
    }

    /// Returns the canonical base58 Vault account derived by official LEZ v0.2 code.
    #[must_use]
    pub fn vault_account_id(&self) -> &str {
        &self.vault_account_id
    }

    /// Returns the official x-only public key in lowercase hexadecimal form.
    #[must_use]
    pub fn x_only_public_key(&self) -> &str {
        &self.x_only_public_key
    }
}

/// Failure to provision a fresh local actor identity.
#[derive(Debug, Error)]
pub enum ActorIdentityProvisionError {
    /// The output path already exists and is never reused.
    #[error("refusing to reuse actor identity output")]
    OutputExists,
    /// The output directory could not be created privately.
    #[error("could not create owner-private actor identity directory")]
    CreateDirectory(#[source] io::Error),
    /// One of the no-clobber output files could not be created.
    #[error("could not create owner-private actor identity file")]
    CreateFile(#[source] io::Error),
    /// A generated identity could not be serialized.
    #[error("could not serialize public actor identity")]
    Serialize(#[source] serde_json::Error),
    /// A complete output could not be durably written.
    #[error("could not write owner-private actor identity")]
    Write(#[source] io::Error),
}

/// Creates a fresh official LEZ key and writes it only to a new owner-private directory.
///
/// The private key is sourced through [`PrivateKey::new_os_random`], never accepted as
/// input, and never included in the returned public identity.
///
/// # Errors
///
/// Returns an error if the output already exists or if its private files cannot be
/// created, serialized, written, and synchronized without reuse.
pub fn provision_local_actor_identity(
    output_directory: &Path,
) -> Result<PublicActorIdentity, ActorIdentityProvisionError> {
    create_output_directory(output_directory)?;

    let result = provision_in_new_directory(output_directory);
    if result.is_err() {
        let _ = fs::remove_file(output_directory.join(PRIVATE_KEY_FILE_NAME));
        let _ = fs::remove_file(output_directory.join(IDENTITY_FILE_NAME));
        let _ = fs::remove_dir(output_directory);
    }
    result
}

fn create_output_directory(path: &Path) -> Result<(), ActorIdentityProvisionError> {
    let mut builder = DirBuilder::new();
    builder.mode(0o700);
    match builder.create(path) {
        Ok(()) => {
            fs::set_permissions(path, Permissions::from_mode(0o700))
                .map_err(ActorIdentityProvisionError::CreateDirectory)?;
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            Err(ActorIdentityProvisionError::OutputExists)
        }
        Err(error) => Err(ActorIdentityProvisionError::CreateDirectory(error)),
    }
}

fn provision_in_new_directory(
    output_directory: &Path,
) -> Result<PublicActorIdentity, ActorIdentityProvisionError> {
    let mut private_file =
        create_owner_private_file(&output_directory.join(PRIVATE_KEY_FILE_NAME))?;
    let mut identity_file = create_owner_private_file(&output_directory.join(IDENTITY_FILE_NAME))?;

    let private_key = PrivateKey::new_os_random();
    let public_key = PublicKey::new_from_private_key(&private_key);
    let account_id = AccountId::from(&public_key);
    let vault_account_id = vault_core::compute_vault_account_id(programs::vault().id(), account_id);
    let identity = PublicActorIdentity {
        schema: SCHEMA,
        version: VERSION,
        account_id: account_id.to_string(),
        account_id_hex: hex::encode(account_id.into_value()),
        vault_account_id: vault_account_id.to_string(),
        vault_account_id_hex: hex::encode(vault_account_id.into_value()),
        x_only_public_key: hex::encode(public_key.value()),
    };

    let mut private_encoded = Zeroizing::new(hex::encode(private_key.value()));
    private_encoded.push('\n');
    private_file
        .write_all(private_encoded.as_bytes())
        .and_then(|()| private_file.sync_all())
        .map_err(ActorIdentityProvisionError::Write)?;

    let mut public_encoded =
        serde_json::to_vec(&identity).map_err(ActorIdentityProvisionError::Serialize)?;
    public_encoded.push(b'\n');
    identity_file
        .write_all(&public_encoded)
        .and_then(|()| identity_file.sync_all())
        .map_err(ActorIdentityProvisionError::Write)?;

    File::open(output_directory)
        .and_then(|directory| directory.sync_all())
        .map_err(ActorIdentityProvisionError::Write)?;

    Ok(identity)
}

fn create_owner_private_file(path: &Path) -> Result<File, ActorIdentityProvisionError> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(ActorIdentityProvisionError::CreateFile)?;
    file.set_permissions(Permissions::from_mode(0o600))
        .map_err(ActorIdentityProvisionError::CreateFile)?;
    Ok(file)
}
