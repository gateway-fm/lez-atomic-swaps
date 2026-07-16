use bitcoin::absolute::LockTime;
use bitcoin::consensus::serialize;
use bitcoin::hashes::Hash as _;
use bitcoin::secp256k1::{Keypair, Message, PublicKey, Secp256k1, SecretKey};
use bitcoin::transaction::Version;
use bitcoin::{Amount, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid, Witness};
use lez_btc_swap_sdk::{
    AdaptorSessionContext, BTC_AGREEMENT_SCHEMA_V1, BitcoinFirstLockEvidenceV1,
    BtcActiveSwapEnvelopeV1, BtcAgreementBodyV1, BtcAgreementRecordV1, BtcChainPolicyV1,
    BtcClaimTermsV1, BtcFirstLockEvidenceV1, BtcFundingTermsV1, BtcLezTermsV1,
    BtcLifecycleActionV1, BtcP2trTermsV1, BtcPairSdk, BtcParticipantIdentityV1, BtcParticipantsV1,
    BtcPreparedLockEffectsV1, BtcProtocolCapabilityGapV1, BtcProtocolTermsV1, BtcRecoveryPlanV1,
    BtcSdkError, CooperativeKeyPathSpend, CsvBlockDelay, LezFirstLockEvidenceV1, P2trSwapOutput,
    PreparedBitcoinFundingV1, PreparedLezFundingV1, RefundXOnlyKey, TwoPartyAggregateKey,
};
use lez_swap_core::{Participant, Phase, SwapDirection};
use lez_swap_sdk_core::{ClaimOrder, SwapProtocol};

const BITCOIN_GENESIS: [u8; 32] = [8; 32];
const LEZ_GENESIS: [u8; 32] = [18; 32];
const REQUIRED_CONFIRMATIONS: u32 = 6;
const FUNDING_VALUE_SAT: u64 = 100_000;
const CLAIM_VALUE_SAT: u64 = 99_000;
const LEZ_AMOUNT: u128 = 5_000;
const LEZ_INITIALIZATION_ID: &str = "lez-init-01";
const LEZ_FUNDING_ID: &str = "lez-fund-02";

struct Fixture {
    record: BtcAgreementRecordV1,
    wire: Vec<u8>,
    lock_effects: BtcPreparedLockEffectsV1,
    funding: Transaction,
}

fn secret(value: u8) -> SecretKey {
    SecretKey::from_slice(&[value; 32]).expect("fixed secret")
}

fn compressed_public_key(secret: &SecretKey) -> [u8; 33] {
    PublicKey::from_secret_key(&Secp256k1::new(), secret).serialize()
}

fn x_only_public_key(secret: &SecretKey) -> [u8; 32] {
    Keypair::from_secret_key(&Secp256k1::new(), secret)
        .x_only_public_key()
        .0
        .serialize()
}

fn destination(secret: &SecretKey) -> Vec<u8> {
    let key = Keypair::from_secret_key(&Secp256k1::new(), secret)
        .x_only_public_key()
        .0;
    ScriptBuf::new_p2tr(&Secp256k1::verification_only(), key, None).into_bytes()
}

fn agreement_signature(secret: &SecretKey, commitment: [u8; 32]) -> [u8; 64] {
    Secp256k1::new()
        .sign_schnorr_no_aux_rand(
            &Message::from_digest(commitment),
            &Keypair::from_secret_key(&Secp256k1::new(), secret),
        )
        .serialize()
}

