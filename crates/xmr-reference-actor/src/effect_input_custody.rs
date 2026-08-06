use std::{
    collections::BTreeSet,
    fmt,
    fs::{self, File},
    io::{Read as _, Seek as _, Write as _},
    os::{
        fd::AsRawFd as _,
        unix::fs::{MetadataExt as _, PermissionsExt as _},
    },
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context as _, Result, ensure};
use lez_bridge_adapter::XmrLezBridgeBindingV3;
use lez_bridge_protocol::{
    MessageContext, Participant as BridgeParticipant, RequestId, RunId, RuntimeDescriptor,
    XmrNativeEscrowTermsV3,
};
#[cfg(feature = "sessions")]
use lez_xmr_release_authority::{PublicationProtectionKey, ReleaseStore};
use lez_xmr_swap_sdk::{MoneroPrivateViewKey, XmrActivatedAgreementV1, XmrAgreementV1};
use rustix::fs::{MemfdFlags, SealFlags, fcntl_add_seals, fcntl_get_seals, memfd_create};
use rustix::io::fcntl_dupfd_cloexec;
use sha2::{Digest as _, Sha256};
use zeroize::{Zeroize as _, Zeroizing};

#[cfg(feature = "sessions")]
use lez_swap_store::{MakerActorHeldLock, PinnedChildFdPlan, PinnedExecutable};

use crate::{
    ActorRole, ValidatedXmrEffectAuthorityV1, ValidatedXmrEffectExecutionV3,
    XmrEffectAuthenticatedRpcV1, XmrEffectChildModeV1, XmrTag14ReleaseNodeProfileV1,
    application_provision::ValidatedXmrEffectApplicationV1, open_path_no_symlinks,
    open_private_directory,
};
#[cfg(feature = "sessions")]
use lez_swap_store::XmrWorkflowStep;

const MAX_RUNTIME_BYTES: u64 = 16 * 1024;
const MAX_SECRET_BYTES: u64 = 256;
const MAX_FINAL_SIGNATURE_PACKET_BYTES: u64 = 4 * 1024;

/// Fixed child descriptor containing exact LEZ runtime bytes.
pub const XMR_EFFECT_RUNTIME_FD: i32 = 200;
/// Fixed child descriptor containing the LEZ capability.
pub const XMR_EFFECT_CAPABILITY_FD: i32 = 201;
/// Fixed child descriptor containing the Monero daemon username.
pub const XMR_EFFECT_DAEMON_USERNAME_FD: i32 = 202;
/// Fixed child descriptor containing the Monero daemon password.
pub const XMR_EFFECT_DAEMON_PASSWORD_FD: i32 = 203;
/// Fixed child descriptor containing the funding-wallet username.
pub const XMR_EFFECT_FUNDING_USERNAME_FD: i32 = 204;
/// Fixed child descriptor containing the funding-wallet password.
pub const XMR_EFFECT_FUNDING_PASSWORD_FD: i32 = 205;
/// Fixed child descriptor containing the shared-wallet username.
pub const XMR_EFFECT_SHARED_USERNAME_FD: i32 = 206;
/// Fixed child descriptor containing the shared-wallet password.
pub const XMR_EFFECT_SHARED_PASSWORD_FD: i32 = 207;
/// Fixed child descriptor containing the role-wallet username.
pub const XMR_EFFECT_ROLE_USERNAME_FD: i32 = 208;
/// Fixed child descriptor containing the role-wallet password.
pub const XMR_EFFECT_ROLE_PASSWORD_FD: i32 = 209;
/// Fixed child descriptor containing the shared-wallet file password.
pub const XMR_EFFECT_SHARED_WALLET_FILE_PASSWORD_FD: i32 = 210;
/// Fixed child descriptor containing exact validated Stage-A wire bytes.
pub const XMR_EFFECT_STAGE_A_FD: i32 = 211;
/// Fixed child descriptor containing exact validated Stage-B wire bytes.
pub const XMR_EFFECT_STAGE_B_FD: i32 = 212;
/// Fixed child descriptor containing the local role's public packet.
pub const XMR_EFFECT_OWN_PUBLIC_PACKET_FD: i32 = 213;
/// Fixed child descriptor containing the peer role's public packet.
pub const XMR_EFFECT_PEER_PUBLIC_PACKET_FD: i32 = 214;
/// Fixed child descriptor containing the validated private-role manifest.
pub const XMR_EFFECT_PRIVATE_MANIFEST_FD: i32 = 215;
/// Fixed child descriptor containing the validated private Monero view key.
pub const XMR_EFFECT_PRIVATE_VIEW_KEY_FD: i32 = 216;
/// Fixed child descriptor containing the canonical secret-free execution plan.
pub const XMR_EFFECT_CHILD_PLAN_FD: i32 = 217;
/// Invocation-only private XMR spend share for Tag16 and Monero sweep senders.
pub const XMR_EFFECT_PRIVATE_XMR_SHARE_FD: i32 = 218;
/// Invocation-only finalized Tag16 signature packet for Maker refund extraction.
pub const XMR_EFFECT_FINALIZED_REFUND_SIGNATURE_FD: i32 = 219;
/// Fixed owner-private artifact name ingested from finalized Tag16 evidence.
pub const XMR_EFFECT_FINALIZED_REFUND_SIGNATURE_FILE: &str = "finalized-refund-signature.json";
/// Typed schema-v2 Tag14 release invocation.
pub const XMR_TAG14_RELEASE_INVOCATION_FD: i32 = 220;
/// Release-only sidecar capability.
pub const XMR_TAG14_RELEASE_CAPABILITY_FD: i32 = 221;
/// Release-journal protection key.
pub const XMR_TAG14_RELEASE_PROTECTION_KEY_FD: i32 = 222;
/// Already-open owner-private release state directory.
pub const XMR_TAG14_RELEASE_STATE_DIRECTORY_FD: i32 = 223;
/// Exact authenticated Tag14 transaction for owner-side finality observation.
pub const XMR_EFFECT_TAG14_EXACT_TRANSACTION_FD: i32 = 224;

