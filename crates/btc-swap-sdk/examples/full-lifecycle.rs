//! One LEZ/BTC swap through every lifecycle stage, then one that refunds.
//!
//! Run it with:
//!
//! ```sh
//! cargo run -p lez-btc-swap-sdk --example full-lifecycle
//! ```
//!
//! This is the worked example behind [the SDK map](../../../docs/sdk-lez-btc.md).
//! It covers the five stages the milestone names — offer discovery,
//! negotiation, escrow creation, claim, and refund — against in-process ports
//! and fixed secrets, so it is deterministic and performs no I/O.
//!
//! What it deliberately does not do: it ships no Delivery or Chat transport
//! (`OfferDiscovery` and `NegotiationChannel` are implemented here in memory,
//! the way an application would supply them), and it does not build LEZ
//! transactions. The SDK consumes exact pre-signed LEZ bytes; constructing them
//! belongs to `lez-bridge-adapter`. The byte strings standing in for them below
//! are placeholders for that reason, not a simplification of the protocol.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bitcoin::absolute::LockTime;
use bitcoin::consensus::serialize;
use bitcoin::hashes::Hash as _;
use bitcoin::secp256k1::{Keypair, Message, PublicKey, Secp256k1, SecretKey};
use bitcoin::transaction::Version;
use bitcoin::{Amount, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid, Witness};
use lez_btc_swap_sdk::{
    AdaptorSessionContext, AdaptorSigner, BTC_AGREEMENT_SCHEMA_V1, BitcoinCanonicalRecoveryStateV1,
    BitcoinFirstLockEvidenceV1, BitcoinFollowupClaimEvidenceV1, BtcAdaptorSessionDomain,
    BtcAgreementBodyV1, BtcAgreementRecordV1, BtcAgreementV1, BtcCanonicalRecoveryStateV1,
    BtcChainPolicyV1, BtcClaimTermsV1, BtcFirstLockEvidenceV1, BtcFollowupClaimEvidenceV1,
    BtcFundingTermsV1, BtcLezTermsV1, BtcLifecycleSdk, BtcLifecycleTransitionOutcomeV1,
    BtcLifecycleTransitionV1, BtcP2trTermsV1, BtcPairSdk, BtcParticipantIdentityV1,
    BtcParticipantsV1, BtcPreparedClaimEffectsV1, BtcPreparedLockEffectsV1, BtcPreparedProtocolV1,
    BtcPreparedRecoveryEffectsV1, BtcProtocolTermsV1, BtcRecoveryPlanV1,
    BtcRevealingClaimEvidenceV1, CooperativeKeyPathSpend, CsvBlockDelay,
    LezCanonicalRecoveryStateV1, LezFirstLockEvidenceV1, P2trSwapOutput, PreparedBitcoinFundingV1,
    PreparedBitcoinRefundV1, PreparedLezClaimTemplateV1, PreparedLezFundingV1, PreparedLezRefundV1,
    RefundXOnlyKey, SigningRole, TwoPartyAggregateKey, adapt_presignature,
};
use lez_swap_core::{Participant, Phase, SwapDirection};
use lez_swap_sdk_core::{
    ExactPublicEffectPlanV1, NegotiationChannel, OfferDiscovery, SwapProtocol,
};

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

/// The direction this example walks: the Taker pays Bitcoin, the Maker pays LEZ.
const DIRECTION: SwapDirection = SwapDirection::TakerSellsForeign;

// ---------------------------------------------------------------- key material

