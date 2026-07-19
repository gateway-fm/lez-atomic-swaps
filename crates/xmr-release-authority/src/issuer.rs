//! Non-forgeable Stage-B release issuer for the XMR claim authorization.
//!
//! The public operation consumes every chain and topology capability required
//! for release. Callers cannot supply a deadline, publication identity,
//! publication bytes, or an untyped success status.

use lez_bridge_adapter::{
    FinalizedXmrLezFirstLockEvidenceV3, PreparedXmrClaimAuthorizationEvidenceV3,
    XmrLezBridgeBindingV3, XmrLezBridgeBindingV3Error,
};
use lez_bridge_protocol::{
    Participant as BridgeParticipant, RunId, RuntimeCompatibility, RuntimeDescriptor,
    XmrNativeEscrowTermsV3,
};
use lez_swap_core::Participant;
use lez_xmr_monero_adapter::{
    MoneroNetwork, MoneroTopologyBindingError, VerifiedMoneroOutputObservation,
    VerifiedMoneroTopologyAttestation,
};
use lez_xmr_swap_sdk::{MoneroAddressNetworkV1, XmrActivatedAgreementV1, XmrAgreementV1};
use thiserror::Error;
use zeroize::Zeroizing;

use super::{
    PublicationProtectionKey, ReleasePlan, append, derive_activation_id, hash, monero_resource_id,
    observation_bytes,
};
use crate::store::{ReleaseError, ReleaseSnapshot, ReleaseStore};

const RUN_ID_DOMAIN: &[u8] = b"lez-atomic-swaps/xmr-release/run-id/v1";
const LEZ_EVIDENCE_DOMAIN: &[u8] = b"lez-atomic-swaps/xmr-release/lez-evidence/v1";
const TOPOLOGY_DOMAIN: &[u8] = b"lez-atomic-swaps/xmr-release/topology/v1";
const TARGET_DOMAIN: &[u8] = b"lez-atomic-swaps/xmr-release/target/v1";
const TERMS_DOMAIN: &[u8] = b"lez-atomic-swaps/xmr-release/terms/v3";

