use std::env;
use std::process::ExitCode;
use std::str::FromStr;

use bitcoin::address::NetworkUnchecked;
use bitcoin::consensus::encode::serialize_hex;
use bitcoin::hashes::{Hash, sha256};
use bitcoin::hex::DisplayHex;
use bitcoin::key::{Keypair, TweakedPublicKey};
use bitcoin::secp256k1::{Message, Secp256k1, SecretKey};
use bitcoin::sighash::{Prevouts, SighashCache, TapSighashType};
use bitcoin::taproot;
use bitcoin::{
    Address, Amount, Network, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid,
    Witness, absolute, transaction,
};
use lez_btc_swap_sdk::{
    AdaptorSessionContext, CooperativeKeyPathSpend, CsvBlockDelay, OutputKeyParity, P2trSwapOutput,
    RefundXOnlyKey, TwoPartyAggregateKey,
};
use musig2::secp::{MaybeScalar, Point, Scalar};
use musig2::{
    AdaptorSignature, FirstRound, KeyAggContext, LiftedSignature, PartialSignature, SecNonceSpices,
};
use zeroize::Zeroize;

const CONTRACT_VALUE_SAT: u64 = 100_000_000;
const FIXED_FEE_SAT: u64 = 1_000;
const REFUND_DELAY_BLOCKS: u32 = 144;
const MAKER_INDEX: usize = 0;
const TAKER_INDEX: usize = 1;
const ADAPTOR_DOMAIN_TAG: &[u8] = b"lez-atomic-swaps/m3/btc-claim/v1";
const MAKER_ROLE_TAG: u8 = 0;
const TAKER_ROLE_TAG: u8 = 1;

struct NonceRounds {
    maker: FirstRound,
    taker: FirstRound,
    maker_commitment: [u8; 32],
    taker_commitment: [u8; 32],
}

struct AdaptorTranscript {
    maker_nonce_commitment: [u8; 32],
    taker_nonce_commitment: [u8; 32],
    presignature: AdaptorSignature,
    final_signature: [u8; 64],
    extracted_scalar: [u8; 32],
}

fn fixture_secret(scalar: u8) -> Result<SecretKey, String> {
    let mut bytes = [0_u8; 32];
    bytes[31] = scalar;
    SecretKey::from_slice(&bytes).map_err(|error| format!("invalid fixture scalar: {error}"))
}

fn fixture_musig_secret(byte: u8) -> Result<Scalar, String> {
    let mut bytes = [byte; 32];
    let secret = Scalar::from_slice(&bytes)
        .map_err(|error| format!("invalid MuSig2 fixture scalar: {error}"))?;
    bytes.zeroize();
    Ok(secret)
}

fn fixture_contract() -> Result<(P2trSwapOutput, KeyAggContext, Scalar, Scalar, Point), String> {
    let maker_secret = fixture_musig_secret(0x31)?;
    let taker_secret = fixture_musig_secret(0x42)?;
    let maker_public = maker_secret.base_point_mul();
    let taker_public = taker_secret.base_point_mul();
    let untweaked_context = KeyAggContext::new([maker_public, taker_public])
        .map_err(|error| format!("MuSig2 key aggregation failed: {error}"))?;
    let internal_point: Point = untweaked_context.aggregated_pubkey_untweaked();

    let secp = Secp256k1::new();
    let refund_keypair = Keypair::from_secret_key(&secp, &fixture_secret(2)?);
    let (refund_key, _) = refund_keypair.x_only_public_key();
    let contract = P2trSwapOutput::new(
        TwoPartyAggregateKey::from_bytes(internal_point.serialize_xonly())
            .map_err(|error| error.to_string())?,
        RefundXOnlyKey::from_bytes(refund_key.serialize()).map_err(|error| error.to_string())?,
        CsvBlockDelay::new(REFUND_DELAY_BLOCKS).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let signing_context = untweaked_context
        .with_taproot_tweak(&contract.merkle_root_bytes())
        .map_err(|error| format!("MuSig2 Taproot tweak failed: {error}"))?;
    let output_point: Point = signing_context.aggregated_pubkey();
    if output_point.serialize_xonly() != contract.output_key_bytes()
        || output_point.has_even_y()
            != matches!(contract.output_key_parity(), OutputKeyParity::Even)
    {
        return Err("MuSig2 Taproot output does not match rust-bitcoin".to_owned());
    }
    let adaptor_point = fixture_musig_secret(0x53)?.base_point_mul();
    Ok((
        contract,
        signing_context,
        maker_secret,
        taker_secret,
        adaptor_point,
    ))
}

fn contract_script(contract: &P2trSwapOutput) -> ScriptBuf {
    ScriptBuf::from_bytes(contract.script_pubkey_bytes().to_vec())
}

fn direct_output_script(keypair: &Keypair) -> ScriptBuf {
    let (output_key, _) = keypair.x_only_public_key();
    ScriptBuf::new_p2tr_tweaked(TweakedPublicKey::dangerous_assume_tweaked(output_key))
}

fn parse_txid(value: &str) -> Result<Txid, String> {
    Txid::from_str(value).map_err(|error| format!("invalid txid: {error}"))
}

fn parse_u32(label: &str, value: &str) -> Result<u32, String> {
    value
        .parse::<u32>()
        .map_err(|error| format!("invalid {label}: {error}"))
}

fn parse_u64(label: &str, value: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .map_err(|error| format!("invalid {label}: {error}"))
}

fn parse_canonical_hex<const N: usize>(label: &str, value: &str) -> Result<[u8; N], String> {
    if value.len() != N * 2
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!(
            "invalid {label}: expected {} lowercase hexadecimal characters",
            N * 2
        ));
    }
    let mut decoded = [0_u8; N];
    for (index, output) in decoded.iter_mut().enumerate() {
        let offset = index * 2;
        *output = u8::from_str_radix(&value[offset..offset + 2], 16)
            .map_err(|error| format!("invalid {label}: {error}"))?;
    }
    Ok(decoded)
}

