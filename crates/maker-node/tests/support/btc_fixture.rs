use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _},
    path::{Path, PathBuf},
};

use bitcoin::{
    Amount, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Txid, Witness, absolute,
    consensus::serialize,
    hashes::Hash as _,
    secp256k1::{Keypair, Message, PublicKey, Secp256k1, SecretKey},
    transaction,
};
use btc_reference_actor::{ActorConfig, ActorRole};
use lez_bridge_protocol::{
    ExactMessageBytes, ExactTransactionBytes, Hex32, MessageContext,
    Participant as BridgeParticipant, PrepareWitnessedClaimResult, PrepareWitnessedEscrowRequest,
    PrepareWitnessedEscrowResult, PreparedTransaction, PreparedWitnessedClaim, RequestId, RunId,
    RuntimeCompatibility, RuntimeDescriptor, TransactionId, WitnessedNativeEscrowTerms,
    WitnessedNativeEscrowTermsInput,
};
use lez_btc_swap_sdk::{
    AdaptorSessionContext, BTC_AGREEMENT_SCHEMA_V1, BtcAdaptorSessionDomain, BtcAgreementBodyV1,
    BtcAgreementDraftV1, BtcAgreementRecordV1, BtcAgreementV1, BtcChainPolicyV1, BtcClaimTermsV1,
    BtcFundingTermsV1, BtcLezTermsV1, BtcP2trTermsV1, BtcParticipantIdentityV1, BtcParticipantsV1,
    BtcRecoveryPlanV1, CsvBlockDelay, FreshAdaptorNonce, P2trSwapOutput,
    PersistedAdaptorSigningMaterial, RefundXOnlyKey, SigningRole, TwoPartyAggregateKey,
    aggregate_adaptor_presignature, sign_persisted_adaptor_partial,
    verify_adaptor_partial_signature, verify_nonce_commitment,
};
use lez_swap_core::{Participant, SwapDirection};
use lez_swap_store::{
    AdaptorNonceCommitment, AdaptorPartialSignature, AdaptorPresignature, AdaptorPublicNonce,
    AdaptorSessionIdentity, AdaptorSessionReservation, AdaptorSessionRole, SecretNonceBytes,
    SqliteAdaptorSessionJournal,
};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

const MAKER_SIGNING_SECRET: u8 = 1;
const TAKER_SIGNING_SECRET: u8 = 2;
const MAKER_REFUND_SECRET: u8 = 3;
const TAKER_REFUND_SECRET: u8 = 4;
const MAKER_CLAIM_SECRET: u8 = 5;
const TAKER_CLAIM_SECRET: u8 = 6;
const ADAPTOR_SECRET: u8 = 7;
const FOREIGN_UNITS_SAT: u64 = 100_000;
const LEZ_UNITS: u128 = 5_000;
const PREPARED_MESSAGE: &[u8] = b"m5-btc-chat-prepared-witnessed-claim";

pub struct BtcAuthorityFixture {
    pub maker_source_config: PathBuf,
    pub taker_source_config: PathBuf,
    pub unsigned_draft: PathBuf,
    pub maker_actor_root: PathBuf,
    pub actor_program: PathBuf,
    pub actor_program_sha256: String,
}

impl BtcAuthorityFixture {
    pub fn new(root: &Path, label: &str, swap_id: [u8; 32]) -> Self {
        Self::new_with_direction(root, label, swap_id, SwapDirection::TakerSellsForeign)
    }

    pub fn new_with_direction(
        root: &Path,
        label: &str,
        swap_id: [u8; 32],
        direction: SwapDirection,
    ) -> Self {
        let fixture_root = root.join(format!("btc-authority-{label}"));
        make_private_directory(&fixture_root);
        let agreement = agreement(swap_id, direction);
        let agreement_wire = agreement.encode_wire().expect("canonical source agreement");
        let unsigned_draft = fixture_root.join("unsigned-draft-v1.borsh");
        let draft = BtcAgreementDraftV1::validate(agreement.body().clone())
            .expect("canonical unsigned BTC agreement body");
        write_private(
            &unsigned_draft,
            &draft.encode_wire().expect("encode unsigned BTC agreement"),
        );

        let source_agreement = fixture_root.join("source-agreement-v1.borsh");
        write_private(&source_agreement, &agreement_wire);
        let maker_source_config = role_config(
            &fixture_root,
            ActorRole::Maker,
            &agreement,
            &agreement_wire,
            &source_agreement,
        );
        let taker_source_config = role_config(
            &fixture_root,
            ActorRole::Taker,
            &agreement,
            &agreement_wire,
            &source_agreement,
        );

        let maker_actor_root = fixture_root.join("maker-accepted-actor");
        make_private_directory(&maker_actor_root);
        let actor_program = PathBuf::from("/usr/bin/true").canonicalize().unwrap();
        let actor_program_sha256 = hex::encode(Sha256::digest(fs::read(&actor_program).unwrap()));
        Self {
            maker_source_config,
            taker_source_config,
            unsigned_draft,
            maker_actor_root,
            actor_program,
            actor_program_sha256,
        }
    }
}

