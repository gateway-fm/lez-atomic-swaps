//! `TakerSellsLez` on the pinned isolated Zebra Regtest node: the Taker claims the
//! Maker-funded Zcash HTLC first and the Maker reads the preimage from that spend.
//!
//! The Taker runs the SDK with the production Zebra port and role-keyed signer: it
//! observes the Maker's real funding, passes the reveal gate and sends its claim through
//! Zebra. The Maker reads the claim with `observe_counterparty_revealing_claim` on the same
//! node, from the mempool on. A claim the Taker builds outside the SDK (another destination,
//! another fee, expiry height 0) reveals the preimage the same way. A refund of the HTLC is
//! classified as the refund branch and never yields a preimage. The HTLC of a Taker-reveals-first
//! profile admits only a 32-byte preimage (the LEZ escrow's length): Zebra refuses a claim with
//! a 33-byte or 31-byte preimage of the digest, and the Maker refunds that HTLC. LEZ is not part
//! of this harness: the Taker's LEZ lock is projected from fixed
//! evidence, and the Maker's LEZ claim that follows the reveal is covered by the SDK and
//! Node lanes.

use std::time::Duration;

use async_trait::async_trait;
use jsonrpsee::{core::client::ClientT, rpc_params};
use jsonrpsee_http_client::{HttpClient, HttpClientBuilder};
use lez_swap_core::{Participant, Phase, SwapDirection, UnixSeconds};
use lez_swap_store::SqliteZecRecoveryStore;
use lez_zebra_node_adapter::{
    HttpZebraRpc, HttpZebraRpcConfig, RoleKeyedZcashSigner, ZebraChainIdentity, ZebraClaimError,
    ZebraRpcClaimPort, ZebraRpcRefundPort, ZebraRpcZcashPort,
};
use lez_zec_swap_sdk::{
    ActiveZecSwap, Bip199Contract, Bip199PreimageRule, ClaimDriveOutcome, ClaimPreimage,
    ClaimStepV1, FirstLockConfirmedEvidenceV1, FirstLockPlanV1, FirstLockStepV1, LezAssetV1,
    LezChainIdentityV1, LezClaimPort, LezEnvironmentV1, LezMakerLockObservationPort,
    LezUnixMilliseconds, MakerLockObservationV1, NegotiationChannel, NegotiationTranscriptV1,
    OfferDiscovery, PreparedClaimSubmissionV1, PreparedFirstLockSubmissionV1, ProtectedClaimKey,
    RefundObservationV1, RefundSubmitOutcomeV1, RevealingClaimObservationV1,
    RevealingClaimSubmitOutcomeV1, TransparentFundingRequest, TransparentSpendRequest,
    TransparentUtxo, ZEC_CONCRETE_AGREEMENT_SCHEMA_V2, ZcashClaimPort, ZcashRefundPort,
    ZcashTransparentDestinationV1, ZecAgreementBodyV1, ZecAgreementRecordV1, ZecAgreementV1,
    ZecLezTermsV1, ZecPairSdk, ZecParticipantIdentityV1, ZecParticipantsV1, ZecProfileId,
    ZecProfileRecordV1, ZecRefundPlanV1, ZecRefundProfile, ZecSdkError, ZecSwapBinding,
    ZecSwapBindingRecordV1, ZecTransactionPolicyV1, build_claim_transaction,
    build_funding_transaction, build_refund_transaction, derive_lez_metadata_account_v1,
    derive_lez_native_custody_account_v1, derive_lez_swap_id_v1,
};
use secp256k1::{Message, PublicKey, Secp256k1, SecretKey};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;
use zcash_primitives::transaction::Transaction;
use zcash_protocol::{
    TxId,
    consensus::{BlockHeight, BranchId, NetworkType},
    value::Zatoshis,
};
use zcash_transparent::{address::TransparentAddress, bundle::OutPoint};

const MINER_ADDRESS: &str = "tmNAP26Sw5Ra2jepAoTr1kqdkggawba6Akd";
const PRINCIPAL: u64 = 100_000_000;
const PROFILE: ZecProfileId = ZecProfileId::DeterministicLocalV3;
/// Inside the claim window: deterministic-local-v3's LEZ deadline is the 100 s anchor plus
/// 120 s, and the window closes the 30 s margin before it (190 000 ms).
const NOW_INSIDE_CLAIM_WINDOW: u64 = 150_000;