struct PublicCooperativePlan {
    contract: P2trSwapOutput,
    spend: CooperativeKeyPathSpend,
    destination: Address,
    output_value_sat: u64,
    maker_public_key: [u8; 33],
    taker_public_key: [u8; 33],
    adaptor_point: [u8; 33],
}

fn public_cooperative_plan(
    funding_txid: Txid,
    funding_vout: u32,
    funding_value_sat: u64,
    destination: &str,
) -> Result<PublicCooperativePlan, String> {
    let destination = destination
        .parse::<Address<NetworkUnchecked>>()
        .map_err(|error| format!("invalid destination address: {error}"))?
        .require_network(Network::Regtest)
        .map_err(|error| format!("destination is not Regtest: {error}"))?;
    let output_value_sat = funding_value_sat
        .checked_sub(FIXED_FEE_SAT)
        .ok_or_else(|| "contract value cannot cover cooperative fee".to_owned())?;
    let (contract, _, maker_secret, taker_secret, adaptor_point) = fixture_contract()?;
    let spend = CooperativeKeyPathSpend::new(
        &contract,
        OutPoint {
            txid: funding_txid,
            vout: funding_vout,
        },
        Amount::from_sat(funding_value_sat),
        vec![TxOut {
            value: Amount::from_sat(output_value_sat),
            script_pubkey: destination.script_pubkey(),
        }],
    )
    .map_err(|error| error.to_string())?;
    Ok(PublicCooperativePlan {
        contract,
        spend,
        destination,
        output_value_sat,
        maker_public_key: maker_secret.base_point_mul().serialize(),
        taker_public_key: taker_secret.base_point_mul().serialize(),
        adaptor_point: adaptor_point.serialize(),
    })
}

fn contract_command() -> Result<(), String> {
    let (contract, _, maker_secret, taker_secret, adaptor_point) = fixture_contract()?;
    let script_pubkey = contract_script(&contract);
    let address = Address::from_script(&script_pubkey, Network::Regtest)
        .map_err(|error| format!("contract address encoding failed: {error}"))?;
    let parity = match contract.output_key_parity() {
        OutputKeyParity::Even => "even",
        OutputKeyParity::Odd => "odd",
    };
    println!(
        r#"{{"schema_version":1,"kind":"p2tr_contract","fixture_only":true,"fixture_authority":"two_party_musig2_adaptor_public_regtest_vector","signing_protocol":"BIP327_MUSIG2_SCHNORR_ADAPTOR","musig2_version":"0.4.1","signer_order":["maker","taker"],"maker_public_key":"{maker_public_key}","taker_public_key":"{taker_public_key}","adaptor_point":"{adaptor_point}","network":"regtest","internal_key":"{internal_key}","refund_key":"{refund_key}","csv_blocks":{csv_blocks},"refund_script":"{refund_script}","leaf_version":{leaf_version},"tapleaf_hash":"{tapleaf_hash}","merkle_root":"{merkle_root}","tap_tweak_hash":"{tap_tweak_hash}","output_key":"{output_key}","output_key_parity":"{parity}","control_block":"{control_block}","script_pubkey":"{script_pubkey_hex}","address":"{address}"}}"#,
        maker_public_key = maker_secret
            .base_point_mul()
            .serialize()
            .to_lower_hex_string(),
        taker_public_key = taker_secret
            .base_point_mul()
            .serialize()
            .to_lower_hex_string(),
        adaptor_point = adaptor_point.serialize().to_lower_hex_string(),
        internal_key = contract
            .aggregate_internal_key_bytes()
            .to_lower_hex_string(),
        refund_key = contract.refund_key_bytes().to_lower_hex_string(),
        csv_blocks = contract.refund_delay().blocks(),
        refund_script = contract.refund_script_bytes().to_lower_hex_string(),
        leaf_version = contract.refund_leaf_version(),
        tapleaf_hash = contract.tapleaf_hash_bytes().to_lower_hex_string(),
        merkle_root = contract.merkle_root_bytes().to_lower_hex_string(),
        tap_tweak_hash = contract.tap_tweak_hash_bytes().to_lower_hex_string(),
        output_key = contract.output_key_bytes().to_lower_hex_string(),
        control_block = contract.refund_control_block_bytes().to_lower_hex_string(),
        script_pubkey_hex = script_pubkey.as_bytes().to_lower_hex_string(),
    );
    Ok(())
}

