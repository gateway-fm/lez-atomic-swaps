//! Role-fixed public LEZ/Monero lifecycle facade.
//!
//! The SDK validates the public Stage-A agreement and bounds the Stage-B wire,
//! but never imports view keys, adaptor shares, wallet credentials, or effect
//! journals. Role-fixed actor owns durable effect journals and revalidates
//! Stage B with its owner-private view key before returning a snapshot.

use std::error::Error;

use async_trait::async_trait;
use lez_swap_core::Participant;
use lez_swap_sdk_core::{NegotiationChannel, OfferDiscovery};
use serde::{Deserialize, Serialize};

use crate::{
    MAX_XMR_ACTIVATION_WIRE_BYTES, MAX_XMR_AGREEMENT_WIRE_BYTES, XmrActivatedAgreementV1,
    XmrAgreementV1, XmrAgreementV1Error, XmrRoleV1,
};

/// Canonical envelope schema returned by the pre-lock negotiation adapter.
pub const XMR_NEGOTIATION_ENVELOPE_SCHEMA_V1: u16 = 1;
const MAX_XMR_NEGOTIATION_ENVELOPE_BYTES: usize =
    MAX_XMR_AGREEMENT_WIRE_BYTES + MAX_XMR_ACTIVATION_WIRE_BYTES + 64;

/// Bounded, canonical Stage-A/Stage-B result from mutually authenticated Chat.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct XmrNegotiationEnvelopeV1 {
    exact_bytes: Box<[u8]>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NegotiationEnvelopeWireV1 {
    schema_version: u16,
    agreement_wire: Vec<u8>,
    activation_wire: Vec<u8>,
}

impl XmrNegotiationEnvelopeV1 {
    /// Validates Stage A, bounds Stage B, and encodes one canonical envelope.
    ///
    /// Stage B still requires the role actor's private view key. This method
    /// intentionally cannot promote it to executable authority.
    ///
    /// # Errors
    ///
    /// Rejects invalid Stage A, empty/oversized Stage B, or an oversized
    /// canonical envelope.
    pub fn new(
        agreement_wire: impl Into<Box<[u8]>>,
        activation_wire: impl Into<Box<[u8]>>,
    ) -> Result<Self, XmrSdkError> {
        let candidate = XmrNegotiationCandidateV1::from_wires(agreement_wire, activation_wire)?;
        let wire = NegotiationEnvelopeWireV1 {
            schema_version: XMR_NEGOTIATION_ENVELOPE_SCHEMA_V1,
            agreement_wire: candidate.agreement_wire.to_vec(),
            activation_wire: candidate.activation_wire.to_vec(),
        };
        let exact_bytes =
            postcard::to_allocvec(&wire).map_err(|_| XmrSdkError::MalformedNegotiationEnvelope)?;
        if exact_bytes.len() > MAX_XMR_NEGOTIATION_ENVELOPE_BYTES {
            return Err(XmrSdkError::OversizedNegotiationEnvelope);
        }
        Ok(Self {
            exact_bytes: exact_bytes.into_boxed_slice(),
        })
    }

    /// Parses one canonical envelope and revalidates its public candidate.
    ///
    /// # Errors
    ///
    /// Rejects empty, oversized, trailing, noncanonical, unknown-schema, or
    /// semantically invalid public bytes.
    pub fn from_wire(bytes: &[u8]) -> Result<Self, XmrSdkError> {
        if bytes.is_empty() || bytes.len() > MAX_XMR_NEGOTIATION_ENVELOPE_BYTES {
            return Err(XmrSdkError::OversizedNegotiationEnvelope);
        }
        let (wire, remainder): (NegotiationEnvelopeWireV1, &[u8]) =
            postcard::take_from_bytes(bytes)
                .map_err(|_| XmrSdkError::MalformedNegotiationEnvelope)?;
        if !remainder.is_empty() {
            return Err(XmrSdkError::MalformedNegotiationEnvelope);
        }
        if wire.schema_version != XMR_NEGOTIATION_ENVELOPE_SCHEMA_V1 {
            return Err(XmrSdkError::UnsupportedNegotiationEnvelope(
                wire.schema_version,
            ));
        }
        let canonical = Self::new(wire.agreement_wire, wire.activation_wire)?;
        if canonical.exact_bytes.as_ref() != bytes {
            return Err(XmrSdkError::NonCanonicalNegotiationEnvelope);
        }
        Ok(canonical)
    }

    /// Exact canonical bytes transported by the negotiation channel.
    #[must_use]
    pub fn exact_bytes(&self) -> &[u8] {
        &self.exact_bytes
    }

