use bitcoin::hashes::Hash as _;
use bitcoin::hex::DisplayHex as _;
use bitcoin::secp256k1::{Keypair, Message, PublicKey, Secp256k1, SecretKey};
use bitcoin::{Amount, OutPoint, ScriptBuf, TxOut, Txid};
use lez_btc_swap_sdk::{
    AdaptorSessionContext, BTC_AGREEMENT_SCHEMA_V1, BtcAgreementBodyV1, BtcAgreementRecordV1,
    BtcAgreementV1, BtcAgreementV1Error, BtcChainPolicyV1, BtcClaimTermsV1, BtcFundingTermsV1,
    BtcLezTermsV1, BtcP2trTermsV1, BtcParticipantIdentityV1, BtcParticipantsV1, BtcRecoveryPlanV1,
    CsvBlockDelay, MAX_BITCOIN_REQUIRED_CONFIRMATIONS, MAX_BTC_AGREEMENT_RECORD_BYTES,
    P2trSwapOutput, RefundXOnlyKey, TwoPartyAggregateKey,
};
use lez_swap_core::{Chain, MakerRecoveryTrigger, Pair, Participant, Phase, SwapDirection};

const FUNDING_VALUE_SAT: u64 = 100_000;
const CLAIM_VALUE_SAT: u64 = 99_000;
const REFUND_CSV_BLOCKS: u32 = 144;
const BITCOIN_FUNDING_ANCHOR_HEIGHT: u32 = 1_000;
const BITCOIN_REFUND_HEIGHT: u32 = 1_144;
const EARLIER_REFUND_LATEST_SECONDS: u64 = 1_700_000_100;
const LATER_REFUND_EARLIEST_SECONDS: u64 = 1_700_000_500;
const REQUIRED_MARGIN_SECONDS: u64 = 300;
const BITCOIN_GENESIS_HASH: [u8; 32] = [8; 32];
const REQUIRED_CONFIRMATIONS: u32 = 6;
const FIRST_DESTINATION_LENGTH_OFFSET: usize = 2 + 32 + 1 + 32 + 4 + 32 + 33 + 32;

#[derive(Clone, Copy)]
struct FixtureOptions {
    refund_role: Option<Participant>,
    drift_contract_script: bool,
    drift_claim_sighash: bool,
    wrong_refund_height: bool,
    maker_destination_length: Option<usize>,
    bitcoin_genesis_hash: [u8; 32],
    required_confirmations: u32,
}

impl Default for FixtureOptions {
    fn default() -> Self {
        Self {
            refund_role: None,
            drift_contract_script: false,
            drift_claim_sighash: false,
            wrong_refund_height: false,
            maker_destination_length: None,
            bitcoin_genesis_hash: BITCOIN_GENESIS_HASH,
            required_confirmations: REQUIRED_CONFIRMATIONS,
        }
    }
}

struct Fixture {
    body: BtcAgreementBodyV1,
    maker_secret: SecretKey,
    taker_secret: SecretKey,
    expected_unsigned_transaction: Vec<u8>,
    expected_sighash: [u8; 32],
}

