use std::fmt::Write as _;

use lez_zec_swap_sdk::{
    Bip199Contract, TransactionBuildError, TransparentSpendRequest, build_claim_transaction,
    build_refund_transaction,
};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use zcash_primitives::transaction::{
    Authorization as TransactionAuthorization, Transaction, TxVersion,
    sighash::{SignableInput as TransactionSignableInput, signature_hash},
    txid::TxIdDigester,
};
use zcash_protocol::{
    consensus::{BlockHeight, BranchId},
    value::Zatoshis,
};
use zcash_script::{
    interpreter::{CallbackTransactionSignatureChecker, Flags},
    script::{Code, Raw},
    signature::{HashType, SignedOutputs},
};
use zcash_transparent::{
    address::{Script, TransparentAddress},
    bundle::{Authorization as TransparentAuthorization, Bundle, OutPoint, TxIn, TxOut},
    sighash::{
        SighashType, SignableInput as TransparentSignableInput, TransparentAuthorizingContext,
    },
};

const PREIMAGE: [u8; 32] = [0x44; 32];
const SECRET_DIGEST: [u8; 32] = [
    0xbb, 0x39, 0x14, 0x15, 0xc0, 0x5e, 0x39, 0xd7, 0x7c, 0xa1, 0x73, 0x81, 0xd3, 0xbe, 0x3f, 0x7d,
    0x0c, 0xd5, 0xe5, 0x33, 0x2e, 0x5a, 0x57, 0x93, 0x11, 0xad, 0xaa, 0x0a, 0xa6, 0x21, 0x06, 0xe9,
];
const EXPECTED_CLAIM_BYTES: &str = concat!(
    "050000800a27a72630f3375400000000a08f3e0001111111111111111111111111111111111111111111111111111111111111111107000000ea4730440220594b99771c2c40e7b551e9982fc73a56777bb7bf392584d0a62b6ddd0f1f7f1102206eaf8cb9771112d0efdf7c75b51bb3d48121aa0c62e597b489c8164f444510a80121024d4b6cd1361032ca9bd2aeb9d900aa4d45d9ead80ac9423374c451a7254d0766204444444444444444444444444444444444444444444444444444444444444444514c5c63a820bb391415c05e39d77ca17381d3be3f7d0cd5e5332e5a579311adaa0aa62106e98876a914ebc0ee0b2ab9e8277a600c251475e22a3241a1c1670320a107b17576a91479b000887626b294a914501a4cd226b58b2359836888ac",
    "ffffffff01905f0100000000001976a914417d4be90d35363267b8f2afafc9531111c41ae488ac000000",
);
const EXPECTED_CLAIM_TXID: &str =
    "34d7f6ee3bcd9f02e30f1d4d7a939b03bb48f01dddb41b313a3afe72c28e2caf";
const EXPECTED_REFUND_BYTES: &str = concat!(
    "050000800a27a72630f3375420a10700a08f3e0001111111111111111111111111111111111111111111111111111111111111111107000000ca4830450221009a038a108e54922dcdf4dfefaeab80b36db06e352fe94e5c4c1930638f491422022078ff9a31701eff9346fbd11f8a68ec0e37d2768819403bcf50fee96f5d5fe3ef0121031b84c5567b126440995d3ed5aaba0565d71e1834604819ff9c17f5e9d5dd078f004c5c63a820bb391415c05e39d77ca17381d3be3f7d0cd5e5332e5a579311adaa0aa62106e98876a914ebc0ee0b2ab9e8277a600c251475e22a3241a1c1670320a107b17576a91479b000887626b294a914501a4cd226b58b2359836888ac",
    "feffffff01905f0100000000001976a914417d4be90d35363267b8f2afafc9531111c41ae488ac000000",
);
const EXPECTED_REFUND_TXID: &str =
    "9b0fedde5f0d09e2f998d3880e4ecede785242a5b9ad9903a99e0af589184d2c";

#[derive(Debug)]
struct VerificationTransparent {
    funding_output: TxOut,
}

impl TransparentAuthorization for VerificationTransparent {
    type ScriptSig = Script;
}

impl TransparentAuthorizingContext for VerificationTransparent {
    fn input_amounts(&self) -> Vec<Zatoshis> {
        vec![self.funding_output.value()]
    }

    fn input_scriptpubkeys(&self) -> Vec<Script> {
        vec![self.funding_output.script_pubkey().clone()]
    }
}

#[derive(Debug)]
struct VerificationAuthorization;

impl TransactionAuthorization for VerificationAuthorization {
    type TransparentAuth = VerificationTransparent;
    type SaplingAuth = sapling::bundle::Authorized;
    type OrchardAuth = orchard::bundle::Authorized;
}