    /// Returns the bounded candidate for role-actor activation.
    ///
    /// # Errors
    ///
    /// Returns an error only if retained canonical bytes no longer decode.
    pub fn candidate(&self) -> Result<XmrNegotiationCandidateV1, XmrSdkError> {
        let (wire, remainder): (NegotiationEnvelopeWireV1, &[u8]) =
            postcard::take_from_bytes(&self.exact_bytes)
                .map_err(|_| XmrSdkError::MalformedNegotiationEnvelope)?;
        if !remainder.is_empty() {
            return Err(XmrSdkError::MalformedNegotiationEnvelope);
        }
        XmrNegotiationCandidateV1::from_wires(wire.agreement_wire, wire.activation_wire)
    }
}

/// Public, bounded negotiation result awaiting owner-private Stage-B validation.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct XmrNegotiationCandidateV1 {
    agreement_wire: Box<[u8]>,
    activation_wire: Box<[u8]>,
    swap_id: [u8; 32],
    agreement_commitment: [u8; 32],
}

impl XmrNegotiationCandidateV1 {
    /// Revalidates public Stage A and bounds the owner-private Stage-B handoff.
    ///
    /// # Errors
    ///
    /// Rejects invalid Stage A or empty/oversized Stage B.
    pub fn from_wires(
        agreement_wire: impl Into<Box<[u8]>>,
        activation_wire: impl Into<Box<[u8]>>,
    ) -> Result<Self, XmrSdkError> {
        let agreement_wire = agreement_wire.into();
        let activation_wire = activation_wire.into();
        let agreement = XmrAgreementV1::from_wire(&agreement_wire)?;
        if activation_wire.is_empty() || activation_wire.len() > MAX_XMR_ACTIVATION_WIRE_BYTES {
            return Err(XmrSdkError::InvalidActivationWireLength);
        }
        Ok(Self {
            swap_id: agreement.body().swap_id(),
            agreement_commitment: agreement.agreement_commitment(),
            agreement_wire,
            activation_wire,
        })
    }

    /// Canonical dual-signed Stage-A agreement wire.
    #[must_use]
    pub fn agreement_wire(&self) -> &[u8] {
        &self.agreement_wire
    }

    /// Bounded Stage-B wire requiring owner-private actor validation.
    #[must_use]
    pub fn activation_wire(&self) -> &[u8] {
        &self.activation_wire
    }

    /// Agreement-derived binary swap identifier.
    #[must_use]
    pub const fn swap_id(&self) -> [u8; 32] {
        self.swap_id
    }

    /// Exact dual-signed Stage-A commitment.
    #[must_use]
    pub const fn agreement_commitment(&self) -> [u8; 32] {
        self.agreement_commitment
    }
}

/// Public lifecycle phase reconstructed from the actor's durable journal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XmrLifecyclePhaseV1 {
    /// Stage B is validated and durable; neither chain lock is acknowledged.
    Activated,
    /// Direction-fixed Taker LEZ lock is finalized.
    LezLocked,
    /// Maker Monero lock is also sufficiently confirmed.
    BothLocked,
    /// Successful-claim authorization is durable and awaiting completion.
    ClaimAuthorized,
    /// Timelocked refund authorization is durable and awaiting completion.
    RefundAuthorized,
    /// Late counterparty punishment authorization is durable.
    PunishAuthorized,
    /// Both successful claim legs completed.
    Completed,
    /// Both refund legs completed.
    Refunded,
    /// The protocol-defined punishment path completed.
    Punished,
}

impl XmrLifecyclePhaseV1 {
    const fn accepts_revision(self, revision: u64) -> bool {
        match self {
            Self::Activated => revision == 0,
            Self::LezLocked => revision == 1,
            Self::BothLocked => revision == 2,
            Self::ClaimAuthorized | Self::PunishAuthorized => revision == 3,
            Self::RefundAuthorized => matches!(revision, 2 | 3),
            Self::Completed | Self::Punished => revision == 4,
            Self::Refunded => matches!(revision, 3 | 4),
        }
    }

    const fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Refunded | Self::Punished)
    }
}

/// User intent sent to a role-fixed, journal-owning actor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XmrLifecycleCommandV1 {
    /// Reconcile observations and execute the next safe lock/protocol action.
    Advance,
    /// Drive the successful claim branch only when current evidence permits it.
    Claim,
    /// Drive the safe refund or punishment branch selected by durable evidence.
    Refund,
}

/// Secret-free durable status returned by a role-fixed actor.
#[derive(Clone, Debug, Eq, PartialEq)]
#[must_use]
pub struct XmrLifecycleSnapshotV1 {
    swap_id: [u8; 32],
    role: XmrRoleV1,
    agreement_commitment: [u8; 32],
    activation_commitment: [u8; 32],
    phase: XmrLifecyclePhaseV1,
    revision: u64,
}