fn agreement(swap_id: [u8; 32], direction: SwapDirection) -> BtcAgreementV1 {
    let maker_signing = secret(MAKER_SIGNING_SECRET);
    let taker_signing = secret(TAKER_SIGNING_SECRET);
    let maker_refund = secret(MAKER_REFUND_SECRET);
    let taker_refund = secret(TAKER_REFUND_SECRET);
    let maker_claim = secret(MAKER_CLAIM_SECRET);
    let taker_claim = secret(TAKER_CLAIM_SECRET);
    let adaptor = secret(ADAPTOR_SECRET);
    let participants = BtcParticipantsV1::new(
        BtcParticipantIdentityV1::new(
            [10; 32],
            public_key(&maker_signing),
            x_only_key(&maker_refund),
            destination(&maker_claim),
        ),
        BtcParticipantIdentityV1::new(
            [11; 32],
            public_key(&taker_signing),
            x_only_key(&taker_refund),
            destination(&taker_claim),
        ),
    );
    let adaptor_point = public_key(&adaptor);
    let aggregate_key = lez_btc_swap_sdk::AdaptorSessionContext::untweaked(
        [public_key(&maker_signing), public_key(&taker_signing)],
        [30; 32],
        adaptor_point,
        [31; 32],
    )
    .expect("aggregate signing context")
    .output_key();
    let refund_key = match direction {
        SwapDirection::TakerSellsForeign => x_only_key(&taker_refund),
        SwapDirection::TakerSellsLez => x_only_key(&maker_refund),
    };
    let contract = lez_btc_swap_sdk::P2trSwapOutput::new(
        TwoPartyAggregateKey::from_bytes(aggregate_key).expect("aggregate key"),
        RefundXOnlyKey::from_bytes(refund_key).expect("Bitcoin depositor refund key"),
        CsvBlockDelay::new(144).expect("CSV delay"),
    )
    .expect("P2TR contract");
    let body = agreement_body(swap_id, direction, participants, adaptor_point, &contract);
    let commitment = body.commitment();
    BtcAgreementV1::validate(BtcAgreementRecordV1::from_parts(
        BTC_AGREEMENT_SCHEMA_V1,
        body,
        commitment,
        agreement_signature(&maker_signing, commitment),
        agreement_signature(&taker_signing, commitment),
    ))
    .expect("valid source agreement")
}

fn agreement_body(
    swap_id: [u8; 32],
    direction: SwapDirection,
    participants: BtcParticipantsV1,
    adaptor_point: [u8; 33],
    contract: &P2trSwapOutput,
) -> BtcAgreementBodyV1 {
    let funding_transaction = funding_transaction(contract.script_pubkey_bytes().to_vec());
    let funding = BtcFundingTermsV1::new(
        funding_transaction.compute_txid().to_byte_array(),
        1,
        FOREIGN_UNITS_SAT,
    );
    let bitcoin_claimant = match direction {
        SwapDirection::TakerSellsForeign => Participant::Maker,
        SwapDirection::TakerSellsLez => Participant::Taker,
    };
    let claim = lez_btc_swap_sdk::CooperativeKeyPathSpend::new(
        contract,
        OutPoint {
            txid: Txid::from_byte_array(*funding.transaction_id()),
            vout: funding.output_index(),
        },
        Amount::from_sat(funding.value_sat()),
        vec![TxOut {
            value: Amount::from_sat(99_000),
            script_pubkey: ScriptBuf::from_bytes(
                participants
                    .for_participant(bitcoin_claimant)
                    .claim_destination_script_pubkey()
                    .to_vec(),
            ),
        }],
    )
    .expect("cooperative claim");
    let prepared_message_hash: [u8; 32] = Sha256::digest(
        [
            b"/LEE/v0.3/Message/Public/\0\0\0\0\0\0\0".as_slice(),
            PREPARED_MESSAGE,
        ]
        .concat(),
    )
    .into();
    let (lez_depositor, lez_claimant) = match direction {
        SwapDirection::TakerSellsForeign => ([10; 32], [11; 32]),
        SwapDirection::TakerSellsLez => ([11; 32], [10; 32]),
    };
    let lez_refund_at_ms = match direction {
        SwapDirection::TakerSellsForeign => 4_102_444_500_000,
        SwapDirection::TakerSellsLez => 4_102_444_800_000,
    };
    BtcAgreementBodyV1::new(
        swap_id,
        direction,
        BtcChainPolicyV1::new([8; 32], 6),
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
            lez_depositor,
            lez_claimant,
            LEZ_UNITS,
            lez_refund_at_ms,
            prepared_message_hash,
        ),
        BtcP2trTermsV1::from_contract(contract),
        funding,
        BtcClaimTermsV1::from_spend(&claim).expect("claim terms"),
        BtcRecoveryPlanV1::new(
            1_000,
            1_144,
            4_102_444_200,
            4_102_444_500,
            4_102_444_800,
            300,
        ),
    )
}

