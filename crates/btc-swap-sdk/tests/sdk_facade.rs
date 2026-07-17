use bitcoin::absolute::LockTime;
use bitcoin::consensus::serialize;
use bitcoin::hashes::Hash as _;
use bitcoin::secp256k1::{Keypair, Message, PublicKey, Secp256k1, SecretKey};
use bitcoin::transaction::Version;
use bitcoin::{Amount, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid, Witness};
use lez_btc_swap_sdk::{
    AdaptorSessionContext, AdaptorSigner, BTC_AGREEMENT_SCHEMA_V1, BitcoinCanonicalRecoveryStateV1,
    BitcoinFirstLockEvidenceV1, BitcoinRevealingClaimEvidenceV1, BtcActiveSwapEnvelopeV1,
    BtcAgreementBodyV1, BtcAgreementRecordV1, BtcCanonicalRecoveryStateV1, BtcChainPolicyV1,
    BtcClaimTermsV1, BtcFirstLockEvidenceV1, BtcFundingTermsV1, BtcLezTermsV1,
    BtcLifecycleActionV1, BtcP2trTermsV1, BtcPairSdk, BtcParticipantIdentityV1, BtcParticipantsV1,
    BtcPreparedClaimEffectsV1, BtcPreparedLockEffectsV1, BtcPreparedProtocolV1,
    BtcPreparedRecoveryEffectsV1, BtcProtocolTermsV1, BtcRecoveryActionV1, BtcRecoveryPlanV1,
    BtcRecoveryWaitReasonV1, BtcRevealingClaimEvidenceV1, BtcSdkError, CooperativeKeyPathSpend,
    CsvBlockDelay, LezCanonicalRecoveryStateV1, LezFirstLockEvidenceV1,
    LezRevealingClaimEvidenceV1, P2trSwapOutput, PreparedBitcoinFundingV1, PreparedBitcoinRefundV1,
    PreparedLezClaimTemplateV1, PreparedLezFundingV1, PreparedLezRefundV1, RefundXOnlyKey,
    SigningRole, TwoPartyAggregateKey, adapt_presignature,
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
const LEZ_CLAIM_ID: &str = "lez-claim-03";
const LEZ_CLAIM_SIGNATURE_OFFSET: usize = 9;
const LEZ_REFUND_ID: &str = "lez-refund-04";
const BITCOIN_REFUND_HEIGHT: u32 = 1_144;
const LEZ_FOREIGN_REFUND_SECONDS: u64 = 1_700_000_100;
const LEZ_REVERSE_REFUND_SECONDS: u64 = 1_700_000_500;

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

fn complete_presignature(
    context: &AdaptorSessionContext,
    maker_secret: &SecretKey,
    taker_secret: &SecretKey,
) -> [u8; 65] {
    let mut maker = AdaptorSigner::new(
        context.clone(),
        SigningRole::Maker,
        maker_secret.secret_bytes(),
    )
    .expect("maker signer");
    let mut taker = AdaptorSigner::new(
        context.clone(),
        SigningRole::Taker,
        taker_secret.secret_bytes(),
    )
    .expect("taker signer");
    maker
        .accept_peer_commitment(taker.nonce_commitment())
        .expect("maker accepts commitment");
    taker
        .accept_peer_commitment(maker.nonce_commitment())
        .expect("taker accepts commitment");
    let maker_nonce = maker.public_nonce().expect("maker nonce");
    let taker_nonce = taker.public_nonce().expect("taker nonce");
    maker
        .accept_peer_nonce(taker_nonce)
        .expect("maker accepts nonce");
    taker
        .accept_peer_nonce(maker_nonce)
        .expect("taker accepts nonce");
    let maker_partial = maker
        .create_partial_signature()
        .expect("maker partial signature");
    let taker_partial = taker
        .create_partial_signature()
        .expect("taker partial signature");
    maker
        .accept_peer_partial_signature(taker_partial)
        .expect("maker accepts partial");
    taker
        .accept_peer_partial_signature(maker_partial)
        .expect("taker accepts partial");
    let presignature = maker.presignature().expect("aggregate presignature");
    assert_eq!(
        presignature,
        taker.presignature().expect("same presignature")
    );
    presignature
}

fn prepared_claims(fixture: &Fixture) -> (BtcPreparedClaimEffectsV1, [u8; 65], [u8; 65], Vec<u8>) {
    let agreement = lez_btc_swap_sdk::BtcAgreementV1::validate(fixture.record.clone())
        .expect("validated agreement");
    let maker_secret = secret(1);
    let taker_secret = secret(2);
    let bitcoin_context = agreement
        .claim_adaptor_session_context(lez_btc_swap_sdk::BtcAdaptorSessionDomain::Bitcoin)
        .expect("agreement-bound Bitcoin claim context");
    let lez_context = agreement
        .claim_adaptor_session_context(lez_btc_swap_sdk::BtcAdaptorSessionDomain::Lez)
        .expect("agreement-bound LEZ claim context");
    let bitcoin_presignature =
        complete_presignature(&bitcoin_context, &maker_secret, &taker_secret);
    let lez_presignature = complete_presignature(&lez_context, &maker_secret, &taker_secret);
    let mut lez_template = b"lez.claim".to_vec();
    lez_template.extend_from_slice(&[0; 64]);
    lez_template.extend_from_slice(b".v1");
    let claims = BtcPreparedClaimEffectsV1::new(
        &agreement,
        bitcoin_presignature,
        lez_presignature,
        PreparedLezClaimTemplateV1::new(
            LEZ_CLAIM_ID,
            lez_template.clone(),
            LEZ_CLAIM_SIGNATURE_OFFSET,
        )
        .expect("bounded LEZ signature template"),
    );
    (claims, bitcoin_presignature, lez_presignature, lez_template)
}

fn fully_prepared(
    direction: SwapDirection,
    role: Participant,
) -> (BtcPairSdk, BtcPreparedProtocolV1) {
    let fixture = fixture(direction);
    let agreement = lez_btc_swap_sdk::BtcAgreementV1::validate(fixture.record.clone())
        .expect("validated agreement");
    let (claims, ..) = prepared_claims(&fixture);
    let refund_secret = match direction {
        SwapDirection::TakerSellsForeign => secret(4),
        SwapDirection::TakerSellsLez => secret(3),
    };
    let signature = Secp256k1::new()
        .sign_schnorr_no_aux_rand(
            &Message::from_digest(agreement.bitcoin_refund().sighash_bytes()),
            &Keypair::from_secret_key(&Secp256k1::new(), &refund_secret),
        )
        .serialize();
    let recovery = BtcPreparedRecoveryEffectsV1::new(
        PreparedBitcoinRefundV1::new(&agreement, signature)
            .expect("canonical signed Bitcoin refund"),
        PreparedLezRefundV1::new(&agreement, LEZ_REFUND_ID, b"signed.lez.refund.v1".to_vec())
            .expect("bounded signed LEZ refund"),
    );
    let sdk = BtcPairSdk::new(
        role,
        BtcChainPolicyV1::new(BITCOIN_GENESIS, REQUIRED_CONFIRMATIONS),
    );
    let terms = BtcProtocolTermsV1::new(fixture.record, fixture.lock_effects)
        .with_claim_effects(claims)
        .with_recovery_effects(recovery);
    let validated = sdk.validate_terms(&terms).expect("validated full terms");
    let prepared = sdk.prepare(validated).expect("fully prepared protocol");
    (sdk, prepared)
}

fn bitcoin_locked(prepared: &BtcPreparedProtocolV1) -> BitcoinCanonicalRecoveryStateV1 {
    BitcoinCanonicalRecoveryStateV1::locked(
        BITCOIN_GENESIS,
        *prepared.agreement().funding_terms().transaction_id(),
        REQUIRED_CONFIRMATIONS,
        true,
    )
}

fn bitcoin_refunded(prepared: &BtcPreparedProtocolV1) -> BitcoinCanonicalRecoveryStateV1 {
    BitcoinCanonicalRecoveryStateV1::refunded(
        BITCOIN_GENESIS,
        *prepared.agreement().funding_terms().transaction_id(),
        prepared
            .recovery_effects()
            .expect("full recovery effects")
            .bitcoin()
            .transaction_id()
            .to_byte_array(),
        REQUIRED_CONFIRMATIONS,
    )
}

fn lez_locked() -> LezCanonicalRecoveryStateV1 {
    LezCanonicalRecoveryStateV1::locked(
        LEZ_GENESIS,
        LEZ_INITIALIZATION_ID,
        LEZ_FUNDING_ID,
        true,
        true,
    )
    .expect("canonical LEZ lock")
}

fn lez_refunded() -> LezCanonicalRecoveryStateV1 {
    LezCanonicalRecoveryStateV1::refunded(
        LEZ_GENESIS,
        LEZ_INITIALIZATION_ID,
        LEZ_FUNDING_ID,
        LEZ_REFUND_ID,
        true,
    )
    .expect("canonical LEZ refund")
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
    let (claims, ..) = prepared_claims(&fixture);
    let sdk = BtcPairSdk::new(
        Participant::Taker,
        BtcChainPolicyV1::new(BITCOIN_GENESIS, REQUIRED_CONFIRMATIONS),
    );
    let terms = BtcProtocolTermsV1::new(fixture.record.clone(), fixture.lock_effects.clone())
        .with_claim_effects(claims);
    let validated = sdk.validate_terms(&terms).expect("validated terms");
    assert!(matches!(
        sdk.prepare(validated),
        Err(BtcSdkError::MissingRecoveryEffects)
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

#[test]
fn common_protocol_extracts_and_builds_exact_claims_in_both_directions() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        let fixture = fixture(direction);
        let (claims, bitcoin_presignature, lez_presignature, lez_template) =
            prepared_claims(&fixture);
        let sdk = BtcPairSdk::new(
            Participant::Maker,
            BtcChainPolicyV1::new(BITCOIN_GENESIS, REQUIRED_CONFIRMATIONS),
        );
        let terms = BtcProtocolTermsV1::new(fixture.record, fixture.lock_effects)
            .with_claim_effects(claims);
        let validated = sdk.validate_terms(&terms).expect("validated claim terms");
        let prepared = sdk.prepare_claims(validated).expect("claim-ready protocol");
        let adaptor_secret = zeroize::Zeroizing::new(secret(7).secret_bytes());

        let evidence = match direction {
            SwapDirection::TakerSellsForeign => {
                let context = prepared
                    .agreement()
                    .claim_adaptor_session_context(lez_btc_swap_sdk::BtcAdaptorSessionDomain::Lez)
                    .expect("LEZ context");
                let signature = adapt_presignature(&context, lez_presignature, adaptor_secret)
                    .expect("revealing LEZ signature");
                let mut exact_claim = lez_template;
                exact_claim[LEZ_CLAIM_SIGNATURE_OFFSET..LEZ_CLAIM_SIGNATURE_OFFSET + 64]
                    .copy_from_slice(&signature);
                BtcRevealingClaimEvidenceV1::Lez(
                    LezRevealingClaimEvidenceV1::new(
                        Participant::Taker,
                        LEZ_GENESIS,
                        LEZ_CLAIM_ID,
                        exact_claim,
                        signature,
                        true,
                    )
                    .expect("canonical LEZ claim evidence"),
                )
            }
            SwapDirection::TakerSellsLez => {
                let context = prepared
                    .agreement()
                    .claim_adaptor_session_context(
                        lez_btc_swap_sdk::BtcAdaptorSessionDomain::Bitcoin,
                    )
                    .expect("Bitcoin context");
                let signature = adapt_presignature(&context, bitcoin_presignature, adaptor_secret)
                    .expect("revealing Bitcoin signature");
                let claim = prepared
                    .agreement()
                    .cooperative_claim()
                    .clone()
                    .finalize(signature)
                    .expect("signed Bitcoin claim");
                BtcRevealingClaimEvidenceV1::Bitcoin(
                    BitcoinRevealingClaimEvidenceV1::new(
                        Participant::Taker,
                        BITCOIN_GENESIS,
                        serialize(&claim),
                        REQUIRED_CONFIRMATIONS,
                    )
                    .expect("canonical Bitcoin claim evidence"),
                )
            }
        };

        let material = sdk
            .validate_revealing_claim(&prepared, &evidence)
            .expect("agreement-bound extracted adaptor material");
        let first = sdk
            .build_followup_claim(&prepared, &material)
            .expect("exact follow-up claim");
        let replay = sdk
            .build_followup_claim(&prepared, &material)
            .expect("deterministic replay");
        assert_eq!(first, replay);
        let [step] = first.steps() else {
            panic!("one exact follow-up claim step");
        };
        match direction {
            SwapDirection::TakerSellsForeign => {
                assert_eq!(step.step().as_str(), "bitcoin.claim");
                let transaction: Transaction =
                    bitcoin::consensus::deserialize(step.exact_bytes().as_slice())
                        .expect("canonical follow-up transaction");
                assert_eq!(
                    transaction.compute_txid().to_string(),
                    step.expected_public_id().as_str()
                );
                assert_eq!(transaction.input[0].witness.len(), 1);
            }
            SwapDirection::TakerSellsLez => {
                assert_eq!(step.step().as_str(), "lez.claim");
                assert_eq!(step.expected_public_id().as_str(), LEZ_CLAIM_ID);
                assert_eq!(&step.exact_bytes().as_slice()[..9], b"lez.claim");
                assert_eq!(&step.exact_bytes().as_slice()[73..], b".v1");
            }
        }
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn claim_lifecycle_rejects_role_byte_and_adaptor_substitution() {
    let fixture = fixture(SwapDirection::TakerSellsForeign);
    let (claims, bitcoin_presignature, lez_presignature, mut lez_template) =
        prepared_claims(&fixture);
    let sdk = BtcPairSdk::new(
        Participant::Maker,
        BtcChainPolicyV1::new(BITCOIN_GENESIS, REQUIRED_CONFIRMATIONS),
    );
    let terms =
        BtcProtocolTermsV1::new(fixture.record, fixture.lock_effects).with_claim_effects(claims);
    let validated = sdk.validate_terms(&terms).expect("validated claim terms");
    let prepared = sdk.prepare_claims(validated).expect("claim-ready protocol");
    let bitcoin_context = prepared
        .agreement()
        .claim_adaptor_session_context(lez_btc_swap_sdk::BtcAdaptorSessionDomain::Bitcoin)
        .expect("Bitcoin context");
    let wrong_domain_signature = adapt_presignature(
        &bitcoin_context,
        bitcoin_presignature,
        zeroize::Zeroizing::new(secret(7).secret_bytes()),
    )
    .expect("valid Bitcoin-domain signature");
    lez_template[LEZ_CLAIM_SIGNATURE_OFFSET..LEZ_CLAIM_SIGNATURE_OFFSET + 64]
        .copy_from_slice(&wrong_domain_signature);
    let wrong_adaptor = BtcRevealingClaimEvidenceV1::Lez(
        LezRevealingClaimEvidenceV1::new(
            Participant::Taker,
            LEZ_GENESIS,
            LEZ_CLAIM_ID,
            lez_template,
            wrong_domain_signature,
            true,
        )
        .expect("bounded but substituted evidence"),
    );
    assert!(matches!(
        sdk.validate_revealing_claim(&prepared, &wrong_adaptor),
        Err(BtcSdkError::InvalidAdaptorClaim(_))
    ));

    let lez_context = prepared
        .agreement()
        .claim_adaptor_session_context(lez_btc_swap_sdk::BtcAdaptorSessionDomain::Lez)
        .expect("LEZ context");
    let valid_signature = adapt_presignature(
        &lez_context,
        lez_presignature,
        zeroize::Zeroizing::new(secret(7).secret_bytes()),
    )
    .expect("valid LEZ signature");
    let mut exact = b"lez.claim".to_vec();
    exact.extend_from_slice(&valid_signature);
    exact.extend_from_slice(b".v1");
    let wrong_network = BtcRevealingClaimEvidenceV1::Lez(
        LezRevealingClaimEvidenceV1::new(
            Participant::Taker,
            [0xff; 32],
            LEZ_CLAIM_ID,
            exact.clone(),
            valid_signature,
            true,
        )
        .expect("bounded wrong-network LEZ evidence"),
    );
    assert!(matches!(
        sdk.validate_revealing_claim(&prepared, &wrong_network),
        Err(BtcSdkError::RevealingClaimNetworkMismatch)
    ));
    let nonfinal = BtcRevealingClaimEvidenceV1::Lez(
        LezRevealingClaimEvidenceV1::new(
            Participant::Taker,
            LEZ_GENESIS,
            LEZ_CLAIM_ID,
            exact.clone(),
            valid_signature,
            false,
        )
        .expect("bounded non-final LEZ evidence"),
    );
    assert!(matches!(
        sdk.validate_revealing_claim(&prepared, &nonfinal),
        Err(BtcSdkError::RevealingClaimNotFinalized)
    ));
    exact[0] ^= 1;
    let byte_drift = BtcRevealingClaimEvidenceV1::Lez(
        LezRevealingClaimEvidenceV1::new(
            Participant::Taker,
            LEZ_GENESIS,
            LEZ_CLAIM_ID,
            exact,
            valid_signature,
            true,
        )
        .expect("bounded drifted evidence"),
    );
    assert!(matches!(
        sdk.validate_revealing_claim(&prepared, &byte_drift),
        Err(BtcSdkError::RevealingClaimPlanMismatch)
    ));

    let role_substitution = BtcRevealingClaimEvidenceV1::Lez(
        LezRevealingClaimEvidenceV1::new(
            Participant::Maker,
            LEZ_GENESIS,
            LEZ_CLAIM_ID,
            {
                let mut bytes = b"lez.claim".to_vec();
                bytes.extend_from_slice(&valid_signature);
                bytes.extend_from_slice(b".v1");
                bytes
            },
            valid_signature,
            true,
        )
        .expect("bounded role-substituted evidence"),
    );
    assert!(matches!(
        sdk.validate_revealing_claim(&prepared, &role_substitution),
        Err(BtcSdkError::RevealingClaimRoleMismatch { .. })
    ));
}

#[test]
#[allow(clippy::too_many_lines)]
fn bitcoin_revealing_claim_rejects_role_byte_adaptor_and_sdk_substitution() {
    let fixture = fixture(SwapDirection::TakerSellsLez);
    let (claims, bitcoin_presignature, lez_presignature, _) = prepared_claims(&fixture);
    let maker = BtcPairSdk::new(
        Participant::Maker,
        BtcChainPolicyV1::new(BITCOIN_GENESIS, REQUIRED_CONFIRMATIONS),
    );
    let terms =
        BtcProtocolTermsV1::new(fixture.record, fixture.lock_effects).with_claim_effects(claims);
    let validated = maker.validate_terms(&terms).expect("validated claim terms");
    let prepared = maker
        .prepare_claims(validated)
        .expect("claim-ready protocol");
    let bitcoin_context = prepared
        .agreement()
        .claim_adaptor_session_context(lez_btc_swap_sdk::BtcAdaptorSessionDomain::Bitcoin)
        .expect("Bitcoin context");
    let bitcoin_signature = adapt_presignature(
        &bitcoin_context,
        bitcoin_presignature,
        zeroize::Zeroizing::new(secret(7).secret_bytes()),
    )
    .expect("valid Bitcoin signature");
    let bitcoin_claim = prepared
        .agreement()
        .cooperative_claim()
        .clone()
        .finalize(bitcoin_signature)
        .expect("signed Bitcoin claim");
    let valid = BtcRevealingClaimEvidenceV1::Bitcoin(
        BitcoinRevealingClaimEvidenceV1::new(
            Participant::Taker,
            BITCOIN_GENESIS,
            serialize(&bitcoin_claim),
            REQUIRED_CONFIRMATIONS,
        )
        .expect("canonical Bitcoin evidence"),
    );
    let _material = maker
        .validate_revealing_claim(&prepared, &valid)
        .expect("valid revealing claim");

    let wrong_network = BtcRevealingClaimEvidenceV1::Bitcoin(
        BitcoinRevealingClaimEvidenceV1::new(
            Participant::Taker,
            [0xff; 32],
            serialize(&bitcoin_claim),
            REQUIRED_CONFIRMATIONS,
        )
        .expect("bounded wrong-network Bitcoin evidence"),
    );
    assert!(matches!(
        maker.validate_revealing_claim(&prepared, &wrong_network),
        Err(BtcSdkError::RevealingClaimNetworkMismatch)
    ));
    let lagging = BtcRevealingClaimEvidenceV1::Bitcoin(
        BitcoinRevealingClaimEvidenceV1::new(
            Participant::Taker,
            BITCOIN_GENESIS,
            serialize(&bitcoin_claim),
            REQUIRED_CONFIRMATIONS - 1,
        )
        .expect("bounded lagging Bitcoin evidence"),
    );
    assert!(matches!(
        maker.validate_revealing_claim(&prepared, &lagging),
        Err(BtcSdkError::RevealingClaimConfirmationLag { .. })
    ));

    let wrong_role = BtcRevealingClaimEvidenceV1::Bitcoin(
        BitcoinRevealingClaimEvidenceV1::new(
            Participant::Maker,
            BITCOIN_GENESIS,
            serialize(&bitcoin_claim),
            REQUIRED_CONFIRMATIONS,
        )
        .expect("bounded wrong-role evidence"),
    );
    assert!(matches!(
        maker.validate_revealing_claim(&prepared, &wrong_role),
        Err(BtcSdkError::RevealingClaimRoleMismatch { .. })
    ));

    let mut byte_drift = bitcoin_claim.clone();
    byte_drift.output[0].value = Amount::from_sat(CLAIM_VALUE_SAT - 1);
    let byte_drift = BtcRevealingClaimEvidenceV1::Bitcoin(
        BitcoinRevealingClaimEvidenceV1::new(
            Participant::Taker,
            BITCOIN_GENESIS,
            serialize(&byte_drift),
            REQUIRED_CONFIRMATIONS,
        )
        .expect("bounded byte-drifted evidence"),
    );
    assert!(matches!(
        maker.validate_revealing_claim(&prepared, &byte_drift),
        Err(BtcSdkError::RevealingClaimPlanMismatch)
    ));

    let lez_context = prepared
        .agreement()
        .claim_adaptor_session_context(lez_btc_swap_sdk::BtcAdaptorSessionDomain::Lez)
        .expect("LEZ context");
    let wrong_domain_signature = adapt_presignature(
        &lez_context,
        lez_presignature,
        zeroize::Zeroizing::new(secret(7).secret_bytes()),
    )
    .expect("valid LEZ-domain signature");
    let mut wrong_adaptor = bitcoin_claim;
    wrong_adaptor.input[0].witness = Witness::from_slice(&[wrong_domain_signature]);
    let wrong_adaptor = BtcRevealingClaimEvidenceV1::Bitcoin(
        BitcoinRevealingClaimEvidenceV1::new(
            Participant::Taker,
            BITCOIN_GENESIS,
            serialize(&wrong_adaptor),
            REQUIRED_CONFIRMATIONS,
        )
        .expect("bounded wrong-adaptor evidence"),
    );
    assert!(matches!(
        maker.validate_revealing_claim(&prepared, &wrong_adaptor),
        Err(BtcSdkError::InvalidAdaptorClaim(_))
    ));

    let taker = BtcPairSdk::new(
        Participant::Taker,
        BtcChainPolicyV1::new(BITCOIN_GENESIS, REQUIRED_CONFIRMATIONS),
    );
    assert!(matches!(
        taker.validate_revealing_claim(&prepared, &valid),
        Err(BtcSdkError::FollowupClaimRoleMismatch { .. })
    ));
}

#[test]
fn protocol_terms_reject_substituted_claim_presignatures_before_prepare() {
    let current = fixture(SwapDirection::TakerSellsForeign);
    let (_, mut bitcoin_presignature, lez_presignature, lez_template) = prepared_claims(&current);
    bitcoin_presignature[64] ^= 1;
    let claims = BtcPreparedClaimEffectsV1::new(
        &lez_btc_swap_sdk::BtcAgreementV1::validate(current.record.clone())
            .expect("validated current agreement"),
        bitcoin_presignature,
        lez_presignature,
        PreparedLezClaimTemplateV1::new(LEZ_CLAIM_ID, lez_template, LEZ_CLAIM_SIGNATURE_OFFSET)
            .expect("bounded LEZ template"),
    );
    let sdk = BtcPairSdk::new(
        Participant::Maker,
        BtcChainPolicyV1::new(BITCOIN_GENESIS, REQUIRED_CONFIRMATIONS),
    );
    let terms = BtcProtocolTermsV1::new(current.record.clone(), current.lock_effects.clone())
        .with_claim_effects(claims);
    assert!(matches!(
        sdk.validate_terms(&terms),
        Err(BtcSdkError::InvalidAdaptorClaim(_))
    ));

    let other = fixture(SwapDirection::TakerSellsLez);
    let (other_claims, ..) = prepared_claims(&other);
    let substituted_agreement = BtcProtocolTermsV1::new(current.record, current.lock_effects)
        .with_claim_effects(other_claims);
    assert!(matches!(
        sdk.validate_terms(&substituted_agreement),
        Err(BtcSdkError::ClaimPreparationAgreementMismatch)
    ));
}

#[test]
fn common_prepare_requires_and_accepts_agreement_bound_signed_refunds() {
    let fixture = fixture(SwapDirection::TakerSellsForeign);
    let agreement = lez_btc_swap_sdk::BtcAgreementV1::validate(fixture.record.clone())
        .expect("validated agreement");
    let (claims, ..) = prepared_claims(&fixture);
    let refund_message = Message::from_digest(agreement.bitcoin_refund().sighash_bytes());
    let refund_signature = Secp256k1::new()
        .sign_schnorr_no_aux_rand(
            &refund_message,
            &Keypair::from_secret_key(&Secp256k1::new(), &secret(4)),
        )
        .serialize();
    let recovery = BtcPreparedRecoveryEffectsV1::new(
        PreparedBitcoinRefundV1::new(&agreement, refund_signature)
            .expect("canonical signed Bitcoin refund"),
        PreparedLezRefundV1::new(&agreement, LEZ_REFUND_ID, b"signed.lez.refund.v1".to_vec())
            .expect("bounded signed LEZ refund"),
    );
    let sdk = BtcPairSdk::new(
        Participant::Taker,
        BtcChainPolicyV1::new(BITCOIN_GENESIS, REQUIRED_CONFIRMATIONS),
    );
    let terms = BtcProtocolTermsV1::new(fixture.record, fixture.lock_effects)
        .with_claim_effects(claims)
        .with_recovery_effects(recovery);
    let validated = sdk.validate_terms(&terms).expect("validated full terms");
    let prepared = sdk.prepare(validated).expect("fully prepared protocol");
    let state = BtcCanonicalRecoveryStateV1::new(
        prepared.agreement(),
        1_143,
        1_700_000_499,
        BitcoinCanonicalRecoveryStateV1::locked(
            BITCOIN_GENESIS,
            prepared
                .agreement()
                .funding_terms()
                .transaction_id()
                .to_owned(),
            REQUIRED_CONFIRMATIONS,
            true,
        ),
        LezCanonicalRecoveryStateV1::absent(),
    );
    assert_eq!(
        sdk.recovery_action(&prepared, &state)
            .expect("pure recovery selection"),
        BtcRecoveryActionV1::Wait(BtcRecoveryWaitReasonV1::AwaitRefundDeadline)
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn recovery_boundaries_cover_first_lock_and_ordered_two_lock_timeouts() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        let (taker, prepared) = fully_prepared(direction, Participant::Taker);
        let maker = BtcPairSdk::new(
            Participant::Maker,
            BtcChainPolicyV1::new(BITCOIN_GENESIS, REQUIRED_CONFIRMATIONS),
        );
        let (first_before, first_boundary, expected_first) = match direction {
            SwapDirection::TakerSellsForeign => (
                BtcCanonicalRecoveryStateV1::new(
                    prepared.agreement(),
                    BITCOIN_REFUND_HEIGHT - 1,
                    LEZ_FOREIGN_REFUND_SECONDS,
                    bitcoin_locked(&prepared),
                    LezCanonicalRecoveryStateV1::absent(),
                ),
                BtcCanonicalRecoveryStateV1::new(
                    prepared.agreement(),
                    BITCOIN_REFUND_HEIGHT,
                    LEZ_FOREIGN_REFUND_SECONDS,
                    bitcoin_locked(&prepared),
                    LezCanonicalRecoveryStateV1::absent(),
                ),
                "bitcoin.refund",
            ),
            SwapDirection::TakerSellsLez => (
                BtcCanonicalRecoveryStateV1::new(
                    prepared.agreement(),
                    BITCOIN_REFUND_HEIGHT,
                    LEZ_REVERSE_REFUND_SECONDS - 1,
                    BitcoinCanonicalRecoveryStateV1::absent(),
                    lez_locked(),
                ),
                BtcCanonicalRecoveryStateV1::new(
                    prepared.agreement(),
                    BITCOIN_REFUND_HEIGHT,
                    LEZ_REVERSE_REFUND_SECONDS,
                    BitcoinCanonicalRecoveryStateV1::absent(),
                    lez_locked(),
                ),
                "lez.refund",
            ),
        };
        assert_eq!(
            taker
                .recovery_action(&prepared, &first_before)
                .expect("pre-boundary first-lock recovery"),
            BtcRecoveryActionV1::Wait(BtcRecoveryWaitReasonV1::AwaitRefundDeadline)
        );
        let first_action = taker
            .recovery_action(&prepared, &first_boundary)
            .expect("boundary first-lock recovery");
        assert_eq!(
            first_action,
            taker
                .recovery_action(&prepared, &first_boundary)
                .expect("deterministic first-lock replay")
        );
        let first_plan = match first_action {
            BtcRecoveryActionV1::SubmitBitcoinRefund(plan)
            | BtcRecoveryActionV1::SubmitLezRefund(plan) => plan,
            other => panic!("unexpected boundary first-lock action: {other:?}"),
        };
        assert_eq!(first_plan.steps()[0].step().as_str(), expected_first);

        let (two_before, two_boundary, expected_earlier, after_earlier, expected_later) =
            match direction {
                SwapDirection::TakerSellsForeign => (
                    BtcCanonicalRecoveryStateV1::new(
                        prepared.agreement(),
                        BITCOIN_REFUND_HEIGHT,
                        LEZ_FOREIGN_REFUND_SECONDS - 1,
                        bitcoin_locked(&prepared),
                        lez_locked(),
                    ),
                    BtcCanonicalRecoveryStateV1::new(
                        prepared.agreement(),
                        BITCOIN_REFUND_HEIGHT,
                        LEZ_FOREIGN_REFUND_SECONDS,
                        bitcoin_locked(&prepared),
                        lez_locked(),
                    ),
                    "lez.refund",
                    BtcCanonicalRecoveryStateV1::new(
                        prepared.agreement(),
                        BITCOIN_REFUND_HEIGHT,
                        LEZ_FOREIGN_REFUND_SECONDS,
                        bitcoin_locked(&prepared),
                        lez_refunded(),
                    ),
                    "bitcoin.refund",
                ),
                SwapDirection::TakerSellsLez => (
                    BtcCanonicalRecoveryStateV1::new(
                        prepared.agreement(),
                        BITCOIN_REFUND_HEIGHT - 1,
                        LEZ_REVERSE_REFUND_SECONDS,
                        bitcoin_locked(&prepared),
                        lez_locked(),
                    ),
                    BtcCanonicalRecoveryStateV1::new(
                        prepared.agreement(),
                        BITCOIN_REFUND_HEIGHT,
                        LEZ_REVERSE_REFUND_SECONDS,
                        bitcoin_locked(&prepared),
                        lez_locked(),
                    ),
                    "bitcoin.refund",
                    BtcCanonicalRecoveryStateV1::new(
                        prepared.agreement(),
                        BITCOIN_REFUND_HEIGHT,
                        LEZ_REVERSE_REFUND_SECONDS,
                        bitcoin_refunded(&prepared),
                        lez_locked(),
                    ),
                    "lez.refund",
                ),
            };
        assert_eq!(
            maker
                .recovery_action(&prepared, &two_before)
                .expect("pre-boundary two-lock recovery"),
            BtcRecoveryActionV1::Wait(BtcRecoveryWaitReasonV1::AwaitRefundDeadline)
        );
        assert_eq!(
            taker
                .recovery_action(&prepared, &two_boundary)
                .expect("later owner cannot bypass earlier refund"),
            BtcRecoveryActionV1::Wait(BtcRecoveryWaitReasonV1::AwaitEarlierRefund)
        );
        let earlier = maker
            .recovery_action(&prepared, &two_boundary)
            .expect("earlier revealing-leg refund at boundary");
        let earlier_plan = match earlier {
            BtcRecoveryActionV1::SubmitBitcoinRefund(plan)
            | BtcRecoveryActionV1::SubmitLezRefund(plan) => plan,
            other => panic!("unexpected earlier refund action: {other:?}"),
        };
        assert_eq!(earlier_plan.steps()[0].step().as_str(), expected_earlier);
        let later = taker
            .recovery_action(&prepared, &after_earlier)
            .expect("later follow-up-leg refund at boundary");
        let later_plan = match later {
            BtcRecoveryActionV1::SubmitBitcoinRefund(plan)
            | BtcRecoveryActionV1::SubmitLezRefund(plan) => plan,
            other => panic!("unexpected later refund action: {other:?}"),
        };
        assert_eq!(later_plan.steps()[0].step().as_str(), expected_later);
    }
}

#[test]
fn claim_only_preparation_cannot_project_refunds() {
    let fixture = fixture(SwapDirection::TakerSellsForeign);
    let (claims, ..) = prepared_claims(&fixture);
    let sdk = BtcPairSdk::new(
        Participant::Taker,
        BtcChainPolicyV1::new(BITCOIN_GENESIS, REQUIRED_CONFIRMATIONS),
    );
    let terms =
        BtcProtocolTermsV1::new(fixture.record, fixture.lock_effects).with_claim_effects(claims);
    let validated = sdk.validate_terms(&terms).expect("validated claim terms");
    let prepared = sdk
        .prepare_claims(validated)
        .expect("claim-only prepared protocol");
    let state = BtcCanonicalRecoveryStateV1::new(
        prepared.agreement(),
        BITCOIN_REFUND_HEIGHT,
        LEZ_FOREIGN_REFUND_SECONDS,
        bitcoin_locked(&prepared),
        LezCanonicalRecoveryStateV1::absent(),
    );
    assert!(matches!(
        sdk.recovery_action(&prepared, &state),
        Err(BtcSdkError::MissingRecoveryEffects)
    ));
}

#[test]
fn recovery_rejects_state_finality_identity_order_and_agreement_substitution() {
    let (taker, prepared) = fully_prepared(SwapDirection::TakerSellsForeign, Participant::Taker);
    let lagging = BtcCanonicalRecoveryStateV1::new(
        prepared.agreement(),
        BITCOIN_REFUND_HEIGHT,
        LEZ_FOREIGN_REFUND_SECONDS,
        BitcoinCanonicalRecoveryStateV1::locked(
            BITCOIN_GENESIS,
            *prepared.agreement().funding_terms().transaction_id(),
            REQUIRED_CONFIRMATIONS - 1,
            true,
        ),
        LezCanonicalRecoveryStateV1::absent(),
    );
    assert!(matches!(
        taker.recovery_action(&prepared, &lagging),
        Err(BtcSdkError::RecoveryObservationLag { .. })
    ));

    let wrong_network = BtcCanonicalRecoveryStateV1::new(
        prepared.agreement(),
        BITCOIN_REFUND_HEIGHT,
        LEZ_FOREIGN_REFUND_SECONDS,
        BitcoinCanonicalRecoveryStateV1::locked(
            [0xff; 32],
            *prepared.agreement().funding_terms().transaction_id(),
            REQUIRED_CONFIRMATIONS,
            true,
        ),
        LezCanonicalRecoveryStateV1::absent(),
    );
    assert!(matches!(
        taker.recovery_action(&prepared, &wrong_network),
        Err(BtcSdkError::RecoveryNetworkMismatch)
    ));

    let wrong_identity = BtcCanonicalRecoveryStateV1::new(
        prepared.agreement(),
        BITCOIN_REFUND_HEIGHT,
        LEZ_FOREIGN_REFUND_SECONDS,
        BitcoinCanonicalRecoveryStateV1::locked(
            BITCOIN_GENESIS,
            [0xee; 32],
            REQUIRED_CONFIRMATIONS,
            true,
        ),
        LezCanonicalRecoveryStateV1::absent(),
    );
    assert!(matches!(
        taker.recovery_action(&prepared, &wrong_identity),
        Err(BtcSdkError::RecoveryPlanMismatch)
    ));

    let wrong_order = BtcCanonicalRecoveryStateV1::new(
        prepared.agreement(),
        BITCOIN_REFUND_HEIGHT,
        LEZ_FOREIGN_REFUND_SECONDS,
        bitcoin_refunded(&prepared),
        lez_locked(),
    );
    assert!(matches!(
        taker.recovery_action(&prepared, &wrong_order),
        Err(BtcSdkError::RecoveryOrderViolation)
    ));

    let (_, other) = fully_prepared(SwapDirection::TakerSellsLez, Participant::Taker);
    let substituted = BtcCanonicalRecoveryStateV1::new(
        other.agreement(),
        BITCOIN_REFUND_HEIGHT,
        LEZ_REVERSE_REFUND_SECONDS,
        BitcoinCanonicalRecoveryStateV1::absent(),
        lez_locked(),
    );
    assert!(matches!(
        taker.recovery_action(&prepared, &substituted),
        Err(BtcSdkError::RecoveryStateAgreementMismatch)
    ));

    let (reverse_taker, reverse) = fully_prepared(SwapDirection::TakerSellsLez, Participant::Taker);
    let nonfinal = BtcCanonicalRecoveryStateV1::new(
        reverse.agreement(),
        BITCOIN_REFUND_HEIGHT,
        LEZ_REVERSE_REFUND_SECONDS,
        BitcoinCanonicalRecoveryStateV1::absent(),
        LezCanonicalRecoveryStateV1::locked(
            LEZ_GENESIS,
            LEZ_INITIALIZATION_ID,
            LEZ_FUNDING_ID,
            false,
            true,
        )
        .expect("bounded non-final LEZ observation"),
    );
    assert!(matches!(
        reverse_taker.recovery_action(&reverse, &nonfinal),
        Err(BtcSdkError::RecoveryNotFinalized)
    ));
}

#[test]
fn signed_refund_preparation_rejects_wrong_role_and_cross_agreement_material() {
    for (direction, wrong_refund_secret) in [
        (SwapDirection::TakerSellsForeign, secret(3)),
        (SwapDirection::TakerSellsLez, secret(4)),
    ] {
        let fixture = fixture(direction);
        let agreement = lez_btc_swap_sdk::BtcAgreementV1::validate(fixture.record)
            .expect("validated agreement");
        let wrong_signature = Secp256k1::new()
            .sign_schnorr_no_aux_rand(
                &Message::from_digest(agreement.bitcoin_refund().sighash_bytes()),
                &Keypair::from_secret_key(&Secp256k1::new(), &wrong_refund_secret),
            )
            .serialize();
        assert!(matches!(
            PreparedBitcoinRefundV1::new(&agreement, wrong_signature),
            Err(BtcSdkError::InvalidBitcoinRefund(_))
        ));
    }

    let current = fixture(SwapDirection::TakerSellsForeign);
    let (claims, ..) = prepared_claims(&current);
    let (_, other) = fully_prepared(SwapDirection::TakerSellsLez, Participant::Taker);
    let sdk = BtcPairSdk::new(
        Participant::Taker,
        BtcChainPolicyV1::new(BITCOIN_GENESIS, REQUIRED_CONFIRMATIONS),
    );
    let substituted = BtcProtocolTermsV1::new(current.record, current.lock_effects)
        .with_claim_effects(claims)
        .with_recovery_effects(
            other
                .recovery_effects()
                .expect("full recovery effects")
                .clone(),
        );
    assert!(matches!(
        sdk.validate_terms(&substituted),
        Err(BtcSdkError::RecoveryPreparationAgreementMismatch)
    ));
}
