use lez_zec_escrow_compat::PROGRAM_IDL_JSON;

#[test]
fn official_spel_client_matches_checked_in_golden() {
    insta::assert_snapshot!("zec_escrow_client", generated_client());
}

#[test]
fn generated_token_methods_sign_with_owners_never_atas() {
    let generated = generated_client();
    let funding = generated_method(&generated, "fund_token", Some("claim_token"));
    assert!(
        funding.contains(
            "let signer_ids: Vec<AccountId> = vec![\n            accounts.depositor_owner,\n        ];"
        ),
        "token funding must be signed by the depositor owner"
    );
    assert!(!funding.contains("accounts.depositor_asset,\n        ];\n        let nonces"));

    let claim = generated_method(&generated, "claim_token", Some("refund_token"));
    assert!(
        claim.contains(
            "let signer_ids: Vec<AccountId> = vec![\n            accounts.claimant_owner,\n        ];"
        ),
        "token claim must be signed by the claimant owner"
    );
    assert!(!claim.contains("accounts.claimant_asset,\n        ];\n        let nonces"));
}

#[test]
fn generated_refunds_and_custody_creation_are_permissionless() {
    let generated = generated_client();
    let empty_signers = "let signer_ids: Vec<AccountId> = vec![\n        ];";
    let create = generated_method(&generated, "create_token_custody", Some("fund_token"));
    let native_refund = generated_method(&generated, "refund_native", Some("initialize_token"));
    let token_refund = generated_method(&generated, "refund_token", None);

    assert!(create.contains(empty_signers));
    assert!(native_refund.contains(empty_signers));
    assert!(token_refund.contains(empty_signers));
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