fn role_config(
    root: &Path,
    role: ActorRole,
    agreement: &BtcAgreementV1,
    agreement_wire: &[u8],
    agreement_file: &Path,
) -> PathBuf {
    let name = match role {
        ActorRole::Maker => "maker",
        ActorRole::Taker => "taker",
    };
    let role_root = root.join(format!("{name}-source"));
    make_private_directory(&role_root);
    let bitcoin_journal = role_root.join("bitcoin-adaptor.sqlite3");
    let lez_journal = role_root.join("lez-adaptor.sqlite3");
    let prepared_claim = role_root.join("prepared-witnessed-claim.json");
    let adaptor_secret = role_root.join("adaptor-secret.key");
    let refund_secret = role_root.join("bitcoin-refund.key");
    let exact_funding = role_root.join("maker-bitcoin-funding.hex");
    let lez_lock_request = role_root.join("maker-lez-lock-request.json");
    let lez_lock_result = role_root.join("maker-lez-lock-result.json");
    let run_id = RunId::new(format!("m5-btc-chat-{name}-authority")).unwrap();
    let runtime = role_runtime(role);

    seed_prepared_claim(&prepared_claim, &run_id, agreement);
    seed_signing_journal(
        role,
        agreement,
        BtcAdaptorSessionDomain::Bitcoin,
        [41; 32],
        &bitcoin_journal,
    );
    seed_signing_journal(
        role,
        agreement,
        BtcAdaptorSessionDomain::Lez,
        [42; 32],
        &lez_journal,
    );
    let bitcoin_funder_role = match agreement.direction() {
        SwapDirection::TakerSellsForeign => ActorRole::Taker,
        SwapDirection::TakerSellsLez => ActorRole::Maker,
    };
    seed_role_secrets(&adaptor_secret, &refund_secret, role, bitcoin_funder_role);
    let maker_lock = if role == ActorRole::Taker {
        None
    } else {
        Some(prepare_maker_lock(
            agreement,
            &run_id,
            &runtime,
            &exact_funding,
            &lez_lock_request,
            &lez_lock_result,
        ))
    };

    let config_path = role_root.join("actor-config.json");
    let mut signing = json!({
        "bitcoin": { "session_id": hex::encode([41; 32]), "journal_db": bitcoin_journal },
        "lez": { "session_id": hex::encode([42; 32]), "journal_db": lez_journal },
        "prepared_witnessed_claim_result_file": prepared_claim
    });
    if role == ActorRole::Taker {
        signing["adaptor_secret_file"] = json!(adaptor_secret);
    }
    let mut config = json!({
        "schema_version": 6,
        "role": name,
        "agreement_file": agreement_file,
        "state_db": role_root.join("actor.sqlite3"),
        "accepted_at_unix_seconds": 1_700_000_000_u64,
        "agreement_sha256": hex::encode(Sha256::digest(agreement_wire)),
        "bitcoin_core": {
            "endpoint": "http://127.0.0.1:1",
            "cookie_file": role_root.join("bitcoin.cookie"),
            "connectivity": "isolated_local"
        },
        "lez_bridge": {
            "endpoint": "http://127.0.0.1:2",
            "capability_file": role_root.join("lez.capability"),
            "run_id": run_id,
            "runtime": runtime,
            "request_timeout_millis": 1_000,
            "discovery_start_height": 1,
            "discovery_max_blocks": 10
        },
        "signing": signing,
        "refund": if role == bitcoin_funder_role {
            json!({ "bitcoin_refund_key_file": refund_secret })
        } else {
            json!({})
        }
    });
    if let Some(maker_lock) = maker_lock {
        config["maker_lock"] = maker_lock;
    }
    write_private(
        &config_path,
        &serde_json::to_vec_pretty(&config).expect("schema-6 config JSON"),
    );
    validate_role_config(&config_path, role, agreement);
    config_path
}