#[allow(clippy::too_many_lines)]
fn fixture(direction: SwapDirection) -> Fixture {
    let maker_secret = secret(1);
    let taker_secret = secret(2);
    let maker_refund_secret = secret(3);
    let taker_refund_secret = secret(4);
    let maker_claim_secret = secret(5);
    let taker_claim_secret = secret(6);
    let adaptor_secret = secret(7);

    let participants = BtcParticipantsV1::new(
        BtcParticipantIdentityV1::new(
            [10; 32],
            compressed_public_key(&maker_secret),
            x_only_public_key(&maker_refund_secret),
            destination(&maker_claim_secret),
        ),
        BtcParticipantIdentityV1::new(
            [11; 32],
            compressed_public_key(&taker_secret),
            x_only_public_key(&taker_refund_secret),
            destination(&taker_claim_secret),
        ),
    );
    let adaptor_point = compressed_public_key(&adaptor_secret);
    let aggregate = AdaptorSessionContext::untweaked(
        [
            compressed_public_key(&maker_secret),
            compressed_public_key(&taker_secret),
        ],
        [30; 32],
        adaptor_point,
        [31; 32],
    )
    .expect("aggregate context")
    .output_key();
    let bitcoin_funder = match direction {
        SwapDirection::TakerSellsForeign => Participant::Taker,
        SwapDirection::TakerSellsLez => Participant::Maker,
    };
    let refund_key = participants
        .for_participant(bitcoin_funder)
        .bitcoin_refund_key();
    let contract = P2trSwapOutput::new(
        TwoPartyAggregateKey::from_bytes(aggregate).expect("aggregate key"),
        RefundXOnlyKey::from_bytes(*refund_key).expect("refund key"),
        CsvBlockDelay::new(144).expect("CSV"),
    )
    .expect("P2TR contract");
    let funding = Transaction {
        version: Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint {
                txid: Txid::from_byte_array([42; 32]),
                vout: 0,
            },
            script_sig: ScriptBuf::from_bytes(vec![0x51]),
            sequence: Sequence::MAX,
            witness: Witness::default(),
        }],
        output: vec![TxOut {
            value: Amount::from_sat(FUNDING_VALUE_SAT),
            script_pubkey: ScriptBuf::from_bytes(contract.script_pubkey_bytes().to_vec()),
        }],
    };
    let funding_terms =
        BtcFundingTermsV1::new(funding.compute_txid().to_byte_array(), 0, FUNDING_VALUE_SAT);
    let bitcoin_claimant = bitcoin_funder.other();
    let claim = CooperativeKeyPathSpend::new(
        &contract,
        OutPoint {
            txid: funding.compute_txid(),
            vout: 0,
        },
        Amount::from_sat(FUNDING_VALUE_SAT),
        vec![TxOut {
            value: Amount::from_sat(CLAIM_VALUE_SAT),
            script_pubkey: ScriptBuf::from_bytes(
                participants
                    .for_participant(bitcoin_claimant)
                    .claim_destination_script_pubkey()
                    .to_vec(),
            ),
        }],
    )
    .expect("cooperative claim");
    let lez_depositor = match direction {
        SwapDirection::TakerSellsForeign => Participant::Maker,
        SwapDirection::TakerSellsLez => Participant::Taker,
    };
    let lez_claimant = lez_depositor.other();
    let lez_refund_at_ms = match direction {
        SwapDirection::TakerSellsForeign => 1_700_000_100_000,
        SwapDirection::TakerSellsLez => 1_700_000_500_000,
    };
    let body = BtcAgreementBodyV1::new(
        [20; 32],
        direction,
        BtcChainPolicyV1::new(BITCOIN_GENESIS, REQUIRED_CONFIRMATIONS),
        participants.clone(),
        adaptor_point,
        BtcLezTermsV1::new(
            [17; 32],
            LEZ_GENESIS,
            [15; 32],
            [16; 32],
            [12; 32],
            [13; 32],
            [14; 32],
            *participants
                .for_participant(lez_depositor)
                .lez_owner_account(),
            *participants
                .for_participant(lez_claimant)
                .lez_owner_account(),
            LEZ_AMOUNT,
            lez_refund_at_ms,
            [19; 32],
        ),
        BtcP2trTermsV1::from_contract(&contract),
        funding_terms,
        BtcClaimTermsV1::from_spend(&claim).expect("claim terms"),
        BtcRecoveryPlanV1::new(
            1_000,
            1_144,
            1_699_999_800,
            1_700_000_100,
            1_700_000_500,
            300,
        ),
    );
    let commitment = body.commitment();
    let record = BtcAgreementRecordV1::from_parts(
        BTC_AGREEMENT_SCHEMA_V1,
        body,
        commitment,
        agreement_signature(&maker_secret, commitment),
        agreement_signature(&taker_secret, commitment),
    );
    let wire = record.encode_wire().expect("agreement wire");
    let bitcoin =
        PreparedBitcoinFundingV1::new(funding.compute_txid().to_string(), serialize(&funding))
            .expect("exact Bitcoin funding");
    let lez = PreparedLezFundingV1::new(
        LEZ_INITIALIZATION_ID,
        vec![1, 2, 3],
        LEZ_FUNDING_ID,
        vec![4, 5, 6],
    )
    .expect("exact LEZ effects");
    Fixture {
        record,
        wire,
        lock_effects: BtcPreparedLockEffectsV1::new(bitcoin, lez),
        funding,
    }
}

