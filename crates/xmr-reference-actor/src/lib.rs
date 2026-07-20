//! Role-fixed process boundary for independently composing M4 XMR material.
//!
//! Each invocation accepts exactly one private role root. Public packets may be
//! exchanged between roles; private signing keys and Monero scalars never
//! cross this crate's output boundary.

#![cfg_attr(not(unix), allow(unused_imports))]

#[cfg(not(unix))]
compile_error!("xmr-reference-actor requires Unix file-permission semantics");

use std::{
    ffi::OsString,
    fs::File,
    io::{Read as _, Write as _},
    os::unix::fs::MetadataExt as _,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, anyhow, ensure};
use clap::{Parser, Subcommand, ValueEnum};
use lez_xmr_swap_sdk::{
    CrossCurveDleqProofV1, CrossCurveScalar, MoneroPrivateViewKey, XmrParticipantIdentityV1,
};
use rustix::{
    fs::{
        AtFlags, CWD, Mode, OFlags, RenameFlags, ResolveFlags, mkdirat, openat, openat2,
        renameat_with, unlinkat,
    },
    io::Errno,
};
use secp256k1::rand::{CryptoRng, RngCore, SeedableRng, rngs::OsRng, rngs::StdRng};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

const ROLE_PACKET_SCHEMA_V1: u16 = 1;
const PRIVATE_MANIFEST_SCHEMA_V1: u16 = 1;
const ROLE_PACKET_MAX_BYTES: u64 = 270 * 1024;
const ROLE_PACKET_MAX_HEX_CHARS: usize = 270 * 1024 * 2;
const PRIVATE_MANIFEST_MAX_BYTES: u64 = 1024;
const PRIVATE_KEY_MAX_BYTES: u64 = 66;
const AGREEMENT_KEY_FILE: &str = "agreement.key";
const CLAIM_KEY_FILE: &str = "claim.key";
const REFUND_KEY_FILE: &str = "refund.key";
const XMR_SHARE_FILE: &str = "xmr-share.key";
const VIEW_KEY_FILE: &str = "monero-view.key";
const PRIVATE_MANIFEST_FILE: &str = "manifest.json";
const PRIVATE_BUNDLE_FILES: [&str; 6] = [
    AGREEMENT_KEY_FILE,
    CLAIM_KEY_FILE,
    REFUND_KEY_FILE,
    XMR_SHARE_FILE,
    VIEW_KEY_FILE,
    PRIVATE_MANIFEST_FILE,
];

/// One private role selected for a process invocation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum ActorRole {
    /// LEZ claimant and Monero funder.
    Maker,
    /// LEZ depositor and Monero claimant.
    Taker,
}

/// One bounded role-process action.
#[derive(Clone, Debug, Subcommand)]
pub enum Action {
    /// Generate fresh private role material and one canonical public packet.
    Provision {
        /// Fixed private role for this root.
        #[arg(value_enum)]
        role: ActorRole,
        /// New role directory under an existing exact owner-only parent.
        #[arg(long, value_name = "NEW_PRIVATE_ROOT")]
        private_root: PathBuf,
        /// Exact lowercase-hex LEZ owner account assigned to this role.
        #[arg(long, value_name = "HEX32")]
        lez_owner_account: String,
        /// Existing owner-private shared view-key handoff; required only by Maker.
        #[arg(long, value_name = "PRIVATE_VIEW_KEY")]
        shared_view_key_file: Option<PathBuf>,
        /// New canonical public role packet under an exact owner-only parent.
        #[arg(long, value_name = "NEW_PUBLIC_JSON")]
        public_packet: PathBuf,
    },
}

