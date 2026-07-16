use std::str::FromStr as _;

use bitcoin::consensus::serialize;
use bitcoin::hashes::Hash as _;
use bitcoin::secp256k1::{Keypair, Message, PublicKey, Secp256k1, SecretKey};
use bitcoin::{
    Amount, BlockHash, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Witness, absolute,
    transaction,
};
use corepc_types::v31::GetRawTransactionVerbose;
use lez_btc_swap_sdk::{
    AdaptorSessionContext, AdaptorSigner, BTC_AGREEMENT_SCHEMA_V1, BtcAgreementBodyV1,
    BtcAgreementRecordV1, BtcAgreementV1, BtcChainPolicyV1, BtcClaimTermsV1, BtcFundingTermsV1,
    BtcLezTermsV1, BtcP2trTermsV1, BtcParticipantIdentityV1, BtcParticipantsV1, BtcRecoveryPlanV1,
    CsvBlockDelay, P2trSwapOutput, RefundXOnlyKey, SigningRole, TwoPartyAggregateKey,
    adapt_presignature,
};
use lez_swap_core::SwapDirection;
use zeroize::Zeroizing;

pub const REGTEST_GENESIS: &str =
    "0f9188f13cb7b2c71f2a335e3a4fc328bf5beb436012afca590b1a11466e2206";
pub const REQUIRED_CONFIRMATIONS: u32 = 6;
const FUNDING_VALUE_SAT: u64 = 100_000;
const CLAIM_VALUE_SAT: u64 = 99_000;
pub const MAKER_SECRET: [u8; 32] = [0x31; 32];
pub const TAKER_SECRET: [u8; 32] = [0x42; 32];
pub const ADAPTOR_SECRET: [u8; 32] = [0x53; 32];
pub const REFUND_SECRET: [u8; 32] = [0x62; 32];
pub const LEZ_PREPARED_MESSAGE_BYTES: &[u8] = b"m3-actor-prepared-witnessed-claim";

#[allow(clippy::missing_panics_doc, clippy::must_use_candidate)]
pub fn lez_claim_message_hash() -> [u8; 32] {
    lez_message_hash(LEZ_PREPARED_MESSAGE_BYTES)
}

#[allow(clippy::missing_panics_doc, clippy::must_use_candidate)]
pub fn lez_message_hash(exact_message: &[u8]) -> [u8; 32] {
    let bytes = [
        b"/LEE/v0.3/Message/Public/\0\0\0\0\0\0\0".as_slice(),
        exact_message,
    ]
    .concat();
    bitcoin::hashes::sha256::Hash::hash(&bytes).to_byte_array()
}

#[derive(Debug)]
pub struct SwapFixture {
    pub agreement: BtcAgreementV1,
    pub funding: Transaction,
    pub claim: Transaction,
    pub refund: Transaction,
}

#[allow(clippy::missing_panics_doc, clippy::must_use_candidate)]
pub fn raw_verbose(
    transaction: &Transaction,
    confirmations: Option<u64>,
    block_hash: Option<&str>,
) -> GetRawTransactionVerbose {
    let inputs: Vec<_> = transaction
        .input
        .iter()
        .map(|input| {
            let witness: Vec<_> = input.witness.iter().map(hex::encode).collect();
            if input.previous_output.is_null() {
                serde_json::json!({
                    "coinbase": hex::encode(input.script_sig.as_bytes()),
                    "txinwitness": witness,
                    "sequence": input.sequence.to_consensus_u32()
                })
            } else {
                serde_json::json!({
                    "txid": input.previous_output.txid.to_string(),
                    "vout": input.previous_output.vout,
                    "scriptSig": {
                        "asm": "",
                        "hex": hex::encode(input.script_sig.as_bytes())
                    },
                    "txinwitness": witness,
                    "sequence": input.sequence.to_consensus_u32()
                })
            }
        })
        .collect();
    let outputs: Vec<_> = transaction
        .output
        .iter()
        .enumerate()
        .map(|(index, output)| {
            serde_json::json!({
                "value": output.value.to_btc(),
                "n": index,
                "scriptPubKey": {
                    "asm": "",
                    "desc": null,
                    "hex": hex::encode(output.script_pubkey.as_bytes()),
                    "reqSigs": null,
                    "type": "nonstandard",
                    "address": null,
                    "addresses": null
                }
            })
        })
        .collect();
    serde_json::from_value(serde_json::json!({
        "hex": hex::encode(serialize(transaction)), "txid": transaction.compute_txid().to_string(),
        "hash": transaction.compute_wtxid().to_string(), "size": transaction.total_size(),
        "vsize": transaction.vsize(), "weight": transaction.weight().to_wu(),
        "version": transaction.version.0, "locktime": transaction.lock_time.to_consensus_u32(),
        "vin": inputs, "vout": outputs, "blockhash": block_hash, "confirmations": confirmations
    }))
    .expect("raw transaction response")
}

