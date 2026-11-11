//! Transactions mined on the public Zcash Testnet, checked against the same
//! librustzcash serialization and ZIP-244 signature hash the swap engine signs
//! with.
//!
//! Both are transparent-only v5 P2PKH spends by one key, fetched read-only
//! from Zebra 7.0.0 (`zcash-testnet-zebrad.gateway.tatum.io`,
//! `getrawtransaction <txid> 1`) on 2026-11-11:
//!
//! - `a85b0f6a…89ea`, mined at height 4,465,026, the NU7 activation block
//!   (consensus branch `0x77190ad9`), spending output 2 of `d759e629…7bb8`;
//! - `d759e629…7bb8`, mined at height 4,465,025, the last NU6.3 block
//!   (consensus branch `0x37a5165b`), spending output 2 of `a768b9fa…3b22`.
//!
//! A transaction reproduces its mined txid only if it is parsed and digested
//! exactly as consensus does; its signature verifies only if our signature
//! hash, including the branch id in its personalization, equals the one the
//! signer used and Zebra accepted.

use secp256k1::{Message, PublicKey, Secp256k1, ecdsa::Signature};
use zcash_primitives::transaction::{
    Authorization as TransactionAuthorization, Transaction,
    sighash::{PrecomputedSighashData, SignableInput as TransactionSignableInput},
};
use zcash_protocol::{consensus::BranchId, value::Zatoshis};
use zcash_script::{
    interpreter::{CallbackTransactionSignatureChecker, Flags},
    script::{Code, Raw},
    signature::{HashType, SignedOutputs},
};
use zcash_transparent::{
    address::Script,
    bundle::{Authorization as TransparentAuthorization, Bundle, TxIn, TxOut},
    sighash::{
        SighashType, SignableInput as TransparentSignableInput, TransparentAuthorizingContext,
    },
};

/// Mined at Testnet height 4,465,026 (NU7 activation).
const NU7_TX: &str = concat!(
    "050000800a27a726d90a197700000000fa21440001b87b4df4d035a31b018abd33efbed35c0e66163d25b3d8",
    "8979c75f0329e659d7020000006a473044022043c971e06555e2413276314bfc5d881487e4eaa749148f87f3",
    "ac55b2b8fe07f1022061d53ac6c9d6e053cca4ce5b072c34eb252683a4cbd14da6a5b0be1cc8e549e8012102",
    "bc0a8bf008166b8a99a1bcf27086172fd709467ea55b429449a3fe6a92f03738ffffffff0300000000000000",
    "001d6a1b535601bd8a560dfb415d4babb99662dc157267d166b7c90000000010270000000000001976a91400",
    "0000000000000000000000000000000000000088aca0aa762a000000001976a9141aded9db78a48344f8541f",
    "fa7d4300ada64fc68088ac000000",
);
const NU7_TXID: &str = "a85b0f6a352b9699d4221a93eeadbab457862c4dec2a3ac7ccd0d2f034fc89ea";
const NU7_PREVOUT_VALUE: u64 = 712_450_000;

/// Mined at Testnet height 4,465,025 (the last NU6.3 block).
const NU6_3_TX: &str = concat!(
    "050000800a27a7265b16a53700000000a921440001223b43649c4480eb2caffc49842cd971e40299b3185159",
    "ef015d843cfab968a7020000006b4830450221008dd6ef1527b7725b50573e3df4b214adaea643d0f14be5af",
    "c88d48f65dff11c102206ec7fc29ede35d228e75b8a29e8b15325797e18422b9e6a41b65ca4ba40058ee0121",
    "02bc0a8bf008166b8a99a1bcf27086172fd709467ea55b429449a3fe6a92f03738ffffffff03000000000000",
    "00001d6a1b535601bd8a560dfb415d4babb99662dc157267d166b7c90000000010270000000000001976a914",
    "000000000000000000000000000000000000000088acd01f772a000000001976a9141aded9db78a48344f854",
    "1ffa7d4300ada64fc68088ac000000",
);
const NU6_3_TXID: &str = "d759e629035fc77989d8b3253d16660e5cd3beef33bd8a011ba335d0f44d7bb8";
const NU6_3_PREVOUT_VALUE: u64 = 712_480_000;

