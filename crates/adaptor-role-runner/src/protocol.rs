use std::path::Path;

use clap::ValueEnum;
use lez_adaptor_signature::{AdaptorSessionContext, SigningRole};
use lez_swap_store::{AdaptorSessionIdentity, AdaptorSessionRole};
use serde::{Deserialize, Serialize};

use crate::{RunnerError, files};

const SCHEMA_VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Maker,
    Taker,
}

impl Role {
    pub(crate) const fn opposite(self) -> Self {
        match self {
            Self::Maker => Self::Taker,
            Self::Taker => Self::Maker,
        }
    }

    pub(crate) const fn sdk(self) -> SigningRole {
        match self {
            Self::Maker => SigningRole::Maker,
            Self::Taker => SigningRole::Taker,
        }
    }

    pub(crate) const fn store(self) -> AdaptorSessionRole {
        match self {
            Self::Maker => AdaptorSessionRole::Maker,
            Self::Taker => AdaptorSessionRole::Taker,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SessionConfigV1 {
    schema_version: u16,
    context: ContextConfigV1,
    session_id: String,
    exact_message: String,
    adaptor_point: String,
    maker_public_key: String,
    taker_public_key: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ContextConfigV1 {
    LezUntweaked,
    BtcTaproot { merkle_root: String },
}

#[derive(Clone, Debug)]
#[must_use]
/// Opaque, validated public signing context accepted by the role runner.
pub struct ValidatedSession {
    context: AdaptorSessionContext,
    id: [u8; 32],
    exact_message: [u8; 32],
    adaptor_point: [u8; 33],
    ordered_public_keys: [[u8; 33]; 2],
    context_binding: [u8; 32],
}

impl ValidatedSession {
    /// Creates a runner session from an already validated untweaked context.
    ///
    /// This constructor rejects a Taproot-tweaked context even though both
    /// context kinds share the same public Rust type. Session bytes remain an
    /// implementation detail of this crate.
    ///
    /// # Errors
    ///
    /// Returns [`RunnerError::InvalidSessionConfig`] when `context` is not the
    /// exact untweaked context reconstructed from its public transcript.
    pub fn from_untweaked_context(context: AdaptorSessionContext) -> Result<Self, RunnerError> {
        let reconstructed = AdaptorSessionContext::untweaked(
            context.ordered_public_keys(),
            context.message(),
            context.adaptor_point(),
            context.session_id(),
        )
        .map_err(|_| RunnerError::InvalidSessionConfig)?;
        if reconstructed.durable_context_binding() != context.durable_context_binding() {
            return Err(RunnerError::InvalidSessionConfig);
        }
        Ok(Self::from_context(context))
    }

    /// Writes the runner's canonical owner-public session file exactly once.
    ///
    /// # Errors
    ///
    /// Returns a fail-closed output or serialization error. An existing path
    /// is never truncated or replaced.
    pub fn write_new(&self, path: &Path) -> Result<(), RunnerError> {
        let config = self.config(ContextConfigV1::LezUntweaked);
        let mut bytes =
            serde_json::to_vec(&config).map_err(|_| RunnerError::PublicPacketSerialization)?;
        bytes.push(b'\n');
        files::write_public_new(path, &bytes)
    }

    pub(crate) fn load(path: &Path) -> Result<Self, RunnerError> {
        let bytes = files::read_public(path)?;
        let raw: SessionConfigV1 =
            serde_json::from_slice(&bytes).map_err(|_| RunnerError::InvalidSessionConfig)?;
        let mut canonical =
            serde_json::to_vec(&raw).map_err(|_| RunnerError::InvalidSessionConfig)?;
        canonical.push(b'\n');
        if canonical != bytes {
            return Err(RunnerError::NoncanonicalSessionConfig);
        }
        if raw.schema_version != SCHEMA_VERSION {
            return Err(RunnerError::InvalidSessionConfig);
        }
        let session_id = decode_exact(&raw.session_id)?;
        let exact_message = decode_exact(&raw.exact_message)?;
        let adaptor_point = decode_exact(&raw.adaptor_point)?;
        let ordered_public_keys = [
            decode_exact(&raw.maker_public_key)?,
            decode_exact(&raw.taker_public_key)?,
        ];
        let context = match raw.context {
            ContextConfigV1::LezUntweaked => AdaptorSessionContext::untweaked(
                ordered_public_keys,
                exact_message,
                adaptor_point,
                session_id,
            ),
            ContextConfigV1::BtcTaproot { merkle_root } => AdaptorSessionContext::taproot(
                ordered_public_keys,
                decode_exact(&merkle_root)?,
                exact_message,
                adaptor_point,
                session_id,
            ),
        }
        .map_err(|_| RunnerError::InvalidSessionConfig)?;
        Ok(Self::from_context(context))
    }

    fn from_context(context: AdaptorSessionContext) -> Self {
        Self {
            id: context.session_id(),
            exact_message: context.message(),
            adaptor_point: context.adaptor_point(),
            ordered_public_keys: context.ordered_public_keys(),
            context_binding: context.durable_context_binding(),
            context,
        }
    }

    fn config(&self, context: ContextConfigV1) -> SessionConfigV1 {
        SessionConfigV1 {
            schema_version: SCHEMA_VERSION,
            context,
            session_id: hex::encode(self.id),
            exact_message: hex::encode(self.exact_message),
            adaptor_point: hex::encode(self.adaptor_point),
            maker_public_key: hex::encode(self.ordered_public_keys[0]),
            taker_public_key: hex::encode(self.ordered_public_keys[1]),
        }
    }

    pub(crate) const fn context(&self) -> &AdaptorSessionContext {
        &self.context
    }

    pub(crate) const fn context_binding(&self) -> [u8; 32] {
        self.context_binding
    }

    /// Derives the exact immutable identity for one role-local session journal.
    ///
    /// The returned value contains only the validated public transcript.
    pub const fn identity(&self, role: Role) -> AdaptorSessionIdentity {
        AdaptorSessionIdentity::new(
            self.id,
            role.store(),
            self.context_binding,
            self.exact_message,
            self.adaptor_point,
            self.ordered_public_keys,
        )
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PacketKind {
    NonceCommitment,
    PublicNonce,
    PartialSignature,
    Presignature,
    FinalSignature,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum PacketSender {
    Maker,
    Taker,
    Aggregate,
}

impl From<Role> for PacketSender {
    fn from(role: Role) -> Self {
        match role {
            Role::Maker => Self::Maker,
            Role::Taker => Self::Taker,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PublicPacketV1 {
    schema_version: u16,
    kind: PacketKind,
    session_id: String,
    sender_role: PacketSender,
    context_binding: String,
    payload: String,
}

impl PublicPacketV1 {
    fn new<const N: usize>(
        kind: PacketKind,
        sender_role: Role,
        session: &ValidatedSession,
        payload: [u8; N],
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            kind,
            session_id: hex::encode(session.id),
            sender_role: sender_role.into(),
            context_binding: hex::encode(session.context_binding),
            payload: hex::encode(payload),
        }
    }

    fn canonical_bytes(&self) -> Result<Vec<u8>, RunnerError> {
        let mut bytes =
            serde_json::to_vec(self).map_err(|_| RunnerError::PublicPacketSerialization)?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    fn load(path: &Path) -> Result<(Self, Vec<u8>), RunnerError> {
        let bytes = files::read_public(path)?;
        let packet: Self =
            serde_json::from_slice(&bytes).map_err(|_| RunnerError::InvalidPublicPacket)?;
        if packet.canonical_bytes()? != bytes {
            return Err(RunnerError::NoncanonicalPublicPacket);
        }
        Ok((packet, bytes))
    }

    fn validate_header(
        &self,
        expected_kind: PacketKind,
        expected_sender: PacketSender,
        session: &ValidatedSession,
    ) -> Result<(), RunnerError> {
        if self.schema_version != SCHEMA_VERSION
            || self.kind != expected_kind
            || self.session_id != hex::encode(session.id)
            || self.context_binding != hex::encode(session.context_binding)
        {
            return Err(RunnerError::PublicPacketCrosswire);
        }
        if self.sender_role != expected_sender {
            return Err(RunnerError::PublicPacketRoleCrosswire);
        }
        Ok(())
    }
}

pub(crate) fn write_aggregate_packet<const N: usize>(
    path: &Path,
    kind: PacketKind,
    session: &ValidatedSession,
    payload: [u8; N],
) -> Result<(), RunnerError> {
    let packet = PublicPacketV1 {
        schema_version: SCHEMA_VERSION,
        kind,
        session_id: hex::encode(session.id),
        sender_role: PacketSender::Aggregate,
        context_binding: hex::encode(session.context_binding),
        payload: hex::encode(payload),
    };
    files::write_public_new(path, &packet.canonical_bytes()?)
}

pub(crate) fn write_packet<const N: usize>(
    path: &Path,
    kind: PacketKind,
    role: Role,
    session: &ValidatedSession,
    payload: [u8; N],
) -> Result<(), RunnerError> {
    let packet = PublicPacketV1::new(kind, role, session, payload);
    files::write_public_new(path, &packet.canonical_bytes()?)
}

pub(crate) fn read_peer_packet<const N: usize>(
    path: &Path,
    kind: PacketKind,
    local_role: Role,
    session: &ValidatedSession,
) -> Result<[u8; N], RunnerError> {
    let (packet, _) = PublicPacketV1::load(path)?;
    packet.validate_header(kind, local_role.opposite().into(), session)?;
    decode_exact(&packet.payload)
}

pub(crate) fn read_aggregate_packet<const N: usize>(
    path: &Path,
    kind: PacketKind,
    session: &ValidatedSession,
) -> Result<[u8; N], RunnerError> {
    let (packet, _) = PublicPacketV1::load(path)?;
    packet.validate_header(kind, PacketSender::Aggregate, session)?;
    decode_exact(&packet.payload)
}

fn decode_exact<const N: usize>(encoded: &str) -> Result<[u8; N], RunnerError> {
    if encoded.len() != N * 2 || encoded.bytes().any(|byte| byte.is_ascii_uppercase()) {
        return Err(RunnerError::InvalidCanonicalHex);
    }
    let decoded = hex::decode(encoded).map_err(|_| RunnerError::InvalidCanonicalHex)?;
    decoded
        .try_into()
        .map_err(|_| RunnerError::InvalidCanonicalHex)
}
