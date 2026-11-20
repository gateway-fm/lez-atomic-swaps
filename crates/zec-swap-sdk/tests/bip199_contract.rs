use lez_zec_swap_sdk::{
    Bip199Contract, Bip199PreimageRule, ExpectedBip199Output, ProfileError, ZecProfileId,
    ZecRefundProfile, ZecRevealOrder, ZecSwapBinding,
};
use zcash_protocol::{
    consensus::{BranchId, NetworkType},
    value::Zatoshis,
};

/// The redeem script every profile used before the preimage size check, fixed from that
/// code: `IF SHA256 <digest> EQUALVERIFY DUP HASH160 <claimant> ELSE <500000> CLTV DROP DUP
/// HASH160 <refund> ENDIF EQUALVERIFY CHECKSIG`.
const DIGEST_ONLY_REDEEM_HEX: &str = "63a8201111111111111111111111111111111111111111111111111111111111111111\
    8876a9143333333333333333333333333333333333333333670320a107b17576a914\
    22222222222222222222222222222222222222226888ac";
const DIGEST_ONLY_P2SH_HEX: &str = "a914df7464ddb0060cac87c7bfe5f58f0154fe62d07887";
/// The Taker-reveals-first profiles' redeem script: `OP_SIZE <32> OP_EQUALVERIFY` before
/// `OP_SHA256`, the rest unchanged.
const EXACTLY_32_BYTES_REDEEM_HEX: &str = "6382012088a820\
    1111111111111111111111111111111111111111111111111111111111111111\
    8876a9143333333333333333333333333333333333333333670320a107b17576a914\
    22222222222222222222222222222222222222226888ac";
const EXACTLY_32_BYTES_P2SH_HEX: &str = "a914849b207ae58affdcac1221314e6b4694bd04bcbf87";

const ALL_PROFILES: [ZecProfileId; 14] = [
    ZecProfileId::DeterministicLocalV1,
    ZecProfileId::PublicTestnetV1,
    ZecProfileId::DeterministicLocalDevnetV1,
    ZecProfileId::DeterministicLocalDevnetV2,
    ZecProfileId::DeterministicLocalDevnetV3,
    ZecProfileId::DeterministicLocalDevnetV4,
    ZecProfileId::PublicTestnetV2,
    ZecProfileId::DeterministicLocalV2,
    ZecProfileId::DeterministicLocalDevnetV5,
    ZecProfileId::DeterministicLocalDevnetV6,
    ZecProfileId::DeterministicLocalV3,
    ZecProfileId::DeterministicLocalDevnetV7,
    ZecProfileId::DeterministicLocalDevnetV8,
    ZecProfileId::PublicTestnetV3,
];

fn profile_contract(profile: ZecProfileId) -> Bip199Contract {
    Bip199Contract::for_profile(profile, 500_000, [0x22; 20], [0x11; 32], [0x33; 20])
}

#[test]
fn legacy_profiles_keep_the_digest_only_script_byte_for_byte() {
    for profile in ALL_PROFILES {
        if ZecRefundProfile::for_id(profile).reveal_order() != ZecRevealOrder::LezRecipientFirst {
            continue;
        }
        let contract = profile_contract(profile);
        assert_eq!(contract.preimage_rule(), Bip199PreimageRule::DigestOnly);
        assert_eq!(
            hex::encode(contract.redeem_script()),
            DIGEST_ONLY_REDEEM_HEX,
            "{profile:?}"
        );
        assert_eq!(
            hex::encode(contract.p2sh_script_pubkey()),
            DIGEST_ONLY_P2SH_HEX
        );
        assert_eq!(
            contract,
            Bip199Contract::new(500_000, [0x22; 20], [0x11; 32], [0x33; 20])
        );
    }
}

#[test]
fn taker_reveals_first_profiles_check_the_preimage_size_before_sha256() {
    let mut checked = 0;
    for profile in ALL_PROFILES {
        if ZecRefundProfile::for_id(profile).reveal_order() != ZecRevealOrder::TakerFirst {
            continue;
        }
        checked += 1;
        let contract = profile_contract(profile);
        assert_eq!(contract.preimage_rule(), Bip199PreimageRule::Exactly32Bytes);
        assert_eq!(
            hex::encode(contract.redeem_script()),
            EXACTLY_32_BYTES_REDEEM_HEX,
            "{profile:?}"
        );
        assert_eq!(contract.redeem_script().len(), 96);
        assert_eq!(
            hex::encode(contract.p2sh_script_pubkey()),
            EXACTLY_32_BYTES_P2SH_HEX
        );
        assert!(contract.admits_preimage_length(32));
        assert!(!contract.admits_preimage_length(31));
        assert!(!contract.admits_preimage_length(33));

        // The claim stack keeps its layout; only the redeem push grows (PUSHDATA1 0x60).
        let signature = [0x30, 0x01];
        let public_key = [0x02; 33];
        let mut expected_claim = vec![0x02, 0x30, 0x01, 0x21];
        expected_claim.extend(public_key);
        expected_claim.push(0x20);
        expected_claim.extend([0x44; 32]);
        expected_claim.extend([0x51, 0x4c, 0x60]);
        expected_claim.extend(contract.redeem_script());
        assert_eq!(
            contract
                .claim_script_sig(&signature, &public_key, &[0x44; 32])
                .unwrap(),
            expected_claim,
        );
        let mut expected_refund = vec![0x02, 0x30, 0x01, 0x21];
        expected_refund.extend(public_key);
        expected_refund.extend([0x00, 0x4c, 0x60]);
        expected_refund.extend(contract.redeem_script());
        assert_eq!(
            contract.refund_script_sig(&signature, &public_key).unwrap(),
            expected_refund,
        );
    }
    assert_eq!(checked, 4);
}