const XMR_RELEASE_JOURNAL_NAME: &str = "xmr-release.sqlite3";

struct PinnedXmrEffectApplicationInputsV1 {
    stage_a: File,
    stage_b: File,
    own_public_packet: File,
    peer_public_packet: File,
    private_manifest: File,
    private_view_key: File,
}

#[derive(serde::Serialize)]
#[serde(deny_unknown_fields)]
struct XmrTag14ReleaseInvocationWireV2 {
    schema_version: u16,
    mode: &'static str,
    public_config: XmrTag14ReleasePublicConfigWireV1,
}

#[derive(serde::Serialize)]
#[serde(deny_unknown_fields)]
struct XmrTag14ReleasePublicConfigWireV1 {
    schema_version: u16,
    sidecar_endpoint: String,
    indexer_endpoint: String,
    node_profile: &'static str,
    run_id: RunId,
    runtime: RuntimeDescriptor,
    terms: XmrNativeEscrowTermsV3,
    protection_key_id: String,
}

/// Invocation-specific custody for the semantic Taker Tag14 release worker.
#[must_use]
pub(crate) struct PinnedXmrTag14ReleaseInputsV1 {
    invocation: File,
    capability: File,
    protection_key: File,
    state_directory: File,
}

impl fmt::Debug for PinnedXmrTag14ReleaseInputsV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PinnedXmrTag14ReleaseInputsV1")
            .field("invocation", &"[SEALED]")
            .field("capability", &"[REDACTED; SEALED]")
            .field("protection_key", &"[REDACTED; SEALED]")
            .field("state_directory", &"[OPEN DIRECTORY]")
            .finish()
    }
}

impl PinnedXmrTag14ReleaseInputsV1 {
    /// Consumes the release-only inputs into one no-argument child mapping.
    pub(crate) fn into_command(
        self,
        executable: PinnedExecutable,
        actor_lock: &MakerActorHeldLock,
        workflow_lock: &MakerActorHeldLock,
    ) -> Result<Command> {
        let descriptors = vec![
            (self.invocation, XMR_TAG14_RELEASE_INVOCATION_FD),
            (self.capability, XMR_TAG14_RELEASE_CAPABILITY_FD),
            (self.protection_key, XMR_TAG14_RELEASE_PROTECTION_KEY_FD),
            (self.state_directory, XMR_TAG14_RELEASE_STATE_DIRECTORY_FD),
        ];
        let plan = PinnedChildFdPlan::new(descriptors)
            .context("validate XMR Tag14 release descriptor plan")?;
        executable
            .into_command_with_locks_and_fd_plan(actor_lock, workflow_lock, plan)
            .context("compose XMR Tag14 release command")
    }
}

/// One immutable secret snapshot intended for descriptor-path child handoff.
#[must_use]
pub struct PinnedXmrEffectSecretV1 {
    snapshot: File,
    child_path: PathBuf,
    redacted_len: usize,
    sha256: [u8; 32],
}

