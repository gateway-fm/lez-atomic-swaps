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
    ExactMessageBytes, Hex32, MessageContext, Participant as BridgeParticipant,
    PrepareWitnessedClaimResult, PreparedWitnessedClaim, RequestId, RunId, RuntimeCompatibility,
    RuntimeDescriptor,
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
use serde_json::json;
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
        let fixture_root = root.join(format!("btc-authority-{label}"));
        make_private_directory(&fixture_root);
        let agreement = agreement(swap_id);
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

fn agreement(swap_id: [u8; 32]) -> BtcAgreementV1 {
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
    let contract = lez_btc_swap_sdk::P2trSwapOutput::new(
        TwoPartyAggregateKey::from_bytes(aggregate_key).expect("aggregate key"),
        RefundXOnlyKey::from_bytes(x_only_key(&taker_refund)).expect("Taker refund key"),
        CsvBlockDelay::new(144).expect("CSV delay"),
    )
    .expect("P2TR contract");
    let body = agreement_body(swap_id, participants, adaptor_point, &contract);
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
                    .for_participant(Participant::Maker)
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
    BtcAgreementBodyV1::new(
        swap_id,
        SwapDirection::TakerSellsForeign,
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
            [10; 32],
            [11; 32],
            LEZ_UNITS,
            4_102_444_500_000,
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
    let run_id = RunId::new(format!("m5-btc-chat-{name}-authority")).unwrap();

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
    if role == ActorRole::Taker {
        write_private(
            &adaptor_secret,
            hex::encode([ADAPTOR_SECRET; 32]).as_bytes(),
        );
        write_private(
            &refund_secret,
            hex::encode([TAKER_REFUND_SECRET; 32]).as_bytes(),
        );
    } else {
        let exact = funding_transaction(agreement.p2tr_contract().script_pubkey_bytes().to_vec());
        write_private(&exact_funding, hex::encode(serialize(&exact)).as_bytes());
    }

    let runtime = role_runtime(role);
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
        "refund": if role == ActorRole::Taker {
            json!({ "bitcoin_refund_key_file": refund_secret })
        } else {
            json!({})
        }
    });
    if role == ActorRole::Maker {
        config["maker_lock"] =
            json!({ "chain": "bitcoin", "exact_funding_transaction_file": exact_funding });
    }
    write_private(
        &config_path,
        &serde_json::to_vec_pretty(&config).expect("schema-6 config JSON"),
    );
    let loaded = ActorConfig::load_private(&config_path).expect("load schema-6 source config");
    assert_eq!(loaded.role(), role);
    assert_eq!(
        loaded.supervised_swap_id().unwrap(),
        *agreement.coordinator().id()
    );
    config_path
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
        MessageContext::new(run_id.clone(), request_id.clone(), BridgeParticipant::Taker),
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