/// Failure to mint one durable XMR claim-authorization release.
#[derive(Debug, Error)]
pub enum XmrClaimReleasePreparationError {
    /// The supplied validated Stage B cannot be re-derived against Stage A.
    #[error("XMR release Stage-B binding could not be re-derived")]
    StageB(#[source] XmrLezBridgeBindingV3Error),
    /// An opaque capability was minted for a role other than the Taker.
    #[error("XMR release evidence is not owned by the Taker")]
    WrongRole,
    /// Finalized Fund and prepared authorization capabilities disagree.
    #[error("XMR release LEZ evidence bindings differ")]
    EvidenceBinding,
    /// The topology attestation does not authenticate the observed output.
    #[error("XMR release Monero topology binding failed")]
    Topology(#[source] MoneroTopologyBindingError),
    /// The observed Monero output differs from the countersigned agreement.
    #[error("XMR release Monero output differs from Stage A")]
    MoneroBinding,
    /// The output has fewer confirmations than the countersigned policy.
    #[error("XMR release Monero output lacks required confirmations")]
    InsufficientConfirmations,
    /// Finalized Fund time is not inside the guest claim interval.
    #[error("XMR release finalized Fund clock is outside the claim interval")]
    InvalidReleaseWindow,
    /// The authenticated durable release journal rejected the derived plan.
    #[error("XMR release journal rejected the derived plan")]
    Journal(#[source] ReleaseError),
}

impl ReleaseStore {
    /// Consumes exact Stage-B, finalized-Fund, Monero, and topology authority and
    /// durably prepares the one committed claim-authorization publication.
    ///
    /// The operational journal interval starts at the finalized Fund clock.
    /// Its exclusive end is derived from the same signed refund timestamp used
    /// by the checked guest. The guest has no corresponding lower-bound
    /// predicate, so only the exclusive upper deadline is identical.
    #[allow(
        clippy::needless_pass_by_value,
        clippy::too_many_arguments,
        reason = "opaque non-Clone evidence is deliberately surrendered by ownership"
    )]
    pub fn prepare_xmr_claim_release(
        &self,
        agreement: &XmrAgreementV1,
        activation: &XmrActivatedAgreementV1,
        first_lock: FinalizedXmrLezFirstLockEvidenceV3,
        authorization: PreparedXmrClaimAuthorizationEvidenceV3,
        observation: VerifiedMoneroOutputObservation,
        topology: VerifiedMoneroTopologyAttestation,
        key: &PublicationProtectionKey,
    ) -> Result<ReleaseSnapshot, XmrClaimReleasePreparationError> {
        let expected = XmrLezBridgeBindingV3::new(agreement, activation)
            .map_err(XmrClaimReleasePreparationError::StageB)?;
        let terms = expected.terms();
        validate_lez_evidence(&first_lock, &authorization, &terms)?;
        topology
            .validate_observation(first_lock.run_id(), &observation)
            .map_err(XmrClaimReleasePreparationError::Topology)?;
        validate_monero_observation(agreement, &observation)?;

        let terms_input = terms.to_input();
        let clock = first_lock.finalized_clock();
        if clock.timestamp_ms >= terms_input.refund_at_ms {
            return Err(XmrClaimReleasePreparationError::InvalidReleaseWindow);
        }

        let run_id = run_id_digest(first_lock.run_id());
        let target = release_target_bytes(first_lock.run_id(), first_lock.runtime(), &terms);
        let authorization = authorization.into_unsubmitted_authorization();
        let publication_id = *authorization.transaction_id.as_bytes();
        let publication = Zeroizing::new(authorization.exact_bytes.into_vec());
        let plan = ReleasePlan {
            activation: derive_activation_id(terms_input.swap_id.as_bytes(), &run_id),
            swap_id: *terms_input.swap_id.as_bytes(),
            run_id,
            lez_commitment: lez_evidence_commitment(&first_lock, &target),
            topology_commitment: topology_commitment(&topology),
            resource_id: monero_resource_id(&observation),
            observation: observation_bytes(&observation),
            claim_partial_commitment: *terms_input.claim_partial_commitment.as_bytes(),
            target,
            publication_id,
            window_start: clock.timestamp_ms,
            window_end: terms_input.refund_at_ms,
            publication,
        };
        self.prepare(plan, key)
            .map_err(XmrClaimReleasePreparationError::Journal)
    }

    /// Authenticates and reloads one derived XMR claim release after restart.
    ///
    /// Callers use the public swap and run identities; the internal binary run
    /// digest and activation key are re-derived here.
    pub fn load_xmr_claim_release(
        &self,
        swap_id: [u8; 32],
        run_id: &RunId,
        key: &PublicationProtectionKey,
    ) -> Result<ReleaseSnapshot, ReleaseError> {
        let run_digest = run_id_digest(run_id);
        self.load_by_activation_run(derive_activation_id(&swap_id, &run_digest), run_digest, key)
    }
}

fn validate_lez_evidence(
    first_lock: &FinalizedXmrLezFirstLockEvidenceV3,
    authorization: &PreparedXmrClaimAuthorizationEvidenceV3,
    expected_terms: &XmrNativeEscrowTermsV3,
) -> Result<(), XmrClaimReleasePreparationError> {
    if first_lock.observer() != Participant::Taker
        || authorization.preparer() != Participant::Taker
        || authorization.context().sidecar_role != BridgeParticipant::Taker
        || first_lock.runtime().sidecar_role != BridgeParticipant::Taker
        || authorization.runtime().sidecar_role != BridgeParticipant::Taker
    {
        return Err(XmrClaimReleasePreparationError::WrongRole);
    }
    if first_lock.run_id() != &authorization.context().run_id
        || first_lock.runtime() != authorization.runtime()
        || first_lock.terms() != *expected_terms
        || authorization.terms() != *expected_terms
    {
        return Err(XmrClaimReleasePreparationError::EvidenceBinding);
    }
    Ok(())
}

fn validate_monero_observation(
    agreement: &XmrAgreementV1,
    observation: &VerifiedMoneroOutputObservation,
) -> Result<(), XmrClaimReleasePreparationError> {
    let expected = agreement.body().monero();
    if !networks_match(expected.network(), observation.network())
        || expected.genesis_hash() != observation.genesis_hash()
        || expected.address() != observation.destination().to_string()
        || expected.amount_piconero() != observation.amount_piconero()
    {
        return Err(XmrClaimReleasePreparationError::MoneroBinding);
    }
    if observation.confirmations() < u64::from(expected.required_confirmations()) {
        return Err(XmrClaimReleasePreparationError::InsufficientConfirmations);
    }
    Ok(())
}

const fn networks_match(expected: MoneroAddressNetworkV1, observed: MoneroNetwork) -> bool {
    matches!(
        (expected, observed),
        (MoneroAddressNetworkV1::Regtest, MoneroNetwork::Regtest)
            | (MoneroAddressNetworkV1::Stagenet, MoneroNetwork::Stagenet)
    )
}

fn run_id_digest(run_id: &RunId) -> [u8; 32] {
    let mut encoded = RUN_ID_DOMAIN.to_vec();
    append(&mut encoded, run_id.as_str().as_bytes());
    hash(&encoded)
}

fn release_target_bytes(
    run_id: &RunId,
    runtime: &RuntimeDescriptor,
    terms: &XmrNativeEscrowTermsV3,
) -> Vec<u8> {
    let mut encoded = TARGET_DOMAIN.to_vec();
    append(&mut encoded, run_id.as_str().as_bytes());
    append(
        &mut encoded,
        &[bridge_participant_tag(runtime.sidecar_role)],
    );
    append(&mut encoded, &[compatibility_tag(runtime.compatibility)]);
    for value in [
        runtime.chain_id.as_bytes(),
        runtime.channel_id.as_bytes(),
        runtime.genesis_block_hash.as_bytes(),
        runtime.escrow_program_id.as_bytes(),
        runtime.signer_account_id.as_bytes(),
    ] {
        append(&mut encoded, value);
    }
    append(&mut encoded, &terms_bytes(terms));
    encoded
}

fn terms_bytes(terms: &XmrNativeEscrowTermsV3) -> Vec<u8> {
    let terms = (*terms).to_input();
    let mut encoded = TERMS_DOMAIN.to_vec();
    for value in [
        terms.swap_id.as_bytes(),
        terms.activation_commitment.as_bytes(),
        terms.escrow_program_id.as_bytes(),
        terms.authenticated_transfer_program_id.as_bytes(),
        terms.metadata_account_id.as_bytes(),
        terms.custody_account_id.as_bytes(),
    ] {
        append(&mut encoded, value);
    }
    append(&mut encoded, &[bridge_participant_tag(terms.depositor)]);
    append(&mut encoded, terms.depositor_account_id.as_bytes());
    append(&mut encoded, &[bridge_participant_tag(terms.claimant)]);
    for value in [
        terms.claimant_account_id.as_bytes(),
        terms.claim_aggregate_x_only_public_key.as_bytes(),
        terms.claim_authority_account_id.as_bytes(),
        terms.refund_aggregate_x_only_public_key.as_bytes(),
        terms.refund_authority_account_id.as_bytes(),
        terms.maker_dleq_transcript_commitment.as_bytes(),
        terms.taker_dleq_transcript_commitment.as_bytes(),
        terms.claim_partial_context_binding.as_bytes(),
        terms.claim_partial_commitment.as_bytes(),
    ] {
        append(&mut encoded, value);
    }
    append(&mut encoded, &terms.amount.to_be_bytes());
    append(&mut encoded, &terms.refund_at_ms.to_be_bytes());
    append(&mut encoded, &terms.punish_at_ms.to_be_bytes());
    for value in [
        terms.claim_message_hash.as_bytes(),
        terms.refund_message_hash.as_bytes(),
        terms.punish_message_hash.as_bytes(),
    ] {
        append(&mut encoded, value);
    }
    encoded
}

fn lez_evidence_commitment(
    evidence: &FinalizedXmrLezFirstLockEvidenceV3,
    target: &[u8],
) -> [u8; 32] {
    let transaction = evidence.exact_funding();
    let clock = evidence.finalized_clock();
    let window = evidence.scanned_window();
    let facts = evidence.facts();
    let mut encoded = LEZ_EVIDENCE_DOMAIN.to_vec();
    append(&mut encoded, target);
    append(&mut encoded, transaction.transaction_id.as_bytes());
    append(&mut encoded, transaction.exact_bytes.as_slice());
    append(&mut encoded, clock.block_hash.as_bytes());
    append(&mut encoded, &clock.height.to_be_bytes());
    append(&mut encoded, &clock.timestamp_ms.to_be_bytes());
    append(&mut encoded, &window.start_height().to_be_bytes());
    append(&mut encoded, &window.max_blocks().to_be_bytes());
    append(
        &mut encoded,
        facts.transaction.position.block_hash.as_bytes(),
    );
    append(
        &mut encoded,
        &facts.transaction.position.height.to_be_bytes(),
    );
    append(
        &mut encoded,
        &facts.transaction.position.transaction_index.to_be_bytes(),
    );
    append(&mut encoded, &facts.containing_block.block_id.to_be_bytes());
    append(&mut encoded, facts.containing_block.block_hash.as_bytes());
    append(
        &mut encoded,
        &facts.containing_block.timestamp_ms.to_be_bytes(),
    );
    append(&mut encoded, facts.custody.account_id.as_bytes());
    append(&mut encoded, facts.custody.owner_program_id.as_bytes());
    append(&mut encoded, &facts.custody.balance.as_u128().to_be_bytes());
    hash(&encoded)
}

fn topology_commitment(topology: &VerifiedMoneroTopologyAttestation) -> [u8; 32] {
    let identity = topology.chain_identity();
    let mut encoded = TOPOLOGY_DOMAIN.to_vec();
    append(&mut encoded, topology.run_id().as_str().as_bytes());
    append(&mut encoded, &[monero_network_tag(identity.network())]);
    append(&mut encoded, &identity.genesis_hash());
    append(&mut encoded, topology.daemon_origin().as_bytes());
    append(&mut encoded, topology.target_wallet_origin().as_bytes());
    append(&mut encoded, topology.foreign_wallet_origin().as_bytes());
    append(&mut encoded, topology.daemon_version().as_bytes());
    append(
        &mut encoded,
        &topology.target_wallet_version().to_be_bytes(),
    );
    append(
        &mut encoded,
        &topology.foreign_wallet_version().to_be_bytes(),
    );
    append(&mut encoded, &[u8::from(topology.offline())]);
    append(&mut encoded, &topology.peer_count().to_be_bytes());
    hash(&encoded)
}

const fn bridge_participant_tag(participant: BridgeParticipant) -> u8 {
    match participant {
        BridgeParticipant::Maker => 0,
        BridgeParticipant::Taker => 1,
    }
}

const fn compatibility_tag(compatibility: RuntimeCompatibility) -> u8 {
    match compatibility {
        RuntimeCompatibility::NssaV0_1_2 => 0,
        RuntimeCompatibility::LeeV0_2_0 => 1,
    }
}

const fn monero_network_tag(network: MoneroNetwork) -> u8 {
    match network {
        MoneroNetwork::Regtest => 0,
        MoneroNetwork::Stagenet => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_digest_is_domain_separated_and_stable() {
        let run = RunId::new("m4-issuer-run-0001").expect("valid run");
        let digest = run_id_digest(&run);
        assert_eq!(digest, run_id_digest(&run));
        assert_ne!(digest, hash(run.as_str().as_bytes()));
        assert_ne!(digest, [0; 32]);
    }

    #[test]
    fn monero_network_domains_must_match_exactly() {
        assert!(networks_match(
            MoneroAddressNetworkV1::Regtest,
            MoneroNetwork::Regtest,
        ));
        assert!(networks_match(
            MoneroAddressNetworkV1::Stagenet,
            MoneroNetwork::Stagenet,
        ));
        assert!(!networks_match(
            MoneroAddressNetworkV1::Regtest,
            MoneroNetwork::Stagenet,
        ));
        assert!(!networks_match(
            MoneroAddressNetworkV1::Stagenet,
            MoneroNetwork::Regtest,
        ));
    }
}
