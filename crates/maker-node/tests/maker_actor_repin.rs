//! `maker_actor_repin_v1` re-pins a swap to the actor program of its own kind.
//!
//! Devnet finding (M2 step 5): the method took the Bitcoin actor's SHA-256 for
//! every swap, so a ZEC swap stranded by a Node image upgrade
//! (`actor_deployment_invalid`) could not be re-pinned at all. The program is
//! now chosen by the process's actor kind; a kind this Node does not run is
//! refused by name instead of silently moving the pin to the wrong binary.

use lez_maker_node::{MakerRpc, rpc_module};
use lez_swap_core::{
    Chain, ChainPosition, ConfirmationPolicy, Pair, RecoverySchedule, SwapCoordinator,
    SwapDirection, SwapId, TimelockSafety,
};
use lez_swap_store::{MakerActorKindV1, MakerActorManifestV1, SqliteSwapStore};
use serde_json::{Value, json};
use tempfile::tempdir;

const NOT_FOUND: i64 = -32_004;

fn zec_swap(id: &str) -> SwapCoordinator {
    SwapCoordinator::new_with_direction(
        SwapId::new(id).unwrap(),
        Pair::Zcash,
        SwapDirection::TakerSellsForeign,
        ConfirmationPolicy::new(2).unwrap(),
        RecoverySchedule::new(
            Pair::Zcash,
            SwapDirection::TakerSellsForeign,
            ChainPosition::block_height(Chain::Lez, 100),
            ChainPosition::block_height(Chain::Zcash, 120),
            TimelockSafety::between(Chain::Lez, Chain::Zcash, 1_000, 1_200, 100).unwrap(),
        )
        .unwrap(),
    )
}

async fn repin_error(module: &jsonrpsee::RpcModule<MakerRpc>, swap: &str) -> (i64, String) {
    let (response, _) = module
        .raw_json_request(
            &json!({
                "jsonrpc": "2.0", "id": 1, "method": "maker_actor_repin_v1",
                "params": [{"request_id": format!("repin-{swap}"), "id": swap, "expected_generation": 0}],
            })
            .to_string(),
            1,
        )
        .await
        .unwrap();
    let response: Value = serde_json::from_str(response.get()).unwrap();
    let error = &response["error"];
    (
        error["code"]
            .as_i64()
            .unwrap_or_else(|| panic!("expected an error: {response}")),
        error["message"].as_str().unwrap_or_default().to_owned(),
    )
}

#[tokio::test]
async fn repin_picks_the_program_by_the_swap_actor_kind() {
    let run = tempdir().unwrap();
    let mut store = SqliteSwapStore::open(run.path().join("maker.sqlite3")).unwrap();
    store.save(&zec_swap("zec-repin-001")).unwrap();
    store
        .register_maker_actor(
            &MakerActorManifestV1::new(
                SwapId::new("zec-repin-001").unwrap(),
                MakerActorKindV1::Zcash,
                run.path().join("zec-repin-001-actor.json"),
                [7; 32],
                "/usr/bin/true".into(),
                [8; 32],
                run.path().join("zec-repin-001-actor.sqlite3"),
            )
            .unwrap(),
            10,
        )
        .unwrap();
    // This Node runs neither pair's actor: the refusal must name the swap's
    // own kind (before the fix every swap was answered with the Bitcoin one).
    let module = rpc_module(MakerRpc::new(store)).unwrap();
    let (code, message) = repin_error(&module, "zec-repin-001").await;
    assert_eq!(code, NOT_FOUND);
    assert!(message.contains("Zcash"), "{message}");
    assert!(!message.contains("Bitcoin"), "{message}");

    let (code, message) = repin_error(&module, "no-such-swap").await;
    assert_eq!(code, NOT_FOUND);
    assert!(message.contains("not found"), "{message}");
}
