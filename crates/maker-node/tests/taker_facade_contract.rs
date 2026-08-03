//! Static and Serde contract for the role-fixed Taker facade boundary.

use lez_bridge_protocol::RequestId;
use lez_maker_node::{
    TAKER_FACADE_METHODS_V1, TakerActionCommitV1, TakerClaimRequestV1, TakerDependencyStateV1,
    TakerHealthRequestV1, TakerHealthV1, TakerInitiationCapabilityV1, TakerInitiationCommitV1,
    TakerMakerIdentityV1, TakerMonitoringCapabilityV1, TakerOfferListRequestV1, TakerOfferListV1,
    TakerPairCapabilityV1, TakerPrivacyGuidanceV1, TakerRefundRequestV1,
    TakerSwapInitiateRequestV1, TakerSwapListRequestV1, TakerSwapListV1, TakerSwapMonitorRequestV1,
    TakerSwapStateV1, TakerSwapViewV1, TakerTerminalActionCapabilityV1, TakerTerminalActionV1,
    taker_pair_capabilities_v1,
};
use lez_swap_core::{Pair, SwapDirection, SwapId};
use lez_swap_store::{MakerOfferId, MakerRouteV1};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

#[test]
fn method_allowlist_is_exact_and_has_no_generic_execution_escape_hatch() {
    assert_eq!(
        TAKER_FACADE_METHODS_V1,
        [
            "taker_health",
            "taker_offer_list_v1",
            "taker_swap_list_v1",
            "taker_swap_initiate_v1",
            "taker_swap_monitor_v1",
            "taker_swap_claim_v1",
            "taker_swap_refund_v1",
        ]
    );
    for method in TAKER_FACADE_METHODS_V1 {
        assert!(!method.contains("execute"));
        assert!(!method.contains("command"));
        assert!(!method.contains("raw"));
    }
}

#[test]
fn every_untrusted_request_is_versioned_strict_and_contains_no_authority_field() {
    let route = json!({"pair": "Zcash", "direction": "TakerSellsLez"});
    let maker_identity = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
    let commitment = vec![3_u8; 32];
    let requests = [
        (
            json!({"schema_version": 1}),
            reject_unknown::<TakerHealthRequestV1> as fn(Value),
        ),
        (
            json!({"schema_version": 1, "route": null}),
            reject_unknown::<TakerOfferListRequestV1> as fn(Value),
        ),
        (
            json!({"schema_version": 1}),
            reject_unknown::<TakerSwapListRequestV1> as fn(Value),
        ),
        (
            json!({
                "schema_version": 1,
                "request_id": "m6-initiate-request-001",
                "offer_id": "m6-zec-offer-001",
                "route": route,
                "maker_identity": maker_identity,
                "signed_envelope_sha256": commitment,
                "foreign_units": 200_000_000_u64,
                "expected_lez_units": 1_820_u128,
            }),
            reject_unknown::<TakerSwapInitiateRequestV1> as fn(Value),
        ),
        (
            json!({"schema_version": 1, "swap_id": "m6-zec-swap-001"}),
            reject_unknown::<TakerSwapMonitorRequestV1> as fn(Value),
        ),
        (
            json!({
                "schema_version": 1,
                "request_id": "m6-claim-request-001",
                "swap_id": "m6-zec-swap-001",
                "expected_generation": 4,
            }),
            reject_unknown::<TakerClaimRequestV1> as fn(Value),
        ),
        (
            json!({
                "schema_version": 1,
                "request_id": "m6-refund-request-001",
                "swap_id": "m6-zec-swap-001",
                "expected_generation": 4,
            }),
            reject_unknown::<TakerRefundRequestV1> as fn(Value),
        ),
    ];

    for (request, reject) in requests {
        assert_secret_and_authority_free(&request);
        reject(request);
    }
}

#[test]
fn pair_capabilities_report_only_current_role_fixed_semantics() {
    let capabilities = taker_pair_capabilities_v1();
    assert_eq!(capabilities.len(), 3);
    assert_capability(
        &capabilities[0],
        Pair::Bitcoin,
        SwapDirection::TakerSellsForeign,
        TakerTerminalActionCapabilityV1::FullLifecycle,
    );
    assert_capability(
        &capabilities[1],
        Pair::Monero,
        SwapDirection::TakerSellsLez,
        TakerTerminalActionCapabilityV1::EffectCheckpointOnly,
    );
    assert_capability(
        &capabilities[2],
        Pair::Zcash,
        SwapDirection::TakerSellsLez,
        TakerTerminalActionCapabilityV1::FullLifecycle,
    );

    let encoded = serde_json::to_value(&capabilities).unwrap();
    assert_secret_and_authority_free(&encoded);
    assert_eq!(encoded[1]["claim"], "effect_checkpoint_only");
    assert_eq!(encoded[1]["refund"], "effect_checkpoint_only");
}

