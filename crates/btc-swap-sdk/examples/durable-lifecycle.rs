//! Application-owned durable BTC/LEZ lifecycle wiring.
//!
//! Negotiation and secret-bearing signing preparation happen before this
//! boundary. A real application supplies a process-durable store plus Bitcoin
//! and LEZ adapters that observe before sending and persist their own exact
//! outbound bytes before any node call.

use lez_btc_swap_sdk::{
    AcceptedBtcAgreementV1, BitcoinBtcLifecyclePort, BtcLifecycleDriveOutcomeV1,
    BtcLifecycleRuntime, BtcLifecycleStore, BtcPairSdk, BtcPreparedProtocolV1, BtcSdkError,
    LezBtcLifecyclePort, StoredBtcLifecycleSdk,
};
use lez_swap_core::SwapId;

/// Persists revision zero before either chain adapter can be called.
#[allow(dead_code)]
async fn activate_role<Store>(
    pair: BtcPairSdk,
    store: Store,
    accepted: AcceptedBtcAgreementV1,
    prepared: BtcPreparedProtocolV1,
) -> Result<SwapId, BtcSdkError>
where
    Store: BtcLifecycleStore,
{
    let stored = StoredBtcLifecycleSdk::new(pair, store);
    let active = stored.activate(accepted, prepared).await?;
    Ok(active.status().swap_id().clone())
}

/// Replays durable state and advances until terminal or waiting on a chain.
///
/// Construct this runtime again after a process restart with the same fixed
/// role, store, and adapter configuration. `Pending` should be retried only
/// after an adapter notification or bounded backoff; it is never authority to
/// submit a second effect. Each `Transition` is clone-validated and committed
/// by exact compare-and-swap before the next action is selected.
#[allow(dead_code)]
async fn drive_until_blocked<Store, Bitcoin, Lez>(
    pair: BtcPairSdk,
    store: Store,
    bitcoin: Bitcoin,
    lez: Lez,
    swap_id: &SwapId,
) -> Result<BtcLifecycleDriveOutcomeV1, BtcSdkError>
where
    Store: BtcLifecycleStore,
    Bitcoin: BitcoinBtcLifecyclePort,
    Lez: LezBtcLifecyclePort,
{
    let runtime = BtcLifecycleRuntime::new(StoredBtcLifecycleSdk::new(pair, store), bitcoin, lez);
    loop {
        match runtime.drive_once(swap_id).await? {
            transition @ BtcLifecycleDriveOutcomeV1::Transition { .. } => {
                let _ = transition;
            }
            blocked_or_complete => return Ok(blocked_or_complete),
        }
    }
}

fn main() {
    println!("wire role-fixed durable store and Bitcoin/LEZ ports; see this example's helpers");
}
