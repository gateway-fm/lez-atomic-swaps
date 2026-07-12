use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use lez_swap_core::{
    Chain, ChainPosition, ConfirmationPolicy, Pair, Participant, Phase, RecoverySchedule,
    SwapCoordinator, SwapDirection, SwapId, TimelockSafety,
};
use lez_zec_swap_sdk::{
    ActiveZecSwap, Bip199Contract, ClaimPreimage, ExpectedBip199Output, NegotiationChannel,
    OfferDiscovery, RecoveryStore, ZEC_AGREEMENT_SCHEMA_V1, ZecAgreement, ZecPairSdk, ZecProfileId,
    ZecRefundProfile, ZecSdkError, ZecSwapBinding,
};
use zcash_protocol::{
    consensus::{BranchId, NetworkType},
    value::Zatoshis,
};

#[derive(Clone, Debug, Eq, PartialEq)]
struct Offer(u64);

#[derive(Clone, Debug)]
struct Proposal;

#[derive(Clone, Debug, Default)]
struct MemoryDiscovery {
    offers: Arc<Mutex<Vec<Offer>>>,
}

#[async_trait]
impl OfferDiscovery for MemoryDiscovery {
    type Error = TestPortError;
    type Offer = Offer;
    type OfferRef = Offer;
    type Query = ();

    async fn publish(&self, offer: Self::Offer) -> Result<Self::OfferRef, Self::Error> {
        self.offers.lock().expect("offers lock").push(offer.clone());
        Ok(offer)
    }

    async fn discover(&self, _query: &Self::Query) -> Result<Vec<Self::OfferRef>, Self::Error> {
        Ok(self.offers.lock().expect("offers lock").clone())
    }
}

#[derive(Clone, Debug)]
struct MemoryNegotiation {
    agreement: ZecAgreement<String>,
}

#[async_trait]
impl NegotiationChannel for MemoryNegotiation {
    type Error = TestPortError;
    type LezTerms = String;
    type LocalProposal = Proposal;
    type OfferRef = Offer;

    async fn negotiate(
        &self,
        _local_participant: Participant,
        _offer: &Self::OfferRef,
        _proposal: Self::LocalProposal,
    ) -> Result<ZecAgreement<Self::LezTerms>, Self::Error> {
        Ok(self.agreement.clone())
    }
}

type AgreementMap = HashMap<String, (u64, ZecAgreement<String>)>;

#[derive(Clone, Debug, Default)]
struct MemoryStore {
    agreements: Arc<Mutex<AgreementMap>>,
}

#[async_trait]
impl RecoveryStore<String> for MemoryStore {
    type Error = TestPortError;

    async fn create_agreement(
        &self,
        local_participant: Participant,
        agreement: &ZecAgreement<String>,
    ) -> Result<u64, Self::Error> {
        let key = store_key(local_participant, agreement.coordinator().id());
        let mut records = self.agreements.lock().expect("agreements lock");
        if records.insert(key, (0, agreement.clone())).is_some() {
            return Err(TestPortError("agreement already exists".to_owned()));
        }
        Ok(0)
    }

    async fn load_agreement(
        &self,
        local_participant: Participant,
        swap_id: &SwapId,
    ) -> Result<Option<(u64, ZecAgreement<String>)>, Self::Error> {
        Ok(self
            .agreements
            .lock()
            .expect("agreements lock")
            .get(&store_key(local_participant, swap_id))
            .cloned())
    }
}

fn store_key(participant: Participant, swap_id: &SwapId) -> String {
    format!("{participant:?}:{}", swap_id.as_str())
}

#[derive(Clone, Copy, Debug)]
struct NoopLez;

#[derive(Clone, Copy, Debug)]
struct NoopZcash;

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{0}")]
struct TestPortError(String);

#[tokio::test]
async fn independent_roles_negotiate_and_activate_without_transport_handles() {
    let agreement = agreement("sdk-forward", SwapDirection::TakerSellsForeign);
    let discovery = MemoryDiscovery::default();
    let negotiation = MemoryNegotiation {
        agreement: agreement.clone(),
    };
    let maker_store = MemoryStore::default();
    let taker_store = MemoryStore::default();
    let maker = ZecPairSdk::new(
        Participant::Maker,
        discovery.clone(),
        negotiation.clone(),
        NoopLez,
        NoopZcash,
        maker_store.clone(),
    );
    let taker = ZecPairSdk::new(
        Participant::Taker,
        discovery,
        negotiation,
        NoopLez,
        NoopZcash,
        taker_store.clone(),
    );

    let published = maker
        .publish_offer(Offer(7))
        .await
        .expect("maker publishes");
    assert_eq!(
        taker.discover_offers(&()).await.expect("taker discovers"),
        vec![published.clone()]
    );
    assert!(matches!(
        taker.publish_offer(Offer(9)).await,
        Err(ZecSdkError::WrongRole {
            expected: Participant::Maker,
            actual: Participant::Taker
        })
    ));

    let maker_terms = maker
        .negotiate(&published, Proposal)
        .await
        .expect("maker obtains countersigned terms");
    let taker_terms = taker
        .negotiate(&published, Proposal)
        .await
        .expect("taker obtains countersigned terms");
    assert_eq!(maker_terms, taker_terms);

    let maker_active: ActiveZecSwap<String, NoopLez, NoopZcash, MemoryStore> = maker
        .activate(maker_terms)
        .await
        .expect("maker persists before activation");
    let taker_active: ActiveZecSwap<String, NoopLez, NoopZcash, MemoryStore> = taker
        .activate(taker_terms)
        .await
        .expect("taker persists before activation");

    assert_eq!(maker_active.local_participant(), Participant::Maker);
    assert_eq!(taker_active.local_participant(), Participant::Taker);
    assert_eq!(maker_active.status(), Phase::Offered);
    assert_eq!(taker_active.status(), Phase::Offered);
    assert_eq!(maker_active.agreement().transcript_commitment(), &[7; 32]);
    assert_eq!(taker_active.agreement().transcript_commitment(), &[7; 32]);
    assert_eq!(maker_store.agreements.lock().expect("maker store").len(), 1);
    assert_eq!(taker_store.agreements.lock().expect("taker store").len(), 1);

    let resumed = maker
        .resume(agreement.coordinator().id())
        .await
        .expect("load succeeds")
        .expect("maker agreement exists");
    assert_eq!(resumed.local_participant(), Participant::Maker);
    assert_eq!(resumed.status(), Phase::Offered);
}

