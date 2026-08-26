//! Signed Delivery offer-index and concurrent Chat-session contracts.

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use lez_bridge_protocol::RequestId;
use lez_maker_node::{
    LogosChatGateway, LogosChatGatewayBindRequestV1, LogosChatGatewayError, LogosChatGatewayRoleV1,
    LogosChatGatewayStatusRequestV1, LogosOfferAnnouncementSnapshotRequestV1,
    LogosOfferAnnouncementSnapshotV1, LogosOfferIngestRequestV1, LogosOfferListRequestV1,
    LogosOfferSelectRequestV1, MakerRpc, RunLocalDelivery, TakerMakerIdentityV1, rpc_module,
};
use lez_swap_core::{Pair, SwapDirection};
use lez_swap_store::{
    LocalPriceV1, MakerOfferId, MakerPairConfigurationV1, MakerPriceSourceKind, MakerRouteV1,
    SqliteSwapStore,
};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use tempfile::{TempDir, tempdir};

fn request(value: &str) -> RequestId {
    RequestId::new(value).unwrap()
}

fn signing_key(byte: u8) -> SecretKey {
    SecretKey::from_slice(&[byte; 32]).unwrap()
}

fn configured_offer(now: u64) -> (TempDir, SqliteSwapStore, MakerOfferId) {
    let run = tempdir().unwrap();
    let mut store = SqliteSwapStore::open(run.path().join("offers.sqlite3")).unwrap();
    let route = MakerRouteV1::new(Pair::Zcash, SwapDirection::TakerSellsLez).unwrap();
    let disabled =
        MakerPairConfigurationV1::new(route, false, MakerPriceSourceKind::Local, 10, 10_000, 300)
            .unwrap();
    store
        .configure_maker_pair(&request("logos-index-pair-create"), None, &disabled)
        .unwrap();
    store
        .set_local_price(
            &request("logos-index-price-create"),
            None,
            &LocalPriceV1::new(route, 5, 2).unwrap(),
        )
        .unwrap();
    let enabled =
        MakerPairConfigurationV1::new(route, true, MakerPriceSourceKind::Local, 10, 10_000, 300)
            .unwrap();
    store
        .configure_maker_pair(&request("logos-index-pair-enable"), Some(1), &enabled)
        .unwrap();
    let offer_id = MakerOfferId::new("logos-index-offer-001").unwrap();
    store
        .publish_local_offer(&request("logos-index-publish"), &offer_id, route, now)
        .unwrap();
    (run, store, offer_id)
}

#[test]
fn signed_rebroadcast_is_active_until_the_one_winner_state_advances() {
    let now = 2_000_000_000;
    let (run, mut store, offer_id) = configured_offer(now);
    let key = signing_key(41);
    let maker_identity = PublicKey::from_secret_key(&Secp256k1::signing_only(), &key).serialize();
    let publisher = RunLocalDelivery::publisher(run.path().join("delivery"), key).unwrap();
    let active = store.list_maker_offer_history(now).unwrap().remove(0);
    let active_wire = publisher
        .sign_logos_offer_announcement(&active, "logos://maker-live", now)
        .unwrap();
    let gateway =
        LogosChatGateway::new_with_clock(LogosChatGatewayRoleV1::Taker, None, move || Ok(now))
            .unwrap();
    let first = gateway
        .ingest_offer_announcement(&LogosOfferIngestRequestV1 {
            schema_version: 1,
            payload_base64: BASE64_STANDARD.encode(&active_wire).into(),
        })
        .unwrap();
    assert!(!first.was_replay);

    let listed = gateway
        .list_offer_announcements(LogosOfferListRequestV1 {
            schema_version: 1,
            route: Some(active.offer().route()),
        })
        .unwrap();
    assert_eq!(listed.offers.len(), 1);
    assert_eq!(
        listed.offers[0].maker_chat_address.as_ref(),
        "logos://maker-live"
    );
    gateway
        .select_offer_announcement(&LogosOfferSelectRequestV1 {
            schema_version: 1,
            maker_identity: TakerMakerIdentityV1::new(maker_identity).unwrap(),
            offer_id: offer_id.clone(),
        })
        .unwrap();

    store
        .reserve_maker_offer(
            &request("logos-index-reserve"),
            &offer_id,
            1,
            &request("logos-index-winning-negotiation"),
            now + 1,
        )
        .unwrap();
    let reserved = store.list_maker_offer_history(now + 1).unwrap().remove(0);
    let reserved_wire = publisher
        .sign_logos_offer_announcement(&reserved, "logos://maker-live", now + 1)
        .unwrap();
    gateway
        .ingest_offer_announcement(&LogosOfferIngestRequestV1 {
            schema_version: 1,
            payload_base64: BASE64_STANDARD.encode(reserved_wire).into(),
        })
        .unwrap();

    let after = gateway
        .list_offer_announcements(LogosOfferListRequestV1 {
            schema_version: 1,
            route: None,
        })
        .unwrap();
    assert!(after.offers.is_empty());
    assert_eq!(after.unavailable_offers, 1);
    assert!(matches!(
        gateway.select_offer_announcement(&LogosOfferSelectRequestV1 {
            schema_version: 1,
            maker_identity: TakerMakerIdentityV1::new(maker_identity).unwrap(),
            offer_id,
        }),
        Err(LogosChatGatewayError::SessionUnavailable)
    ));
    assert!(
        gateway
            .ingest_offer_announcement(&LogosOfferIngestRequestV1 {
                schema_version: 1,
                payload_base64: BASE64_STANDARD.encode(active_wire).into(),
            })
            .unwrap()
            .was_replay,
        "late active rebroadcast cannot resurrect a reserved offer"
    );
}