fn bitcoin_evidence(fixture: &Fixture, confirmations: u32) -> BtcFirstLockEvidenceV1 {
    BtcFirstLockEvidenceV1::Bitcoin(
        BitcoinFirstLockEvidenceV1::new(
            BITCOIN_GENESIS,
            serialize(&fixture.funding),
            confirmations,
        )
        .expect("Bitcoin evidence"),
    )
}

fn lez_evidence(finalized: bool) -> BtcFirstLockEvidenceV1 {
    BtcFirstLockEvidenceV1::Lez(
        LezFirstLockEvidenceV1::new(
            LEZ_GENESIS,
            LEZ_INITIALIZATION_ID,
            vec![1, 2, 3],
            LEZ_FUNDING_ID,
            vec![4, 5, 6],
            [13; 32],
            [14; 32],
            LEZ_AMOUNT,
            finalized,
        )
        .expect("LEZ evidence"),
    )
}

#[test]
fn both_directions_expose_role_fixed_exact_plans_and_restart() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        let fixture = fixture(direction);
        for role in [Participant::Taker, Participant::Maker] {
            let sdk = BtcPairSdk::new(
                role,
                BtcChainPolicyV1::new(BITCOIN_GENESIS, REQUIRED_CONFIRMATIONS),
            );
            let accepted = sdk.accept_wire(&fixture.wire).expect("accepted agreement");
            assert_eq!(accepted.local_participant(), role);
            assert_eq!(accepted.revision(), 0);
            let active = sdk
                .activate(accepted, fixture.lock_effects.clone())
                .expect("active");
            let status = active.status();
            assert_eq!(status.local_participant(), role);
            assert_eq!(status.direction(), direction);
            assert_eq!(status.phase(), Phase::Offered);
            assert_eq!(status.revision(), 0);
            assert_eq!(
                status.next_action(),
                match (role, direction) {
                    (Participant::Maker, _) => BtcLifecycleActionV1::AwaitTakerFirstLock,
                    (Participant::Taker, SwapDirection::TakerSellsForeign) => {
                        BtcLifecycleActionV1::PublishBitcoinFirstLock
                    }
                    (Participant::Taker, SwapDirection::TakerSellsLez) => {
                        BtcLifecycleActionV1::PublishLezFirstLock
                    }
                }
            );
            assert_eq!(
                active.first_lock_plan().steps().len(),
                match direction {
                    SwapDirection::TakerSellsForeign => 1,
                    SwapDirection::TakerSellsLez => 2,
                }
            );
            let first = match direction {
                SwapDirection::TakerSellsForeign => {
                    active.validate_first_lock(&bitcoin_evidence(&fixture, REQUIRED_CONFIRMATIONS))
                }
                SwapDirection::TakerSellsLez => active.validate_first_lock(&lez_evidence(true)),
            }
            .expect("confirmed first lock");
            assert_eq!(
                active
                    .second_lock_plan(&first)
                    .expect("second lock")
                    .steps()
                    .len(),
                match direction {
                    SwapDirection::TakerSellsForeign => 2,
                    SwapDirection::TakerSellsLez => 1,
                }
            );
            assert_eq!(
                active.claim_order(),
                match direction {
                    SwapDirection::TakerSellsForeign => ClaimOrder::LEZ_THEN_FOREIGN,
                    SwapDirection::TakerSellsLez => ClaimOrder::FOREIGN_THEN_LEZ,
                }
            );
            let resumed = sdk
                .resume(active.durable_envelope())
                .expect("offline resume");
            assert_eq!(resumed.status(), status);
            assert_eq!(
                resumed.first_lock_plan().commitment(),
                active.first_lock_plan().commitment()
            );
        }
    }
}

