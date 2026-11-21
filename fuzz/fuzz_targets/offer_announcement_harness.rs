use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use lez_node_common::{
    LogosChatGateway, LogosChatGatewayRoleV1, LogosChatGatewayStatusRequestV1,
    LogosChatGatewayStatusV1, LogosOfferIngestRequestV1, verify_logos_offer_announcement,
};
use lez_swap_store::MakerOfferV1;

/// Trusted local time at which the checked-in announcement seeds are live.
const NOW: u64 = 1_794_000_000;

/// Replays one fuzz input as the bytes a Delivery stranger publishes to a Taker: a signed offer
/// announcement, and the bare offer JSON that the announcement's nested envelope carries. The
/// announcing key is the stranger's own, so every check behind its signature is reachable too.
pub fn run(data: &[u8]) {
    let gateway = LogosChatGateway::new_with_clock(LogosChatGatewayRoleV1::Taker, None, || Ok(NOW))
        .expect("fixed Taker gateway must construct");
    let request = LogosOfferIngestRequestV1 {
        schema_version: 1,
        payload_base64: BASE64_STANDARD.encode(data).into_boxed_str(),
    };
    let before = status(&gateway);
    match gateway.ingest_offer_announcement(&request) {
        Ok(ack) => {
            assert!(!ack.was_replay, "a fresh index replayed an announcement");
            assert_eq!(status(&gateway).discovered_offers, 1);
            let again = gateway
                .ingest_offer_announcement(&request)
                .expect("an accepted announcement must be accepted again");
            assert!(again.was_replay, "the same announcement was indexed twice");
            assert_eq!(status(&gateway).discovered_offers, 1);
            let verified = verify_logos_offer_announcement(data, NOW)
                .expect("an indexed announcement must verify on its own");
            assert_eq!(
                verified.encoded(),
                data,
                "verified announcement is not the input"
            );
            check_offer(verified.offer().offer());
        }
        Err(_) => assert_eq!(status(&gateway), before, "rejected announcement left state"),
    }

    if let Ok(offer) = serde_json::from_slice::<MakerOfferV1>(data)
        && offer.validate().is_ok()
    {
        check_offer(&offer);
    }
}

/// A validated offer re-encodes to an equal offer and quotes every bound without panicking.
fn check_offer(offer: &MakerOfferV1) {
    offer.validate().expect("checked offer must validate");
    let encoded = serde_json::to_vec(offer).expect("validated offer must serialize");
    let decoded: MakerOfferV1 =
        serde_json::from_slice(&encoded).expect("validated offer must round-trip");
    assert_eq!(&decoded, offer, "offer changed across a JSON round trip");
    let minimum = offer.minimum_foreign_units();
    let maximum = offer.maximum_foreign_units();
    for units in [
        0,
        minimum.saturating_sub(1),
        minimum,
        minimum / 2 + maximum / 2,
        maximum,
        maximum.saturating_add(1),
        u64::MAX,
    ] {
        let _ = offer.quote_foreign_amount(units);
    }
}

fn status(gateway: &LogosChatGateway) -> LogosChatGatewayStatusV1 {
    gateway
        .status(LogosChatGatewayStatusRequestV1 { schema_version: 1 })
        .expect("gateway status must be readable")
}
