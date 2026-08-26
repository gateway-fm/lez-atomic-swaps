//! Fixture-only dual-session adaptor link for the M3 progressive `PoC`.
//!
//! This proves both atomic reveal orders over exact 32-byte BTC and LEZ
//! messages. It does not submit the LEZ signature to a node yet.

use std::error::Error;

use bitcoin::hashes::{Hash as _, sha256};
use bitcoin::hex::DisplayHex as _;
use bitcoin::secp256k1::XOnlyPublicKey;
use bitcoin::{Amount, OutPoint, ScriptBuf, Transaction, TxOut, Txid};
use lez_btc_swap_sdk::{
    AdaptorSessionContext, AdaptorSigner, CooperativeKeyPathSpend, CsvBlockDelay, OutputKeyParity,
    P2trSwapOutput, RefundXOnlyKey, SigningRole, TwoPartyAggregateKey, adapt_presignature,
    extract_adaptor_secret, verify_final_signature,
};
use musig2::KeyAggContext;
use musig2::secp::{Point, Scalar};
use zeroize::Zeroizing;

const BTC_MAKER_SECRET: [u8; 32] = [0x31; 32];
const BTC_TAKER_SECRET: [u8; 32] = [0x42; 32];
const LEZ_MAKER_SECRET: [u8; 32] = [0x35; 32];
const LEZ_TAKER_SECRET: [u8; 32] = [0x46; 32];
const ADAPTOR_SECRET: [u8; 32] = [0x53; 32];

struct CompletedSession {
    maker_commitment: [u8; 32],
    taker_commitment: [u8; 32],
    presignature: [u8; 65],
}

fn scalar(bytes: [u8; 32]) -> Result<Scalar, Box<dyn Error>> {
    Ok(Scalar::from_slice(&bytes)?)
}

fn public_key(secret: [u8; 32]) -> Result<[u8; 33], Box<dyn Error>> {
    Ok(scalar(secret)?.base_point_mul().serialize())
}

fn complete_session(
    context: &AdaptorSessionContext,
    maker_secret: [u8; 32],
    taker_secret: [u8; 32],
) -> Result<CompletedSession, Box<dyn Error>> {
    let mut maker = AdaptorSigner::new(context.clone(), SigningRole::Maker, maker_secret)?;
    let mut taker = AdaptorSigner::new(context.clone(), SigningRole::Taker, taker_secret)?;

    let maker_commitment = maker.nonce_commitment();
    let taker_commitment = taker.nonce_commitment();
    maker.accept_peer_commitment(taker_commitment)?;
    taker.accept_peer_commitment(maker_commitment)?;

    let maker_nonce = maker.public_nonce()?;
    let taker_nonce = taker.public_nonce()?;
    maker.accept_peer_nonce(taker_nonce)?;
    taker.accept_peer_nonce(maker_nonce)?;

    let maker_partial = maker.create_partial_signature()?;
    let taker_partial = taker.create_partial_signature()?;
    maker.accept_peer_partial_signature(taker_partial)?;
    taker.accept_peer_partial_signature(maker_partial)?;

    let maker_presignature = maker.presignature()?;
    let taker_presignature = taker.presignature()?;
    if maker_presignature != taker_presignature {
        return Err("roles derived different presignatures".into());
    }
    Ok(CompletedSession {
        maker_commitment,
        taker_commitment,
        presignature: maker_presignature,
    })
}