fn funding_command(txid: Txid, vout: u32, funding_value_sat: u64) -> Result<(), String> {
    if funding_value_sat > Amount::MAX_MONEY.to_sat() {
        return Err("funding value exceeds Bitcoin MAX_MONEY".to_owned());
    }
    let required = CONTRACT_VALUE_SAT
        .checked_add(FIXED_FEE_SAT)
        .ok_or_else(|| "funding requirement overflow".to_owned())?;
    let change_value_sat = funding_value_sat
        .checked_sub(required)
        .ok_or_else(|| "funding input cannot cover contract value and fee".to_owned())?;
    if change_value_sat == 0 {
        return Err("funding fixture requires a nonzero change output".to_owned());
    }

    let secp = Secp256k1::new();
    let funding_keypair = Keypair::from_secret_key(&secp, &fixture_secret(1)?);
    let funding_script = direct_output_script(&funding_keypair);
    let (contract, ..) = fixture_contract()?;
    let mut transaction = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint { txid, vout },
            script_sig: ScriptBuf::new(),
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            witness: Witness::new(),
        }],
        output: vec![
            TxOut {
                value: Amount::from_sat(CONTRACT_VALUE_SAT),
                script_pubkey: contract_script(&contract),
            },
            TxOut {
                value: Amount::from_sat(change_value_sat),
                script_pubkey: funding_script.clone(),
            },
        ],
    };
    let prevouts = [TxOut {
        value: Amount::from_sat(funding_value_sat),
        script_pubkey: funding_script,
    }];
    let sighash = SighashCache::new(&transaction)
        .taproot_key_spend_signature_hash(0, &Prevouts::All(&prevouts), TapSighashType::Default)
        .map_err(|error| format!("funding sighash failed: {error}"))?;
    let signature = secp.sign_schnorr_no_aux_rand(
        &Message::from_digest(sighash.to_byte_array()),
        &funding_keypair,
    );
    let taproot_signature = taproot::Signature::from_slice(&signature.serialize())
        .map_err(|error| format!("funding signature encoding failed: {error}"))?;
    transaction.input[0].witness = Witness::p2tr_key_spend(&taproot_signature);

    println!(
        r#"{{"schema_version":1,"kind":"p2tr_funding_transaction","fixture_only":true,"network":"regtest","input_txid":"{input_txid}","input_vout":{input_vout},"input_value_sat":{input_value_sat},"contract_vout":0,"contract_value_sat":{contract_value_sat},"change_vout":1,"change_value_sat":{change_value_sat},"fee_sat":{fee_sat},"sighash":"{sighash}","raw_transaction":"{raw_transaction}","txid":"{txid}","wtxid":"{wtxid}","witness_items":1,"witness_bytes":64}}"#,
        input_txid = txid,
        input_vout = vout,
        input_value_sat = funding_value_sat,
        contract_value_sat = CONTRACT_VALUE_SAT,
        fee_sat = FIXED_FEE_SAT,
        sighash = sighash.to_byte_array().to_lower_hex_string(),
        raw_transaction = serialize_hex(&transaction),
        txid = transaction.compute_txid(),
        wtxid = transaction.compute_wtxid(),
    );
    Ok(())
}

fn nonce_commitment(
    role: u8,
    session_domain: &[u8; 32],
    message: &[u8; 32],
    public_nonce: &[u8; 66],
) -> [u8; 32] {
    let mut preimage = Vec::with_capacity(
        ADAPTOR_DOMAIN_TAG.len() + 1 + session_domain.len() + message.len() + 66,
    );
    preimage.extend_from_slice(ADAPTOR_DOMAIN_TAG);
    preimage.push(role);
    preimage.extend_from_slice(session_domain);
    preimage.extend_from_slice(message);
    preimage.extend_from_slice(public_nonce);
    sha256::Hash::hash(&preimage).to_byte_array()
}
fn adaptor_session_domain(contract: &P2trSwapOutput, sighash: &[u8; 32]) -> [u8; 32] {
    sha256::Hash::hash(
        &[
            ADAPTOR_DOMAIN_TAG,
            &contract.aggregate_internal_key_bytes(),
            &contract.merkle_root_bytes(),
            &contract.output_key_bytes(),
            sighash,
        ]
        .concat(),
    )
    .to_byte_array()
}

