use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=../lez-v0.2-provisional/escrow/src/lib.rs");
    let generated = spel_client_gen::generate_from_idl_json(lez_zec_escrow_v02::PROGRAM_IDL_JSON)
        .expect("the exact pinned SPEL generator must accept the v0.2 escrow IDL");
    assert_native_prepare_surface(&generated.client_code);
    let destination = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo provides OUT_DIR"))
        .join("zec_escrow_client.rs");
    fs::write(&destination, generated.client_code)
        .expect("write the generated v0.2 escrow client into OUT_DIR");
    let wrapper = destination.with_file_name("zec_escrow_client_module.rs");
    let destination_literal = destination
        .to_str()
        .expect("Cargo OUT_DIR must be valid UTF-8")
        .escape_default();
    let wrapper_source = format!(
        "#[allow(\n    clippy::too_many_arguments,\n    clippy::doc_markdown,\n    clippy::must_use_candidate,\n    clippy::uninlined_format_args,\n    dead_code,\n    unused_imports,\n    unused_mut\n)]\n#[path = \"{destination_literal}\"]\nmod exact_generated_client;\npub use exact_generated_client::*;\n"
    );
    fs::write(wrapper, wrapper_source).expect("write the generated-client module wrapper");
}

fn assert_native_prepare_surface(client: &str) {
    let initialize = generated_method(client, "initialize_native", "fund_native");
    let initialize_witnessed =
        generated_method(client, "initialize_native_witnessed", "fund_native");
    let funding = generated_method(client, "fund_native", "claim_native");
    let claim_witnessed = generated_method(client, "claim_native_witnessed", "refund_native");
    for expected in [
        "let mut account_ids: Vec<AccountId> = vec![\n            accounts.metadata,\n            accounts.custody,\n            accounts.depositor,\n            accounts.claimant,\n        ];",
        "let signer_ids: Vec<AccountId> = vec![\n            accounts.depositor,\n        ];",
    ] {
        assert!(
            initialize.contains(expected),
            "pinned generated initialize_native role/account order changed"
        );
    }
    for expected in [
        "let mut account_ids: Vec<AccountId> = vec![\n            accounts.metadata,\n            accounts.custody,\n            accounts.depositor,\n            accounts.claimant,\n            accounts.aggregate_authority,\n        ];",
        "let signer_ids: Vec<AccountId> = vec![\n            accounts.depositor,\n        ];",
    ] {
        assert!(
            initialize_witnessed.contains(expected),
            "pinned generated initialize_native_witnessed role/account order changed"
        );
    }
    for expected in [
        "let mut account_ids: Vec<AccountId> = vec![\n            accounts.metadata,\n            accounts.custody,\n            accounts.depositor,\n        ];",
        "let signer_ids: Vec<AccountId> = vec![\n            accounts.depositor,\n        ];",
    ] {
        assert!(
            funding.contains(expected),
            "pinned generated fund_native role/account order changed"
        );
    }
    for expected in [
        "let mut account_ids: Vec<AccountId> = vec![\n            accounts.metadata,\n            accounts.custody,\n            accounts.claimant,\n            accounts.aggregate_authority,\n        ];",
        "let signer_ids: Vec<AccountId> = vec![\n            accounts.aggregate_authority,\n        ];",
    ] {
        assert!(
            claim_witnessed.contains(expected),
            "pinned generated claim_native_witnessed role/account order changed"
        );
    }
}

fn generated_method<'a>(client: &'a str, name: &str, next: &str) -> &'a str {
    let start_marker = format!("    pub async fn {name}(");
    let start = client
        .find(&start_marker)
        .unwrap_or_else(|| panic!("pinned generated client is missing {name}"));
    let method_and_tail = &client[start..];
    let next_marker = format!("    pub async fn {next}(");
    let end = method_and_tail
        .find(&next_marker)
        .unwrap_or_else(|| panic!("pinned generated client is missing {next}"));
    &method_and_tail[..end]
}