impl fmt::Debug for PinnedXmrEffectSecretV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PinnedXmrEffectSecretV1")
            .field("child_path", &self.child_path)
            .field("redacted_len", &self.redacted_len)
            .field("sha256", &hex::encode(self.sha256))
            .field("value", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl PinnedXmrEffectSecretV1 {
    /// Parent-process descriptor path for validating the retained snapshot.
    ///
    /// Child execution uses a fixed role-specific descriptor instead.
    #[must_use]
    pub fn child_path(&self) -> &Path {
        &self.child_path
    }

    /// Original byte length without exposing contents.
    #[must_use]
    pub const fn redacted_len(&self) -> usize {
        self.redacted_len
    }

    /// SHA-256 of the exact immutable snapshot.
    #[must_use]
    pub const fn sha256(&self) -> [u8; 32] {
        self.sha256
    }

    fn into_snapshot(self) -> File {
        self.snapshot
    }
}

/// Pinned username/password pair for one authenticated RPC.
#[derive(Debug)]
#[must_use]
pub struct PinnedXmrEffectRpcCredentialsV1 {
    username: PinnedXmrEffectSecretV1,
    password: PinnedXmrEffectSecretV1,
}

impl PinnedXmrEffectRpcCredentialsV1 {
    /// Username snapshot.
    pub const fn username(&self) -> &PinnedXmrEffectSecretV1 {
        &self.username
    }

    /// Password snapshot.
    pub const fn password(&self) -> &PinnedXmrEffectSecretV1 {
        &self.password
    }
}

/// Role-separated pinned Monero RPC credentials.
#[derive(Debug)]
#[must_use]
pub struct PinnedXmrEffectMoneroCredentialsV1 {
    daemon: PinnedXmrEffectRpcCredentialsV1,
    funding_wallet: PinnedXmrEffectRpcCredentialsV1,
    shared_wallet: PinnedXmrEffectRpcCredentialsV1,
    role_wallet: PinnedXmrEffectRpcCredentialsV1,
    shared_wallet_file_password: PinnedXmrEffectSecretV1,
}

impl PinnedXmrEffectMoneroCredentialsV1 {
    /// Official daemon credentials.
    pub const fn daemon(&self) -> &PinnedXmrEffectRpcCredentialsV1 {
        &self.daemon
    }

    /// Maker funding/mining wallet credentials.
    pub const fn funding_wallet(&self) -> &PinnedXmrEffectRpcCredentialsV1 {
        &self.funding_wallet
    }

    /// Neutral shared-wallet credentials.
    pub const fn shared_wallet(&self) -> &PinnedXmrEffectRpcCredentialsV1 {
        &self.shared_wallet
    }

    /// Local-role destination wallet credentials.
    pub const fn role_wallet(&self) -> &PinnedXmrEffectRpcCredentialsV1 {
        &self.role_wallet
    }

    /// Reconstructed shared-wallet file-password snapshot.
    pub const fn shared_wallet_file_password(&self) -> &PinnedXmrEffectSecretV1 {
        &self.shared_wallet_file_password
    }
}

/// Immutable runtime and secret snapshots for one validated effect authority.
#[must_use]
pub struct PinnedXmrEffectInputsV1 {
    runtime_bytes: Vec<u8>,
    runtime_snapshot: File,
    capability: PinnedXmrEffectSecretV1,
    monero: PinnedXmrEffectMoneroCredentialsV1,
    application: Option<PinnedXmrEffectApplicationInputsV1>,
    child_plan: Option<File>,
    invocation_xmr_share: Option<File>,
    invocation_refund_signature: Option<File>,
    tag14_exact_transaction: Option<File>,
}

impl fmt::Debug for PinnedXmrEffectInputsV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PinnedXmrEffectInputsV1")
            .field("runtime_len", &self.runtime_bytes.len())
            .field(
                "runtime_sha256",
                &hex::encode(Sha256::digest(&self.runtime_bytes)),
            )
            .field("capability", &self.capability)
            .field("monero", &self.monero)
            .field(
                "application_material",
                &self.application.as_ref().map(|_| "[REDACTED; SEALED]"),
            )
            .field("child_plan", &self.child_plan.as_ref().map(|_| "[SEALED]"))
            .field(
                "invocation_xmr_share",
                &self.invocation_xmr_share.as_ref().map(|_| "[SEALED]"),
            )
            .field(
                "invocation_refund_signature",
                &self
                    .invocation_refund_signature
                    .as_ref()
                    .map(|_| "[SEALED]"),
            )
            .field(
                "tag14_exact_transaction",
                &self.tag14_exact_transaction.as_ref().map(|_| "[SEALED]"),
            )
            .finish_non_exhaustive()
    }
}

impl PinnedXmrEffectInputsV1 {
    /// Exact hash-pinned LEZ runtime bytes.
    #[must_use]
    pub fn runtime_bytes(&self) -> &[u8] {
        &self.runtime_bytes
    }

    /// Current sealed LEZ capability snapshot.
    pub const fn capability(&self) -> &PinnedXmrEffectSecretV1 {
        &self.capability
    }

    /// Current sealed role-separated Monero credential snapshots.
    pub const fn monero(&self) -> &PinnedXmrEffectMoneroCredentialsV1 {
        &self.monero
    }