fn begin_nonce_rounds(
    signing_context: &KeyAggContext,
    maker_secret: Scalar,
    taker_secret: Scalar,
    sighash: &[u8; 32],
    session_domain: &[u8; 32],
) -> Result<NonceRounds, String> {
    let maker_spices = SecNonceSpices::new()
        .with_seckey(maker_secret)
        .with_message(sighash)
        .with_extra_input(session_domain);
    let taker_spices = SecNonceSpices::new()
        .with_seckey(taker_secret)
        .with_message(sighash)
        .with_extra_input(session_domain);
    let mut maker_nonce_seed = [0x71; 32];
    let mut taker_nonce_seed = [0x82; 32];
    let mut maker = FirstRound::new(
        signing_context.clone(),
        maker_nonce_seed,
        MAKER_INDEX,
        maker_spices,
    )
    .map_err(|error| format!("maker MuSig2 first round failed: {error}"))?;
    let mut taker = FirstRound::new(
        signing_context.clone(),
        taker_nonce_seed,
        TAKER_INDEX,
        taker_spices,
    )
    .map_err(|error| format!("taker MuSig2 first round failed: {error}"))?;
    maker_nonce_seed.zeroize();
    taker_nonce_seed.zeroize();

    let maker_nonce = maker.our_public_nonce();
    let taker_nonce = taker.our_public_nonce();
    let maker_commitment = nonce_commitment(
        MAKER_ROLE_TAG,
        session_domain,
        sighash,
        &maker_nonce.serialize(),
    );
    let taker_commitment = nonce_commitment(
        TAKER_ROLE_TAG,
        session_domain,
        sighash,
        &taker_nonce.serialize(),
    );
    maker
        .receive_nonce(TAKER_INDEX, taker_nonce)
        .map_err(|error| format!("maker rejected taker public nonce: {error}"))?;
    taker
        .receive_nonce(MAKER_INDEX, maker_nonce)
        .map_err(|error| format!("taker rejected maker public nonce: {error}"))?;

    Ok(NonceRounds {
        maker,
        taker,
        maker_commitment,
        taker_commitment,
    })
}

fn complete_adaptor_transcript(
    rounds: NonceRounds,
    signing_context: &KeyAggContext,
    maker_secret: Scalar,
    taker_secret: Scalar,
    adaptor_point: Point,
    sighash: [u8; 32],
) -> Result<AdaptorTranscript, String> {
    let mut maker = rounds
        .maker
        .finalize_adaptor(maker_secret, adaptor_point, sighash)
        .map_err(|error| format!("maker adaptor partial failed: {error}"))?;
    let mut taker = rounds
        .taker
        .finalize_adaptor(taker_secret, adaptor_point, sighash)
        .map_err(|error| format!("taker adaptor partial failed: {error}"))?;
    let maker_partial: PartialSignature = maker.our_signature();
    let taker_partial: PartialSignature = taker.our_signature();
    maker
        .receive_signature(TAKER_INDEX, taker_partial)
        .map_err(|error| format!("maker rejected taker partial: {error}"))?;
    taker
        .receive_signature(MAKER_INDEX, maker_partial)
        .map_err(|error| format!("taker rejected maker partial: {error}"))?;
    let maker_presignature = maker
        .finalize_adaptor::<AdaptorSignature>()
        .map_err(|error| format!("maker adaptor aggregation failed: {error}"))?;
    let taker_presignature = taker
        .finalize_adaptor::<AdaptorSignature>()
        .map_err(|error| format!("taker adaptor aggregation failed: {error}"))?;
    if maker_presignature != taker_presignature {
        return Err("actors derived different adaptor presignatures".to_owned());
    }

    let output_point: Point = signing_context.aggregated_pubkey();
    musig2::adaptor::verify_single(output_point, &maker_presignature, sighash, adaptor_point)
        .map_err(|error| format!("aggregate adaptor verification failed: {error}"))?;
    let final_signature: LiftedSignature = maker_presignature
        .adapt(fixture_musig_secret(0x53)?)
        .ok_or_else(|| "adaptor secret produced an invalid final nonce".to_owned())?;
    musig2::verify_single(output_point, final_signature, sighash)
        .map_err(|error| format!("final MuSig2 signature verification failed: {error}"))?;
    let extracted: MaybeScalar = maker_presignature
        .reveal_secret(&final_signature)
        .ok_or_else(|| "failed to extract adaptor scalar".to_owned())?;
    let expected_adaptor_secret = fixture_musig_secret(0x53)?;
    if extracted != MaybeScalar::Valid(expected_adaptor_secret) {
        return Err("extracted adaptor scalar differs from fixture".to_owned());
    }
    let extracted_scalar: [u8; 32] = extracted.into();
    if Scalar::from_slice(&extracted_scalar)
        .map_err(|error| format!("invalid extracted scalar: {error}"))?
        .base_point_mul()
        != adaptor_point
    {
        return Err("extracted scalar does not match adaptor point".to_owned());
    }

    Ok(AdaptorTranscript {
        maker_nonce_commitment: rounds.maker_commitment,
        taker_nonce_commitment: rounds.taker_commitment,
        presignature: maker_presignature,
        final_signature: final_signature.serialize(),
        extracted_scalar,
    })
}