/// CLI for one role-fixed material invocation.
#[derive(Clone, Debug, Parser)]
#[command(about = "PoC role-fixed LEZ/XMR stage-material actor")]
pub struct Cli {
    /// Exactly one monotonic material action.
    #[command(subcommand)]
    pub action: Action,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct RolePacketV1 {
    schema_version: u16,
    role: ActorRole,
    lez_owner_account: String,
    agreement_public_key: String,
    claim_session_public_key: String,
    refund_session_public_key: String,
    dleq_proof_wire: String,
    public_view_key: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct PrivateManifestV1 {
    schema_version: u16,
    role: ActorRole,
    lez_owner_account: String,
    public_packet_sha256: String,
}

/// Validated public role packet. Private material is never retained here.
#[derive(Clone, Debug)]
#[must_use]
pub struct ValidatedRolePacket {
    role: ActorRole,
    identity: XmrParticipantIdentityV1,
    proof: CrossCurveDleqProofV1,
    public_view_key: [u8; 32],
}

impl ValidatedRolePacket {
    /// Reads a canonical, bounded public role packet and revalidates its proof.
    ///
    /// # Errors
    ///
    /// Rejects unsafe files, noncanonical JSON/hex, invalid or aliased keys, or
    /// a failed DLEQ proof.
    pub fn read(path: &Path) -> Result<Self> {
        let file = open_path_no_symlinks(path, "public role packet")?;
        let bytes = read_bounded_file(
            file,
            ROLE_PACKET_MAX_BYTES,
            FilePolicy::Public,
            "public role packet",
        )?;
        Self::from_bytes(&bytes)
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let raw: RolePacketV1 =
            serde_json::from_slice(bytes).context("public role packet is malformed")?;
        ensure!(
            raw.canonical_bytes()? == bytes,
            "public role packet is noncanonical"
        );
        ensure!(
            raw.schema_version == ROLE_PACKET_SCHEMA_V1,
            "public role packet schema is unsupported"
        );
        let owner = decode_exact(&raw.lez_owner_account)?;
        ensure!(owner != [0; 32], "public role owner account is invalid");
        let proof_wire = decode_vec(&raw.dleq_proof_wire)?;
        let proof = CrossCurveDleqProofV1::from_wire_bytes(&proof_wire)
            .context("public role DLEQ proof is invalid")?;
        let signing_keys = [
            decode_public_key(&raw.agreement_public_key)?,
            decode_public_key(&raw.claim_session_public_key)?,
            decode_public_key(&raw.refund_session_public_key)?,
        ];
        validate_intra_role_keys(signing_keys, &proof)?;
        let identity =
            XmrParticipantIdentityV1::new(owner, signing_keys[0], signing_keys[1], signing_keys[2]);
        let public_view_key = decode_exact(&raw.public_view_key)?;
        Ok(Self {
            role: raw.role,
            identity,
            proof,
            public_view_key,
        })
    }

    /// Role that produced this packet.
    #[must_use]
    pub const fn role(&self) -> ActorRole {
        self.role
    }

    /// Exact public identity committed into Stage A.
    #[must_use]
    pub const fn identity(&self) -> &XmrParticipantIdentityV1 {
        &self.identity
    }

    /// Verified public cross-curve proof owned by this role.
    #[must_use]
    pub const fn proof(&self) -> &CrossCurveDleqProofV1 {
        &self.proof
    }

    /// Public half of the shared Monero view key.
    #[must_use]
    pub const fn public_view_key(&self) -> [u8; 32] {
        self.public_view_key
    }
}

/// Validated role/owner/public-packet binding retained in one private root.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct ValidatedPrivateManifest {
    role: ActorRole,
    lez_owner_account: [u8; 32],
    public_packet_sha256: [u8; 32],
}

impl ValidatedPrivateManifest {
    /// Opens an exact owner-only root and reads its canonical manifest without
    /// following symbolic links.
    ///
    /// # Errors
    ///
    /// Rejects unsafe roots/files and malformed or noncanonical manifests.
    pub fn read(private_root: &Path) -> Result<Self> {
        let root = open_private_directory(private_root, "private role root")?;
        let manifest = openat(
            &root,
            PRIVATE_MANIFEST_FILE,
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map(File::from)
        .map_err(|_| anyhow!("private manifest is unavailable"))?;
        let bytes = read_bounded_file(
            manifest,
            PRIVATE_MANIFEST_MAX_BYTES,
            FilePolicy::Private,
            "private manifest",
        )?;
        let raw: PrivateManifestV1 =
            serde_json::from_slice(&bytes).context("private manifest is malformed")?;
        ensure!(
            raw.canonical_bytes()? == bytes,
            "private manifest is noncanonical"
        );
        ensure!(
            raw.schema_version == PRIVATE_MANIFEST_SCHEMA_V1,
            "private manifest schema is unsupported"
        );
        let owner = decode_exact(&raw.lez_owner_account)?;
        ensure!(owner != [0; 32], "private manifest owner is invalid");
        Ok(Self {
            role: raw.role,
            lez_owner_account: owner,
            public_packet_sha256: decode_exact(&raw.public_packet_sha256)?,
        })
    }

    /// Private role durably bound to this root.
    #[must_use]
    pub const fn role(&self) -> ActorRole {
        self.role
    }

    /// Exact LEZ owner durably bound to this root.
    #[must_use]
    pub const fn lez_owner_account(&self) -> [u8; 32] {
        self.lez_owner_account
    }

    /// SHA-256 of the exact canonical public packet bytes.
    #[must_use]
    pub const fn public_packet_sha256(&self) -> [u8; 32] {
        self.public_packet_sha256
    }
}

impl RolePacketV1 {
    fn canonical_bytes(&self) -> Result<Vec<u8>> {
        canonical_json_bytes(self, "encode public role packet")
    }
}

impl PrivateManifestV1 {
    fn canonical_bytes(&self) -> Result<Vec<u8>> {
        canonical_json_bytes(self, "encode private manifest")
    }
}

/// Executes one role-fixed material command.
///
/// # Errors
///
/// Returns a redacted error when private-file, randomness, proof, or
/// public-packet validation fails.
pub fn execute(cli: Cli) -> Result<()> {
    match cli.action {
        Action::Provision {
            role,
            private_root,
            lez_owner_account,
            shared_view_key_file,
            public_packet,
        } => provision(
            role,
            &private_root,
            &lez_owner_account,
            shared_view_key_file.as_deref(),
            &public_packet,
        ),
    }
}

fn provision(
    role: ActorRole,
    private_root: &Path,
    lez_owner_account: &str,
    shared_view_key_file: Option<&Path>,
    public_packet: &Path,
) -> Result<()> {
    ensure!(
        matches!(
            (role, shared_view_key_file),
            (ActorRole::Maker, Some(_)) | (ActorRole::Taker, None)
        ),
        "Maker must import and Taker must generate the shared view key"
    );
    let lez_owner_account: [u8; 32] = decode_exact(lez_owner_account)?;
    ensure!(lez_owner_account != [0; 32], "LEZ owner account is invalid");

    let private_destination = SecureDestination::new(private_root, "private role root")?;
    private_destination.ensure_absent("private role root")?;
    let public_destination = SecureDestination::new(public_packet, "public packet")?;
    public_destination.ensure_absent("public packet")?;

    let view_key = match shared_view_key_file {
        Some(path) => read_private_view_key(path)?,
        None => MoneroPrivateViewKey::generate().context("generate private Monero view key")?,
    };
    let mut rng = fallible_seeded_rng()?;
    let agreement = GeneratedSecpKey::generate(&mut rng);
    let claim = GeneratedSecpKey::generate(&mut rng);
    let refund = GeneratedSecpKey::generate(&mut rng);
    let share = CrossCurveScalar::generate().context("generate private Monero share")?;
    let proof =
        CrossCurveDleqProofV1::prove(&share, &mut rng).context("create cross-curve proof")?;
    let public_view_key = view_key.public_key();

    let secp = Secp256k1::new();
    let packet = RolePacketV1 {
        schema_version: ROLE_PACKET_SCHEMA_V1,
        role,
        lez_owner_account: hex::encode(lez_owner_account),
        agreement_public_key: hex::encode(agreement.public_key(&secp).serialize()),
        claim_session_public_key: hex::encode(claim.public_key(&secp).serialize()),
        refund_session_public_key: hex::encode(refund.public_key(&secp).serialize()),
        dleq_proof_wire: hex::encode(proof.to_wire_bytes().context("encode public DLEQ proof")?),
        public_view_key: hex::encode(public_view_key),
    };
    let packet_bytes = packet.canonical_bytes()?;
    let validated = ValidatedRolePacket::from_bytes(&packet_bytes)?;
    ensure!(
        validated.role == role
            && validated.identity.lez_owner_account() == lez_owner_account
            && validated.public_view_key == public_view_key,
        "generated public role packet changed identity"
    );
    let packet_digest: [u8; 32] = Sha256::digest(&packet_bytes).into();
    let manifest = PrivateManifestV1 {
        schema_version: PRIVATE_MANIFEST_SCHEMA_V1,
        role,
        lez_owner_account: hex::encode(lez_owner_account),
        public_packet_sha256: hex::encode(packet_digest),
    };
    let manifest_bytes = manifest.canonical_bytes()?;

    let (public_stage_name, public_stage_file) =
        create_staged_file(&public_destination.parent, "public", &packet_bytes)?;
    let private_result = publish_private_bundle(
        &private_destination,
        &agreement,
        &claim,
        &refund,
        share,
        view_key,
        &manifest_bytes,
    );
    if let Err(error) = private_result {
        cleanup_staged_file(&public_destination.parent, &public_stage_name);
        return Err(error);
    }

    let publish_result = publish_staged_file(
        &public_destination,
        &public_stage_name,
        &public_stage_file,
        "public packet",
    );
    if publish_result.is_err() {
        cleanup_staged_file(&public_destination.parent, &public_stage_name);
    }
    publish_result
}

fn publish_private_bundle(
    destination: &SecureDestination,
    agreement: &GeneratedSecpKey,
    claim: &GeneratedSecpKey,
    refund: &GeneratedSecpKey,
    share: CrossCurveScalar,
    view_key: MoneroPrivateViewKey,
    manifest_bytes: &[u8],
) -> Result<()> {
    destination.revalidate()?;
    let (stage_name, stage) = create_staging_directory(&destination.parent)?;
    let mut published = false;
    let result = (|| {
        write_secret_hex_new_at(&stage, AGREEMENT_KEY_FILE, agreement.secret_bytes())?;
        write_secret_hex_new_at(&stage, CLAIM_KEY_FILE, claim.secret_bytes())?;
        write_secret_hex_new_at(&stage, REFUND_KEY_FILE, refund.secret_bytes())?;
        let share_bytes = share.into_monero_little_endian();
        write_secret_hex_new_at(&stage, XMR_SHARE_FILE, &share_bytes)?;
        drop(share_bytes);
        let view_bytes = view_key.into_monero_little_endian();
        write_secret_hex_new_at(&stage, VIEW_KEY_FILE, &view_bytes)?;
        drop(view_bytes);
        write_new_at(
            &stage,
            PRIVATE_MANIFEST_FILE,
            manifest_bytes,
            "private manifest",
        )?;
        stage.sync_all().context("sync private staging directory")?;
        validate_owner_directory(&stage, "private staging directory")?;
        destination.revalidate()?;
        renameat_with(
            &destination.parent,
            stage_name.as_str(),
            &destination.parent,
            &destination.name,
            RenameFlags::NOREPLACE,
        )
        .map_err(|error| {
            if error == Errno::EXIST {
                anyhow!("private role root already exists")
            } else {
                anyhow!("publish private role root failed")
            }
        })?;
        published = true;
        destination
            .parent
            .sync_all()
            .context("sync private role parent")?;
        destination.revalidate()?;
        Ok(())
    })();
    if result.is_err() && !published {
        cleanup_staging_directory(&destination.parent, &stage_name, &stage);
    }
    result
}

fn publish_staged_file(
    destination: &SecureDestination,
    stage_name: &str,
    stage_file: &File,
    label: &'static str,
) -> Result<()> {
    destination.revalidate()?;
    validate_file(
        stage_file,
        FilePolicy::Private,
        ROLE_PACKET_MAX_BYTES,
        label,
    )?;
    renameat_with(
        &destination.parent,
        stage_name,
        &destination.parent,
        &destination.name,
        RenameFlags::NOREPLACE,
    )
    .map_err(|error| {
        if error == Errno::EXIST {
            anyhow!("public packet already exists")
        } else {
            anyhow!("publish public packet failed")
        }
    })?;
    destination
        .parent
        .sync_all()
        .context("sync public packet parent")?;
    destination.revalidate()
}

fn read_private_view_key(path: &Path) -> Result<MoneroPrivateViewKey> {
    let file = open_path_no_symlinks(path, "private key file")?;
    let encoded = Zeroizing::new(read_bounded_file(
        file,
        PRIVATE_KEY_MAX_BYTES,
        FilePolicy::Private,
        "private key file",
    )?);
    let trimmed = encoded
        .strip_suffix(b"\r\n")
        .or_else(|| encoded.strip_suffix(b"\n"))
        .unwrap_or(&encoded);
    let bytes = decode_secret_exact(trimmed)?;
    MoneroPrivateViewKey::from_monero_little_endian(*bytes)
        .context("private Monero view key is invalid")
}

fn validate_intra_role_keys(encoded: [[u8; 33]; 3], proof: &CrossCurveDleqProofV1) -> Result<()> {
    ensure!(
        encoded[0] != encoded[1] && encoded[0] != encoded[2] && encoded[1] != encoded[2],
        "public role signing keys are aliased"
    );
    let parsed = encoded.map(|bytes| {
        PublicKey::from_slice(&bytes).expect("public role keys were parsed before validation")
    });
    let x_only = parsed.map(|key| key.x_only_public_key().0.serialize());
    ensure!(
        x_only[0] != x_only[1] && x_only[0] != x_only[2] && x_only[1] != x_only[2],
        "public role signing keys have aliased x-only identities"
    );
    let proof_x_only = PublicKey::from_slice(&proof.secp256k1_public_key())
        .context("public role DLEQ point is invalid")?
        .x_only_public_key()
        .0
        .serialize();
    ensure!(
        !x_only.contains(&proof_x_only),
        "public role DLEQ point aliases a signing key"
    );
    Ok(())
}

struct GeneratedSecpKey {
    key: SecretKey,
    bytes: Zeroizing<[u8; 32]>,
}

impl GeneratedSecpKey {
    fn generate(rng: &mut (impl CryptoRng + RngCore)) -> Self {
        loop {
            let mut bytes = Zeroizing::new([0; 32]);
            rng.fill_bytes(&mut *bytes);
            if let Ok(key) = SecretKey::from_slice(bytes.as_ref()) {
                return Self { key, bytes };
            }
        }
    }

