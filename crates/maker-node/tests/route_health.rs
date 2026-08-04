//! Route-scoped dependency-health contract for the headless Maker.

use std::time::{SystemTime, UNIX_EPOCH};
use std::{
    fs,
    os::unix::fs::PermissionsExt as _,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
};

use lez_bridge_protocol::RequestId;
use lez_maker_node::{
    ListRequest, MakerDependencyStateV1, MakerRouteHealthProbe, MakerRpc, OfferPublishRequest,
    PriceQuoteRequest, rpc_module,
};
use lez_swap_core::{Pair, SwapDirection};
use lez_swap_store::{
    LocalPriceV1, MakerLocalRouteCommit, MakerOfferCommit, MakerOfferId, MakerOfferRecordV1,
    MakerOfferStatus, MakerPairConfigurationV1, MakerPriceSourceKind, MakerRouteV1,
    SqliteSwapStore,
};
use rustix::process::{Pid, Signal, kill_process};
use sha2::{Digest as _, Sha256};
use tempfile::tempdir;

#[derive(Debug, Default)]
struct MutableRouteHealth {
    unavailable: Mutex<Vec<MakerRouteV1>>,
}

impl MutableRouteHealth {
    fn set_unavailable(&self, route: MakerRouteV1) {
        self.unavailable.lock().expect("health lock").push(route);
    }
}

impl MakerRouteHealthProbe for MutableRouteHealth {
    fn state(&self, route: MakerRouteV1) -> MakerDependencyStateV1 {
        if self
            .unavailable
            .lock()
            .expect("health lock")
            .contains(&route)
        {
            MakerDependencyStateV1::Unavailable
        } else {
            MakerDependencyStateV1::Available
        }
    }
}

fn route(pair: Pair, direction: SwapDirection) -> MakerRouteV1 {
    MakerRouteV1::new(pair, direction).expect("supported route")
}

async fn configure(module: &jsonrpsee::RpcModule<MakerRpc>, route: MakerRouteV1, suffix: &str) {
    let request = lez_maker_node::LocalRouteSaveRequest {
        request_id: RequestId::new(format!("m7-health-config-{suffix}")).unwrap(),
        expected_pair_revision: None,
        expected_price_revision: None,
        configuration: MakerPairConfigurationV1::new(
            route,
            true,
            MakerPriceSourceKind::Local,
            1,
            1_000,
            300,
        )
        .unwrap(),
        price: LocalPriceV1::new(route, 5, 1).unwrap(),
    };
    let _: MakerLocalRouteCommit = module
        .call("maker_local_route_save_v1", [request])
        .await
        .expect("configure route");
}

async fn publish(
    module: &jsonrpsee::RpcModule<MakerRpc>,
    route: MakerRouteV1,
    suffix: &str,
) -> MakerOfferId {
    let offer_id = MakerOfferId::new(format!("m7-health-offer-{suffix}")).unwrap();
    let request = OfferPublishRequest {
        request_id: RequestId::new(format!("m7-health-publish-{suffix}")).unwrap(),
        offer_id: offer_id.clone(),
        route,
    };
    let _: MakerOfferCommit = module
        .call("maker_offer_publish", [request])
        .await
        .expect("publish offer");
    offer_id
}