#[test]
fn agreement_rejects_wrong_pair_and_confirmation_policy() {
    let valid = agreement("sdk-valid", SwapDirection::TakerSellsForeign);
    assert_eq!(valid.schema_version(), ZEC_AGREEMENT_SCHEMA_V1);

    let profile = ZecRefundProfile::for_id(ZecProfileId::DeterministicLocalV1);
    let wrong_pair = SwapCoordinator::new_with_confirmation_policies(
        SwapId::new("wrong-pair").expect("id"),
        Pair::Bitcoin,
        SwapDirection::TakerSellsForeign,
        ConfirmationPolicy::new(1).expect("policy"),
        ConfirmationPolicy::new(1).expect("policy"),
        schedule(SwapDirection::TakerSellsForeign),
    );
    assert!(ZecAgreement::new(1, wrong_pair, binding(), String::new(), [7; 32]).is_err());

    let wrong_policy = SwapCoordinator::new_with_confirmation_policies(
        SwapId::new("wrong-policy").expect("id"),
        Pair::Zcash,
        SwapDirection::TakerSellsForeign,
        ConfirmationPolicy::new(profile.zcash_confirmations() + 1).expect("policy"),
        ConfirmationPolicy::new(profile.lez_confirmations()).expect("policy"),
        schedule(SwapDirection::TakerSellsForeign),
    );
    assert!(ZecAgreement::new(1, wrong_policy, binding(), String::new(), [7; 32]).is_err());
}

#[test]
fn claim_preimage_is_redacted_and_not_a_wire_record() {
    let preimage = ClaimPreimage::new([0x42; 32]);
    assert_eq!(preimage.expose_secret(), &[0x42; 32]);
    assert_eq!(format!("{preimage:?}"), "ClaimPreimage([REDACTED])");
}

fn agreement(id: &str, direction: SwapDirection) -> ZecAgreement<String> {
    let profile = ZecRefundProfile::for_id(ZecProfileId::DeterministicLocalV1);
    let coordinator = SwapCoordinator::new_with_confirmation_policies(
        SwapId::new(id).expect("id"),
        Pair::Zcash,
        direction,
        ConfirmationPolicy::new(match direction {
            SwapDirection::TakerSellsForeign => profile.zcash_confirmations(),
            SwapDirection::TakerSellsLez => profile.lez_confirmations(),
        })
        .expect("taker policy"),
        ConfirmationPolicy::new(match direction {
            SwapDirection::TakerSellsForeign => profile.lez_confirmations(),
            SwapDirection::TakerSellsLez => profile.zcash_confirmations(),
        })
        .expect("maker policy"),
        schedule(direction),
    );
    ZecAgreement::new(
        ZEC_AGREEMENT_SCHEMA_V1,
        coordinator,
        binding(),
        "typed LEZ terms supplied by the generated client".to_owned(),
        [7; 32],
    )
    .expect("valid agreement")
}

fn binding() -> ZecSwapBinding {
    let contract = Bip199Contract::new(120, [1; 20], [2; 32], [3; 20]);
    let output = ExpectedBip199Output::new(
        NetworkType::Regtest,
        BranchId::Nu6_2,
        Zatoshis::from_u64(100_000_000).expect("value"),
        contract,
    );
    ZecSwapBinding::new(ZecProfileId::DeterministicLocalV1, output).expect("binding")
}

fn schedule(direction: SwapDirection) -> RecoverySchedule {
    let safety =
        TimelockSafety::between(Chain::Lez, Chain::Zcash, 1_000, 1_200, 100).expect("margin");
    let (maker, taker) = match direction {
        SwapDirection::TakerSellsForeign => (
            ChainPosition::timestamp(Chain::Lez, 1_000),
            ChainPosition::block_height(Chain::Zcash, 120),
        ),
        SwapDirection::TakerSellsLez => (
            ChainPosition::block_height(Chain::Zcash, 120),
            ChainPosition::timestamp(Chain::Lez, 1_000),
        ),
    };
    RecoverySchedule::new(Pair::Zcash, direction, maker, taker, safety).expect("schedule")
}