/// Exact public lifecycle identity retained independently across restart.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use]
pub struct XmrLifecycleIdentityV1 {
    swap_id: [u8; 32],
    role: XmrRoleV1,
    agreement_commitment: [u8; 32],
    activation_commitment: [u8; 32],
}

impl XmrLifecycleIdentityV1 {
    /// Binary swap identifier.
    #[must_use]
    pub const fn swap_id(self) -> [u8; 32] {
        self.swap_id
    }

    /// Owner role fixed at activation.
    #[must_use]
    pub const fn role(self) -> XmrRoleV1 {
        self.role
    }

    /// Dual-signed Stage-A commitment.
    #[must_use]
    pub const fn agreement_commitment(self) -> [u8; 32] {
        self.agreement_commitment
    }

    /// Dual-signed Stage-B commitment.
    #[must_use]
    pub const fn activation_commitment(self) -> [u8; 32] {
        self.activation_commitment
    }
}

impl XmrLifecycleSnapshotV1 {
    /// Constructs a snapshot only from fully validated Stage A and Stage B.
    ///
    /// This constructor is intended for the role actor after owner-private view
    /// key validation and durable journal recovery.
    ///
    /// # Errors
    ///
    /// Rejects crossed activation/agreement or a noncanonical phase revision.
    pub fn from_validated(
        agreement: &XmrAgreementV1,
        activation: &XmrActivatedAgreementV1,
        role: XmrRoleV1,
        phase: XmrLifecyclePhaseV1,
        revision: u64,
    ) -> Result<Self, XmrSdkError> {
        let _ = activation.initial_coordinator(agreement)?;
        if !phase.accepts_revision(revision) {
            return Err(XmrSdkError::InvalidActorRevision);
        }
        Ok(Self {
            swap_id: agreement.body().swap_id(),
            role,
            agreement_commitment: agreement.agreement_commitment(),
            activation_commitment: activation.activation_commitment(),
            phase,
            revision,
        })
    }

    /// Binary swap identifier.
    #[must_use]
    pub const fn swap_id(&self) -> [u8; 32] {
        self.swap_id
    }

    /// Fixed owner role.
    #[must_use]
    pub const fn role(&self) -> XmrRoleV1 {
        self.role
    }

    /// Dual-signed Stage-A commitment.
    #[must_use]
    pub const fn agreement_commitment(&self) -> [u8; 32] {
        self.agreement_commitment
    }

    /// Dual-signed Stage-B commitment.
    #[must_use]
    pub const fn activation_commitment(&self) -> [u8; 32] {
        self.activation_commitment
    }

    /// Current durable phase.
    #[must_use]
    pub const fn phase(&self) -> XmrLifecyclePhaseV1 {
        self.phase
    }

    /// Canonical monotonic revision.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Public identity the application persists independently for restart.
    pub const fn identity(&self) -> XmrLifecycleIdentityV1 {
        XmrLifecycleIdentityV1 {
            swap_id: self.swap_id,
            role: self.role,
            agreement_commitment: self.agreement_commitment,
            activation_commitment: self.activation_commitment,
        }
    }
}

/// Process boundary for owner-private XMR protocol and chain effects.
///
/// Implementations must atomically persist intent before each external effect,
/// reconcile unknown outcomes before resubmission, and durably commit the
/// returned snapshot before returning. The SDK never receives actor secrets.
#[async_trait]
pub trait XmrRoleActorPort: Clone + Send + Sync {
    /// Structured adapter error.
    type Error: Error + Send + Sync + 'static;

    /// Validates Stage B with owner-private material and creates revision zero.
    async fn activate(
        &self,
        role: XmrRoleV1,
        candidate: &XmrNegotiationCandidateV1,
    ) -> Result<XmrLifecycleSnapshotV1, Self::Error>;

    /// Loads one already validated role-local lifecycle without Chat/Delivery.
    async fn resume(
        &self,
        role: XmrRoleV1,
        swap_id: [u8; 32],
    ) -> Result<Option<XmrLifecycleSnapshotV1>, Self::Error>;

    /// Reconciles or executes one semantic, idempotent lifecycle command.
    async fn drive(
        &self,
        current: &XmrLifecycleSnapshotV1,
        command: XmrLifecycleCommandV1,
    ) -> Result<XmrLifecycleSnapshotV1, Self::Error>;
}

/// Pre-lock facade composed from Delivery, Chat, and one role-fixed actor.
pub struct XmrPairSdk<Discovery, Negotiation, Actor> {
    role: XmrRoleV1,
    discovery: Discovery,
    negotiation: Negotiation,
    actor: Actor,
}

