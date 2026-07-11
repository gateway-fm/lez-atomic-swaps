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
