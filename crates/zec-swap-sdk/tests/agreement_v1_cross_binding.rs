use lez_swap_core::{Chain, Participant, Phase, SwapDirection, UnixSeconds};
use lez_zec_swap_sdk::{
    AcceptedZecAgreementV1, Bip199Contract, ExpectedBip199Output, LezAssetV1, LezChainIdentityV1,
    LezEnvironmentV1, MAX_ZEC_AGREEMENT_RECORD_BYTES, MAX_ZEC_APPLICATION_SWAP_ID_BYTES,
    MAX_ZEC_FUNDING_INPUTS, MAX_ZEC_FUNDING_SCRIPT_BYTES, NegotiationTranscriptV1, TransparentUtxo,
    ZEC_CONCRETE_AGREEMENT_SCHEMA_V1, ZEC_CONCRETE_AGREEMENT_SCHEMA_V2, ZcashFundingInputSetV1,
    ZcashFundingInputV1, ZcashTransparentDestinationV1, ZecAgreementBodyV1,
    ZecAgreementExecutionError, ZecAgreementRecordV1, ZecAgreementV1, ZecAgreementV1Error,
    ZecLezTermsV1, ZecParticipantIdentityV1, ZecParticipantsV1, ZecProfileId, ZecProfileRecordV1,
    ZecRefundPlanV1, ZecRolePayoutV1, ZecSwapBinding, ZecSwapBindingRecordV1,
    ZecTransactionPolicyV1, derive_lez_metadata_account_v1, derive_lez_native_custody_account_v1,
    derive_lez_swap_id_v1, derive_lez_token_account_v1, derive_nssa_v0_1_2_metadata_account_v1,
    derive_nssa_v0_1_2_native_custody_account_v1,
};
use secp256k1::{Message, PublicKey, Secp256k1, SecretKey};
use zcash_protocol::{
    consensus::{BlockHeight, BranchId, NetworkType},
    value::Zatoshis,
};
use zcash_script::script::Code;
use zcash_transparent::{
    address::{Script, TransparentAddress},
    bundle::{OutPoint, TxOut},
};

const NOW: u64 = 10;

#[derive(Clone)]
struct Fixture {
    application_swap_id: String,
    direction: SwapDirection,
    body_profile: ZecProfileId,
    binding_profile: ZecProfileId,
    environment: LezEnvironmentV1,
    channel_id: [u8; 32],
    genesis: [u8; 32],
    escrow_program: [u32; 8],
    asset: LezAssetV1,
    lez_amount: u128,
    metadata_account: [u8; 32],
    custody_account: [u8; 32],
    maker_lez: [u8; 32],
    taker_lez: [u8; 32],
    maker_key: [u8; 33],
    taker_key: [u8; 33],
    body_digest: [u8; 32],
    contract_digest: [u8; 32],
    contract_refund_hash: [u8; 20],
    contract_claimant_hash: [u8; 20],
    zcash_value: u64,
    zcash_refund_lock: u32,
    lez_anchor: u64,
    zcash_anchor: u32,
    earlier_latest_ms: u64,
    later_earliest: u64,
    funding_input_set_commitment: [u8; 32],
    funding_change_hash: [u8; 20],
    funding_fee: u64,
    minimum_change: u64,
    claim_destination_hash: [u8; 20],
    claim_fee: u64,
    refund_destination_hash: [u8; 20],
    refund_fee: u64,
    expiry_delta: u32,
    session_id: [u8; 32],
    offer_commitment: [u8; 32],
    expires_at: u64,
    schema: u16,
    commitment_override: Option<[u8; 32]>,
    maker_signer: SecretKey,
    taker_signer: SecretKey,
    high_s_maker: bool,
}