    pub(crate) fn with_application_material(
        mut self,
        application: &ValidatedXmrEffectApplicationV1,
    ) -> Result<Self> {
        ensure!(
            self.application.is_none(),
            "XMR effect application material is already pinned"
        );
        self.application = Some(PinnedXmrEffectApplicationInputsV1 {
            stage_a: seal_bytes("XMR Stage-A wire", &application.stage_a_wire)?,
            stage_b: seal_bytes("XMR Stage-B wire", &application.stage_b_wire)?,
            own_public_packet: seal_bytes(
                "XMR own public role packet",
                &application.own_public_packet,
            )?,
            peer_public_packet: seal_bytes(
                "XMR peer public role packet",
                &application.peer_public_packet,
            )?,
            private_manifest: seal_bytes(
                "XMR private role manifest",
                &application.private_manifest,
            )?,
            private_view_key: seal_bytes(
                "XMR private Monero view key",
                &application.private_view_key,
            )?,
        });
        Ok(self)
    }

    pub(crate) fn with_child_plan(mut self, bytes: &[u8]) -> Result<Self> {
        ensure!(
            self.child_plan.is_none(),
            "XMR effect child plan is already pinned"
        );
        self.child_plan = Some(seal_bytes("XMR effect child plan", bytes)?);
        Ok(self)
    }

    #[cfg(feature = "sessions")]
    pub(crate) fn with_tag14_exact_transaction(
        mut self,
        execution: &ValidatedXmrEffectExecutionV3,
    ) -> Result<Self> {
        ensure!(
            self.tag14_exact_transaction.is_none(),
            "Tag14 exact observation transaction is already pinned"
        );
        let authority = execution.effect_authority();
        ensure!(
            authority.role() == ActorRole::Taker && authority.schema_version() == 2,
            "exact Tag14 observation requires schema-v2 Taker authority"
        );
        let release = authority
            .tag14_release()
            .context("Tag14 release authority is unavailable")?;
        let protection_key = PublicationProtectionKey::from_owner_private_file(
            release.protection_key_id(),
            release.protection_key_file(),
        )
        .context("authenticate Tag14 release protection key")?;
        let state_directory = open_private_directory(
            release.state_directory(),
            "XMR Tag14 release state directory",
        )?;
        let store =
            ReleaseStore::open_existing_in_directory(state_directory, XMR_RELEASE_JOURNAL_NAME)
                .context("open existing Tag14 release journal")?;
        let run_id = RunId::new(authority.run_id().to_owned())
            .context("invalid Tag14 observation run ID")?;
        let snapshot = store
            .load_xmr_claim_release(authority.swap_id(), &run_id, &protection_key)
            .context("authenticate Tag14 release snapshot")?;
        let transaction = store
            .exact_publication(&snapshot, &protection_key)
            .context("load exact Tag14 publication")?;
        let bytes = serde_json::to_vec(&transaction)
            .context("encode exact Tag14 observation transaction")?;
        self.tag14_exact_transaction =
            Some(seal_bytes("exact Tag14 observation transaction", &bytes)?);
        Ok(self)
    }

    #[cfg(feature = "sessions")]
    pub(crate) fn with_invocation_material(
        mut self,
        application: &ValidatedXmrEffectApplicationV1,
        step: XmrWorkflowStep,
        evidence_root: &Path,
    ) -> Result<Self> {
        ensure!(
            self.invocation_xmr_share.is_none(),
            "XMR effect invocation material is already pinned"
        );
        ensure!(
            self.invocation_refund_signature.is_none(),
            "XMR refund signature material is already pinned"
        );
        if matches!(
            step,
            XmrWorkflowStep::SweepMoneroClaim
                | XmrWorkflowStep::RefundLezTag16
                | XmrWorkflowStep::SweepMoneroRefund
        ) {
            self.invocation_xmr_share = Some(seal_bytes(
                "XMR private spend share",
                application.private_xmr_share.as_ref(),
            )?);
        }
        if step == XmrWorkflowStep::SweepMoneroRefund {
            let signature = read_stable_private_source(
                &evidence_root.join(XMR_EFFECT_FINALIZED_REFUND_SIGNATURE_FILE),
                MAX_FINAL_SIGNATURE_PACKET_BYTES,
                "finalized XMR refund signature",
            )?;
            self.invocation_refund_signature = Some(seal_bytes(
                "finalized XMR refund signature",
                &signature.bytes,
            )?);
        }
        Ok(self)
    }

