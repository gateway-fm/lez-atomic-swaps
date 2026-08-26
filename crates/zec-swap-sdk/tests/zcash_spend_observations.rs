use lez_zec_swap_sdk::{
    Bip199Contract, Bip199SpendKind, CanonicalZcashSpendObservation, ExpectedBip199Spend,
    SpendObservationError, TransparentSpendRequest, ZcashSpendNodeSnapshot, ZcashStableTip,
    build_claim_transaction, build_refund_transaction,
};
use orchard::bundle as orchard_bundle;
use sapling::bundle as sapling_bundle;
use secp256k1::{Message, PublicKey, Secp256k1, SecretKey, ecdsa::Signature};
use zcash_primitives::{
    block::BlockHash,
    transaction::{
        Authorization as TransactionAuthorization, Authorized as TransactionAuthorized,
        Transaction, TransactionData, TxVersion,
        sighash::{SignableInput as TransactionSignableInput, signature_hash},
        txid::TxIdDigester,
    },
};
use zcash_protocol::{
    consensus::{BlockHeight, BranchId, NetworkType},
    value::Zatoshis,
};
use zcash_script::{Opcode, opcode::PossiblyBad, script::Code};
use zcash_transparent::{
    address::{Script, TransparentAddress},
    bundle::{
        Authorization as TransparentAuthorization, Authorized as TransparentAuthorized, Bundle,
        OutPoint, TxIn, TxOut,
    },
    sighash::{
        SighashType, SignableInput as TransparentSignableInput, TransparentAuthorizingContext,
    },
};

const PREIMAGE: [u8; 32] = [0x44; 32];
const SECRET_DIGEST: [u8; 32] = [
    0xbb, 0x39, 0x14, 0x15, 0xc0, 0x5e, 0x39, 0xd7, 0x7c, 0xa1, 0x73, 0x81, 0xd3, 0xbe, 0x3f, 0x7d,
    0x0c, 0xd5, 0xe5, 0x33, 0x2e, 0x5a, 0x57, 0x93, 0x11, 0xad, 0xaa, 0x0a, 0xa6, 0x21, 0x06, 0xe9,
];

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
    Transaction,
    Transaction,
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
    let claim = build_claim_transaction(&contract, &request, &claimant_key, &PREIMAGE).unwrap();
    let refund = build_refund_transaction(&contract, &request, &refund_key).unwrap();
    (contract, request, claim, refund)
}

fn expected(contract: Bip199Contract, request: &TransparentSpendRequest) -> ExpectedBip199Spend {
    ExpectedBip199Spend::from_request(NetworkType::Regtest, contract, request).unwrap()
}

fn serialize(transaction: &Transaction) -> Vec<u8> {
    let mut bytes = vec![];
    transaction.write(&mut bytes).unwrap();
    bytes
}

fn snapshot(transaction: &Transaction, raw: Vec<u8>) -> ZcashSpendNodeSnapshot {
    ZcashSpendNodeSnapshot::new(
        NetworkType::Regtest,
        BranchId::Nu6_2,
        true,
        BlockHash([0x55; 32]),
        BlockHash([0x55; 32]),
        BlockHeight::from_u32(200),
        ZcashStableTip::new(
            BlockHash([0xaa; 32]),
            BlockHeight::from_u32(204),
            BlockHash([0xaa; 32]),
            BlockHeight::from_u32(204),
        ),
        transaction.txid(),
        raw,
        5,
    )
}

fn replace_once(raw: &mut [u8], needle: &[u8], replacement: &[u8]) {
    assert_eq!(needle.len(), replacement.len());
    let offset = raw
        .windows(needle.len())
        .position(|candidate| candidate == needle)
        .expect("fixture contains the selected stack item");
    raw[offset..offset + needle.len()].copy_from_slice(replacement);
}

#[derive(Debug)]
struct SigningTransparent {
    funding_output: TxOut,
}

impl TransparentAuthorization for SigningTransparent {
    type ScriptSig = ();
}

