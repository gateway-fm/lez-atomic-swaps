use lez_swap_core::{SwapDirection, UnixSeconds};
use lez_zec_swap_sdk::{
    Bip199Contract, ClaimError, ClaimPreimage, ClaimStepV1, ExpectedBip199Output, LezAssetV1,
    LezChainIdentityV1, LezClaimInstructionV1, LezClaimNodeSnapshotV1, LezClaimObservationError,
    LezClaimTransactionSnapshotV1, LezCustodySnapshotV1, LezEnvironmentV1,
    LezEscrowMetadataSnapshotV1, LezEscrowStatusV1, LezInclusionStatusV1, LezObservationError,
    LezStableTipV1, NegotiationTranscriptV1, PreparedClaimSubmissionV1, RevealingClaimEvidenceV1,
    ZEC_CONCRETE_AGREEMENT_SCHEMA_V2, ZcashTransparentDestinationV1, ZecAgreementBodyV1,
    ZecAgreementRecordV1, ZecAgreementV1, ZecLezTermsV1, ZecParticipantIdentityV1,
    ZecParticipantsV1, ZecProfileId, ZecProfileRecordV1, ZecRefundPlanV1, ZecSwapBinding,
    ZecSwapBindingRecordV1, ZecTransactionPolicyV1, derive_lez_metadata_account_v1,
    derive_lez_native_custody_account_v1, derive_lez_swap_id_v1,
};
use secp256k1::{Message, PublicKey, Secp256k1, SecretKey};
use sha2::{Digest as _, Sha256};
use zcash_protocol::{
    consensus::{BranchId, NetworkType},
    value::Zatoshis,
};
use zcash_transparent::address::TransparentAddress;

const ACCEPTED_AT: UnixSeconds = UnixSeconds::new(10);
const PREIMAGE: [u8; 32] = [0x2a; 32];
const CLAIM_ID: [u8; 32] = [0x31; 32];

#[test]
fn canonical_snapshot_constructs_evidence_only_after_every_node_fact_is_bound() {
    let agreement = agreement();
    let snapshot = claim_snapshot(&agreement, Mutation::Valid);
    let debug = format!("{snapshot:?}");
    assert!(debug.contains("[REDACTED]"));
    assert!(!debug.contains(&format!("{PREIMAGE:?}")));

    let evidence = RevealingClaimEvidenceV1::from_lez_claim_snapshot(&agreement, snapshot)
        .expect("fully agreement-bound canonical claim");
    assert_eq!(evidence.observed_submission_id(), &CLAIM_ID);
    assert_eq!(evidence.confirmations(), 2);
    assert_eq!(evidence.preimage().expose_secret(), &PREIMAGE);

    RevealingClaimEvidenceV1::from_lez_claim_snapshot(
        &agreement,
        claim_snapshot(&agreement, Mutation::Pending),
    )
    .expect("depth-qualified deterministic standalone inclusion may remain Bedrock Pending");
}

