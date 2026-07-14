use lez_zec_escrow_v02::PROGRAM_IDL_JSON;

#[allow(dead_code, unused_imports, unused_mut)]
mod generated_client {
    include!(concat!(env!("OUT_DIR"), "/zec_escrow_client_module.rs"));
}

const EXPECTED_AUTHENTICATED_TRANSFER: [i64; 8] = [
    3_170_810_844,
    2_526_647_253,
    999_807_262,
    1_205_602_179,
    3_401_962_591,
    3_484_055_895,
    2_106_546_407,
    1_900_691_388,
];
const EXPECTED_TOKEN: [i64; 8] = [
    2_282_739_141,
    348_907_455,
    1_046_946_228,
    3_735_699_860,
    585_462_133,
    3_426_087_150,
    772_528_164,
    2_090_518_099,
];
const EXPECTED_ASSOCIATED_TOKEN_ACCOUNT: [i64; 8] = [
    3_357_312_149,
    3_615_960_253,
    3_351_583_505,
    2_234_166_003,
    4_153_433_811,
    2_743_238_177,
    2_886_052_503,
    4_160_755_157,
];

#[test]
fn v02_escrow_guest_generated_client_and_deployment_inputs_exist() {
    let idl: serde_json::Value =
        serde_json::from_str(PROGRAM_IDL_JSON).expect("SPEL must generate valid escrow IDL");
    let instructions = idl["instructions"]
        .as_array()
        .expect("generated IDL instructions");
    let names = instructions
        .iter()
        .map(|instruction| instruction["name"].as_str().expect("instruction name"))
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            "initialize_native",
            "fund_native",
            "claim_native",
            "refund_native",
            "initialize_token",
            "create_token_custody",
            "fund_token",
            "claim_token",
            "refund_token",
        ]
    );

    let generated = spel_client_gen::generate_from_idl_json(PROGRAM_IDL_JSON)
        .expect("the exact SPEL PR head must generate the typed v0.2 client")
        .client_code;
    let funding = generated_method(&generated, "fund_token", Some("claim_token"));
    assert!(funding.contains(
        "let signer_ids: Vec<AccountId> = vec![\n            accounts.depositor_owner,\n        ];"
    ));
    assert!(!funding.contains("accounts.depositor_asset,\n        ];\n        let nonces"));
    let claim = generated_method(&generated, "claim_token", Some("refund_token"));
    assert!(claim.contains(
        "let signer_ids: Vec<AccountId> = vec![\n            accounts.claimant_owner,\n        ];"
    ));
    assert!(!claim.contains("accounts.claimant_asset,\n        ];\n        let nonces"));
    let no_signers = "let signer_ids: Vec<AccountId> = vec![\n        ];";
    assert!(
        generated_method(&generated, "refund_native", Some("initialize_token"))
            .contains(no_signers)
    );
    assert!(
        generated_method(&generated, "create_token_custody", Some("fund_token"))
            .contains(no_signers)
    );
    assert!(generated_method(&generated, "refund_token", None).contains(no_signers));
    for method in names {
        assert!(
            generated.contains(&format!("pub async fn {method}(")),
            "generated client is missing {method}"
        );
    }

    let guest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("escrow/methods/guest/src/bin/zec_escrow_v02.rs");
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("escrow/methods/guest/deployment-manifest.toml");
    assert!(guest.is_file(), "missing deployable v0.2 Risc0 guest");
    assert!(
        manifest.is_file(),
        "missing immutable v0.2 deployment manifest"
    );

    let manifest: toml::Value = include_str!("../escrow/methods/guest/deployment-manifest.toml")
        .parse()
        .expect("deployment manifest must be valid TOML");
    assert_eq!(manifest["source"]["lez_tag"].as_str(), Some("v0.2.0"));
    assert_eq!(
        manifest["source"]["lez_commit"].as_str(),
        Some("a58fbce2ff48c58b7bb5001b1a27e64b9596ee3a")
    );
    assert_eq!(
        manifest["source"]["spel_commit"].as_str(),
        Some("df17acd98436be4f09c55877dae1fe2e73cbcdca")
    );
    assert_eq!(
        manifest["target"]["rpc_url"].as_str(),
        Some("https://testnet.lez.logos.co")
    );
    assert_eq!(
        manifest["target"]["channel_id"].as_str(),
        Some("0101010101010101010101010101010101010101010101010101010101010101")
    );
    for (field, expected) in [
        (
            "authenticated_transfer_program_id",
            EXPECTED_AUTHENTICATED_TRANSFER,
        ),
        ("token_program_id", EXPECTED_TOKEN),
        (
            "associated_token_account_program_id",
            EXPECTED_ASSOCIATED_TOKEN_ACCOUNT,
        ),
    ] {
        let actual = manifest["target"][field]
            .as_array()
            .unwrap_or_else(|| panic!("{field} must be eight words"))
            .iter()
            .map(|word| word.as_integer().expect("ProgramId word"))
            .collect::<Vec<_>>();
        assert_eq!(actual, expected, "wrong {field}");
    }
    assert_eq!(
        manifest["target"]["associated_token_account_identity_source"].as_str(),
        Some("lez-v0.2.0-checked-elf-rpc-map-omits-key")
    );
    assert_eq!(
        manifest["artifact_status"].as_str(),
        Some("locally-built-artifact-checked")
    );
    assert_eq!(
        manifest["artifact"]["elf_sha256"].as_str(),
        Some("c85055f6fe85b71535a322ba84ffc612f5d093954a721ba3b529428814dc9d2e")
    );
    assert_eq!(
        manifest["artifact"]["image_id"].as_str(),
        Some("5cf8c5a4eedb3c2873956cb7898eb33a495407c9746fb1a065c99638159329c1")
    );
    assert_eq!(
        manifest["artifact"]["program_id_words"]
            .as_array()
            .expect("artifact ProgramId words")
            .iter()
            .map(|word| word.as_integer().expect("ProgramId word"))
            .collect::<Vec<_>>(),
        vec![
            2_764_437_596,
            675_077_102,
            3_077_346_675,
            984_845_961,
            3_372_700_745,
            2_695_982_964,
            949_406_053,
            3_240_727_317,
        ]
    );
    assert!(
        deployment_ready(&manifest).is_err(),
        "the source template must never masquerade as accepted live evidence"
    );
}