    /// Consumes all pinned inputs into one executable-and-lock child mapping.
    ///
    /// Program FD 197, actor lock FD 198, workflow lock FD 199, runtime FD 200,
    /// capability FD 201, role-separated Monero RPC credentials FDs 202..=209,
    /// and shared-wallet file password FD 210 are installed by one command
    /// mapping. A semantically validated execution additionally installs exact
    /// immutable application material on FDs 211 through 216 and a canonical
    /// secret-free execution plan on FD 217. No secret enters argv or env.
    ///
    /// # Errors
    ///
    /// Rejects invalid or aliased descriptor plans, changed/crossed locks, and
    /// any child mapping failure before spawn.
    #[cfg(feature = "sessions")]
    pub fn into_command(
        self,
        executable: PinnedExecutable,
        actor_lock: &MakerActorHeldLock,
        workflow_lock: &MakerActorHeldLock,
    ) -> Result<Command> {
        let Self {
            runtime_bytes: _,
            runtime_snapshot,
            capability,
            monero,
            application,
            child_plan,
            invocation_xmr_share,
            invocation_refund_signature,
            tag14_exact_transaction,
        } = self;
        let PinnedXmrEffectMoneroCredentialsV1 {
            daemon,
            funding_wallet,
            shared_wallet,
            role_wallet,
            shared_wallet_file_password,
        } = monero;
        let PinnedXmrEffectRpcCredentialsV1 {
            username: daemon_username,
            password: daemon_password,
        } = daemon;
        let PinnedXmrEffectRpcCredentialsV1 {
            username: funding_username,
            password: funding_password,
        } = funding_wallet;
        let PinnedXmrEffectRpcCredentialsV1 {
            username: shared_username,
            password: shared_password,
        } = shared_wallet;
        let PinnedXmrEffectRpcCredentialsV1 {
            username: role_username,
            password: role_password,
        } = role_wallet;
        let mut descriptors = vec![
            (runtime_snapshot, XMR_EFFECT_RUNTIME_FD),
            (capability.into_snapshot(), XMR_EFFECT_CAPABILITY_FD),
            (
                daemon_username.into_snapshot(),
                XMR_EFFECT_DAEMON_USERNAME_FD,
            ),
            (
                daemon_password.into_snapshot(),
                XMR_EFFECT_DAEMON_PASSWORD_FD,
            ),
            (
                funding_username.into_snapshot(),
                XMR_EFFECT_FUNDING_USERNAME_FD,
            ),
            (
                funding_password.into_snapshot(),
                XMR_EFFECT_FUNDING_PASSWORD_FD,
            ),
            (
                shared_username.into_snapshot(),
                XMR_EFFECT_SHARED_USERNAME_FD,
            ),
            (
                shared_password.into_snapshot(),
                XMR_EFFECT_SHARED_PASSWORD_FD,
            ),
            (role_username.into_snapshot(), XMR_EFFECT_ROLE_USERNAME_FD),
            (role_password.into_snapshot(), XMR_EFFECT_ROLE_PASSWORD_FD),
            (
                shared_wallet_file_password.into_snapshot(),
                XMR_EFFECT_SHARED_WALLET_FILE_PASSWORD_FD,
            ),
        ];
        if let Some(application) = application {
            descriptors.extend([
                (application.stage_a, XMR_EFFECT_STAGE_A_FD),
                (application.stage_b, XMR_EFFECT_STAGE_B_FD),
                (
                    application.own_public_packet,
                    XMR_EFFECT_OWN_PUBLIC_PACKET_FD,
                ),
                (
                    application.peer_public_packet,
                    XMR_EFFECT_PEER_PUBLIC_PACKET_FD,
                ),
                (application.private_manifest, XMR_EFFECT_PRIVATE_MANIFEST_FD),
                (application.private_view_key, XMR_EFFECT_PRIVATE_VIEW_KEY_FD),
            ]);
        }
        if let Some(child_plan) = child_plan {
            descriptors.push((child_plan, XMR_EFFECT_CHILD_PLAN_FD));
        }
        if let Some(invocation_xmr_share) = invocation_xmr_share {
            descriptors.push((invocation_xmr_share, XMR_EFFECT_PRIVATE_XMR_SHARE_FD));
        }
        if let Some(invocation_refund_signature) = invocation_refund_signature {
            descriptors.push((
                invocation_refund_signature,
                XMR_EFFECT_FINALIZED_REFUND_SIGNATURE_FD,
            ));
        }
        if let Some(tag14_exact_transaction) = tag14_exact_transaction {
            descriptors.push((
                tag14_exact_transaction,
                XMR_EFFECT_TAG14_EXACT_TRANSACTION_FD,
            ));
        }
        let plan = PinnedChildFdPlan::new(descriptors)
            .context("validate XMR effect child descriptor plan")?;
        executable
            .into_command_with_locks_and_fd_plan(actor_lock, workflow_lock, plan)
            .context("compose XMR effect command")
    }
}