fn secret(value: u8) -> SecretKey {
    SecretKey::from_slice(&[value; 32]).expect("fixed example secret")
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

// ------------------------------------------------------------- the agreement

/// Everything two roles agree on before either of them locks anything.
struct Fixture {
    record: BtcAgreementRecordV1,
    wire: Vec<u8>,
    lock_effects: BtcPreparedLockEffectsV1,
    funding: Transaction,
}

#[allow(clippy::too_many_lines)]
fn fixture() -> Fixture {
    let maker_secret = secret(1);
    let taker_secret = secret(2);
    let adaptor_secret = secret(7);

    let participants = BtcParticipantsV1::new(
        BtcParticipantIdentityV1::new(
            [10; 32],
            compressed_public_key(&maker_secret),
            x_only_public_key(&secret(3)),
            destination(&secret(5)),
        ),
        BtcParticipantIdentityV1::new(
            [11; 32],
            compressed_public_key(&taker_secret),
            x_only_public_key(&secret(4)),
            destination(&secret(6)),
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

    // The Taker funds Bitcoin in this direction, so the refund branch belongs
    // to the Taker and the cooperative claim pays the Maker.
    let refund_key = participants
        .for_participant(Participant::Taker)
        .bitcoin_refund_key();
    let contract = P2trSwapOutput::new(
        TwoPartyAggregateKey::from_bytes(aggregate).expect("aggregate key"),
        RefundXOnlyKey::from_bytes(*refund_key).expect("refund key"),
        CsvBlockDelay::new(144).expect("CSV delay"),
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
                    .for_participant(Participant::Maker)
                    .claim_destination_script_pubkey()
                    .to_vec(),
            ),
        }],
    )
    .expect("cooperative claim");

    let body = BtcAgreementBodyV1::new(
        [20; 32],
        DIRECTION,
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
                .for_participant(Participant::Maker)
                .lez_owner_account(),
            *participants
                .for_participant(Participant::Taker)
                .lez_owner_account(),
            LEZ_AMOUNT,
            1_700_000_100_000,
            [19; 32],
        ),
        BtcP2trTermsV1::from_contract(&contract),
        BtcFundingTermsV1::new(funding.compute_txid().to_byte_array(), 0, FUNDING_VALUE_SAT),
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

    Fixture {
        record,
        wire,
        lock_effects: BtcPreparedLockEffectsV1::new(
            PreparedBitcoinFundingV1::new(funding.compute_txid().to_string(), serialize(&funding))
                .expect("exact Bitcoin funding"),
            PreparedLezFundingV1::new(
                LEZ_INITIALIZATION_ID,
                vec![1, 2, 3],
                LEZ_FUNDING_ID,
                vec![4, 5, 6],
            )
            .expect("exact LEZ escrow effects"),
        ),
        funding,
    }
}

// ----------------------------------------------- presigned claims and refunds