impl TransparentAuthorizingContext for SigningTransparent {
    fn input_amounts(&self) -> Vec<Zatoshis> {
        vec![self.funding_output.value()]
    }

    fn input_scriptpubkeys(&self) -> Vec<Script> {
        vec![self.funding_output.script_pubkey().clone()]
    }
}

#[derive(Debug)]
struct SigningAuthorization;

impl TransactionAuthorization for SigningAuthorization {
    type TransparentAuth = SigningTransparent;
    type SaplingAuth = sapling_bundle::Authorized;
    type OrchardAuth = orchard_bundle::Authorized;
}

fn signed_claim_with(
    contract: &Bip199Contract,
    request: &TransparentSpendRequest,
    secret_key: &SecretKey,
    sighash_type: SighashType,
    outputs: Vec<TxOut>,
    expiry_height: BlockHeight,
) -> Transaction {
    let unsigned_bundle = Bundle {
        vin: vec![TxIn::from_parts(request.prevout().clone(), (), u32::MAX)],
        vout: outputs.clone(),
        authorization: SigningTransparent {
            funding_output: request.funding_output().clone(),
        },
    };
    let unsigned = TransactionData::<SigningAuthorization>::from_parts(
        TxVersion::V5,
        request.consensus_branch_id(),
        0,
        expiry_height,
        Some(unsigned_bundle),
        None,
        None,
        None,
    );
    let txid_parts = unsigned.digest(TxIdDigester);
    let bundle = unsigned.transparent_bundle().unwrap();
    let redeem_script = Script(Code(contract.redeem_script().to_vec()));
    let signable = TransparentSignableInput::from_parts(
        bundle,
        sighash_type,
        0,
        &redeem_script,
        request.funding_output().script_pubkey(),
        request.funding_output().value(),
    )
    .unwrap();
    let digest = signature_hash(
        &unsigned,
        &TransactionSignableInput::Transparent(signable),
        &txid_parts,
    );
    let secp = Secp256k1::signing_only();
    let mut signature = secp
        .sign_ecdsa(&Message::from_digest(*digest.as_ref()), secret_key)
        .serialize_der()
        .to_vec();
    signature.push(sighash_type.encode());
    let public_key = PublicKey::from_secret_key(&secp, secret_key).serialize();
    let script_sig = contract
        .claim_script_sig(&signature, &public_key, &PREIMAGE)
        .unwrap();
    let authorized_bundle = Bundle {
        vin: vec![TxIn::from_parts(
            request.prevout().clone(),
            Script(Code(script_sig)),
            u32::MAX,
        )],
        vout: outputs,
        authorization: TransparentAuthorized,
    };
    TransactionData::<TransactionAuthorized>::from_parts(
        TxVersion::V5,
        request.consensus_branch_id(),
        0,
        expiry_height,
        Some(authorized_bundle),
        None,
        None,
        None,
    )
    .freeze()
    .unwrap()
}

fn with_script_sig(transaction: &Transaction, script_sig: Vec<u8>) -> Transaction {
    let bundle = transaction.transparent_bundle().unwrap();
    let authorized_bundle = Bundle {
        vin: vec![TxIn::from_parts(
            bundle.vin[0].prevout().clone(),
            Script(Code(script_sig)),
            bundle.vin[0].sequence(),
        )],
        vout: bundle.vout.clone(),
        authorization: TransparentAuthorized,
    };
    TransactionData::<TransactionAuthorized>::from_parts(
        TxVersion::V5,
        transaction.consensus_branch_id(),
        transaction.lock_time(),
        transaction.expiry_height(),
        Some(authorized_bundle),
        None,
        None,
        None,
    )
    .freeze()
    .unwrap()
}

fn script_pushes(transaction: &Transaction) -> Vec<Vec<u8>> {
    Code(
        transaction.transparent_bundle().unwrap().vin[0]
            .script_sig()
            .0
            .0
            .clone(),
    )
    .parse()
    .map(|parsed| match parsed.unwrap() {
        PossiblyBad::Good(Opcode::PushValue(value)) => value.value(),
        _ => panic!("fixture is push-only"),
    })
    .collect()
}