fn secret(value: u8) -> SecretKey {
    SecretKey::from_slice(&[value; 32]).expect("valid fixed secret")
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

fn claim_destination(secret: &SecretKey) -> Vec<u8> {
    let key = Keypair::from_secret_key(&Secp256k1::new(), secret)
        .x_only_public_key()
        .0;
    ScriptBuf::new_p2tr(&Secp256k1::verification_only(), key, None).into_bytes()
}

// Keeping the complete canonical fixture assembly together makes field drift
// visible in the agreement tests; production paths remain split by concern.
#[allow(clippy::too_many_lines)]
fn fixture(direction: SwapDirection, options: FixtureOptions) -> Fixture {
    let maker_secret = secret(1);
    let taker_secret = secret(2);
    let maker_refund_secret = secret(3);
    let taker_refund_secret = secret(4);
    let maker_claim_secret = secret(5);
    let taker_claim_secret = secret(6);
    let adaptor_secret = secret(7);

    let maker = BtcParticipantIdentityV1::new(
        [10; 32],
        compressed_public_key(&maker_secret),
        x_only_public_key(&maker_refund_secret),
        if let Some(length) = options.maker_destination_length {
            vec![0x51; length]
        } else {
            claim_destination(&maker_claim_secret)
        },
    );
    let taker = BtcParticipantIdentityV1::new(
        [11; 32],
        compressed_public_key(&taker_secret),
        x_only_public_key(&taker_refund_secret),
        claim_destination(&taker_claim_secret),
    );
    let participants = BtcParticipantsV1::new(maker, taker);
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
    .expect("valid aggregate context")
    .output_key();

    let bitcoin_funder = match direction {
        SwapDirection::TakerSellsForeign => Participant::Taker,
        SwapDirection::TakerSellsLez => Participant::Maker,
    };
    let bitcoin_claimant = bitcoin_funder.other();
    let refund_role = options.refund_role.unwrap_or(bitcoin_funder);
    let refund_key = match refund_role {
        Participant::Maker => x_only_public_key(&maker_refund_secret),
        Participant::Taker => x_only_public_key(&taker_refund_secret),
    };
    let contract = P2trSwapOutput::new(
        TwoPartyAggregateKey::from_bytes(aggregate).expect("aggregate key"),
        RefundXOnlyKey::from_bytes(refund_key).expect("refund key"),
        CsvBlockDelay::new(REFUND_CSV_BLOCKS).expect("CSV"),
    )
    .expect("contract");
    let mut p2tr = BtcP2trTermsV1::from_contract(&contract);
    if options.drift_contract_script {
        let mut script_pubkey = contract.script_pubkey_bytes().to_vec();
        script_pubkey[0] ^= 1;
        p2tr = BtcP2trTermsV1::from_parts(
            contract.aggregate_internal_key_bytes(),
            contract.refund_key_bytes(),
            u32::from(contract.refund_delay().blocks()),
            contract.refund_leaf_version(),
            contract.refund_script_bytes().to_vec(),
            contract.tapleaf_hash_bytes(),
            contract.merkle_root_bytes(),
            contract.tap_tweak_hash_bytes(),
            contract.output_key_bytes(),
            contract.output_key_parity().into(),
            contract.refund_control_block_bytes(),
            script_pubkey,
        );
    }

    let funding = BtcFundingTermsV1::new([21; 32], 1, FUNDING_VALUE_SAT);
    let destination = participants
        .for_participant(bitcoin_claimant)
        .claim_destination_script_pubkey()
        .to_vec();
    let spend = lez_btc_swap_sdk::CooperativeKeyPathSpend::new(
        &contract,
        OutPoint {
            txid: Txid::from_byte_array(*funding.transaction_id()),
            vout: funding.output_index(),
        },
        Amount::from_sat(funding.value_sat()),
        vec![TxOut {
            value: Amount::from_sat(CLAIM_VALUE_SAT),
            script_pubkey: ScriptBuf::from_bytes(destination),
        }],
    )
    .expect("claim");
    let expected_unsigned_transaction = spend.unsigned_transaction_bytes();
    let expected_sighash = spend.sighash_bytes();
    let mut claim = BtcClaimTermsV1::from_spend(&spend).expect("single-output claim");
    if options.drift_claim_sighash {
        let mut changed = spend.sighash_bytes();
        changed[0] ^= 1;
        claim = BtcClaimTermsV1::from_parts(
            spend.unsigned_transaction().output[0]
                .script_pubkey
                .as_bytes()
                .to_vec(),
            spend.unsigned_transaction().output[0].value.to_sat(),
            spend.fee().to_sat(),
            spend.unsigned_transaction_bytes(),
            changed,
        );
    }

    let lez_depositor = match direction {
        SwapDirection::TakerSellsForeign => Participant::Maker,
        SwapDirection::TakerSellsLez => Participant::Taker,
    };
    let lez_claimant = lez_depositor.other();
    let lez_refund_at_ms = match direction {
        SwapDirection::TakerSellsForeign => EARLIER_REFUND_LATEST_SECONDS * 1_000,
        SwapDirection::TakerSellsLez => LATER_REFUND_EARLIEST_SECONDS * 1_000,
    };
    let lez = BtcLezTermsV1::new(
        [17; 32],
        [18; 32],
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
        5_000,
        lez_refund_at_ms,
        [19; 32],
    );
    let recovery = BtcRecoveryPlanV1::new(
        BITCOIN_FUNDING_ANCHOR_HEIGHT,
        if options.wrong_refund_height {
            BITCOIN_REFUND_HEIGHT + 1
        } else {
            BITCOIN_REFUND_HEIGHT
        },
        EARLIER_REFUND_LATEST_SECONDS,
        LATER_REFUND_EARLIEST_SECONDS,
        REQUIRED_MARGIN_SECONDS,
    );
    let body = BtcAgreementBodyV1::new(
        [20; 32],
        direction,
        BtcChainPolicyV1::new(options.bitcoin_genesis_hash, options.required_confirmations),
        participants,
        adaptor_point,
        lez,
        p2tr,
        funding,
        claim,
        recovery,
    );
    Fixture {
        body,
        maker_secret,
        taker_secret,
        expected_unsigned_transaction,
        expected_sighash,
    }
}

fn signature(secret: &SecretKey, commitment: [u8; 32]) -> [u8; 64] {
    Secp256k1::new()
        .sign_schnorr_no_aux_rand(
            &Message::from_digest(commitment),
            &Keypair::from_secret_key(&Secp256k1::new(), secret),
        )
        .serialize()
}

fn signed_record(fixture: &Fixture) -> BtcAgreementRecordV1 {
    let commitment = fixture.body.commitment();
    BtcAgreementRecordV1::from_parts(
        BTC_AGREEMENT_SCHEMA_V1,
        fixture.body.clone(),
        commitment,
        signature(&fixture.maker_secret, commitment),
        signature(&fixture.taker_secret, commitment),
    )
}

fn assert_actor_activation_view(agreement: &BtcAgreementV1) {
    assert_eq!(agreement.lez_terms().channel_id(), &[17; 32]);
    assert_eq!(agreement.lez_terms().genesis_block_hash(), &[18; 32]);
    assert_eq!(agreement.lez_terms().escrow_program_id(), &[15; 32]);
    assert_eq!(
        agreement.lez_terms().authenticated_transfer_program_id(),
        &[16; 32]
    );
    assert_eq!(
        agreement.lez_terms().aggregate_authority_account(),
        &[12; 32]
    );
    assert_eq!(agreement.lez_terms().metadata_account(), &[13; 32]);
    assert_eq!(agreement.lez_terms().custody_account(), &[14; 32]);
    assert_eq!(agreement.lez_terms().amount(), 5_000);
    assert_eq!(
        agreement.lez_terms().depositor_account(),
        agreement
            .participant(agreement.lez_depositor())
            .lez_owner_account()
    );
    assert_eq!(
        agreement.lez_terms().claimant_account(),
        agreement
            .participant(agreement.lez_claimant())
            .lez_owner_account()
    );

    let recovery = agreement.body().recovery_plan();
    assert_eq!(
        recovery.bitcoin_funding_anchor_height(),
        BITCOIN_FUNDING_ANCHOR_HEIGHT
    );
    assert_eq!(recovery.bitcoin_refund_height(), BITCOIN_REFUND_HEIGHT);
    assert_eq!(recovery.required_margin_seconds(), REQUIRED_MARGIN_SECONDS);
    assert!(
        recovery.later_refund_earliest_unix_seconds()
            >= recovery
                .earlier_refund_latest_unix_seconds()
                .checked_add(recovery.required_margin_seconds())
                .expect("fixture margin does not overflow")
    );
}

#[test]
fn canonical_countersigned_agreement_reconstructs_both_directions() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        let fixture = fixture(direction, FixtureOptions::default());
        let wire = signed_record(&fixture).encode_wire().expect("wire");
        let bitcoin_policy = BtcChainPolicyV1::new(BITCOIN_GENESIS_HASH, REQUIRED_CONFIRMATIONS);
        let agreement = BtcAgreementV1::from_wire_for_bitcoin_policy(&wire, &bitcoin_policy)
            .expect("agreement");

        assert_eq!(agreement.encode_wire().expect("wire replay"), wire);
        assert_eq!(agreement.record().body(), agreement.body());
        assert_eq!(
            agreement.record().agreement_commitment(),
            agreement.agreement_commitment()
        );
        assert_eq!(agreement.participants(), agreement.body().participants());
        assert_eq!(
            agreement.participant(Participant::Maker),
            agreement.participants().for_participant(Participant::Maker)
        );
        assert_eq!(agreement.adaptor_point(), agreement.body().adaptor_point());
        assert_eq!(agreement.funding_terms(), agreement.body().funding_terms());
        assert_eq!(
            agreement.bitcoin_chain_policy(),
            agreement.body().bitcoin_chain_policy()
        );
        assert_eq!(
            agreement.body().encode_canonical().expect("canonical body"),
            borsh::to_vec(agreement.body()).expect("canonical body")
        );
        assert_eq!(agreement.direction(), direction);
        assert_eq!(
            agreement.role_session_binding(),
            *agreement.agreement_commitment()
        );
        assert_eq!(
            agreement.lez_terms_binding(),
            *agreement.agreement_commitment()
        );
        assert_eq!(
            agreement.cooperative_claim().unsigned_transaction_bytes(),
            fixture.expected_unsigned_transaction
        );
        assert_eq!(
            agreement.cooperative_claim().sighash_bytes(),
            fixture.expected_sighash
        );
        assert_eq!(
            agreement.bitcoin_funder(),
            match direction {
                SwapDirection::TakerSellsForeign => Participant::Taker,
                SwapDirection::TakerSellsLez => Participant::Maker,
            }
        );
        assert_eq!(
            agreement.bitcoin_claimant(),
            agreement.bitcoin_funder().other()
        );
        assert_eq!(agreement.lez_claimant(), agreement.lez_depositor().other());
        assert_actor_activation_view(&agreement);

        let maker_trigger = agreement.recovery_schedule().maker_trigger();
        assert!(matches!(maker_trigger, MakerRecoveryTrigger::Deadline(_)));
        assert_eq!(
            agreement
                .recovery_schedule()
                .deadline_for_chain(Chain::Bitcoin)
                .expect("Bitcoin refund")
                .value(),
            u64::from(BITCOIN_REFUND_HEIGHT)
        );
        assert_eq!(
            agreement
                .recovery_schedule()
                .deadline_for_chain(Chain::Lez)
                .expect("LEZ refund")
                .value(),
            agreement.lez_terms().refund_at_ms() / 1_000
        );
    }
}