impl Fixture {
    fn new(direction: SwapDirection) -> Self {
        let maker_signer = key(1);
        let taker_signer = key(2);
        let maker_key = public_key(&maker_signer).serialize();
        let taker_key = public_key(&taker_signer).serialize();
        let (refund_key, claimant_key) = match direction {
            SwapDirection::TakerSellsForeign => (taker_key, maker_key),
            SwapDirection::TakerSellsLez => (maker_key, taker_key),
        };
        let application_swap_id = "agreement-v1".to_owned();
        let escrow_program = [1; 8];
        let onchain_swap_id = derive_lez_swap_id_v1(application_swap_id.as_bytes());
        let metadata_account = derive_lez_metadata_account_v1(&escrow_program, &onchain_swap_id);
        let custody_account =
            derive_lez_native_custody_account_v1(&escrow_program, &onchain_swap_id);
        Self {
            application_swap_id,
            direction,
            body_profile: ZecProfileId::DeterministicLocalV1,
            binding_profile: ZecProfileId::DeterministicLocalV1,
            environment: LezEnvironmentV1::DeterministicLocalV0_2,
            channel_id: [8; 32],
            genesis: [7; 32],
            escrow_program,
            asset: LezAssetV1::Native {
                authenticated_transfer_program_id: [2; 8],
            },
            lez_amount: 42,
            metadata_account,
            custody_account,
            maker_lez: [3; 32],
            taker_lez: [4; 32],
            maker_key,
            taker_key,
            body_digest: [9; 32],
            contract_digest: [9; 32],
            contract_refund_hash: pubkey_hash(&refund_key),
            contract_claimant_hash: pubkey_hash(&claimant_key),
            zcash_value: 100_000_000,
            zcash_refund_lock: 120,
            lez_anchor: 100,
            zcash_anchor: 116,
            earlier_latest_ms: 160_000,
            later_earliest: 200,
            funding_input_set_commitment: [12; 32],
            funding_change_hash: pubkey_hash(&refund_key),
            funding_fee: 10_000,
            minimum_change: 1_000,
            claim_destination_hash: pubkey_hash(&claimant_key),
            claim_fee: 10_000,
            refund_destination_hash: pubkey_hash(&refund_key),
            refund_fee: 10_000,
            expiry_delta: 40,
            session_id: [5; 32],
            offer_commitment: [6; 32],
            expires_at: 1_000,
            schema: ZEC_CONCRETE_AGREEMENT_SCHEMA_V2,
            commitment_override: None,
            maker_signer,
            taker_signer,
            high_s_maker: false,
        }
    }

    fn public(direction: SwapDirection) -> Self {
        let mut fixture = Self::new(direction);
        fixture.body_profile = ZecProfileId::PublicTestnetV1;
        fixture.binding_profile = ZecProfileId::PublicTestnetV1;
        fixture.environment = LezEnvironmentV1::PublicTestnetV0_2;
        fixture.zcash_anchor = 100;
        fixture.zcash_refund_lock = 292;
        fixture.earlier_latest_ms = 7_300_000;
        fixture.later_earliest = 14_500;
        fixture
    }

    fn use_nssa_v0_1_2_compatibility(&mut self) {
        let onchain_swap_id = derive_lez_swap_id_v1(self.application_swap_id.as_bytes());
        self.environment = LezEnvironmentV1::DeterministicLocalV0_1_2Compatibility;
        self.metadata_account =
            derive_nssa_v0_1_2_metadata_account_v1(&self.escrow_program, &onchain_swap_id);
        self.custody_account =
            derive_nssa_v0_1_2_native_custody_account_v1(&self.escrow_program, &onchain_swap_id);
    }

    fn use_token_asset(
        &mut self,
        definition_account: [u8; 32],
        token_program_id: [u32; 8],
        ata_program_id: [u32; 8],
    ) {
        let (depositor, claimant) = match self.direction {
            SwapDirection::TakerSellsForeign => (self.maker_lez, self.taker_lez),
            SwapDirection::TakerSellsLez => (self.taker_lez, self.maker_lez),
        };
        self.custody_account = derive_lez_token_account_v1(
            &ata_program_id,
            &self.metadata_account,
            &definition_account,
        );
        self.asset = LezAssetV1::FungibleToken {
            definition_account,
            token_program_id,
            ata_program_id,
            depositor_ata: derive_lez_token_account_v1(
                &ata_program_id,
                &depositor,
                &definition_account,
            ),
            claimant_ata: derive_lez_token_account_v1(
                &ata_program_id,
                &claimant,
                &definition_account,
            ),
        };
    }