/// The Maker funds and refunds with the deterministic miner key.
fn maker_key() -> SecretKey {
    SecretKey::from_slice(&[4; 32]).expect("fixed Maker key")
}

fn taker_key() -> SecretKey {
    SecretKey::from_slice(&[2; 32]).expect("fixed Taker key")
}

fn pubkey_hash(key: &SecretKey) -> [u8; 20] {
    let public_key = PublicKey::from_secret_key(&Secp256k1::new(), key);
    match lez_zec_swap_sdk::p2pkh_address(&public_key) {
        TransparentAddress::PublicKeyHash(hash) => hash,
        TransparentAddress::ScriptHash(_) => unreachable!("public keys produce P2PKH"),
    }
}

fn rpc_url() -> String {
    std::env::var("ZEBRA_RPC_URL").expect("ZEBRA_RPC_URL is supplied by scripts/run-zebra-e2e.sh")
}

fn client() -> HttpClient {
    HttpClientBuilder::default()
        .request_timeout(Duration::from_mins(2))
        .build(rpc_url())
        .expect("isolated Zebra RPC URL is valid")
}

fn typed_rpc() -> HttpZebraRpc {
    HttpZebraRpc::connect(&HttpZebraRpcConfig::new(rpc_url())).expect("loopback Zebra RPC")
}

async fn block_count(client: &HttpClient) -> u32 {
    client
        .request("getblockcount", rpc_params![])
        .await
        .expect("getblockcount succeeds")
}

async fn generate_to(client: &HttpClient, target: u32) {
    loop {
        let current = block_count(client).await;
        if current >= target {
            break;
        }
        let batch = (target - current).min(10);
        let _: Vec<String> = client
            .request("generate", rpc_params![batch])
            .await
            .expect("Regtest block generation succeeds");
    }
}

async fn mine_one(client: &HttpClient) {
    generate_to(client, block_count(client).await + 1).await;
}

async fn mempool(client: &HttpClient) -> Vec<String> {
    client
        .request("getrawmempool", rpc_params![])
        .await
        .expect("Zebra returns its mempool")
}

/// A mature miner output that no earlier step of this run spent.
async fn miner_utxo(client: &HttpClient, skip: &[OutPoint]) -> TransparentUtxo {
    let tip = block_count(client).await;
    let response: Value = client
        .request(
            "getaddressutxos",
            rpc_params![json!({"addresses": [MINER_ADDRESS], "chaininfo": true})],
        )
        .await
        .expect("Zebra returns the miner's transparent UTXOs");
    for entry in response
        .as_array()
        .expect("getaddressutxos returns an array")
    {
        let height = entry["height"].as_u64().expect("UTXO has a height");
        if height + 100 > u64::from(tip) {
            continue;
        }
        let transaction_id = entry["txid"].as_str().expect("UTXO has a txid");
        let index = u32::try_from(
            entry["outputIndex"]
                .as_u64()
                .expect("UTXO has an output index"),
        )
        .expect("output index fits u32");
        let raw: String = client
            .request("getrawtransaction", rpc_params![transaction_id, 0])
            .await
            .expect("coinbase transaction is available by id");
        let transaction = Transaction::read(
            hex::decode(raw).expect("transaction hex").as_slice(),
            BranchId::Nu7,
        )
        .expect("NU7 coinbase decodes");
        let outpoint = OutPoint::new(*transaction.txid().as_ref(), index);
        if skip.contains(&outpoint) {
            continue;
        }
        let output = transaction
            .transparent_bundle()
            .expect("coinbase pays a transparent miner output")
            .vout[usize::try_from(index).expect("u32 fits usize")]
        .clone();
        return TransparentUtxo::new(outpoint, output);
    }
    panic!("the deterministic miner has no unused mature output")
}

async fn try_broadcast(client: &HttpClient, transaction: &Transaction) -> Result<String, String> {
    let mut bytes = Vec::new();
    transaction.write(&mut bytes).expect("serialize");
    client
        .request("sendrawtransaction", rpc_params![hex::encode(bytes)])
        .await
        .map_err(|error| error.to_string())
}