impl ValidatedXmrEffectAuthorityV1 {
    /// Pins the runtime identity and every current private credential at use.
    ///
    /// Runtime bytes must match the authority SHA-256. Capability and RPC
    /// credentials are deliberately current rotating secrets: their exact
    /// bytes, lengths, and hashes are snapshotted into sealed read-only memfds
    /// without becoming serializable, cloneable, or printable.
    ///
    /// # Errors
    ///
    /// Rejects unsafe parents, symlinks, aliases, hard links, modes, owners,
    /// sizes, unstable identities, invalid secret text, or runtime digest drift.
    pub fn pin_effect_inputs_at_use(&self) -> Result<PinnedXmrEffectInputsV1> {
        let runtime = read_stable_private_source(
            self.lez().runtime_file(),
            MAX_RUNTIME_BYTES,
            "XMR LEZ runtime",
        )?;
        ensure!(
            Sha256::digest(&runtime.bytes).as_slice() == self.lez().runtime_sha256(),
            "XMR LEZ runtime digest changed at use"
        );

        let mut identities = BTreeSet::from([(runtime.identity.device, runtime.identity.inode)]);
        let capability = pin_secret(
            self.lez().capability_file(),
            "XMR LEZ capability",
            &mut identities,
        )?;
        let daemon = pin_rpc(self.monero().daemon(), "daemon", &mut identities)?;
        let funding_wallet = pin_rpc(
            self.monero().funding_wallet(),
            "funding wallet",
            &mut identities,
        )?;
        let shared_wallet = pin_rpc(
            self.monero().shared_wallet(),
            "shared wallet",
            &mut identities,
        )?;
        let role_wallet = pin_rpc(self.monero().role_wallet(), "role wallet", &mut identities)?;
        let shared_wallet_file_password = pin_secret(
            self.monero().shared_wallet_file_password_file(),
            "XMR shared-wallet file password",
            &mut identities,
        )?;

        let runtime_snapshot = seal_bytes("XMR LEZ runtime", &runtime.bytes)?;
        Ok(PinnedXmrEffectInputsV1 {
            runtime_bytes: runtime.bytes.to_vec(),
            runtime_snapshot,
            capability,
            monero: PinnedXmrEffectMoneroCredentialsV1 {
                daemon,
                funding_wallet,
                shared_wallet,
                role_wallet,
                shared_wallet_file_password,
            },
            application: None,
            child_plan: None,
            invocation_xmr_share: None,
            invocation_refund_signature: None,
            tag14_exact_transaction: None,
        })
    }
}

impl ValidatedXmrEffectExecutionV3 {
    /// Pins only the four descriptor-native inputs required by the schema-v2
    /// Taker Tag14 release service.
    ///
    /// The general LEZ capability, every Monero RPC credential, application
    /// private manifest, view key, and spend share are deliberately excluded
    /// from the returned child plan. Stage A/B and the view key are used only
    /// inside the parent to rederive the exact public release binding.
    pub(crate) fn pin_tag14_release_inputs_at_use(
        &self,
        mode: XmrEffectChildModeV1,
    ) -> Result<PinnedXmrTag14ReleaseInputsV1> {
        ensure!(
            self.effect_authority().role() == ActorRole::Taker
                && self.effect_authority().schema_version() == 2,
            "semantic Tag14 release requires schema-v2 Taker authority"
        );
        let mode = match mode {
            XmrEffectChildModeV1::Preflight => "preflight",
            XmrEffectChildModeV1::Invoke => "invoke",
            XmrEffectChildModeV1::Observe => {
                anyhow::bail!("Tag14 release custody cannot authorize observation")
            }
        };
        let runtime_source = read_stable_private_source(
            self.effect_authority().lez().runtime_file(),
            MAX_RUNTIME_BYTES,
            "XMR Tag14 runtime",
        )?;
        ensure!(
            Sha256::digest(&runtime_source.bytes).as_slice()
                == self.effect_authority().lez().runtime_sha256(),
            "XMR Tag14 runtime digest changed at use"
        );
        let runtime: RuntimeDescriptor = serde_json::from_slice(&runtime_source.bytes)
            .context("XMR Tag14 runtime JSON is invalid")?;
        let invocation_bytes = self.tag14_release_invocation_bytes(mode, runtime)?;
        let release = self
            .effect_authority()
            .tag14_release()
            .context("Tag14 release authority is unavailable")?;
        let mut identities = BTreeSet::from([(
            runtime_source.identity.device,
            runtime_source.identity.inode,
        )]);
        let capability = pin_secret(
            release.capability_file(),
            "XMR Tag14 release capability",
            &mut identities,
        )?
        .into_snapshot();
        let protection_key = pin_secret(
            release.protection_key_file(),
            "XMR Tag14 release protection key",
            &mut identities,
        )?
        .into_snapshot();
        let state_directory = open_private_directory(
            release.state_directory(),
            "XMR Tag14 release state directory",
        )?;
        Ok(PinnedXmrTag14ReleaseInputsV1 {
            invocation: seal_bytes("XMR Tag14 release invocation", &invocation_bytes)?,
            capability,
            protection_key,
            state_directory,
        })
    }

