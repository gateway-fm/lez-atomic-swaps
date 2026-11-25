//! Monero Chat negotiation: the take-time reservation and ceremony rounds
//! (`xmr_reserve.rs`), Stage-A staging and Stage-B activation.

use super::*;

pub(super) fn register_xmr_chat_methods(module: &mut RpcModule<MakerRpc>) -> anyhow::Result<()> {
    module.register_async_method("xmr_reserve_v1", |params, context, _| async move {
        let request: XmrReserveRequestV1 = params.one()?;
        super::xmr_reserve::reserve(request, context).await
    })?;
    module.register_async_method("xmr_ceremony_reserve_v1", |params, context, _| async move {
        let request: XmrCeremonyReserveRequestV1 = params.one()?;
        super::xmr_reserve::ceremony_reserve(request, context).await
    })?;
    module.register_async_method("xmr_ceremony_nonce_v1", |params, context, _| async move {
        let request: XmrCeremonyNonceRequestV1 = params.one()?;
        super::xmr_reserve::ceremony_nonce(request, context).await
    })?;
    module.register_async_method("xmr_ceremony_partial_v1", |params, context, _| async move {
        let request: XmrCeremonyPartialRequestV1 = params.one()?;
        super::xmr_reserve::ceremony_partial(request, context).await
    })?;
    module.register_blocking_method::<RpcResult<XmrChatStageAResponseV1>, _>(
        "xmr_chat_stage_a_v1",
        |params, context, _| {
            let request: XmrChatStageARequestV1 = params.one()?;
            stage_xmr_chat(&request, &context)
        },
    )?;
    module.register_blocking_method::<RpcResult<XmrChatActivateResponseV1>, _>(
        "xmr_chat_activate_v1",
        |params, context, _| {
            let request: XmrChatActivateRequestV1 = params.one()?;
            activate_xmr_chat(&request, &context)
        },
    )?;
    Ok(())
}

const fn offer_time_is_acceptable(
    has_durable_negotiation: bool,
    created_at: u64,
    expires_at: u64,
    now: u64,
) -> bool {
    has_durable_negotiation || (created_at <= now && now < expires_at)
}

#[allow(clippy::too_many_lines)]
fn stage_xmr_chat(
    request: &XmrChatStageARequestV1,
    context: &MakerRpc,
) -> RpcResult<XmrChatStageAResponseV1> {
    if request.schema_version != 1
        || request.foreign_units == 0
        || request.stage_a_wire.is_empty()
        || request.stage_a_wire.len() > MAX_XMR_AGREEMENT_WIRE_BYTES
    {
        return Err(invalid_request("unsupported or empty XMR Chat Stage A"));
    }
    let delivery = context
        .delivery
        .as_ref()
        .ok_or_else(|| invalid_request("maker Chat Delivery is unavailable"))?;
    let reservations = context
        .xmr_reservations
        .as_ref()
        .ok_or_else(|| invalid_request("maker XMR reservations are unavailable"))?;
    let existing = {
        let store = context
            .store
            .lock()
            .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
        store
            .load_xmr_maker_negotiation(&request.offer_id)
            .map_err(application_store_error)?
    };
    let authenticated = delivery
        .authenticate_envelope(&request.signed_offer_envelope)
        .map_err(|error| invalid_request(error.to_string()))?;
    let offer = authenticated.offer();
    let agreement = XmrAgreementV1::from_wire(&request.stage_a_wire).map_err(invalid_request)?;
    let lez_units = offer
        .quote_foreign_amount(request.foreign_units)
        .map_err(invalid_request)?;
    let offer_commitment = authenticated.commitment();
    let now_unix_seconds = trusted_now_unix_seconds()?;
    if offer.id() != &request.offer_id
        || offer.route().pair() != Pair::Monero
        || offer.route().direction() != SwapDirection::TakerSellsLez
        || !offer_time_is_acceptable(
            existing.is_some(),
            offer.created_at_unix_seconds(),
            offer.expires_at_unix_seconds(),
            now_unix_seconds,
        )
        || agreement.body().direction() != XmrSwapDirectionV1::TakerSellsLez
        || agreement.body().swap_id()
            != maker_xmr_chat_swap_id(&offer_commitment, &request.reservation_id)
        || agreement.body().monero().amount_piconero() != request.foreign_units
        || agreement.body().lez().amount() != lez_units
    {
        return Err(invalid_request(
            "XMR Stage A is not bound to the selected offer",
        ));
    }
    reservations.check_stage_a(
        &request.reservation_id,
        &request.stage_a_wire,
        now_unix_seconds,
    )?;

    let mut store = context
        .store
        .lock()
        .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
    let reserved_at = existing.as_ref().map_or(
        now_unix_seconds,
        MakerXmrNegotiationV1::reserved_at_unix_seconds,
    );
    let candidate = MakerXmrNegotiationV1::stage_a(
        request.reservation_id.clone(),
        offer_commitment,
        request.foreign_units,
        lez_units,
        reserved_at,
        request.stage_a_wire.clone(),
    )
    .map_err(invalid_request)?;
    if let Some(commit) = store
        .preflight_maker_xmr_stage_a_replay(
            &request.request_id,
            &request.offer_id,
            request.expected_offer_revision,
            &candidate,
        )
        .map_err(application_store_error)?
    {
        return Ok(XmrChatStageAResponseV1 {
            schema_version: 1,
            offer_revision: commit.revision(),
            was_replay: true,
            reservation_id: request.reservation_id.clone(),
            lez_units,
            swap_id: hex::encode(agreement.body().swap_id()).into(),
            agreement_commitment: agreement.agreement_commitment(),
        });
    }
    let commit = store
        .stage_xmr_maker_negotiation(
            &request.request_id,
            &request.offer_id,
            request.expected_offer_revision,
            &candidate,
        )
        .map_err(application_store_error)?;
    Ok(XmrChatStageAResponseV1 {
        schema_version: 1,
        offer_revision: commit.revision(),
        was_replay: commit.was_replay(),
        reservation_id: request.reservation_id.clone(),
        lez_units,
        swap_id: hex::encode(agreement.body().swap_id()).into(),
        agreement_commitment: agreement.agreement_commitment(),
    })
}