fn minimally_encode_pushes(pushes: &[Vec<u8>]) -> Vec<u8> {
    let mut encoded = vec![];
    for value in pushes {
        match value.as_slice() {
            [] => encoded.push(0),
            [0x81] => encoded.push(0x4f),
            [value @ 1..=16] => encoded.push(0x50 + value),
            _ if value.len() <= 75 => {
                encoded.push(u8::try_from(value.len()).expect("length is at most 75"));
                encoded.extend_from_slice(value);
            }
            _ => {
                encoded.extend_from_slice(&[
                    0x4c,
                    u8::try_from(value.len()).expect("fixture push is at most 255 bytes"),
                ]);
                encoded.extend_from_slice(value);
            }
        }
    }
    encoded
}

fn nonminimal_encode_pushes(pushes: &[Vec<u8>]) -> Vec<u8> {
    let mut encoded = vec![];
    for value in pushes {
        encoded.extend_from_slice(&[
            0x4c,
            u8::try_from(value.len()).expect("fixture push is at most 255 bytes"),
        ]);
        encoded.extend_from_slice(value);
    }
    encoded
}

fn high_s_signature(signature_with_hash_type: &[u8]) -> Vec<u8> {
    const CURVE_ORDER: [u8; 32] = [
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xfe, 0xba, 0xae, 0xdc, 0xe6, 0xaf, 0x48, 0xa0, 0x3b, 0xbf, 0xd2, 0x5e, 0x8c, 0xd0, 0x36,
        0x41, 0x41,
    ];
    let (&hash_type, der) = signature_with_hash_type.split_last().unwrap();
    let compact = Signature::from_der(der).unwrap().serialize_compact();
    let mut high_s = [0u8; 32];
    let mut borrow = 0u16;
    for index in (0..32).rev() {
        let minuend = u16::from(CURVE_ORDER[index]);
        let subtrahend = u16::from(compact[32 + index]) + borrow;
        if minuend >= subtrahend {
            high_s[index] =
                u8::try_from(minuend - subtrahend).expect("single-byte subtraction result");
            borrow = 0;
        } else {
            high_s[index] = u8::try_from(minuend + 256 - subtrahend)
                .expect("single-byte subtraction result with borrow");
            borrow = 1;
        }
    }
    let mut high_compact = compact;
    high_compact[32..].copy_from_slice(&high_s);
    let mut encoded = Signature::from_compact(&high_compact)
        .unwrap()
        .serialize_der()
        .to_vec();
    encoded.push(hash_type);
    encoded
}

#[test]
fn canonical_claim_and_refund_bind_the_complete_spend_and_role() {
    let (contract, request, claim, refund) = fixture();
    let expected = expected(contract, &request);

    let claim_raw = serialize(&claim);
    let claim_observation =
        CanonicalZcashSpendObservation::validate(&expected, &snapshot(&claim, claim_raw.clone()))
            .unwrap();
    assert_eq!(claim_observation.network(), NetworkType::Regtest);
    assert_eq!(claim_observation.consensus_branch_id(), BranchId::Nu6_2);
    assert_eq!(claim_observation.transaction_id(), claim.txid());
    assert_eq!(claim_observation.spent_outpoint(), request.prevout());
    assert_eq!(claim_observation.block_hash(), BlockHash([0x55; 32]));
    assert_eq!(claim_observation.block_height(), BlockHeight::from_u32(200));
    assert_eq!(claim_observation.tip_block_hash(), BlockHash([0xaa; 32]));
    assert_eq!(claim_observation.tip_height(), BlockHeight::from_u32(204));
    assert_eq!(claim_observation.confirmations().get(), 5);
    assert_eq!(claim_observation.raw_transaction(), claim_raw);
    assert!(matches!(
        claim_observation.kind(),
        Bip199SpendKind::Claim { preimage, .. } if preimage.as_ref() == PREIMAGE
    ));
    assert_eq!(claim_observation.lock_time(), 0);
    assert_eq!(claim_observation.expiry_height(), request.expiry_height());
    assert_eq!(claim_observation.input_sequence(), u32::MAX);
    assert_eq!(
        claim_observation.transparent_outputs(),
        &claim.transparent_bundle().unwrap().vout
    );
    assert!(claim_observation.sdk_canonical_policy().is_compliant());

    let refund_observation =
        CanonicalZcashSpendObservation::validate(&expected, &snapshot(&refund, serialize(&refund)))
            .unwrap();
    assert!(matches!(
        refund_observation.kind(),
        Bip199SpendKind::Refund { .. }
    ));
}