/// The P2PKH script both spent outputs are locked by (`tmCARs7pqhUaYwev5vi3Fn3V7oB8PYzKmUS`).
const PREVOUT_SCRIPT: &str = "76a9141aded9db78a48344f8541ffa7d4300ada64fc68088ac";

#[derive(Debug)]
struct VerificationTransparent {
    spent_output: TxOut,
}

impl TransparentAuthorization for VerificationTransparent {
    type ScriptSig = Script;
}

impl TransparentAuthorizingContext for VerificationTransparent {
    fn input_amounts(&self) -> Vec<Zatoshis> {
        vec![self.spent_output.value()]
    }

    fn input_scriptpubkeys(&self) -> Vec<Script> {
        vec![self.spent_output.script_pubkey().clone()]
    }
}

#[derive(Debug)]
struct VerificationAuthorization;

impl TransactionAuthorization for VerificationAuthorization {
    type TransparentAuth = VerificationTransparent;
    type SaplingAuth = sapling::bundle::Authorized;
    type OrchardAuth = orchard::bundle::Authorized;
}

fn unhex(text: &str) -> Vec<u8> {
    hex::decode(text).expect("vector is hex")
}

fn spent_output(value: u64) -> TxOut {
    TxOut::new(
        Zatoshis::from_u64(value).unwrap(),
        Script(Code(unhex(PREVOUT_SCRIPT))),
    )
}

fn parse(text: &str) -> (Vec<u8>, Transaction) {
    let bytes = unhex(text);
    // A v5 transaction names its own branch; the argument only matters for v4 and older.
    let transaction = Transaction::read(bytes.as_slice(), BranchId::Nu6_2).unwrap();
    (bytes, transaction)
}

/// Splits a P2PKH scriptSig into its DER signature (without the sighash type
/// byte), its sighash type byte and its compressed public key.
fn p2pkh_script_sig(script_sig: &[u8]) -> (&[u8], u8, &[u8]) {
    let signature_length = usize::from(script_sig[0]);
    let signature = &script_sig[1..signature_length];
    let hash_type = script_sig[signature_length];
    let key_length = usize::from(script_sig[1 + signature_length]);
    let public_key = &script_sig[2 + signature_length..2 + signature_length + key_length];
    assert_eq!(script_sig.len(), 2 + signature_length + key_length);
    (signature, hash_type, public_key)
}

fn verification_data(
    transaction: &Transaction,
    spent: &TxOut,
) -> PrecomputedSighashData<VerificationAuthorization> {
    let data = transaction
        .clone()
        .into_data()
        .map_bundles::<VerificationAuthorization>(
            |bundle| {
                bundle.map(|bundle| Bundle {
                    vin: bundle
                        .vin
                        .into_iter()
                        .map(|input| {
                            TxIn::from_parts(
                                input.prevout().clone(),
                                input.script_sig().clone(),
                                input.sequence(),
                            )
                        })
                        .collect(),
                    vout: bundle.vout,
                    authorization: VerificationTransparent {
                        spent_output: spent.clone(),
                    },
                })
            },
            |bundle| bundle,
            |bundle| bundle,
        );
    PrecomputedSighashData::new_zip244(data).unwrap()
}

fn sighash_all(precomputed: &PrecomputedSighashData<VerificationAuthorization>) -> [u8; 32] {
    let bundle = precomputed.tx().transparent_bundle().unwrap();
    let spent = &bundle.authorization.spent_output;
    let signable = TransparentSignableInput::from_parts(
        bundle,
        SighashType::ALL,
        0,
        spent.script_pubkey(),
        spent.script_pubkey(),
        spent.value(),
    )
    .unwrap();
    *precomputed
        .signature_hash(&TransactionSignableInput::Transparent(signable))
        .as_ref()
}

