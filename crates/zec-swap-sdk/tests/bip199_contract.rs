use lez_zec_swap_sdk::Bip199Contract;

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
