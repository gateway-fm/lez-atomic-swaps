use lez_swap_core::ClaimEvidence;

#[test]
fn serialized_claim_evidence_never_contains_the_plaintext_preimage() {
    let preimage = [0x5a; 32];
    let encoded = serde_json::to_value(ClaimEvidence::new(preimage)).expect("evidence serializes");
    let plaintext = serde_json::to_value(preimage).expect("preimage serializes");

    assert_ne!(
        encoded, plaintext,
        "durable evidence must be a one-way marker"
    );
    assert!(
        serde_json::from_value::<ClaimEvidence>(plaintext).is_err(),
        "legacy untagged plaintext must fail closed instead of becoming a marker"
    );
}
