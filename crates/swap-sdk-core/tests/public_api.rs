//! External-consumer compile and behavior contract for the shared SDK API.

use std::{collections::HashMap, sync::Mutex};

use async_trait::async_trait;
use lez_swap_sdk_core::{
    ClaimOrder, ErrorCategory, ExactPublicEffectBytes, ExactPublicEffectPlanV1,
    ExpectedPublicEffectId, NegotiationChannel, OfferDiscovery, Participant, ProtocolError,
    PublicEffectStepId, PublicEffectStepV1, SwapProtocol,
};

#[derive(Clone, Debug, Eq, PartialEq)]
struct Terms {
    safe: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ValidatedTerms(Terms);

#[derive(Clone, Debug, Eq, PartialEq)]
struct Prepared(ValidatedTerms);

#[derive(Clone, Debug, Eq, PartialEq)]
struct Evidence;

#[derive(Clone, Debug, Eq, PartialEq)]
struct Confirmed;

#[derive(Clone, Debug, Eq, PartialEq)]
struct Recovered([u8; 32]);

#[derive(Clone, Debug, Eq, PartialEq)]
struct CanonicalState;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TestError(ErrorCategory);

impl std::fmt::Display for TestError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("typed test protocol error")
    }
}

impl std::error::Error for TestError {}

impl ProtocolError for TestError {
    fn category(&self) -> ErrorCategory {
        self.0
    }
}

#[derive(Debug)]
struct Protocol;

impl SwapProtocol for Protocol {
    type Terms = Terms;
    type ValidatedTerms = ValidatedTerms;
    type Prepared = Prepared;
    type FirstLockTemplate = ExactPublicEffectPlanV1;
    type FirstLockEvidence = Evidence;
    type ConfirmedFirstLock = Confirmed;
    type SecondLockTemplate = ExactPublicEffectPlanV1;
    type RevealingClaimEvidence = Evidence;
    type RecoveredClaimMaterial = Recovered;
    type FollowupClaimTemplate = ExactPublicEffectPlanV1;
    type CanonicalChainState = CanonicalState;
    type RecoveryAction = &'static str;
    type Error = TestError;

    fn validate_terms(&self, terms: &Terms) -> Result<ValidatedTerms, TestError> {
        terms
            .safe
            .then(|| ValidatedTerms(terms.clone()))
            .ok_or(TestError(ErrorCategory::UnsafeDeadlineProfile))
    }

    fn prepare(&self, terms: ValidatedTerms) -> Result<Prepared, TestError> {
        Ok(Prepared(terms))
    }

    fn build_first_lock(&self, _: &Prepared) -> Result<ExactPublicEffectPlanV1, TestError> {
        one_step_plan("bitcoin.first_lock", "first", &[1])
    }

    fn validate_first_lock(&self, _: &Prepared, _: &Evidence) -> Result<Confirmed, TestError> {
        Ok(Confirmed)
    }

    fn build_second_lock(
        &self,
        _: &Prepared,
        _: &Confirmed,
    ) -> Result<ExactPublicEffectPlanV1, TestError> {
        ExactPublicEffectPlanV1::new(vec![
            effect("lez.initialize", "initialize-id", &[2]),
            effect("lez.fund", "fund-id", &[3]),
        ])
        .map_err(|_| TestError(ErrorCategory::MalformedEvidence))
    }

    fn claim_order(&self, _: &Prepared) -> ClaimOrder {
        ClaimOrder::FOREIGN_THEN_LEZ
    }

    fn validate_revealing_claim(&self, _: &Prepared, _: &Evidence) -> Result<Recovered, TestError> {
        Ok(Recovered([7; 32]))
    }

    fn build_followup_claim(
        &self,
        _: &Prepared,
        _: &Recovered,
    ) -> Result<ExactPublicEffectPlanV1, TestError> {
        one_step_plan("lez.followup_claim", "followup", &[4])
    }

