#![cfg(target_os = "linux")]

use std::{fs, os::unix::fs::PermissionsExt, str::FromStr};

use lez_v0_2_sidecar::actor_identity::{
    IDENTITY_FILE_NAME, PRIVATE_KEY_FILE_NAME, provision_local_actor_identity,
};
use nssa::{AccountId, PrivateKey, PublicKey};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IdentityFile {
    schema: String,
    version: u8,
    account_id: String,
    account_id_hex: String,
    x_only_public_key: String,
}

fn mode(path: &std::path::Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn generates_official_identity_in_owner_only_files() {
    let parent = tempfile::tempdir().unwrap();
    let output = parent.path().join("maker");

    let public = provision_local_actor_identity(&output).unwrap();
    let private_path = output.join(PRIVATE_KEY_FILE_NAME);
    let identity_path = output.join(IDENTITY_FILE_NAME);

    assert_eq!(mode(&output), 0o700);
    assert_eq!(mode(&private_path), 0o600);
    assert_eq!(mode(&identity_path), 0o600);

    let private_text = fs::read_to_string(&private_path).unwrap();
    assert_eq!(private_text.len(), 65);
    assert!(private_text.ends_with('\n'));
    let private_key = PrivateKey::from_str(private_text.trim_end()).unwrap();
    let public_key = PublicKey::new_from_private_key(&private_key);
    let account_id = AccountId::from(&public_key);

    let identity: IdentityFile =
        serde_json::from_slice(&fs::read(&identity_path).unwrap()).unwrap();
    assert_eq!(identity.schema, "lez-v0.2-local-actor-identity");
    assert_eq!(identity.version, 1);
    assert_eq!(identity.account_id, account_id.to_string());
    assert_eq!(
        identity.account_id_hex,
        hex::encode(account_id.into_value())
    );
    assert_eq!(identity.x_only_public_key, hex::encode(public_key.value()));
    assert_eq!(public.account_id(), identity.account_id);

    let serialized_public = serde_json::to_string(&public).unwrap();
    assert!(!serialized_public.contains(private_text.trim_end()));
}

#[test]
fn refuses_to_clobber_an_existing_output_and_preserves_it() {
    let parent = tempfile::tempdir().unwrap();
    let output = parent.path().join("maker");
    let first = provision_local_actor_identity(&output).unwrap();
    let private_before = fs::read(output.join(PRIVATE_KEY_FILE_NAME)).unwrap();
    let identity_before = fs::read(output.join(IDENTITY_FILE_NAME)).unwrap();

    let error = provision_local_actor_identity(&output).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("refusing to reuse actor identity output")
    );
    assert_eq!(
        fs::read(output.join(PRIVATE_KEY_FILE_NAME)).unwrap(),
        private_before
    );
    assert_eq!(
        fs::read(output.join(IDENTITY_FILE_NAME)).unwrap(),
        identity_before
    );
    assert_eq!(
        serde_json::from_slice::<IdentityFile>(&identity_before)
            .unwrap()
            .account_id,
        first.account_id()
    );
}

#[test]
fn independently_provisioned_actors_have_distinct_identities() {
    let parent = tempfile::tempdir().unwrap();

    let maker = provision_local_actor_identity(&parent.path().join("maker")).unwrap();
    let taker = provision_local_actor_identity(&parent.path().join("taker")).unwrap();

    assert_ne!(maker.account_id(), taker.account_id());
    assert_ne!(maker.x_only_public_key(), taker.x_only_public_key());
    assert_ne!(
        fs::read(parent.path().join("maker").join(PRIVATE_KEY_FILE_NAME)).unwrap(),
        fs::read(parent.path().join("taker").join(PRIVATE_KEY_FILE_NAME)).unwrap()
    );
}
