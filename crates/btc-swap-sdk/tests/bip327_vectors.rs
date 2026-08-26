mod support;

use musig2::errors::{SigningError, VerifyError};
use musig2::secp::{MaybeScalar, Point, Scalar};
use musig2::{
    AggNonce, BinaryEncoding as _, CompactSignature, KeyAggContext, PartialSignature, PubNonce,
    SecNonce, aggregate_partial_signatures, sign_partial, verify_partial,
};
use serde_json::Value;

use support::{bip327, cases, hex_array, hex_vec, indexes, strings};

fn point(value: &str) -> Result<Point, musig2::secp::errors::InvalidPointBytes> {
    Point::from_slice(&hex_vec(value))
}

fn scalar(value: &str) -> Result<Scalar, musig2::secp::errors::InvalidScalarBytes> {
    Scalar::from_slice(&hex_vec(value))
}

fn maybe_scalar(value: &str) -> Result<MaybeScalar, musig2::secp::errors::InvalidScalarBytes> {
    MaybeScalar::from_slice(&hex_vec(value))
}

fn pubnonce(value: &str) -> Result<PubNonce, String> {
    PubNonce::from_bytes(&hex_vec(value)).map_err(|error| error.to_string())
}

fn aggnonce(value: &str) -> Result<AggNonce, String> {
    AggNonce::from_bytes(&hex_vec(value)).map_err(|error| error.to_string())
}

fn secnonce(value: &str) -> Result<SecNonce, String> {
    SecNonce::from_bytes(&hex_vec(value)).map_err(|error| error.to_string())
}

fn usize_value(value: &Value, label: &str) -> usize {
    let raw = value
        .as_u64()
        .unwrap_or_else(|| panic!("{label} must be an unsigned integer"));
    usize::try_from(raw).unwrap_or_else(|_| panic!("{label} exceeds usize"))
}

fn selected_points(root: &Value, case: &Value) -> Result<Vec<Point>, (usize, String)> {
    let pubkeys = strings(root, "pubkeys");
    indexes(case, "key_indices")
        .into_iter()
        .enumerate()
        .map(|(signer, index)| point(&pubkeys[index]).map_err(|error| (signer, error.to_string())))
        .collect()
}

fn selected_nonces(root: &Value, case: &Value) -> Result<Vec<PubNonce>, (usize, String)> {
    let nonces = strings(root, "pnonces");
    indexes(case, "nonce_indices")
        .into_iter()
        .enumerate()
        .map(|(signer, index)| pubnonce(&nonces[index]).map_err(|error| (signer, error)))
        .collect()
}

fn apply_valid_tweaks(mut context: KeyAggContext, root: &Value, case: &Value) -> KeyAggContext {
    let tweaks = strings(root, "tweaks");
    let kinds = case["is_xonly"].as_array().expect("is_xonly array");
    for (position, index) in indexes(case, "tweak_indices").into_iter().enumerate() {
        let tweak = maybe_scalar(&tweaks[index]).expect("valid official tweak");
        context = context
            .with_tweak(
                tweak,
                kinds[position].as_bool().expect("boolean tweak kind"),
            )
            .expect("valid official tweak application");
    }
    context
}

#[test]
fn official_bip327_corpus_is_complete_and_deterministic_extension_is_classified() {
    let expected = [
        ("det_sign_vectors.json", 4, 5),
        ("key_agg_vectors.json", 4, 5),
        ("key_sort_vectors.json", 0, 0),
        ("nonce_agg_vectors.json", 2, 3),
        ("nonce_gen_vectors.json", 0, 0),
        ("sig_agg_vectors.json", 4, 1),
        ("sign_verify_vectors.json", 6, 0),
        ("tweak_vectors.json", 5, 1),
    ];
    for (name, valid, errors) in expected {
        let root = bip327(name);
        assert!(root.is_object(), "{name}");
        if valid > 0 {
            assert_eq!(cases(&root, "valid_test_cases").len(), valid, "{name}");
        }
        if errors > 0 {
            assert_eq!(cases(&root, "error_test_cases").len(), errors, "{name}");
        }
    }

    // BIP-327's deterministic-signing modification is optional for stateless
    // signers. This SDK instead reserves and consumes stateful random nonces.
    // Retain and validate the complete official extension corpus without
    // inventing a second signing implementation that production never calls.
    let root = bip327("det_sign_vectors.json");
    let public_keys = strings(&root, "pubkeys");
    let messages = strings(&root, "msgs");
    for case in cases(&root, "valid_test_cases") {
        let keys = selected_points(&root, case).expect("valid deterministic-signing keys");
        let mut context = KeyAggContext::new(keys).expect("valid key aggregation");
        let tweaks = strings(case, "tweaks");
        let kinds = case["is_xonly"].as_array().expect("is_xonly array");
        assert_eq!(tweaks.len(), kinds.len());
        for (position, tweak) in tweaks.into_iter().enumerate() {
            context = context
                .with_tweak(
                    maybe_scalar(&tweak).expect("valid deterministic-signing tweak"),
                    kinds[position].as_bool().expect("boolean tweak kind"),
                )
                .expect("valid deterministic-signing tweak application");
        }
        let _ = context.aggregated_pubkey::<Point>();
        aggnonce(
            case["aggothernonce"]
                .as_str()
                .expect("aggregate other nonce"),
        )
        .expect("valid aggregate other nonce");
        let signer = usize::try_from(case["signer_index"].as_u64().expect("signer index"))
            .expect("signer index fits");
        point(&public_keys[indexes(case, "key_indices")[signer]])
            .expect("valid deterministic signer");
        let message_index = usize::try_from(case["msg_index"].as_u64().expect("message index"))
            .expect("message index fits");
        let _ = hex_vec(&messages[message_index]);
        let expected = case["expected"]
            .as_array()
            .expect("expected nonce and partial");
        assert_eq!(
            hex_vec(expected[0].as_str().expect("public nonce")).len(),
            66
        );
        assert_eq!(
            hex_vec(expected[1].as_str().expect("partial signature")).len(),
            32
        );
    }
    assert_eq!(cases(&root, "error_test_cases").len(), 5);
}

