//! Production exact-outpoint Zcash funding planner contract.

#![forbid(unsafe_code)]

use lez_zebra_node_adapter::{
    ExactOutpointZcashFundingPlanner, ExactOutpointZcashFundingPlannerError, ZebraFundingSigner,
};

#[test]
fn production_funding_planner_api_is_public() {
    fn type_is_public<T>() {}
    fn accepts_signer<T: ZebraFundingSigner>() {}

    type_is_public::<ExactOutpointZcashFundingPlanner<(), ()>>();
    type_is_public::<ExactOutpointZcashFundingPlannerError<std::io::Error, std::io::Error>>();
    let _ = accepts_signer::<lez_zebra_node_adapter::RoleKeyedZcashSigner>;
}
