use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bitcoin::{
    Amount, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid, Witness,
    absolute::LockTime,
    hashes::Hash as _,
    secp256k1::{Keypair, Message, PublicKey, Secp256k1, SecretKey},
    transaction::Version,
};
use lez_bridge_adapter::{
    CurrentLezFirstLockError, LezBridgeAdapter, LezBridgeCurrentEscrowTransport,
};
use lez_bridge_protocol::{
    ChainClock, EscrowState, Hex32, NativeCustodyFacts, NativeEscrowAccountFacts,
    NativeEscrowAccountObservation, NativeRefundObservation, NativeRefundObservationTarget,
    ObserveNativeRefundRequest, ObserveNativeRefundResult, Participant as BridgeParticipant,
    RequestId, RunId, RuntimeCompatibility, RuntimeDescriptor, WitnessedEscrowMetadataFacts,
};
use lez_btc_swap_sdk::{
    AdaptorSessionContext, BTC_AGREEMENT_SCHEMA_V1, BtcAgreementBodyV1, BtcAgreementRecordV1,
    BtcAgreementV1, BtcChainPolicyV1, BtcClaimTermsV1, BtcFundingTermsV1, BtcLezTermsV1,
    BtcP2trTermsV1, BtcParticipantIdentityV1, BtcParticipantsV1, BtcRecoveryPlanV1,
    CooperativeKeyPathSpend, CsvBlockDelay, P2trSwapOutput, RefundXOnlyKey, TwoPartyAggregateKey,
};
use lez_swap_core::{Participant, SwapDirection};

const LEZ_CHANNEL: [u8; 32] = [17; 32];
const LEZ_GENESIS: [u8; 32] = [18; 32];
const ESCROW_PROGRAM: [u8; 32] = [15; 32];
const TRANSFER_PROGRAM: [u8; 32] = [16; 32];
const METADATA_ACCOUNT: [u8; 32] = [13; 32];
const CUSTODY_ACCOUNT: [u8; 32] = [14; 32];
const MAKER_ACCOUNT: [u8; 32] = [10; 32];
const TAKER_ACCOUNT: [u8; 32] = [11; 32];
const LEZ_AMOUNT: u128 = 5_000;

#[derive(Clone, Copy, Debug)]
enum Mutation {
    None,
    ClockDrift,
    Claimed,
    MetadataAccount,
    CustodyAccount,
    CustodyOwner,
    CustodyValue,
    AccountsAbsent,
    RefundLookedUp,
}

#[derive(Clone, Debug)]
struct ReadOnlyTransport {
    mutation: Mutation,
    requests: Arc<Mutex<Vec<ObserveNativeRefundRequest>>>,
}

