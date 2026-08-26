//! Fixture-only two-party `MuSig2` adaptor happy path for the M3 `PoC`.
//!
//! This example intentionally keeps both role secrets in one process so the
//! cryptographic composition can be reproduced before actor/store integration.
//! It is not a production signer and must never be used with non-Regtest keys.

use std::error::Error;

use bitcoin::hashes::{Hash, sha256};
use bitcoin::hex::DisplayHex;
use bitcoin::secp256k1::XOnlyPublicKey;
use bitcoin::{Amount, OutPoint, ScriptBuf, Transaction, TxOut, Txid};
use lez_btc_swap_sdk::{
    CooperativeKeyPathSpend, CsvBlockDelay, OutputKeyParity, P2trSwapOutput, RefundXOnlyKey,
    TwoPartyAggregateKey,
};
use musig2::secp::{MaybeScalar, Point, Scalar};
use musig2::{
    AdaptorSignature, FirstRound, KeyAggContext, LiftedSignature, PartialSignature, SecNonceSpices,
};
use zeroize::Zeroize;

const MAKER_ROLE: u8 = 0;
const TAKER_ROLE: u8 = 1;
const DOMAIN_TAG: &[u8] = b"lez-atomic-swaps/m3/btc-claim/v1";

struct Fixture {
    maker_secret: Scalar,
    taker_secret: Scalar,
    adaptor_secret: Scalar,
    contract: P2trSwapOutput,
    signing_context: KeyAggContext,
    output_point: Point,
}

struct Evidence<'a> {
    contract: &'a P2trSwapOutput,
    adaptor_point: &'a Point,
    maker_commitment: &'a [u8; 32],
    taker_commitment: &'a [u8; 32],
    presignature: &'a AdaptorSignature,
    final_signature: &'a [u8; 64],
    extracted_scalar: &'a [u8; 32],
    signed: &'a Transaction,
}
fn nonce_commitment(
    role: u8,
    session_domain: &[u8; 32],
    message: &[u8; 32],
    public_nonce: &[u8; 66],
) -> [u8; 32] {
    let mut preimage =
        Vec::with_capacity(DOMAIN_TAG.len() + 1 + session_domain.len() + message.len() + 66);
    preimage.extend_from_slice(DOMAIN_TAG);
    preimage.push(role);
    preimage.extend_from_slice(session_domain);
    preimage.extend_from_slice(message);
    preimage.extend_from_slice(public_nonce);
    sha256::Hash::hash(&preimage).to_byte_array()
}

fn fixture() -> Result<Fixture, Box<dyn Error>> {
    let mut maker_secret_bytes = [0x31; 32];
    let mut taker_secret_bytes = [0x42; 32];
    let mut adaptor_secret_bytes = [0x53; 32];

    let maker_secret = Scalar::from_slice(&maker_secret_bytes)?;
    let taker_secret = Scalar::from_slice(&taker_secret_bytes)?;
    let adaptor_secret = Scalar::from_slice(&adaptor_secret_bytes)?;
    maker_secret_bytes.zeroize();
    taker_secret_bytes.zeroize();
    adaptor_secret_bytes.zeroize();

    let maker_public = maker_secret.base_point_mul();
    let taker_public = taker_secret.base_point_mul();
    let untweaked_context = KeyAggContext::new([maker_public, taker_public])?;
    let internal_point: Point = untweaked_context.aggregated_pubkey_untweaked();

    let refund_secret = Scalar::from_slice(&[0x64; 32])?;
    let refund_point = refund_secret.base_point_mul();
    let contract = P2trSwapOutput::new(
        TwoPartyAggregateKey::from_bytes(internal_point.serialize_xonly())?,
        RefundXOnlyKey::from_bytes(refund_point.serialize_xonly())?,
        CsvBlockDelay::new(144)?,
    )?;

    let signing_context = untweaked_context.with_taproot_tweak(&contract.merkle_root_bytes())?;
    let output_point: Point = signing_context.aggregated_pubkey();
    assert_eq!(contract.output_key_bytes(), output_point.serialize_xonly());
    assert_eq!(
        matches!(contract.output_key_parity(), OutputKeyParity::Even),
        output_point.has_even_y(),
    );

    Ok(Fixture {
        maker_secret,
        taker_secret,
        adaptor_secret,
        contract,
        signing_context,
        output_point,
    })
}

fn cooperative_spend(contract: &P2trSwapOutput) -> Result<CooperativeKeyPathSpend, Box<dyn Error>> {
    let destination_key = XOnlyPublicKey::from_slice(&[
        0x79, 0xbe, 0x66, 0x7e, 0xf9, 0xdc, 0xbb, 0xac, 0x55, 0xa0, 0x62, 0x95, 0xce, 0x87, 0x0b,
        0x07, 0x02, 0x9b, 0xfc, 0xdb, 0x2d, 0xce, 0x28, 0xd9, 0x59, 0xf2, 0x81, 0x5b, 0x16, 0xf8,
        0x17, 0x98,
    ])?;
    Ok(CooperativeKeyPathSpend::new(
        contract,
        OutPoint {
            txid: Txid::from_byte_array([0x75; 32]),
            vout: 0,
        },
        Amount::from_sat(100_000_000),
        vec![TxOut {
            value: Amount::from_sat(99_999_000),
            script_pubkey: ScriptBuf::new_p2tr(
                &bitcoin::secp256k1::Secp256k1::verification_only(),
                destination_key,
                None,
            ),
        }],
    )?)
}