fn seed_role_secrets(
    adaptor_secret: &Path,
    refund_secret: &Path,
    role: ActorRole,
    bitcoin_funder_role: ActorRole,
) {
    if role == ActorRole::Taker {
        write_private(adaptor_secret, hex::encode([ADAPTOR_SECRET; 32]).as_bytes());
    }
    if role == bitcoin_funder_role {
        let secret = match role {
            ActorRole::Maker => MAKER_REFUND_SECRET,
            ActorRole::Taker => TAKER_REFUND_SECRET,
        };
        write_private(refund_secret, hex::encode([secret; 32]).as_bytes());
    }
}

fn validate_role_config(path: &Path, role: ActorRole, agreement: &BtcAgreementV1) {
    let loaded = ActorConfig::load_private(path).expect("load schema-6 source config");
    assert_eq!(loaded.role(), role);
    assert_eq!(
        loaded.supervised_swap_id().unwrap(),
        *agreement.coordinator().id()
    );
}

fn prepare_maker_lock(
    agreement: &BtcAgreementV1,
    run_id: &RunId,
    runtime: &RuntimeDescriptor,
    exact_funding: &Path,
    lez_lock_request: &Path,
    lez_lock_result: &Path,
) -> Value {
    match agreement.direction() {
        SwapDirection::TakerSellsForeign => {
            let context = MessageContext::new(
                run_id.clone(),
                RequestId::new("m5-btc-chat-maker-lez-lock").unwrap(),
                BridgeParticipant::Maker,
            );
            let request = PrepareWitnessedEscrowRequest::new(
                context.clone(),
                runtime.clone(),
                witnessed_lez_terms(agreement),
            );
            let result = PrepareWitnessedEscrowResult::new(
                context,
                PreparedTransaction::new(
                    TransactionId::from_bytes([81; 32]),
                    ExactTransactionBytes::new(b"m5-btc-chat-lez-initialize".to_vec()).unwrap(),
                ),
                PreparedTransaction::new(
                    TransactionId::from_bytes([82; 32]),
                    ExactTransactionBytes::new(b"m5-btc-chat-lez-fund".to_vec()).unwrap(),
                ),
            );
            write_private(lez_lock_request, &serde_json::to_vec(&request).unwrap());
            write_private(lez_lock_result, &serde_json::to_vec(&result).unwrap());
            json!({
                "chain": "lez",
                "preparation_request_file": lez_lock_request,
                "preparation_result_file": lez_lock_result
            })
        }
        SwapDirection::TakerSellsLez => {
            let exact =
                funding_transaction(agreement.p2tr_contract().script_pubkey_bytes().to_vec());
            write_private(exact_funding, hex::encode(serialize(&exact)).as_bytes());
            json!({
                "chain": "bitcoin",
                "exact_funding_transaction_file": exact_funding
            })
        }
    }
}

fn witnessed_lez_terms(agreement: &BtcAgreementV1) -> WitnessedNativeEscrowTerms {
    let signed = agreement.lez_terms();
    WitnessedNativeEscrowTerms::new(WitnessedNativeEscrowTermsInput {
        swap_id: Hex32::from_bytes(*agreement.body().swap_id()),
        terms_hash: Hex32::from_bytes(*agreement.agreement_commitment()),
        depositor: bridge_participant(agreement.lez_depositor()),
        depositor_account_id: Hex32::from_bytes(*signed.depositor_account()),
        claimant: bridge_participant(agreement.lez_claimant()),
        claimant_account_id: Hex32::from_bytes(*signed.claimant_account()),
        aggregate_authority_account_id: Hex32::from_bytes(*signed.aggregate_authority_account()),
        aggregate_x_only_public_key: Hex32::from_bytes(
            agreement.p2tr_contract().aggregate_internal_key_bytes(),
        ),
        amount: signed.amount(),
        refund_at_ms: signed.refund_at_ms(),
        authenticated_transfer_program_id: Hex32::from_bytes(
            *signed.authenticated_transfer_program_id(),
        ),
    })
    .expect("valid witnessed LEZ terms")
}

