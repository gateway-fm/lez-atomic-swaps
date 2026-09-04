//! Per-swap directory layout under a role's swaps root.
//!
//! One swap owns one owner-private directory named by its reservation id, so
//! every artifact of the swap (role root, journals, prepared LEZ material,
//! funding plan, actor bundle) lives together and is deleted together.

use std::{
    fs::{self, DirBuilder, OpenOptions},
    io::Write as _,
    os::unix::fs::{DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _},
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, ensure};
use btc_role_preflight::RoleRootLayout;
use lez_bridge_protocol::RequestId;
use rustix::fs::{Mode, OFlags};
use std::fs::{File, Metadata};
use std::io::Read as _;
use std::path::Component;

/// The files of one swap for one role.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SwapLayout {
    root: PathBuf,
}

impl SwapLayout {
    /// The layout for `reservation_id` under `swaps_root` (not created yet).
    #[must_use]
    pub fn new(swaps_root: &Path, reservation_id: &RequestId) -> Self {
        Self {
            root: swaps_root.join(reservation_id.as_str()),
        }
    }

    /// Creates the swap directory owner-private; fails if it already exists.
    ///
    /// # Errors
    ///
    /// Fails when the directory exists or cannot be created.
    pub fn create(&self) -> Result<()> {
        DirBuilder::new()
            .mode(0o700)
            .create(&self.root)
            .with_context(|| format!("create swap directory {}", self.root.display()))?;
        for sub in ["journals", "lez", "bitcoin", "actor"] {
            DirBuilder::new().mode(0o700).create(self.root.join(sub))?;
        }
        Ok(())
    }

    /// Whether the swap directory exists and is owner-private.
    #[must_use]
    pub fn exists(&self) -> bool {
        fs::symlink_metadata(&self.root)
            .is_ok_and(|metadata| metadata.is_dir() && metadata.mode().trailing_zeros() >= 6)
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The preflight role root (contribution, private keys, bound agreement).
    #[must_use]
    pub fn role_root(&self) -> RoleRootLayout {
        RoleRootLayout::new(self.root.join("role"))
    }

    #[must_use]
    pub fn bitcoin_journal(&self) -> PathBuf {
        self.root.join("journals").join("bitcoin.sqlite")
    }

    #[must_use]
    pub fn lez_journal(&self) -> PathBuf {
        self.root.join("journals").join("lez.sqlite")
    }

    /// The claimant's final prepared witnessed claim (both roles hold a copy).
    #[must_use]
    pub fn prepared_claim_file(&self) -> PathBuf {
        self.root.join("lez").join("prepared-claim.json")
    }

    #[must_use]
    pub fn escrow_request_file(&self) -> PathBuf {
        self.root.join("lez").join("prepare-escrow-request.json")
    }

    #[must_use]
    pub fn escrow_result_file(&self) -> PathBuf {
        self.root.join("lez").join("prepared-escrow.json")
    }

    /// The exact signed Bitcoin funding transaction (lowercase hex).
    #[must_use]
    pub fn funding_transaction_file(&self) -> PathBuf {
        self.root.join("bitcoin").join("funding-transaction.hex")
    }

    #[must_use]
    pub fn funding_plan_file(&self) -> PathBuf {
        self.root.join("bitcoin").join("funding-plan.json")
    }

    /// Ceremony session ids and peer facts recorded for replay.
    #[must_use]
    pub fn ceremony_file(&self) -> PathBuf {
        self.root.join("ceremony.json")
    }

    #[must_use]
    pub fn actor_root(&self) -> PathBuf {
        self.root.join("actor")
    }

    #[must_use]
    pub fn actor_config_file(&self) -> PathBuf {
        self.actor_root().join("actor-config.json")
    }

    #[must_use]
    pub fn actor_state_db(&self) -> PathBuf {
        self.actor_root().join("state.sqlite3")
    }

    #[must_use]
    pub fn actor_refund_key_file(&self) -> PathBuf {
        self.actor_root().join("bitcoin-refund.key")
    }

    #[must_use]
    pub fn actor_adaptor_secret_file(&self) -> PathBuf {
        self.actor_root().join("adaptor-secret.key")
    }

    #[must_use]
    pub fn receipt_file(&self) -> PathBuf {
        self.root.join("acceptance-receipt.json")
    }
}

/// Writes `bytes` to a new owner-private file, or verifies an existing file
/// holds exactly `bytes` (idempotent replay).
///
/// # Errors
///
/// Fails when the file exists with different content or cannot be created.
pub fn write_private_exact(path: &Path, bytes: &[u8]) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            ensure!(
                metadata.is_file(),
                "{} is not a regular file",
                path.display()
            );
            // Read back through the same no-follow, bounded path: a file that
            // is larger than what we would write already differs.
            let existing = read_vetted(path, bytes.len())
                .with_context(|| format!("{} already holds different content", path.display()))?;
            ensure!(
                existing == bytes,
                "{} already holds different content",
                path.display()
            );
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)
                .with_context(|| format!("create {}", path.display()))?;
            file.write_all(bytes)?;
            file.sync_all()?;
            Ok(false)
        }
        Err(error) => Err(error.into()),
    }
}