fn spend_command(
    funding_txid: Txid,
    funding_vout: u32,
    funding_value_sat: u64,
    destination: &str,
) -> Result<(), String> {
    let destination = destination
        .parse::<Address<NetworkUnchecked>>()
        .map_err(|error| format!("invalid destination address: {error}"))?
        .require_network(Network::Regtest)
        .map_err(|error| format!("destination is not Regtest: {error}"))?;
    let output_value_sat = funding_value_sat
        .checked_sub(FIXED_FEE_SAT)
        .ok_or_else(|| "contract value cannot cover cooperative fee".to_owned())?;
    let (contract, signing_context, maker_secret, taker_secret, adaptor_point) =
        fixture_contract()?;
    let spend = CooperativeKeyPathSpend::new(
        &contract,
        OutPoint {
            txid: funding_txid,
            vout: funding_vout,
        },
        Amount::from_sat(funding_value_sat),
        vec![TxOut {
            value: Amount::from_sat(output_value_sat),
            script_pubkey: destination.script_pubkey(),
        }],
    )
    .map_err(|error| error.to_string())?;
    let sighash = spend.sighash_bytes();
    let unsigned_transaction = serialize_hex(spend.unsigned_transaction());
    let session_domain = adaptor_session_domain(&contract, &sighash);
    let nonce_rounds = begin_nonce_rounds(
        &signing_context,
        maker_secret,
        taker_secret,
        &sighash,
        &session_domain,
    )?;
    let transcript = complete_adaptor_transcript(
        nonce_rounds,
        &signing_context,
        maker_secret,
        taker_secret,
        adaptor_point,
        sighash,
    )?;
    let transaction = spend
        .finalize(transcript.final_signature)
        .map_err(|error| error.to_string())?;

    println!(
        r#"{{"schema_version":1,"kind":"p2tr_cooperative_spend","fixture_only":true,"fixture_authority":"two_party_musig2_adaptor_public_regtest_vector","signing_protocol":"BIP327_MUSIG2_SCHNORR_ADAPTOR","musig2_version":"0.4.1","signer_order":["maker","taker"],"nonce_commitment_scheme":"SHA256_domain_role_session_message_pubnonce","maker_nonce_commitment":"{maker_nonce_commitment}","taker_nonce_commitment":"{taker_nonce_commitment}","adaptor_point":"{adaptor_point}","adaptor_presignature":"{adaptor_presignature}","adaptor_presignature_bytes":65,"adaptor_presignature_verified":true,"final_signature":"{final_signature}","final_signature_verified_under_q":true,"extracted_scalar":"{extracted_scalar}","extracted_scalar_public_fixture":true,"extracted_point_matches":true,"network":"regtest","funding_txid":"{funding_txid}","funding_vout":{funding_vout},"funding_value_sat":{funding_value_sat},"destination":"{destination}","output_value_sat":{output_value_sat},"fee_sat":{fee_sat},"sighash":"{sighash}","unsigned_transaction":"{unsigned_transaction}","raw_transaction":"{raw_transaction}","txid":"{txid}","wtxid":"{wtxid}","witness_items":1,"witness_bytes":64,"sighash_type":"DEFAULT","annex":false}}"#,
        maker_nonce_commitment = transcript.maker_nonce_commitment.to_lower_hex_string(),
        taker_nonce_commitment = transcript.taker_nonce_commitment.to_lower_hex_string(),
        adaptor_point = adaptor_point.serialize().to_lower_hex_string(),
        adaptor_presignature = transcript.presignature.serialize().to_lower_hex_string(),
        final_signature = transcript.final_signature.to_lower_hex_string(),
        extracted_scalar = transcript.extracted_scalar.to_lower_hex_string(),
        fee_sat = FIXED_FEE_SAT,
        sighash = sighash.to_lower_hex_string(),
        raw_transaction = serialize_hex(&transaction),
        txid = transaction.compute_txid(),
        wtxid = transaction.compute_wtxid(),
    );
    Ok(())
}

fn plan_spend_command(
    funding_txid: Txid,
    funding_vout: u32,
    funding_value_sat: u64,
    destination: &str,
) -> Result<(), String> {
    let plan = public_cooperative_plan(funding_txid, funding_vout, funding_value_sat, destination)?;
    let contract_address = Address::from_script(&contract_script(&plan.contract), Network::Regtest)
        .map_err(|error| format!("contract address encoding failed: {error}"))?;
    let parity = match plan.contract.output_key_parity() {
        OutputKeyParity::Even => "even",
        OutputKeyParity::Odd => "odd",
    };
    println!(
        r#"{{"schema_version":1,"kind":"p2tr_cooperative_spend_plan","fixture_only":true,"fixture_authority":"two_party_musig2_adaptor_public_regtest_vector","signing_protocol":"BIP327_MUSIG2_SCHNORR_ADAPTOR","musig2_version":"0.4.1","signer_order":["maker","taker"],"role_runner_context":"btc_taproot","maker_public_key":"{maker_public_key}","taker_public_key":"{taker_public_key}","adaptor_point":"{adaptor_point}","network":"regtest","funding_txid":"{funding_txid}","funding_vout":{funding_vout},"funding_value_sat":{funding_value_sat},"destination":"{destination}","output_value_sat":{output_value_sat},"fee_sat":{fee_sat},"internal_key":"{internal_key}","refund_key":"{refund_key}","csv_blocks":{csv_blocks},"refund_script":"{refund_script}","leaf_version":{leaf_version},"tapleaf_hash":"{tapleaf_hash}","merkle_root":"{merkle_root}","tap_tweak_hash":"{tap_tweak_hash}","output_key":"{output_key}","output_key_parity":"{parity}","control_block":"{control_block}","script_pubkey":"{script_pubkey}","contract_address":"{contract_address}","sighash":"{sighash}","unsigned_transaction":"{unsigned_transaction}","sighash_type":"DEFAULT","annex":false}}"#,
        maker_public_key = plan.maker_public_key.to_lower_hex_string(),
        taker_public_key = plan.taker_public_key.to_lower_hex_string(),
        adaptor_point = plan.adaptor_point.to_lower_hex_string(),
        destination = plan.destination,
        output_value_sat = plan.output_value_sat,
        fee_sat = plan.spend.fee().to_sat(),
        internal_key = plan
            .contract
            .aggregate_internal_key_bytes()
            .to_lower_hex_string(),
        refund_key = plan.contract.refund_key_bytes().to_lower_hex_string(),
        csv_blocks = plan.contract.refund_delay().blocks(),
        refund_script = plan.contract.refund_script_bytes().to_lower_hex_string(),
        leaf_version = plan.contract.refund_leaf_version(),
        tapleaf_hash = plan.contract.tapleaf_hash_bytes().to_lower_hex_string(),
        merkle_root = plan.contract.merkle_root_bytes().to_lower_hex_string(),
        tap_tweak_hash = plan.contract.tap_tweak_hash_bytes().to_lower_hex_string(),
        output_key = plan.contract.output_key_bytes().to_lower_hex_string(),
        control_block = plan
            .contract
            .refund_control_block_bytes()
            .to_lower_hex_string(),
        script_pubkey = plan.contract.script_pubkey_bytes().to_lower_hex_string(),
        sighash = plan.spend.sighash_bytes().to_lower_hex_string(),
        unsigned_transaction = serialize_hex(plan.spend.unsigned_transaction()),
    );
    Ok(())
}