async fn broadcast(client: &HttpClient, transaction: &Transaction) -> String {
    let transaction_id = try_broadcast(client, transaction)
        .await
        .expect("Zebra accepts the consensus-valid transaction");
    assert_eq!(transaction_id, transaction.txid().to_string());
    transaction_id
}

/// A dual-signed `TakerSellsLez` agreement under deterministic-local-v3 whose Zcash anchor is
/// the next block: the Maker's HTLC refunds 8 blocks later, before the Taker's LEZ refund.
fn agreement(label: &str, anchor: u32, secret_digest: [u8; 32]) -> ZecAgreementV1 {
    let secp = Secp256k1::new();
    let maker = PublicKey::from_secret_key(&secp, &maker_key()).serialize();
    let taker = PublicKey::from_secret_key(&secp, &taker_key()).serialize();
    let refund_hash = pubkey_hash(&maker_key());
    let claimant_hash = pubkey_hash(&taker_key());
    let profile = ZecRefundProfile::for_id(PROFILE);
    let contract = Bip199Contract::for_profile(
        PROFILE,
        anchor + profile.zcash_refund_blocks(),
        refund_hash,
        secret_digest,
        claimant_hash,
    );
    let binding = ZecSwapBinding::new(
        PROFILE,
        lez_zec_swap_sdk::ExpectedBip199Output::new(
            NetworkType::Regtest,
            BranchId::Nu7,
            Zatoshis::from_u64(PRINCIPAL).expect("principal"),
            contract,
        ),
    )
    .expect("deterministic-local-v3 binds Regtest NU7");
    let id = format!("zebra-regtest-reveal-{label}-{anchor}");
    let escrow_program = [1; 8];
    let onchain_id = derive_lez_swap_id_v1(id.as_bytes());
    let lez_deadline = 100 + profile.lez_refund_delay().value();
    let body = ZecAgreementBodyV1::new(
        id,
        SwapDirection::TakerSellsLez,
        ZecProfileRecordV1::from(PROFILE),
        ZecParticipantsV1::new(
            ZecParticipantIdentityV1::new([3; 32], maker),
            ZecParticipantIdentityV1::new([4; 32], taker),
        ),
        secret_digest,
        ZecLezTermsV1::new(
            LezChainIdentityV1::new(LezEnvironmentV1::DeterministicLocalV0_2, [8; 32], [7; 32]),
            escrow_program,
            LezAssetV1::Native {
                authenticated_transfer_program_id: [2; 8],
            },
            42,
            derive_lez_metadata_account_v1(&escrow_program, &onchain_id),
            derive_lez_native_custody_account_v1(&escrow_program, &onchain_id),
        ),
        ZecSwapBindingRecordV1::from_binding(&binding),
        ZecTransactionPolicyV1::new(
            [12; 32],
            ZcashTransparentDestinationV1::p2pkh(refund_hash),
            10_000,
            1_000,
            ZcashTransparentDestinationV1::p2pkh(claimant_hash),
            10_000,
            ZcashTransparentDestinationV1::p2pkh(refund_hash),
            10_000,
            profile.expiry_delta_blocks(),
        ),
        // Regtest declares the earlier (Zcash) refund's latest time as the LEZ deadline
        // minus the margin; the later refund is the LEZ deadline itself.
        ZecRefundPlanV1::new(
            100,
            anchor,
            (lez_deadline - profile.required_margin().value()) * 1_000,
            lez_deadline,
        ),
        NegotiationTranscriptV1::new([5; 32], [6; 32], 1_000),
    );
    let commitment = body.commitment();
    let record = ZecAgreementRecordV1::from_parts(
        ZEC_CONCRETE_AGREEMENT_SCHEMA_V2,
        body,
        commitment,
        secp.sign_ecdsa(&Message::from_digest(commitment), &maker_key())
            .serialize_compact(),
        secp.sign_ecdsa(&Message::from_digest(commitment), &taker_key())
            .serialize_compact(),
    );
    ZecAgreementV1::from_wire_at(
        &record.encode_wire().expect("bounded agreement"),
        UnixSeconds::new(10),
    )
    .expect("valid TakerSellsLez agreement")
}