const fn bridge_participant(participant: Participant) -> BridgeParticipant {
    match participant {
        Participant::Maker => BridgeParticipant::Maker,
        Participant::Taker => BridgeParticipant::Taker,
    }
}

fn role_runtime(role: ActorRole) -> RuntimeDescriptor {
    RuntimeDescriptor::new(
        match role {
            ActorRole::Maker => BridgeParticipant::Maker,
            ActorRole::Taker => BridgeParticipant::Taker,
        },
        RuntimeCompatibility::LeeV0_2_0,
        Hex32::from_bytes([99; 32]),
        Hex32::from_bytes([17; 32]),
        Hex32::from_bytes([18; 32]),
        Hex32::from_bytes([15; 32]),
        Hex32::from_bytes(match role {
            ActorRole::Maker => [10; 32],
            ActorRole::Taker => [11; 32],
        }),
    )
}

fn seed_prepared_claim(path: &Path, run_id: &RunId, agreement: &BtcAgreementV1) {
    let request_id = RequestId::new("m5-btc-prepared-claim-001").unwrap();
    let prepared = PrepareWitnessedClaimResult::new(
        MessageContext::new(
            run_id.clone(),
            request_id.clone(),
            bridge_participant(agreement.lez_claimant()),
        ),
        PreparedWitnessedClaim::new(
            request_id,
            Hex32::from_bytes(*agreement.lez_terms().claim_message_hash()),
            ExactMessageBytes::new(PREPARED_MESSAGE.to_vec()).unwrap(),
        ),
    );
    write_private(path, &serde_json::to_vec(&prepared).unwrap());
}

fn seed_signing_journal(
    role: ActorRole,
    agreement: &BtcAgreementV1,
    domain: BtcAdaptorSessionDomain,
    session_id: [u8; 32],
    journal_path: &Path,
) {
    let context = agreement
        .adaptor_session_context(domain, session_id)
        .unwrap();
    let maker_nonce =
        FreshAdaptorNonce::generate(&context, SigningRole::Maker, [MAKER_SIGNING_SECRET; 32])
            .unwrap();
    let taker_nonce =
        FreshAdaptorNonce::generate(&context, SigningRole::Taker, [TAKER_SIGNING_SECRET; 32])
            .unwrap();
    let actors = signing_actors(role, &maker_nonce, &taker_nonce);
    let identity = AdaptorSessionIdentity::new(
        session_id,
        actors.local_store_role,
        context.durable_context_binding(),
        context.message(),
        context.adaptor_point(),
        context.ordered_public_keys(),
    );
    let mut journal = SqliteAdaptorSessionJournal::open(journal_path).unwrap();
    let _ = journal
        .reserve(AdaptorSessionReservation::new(
            identity.clone(),
            SecretNonceBytes::new(*actors.local_nonce.secret_nonce()),
            AdaptorPublicNonce::new(actors.local_nonce.public_nonce()),
            AdaptorNonceCommitment::new(actors.local_nonce.commitment()),
        ))
        .unwrap();
    let _ = journal
        .record_peer_commitment(
            &identity,
            AdaptorNonceCommitment::new(actors.peer_nonce.commitment()),
        )
        .unwrap();
    verify_nonce_commitment(
        &context,
        actors.peer_role,
        actors.peer_nonce.commitment(),
        actors.peer_nonce.public_nonce(),
    )
    .unwrap();
    let _ = journal
        .record_verified_peer_public_nonce(
            &identity,
            AdaptorPublicNonce::new(actors.peer_nonce.public_nonce()),
        )
        .unwrap();
    complete_signing_journal(&context, &identity, &mut journal, &actors);
}

struct SigningActors<'a> {
    actor_role: ActorRole,
    local_role: SigningRole,
    local_store_role: AdaptorSessionRole,
    local_secret: [u8; 32],
    local_nonce: &'a FreshAdaptorNonce,
    peer_role: SigningRole,
    peer_secret: [u8; 32],
    peer_nonce: &'a FreshAdaptorNonce,
}

fn signing_actors<'a>(
    role: ActorRole,
    maker_nonce: &'a FreshAdaptorNonce,
    taker_nonce: &'a FreshAdaptorNonce,
) -> SigningActors<'a> {
    match role {
        ActorRole::Maker => SigningActors {
            actor_role: role,
            local_role: SigningRole::Maker,
            local_store_role: AdaptorSessionRole::Maker,
            local_secret: [MAKER_SIGNING_SECRET; 32],
            local_nonce: maker_nonce,
            peer_role: SigningRole::Taker,
            peer_secret: [TAKER_SIGNING_SECRET; 32],
            peer_nonce: taker_nonce,
        },
        ActorRole::Taker => SigningActors {
            actor_role: role,
            local_role: SigningRole::Taker,
            local_store_role: AdaptorSessionRole::Taker,
            local_secret: [TAKER_SIGNING_SECRET; 32],
            local_nonce: taker_nonce,
            peer_role: SigningRole::Maker,
            peer_secret: [MAKER_SIGNING_SECRET; 32],
            peer_nonce: maker_nonce,
        },
    }
}

