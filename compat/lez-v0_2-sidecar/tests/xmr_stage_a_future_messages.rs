use std::sync::Arc;

use async_trait::async_trait;
use borsh::BorshDeserialize as _;
use lez_bridge_protocol::{
    AggregateBip340Signature, CompleteNativeXmrClaimV3Request, Hex32, MessageContext, Participant,
    PrepareNativeXmrClaimV3Request, RequestId, RunId, RuntimeCompatibility, RuntimeDescriptor,
    XmrNativeEscrowTermsV3, XmrNativeEscrowTermsV3Input,
};
use lez_v0_2_sidecar::{
    M4StageAFinalizedNonces, M4StageAFutureMessageInput, M4StageAFutureMessagePlanError,
    NativeEscrowPlanner, NativePrepareError, NonceSource, ZecEscrowInstruction,
    compute_custody_pda, compute_metadata_pda, decode_official_public_transaction,
    plan_m4_stage_a_future_messages, program_id_to_hex,
};
use nssa::{AccountId, PrivateKey, PublicKey, Signature, program::Program};

const PROGRAM: [u32; 8] = [0x1020_3040; 8];
const TRANSFER_PROGRAM: [u32; 8] = [0x5060_7080; 8];
const SWAP_ID: [u8; 32] = [0x51; 32];

fn identity(byte: u8) -> (AccountId, PublicKey) {
    let private = PrivateKey::try_new([byte; 32]).expect("valid fixture scalar");
    let public = PublicKey::new_from_private_key(&private);
    (AccountId::from(&public), public)
}

#[test]
fn exact_future_messages_bind_generated_accounts_signers_and_planned_nonce_order() {
    let (maker, _) = identity(11);
    let (taker, _) = identity(12);
    let (claim_authority, claim_key) = identity(13);
    let (refund_authority, refund_key) = identity(14);

    let plan = plan_m4_stage_a_future_messages(M4StageAFutureMessageInput::new(
        PROGRAM,
        SWAP_ID,
        maker,
        taker,
        *claim_key.value(),
        *refund_key.value(),
        M4StageAFinalizedNonces::new(1, 1, 1, 1),
    ))
    .expect("exact Stage-A future-message plan");

    assert_eq!(plan.claim_authority(), claim_authority);
    assert_eq!(plan.refund_authority(), refund_authority);
    assert_eq!(plan.nonces().maker_owner_finalized(), 1);
    assert_eq!(plan.nonces().taker_owner_finalized(), 1);
    assert_eq!(plan.nonces().initialize(), 1);
    assert_eq!(plan.nonces().fund(), 2);
    assert_eq!(plan.nonces().authorize(), 3);
    assert_eq!(plan.nonces().claim(), 1);
    assert_eq!(plan.nonces().refund(), 1);
    assert_eq!(plan.nonces().punish(), 1);

    let metadata = compute_metadata_pda(&PROGRAM, &SWAP_ID);
    let custody = compute_custody_pda(&PROGRAM, &SWAP_ID);
    assert_eq!(
        plan.claim_message().account_ids,
        [metadata, custody, maker, claim_authority]
    );
    assert_eq!(plan.claim_message().nonces, [1_u128.into()]);
    assert_eq!(
        plan.refund_message().account_ids,
        [metadata, custody, taker, refund_authority]
    );
    assert_eq!(plan.refund_message().nonces, [1_u128.into()]);
    assert_eq!(
        plan.punish_message().account_ids,
        [metadata, custody, maker]
    );
    assert_eq!(plan.punish_message().nonces, [1_u128.into()]);

    for (message, expected) in [
        (
            plan.claim_message(),
            ZecEscrowInstruction::ClaimNativeXmr { swap_id: SWAP_ID },
        ),
        (
            plan.refund_message(),
            ZecEscrowInstruction::RefundNativeXmr { swap_id: SWAP_ID },
        ),
        (
            plan.punish_message(),
            ZecEscrowInstruction::PunishNativeXmr { swap_id: SWAP_ID },
        ),
    ] {
        assert_eq!(
            message.instruction_data,
            Program::serialize_instruction(expected)
                .expect("checked generated instruction encoding")
        );
    }

    assert_eq!(plan.claim_hash(), plan.claim_message().hash());
    assert_eq!(plan.refund_hash(), plan.refund_message().hash());
    assert_eq!(plan.punish_hash(), plan.punish_message().hash());
    assert_ne!(plan.claim_hash(), plan.refund_hash());
    assert_ne!(plan.claim_hash(), plan.punish_hash());
    assert_ne!(plan.refund_hash(), plan.punish_hash());

    let claim_round_trip = nssa::public_transaction::Message::try_from_slice(
        &borsh::to_vec(plan.claim_message()).expect("official message bytes"),
    )
    .expect("canonical official message");
    assert_eq!(claim_round_trip, *plan.claim_message());
}
fn full_identity(byte: u8) -> (AccountId, PrivateKey, PublicKey) {
    let private = PrivateKey::try_new([byte; 32]).expect("valid fixture scalar");
    let public = PublicKey::new_from_private_key(&private);
    (AccountId::from(&public), private, public)
}