fn key(byte: u8) -> SecretKey {
    SecretKey::from_slice(&[byte; 32]).unwrap()
}

fn pubkey_hash(secret_key: &SecretKey) -> [u8; 20] {
    let public_key = PublicKey::from_secret_key(&Secp256k1::new(), secret_key);
    match TransparentAddress::from_pubkey(&public_key) {
        TransparentAddress::PublicKeyHash(hash) => hash,
        TransparentAddress::ScriptHash(_) => unreachable!(),
    }
}

fn fixture() -> (
    Bip199Contract,
    TransparentSpendRequest,
    SecretKey,
    SecretKey,
) {
    let refund_key = key(1);
    let claimant_key = key(2);
    let destination_key = key(3);
    let destination = TransparentAddress::from_pubkey(&PublicKey::from_secret_key(
        &Secp256k1::new(),
        &destination_key,
    ));
    let contract = Bip199Contract::new(
        500_000,
        pubkey_hash(&refund_key),
        SECRET_DIGEST,
        pubkey_hash(&claimant_key),
    );
    let funding_output = TxOut::new(
        Zatoshis::from_u64(100_000).unwrap(),
        Script(Code(contract.p2sh_script_pubkey().to_vec())),
    );
    let request = TransparentSpendRequest::new(
        &contract,
        OutPoint::new([0x11; 32], 7),
        funding_output,
        destination,
        Zatoshis::from_u64(10_000).unwrap(),
        BlockHeight::from_u32(4_100_000),
        BranchId::Nu6_2,
    )
    .unwrap();

    (contract, request, refund_key, claimant_key)
}

fn hex(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

fn script_flags() -> Flags {
    Flags::P2SH
        | Flags::SigPushOnly
        | Flags::MinimalData
        | Flags::CleanStack
        | Flags::CHECKLOCKTIMEVERIFY
        | Flags::StrictEnc
}

fn execute_script(
    transaction: &Transaction,
    request: &TransparentSpendRequest,
    script_sig: Vec<u8>,
) -> bool {
    let funding_output = request.funding_output().clone();
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
                        funding_output: funding_output.clone(),
                    },
                })
            },
            |bundle| bundle,
            |bundle| bundle,
        );
    let txid_parts = data.digest(TxIdDigester);
    let bundle = data.transparent_bundle().unwrap();
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
            funding_output.script_pubkey(),
            funding_output.value(),
        )
        .ok()?;
        Some(
            *signature_hash(
                &data,
                &TransactionSignableInput::Transparent(signable),
                &txid_parts,
            )
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
        Raw::from_raw_parts(script_sig, funding_output.script_pubkey().0.0.clone(),)
            .eval(script_flags(), &checker),
        Ok(true)
    )
}

fn serialize(transaction: &Transaction) -> Vec<u8> {
    let mut bytes = vec![];
    transaction.write(&mut bytes).unwrap();
    bytes
}

fn only_output(transaction: &Transaction) -> &TxOut {
    &transaction.transparent_bundle().unwrap().vout[0]
}

#[test]
fn refund_is_zip244_signed_with_exact_cltv_policy_and_value_conservation() {
    let (contract, request, refund_key, _) = fixture();

    let transaction = build_refund_transaction(&contract, &request, &refund_key).unwrap();
    let bundle = transaction.transparent_bundle().unwrap();

    assert_eq!(transaction.lock_time(), contract.refund_lock_time());
    assert_eq!(bundle.vin.len(), 1);
    assert_eq!(bundle.vin[0].sequence(), contract.refund_input_sequence());
    assert_eq!(bundle.vout.len(), 1);
    assert_eq!(
        only_output(&transaction).value(),
        Zatoshis::from_u64(90_000).unwrap()
    );
    assert_eq!(
        transaction
            .fee_paid::<zcash_protocol::value::BalanceError, _>(|outpoint| {
                assert_eq!(outpoint, request.prevout());
                Ok(Some(request.prevout_value()))
            })
            .unwrap(),
        Some(request.fee())
    );
}

#[test]
fn claim_is_signed_without_a_refund_lock_and_commits_the_preimage() {
    let (contract, request, _, claimant_key) = fixture();

    let transaction =
        build_claim_transaction(&contract, &request, &claimant_key, &PREIMAGE).unwrap();
    let input = &transaction.transparent_bundle().unwrap().vin[0];

    assert_eq!(transaction.lock_time(), 0);
    assert_eq!(input.sequence(), u32::MAX);
    assert!(
        input
            .script_sig()
            .0
            .0
            .windows(PREIMAGE.len())
            .any(|w| w == PREIMAGE)
    );
}

