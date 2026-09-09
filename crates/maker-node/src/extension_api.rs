//! Owner API for publishing the exact local terms chosen by an external strategy.

use jsonrpsee::{RpcModule, core::RpcResult};
use lez_bridge_protocol::RequestId;
use lez_swap_store::{MakerOfferCommit, MakerOfferId, MakerRouteV1};
use serde::{Deserialize, Serialize};

use super::{
    INTERNAL_ERROR, MakerRpc, application_store_error, ensure_route_dependency_available,
    invalid_request, publish_offer_to_delivery, rpc_error, trusted_now_unix_seconds,
};

/// Publishes one immutable offer using the reviewed route and price revisions.
///
/// Use revisions returned by `maker_local_route_save_v1`, or by the policy and
/// local-price list methods. A concurrent configuration change rejects the
/// publication rather than substituting different terms. Only local-price
/// routes are accepted: the external program owns pricing policy.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OfferPublishRequestV1 {
    /// Request schema version; must be one.
    pub schema_version: u16,
    /// Durable identity for exact replay, including the revision guards.
    pub request_id: RequestId,
    /// New immutable offer identity.
    pub offer_id: MakerOfferId,
    /// Exact pair and economic direction.
    pub route: MakerRouteV1,
    /// Positive policy revision reviewed by the caller.
    pub expected_pair_revision: u64,
    /// Positive local-price revision reviewed by the caller.
    pub expected_price_revision: u64,
}

pub(super) fn register(module: &mut RpcModule<MakerRpc>) -> anyhow::Result<()> {
    module.register_blocking_method::<RpcResult<MakerOfferCommit>, _>(
        "maker_offer_publish_v1",
        |params, context, _| {
            let request: OfferPublishRequestV1 = params.one()?;
            if request.schema_version != 1
                || request.expected_pair_revision == 0
                || request.expected_price_revision == 0
            {
                return Err(invalid_request(
                    "expected schema version 1 and positive revisions",
                ));
            }
            ensure_route_dependency_available(&context, request.route)?;
            let commit = context
                .store
                .lock()
                .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?
                .publish_local_offer_at_revisions(
                    &request.request_id,
                    &request.offer_id,
                    request.route,
                    (
                        request.expected_pair_revision,
                        request.expected_price_revision,
                    ),
                    trusted_now_unix_seconds()?,
                )
                .map_err(application_store_error)?;
            publish_offer_to_delivery(&context, &request.offer_id)?;
            Ok(commit)
        },
    )?;
    Ok(())
}