fn maker_funding(agreement: &ZecAgreementV1, utxo: TransparentUtxo, tip: u32) -> Transaction {
    let expected = agreement.binding().expected_output();
    let request = TransparentFundingRequest::new(
        vec![utxo],
        PublicKey::from_secret_key(&Secp256k1::new(), &maker_key()),
        expected.value(),
        Zatoshis::from_u64(10_000).expect("fee"),
        Zatoshis::from_u64(10_000).expect("change floor"),
        BlockHeight::from_u32(tip + 40),
        BranchId::Nu7,
    )
    .expect("the Maker owns a mature output");
    build_funding_transaction(expected.contract(), &request, &maker_key())
        .expect("canonical Maker funding")
}

#[derive(Clone, Debug, thiserror::Error)]
#[error("Regtest LEZ stand-in error")]
struct StandInError;

#[derive(Clone)]
struct FixedDiscovery;

#[async_trait]
impl OfferDiscovery for FixedDiscovery {
    type Error = StandInError;
    type Offer = ();
    type OfferRef = ();
    type Query = ();

    async fn publish(&self, offer: Self::Offer) -> Result<Self::OfferRef, Self::Error> {
        Ok(offer)
    }

    async fn discover(&self, _query: &Self::Query) -> Result<Vec<Self::OfferRef>, Self::Error> {
        Ok(vec![()])
    }
}

#[derive(Clone)]
struct FixedNegotiation(Vec<u8>);

#[async_trait]
impl NegotiationChannel for FixedNegotiation {
    type Error = StandInError;
    type LocalProposal = ();
    type OfferRef = ();

    async fn negotiate(
        &self,
        _local_participant: Participant,
        _offer: &Self::OfferRef,
        _proposal: Self::LocalProposal,
    ) -> Result<Vec<u8>, Self::Error> {
        Ok(self.0.clone())
    }
}

/// LEZ is outside this harness; in `TakerSellsLez` the Taker never claims on LEZ, and the
/// Maker's LEZ claim that follows the reveal is not yet visible.
#[derive(Clone)]
struct NoLez;

#[async_trait]
impl LezMakerLockObservationPort for NoLez {
    type Error = StandInError;

    async fn observe_maker_lock(
        &self,
        _agreement: &ZecAgreementV1,
    ) -> Result<MakerLockObservationV1, Self::Error> {
        panic!("TakerSellsLez: the Maker locks on Zcash")
    }
}

#[async_trait]
impl LezClaimPort for NoLez {
    type Error = StandInError;

    async fn prepare_revealing_claim(
        &self,
        _agreement: &ZecAgreementV1,
        _preimage: &ClaimPreimage,
    ) -> Result<PreparedClaimSubmissionV1, Self::Error> {
        panic!("TakerSellsLez: the Taker never claims on LEZ")
    }

    async fn observe_prepared_revealing_claim(
        &self,
        _agreement: &ZecAgreementV1,
        _prepared: &PreparedClaimSubmissionV1,
    ) -> Result<RevealingClaimObservationV1, Self::Error> {
        panic!("TakerSellsLez: the Taker never claims on LEZ")
    }

    async fn observe_counterparty_revealing_claim(
        &self,
        _agreement: &ZecAgreementV1,
    ) -> Result<RevealingClaimObservationV1, Self::Error> {
        Ok(RevealingClaimObservationV1::Absent)
    }

    async fn submit_revealing_claim(
        &self,
        _agreement: &ZecAgreementV1,
        _prepared: &PreparedClaimSubmissionV1,
    ) -> Result<RevealingClaimSubmitOutcomeV1, Self::Error> {
        panic!("TakerSellsLez: the Taker never claims on LEZ")
    }
}

type TakerSwap = ActiveZecSwap<
    NoLez,
    ZebraRpcZcashPort<HttpZebraRpc, RoleKeyedZcashSigner>,
    SqliteZecRecoveryStore,
>;

