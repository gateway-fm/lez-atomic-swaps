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
use lez_swap_core::path_safety::{PathSafetyError, check_normalized_absolute, join_component};
use rustix::fs::{Mode, OFlags};
use std::fs::{File, Metadata};
use std::io::Read as _;

/// The directory under a swaps root where reservations are composed before
/// one rename publishes each as its swap directory. Its name is no
/// reservation id, so it is never a swap directory, and nothing in it is one.
pub const STAGE_DIRECTORY: &str = ".stage";

/// The files of one swap for one role.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SwapLayout {
    root: PathBuf,
}

impl SwapLayout {
    /// The layout for `reservation_id` under `swaps_root` (not created yet).
    ///
    /// The swap directory is one plain name directly under `swaps_root`.
    ///
    /// # Errors
    ///
    /// Fails when the id is not one plain path component. Defence in depth:
    /// [`RequestId`] already admits only such names.
    pub fn new(swaps_root: &Path, reservation_id: &RequestId) -> Result<Self> {
        let root = join_component(swaps_root, reservation_id.as_str()).with_context(|| {
            format!("{:?} cannot name a swap directory", reservation_id.as_str())
        })?;
        Ok(Self { root })
    }

    /// Creates the swap directory owner-private; fails if it already exists.
    ///
    /// # Errors
    ///
    /// Fails when the directory exists or cannot be created.
    pub fn create(&self) -> Result<()> {
        self.reservation_id()?;
        DirBuilder::new()
            .mode(0o700)
            .create(&self.root)
            .with_context(|| format!("create swap directory {}", self.root.display()))?;
        for sub in ["journals", "lez", "bitcoin", "actor"] {
            DirBuilder::new()
                .mode(0o700)
                .create(join_component(&self.root, sub)?)?;
        }
        Ok(())
    }

    /// The reservation id that names this swap directory.
    ///
    /// Fails for a name outside the reservation-id grammar
    /// ([`RequestId::is_reservation_id`]): no directory is made for it.
    fn reservation_id(&self) -> Result<&str> {
        let name = self
            .root
            .file_name()
            .and_then(|name| name.to_str())
            .context("the swap directory has no name")?;
        ensure!(
            RequestId::new(name).is_ok_and(|id| id.is_reservation_id()),
            "{name:?} is not a reservation id"
        );
        Ok(name)
    }

    /// A fresh, empty, owner-private directory in which this swap's `pair`
    /// reservation is composed: `<swaps root>/.stage/<reservation id>.<pair>`.
    ///
    /// A stale stage of the same reservation and pair (a crash before its
    /// rename) is removed first. The stage lives outside the swaps root's id
    /// namespace, so removing it never touches another swap's directory.
    /// The caller publishes it with one rename onto [`Self::root`].
    ///
    /// # Errors
    ///
    /// Fails for an id outside the reservation-id grammar, a `pair` that is
    /// not lowercase ASCII letters, or a stage directory that cannot be made
    /// owner-private.
    pub fn fresh_stage(&self, pair: &str) -> Result<PathBuf> {
        let name = self.reservation_id()?;
        ensure!(
            !pair.is_empty() && pair.bytes().all(|byte| byte.is_ascii_lowercase()),
            "{pair:?} is not a pair name"
        );
        let stages = join_component(
            self.root
                .parent()
                .context("the swap directory has no swaps root")?,
            STAGE_DIRECTORY,
        )?;
        match DirBuilder::new().mode(0o700).create(&stages) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(error).with_context(|| format!("create {}", stages.display()));
            }
        }
        let metadata = fs::symlink_metadata(&stages)?;
        ensure!(
            metadata.is_dir()
                && metadata.mode().trailing_zeros() >= 6
                && metadata.uid() == rustix::process::geteuid().as_raw(),
            "{} must be an owner-private directory",
            stages.display()
        );
        let stage = join_component(&stages, &format!("{name}.{pair}"))
            .with_context(|| format!("{name:?}.{pair:?} cannot name a stage"))?;
        match fs::remove_dir_all(&stage) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("remove {}", stage.display()));
            }
        }
        DirBuilder::new()
            .mode(0o700)
            .create(&stage)
            .with_context(|| format!("create {}", stage.display()))?;
        Ok(stage)
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
    vet_configured_path(path, "file path")?;
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
            // Written whole, then published by a link that fails if the
            // name exists: a crash leaves no file or the whole one.
            let staging = staging_path(path)?;
            let _ = fs::remove_file(&staging);
            let written = (|| -> Result<()> {
                let mut file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&staging)
                    .with_context(|| format!("create {}", staging.display()))?;
                file.write_all(bytes)?;
                file.sync_all()?;
                drop(file);
                fs::hard_link(&staging, path)
                    .with_context(|| format!("create {}", path.display()))?;
                sync_parent(path)
            })();
            let _ = fs::remove_file(&staging);
            written.map(|()| false)
        }
        Err(error) => Err(error.into()),
    }
}

