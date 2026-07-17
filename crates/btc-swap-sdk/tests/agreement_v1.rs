use bitcoin::hashes::Hash as _;
use bitcoin::hex::DisplayHex as _;
use bitcoin::secp256k1::{Keypair, Message, PublicKey, Secp256k1, SecretKey};
use bitcoin::{Amount, OutPoint, ScriptBuf, TxOut, Txid};
use lez_btc_swap_sdk::{
    AdaptorSessionContext, BTC_AGREEMENT_SCHEMA_V1, BTC_LEZ_ASSET_EXTENSION_SCHEMA_V1,
    BtcAdaptorSessionDomain, BtcAgreementBodyV1, BtcAgreementRecordV1, BtcAgreementV1,
    BtcAgreementV1Error, BtcChainPolicyV1, BtcClaimTermsV1, BtcFundingTermsV1,
    BtcLezAssetExtensionBodyV1, BtcLezAssetExtensionRecordV1, BtcLezAssetExtensionV1,
    BtcLezAssetExtensionV1Error, BtcLezAssetV1, BtcLezCustomTokenTermsV1, BtcLezTermsV1,
    BtcP2trTermsV1, BtcParticipantIdentityV1, BtcParticipantsV1, BtcRecoveryPlanV1, CsvBlockDelay,
    MAX_BITCOIN_REQUIRED_CONFIRMATIONS, MAX_BTC_AGREEMENT_RECORD_BYTES, P2trSwapOutput,
    RefundXOnlyKey, TwoPartyAggregateKey,
};
use lez_swap_core::{Chain, MakerRecoveryTrigger, Pair, Participant, Phase, SwapDirection};

const FUNDING_VALUE_SAT: u64 = 100_000;
const CLAIM_VALUE_SAT: u64 = 99_000;
const REFUND_CSV_BLOCKS: u32 = 144;
const BITCOIN_FUNDING_ANCHOR_HEIGHT: u32 = 1_000;
const BITCOIN_REFUND_HEIGHT: u32 = 1_144;
const MAKER_SECOND_LOCK_CUTOFF_SECONDS: u64 = 1_699_999_800;
const EARLIER_REFUND_LATEST_SECONDS: u64 = 1_700_000_100;
const LATER_REFUND_EARLIEST_SECONDS: u64 = 1_700_000_500;
const REQUIRED_MARGIN_SECONDS: u64 = 300;
const BITCOIN_GENESIS_HASH: [u8; 32] = [8; 32];
const REQUIRED_CONFIRMATIONS: u32 = 6;
const FIRST_DESTINATION_LENGTH_OFFSET: usize = 2 + 32 + 1 + 32 + 4 + 32 + 33 + 32;