fn contract(
    maker_secret: [u8; 32],
    taker_secret: [u8; 32],
) -> Result<P2trSwapOutput, Box<dyn Error>> {
    let aggregation = KeyAggContext::new([
        scalar(maker_secret)?.base_point_mul(),
        scalar(taker_secret)?.base_point_mul(),
    ])?;
    let internal: Point = aggregation.aggregated_pubkey_untweaked();
    let refund = scalar([0x64; 32])?.base_point_mul();
    Ok(P2trSwapOutput::new(
        TwoPartyAggregateKey::from_bytes(internal.serialize_xonly())?,
        RefundXOnlyKey::from_bytes(refund.serialize_xonly())?,
        CsvBlockDelay::new(144)?,
    )?)
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

fn session_id(agreement: &[u8; 32], purpose: &[u8]) -> [u8; 32] {
    let mut bytes = Vec::with_capacity(agreement.len() + purpose.len());
    bytes.extend_from_slice(agreement);
    bytes.extend_from_slice(purpose);
    sha256::Hash::hash(&bytes).to_byte_array()
}

fn print_result(
    btc_context: &AdaptorSessionContext,
    lez_context: &AdaptorSessionContext,
    btc: &CompletedSession,
    lez: &CompletedSession,
    btc_signature: &[u8; 64],
    lez_signature: &[u8; 64],
    signed: &Transaction,
) {
    println!("schema_version=1");
    println!("fixture_only=true");
    println!("signer_separation=distinct_state_objects");
    println!("nonce_source=os_random");
    println!("commitment_exchange_before_nonce_reveal=true");
    println!("dual_domain_sessions=true");
    println!("shared_adaptor_point=true");
    println!("taker_sells_foreign_order=lez_reveal_then_btc_claim");
    println!("taker_sells_lez_order=btc_reveal_then_lez_claim");
    println!(
        "btc_session_id={}",
        btc_context.session_id().to_lower_hex_string()
    );
    println!(
        "lez_session_id={}",
        lez_context.session_id().to_lower_hex_string()
    );
    println!(
        "btc_output_key={}",
        btc_context.output_key().to_lower_hex_string()
    );
    println!(
        "lez_aggregate_key={}",
        lez_context.output_key().to_lower_hex_string()
    );
    println!(
        "btc_maker_commitment={}",
        btc.maker_commitment.to_lower_hex_string()
    );
    println!(
        "btc_taker_commitment={}",
        btc.taker_commitment.to_lower_hex_string()
    );
    println!(
        "lez_maker_commitment={}",
        lez.maker_commitment.to_lower_hex_string()
    );
    println!(
        "lez_taker_commitment={}",
        lez.taker_commitment.to_lower_hex_string()
    );
    println!(
        "btc_presignature={}",
        btc.presignature.to_lower_hex_string()
    );
    println!(
        "lez_presignature={}",
        lez.presignature.to_lower_hex_string()
    );
    println!(
        "btc_final_signature={}",
        btc_signature.to_lower_hex_string()
    );
    println!(
        "lez_final_signature={}",
        lez_signature.to_lower_hex_string()
    );
    println!("btc_signed_txid={}", signed.compute_txid());
    println!("btc_signed_wtxid={}", signed.compute_wtxid());
    println!("btc_witness_items=1");
    println!("btc_witness_bytes=64");
    println!("actual_lez_submission=false");
    println!("durable_nonce_journal=false");
}

fn main() -> Result<(), Box<dyn Error>> {
    let adaptor_point = scalar(ADAPTOR_SECRET)?.base_point_mul().serialize();

    let contract = contract(BTC_MAKER_SECRET, BTC_TAKER_SECRET)?;
    let spend = cooperative_spend(&contract)?;
    let btc_message = spend.sighash_bytes();
    let lez_message =
        sha256::Hash::hash(b"fixture exact LEZ witnessed-claim public message v1").to_byte_array();
    let agreement = sha256::Hash::hash(
        &[
            b"lez-atomic-swaps/m3/dual-chain-agreement/v1".as_slice(),
            &contract.output_key_bytes(),
            &btc_message,
            &lez_message,
            &adaptor_point,
        ]
        .concat(),
    )
    .to_byte_array();

    let btc_context = AdaptorSessionContext::taproot(
        [public_key(BTC_MAKER_SECRET)?, public_key(BTC_TAKER_SECRET)?],
        contract.merkle_root_bytes(),
        btc_message,
        adaptor_point,
        session_id(&agreement, b"/btc-claim"),
    )?;
    if btc_context.output_key() != contract.output_key_bytes() {
        return Err("MuSig2 Taproot output key differs from transaction contract".into());
    }
    if matches!(contract.output_key_parity(), OutputKeyParity::Even)
        != btc_context.output_key_has_even_y()
    {
        return Err("Taproot output parity mismatch".into());
    }
    let lez_context = AdaptorSessionContext::untweaked(
        [public_key(LEZ_MAKER_SECRET)?, public_key(LEZ_TAKER_SECRET)?],
        lez_message,
        adaptor_point,
        session_id(&agreement, b"/lez-claim"),
    )?;

    let btc = complete_session(&btc_context, BTC_MAKER_SECRET, BTC_TAKER_SECRET)?;
    let lez = complete_session(&lez_context, LEZ_MAKER_SECRET, LEZ_TAKER_SECRET)?;

    let lez_first = adapt_presignature(
        &lez_context,
        lez.presignature,
        Zeroizing::new(ADAPTOR_SECRET),
    )?;
    let extracted_from_lez = extract_adaptor_secret(&lez_context, lez.presignature, lez_first)?;
    let btc_after_lez = adapt_presignature(&btc_context, btc.presignature, extracted_from_lez)?;

    let btc_first = adapt_presignature(
        &btc_context,
        btc.presignature,
        Zeroizing::new(ADAPTOR_SECRET),
    )?;
    let extracted_from_btc = extract_adaptor_secret(&btc_context, btc.presignature, btc_first)?;
    let lez_after_btc = adapt_presignature(&lez_context, lez.presignature, extracted_from_btc)?;

    if btc_first != btc_after_lez || lez_first != lez_after_btc {
        return Err("direction reveal order changed a final signature".into());
    }
    verify_final_signature(&btc_context, btc_first)?;
    verify_final_signature(&lez_context, lez_first)?;

    let signed = spend.finalize(btc_first)?;
    print_result(
        &btc_context,
        &lez_context,
        &btc,
        &lez,
        &btc_first,
        &lez_first,
        &signed,
    );
    Ok(())
}
