mod support;

use bitcoin::hashes::{Hash as _, sha256};
use bitcoin::secp256k1::XOnlyPublicKey;
use bitcoin::{Amount, OutPoint, ScriptBuf, TxOut, Txid};
use k256::schnorr::{Signature as K256Signature, VerifyingKey};
use lez_btc_swap_sdk::{
    AdaptorSessionContext, CooperativeKeyPathSpend, CsvBlockDelay, OutputKeyParity, P2trSwapOutput,
    RefundXOnlyKey, TwoPartyAggregateKey, adapt_presignature, extract_adaptor_secret,
    verify_adaptor_presignature, verify_final_signature,
};
use musig2::secp::{Point, Scalar};
use musig2::{AdaptorSignature, FirstRound, KeyAggContext, PartialSignature, SecNonceSpices};
use serde_json::Value;
use zeroize::Zeroizing;

use support::{hex_array, vector_root};

const COMMITMENT_DOMAIN: &[u8] = b"lez-atomic-swaps/m3/btc-claim/v1";

fn fixture() -> Value {
    let path = vector_root()
        .parent()
        .expect("official corpus has parent")
        .join("lez-btc-adaptor-v1.json");
    serde_json::from_slice(&std::fs::read(&path).expect("read adaptor fixture"))
        .expect("parse adaptor fixture")
}

fn scalar(value: &Value, key: &str) -> Scalar {
    Scalar::from_slice(&hex_array::<32>(
        value[key]
            .as_str()
            .unwrap_or_else(|| panic!("missing {key}")),
    ))
    .unwrap_or_else(|_| panic!("invalid {key}"))
}

fn u32_field(value: &Value, key: &str) -> u32 {
    let raw = value[key]
        .as_u64()
        .unwrap_or_else(|| panic!("missing {key}"));
    u32::try_from(raw).unwrap_or_else(|_| panic!("{key} exceeds u32"))
}

fn nonce_commitment(
    role: u8,
    session_domain: &[u8; 32],
    message: &[u8; 32],
    public_nonce: &[u8; 66],
) -> [u8; 32] {
    let mut preimage = Vec::with_capacity(
        COMMITMENT_DOMAIN.len() + 1 + session_domain.len() + message.len() + public_nonce.len(),
    );
    preimage.extend_from_slice(COMMITMENT_DOMAIN);
    preimage.push(role);
    preimage.extend_from_slice(session_domain);
    preimage.extend_from_slice(message);
    preimage.extend_from_slice(public_nonce);
    sha256::Hash::hash(&preimage).to_byte_array()
}

struct DeterministicPresignatureInput {
    context: KeyAggContext,
    maker_secret: Scalar,
    taker_secret: Scalar,
    maker_seed: [u8; 32],
    taker_seed: [u8; 32],
    adaptor_point: Point,
    message: [u8; 32],
    session_domain: [u8; 32],
}