/// Rejects a configured path that is not absolute or that walks the tree.
///
/// Every file the role configuration names (Core cookie, signer key, sidecar
/// program, actor program, swaps root) is opened by exactly that name, so a
/// relative path or a `.`/`..` component has no honest use.
///
/// # Errors
///
/// Fails when `path` is relative or contains a `.` or `..` component.
pub fn vet_configured_path(path: &Path, label: &str) -> Result<()> {
    ensure!(
        path.is_absolute(),
        "{label} must be an absolute path: {}",
        path.display()
    );
    ensure!(
        path.components()
            .all(|component| matches!(component, Component::RootDir | Component::Normal(_))),
        "{label} must not contain `.` or `..` components: {}",
        path.display()
    );
    Ok(())
}

/// Opens `path` without following a final symlink, requires a regular file no
/// larger than `maximum`, and reads it through that same descriptor.
fn read_bounded_no_follow(path: &Path, maximum: usize) -> Result<(Vec<u8>, Metadata)> {
    vet_configured_path(path, "file path")?;
    let fd = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .with_context(|| format!("open {}", path.display()))?;
    let mut file = File::from(fd);
    let metadata = file
        .metadata()
        .with_context(|| format!("inspect {}", path.display()))?;
    ensure!(
        metadata.is_file(),
        "{} must be a regular file",
        path.display()
    );
    let length = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
    ensure!(length <= maximum, "{} exceeds its bound", path.display());
    let mut bytes = Vec::with_capacity(length);
    (&mut file)
        .take(u64::try_from(maximum).unwrap_or(u64::MAX).saturating_add(1))
        .read_to_end(&mut bytes)
        .with_context(|| format!("read {}", path.display()))?;
    ensure!(
        bytes.len() <= maximum,
        "{} exceeds its bound",
        path.display()
    );
    Ok((bytes, metadata))
}

/// Reads a private file that must exist.
///
/// The path must be absolute and normalized, the final component is not
/// followed as a symlink, and the file must be a regular file owned by this
/// process with no group or world permission bits.
///
/// # Errors
///
/// Fails when the file is missing, not owner-private, or larger than `maximum`.
pub fn read_private(path: &Path, maximum: usize) -> Result<Vec<u8>> {
    let (bytes, metadata) = read_bounded_no_follow(path, maximum)?;
    ensure!(
        metadata.mode().trailing_zeros() >= 6
            && metadata.uid() == rustix::process::geteuid().as_raw(),
        "{} must be an owner-private file",
        path.display()
    );
    Ok(bytes)
}

/// Reads a file that is not owner-private (an installed program, a shared
/// public identity) with the same path vetting, no-follow open and size bound
/// as [`read_private`].
///
/// # Errors
///
/// Fails when the path is not absolute and normalized, the file is missing or
/// not regular, or it is larger than `maximum`.
pub fn read_vetted(path: &Path, maximum: usize) -> Result<Vec<u8>> {
    read_bounded_no_follow(path, maximum).map(|(bytes, _)| bytes)
}