const fn h(byte: u8) -> Hex32 {
    Hex32::from_bytes([byte; 32])
}

#[derive(Debug)]
struct ExactAuthorityNonce {
    authority: AccountId,
    nonce: u128,
}

#[async_trait]
impl NonceSource for ExactAuthorityNonce {
    async fn account_nonce(&self, account_id: AccountId) -> Result<u128, NativePrepareError> {
        if account_id != self.authority {
            return Err(NativePrepareError::NonceUnavailable);
        }
        Ok(self.nonce)
    }
}

#[tokio::test]
async fn existing_tag15_prepare_and_complete_accept_the_planned_claim_hash_and_message() {
    let (maker, maker_private, _) = full_identity(21);
    let (taker, _, _) = full_identity(22);
    let (claim_authority, claim_private, claim_public) = full_identity(23);
    let (refund_authority, _, refund_public) = full_identity(24);
    let plan = plan_m4_stage_a_future_messages(M4StageAFutureMessageInput::new(
        PROGRAM,
        SWAP_ID,
        maker,
        taker,
        *claim_public.value(),
        *refund_public.value(),
        M4StageAFinalizedNonces::new(1, 1, 1, 1),
    ))
    .expect("exact future-message plan");

    let terms = XmrNativeEscrowTermsV3::new(XmrNativeEscrowTermsV3Input {
        swap_id: Hex32::from_bytes(SWAP_ID),
        activation_commitment: h(2),
        escrow_program_id: program_id_to_hex(PROGRAM),
        authenticated_transfer_program_id: program_id_to_hex(TRANSFER_PROGRAM),
        metadata_account_id: Hex32::from_bytes(
            compute_metadata_pda(&PROGRAM, &SWAP_ID).into_value(),
        ),
        custody_account_id: Hex32::from_bytes(compute_custody_pda(&PROGRAM, &SWAP_ID).into_value()),
        depositor: Participant::Taker,
        depositor_account_id: Hex32::from_bytes(taker.into_value()),
        claimant: Participant::Maker,
        claimant_account_id: Hex32::from_bytes(maker.into_value()),
        claim_aggregate_x_only_public_key: Hex32::from_bytes(*claim_public.value()),
        claim_authority_account_id: Hex32::from_bytes(claim_authority.into_value()),
        refund_aggregate_x_only_public_key: Hex32::from_bytes(*refund_public.value()),
        refund_authority_account_id: Hex32::from_bytes(refund_authority.into_value()),
        maker_dleq_transcript_commitment: h(13),
        taker_dleq_transcript_commitment: h(14),
        claim_partial_context_binding: h(15),
        claim_partial_commitment: h(16),
        amount: 75,
        refund_at_ms: 10_000,
        punish_at_ms: 20_000,
        claim_message_hash: Hex32::from_bytes(plan.claim_hash()),
        refund_message_hash: Hex32::from_bytes(plan.refund_hash()),
        punish_message_hash: Hex32::from_bytes(plan.punish_hash()),
    })
    .expect("Stage-A hashes form exact v3 terms");
    let runtime = RuntimeDescriptor::new(
        Participant::Maker,
        RuntimeCompatibility::LeeV0_2_0,
        h(40),
        h(41),
        h(42),
        program_id_to_hex(PROGRAM),
        Hex32::from_bytes(maker.into_value()),
    );
    let planner = NativeEscrowPlanner::new(
        Participant::Maker,
        maker_private,
        PROGRAM,
        TRANSFER_PROGRAM,
        runtime.clone(),
        Arc::new(ExactAuthorityNonce {
            authority: claim_authority,
            nonce: plan.nonces().claim(),
        }),
    )
    .expect("Maker planner");
    let prepare_request = PrepareNativeXmrClaimV3Request::new(
        MessageContext::new(
            RunId::new("m4-stage-a-future-message-run").expect("run id"),
            RequestId::new("m4-stage-a-future-message-prepare").expect("request id"),
            Participant::Maker,
        ),
        runtime.clone(),
        terms,
    );
    let prepared = planner
        .prepare_native_xmr_claim_v3(&prepare_request)
        .await
        .expect("existing prepare builder accepts planned hash");
    let prepared_message = nssa::public_transaction::Message::try_from_slice(
        prepared.claim.exact_message_bytes.as_slice(),
    )
    .expect("canonical prepared message");
    assert_eq!(&prepared_message, plan.claim_message());
    assert_eq!(prepared_message.hash(), plan.claim_hash());
    planner
        .validate_prepared_native_xmr_claim_v3(&prepare_request, &prepared)
        .expect("existing validator accepts exact plan");

    let aggregate_signature = Signature::new(&claim_private, &plan.claim_hash());
    let complete_request = CompleteNativeXmrClaimV3Request::new(
        MessageContext::new(
            RunId::new("m4-stage-a-future-message-run").expect("run id"),
            RequestId::new("m4-stage-a-future-message-complete").expect("request id"),
            Participant::Maker,
        ),
        runtime,
        terms,
        prepared.claim,
        AggregateBip340Signature::from_bytes(aggregate_signature.value),
    )
    .expect("exact completion request");
    let completed = planner
        .complete_native_xmr_claim_v3(&complete_request)
        .await
        .expect("existing complete builder accepts planned hash");
    let transaction = decode_official_public_transaction(completed.claim.exact_bytes.as_slice())
        .expect("canonical completed transaction");
    assert_eq!(transaction.message(), plan.claim_message());
    assert_eq!(transaction.message().hash(), plan.claim_hash());
}