/// The Taker's swap after its LEZ lock and the Maker's real Zcash funding: `BothLegsLocked`.
async fn taker_at_both_legs_locked(
    client: &HttpClient,
    data: &TempDir,
    label: &str,
    secret: [u8; 32],
    spent: &mut Vec<OutPoint>,
) -> (TakerSwap, Transaction) {
    let tip = block_count(client).await;
    let agreement = agreement(label, tip + 1, Sha256::digest(secret).into());
    let wire = agreement.encode_wire().expect("bounded agreement wire");
    let zcash = ZebraRpcZcashPort::new(
        typed_rpc(),
        RoleKeyedZcashSigner::new(Participant::Taker, taker_key()),
        ZebraChainIdentity::deterministic_regtest_nu7(),
        Participant::Taker,
    )
    .expect("Taker-bound Zcash port");
    let store = SqliteZecRecoveryStore::open_claim_capable(
        data.path().join(format!("{label}-taker.sqlite3")),
        Participant::Taker,
        ProtectedClaimKey::new("zebra-regtest-reveal-v1", [0x7d; 32]).expect("test key"),
    )
    .expect("claim-capable Taker store");
    let sdk = ZecPairSdk::new(
        Participant::Taker,
        FixedDiscovery,
        FixedNegotiation(wire),
        NoLez,
        zcash,
        store,
    );
    let accepted = sdk
        .negotiate_at(&(), (), UnixSeconds::new(10))
        .await
        .expect("authentic agreement");
    let mut active = sdk
        .activate_with_claim_preimage(accepted, ClaimPreimage::new(secret))
        .await
        .expect("the Taker holds the preimage");
    active
        .stage_first_lock(
            FirstLockPlanV1::lez(
                PreparedFirstLockSubmissionV1::new(
                    FirstLockStepV1::LezInitialize,
                    [0x41; 32],
                    b"initialize".to_vec(),
                )
                .expect("initialize"),
                PreparedFirstLockSubmissionV1::new(
                    FirstLockStepV1::LezFund,
                    [0x42; 32],
                    b"fund".to_vec(),
                )
                .expect("fund"),
            )
            .expect("TakerSellsLez first lock is on LEZ"),
        )
        .await
        .expect("durable LEZ first-lock intent");
    active
        .project_first_lock(
            FirstLockConfirmedEvidenceV1::new(
                FirstLockStepV1::LezFund,
                [0x42; 32],
                hex::encode([0x42; 32]),
                1,
            )
            .expect("LEZ lock evidence"),
        )
        .await
        .expect("Taker LEZ lock projected");

    let utxo = miner_utxo(client, spent).await;
    spent.push(utxo.outpoint().clone());
    let funding = maker_funding(active.agreement(), utxo, tip);
    broadcast(client, &funding).await;
    mine_one(client).await;
    active
        .observe_maker_lock()
        .await
        .expect("the Taker observes the Maker's Zcash funding on Zebra");
    assert_eq!(active.status(), Phase::BothLegsLocked);
    (active, funding)
}

fn maker_observer() -> ZebraRpcClaimPort<HttpZebraRpc, RoleKeyedZcashSigner> {
    ZebraRpcClaimPort::new(
        typed_rpc(),
        RoleKeyedZcashSigner::new(Participant::Maker, maker_key()),
        ZebraChainIdentity::deterministic_regtest_nu7(),
    )
}