/// Runs the two-party `MuSig2` adaptor ceremony to its aggregate pre-signature.
fn complete_presignature(context: &AdaptorSessionContext) -> [u8; 65] {
    let mut maker = AdaptorSigner::new(
        context.clone(),
        SigningRole::Maker,
        secret(1).secret_bytes(),
    )
    .expect("maker signer");
    let mut taker = AdaptorSigner::new(
        context.clone(),
        SigningRole::Taker,
        secret(2).secret_bytes(),
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
    let maker_partial = maker.create_partial_signature().expect("maker partial");
    let taker_partial = taker.create_partial_signature().expect("taker partial");
    maker
        .accept_peer_partial_signature(taker_partial)
        .expect("maker accepts partial");
    taker
        .accept_peer_partial_signature(maker_partial)
        .expect("taker accepts partial");

    maker.presignature().expect("aggregate presignature")
}

/// The role-fixed SDK plus every pre-signed effect it needs to drive a swap.
struct Prepared {
    fixture: Fixture,
    sdk: BtcPairSdk,
    protocol: BtcPreparedProtocolV1,
    /// Only the LEZ leg's pre-signature is adapted below, because in this
    /// direction the Taker's LEZ claim is the revealing one. The Bitcoin leg is
    /// pre-signed too — `BtcPreparedClaimEffectsV1` takes both — and the Maker
    /// completes it from the secret the LEZ claim publishes.
    lez_presignature: [u8; 65],
    lez_template: Vec<u8>,
}

fn prepare(role: Participant) -> Prepared {
    let fixture = fixture();
    let agreement = BtcAgreementV1::validate(fixture.record.clone()).expect("validated agreement");

    let bitcoin_presignature = complete_presignature(
        &agreement
            .claim_adaptor_session_context(BtcAdaptorSessionDomain::Bitcoin)
            .expect("Bitcoin claim context"),
    );
    let lez_presignature = complete_presignature(
        &agreement
            .claim_adaptor_session_context(BtcAdaptorSessionDomain::Lez)
            .expect("LEZ claim context"),
    );

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

    // The Bitcoin refund belongs to whoever funded Bitcoin: the Taker here.
    let refund_signature = Secp256k1::new()
        .sign_schnorr_no_aux_rand(
            &Message::from_digest(agreement.bitcoin_refund().sighash_bytes()),
            &Keypair::from_secret_key(&Secp256k1::new(), &secret(4)),
        )
        .serialize();
    let recovery = BtcPreparedRecoveryEffectsV1::new(
        PreparedBitcoinRefundV1::new(&agreement, refund_signature).expect("signed Bitcoin refund"),
        PreparedLezRefundV1::new(&agreement, LEZ_REFUND_ID, b"signed.lez.refund.v1".to_vec())
            .expect("signed LEZ refund"),
    );

    let sdk = BtcPairSdk::new(
        role,
        BtcChainPolicyV1::new(BITCOIN_GENESIS, REQUIRED_CONFIRMATIONS),
    );
    let terms = BtcProtocolTermsV1::new(fixture.record.clone(), fixture.lock_effects.clone())
        .with_claim_effects(claims)
        .with_recovery_effects(recovery);
    let protocol = sdk
        .prepare(sdk.validate_terms(&terms).expect("validated terms"))
        .expect("complete preparation");

    Prepared {
        fixture,
        sdk,
        protocol,
        lez_presignature,
        lez_template,
    }
}

// ------------------------------------------------------------- chain evidence

fn bitcoin_first_lock(fixture: &Fixture) -> BtcFirstLockEvidenceV1 {
    BtcFirstLockEvidenceV1::Bitcoin(
        BitcoinFirstLockEvidenceV1::new(
            BITCOIN_GENESIS,
            serialize(&fixture.funding),
            REQUIRED_CONFIRMATIONS,
        )
        .expect("Bitcoin first-lock evidence"),
    )
}

fn lez_second_lock() -> BtcFirstLockEvidenceV1 {
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
            true,
        )
        .expect("LEZ second-lock evidence"),
    )
}

/// The claim that publishes the adaptor secret. In this direction it is the
/// Taker's LEZ claim, completed by adapting the pre-signature with `t`.
fn revealing_claim(prepared: &Prepared) -> BtcRevealingClaimEvidenceV1 {
    let context = prepared
        .protocol
        .agreement()
        .claim_adaptor_session_context(BtcAdaptorSessionDomain::Lez)
        .expect("LEZ claim context");
    let signature = adapt_presignature(
        &context,
        prepared.lez_presignature,
        zeroize::Zeroizing::new(secret(7).secret_bytes()),
    )
    .expect("adapted revealing signature");

    let mut template = prepared.lez_template.clone();
    template[LEZ_CLAIM_SIGNATURE_OFFSET..LEZ_CLAIM_SIGNATURE_OFFSET + 64]
        .copy_from_slice(&signature);

    BtcRevealingClaimEvidenceV1::Lez(
        lez_btc_swap_sdk::LezRevealingClaimEvidenceV1::new(
            Participant::Taker,
            LEZ_GENESIS,
            LEZ_CLAIM_ID,
            template,
            signature,
            true,
        )
        .expect("canonical LEZ revealing claim"),
    )
}

fn followup_claim(plan: &ExactPublicEffectPlanV1) -> BtcFollowupClaimEvidenceV1 {
    let [step] = plan.steps() else {
        panic!("the follow-up plan has exactly one effect");
    };
    BtcFollowupClaimEvidenceV1::Bitcoin(
        BitcoinFollowupClaimEvidenceV1::new(
            BITCOIN_GENESIS,
            step.exact_bytes().as_slice().to_vec(),
            REQUIRED_CONFIRMATIONS,
        )
        .expect("canonical Bitcoin follow-up claim"),
    )
}