fn deterministic_presignature(
    input: DeterministicPresignatureInput,
) -> (AdaptorSignature, [u8; 32], [u8; 32]) {
    let DeterministicPresignatureInput {
        context,
        maker_secret,
        taker_secret,
        maker_seed,
        taker_seed,
        adaptor_point,
        message,
        session_domain,
    } = input;
    let maker_spices = SecNonceSpices::new()
        .with_seckey(maker_secret)
        .with_message(&message)
        .with_extra_input(&session_domain);
    let taker_spices = SecNonceSpices::new()
        .with_seckey(taker_secret)
        .with_message(&message)
        .with_extra_input(&session_domain);
    let mut maker =
        FirstRound::new(context.clone(), maker_seed, 0, maker_spices).expect("maker first round");
    let mut taker =
        FirstRound::new(context, taker_seed, 1, taker_spices).expect("taker first round");
    let maker_nonce = maker.our_public_nonce();
    let taker_nonce = taker.our_public_nonce();
    let maker_commitment = nonce_commitment(0, &session_domain, &message, &maker_nonce.serialize());
    let taker_commitment = nonce_commitment(1, &session_domain, &message, &taker_nonce.serialize());
    maker
        .receive_nonce(1, taker_nonce)
        .expect("maker accepts taker nonce");
    taker
        .receive_nonce(0, maker_nonce)
        .expect("taker accepts maker nonce");
    let mut maker = maker
        .finalize_adaptor(maker_secret, adaptor_point, message)
        .expect("maker adaptor partial");
    let mut taker = taker
        .finalize_adaptor(taker_secret, adaptor_point, message)
        .expect("taker adaptor partial");
    let maker_partial: PartialSignature = maker.our_signature();
    let taker_partial: PartialSignature = taker.our_signature();
    maker
        .receive_signature(1, taker_partial)
        .expect("maker accepts taker partial");
    taker
        .receive_signature(0, maker_partial)
        .expect("taker accepts maker partial");
    let maker_presignature = maker
        .finalize_adaptor::<AdaptorSignature>()
        .expect("maker aggregate presignature");
    let taker_presignature = taker
        .finalize_adaptor::<AdaptorSignature>()
        .expect("taker aggregate presignature");
    assert_eq!(maker_presignature, taker_presignature);
    (maker_presignature, maker_commitment, taker_commitment)
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the positive fixture test intentionally preserves one auditable end-to-end transcript"
)]
fn pinned_swap_adaptor_fixture_adapts_extracts_and_cross_checks_independently() {
    let fixture = fixture();
    let inputs = &fixture["inputs"];
    let expected = &fixture["expected"];
    let maker_secret = scalar(inputs, "maker_secret");
    let taker_secret = scalar(inputs, "taker_secret");
    let adaptor_secret = scalar(inputs, "adaptor_secret");
    let refund_secret = scalar(inputs, "refund_secret");
    let maker_public = maker_secret.base_point_mul();
    let taker_public = taker_secret.base_point_mul();
    let untweaked = KeyAggContext::new([maker_public, taker_public]).expect("key aggregation");
    let internal: Point = untweaked.aggregated_pubkey_untweaked();
    assert_eq!(
        internal.serialize_xonly(),
        hex_array::<32>(
            expected["aggregate_internal_key"]
                .as_str()
                .expect("internal key")
        )
    );
    let contract = P2trSwapOutput::new(
        TwoPartyAggregateKey::from_bytes(internal.serialize_xonly()).expect("internal key"),
        RefundXOnlyKey::from_bytes(refund_secret.base_point_mul().serialize_xonly())
            .expect("refund key"),
        CsvBlockDelay::new(u32_field(inputs, "csv_blocks")).expect("valid CSV"),
    )
    .expect("Taproot contract");
    assert_eq!(
        contract.output_key_bytes(),
        hex_array::<32>(expected["taproot_output_key"].as_str().expect("output key"))
    );
    let destination = XOnlyPublicKey::from_slice(&hex_array::<32>(
        inputs["claim_destination_xonly"]
            .as_str()
            .expect("destination"),
    ))
    .expect("valid destination");
    let spend = CooperativeKeyPathSpend::new(
        &contract,
        OutPoint {
            txid: Txid::from_byte_array(hex_array::<32>(
                inputs["funding_txid_bytes"].as_str().expect("funding txid"),
            )),
            vout: u32_field(inputs, "funding_vout"),
        },
        Amount::from_sat(inputs["funding_value_sat"].as_u64().expect("funding value")),
        vec![TxOut {
            value: Amount::from_sat(inputs["claim_value_sat"].as_u64().expect("claim value")),
            script_pubkey: ScriptBuf::new_p2tr(
                &bitcoin::secp256k1::Secp256k1::verification_only(),
                destination,
                None,
            ),
        }],
    )
    .expect("cooperative spend");
    let message = spend.sighash_bytes();
    let session_domain = sha256::Hash::hash(
        &[
            COMMITMENT_DOMAIN,
            &contract.aggregate_internal_key_bytes(),
            &contract.merkle_root_bytes(),
            &contract.output_key_bytes(),
            &message,
        ]
        .concat(),
    )
    .to_byte_array();
    let signing_context = untweaked
        .clone()
        .with_taproot_tweak(&contract.merkle_root_bytes())
        .expect("Taproot signing tweak");
    let output: Point = signing_context.aggregated_pubkey();
    assert_eq!(output.serialize_xonly(), contract.output_key_bytes());
    assert_eq!(
        output.has_even_y(),
        matches!(contract.output_key_parity(), OutputKeyParity::Even)
    );

    let adaptor_point = adaptor_secret.base_point_mul();
    assert_eq!(
        adaptor_point.serialize(),
        hex_array::<33>(expected["adaptor_point"].as_str().expect("adaptor point"))
    );
    let (presignature, maker_commitment, taker_commitment) =
        deterministic_presignature(DeterministicPresignatureInput {
            context: signing_context,
            maker_secret,
            taker_secret,
            maker_seed: hex_array::<32>(inputs["maker_nonce_seed"].as_str().expect("maker seed")),
            taker_seed: hex_array::<32>(inputs["taker_nonce_seed"].as_str().expect("taker seed")),
            adaptor_point,
            message,
            session_domain,
        });
    assert_eq!(
        maker_commitment,
        hex_array::<32>(
            expected["maker_nonce_commitment"]
                .as_str()
                .expect("maker commitment")
        )
    );
    assert_eq!(
        taker_commitment,
        hex_array::<32>(
            expected["taker_nonce_commitment"]
                .as_str()
                .expect("taker commitment")
        )
    );
    let presignature_bytes = presignature.serialize();
    assert_eq!(
        presignature_bytes,
        hex_array::<65>(
            expected["adaptor_presignature"]
                .as_str()
                .expect("presignature")
        )
    );

    let sdk_context = AdaptorSessionContext::taproot(
        [maker_public.serialize(), taker_public.serialize()],
        contract.merkle_root_bytes(),
        message,
        adaptor_point.serialize(),
        session_domain,
    )
    .expect("SDK Taproot context");
    assert_eq!(sdk_context.output_key(), contract.output_key_bytes());
    verify_adaptor_presignature(&sdk_context, presignature_bytes)
        .expect("SDK verifies fixture presignature");
    let final_signature = adapt_presignature(
        &sdk_context,
        presignature_bytes,
        Zeroizing::new(adaptor_secret.serialize()),
    )
    .expect("SDK adapts fixture");
    assert_eq!(
        final_signature,
        hex_array::<64>(
            expected["final_signature"]
                .as_str()
                .expect("final signature")
        )
    );
    verify_final_signature(&sdk_context, final_signature).expect("SDK final verification");
    let extracted = extract_adaptor_secret(&sdk_context, presignature_bytes, final_signature)
        .expect("SDK extracts fixture scalar");
    assert_eq!(
        *extracted,
        hex_array::<32>(
            expected["extracted_scalar"]
                .as_str()
                .expect("extracted scalar")
        )
    );

    let independent_key = VerifyingKey::from_bytes(&sdk_context.output_key())
        .expect("k256 accepts Taproot output key");
    let independent_signature =
        K256Signature::try_from(final_signature.as_slice()).expect("k256 accepts final signature");
    independent_key
        .verify_raw(&message, &independent_signature)
        .expect("independent k256 verification");

    let signed = spend
        .finalize(final_signature)
        .expect("rust-bitcoin verification");
    assert_eq!(
        signed.compute_txid().to_string(),
        expected["signed_txid"].as_str().expect("txid")
    );
    assert_eq!(
        signed.compute_wtxid().to_string(),
        expected["signed_wtxid"].as_str().expect("wtxid")
    );
    assert_eq!(signed.input[0].witness.len(), 1);
    assert_eq!(signed.input[0].witness[0].len(), 64);
}