#[test]
fn response_shapes_are_versioned_and_never_carry_paths_keys_or_raw_effect_material() {
    let health = TakerHealthV1::new(
        true,
        TakerDependencyStateV1::Available,
        TakerDependencyStateV1::Unavailable,
    );
    let methods = health.registered_methods();
    assert!(methods.health());
    assert!(methods.offer_list());
    assert!(!methods.swap_list());
    assert!(!methods.initiate());
    assert!(!methods.monitor());
    assert!(!methods.claim());
    assert!(!methods.refund());
    let route = MakerRouteV1::new(Pair::Zcash, SwapDirection::TakerSellsLez).unwrap();
    let swap = TakerSwapViewV1 {
        schema_version: 1,
        swap_id: SwapId::new("m6-zec-swap-001").unwrap(),
        offer_id: MakerOfferId::new("m6-zec-offer-001").unwrap(),
        route,
        foreign_units: 200_000_000,
        lez_units: 1_820,
        progress_generation: 4,
        state: TakerSwapStateV1::Completed,
        available_action: None,
        privacy_guidance: Some(TakerPrivacyGuidanceV1::ShieldReceivedTransparentZecSeparately),
    };
    let action = TakerActionCommitV1 {
        schema_version: 1,
        swap_id: swap.swap_id.clone(),
        action: TakerTerminalActionV1::Claim,
        requested_after_generation: 3,
        was_replay: false,
    };
    let responses = [
        serde_json::to_value(health).unwrap(),
        serde_json::to_value(TakerOfferListV1 {
            schema_version: 1,
            offers: Vec::new(),
        })
        .unwrap(),
        serde_json::to_value(TakerSwapListV1 {
            schema_version: 1,
            swaps: vec![swap.clone()],
        })
        .unwrap(),
        serde_json::to_value(TakerInitiationCommitV1 {
            schema_version: 1,
            swap: swap.clone(),
            was_replay: false,
        })
        .unwrap(),
        serde_json::to_value(swap).unwrap(),
        serde_json::to_value(action).unwrap(),
    ];
    for response in responses {
        assert_eq!(response["schema_version"], 1);
        assert_secret_and_authority_free(&response);
    }
}

#[test]
fn maker_identity_wire_is_fixed_canonical_and_curve_validated() {
    let generator = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
    let identity: TakerMakerIdentityV1 = serde_json::from_value(json!(generator)).unwrap();
    assert_eq!(serde_json::to_value(identity).unwrap(), generator);

    for invalid in [
        json!(generator.to_uppercase()),
        json!("02"),
        json!(format!("04{}", "00".repeat(32))),
        json!(vec![2_u8; 33]),
    ] {
        assert!(serde_json::from_value::<TakerMakerIdentityV1>(invalid).is_err());
    }
}

#[test]
fn action_requests_are_distinct_types_without_a_caller_selected_action_field() {
    let request_id = RequestId::new("m6-action-request-001").unwrap();
    let swap_id = SwapId::new("m6-zec-swap-001").unwrap();
    let claim = TakerClaimRequestV1 {
        schema_version: 1,
        request_id: request_id.clone(),
        swap_id: swap_id.clone(),
        expected_generation: 7,
    };
    let refund = TakerRefundRequestV1 {
        schema_version: 1,
        request_id,
        swap_id,
        expected_generation: 7,
    };
    for request in [
        serde_json::to_value(claim).unwrap(),
        serde_json::to_value(refund).unwrap(),
    ] {
        assert!(request.get("action").is_none());
        assert!(request.get("expected_generation").is_some());
    }
}

#[test]
fn every_request_accepts_schema_version_one() {
    assert_all_request_schema_versions(1, true);
}

#[test]
fn every_request_rejects_schema_version_zero() {
    assert_all_request_schema_versions(0, false);
}

#[test]
fn every_request_rejects_schema_version_two() {
    assert_all_request_schema_versions(2, false);
}