fn recovery_state(prepared: &Prepared, bitcoin_refunded: bool) -> BtcCanonicalRecoveryStateV1 {
    let bitcoin = if bitcoin_refunded {
        BitcoinCanonicalRecoveryStateV1::refunded(
            BITCOIN_GENESIS,
            *prepared
                .protocol
                .agreement()
                .funding_terms()
                .transaction_id(),
            prepared
                .protocol
                .recovery_effects()
                .expect("recovery effects")
                .bitcoin()
                .transaction_id()
                .to_byte_array(),
            REQUIRED_CONFIRMATIONS,
        )
    } else {
        BitcoinCanonicalRecoveryStateV1::locked(
            BITCOIN_GENESIS,
            *prepared
                .protocol
                .agreement()
                .funding_terms()
                .transaction_id(),
            REQUIRED_CONFIRMATIONS,
            true,
        )
    };
    BtcCanonicalRecoveryStateV1::new(
        prepared.protocol.agreement(),
        BITCOIN_REFUND_HEIGHT,
        LEZ_FOREIGN_REFUND_SECONDS,
        bitcoin,
        LezCanonicalRecoveryStateV1::refunded(
            LEZ_GENESIS,
            LEZ_INITIALIZATION_ID,
            LEZ_FUNDING_ID,
            LEZ_REFUND_ID,
            true,
        )
        .expect("canonical LEZ refund"),
    )
}

// ---------------------------------------------------------- in-process ports

/// Stands in for a Delivery adapter. A real one authenticates publishers and
/// drops expired offers before returning a reference.
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
        let mut offers = self.offers.lock().expect("discovery lock");
        let reference = offers.len();
        offers.push(offer);
        Ok(reference)
    }

    async fn discover(&self, (): &Self::Query) -> Result<Vec<Self::OfferRef>, Self::Error> {
        let length = self.offers.lock().expect("discovery lock").len();
        Ok((0..length).collect())
    }
}

/// Stands in for a Chat adapter. Whatever it returns stays untrusted until
/// `BtcPairSdk::accept_wire` has validated it.
#[derive(Clone)]
struct FixedNegotiation {
    wire: Arc<Vec<u8>>,
}

#[async_trait]
impl NegotiationChannel for FixedNegotiation {
    type Error = std::io::Error;
    type LocalProposal = ();
    type OfferRef = usize;

    async fn negotiate(
        &self,
        _local_participant: Participant,
        _offer: &Self::OfferRef,
        (): Self::LocalProposal,
    ) -> Result<Vec<u8>, Self::Error> {
        Ok(self.wire.as_ref().clone())
    }
}

// --------------------------------------------------------------------- stages

/// Offer discovery, negotiation, and escrow creation, up to an active swap.
async fn through_negotiation(prepared: &Prepared) -> lez_btc_swap_sdk::ActiveBtcSwap {
    let lifecycle = BtcLifecycleSdk::new(
        BtcPairSdk::new(
            Participant::Maker,
            BtcChainPolicyV1::new(BITCOIN_GENESIS, REQUIRED_CONFIRMATIONS),
        ),
        MemoryDiscovery::default(),
        FixedNegotiation {
            wire: Arc::new(prepared.fixture.wire.clone()),
        },
    );

    let offer = lifecycle
        .publish_offer("1000 LEZ for 0.001 BTC".to_owned())
        .await
        .expect("publish offer");
    println!("  1. offer discovery   published offer, reference {offer}");

    let found = lifecycle.discover(&()).await.expect("discover offers");
    println!("  2. offer discovery   discovered {} offer(s)", found.len());

    let accepted = lifecycle
        .negotiate(&offer, ())
        .await
        .expect("negotiate countersigned agreement");
    println!("  3. negotiation       countersigned wire accepted and validated");

    let active = lifecycle
        .activate(accepted, prepared.protocol.clone())
        .expect("activate prepared material");
    println!(
        "  4. escrow creation   activated at phase {:?}; the pre-lock ports are gone from the type",
        active.status().phase()
    );
    active
}

