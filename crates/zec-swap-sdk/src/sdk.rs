//! Pre-lock and post-lock LEZ/ZEC SDK facades.

use lez_swap_core::{Participant, Phase, SwapId};

use crate::{
    NegotiationChannel, OfferDiscovery, RecoveryStore, ZecAgreement, ZecLifecycleAction,
    ZecSdkError, lifecycle::next_action,
};

/// Complete pre-lock facade composed from application-supplied ports.
///
/// Activation returns [`ActiveZecSwap`], whose type deliberately omits the
/// discovery and negotiation handles.
#[derive(Debug)]
pub struct ZecPairSdk<Discovery, Negotiation, Lez, Zcash, Store> {
    local_participant: Participant,
    discovery: Discovery,
    negotiation: Negotiation,
    lez: Lez,
    zcash: Zcash,
    store: Store,
}

impl<Discovery, Negotiation, Lez, Zcash, Store>
    ZecPairSdk<Discovery, Negotiation, Lez, Zcash, Store>
{
    /// Composes a role-fixed SDK from narrow application capabilities.
    #[must_use]
    pub const fn new(
        local_participant: Participant,
        discovery: Discovery,
        negotiation: Negotiation,
        lez: Lez,
        zcash: Zcash,
        store: Store,
    ) -> Self {
        Self {
            local_participant,
            discovery,
            negotiation,
            lez,
            zcash,
            store,
        }
    }

    /// Participant fixed for every operation on this instance.
    #[must_use]
    pub const fn local_participant(&self) -> Participant {
        self.local_participant
    }
}

impl<Discovery, Negotiation, Lez, Zcash, Store>
    ZecPairSdk<Discovery, Negotiation, Lez, Zcash, Store>
where
    Discovery: OfferDiscovery,
    Negotiation: NegotiationChannel<OfferRef = Discovery::OfferRef>,
    Negotiation::LezTerms: Clone + std::fmt::Debug + Eq + Send + Sync + 'static,
    Lez: Clone,
    Zcash: Clone,
    Store: RecoveryStore<Negotiation::LezTerms>,
{
    /// Publishes a maker offer through the configured authenticated adapter.
    ///
    /// # Errors
    ///
    /// Returns [`ZecSdkError::WrongRole`] for a taker SDK or preserves the
    /// structured discovery-adapter source.
    pub async fn publish_offer(
        &self,
        offer: Discovery::Offer,
    ) -> Result<Discovery::OfferRef, ZecSdkError> {
        self.require_role(Participant::Maker)?;
        self.discovery
            .publish(offer)
            .await
            .map_err(|error| ZecSdkError::Discovery(Box::new(error)))
    }

    /// Discovers authenticated, unexpired offers without changing swap state.
    ///
    /// # Errors
    ///
    /// Preserves the structured discovery-adapter source.
    pub async fn discover_offers(
        &self,
        query: &Discovery::Query,
    ) -> Result<Vec<Discovery::OfferRef>, ZecSdkError> {
        self.discovery
            .discover(query)
            .await
            .map_err(|error| ZecSdkError::Discovery(Box::new(error)))
    }

    /// Obtains immutable mutually authenticated terms from the negotiation port.
    ///
    /// # Errors
    ///
    /// Preserves the structured negotiation-adapter source.
    pub async fn negotiate(
        &self,
        offer: &Discovery::OfferRef,
        proposal: Negotiation::LocalProposal,
    ) -> Result<ZecAgreement<Negotiation::LezTerms>, ZecSdkError> {
        self.negotiation
            .negotiate(self.local_participant, offer, proposal)
            .await
            .map_err(|error| ZecSdkError::Negotiation(Box::new(error)))
    }

    /// Persists immutable terms before returning a post-lock-capable SDK.
    ///
    /// The return type has no discovery or negotiation generic parameters, so
    /// those transports cannot become post-lock dependencies accidentally.
    ///
    /// # Errors
    ///
    /// Returns a persistence error before any chain capability is exposed.
    pub async fn activate(
        &self,
        agreement: ZecAgreement<Negotiation::LezTerms>,
    ) -> Result<ActiveZecSwap<Negotiation::LezTerms, Lez, Zcash, Store>, ZecSdkError> {
        let revision = self
            .store
            .create_agreement(self.local_participant, &agreement)
            .await
            .map_err(|error| ZecSdkError::Persistence(Box::new(error)))?;
        Ok(ActiveZecSwap {
            local_participant: self.local_participant,
            agreement,
            revision,
            lez: self.lez.clone(),
            zcash: self.zcash.clone(),
            store: self.store.clone(),
        })
    }

    /// Resumes a role-local active swap without consulting discovery or Chat.
    ///
    /// # Errors
    ///
    /// Preserves the structured recovery-store source.
    pub async fn resume(
        &self,
        swap_id: &SwapId,
    ) -> Result<Option<ActiveZecSwap<Negotiation::LezTerms, Lez, Zcash, Store>>, ZecSdkError> {
        let Some((revision, agreement)) = self
            .store
            .load_agreement(self.local_participant, swap_id)
            .await
            .map_err(|error| ZecSdkError::Persistence(Box::new(error)))?
        else {
            return Ok(None);
        };
        Ok(Some(ActiveZecSwap {
            local_participant: self.local_participant,
            agreement,
            revision,
            lez: self.lez.clone(),
            zcash: self.zcash.clone(),
            store: self.store.clone(),
        }))
    }

    fn require_role(&self, expected: Participant) -> Result<(), ZecSdkError> {
        if self.local_participant == expected {
            Ok(())
        } else {
            Err(ZecSdkError::WrongRole {
                expected,
                actual: self.local_participant,
            })
        }
    }
}

/// Post-lock SDK state containing only chain and recovery capabilities.
#[derive(Debug)]
pub struct ActiveZecSwap<LezTerms, Lez, Zcash, Store> {
    local_participant: Participant,
    agreement: ZecAgreement<LezTerms>,
    revision: u64,
    lez: Lez,
    zcash: Zcash,
    store: Store,
}

impl<LezTerms, Lez, Zcash, Store> ActiveZecSwap<LezTerms, Lez, Zcash, Store> {
    /// Fixed local participant; action methods never accept an arbitrary role.
    #[must_use]
    pub const fn local_participant(&self) -> Participant {
        self.local_participant
    }

    /// Immutable countersigned agreement.
    #[must_use]
    pub const fn agreement(&self) -> &ZecAgreement<LezTerms> {
        &self.agreement
    }

    /// Current deterministic protocol phase.
    #[must_use]
    pub const fn status(&self) -> Phase {
        self.agreement.coordinator().phase()
    }

    /// Last role-local durable revision loaded during activation/resume.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Next construction-specific role action, if one is currently safe.
    #[must_use]
    pub fn next_action(&self) -> ZecLifecycleAction {
        next_action(self.agreement.coordinator(), self.local_participant)
    }

    /// Typed LEZ chain capability owned after activation.
    #[must_use]
    pub const fn lez_port(&self) -> &Lez {
        &self.lez
    }

    /// Typed Zcash chain capability owned after activation.
    #[must_use]
    pub const fn zcash_port(&self) -> &Zcash {
        &self.zcash
    }

    /// Role-local recovery capability owned after activation.
    #[must_use]
    pub const fn recovery_store(&self) -> &Store {
        &self.store
    }
}