impl ReadOnlyTransport {
    fn new(mutation: Mutation) -> Self {
        Self {
            mutation,
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("test transport error")]
struct TestTransportError;

#[async_trait]
impl LezBridgeCurrentEscrowTransport for ReadOnlyTransport {
    type Error = TestTransportError;

    async fn observe_native_refund(
        &self,
        request: ObserveNativeRefundRequest,
    ) -> Result<ObserveNativeRefundResult, Self::Error> {
        self.requests
            .lock()
            .expect("request log")
            .push(request.clone());
        let terms = request.terms.witnessed().expect("witnessed terms");
        let state = if matches!(self.mutation, Mutation::Claimed) {
            EscrowState::Claimed
        } else {
            EscrowState::Funded
        };
        let metadata_account = if matches!(self.mutation, Mutation::MetadataAccount) {
            Hex32::from_bytes([0xa1; 32])
        } else {
            Hex32::from_bytes(METADATA_ACCOUNT)
        };
        let custody_account = if matches!(self.mutation, Mutation::CustodyAccount) {
            Hex32::from_bytes([0xa2; 32])
        } else {
            Hex32::from_bytes(CUSTODY_ACCOUNT)
        };
        let custody_owner = if matches!(self.mutation, Mutation::CustodyOwner) {
            Hex32::from_bytes([0xa3; 32])
        } else {
            terms.authenticated_transfer_program_id()
        };
        let custody_value = if matches!(self.mutation, Mutation::CustodyValue) {
            LEZ_AMOUNT - 1
        } else if state == EscrowState::Funded {
            LEZ_AMOUNT
        } else {
            0
        };
        let accounts = if matches!(self.mutation, Mutation::AccountsAbsent) {
            NativeEscrowAccountObservation::Absent
        } else {
            NativeEscrowAccountObservation::found(NativeEscrowAccountFacts::new_witnessed(
                WitnessedEscrowMetadataFacts::from_witnessed_native_terms(
                    metadata_account,
                    request.runtime.escrow_program_id,
                    custody_account,
                    terms,
                    state,
                ),
                NativeCustodyFacts::new(custody_account, custody_owner, custody_value),
            ))
        };
        let before = ChainClock::new(Hex32::from_bytes([0x90; 32]), 12, 200_000);
        let after = if matches!(self.mutation, Mutation::ClockDrift) {
            ChainClock::new(Hex32::from_bytes([0x91; 32]), 13, 201_000)
        } else {
            before
        };
        let refund = if matches!(self.mutation, Mutation::RefundLookedUp) {
            NativeRefundObservation::Absent
        } else {
            NativeRefundObservation::NotRequested
        };
        Ok(ObserveNativeRefundResult::new(
            request.context,
            before,
            accounts,
            refund,
            after,
        ))
    }
}

fn secret(value: u8) -> SecretKey {
    SecretKey::from_slice(&[value; 32]).expect("fixed secret")
}

fn public_key(secret: &SecretKey) -> [u8; 33] {
    PublicKey::from_secret_key(&Secp256k1::new(), secret).serialize()
}

fn x_only(secret: &SecretKey) -> [u8; 32] {
    Keypair::from_secret_key(&Secp256k1::new(), secret)
        .x_only_public_key()
        .0
        .serialize()
}

fn destination(secret: &SecretKey) -> Vec<u8> {
    ScriptBuf::new_p2tr(
        &Secp256k1::verification_only(),
        Keypair::from_secret_key(&Secp256k1::new(), secret)
            .x_only_public_key()
            .0,
        None,
    )
    .into_bytes()
}

fn sign(secret: &SecretKey, commitment: [u8; 32]) -> [u8; 64] {
    Secp256k1::new()
        .sign_schnorr_no_aux_rand(
            &Message::from_digest(commitment),
            &Keypair::from_secret_key(&Secp256k1::new(), secret),
        )
        .serialize()
}

#[allow(clippy::too_many_lines)]
fn agreement(direction: SwapDirection) -> BtcAgreementV1 {
    let maker = secret(1);
    let taker = secret(2);
    let participants = BtcParticipantsV1::new(
        BtcParticipantIdentityV1::new(
            MAKER_ACCOUNT,
            public_key(&maker),
            x_only(&secret(3)),
            destination(&secret(5)),
        ),
        BtcParticipantIdentityV1::new(
            TAKER_ACCOUNT,
            public_key(&taker),
            x_only(&secret(4)),
            destination(&secret(6)),
        ),
    );
    let adaptor_point = public_key(&secret(7));
    let aggregate = AdaptorSessionContext::untweaked(
        [public_key(&maker), public_key(&taker)],
        [30; 32],
        adaptor_point,
        [31; 32],
    )
    .expect("aggregate context")
    .output_key();
    let bitcoin_funder = match direction {
        SwapDirection::TakerSellsForeign => Participant::Taker,
        SwapDirection::TakerSellsLez => Participant::Maker,
    };
    let contract = P2trSwapOutput::new(
        TwoPartyAggregateKey::from_bytes(aggregate).expect("aggregate key"),
        RefundXOnlyKey::from_bytes(
            *participants
                .for_participant(bitcoin_funder)
                .bitcoin_refund_key(),
        )
        .expect("refund key"),
        CsvBlockDelay::new(144).expect("CSV"),
    )
    .expect("contract");
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
            value: Amount::from_sat(100_000),
            script_pubkey: ScriptBuf::from_bytes(contract.script_pubkey_bytes().to_vec()),
        }],
    };
    let claim = CooperativeKeyPathSpend::new(
        &contract,
        OutPoint {
            txid: funding.compute_txid(),
            vout: 0,
        },
        Amount::from_sat(100_000),
        vec![TxOut {
            value: Amount::from_sat(99_000),
            script_pubkey: ScriptBuf::from_bytes(
                participants
                    .for_participant(bitcoin_funder.other())
                    .claim_destination_script_pubkey()
                    .to_vec(),
            ),
        }],
    )
    .expect("claim");
    let lez_depositor = match direction {
        SwapDirection::TakerSellsForeign => Participant::Maker,
        SwapDirection::TakerSellsLez => Participant::Taker,
    };
    let lez_refund_at_ms = match direction {
        SwapDirection::TakerSellsForeign => 1_700_000_100_000,
        SwapDirection::TakerSellsLez => 1_700_000_500_000,
    };
    let body = BtcAgreementBodyV1::new(
        [20; 32],
        direction,
        BtcChainPolicyV1::new([8; 32], 6),
        participants.clone(),
        adaptor_point,
        BtcLezTermsV1::new(
            LEZ_CHANNEL,
            LEZ_GENESIS,
            ESCROW_PROGRAM,
            TRANSFER_PROGRAM,
            [12; 32],
            METADATA_ACCOUNT,
            CUSTODY_ACCOUNT,
            *participants
                .for_participant(lez_depositor)
                .lez_owner_account(),
            *participants
                .for_participant(lez_depositor.other())
                .lez_owner_account(),
            LEZ_AMOUNT,
            lez_refund_at_ms,
            [19; 32],
        ),
        BtcP2trTermsV1::from_contract(&contract),
        BtcFundingTermsV1::new(funding.compute_txid().to_byte_array(), 0, 100_000),
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
    BtcAgreementV1::validate(BtcAgreementRecordV1::from_parts(
        BTC_AGREEMENT_SCHEMA_V1,
        body,
        commitment,
        sign(&maker, commitment),
        sign(&taker, commitment),
    ))
    .expect("agreement")
}