#[tokio::test]
#[ignore = "requires scripts/run-zebra-e2e.sh and pinned Docker Zebra"]
async fn maker_reads_the_preimage_from_the_taker_zcash_claim_on_zebra() {
    const SECRET: [u8; 32] = [0x5c; 32];
    let client = client();
    generate_to(&client, 110).await;
    let data = TempDir::new().expect("isolated stores");
    let mut spent = Vec::new();
    let (mut taker, funding) =
        taker_at_both_legs_locked(&client, &data, "claim", SECRET, &mut spent).await;
    let agreement = taker.agreement().clone();
    let context = taker
        .zcash_claim_context()
        .expect("the revealing Zcash claim context exists at BothLegsLocked");
    let maker = maker_observer();
    assert!(matches!(
        maker
            .observe_counterparty_revealing_claim(&agreement, &context)
            .await
            .expect("unspent HTLC"),
        RevealingClaimObservationV1::Absent
    ));

    // The reveal gate asks for two confirmations of the Maker's funding.
    mine_one(&client).await;
    let outcome = taker
        .drive_claim(LezUnixMilliseconds::new(NOW_INSIDE_CLAIM_WINDOW))
        .await
        .expect("the Taker sends its revealing Zcash claim");
    assert_eq!(
        outcome,
        ClaimDriveOutcome::Submitted(ClaimStepV1::RevealingZcash)
    );
    let pending = mempool(&client).await;
    assert_eq!(pending.len(), 1, "exactly the Taker's claim is pending");
    // The Maker reads the preimage from the mempool claim; it does not wait for depth.
    let RevealingClaimObservationV1::Confirmed(in_mempool) = maker
        .observe_counterparty_revealing_claim(&agreement, &context)
        .await
        .expect("mempool claim")
    else {
        panic!("the mempool claim reveals the preimage")
    };
    assert_eq!(in_mempool.preimage().expose_secret(), &SECRET);
    assert_eq!(in_mempool.confirmations(), 0);
    assert_eq!(in_mempool.transaction_id(), pending[0]);

    mine_one(&client).await;
    let RevealingClaimObservationV1::Confirmed(evidence) = maker
        .observe_counterparty_revealing_claim(&agreement, &context)
        .await
        .expect("canonical claim at depth")
    else {
        panic!("the mined claim is confirmed revealing evidence")
    };
    assert_eq!(evidence.preimage().expose_secret(), &SECRET);
    assert_eq!(
        <[u8; 32]>::from(Sha256::digest(evidence.preimage().expose_secret())),
        *agreement.secret_digest()
    );
    assert_eq!(evidence.transaction_id(), pending[0]);
    let claimed: Value = client
        .request("getrawtransaction", rpc_params![&pending[0], 1])
        .await
        .expect("claim is indexed");
    assert_eq!(
        claimed["vin"][0]["txid"],
        funding.txid().to_string(),
        "the claim spends the Maker's exact HTLC outpoint"
    );

    let committed = taker
        .drive_claim(LezUnixMilliseconds::new(NOW_INSIDE_CLAIM_WINDOW))
        .await
        .expect("the Taker commits its own revealing claim");
    assert!(
        matches!(
            committed,
            ClaimDriveOutcome::Projected {
                step: ClaimStepV1::RevealingZcash,
                ..
            }
        ),
        "{committed:?}"
    );
    assert_eq!(taker.status(), Phase::ClaimEvidenceAvailable);
    eprintln!(
        "Maker read the preimage from Taker Zcash claim {} at {} confirmation(s)",
        evidence.transaction_id(),
        evidence.confirmations()
    );
}