#[test]
fn spend_observation_bounds_untrusted_transaction_bytes_before_decode() {
    let (contract, request, claim, _) = fixture();
    let expected = expected(contract, &request);

    let at_limit = vec![0; 2_000_000];
    assert_eq!(
        CanonicalZcashSpendObservation::validate(&expected, &snapshot(&claim, at_limit)),
        Err(SpendObservationError::MalformedTransaction)
    );

    let above_limit = vec![0; 2_000_001];
    assert_eq!(
        CanonicalZcashSpendObservation::validate(&expected, &snapshot(&claim, above_limit)),
        Err(SpendObservationError::RawTransactionTooLarge {
            actual: 2_000_001,
            maximum: 2_000_000,
        })
    );
}

#[test]
fn every_defined_zip244_sighash_mode_is_recognized_but_policy_is_separate() {
    let (contract, request, claim, _) = fixture();
    let expected = expected(contract.clone(), &request);
    let outputs = claim.transparent_bundle().unwrap().vout.clone();
    let modes = [
        SighashType::ALL,
        SighashType::NONE,
        SighashType::SINGLE,
        SighashType::ALL_ANYONECANPAY,
        SighashType::NONE_ANYONECANPAY,
        SighashType::SINGLE_ANYONECANPAY,
    ];

    for mode in modes {
        let transaction = signed_claim_with(
            &contract,
            &request,
            &key(2),
            mode,
            outputs.clone(),
            request.expiry_height(),
        );
        let observation = CanonicalZcashSpendObservation::validate(
            &expected,
            &snapshot(&transaction, serialize(&transaction)),
        )
        .unwrap();
        assert!(matches!(
            observation.kind(),
            Bip199SpendKind::Claim { preimage, .. } if preimage.as_ref() == PREIMAGE
        ));
        assert_eq!(
            observation.sdk_canonical_policy().sighash_type(),
            mode.encode()
        );
        assert_eq!(
            observation
                .sdk_canonical_policy()
                .uses_all_without_anyone_can_pay(),
            mode == SighashType::ALL
        );
        assert_eq!(
            observation.sdk_canonical_policy().is_compliant(),
            mode == SighashType::ALL
        );
    }
}