    fn recovery_action(&self, _: &Prepared, _: &CanonicalState) -> Result<&'static str, TestError> {
        Ok("wait")
    }
}

fn effect(step: &str, expected_id: &str, bytes: &[u8]) -> PublicEffectStepV1 {
    PublicEffectStepV1::new(
        PublicEffectStepId::new(step).expect("valid step"),
        ExpectedPublicEffectId::new(expected_id).expect("valid expected ID"),
        ExactPublicEffectBytes::new(bytes.to_vec()).expect("valid exact bytes"),
    )
}

fn one_step_plan(
    step: &str,
    expected_id: &str,
    bytes: &[u8],
) -> Result<ExactPublicEffectPlanV1, TestError> {
    ExactPublicEffectPlanV1::new(vec![effect(step, expected_id, bytes)])
        .map_err(|_| TestError(ErrorCategory::MalformedEvidence))
}

#[test]
fn pair_crate_can_implement_complete_lifecycle_with_multi_step_lock() {
    let protocol = Protocol;
    let terms = protocol
        .validate_terms(&Terms { safe: true })
        .expect("validated terms");
    let prepared = protocol.prepare(terms).expect("prepared protocol");
    let first = protocol
        .validate_first_lock(&prepared, &Evidence)
        .expect("confirmed first lock");
    let maker_lock = protocol
        .build_second_lock(&prepared, &first)
        .expect("maker lock plan");

    assert_eq!(maker_lock.steps().len(), 2);
    assert_eq!(maker_lock.steps()[0].step().as_str(), "lez.initialize");
    assert_eq!(maker_lock.steps()[1].step().as_str(), "lez.fund");
    assert_eq!(
        protocol.claim_order(&prepared),
        ClaimOrder::FOREIGN_THEN_LEZ
    );
    assert_eq!(
        protocol.recovery_action(&prepared, &CanonicalState),
        Ok("wait")
    );
}

#[derive(Debug, thiserror::Error)]
#[error("memory transport failed")]
struct TransportError;

#[derive(Debug, Default)]
struct MemoryDiscovery {
    offers: Mutex<Vec<Vec<u8>>>,
}

#[async_trait]
impl OfferDiscovery for MemoryDiscovery {
    type Error = TransportError;
    type Offer = Vec<u8>;
    type OfferRef = usize;
    type Query = ();

    async fn publish(&self, offer: Vec<u8>) -> Result<usize, TransportError> {
        let mut offers = self.offers.lock().expect("offer lock");
        offers.push(offer);
        Ok(offers.len() - 1)
    }

    async fn discover(&self, (): &()) -> Result<Vec<usize>, TransportError> {
        Ok((0..self.offers.lock().expect("offer lock").len()).collect())
    }
}

#[derive(Debug, Default)]
struct MemoryNegotiation {
    records: Mutex<HashMap<usize, Vec<u8>>>,
}

#[async_trait]
impl NegotiationChannel for MemoryNegotiation {
    type Error = TransportError;
    type LocalProposal = Vec<u8>;
    type OfferRef = usize;

    async fn negotiate(
        &self,
        _: Participant,
        offer: &usize,
        proposal: Vec<u8>,
    ) -> Result<Vec<u8>, TransportError> {
        self.records
            .lock()
            .expect("negotiation lock")
            .insert(*offer, proposal.clone());
        Ok(proposal)
    }
}

#[tokio::test]
async fn application_can_supply_discovery_and_negotiation_without_chain_ports() {
    let discovery = MemoryDiscovery::default();
    let offer = discovery
        .publish(b"signed-expiring-offer".to_vec())
        .await
        .expect("publish");
    assert_eq!(discovery.discover(&()).await.expect("discover"), [offer]);

    let negotiation = MemoryNegotiation::default();
    let wire = negotiation
        .negotiate(Participant::Taker, &offer, b"dual-signed-wire".to_vec())
        .await
        .expect("negotiate");
    assert_eq!(wire, b"dual-signed-wire");
}