#[test]
fn official_key_aggregation_and_sort_vectors_pass_and_reject_exact_errors() {
    let root = bip327("key_agg_vectors.json");
    for case in cases(&root, "valid_test_cases") {
        let keys = selected_points(&root, case).expect("valid key aggregation inputs");
        let aggregate = KeyAggContext::new(keys)
            .expect("valid key aggregation")
            .aggregated_pubkey::<Point>()
            .serialize_xonly();
        assert_eq!(
            aggregate,
            hex_array::<32>(case["expected"].as_str().expect("expected key"))
        );
    }

    for case in cases(&root, "error_test_cases") {
        let expected_signer = case["error"]["signer"]
            .as_u64()
            .map(|value| usize::try_from(value).expect("signer index fits usize"));
        match selected_points(&root, case) {
            Err((signer, _)) => assert_eq!(Some(signer), expected_signer),
            Ok(keys) => {
                let mut context = KeyAggContext::new(keys).expect("pre-tweak context");
                let tweaks = strings(&root, "tweaks");
                let kinds = case["is_xonly"].as_array().expect("is_xonly array");
                let mut rejected = false;
                for (position, index) in indexes(case, "tweak_indices").into_iter().enumerate() {
                    let Ok(tweak) = maybe_scalar(&tweaks[index]) else {
                        rejected = true;
                        break;
                    };
                    if let Ok(next) = context.with_tweak(
                        tweak,
                        kinds[position].as_bool().expect("boolean tweak kind"),
                    ) {
                        context = next;
                    } else {
                        rejected = true;
                        break;
                    }
                }
                assert!(rejected, "official invalid tweak must be rejected");
            }
        }
    }

    let root = bip327("key_sort_vectors.json");
    let mut keys = strings(&root, "pubkeys")
        .into_iter()
        .map(|key| point(&key).expect("valid sort key"))
        .collect::<Vec<_>>();
    keys.sort_unstable();
    let expected = strings(&root, "sorted_pubkeys")
        .into_iter()
        .map(|key| point(&key).expect("valid sorted key"))
        .collect::<Vec<_>>();
    assert_eq!(keys, expected);
}