#[test]
#[allow(
    clippy::too_many_lines,
    reason = "the negative fixture test intentionally audits every transcript substitution inline"
)]
fn pinned_swap_adaptor_fixture_rejects_context_secret_and_signature_substitution() {
    let fixture = fixture();
    let inputs = &fixture["inputs"];
    let expected = &fixture["expected"];
    let maker_secret = scalar(inputs, "maker_secret");
    let taker_secret = scalar(inputs, "taker_secret");
    let adaptor_secret = scalar(inputs, "adaptor_secret");
    let refund_secret = scalar(inputs, "refund_secret");
    let maker_public = maker_secret.base_point_mul();
    let taker_public = taker_secret.base_point_mul();
    let internal: Point = KeyAggContext::new([maker_public, taker_public])
        .expect("key aggregation")
        .aggregated_pubkey_untweaked();
    let contract = P2trSwapOutput::new(
        TwoPartyAggregateKey::from_bytes(internal.serialize_xonly()).expect("internal key"),
        RefundXOnlyKey::from_bytes(refund_secret.base_point_mul().serialize_xonly())
            .expect("refund key"),
        CsvBlockDelay::new(u32_field(inputs, "csv_blocks")).expect("valid CSV"),
    )
    .expect("Taproot contract");
    let destination = XOnlyPublicKey::from_slice(&hex_array::<32>(
        inputs["claim_destination_xonly"]
            .as_str()
            .expect("destination"),
    ))
    .expect("valid destination");
    let spend = CooperativeKeyPathSpend::new(
        &contract,
        OutPoint {
            txid: Txid::from_byte_array(hex_array::<32>(
                inputs["funding_txid_bytes"].as_str().expect("funding txid"),
            )),
            vout: u32_field(inputs, "funding_vout"),
        },
        Amount::from_sat(inputs["funding_value_sat"].as_u64().expect("funding value")),
        vec![TxOut {
            value: Amount::from_sat(inputs["claim_value_sat"].as_u64().expect("claim value")),
            script_pubkey: ScriptBuf::new_p2tr(
                &bitcoin::secp256k1::Secp256k1::verification_only(),
                destination,
                None,
            ),
        }],
    )
    .expect("cooperative spend");
    let message = spend.sighash_bytes();
    let mut changed_message = message;
    changed_message[0] ^= 1;
    let adaptor_point = adaptor_secret.base_point_mul().serialize();
    let presignature = hex_array::<65>(
        expected["adaptor_presignature"]
            .as_str()
            .expect("presignature"),
    );
    let final_signature = hex_array::<64>(
        expected["final_signature"]
            .as_str()
            .expect("final signature"),
    );
    let session = sha256::Hash::hash(
        &[
            COMMITMENT_DOMAIN,
            &contract.aggregate_internal_key_bytes(),
            &contract.merkle_root_bytes(),
            &contract.output_key_bytes(),
            &message,
        ]
        .concat(),
    )
    .to_byte_array();

    let contexts = [
        AdaptorSessionContext::taproot(
            [maker_public.serialize(), taker_public.serialize()],
            contract.merkle_root_bytes(),
            changed_message,
            adaptor_point,
            session,
        )
        .expect("changed-message context"),
        AdaptorSessionContext::taproot(
            [taker_public.serialize(), maker_public.serialize()],
            contract.merkle_root_bytes(),
            message,
            adaptor_point,
            session,
        )
        .expect("changed-role context"),
        AdaptorSessionContext::taproot(
            [maker_public.serialize(), taker_public.serialize()],
            {
                let mut root = contract.merkle_root_bytes();
                root[0] ^= 1;
                root
            },
            message,
            adaptor_point,
            session,
        )
        .expect("changed-tweak context"),
        AdaptorSessionContext::taproot(
            [maker_public.serialize(), taker_public.serialize()],
            contract.merkle_root_bytes(),
            message,
            Scalar::from_slice(&[0x54; 32])
                .expect("wrong adaptor scalar")
                .base_point_mul()
                .serialize(),
            session,
        )
        .expect("changed-adaptor context"),
    ];
    for context in contexts {
        assert!(verify_adaptor_presignature(&context, presignature).is_err());
    }

    let context = AdaptorSessionContext::taproot(
        [maker_public.serialize(), taker_public.serialize()],
        contract.merkle_root_bytes(),
        message,
        adaptor_point,
        session,
    )
    .expect("canonical negative context");
    verify_adaptor_presignature(&context, presignature).expect("valid negative-test baseline");
    verify_final_signature(&context, final_signature).expect("valid final-signature baseline");
    let mut mutated_presignature = presignature;
    mutated_presignature[64] ^= 1;
    assert!(verify_adaptor_presignature(&context, mutated_presignature).is_err());
    let mut mutated_final = final_signature;
    mutated_final[63] ^= 1;
    assert!(verify_final_signature(&context, mutated_final).is_err());
    assert!(adapt_presignature(&context, presignature, Zeroizing::new([0; 32])).is_err());
    assert!(adapt_presignature(&context, presignature, Zeroizing::new([0x54; 32])).is_err());
    assert!(
        AdaptorSessionContext::taproot(
            [maker_public.serialize(), taker_public.serialize()],
            contract.merkle_root_bytes(),
            message,
            [0; 33],
            session,
        )
        .is_err()
    );
}