#[test]
fn signing_is_deterministic_and_canonical_bytes_roundtrip() {
    let (contract, request, refund_key, _) = fixture();

    let first = build_refund_transaction(&contract, &request, &refund_key).unwrap();
    let second = build_refund_transaction(&contract, &request, &refund_key).unwrap();
    let bytes = serialize(&first);

    assert_eq!(bytes, serialize(&second));
    let decoded = Transaction::read(bytes.as_slice(), BranchId::Nu6_2).unwrap();
    assert_eq!(serialize(&decoded), bytes);
    assert_eq!(decoded.txid(), first.txid());
}

#[test]
fn claim_and_refund_match_fixed_serialization_and_txid_vectors() {
    let (contract, request, refund_key, claimant_key) = fixture();
    let claim = build_claim_transaction(&contract, &request, &claimant_key, &PREIMAGE).unwrap();
    let refund = build_refund_transaction(&contract, &request, &refund_key).unwrap();

    assert_eq!(hex(&serialize(&claim)), EXPECTED_CLAIM_BYTES);
    assert_eq!(claim.txid().to_string(), EXPECTED_CLAIM_TXID);
    assert_eq!(hex(&serialize(&refund)), EXPECTED_REFUND_BYTES);
    assert_eq!(refund.txid().to_string(), EXPECTED_REFUND_TXID);
}

#[test]
fn produced_claim_and_refund_signatures_execute_and_mutations_fail() {
    let (contract, request, refund_key, claimant_key) = fixture();
    let claim = build_claim_transaction(&contract, &request, &claimant_key, &PREIMAGE).unwrap();
    let refund = build_refund_transaction(&contract, &request, &refund_key).unwrap();

    for transaction in [&claim, &refund] {
        let script_sig = transaction.transparent_bundle().unwrap().vin[0]
            .script_sig()
            .0
            .0
            .clone();
        assert!(execute_script(transaction, &request, script_sig.clone()));

        let mut mutated = script_sig;
        mutated[12] ^= 1;
        assert!(!execute_script(transaction, &request, mutated));
    }
}

#[test]
fn wrong_role_key_and_fee_exceeding_value_fail_closed() {
    let (contract, request, refund_key, claimant_key) = fixture();

    assert_eq!(
        build_refund_transaction(&contract, &request, &claimant_key).unwrap_err(),
        TransactionBuildError::WrongSpendingKey
    );
    assert_eq!(
        build_claim_transaction(&contract, &request, &refund_key, &PREIMAGE).unwrap_err(),
        TransactionBuildError::WrongSpendingKey
    );
    assert_eq!(
        build_claim_transaction(&contract, &request, &claimant_key, &[0x45; 32]).unwrap_err(),
        TransactionBuildError::WrongPreimage
    );

    let invalid = TransparentSpendRequest::new(
        &contract,
        request.prevout().clone(),
        TxOut::new(
            Zatoshis::from_u64(9_999).unwrap(),
            request.funding_output().script_pubkey().clone(),
        ),
        request.destination(),
        Zatoshis::from_u64(10_000).unwrap(),
        request.expiry_height(),
        request.consensus_branch_id(),
    );
    assert_eq!(invalid.unwrap_err(), TransactionBuildError::FeeExceedsInput);
}

#[test]
fn request_rejects_non_v5_branch_and_funding_script_mismatch() {
    let (contract, request, _, _) = fixture();
    let invalid_branch = TransparentSpendRequest::new(
        &contract,
        request.prevout().clone(),
        request.funding_output().clone(),
        request.destination(),
        request.fee(),
        request.expiry_height(),
        BranchId::Canopy,
    );
    assert_eq!(
        invalid_branch.unwrap_err(),
        TransactionBuildError::UnsupportedConsensusBranch {
            branch_id: BranchId::Canopy,
            transaction_version: TxVersion::V5,
        }
    );

    let wrong_output = TxOut::new(
        request.prevout_value(),
        request.destination().script().into(),
    );
    let mismatch = TransparentSpendRequest::new(
        &contract,
        request.prevout().clone(),
        wrong_output,
        request.destination(),
        request.fee(),
        request.expiry_height(),
        request.consensus_branch_id(),
    );
    assert_eq!(
        mismatch.unwrap_err(),
        TransactionBuildError::FundingScriptMismatch
    );

    let other_contract = Bip199Contract::new(500_000, [1; 20], SECRET_DIGEST, [2; 20]);
    assert_eq!(
        build_refund_transaction(&other_contract, &request, &key(1)).unwrap_err(),
        TransactionBuildError::FundingScriptMismatch
    );
}