fn btc_session_json(plan: &PublicCooperativePlan, session_id: [u8; 32]) -> Result<String, String> {
    let message = plan.spend.sighash_bytes();
    let merkle_root = plan.contract.merkle_root_bytes();
    let context = AdaptorSessionContext::taproot(
        [plan.maker_public_key, plan.taker_public_key],
        merkle_root,
        message,
        plan.adaptor_point,
        session_id,
    )
    .map_err(|error| format!("invalid BTC adaptor session: {error}"))?;
    if context.output_key() != plan.contract.output_key_bytes()
        || context.output_key_has_even_y()
            != matches!(plan.contract.output_key_parity(), OutputKeyParity::Even)
    {
        return Err("role-runner BTC session does not reproduce contract output key Q".to_owned());
    }
    Ok(format!(
        r#"{{"schema_version":1,"context":{{"kind":"btc_taproot","merkle_root":"{merkle_root}"}},"session_id":"{session_id}","exact_message":"{message}","adaptor_point":"{adaptor_point}","maker_public_key":"{maker_public_key}","taker_public_key":"{taker_public_key}"}}"#,
        merkle_root = merkle_root.to_lower_hex_string(),
        session_id = session_id.to_lower_hex_string(),
        message = message.to_lower_hex_string(),
        adaptor_point = plan.adaptor_point.to_lower_hex_string(),
        maker_public_key = plan.maker_public_key.to_lower_hex_string(),
        taker_public_key = plan.taker_public_key.to_lower_hex_string(),
    ))
}

fn btc_session_command(
    funding_txid: Txid,
    funding_vout: u32,
    funding_value_sat: u64,
    destination: &str,
    session_id: [u8; 32],
) -> Result<(), String> {
    let plan = public_cooperative_plan(funding_txid, funding_vout, funding_value_sat, destination)?;
    println!("{}", btc_session_json(&plan, session_id)?);
    Ok(())
}

fn finalize_spend_command(
    funding_txid: Txid,
    funding_vout: u32,
    funding_value_sat: u64,
    destination: &str,
    final_signature: [u8; 64],
) -> Result<(), String> {
    let plan = public_cooperative_plan(funding_txid, funding_vout, funding_value_sat, destination)?;
    let sighash = plan.spend.sighash_bytes();
    let unsigned_transaction = serialize_hex(plan.spend.unsigned_transaction());
    let output_key = plan.contract.output_key_bytes();
    let destination = plan.destination;
    let output_value_sat = plan.output_value_sat;
    let transaction = plan
        .spend
        .finalize(final_signature)
        .map_err(|error| error.to_string())?;
    println!(
        r#"{{"schema_version":1,"kind":"p2tr_cooperative_spend_finalized","fixture_only":true,"fixture_authority":"two_party_musig2_adaptor_public_regtest_vector","signing_protocol":"BIP327_MUSIG2_SCHNORR_ADAPTOR","network":"regtest","funding_txid":"{funding_txid}","funding_vout":{funding_vout},"funding_value_sat":{funding_value_sat},"destination":"{destination}","output_value_sat":{output_value_sat},"fee_sat":{fee_sat},"output_key":"{output_key}","sighash":"{sighash}","unsigned_transaction":"{unsigned_transaction}","final_signature":"{final_signature}","final_signature_verified_under_q":true,"raw_transaction":"{raw_transaction}","txid":"{txid}","wtxid":"{wtxid}","witness_items":1,"witness_bytes":64,"sighash_type":"DEFAULT","annex":false}}"#,
        fee_sat = FIXED_FEE_SAT,
        output_key = output_key.to_lower_hex_string(),
        sighash = sighash.to_lower_hex_string(),
        final_signature = final_signature.to_lower_hex_string(),
        raw_transaction = serialize_hex(&transaction),
        txid = transaction.compute_txid(),
        wtxid = transaction.compute_wtxid(),
    );
    Ok(())
}