    fn body(&self) -> ZecAgreementBodyV1 {
        let contract = Bip199Contract::new(
            self.zcash_refund_lock,
            self.contract_refund_hash,
            self.contract_digest,
            self.contract_claimant_hash,
        );
        let (network, branch) = match self.binding_profile {
            ZecProfileId::DeterministicLocalV1 => (NetworkType::Regtest, BranchId::Nu6_2),
            ZecProfileId::PublicTestnetV1 => (NetworkType::Test, BranchId::Nu6_2),
        };
        let output = ExpectedBip199Output::new(
            network,
            branch,
            Zatoshis::from_u64(self.zcash_value).expect("test value is in range"),
            contract,
        );
        let binding = ZecSwapBinding::new(self.binding_profile, output).expect("fixture binding");
        ZecAgreementBodyV1::new(
            self.application_swap_id.clone(),
            self.direction,
            ZecProfileRecordV1::from(self.body_profile),
            ZecParticipantsV1::new(
                ZecParticipantIdentityV1::new(self.maker_lez, self.maker_key),
                ZecParticipantIdentityV1::new(self.taker_lez, self.taker_key),
            ),
            self.body_digest,
            ZecLezTermsV1::new(
                LezChainIdentityV1::new(self.environment, self.channel_id, self.genesis),
                self.escrow_program,
                self.asset.clone(),
                self.lez_amount,
                self.metadata_account,
                self.custody_account,
            ),
            ZecSwapBindingRecordV1::from_binding(&binding),
            ZecTransactionPolicyV1::new(
                self.funding_input_set_commitment,
                ZcashTransparentDestinationV1::p2pkh(self.funding_change_hash),
                self.funding_fee,
                self.minimum_change,
                ZcashTransparentDestinationV1::p2pkh(self.claim_destination_hash),
                self.claim_fee,
                ZcashTransparentDestinationV1::p2pkh(self.refund_destination_hash),
                self.refund_fee,
                self.expiry_delta,
            ),
            ZecRefundPlanV1::new(
                self.lez_anchor,
                self.zcash_anchor,
                self.earlier_latest_ms,
                self.later_earliest,
            ),
            NegotiationTranscriptV1::new(self.session_id, self.offer_commitment, self.expires_at),
        )
    }

    fn record(&self) -> ZecAgreementRecordV1 {
        let body = self.body();
        let commitment = body.commitment();
        let mut maker_signature = sign(commitment, &self.maker_signer);
        if self.high_s_maker {
            maker_signature = to_high_s(maker_signature);
        }
        ZecAgreementRecordV1::from_parts(
            self.schema,
            body,
            self.commitment_override.unwrap_or(commitment),
            maker_signature,
            sign(commitment, &self.taker_signer),
        )
    }

    fn validate(&self) -> Result<ZecAgreementV1, ZecAgreementV1Error> {
        ZecAgreementV1::validate_at(self.record(), UnixSeconds::new(NOW))
    }
}

#[test]
fn bounded_wire_rejects_malformed_trailing_oversized_and_declared_id() {
    let agreement = Fixture::new(SwapDirection::TakerSellsForeign)
        .validate()
        .expect("agreement");
    let wire = agreement.encode_wire().expect("bounded wire");
    let decoded =
        ZecAgreementV1::from_wire_at(&wire, UnixSeconds::new(NOW)).expect("exact wire validates");
    assert_eq!(decoded.record(), agreement.record());

    let mut legacy_v1 = wire.clone();
    legacy_v1[..2].copy_from_slice(&ZEC_CONCRETE_AGREEMENT_SCHEMA_V1.to_le_bytes());
    let application_id_len = "agreement-v1".len();
    let channel_offset = 2 + 4 + application_id_len + 1 + 1 + (32 + 33) * 2 + 32 + 1;
    legacy_v1.drain(channel_offset..channel_offset + 32);
    assert_eq!(
        ZecAgreementV1::from_wire_at(&legacy_v1, UnixSeconds::new(NOW)),
        Err(ZecAgreementV1Error::UnsupportedSchema(
            ZEC_CONCRETE_AGREEMENT_SCHEMA_V1
        )),
        "a signed schema-v1 agreement cannot acquire an unsigned channel during migration"
    );

    let mut trailing = wire.clone();
    trailing.push(0);
    assert_eq!(
        ZecAgreementV1::from_wire_at(&trailing, UnixSeconds::new(NOW)),
        Err(ZecAgreementV1Error::MalformedWireRecord)
    );
    assert_eq!(
        ZecAgreementV1::from_wire_at(&wire[..5], UnixSeconds::new(NOW)),
        Err(ZecAgreementV1Error::MalformedWireRecord)
    );
    let oversized = vec![0_u8; MAX_ZEC_AGREEMENT_RECORD_BYTES + 1];
    assert_eq!(
        ZecAgreementV1::from_wire_at(&oversized, UnixSeconds::new(NOW)),
        Err(ZecAgreementV1Error::OversizedWireRecord {
            actual: MAX_ZEC_AGREEMENT_RECORD_BYTES + 1,
            maximum: MAX_ZEC_AGREEMENT_RECORD_BYTES,
        })
    );
    let mut declared = wire;
    let excessive = u32::try_from(MAX_ZEC_APPLICATION_SWAP_ID_BYTES + 1).expect("small limit");
    declared[2..6].copy_from_slice(&excessive.to_le_bytes());
    assert_eq!(
        ZecAgreementV1::from_wire_at(&declared, UnixSeconds::new(NOW)),
        Err(ZecAgreementV1Error::ApplicationIdTooLong)
    );

    for dynamic in [
        agreement
            .binding()
            .expected_output()
            .contract()
            .redeem_script(),
        agreement
            .binding()
            .expected_output()
            .contract()
            .p2sh_script_pubkey(),
    ] {
        let mut malicious = agreement.encode_wire().expect("wire");
        let data_offset = malicious
            .windows(dynamic.len())
            .position(|window| window == dynamic)
            .expect("derived script occurs in binding record");
        let length_offset = data_offset.checked_sub(4).expect("Borsh Vec prefix");
        malicious[length_offset..data_offset].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(
            ZecAgreementV1::from_wire_at(&malicious, UnixSeconds::new(NOW)),
            Err(ZecAgreementV1Error::MalformedWireRecord)
        );
    }

    let mut overlong = Fixture::new(SwapDirection::TakerSellsForeign);
    overlong.application_swap_id = "x".repeat(MAX_ZEC_APPLICATION_SWAP_ID_BYTES + 1);
    assert_eq!(
        overlong.validate(),
        Err(ZecAgreementV1Error::ApplicationIdTooLong)
    );
}