    fn secret_bytes(&self) -> &[u8; 32] {
        &self.bytes
    }

    fn public_key(&self, secp: &Secp256k1<secp256k1::All>) -> PublicKey {
        PublicKey::from_secret_key(secp, &self.key)
    }
}

impl Drop for GeneratedSecpKey {
    fn drop(&mut self) {
        self.key.non_secure_erase();
    }
}

fn fallible_seeded_rng() -> Result<StdRng> {
    let mut seed = Zeroizing::new([0; 32]);
    OsRng
        .try_fill_bytes(&mut *seed)
        .context("operating-system entropy is unavailable")?;
    Ok(StdRng::from_seed(*seed))
}

struct SecureDestination {
    parent: File,
    name: OsString,
}

impl SecureDestination {
    fn new(path: &Path, label: &'static str) -> Result<Self> {
        let name = path
            .file_name()
            .filter(|name| !name.is_empty())
            .ok_or_else(|| anyhow!("{label} path is invalid"))?
            .to_os_string();
        let parent_path = match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent,
            Some(_) | None => Path::new("."),
        };
        let parent = open_private_directory(parent_path, "destination parent")?;
        Ok(Self { parent, name })
    }

    fn ensure_absent(&self, label: &'static str) -> Result<()> {
        self.revalidate()?;
        match openat(
            &self.parent,
            &self.name,
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Err(Errno::NOENT) => Ok(()),
            Ok(_) | Err(Errno::LOOP) => Err(anyhow!("{label} already exists")),
            Err(_) => Err(anyhow!("{label} destination is unsafe")),
        }
    }

