use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bitcoin::absolute::LockTime;
use bitcoin::consensus::serialize;
use bitcoin::hashes::Hash as _;
use bitcoin::secp256k1::{Keypair, Message, PublicKey, Secp256k1, SecretKey};
use bitcoin::transaction::Version;
use bitcoin::{Amount, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid, Witness};
use lez_btc_swap_sdk::{
    AdaptorSessionContext, AdaptorSigner, BTC_AGREEMENT_SCHEMA_V1, BitcoinBtcLifecyclePort,
    BitcoinCanonicalRecoveryStateV1, BitcoinFirstLockEvidenceV1, BitcoinFollowupClaimEvidenceV1,
    BitcoinRevealingClaimEvidenceV1, BtcActiveSwapEnvelopeV1, BtcAgreementBodyV1,
    BtcAgreementRecordV1, BtcCanonicalRecoveryStateV1, BtcChainPolicyV1, BtcClaimTermsV1,
    BtcFirstLockEvidenceV1, BtcFollowupClaimEvidenceV1, BtcFundingTermsV1, BtcLezTermsV1,
    BtcLifecycleActionV1, BtcLifecycleChainOutcomeV1, BtcLifecycleCodecError,
    BtcLifecycleDriveOutcomeV1, BtcLifecycleDriveRequestV1, BtcLifecycleRecordV1,
    BtcLifecycleRuntime, BtcLifecycleSdk, BtcLifecycleStore, BtcLifecycleStoreCompareExchangeV1,
    BtcLifecycleStoreCreateV1, BtcLifecycleTransitionOutcomeV1, BtcLifecycleTransitionV1,
    BtcP2trTermsV1, BtcPairSdk, BtcParticipantIdentityV1, BtcParticipantsV1,
    BtcPreparedClaimEffectsV1, BtcPreparedLockEffectsV1, BtcPreparedProtocolV1,
    BtcPreparedRecoveryEffectsV1, BtcProtocolTermsV1, BtcRecoveryActionV1, BtcRecoveryPlanV1,
    BtcRecoveryWaitReasonV1, BtcRevealingClaimEvidenceV1, BtcSdkError, CooperativeKeyPathSpend,
    CsvBlockDelay, InMemoryBtcLifecycleStore, LezBtcLifecyclePort, LezCanonicalRecoveryStateV1,
    LezFirstLockEvidenceV1, LezFollowupClaimEvidenceV1, LezRevealingClaimEvidenceV1,
    P2trSwapOutput, PlannedBitcoinFundingV1, PreparedBitcoinFundingV1, PreparedBitcoinRefundV1,
    PreparedLezClaimTemplateV1, PreparedLezFundingV1, PreparedLezRefundV1, RefundXOnlyKey,
    SigningRole, StoredBtcLifecycleSdk, TwoPartyAggregateKey, adapt_presignature,
};
use lez_swap_core::{Participant, Phase, SwapDirection};
use lez_swap_sdk_core::{ClaimOrder, NegotiationChannel, OfferDiscovery, SwapProtocol};

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

#[allow(clippy::type_complexity)]
fn lifecycle_prepared(
    direction: SwapDirection,
    role: Participant,
) -> (
    Fixture,
    BtcPairSdk,
    BtcPreparedProtocolV1,
    [u8; 65],
    [u8; 65],
    Vec<u8>,
) {
    let fixture = fixture(direction);
    let agreement = lez_btc_swap_sdk::BtcAgreementV1::validate(fixture.record.clone())
        .expect("validated lifecycle agreement");
    let (claims, bitcoin_presignature, lez_presignature, lez_template) = prepared_claims(&fixture);
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
            .expect("signed lifecycle Bitcoin refund"),
        PreparedLezRefundV1::new(&agreement, LEZ_REFUND_ID, b"signed.lez.refund.v1".to_vec())
            .expect("signed lifecycle LEZ refund"),
    );
    let sdk = BtcPairSdk::new(
        role,
        BtcChainPolicyV1::new(BITCOIN_GENESIS, REQUIRED_CONFIRMATIONS),
    );
    let terms = BtcProtocolTermsV1::new(fixture.record.clone(), fixture.lock_effects.clone())
        .with_claim_effects(claims)
        .with_recovery_effects(recovery);
    let validated = sdk
        .validate_terms(&terms)
        .expect("validated lifecycle terms");
    let prepared = sdk
        .prepare(validated)
        .expect("complete lifecycle preparation");
    (
        fixture,
        sdk,
        prepared,
        bitcoin_presignature,
        lez_presignature,
        lez_template,
    )
}

fn lifecycle_revealing_evidence(
    direction: SwapDirection,
    prepared: &BtcPreparedProtocolV1,
    bitcoin_presignature: [u8; 65],
    lez_presignature: [u8; 65],
    mut lez_template: Vec<u8>,
) -> BtcRevealingClaimEvidenceV1 {
    let adaptor_secret = zeroize::Zeroizing::new(secret(7).secret_bytes());
    match direction {
        SwapDirection::TakerSellsForeign => {
            let context = prepared
                .agreement()
                .claim_adaptor_session_context(lez_btc_swap_sdk::BtcAdaptorSessionDomain::Lez)
                .expect("LEZ lifecycle context");
            let signature = adapt_presignature(&context, lez_presignature, adaptor_secret)
                .expect("revealing LEZ lifecycle signature");
            lez_template[LEZ_CLAIM_SIGNATURE_OFFSET..LEZ_CLAIM_SIGNATURE_OFFSET + 64]
                .copy_from_slice(&signature);
            BtcRevealingClaimEvidenceV1::Lez(
                LezRevealingClaimEvidenceV1::new(
                    Participant::Taker,
                    LEZ_GENESIS,
                    LEZ_CLAIM_ID,
                    lez_template,
                    signature,
                    true,
                )
                .expect("canonical LEZ lifecycle claim"),
            )
        }
        SwapDirection::TakerSellsLez => {
            let context = prepared
                .agreement()
                .claim_adaptor_session_context(lez_btc_swap_sdk::BtcAdaptorSessionDomain::Bitcoin)
                .expect("Bitcoin lifecycle context");
            let signature = adapt_presignature(&context, bitcoin_presignature, adaptor_secret)
                .expect("revealing Bitcoin lifecycle signature");
            let claim = prepared
                .agreement()
                .cooperative_claim()
                .clone()
                .finalize(signature)
                .expect("canonical revealing Bitcoin claim");
            BtcRevealingClaimEvidenceV1::Bitcoin(
                BitcoinRevealingClaimEvidenceV1::new(
                    Participant::Taker,
                    BITCOIN_GENESIS,
                    serialize(&claim),
                    REQUIRED_CONFIRMATIONS,
                )
                .expect("canonical Bitcoin lifecycle claim"),
            )
        }
    }
}