impl<Discovery, Negotiation, Actor> std::fmt::Debug for XmrPairSdk<Discovery, Negotiation, Actor> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("XmrPairSdk")
            .field("role", &self.role)
            .field("capabilities", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl<Discovery, Negotiation, Actor> XmrPairSdk<Discovery, Negotiation, Actor> {
    /// Composes a role-fixed SDK from narrow application capabilities.
    pub const fn new(
        role: XmrRoleV1,
        discovery: Discovery,
        negotiation: Negotiation,
        actor: Actor,
    ) -> Self {
        Self {
            role,
            discovery,
            negotiation,
            actor,
        }
    }

    /// Immutable role used for every operation.
    pub const fn role(&self) -> XmrRoleV1 {
        self.role
    }

    /// Publishes an authenticated Maker offer.
    ///
    /// # Errors
    ///
    /// Rejects Taker publication and preserves the discovery adapter error.
    pub async fn publish_offer(
        &self,
        offer: Discovery::Offer,
    ) -> Result<Discovery::OfferRef, XmrSdkError>
    where
        Discovery: OfferDiscovery,
    {
        self.require_role(XmrRoleV1::Maker)?;
        self.discovery
            .publish(offer)
            .await
            .map_err(|error| XmrSdkError::Discovery(Box::new(error)))
    }

    /// Discovers authenticated, unexpired offers without state mutation.
    ///
    /// # Errors
    ///
    /// Preserves the discovery adapter error.
    pub async fn discover_offers(
        &self,
        query: &Discovery::Query,
    ) -> Result<Vec<Discovery::OfferRef>, XmrSdkError>
    where
        Discovery: OfferDiscovery,
    {
        self.discovery
            .discover(query)
            .await
            .map_err(|error| XmrSdkError::Discovery(Box::new(error)))
    }

    /// Negotiates and validates one bounded canonical Stage-A/Stage-B envelope.
    ///
    /// # Errors
    ///
    /// Preserves the Chat adapter error and rejects invalid envelope/agreement
    /// bytes before the actor receives them.
    pub async fn negotiate(
        &self,
        offer: &Discovery::OfferRef,
        proposal: Negotiation::LocalProposal,
    ) -> Result<XmrNegotiationCandidateV1, XmrSdkError>
    where
        Discovery: OfferDiscovery,
        Negotiation: NegotiationChannel<OfferRef = Discovery::OfferRef>,
    {
        let wire = self
            .negotiation
            .negotiate(role_participant(self.role), offer, proposal)
            .await
            .map_err(|error| XmrSdkError::Negotiation(Box::new(error)))?;
        XmrNegotiationEnvelopeV1::from_wire(&wire)?.candidate()
    }

    /// Activates through the owner-private role actor after public negotiation.
    ///
    /// # Errors
    ///
    /// Preserves actor failure and rejects role, swap, agreement, phase, or
    /// revision substitution in the returned durable snapshot.
    pub async fn activate(
        &self,
        candidate: XmrNegotiationCandidateV1,
    ) -> Result<ActiveXmrSwap<Actor>, XmrSdkError>
    where
        Actor: XmrRoleActorPort,
    {
        let snapshot = self
            .actor
            .activate(self.role, &candidate)
            .await
            .map_err(|error| XmrSdkError::Actor(Box::new(error)))?;
        validate_activation_snapshot(&candidate, self.role, &snapshot)?;
        Ok(ActiveXmrSwap {
            actor: self.actor.clone(),
            snapshot,
        })
    }

    /// Resumes post-lock state against an independently retained identity.
    ///
    /// # Errors
    ///
    /// Preserves actor failure and rejects a substituted role, swap ID, phase,
    /// or revision.
    pub async fn resume(
        &self,
        identity: XmrLifecycleIdentityV1,
    ) -> Result<Option<ActiveXmrSwap<Actor>>, XmrSdkError>
    where
        Actor: XmrRoleActorPort,
    {
        if identity.role != self.role {
            return Err(XmrSdkError::ActorIdentityMismatch);
        }
        let Some(snapshot) = self
            .actor
            .resume(self.role, identity.swap_id)
            .await
            .map_err(|error| XmrSdkError::Actor(Box::new(error)))?
        else {
            return Ok(None);
        };
        if snapshot.identity() != identity || !snapshot.phase.accepts_revision(snapshot.revision) {
            return Err(XmrSdkError::ActorIdentityMismatch);
        }
        Ok(Some(ActiveXmrSwap {
            actor: self.actor.clone(),
            snapshot,
        }))
    }

    fn require_role(&self, expected: XmrRoleV1) -> Result<(), XmrSdkError> {
        if self.role == expected {
            Ok(())
        } else {
            Err(XmrSdkError::WrongRole {
                expected,
                actual: self.role,
            })
        }
    }
}

/// Post-activation role facade with no discovery or negotiation capability.
#[must_use]
pub struct ActiveXmrSwap<Actor> {
    actor: Actor,
    snapshot: XmrLifecycleSnapshotV1,
}

impl<Actor> std::fmt::Debug for ActiveXmrSwap<Actor> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ActiveXmrSwap")
            .field("actor", &"[REDACTED]")
            .field("snapshot", &self.snapshot)
            .finish()
    }
}