    fn revalidate(&self) -> Result<()> {
        validate_owner_directory(&self.parent, "destination parent")
    }
}

fn open_private_directory(path: &Path, label: &'static str) -> Result<File> {
    let file = openat2(
        CWD,
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::NO_SYMLINKS,
    )
    .map(File::from)
    .map_err(|_| anyhow!("{label} is unavailable or unsafe"))?;
    validate_owner_directory(&file, label)?;
    Ok(file)
}

fn validate_owner_directory(file: &File, label: &'static str) -> Result<()> {
    let metadata = file
        .metadata()
        .map_err(|_| anyhow!("{label} is unavailable or unsafe"))?;
    ensure!(
        metadata.file_type().is_dir()
            && metadata.uid() == effective_uid()
            && metadata.mode() & 0o7777 == 0o700,
        "{label} is not an exact owner-only directory"
    );
    Ok(())
}

fn effective_uid() -> u32 {
    rustix::process::geteuid().as_raw()
}

fn open_path_no_symlinks(path: &Path, label: &'static str) -> Result<File> {
    openat2(
        CWD,
        path,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
        ResolveFlags::NO_SYMLINKS,
    )
    .map(File::from)
    .map_err(|_| anyhow!("{label} is unavailable or unsafe"))
}

#[derive(Clone, Copy)]
enum FilePolicy {
    Public,
    Private,
}

fn read_bounded_file(
    mut file: File,
    max_bytes: u64,
    policy: FilePolicy,
    label: &'static str,
) -> Result<Vec<u8>> {
    let before = validate_file(&file, policy, max_bytes, label)?;
    let mut bytes = Vec::new();
    std::io::Read::by_ref(&mut file)
        .take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow!("read {label} failed"))?;
    ensure!(
        u64::try_from(bytes.len()).unwrap_or(u64::MAX) <= max_bytes,
        "{label} is oversized"
    );
    let after = validate_file(&file, policy, max_bytes, label)?;
    ensure!(
        before.dev() == after.dev()
            && before.ino() == after.ino()
            && before.len() == after.len()
            && after.len() == u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        "{label} changed while it was read"
    );
    Ok(bytes)
}