fn interpreter_accepts(
    transaction: &Transaction,
    precomputed: &PrecomputedSighashData<VerificationAuthorization>,
) -> bool {
    let bundle = precomputed.tx().transparent_bundle().unwrap();
    let spent = &bundle.authorization.spent_output;
    let sighash = |script_code: &Code, hash_type: &HashType| {
        if hash_type.signed_outputs() != SignedOutputs::All || hash_type.anyone_can_pay() {
            return None;
        }
        let script_code = Script(script_code.clone());
        let signable = TransparentSignableInput::from_parts(
            bundle,
            SighashType::ALL,
            0,
            &script_code,
            spent.script_pubkey(),
            spent.value(),
        )
        .ok()?;
        Some(
            *precomputed
                .signature_hash(&TransactionSignableInput::Transparent(signable))
                .as_ref(),
        )
    };
    let input = &transaction.transparent_bundle().unwrap().vin[0];
    let checker = CallbackTransactionSignatureChecker {
        sighash: &sighash,
        lock_time: i64::from(transaction.lock_time()),
        is_final: input.sequence() == u32::MAX,
    };
    matches!(
        Raw::from_raw_parts(
            input.script_sig().0.0.clone(),
            spent.script_pubkey().0.0.clone()
        )
        .eval(Flags::P2SH | Flags::StrictEnc, &checker),
        Ok(true)
    )
}

fn assert_mined_vector(text: &str, txid: &str, branch: BranchId, branch_id: u32, value: u64) {
    let (bytes, transaction) = parse(text);

    // The header carries the branch id little-endian right after the version group id.
    assert_eq!(bytes[8..12], branch_id.to_le_bytes());
    assert_eq!(transaction.consensus_branch_id(), branch);
    assert_eq!(u32::from(branch), branch_id);
    assert_eq!(transaction.txid().to_string(), txid);
    let mut reserialized = vec![];
    transaction.write(&mut reserialized).unwrap();
    assert_eq!(reserialized, bytes);

    let spent = spent_output(value);
    let precomputed = verification_data(&transaction, &spent);
    let input = &transaction.transparent_bundle().unwrap().vin[0];
    let (signature, hash_type, public_key) = p2pkh_script_sig(&input.script_sig().0.0);
    assert_eq!(hash_type, SighashType::ALL.encode());
    let public_key = PublicKey::from_slice(public_key).unwrap();
    let script: Script = lez_zec_swap_sdk::p2pkh_address(&public_key).script().into();
    assert_eq!(script.0.0, unhex(PREVOUT_SCRIPT));
    let signature = Signature::from_der(signature).unwrap();
    let message = Message::from_digest(sighash_all(&precomputed));
    Secp256k1::verification_only()
        .verify_ecdsa(&message, &signature, &public_key)
        .expect("the mined signature verifies under our ZIP-244 signature hash");
    assert!(interpreter_accepts(&transaction, &precomputed));

    // The same transaction claiming any other branch hashes differently, so
    // the mined signature no longer verifies: the branch id is committed.
    for other in [
        BranchId::Nu6_1,
        BranchId::Nu6_2,
        BranchId::Nu6_3,
        BranchId::Nu7,
    ] {
        if other == branch {
            continue;
        }
        let mut relabelled = bytes.clone();
        relabelled[8..12].copy_from_slice(&u32::from(other).to_le_bytes());
        let (_, relabelled) = parse(&hex::encode(relabelled));
        assert_ne!(relabelled.txid().to_string(), txid);
        let precomputed = verification_data(&relabelled, &spent);
        let message = Message::from_digest(sighash_all(&precomputed));
        assert!(
            Secp256k1::verification_only()
                .verify_ecdsa(&message, &signature, &public_key)
                .is_err()
        );
        assert!(!interpreter_accepts(&relabelled, &precomputed));
    }

    // The signature hash also commits to the spent amount (ZIP-244 S.2).
    let wrong_value = verification_data(&transaction, &spent_output(value + 1));
    assert!(!interpreter_accepts(&transaction, &wrong_value));
}

#[test]
fn mined_nu7_testnet_spend_reproduces_its_txid_and_signature() {
    assert_mined_vector(
        NU7_TX,
        NU7_TXID,
        BranchId::Nu7,
        0x7719_0ad9,
        NU7_PREVOUT_VALUE,
    );
}

#[test]
fn mined_nu6_3_testnet_spend_reproduces_its_txid_and_signature() {
    assert_mined_vector(
        NU6_3_TX,
        NU6_3_TXID,
        BranchId::Nu6_3,
        0x37a5_165b,
        NU6_3_PREVOUT_VALUE,
    );
}