#[test]
fn prepared_observation_requires_the_exact_durable_submission_identity() {
    let agreement = agreement();
    let prepared = PreparedClaimSubmissionV1::new(
        ClaimStepV1::RevealingLez,
        CLAIM_ID,
        b"official-lez-canonical-claim-bytes".to_vec(),
    )
    .expect("prepared revealing claim");
    RevealingClaimEvidenceV1::from_prepared_lez_claim_snapshot(
        &agreement,
        &prepared,
        claim_snapshot(&agreement, Mutation::Valid),
    )
    .expect("canonical node identity matches protected plan");

    let substituted = PreparedClaimSubmissionV1::new(
        ClaimStepV1::RevealingLez,
        [0x91; 32],
        b"different-official-lez-claim".to_vec(),
    )
    .expect("bounded substituted claim");
    assert_eq!(
        RevealingClaimEvidenceV1::from_prepared_lez_claim_snapshot(
            &agreement,
            &substituted,
            claim_snapshot(&agreement, Mutation::Valid),
        )
        .expect_err("a different prepared identity must not be projected"),
        LezClaimObservationError::PreparedIdentityMismatch,
    );

    let wrong_step = PreparedClaimSubmissionV1::new(
        ClaimStepV1::FollowupZcash,
        CLAIM_ID,
        b"zcash-claim".to_vec(),
    )
    .expect("bounded wrong-step claim");
    assert_eq!(
        RevealingClaimEvidenceV1::from_prepared_lez_claim_snapshot(
            &agreement,
            &wrong_step,
            claim_snapshot(&agreement, Mutation::Valid),
        )
        .expect_err("a Zcash plan cannot authenticate a LEZ observation"),
        LezClaimObservationError::PreparedStepMismatch,
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn negative_matrix_rejects_each_independent_node_or_agreement_deviation() {
    let agreement = agreement();
    let cases = [
        (
            Mutation::Environment,
            chain(LezObservationError::ChainIdentityMismatch),
        ),
        (
            Mutation::Channel,
            chain(LezObservationError::ChainIdentityMismatch),
        ),
        (
            Mutation::Genesis,
            chain(LezObservationError::ChainIdentityMismatch),
        ),
        (Mutation::TipDrift, chain(LezObservationError::UnstableTip)),
        (
            Mutation::InclusionBlock,
            chain(LezObservationError::NoncanonicalInclusion),
        ),
        (
            Mutation::InclusionAboveTip,
            chain(LezObservationError::NoncanonicalInclusion),
        ),
        (
            Mutation::CanonicalHash,
            LezClaimObservationError::TransactionIdentityMismatch,
        ),
        (
            Mutation::NonPublic,
            LezClaimObservationError::TransactionBindingMismatch,
        ),
        (
            Mutation::InvalidSignature,
            LezClaimObservationError::TransactionBindingMismatch,
        ),
        (
            Mutation::Program,
            LezClaimObservationError::TransactionBindingMismatch,
        ),
        (
            Mutation::Signer,
            LezClaimObservationError::TransactionBindingMismatch,
        ),
        (
            Mutation::InstructionKind,
            LezClaimObservationError::TransactionBindingMismatch,
        ),
        (
            Mutation::SwapId,
            LezClaimObservationError::TransactionBindingMismatch,
        ),
        (
            Mutation::Preimage,
            LezClaimObservationError::Claim(ClaimError::SecretDigestMismatch),
        ),
        (
            Mutation::Accounts,
            LezClaimObservationError::TransactionAccountsMismatch,
        ),
        (
            Mutation::MetadataOwner,
            LezClaimObservationError::MetadataBindingMismatch,
        ),
        (
            Mutation::MetadataAccount,
            LezClaimObservationError::MetadataBindingMismatch,
        ),
        (
            Mutation::MetadataVersion,
            LezClaimObservationError::MetadataBindingMismatch,
        ),
        (
            Mutation::MetadataTerms,
            LezClaimObservationError::MetadataBindingMismatch,
        ),
        (
            Mutation::MetadataStatus,
            LezClaimObservationError::MetadataBindingMismatch,
        ),
        (
            Mutation::CustodyAccount,
            LezClaimObservationError::CustodyBindingMismatch,
        ),
        (
            Mutation::CustodyOwner,
            LezClaimObservationError::CustodyBindingMismatch,
        ),
        (
            Mutation::CustodyNotEmptied,
            LezClaimObservationError::CustodyBindingMismatch,
        ),
        (
            Mutation::InvalidDepth,
            chain(LezObservationError::InvalidConfirmationDepth),
        ),
    ];

    for (mutation, expected) in cases {
        assert_eq!(
            RevealingClaimEvidenceV1::from_lez_claim_snapshot(
                &agreement,
                claim_snapshot(&agreement, mutation),
            )
            .expect_err("one changed primitive must fail closed"),
            expected,
            "mutation {mutation:?}",
        );
    }
}

fn chain(error: LezObservationError) -> LezClaimObservationError {
    LezClaimObservationError::Chain(error)
}

#[derive(Clone, Copy, Debug)]
enum Mutation {
    Valid,
    Environment,
    Channel,
    Genesis,
    TipDrift,
    InclusionBlock,
    InclusionAboveTip,
    CanonicalHash,
    Pending,
    NonPublic,
    InvalidSignature,
    Program,
    Signer,
    InstructionKind,
    SwapId,
    Preimage,
    Accounts,
    MetadataOwner,
    MetadataAccount,
    MetadataVersion,
    MetadataTerms,
    MetadataStatus,
    CustodyAccount,
    CustodyOwner,
    CustodyNotEmptied,
    InvalidDepth,
}

#[allow(clippy::too_many_lines)]
fn claim_snapshot(agreement: &ZecAgreementV1, mutation: Mutation) -> LezClaimNodeSnapshotV1 {
    let terms = agreement.lez_terms();
    let LezAssetV1::Native {
        authenticated_transfer_program_id,
    } = terms.asset()
    else {
        panic!("claim fixture uses native LEZ")
    };
    let depositor = *agreement.lez_account(agreement.lez_depositor());
    let claimant = *agreement.lez_account(agreement.lez_claimant());
    let instruction_preimage = if matches!(mutation, Mutation::Preimage) {
        [0x99; 32]
    } else {
        PREIMAGE
    };
    let instruction_swap_id = if matches!(mutation, Mutation::SwapId) {
        [0x88; 32]
    } else {
        *agreement.onchain_swap_id()
    };
    let instruction = if matches!(mutation, Mutation::InstructionKind) {
        LezClaimInstructionV1::Token {
            swap_id: instruction_swap_id,
            preimage: ClaimPreimage::new(instruction_preimage),
        }
    } else {
        LezClaimInstructionV1::Native {
            swap_id: instruction_swap_id,
            preimage: ClaimPreimage::new(instruction_preimage),
        }
    };
    let inclusion_height = if matches!(mutation, Mutation::InclusionAboveTip) {
        103
    } else {
        100
    };
    let (inclusion_height, tip_height) = if matches!(mutation, Mutation::InvalidDepth) {
        (0, u64::MAX)
    } else {
        (inclusion_height, 101)
    };
    let inclusion_block_hash = [0x41; 32];
    let canonical_block_hash = if matches!(mutation, Mutation::InclusionBlock) {
        [0x42; 32]
    } else {
        inclusion_block_hash
    };
    let transaction = LezClaimTransactionSnapshotV1::new(
        CLAIM_ID,
        if matches!(mutation, Mutation::CanonicalHash) {
            [0x32; 32]
        } else {
            CLAIM_ID
        },
        if matches!(mutation, Mutation::Program) {
            [0x77; 8]
        } else {
            *terms.escrow_program_id()
        },
        if matches!(mutation, Mutation::Signer) {
            depositor
        } else {
            claimant
        },
        if matches!(mutation, Mutation::Accounts) {
            vec![
                *terms.custody_account(),
                *terms.metadata_account(),
                claimant,
            ]
        } else {
            vec![
                *terms.metadata_account(),
                *terms.custody_account(),
                claimant,
            ]
        },
        instruction,
        !matches!(mutation, Mutation::NonPublic),
        !matches!(mutation, Mutation::InvalidSignature),
        inclusion_height,
        inclusion_block_hash,
        canonical_block_hash,
        if matches!(mutation, Mutation::Pending) {
            LezInclusionStatusV1::Pending
        } else {
            LezInclusionStatusV1::Safe
        },
    );
    let metadata = LezEscrowMetadataSnapshotV1::new(
        if matches!(mutation, Mutation::MetadataVersion) {
            1
        } else {
            2
        },
        *agreement.onchain_swap_id(),
        if matches!(mutation, Mutation::MetadataTerms) {
            [0x66; 32]
        } else {
            *agreement.agreement_commitment()
        },
        *agreement.secret_digest(),
        depositor,
        depositor,
        claimant,
        claimant,
        *terms.custody_account(),
        *authenticated_transfer_program_id,
        *authenticated_transfer_program_id,
        [0; 32],
        terms.amount(),
        agreement.lez_refund_at_ms(),
        if matches!(mutation, Mutation::MetadataStatus) {
            LezEscrowStatusV1::Funded
        } else {
            LezEscrowStatusV1::Claimed
        },
    );
    let tip = if matches!(mutation, Mutation::TipDrift) {
        LezStableTipV1::new([0x51; 32], tip_height, [0x52; 32], tip_height + 1)
    } else {
        LezStableTipV1::new([0x51; 32], tip_height, [0x51; 32], tip_height)
    };
    LezClaimNodeSnapshotV1::new(
        if matches!(mutation, Mutation::Environment) {
            LezEnvironmentV1::PublicTestnetV0_2
        } else {
            terms.chain().environment()
        },
        if matches!(mutation, Mutation::Channel) {
            [0x97; 32]
        } else {
            *terms.chain().channel_id()
        },
        if matches!(mutation, Mutation::Genesis) {
            [0x98; 32]
        } else {
            *terms.chain().genesis_block_hash()
        },
        tip,
        transaction,
        if matches!(mutation, Mutation::MetadataOwner) {
            [0x76; 8]
        } else {
            *terms.escrow_program_id()
        },
        if matches!(mutation, Mutation::MetadataAccount) {
            [0x75; 32]
        } else {
            *terms.metadata_account()
        },
        metadata,
        if matches!(mutation, Mutation::CustodyAccount) {
            [0x74; 32]
        } else {
            *terms.custody_account()
        },
        LezCustodySnapshotV1::Native {
            program_owner: if matches!(mutation, Mutation::CustodyOwner) {
                [0x73; 8]
            } else {
                *authenticated_transfer_program_id
            },
            balance: u128::from(matches!(mutation, Mutation::CustodyNotEmptied)),
        },
    )
}

#[allow(clippy::too_many_lines)]
fn agreement() -> ZecAgreementV1 {
    let maker_secret = SecretKey::from_slice(&[1; 32]).expect("maker key");
    let taker_secret = SecretKey::from_slice(&[2; 32]).expect("taker key");
    let secp = Secp256k1::new();
    let maker_key = PublicKey::from_secret_key(&secp, &maker_secret).serialize();
    let taker_key = PublicKey::from_secret_key(&secp, &taker_secret).serialize();
    let refund_hash = pubkey_hash(&maker_key);
    let claimant_hash = pubkey_hash(&taker_key);
    let secret_digest = Sha256::digest(PREIMAGE).into();
    let application_id = "canonical-lez-claim";
    let escrow_program = [1; 8];
    let onchain_swap_id = derive_lez_swap_id_v1(application_id.as_bytes());
    let metadata_account = derive_lez_metadata_account_v1(&escrow_program, &onchain_swap_id);
    let custody_account = derive_lez_native_custody_account_v1(&escrow_program, &onchain_swap_id);
    let profile = ZecProfileId::DeterministicLocalV1;
    let binding = ZecSwapBinding::new(
        profile,
        ExpectedBip199Output::new(
            NetworkType::Regtest,
            BranchId::Nu6_2,
            Zatoshis::from_u64(100_000_000).expect("valid value"),
            Bip199Contract::new(120, refund_hash, secret_digest, claimant_hash),
        ),
    )
    .expect("profile-bound Zcash output");
    let body = ZecAgreementBodyV1::new(
        application_id.to_owned(),
        SwapDirection::TakerSellsLez,
        ZecProfileRecordV1::from(profile),
        ZecParticipantsV1::new(
            ZecParticipantIdentityV1::new([3; 32], maker_key),
            ZecParticipantIdentityV1::new([4; 32], taker_key),
        ),
        secret_digest,
        ZecLezTermsV1::new(
            LezChainIdentityV1::new(LezEnvironmentV1::DeterministicLocalV0_2, [8; 32], [7; 32]),
            escrow_program,
            LezAssetV1::Native {
                authenticated_transfer_program_id: [2; 8],
            },
            42,
            metadata_account,
            custody_account,
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
            40,
        ),
        ZecRefundPlanV1::new(100, 116, 160_000, 200),
        NegotiationTranscriptV1::new([5; 32], [6; 32], 1_000),
    );
    let commitment = body.commitment();
    let record = ZecAgreementRecordV1::from_parts(
        ZEC_CONCRETE_AGREEMENT_SCHEMA_V2,
        body,
        commitment,
        secp.sign_ecdsa(&Message::from_digest(commitment), &maker_secret)
            .serialize_compact(),
        secp.sign_ecdsa(&Message::from_digest(commitment), &taker_secret)
            .serialize_compact(),
    );
    ZecAgreementV1::from_wire_at(
        &record.encode_wire().expect("bounded agreement wire"),
        ACCEPTED_AT,
    )
    .expect("valid deterministic agreement")
}

fn pubkey_hash(bytes: &[u8; 33]) -> [u8; 20] {
    match TransparentAddress::from_pubkey(&PublicKey::from_slice(bytes).expect("fixture pubkey")) {
        TransparentAddress::PublicKeyHash(hash) => hash,
        TransparentAddress::ScriptHash(_) => unreachable!("public keys produce P2PKH"),
    }
}