/// A Taker that claims with its own software: the claim pays another destination, pays
/// another fee and has expiry height 0. Zebra accepts it; the Maker reads the preimage from it
/// in the mempool and in its block.
#[tokio::test]
#[ignore = "requires scripts/run-zebra-e2e.sh and pinned Docker Zebra"]
async fn maker_reads_the_preimage_from_a_non_template_taker_claim_on_zebra() {
    const SECRET: [u8; 32] = [0x7e; 32];
    const OTHER_FEE: u64 = 15_000;
    let client = client();
    generate_to(&client, 110).await;
    let data = TempDir::new().expect("isolated stores");
    let mut spent = Vec::new();
    let (taker, funding) =
        taker_at_both_legs_locked(&client, &data, "foreign", SECRET, &mut spent).await;
    let agreement = taker.agreement().clone();
    let context = taker
        .zcash_claim_context()
        .expect("the revealing Zcash claim context exists at BothLegsLocked");
    let template = agreement
        .claim_spend_request(
            context.funding_outpoint().clone(),
            funding.transparent_bundle().expect("funding").vout
                [usize::try_from(context.funding_outpoint().n()).expect("u32 fits usize")]
            .clone(),
            BlockHeight::from_u32(block_count(&client).await),
        )
        .expect("the SDK's claim request");
    let other_destination = TransparentAddress::PublicKeyHash(pubkey_hash(
        &SecretKey::from_slice(&[9; 32]).expect("another key"),
    ));
    assert_ne!(other_destination, template.destination());
    assert_ne!(u64::from(template.fee()), OTHER_FEE);
    let request = TransparentSpendRequest::new(
        agreement.binding().expected_output().contract(),
        context.funding_outpoint().clone(),
        template.funding_output().clone(),
        other_destination,
        Zatoshis::from_u64(OTHER_FEE).expect("fee"),
        BlockHeight::from_u32(0),
        BranchId::Nu7,
    )
    .expect("a claim request outside the agreement's policy");
    let claim = build_claim_transaction(
        agreement.binding().expected_output().contract(),
        &request,
        &taker_key(),
        &SECRET,
    )
    .expect("the Taker signs its own claim");
    let claim_id = broadcast(&client, &claim).await;

    let maker = maker_observer();
    let RevealingClaimObservationV1::Confirmed(in_mempool) = maker
        .observe_counterparty_revealing_claim(&agreement, &context)
        .await
        .expect("the non-template claim in the mempool")
    else {
        panic!("the mempool claim reveals the preimage")
    };
    assert_eq!(in_mempool.preimage().expose_secret(), &SECRET);
    assert_eq!(
        (in_mempool.transaction_id(), in_mempool.confirmations()),
        (claim_id.as_str(), 0)
    );

    mine_one(&client).await;
    let mined: Value = client
        .request("getrawtransaction", rpc_params![&claim_id, 1])
        .await
        .expect("claim is indexed");
    assert_eq!(
        mined["confirmations"], 1,
        "Zebra mined the non-template claim"
    );
    assert_eq!(mined["expiryheight"], 0);
    assert_eq!(mined["vin"][0]["txid"], funding.txid().to_string());
    let RevealingClaimObservationV1::Confirmed(in_block) = maker
        .observe_counterparty_revealing_claim(&agreement, &context)
        .await
        .expect("the non-template claim in a block")
    else {
        panic!("the mined claim reveals the preimage")
    };
    assert_eq!(in_block.preimage().expose_secret(), &SECRET);
    assert_eq!(
        (in_block.transaction_id(), in_block.confirmations()),
        (claim_id.as_str(), 1)
    );
    eprintln!(
        "Maker read the preimage from non-template Taker claim {claim_id} (fee {OTHER_FEE}, expiry 0)"
    );
}

#[tokio::test]
#[ignore = "requires scripts/run-zebra-e2e.sh and pinned Docker Zebra"]
async fn maker_refund_of_the_htlc_is_classified_as_refund_on_zebra() {
    const SECRET: [u8; 32] = [0x6d; 32];
    let client = client();
    generate_to(&client, 110).await;
    let data = TempDir::new().expect("isolated stores");
    let mut spent = Vec::new();
    let (mut taker, _funding) =
        taker_at_both_legs_locked(&client, &data, "refund", SECRET, &mut spent).await;
    let agreement = taker.agreement().clone();
    let claim_context = taker.zcash_claim_context().expect("claim context");
    let funding_context = taker.zcash_funding_context().expect("funding context");
    let refund_height = agreement.zcash_refund_at_height();

    // The Taker's reveal gate closes `zcash_reveal_margin_blocks` before the Maker's refund.
    let margin = ZecRefundProfile::for_id(PROFILE).zcash_reveal_margin_blocks();
    generate_to(&client, refund_height - margin).await;
    assert!(matches!(
        taker
            .drive_claim(LezUnixMilliseconds::new(NOW_INSIDE_CLAIM_WINDOW))
            .await,
        Err(ZecSdkError::ZcashRevealWindowClosed { .. })
    ));
    assert!(mempool(&client).await.is_empty(), "no claim was sent");

    generate_to(&client, refund_height).await;
    let refund_port: ZebraRpcRefundPort<HttpZebraRpc, RoleKeyedZcashSigner> = maker_observer();
    let refund = refund_port
        .prepare_refund(&agreement, &funding_context)
        .await
        .expect("the Maker's refund is mature");
    assert_eq!(
        refund_port
            .submit_refund(&agreement, &funding_context, &refund)
            .await
            .expect("refund broadcast"),
        RefundSubmitOutcomeV1::Accepted
    );
    let maker = maker_observer();
    assert!(matches!(
        maker
            .observe_counterparty_revealing_claim(&agreement, &claim_context)
            .await,
        Err(ZebraClaimError::WrongSpendBranch)
    ));

    mine_one(&client).await;
    assert!(matches!(
        maker
            .observe_counterparty_revealing_claim(&agreement, &claim_context)
            .await,
        Err(ZebraClaimError::WrongSpendBranch)
    ));
    let taker_view = ZebraRpcRefundPort::new(
        typed_rpc(),
        RoleKeyedZcashSigner::new(Participant::Taker, taker_key()),
        ZebraChainIdentity::deterministic_regtest_nu7(),
    );
    assert!(matches!(
        taker_view
            .observe_counterparty_refund(&agreement, &funding_context)
            .await
            .expect("the Taker sees the Maker's refund"),
        RefundObservationV1::Confirmed(_)
    ));
    let refund_id = TxId::from_bytes(*refund.expected_submission_id());
    eprintln!("Maker refund {refund_id} classified as the refund branch, no preimage");
}