impl<Actor> ActiveXmrSwap<Actor>
where
    Actor: XmrRoleActorPort,
{
    /// Latest secret-free durable actor snapshot.
    pub const fn snapshot(&self) -> &XmrLifecycleSnapshotV1 {
        &self.snapshot
    }

    /// Reconciles and drives the next role-safe lifecycle action.
    ///
    /// # Errors
    ///
    /// Preserves actor failure and rejects identity, revision, transition, or
    /// command substitution.
    pub async fn advance(&mut self) -> Result<&XmrLifecycleSnapshotV1, XmrSdkError> {
        self.drive(XmrLifecycleCommandV1::Advance).await
    }

    /// Drives only the successful-claim branch.
    ///
    /// # Errors
    ///
    /// Rejects a claim from a non-claim phase and otherwise applies the same
    /// actor and transition checks as [`Self::advance`].
    pub async fn claim(&mut self) -> Result<&XmrLifecycleSnapshotV1, XmrSdkError> {
        if !matches!(
            self.snapshot.phase,
            XmrLifecyclePhaseV1::BothLocked | XmrLifecyclePhaseV1::ClaimAuthorized
        ) {
            return Err(XmrSdkError::CommandNotAvailable);
        }
        self.drive(XmrLifecycleCommandV1::Claim).await
    }

    /// Drives the evidence-selected refund or punishment branch.
    ///
    /// # Errors
    ///
    /// Rejects recovery before the LEZ lock or after a terminal phase and
    /// otherwise applies the actor and transition checks from [`Self::advance`].
    pub async fn refund(&mut self) -> Result<&XmrLifecycleSnapshotV1, XmrSdkError> {
        if !matches!(
            self.snapshot.phase,
            XmrLifecyclePhaseV1::LezLocked
                | XmrLifecyclePhaseV1::BothLocked
                | XmrLifecyclePhaseV1::RefundAuthorized
                | XmrLifecyclePhaseV1::PunishAuthorized
        ) {
            return Err(XmrSdkError::CommandNotAvailable);
        }
        self.drive(XmrLifecycleCommandV1::Refund).await
    }

    async fn drive(
        &mut self,
        command: XmrLifecycleCommandV1,
    ) -> Result<&XmrLifecycleSnapshotV1, XmrSdkError> {
        if self.snapshot.phase.terminal() {
            return Ok(&self.snapshot);
        }
        let next = self
            .actor
            .drive(&self.snapshot, command)
            .await
            .map_err(|error| XmrSdkError::Actor(Box::new(error)))?;
        validate_successor(&self.snapshot, &next, command)?;
        self.snapshot = next;
        Ok(&self.snapshot)
    }
}

fn validate_activation_snapshot(
    candidate: &XmrNegotiationCandidateV1,
    role: XmrRoleV1,
    snapshot: &XmrLifecycleSnapshotV1,
) -> Result<(), XmrSdkError> {
    if snapshot.swap_id != candidate.swap_id
        || snapshot.role != role
        || snapshot.agreement_commitment != candidate.agreement_commitment
        || snapshot.phase != XmrLifecyclePhaseV1::Activated
        || snapshot.revision != 0
        || snapshot.activation_commitment == [0; 32]
    {
        return Err(XmrSdkError::ActorIdentityMismatch);
    }
    Ok(())
}