// Independent corruption toggles keep every signed-field mutation explicit in
// the canonical fixture; combining them into a mode enum would hide mixtures.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy)]
struct FixtureOptions {
    refund_role: Option<Participant>,
    drift_contract_script: bool,
    drift_claim_sighash: bool,
    wrong_refund_height: bool,
    unsafe_maker_second_lock_cutoff: bool,
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
            unsafe_maker_second_lock_cutoff: false,
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
        if options.unsafe_maker_second_lock_cutoff {
            MAKER_SECOND_LOCK_CUTOFF_SECONDS + 1
        } else {
            MAKER_SECOND_LOCK_CUTOFF_SECONDS
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

#[derive(Clone, Copy)]
struct TokenFixtureOptions {
    token_program_id: [u8; 32],
    ata_program_id: [u8; 32],
    token_definition_account: [u8; 32],
    depositor_owner_account: Option<[u8; 32]>,
    depositor_ata_account: [u8; 32],
    claimant_owner_account: Option<[u8; 32]>,
    claimant_ata_account: [u8; 32],
    custody_ata_account: [u8; 32],
    amount: Option<u128>,
    refund_at_ms: Option<u64>,
    aggregate_authority_account: Option<[u8; 32]>,
    aggregate_x_only_public_key: Option<[u8; 32]>,
}

impl Default for TokenFixtureOptions {
    fn default() -> Self {
        Self {
            token_program_id: [40; 32],
            ata_program_id: [41; 32],
            token_definition_account: [42; 32],
            depositor_owner_account: None,
            depositor_ata_account: [43; 32],
            claimant_owner_account: None,
            claimant_ata_account: [44; 32],
            custody_ata_account: [45; 32],
            amount: None,
            refund_at_ms: None,
            aggregate_authority_account: None,
            aggregate_x_only_public_key: None,
        }
    }
}

fn custom_token_terms_with(
    agreement: &BtcAgreementV1,
    options: &TokenFixtureOptions,
) -> BtcLezCustomTokenTermsV1 {
    BtcLezCustomTokenTermsV1::new(
        options.token_program_id,
        options.ata_program_id,
        options.token_definition_account,
        options
            .depositor_owner_account
            .unwrap_or(*agreement.lez_terms().depositor_account()),
        options.depositor_ata_account,
        options
            .claimant_owner_account
            .unwrap_or(*agreement.lez_terms().claimant_account()),
        options.claimant_ata_account,
        options.custody_ata_account,
        options.amount.unwrap_or(agreement.lez_terms().amount()),
        options
            .refund_at_ms
            .unwrap_or(agreement.lez_terms().refund_at_ms()),
        options
            .aggregate_authority_account
            .unwrap_or(*agreement.lez_terms().aggregate_authority_account()),
        options
            .aggregate_x_only_public_key
            .unwrap_or(agreement.p2tr_contract().aggregate_internal_key_bytes()),
    )
}

fn custom_token_terms(agreement: &BtcAgreementV1) -> BtcLezCustomTokenTermsV1 {
    custom_token_terms_with(agreement, &TokenFixtureOptions::default())
}

fn custom_token_asset(terms: &BtcLezCustomTokenTermsV1) -> BtcLezAssetV1 {
    BtcLezAssetV1::CustomToken(Box::new(*terms))
}

fn signed_asset_extension(
    fixture: &Fixture,
    agreement: &BtcAgreementV1,
    asset: BtcLezAssetV1,
) -> BtcLezAssetExtensionRecordV1 {
    let body = BtcLezAssetExtensionBodyV1::new(*agreement.agreement_commitment(), asset);
    let commitment = body.commitment();
    BtcLezAssetExtensionRecordV1::from_parts(
        BTC_LEZ_ASSET_EXTENSION_SCHEMA_V1,
        body,
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
    assert_eq!(
        recovery.maker_second_lock_cutoff_unix_seconds(),
        MAKER_SECOND_LOCK_CUTOFF_SECONDS
    );
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
fn countersigned_agreement_derives_exact_funder_refund_in_both_directions() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        let fixture = fixture(direction, FixtureOptions::default());
        let agreement =
            BtcAgreementV1::validate(signed_record(&fixture)).expect("validated agreement");
        let refund = agreement.bitcoin_refund();
        let [output] = refund.unsigned_transaction().output.as_slice() else {
            panic!("refund must have exactly one output");
        };

        assert_eq!(
            refund.funding_outpoint(),
            agreement.cooperative_claim().funding_outpoint()
        );
        assert_eq!(
            refund.funding_prevout(),
            agreement.cooperative_claim().funding_prevout()
        );
        assert_eq!(
            refund.unsigned_transaction().input[0]
                .sequence
                .to_consensus_u32(),
            agreement.p2tr_contract().refund_sequence()
        );
        assert_eq!(refund.fee(), agreement.cooperative_claim().fee());
        assert_eq!(output.value, Amount::from_sat(CLAIM_VALUE_SAT));
        assert_eq!(
            output.script_pubkey.as_bytes(),
            agreement
                .participant(agreement.bitcoin_funder())
                .claim_destination_script_pubkey()
        );
        assert_ne!(
            refund.sighash_bytes(),
            agreement.cooperative_claim().sighash_bytes()
        );
    }
}

#[test]
fn validated_agreement_derives_both_fresh_adaptor_session_contexts() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        let fixture = fixture(direction, FixtureOptions::default());
        let agreement = BtcAgreementV1::from_wire(
            &signed_record(&fixture)
                .encode_wire()
                .expect("agreement wire"),
        )
        .expect("validated agreement");
        let ordered_keys = [
            *agreement
                .participant(Participant::Maker)
                .musig2_public_key(),
            *agreement
                .participant(Participant::Taker)
                .musig2_public_key(),
        ];

        let bitcoin_session_id = [0xb1; 32];
        let bitcoin = agreement
            .adaptor_session_context(BtcAdaptorSessionDomain::Bitcoin, bitcoin_session_id)
            .expect("agreement-derived Bitcoin context");
        let expected_bitcoin = AdaptorSessionContext::taproot(
            ordered_keys,
            agreement.p2tr_contract().merkle_root_bytes(),
            agreement.cooperative_claim().sighash_bytes(),
            *agreement.adaptor_point(),
            bitcoin_session_id,
        )
        .expect("manual Bitcoin context");
        assert_eq!(
            bitcoin.durable_context_binding(),
            expected_bitcoin.durable_context_binding()
        );

        let lez_session_id = [0xc2; 32];
        let lez = agreement
            .adaptor_session_context(BtcAdaptorSessionDomain::Lez, lez_session_id)
            .expect("agreement-derived LEZ context");
        let expected_lez = AdaptorSessionContext::untweaked(
            ordered_keys,
            *agreement.lez_terms().claim_message_hash(),
            *agreement.adaptor_point(),
            lez_session_id,
        )
        .expect("manual LEZ context");
        assert_eq!(
            lez.durable_context_binding(),
            expected_lez.durable_context_binding()
        );
        assert_ne!(
            bitcoin.durable_context_binding(),
            lez.durable_context_binding(),
            "chain sessions must remain domain-separated"
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

    let unsafe_cutoff = fixture(
        SwapDirection::TakerSellsLez,
        FixtureOptions {
            unsafe_maker_second_lock_cutoff: true,
            ..FixtureOptions::default()
        },
    );
    assert_eq!(
        BtcAgreementV1::validate(signed_record(&unsafe_cutoff)),
        Err(BtcAgreementV1Error::RecoveryScheduleMismatch)
    );

    let valid = fixture(SwapDirection::TakerSellsLez, FixtureOptions::default());
    let agreement = BtcAgreementV1::validate(signed_record(&valid)).expect("valid schedule");
    assert_eq!(agreement.lez_depositor(), Participant::Taker);
    assert_eq!(agreement.bitcoin_funder(), Participant::Maker);
}

#[test]
fn additive_asset_extension_is_explicit_countersigned_and_leaves_v1_bytes_unchanged() {
    let fixture = fixture(SwapDirection::TakerSellsForeign, FixtureOptions::default());
    let legacy_record = signed_record(&fixture);
    let legacy_wire = legacy_record.encode_wire().expect("legacy wire");
    let legacy_commitment = fixture.body.commitment();
    let agreement = BtcAgreementV1::validate(legacy_record).expect("agreement");

    let native_record = signed_asset_extension(&fixture, &agreement, BtcLezAssetV1::Native);
    let native_wire = native_record.encode_wire().expect("native extension wire");
    let native =
        BtcLezAssetExtensionV1::from_wire(&native_wire, &agreement).expect("native binding");
    assert_eq!(native.encode_wire().expect("wire replay"), native_wire);
    assert_eq!(
        native.base_agreement_commitment(),
        agreement.agreement_commitment()
    );
    assert_eq!(native.asset(), &BtcLezAssetV1::Native);
    assert_eq!(native.lez_terms_binding(), *native.asset_commitment());

    let custom_asset = custom_token_asset(&custom_token_terms(&agreement));
    let custom_record = signed_asset_extension(&fixture, &agreement, custom_asset.clone());
    let custom_wire = custom_record.encode_wire().expect("custom extension wire");
    let custom =
        BtcLezAssetExtensionV1::from_wire(&custom_wire, &agreement).expect("custom binding");
    assert_eq!(custom.encode_wire().expect("wire replay"), custom_wire);
    assert_eq!(custom.asset(), &custom_asset);
    assert_ne!(custom.asset_commitment(), native.asset_commitment());
    assert_ne!(custom_wire, native_wire);
    custom
        .ensure_asset(&custom_asset)
        .expect("exact locally expected asset");

    let BtcLezAssetV1::CustomToken(token) = custom.asset() else {
        panic!("custom token asset");
    };
    assert_eq!(token.token_program_id(), &[40; 32]);
    assert_eq!(token.ata_program_id(), &[41; 32]);
    assert_eq!(token.token_definition_account(), &[42; 32]);
    assert_eq!(
        token.depositor_owner_account(),
        agreement.lez_terms().depositor_account()
    );
    assert_eq!(token.depositor_ata_account(), &[43; 32]);
    assert_eq!(
        token.claimant_owner_account(),
        agreement.lez_terms().claimant_account()
    );
    assert_eq!(token.claimant_ata_account(), &[44; 32]);
    assert_eq!(token.custody_ata_account(), &[45; 32]);
    assert_eq!(token.amount(), agreement.lez_terms().amount());
    assert_eq!(token.refund_at_ms(), agreement.lez_terms().refund_at_ms());
    assert_eq!(
        token.aggregate_authority_account(),
        agreement.lez_terms().aggregate_authority_account()
    );
    assert_eq!(
        token.aggregate_x_only_public_key(),
        &agreement.p2tr_contract().aggregate_internal_key_bytes()
    );

    assert_eq!(
        signed_record(&fixture)
            .encode_wire()
            .expect("legacy replay"),
        legacy_wire
    );
    assert_eq!(fixture.body.commitment(), legacy_commitment);
}

#[test]
fn asset_extension_rejects_cross_agreement_network_role_and_base_term_substitution() {
    let base_fixture = fixture(SwapDirection::TakerSellsForeign, FixtureOptions::default());
    let agreement =
        BtcAgreementV1::validate(signed_record(&base_fixture)).expect("validated agreement");
    let valid_record = signed_asset_extension(
        &base_fixture,
        &agreement,
        custom_token_asset(&custom_token_terms(&agreement)),
    );

    let other_fixture = fixture(
        SwapDirection::TakerSellsForeign,
        FixtureOptions {
            bitcoin_genesis_hash: [9; 32],
            ..FixtureOptions::default()
        },
    );
    let other_agreement =
        BtcAgreementV1::validate(signed_record(&other_fixture)).expect("other network agreement");
    assert_eq!(
        BtcLezAssetExtensionV1::validate(valid_record.clone(), &other_agreement),
        Err(BtcLezAssetExtensionV1Error::BaseAgreementMismatch)
    );

    for options in [
        TokenFixtureOptions {
            depositor_owner_account: Some(*agreement.lez_terms().claimant_account()),
            ..TokenFixtureOptions::default()
        },
        TokenFixtureOptions {
            claimant_owner_account: Some(*agreement.lez_terms().depositor_account()),
            ..TokenFixtureOptions::default()
        },
    ] {
        let record = signed_asset_extension(
            &base_fixture,
            &agreement,
            custom_token_asset(&custom_token_terms_with(&agreement, &options)),
        );
        assert_eq!(
            BtcLezAssetExtensionV1::validate(record, &agreement),
            Err(BtcLezAssetExtensionV1Error::LezAssetRoleMismatch)
        );
    }

    for options in [
        TokenFixtureOptions {
            amount: Some(agreement.lez_terms().amount() + 1),
            ..TokenFixtureOptions::default()
        },
        TokenFixtureOptions {
            refund_at_ms: Some(agreement.lez_terms().refund_at_ms() + 1_000),
            ..TokenFixtureOptions::default()
        },
        TokenFixtureOptions {
            aggregate_authority_account: Some([51; 32]),
            ..TokenFixtureOptions::default()
        },
    ] {
        let record = signed_asset_extension(
            &base_fixture,
            &agreement,
            custom_token_asset(&custom_token_terms_with(&agreement, &options)),
        );
        assert_eq!(
            BtcLezAssetExtensionV1::validate(record, &agreement),
            Err(BtcLezAssetExtensionV1Error::LezBaseTermsMismatch)
        );
    }

    let wrong_authority_key = signed_asset_extension(
        &base_fixture,
        &agreement,
        custom_token_asset(&custom_token_terms_with(
            &agreement,
            &TokenFixtureOptions {
                aggregate_x_only_public_key: Some(x_only_public_key(&secret(9))),
                ..TokenFixtureOptions::default()
            },
        )),
    );
    assert_eq!(
        BtcLezAssetExtensionV1::validate(wrong_authority_key, &agreement),
        Err(BtcLezAssetExtensionV1Error::AggregateAuthorityMismatch)
    );
}

#[test]
fn every_asset_field_is_covered_by_the_commitment() {
    let fixture = fixture(SwapDirection::TakerSellsLez, FixtureOptions::default());
    let agreement = BtcAgreementV1::validate(signed_record(&fixture)).expect("validated agreement");
    let expected = custom_token_asset(&custom_token_terms(&agreement));
    let signed = signed_asset_extension(&fixture, &agreement, expected.clone());

    let substitutions = [
        BtcLezAssetV1::Native,
        custom_token_asset(&custom_token_terms_with(
            &agreement,
            &TokenFixtureOptions {
                token_program_id: [52; 32],
                ..TokenFixtureOptions::default()
            },
        )),
        custom_token_asset(&custom_token_terms_with(
            &agreement,
            &TokenFixtureOptions {
                ata_program_id: [53; 32],
                ..TokenFixtureOptions::default()
            },
        )),
        custom_token_asset(&custom_token_terms_with(
            &agreement,
            &TokenFixtureOptions {
                token_definition_account: [54; 32],
                ..TokenFixtureOptions::default()
            },
        )),
        custom_token_asset(&custom_token_terms_with(
            &agreement,
            &TokenFixtureOptions {
                depositor_ata_account: [55; 32],
                ..TokenFixtureOptions::default()
            },
        )),
        custom_token_asset(&custom_token_terms_with(
            &agreement,
            &TokenFixtureOptions {
                claimant_ata_account: [56; 32],
                ..TokenFixtureOptions::default()
            },
        )),
        custom_token_asset(&custom_token_terms_with(
            &agreement,
            &TokenFixtureOptions {
                custody_ata_account: [60; 32],
                ..TokenFixtureOptions::default()
            },
        )),
    ];

    for substituted_asset in substitutions {
        let substituted_body =
            BtcLezAssetExtensionBodyV1::new(*agreement.agreement_commitment(), substituted_asset);
        let substituted_record = BtcLezAssetExtensionRecordV1::from_parts(
            BTC_LEZ_ASSET_EXTENSION_SCHEMA_V1,
            substituted_body,
            *signed.asset_commitment(),
            *signed.signature(Participant::Maker),
            *signed.signature(Participant::Taker),
        );
        assert_eq!(
            BtcLezAssetExtensionV1::validate(substituted_record, &agreement),
            Err(BtcLezAssetExtensionV1Error::AssetCommitmentMismatch)
        );
    }
}

#[test]
fn exact_asset_policy_rejects_kind_program_definition_and_custody_substitutions() {
    let fixture = fixture(SwapDirection::TakerSellsLez, FixtureOptions::default());
    let agreement = BtcAgreementV1::validate(signed_record(&fixture)).expect("validated agreement");
    let expected = custom_token_asset(&custom_token_terms(&agreement));
    let signed = signed_asset_extension(&fixture, &agreement, expected);

    let valid = BtcLezAssetExtensionV1::validate(signed, &agreement).expect("valid extension");
    for unexpected in [
        BtcLezAssetV1::Native,
        custom_token_asset(&custom_token_terms_with(
            &agreement,
            &TokenFixtureOptions {
                token_program_id: [57; 32],
                ..TokenFixtureOptions::default()
            },
        )),
        custom_token_asset(&custom_token_terms_with(
            &agreement,
            &TokenFixtureOptions {
                ata_program_id: [58; 32],
                ..TokenFixtureOptions::default()
            },
        )),
        custom_token_asset(&custom_token_terms_with(
            &agreement,
            &TokenFixtureOptions {
                token_definition_account: [59; 32],
                ..TokenFixtureOptions::default()
            },
        )),
        custom_token_asset(&custom_token_terms_with(
            &agreement,
            &TokenFixtureOptions {
                custody_ata_account: [61; 32],
                ..TokenFixtureOptions::default()
            },
        )),
    ] {
        assert_eq!(
            valid.ensure_asset(&unexpected),
            Err(BtcLezAssetExtensionV1Error::LezAssetMismatch)
        );
    }
}

#[test]
fn asset_extension_rejects_aliases_invalid_authority_schema_signature_and_wire() {
    let fixture = fixture(SwapDirection::TakerSellsForeign, FixtureOptions::default());
    let agreement = BtcAgreementV1::validate(signed_record(&fixture)).expect("validated agreement");

    for options in [
        TokenFixtureOptions {
            ata_program_id: [40; 32],
            ..TokenFixtureOptions::default()
        },
        TokenFixtureOptions {
            claimant_ata_account: [43; 32],
            ..TokenFixtureOptions::default()
        },
        TokenFixtureOptions {
            token_definition_account: *agreement.lez_terms().metadata_account(),
            ..TokenFixtureOptions::default()
        },
        TokenFixtureOptions {
            custody_ata_account: [43; 32],
            ..TokenFixtureOptions::default()
        },
    ] {
        let aliased = signed_asset_extension(
            &fixture,
            &agreement,
            custom_token_asset(&custom_token_terms_with(&agreement, &options)),
        );
        assert_eq!(
            BtcLezAssetExtensionV1::validate(aliased, &agreement),
            Err(BtcLezAssetExtensionV1Error::LezAssetAlias)
        );
    }

    let invalid_key = signed_asset_extension(
        &fixture,
        &agreement,
        custom_token_asset(&custom_token_terms_with(
            &agreement,
            &TokenFixtureOptions {
                aggregate_x_only_public_key: Some([0; 32]),
                ..TokenFixtureOptions::default()
            },
        )),
    );
    assert_eq!(
        BtcLezAssetExtensionV1::validate(invalid_key, &agreement),
        Err(BtcLezAssetExtensionV1Error::InvalidAggregateAuthorityKey)
    );

    let valid = signed_asset_extension(&fixture, &agreement, BtcLezAssetV1::Native);
    let unsupported = BtcLezAssetExtensionRecordV1::from_parts(
        BTC_LEZ_ASSET_EXTENSION_SCHEMA_V1 + 1,
        valid.body().clone(),
        *valid.asset_commitment(),
        *valid.signature(Participant::Maker),
        *valid.signature(Participant::Taker),
    );
    assert_eq!(
        BtcLezAssetExtensionV1::validate(unsupported, &agreement),
        Err(BtcLezAssetExtensionV1Error::UnsupportedSchema(
            BTC_LEZ_ASSET_EXTENSION_SCHEMA_V1 + 1
        ))
    );

    let swapped = BtcLezAssetExtensionRecordV1::from_parts(
        BTC_LEZ_ASSET_EXTENSION_SCHEMA_V1,
        valid.body().clone(),
        *valid.asset_commitment(),
        *valid.signature(Participant::Taker),
        *valid.signature(Participant::Maker),
    );
    assert_eq!(
        BtcLezAssetExtensionV1::validate(swapped, &agreement),
        Err(BtcLezAssetExtensionV1Error::SignatureMismatch(
            Participant::Maker
        ))
    );

    let mut trailing = valid.encode_wire().expect("extension wire");
    trailing.push(0);
    assert_eq!(
        BtcLezAssetExtensionV1::from_wire(&trailing, &agreement),
        Err(BtcLezAssetExtensionV1Error::MalformedWireRecord)
    );
}