fn complete_signing_journal(
    context: &AdaptorSessionContext,
    identity: &AdaptorSessionIdentity,
    journal: &mut SqliteAdaptorSessionJournal,
    actors: &SigningActors<'_>,
) {
    let own_partial = journal
        .sign_and_persist_partial(identity, |material| {
            sign_persisted_adaptor_partial(
                context,
                actors.local_role,
                actors.local_secret,
                PersistedAdaptorSigningMaterial::new(
                    *material.identity().signing_domain(),
                    material.secret_nonce(),
                    *material.own_public_nonce().bytes(),
                    actors.local_nonce.commitment(),
                    actors.peer_nonce.commitment(),
                    *material.peer_public_nonce().bytes(),
                ),
            )
            .map(AdaptorPartialSignature::new)
            .map_err(|_| ())
        })
        .unwrap()
        .partial();
    let peer_partial = sign_persisted_adaptor_partial(
        context,
        actors.peer_role,
        actors.peer_secret,
        PersistedAdaptorSigningMaterial::new(
            context.durable_context_binding(),
            actors.peer_nonce.secret_nonce(),
            actors.peer_nonce.public_nonce(),
            actors.peer_nonce.commitment(),
            actors.local_nonce.commitment(),
            actors.local_nonce.public_nonce(),
        ),
    )
    .unwrap();
    let (maker_public_nonce, taker_public_nonce, maker_partial, taker_partial) =
        match actors.actor_role {
            ActorRole::Maker => (
                actors.local_nonce.public_nonce(),
                actors.peer_nonce.public_nonce(),
                *own_partial.bytes(),
                peer_partial,
            ),
            ActorRole::Taker => (
                actors.peer_nonce.public_nonce(),
                actors.local_nonce.public_nonce(),
                peer_partial,
                *own_partial.bytes(),
            ),
        };
    verify_adaptor_partial_signature(
        context,
        actors.peer_role,
        maker_public_nonce,
        taker_public_nonce,
        peer_partial,
    )
    .unwrap();
    let _ = journal
        .record_verified_peer_partial(identity, AdaptorPartialSignature::new(peer_partial))
        .unwrap();
    let presignature = aggregate_adaptor_presignature(
        context,
        maker_public_nonce,
        taker_public_nonce,
        maker_partial,
        taker_partial,
    )
    .unwrap();
    let _ = journal
        .record_verified_presignature(identity, AdaptorPresignature::new(presignature))
        .unwrap();
}

fn funding_transaction(script_pubkey: Vec<u8>) -> Transaction {
    Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint {
                txid: Txid::from_byte_array([42; 32]),
                vout: 0,
            },
            script_sig: ScriptBuf::from_bytes(vec![0x51]),
            sequence: Sequence::MAX,
            witness: Witness::default(),
        }],
        output: vec![
            TxOut {
                value: Amount::from_sat(1),
                script_pubkey: ScriptBuf::new(),
            },
            TxOut {
                value: Amount::from_sat(FOREIGN_UNITS_SAT),
                script_pubkey: ScriptBuf::from_bytes(script_pubkey),
            },
        ],
    }
}

fn secret(byte: u8) -> SecretKey {
    SecretKey::from_slice(&[byte; 32]).unwrap()
}
fn public_key(secret: &SecretKey) -> [u8; 33] {
    PublicKey::from_secret_key(&Secp256k1::new(), secret).serialize()
}
fn x_only_key(secret: &SecretKey) -> [u8; 32] {
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
fn agreement_signature(secret: &SecretKey, commitment: [u8; 32]) -> [u8; 64] {
    Secp256k1::new()
        .sign_schnorr_no_aux_rand(
            &Message::from_digest(commitment),
            &Keypair::from_secret_key(&Secp256k1::new(), secret),
        )
        .serialize()
}

fn make_private_directory(path: &Path) {
    fs::DirBuilder::new().mode(0o700).create(path).unwrap();
}

fn write_private(path: &Path, bytes: &[u8]) {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
}