#[test]
fn both_directions_derive_roles_deadlines_amounts_and_fresh_coordinator() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        let fixture = Fixture::new(direction);
        let agreement = fixture.validate().expect("valid countersigned agreement");
        let (depositor, claimant) = match direction {
            SwapDirection::TakerSellsForeign => (Participant::Maker, Participant::Taker),
            SwapDirection::TakerSellsLez => (Participant::Taker, Participant::Maker),
        };
        assert_eq!(agreement.coordinator().phase(), Phase::Offered);
        assert_eq!(agreement.coordinator().direction(), direction);
        assert_eq!(agreement.coordinator().funded_chain(depositor), Chain::Lez);
        assert_eq!(agreement.coordinator().funded_chain(claimant), Chain::Zcash);
        assert_eq!(agreement.lez_depositor(), depositor);
        assert_eq!(agreement.lez_claimant(), claimant);
        assert_eq!(
            agreement.lez_account(depositor),
            fixture_account(&fixture, depositor)
        );
        assert_eq!(
            agreement.lez_account(claimant),
            fixture_account(&fixture, claimant)
        );
        assert_eq!(agreement.lez_refund_at_ms(), 160_000);
        assert_eq!(agreement.zcash_refund_at_height(), 120);
        assert_eq!(
            agreement.binding().expected_output().value(),
            Zatoshis::from_u64(fixture.zcash_value).expect("value")
        );
        assert_eq!(
            agreement.agreement_commitment(),
            &agreement.record().body().commitment()
        );
    }
}

#[test]
fn public_profile_accepts_exact_signed_deployment_identity_in_both_directions() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        let fixture = Fixture::public(direction);
        let agreement = fixture
            .validate()
            .expect("public profile accepts exact signed deployment terms");
        assert_eq!(
            agreement.binding().profile_id(),
            ZecProfileId::PublicTestnetV1
        );
        assert_eq!(
            agreement.lez_terms().chain().environment(),
            LezEnvironmentV1::PublicTestnetV0_2
        );
        assert_eq!(
            agreement.lez_terms().chain().channel_id(),
            &fixture.channel_id
        );
        assert_eq!(
            agreement.lez_terms().chain().genesis_block_hash(),
            &fixture.genesis
        );
        assert_eq!(
            agreement.lez_terms().escrow_program_id(),
            &fixture.escrow_program
        );
    }
}

#[test]
fn earlier_latest_must_not_precede_actual_lez_deadline_in_either_direction() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        let mut fixture = Fixture::new(direction);
        fixture.earlier_latest_ms = 159_999;
        assert_eq!(
            fixture.validate(),
            Err(ZecAgreementV1Error::EarlierRefundBoundBeforeLezDeadline)
        );
    }
}

