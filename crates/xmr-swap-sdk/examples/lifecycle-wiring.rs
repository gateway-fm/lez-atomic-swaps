//! Type-checked external wiring skeleton for the public XMR lifecycle facade.

use std::convert::Infallible;

use async_trait::async_trait;
use lez_swap_core::Participant;
use lez_swap_sdk_core::{NegotiationChannel, OfferDiscovery};
use lez_xmr_swap_sdk::{
    XmrLifecycleCommandV1, XmrLifecycleSnapshotV1, XmrNegotiationCandidateV1, XmrPairSdk,
    XmrRoleActorPort, XmrRoleV1,
};

#[derive(Clone)]
struct Delivery;

#[async_trait]
impl OfferDiscovery for Delivery {
    type Error = Infallible;
    type Offer = String;
    type OfferRef = String;
    type Query = String;

    async fn publish(&self, offer: Self::Offer) -> Result<Self::OfferRef, Self::Error> {
        Ok(offer)
    }

    async fn discover(&self, _query: &Self::Query) -> Result<Vec<Self::OfferRef>, Self::Error> {
        Ok(Vec::new())
    }
}

#[derive(Clone)]
struct Chat;

#[async_trait]
impl NegotiationChannel for Chat {
    type Error = Infallible;
    type LocalProposal = Vec<u8>;
    type OfferRef = String;

    async fn negotiate(
        &self,
        _local_participant: Participant,
        _offer: &Self::OfferRef,
        proposal: Self::LocalProposal,
    ) -> Result<Vec<u8>, Self::Error> {
        Ok(proposal)
    }
}

#[derive(Clone)]
struct RoleActor;

#[async_trait]
impl XmrRoleActorPort for RoleActor {
    type Error = Infallible;

    async fn activate(
        &self,
        _role: XmrRoleV1,
        _candidate: &XmrNegotiationCandidateV1,
    ) -> Result<XmrLifecycleSnapshotV1, Self::Error> {
        unreachable!("the real role actor validates Stage B and creates its journal")
    }

    async fn resume(
        &self,
        _role: XmrRoleV1,
        _swap_id: [u8; 32],
    ) -> Result<Option<XmrLifecycleSnapshotV1>, Self::Error> {
        Ok(None)
    }

    async fn drive(
        &self,
        _current: &XmrLifecycleSnapshotV1,
        _command: XmrLifecycleCommandV1,
    ) -> Result<XmrLifecycleSnapshotV1, Self::Error> {
        unreachable!("the real role actor persists intent before effects")
    }
}

fn main() {
    let sdk = XmrPairSdk::new(XmrRoleV1::Taker, Delivery, Chat, RoleActor);
    assert_eq!(sdk.role(), XmrRoleV1::Taker);
}