#[test]
fn consensus_valid_nonminimal_high_s_and_semantic_stacks_reveal_the_claim() {
    let (contract, request, claim, refund) = fixture();
    let expected = expected(contract, &request);

    let pushes = script_pushes(&claim);
    let nonminimal = with_script_sig(&claim, nonminimal_encode_pushes(&pushes));
    let observation = CanonicalZcashSpendObservation::validate(
        &expected,
        &snapshot(&nonminimal, serialize(&nonminimal)),
    )
    .unwrap();
    assert!(matches!(
        observation.kind(),
        Bip199SpendKind::Claim { preimage, .. } if preimage.as_ref() == PREIMAGE
    ));
    assert!(!observation.sdk_canonical_policy().uses_minimal_pushes());
    assert!(!observation.sdk_canonical_policy().has_exact_script_sig());

    let mut high_s_pushes = pushes.clone();
    high_s_pushes[0] = high_s_signature(&high_s_pushes[0]);
    let high_s = with_script_sig(&claim, minimally_encode_pushes(&high_s_pushes));
    let observation =
        CanonicalZcashSpendObservation::validate(&expected, &snapshot(&high_s, serialize(&high_s)))
            .unwrap();
    assert!(matches!(
        observation.kind(),
        Bip199SpendKind::Claim { preimage, .. } if preimage.as_ref() == PREIMAGE
    ));
    assert!(!observation.sdk_canonical_policy().uses_low_s());

    let mut truthy_selector_pushes = pushes.clone();
    truthy_selector_pushes[3] = vec![2];
    let truthy_selector = with_script_sig(&claim, minimally_encode_pushes(&truthy_selector_pushes));
    let observation = CanonicalZcashSpendObservation::validate(
        &expected,
        &snapshot(&truthy_selector, serialize(&truthy_selector)),
    )
    .unwrap();
    assert!(matches!(
        observation.kind(),
        Bip199SpendKind::Claim { preimage, .. } if preimage.as_ref() == PREIMAGE
    ));
    assert!(!observation.sdk_canonical_policy().has_exact_script_sig());

    let mut extra_stack_pushes = pushes;
    extra_stack_pushes.insert(0, vec![]);
    let extra_stack = with_script_sig(&claim, minimally_encode_pushes(&extra_stack_pushes));
    let observation = CanonicalZcashSpendObservation::validate(
        &expected,
        &snapshot(&extra_stack, serialize(&extra_stack)),
    )
    .unwrap();
    assert!(matches!(
        observation.kind(),
        Bip199SpendKind::Claim { preimage, .. } if preimage.as_ref() == PREIMAGE
    ));
    assert!(!observation.sdk_canonical_policy().has_exact_script_sig());

    let mut negative_zero_refund = script_pushes(&refund);
    negative_zero_refund[2] = vec![0x80];
    let negative_zero_refund =
        with_script_sig(&refund, minimally_encode_pushes(&negative_zero_refund));
    let observation = CanonicalZcashSpendObservation::validate(
        &expected,
        &snapshot(&negative_zero_refund, serialize(&negative_zero_refund)),
    )
    .unwrap();
    assert!(matches!(observation.kind(), Bip199SpendKind::Refund { .. }));
    assert!(!observation.sdk_canonical_policy().has_exact_script_sig());
}

#[test]
fn consensus_observation_preserves_effects_while_policy_reports_mismatches() {
    let (contract, request, _, _) = fixture();
    let expected = expected(contract.clone(), &request);
    let wrong_destination =
        TransparentAddress::from_pubkey(&PublicKey::from_secret_key(&Secp256k1::new(), &key(4)));
    let wrong_output = TxOut::new(
        Zatoshis::from_u64(80_000).unwrap(),
        wrong_destination.script().into(),
    );
    let wrong_expiry = BlockHeight::from_u32(u32::from(request.expiry_height()) + 1);
    let transaction = signed_claim_with(
        &contract,
        &request,
        &key(2),
        SighashType::ALL,
        vec![wrong_output.clone()],
        wrong_expiry,
    );
    let observation = CanonicalZcashSpendObservation::validate(
        &expected,
        &snapshot(&transaction, serialize(&transaction)),
    )
    .unwrap();

    assert!(matches!(
        observation.kind(),
        Bip199SpendKind::Claim { preimage, .. } if preimage.as_ref() == PREIMAGE
    ));
    assert_eq!(observation.transparent_outputs(), [wrong_output]);
    assert_eq!(observation.lock_time(), 0);
    assert_eq!(observation.expiry_height(), wrong_expiry);
    assert_eq!(observation.input_sequence(), u32::MAX);
    let policy = observation.sdk_canonical_policy();
    assert!(policy.has_expected_shape());
    assert!(!policy.has_expected_destination());
    assert!(!policy.has_expected_fee());
    assert!(!policy.has_expected_expiry_height());
    assert!(!policy.is_compliant());

    let canonical_value = Zatoshis::from_u64(90_000).unwrap();
    let canonical_output = TxOut::new(canonical_value, request.destination().script().into());
    let extra_output = TxOut::new(Zatoshis::ZERO, request.destination().script().into());
    let multi_output = signed_claim_with(
        &contract,
        &request,
        &key(2),
        SighashType::ALL,
        vec![canonical_output, extra_output],
        request.expiry_height(),
    );
    let observation = CanonicalZcashSpendObservation::validate(
        &expected,
        &snapshot(&multi_output, serialize(&multi_output)),
    )
    .unwrap();
    assert!(!observation.sdk_canonical_policy().has_expected_shape());
    assert_eq!(observation.transparent_outputs().len(), 2);
}