#[test]
fn validated_agreement_derives_exact_role_neutral_initial_coordinator() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        let fixture = fixture(direction, FixtureOptions::default());
        let agreement =
            BtcAgreementV1::validate(signed_record(&fixture)).expect("validated agreement");
        let coordinator = agreement.coordinator();

        assert_eq!(
            coordinator.id().as_str(),
            agreement.body().swap_id().to_lower_hex_string()
        );
        assert_eq!(coordinator.pair(), Pair::Bitcoin);
        assert_eq!(coordinator.direction(), direction);
        assert_eq!(coordinator.phase(), Phase::Offered);
        assert_eq!(
            coordinator.recovery_schedule(),
            agreement.recovery_schedule()
        );

        for role in [Participant::Taker, Participant::Maker] {
            let expected_chain = match (direction, role) {
                (SwapDirection::TakerSellsForeign, Participant::Taker)
                | (SwapDirection::TakerSellsLez, Participant::Maker) => Chain::Bitcoin,
                (SwapDirection::TakerSellsForeign, Participant::Maker)
                | (SwapDirection::TakerSellsLez, Participant::Taker) => Chain::Lez,
            };
            assert_eq!(coordinator.funded_chain(role), expected_chain);
            assert_eq!(
                coordinator.required_confirmations(role),
                if expected_chain == Chain::Bitcoin {
                    REQUIRED_CONFIRMATIONS
                } else {
                    1
                }
            );
            assert_eq!(coordinator.funding_transaction_id(role), None);
        }

        assert_eq!(
            coordinator.funded_chain(Participant::Taker),
            match direction {
                SwapDirection::TakerSellsForeign => Chain::Bitcoin,
                SwapDirection::TakerSellsLez => Chain::Lez,
            }
        );
        assert_eq!(
            coordinator.funded_chain(Participant::Maker),
            match direction {
                SwapDirection::TakerSellsForeign => Chain::Lez,
                SwapDirection::TakerSellsLez => Chain::Bitcoin,
            }
        );
    }
}