/// The LEZ escrow takes a 32-byte preimage only, so the Maker can claim LEZ only with one.
/// A preimage of another length whose SHA-256 is the digest must therefore not spend the
/// Maker's HTLC: Zebra refuses a 33-byte and a 31-byte claim, the HTLC stays unspent, and the
/// Maker refunds it at its refund height. The 32-byte claim is the one the other tests send.
#[tokio::test]
#[ignore = "requires scripts/run-zebra-e2e.sh and pinned Docker Zebra"]
async fn htlc_refuses_a_claim_whose_preimage_is_not_32_bytes_on_zebra() {
    let client = client();
    generate_to(&client, 110).await;
    let mut spent = Vec::new();
    let mut htlcs = Vec::new();
    for (label, length) in [("long", 33_usize), ("short", 31)] {
        let preimage = vec![0x3a; length];
        let tip = block_count(&client).await;
        let agreement = agreement(label, tip + 1, Sha256::digest(&preimage).into());
        let utxo = miner_utxo(&client, &spent).await;
        spent.push(utxo.outpoint().clone());
        let funding = maker_funding(&agreement, utxo, tip);
        broadcast(&client, &funding).await;
        mine_one(&client).await;
        htlcs.push((agreement, funding, preimage));
    }

    for (agreement, funding, preimage) in &htlcs {
        let contract = agreement.binding().expected_output().contract();
        assert_eq!(contract.preimage_rule(), Bip199PreimageRule::Exactly32Bytes);
        let outpoint = OutPoint::new(*funding.txid().as_ref(), 0);
        let funding_output = funding.transparent_bundle().expect("funding").vout[0].clone();
        assert_eq!(
            funding_output.script_pubkey().0.0,
            contract.p2sh_script_pubkey()
        );
        let request = agreement
            .claim_spend_request(
                outpoint.clone(),
                funding_output.clone(),
                BlockHeight::from_u32(block_count(&client).await),
            )
            .expect("the agreement's claim request");
        let claim = build_claim_transaction(contract, &request, &taker_key(), preimage)
            .expect("the claim is signed; only the script decides");
        let refused = try_broadcast(&client, &claim).await;
        assert!(
            refused.is_err(),
            "Zebra accepted a claim with a {}-byte preimage: {refused:?}",
            preimage.len()
        );
        eprintln!(
            "Zebra refused the {}-byte preimage claim: {}",
            preimage.len(),
            refused.expect_err("refused")
        );
    }
    mine_one(&client).await;

    // The HTLC is still the Maker's: it refunds at its refund height.
    let (agreement, funding, _) = &htlcs[1];
    let refund_height = agreement.zcash_refund_at_height();
    generate_to(&client, refund_height).await;
    let contract = agreement.binding().expected_output().contract();
    let request = agreement
        .refund_spend_request(
            OutPoint::new(*funding.txid().as_ref(), 0),
            funding.transparent_bundle().expect("funding").vout[0].clone(),
            BlockHeight::from_u32(block_count(&client).await),
        )
        .expect("the agreement's refund request");
    let refund = build_refund_transaction(contract, &request, &maker_key())
        .expect("the Maker signs its refund");
    let refund_id = broadcast(&client, &refund).await;
    mine_one(&client).await;
    let mined: Value = client
        .request("getrawtransaction", rpc_params![&refund_id, 1])
        .await
        .expect("refund is indexed");
    assert_eq!(mined["confirmations"], 1, "Zebra mined the Maker's refund");
    eprintln!("Maker refunded the HTLC a short preimage could not claim: {refund_id}");
}