    fn tag14_release_invocation_bytes(
        &self,
        mode: &'static str,
        runtime: RuntimeDescriptor,
    ) -> Result<Vec<u8>> {
        let release = self
            .effect_authority()
            .tag14_release()
            .context("Tag14 release authority is unavailable")?;
        let agreement = XmrAgreementV1::from_wire(&self.application.stage_a_wire)
            .context("XMR Tag14 Stage-A agreement is invalid")?;
        let view_key = parse_private_view_key_bytes(&self.application.private_view_key)?;
        let activation = XmrActivatedAgreementV1::from_wire(
            &agreement,
            &self.application.stage_b_wire,
            &view_key,
        )
        .context("XMR Tag14 Stage-B activation is invalid")?;
        ensure!(
            agreement.body().swap_id() == self.effect_authority().swap_id()
                && agreement.agreement_commitment()
                    == self.effect_authority().agreement_commitment()
                && activation.activation_commitment()
                    == self.effect_authority().activation_commitment(),
            "XMR Tag14 application identity changed"
        );
        let binding = XmrLezBridgeBindingV3::new(&agreement, &activation)
            .context("XMR Tag14 Stage-B binding is invalid")?;
        let run_id = RunId::new(self.effect_authority().run_id().to_owned())
            .context("XMR Tag14 run ID is invalid")?;
        let context = MessageContext::new(
            run_id.clone(),
            RequestId::new("release-config").context("XMR Tag14 request ID is invalid")?,
            BridgeParticipant::Taker,
        );
        binding
            .terms()
            .validate_runtime_binding(&context, &runtime)
            .context("XMR Tag14 runtime is not bound by Stage B")?;
        let invocation = XmrTag14ReleaseInvocationWireV2 {
            schema_version: 2,
            mode,
            public_config: XmrTag14ReleasePublicConfigWireV1 {
                schema_version: 1,
                sidecar_endpoint: release.sidecar_url().as_str().to_owned(),
                indexer_endpoint: release.indexer_url().as_str().to_owned(),
                node_profile: match release.node_profile() {
                    XmrTag14ReleaseNodeProfileV1::Local => "local",
                    XmrTag14ReleaseNodeProfileV1::OfficialPublic => "official_public",
                },
                run_id,
                runtime,
                terms: binding.terms(),
                protection_key_id: release.protection_key_id().to_owned(),
            },
        };
        let mut invocation_bytes =
            serde_json::to_vec(&invocation).context("encode XMR Tag14 release invocation")?;
        invocation_bytes.push(b'\n');
        Ok(invocation_bytes)
    }
}

pub(crate) fn parse_private_view_key_bytes(bytes: &[u8]) -> Result<MoneroPrivateViewKey> {
    let mut text = Zeroizing::new(
        String::from_utf8(bytes.to_vec()).context("XMR Tag14 view key is not UTF-8")?,
    );
    while text.ends_with(['\n', '\r']) {
        text.pop();
    }
    ensure!(
        text.len() == 64
            && text
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "XMR Tag14 view key is not exact lowercase hex"
    );
    let mut scalar = Zeroizing::new([0_u8; 32]);
    hex::decode_to_slice(text.as_bytes(), scalar.as_mut()).context("decode XMR Tag14 view key")?;
    text.zeroize();
    MoneroPrivateViewKey::from_monero_little_endian(*scalar)
        .context("XMR Tag14 view key is not a canonical scalar")
}

fn pin_rpc(
    rpc: &XmrEffectAuthenticatedRpcV1,
    label: &'static str,
    identities: &mut BTreeSet<(u64, u64)>,
) -> Result<PinnedXmrEffectRpcCredentialsV1> {
    Ok(PinnedXmrEffectRpcCredentialsV1 {
        username: pin_secret(
            rpc.username_file(),
            match label {
                "daemon" => "XMR daemon username",
                "funding wallet" => "XMR funding-wallet username",
                "shared wallet" => "XMR shared-wallet username",
                "role wallet" => "XMR role-wallet username",
                _ => "XMR RPC username",
            },
            identities,
        )?,
        password: pin_secret(
            rpc.password_file(),
            match label {
                "daemon" => "XMR daemon password",
                "funding wallet" => "XMR funding-wallet password",
                "shared wallet" => "XMR shared-wallet password",
                "role wallet" => "XMR role-wallet password",
                _ => "XMR RPC password",
            },
            identities,
        )?,
    })
}

fn pin_secret(
    path: &Path,
    label: &'static str,
    identities: &mut BTreeSet<(u64, u64)>,
) -> Result<PinnedXmrEffectSecretV1> {
    let mut source = read_stable_private_source(path, MAX_SECRET_BYTES, label)?;
    validate_secret_text(&source.bytes, label)?;
    ensure!(
        identities.insert((source.identity.device, source.identity.inode)),
        "XMR effect secret sources alias"
    );
    let sha256: [u8; 32] = Sha256::digest(&source.bytes).into();
    let redacted_len = source.bytes.len();
    let snapshot = seal_bytes(label, &source.bytes)?;
    source.bytes.zeroize();
    let child_path = PathBuf::from(format!("/proc/self/fd/{}", snapshot.as_raw_fd()));
    Ok(PinnedXmrEffectSecretV1 {
        snapshot,
        child_path,
        redacted_len,
        sha256,
    })
}