/// Both locks, both claims, to `Phase::Completed`.
async fn settled_swap() {
    println!("A swap that settles");
    let prepared = prepare(Participant::Maker);
    let mut active = through_negotiation(&prepared).await;

    let steps = active.first_lock_plan().steps().len();
    println!("  5. escrow creation   first-lock plan has {steps} exact effect(s)");

    let outcome = active
        .apply_transition(BtcLifecycleTransitionV1::FirstLockConfirmed(
            bitcoin_first_lock(&prepared.fixture),
        ))
        .expect("first lock confirmed");
    assert_eq!(
        outcome,
        BtcLifecycleTransitionOutcomeV1::Applied { revision: 1 }
    );
    println!(
        "  6. escrow creation   Taker's Bitcoin lock confirmed; phase {:?}",
        active.status().phase()
    );

    let _ = active
        .apply_transition(BtcLifecycleTransitionV1::SecondLockConfirmed(
            lez_second_lock(),
        ))
        .expect("second lock confirmed");
    println!(
        "  7. escrow creation   Maker's LEZ escrow finalized; phase {:?}",
        active.status().phase()
    );

    let revealing = revealing_claim(&prepared);
    let material = prepared
        .sdk
        .validate_revealing_claim(&prepared.protocol, &revealing)
        .expect("extract the adaptor secret and check it against T");
    println!("  8. claim             revealing claim published; secret extracted");

    let _ = active
        .apply_transition(BtcLifecycleTransitionV1::RevealingClaimConfirmed(revealing))
        .expect("revealing claim confirmed");

    let plan = prepared
        .sdk
        .build_followup_claim(&prepared.protocol, &material)
        .expect("build the follow-up claim from the extracted secret");
    let _ = active
        .apply_transition(BtcLifecycleTransitionV1::FollowupClaimConfirmed(
            followup_claim(&plan),
        ))
        .expect("follow-up claim confirmed");
    println!(
        "  9. claim             follow-up claim confirmed; phase {:?}, revision {}",
        active.status().phase(),
        active.status().revision()
    );

    assert_eq!(active.status().phase(), Phase::Completed);

    // A restart replays durable state instead of re-sending anything.
    let resumed = prepared
        .sdk
        .resume(active.durable_envelope())
        .expect("resume from the durable envelope");
    assert_eq!(resumed.status(), active.status());
    println!(" 10. durability       resumed from the durable envelope, same status");
}

/// The same agreement, nobody claims, both legs refund in signed order.
fn refunded_swap() {
    println!("\nA swap that refunds");
    let prepared = prepare(Participant::Maker);
    let mut active = prepared
        .sdk
        .activate_prepared(
            prepared
                .sdk
                .accept_wire(&prepared.fixture.wire)
                .expect("accept wire"),
            prepared.protocol.clone(),
        )
        .expect("activate");

    let _ = active
        .apply_transition(BtcLifecycleTransitionV1::FirstLockConfirmed(
            bitcoin_first_lock(&prepared.fixture),
        ))
        .expect("first lock confirmed");
    let _ = active
        .apply_transition(BtcLifecycleTransitionV1::SecondLockConfirmed(
            lez_second_lock(),
        ))
        .expect("second lock confirmed");
    println!("  1. escrow creation   both legs locked, and nobody claims");

    let _ = active
        .apply_transition(BtcLifecycleTransitionV1::RecoveryObserved(recovery_state(
            &prepared, false,
        )))
        .expect("earlier refund observed");
    println!(
        "  2. refund            Maker-funded LEZ leg refunded first; phase {:?}",
        active.status().phase()
    );

    let _ = active
        .apply_transition(BtcLifecycleTransitionV1::RecoveryObserved(recovery_state(
            &prepared, true,
        )))
        .expect("later refund observed");
    println!(
        "  3. refund            Taker-funded Bitcoin leg refunded; phase {:?}, revision {}",
        active.status().phase(),
        active.status().revision()
    );

    assert_eq!(active.status().phase(), Phase::Refunded);
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    println!("lez-btc-swap-sdk: offer discovery, negotiation, escrow, claim, refund\n");
    settled_swap().await;
    refunded_swap();
    println!("\nBoth terminal states reached. No I/O was performed.");
}