#[test]
fn maker_gateway_binds_multiple_takers_but_never_aliases_a_peer() {
    let run = tempdir().unwrap();
    let gateway = LogosChatGateway::new(
        LogosChatGatewayRoleV1::Maker,
        Some(run.path().join("maker-chat.sock")),
    )
    .unwrap();
    for marker in ["a", "b"] {
        assert!(
            !gateway
                .bind_session(&LogosChatGatewayBindRequestV1 {
                    schema_version: 1,
                    conversation_id: format!("conversation-{marker}").into(),
                    local_address: "logos://maker".into(),
                    peer_address: format!("logos://taker-{marker}").into(),
                })
                .unwrap()
                .was_replay
        );
    }
    let status = gateway
        .status(LogosChatGatewayStatusRequestV1 { schema_version: 1 })
        .unwrap();
    assert_eq!(status.session_count, 2);
    assert!(matches!(
        gateway.bind_session(&LogosChatGatewayBindRequestV1 {
            schema_version: 1,
            conversation_id: "conversation-c".into(),
            local_address: "logos://maker".into(),
            peer_address: "logos://taker-a".into(),
        }),
        Err(LogosChatGatewayError::SessionConflict)
    ));
}

#[tokio::test]
async fn owner_rpc_snapshot_is_exactly_what_the_taker_gateway_indexes() {
    let now = 2_000_000_000;
    let (run, mut store, _offer_id) = configured_offer(now);
    let route = MakerRouteV1::new(Pair::Zcash, SwapDirection::TakerSellsLez).unwrap();
    for index in 2..=140 {
        store
            .publish_local_offer(
                &request(&format!("logos-index-publish-{index:03}")),
                &MakerOfferId::new(format!("logos-index-offer-{index:03}")).unwrap(),
                route,
                now,
            )
            .unwrap();
    }
    let delivery =
        RunLocalDelivery::publisher(run.path().join("delivery-rpc"), signing_key(51)).unwrap();
    let module = rpc_module(
        MakerRpc::with_delivery_transport(store, delivery, signing_key(52))
            .with_offer_snapshot_clock(move || Ok(now)),
    )
    .unwrap();
    let mut after_offer_id = None;
    let mut announcements = Vec::new();
    let mut page_count = 0;
    loop {
        let snapshot: LogosOfferAnnouncementSnapshotV1 = module
            .call(
                "maker_offer_announcement_snapshot_v1",
                [LogosOfferAnnouncementSnapshotRequestV1 {
                    schema_version: 1,
                    maker_chat_address: "logos://maker-rpc-live".into(),
                    after_offer_id,
                }],
            )
            .await
            .unwrap();
        assert!(serde_json::to_vec(&snapshot).unwrap().len() < 64 * 1024);
        page_count += 1;
        announcements.extend(snapshot.announcements_base64);
        after_offer_id = snapshot.next_after_offer_id;
        if after_offer_id.is_none() {
            break;
        }
    }
    assert!(page_count > 1);
    assert_eq!(announcements.len(), 140);
    let gateway =
        LogosChatGateway::new_with_clock(LogosChatGatewayRoleV1::Taker, None, move || Ok(now))
            .unwrap();
    for announcement in announcements.iter().take(24) {
        gateway
            .ingest_offer_announcement(&LogosOfferIngestRequestV1 {
                schema_version: 1,
                payload_base64: announcement.clone(),
            })
            .unwrap();
    }
    let list = gateway
        .list_offer_announcements(LogosOfferListRequestV1 {
            schema_version: 1,
            route: None,
        })
        .unwrap();
    assert_eq!(list.offers.len(), 16);
    assert_eq!(list.omitted_offers, 8);
    assert!(serde_json::to_vec(&list).unwrap().len() < 4 * 1024 * 1024);
    assert_eq!(
        list.offers[0].maker_chat_address.as_ref(),
        "logos://maker-rpc-live"
    );
}

#[tokio::test]
async fn owner_rpc_snapshot_skips_future_active_rows_without_aborting() {
    let now = 2_000_000_000;
    let (run, store, _offer_id) = configured_offer(now + 1);
    let delivery =
        RunLocalDelivery::publisher(run.path().join("delivery-skew"), signing_key(61)).unwrap();
    let module = rpc_module(
        MakerRpc::with_delivery_transport(store, delivery, signing_key(62))
            .with_offer_snapshot_clock(move || Ok(now)),
    )
    .unwrap();
    let snapshot: LogosOfferAnnouncementSnapshotV1 = module
        .call(
            "maker_offer_announcement_snapshot_v1",
            [LogosOfferAnnouncementSnapshotRequestV1 {
                schema_version: 1,
                maker_chat_address: "logos://maker-skewed-row".into(),
                after_offer_id: None,
            }],
        )
        .await
        .unwrap();
    assert!(snapshot.announcements_base64.is_empty());
    assert!(snapshot.next_after_offer_id.is_none());
}