fn lez_session_json(
    session_id: [u8; 32],
    message: [u8; 32],
    adaptor_point: [u8; 33],
) -> Result<String, String> {
    let (_, _, maker_secret, taker_secret, _) = fixture_contract()?;
    let maker_public_key = maker_secret.base_point_mul().serialize();
    let taker_public_key = taker_secret.base_point_mul().serialize();
    AdaptorSessionContext::untweaked(
        [maker_public_key, taker_public_key],
        message,
        adaptor_point,
        session_id,
    )
    .map_err(|error| format!("invalid LEZ adaptor session: {error}"))?;
    Ok(format!(
        r#"{{"schema_version":1,"context":{{"kind":"lez_untweaked"}},"session_id":"{session_id}","exact_message":"{message}","adaptor_point":"{adaptor_point}","maker_public_key":"{maker_public_key}","taker_public_key":"{taker_public_key}"}}"#,
        session_id = session_id.to_lower_hex_string(),
        message = message.to_lower_hex_string(),
        adaptor_point = adaptor_point.to_lower_hex_string(),
        maker_public_key = maker_public_key.to_lower_hex_string(),
        taker_public_key = taker_public_key.to_lower_hex_string(),
    ))
}

fn lez_session_command(
    session_id: [u8; 32],
    message: [u8; 32],
    adaptor_point: [u8; 33],
) -> Result<(), String> {
    println!("{}", lez_session_json(session_id, message, adaptor_point)?);
    Ok(())
}

fn next_arg(args: &mut impl Iterator<Item = String>, name: &str) -> Result<String, String> {
    args.next().ok_or_else(|| format!("missing {name}"))
}