fn validate_file(
    file: &File,
    policy: FilePolicy,
    max_bytes: u64,
    label: &'static str,
) -> Result<std::fs::Metadata> {
    let metadata = file
        .metadata()
        .map_err(|_| anyhow!("inspect {label} failed"))?;
    let private_ok = match policy {
        FilePolicy::Public => true,
        FilePolicy::Private => {
            metadata.uid() == effective_uid() && metadata.mode() & 0o7777 == 0o600
        }
    };
    ensure!(
        metadata.file_type().is_file()
            && metadata.nlink() == 1
            && metadata.len() <= max_bytes
            && private_ok,
        "{label} is unsafe or oversized"
    );
    Ok(metadata)
}

fn create_staging_directory(parent: &File) -> Result<(String, File)> {
    let stage_name = temporary_name("private")?;
    mkdirat(
        parent,
        stage_name.as_str(),
        Mode::RUSR | Mode::WUSR | Mode::XUSR,
    )
    .map_err(|_| anyhow!("create private staging directory failed"))?;
    let open_result = openat(
        parent,
        stage_name.as_str(),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|_| anyhow!("open private staging directory failed"));
    let stage = match open_result {
        Ok(stage) => stage,
        Err(error) => {
            let _ = unlinkat(parent, stage_name.as_str(), AtFlags::REMOVEDIR);
            let _ = parent.sync_all();
            return Err(error);
        }
    };
    if let Err(error) = validate_owner_directory(&stage, "private staging directory") {
        let _ = unlinkat(parent, stage_name.as_str(), AtFlags::REMOVEDIR);
        let _ = parent.sync_all();
        return Err(error);
    }
    Ok((stage_name, stage))
}