fn validate_secret_text(bytes: &[u8], label: &'static str) -> Result<()> {
    let logical = if let Some(value) = bytes.strip_suffix(b"\r\n") {
        value
    } else if let Some(value) = bytes.strip_suffix(b"\n") {
        value
    } else {
        bytes
    };
    ensure!(
        !logical.is_empty()
            && logical
                .iter()
                .all(|byte| byte.is_ascii_graphic() && *byte != b'\0')
            && !logical.contains(&b'\n')
            && !logical.contains(&b'\r'),
        "{label} is not one bounded credential value"
    );
    Ok(())
}

struct PrivateSource {
    bytes: Zeroizing<Vec<u8>>,
    identity: SourceIdentity,
}

#[derive(Clone, Copy)]
struct SourceIdentity {
    device: u64,
    inode: u64,
}

fn read_stable_private_source(
    path: &Path,
    maximum: u64,
    label: &'static str,
) -> Result<PrivateSource> {
    let parent_path = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .context("XMR effect input has no parent")?;
    let parent = open_private_directory(parent_path, label)?;
    let parent_before = parent
        .metadata()
        .context("inspect XMR effect input parent")?;

    let mut file = open_path_no_symlinks(path, label)?;
    let before = validate_private_file(&file, maximum, label)?;
    let mut bytes = Zeroizing::new(Vec::new());
    std::io::Read::by_ref(&mut file)
        .take(maximum + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("read {label}"))?;
    ensure!(
        u64::try_from(bytes.len()).unwrap_or(u64::MAX) <= maximum,
        "{label} is oversized"
    );
    let after = validate_private_file(&file, maximum, label)?;
    let named = fs::symlink_metadata(path).with_context(|| format!("reinspect {label}"))?;
    ensure!(
        stable_file(&before, &after)
            && stable_file(&before, &named)
            && after.len() == u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        "{label} changed while it was pinned"
    );

    let reopened_parent = open_private_directory(parent_path, label)?;
    let parent_after = reopened_parent
        .metadata()
        .context("reinspect XMR effect input parent")?;
    ensure!(
        parent_before.dev() == parent_after.dev()
            && parent_before.ino() == parent_after.ino()
            && parent_before.mode() == parent_after.mode()
            && parent_before.uid() == parent_after.uid(),
        "{label} parent changed while it was pinned"
    );

    Ok(PrivateSource {
        bytes,
        identity: SourceIdentity {
            device: before.dev(),
            inode: before.ino(),
        },
    })
}

fn validate_private_file(file: &File, maximum: u64, label: &'static str) -> Result<fs::Metadata> {
    let metadata = file
        .metadata()
        .with_context(|| format!("inspect {label}"))?;
    ensure!(
        metadata.file_type().is_file()
            && metadata.uid() == rustix::process::geteuid().as_raw()
            && metadata.permissions().mode() & 0o7777 == 0o600
            && metadata.nlink() == 1
            && metadata.len() > 0
            && metadata.len() <= maximum,
        "{label} is unsafe or oversized"
    );
    Ok(metadata)
}

fn stable_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.file_type().is_file()
        && right.file_type().is_file()
        && left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.len() == right.len()
        && left.uid() == right.uid()
        && left.mode() == right.mode()
        && left.nlink() == right.nlink()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
        && left.ctime() == right.ctime()
        && left.ctime_nsec() == right.ctime_nsec()
}

fn seal_bytes(label: &str, bytes: &[u8]) -> Result<File> {
    let descriptor = memfd_create(label, MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING)
        .context("create sealed XMR effect input")?;
    let mut writer = File::from(descriptor);
    writer
        .write_all(bytes)
        .and_then(|()| writer.flush())
        .context("write sealed XMR effect input")?;
    writer
        .set_permissions(fs::Permissions::from_mode(0o400))
        .context("protect sealed XMR effect input")?;
    let seals = SealFlags::SEAL | SealFlags::SHRINK | SealFlags::GROW | SealFlags::WRITE;
    fcntl_add_seals(&writer, seals).context("seal XMR effect input")?;
    ensure!(
        fcntl_get_seals(&writer)
            .context("inspect XMR effect input seals")?
            .contains(seals),
        "XMR effect input seals are incomplete"
    );
    writer
        .seek(std::io::SeekFrom::Start(0))
        .context("rewind sealed XMR effect input")?;
    let descriptor_path = format!("/proc/self/fd/{}", writer.as_raw_fd());
    let snapshot = File::open(descriptor_path).context("open read-only XMR effect input")?;
    let metadata = snapshot
        .metadata()
        .context("inspect sealed XMR effect input")?;
    ensure!(
        metadata.len() == u64::try_from(bytes.len()).unwrap_or(u64::MAX)
            && metadata.permissions().mode() & 0o7777 == 0o400,
        "sealed XMR effect input has wrong metadata"
    );
    drop(writer);
    let high_descriptor = fcntl_dupfd_cloexec(&snapshot, 200)
        .context("allocate collision-free XMR effect input descriptor")?;
    drop(snapshot);
    Ok(File::from(high_descriptor))
}
