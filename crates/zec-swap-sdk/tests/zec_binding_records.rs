use lez_zec_swap_sdk::{
    Bip199Contract, ExpectedBip199Output, ZecProfileId, ZecSwapBinding, ZecSwapBindingRecordV1,
};
use zcash_protocol::{
    consensus::{BranchId, NetworkType},
    value::Zatoshis,
};

fn local_binding() -> ZecSwapBinding {
    ZecSwapBinding::new(
        ZecProfileId::DeterministicLocalV1,
        ExpectedBip199Output::new(
            NetworkType::Regtest,
            BranchId::Nu6_2,
            Zatoshis::from_u64(100_000).unwrap(),
            Bip199Contract::new(500_000, [0x11; 20], [0x22; 32], [0x33; 20]),
        ),
    )
    .unwrap()
}

#[test]
fn version_one_binding_record_roundtrips_only_after_full_revalidation() {
    let binding = local_binding();
    let record = ZecSwapBindingRecordV1::from_binding(&binding);
    let json = serde_json::to_string(&record).unwrap();
    let decoded: ZecSwapBindingRecordV1 = serde_json::from_str(&json).unwrap();

    assert_eq!(decoded.validate().unwrap(), binding);
}

#[test]
fn binding_record_rejects_profile_network_and_derived_script_mismatch() {
    let binding = local_binding();
    let record = ZecSwapBindingRecordV1::from_binding(&binding);

    let mut wrong_profile = serde_json::to_value(&record).unwrap();
    wrong_profile["profile_id"] = serde_json::json!("public_testnet_v1");
    let wrong_profile: ZecSwapBindingRecordV1 = serde_json::from_value(wrong_profile).unwrap();
    assert!(wrong_profile.validate().is_err());

    let mut wrong_redeem = serde_json::to_value(&record).unwrap();
    wrong_redeem["expected_output"]["redeem_script"][0] = serde_json::json!(0xff);
    let wrong_redeem: ZecSwapBindingRecordV1 = serde_json::from_value(wrong_redeem).unwrap();
    assert!(wrong_redeem.validate().is_err());
}