fn print_evidence(evidence: &Evidence<'_>) {
    println!("schema_version=1");
    println!("fixture_only=true");
    println!("role_order=maker,taker");
    println!(
        "aggregate_internal_key={}",
        evidence
            .contract
            .aggregate_internal_key_bytes()
            .to_lower_hex_string()
    );
    println!(
        "taproot_output_key={}",
        evidence.contract.output_key_bytes().to_lower_hex_string()
    );
    println!(
        "adaptor_point={}",
        evidence.adaptor_point.serialize().to_lower_hex_string()
    );
    println!(
        "maker_nonce_commitment={}",
        evidence.maker_commitment.to_lower_hex_string()
    );
    println!(
        "taker_nonce_commitment={}",
        evidence.taker_commitment.to_lower_hex_string()
    );
    println!(
        "adaptor_presignature={}",
        evidence.presignature.serialize().to_lower_hex_string()
    );
    println!(
        "final_signature={}",
        evidence.final_signature.to_lower_hex_string()
    );
    println!(
        "extracted_scalar={}",
        evidence.extracted_scalar.to_lower_hex_string()
    );
    println!("signed_txid={}", evidence.signed.compute_txid());
    println!("signed_wtxid={}", evidence.signed.compute_wtxid());
    println!("witness_items=1");
    println!("witness_bytes=64");
}

fn main() -> Result<(), Box<dyn Error>> {
    let Fixture {
        maker_secret,
        taker_secret,
        adaptor_secret,
        contract,
        signing_context,
        output_point,
    } = fixture()?;

    let spend = cooperative_spend(&contract)?;
    let message = spend.sighash_bytes();
    let session_domain = sha256::Hash::hash(
        &[
            DOMAIN_TAG,
            &contract.aggregate_internal_key_bytes(),
            &contract.merkle_root_bytes(),
            &contract.output_key_bytes(),
            &message,
        ]
        .concat(),
    )
    .to_byte_array();

    let maker_spices = SecNonceSpices::new()
        .with_seckey(maker_secret)
        .with_message(&message)
        .with_extra_input(&session_domain);
    let taker_spices = SecNonceSpices::new()
        .with_seckey(taker_secret)
        .with_message(&message)
        .with_extra_input(&session_domain);
    let mut maker_first = FirstRound::new(
        signing_context.clone(),
        [0x71; 32],
        usize::from(MAKER_ROLE),
        maker_spices,
    )?;
    let mut taker_first = FirstRound::new(
        signing_context.clone(),
        [0x82; 32],
        usize::from(TAKER_ROLE),
        taker_spices,
    )?;

    let maker_nonce = maker_first.our_public_nonce();
    let taker_nonce = taker_first.our_public_nonce();
    let maker_nonce_bytes = maker_nonce.serialize();
    let taker_nonce_bytes = taker_nonce.serialize();
    let maker_commitment =
        nonce_commitment(MAKER_ROLE, &session_domain, &message, &maker_nonce_bytes);
    let taker_commitment =
        nonce_commitment(TAKER_ROLE, &session_domain, &message, &taker_nonce_bytes);

    assert_eq!(
        maker_commitment,
        nonce_commitment(MAKER_ROLE, &session_domain, &message, &maker_nonce_bytes)
    );
    assert_eq!(
        taker_commitment,
        nonce_commitment(TAKER_ROLE, &session_domain, &message, &taker_nonce_bytes)
    );
    maker_first.receive_nonce(usize::from(TAKER_ROLE), taker_nonce)?;
    taker_first.receive_nonce(usize::from(MAKER_ROLE), maker_nonce)?;

    let adaptor_point = adaptor_secret.base_point_mul();
    let mut maker_second = maker_first.finalize_adaptor(maker_secret, adaptor_point, message)?;
    let mut taker_second = taker_first.finalize_adaptor(taker_secret, adaptor_point, message)?;
    let maker_partial: PartialSignature = maker_second.our_signature();
    let taker_partial: PartialSignature = taker_second.our_signature();
    maker_second.receive_signature(usize::from(TAKER_ROLE), taker_partial)?;
    taker_second.receive_signature(usize::from(MAKER_ROLE), maker_partial)?;

    let maker_presignature = maker_second.finalize_adaptor::<AdaptorSignature>()?;
    let taker_presignature = taker_second.finalize_adaptor::<AdaptorSignature>()?;
    assert_eq!(maker_presignature, taker_presignature);
    musig2::adaptor::verify_single(output_point, &maker_presignature, message, adaptor_point)?;

    let final_signature: LiftedSignature = maker_presignature
        .adapt(adaptor_secret)
        .ok_or("adaptor secret produced an invalid final nonce")?;
    musig2::verify_single(output_point, final_signature, message)?;
    let final_signature_bytes = final_signature.serialize();
    let extracted: MaybeScalar = maker_presignature
        .reveal_secret(&final_signature)
        .ok_or("failed to extract adaptor secret")?;
    assert_eq!(extracted, MaybeScalar::Valid(adaptor_secret));
    let extracted_bytes: [u8; 32] = extracted.into();
    assert_eq!(extracted_bytes, adaptor_secret.serialize());
    assert_eq!(
        Scalar::from_slice(&extracted_bytes)?.base_point_mul(),
        adaptor_point
    );

    let signed = spend.finalize(final_signature_bytes)?;
    assert_eq!(signed.input[0].witness.len(), 1);
    assert_eq!(signed.input[0].witness.iter().next().unwrap().len(), 64);

    print_evidence(&Evidence {
        contract: &contract,
        adaptor_point: &adaptor_point,
        maker_commitment: &maker_commitment,
        taker_commitment: &taker_commitment,
        presignature: &maker_presignature,
        final_signature: &final_signature_bytes,
        extracted_scalar: &extracted_bytes,
        signed: &signed,
    });

    Ok(())
}
