//! Public production Zebra Zcash port composition contract.

#![forbid(unsafe_code)]

use lez_zebra_node_adapter::{ZebraRpcZcashPort, ZebraRpcZcashPortConfigError};
use lez_zec_swap_sdk::{
    ZcashClaimPort, ZcashFirstLockPort, ZcashMakerLockObservationPort, ZcashRefundPort,
    ZcashTakerFirstLockObservationPort,
};

#[test]
fn production_composite_api_is_public_and_covers_every_sdk_zcash_port() {
    fn type_is_public<T>() {}
    fn all_zcash_ports<T>()
    where
        T: ZcashFirstLockPort
            + ZcashTakerFirstLockObservationPort
            + ZcashMakerLockObservationPort
            + ZcashClaimPort
            + ZcashRefundPort,
    {
    }

    type_is_public::<ZebraRpcZcashPort<(), ()>>();
    type_is_public::<ZebraRpcZcashPortConfigError>();
    let _ = all_zcash_ports::<
        ZebraRpcZcashPort<
            lez_zebra_node_adapter::HttpZebraRpc,
            lez_zebra_node_adapter::RoleKeyedZcashSigner,
        >,
    >;
}