fn activate_xmr_chat(
    request: &XmrChatActivateRequestV1,
    context: &MakerRpc,
) -> RpcResult<XmrChatActivateResponseV1> {
    if request.schema_version != 1
        || request.activation_wire.is_empty()
        || request.activation_wire.len() > MAX_XMR_ACTIVATION_WIRE_BYTES
    {
        return Err(invalid_request("unsupported or empty XMR Chat Stage B"));
    }
    let reservations = context
        .xmr_reservations
        .as_ref()
        .ok_or_else(|| invalid_request("maker XMR reservations are unavailable"))?;
    let now_unix_seconds = trusted_now_unix_seconds()?;
    let staged = context
        .store
        .lock()
        .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?
        .load_xmr_maker_negotiation(&request.offer_id)
        .map_err(application_store_error)?
        .ok_or_else(|| rpc_error(NOT_FOUND, "XMR Stage A is unavailable"))?;
    if staged.reservation_id() != &request.reservation_id {
        return Err(invalid_request(
            "XMR Stage B does not match the durable reservation",
        ));
    }
    let agreement = XmrAgreementV1::from_wire(staged.stage_a_wire()).map_err(invalid_request)?;
    // The swap's own view key validates Stage B; its actor is provisioned
    // before the store commit, outside the store lock.
    let (activation, actor) = reservations.activate(
        &request.reservation_id,
        &agreement,
        &request.activation_wire,
        now_unix_seconds,
    )?;
    let initial = activation
        .initial_coordinator(&agreement)
        .map_err(invalid_request)?;
    let mut store = context
        .store
        .lock()
        .map_err(|_| rpc_error(INTERNAL_ERROR, "swap store lock poisoned"))?;
    let accepted_at = match staged.status() {
        MakerXmrNegotiationStatus::StageAAccepted => now_unix_seconds,
        MakerXmrNegotiationStatus::Activated => staged
            .activated_at_unix_seconds()
            .ok_or_else(|| rpc_error(INTERNAL_ERROR, "XMR activation state is corrupt"))?,
    };
    let accepted = MakerXmrActivationAcceptance::new(
        &initial,
        Participant::Maker,
        &agreement,
        &activation,
        request.activation_wire.clone(),
        accepted_at,
    )
    .map_err(invalid_request)?;
    let commit = store
        .complete_maker_xmr_negotiation_and_register_actor(
            &request.request_id,
            &request.offer_id,
            request.expected_offer_revision,
            &request.reservation_id,
            &accepted,
            &initial,
            &actor,
            accepted_at,
        )
        .map_err(application_store_error)?;
    Ok(XmrChatActivateResponseV1 {
        schema_version: 1,
        offer_revision: commit.offer_revision(),
        was_replay: commit.was_replay(),
        swap_id: initial.id().as_str().into(),
        activation_commitment: activation.activation_commitment(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request_id(value: &str) -> RequestId {
        RequestId::new(value).expect("bounded request ID")
    }

    fn offer_id(value: &str) -> MakerOfferId {
        MakerOfferId::new(value).expect("bounded offer ID")
    }

    #[test]
    fn live_offer_window_applies_only_before_durable_stage_a() {
        assert!(offer_time_is_acceptable(false, 100, 200, 100));
        assert!(!offer_time_is_acceptable(false, 100, 200, 200));
        assert!(offer_time_is_acceptable(true, 100, 200, 200));
        assert!(offer_time_is_acceptable(true, 100, 200, u64::MAX));
    }

    #[test]
    fn responses_are_explicitly_secret_free() {
        let stage = XmrChatStageAResponseV1 {
            schema_version: 1,
            offer_revision: 2,
            was_replay: false,
            reservation_id: request_id("xmr-chat-secret-free-reservation-001"),
            lez_units: 42,
            swap_id: "11".repeat(32).into(),
            agreement_commitment: [0x22; 32],
        };
        let activation = XmrChatActivateResponseV1 {
            schema_version: 1,
            offer_revision: 3,
            was_replay: false,
            swap_id: "11".repeat(32).into(),
            activation_commitment: [0x33; 32],
        };
        for encoded in [
            serde_json::to_string(&stage).unwrap(),
            serde_json::to_string(&activation).unwrap(),
        ] {
            for forbidden in ["private", "path", "manifest", "actor", "wire"] {
                assert!(!encoded.contains(forbidden), "response leaked {forbidden}");
            }
        }
    }

    #[test]
    fn maximum_stage_a_wire_round_trips_below_chat_transport_cap() {
        let request = XmrChatStageARequestV1 {
            schema_version: 1,
            request_id: request_id("xmr-chat-size-stage-001"),
            offer_id: offer_id("xmr-chat-size-offer-001"),
            expected_offer_revision: 1,
            reservation_id: request_id("xmr-chat-size-reservation-001"),
            foreign_units: 1,
            signed_offer_envelope: vec![u8::MAX; 65_536],
            stage_a_wire: vec![u8::MAX; MAX_XMR_AGREEMENT_WIRE_BYTES],
        };
        let json = serde_json::to_vec(&request).expect("serialize maximum Stage A");
        assert!(
            json.len() < 1024 * 1024,
            "maximum binary wires plus worst-case Delivery JSON must fit Chat"
        );
        let decoded: XmrChatStageARequestV1 =
            serde_json::from_slice(&json).expect("round-trip maximum Stage A");
        assert_eq!(decoded.stage_a_wire, request.stage_a_wire);
        assert_eq!(decoded.signed_offer_envelope, request.signed_offer_envelope);
    }

    #[test]
    fn maximum_activation_wire_round_trips_canonically() {
        let request = XmrChatActivateRequestV1 {
            schema_version: 1,
            request_id: request_id("xmr-chat-size-activate-001"),
            offer_id: offer_id("xmr-chat-size-offer-002"),
            expected_offer_revision: 2,
            reservation_id: request_id("xmr-chat-size-reservation-002"),
            activation_wire: vec![0xa5; MAX_XMR_ACTIVATION_WIRE_BYTES],
        };
        let json = serde_json::to_vec(&request).expect("serialize maximum Stage B");
        let decoded: XmrChatActivateRequestV1 =
            serde_json::from_slice(&json).expect("round-trip maximum Stage B");
        assert_eq!(decoded.activation_wire, request.activation_wire);
    }

    #[test]
    fn oversized_wires_are_rejected_before_transport() {
        let stage_a = XmrChatStageARequestV1 {
            schema_version: 1,
            request_id: request_id("xmr-chat-oversize-stage-001"),
            offer_id: offer_id("xmr-chat-oversize-offer-001"),
            expected_offer_revision: 1,
            reservation_id: request_id("xmr-chat-oversize-reservation-001"),
            foreign_units: 1,
            signed_offer_envelope: Vec::new(),
            stage_a_wire: vec![0; MAX_XMR_AGREEMENT_WIRE_BYTES + 1],
        };
        assert!(serde_json::to_vec(&stage_a).is_err());
    }
}