#[test]
fn a_binding_refuses_a_contract_whose_preimage_rule_is_not_the_profiles() {
    let output = |rule| {
        ExpectedBip199Output::new(
            NetworkType::Regtest,
            BranchId::Nu7,
            Zatoshis::from_u64(10_000).unwrap(),
            Bip199Contract::with_preimage_rule(rule, 500_000, [0x22; 20], [0x11; 32], [0x33; 20]),
        )
    };
    assert_eq!(
        ZecSwapBinding::new(
            ZecProfileId::DeterministicLocalDevnetV7,
            output(Bip199PreimageRule::DigestOnly)
        ),
        Err(ProfileError::PreimageRuleMismatch)
    );
    assert!(
        ZecSwapBinding::new(
            ZecProfileId::DeterministicLocalDevnetV7,
            output(Bip199PreimageRule::Exactly32Bytes)
        )
        .is_ok()
    );
    assert_eq!(
        ZecSwapBinding::new(
            ZecProfileId::DeterministicLocalDevnetV5,
            output(Bip199PreimageRule::Exactly32Bytes)
        ),
        Err(ProfileError::PreimageRuleMismatch)
    );
    assert!(
        ZecSwapBinding::new(
            ZecProfileId::DeterministicLocalDevnetV5,
            output(Bip199PreimageRule::DigestOnly)
        )
        .is_ok()
    );
}

#[test]
fn redeem_script_is_the_exact_bip199_sha256_cltv_p2pkh_template() {
    let contract = Bip199Contract::new(500_000, [0x22; 20], [0x11; 32], [0x33; 20]);

    let mut expected = vec![0x63, 0xa8, 0x20]; // IF SHA256 PUSH32
    expected.extend([0x11; 32]); // secret digest
    expected.extend([0x88, 0x76, 0xa9, 0x14]); // EQUALVERIFY DUP HASH160 PUSH20
    expected.extend([0x33; 20]); // claimant pubkey hash
    expected.extend([0x67, 0x03, 0x20, 0xa1, 0x07]); // ELSE PUSH3 500000
    expected.extend([0xb1, 0x75, 0x76, 0xa9, 0x14]); // CLTV DROP DUP HASH160 PUSH20
    expected.extend([0x22; 20]); // refund pubkey hash
    expected.extend([0x68, 0x88, 0xac]); // ENDIF EQUALVERIFY CHECKSIG

    assert_eq!(contract.redeem_script(), expected);
    assert_eq!(contract.redeem_script().len(), 92);
}

#[test]
fn p2sh_and_claim_refund_stacks_are_consensus_encoded_in_role_order() {
    let contract = Bip199Contract::new(500_000, [0x22; 20], [0x11; 32], [0x33; 20]);
    let signature = [0x30, 0x01];
    let public_key = [0x02; 33];

    assert_eq!(
        contract.p2sh_script_pubkey(),
        [
            0xa9, 0x14, 0xdf, 0x74, 0x64, 0xdd, 0xb0, 0x06, 0x0c, 0xac, 0x87, 0xc7, 0xbf, 0xe5,
            0xf5, 0x8f, 0x01, 0x54, 0xfe, 0x62, 0xd0, 0x78, 0x87,
        ],
    );

    let mut expected_claim = vec![0x02, 0x30, 0x01, 0x21];
    expected_claim.extend(public_key);
    expected_claim.push(0x20);
    expected_claim.extend([0x44; 32]);
    expected_claim.extend([0x51, 0x4c, 0x5c]); // TRUE, PUSHDATA1, redeem length
    expected_claim.extend(contract.redeem_script());
    assert_eq!(
        contract
            .claim_script_sig(&signature, &public_key, &[0x44; 32])
            .unwrap(),
        expected_claim,
    );

    let mut expected_refund = vec![0x02, 0x30, 0x01, 0x21];
    expected_refund.extend(public_key);
    expected_refund.extend([0x00, 0x4c, 0x5c]); // FALSE, PUSHDATA1, redeem length
    expected_refund.extend(contract.redeem_script());
    assert_eq!(
        contract.refund_script_sig(&signature, &public_key).unwrap(),
        expected_refund,
    );
}

#[test]
fn script_sig_builder_rejects_oversized_stack_items() {
    let contract = Bip199Contract::new(500_000, [0x22; 20], [0x11; 32], [0x33; 20]);

    assert!(
        contract
            .claim_script_sig(&[0x30, 0x01], &[0x02; 33], &[0x44; 521])
            .is_err()
    );
}

#[test]
fn refund_transaction_policy_pins_cltv_and_a_non_final_input() {
    let contract = Bip199Contract::new(500_000, [0x22; 20], [0x11; 32], [0x33; 20]);

    assert_eq!(contract.refund_lock_time(), 500_000);
    assert_eq!(contract.refund_input_sequence(), u32::MAX - 1);
    assert_ne!(contract.refund_input_sequence(), u32::MAX);
}