#[test]
fn invalid_or_locally_unsupported_bitcoin_policy_never_derives_a_coordinator() {
    let expected = BtcChainPolicyV1::new(BITCOIN_GENESIS_HASH, REQUIRED_CONFIRMATIONS);
    let unsupported = fixture(
        SwapDirection::TakerSellsForeign,
        FixtureOptions {
            required_confirmations: REQUIRED_CONFIRMATIONS + 1,
            ..FixtureOptions::default()
        },
    );
    let unsupported_wire = signed_record(&unsupported).encode_wire().expect("wire");
    assert_eq!(
        BtcAgreementV1::from_wire_for_bitcoin_policy(&unsupported_wire, &expected),
        Err(BtcAgreementV1Error::BitcoinChainPolicyMismatch)
    );

    let valid = fixture(SwapDirection::TakerSellsForeign, FixtureOptions::default());
    let signed = signed_record(&valid);
    let corrupted = BtcAgreementRecordV1::from_parts(
        BTC_AGREEMENT_SCHEMA_V1,
        unsupported.body,
        *signed.agreement_commitment(),
        *signed.signature(Participant::Maker),
        *signed.signature(Participant::Taker),
    );
    assert_eq!(
        BtcAgreementV1::validate(corrupted),
        Err(BtcAgreementV1Error::CommitmentMismatch)
    );

    let invalid = fixture(
        SwapDirection::TakerSellsLez,
        FixtureOptions {
            required_confirmations: 0,
            ..FixtureOptions::default()
        },
    );
    assert_eq!(
        BtcAgreementV1::validate(signed_record(&invalid)),
        Err(BtcAgreementV1Error::InvalidBitcoinChainPolicy)
    );
}