fn validate_successor(
    current: &XmrLifecycleSnapshotV1,
    next: &XmrLifecycleSnapshotV1,
    command: XmrLifecycleCommandV1,
) -> Result<(), XmrSdkError> {
    if current.swap_id != next.swap_id
        || current.role != next.role
        || current.agreement_commitment != next.agreement_commitment
        || current.activation_commitment != next.activation_commitment
    {
        return Err(XmrSdkError::ActorIdentityMismatch);
    }
    if !next.phase.accepts_revision(next.revision) {
        return Err(XmrSdkError::InvalidActorRevision);
    }
    if current == next {
        return Ok(());
    }
    if next.revision != current.revision + 1 {
        return Err(XmrSdkError::InvalidActorRevision);
    }
    let valid_phase = matches!(
        (current.phase, next.phase),
        (
            XmrLifecyclePhaseV1::Activated,
            XmrLifecyclePhaseV1::LezLocked
        ) | (
            XmrLifecyclePhaseV1::LezLocked,
            XmrLifecyclePhaseV1::BothLocked | XmrLifecyclePhaseV1::RefundAuthorized
        ) | (
            XmrLifecyclePhaseV1::BothLocked,
            XmrLifecyclePhaseV1::ClaimAuthorized
                | XmrLifecyclePhaseV1::RefundAuthorized
                | XmrLifecyclePhaseV1::PunishAuthorized
        ) | (
            XmrLifecyclePhaseV1::ClaimAuthorized,
            XmrLifecyclePhaseV1::Completed
        ) | (
            XmrLifecyclePhaseV1::RefundAuthorized,
            XmrLifecyclePhaseV1::Refunded
        ) | (
            XmrLifecyclePhaseV1::PunishAuthorized,
            XmrLifecyclePhaseV1::Punished
        )
    );
    if !valid_phase {
        return Err(XmrSdkError::InvalidActorTransition);
    }
    if command == XmrLifecycleCommandV1::Claim
        && !matches!(
            next.phase,
            XmrLifecyclePhaseV1::ClaimAuthorized | XmrLifecyclePhaseV1::Completed
        )
    {
        return Err(XmrSdkError::InvalidActorTransition);
    }
    if command == XmrLifecycleCommandV1::Refund
        && !matches!(
            next.phase,
            XmrLifecyclePhaseV1::RefundAuthorized
                | XmrLifecyclePhaseV1::PunishAuthorized
                | XmrLifecyclePhaseV1::Refunded
                | XmrLifecyclePhaseV1::Punished
        )
    {
        return Err(XmrSdkError::InvalidActorTransition);
    }
    Ok(())
}

const fn role_participant(role: XmrRoleV1) -> Participant {
    match role {
        XmrRoleV1::Maker => Participant::Maker,
        XmrRoleV1::Taker => Participant::Taker,
    }
}

