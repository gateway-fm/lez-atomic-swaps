use anyhow::{Context as _, Result, ensure};
use lez_standalone_e2e::{
    LocalActorManifest, LocalNodeReadinessManifest, TRACKED_GUEST_ARTIFACT_MANIFEST,
    verify_checked_guest_artifact, write_private_readiness_manifest,
};

fn fixture() -> LocalNodeReadinessManifest {
    LocalNodeReadinessManifest {
        schema_version: 2,
        endpoint: "http://127.0.0.1:49152".to_owned(),
        genesis_block_id: 1,
        genesis_block_hash: "11".repeat(32),
        channel_id: "00".repeat(32),
        elf_sha256: "22".repeat(32),
        image_id: "33".repeat(32),
        escrow_program_id: [7; 8],
        authenticated_transfer_program_id: [8; 8],
        deployment_transaction_hash: "44".repeat(32),
        deployment_block_id: 3,
        deployment_block_hash: "55".repeat(32),
        actors: vec![LocalActorManifest {
            account_id: "local-actor".to_owned(),
            private_key: "22".repeat(32),
            balance: 1_000_000,
        }],
    }
}

#[test]
fn alternate_guest_artifact_manifest_is_rejected() -> Result<()> {
    let directory = tempfile::tempdir().context("create artifact workspace")?;
    let path = directory.path().join("untracked-artifact.toml");
    std::fs::write(&path, format!("{TRACKED_GUEST_ARTIFACT_MANIFEST}\n"))
        .context("write alternate artifact manifest")?;

    ensure!(
        verify_checked_guest_artifact(b"not-an-elf", &path).is_err(),
        "only the exact repository-tracked artifact manifest may authorize a guest"
    );
    Ok(())
}

#[test]
fn readiness_manifest_is_private_atomic_and_write_once() -> Result<()> {
    let directory = tempfile::tempdir().context("create readiness workspace")?;
    let path = directory.path().join("readiness.json");
    let expected = fixture();

    write_private_readiness_manifest(&path, &expected).context("write readiness manifest")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&path)
            .context("read readiness metadata")?
            .permissions()
            .mode()
            & 0o777;
        ensure!(mode == 0o600, "readiness mode must be 0600");
    }
    let actual: LocalNodeReadinessManifest =
        serde_json::from_slice(&std::fs::read(&path).context("read readiness manifest")?)
            .context("decode readiness manifest")?;
    ensure!(
        actual == expected,
        "readiness manifest must round-trip exactly"
    );
    ensure!(
        write_private_readiness_manifest(&path, &fixture()).is_err(),
        "readiness manifest must refuse overwrite"
    );
    let entries = std::fs::read_dir(directory.path())
        .context("list readiness workspace")?
        .collect::<std::io::Result<Vec<_>>>()?;
    ensure!(
        entries.len() == 1,
        "only the persisted readiness file may remain"
    );
    Ok(())
}
