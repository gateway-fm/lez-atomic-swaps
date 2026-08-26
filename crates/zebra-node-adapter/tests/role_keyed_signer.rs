//! Production role-keyed Zcash signer contract.

#![forbid(unsafe_code)]

use lez_swap_core::Participant;
use lez_zebra_node_adapter::{
    RoleKeyedZcashSigner, RoleKeyedZcashSignerError, ZebraClaimSigner, ZebraRefundSigner,
};
use lez_zec_swap_sdk::{
    Bip199Contract, ClaimPreimage, TransactionBuildError, TransparentSpendRequest,
    build_claim_transaction, build_refund_transaction,
};
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use sha2::{Digest as _, Sha256};
use zcash_primitives::transaction::Transaction;
use zcash_protocol::{
    consensus::{BlockHeight, BranchId},
    value::Zatoshis,
};
use zcash_script::script::Code;
use zcash_transparent::{
    address::{Script, TransparentAddress},
    bundle::{OutPoint, TxOut},
};

#[tokio::test]
async fn role_keys_build_the_exact_canonical_claim_and_refund_bytes() {
    let fixture = fixture();
    let claimant = RoleKeyedZcashSigner::new(Participant::Maker, key(1));
    let refunder = RoleKeyedZcashSigner::new(Participant::Taker, key(2));

    assert_eq!(claimant.participant(), Participant::Maker);
    assert_eq!(claimant.public_key(), public_key(&key(1)));
    assert_eq!(refunder.participant(), Participant::Taker);

    let actual_claim = claimant
        .sign_claim(&fixture.contract, &fixture.request, &fixture.preimage)
        .await
        .expect("canonical claim signature");
    let expected_claim = serialize(
        &build_claim_transaction(
            &fixture.contract,
            &fixture.request,
            &key(1),
            fixture.preimage.expose_secret(),
        )
        .expect("direct canonical claim"),
    );
    assert_eq!(actual_claim, expected_claim);

    let actual_refund = refunder
        .sign_refund(&fixture.contract, &fixture.request)
        .await
        .expect("canonical refund signature");
    let expected_refund = serialize(
        &build_refund_transaction(&fixture.contract, &fixture.request, &key(2))
            .expect("direct canonical refund"),
    );
    assert_eq!(actual_refund, expected_refund);
    assert_eq!(
        ZebraRefundSigner::participant(&refunder),
        Participant::Taker
    );
}

#[tokio::test]
async fn a_role_key_cannot_sign_a_branch_owned_by_another_key() {
    let fixture = fixture();
    let wrong = RoleKeyedZcashSigner::new(Participant::Maker, key(3));

    let claim = wrong
        .sign_claim(&fixture.contract, &fixture.request, &fixture.preimage)
        .await
        .expect_err("foreign claimant key must fail");
    assert!(matches!(
        claim,
        RoleKeyedZcashSignerError::Build(TransactionBuildError::WrongSpendingKey)
    ));

    let refund = wrong
        .sign_refund(&fixture.contract, &fixture.request)
        .await
        .expect_err("foreign refund key must fail");
    assert!(matches!(
        refund,
        RoleKeyedZcashSignerError::Build(TransactionBuildError::WrongSpendingKey)
    ));
}

#[test]
fn signer_diagnostics_disclose_role_but_never_secret_key_material() {
    let signer = RoleKeyedZcashSigner::new(Participant::Maker, key(1));
    let diagnostic = format!("{signer:?}");

    assert!(diagnostic.contains("RoleKeyedZcashSigner"));
    assert!(diagnostic.contains("Maker"));
    assert!(diagnostic.contains("[REDACTED]"));
    assert!(!diagnostic.contains(&hex::encode([1_u8; 32])));
}

struct Fixture {
    contract: Bip199Contract,
    request: TransparentSpendRequest,
    preimage: ClaimPreimage,
}

fn fixture() -> Fixture {
    let secret = [0x91; 32];
    let contract = Bip199Contract::new(
        120,
        public_key_hash(&key(2)),
        Sha256::digest(secret).into(),
        public_key_hash(&key(1)),
    );
    let funding_output = TxOut::new(
        Zatoshis::from_u64(100_000).expect("funding amount"),
        Script(Code(contract.p2sh_script_pubkey().to_vec())),
    );
    let request = TransparentSpendRequest::new(
        &contract,
        OutPoint::new([0x77; 32], 0),
        funding_output,
        TransparentAddress::PublicKeyHash([0x55; 20]),
        Zatoshis::from_u64(1_000).expect("fee"),
        BlockHeight::from_u32(200),
        BranchId::Nu6_2,
    )
    .expect("canonical spend request");
    Fixture {
        contract,
        request,
        preimage: ClaimPreimage::new(secret),
    }
}

fn key(byte: u8) -> SecretKey {
    SecretKey::from_slice(&[byte; 32]).expect("fixture key")
}

fn public_key(key: &SecretKey) -> PublicKey {
    PublicKey::from_secret_key(&Secp256k1::new(), key)
}

fn public_key_hash(key: &SecretKey) -> [u8; 20] {
    match TransparentAddress::from_pubkey(&public_key(key)) {
        TransparentAddress::PublicKeyHash(hash) => hash,
        TransparentAddress::ScriptHash(_) => unreachable!("public key produces P2PKH"),
    }
}

fn serialize(transaction: &Transaction) -> Vec<u8> {
    let mut exact = Vec::new();
    transaction
        .write(&mut exact)
        .expect("canonical serialization");
    exact
}