fn create_staged_file(parent: &File, kind: &'static str, bytes: &[u8]) -> Result<(String, File)> {
    let stage_name = temporary_name(kind)?;
    let mut file = openat(
        parent,
        stage_name.as_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    )
    .map(File::from)
    .map_err(|_| anyhow!("create staged public packet failed"))?;
    let result = (|| {
        validate_file(
            &file,
            FilePolicy::Private,
            ROLE_PACKET_MAX_BYTES,
            "staged public packet",
        )?;
        file.write_all(bytes)
            .context("write staged public packet")?;
        file.sync_all().context("sync staged public packet")?;
        validate_file(
            &file,
            FilePolicy::Private,
            ROLE_PACKET_MAX_BYTES,
            "staged public packet",
        )?;
        parent.sync_all().context("sync public packet staging")
    })();
    if let Err(error) = result {
        cleanup_staged_file(parent, &stage_name);
        return Err(error);
    }
    Ok((stage_name, file))
}

fn write_secret_hex_new_at(directory: &File, name: &str, bytes: &[u8; 32]) -> Result<()> {
    let mut encoded = Zeroizing::new(hex::encode(bytes));
    encoded.push('\n');
    write_new_at(directory, name, encoded.as_bytes(), "private material")
}

fn write_new_at(directory: &File, name: &str, bytes: &[u8], label: &'static str) -> Result<()> {
    let mut file = openat(
        directory,
        name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    )
    .map(File::from)
    .map_err(|_| anyhow!("create new {label} failed"))?;
    validate_file(
        &file,
        FilePolicy::Private,
        u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        label,
    )?;
    file.write_all(bytes)
        .map_err(|_| anyhow!("write {label} failed"))?;
    file.sync_all()
        .map_err(|_| anyhow!("sync {label} failed"))?;
    let metadata = validate_file(
        &file,
        FilePolicy::Private,
        u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        label,
    )?;
    ensure!(
        metadata.len() == u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        "written {label} has the wrong length"
    );
    Ok(())
}