fn run_public_command(
    command: &str,
    mut args: &mut impl Iterator<Item = String>,
) -> Result<(), String> {
    match command {
        "plan-spend" => {
            let txid = parse_txid(&next_arg(&mut args, "funding txid")?)?;
            let vout = parse_u32("funding vout", &next_arg(&mut args, "funding vout")?)?;
            let value_sat = parse_u64(
                "funding value_sat",
                &next_arg(&mut args, "funding value_sat")?,
            )?;
            let destination = next_arg(&mut args, "Regtest destination")?;
            if args.next().is_some() {
                return Err(
                    "plan-spend accepts exactly txid, vout, value_sat, and destination".to_owned(),
                );
            }
            plan_spend_command(txid, vout, value_sat, &destination)
        }
        "btc-session" => {
            let txid = parse_txid(&next_arg(&mut args, "funding txid")?)?;
            let vout = parse_u32("funding vout", &next_arg(&mut args, "funding vout")?)?;
            let value_sat = parse_u64(
                "funding value_sat",
                &next_arg(&mut args, "funding value_sat")?,
            )?;
            let destination = next_arg(&mut args, "Regtest destination")?;
            let session_id = parse_canonical_hex(
                "session_id",
                &next_arg(&mut args, "32-byte session_id")?,
            )?;
            if args.next().is_some() {
                return Err("btc-session accepts exactly txid, vout, value_sat, destination, and session_id".to_owned());
            }
            btc_session_command(txid, vout, value_sat, &destination, session_id)
        }
        "finalize-spend" => {
            let txid = parse_txid(&next_arg(&mut args, "funding txid")?)?;
            let vout = parse_u32("funding vout", &next_arg(&mut args, "funding vout")?)?;
            let value_sat = parse_u64(
                "funding value_sat",
                &next_arg(&mut args, "funding value_sat")?,
            )?;
            let destination = next_arg(&mut args, "Regtest destination")?;
            let final_signature = parse_canonical_hex(
                "final_signature",
                &next_arg(&mut args, "64-byte final_signature")?,
            )?;
            if args.next().is_some() {
                return Err("finalize-spend accepts exactly txid, vout, value_sat, destination, and final_signature".to_owned());
            }
            finalize_spend_command(txid, vout, value_sat, &destination, final_signature)
        }
        "lez-session" => {
            let session_id = parse_canonical_hex(
                "session_id",
                &next_arg(&mut args, "32-byte session_id")?,
            )?;
            let message = parse_canonical_hex(
                "message",
                &next_arg(&mut args, "32-byte message")?,
            )?;
            let adaptor_point = parse_canonical_hex(
                "adaptor_point",
                &next_arg(&mut args, "33-byte adaptor_point")?,
            )?;
            if args.next().is_some() {
                return Err(
                    "lez-session accepts exactly session_id, message, and adaptor_point".to_owned(),
                );
            }
            lez_session_command(session_id, message, adaptor_point)
        }
        _ => Err(
            "command must be contract, fund, spend, plan-spend, btc-session, finalize-spend, or lez-session"
                .to_owned(),
        ),
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let command = next_arg(&mut args, "command")?;
    match command.as_str() {
        "contract" => {
            if args.next().is_some() {
                return Err("contract accepts no arguments".to_owned());
            }
            contract_command()
        }
        "fund" => {
            let txid = parse_txid(&next_arg(&mut args, "coinbase txid")?)?;
            let vout = parse_u32("coinbase vout", &next_arg(&mut args, "coinbase vout")?)?;
            let value_sat = parse_u64(
                "coinbase value_sat",
                &next_arg(&mut args, "coinbase value_sat")?,
            )?;
            if args.next().is_some() {
                return Err("fund accepts exactly txid, vout, and value_sat".to_owned());
            }
            funding_command(txid, vout, value_sat)
        }
        "spend" => {
            let txid = parse_txid(&next_arg(&mut args, "funding txid")?)?;
            let vout = parse_u32("funding vout", &next_arg(&mut args, "funding vout")?)?;
            let value_sat = parse_u64(
                "funding value_sat",
                &next_arg(&mut args, "funding value_sat")?,
            )?;
            let destination = next_arg(&mut args, "Regtest destination")?;
            if args.next().is_some() {
                return Err(
                    "spend accepts exactly txid, vout, value_sat, and destination".to_owned(),
                );
            }
            spend_command(txid, vout, value_sat, &destination)
        }
        _ => run_public_command(&command, &mut args),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("btc-core-p2tr-fixture: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn destination() -> String {
        let secp = Secp256k1::new();
        let keypair = Keypair::from_secret_key(&secp, &fixture_secret(3).unwrap());
        Address::from_script(&direct_output_script(&keypair), Network::Regtest)
            .unwrap()
            .to_string()
    }

    fn public_plan() -> PublicCooperativePlan {
        public_cooperative_plan(
            Txid::from_byte_array([0x11; 32]),
            2,
            CONTRACT_VALUE_SAT,
            &destination(),
        )
        .unwrap()
    }

    #[test]
    fn role_runner_sessions_are_exact_canonical_public_json() {
        let plan = public_plan();
        let session_id = [0x22; 32];
        let btc = btc_session_json(&plan, session_id).unwrap();
        let expected_btc = format!(
            r#"{{"schema_version":1,"context":{{"kind":"btc_taproot","merkle_root":"{merkle_root}"}},"session_id":"{session_id}","exact_message":"{message}","adaptor_point":"{adaptor_point}","maker_public_key":"{maker_public_key}","taker_public_key":"{taker_public_key}"}}"#,
            merkle_root = plan.contract.merkle_root_bytes().to_lower_hex_string(),
            session_id = session_id.to_lower_hex_string(),
            message = plan.spend.sighash_bytes().to_lower_hex_string(),
            adaptor_point = plan.adaptor_point.to_lower_hex_string(),
            maker_public_key = plan.maker_public_key.to_lower_hex_string(),
            taker_public_key = plan.taker_public_key.to_lower_hex_string(),
        );
        assert_eq!(btc, expected_btc);
        assert!(!btc.contains('\n'));

        let message = [0x33; 32];
        let lez = lez_session_json(session_id, message, plan.adaptor_point).unwrap();
        let expected_lez = format!(
            r#"{{"schema_version":1,"context":{{"kind":"lez_untweaked"}},"session_id":"{session_id}","exact_message":"{message}","adaptor_point":"{adaptor_point}","maker_public_key":"{maker_public_key}","taker_public_key":"{taker_public_key}"}}"#,
            session_id = session_id.to_lower_hex_string(),
            message = message.to_lower_hex_string(),
            adaptor_point = plan.adaptor_point.to_lower_hex_string(),
            maker_public_key = plan.maker_public_key.to_lower_hex_string(),
            taker_public_key = plan.taker_public_key.to_lower_hex_string(),
        );
        assert_eq!(lez, expected_lez);
        assert!(!lez.contains('\n'));
    }

    #[test]
    fn external_final_signature_completes_the_exact_prepared_plan() {
        let plan = public_plan();
        let sighash = plan.spend.sighash_bytes();
        let (contract, signing_context, maker_secret, taker_secret, adaptor_point) =
            fixture_contract().unwrap();
        assert_eq!(contract, plan.contract);
        let session_domain = adaptor_session_domain(&contract, &sighash);
        let nonce_rounds = begin_nonce_rounds(
            &signing_context,
            maker_secret,
            taker_secret,
            &sighash,
            &session_domain,
        )
        .unwrap();
        let transcript = complete_adaptor_transcript(
            nonce_rounds,
            &signing_context,
            maker_secret,
            taker_secret,
            adaptor_point,
            sighash,
        )
        .unwrap();
        let unsigned_txid = plan.spend.unsigned_transaction().compute_txid();
        let transaction = plan.spend.finalize(transcript.final_signature).unwrap();
        assert_eq!(transaction.compute_txid(), unsigned_txid);
        assert_eq!(transaction.input[0].witness.len(), 1);
        assert_eq!(
            transaction.input[0].witness.iter().next().unwrap().len(),
            64
        );
    }

    #[test]
    fn public_hex_inputs_must_already_be_canonical() {
        assert_eq!(
            parse_canonical_hex::<32>("message", &"ab".repeat(32)).unwrap(),
            [0xab; 32]
        );
        assert!(parse_canonical_hex::<32>("message", &"AB".repeat(32)).is_err());
        assert!(parse_canonical_hex::<32>("message", "00").is_err());
    }
}