fn assert_all_request_schema_versions(schema_version: u16, accepted: bool) {
    let route = json!({"pair": "Zcash", "direction": "TakerSellsLez"});
    let maker_identity = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
    let commitment = vec![3_u8; 32];

    assert_request_schema_version::<TakerHealthRequestV1>(
        json!({"schema_version": schema_version}),
        accepted,
        TakerHealthRequestV1::validate_schema_version,
    );
    assert_request_schema_version::<TakerOfferListRequestV1>(
        json!({"schema_version": schema_version, "route": null}),
        accepted,
        TakerOfferListRequestV1::validate_schema_version,
    );
    assert_request_schema_version::<TakerSwapListRequestV1>(
        json!({"schema_version": schema_version}),
        accepted,
        TakerSwapListRequestV1::validate_schema_version,
    );
    assert_request_schema_version::<TakerSwapInitiateRequestV1>(
        json!({
            "schema_version": schema_version,
            "request_id": "m6-initiate-request-001",
            "offer_id": "m6-zec-offer-001",
            "route": route,
            "maker_identity": maker_identity,
            "signed_envelope_sha256": commitment,
            "foreign_units": 200_000_000_u64,
            "expected_lez_units": 1_820_u128,
        }),
        accepted,
        TakerSwapInitiateRequestV1::validate_schema_version,
    );
    assert_request_schema_version::<TakerSwapMonitorRequestV1>(
        json!({"schema_version": schema_version, "swap_id": "m6-zec-swap-001"}),
        accepted,
        TakerSwapMonitorRequestV1::validate_schema_version,
    );
    assert_request_schema_version::<TakerClaimRequestV1>(
        json!({
            "schema_version": schema_version,
            "request_id": "m6-claim-request-001",
            "swap_id": "m6-zec-swap-001",
            "expected_generation": 4,
        }),
        accepted,
        TakerClaimRequestV1::validate_schema_version,
    );
    assert_request_schema_version::<TakerRefundRequestV1>(
        json!({
            "schema_version": schema_version,
            "request_id": "m6-refund-request-001",
            "swap_id": "m6-zec-swap-001",
            "expected_generation": 4,
        }),
        accepted,
        TakerRefundRequestV1::validate_schema_version,
    );
}

fn assert_request_schema_version<T: DeserializeOwned>(
    value: Value,
    accepted: bool,
    validate: fn(&T) -> Result<(), lez_maker_node::TakerFacadeSchemaVersionError>,
) {
    let schema_version = u16::try_from(value["schema_version"].as_u64().unwrap()).unwrap();
    let request: T = serde_json::from_value(value).unwrap();
    let result = validate(&request);
    if accepted {
        result.unwrap();
    } else {
        let error = result.unwrap_err();
        assert_eq!(error.actual(), schema_version);
        assert_eq!(
            error.expected(),
            lez_maker_node::TAKER_FACADE_SCHEMA_VERSION_V1
        );
    }
}

fn reject_unknown<T: DeserializeOwned>(mut value: Value) {
    assert!(serde_json::from_value::<T>(value.clone()).is_ok());
    value["unexpected"] = json!(true);
    assert!(serde_json::from_value::<T>(value).is_err());
}

fn assert_capability(
    capability: &TakerPairCapabilityV1,
    pair: Pair,
    direction: SwapDirection,
    action: TakerTerminalActionCapabilityV1,
) {
    assert_eq!(capability.pair(), pair);
    assert_eq!(capability.supported_direction(), direction);
    assert!(capability.authenticated_offer_browsing());
    assert_eq!(
        capability.initiation(),
        TakerInitiationCapabilityV1::PreparedPrivateMaterial
    );
    assert_eq!(
        capability.monitoring(),
        TakerMonitoringCapabilityV1::ReceiptBound
    );
    assert_eq!(capability.claim(), action);
    assert_eq!(capability.refund(), action);
}

fn assert_secret_and_authority_free(value: &Value) {
    match value {
        Value::Object(fields) => {
            for (name, value) in fields {
                assert!(
                    ![
                        "path",
                        "file",
                        "socket",
                        "endpoint",
                        "credential",
                        "secret",
                        "argv",
                        "command",
                        "raw_wire",
                        "receipt",
                        "evidence",
                    ]
                    .iter()
                    .any(|forbidden| name.contains(forbidden)),
                    "authority-bearing field escaped the facade: {name}"
                );
                assert_secret_and_authority_free(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                assert_secret_and_authority_free(value);
            }
        }
        _ => {}
    }
}