#[test]
fn malformed_unsigned_or_identity_drifted_bitcoin_effects_fail_closed() {
    assert!(matches!(
        PreparedBitcoinFundingV1::new("not-the-txid", vec![1, 2, 3]),
        Err(BtcSdkError::MalformedBitcoinFunding(_))
    ));

    let mut unsigned = fixture(SwapDirection::TakerSellsForeign).funding;
    unsigned.input[0].script_sig = ScriptBuf::new();
    assert!(matches!(
        PreparedBitcoinFundingV1::new(unsigned.compute_txid().to_string(), serialize(&unsigned)),
        Err(BtcSdkError::UnsignedBitcoinFunding)
    ));

    let fixture = fixture(SwapDirection::TakerSellsForeign);
    let mut different = fixture.funding.clone();
    different.input[0].previous_output.vout = 7;
    let drifted = BtcPreparedLockEffectsV1::new(
        PreparedBitcoinFundingV1::new(different.compute_txid().to_string(), serialize(&different))
            .expect("valid but agreement-drifted transaction"),
        fixture.lock_effects.lez().clone(),
    );
    let sdk = BtcPairSdk::new(
        Participant::Taker,
        BtcChainPolicyV1::new(BITCOIN_GENESIS, REQUIRED_CONFIRMATIONS),
    );
    let accepted = sdk.accept_wire(&fixture.wire).expect("accepted");
    assert!(matches!(
        sdk.activate(accepted, drifted),
        Err(BtcSdkError::BitcoinFundingAgreementMismatch)
    ));
}

#[test]
fn first_lock_finality_chain_and_role_substitution_fail_closed() {
    let bitcoin = fixture(SwapDirection::TakerSellsForeign);
    let taker = BtcPairSdk::new(
        Participant::Taker,
        BtcChainPolicyV1::new(BITCOIN_GENESIS, REQUIRED_CONFIRMATIONS),
    );
    let active = taker
        .activate(
            taker.accept_wire(&bitcoin.wire).expect("accepted"),
            bitcoin.lock_effects.clone(),
        )
        .expect("active");
    assert!(matches!(
        active.validate_first_lock(&bitcoin_evidence(&bitcoin, REQUIRED_CONFIRMATIONS - 1)),
        Err(BtcSdkError::FirstLockConfirmationLag { .. })
    ));
    assert!(matches!(
        active.validate_first_lock(&lez_evidence(true)),
        Err(BtcSdkError::WrongFirstLockChain)
    ));

    let lez = fixture(SwapDirection::TakerSellsLez);
    let active = taker
        .activate(
            taker.accept_wire(&lez.wire).expect("accepted"),
            lez.lock_effects.clone(),
        )
        .expect("active");
    assert!(matches!(
        active.validate_first_lock(&lez_evidence(false)),
        Err(BtcSdkError::FirstLockNotFinalized)
    ));

    let maker = BtcPairSdk::new(
        Participant::Maker,
        BtcChainPolicyV1::new(BITCOIN_GENESIS, REQUIRED_CONFIRMATIONS),
    );
    let maker_accepted = maker.accept_wire(&lez.wire).expect("maker accepted");
    assert!(matches!(
        taker.activate(maker_accepted, lez.lock_effects),
        Err(BtcSdkError::LocalRoleMismatch { .. })
    ));
}