#[test]
fn exact_transaction_policy_binds_roles_fees_inputs_and_expiry() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        let fixture = Fixture::new(direction);
        let agreement = fixture.validate().expect("valid transaction policy");
        let policy = agreement.transaction_policy();
        assert_eq!(policy.funding_input_set_commitment(), &[12; 32]);
        assert_eq!(policy.funding_fee_zatoshis(), 10_000);
        assert_eq!(policy.minimum_change_zatoshis(), 1_000);
        assert_eq!(policy.claim_fee_zatoshis(), 10_000);
        assert_eq!(policy.refund_fee_zatoshis(), 10_000);
        assert_eq!(policy.expiry_delta_blocks(), 40);
        assert_eq!(
            agreement.payout_for(agreement.lez_claimant()),
            ZecRolePayoutV1::LezAccount(*fixture_account(&fixture, agreement.lez_claimant()))
        );
        assert_eq!(
            agreement.payout_for(agreement.lez_depositor()),
            ZecRolePayoutV1::ZcashTransparent(policy.claim_destination())
        );
        assert_ne!(
            agreement.onchain_swap_id(),
            agreement.agreement_commitment()
        );
    }

    let mut fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.funding_input_set_commitment = [0; 32];
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::EmptyFundingInputSetCommitment)
    );
    fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.funding_fee = fixture.zcash_value + 1;
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::UnsafeTransactionFee("funding"))
    );
    fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.minimum_change = 0;
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::InvalidDustPolicy)
    );
    fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.claim_fee = fixture.zcash_value;
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::UnsafeTransactionFee("claim"))
    );
    fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.refund_fee = fixture.zcash_value;
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::UnsafeTransactionFee("refund"))
    );
    fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.funding_change_hash = [0x44; 20];
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::TransactionDestinationMismatch(
            "funding change"
        ))
    );
    fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.claim_destination_hash = [0x44; 20];
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::TransactionDestinationMismatch("claim"))
    );
    fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.refund_destination_hash = [0x44; 20];
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::TransactionDestinationMismatch(
            "refund"
        ))
    );
    fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.expiry_delta -= 1;
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::ExpiryDeltaMismatch)
    );
}

#[test]
fn funding_input_set_commitment_is_bounded_deduplicated_and_order_independent() {
    let first = ZcashFundingInputV1::new([1; 32], 1, 50_000_000, vec![0x51]);
    let second = ZcashFundingInputV1::new([2; 32], 0, 60_000_000, vec![0x52]);
    let ordered =
        ZcashFundingInputSetV1::new(vec![first.clone(), second.clone()]).expect("valid input set");
    let reversed = ZcashFundingInputSetV1::new(vec![second.clone(), first.clone()])
        .expect("canonical sorting");
    assert_eq!(ordered.commitment(), reversed.commitment());
    assert_eq!(ordered.inputs()[0], first);
    assert!(ZcashFundingInputSetV1::new(vec![second.clone(), second]).is_err());
    assert!(ZcashFundingInputSetV1::new(Vec::new()).is_err());
    assert!(
        ZcashFundingInputSetV1::new(vec![
            ZcashFundingInputV1::new([3; 32], 0, 1, vec![0x51]);
            MAX_ZEC_FUNDING_INPUTS + 1
        ])
        .is_err()
    );
    assert!(
        ZcashFundingInputSetV1::new(vec![ZcashFundingInputV1::new(
            [4; 32],
            0,
            1,
            vec![0x51; MAX_ZEC_FUNDING_SCRIPT_BYTES + 1],
        )])
        .is_err()
    );
    assert!(
        ZcashFundingInputSetV1::new(vec![ZcashFundingInputV1::new([0; 32], 0, 1, vec![0x51],)])
            .is_err()
    );
}

#[test]
fn lez_metadata_custody_and_token_destinations_are_exact_and_collision_free() {
    let fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    let agreement = fixture.validate().expect("exact LEZ destinations");
    assert_eq!(
        agreement.lez_terms().metadata_account(),
        &fixture.metadata_account
    );
    assert_eq!(
        agreement.lez_terms().custody_account(),
        &fixture.custody_account
    );

    let mut fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.metadata_account = [0; 32];
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::LezDerivationMismatch)
    );
    fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.custody_account = fixture.metadata_account;
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::LezDerivationMismatch)
    );
    fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.metadata_account = fixture.maker_lez;
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::LezDerivationMismatch)
    );

    fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.use_token_asset([8; 32], [2; 8], [3; 8]);
    if let LezAssetV1::FungibleToken {
        depositor_ata,
        claimant_ata,
        ..
    } = &mut fixture.asset
    {
        *claimant_ata = *depositor_ata;
    }
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::LezDerivationMismatch)
    );
}

#[test]
fn accepted_at_envelope_resumes_after_expiry_without_reaccepting_expired_wire() {
    let fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    let wire = fixture.record().encode_wire().expect("wire");
    let accepted =
        AcceptedZecAgreementV1::accept_wire_at(&wire, UnixSeconds::new(NOW), Participant::Maker, 7)
            .expect("accepted before expiry");
    let envelope = accepted.durable_envelope().expect("durable envelope");
    assert_eq!(envelope.accepted_at(), UnixSeconds::new(NOW));
    assert_eq!(envelope.local_participant(), Participant::Maker);
    assert_eq!(envelope.revision(), 7);
    let resumed = AcceptedZecAgreementV1::resume_from_durable_parts(
        envelope.agreement_wire(),
        envelope.accepted_at(),
        envelope.local_participant(),
        envelope.revision(),
    )
    .expect("resume uses accepted_at");
    assert_eq!(resumed.accepted_at(), UnixSeconds::new(NOW));
    assert_eq!(resumed.local_participant(), Participant::Maker);
    assert_eq!(resumed.revision(), 7);
    assert_eq!(resumed.agreement(), accepted.agreement());
    assert_eq!(
        AcceptedZecAgreementV1::accept_wire_at(
            &wire,
            UnixSeconds::new(fixture.expires_at),
            Participant::Maker,
            8,
        ),
        Err(ZecAgreementV1Error::Expired)
    );
}

