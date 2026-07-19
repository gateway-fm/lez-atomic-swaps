//! M4 capability boundary for publishing the Stage-B Taker claim partial.
//!
//! Primitive bridge requests can be constructed by any caller. This module
//! mints an opaque, linear capability only after a validated Stage-B activation
//! proves the exact published partial and the concrete authenticated bridge
//! client returns one exact prepared authorization transaction.

use lez_bridge_client::{BridgeClient, BridgeClientError};
use lez_bridge_protocol::{
    MessageContext, Participant as BridgeParticipant, PrepareNativeXmrClaimAuthorizationV3Request,
    PreparedTransaction, ProtocolValueError, RequestId, RuntimeDescriptor, XmrClaimPartialV3,
    XmrNativeEscrowTermsV3,
};
use lez_swap_core::Participant;
use lez_xmr_swap_sdk::{XmrActivatedAgreementV1, XmrAgreementV1, XmrAgreementV1Error};
use std::fmt;
use thiserror::Error;

use crate::{LezBridgeAdapter, XmrLezBridgeBindingV3, XmrLezBridgeBindingV3Error};

/// Exact prepared capability for publishing the committed Taker claim partial.
///
/// Fields and construction are private, and the value is deliberately not
/// `Clone`: downstream orchestration must move the one authenticated result
/// into its durable one-attempt publication boundary.
/// ```compile_fail
/// use lez_bridge_adapter::PreparedXmrClaimAuthorizationEvidenceV3;
///
/// fn requires_clone<T: Clone>() {}
/// requires_clone::<PreparedXmrClaimAuthorizationEvidenceV3>();
/// ```
#[derive(Eq, PartialEq)]
#[must_use]
pub struct PreparedXmrClaimAuthorizationEvidenceV3 {
    context: MessageContext,
    preparer: Participant,
    runtime: RuntimeDescriptor,
    terms: XmrNativeEscrowTermsV3,
    authorization: PreparedTransaction,
}

impl fmt::Debug for PreparedXmrClaimAuthorizationEvidenceV3 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedXmrClaimAuthorizationEvidenceV3")
            .field("authority", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl PreparedXmrClaimAuthorizationEvidenceV3 {
    /// Exact authenticated run and request identity used for preparation.
    pub const fn context(&self) -> &MessageContext {
        &self.context
    }

    /// Actor that proved and prepared publication of its committed partial.
    #[must_use]
    pub const fn preparer(&self) -> Participant {
        self.preparer
    }

    /// Exact dedicated Taker runtime used by the concrete bridge client.
    pub const fn runtime(&self) -> &RuntimeDescriptor {
        &self.runtime
    }

    /// Exact Stage-B-derived native-XMR guest terms.
    pub const fn terms(&self) -> XmrNativeEscrowTermsV3 {
        self.terms
    }

    /// Consumes the linear capability and returns its exact prepared transaction.
    ///
    /// This is a trusted-single-process `PoC` escape hatch between workspace
    /// crates, not a hostile-caller or production non-bypassability guarantee.
    /// The generic sidecar route still rejects this transaction and the local
    /// node route must remain isolated from the actor. Production moves this
    /// extraction behind a dedicated release-service process.
    pub fn into_unsubmitted_authorization(self) -> PreparedTransaction {
        self.authorization
    }
}

/// Failure preparing one Stage-B-authorized claim-partial publication.
#[derive(Debug, Error)]
pub enum PreparedXmrClaimAuthorizationErrorV3 {
    /// Only the Taker owns the partial committed for guest publication.
    #[error("only the XMR Taker may prepare claim-partial publication")]
    WrongPreparer,
    /// Stage B could not be re-derived against the supplied base agreement.
    #[error("XMR Stage-B bridge binding could not be re-derived")]
    StageB(#[source] XmrLezBridgeBindingV3Error),
    /// The caller supplied a valid binding for a different Stage-B activation.
    #[error("XMR claim authorization binding differs from re-derived Stage B")]
    BindingMismatch,
    /// The partial is not the exact valid Taker partial committed by Stage B.
    #[error("XMR claim partial is not authorized by Stage B")]
    PublishedPartial(#[source] XmrAgreementV1Error),
    /// Stage-B terms do not bind the adapter-owned role and runtime.
    #[error("XMR claim authorization is not bound to the adapter runtime")]
    RuntimeBinding(#[source] ProtocolValueError),
    /// The exact partial cannot be represented by the strict bridge protocol.
    #[error("XMR claim partial violates the strict bridge protocol")]
    Protocol(#[source] ProtocolValueError),
    /// The authenticated concrete bridge preparation failed.
    #[error("XMR claim authorization bridge preparation failed")]
    Bridge(#[source] BridgeClientError),
    /// The response contradicted the caller-owned request after transport.
    #[error("XMR claim authorization response changed its request binding")]
    ResponseBinding,
}

impl LezBridgeAdapter<BridgeClient> {
    /// Prepares publication of the exact Taker claim partial committed in Stage B.
    ///
    /// Stage B and the exact partial are independently revalidated before the
    /// authenticated bridge client is called. The client then enforces its own
    /// immutable run, role, and runtime binding and makes exactly one wire call.
    ///
    /// # Errors
    ///
    /// Rejects role, Stage-B, binding, partial, run, runtime, protocol, bridge,
    /// response-context, terms, or empty/oversized prepared-transaction bytes.
    pub async fn prepare_xmr_claim_authorization_v3(
        &self,
        agreement: &XmrAgreementV1,
        activation: &XmrActivatedAgreementV1,
        binding: &XmrLezBridgeBindingV3,
        request_id: RequestId,
        taker_claim_partial: [u8; 32],
    ) -> Result<PreparedXmrClaimAuthorizationEvidenceV3, PreparedXmrClaimAuthorizationErrorV3> {
        if self.local_participant != Participant::Taker {
            return Err(PreparedXmrClaimAuthorizationErrorV3::WrongPreparer);
        }
        let rederived = XmrLezBridgeBindingV3::new(agreement, activation)
            .map_err(PreparedXmrClaimAuthorizationErrorV3::StageB)?;
        if &rederived != binding {
            return Err(PreparedXmrClaimAuthorizationErrorV3::BindingMismatch);
        }
        activation
            .verify_published_taker_claim_partial(agreement, taker_claim_partial)
            .map_err(PreparedXmrClaimAuthorizationErrorV3::PublishedPartial)?;

        let context =
            MessageContext::new(self.run_id.clone(), request_id, BridgeParticipant::Taker);
        binding
            .validate_runtime_binding(&context, &self.runtime)
            .map_err(PreparedXmrClaimAuthorizationErrorV3::RuntimeBinding)?;
        let claim_partial = XmrClaimPartialV3::new(taker_claim_partial)
            .map_err(PreparedXmrClaimAuthorizationErrorV3::Protocol)?;
        let request = PrepareNativeXmrClaimAuthorizationV3Request::new(
            context.clone(),
            self.runtime.clone(),
            binding.terms(),
            claim_partial,
        );
        let result = self
            .transport
            .prepare_native_xmr_claim_authorization_v3(request)
            .await
            .map_err(PreparedXmrClaimAuthorizationErrorV3::Bridge)?;
        if result.context != context || result.terms != binding.terms() {
            return Err(PreparedXmrClaimAuthorizationErrorV3::ResponseBinding);
        }
        Ok(PreparedXmrClaimAuthorizationEvidenceV3 {
            context,
            preparer: Participant::Taker,
            runtime: self.runtime.clone(),
            terms: binding.terms(),
            authorization: result.authorization,
        })
    }
}