fn secret(bytes: [u8; 32]) -> SecretKey {
    SecretKey::from_slice(&bytes).expect("valid fixture secret")
}

fn public_key(bytes: [u8; 32]) -> [u8; 33] {
    PublicKey::from_secret_key(&Secp256k1::new(), &secret(bytes)).serialize()
}

fn x_only(bytes: [u8; 32]) -> [u8; 32] {
    Keypair::from_secret_key(&Secp256k1::new(), &secret(bytes))
        .x_only_public_key()
        .0
        .serialize()
}

fn destination(bytes: [u8; 32]) -> Vec<u8> {
    let key = Keypair::from_secret_key(&Secp256k1::new(), &secret(bytes))
        .x_only_public_key()
        .0;
    ScriptBuf::new_p2tr(&Secp256k1::verification_only(), key, None).into_bytes()
}

fn complete_adaptor_signature(
    context: &AdaptorSessionContext,
    maker_secret: [u8; 32],
    taker_secret: [u8; 32],
) -> [u8; 64] {
    let mut maker = AdaptorSigner::new(context.clone(), SigningRole::Maker, maker_secret)
        .expect("maker signer");
    let mut taker = AdaptorSigner::new(context.clone(), SigningRole::Taker, taker_secret)
        .expect("taker signer");
    let maker_commitment = maker.nonce_commitment();
    let taker_commitment = taker.nonce_commitment();
    maker
        .accept_peer_commitment(taker_commitment)
        .expect("maker commitment");
    taker
        .accept_peer_commitment(maker_commitment)
        .expect("taker commitment");
    let maker_nonce = maker.public_nonce().expect("maker nonce");
    let taker_nonce = taker.public_nonce().expect("taker nonce");
    maker.accept_peer_nonce(taker_nonce).expect("maker nonce");
    taker.accept_peer_nonce(maker_nonce).expect("taker nonce");
    let maker_partial = maker.create_partial_signature().expect("maker partial");
    let taker_partial = taker.create_partial_signature().expect("taker partial");
    maker
        .accept_peer_partial_signature(taker_partial)
        .expect("maker partial");
    taker
        .accept_peer_partial_signature(maker_partial)
        .expect("taker partial");
    let presignature = maker.presignature().expect("presignature");
    assert_eq!(presignature, taker.presignature().expect("presignature"));
    adapt_presignature(context, presignature, Zeroizing::new(ADAPTOR_SECRET))
        .expect("adapted signature")
}

fn agreement_signature(secret_bytes: [u8; 32], commitment: [u8; 32]) -> [u8; 64] {
    let secp = Secp256k1::new();
    secp.sign_schnorr_no_aux_rand(
        &Message::from_digest(commitment),
        &Keypair::from_secret_key(&secp, &secret(secret_bytes)),
    )
    .serialize()
}

