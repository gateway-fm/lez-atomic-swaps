use std::env;
use std::process::ExitCode;
use std::str::FromStr;

use bitcoin::address::NetworkUnchecked;
use bitcoin::consensus::encode::serialize_hex;
use bitcoin::hashes::Hash;
use bitcoin::hex::DisplayHex;
use bitcoin::key::{Keypair, TapTweak, TweakedPublicKey};
use bitcoin::secp256k1::{Message, Secp256k1, SecretKey};
use bitcoin::sighash::{Prevouts, SighashCache, TapSighashType};
use bitcoin::taproot::{self, TapNodeHash};
use bitcoin::{
    Address, Amount, Network, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid,
    Witness, absolute, transaction,
};
use lez_btc_swap_sdk::{
    CooperativeKeyPathSpend, CsvBlockDelay, OutputKeyParity, P2trSwapOutput, RefundXOnlyKey,
    TwoPartyAggregateKey,
};

const CONTRACT_VALUE_SAT: u64 = 100_000_000;
const FIXED_FEE_SAT: u64 = 1_000;
const REFUND_DELAY_BLOCKS: u32 = 144;

fn fixture_secret(scalar: u8) -> Result<SecretKey, String> {
    let mut bytes = [0_u8; 32];
    bytes[31] = scalar;
    SecretKey::from_slice(&bytes).map_err(|error| format!("invalid fixture scalar: {error}"))
}

fn fixture_contract() -> Result<(P2trSwapOutput, Keypair), String> {
    let secp = Secp256k1::new();
    let internal_keypair = Keypair::from_secret_key(&secp, &fixture_secret(1)?);
    let refund_keypair = Keypair::from_secret_key(&secp, &fixture_secret(2)?);
    let (internal_key, _) = internal_keypair.x_only_public_key();
    let (refund_key, _) = refund_keypair.x_only_public_key();
    let contract = P2trSwapOutput::new(
        TwoPartyAggregateKey::from_bytes(internal_key.serialize())
            .map_err(|error| error.to_string())?,
        RefundXOnlyKey::from_bytes(refund_key.serialize()).map_err(|error| error.to_string())?,
        CsvBlockDelay::new(REFUND_DELAY_BLOCKS).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    Ok((contract, internal_keypair))
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

fn contract_command() -> Result<(), String> {
    let (contract, _) = fixture_contract()?;
    let script_pubkey = contract_script(&contract);
    let address = Address::from_script(&script_pubkey, Network::Regtest)
        .map_err(|error| format!("contract address encoding failed: {error}"))?;
    let parity = match contract.output_key_parity() {
        OutputKeyParity::Even => "even",
        OutputKeyParity::Odd => "odd",
    };
    println!(
        r#"{{"schema_version":1,"kind":"p2tr_contract","fixture_only":true,"fixture_authority":"known_regtest_scalar_1_not_musig2","network":"regtest","internal_key":"{internal_key}","refund_key":"{refund_key}","csv_blocks":{csv_blocks},"refund_script":"{refund_script}","leaf_version":{leaf_version},"tapleaf_hash":"{tapleaf_hash}","merkle_root":"{merkle_root}","tap_tweak_hash":"{tap_tweak_hash}","output_key":"{output_key}","output_key_parity":"{parity}","control_block":"{control_block}","script_pubkey":"{script_pubkey_hex}","address":"{address}"}}"#,
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
    let (contract, _) = fixture_contract()?;
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
    let (contract, internal_keypair) = fixture_contract()?;
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
    let secp = Secp256k1::new();
    let merkle_root = TapNodeHash::from_byte_array(contract.merkle_root_bytes());
    let tweaked_keypair = internal_keypair.tap_tweak(&secp, Some(merkle_root));
    let signature =
        secp.sign_schnorr_no_aux_rand(&Message::from_digest(sighash), tweaked_keypair.as_keypair());
    let transaction = spend
        .finalize(signature.serialize())
        .map_err(|error| error.to_string())?;

    println!(
        r#"{{"schema_version":1,"kind":"p2tr_cooperative_spend","fixture_only":true,"network":"regtest","funding_txid":"{funding_txid}","funding_vout":{funding_vout},"funding_value_sat":{funding_value_sat},"destination":"{destination}","output_value_sat":{output_value_sat},"fee_sat":{fee_sat},"sighash":"{sighash}","unsigned_transaction":"{unsigned_transaction}","raw_transaction":"{raw_transaction}","txid":"{txid}","wtxid":"{wtxid}","witness_items":1,"witness_bytes":64,"sighash_type":"DEFAULT","annex":false}}"#,
        fee_sat = FIXED_FEE_SAT,
        sighash = sighash.to_lower_hex_string(),
        raw_transaction = serialize_hex(&transaction),
        txid = transaction.compute_txid(),
        wtxid = transaction.compute_wtxid(),
    );
    Ok(())
}

fn next_arg(args: &mut impl Iterator<Item = String>, name: &str) -> Result<String, String> {
    args.next().ok_or_else(|| format!("missing {name}"))
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
        _ => Err("command must be contract, fund, or spend".to_owned()),
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