#[test]
fn bitcoin_first_lock_rejects_same_txid_with_different_witness_bytes() {
    let fixture = fixture(SwapDirection::TakerSellsForeign);
    let sdk = BtcPairSdk::new(
        Participant::Taker,
        BtcChainPolicyV1::new(BITCOIN_GENESIS, REQUIRED_CONFIRMATIONS),
    );
    let active = sdk
        .activate(
            sdk.accept_wire(&fixture.wire).expect("accepted"),
            fixture.lock_effects.clone(),
        )
        .expect("active");
    let mut witness_drift = fixture.funding.clone();
    witness_drift.input[0].witness = Witness::from_slice(&[[0x55; 64]]);
    assert_eq!(witness_drift.compute_txid(), fixture.funding.compute_txid());
    assert_ne!(
        witness_drift.compute_wtxid(),
        fixture.funding.compute_wtxid()
    );
    let evidence = BtcFirstLockEvidenceV1::Bitcoin(
        BitcoinFirstLockEvidenceV1::new(
            BITCOIN_GENESIS,
            serialize(&witness_drift),
            REQUIRED_CONFIRMATIONS,
        )
        .expect("structurally valid observed funding"),
    );
    assert!(matches!(
        active.validate_first_lock(&evidence),
        Err(BtcSdkError::FirstLockPlanMismatch)
    ));
}

#[test]
fn lez_first_lock_rejects_same_ids_with_different_exact_bytes() {
    let fixture = fixture(SwapDirection::TakerSellsLez);
    let sdk = BtcPairSdk::new(
        Participant::Taker,
        BtcChainPolicyV1::new(BITCOIN_GENESIS, REQUIRED_CONFIRMATIONS),
    );
    let active = sdk
        .activate(
            sdk.accept_wire(&fixture.wire).expect("accepted"),
            fixture.lock_effects.clone(),
        )
        .expect("active");
    let evidence = BtcFirstLockEvidenceV1::Lez(
        LezFirstLockEvidenceV1::new(
            LEZ_GENESIS,
            LEZ_INITIALIZATION_ID,
            vec![1, 2, 99],
            LEZ_FUNDING_ID,
            vec![4, 5, 6],
            [13; 32],
            [14; 32],
            LEZ_AMOUNT,
            true,
        )
        .expect("structurally valid observed LEZ effects"),
    );
    assert!(matches!(
        active.validate_first_lock(&evidence),
        Err(BtcSdkError::FirstLockPlanMismatch)
    ));
}

#[test]
fn common_protocol_validates_terms_but_refuses_incomplete_recovery_preparation() {
    let fixture = fixture(SwapDirection::TakerSellsForeign);
    let sdk = BtcPairSdk::new(
        Participant::Taker,
        BtcChainPolicyV1::new(BITCOIN_GENESIS, REQUIRED_CONFIRMATIONS),
    );
    let terms = BtcProtocolTermsV1::new(fixture.record.clone(), fixture.lock_effects.clone());
    let validated = sdk.validate_terms(&terms).expect("validated terms");
    assert!(matches!(
        sdk.prepare(validated),
        Err(BtcSdkError::UnsupportedCapability(
            BtcProtocolCapabilityGapV1::PreLockRecovery
        ))
    ));
}

#[test]
fn durable_resume_rejects_role_and_unsupported_transition_revision() {
    let fixture = fixture(SwapDirection::TakerSellsLez);
    let sdk = BtcPairSdk::new(
        Participant::Taker,
        BtcChainPolicyV1::new(BITCOIN_GENESIS, REQUIRED_CONFIRMATIONS),
    );
    let wrong_role = BtcActiveSwapEnvelopeV1::from_parts(
        fixture.wire.clone(),
        Participant::Maker,
        0,
        fixture.lock_effects.clone(),
    );
    assert!(matches!(
        sdk.resume(wrong_role),
        Err(BtcSdkError::LocalRoleMismatch { .. })
    ));
    let future = BtcActiveSwapEnvelopeV1::from_parts(
        fixture.wire,
        Participant::Taker,
        1,
        fixture.lock_effects,
    );
    assert!(matches!(
        sdk.resume(future),
        Err(BtcSdkError::UnsupportedResumeRevision(1))
    ));
}