fn cleanup_staged_file(parent: &File, stage_name: &str) {
    let _ = unlinkat(parent, stage_name, AtFlags::empty());
    let _ = parent.sync_all();
}

fn cleanup_staging_directory(parent: &File, stage_name: &str, stage: &File) {
    for name in PRIVATE_BUNDLE_FILES {
        let _ = unlinkat(stage, name, AtFlags::empty());
    }
    let _ = unlinkat(parent, stage_name, AtFlags::REMOVEDIR);
    let _ = parent.sync_all();
}

fn temporary_name(kind: &str) -> Result<String> {
    let mut random = Zeroizing::new([0; 16]);
    OsRng
        .try_fill_bytes(&mut *random)
        .context("operating-system entropy is unavailable")?;
    Ok(format!(
        ".xmr-reference-actor-{kind}-{}-{}",
        std::process::id(),
        hex::encode(*random)
    ))
}

fn canonical_json_bytes(value: &impl Serialize, label: &'static str) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(value).with_context(|| label)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn decode_public_key(encoded: &str) -> Result<[u8; 33]> {
    let bytes = decode_exact(encoded)?;
    PublicKey::from_slice(&bytes).context("public role key is invalid")?;
    Ok(bytes)
}

fn decode_exact<const N: usize>(encoded: &str) -> Result<[u8; N]> {
    ensure!(
        encoded.len() == N * 2
            && encoded
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "canonical lowercase hex is invalid"
    );
    hex::decode(encoded)
        .context("canonical lowercase hex is invalid")?
        .try_into()
        .map_err(|_| anyhow!("canonical lowercase hex has the wrong width"))
}

fn decode_secret_exact(encoded: &[u8]) -> Result<Zeroizing<[u8; 32]>> {
    ensure!(
        encoded.len() == 64
            && encoded
                .iter()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte)),
        "canonical private hex is invalid"
    );
    let mut result = Zeroizing::new([0; 32]);
    for (index, pair) in encoded.chunks_exact(2).enumerate() {
        result[index] = (hex_nibble(pair[0]) << 4) | hex_nibble(pair[1]);
    }
    Ok(result)
}

fn hex_nibble(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        _ => unreachable!("private hex was validated before decoding"),
    }
}

fn decode_vec(encoded: &str) -> Result<Vec<u8>> {
    ensure!(
        encoded.len().is_multiple_of(2)
            && encoded.len() <= ROLE_PACKET_MAX_HEX_CHARS
            && encoded
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "canonical lowercase hex is invalid"
    );
    hex::decode(encoded).context("canonical lowercase hex is invalid")
}