#[test]
fn bitcoin_chain_and_confirmation_policy_are_bound_and_enforced() {
    let expected = BtcChainPolicyV1::new(BITCOIN_GENESIS_HASH, REQUIRED_CONFIRMATIONS);
    let valid = fixture(SwapDirection::TakerSellsForeign, FixtureOptions::default());
    let agreement = BtcAgreementV1::validate_for_bitcoin_policy(signed_record(&valid), &expected)
        .expect("matching Bitcoin policy");
    assert_eq!(agreement.bitcoin_genesis_hash(), &BITCOIN_GENESIS_HASH);
    assert_eq!(
        agreement.required_bitcoin_confirmations(),
        REQUIRED_CONFIRMATIONS
    );

    let wrong_genesis = fixture(
        SwapDirection::TakerSellsForeign,
        FixtureOptions {
            bitcoin_genesis_hash: [9; 32],
            ..FixtureOptions::default()
        },
    );
    assert_eq!(
        BtcAgreementV1::validate_for_bitcoin_policy(signed_record(&wrong_genesis), &expected),
        Err(BtcAgreementV1Error::BitcoinChainPolicyMismatch)
    );

    let signed_valid = signed_record(&valid);
    let drifted_after_signing = BtcAgreementRecordV1::from_parts(
        BTC_AGREEMENT_SCHEMA_V1,
        wrong_genesis.body.clone(),
        *signed_valid.agreement_commitment(),
        *signed_valid.signature(Participant::Maker),
        *signed_valid.signature(Participant::Taker),
    );
    assert_eq!(
        BtcAgreementV1::validate(drifted_after_signing),
        Err(BtcAgreementV1Error::CommitmentMismatch)
    );

    let wrong_confirmations = fixture(
        SwapDirection::TakerSellsForeign,
        FixtureOptions {
            required_confirmations: REQUIRED_CONFIRMATIONS + 1,
            ..FixtureOptions::default()
        },
    );
    assert_eq!(
        BtcAgreementV1::validate_for_bitcoin_policy(signed_record(&wrong_confirmations), &expected),
        Err(BtcAgreementV1Error::BitcoinChainPolicyMismatch)
    );

    for required_confirmations in [0, MAX_BITCOIN_REQUIRED_CONFIRMATIONS + 1] {
        let invalid = fixture(
            SwapDirection::TakerSellsForeign,
            FixtureOptions {
                required_confirmations,
                ..FixtureOptions::default()
            },
        );
        assert_eq!(
            BtcAgreementV1::validate(signed_record(&invalid)),
            Err(BtcAgreementV1Error::InvalidBitcoinChainPolicy)
        );
    }
}

#[test]
fn derived_bitcoin_drift_is_rejected_even_when_both_roles_sign_it() {
    let contract_drift = fixture(
        SwapDirection::TakerSellsForeign,
        FixtureOptions {
            drift_contract_script: true,
            ..FixtureOptions::default()
        },
    );
    assert_eq!(
        BtcAgreementV1::validate(signed_record(&contract_drift)),
        Err(BtcAgreementV1Error::BitcoinContractMismatch)
    );

    let claim_drift = fixture(
        SwapDirection::TakerSellsForeign,
        FixtureOptions {
            drift_claim_sighash: true,
            ..FixtureOptions::default()
        },
    );
    assert_eq!(
        BtcAgreementV1::validate(signed_record(&claim_drift)),
        Err(BtcAgreementV1Error::BitcoinClaimMismatch)
    );
}