#[test]
fn official_nonce_generation_and_aggregation_vectors_pass_and_reject_exact_errors() {
    let root = bip327("nonce_gen_vectors.json");
    for case in cases(&root, "test_cases") {
        let seed = hex_array::<32>(case["rand_"].as_str().expect("nonce seed"));
        let public_key = point(case["pk"].as_str().expect("public key")).expect("valid public key");
        let mut builder = if let Some(secret) = case["sk"].as_str() {
            let secret = scalar(secret).expect("valid nonce secret");
            assert_eq!(secret.base_point_mul(), public_key);
            SecNonce::build_with_seckey(seed, secret)
        } else {
            SecNonce::build_with_pubkey(seed, public_key)
        };
        if let Some(aggregate) = case["aggpk"].as_str() {
            builder = builder.with_aggregated_pubkey(
                Point::lift_x(hex_array::<32>(aggregate)).expect("valid aggregate x-only key"),
            );
        }
        let message = case["msg"].as_str().map(hex_vec);
        if let Some(message) = message.as_ref() {
            builder = builder.with_message(message);
        }
        let extra = case["extra_in"].as_str().map(hex_vec);
        if let Some(extra) = extra.as_ref() {
            builder = builder.with_extra_input(extra);
        }
        let nonce = builder.build();
        assert_eq!(
            nonce.serialize(),
            hex_array::<97>(case["expected_secnonce"].as_str().expect("secret nonce"))
        );
        assert_eq!(
            nonce.public_nonce().serialize(),
            hex_array::<66>(case["expected_pubnonce"].as_str().expect("public nonce"))
        );
    }

    let root = bip327("nonce_agg_vectors.json");
    let all_nonces = strings(&root, "pnonces");
    for case in cases(&root, "valid_test_cases") {
        let nonces = indexes(case, "pnonce_indices")
            .into_iter()
            .map(|index| pubnonce(&all_nonces[index]).expect("valid public nonce"))
            .collect::<Vec<_>>();
        assert_eq!(
            AggNonce::sum(&nonces).serialize(),
            hex_array::<66>(case["expected"].as_str().expect("aggregate nonce"))
        );
    }
    for case in cases(&root, "error_test_cases") {
        let expected_signer = usize_value(&case["error"]["signer"], "invalid signer");
        for (signer, index) in indexes(case, "pnonce_indices").into_iter().enumerate() {
            assert_eq!(
                pubnonce(&all_nonces[index]).is_err(),
                signer == expected_signer
            );
        }
    }
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the official fixture groups signing and verification success and rejection vectors"
)]
fn official_sign_verify_vectors_pass_and_cover_all_rejections() {
    let root = bip327("sign_verify_vectors.json");
    let secret = scalar(root["sk"].as_str().expect("signing key")).expect("valid signing key");
    let secret_nonces = strings(&root, "secnonces");
    let aggregate_nonces = strings(&root, "aggnonces");
    let messages = strings(&root, "msgs");

    for case in cases(&root, "valid_test_cases") {
        let keys = selected_points(&root, case).expect("valid signing keys");
        let signer = usize_value(&case["signer_index"], "signer index");
        assert_eq!(keys[signer], secret.base_point_mul());
        let context = KeyAggContext::new(keys.clone()).expect("valid signing context");
        let aggregate =
            aggnonce(&aggregate_nonces[usize_value(&case["aggnonce_index"], "nonce index")])
                .expect("valid aggregate nonce");
        let message = hex_vec(&messages[usize_value(&case["msg_index"], "message index")]);
        let partial: PartialSignature = sign_partial(
            &context,
            secret,
            secnonce(&secret_nonces[0]).expect("valid secret nonce"),
            &aggregate,
            &message,
        )
        .expect("valid official partial signing");
        assert_eq!(
            partial.serialize(),
            hex_array::<32>(case["expected"].as_str().expect("partial signature"))
        );
        let nonces = selected_nonces(&root, case).expect("valid signer nonces");
        assert_eq!(AggNonce::sum(&nonces), aggregate);
        verify_partial(
            &context,
            partial,
            &aggregate,
            keys[signer],
            &nonces[signer],
            &message,
        )
        .expect("valid official partial verification");
    }

    let sign_errors = cases(&root, "sign_error_test_cases");
    let missing = &sign_errors[0];
    let context = KeyAggContext::new(selected_points(&root, missing).expect("valid other keys"))
        .expect("valid missing-signer context");
    let aggregate =
        aggnonce(&aggregate_nonces[usize_value(&missing["aggnonce_index"], "nonce index")])
            .expect("valid aggregate nonce");
    let message = hex_vec(&messages[usize_value(&missing["msg_index"], "message index")]);
    assert_eq!(
        sign_partial::<PartialSignature>(
            &context,
            secret,
            secnonce(&secret_nonces[0]).expect("valid secret nonce"),
            &aggregate,
            &message,
        ),
        Err(SigningError::UnknownKey)
    );
    assert_eq!(
        selected_points(&root, &sign_errors[1])
            .expect_err("invalid pubkey")
            .0,
        2
    );
    for case in &sign_errors[2..5] {
        assert!(
            aggnonce(&aggregate_nonces[usize_value(&case["aggnonce_index"], "nonce index")])
                .is_err()
        );
    }
    assert!(secnonce(&secret_nonces[1]).is_err());

    for (position, case) in cases(&root, "verify_fail_test_cases").iter().enumerate() {
        let signature_bytes = hex_vec(case["sig"].as_str().expect("partial signature"));
        if position == 2 {
            assert!(MaybeScalar::from_slice(&signature_bytes).is_err());
            continue;
        }
        let partial = MaybeScalar::from_slice(&signature_bytes).expect("canonical invalid partial");
        let keys = selected_points(&root, case).expect("valid verification keys");
        let nonces = selected_nonces(&root, case).expect("valid verification nonces");
        let signer = usize_value(&case["signer_index"], "signer index");
        let context = KeyAggContext::new(keys.clone()).expect("valid verification context");
        let message = hex_vec(&messages[usize_value(&case["msg_index"], "message index")]);
        assert_eq!(
            verify_partial(
                &context,
                partial,
                &AggNonce::sum(&nonces),
                keys[signer],
                &nonces[signer],
                &message,
            ),
            Err(VerifyError::BadSignature)
        );
    }
    for case in cases(&root, "verify_error_test_cases") {
        let expected_signer = usize_value(&case["error"]["signer"], "invalid signer");
        match case["error"]["contrib"]
            .as_str()
            .expect("invalid contribution")
        {
            "pubnonce" => assert_eq!(
                selected_nonces(&root, case).expect_err("invalid nonce").0,
                expected_signer
            ),
            "pubkey" => assert_eq!(
                selected_points(&root, case).expect_err("invalid key").0,
                expected_signer
            ),
            other => panic!("unsupported official contribution: {other}"),
        }
    }
}