#[test]
fn script_and_sighash_malformed_boundaries_fail_without_hiding_valid_forms() {
    let (contract, request, claim, _) = fixture();
    let expected = expected(contract, &request);
    let canonical_script = claim.transparent_bundle().unwrap().vin[0]
        .script_sig()
        .0
        .0
        .clone();

    let mut at_limit = vec![0; 10_000 - canonical_script.len()];
    at_limit.extend_from_slice(&canonical_script);
    let at_limit = with_script_sig(&claim, at_limit);
    assert_eq!(
        CanonicalZcashSpendObservation::validate(
            &expected,
            &snapshot(&at_limit, serialize(&at_limit))
        ),
        Err(SpendObservationError::InvalidSpendScript)
    );

    let mut above_limit = vec![0; 10_001 - canonical_script.len()];
    above_limit.extend_from_slice(&canonical_script);
    let above_limit = with_script_sig(&claim, above_limit);
    assert_eq!(
        CanonicalZcashSpendObservation::validate(
            &expected,
            &snapshot(&above_limit, serialize(&above_limit))
        ),
        Err(SpendObservationError::ScriptSigTooLarge {
            actual: 10_001,
            maximum: 10_000,
        })
    );

    for invalid_hash_type in [0x00, 0x04, 0x41, 0xff] {
        let mut pushes = script_pushes(&claim);
        *pushes[0].last_mut().unwrap() = invalid_hash_type;
        let invalid = with_script_sig(&claim, minimally_encode_pushes(&pushes));
        assert_eq!(
            CanonicalZcashSpendObservation::validate(
                &expected,
                &snapshot(&invalid, serialize(&invalid))
            ),
            Err(SpendObservationError::InvalidSpendScript)
        );
    }
}

#[test]
fn spend_observation_rejects_wrong_outpoint_script_preimage_and_role() {
    let (contract, request, claim, _) = fixture();
    let expected = expected(contract.clone(), &request);
    let claim_raw = serialize(&claim);

    let wrong_outpoint = ExpectedBip199Spend::new(
        NetworkType::Regtest,
        BranchId::Nu6_2,
        OutPoint::new([0x22; 32], 7),
        request.funding_output().clone(),
        contract,
    )
    .unwrap();
    assert_eq!(
        CanonicalZcashSpendObservation::validate(
            &wrong_outpoint,
            &snapshot(&claim, claim_raw.clone())
        ),
        Err(SpendObservationError::OutpointMismatch)
    );

    let input = &claim.transparent_bundle().unwrap().vin[0];
    let script_sig = &input.script_sig().0.0;
    let claimant_public_key = PublicKey::from_secret_key(&Secp256k1::new(), &key(2)).serialize();

    let mut wrong_preimage = claim_raw.clone();
    replace_once(&mut wrong_preimage, &PREIMAGE, &[0x45; 32]);
    assert_eq!(
        CanonicalZcashSpendObservation::validate(&expected, &snapshot(&claim, wrong_preimage)),
        Err(SpendObservationError::WrongPreimage)
    );

    let mut wrong_role = claim_raw.clone();
    replace_once(
        &mut wrong_role,
        &claimant_public_key,
        &PublicKey::from_secret_key(&Secp256k1::new(), &key(1)).serialize(),
    );
    assert_eq!(
        CanonicalZcashSpendObservation::validate(&expected, &snapshot(&claim, wrong_role)),
        Err(SpendObservationError::WrongSpendingRole)
    );

    let mut wrong_script = claim_raw;
    let mut mutated_redeem_script = expected.contract().redeem_script().to_vec();
    mutated_redeem_script[10] ^= 1;
    replace_once(
        &mut wrong_script,
        expected.contract().redeem_script(),
        &mutated_redeem_script,
    );
    assert_eq!(
        CanonicalZcashSpendObservation::validate(&expected, &snapshot(&claim, wrong_script)),
        Err(SpendObservationError::SpendScriptMismatch)
    );

    assert!(
        script_sig
            .windows(PREIMAGE.len())
            .any(|item| item == PREIMAGE)
    );
}