/// Public lifecycle failure with adapter sources preserved but secrets omitted.
#[derive(Debug, thiserror::Error)]
pub enum XmrSdkError {
    /// Operation is not authorized for this fixed role.
    #[error("XMR SDK role mismatch: expected {expected:?}, actual {actual:?}")]
    WrongRole {
        /// Required role.
        expected: XmrRoleV1,
        /// SDK role.
        actual: XmrRoleV1,
    },
    /// Stage-A validation failed.
    #[error(transparent)]
    Agreement(#[from] XmrAgreementV1Error),
    /// Stage-B wire is empty or exceeds its fixed bound.
    #[error("XMR activation wire length is invalid")]
    InvalidActivationWireLength,
    /// Negotiation envelope is empty or exceeds its fixed bound.
    #[error("XMR negotiation envelope is oversized")]
    OversizedNegotiationEnvelope,
    /// Negotiation envelope is malformed or has trailing bytes.
    #[error("XMR negotiation envelope is malformed")]
    MalformedNegotiationEnvelope,
    /// Negotiation envelope schema is unsupported.
    #[error("XMR negotiation envelope schema {0} is unsupported")]
    UnsupportedNegotiationEnvelope(u16),
    /// Negotiation envelope differs from its canonical encoding.
    #[error("XMR negotiation envelope is not canonical")]
    NonCanonicalNegotiationEnvelope,
    /// Delivery adapter failed.
    #[error("XMR offer discovery failed")]
    Discovery(#[source] Box<dyn Error + Send + Sync>),
    /// Chat adapter failed.
    #[error("XMR negotiation failed")]
    Negotiation(#[source] Box<dyn Error + Send + Sync>),
    /// Role actor failed while owning private/durable effects.
    #[error("XMR role actor failed")]
    Actor(#[source] Box<dyn Error + Send + Sync>),
    /// Actor returned a substituted role, swap, agreement, or activation.
    #[error("XMR actor snapshot identity mismatch")]
    ActorIdentityMismatch,
    /// Actor returned a skipped, regressed, or noncanonical revision.
    #[error("XMR actor snapshot revision is invalid")]
    InvalidActorRevision,
    /// Actor returned a lifecycle transition not permitted by the protocol.
    #[error("XMR actor lifecycle transition is invalid")]
    InvalidActorTransition,
    /// Requested manual action is unsafe in the current phase.
    #[error("XMR lifecycle command is not available in the current phase")]
    CommandNotAvailable,
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use super::*;

    #[derive(Clone)]
    struct Actor;

    #[async_trait]
    impl XmrRoleActorPort for Actor {
        type Error = Infallible;

        async fn activate(
            &self,
            role: XmrRoleV1,
            candidate: &XmrNegotiationCandidateV1,
        ) -> Result<XmrLifecycleSnapshotV1, Self::Error> {
            Ok(XmrLifecycleSnapshotV1 {
                swap_id: candidate.swap_id,
                role,
                agreement_commitment: candidate.agreement_commitment,
                activation_commitment: [3; 32],
                phase: XmrLifecyclePhaseV1::Activated,
                revision: 0,
            })
        }

        async fn resume(
            &self,
            role: XmrRoleV1,
            swap_id: [u8; 32],
        ) -> Result<Option<XmrLifecycleSnapshotV1>, Self::Error> {
            Ok(Some(XmrLifecycleSnapshotV1 {
                swap_id,
                role,
                agreement_commitment: [2; 32],
                activation_commitment: [3; 32],
                phase: XmrLifecyclePhaseV1::LezLocked,
                revision: 1,
            }))
        }

        async fn drive(
            &self,
            current: &XmrLifecycleSnapshotV1,
            command: XmrLifecycleCommandV1,
        ) -> Result<XmrLifecycleSnapshotV1, Self::Error> {
            let phase = match (current.phase, command) {
                (XmrLifecyclePhaseV1::Activated, XmrLifecycleCommandV1::Advance) => {
                    XmrLifecyclePhaseV1::LezLocked
                }
                (XmrLifecyclePhaseV1::LezLocked, XmrLifecycleCommandV1::Advance) => {
                    XmrLifecyclePhaseV1::BothLocked
                }
                (XmrLifecyclePhaseV1::BothLocked, XmrLifecycleCommandV1::Claim) => {
                    XmrLifecyclePhaseV1::ClaimAuthorized
                }
                (XmrLifecyclePhaseV1::ClaimAuthorized, XmrLifecycleCommandV1::Claim) => {
                    XmrLifecyclePhaseV1::Completed
                }
                (XmrLifecyclePhaseV1::LezLocked, XmrLifecycleCommandV1::Refund) => {
                    XmrLifecyclePhaseV1::RefundAuthorized
                }
                (XmrLifecyclePhaseV1::RefundAuthorized, XmrLifecycleCommandV1::Refund) => {
                    XmrLifecyclePhaseV1::Refunded
                }
                _ => current.phase,
            };
            let mut next = current.clone();
            if phase != current.phase {
                next.revision += 1;
            }
            next.phase = phase;
            Ok(next)
        }
    }

    fn candidate() -> XmrNegotiationCandidateV1 {
        XmrNegotiationCandidateV1 {
            agreement_wire: vec![4].into_boxed_slice(),
            activation_wire: vec![5].into_boxed_slice(),
            swap_id: [1; 32],
            agreement_commitment: [2; 32],
        }
    }

    fn snapshot(phase: XmrLifecyclePhaseV1) -> XmrLifecycleSnapshotV1 {
        XmrLifecycleSnapshotV1 {
            swap_id: [1; 32],
            role: XmrRoleV1::Taker,
            agreement_commitment: [2; 32],
            activation_commitment: [3; 32],
            phase,
            revision: match phase {
                XmrLifecyclePhaseV1::Activated => 0,
                XmrLifecyclePhaseV1::LezLocked => 1,
                XmrLifecyclePhaseV1::BothLocked => 2,
                XmrLifecyclePhaseV1::ClaimAuthorized
                | XmrLifecyclePhaseV1::RefundAuthorized
                | XmrLifecyclePhaseV1::PunishAuthorized => 3,
                XmrLifecyclePhaseV1::Completed
                | XmrLifecyclePhaseV1::Refunded
                | XmrLifecyclePhaseV1::Punished => 4,
            },
        }
    }

    #[test]
    fn lifecycle_transition_graph_accepts_claim_refund_and_exact_replay() {
        let activated = snapshot(XmrLifecyclePhaseV1::Activated);
        let lez_locked = snapshot(XmrLifecyclePhaseV1::LezLocked);
        let both = snapshot(XmrLifecyclePhaseV1::BothLocked);
        let claim = snapshot(XmrLifecyclePhaseV1::ClaimAuthorized);
        let complete = snapshot(XmrLifecyclePhaseV1::Completed);
        let refund = snapshot(XmrLifecyclePhaseV1::RefundAuthorized);
        let refunded = snapshot(XmrLifecyclePhaseV1::Refunded);

        validate_successor(&activated, &activated, XmrLifecycleCommandV1::Advance).unwrap();
        validate_successor(&activated, &lez_locked, XmrLifecycleCommandV1::Advance).unwrap();
        validate_successor(&lez_locked, &both, XmrLifecycleCommandV1::Advance).unwrap();
        validate_successor(&both, &claim, XmrLifecycleCommandV1::Claim).unwrap();
        validate_successor(&claim, &complete, XmrLifecycleCommandV1::Claim).unwrap();
        validate_successor(&both, &refund, XmrLifecycleCommandV1::Refund).unwrap();
        validate_successor(&refund, &refunded, XmrLifecycleCommandV1::Refund).unwrap();
    }

    #[test]
    fn lifecycle_transition_graph_rejects_skip_regression_identity_and_branch_crossing() {
        let activated = snapshot(XmrLifecyclePhaseV1::Activated);
        let mut both = snapshot(XmrLifecyclePhaseV1::BothLocked);
        assert!(matches!(
            validate_successor(&activated, &both, XmrLifecycleCommandV1::Advance),
            Err(XmrSdkError::InvalidActorRevision)
        ));

        both.swap_id = [9; 32];
        assert!(matches!(
            validate_successor(&activated, &both, XmrLifecycleCommandV1::Advance),
            Err(XmrSdkError::ActorIdentityMismatch)
        ));

        let claim = snapshot(XmrLifecyclePhaseV1::ClaimAuthorized);
        let refunded = snapshot(XmrLifecyclePhaseV1::Refunded);
        assert!(matches!(
            validate_successor(&claim, &refunded, XmrLifecycleCommandV1::Advance),
            Err(XmrSdkError::InvalidActorTransition)
        ));
    }

    #[test]
    fn negotiation_envelope_rejects_empty_trailing_and_unknown_schema() {
        assert!(matches!(
            XmrNegotiationEnvelopeV1::from_wire(&[]),
            Err(XmrSdkError::OversizedNegotiationEnvelope)
        ));
        let malformed = postcard::to_allocvec(&NegotiationEnvelopeWireV1 {
            schema_version: 9,
            agreement_wire: vec![0],
            activation_wire: vec![0],
        })
        .unwrap();
        assert!(matches!(
            XmrNegotiationEnvelopeV1::from_wire(&malformed),
            Err(XmrSdkError::UnsupportedNegotiationEnvelope(9))
        ));
        let mut trailing = malformed;
        trailing.push(0);
        assert!(matches!(
            XmrNegotiationEnvelopeV1::from_wire(&trailing),
            Err(XmrSdkError::MalformedNegotiationEnvelope)
        ));
    }

    #[tokio::test]
    async fn public_facade_claims_without_post_lock_discovery_or_negotiation() {
        let sdk = XmrPairSdk::new(XmrRoleV1::Taker, (), (), Actor);
        let mut active = sdk.activate(candidate()).await.unwrap();
        assert_eq!(
            active.advance().await.unwrap().phase(),
            XmrLifecyclePhaseV1::LezLocked
        );
        assert_eq!(
            active.advance().await.unwrap().phase(),
            XmrLifecyclePhaseV1::BothLocked
        );
        assert_eq!(
            active.claim().await.unwrap().phase(),
            XmrLifecyclePhaseV1::ClaimAuthorized
        );
        assert_eq!(
            active.claim().await.unwrap().phase(),
            XmrLifecyclePhaseV1::Completed
        );
    }

    #[tokio::test]
    async fn public_facade_resumes_and_refunds_through_the_actor_only() {
        let sdk = XmrPairSdk::new(XmrRoleV1::Taker, (), (), Actor);
        let identity = snapshot(XmrLifecyclePhaseV1::LezLocked).identity();
        let mut active = sdk.resume(identity).await.unwrap().unwrap();
        assert_eq!(active.snapshot().phase(), XmrLifecyclePhaseV1::LezLocked);
        assert_eq!(
            active.refund().await.unwrap().phase(),
            XmrLifecyclePhaseV1::RefundAuthorized
        );
        assert_eq!(
            active.refund().await.unwrap().phase(),
            XmrLifecyclePhaseV1::Refunded
        );
    }

    #[tokio::test]
    async fn resume_rejects_retained_activation_or_role_substitution() {
        let sdk = XmrPairSdk::new(XmrRoleV1::Taker, (), (), Actor);
        let mut identity = snapshot(XmrLifecyclePhaseV1::LezLocked).identity();
        identity.activation_commitment = [9; 32];
        assert!(matches!(
            sdk.resume(identity).await,
            Err(XmrSdkError::ActorIdentityMismatch)
        ));

        identity = snapshot(XmrLifecyclePhaseV1::LezLocked).identity();
        identity.role = XmrRoleV1::Maker;
        assert!(matches!(
            sdk.resume(identity).await,
            Err(XmrSdkError::ActorIdentityMismatch)
        ));
    }
}