#[tokio::test]
async fn unhealthy_route_is_fail_closed_and_reconciliation_is_pair_scoped() {
    let run = tempdir().expect("isolated store");
    let database = run.path().join("maker.sqlite3");
    let store = SqliteSwapStore::open(&database).expect("open store");
    let health = Arc::new(MutableRouteHealth::default());
    let context = MakerRpc::new(store).with_route_health_probe(health.clone());
    let module = rpc_module(context).expect("build owner RPC");
    let zec = route(Pair::Zcash, SwapDirection::TakerSellsLez);
    let btc = route(Pair::Bitcoin, SwapDirection::TakerSellsForeign);
    configure(&module, zec, "zec").await;
    configure(&module, btc, "btc").await;
    let zec_offer = publish(&module, zec, "zec").await;
    let reserved_zec_offer = publish(&module, zec, "zec-reserved").await;
    let btc_offer = publish(&module, btc, "btc").await;
    let reservation_id = RequestId::new("m7-health-reservation-zec").unwrap();
    let mut second_connection = SqliteSwapStore::open(&database).expect("second store connection");
    second_connection
        .reserve_maker_offer(
            &RequestId::new("m7-health-reserve-zec").unwrap(),
            &reserved_zec_offer,
            1,
            &reservation_id,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        )
        .expect("reserve one ZEC offer before dependency loss");
    drop(second_connection);

    health.set_unavailable(zec);
    let report: lez_maker_node::MakerHealthV1 = module
        .call("maker_health", [ListRequest::default()])
        .await
        .expect("reconcile route health");
    assert!(report.is_degraded());
    assert_eq!(report.route_state(zec), MakerDependencyStateV1::Unavailable);
    assert_eq!(report.route_state(btc), MakerDependencyStateV1::Available);

    let offers: Vec<MakerOfferRecordV1> = module
        .call("maker_offer_list", [ListRequest::default()])
        .await
        .expect("list reconciled offers");
    let zec_record = offers
        .iter()
        .find(|record| record.offer().id() == &zec_offer)
        .expect("ZEC offer retained in history");
    let btc_record = offers
        .iter()
        .find(|record| record.offer().id() == &btc_offer)
        .expect("BTC offer retained in history");
    let reserved_record = offers
        .iter()
        .find(|record| record.offer().id() == &reserved_zec_offer)
        .expect("reserved ZEC offer retained in history");
    assert_eq!(zec_record.status(), MakerOfferStatus::Withdrawn);
    assert_eq!(btc_record.status(), MakerOfferStatus::Active);
    assert_eq!(reserved_record.status(), MakerOfferStatus::Reserved);
    assert_eq!(reserved_record.reservation_id(), Some(&reservation_id));

    let quote_error = module
        .call::<_, serde_json::Value>("maker_price_quote", [PriceQuoteRequest { route: zec }])
        .await
        .expect_err("unhealthy route quote must fail closed");
    assert!(
        quote_error
            .to_string()
            .contains("chain dependency is unavailable")
    );

    let healthy_quote: lez_maker_node::PriceQuoteV1 = module
        .call("maker_price_quote", [PriceQuoteRequest { route: btc }])
        .await
        .expect("healthy route quote remains available");
    assert_eq!(healthy_quote.price().route(), btc);

    let rejected = OfferPublishRequest {
        request_id: RequestId::new("m7-health-publish-zec-after-loss").unwrap(),
        offer_id: MakerOfferId::new("m7-health-offer-zec-after-loss").unwrap(),
        route: zec,
    };
    let publish_error = module
        .call::<_, MakerOfferCommit>("maker_offer_publish", [rejected])
        .await
        .expect_err("unhealthy route publication must fail closed");
    assert!(
        publish_error
            .to_string()
            .contains("chain dependency is unavailable")
    );
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_ready(child: &mut Child, ready: &std::path::Path, socket: &std::path::Path) {
    for _ in 0..200 {
        assert_eq!(child.try_wait().unwrap(), None, "Maker daemon exited early");
        if ready.is_file() && socket.exists() {
            return;
        }
        thread::sleep(std::time::Duration::from_millis(25));
    }
    panic!("Maker daemon did not become ready");
}

#[tokio::test]
async fn daemon_periodically_withdraws_without_a_health_request() {
    let run = tempdir().expect("isolated process run");
    fs::set_permissions(run.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let worker = run.path().join("semantic-health");
    let dependency = run.path().join("zcash.available");
    fs::write(&worker, b"#!/bin/sh\n[ -f \"$1\" ]\n").unwrap();
    fs::set_permissions(&worker, fs::Permissions::from_mode(0o500)).unwrap();
    fs::write(&dependency, b"available").unwrap();
    let worker_sha256: [u8; 32] = Sha256::digest(fs::read(&worker).unwrap()).into();
    let health_config = run.path().join("route-health.json");
    fs::write(
        &health_config,
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "commands": [{
                "route": {"pair": "Zcash", "direction": "TakerSellsLez"},
                "program": worker,
                "program_sha256": hex::encode(worker_sha256),
                "args": [dependency],
                "timeout_milliseconds": 100
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    fs::set_permissions(&health_config, fs::Permissions::from_mode(0o600)).unwrap();
    let socket = run.path().join("maker.sock");
    let ready = run.path().join("ready");
    let database = run.path().join("maker.sqlite3");
    let child = Command::new(env!("CARGO_BIN_EXE_lez-maker-daemon"))
        .arg("--socket")
        .arg(&socket)
        .arg("--database")
        .arg(&database)
        .arg("--ready-file")
        .arg(&ready)
        .arg("--route-health-config")
        .arg(&health_config)
        .arg("--route-health-poll-milliseconds")
        .arg("100")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start Maker daemon");
    let mut daemon = ChildGuard(child);
    wait_ready(&mut daemon.0, &ready, &socket);

    let zec = route(Pair::Zcash, SwapDirection::TakerSellsLez);
    let save = lez_maker_node::LocalRouteSaveRequest {
        request_id: RequestId::new("m7-process-health-config-zec").unwrap(),
        expected_pair_revision: None,
        expected_price_revision: None,
        configuration: MakerPairConfigurationV1::new(
            zec,
            true,
            MakerPriceSourceKind::Local,
            1,
            1_000,
            300,
        )
        .unwrap(),
        price: LocalPriceV1::new(zec, 5, 1).unwrap(),
    };
    let _: MakerLocalRouteCommit =
        lez_maker_node::call_local_rpc(&socket, "maker_local_route_save_v1", &save)
            .await
            .expect("configure healthy route");
    let offer_id = MakerOfferId::new("m7-process-health-offer-zec").unwrap();
    let publish = OfferPublishRequest {
        request_id: RequestId::new("m7-process-health-publish-zec").unwrap(),
        offer_id: offer_id.clone(),
        route: zec,
    };
    let _: MakerOfferCommit =
        lez_maker_node::call_local_rpc(&socket, "maker_offer_publish", &publish)
            .await
            .expect("publish while semantic checks pass");

    fs::remove_file(&dependency).expect("stop the selected route dependency");
    let mut withdrawn = false;
    for _ in 0..100 {
        let offers: Vec<MakerOfferRecordV1> =
            lez_maker_node::call_local_rpc(&socket, "maker_offer_list", &ListRequest::default())
                .await
                .expect("query offer history without invoking health");
        withdrawn = offers.iter().any(|record| {
            record.offer().id() == &offer_id && record.status() == MakerOfferStatus::Withdrawn
        });
        if withdrawn {
            break;
        }
        thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(withdrawn, "periodic daemon loop did not withdraw the offer");

    let health: lez_maker_node::MakerHealthV1 =
        lez_maker_node::call_local_rpc(&socket, "maker_health", &ListRequest::default())
            .await
            .expect("read degraded state after automatic withdrawal");
    assert_eq!(health.route_state(zec), MakerDependencyStateV1::Unavailable);
    kill_process(Pid::from_child(&daemon.0), Signal::TERM).expect("stop Maker daemon");
    assert!(daemon.0.wait().unwrap().success());
}
