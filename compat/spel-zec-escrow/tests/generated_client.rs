use lez_zec_escrow_compat::PROGRAM_IDL_JSON;

#[test]
fn official_spel_client_matches_checked_in_golden() {
    insta::assert_snapshot!("zec_escrow_client", generated_client());
}

#[test]
fn generated_claim_hashlock_requires_claimant_signature() {
    let generated = generated_client();
    let claim = generated_method(&generated, "claim_hashlock", Some("refund"));
    assert!(
        claim.contains(
            "let signer_ids: Vec<AccountId> = vec![\n            accounts.claimant,\n        ];"
        ),
        "claim_hashlock must be signed by the claimant actor"
    );
}

#[test]
fn generated_refund_requires_depositor_signature() {
    let generated = generated_client();
    let refund = generated_method(&generated, "refund", None);
    assert!(
        refund.contains(
            "let signer_ids: Vec<AccountId> = vec![\n            accounts.depositor,\n        ];"
        ),
        "refund must be signed by the original depositor actor"
    );
}

fn generated_client() -> String {
    spel_client_gen::generate_from_idl_json(PROGRAM_IDL_JSON)
        .expect("the SPEL-generated escrow IDL must generate a typed client")
        .client_code
}

fn generated_method<'a>(client: &'a str, name: &str, next_name: Option<&str>) -> &'a str {
    let marker = format!("    pub async fn {name}(");
    let start = client
        .find(&marker)
        .unwrap_or_else(|| panic!("generated client is missing {name}"));
    let method_and_tail = &client[start..];

    let Some(next_name) = next_name else {
        return method_and_tail;
    };
    let next_marker = format!("    pub async fn {next_name}(");
    let end = method_and_tail
        .find(&next_marker)
        .unwrap_or_else(|| panic!("generated client is missing {next_name}"));
    &method_and_tail[..end]
}