#[test]
fn official_tweak_and_signature_aggregation_vectors_pass_and_reject_errors() {
    let root = bip327("tweak_vectors.json");
    let secret = scalar(root["sk"].as_str().expect("signing key")).expect("valid signing key");
    let aggregate = aggnonce(root["aggnonce"].as_str().expect("aggregate nonce"))
        .expect("valid aggregate nonce");
    let message = hex_vec(root["msg"].as_str().expect("message"));
    for case in cases(&root, "valid_test_cases") {
        let keys = selected_points(&root, case).expect("valid tweak keys");
        let signer = usize_value(&case["signer_index"], "signer index");
        let context = apply_valid_tweaks(
            KeyAggContext::new(keys.clone()).expect("valid tweak context"),
            &root,
            case,
        );
        let partial: PartialSignature = sign_partial(
            &context,
            secret,
            secnonce(root["secnonce"].as_str().expect("secret nonce")).expect("valid secret nonce"),
            &aggregate,
            &message,
        )
        .expect("valid tweaked partial");
        assert_eq!(
            partial.serialize(),
            hex_array::<32>(case["expected"].as_str().expect("partial signature"))
        );
        let nonces = selected_nonces(&root, case).expect("valid tweak nonces");
        verify_partial(
            &context,
            partial,
            &aggregate,
            keys[signer],
            &nonces[signer],
            &message,
        )
        .expect("valid tweaked verification");
    }
    let invalid = &cases(&root, "error_test_cases")[0];
    let invalid_tweak = &strings(&root, "tweaks")[indexes(invalid, "tweak_indices")[0]];
    assert!(maybe_scalar(invalid_tweak).is_err());

    let root = bip327("sig_agg_vectors.json");
    let message = hex_vec(root["msg"].as_str().expect("message"));
    let all_nonces = strings(&root, "pnonces");
    let all_partials = strings(&root, "psigs");
    for case in cases(&root, "valid_test_cases") {
        let context = apply_valid_tweaks(
            KeyAggContext::new(selected_points(&root, case).expect("valid aggregation keys"))
                .expect("valid aggregation context"),
            &root,
            case,
        );
        let nonces = indexes(case, "nonce_indices")
            .into_iter()
            .map(|index| pubnonce(&all_nonces[index]).expect("valid aggregation nonce"))
            .collect::<Vec<_>>();
        let aggregate = AggNonce::sum(&nonces);
        assert_eq!(
            aggregate.serialize(),
            hex_array::<66>(case["aggnonce"].as_str().expect("aggregate nonce"))
        );
        let partials = indexes(case, "psig_indices")
            .into_iter()
            .map(|index| scalar(&all_partials[index]).expect("valid partial signature"))
            .collect::<Vec<_>>();
        let signature: CompactSignature =
            aggregate_partial_signatures(&context, &aggregate, partials, &message)
                .expect("valid signature aggregation");
        assert_eq!(
            signature.to_bytes(),
            hex_array::<64>(case["expected"].as_str().expect("aggregate signature"))
        );
        musig2::verify_single(context.aggregated_pubkey::<Point>(), signature, &message)
            .expect("valid final BIP-340 signature");
    }
    let invalid = &cases(&root, "error_test_cases")[0];
    let expected_signer = usize_value(&invalid["error"]["signer"], "invalid signer");
    for (signer, index) in indexes(invalid, "psig_indices").into_iter().enumerate() {
        assert_eq!(
            scalar(&all_partials[index]).is_err(),
            signer == expected_signer
        );
    }
}