fn runtime(role: Participant) -> RuntimeDescriptor {
    RuntimeDescriptor::new(
        match role {
            Participant::Maker => BridgeParticipant::Maker,
            Participant::Taker => BridgeParticipant::Taker,
        },
        RuntimeCompatibility::LeeV0_2_0,
        Hex32::from_bytes([9; 32]),
        Hex32::from_bytes(LEZ_CHANNEL),
        Hex32::from_bytes(LEZ_GENESIS),
        Hex32::from_bytes(ESCROW_PROGRAM),
        Hex32::from_bytes(match role {
            Participant::Maker => MAKER_ACCOUNT,
            Participant::Taker => TAKER_ACCOUNT,
        }),
    )
}

fn adapter(transport: ReadOnlyTransport, role: Participant) -> LezBridgeAdapter<ReadOnlyTransport> {
    LezBridgeAdapter::new(
        transport,
        RunId::new("btc-current-first-lock-run").expect("run ID"),
        runtime(role),
        role,
    )
    .expect("adapter")
}

#[tokio::test]
async fn both_roles_derive_the_reverse_first_lock_and_read_state_only_once() {
    let agreement = agreement(SwapDirection::TakerSellsLez);
    for role in [Participant::Maker, Participant::Taker] {
        let transport = ReadOnlyTransport::new(Mutation::None);
        let adapter = adapter(transport.clone(), role);
        let evidence = adapter
            .observe_current_lez_first_lock(
                &agreement,
                RequestId::new(format!("current-first-lock-{role:?}").to_lowercase())
                    .expect("request ID"),
            )
            .await
            .expect("current funded first lock");
        assert_eq!(evidence.clock().height, 12);
        assert_eq!(evidence.clock().timestamp_ms, 200_000);
        assert_eq!(evidence.metadata().account_id.as_bytes(), &METADATA_ACCOUNT);
        assert_eq!(evidence.custody().account_id.as_bytes(), &CUSTODY_ACCOUNT);
        assert_eq!(evidence.custody().balance.as_u128(), LEZ_AMOUNT);

        let requests = transport.requests.lock().expect("request log");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].context.sidecar_role, runtime(role).sidecar_role);
        assert_eq!(requests[0].terms.depositor(), BridgeParticipant::Taker);
        assert_eq!(requests[0].terms.claimant(), BridgeParticipant::Maker);
        assert_eq!(requests[0].target, NativeRefundObservationTarget::StateOnly);
    }
}

#[tokio::test]
async fn forward_direction_is_not_misrepresented_as_a_lez_first_lock() {
    let agreement = agreement(SwapDirection::TakerSellsForeign);
    for role in [Participant::Maker, Participant::Taker] {
        let transport = ReadOnlyTransport::new(Mutation::None);
        let error = adapter(transport.clone(), role)
            .observe_current_lez_first_lock(
                &agreement,
                RequestId::new(format!("wrong-direction-{role:?}").to_lowercase())
                    .expect("request ID"),
            )
            .await
            .expect_err("Bitcoin is the first lock in this direction");
        assert!(matches!(error, CurrentLezFirstLockError::WrongDirection));
        assert!(transport.requests.lock().expect("request log").is_empty());
    }
}

#[tokio::test]
async fn drift_spent_wrong_accounts_and_value_fail_closed() {
    let agreement = agreement(SwapDirection::TakerSellsLez);
    for (mutation, expected) in [
        (Mutation::ClockDrift, "clock"),
        (Mutation::Claimed, "funded"),
        (Mutation::MetadataAccount, "account"),
        (Mutation::CustodyAccount, "account"),
        (Mutation::CustodyOwner, "account"),
        (Mutation::CustodyValue, "value"),
        (Mutation::AccountsAbsent, "unavailable"),
        (Mutation::RefundLookedUp, "state-only"),
    ] {
        let transport = ReadOnlyTransport::new(mutation);
        let error = adapter(transport.clone(), Participant::Maker)
            .observe_current_lez_first_lock(
                &agreement,
                RequestId::new(format!("mutation-{mutation:?}").to_lowercase())
                    .expect("request ID"),
            )
            .await
            .expect_err("mutation must fail closed");
        assert!(
            error.to_string().contains(expected),
            "{mutation:?}: {error}"
        );
        assert_eq!(transport.requests.lock().expect("request log").len(), 1);
    }
}