/// Replaces the owner-private file at `path` with `bytes` as one step: the
/// bytes go to a temporary file in the same directory, which is synced and
/// renamed over `path`, and the directory is synced. A crash leaves the old
/// file or the new one, never a torn one.
///
/// # Errors
///
/// Fails when the temporary file cannot be written, synced or renamed.
pub fn replace_private(path: &Path, bytes: &[u8]) -> Result<()> {
    vet_configured_path(path, "file path")?;
    let staging = staging_path(path)?;
    let _ = fs::remove_file(&staging);
    let written = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&staging)
            .with_context(|| format!("create {}", staging.display()))?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&staging, path).with_context(|| format!("replace {}", path.display()))?;
        sync_parent(path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&staging);
    }
    written
}

/// The temporary name beside `path` a whole write goes through.
fn staging_path(path: &Path) -> Result<PathBuf> {
    let parent = path
        .parent()
        .with_context(|| format!("{} has no directory", path.display()))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .with_context(|| format!("{} has no file name", path.display()))?;
    join_component(parent, &format!(".{name}.tmp-{}", std::process::id()))
        .with_context(|| format!("{} has no staging name", path.display()))
}

fn sync_parent(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("{} has no directory", path.display()))?;
    vet_configured_path(parent, "directory")?;
    File::open(parent)?.sync_all()?;
    Ok(())
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
    match check_normalized_absolute(path) {
        Ok(_) => Ok(()),
        Err(PathSafetyError::NotAbsolute) => Err(anyhow::anyhow!(
            "{label} must be an absolute path: {}",
            path.display()
        )),
        Err(_) => Err(anyhow::anyhow!(
            "{label} must not contain `.` or `..` components: {}",
            path.display()
        )),
    }
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
/// process with no group or world permission bits, or the single-link
/// root-owned credential systemd hands the service (`lez_systemd_credential`).
///
/// # Errors
///
/// Fails when the file is missing, not owner-private, or larger than `maximum`.
pub fn read_private(path: &Path, maximum: usize) -> Result<Vec<u8>> {
    let credential = lez_systemd_credential::is_credential_path(path);
    let (bytes, metadata) = read_bounded_no_follow(path, maximum)?;
    ensure!(
        (metadata.mode().trailing_zeros() >= 6
            && metadata.uid() == rustix::process::geteuid().as_raw())
            || (credential
                && metadata.nlink() == 1
                && lez_systemd_credential::credential_ownership_ok(
                    metadata.uid(),
                    metadata.gid(),
                    metadata.mode() & 0o7777,
                )),
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

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt as _};

    use std::path::Path;

    use lez_bridge_protocol::RequestId;

    use super::{STAGE_DIRECTORY, SwapLayout, read_private, replace_private, write_private_exact};

    /// A replaced file holds the new bytes, stays owner-private, and leaves
    /// no temporary file beside it, also over a stale one from a crash.
    #[test]
    fn a_file_is_replaced_whole_and_private() {
        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path().join("swap");
        private_dir(&root);
        let path = root.join("record.json");
        replace_private(&path, b"first").unwrap();
        fs::write(
            root.join(format!(".record.json.tmp-{}", std::process::id())),
            b"torn",
        )
        .unwrap();
        replace_private(&path, b"second").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let names: Vec<_> = fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, [std::ffi::OsString::from("record.json")]);
    }

    fn private_dir(path: &Path) {
        fs::create_dir(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }

    /// A Taker named one swap `Y.xmr-stage`, then reserves `Y`: the stage of
    /// `Y` is not the first swap's directory, so composing it leaves that
    /// swap whole. The same holds for every pair's stage suffix.
    #[test]
    fn staging_a_reservation_never_touches_a_swap_named_like_its_stage() {
        let run = tempfile::tempdir().unwrap();
        let swaps = run.path().join("swaps");
        private_dir(&swaps);
        let id = RequestId::new("victim-reservation-01").unwrap();
        let layout = SwapLayout::new(&swaps, &id).unwrap();
        for pair in ["xmr", "zec", "btc"] {
            for suffix in [format!(".{pair}-stage"), format!(".{pair}")] {
                let funded = swaps.join(format!("{}{suffix}", id.as_str()));
                private_dir(&funded);
                fs::write(funded.join("role-key"), b"funded").unwrap();
            }
            let stage = layout.fresh_stage(pair).unwrap();
            assert_eq!(
                stage,
                swaps
                    .join(STAGE_DIRECTORY)
                    .join(format!("{}.{pair}", id.as_str()))
            );
            assert!(stage.is_dir() && fs::read_dir(&stage).unwrap().next().is_none());
        }
        for pair in ["xmr", "zec", "btc"] {
            for suffix in [format!(".{pair}-stage"), format!(".{pair}")] {
                let funded = swaps.join(format!("{}{suffix}", id.as_str()));
                assert_eq!(fs::read(funded.join("role-key")).unwrap(), b"funded");
            }
        }
    }

    #[test]
    fn a_stale_stage_is_made_again_empty() {
        let run = tempfile::tempdir().unwrap();
        let swaps = run.path().join("swaps");
        private_dir(&swaps);
        let layout =
            SwapLayout::new(&swaps, &RequestId::new("stale-reservation-1").unwrap()).unwrap();
        let stage = layout.fresh_stage("xmr").unwrap();
        fs::write(stage.join("half"), b"x").unwrap();
        assert_eq!(layout.fresh_stage("xmr").unwrap(), stage);
        assert!(fs::read_dir(&stage).unwrap().next().is_none());
    }

    /// An id outside the reservation-id grammar gets no swap directory and
    /// no stage, whatever made it past the wire.
    #[test]
    fn no_directory_is_made_for_an_id_outside_the_reservation_grammar() {
        let run = tempfile::tempdir().unwrap();
        let swaps = run.path().join("swaps");
        private_dir(&swaps);
        let dotted =
            SwapLayout::new(&swaps, &RequestId::new("victim-01.xmr-stage").unwrap()).unwrap();
        assert!(dotted.create().is_err());
        assert!(dotted.fresh_stage("xmr").is_err());
        assert!(!dotted.root().exists());
        let good =
            SwapLayout::new(&swaps, &RequestId::new("good_reservation-01").unwrap()).unwrap();
        assert!(good.fresh_stage("x.y").is_err());
        assert!(good.fresh_stage("").is_err());
        good.create().unwrap();
    }

    /// A swap directory is always one name directly under the swaps root.
    /// No [`RequestId`] can hold a separator or be `.`/`..`, so no hostile id
    /// can be built here; the grammar's edge cases still land directly under
    /// the root.
    #[test]
    fn a_swap_directory_is_a_direct_child_of_the_swaps_root() {
        let swaps = Path::new("/var/lib/lez/swaps");
        for id in ["........", "..-..-..", "a.b.c.d.e"] {
            let layout = SwapLayout::new(swaps, &RequestId::new(id).unwrap()).unwrap();
            assert_eq!(layout.root(), swaps.join(id));
            assert_eq!(layout.root().parent(), Some(swaps));
        }
        assert!(RequestId::new("../../etc").is_err());
        assert!(RequestId::new("/etc/passwd").is_err());
    }

    /// The write sinks refuse a path that is relative or walks the tree.
    #[test]
    fn writes_refuse_a_path_that_walks_the_tree() {
        let run = tempfile::tempdir().unwrap();
        let root = run.path().join("swap");
        private_dir(&root);
        let walking = root.join("..").join("escaped.json");
        assert!(write_private_exact(&walking, b"x").is_err());
        assert!(replace_private(&walking, b"x").is_err());
        assert!(write_private_exact(Path::new("relative.json"), b"x").is_err());
        assert!(replace_private(Path::new("relative.json"), b"x").is_err());
        assert!(!run.path().join("escaped.json").exists());
        assert!(!Path::new("relative.json").exists());
    }

    #[test]
    fn read_private_keeps_its_owner_rule_outside_the_credential_directory() {
        let run = tempfile::tempdir().unwrap();
        let path = run.path().join("lez-signer.key");
        fs::write(&path, b"key").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(read_private(&path, 8).unwrap(), b"key");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o440)).unwrap();
        assert!(read_private(&path, 8).is_err());
    }
}