#[allow(
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::too_many_lines
)]
pub fn swap_fixture() -> SwapFixture {
    let maker = BtcParticipantIdentityV1::new(
        [10; 32],
        public_key(MAKER_SECRET),
        x_only([0x61; 32]),
        destination([0x71; 32]),
    );
    let taker = BtcParticipantIdentityV1::new(
        [11; 32],
        public_key(TAKER_SECRET),
        x_only([0x62; 32]),
        destination([0x72; 32]),
    );
    let participants = BtcParticipantsV1::new(maker, taker);
    let adaptor_point = public_key(ADAPTOR_SECRET);
    let aggregate = AdaptorSessionContext::untweaked(
        [public_key(MAKER_SECRET), public_key(TAKER_SECRET)],
        [30; 32],
        adaptor_point,
        [31; 32],
    )
    .expect("aggregate context")
    .output_key();
    let contract = P2trSwapOutput::new(
        TwoPartyAggregateKey::from_bytes(aggregate).expect("aggregate key"),
        RefundXOnlyKey::from_bytes(x_only([0x62; 32])).expect("refund key"),
        CsvBlockDelay::new(144).expect("CSV"),
    )
    .expect("contract");
    let funding = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::null(),
            script_sig: ScriptBuf::from_bytes(vec![1]),
            sequence: Sequence::MAX,
            witness: Witness::new(),
        }],
        output: vec![
            TxOut {
                value: Amount::from_sat(1),
                script_pubkey: ScriptBuf::new(),
            },
            TxOut {
                value: Amount::from_sat(FUNDING_VALUE_SAT),
                script_pubkey: ScriptBuf::from_bytes(contract.script_pubkey_bytes().to_vec()),
            },
        ],
    };
    let funding_terms =
        BtcFundingTermsV1::new(funding.compute_txid().to_byte_array(), 1, FUNDING_VALUE_SAT);
    let spend = lez_btc_swap_sdk::CooperativeKeyPathSpend::new(
        &contract,
        OutPoint {
            txid: funding.compute_txid(),
            vout: 1,
        },
        Amount::from_sat(FUNDING_VALUE_SAT),
        vec![TxOut {
            value: Amount::from_sat(CLAIM_VALUE_SAT),
            script_pubkey: ScriptBuf::from_bytes(
                participants
                    .for_participant(lez_swap_core::Participant::Maker)
                    .claim_destination_script_pubkey()
                    .to_vec(),
            ),
        }],
    )
    .expect("claim spend");
    let body = BtcAgreementBodyV1::new(
        [20; 32],
        SwapDirection::TakerSellsForeign,
        BtcChainPolicyV1::new(
            BlockHash::from_str(REGTEST_GENESIS)
                .expect("regtest genesis")
                .to_byte_array(),
            REQUIRED_CONFIRMATIONS,
        ),
        participants,
        adaptor_point,
        BtcLezTermsV1::new(
            [17; 32],
            [18; 32],
            [15; 32],
            [16; 32],
            [12; 32],
            [13; 32],
            [14; 32],
            [10; 32],
            [11; 32],
            5_000,
            1_700_000_100_000,
            lez_claim_message_hash(),
        ),
        BtcP2trTermsV1::from_contract(&contract),
        funding_terms,
        BtcClaimTermsV1::from_spend(&spend).expect("claim terms"),
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
        agreement_signature(MAKER_SECRET, commitment),
        agreement_signature(TAKER_SECRET, commitment),
    );
    let agreement = BtcAgreementV1::validate(record).expect("valid agreement");
    let context = AdaptorSessionContext::taproot(
        [public_key(MAKER_SECRET), public_key(TAKER_SECRET)],
        agreement.p2tr_contract().merkle_root_bytes(),
        agreement.cooperative_claim().sighash_bytes(),
        adaptor_point,
        agreement.role_session_binding(),
    )
    .expect("claim context");
    let signature = complete_adaptor_signature(&context, MAKER_SECRET, TAKER_SECRET);
    let claim = agreement
        .cooperative_claim()
        .clone()
        .finalize(signature)
        .expect("signed claim");
    let refund_signature = Secp256k1::new()
        .sign_schnorr_no_aux_rand(
            &Message::from_digest(agreement.bitcoin_refund().sighash_bytes()),
            &Keypair::from_secret_key(&Secp256k1::new(), &secret(REFUND_SECRET)),
        )
        .serialize();
    let refund = agreement
        .bitcoin_refund()
        .clone()
        .finalize(refund_signature)
        .expect("signed refund");
    SwapFixture {
        agreement,
        funding,
        claim,
        refund,
    }
}

#[test]
fn refund_fixture_is_exact_signed_agreement_transaction() {
    let fixture = swap_fixture();
    assert_eq!(
        fixture.refund.compute_txid(),
        fixture
            .agreement
            .bitcoin_refund()
            .unsigned_transaction()
            .compute_txid()
    );
    assert_eq!(fixture.refund.input[0].witness.len(), 3);
}