fn generated_method<'a>(client: &'a str, name: &str, next: Option<&str>) -> &'a str {
    let marker = format!("    pub async fn {name}(");
    let start = client
        .find(&marker)
        .unwrap_or_else(|| panic!("generated client is missing {name}"));
    let method_and_tail = &client[start..];
    let Some(next) = next else {
        return method_and_tail;
    };
    let next_marker = format!("    pub async fn {next}(");
    let end = method_and_tail
        .find(&next_marker)
        .unwrap_or_else(|| panic!("generated client is missing {next}"));
    &method_and_tail[..end]
}

#[test]
fn official_generated_client_is_compiled_and_its_typed_surface_is_used() {
    use generated_client::{
        InitializeNativeAccounts, RefundTokenAccounts, ZecEscrowInstruction, compute_custody_pda,
        compute_metadata_pda, parse_program_id_hex,
    };

    let program_id =
        parse_program_id_hex("0100000002000000030000000400000005000000060000000700000008000000")
            .expect("generated client parses the deployment ProgramId encoding");
    assert_eq!(program_id, [1, 2, 3, 4, 5, 6, 7, 8]);
    let swap_id = [11; 32];
    let metadata = compute_metadata_pda(&program_id, &swap_id);
    let custody = compute_custody_pda(&program_id, &swap_id);
    assert_ne!(metadata, custody);

    let native_accounts = InitializeNativeAccounts {
        metadata,
        custody,
        depositor: nssa::AccountId::new([1; 32]),
        claimant: nssa::AccountId::new([2; 32]),
    };
    assert_eq!(native_accounts.metadata, metadata);
    let refund_accounts = RefundTokenAccounts {
        metadata,
        custody: nssa::AccountId::new([3; 32]),
        depositor_asset: nssa::AccountId::new([4; 32]),
    };
    assert_eq!(refund_accounts.metadata, metadata);
    assert!(matches!(
        ZecEscrowInstruction::ClaimToken {
            swap_id,
            preimage: [12; 32],
        },
        ZecEscrowInstruction::ClaimToken { .. }
    ));
}

#[test]
#[ignore = "M2 live gate: requires checked ELF deployment and canonical inclusion evidence"]
fn live_deployment_manifest_rejects_every_pending_or_missing_identity() {
    let manifest: toml::Value = include_str!("../escrow/methods/guest/deployment-manifest.toml")
        .parse()
        .expect("deployment manifest must be valid TOML");
    deployment_ready(&manifest).expect("complete immutable v0.2 deployment evidence");
}

fn deployment_ready(manifest: &toml::Value) -> Result<(), String> {
    if manifest["artifact_status"].as_str() != Some("deployed-and-observed") {
        return Err("artifact status is not deployed-and-observed".to_owned());
    }
    for (section, field) in [
        ("artifact", "elf_sha256"),
        ("artifact", "image_id"),
        ("deployment", "transaction_hash"),
        ("deployment", "inclusion_block_hash"),
    ] {
        let value = manifest[section][field]
            .as_str()
            .ok_or_else(|| format!("missing {section}.{field}"))?;
        if value == "pending"
            || value.len() != 64
            || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(format!("invalid {section}.{field}"));
        }
    }
    let words = manifest["artifact"]["program_id_words"]
        .as_array()
        .ok_or_else(|| "missing artifact.program_id_words".to_owned())?;
    if words.len() != 8
        || words
            .iter()
            .all(|word| word.as_integer().is_some_and(|word| word == 0))
    {
        return Err("invalid artifact.program_id_words".to_owned());
    }
    if manifest["deployment"]["inclusion_block_id"]
        .as_integer()
        .is_none_or(|block| block <= 0)
    {
        return Err("invalid deployment.inclusion_block_id".to_owned());
    }
    Ok(())
}