#[test]
fn spend_observation_rejects_invalid_signature_noncanonical_data_and_tip_drift() {
    let (contract, request, claim, _) = fixture();
    let expected = expected(contract, &request);
    let raw = serialize(&claim);

    let mut invalid_signature = raw.clone();
    invalid_signature[70] ^= 1;
    assert_eq!(
        CanonicalZcashSpendObservation::validate(&expected, &snapshot(&claim, invalid_signature)),
        Err(SpendObservationError::InvalidSpendScript)
    );

    let mut inactive = snapshot(&claim, raw.clone());
    inactive.set_in_active_chain(false);
    assert_eq!(
        CanonicalZcashSpendObservation::validate(&expected, &inactive),
        Err(SpendObservationError::InactiveChain)
    );

    let mut unstable = snapshot(&claim, raw.clone());
    unstable.set_tip_after(BlockHash([0xbb; 32]), BlockHeight::from_u32(205));
    assert_eq!(
        CanonicalZcashSpendObservation::validate(&expected, &unstable),
        Err(SpendObservationError::UnstableTip)
    );

    let mut trailing = raw.clone();
    trailing.push(0);
    assert_eq!(
        CanonicalZcashSpendObservation::validate(&expected, &snapshot(&claim, trailing)),
        Err(SpendObservationError::TrailingTransactionBytes)
    );

    let mut wrong_network = snapshot(&claim, raw.clone());
    wrong_network.set_network(NetworkType::Test);
    assert_eq!(
        CanonicalZcashSpendObservation::validate(&expected, &wrong_network),
        Err(SpendObservationError::NetworkMismatch)
    );

    let mut wrong_branch = snapshot(&claim, raw.clone());
    wrong_branch.set_consensus_branch_id(BranchId::Nu6);
    assert_eq!(
        CanonicalZcashSpendObservation::validate(&expected, &wrong_branch),
        Err(SpendObservationError::ConsensusBranchMismatch)
    );

    let mut wrong_block = snapshot(&claim, raw.clone());
    wrong_block.set_canonical_block_hash(BlockHash([0x66; 32]));
    assert_eq!(
        CanonicalZcashSpendObservation::validate(&expected, &wrong_block),
        Err(SpendObservationError::BlockHashMismatch)
    );

    let mut above_tip = snapshot(&claim, raw.clone());
    above_tip.set_block_height(BlockHeight::from_u32(205));
    assert_eq!(
        CanonicalZcashSpendObservation::validate(&expected, &above_tip),
        Err(SpendObservationError::BlockAboveTip)
    );

    let mut wrong_depth = snapshot(&claim, raw.clone());
    wrong_depth.set_reported_confirmations(4);
    assert_eq!(
        CanonicalZcashSpendObservation::validate(&expected, &wrong_depth),
        Err(SpendObservationError::ConfirmationMismatch)
    );

    let mut wrong_txid = snapshot(&claim, raw);
    wrong_txid.set_reported_transaction_id(zcash_protocol::TxId::from_bytes([0x77; 32]));
    assert_eq!(
        CanonicalZcashSpendObservation::validate(&expected, &wrong_txid),
        Err(SpendObservationError::TransactionIdMismatch)
    );
}