#[test]
fn agreement_and_accepted_debug_are_redacted() {
    let fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    let agreement = fixture.validate().expect("agreement");
    let wire = agreement.encode_wire().expect("wire");
    let accepted =
        AcceptedZecAgreementV1::accept_wire_at(&wire, UnixSeconds::new(NOW), Participant::Taker, 0)
            .expect("accepted");
    for rendered in [
        format!("{:?}", fixture.body()),
        format!("{:?}", fixture.record()),
        format!("{agreement:?}"),
        format!("{accepted:?}"),
        format!("{:?}", accepted.durable_envelope().expect("envelope")),
    ] {
        assert!(rendered.contains("[REDACTED]"));
        assert!(!rendered.contains("maker_signature: ["));
        assert!(!rendered.contains("secret_digest: [9"));
    }
}

#[test]
fn agreement_derives_and_validates_canonical_funding_claim_and_refund_requests() {
    let mut fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    let funder_key = public_key(&fixture.taker_signer);
    let candidate = TransparentUtxo::new(
        OutPoint::new([0x31; 32], 2),
        TxOut::new(
            Zatoshis::from_u64(150_000_000).expect("value"),
            TransparentAddress::from_pubkey(&funder_key).script().into(),
        ),
    );
    fixture.funding_input_set_commitment =
        ZcashFundingInputSetV1::new(vec![ZcashFundingInputV1::new(
            [0x31; 32],
            2,
            150_000_000,
            candidate.output().script_pubkey().0.0.clone(),
        )])
        .expect("input set")
        .commitment();
    let agreement = fixture.validate().expect("agreement");
    let current_height = BlockHeight::from_u32(100);
    let funding = agreement
        .funding_request(vec![candidate.clone()], current_height)
        .expect("derived funding request");
    assert_eq!(
        funding.target_fee(),
        Zatoshis::from_u64(10_000).expect("fee")
    );
    assert_eq!(
        funding.minimum_change(),
        Zatoshis::from_u64(1_000).expect("dust")
    );
    assert_eq!(funding.expiry_height(), BlockHeight::from_u32(140));
    agreement
        .validate_funding_request(&funding, current_height)
        .expect("exact funding validates");
    let changed_candidate =
        TransparentUtxo::new(OutPoint::new([0x32; 32], 2), candidate.output().clone());
    assert_eq!(
        agreement.funding_request(vec![changed_candidate], current_height),
        Err(ZecAgreementExecutionError::FundingInputCommitmentMismatch)
    );

    let funding_output = TxOut::new(
        agreement.binding().expected_output().value(),
        Script(Code(
            agreement
                .binding()
                .expected_output()
                .contract()
                .p2sh_script_pubkey()
                .to_vec(),
        )),
    );
    let prevout = OutPoint::new([0x44; 32], 0);
    let claim = agreement
        .claim_spend_request(prevout.clone(), funding_output.clone(), current_height)
        .expect("claim request");
    let refund = agreement
        .refund_spend_request(prevout, funding_output.clone(), current_height)
        .expect("refund request");
    assert_eq!(
        claim.destination(),
        TransparentAddress::PublicKeyHash(fixture.claim_destination_hash)
    );
    assert_eq!(
        refund.destination(),
        TransparentAddress::PublicKeyHash(fixture.refund_destination_hash)
    );
    agreement
        .validate_claim_spend_request(&claim, current_height)
        .expect("claim validates");
    agreement
        .validate_refund_spend_request(&refund, current_height)
        .expect("refund validates");
    let wrong_value = TxOut::new(
        Zatoshis::from_u64(fixture.zcash_value - 1).expect("value"),
        funding_output.script_pubkey().clone(),
    );
    assert_eq!(
        agreement.claim_spend_request(OutPoint::new([0x55; 32], 0), wrong_value, current_height,),
        Err(ZecAgreementExecutionError::FundingOutputMismatch)
    );
}