#[test]
fn schema_trailing_and_oversized_wire_fail_before_acceptance() {
    let fixture = fixture(SwapDirection::TakerSellsForeign, FixtureOptions::default());
    let canonical = signed_record(&fixture);
    let mut trailing = canonical.encode_wire().expect("wire");
    trailing.push(0);
    assert_eq!(
        BtcAgreementV1::from_wire(&trailing),
        Err(BtcAgreementV1Error::MalformedWireRecord)
    );

    let unsupported = BtcAgreementRecordV1::from_parts(
        BTC_AGREEMENT_SCHEMA_V1 + 1,
        fixture.body.clone(),
        fixture.body.commitment(),
        [0; 64],
        [0; 64],
    )
    .encode_wire()
    .expect("bounded unsupported wire");
    assert_eq!(
        BtcAgreementV1::from_wire(&unsupported),
        Err(BtcAgreementV1Error::UnsupportedSchema(
            BTC_AGREEMENT_SCHEMA_V1 + 1
        ))
    );

    let oversized = vec![0; MAX_BTC_AGREEMENT_RECORD_BYTES + 1];
    assert_eq!(
        BtcAgreementV1::from_wire(&oversized),
        Err(BtcAgreementV1Error::OversizedWireRecord {
            actual: MAX_BTC_AGREEMENT_RECORD_BYTES + 1,
            maximum: MAX_BTC_AGREEMENT_RECORD_BYTES,
        })
    );

    let mut hostile_length = canonical.encode_wire().expect("wire");
    hostile_length[FIRST_DESTINATION_LENGTH_OFFSET..FIRST_DESTINATION_LENGTH_OFFSET + 4]
        .copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(
        BtcAgreementV1::from_wire(&hostile_length),
        Err(BtcAgreementV1Error::MalformedWireRecord)
    );
}

#[test]
fn caller_constructed_oversized_field_fails_before_total_wire_encoding() {
    // In this direction the taker is the Bitcoin claimant, so fixture
    // construction never needs to interpret the hostile maker destination.
    let hostile = fixture(
        SwapDirection::TakerSellsLez,
        FixtureOptions {
            maker_destination_length: Some(MAX_BTC_AGREEMENT_RECORD_BYTES + 1),
            ..FixtureOptions::default()
        },
    );

    assert_eq!(
        BtcAgreementV1::validate(signed_record(&hostile)),
        Err(BtcAgreementV1Error::InvalidIdentity)
    );
}

#[test]
fn swapped_signatures_and_cross_role_refund_authority_fail_closed() {
    let base = fixture(SwapDirection::TakerSellsForeign, FixtureOptions::default());
    let commitment = base.body.commitment();
    let swapped = BtcAgreementRecordV1::from_parts(
        BTC_AGREEMENT_SCHEMA_V1,
        base.body.clone(),
        commitment,
        signature(&base.taker_secret, commitment),
        signature(&base.maker_secret, commitment),
    );
    assert_eq!(
        BtcAgreementV1::validate(swapped),
        Err(BtcAgreementV1Error::SignatureMismatch(Participant::Maker))
    );

    let corrupt_signature = BtcAgreementRecordV1::from_parts(
        BTC_AGREEMENT_SCHEMA_V1,
        base.body.clone(),
        commitment,
        [0xff; 64],
        signature(&base.taker_secret, commitment),
    );
    assert_eq!(
        BtcAgreementV1::validate(corrupt_signature),
        Err(BtcAgreementV1Error::SignatureMismatch(Participant::Maker))
    );

    let wrong_refunder = fixture(
        SwapDirection::TakerSellsForeign,
        FixtureOptions {
            refund_role: Some(Participant::Maker),
            ..FixtureOptions::default()
        },
    );
    assert_eq!(
        BtcAgreementV1::validate(signed_record(&wrong_refunder)),
        Err(BtcAgreementV1Error::BitcoinRefundRoleMismatch)
    );
}

#[test]
fn direction_correct_recovery_schedule_is_reconstructed_and_checked() {
    let wrong_height = fixture(
        SwapDirection::TakerSellsLez,
        FixtureOptions {
            wrong_refund_height: true,
            ..FixtureOptions::default()
        },
    );
    assert_eq!(
        BtcAgreementV1::validate(signed_record(&wrong_height)),
        Err(BtcAgreementV1Error::RecoveryScheduleMismatch)
    );

    let valid = fixture(SwapDirection::TakerSellsLez, FixtureOptions::default());
    let agreement = BtcAgreementV1::validate(signed_record(&valid)).expect("valid schedule");
    assert_eq!(agreement.lez_depositor(), Participant::Taker);
    assert_eq!(agreement.bitcoin_funder(), Participant::Maker);
}