#[test]
fn aliases_invalid_keys_and_taker_predecessor_overflow_fail_closed() {
    let (maker, maker_public) = identity(31);
    let (taker, taker_public) = identity(32);
    let (_, claim_public) = identity(33);
    let (_, refund_public) = identity(34);

    assert_eq!(
        plan_m4_stage_a_future_messages(M4StageAFutureMessageInput::new(
            PROGRAM,
            SWAP_ID,
            maker,
            maker,
            *claim_public.value(),
            *refund_public.value(),
            M4StageAFinalizedNonces::new(1, 1, 1, 1),
        )),
        Err(M4StageAFutureMessagePlanError::InvalidIdentity)
    );
    assert_eq!(
        plan_m4_stage_a_future_messages(M4StageAFutureMessageInput::new(
            PROGRAM,
            SWAP_ID,
            AccountId::new([0; 32]),
            taker,
            *claim_public.value(),
            *refund_public.value(),
            M4StageAFinalizedNonces::new(1, 1, 1, 1),
        )),
        Err(M4StageAFutureMessagePlanError::InvalidIdentity)
    );
    assert_eq!(
        plan_m4_stage_a_future_messages(M4StageAFutureMessageInput::new(
            PROGRAM,
            SWAP_ID,
            maker,
            taker,
            *maker_public.value(),
            *refund_public.value(),
            M4StageAFinalizedNonces::new(1, 1, 1, 1),
        )),
        Err(M4StageAFutureMessagePlanError::InvalidIdentity)
    );
    assert_eq!(
        plan_m4_stage_a_future_messages(M4StageAFutureMessageInput::new(
            PROGRAM,
            SWAP_ID,
            maker,
            taker,
            [0; 32],
            *refund_public.value(),
            M4StageAFinalizedNonces::new(1, 1, 1, 1),
        )),
        Err(M4StageAFutureMessagePlanError::InvalidIdentity)
    );
    assert_eq!(
        plan_m4_stage_a_future_messages(M4StageAFutureMessageInput::new(
            PROGRAM,
            SWAP_ID,
            maker,
            taker,
            *claim_public.value(),
            *refund_public.value(),
            M4StageAFinalizedNonces::new(1, u128::MAX - 1, 1, 1),
        )),
        Err(M4StageAFutureMessagePlanError::NonceOverflow)
    );
    assert_ne!(maker_public, taker_public);
}