#[test]
fn pinned_lez_v02_derivation_has_stable_golden_vectors() {
    let program = [1_u32; 8];
    let swap_id = derive_lez_swap_id_v1(b"agreement-v1");
    assert_eq!(
        hex::encode(swap_id),
        "aaa0cfc2ff17c00df0414248a2bf203e9a4897a93ab94b274285059dd90c36af"
    );
    assert_eq!(
        hex::encode(derive_lez_metadata_account_v1(&program, &swap_id)),
        "1a978a31e9fb377ddc2b6eef5bd63e00af0b85fd3ddd6e0989decf3c382d1666"
    );
    assert_eq!(
        hex::encode(derive_lez_native_custody_account_v1(&program, &swap_id)),
        "59e69665d2f50e439ef626f23e83f26861e9ffbdab7cea77b64d651326ced927"
    );
    assert_eq!(
        hex::encode(derive_lez_token_account_v1(&[3; 8], &[3; 32], &[8; 32])),
        "a53b67a18d7eea8ba403d81edea1882aaa4e0fa1fcfdd799bc0e3003ac7a9e3c"
    );
}

#[test]
fn deterministic_profile_accepts_explicit_nssa_v0_1_2_compatibility_identity() {
    let mut fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.use_nssa_v0_1_2_compatibility();

    let agreement = fixture
        .validate()
        .expect("the explicitly named pinned runtime compatibility identity is accepted");
    assert_eq!(
        agreement.lez_terms().chain().environment(),
        LezEnvironmentV1::DeterministicLocalV0_1_2Compatibility
    );
    assert_eq!(
        agreement.lez_terms().metadata_account(),
        &fixture.metadata_account
    );
    assert_eq!(
        agreement.lez_terms().custody_account(),
        &fixture.custody_account
    );

    let wire = agreement.encode_wire().expect("bounded compatibility wire");
    let decoded =
        ZecAgreementV1::from_wire_at(&wire, UnixSeconds::new(NOW)).expect("wire round trip");
    assert_eq!(decoded.record(), agreement.record());
}

#[test]
fn canonical_body_commitment_binds_representative_executable_fields() {
    let baseline = Fixture::new(SwapDirection::TakerSellsForeign);
    assert_eq!(baseline.body().commitment(), baseline.body().commitment());

    let mut changed = baseline.clone();
    changed.lez_amount += 1;
    assert_ne!(baseline.body().commitment(), changed.body().commitment());
    changed = baseline.clone();
    changed.offer_commitment[0] ^= 1;
    assert_ne!(baseline.body().commitment(), changed.body().commitment());
    changed = baseline.clone();
    changed.genesis[0] ^= 1;
    assert_ne!(baseline.body().commitment(), changed.body().commitment());
    changed = baseline.clone();
    changed.metadata_account[0] ^= 1;
    assert_ne!(baseline.body().commitment(), changed.body().commitment());
    changed = baseline.clone();
    changed.claim_fee += 1;
    assert_ne!(baseline.body().commitment(), changed.body().commitment());
    changed = baseline.clone();
    changed.direction = SwapDirection::TakerSellsLez;
    assert_ne!(baseline.body().commitment(), changed.body().commitment());
}

#[test]
fn rejects_schema_commitment_and_dual_signature_failures() {
    let mut fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.schema = ZEC_CONCRETE_AGREEMENT_SCHEMA_V1;
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::UnsupportedSchema(
            ZEC_CONCRETE_AGREEMENT_SCHEMA_V1
        ))
    );

    fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.schema += 1;
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::UnsupportedSchema(3))
    );

    fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.commitment_override = Some([0x55; 32]);
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::CommitmentMismatch)
    );

    fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.maker_signer = key(8);
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::SignatureMismatch(Participant::Maker))
    );

    fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.taker_signer = key(8);
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::SignatureMismatch(Participant::Taker))
    );

    fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.high_s_maker = true;
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::NonCanonicalSignature(
            Participant::Maker
        ))
    );
}

#[test]
fn rejects_unbound_digest_zcash_roles_and_refund_deadline() {
    let mut fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.contract_digest[0] ^= 1;
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::SecretDigestMismatch)
    );

    fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.contract_refund_hash = pubkey_hash(&fixture.maker_key);
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::ZcashRefundAuthorityMismatch)
    );

    fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.contract_claimant_hash = pubkey_hash(&fixture.taker_key);
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::ZcashClaimantMismatch)
    );

    fixture = Fixture::new(SwapDirection::TakerSellsLez);
    fixture.contract_refund_hash = pubkey_hash(&fixture.taker_key);
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::ZcashRefundAuthorityMismatch)
    );

    fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.zcash_refund_lock += 1;
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::ZcashRefundDeadlineMismatch)
    );
}