fn lifecycle_followup_evidence(
    direction: SwapDirection,
    plan: &lez_swap_sdk_core::ExactPublicEffectPlanV1,
) -> BtcFollowupClaimEvidenceV1 {
    let [step] = plan.steps() else {
        panic!("one follow-up effect");
    };
    match direction {
        SwapDirection::TakerSellsForeign => BtcFollowupClaimEvidenceV1::Bitcoin(
            BitcoinFollowupClaimEvidenceV1::new(
                BITCOIN_GENESIS,
                step.exact_bytes().as_slice().to_vec(),
                REQUIRED_CONFIRMATIONS,
            )
            .expect("canonical Bitcoin follow-up"),
        ),
        SwapDirection::TakerSellsLez => BtcFollowupClaimEvidenceV1::Lez(
            LezFollowupClaimEvidenceV1::new(
                LEZ_GENESIS,
                step.expected_public_id().as_str(),
                step.exact_bytes().as_slice().to_vec(),
                true,
            )
            .expect("canonical LEZ follow-up"),
        ),
    }
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
fn unsigned_funding_template_is_frozen_before_role_owned_authorization() {
    let mut unsigned = fixture(SwapDirection::TakerSellsForeign).funding;
    unsigned.input[0].script_sig = ScriptBuf::new();
    unsigned.input[0].witness = Witness::default();
    let planned =
        PlannedBitcoinFundingV1::new(unsigned.compute_txid().to_string(), serialize(&unsigned))
            .expect("authorization-free funding template");
    assert_eq!(planned.transaction_id(), unsigned.compute_txid());
    assert_eq!(planned.exact_unsigned_transaction(), serialize(&unsigned));

    let mut signed = unsigned.clone();
    signed.input[0].witness = Witness::from_slice(&[[0x42; 64]]);
    let prepared = planned
        .authorize(serialize(&signed))
        .expect("witness-only authorization");
    assert_eq!(prepared.transaction_id(), planned.transaction_id());

    assert!(matches!(
        PlannedBitcoinFundingV1::new(signed.compute_txid().to_string(), serialize(&signed)),
        Err(BtcSdkError::AuthorizedBitcoinFundingTemplate)
    ));
    assert!(matches!(
        planned.authorize(serialize(&unsigned)),
        Err(BtcSdkError::UnsignedBitcoinFunding)
    ));

    let mut changed = signed;
    changed.output[0].value = Amount::from_sat(FUNDING_VALUE_SAT - 1);
    assert!(matches!(
        planned.authorize(serialize(&changed)),
        Err(BtcSdkError::BitcoinFundingIdentityMismatch
            | BtcSdkError::BitcoinFundingTemplateMismatch)
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
        5,
        fixture.lock_effects,
    );
    assert!(matches!(
        sdk.resume(future),
        Err(BtcSdkError::UnsupportedResumeRevision(5))
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

#[tokio::test(flavor = "current_thread")]
async fn durable_activation_rejects_claim_only_preparation_without_panicking() {
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
    let accepted = sdk
        .accept_wire(&fixture.wire)
        .expect("accepted claim-only agreement");
    let result = StoredBtcLifecycleSdk::new(sdk, InMemoryBtcLifecycleStore::default())
        .activate(accepted, prepared)
        .await;
    assert!(matches!(result, Err(BtcSdkError::MissingRecoveryEffects)));
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

#[test]
#[allow(clippy::too_many_lines)]
fn durable_lifecycle_resumes_revisions_one_through_four_both_directions_and_roles() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        let (fixture, maker, prepared, bitcoin_pre, lez_pre, lez_template) =
            lifecycle_prepared(direction, Participant::Maker);
        let revealing =
            lifecycle_revealing_evidence(direction, &prepared, bitcoin_pre, lez_pre, lez_template);
        let material = maker
            .validate_revealing_claim(&prepared, &revealing)
            .expect("revealing lifecycle material");
        let followup_plan = maker
            .build_followup_claim(&prepared, &material)
            .expect("follow-up lifecycle plan");
        let followup = lifecycle_followup_evidence(direction, &followup_plan);
        let (first, second) = match direction {
            SwapDirection::TakerSellsForeign => (
                bitcoin_evidence(&fixture, REQUIRED_CONFIRMATIONS),
                lez_evidence(true),
            ),
            SwapDirection::TakerSellsLez => (
                lez_evidence(true),
                bitcoin_evidence(&fixture, REQUIRED_CONFIRMATIONS),
            ),
        };
        let transitions = [
            BtcLifecycleTransitionV1::FirstLockConfirmed(first),
            BtcLifecycleTransitionV1::SecondLockConfirmed(second),
            BtcLifecycleTransitionV1::RevealingClaimConfirmed(revealing),
            BtcLifecycleTransitionV1::FollowupClaimConfirmed(followup),
        ];
        let phases = [
            Phase::TakerLockConfirmed,
            Phase::BothLegsLocked,
            Phase::ClaimEvidenceAvailable,
            Phase::Completed,
        ];

        for role in [Participant::Maker, Participant::Taker] {
            let sdk = BtcPairSdk::new(
                role,
                BtcChainPolicyV1::new(BITCOIN_GENESIS, REQUIRED_CONFIRMATIONS),
            );
            let mut active = sdk
                .activate_prepared(
                    sdk.accept_wire(&fixture.wire)
                        .expect("accepted lifecycle wire"),
                    prepared.clone(),
                )
                .expect("complete lifecycle activation");
            for (index, transition) in transitions.iter().cloned().enumerate() {
                let revision = index as u64 + 1;
                assert_eq!(
                    active
                        .apply_transition(transition.clone())
                        .expect("transition"),
                    BtcLifecycleTransitionOutcomeV1::Applied { revision }
                );
                assert_eq!(
                    (active.status().phase(), active.status().revision()),
                    (phases[index], revision)
                );
                if index == 0 {
                    assert_eq!(
                        active.apply_transition(transition).expect("exact replay"),
                        BtcLifecycleTransitionOutcomeV1::AlreadyApplied { revision: 1 }
                    );
                    let before_rejected_transition = active.durable_envelope();
                    assert!(matches!(
                        active.apply_transition(transitions[2].clone()),
                        Err(BtcSdkError::LifecycleTransitionOrder { .. })
                    ));
                    assert_eq!(active.durable_envelope(), before_rejected_transition);
                }
                let resumed = sdk
                    .resume(active.durable_envelope())
                    .expect("offline resume");
                assert_eq!(resumed.status(), active.status());
                active = resumed;
            }
            assert_eq!(
                active
                    .apply_transition(transitions[0].clone())
                    .expect("non-head replay"),
                BtcLifecycleTransitionOutcomeV1::AlreadyApplied { revision: 1 }
            );
            assert_eq!(active.status().revision(), 4);
            assert_eq!(active.next_action(), BtcLifecycleActionV1::Complete);
        }
    }
}

#[test]
fn durable_lifecycle_rejects_role_agreement_revision_and_byte_substitution() {
    let (fixture, maker, prepared, ..) =
        lifecycle_prepared(SwapDirection::TakerSellsForeign, Participant::Maker);
    let taker = BtcPairSdk::new(
        Participant::Taker,
        BtcChainPolicyV1::new(BITCOIN_GENESIS, REQUIRED_CONFIRMATIONS),
    );
    let envelope = |wire, role, revision, effects, prepared, transitions| {
        BtcActiveSwapEnvelopeV1::from_lifecycle_parts(
            wire,
            role,
            revision,
            effects,
            prepared,
            transitions,
        )
    };
    assert!(matches!(
        taker.resume(envelope(
            fixture.wire.clone(),
            Participant::Maker,
            0,
            fixture.lock_effects.clone(),
            prepared.clone(),
            vec![],
        )),
        Err(BtcSdkError::LocalRoleMismatch { .. })
    ));
    let other = lifecycle_prepared(SwapDirection::TakerSellsLez, Participant::Maker).0;
    assert!(matches!(
        maker.resume(envelope(
            other.wire,
            Participant::Maker,
            0,
            fixture.lock_effects.clone(),
            prepared.clone(),
            vec![],
        )),
        Err(BtcSdkError::LifecycleAgreementMismatch)
    ));
    assert!(matches!(
        maker.resume(envelope(
            fixture.wire.clone(),
            Participant::Maker,
            2,
            fixture.lock_effects.clone(),
            prepared.clone(),
            vec![BtcLifecycleTransitionV1::FirstLockConfirmed(
                bitcoin_evidence(&fixture, REQUIRED_CONFIRMATIONS),
            )],
        )),
        Err(BtcSdkError::LifecycleRevisionMismatch { .. })
    ));

    let mut changed = fixture.funding.clone();
    changed.input[0].witness = Witness::from_slice(&[b"different-signed-input"]);
    assert_eq!(changed.compute_txid(), fixture.funding.compute_txid());
    let changed = BtcFirstLockEvidenceV1::Bitcoin(
        BitcoinFirstLockEvidenceV1::new(
            BITCOIN_GENESIS,
            serialize(&changed),
            REQUIRED_CONFIRMATIONS,
        )
        .expect("structurally signed substituted bytes"),
    );
    assert!(matches!(
        maker.resume(envelope(
            fixture.wire,
            Participant::Maker,
            1,
            fixture.lock_effects,
            prepared,
            vec![BtcLifecycleTransitionV1::FirstLockConfirmed(changed)],
        )),
        Err(BtcSdkError::FirstLockPlanMismatch)
    ));
}

#[test]
fn durable_lifecycle_replays_ordered_refunds_to_revision_four() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        let (fixture, maker, prepared, ..) = lifecycle_prepared(direction, Participant::Maker);
        let mut active = maker
            .activate_prepared(
                maker
                    .accept_wire(&fixture.wire)
                    .expect("accepted refund lifecycle"),
                prepared.clone(),
            )
            .expect("refund-capable activation");
        let (first, second) = match direction {
            SwapDirection::TakerSellsForeign => (
                bitcoin_evidence(&fixture, REQUIRED_CONFIRMATIONS),
                lez_evidence(true),
            ),
            SwapDirection::TakerSellsLez => (
                lez_evidence(true),
                bitcoin_evidence(&fixture, REQUIRED_CONFIRMATIONS),
            ),
        };
        let _ = active
            .apply_transition(BtcLifecycleTransitionV1::FirstLockConfirmed(first))
            .unwrap();
        let _ = active
            .apply_transition(BtcLifecycleTransitionV1::SecondLockConfirmed(second))
            .unwrap();

        let (earlier, both) = match direction {
            SwapDirection::TakerSellsForeign => (
                BtcCanonicalRecoveryStateV1::new(
                    prepared.agreement(),
                    BITCOIN_REFUND_HEIGHT,
                    LEZ_FOREIGN_REFUND_SECONDS,
                    bitcoin_locked(&prepared),
                    lez_refunded(),
                ),
                BtcCanonicalRecoveryStateV1::new(
                    prepared.agreement(),
                    BITCOIN_REFUND_HEIGHT,
                    LEZ_FOREIGN_REFUND_SECONDS,
                    bitcoin_refunded(&prepared),
                    lez_refunded(),
                ),
            ),
            SwapDirection::TakerSellsLez => (
                BtcCanonicalRecoveryStateV1::new(
                    prepared.agreement(),
                    BITCOIN_REFUND_HEIGHT,
                    LEZ_REVERSE_REFUND_SECONDS,
                    bitcoin_refunded(&prepared),
                    lez_locked(),
                ),
                BtcCanonicalRecoveryStateV1::new(
                    prepared.agreement(),
                    BITCOIN_REFUND_HEIGHT,
                    LEZ_REVERSE_REFUND_SECONDS,
                    bitcoin_refunded(&prepared),
                    lez_refunded(),
                ),
            ),
        };
        assert_eq!(
            active
                .apply_transition(BtcLifecycleTransitionV1::RecoveryObserved(earlier))
                .unwrap(),
            BtcLifecycleTransitionOutcomeV1::Applied { revision: 3 }
        );
        assert_eq!(active.status().phase(), Phase::MakerLegRefunded);
        assert_eq!(
            active
                .apply_transition(BtcLifecycleTransitionV1::RecoveryObserved(both))
                .unwrap(),
            BtcLifecycleTransitionOutcomeV1::Applied { revision: 4 }
        );
        assert_eq!(active.status().phase(), Phase::Refunded);
        assert_eq!(
            maker.resume(active.durable_envelope()).unwrap().status(),
            active.status()
        );
    }
}

#[derive(Clone, Default)]
struct MemoryDiscovery {
    offers: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl OfferDiscovery for MemoryDiscovery {
    type Error = std::io::Error;
    type Offer = String;
    type OfferRef = usize;
    type Query = ();

    async fn publish(&self, offer: Self::Offer) -> Result<Self::OfferRef, Self::Error> {
        let mut offers = self.offers.lock().expect("memory discovery lock");
        let reference = offers.len();
        offers.push(offer);
        Ok(reference)
    }

    async fn discover(&self, _query: &Self::Query) -> Result<Vec<Self::OfferRef>, Self::Error> {
        let length = self.offers.lock().expect("memory discovery lock").len();
        Ok((0..length).collect())
    }
}

#[derive(Clone)]
struct FixedNegotiation {
    wire: Arc<Vec<u8>>,
    calls: Arc<Mutex<Vec<(Participant, usize)>>>,
}

impl FixedNegotiation {
    fn new(wire: Vec<u8>) -> Self {
        Self {
            wire: Arc::new(wire),
            calls: Arc::default(),
        }
    }
}

#[async_trait]
impl NegotiationChannel for FixedNegotiation {
    type Error = std::io::Error;
    type LocalProposal = ();
    type OfferRef = usize;

    async fn negotiate(
        &self,
        local_participant: Participant,
        offer: &Self::OfferRef,
        (): Self::LocalProposal,
    ) -> Result<Vec<u8>, Self::Error> {
        self.calls
            .lock()
            .expect("memory negotiation lock")
            .push((local_participant, *offer));
        Ok(self.wire.as_ref().clone())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn pre_lock_ports_compose_then_activation_loses_negotiation_capability() {
    let (current, pair, prepared, ..) =
        lifecycle_prepared(SwapDirection::TakerSellsForeign, Participant::Maker);
    let discovery = MemoryDiscovery::default();
    let negotiation = FixedNegotiation::new(current.wire.clone());
    let lifecycle = BtcLifecycleSdk::new(pair, discovery.clone(), negotiation.clone());

    let offer = lifecycle
        .publish_offer("btc-lez-offer".to_owned())
        .await
        .expect("publish authenticated offer");
    assert_eq!(
        lifecycle.discover(&()).await.expect("discover offer"),
        vec![offer]
    );
    let accepted = lifecycle
        .negotiate(&offer, ())
        .await
        .expect("negotiate and validate countersigned wire");
    assert_eq!(
        negotiation
            .calls
            .lock()
            .expect("negotiation calls")
            .as_slice(),
        &[(Participant::Maker, offer)]
    );
    let active = lifecycle
        .activate(accepted, prepared.clone())
        .expect("activate complete pre-lock material");
    assert_eq!(active.status().phase(), Phase::Offered);

    // ActiveBtcSwap has no discovery/negotiation generic parameters or methods.
    // The compile-fail API test on ActiveBtcSwap proves those methods cannot be
    // reached after activation; only its deterministic lifecycle API remains.
    let _: &lez_btc_swap_sdk::ActiveBtcSwap = &active;

    let (substituted, ..) = lifecycle_prepared(SwapDirection::TakerSellsLez, Participant::Maker);
    let hostile = BtcLifecycleSdk::new(
        BtcPairSdk::new(
            Participant::Maker,
            BtcChainPolicyV1::new(BITCOIN_GENESIS, REQUIRED_CONFIRMATIONS),
        ),
        MemoryDiscovery::default(),
        FixedNegotiation::new(substituted.wire),
    );
    let accepted_substitution = hostile
        .negotiate(&0, ())
        .await
        .expect("substituted wire remains independently valid");
    assert!(matches!(
        hostile.activate(accepted_substitution, prepared),
        Err(BtcSdkError::LifecycleAgreementMismatch)
    ));
}

use lez_btc_swap_sdk::{
    BTC_LEZ_ASSET_EXTENSION_SCHEMA_V1, BtcAgreementV1, BtcLezAssetExtensionBodyV1,
    BtcLezAssetExtensionRecordV1, BtcLezAssetExtensionV1, BtcLezAssetFirstLockEvidenceV1,
    BtcLezAssetPreparedLockEffectsV1, BtcLezAssetSdkError, BtcLezAssetV1, BtcLezCustomTokenTermsV1,
    LezAssetCustodyEvidenceV1, LezAssetFirstLockEvidenceV1, PreparedLezAssetFundingV1,
};
use lez_swap_sdk_core::{
    ExactPublicEffectBytes, ExactPublicEffectPlanV1, ExpectedPublicEffectId, PublicEffectStepId,
    PublicEffectStepV1,
};

fn f7_asset(agreement: &BtcAgreementV1, custom: bool) -> BtcLezAssetV1 {
    if !custom {
        return BtcLezAssetV1::Native;
    }
    BtcLezAssetV1::CustomToken(Box::new(BtcLezCustomTokenTermsV1::new(
        [40; 32],
        [41; 32],
        [42; 32],
        *agreement.lez_terms().depositor_account(),
        [43; 32],
        *agreement.lez_terms().claimant_account(),
        [44; 32],
        [45; 32],
        agreement.lez_terms().amount(),
        agreement.lez_terms().refund_at_ms(),
        *agreement.lez_terms().aggregate_authority_account(),
        agreement.p2tr_contract().aggregate_internal_key_bytes(),
    )))
}

fn f7_extension(agreement: &BtcAgreementV1, asset: BtcLezAssetV1) -> BtcLezAssetExtensionV1 {
    let body = BtcLezAssetExtensionBodyV1::new(*agreement.agreement_commitment(), asset);
    let commitment = body.commitment();
    let record = BtcLezAssetExtensionRecordV1::from_parts(
        BTC_LEZ_ASSET_EXTENSION_SCHEMA_V1,
        body,
        commitment,
        agreement_signature(&secret(1), commitment),
        agreement_signature(&secret(2), commitment),
    );
    BtcLezAssetExtensionV1::validate(record, agreement).expect("valid F7 extension")
}

fn f7_step(step: &str, public_id: &str, bytes: Vec<u8>) -> PublicEffectStepV1 {
    PublicEffectStepV1::new(
        PublicEffectStepId::new(step).expect("F7 step"),
        ExpectedPublicEffectId::new(public_id).expect("F7 transaction ID"),
        ExactPublicEffectBytes::new(bytes).expect("F7 exact bytes"),
    )
}

fn f7_plan(custom: bool, nonce: u8) -> ExactPublicEffectPlanV1 {
    let mut steps = vec![f7_step(
        "lez.initialize",
        &format!("f7-initialize-{nonce}"),
        vec![0xa0, nonce],
    )];
    if custom {
        steps.push(f7_step(
            "lez.create_custody_ata",
            &format!("f7-custody-{nonce}"),
            vec![0xb0, nonce],
        ));
    }
    steps.push(f7_step(
        "lez.fund",
        &format!("f7-fund-{nonce}"),
        vec![0xc0, nonce],
    ));
    ExactPublicEffectPlanV1::new(steps).expect("exact F7 plan")
}

fn f7_prepared_lez(
    extension: &BtcLezAssetExtensionV1,
    plan: ExactPublicEffectPlanV1,
) -> PreparedLezAssetFundingV1 {
    match extension.asset() {
        BtcLezAssetV1::Native => PreparedLezAssetFundingV1::native(plan).expect("native F7 plan"),
        BtcLezAssetV1::CustomToken(_) => {
            PreparedLezAssetFundingV1::custom_token(plan).expect("token F7 plan")
        }
    }
}

fn f7_locks(
    fixture: &Fixture,
    agreement: &BtcAgreementV1,
    extension: &BtcLezAssetExtensionV1,
    plan: ExactPublicEffectPlanV1,
) -> BtcLezAssetPreparedLockEffectsV1 {
    let bitcoin = PreparedBitcoinFundingV1::new(
        fixture.funding.compute_txid().to_string(),
        serialize(&fixture.funding),
    )
    .expect("exact Bitcoin funding");
    BtcLezAssetPreparedLockEffectsV1::new(
        agreement,
        extension.clone(),
        bitcoin,
        f7_prepared_lez(extension, plan),
    )
    .expect("agreement-bound F7 locks")
}

fn f7_custody(extension: &BtcLezAssetExtensionV1) -> LezAssetCustodyEvidenceV1 {
    match extension.asset() {
        BtcLezAssetV1::Native => LezAssetCustodyEvidenceV1::Native {
            custody_account: [14; 32],
        },
        BtcLezAssetV1::CustomToken(token) => LezAssetCustodyEvidenceV1::CustomToken {
            custody_ata_account: *token.custody_ata_account(),
            token_definition_account: *token.token_definition_account(),
        },
    }
}

fn f7_lez_evidence(
    _extension: &BtcLezAssetExtensionV1,
    observed_plan: ExactPublicEffectPlanV1,
    genesis: [u8; 32],
    metadata: [u8; 32],
    custody: LezAssetCustodyEvidenceV1,
    amount: u128,
    finalized: bool,
) -> BtcLezAssetFirstLockEvidenceV1 {
    BtcLezAssetFirstLockEvidenceV1::Lez(LezAssetFirstLockEvidenceV1::new(
        genesis,
        observed_plan,
        metadata,
        custody,
        amount,
        finalized,
    ))
}

#[test]
fn f7_asset_activation_authorizes_both_directions_and_both_asset_kinds() {
    for (direction, custom) in [
        (SwapDirection::TakerSellsForeign, false),
        (SwapDirection::TakerSellsForeign, true),
        (SwapDirection::TakerSellsLez, false),
        (SwapDirection::TakerSellsLez, true),
    ] {
        let fixture = fixture(direction);
        let sdk = BtcPairSdk::new(
            Participant::Maker,
            BtcChainPolicyV1::new(BITCOIN_GENESIS, REQUIRED_CONFIRMATIONS),
        );
        let accepted = sdk.accept_wire(&fixture.wire).expect("accepted agreement");
        let agreement = accepted.agreement().clone();
        let extension = f7_extension(&agreement, f7_asset(&agreement, custom));
        let lez_plan = f7_plan(custom, 1);
        let locks = f7_locks(&fixture, &agreement, &extension, lez_plan.clone());
        let active = sdk
            .activate_asset(accepted, locks)
            .expect("role-fixed asset activation");
        assert_eq!(active.local_participant(), Participant::Maker);
        assert_eq!(
            active.asset_extension().asset_commitment(),
            extension.asset_commitment()
        );

        let evidence = match direction {
            SwapDirection::TakerSellsForeign => BtcLezAssetFirstLockEvidenceV1::Bitcoin(
                BitcoinFirstLockEvidenceV1::new(
                    BITCOIN_GENESIS,
                    serialize(&fixture.funding),
                    REQUIRED_CONFIRMATIONS,
                )
                .expect("final Bitcoin first lock"),
            ),
            SwapDirection::TakerSellsLez => f7_lez_evidence(
                &extension,
                lez_plan.clone(),
                LEZ_GENESIS,
                [13; 32],
                f7_custody(&extension),
                LEZ_AMOUNT,
                true,
            ),
        };
        let confirmed = active
            .validate_first_lock(&evidence)
            .expect("exact finalized taker first lock");
        assert_eq!(
            confirmed.base_agreement_commitment(),
            agreement.agreement_commitment()
        );
        assert_eq!(confirmed.asset_commitment(), extension.asset_commitment());
        assert_eq!(
            confirmed.first_plan_commitment(),
            &active.first_lock_plan().commitment()
        );
        assert_eq!(confirmed.direction(), direction);

        let second = active
            .second_lock_plan(&confirmed)
            .expect("maker second-lock authorization");
        match direction {
            SwapDirection::TakerSellsForeign => assert_eq!(second, &lez_plan),
            SwapDirection::TakerSellsLez => {
                assert_eq!(second, active.prepared_lock_effects().bitcoin().plan());
            }
        }
    }
}

#[test]
fn f7_prepared_lock_validation_rejects_shape_duplicates_and_substitution() {
    let reordered = ExactPublicEffectPlanV1::new(vec![
        f7_step("lez.fund", "fund-a", vec![1]),
        f7_step("lez.initialize", "init-a", vec![2]),
    ])
    .expect("well-formed but reordered plan");
    assert_eq!(
        PreparedLezAssetFundingV1::native(reordered),
        Err(BtcLezAssetSdkError::LezAssetPlanShape)
    );

    let duplicate_ids = ExactPublicEffectPlanV1::new(vec![
        f7_step("lez.initialize", "same-id", vec![1]),
        f7_step("lez.fund", "same-id", vec![2]),
    ])
    .expect("semantic steps with duplicate transaction IDs");
    assert_eq!(
        PreparedLezAssetFundingV1::native(duplicate_ids),
        Err(BtcLezAssetSdkError::DuplicateLezAssetEffectIdentity)
    );

    let duplicate_bytes = ExactPublicEffectPlanV1::new(vec![
        f7_step("lez.initialize", "init-b", vec![9]),
        f7_step("lez.fund", "fund-b", vec![9]),
    ])
    .expect("semantic steps with duplicate exact bytes");
    assert_eq!(
        PreparedLezAssetFundingV1::native(duplicate_bytes),
        Err(BtcLezAssetSdkError::DuplicateLezAssetEffectBytes)
    );
    assert_eq!(
        PreparedLezAssetFundingV1::custom_token(f7_plan(false, 3)),
        Err(BtcLezAssetSdkError::LezAssetPlanShape)
    );

    let forward = fixture(SwapDirection::TakerSellsForeign);
    let reverse = fixture(SwapDirection::TakerSellsLez);
    let forward_agreement = BtcAgreementV1::validate(forward.record.clone()).expect("forward");
    let reverse_agreement = BtcAgreementV1::validate(reverse.record.clone()).expect("reverse");
    let native_extension = f7_extension(&forward_agreement, BtcLezAssetV1::Native);
    let reverse_bitcoin = PreparedBitcoinFundingV1::new(
        reverse.funding.compute_txid().to_string(),
        serialize(&reverse.funding),
    )
    .expect("reverse Bitcoin funding");
    assert_eq!(
        BtcLezAssetPreparedLockEffectsV1::new(
            &reverse_agreement,
            native_extension.clone(),
            reverse_bitcoin,
            PreparedLezAssetFundingV1::native(f7_plan(false, 4)).expect("native plan"),
        ),
        Err(BtcLezAssetSdkError::AssetExtensionAgreementMismatch)
    );

    let forward_bitcoin = PreparedBitcoinFundingV1::new(
        forward.funding.compute_txid().to_string(),
        serialize(&forward.funding),
    )
    .expect("forward Bitcoin funding");
    assert_eq!(
        BtcLezAssetPreparedLockEffectsV1::new(
            &forward_agreement,
            native_extension.clone(),
            forward_bitcoin,
            PreparedLezAssetFundingV1::custom_token(f7_plan(true, 5)).expect("token plan"),
        ),
        Err(BtcLezAssetSdkError::AssetPlanKindMismatch)
    );

    let substituted_bitcoin = PreparedBitcoinFundingV1::new(
        reverse.funding.compute_txid().to_string(),
        serialize(&reverse.funding),
    )
    .expect("substituted Bitcoin funding");
    assert_eq!(
        BtcLezAssetPreparedLockEffectsV1::new(
            &forward_agreement,
            native_extension,
            substituted_bitcoin,
            PreparedLezAssetFundingV1::native(f7_plan(false, 6)).expect("native plan"),
        ),
        Err(BtcLezAssetSdkError::BitcoinFundingAgreementMismatch)
    );
}

struct F7ActiveFixture {
    fixture: Fixture,
    sdk: BtcPairSdk,
    agreement: BtcAgreementV1,
    extension: BtcLezAssetExtensionV1,
    lez_plan: ExactPublicEffectPlanV1,
    active: lez_btc_swap_sdk::ActiveBtcLezAssetSwapV1,
}

fn f7_active_fixture(direction: SwapDirection, custom: bool, nonce: u8) -> F7ActiveFixture {
    let fixture = fixture(direction);
    let sdk = BtcPairSdk::new(
        Participant::Maker,
        BtcChainPolicyV1::new(BITCOIN_GENESIS, REQUIRED_CONFIRMATIONS),
    );
    let accepted = sdk.accept_wire(&fixture.wire).expect("accepted agreement");
    let agreement = accepted.agreement().clone();
    let extension = f7_extension(&agreement, f7_asset(&agreement, custom));
    let lez_plan = f7_plan(custom, nonce);
    let active = sdk
        .activate_asset(
            accepted,
            f7_locks(&fixture, &agreement, &extension, lez_plan.clone()),
        )
        .expect("asset activation");
    F7ActiveFixture {
        fixture,
        sdk,
        agreement,
        extension,
        lez_plan,
        active,
    }
}

fn valid_f7_lez_evidence(f7: &F7ActiveFixture) -> BtcLezAssetFirstLockEvidenceV1 {
    f7_lez_evidence(
        &f7.extension,
        f7.lez_plan.clone(),
        LEZ_GENESIS,
        [13; 32],
        f7_custody(&f7.extension),
        LEZ_AMOUNT,
        true,
    )
}

#[test]
fn f7_lez_evidence_rejects_finality_network_and_plan_substitution() {
    let f7 = f7_active_fixture(SwapDirection::TakerSellsLez, true, 7);
    let custody = f7_custody(&f7.extension);
    for (evidence, expected) in [
        (
            f7_lez_evidence(
                &f7.extension,
                f7.lez_plan.clone(),
                LEZ_GENESIS,
                [13; 32],
                custody.clone(),
                LEZ_AMOUNT,
                false,
            ),
            BtcLezAssetSdkError::AssetFirstLockNotFinalized,
        ),
        (
            f7_lez_evidence(
                &f7.extension,
                f7.lez_plan.clone(),
                [99; 32],
                [13; 32],
                custody.clone(),
                LEZ_AMOUNT,
                true,
            ),
            BtcLezAssetSdkError::AssetFirstLockNetworkMismatch,
        ),
        (
            f7_lez_evidence(
                &f7.extension,
                f7_plan(true, 8),
                LEZ_GENESIS,
                [13; 32],
                custody,
                LEZ_AMOUNT,
                true,
            ),
            BtcLezAssetSdkError::AssetFirstLockPlanMismatch,
        ),
    ] {
        assert_eq!(f7.active.validate_first_lock(&evidence), Err(expected));
    }
}

#[test]
fn f7_lez_evidence_rejects_metadata_custody_definition_and_amount_substitution() {
    let f7 = f7_active_fixture(SwapDirection::TakerSellsLez, true, 9);
    let custody = f7_custody(&f7.extension);
    for (evidence, expected) in [
        (
            f7_lez_evidence(
                &f7.extension,
                f7.lez_plan.clone(),
                LEZ_GENESIS,
                [99; 32],
                custody.clone(),
                LEZ_AMOUNT,
                true,
            ),
            BtcLezAssetSdkError::AssetFirstLockTermsMismatch,
        ),
        (
            f7_lez_evidence(
                &f7.extension,
                f7.lez_plan.clone(),
                LEZ_GENESIS,
                [13; 32],
                LezAssetCustodyEvidenceV1::CustomToken {
                    custody_ata_account: [99; 32],
                    token_definition_account: [42; 32],
                },
                LEZ_AMOUNT,
                true,
            ),
            BtcLezAssetSdkError::AssetFirstLockTermsMismatch,
        ),
        (
            f7_lez_evidence(
                &f7.extension,
                f7.lez_plan.clone(),
                LEZ_GENESIS,
                [13; 32],
                custody,
                LEZ_AMOUNT + 1,
                true,
            ),
            BtcLezAssetSdkError::AssetFirstLockTermsMismatch,
        ),
    ] {
        assert_eq!(f7.active.validate_first_lock(&evidence), Err(expected));
    }
}

#[test]
fn f7_confirmation_token_rejects_asset_and_first_plan_substitution() {
    let f7 = f7_active_fixture(SwapDirection::TakerSellsLez, true, 10);
    let confirmed = f7
        .active
        .validate_first_lock(&valid_f7_lez_evidence(&f7))
        .expect("valid custom-token first lock");

    let native_extension = f7_extension(&f7.agreement, BtcLezAssetV1::Native);
    let native_accepted = f7
        .sdk
        .accept_wire(&f7.fixture.wire)
        .expect("same accepted agreement");
    let native_active = f7
        .sdk
        .activate_asset(
            native_accepted,
            f7_locks(
                &f7.fixture,
                &f7.agreement,
                &native_extension,
                f7_plan(false, 11),
            ),
        )
        .expect("native activation");
    assert_eq!(
        native_active.second_lock_plan(&confirmed),
        Err(BtcLezAssetSdkError::AssetFirstLockConfirmationMismatch)
    );

    let changed_plan_accepted = f7
        .sdk
        .accept_wire(&f7.fixture.wire)
        .expect("same accepted agreement");
    let changed_plan_active = f7
        .sdk
        .activate_asset(
            changed_plan_accepted,
            f7_locks(&f7.fixture, &f7.agreement, &f7.extension, f7_plan(true, 12)),
        )
        .expect("changed-plan activation");
    assert_eq!(
        changed_plan_active.second_lock_plan(&confirmed),
        Err(BtcLezAssetSdkError::AssetFirstLockConfirmationMismatch)
    );
}

#[test]
fn f7_direction_finality_and_role_substitution_fail_closed() {
    let reverse = f7_active_fixture(SwapDirection::TakerSellsLez, true, 13);
    let reverse_confirmed = reverse
        .active
        .validate_first_lock(&valid_f7_lez_evidence(&reverse))
        .expect("valid reverse first lock");
    let forward = f7_active_fixture(SwapDirection::TakerSellsForeign, false, 14);
    let lagging = BtcLezAssetFirstLockEvidenceV1::Bitcoin(
        BitcoinFirstLockEvidenceV1::new(
            BITCOIN_GENESIS,
            serialize(&forward.fixture.funding),
            REQUIRED_CONFIRMATIONS - 1,
        )
        .expect("lagging Bitcoin evidence"),
    );
    assert_eq!(
        forward.active.validate_first_lock(&lagging),
        Err(BtcLezAssetSdkError::AssetFirstLockConfirmationLag {
            required: REQUIRED_CONFIRMATIONS,
            actual: REQUIRED_CONFIRMATIONS - 1,
        })
    );
    assert_eq!(
        forward.active.second_lock_plan(&reverse_confirmed),
        Err(BtcLezAssetSdkError::AssetFirstLockConfirmationMismatch)
    );

    let wrong_role = BtcPairSdk::new(
        Participant::Taker,
        BtcChainPolicyV1::new(BITCOIN_GENESIS, REQUIRED_CONFIRMATIONS),
    );
    let maker_accepted = forward
        .sdk
        .accept_wire(&forward.fixture.wire)
        .expect("maker accepted");
    let wrong_role_locks = f7_locks(
        &forward.fixture,
        &forward.agreement,
        &forward.extension,
        f7_plan(false, 15),
    );
    assert_eq!(
        wrong_role.activate_asset(maker_accepted, wrong_role_locks),
        Err(BtcLezAssetSdkError::LocalRoleMismatch {
            expected: Participant::Taker,
            actual: Participant::Maker,
        })
    );
}

#[derive(Clone, Default)]
struct ScriptedLifecyclePort {
    outcomes: Arc<Mutex<Vec<BtcLifecycleChainOutcomeV1>>>,
    requests: Arc<Mutex<Vec<BtcLifecycleDriveRequestV1>>>,
}

impl ScriptedLifecyclePort {
    fn new(outcomes: Vec<BtcLifecycleTransitionV1>) -> Self {
        Self {
            outcomes: Arc::new(Mutex::new(
                outcomes
                    .into_iter()
                    .map(|transition| BtcLifecycleChainOutcomeV1::Transition(Box::new(transition)))
                    .collect(),
            )),
            requests: Arc::default(),
        }
    }

    fn request_count(&self) -> usize {
        self.requests.lock().expect("scripted request lock").len()
    }

    fn next(
        &self,
        request: BtcLifecycleDriveRequestV1,
    ) -> Result<BtcLifecycleChainOutcomeV1, std::io::Error> {
        self.requests
            .lock()
            .map_err(|_| std::io::Error::other("request lock"))?
            .push(request);
        let mut outcomes = self
            .outcomes
            .lock()
            .map_err(|_| std::io::Error::other("outcome lock"))?;
        if outcomes.is_empty() {
            Ok(BtcLifecycleChainOutcomeV1::Pending)
        } else {
            Ok(outcomes.remove(0))
        }
    }
}

#[async_trait]
impl BitcoinBtcLifecyclePort for ScriptedLifecyclePort {
    type Error = std::io::Error;

    async fn drive(
        &self,
        request: BtcLifecycleDriveRequestV1,
    ) -> Result<BtcLifecycleChainOutcomeV1, Self::Error> {
        self.next(request)
    }
}

#[async_trait]
impl LezBtcLifecyclePort for ScriptedLifecyclePort {
    type Error = std::io::Error;

    async fn drive(
        &self,
        request: BtcLifecycleDriveRequestV1,
    ) -> Result<BtcLifecycleChainOutcomeV1, Self::Error> {
        self.next(request)
    }
}

fn claim_lifecycle_fixture(
    direction: SwapDirection,
) -> (
    Fixture,
    BtcPairSdk,
    BtcPreparedProtocolV1,
    Vec<BtcLifecycleTransitionV1>,
) {
    let (fixture, pair, prepared, bitcoin_pre, lez_pre, lez_template) =
        lifecycle_prepared(direction, Participant::Maker);
    let revealing =
        lifecycle_revealing_evidence(direction, &prepared, bitcoin_pre, lez_pre, lez_template);
    let material = pair
        .validate_revealing_claim(&prepared, &revealing)
        .expect("canonical revealing material");
    let followup_plan = pair
        .build_followup_claim(&prepared, &material)
        .expect("canonical follow-up plan");
    let followup = lifecycle_followup_evidence(direction, &followup_plan);
    let (first, second) = match direction {
        SwapDirection::TakerSellsForeign => (
            bitcoin_evidence(&fixture, REQUIRED_CONFIRMATIONS),
            lez_evidence(true),
        ),
        SwapDirection::TakerSellsLez => (
            lez_evidence(true),
            bitcoin_evidence(&fixture, REQUIRED_CONFIRMATIONS),
        ),
    };
    (
        fixture,
        pair,
        prepared,
        vec![
            BtcLifecycleTransitionV1::FirstLockConfirmed(first),
            BtcLifecycleTransitionV1::SecondLockConfirmed(second),
            BtcLifecycleTransitionV1::RevealingClaimConfirmed(revealing),
            BtcLifecycleTransitionV1::FollowupClaimConfirmed(followup),
        ],
    )
}

fn refund_lifecycle_fixture(
    direction: SwapDirection,
) -> (
    Fixture,
    BtcPairSdk,
    BtcPreparedProtocolV1,
    Vec<BtcLifecycleTransitionV1>,
) {
    let (fixture, pair, prepared, ..) = lifecycle_prepared(direction, Participant::Maker);
    let (first, second, earlier, both) = match direction {
        SwapDirection::TakerSellsForeign => (
            bitcoin_evidence(&fixture, REQUIRED_CONFIRMATIONS),
            lez_evidence(true),
            BtcCanonicalRecoveryStateV1::new(
                prepared.agreement(),
                BITCOIN_REFUND_HEIGHT,
                LEZ_FOREIGN_REFUND_SECONDS,
                bitcoin_locked(&prepared),
                lez_refunded(),
            ),
            BtcCanonicalRecoveryStateV1::new(
                prepared.agreement(),
                BITCOIN_REFUND_HEIGHT,
                LEZ_FOREIGN_REFUND_SECONDS,
                bitcoin_refunded(&prepared),
                lez_refunded(),
            ),
        ),
        SwapDirection::TakerSellsLez => (
            lez_evidence(true),
            bitcoin_evidence(&fixture, REQUIRED_CONFIRMATIONS),
            BtcCanonicalRecoveryStateV1::new(
                prepared.agreement(),
                BITCOIN_REFUND_HEIGHT,
                LEZ_REVERSE_REFUND_SECONDS,
                bitcoin_refunded(&prepared),
                lez_locked(),
            ),
            BtcCanonicalRecoveryStateV1::new(
                prepared.agreement(),
                BITCOIN_REFUND_HEIGHT,
                LEZ_REVERSE_REFUND_SECONDS,
                bitcoin_refunded(&prepared),
                lez_refunded(),
            ),
        ),
    };
    (
        fixture,
        pair,
        prepared,
        vec![
            BtcLifecycleTransitionV1::FirstLockConfirmed(first),
            BtcLifecycleTransitionV1::SecondLockConfirmed(second),
            BtcLifecycleTransitionV1::RecoveryObserved(earlier),
            BtcLifecycleTransitionV1::RecoveryObserved(both),
        ],
    )
}

fn split_chain_scripts(
    direction: SwapDirection,
    transitions: &[BtcLifecycleTransitionV1],
) -> (Vec<BtcLifecycleTransitionV1>, Vec<BtcLifecycleTransitionV1>) {
    match direction {
        SwapDirection::TakerSellsForeign => (
            vec![transitions[0].clone(), transitions[3].clone()],
            vec![transitions[1].clone(), transitions[2].clone()],
        ),
        SwapDirection::TakerSellsLez => (
            vec![transitions[1].clone(), transitions[2].clone()],
            vec![transitions[0].clone(), transitions[3].clone()],
        ),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn durable_codec_and_store_are_canonical_secret_free_and_exact_cas() {
    let (fixture, pair, prepared, transitions) =
        claim_lifecycle_fixture(SwapDirection::TakerSellsForeign);
    let active = pair
        .activate_prepared(
            pair.accept_wire(&fixture.wire).expect("accepted wire"),
            prepared,
        )
        .expect("active lifecycle");
    let record = BtcLifecycleRecordV1::from_active(&active).expect("canonical record");
    assert_eq!(
        BtcLifecycleRecordV1::from_exact_bytes(record.exact_bytes().to_vec())
            .expect("canonical replay"),
        record
    );
    assert!(
        !record
            .exact_bytes()
            .windows(32)
            .any(|window| window == secret(7).secret_bytes())
    );
    assert!(format!("{record:?}").contains("[REDACTED]"));

    let mut whitespace = vec![b' '];
    whitespace.extend_from_slice(record.exact_bytes());
    assert!(matches!(
        BtcLifecycleRecordV1::from_exact_bytes(whitespace),
        Err(BtcLifecycleCodecError::NonCanonical)
    ));
    let mut unknown: serde_json::Value =
        serde_json::from_slice(record.exact_bytes()).expect("record JSON");
    unknown
        .as_object_mut()
        .expect("record object")
        .insert("unknown".to_owned(), serde_json::Value::Bool(true));
    assert!(matches!(
        BtcLifecycleRecordV1::from_exact_bytes(
            serde_json::to_vec(&unknown).expect("unknown-field JSON")
        ),
        Err(BtcLifecycleCodecError::Malformed)
    ));
    let mut future: serde_json::Value =
        serde_json::from_slice(record.exact_bytes()).expect("record JSON");
    future["schema_version"] = serde_json::Value::from(2);
    assert!(matches!(
        BtcLifecycleRecordV1::from_exact_bytes(
            serde_json::to_vec(&future).expect("future-schema JSON")
        ),
        Err(BtcLifecycleCodecError::UnsupportedSchema(2))
    ));

    let store = InMemoryBtcLifecycleStore::default();
    assert_eq!(
        store.create(&record).await.expect("create"),
        BtcLifecycleStoreCreateV1::Created
    );
    assert_eq!(
        store.create(&record).await.expect("exact create replay"),
        BtcLifecycleStoreCreateV1::ExistingSame
    );
    let mut first = pair
        .resume(record.decode().expect("revision-zero envelope"))
        .expect("revision-zero replay");
    let _ = first
        .apply_transition(transitions[0].clone())
        .expect("first transition");
    let successor = BtcLifecycleRecordV1::from_active(&first).expect("revision one record");
    assert_eq!(
        store
            .compare_exchange(&record, &successor)
            .await
            .expect("CAS"),
        BtcLifecycleStoreCompareExchangeV1::Applied
    );
    assert_eq!(
        store
            .compare_exchange(&record, &successor)
            .await
            .expect("exact CAS replay"),
        BtcLifecycleStoreCompareExchangeV1::ExistingSame
    );

    let mut competing = pair
        .resume(record.decode().expect("competing envelope"))
        .expect("competing replay");
    let _ = competing
        .apply_transition(BtcLifecycleTransitionV1::FirstLockConfirmed(
            bitcoin_evidence(&fixture, REQUIRED_CONFIRMATIONS + 1),
        ))
        .expect("different valid first observation");
    let competing = BtcLifecycleRecordV1::from_active(&competing).expect("competing record");
    assert_eq!(
        store
            .compare_exchange(&record, &competing)
            .await
            .expect("stale CAS"),
        BtcLifecycleStoreCompareExchangeV1::Conflict {
            actual_revision: Some(1)
        }
    );
}

#[tokio::test(flavor = "current_thread")]
async fn public_runtime_completes_both_claim_directions_with_restart_and_zero_replay() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        let (fixture, pair, prepared, transitions) = claim_lifecycle_fixture(direction);
        let store = InMemoryBtcLifecycleStore::default();
        let stored = StoredBtcLifecycleSdk::new(pair.clone(), store.clone());
        let active = stored
            .activate(
                pair.accept_wire(&fixture.wire)
                    .expect("accepted claim wire"),
                prepared,
            )
            .await
            .expect("durable activation");
        let swap_id = active.status().swap_id().clone();
        let (bitcoin_script, lez_script) = split_chain_scripts(direction, &transitions);
        let bitcoin = ScriptedLifecyclePort::new(bitcoin_script);
        let lez = ScriptedLifecyclePort::new(lez_script);

        for expected_revision in 1..=4 {
            let runtime = BtcLifecycleRuntime::new(
                StoredBtcLifecycleSdk::new(pair.clone(), store.clone()),
                bitcoin.clone(),
                lez.clone(),
            );
            let BtcLifecycleDriveOutcomeV1::Transition { status, .. } =
                runtime.drive_once(&swap_id).await.unwrap_or_else(|error| {
                    panic!("claim drive revision {expected_revision}: {error:?}")
                })
            else {
                panic!("expected one durable transition");
            };
            assert_eq!(status.revision(), expected_revision);
        }
        let runtime = BtcLifecycleRuntime::new(
            StoredBtcLifecycleSdk::new(pair.clone(), store.clone()),
            bitcoin.clone(),
            lez.clone(),
        );
        assert!(matches!(
            runtime.drive_once(&swap_id).await.expect("terminal replay"),
            BtcLifecycleDriveOutcomeV1::Complete(status)
                if status.phase() == Phase::Completed && status.revision() == 4
        ));
        assert_eq!(bitcoin.request_count() + lez.request_count(), 4);

        let before = store
            .load(&swap_id, Participant::Maker)
            .await
            .expect("load before replay")
            .expect("durable terminal");
        assert_eq!(
            stored
                .apply_transition(&swap_id, transitions[0].clone())
                .await
                .expect("historical replay"),
            BtcLifecycleTransitionOutcomeV1::AlreadyApplied { revision: 1 }
        );
        let after = store
            .load(&swap_id, Participant::Maker)
            .await
            .expect("load after replay")
            .expect("durable terminal");
        assert_eq!(before.sha256(), after.sha256());

        let taker = StoredBtcLifecycleSdk::new(
            BtcPairSdk::new(
                Participant::Taker,
                BtcChainPolicyV1::new(BITCOIN_GENESIS, REQUIRED_CONFIRMATIONS),
            ),
            store.clone(),
        );
        assert!(taker.resume(&swap_id).await.expect("role lookup").is_none());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn public_runtime_completes_both_ordered_refund_directions() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        let (fixture, pair, prepared, transitions) = refund_lifecycle_fixture(direction);
        let store = InMemoryBtcLifecycleStore::default();
        let stored = StoredBtcLifecycleSdk::new(pair.clone(), store.clone());
        let active = stored
            .activate(
                pair.accept_wire(&fixture.wire)
                    .expect("accepted refund wire"),
                prepared,
            )
            .await
            .expect("durable refund activation");
        let swap_id = active.status().swap_id().clone();
        let (bitcoin_script, lez_script) = split_chain_scripts(direction, &transitions);
        let runtime = BtcLifecycleRuntime::new(
            stored,
            ScriptedLifecyclePort::new(bitcoin_script),
            ScriptedLifecyclePort::new(lez_script),
        );
        for expected_revision in 1..=4 {
            let BtcLifecycleDriveOutcomeV1::Transition { status, .. } =
                runtime.drive_once(&swap_id).await.unwrap_or_else(|error| {
                    panic!("refund drive revision {expected_revision}: {error:?}")
                })
            else {
                panic!("expected refund transition");
            };
            assert_eq!(status.revision(), expected_revision);
        }
        assert!(matches!(
            runtime.drive_once(&swap_id).await.expect("terminal refund"),
            BtcLifecycleDriveOutcomeV1::Complete(status)
                if status.phase() == Phase::Refunded && status.revision() == 4
        ));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn public_runtime_rejects_chain_substitution_without_store_change() {
    let (fixture, pair, prepared, _) = claim_lifecycle_fixture(SwapDirection::TakerSellsForeign);
    let store = InMemoryBtcLifecycleStore::default();
    let stored = StoredBtcLifecycleSdk::new(pair.clone(), store.clone());
    let active = stored
        .activate(
            pair.accept_wire(&fixture.wire).expect("accepted wire"),
            prepared,
        )
        .await
        .expect("durable activation");
    let swap_id = active.status().swap_id().clone();
    let before = store
        .load(&swap_id, Participant::Maker)
        .await
        .expect("load before substitution")
        .expect("durable activation");
    let runtime = BtcLifecycleRuntime::new(
        stored,
        ScriptedLifecyclePort::new(vec![BtcLifecycleTransitionV1::FirstLockConfirmed(
            lez_evidence(true),
        )]),
        ScriptedLifecyclePort::default(),
    );
    assert!(matches!(
        runtime.drive_once(&swap_id).await,
        Err(BtcSdkError::WrongFirstLockChain)
    ));
    let after = store
        .load(&swap_id, Participant::Maker)
        .await
        .expect("load after substitution")
        .expect("durable activation");
    assert_eq!(before, after);
}
#[test]
fn f7_prepared_locks_reject_txid_matching_wrong_bitcoin_output() {
    let fixture = fixture(SwapDirection::TakerSellsForeign);
    let base = BtcAgreementV1::validate(fixture.record.clone()).expect("base agreement");
    let mut wrong_output_funding = fixture.funding.clone();
    wrong_output_funding.output[0].value = Amount::from_sat(FUNDING_VALUE_SAT + 1);
    let bitcoin_claimant = base.bitcoin_funder().other();
    let claim = CooperativeKeyPathSpend::new(
        base.p2tr_contract(),
        OutPoint {
            txid: wrong_output_funding.compute_txid(),
            vout: 0,
        },
        Amount::from_sat(FUNDING_VALUE_SAT),
        vec![TxOut {
            value: Amount::from_sat(CLAIM_VALUE_SAT),
            script_pubkey: ScriptBuf::from_bytes(
                base.participant(bitcoin_claimant)
                    .claim_destination_script_pubkey()
                    .to_vec(),
            ),
        }],
    )
    .expect("agreement-consistent claim");
    let original = fixture.record.body();
    let body = BtcAgreementBodyV1::new(
        *original.swap_id(),
        original.direction(),
        *original.bitcoin_chain_policy(),
        original.participants().clone(),
        *original.adaptor_point(),
        original.lez_terms().clone(),
        original.p2tr_terms().clone(),
        BtcFundingTermsV1::new(
            wrong_output_funding.compute_txid().to_byte_array(),
            0,
            FUNDING_VALUE_SAT,
        ),
        BtcClaimTermsV1::from_spend(&claim).expect("claim terms"),
        *original.recovery_plan(),
    );
    let commitment = body.commitment();
    let record = BtcAgreementRecordV1::from_parts(
        BTC_AGREEMENT_SCHEMA_V1,
        body,
        commitment,
        agreement_signature(&secret(1), commitment),
        agreement_signature(&secret(2), commitment),
    );
    let agreement = BtcAgreementV1::validate(record).expect("wrong-output agreement");
    let extension = f7_extension(&agreement, BtcLezAssetV1::Native);
    let bitcoin = PreparedBitcoinFundingV1::new(
        wrong_output_funding.compute_txid().to_string(),
        serialize(&wrong_output_funding),
    )
    .expect("exact wrong-output funding");
    assert_eq!(
        BtcLezAssetPreparedLockEffectsV1::new(
            &agreement,
            extension,
            bitcoin,
            PreparedLezAssetFundingV1::native(f7_plan(false, 16)).expect("native plan"),
        ),
        Err(BtcLezAssetSdkError::BitcoinFundingOutputMismatch)
    );
}