#[test]
fn rejects_identity_profile_amount_and_transcript_failures() {
    let mut fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.maker_lez = [0; 32];
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::EmptyLezIdentity)
    );

    fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.taker_lez = fixture.maker_lez;
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::DuplicateParticipantIdentity)
    );

    fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.taker_key = fixture.maker_key;
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::DuplicateParticipantIdentity)
    );

    fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.maker_key[0] = 4;
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::InvalidZcashPublicKey(
            Participant::Maker
        ))
    );

    fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.environment = LezEnvironmentV1::PublicTestnetV0_2;
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::LezEnvironmentMismatch)
    );

    fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.binding_profile = ZecProfileId::PublicTestnetV1;
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::ProfileMismatch)
    );

    fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.lez_amount = 0;
    assert_eq!(fixture.validate(), Err(ZecAgreementV1Error::EmptyLezAmount));

    fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.zcash_value = 0;
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::EmptyZcashAmount)
    );

    fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.body_digest = [0; 32];
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::EmptySecretDigest)
    );

    fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.session_id = [0; 32];
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::EmptyTranscriptIdentity)
    );

    fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.expires_at = NOW;
    assert_eq!(fixture.validate(), Err(ZecAgreementV1Error::Expired));
}

#[test]
fn rejects_unsafe_or_aliased_lez_program_and_refund_terms() {
    let mut fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.channel_id = [0; 32];
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::EmptyLezChannel)
    );

    fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.genesis = [0; 32];
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::EmptyLezGenesis)
    );

    fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.escrow_program = [0; 8];
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::EmptyLezIdentity)
    );

    fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.asset = LezAssetV1::Native {
        authenticated_transfer_program_id: fixture.escrow_program,
    };
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::ConflictingLezPrograms)
    );

    fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.use_token_asset([8; 32], [2; 8], [2; 8]);
    assert_eq!(
        fixture.validate(),
        Err(ZecAgreementV1Error::ConflictingLezPrograms)
    );

    fixture = Fixture::new(SwapDirection::TakerSellsForeign);
    fixture.later_earliest = 189;
    assert!(matches!(
        fixture.validate(),
        Err(ZecAgreementV1Error::InvalidRefundProfile(_))
    ));
}

#[test]
fn valid_token_asset_is_bound_by_the_same_dual_signed_body() {
    let mut fixture = Fixture::new(SwapDirection::TakerSellsLez);
    fixture.use_token_asset([8; 32], [2; 8], [3; 8]);
    assert!(fixture.validate().is_ok());
}

fn fixture_account(fixture: &Fixture, participant: Participant) -> &[u8; 32] {
    match participant {
        Participant::Maker => &fixture.maker_lez,
        Participant::Taker => &fixture.taker_lez,
    }
}

fn key(value: u8) -> SecretKey {
    SecretKey::from_slice(&[value; 32]).expect("valid fixture key")
}

fn public_key(secret_key: &SecretKey) -> PublicKey {
    PublicKey::from_secret_key(&Secp256k1::new(), secret_key)
}

fn pubkey_hash(bytes: &[u8; 33]) -> [u8; 20] {
    match TransparentAddress::from_pubkey(&PublicKey::from_slice(bytes).expect("fixture pubkey")) {
        TransparentAddress::PublicKeyHash(hash) => hash,
        TransparentAddress::ScriptHash(_) => unreachable!("public keys produce P2PKH"),
    }
}

fn sign(commitment: [u8; 32], secret_key: &SecretKey) -> [u8; 64] {
    Secp256k1::new()
        .sign_ecdsa(&Message::from_digest(commitment), secret_key)
        .serialize_compact()
}

fn to_high_s(mut compact: [u8; 64]) -> [u8; 64] {
    const CURVE_ORDER: [u8; 32] = [
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
        0xfe, 0xba, 0xae, 0xdc, 0xe6, 0xaf, 0x48, 0xa0, 0x3b, 0xbf, 0xd2, 0x5e, 0x8c, 0xd0, 0x36,
        0x41, 0x41,
    ];
    let low_s: [u8; 32] = compact[32..].try_into().expect("fixed compact signature");
    let mut high_s = [0_u8; 32];
    let mut borrow = 0_u16;
    for index in (0..32).rev() {
        let minuend = u16::from(CURVE_ORDER[index]);
        let subtrahend = u16::from(low_s[index]) + borrow;
        if minuend >= subtrahend {
            high_s[index] = u8::try_from(minuend - subtrahend).expect("borrow-free byte");
            borrow = 0;
        } else {
            high_s[index] = u8::try_from(minuend + 256 - subtrahend).expect("borrowed byte");
            borrow = 1;
        }
    }
    assert_eq!(borrow, 0);
    compact[32..].copy_from_slice(&high_s);
    compact
}
