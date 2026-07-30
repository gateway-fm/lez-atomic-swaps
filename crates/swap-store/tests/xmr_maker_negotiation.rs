use std::sync::{Arc, Barrier, OnceLock};
use std::thread;

use lez_adaptor_signature::{AdaptorSigner, SigningRole};
use lez_bridge_protocol::RequestId;
use lez_swap_core::{Pair, Participant, SwapDirection, SwapId};
use lez_swap_store::{
    LocalPriceV1, MakerActorKindV1, MakerActorManifestV1, MakerOfferId, MakerOfferStatus,
    MakerPairConfigurationV1, MakerPriceSourceKind, MakerRouteV1, MakerXmrActivationAcceptance,
    MakerXmrNegotiationStatus, MakerXmrNegotiationV1, SqliteSwapStore, StoreError,
    maker_xmr_chat_swap_id,
};
use lez_xmr_swap_sdk::{
    CrossCurveDleqProofV1, CrossCurveScalar, MoneroAddressNetworkV1, MoneroPrivateViewKey,
    MoneroSharedAddressV1, ValidatedXmrActivationBodyV1, ValidatedXmrAgreementBodyV1,
    XMR_AGREEMENT_SCHEMA_V1, XmrActivatedAgreementV1, XmrActivationBodyV1, XmrAgreementBodyV1,
    XmrAgreementRecordV1, XmrAgreementV1, XmrLezTermsV1, XmrMessagesV1, XmrMoneroTermsV1,
    XmrNamedProfileV1, XmrParticipantIdentityV1, XmrParticipantsV1, XmrSessionTranscriptV1,
    XmrSwapDirectionV1, XmrWindowsV1,
};
use rand_chacha::{ChaCha20Rng, rand_core::SeedableRng as _};
use rusqlite::{Connection, params};
use secp256k1::{Keypair, Message, PublicKey, Secp256k1, SecretKey};
use tempfile::tempdir;

const OFFER_COMMITMENT: [u8; 32] = [0x61; 32];
const XMR_PICONERO: u64 = 1_000_000_000_000;
const LEZ_UNITS: u128 = 1_000;
const MAKER_AGREEMENT_SECRET: [u8; 32] = [7; 32];
const TAKER_AGREEMENT_SECRET: [u8; 32] = [8; 32];
const MAKER_CLAIM_SECRET: [u8; 32] = [9; 32];
const TAKER_CLAIM_SECRET: [u8; 32] = [10; 32];
const MAKER_REFUND_SECRET: [u8; 32] = [11; 32];
const TAKER_REFUND_SECRET: [u8; 32] = [12; 32];
const VIEW_KEY_BYTES: [u8; 32] = {
    let mut bytes = [0; 32];
    bytes[0] = 17;
    bytes
};

fn request(value: &str) -> RequestId {
    RequestId::new(value).expect("bounded request ID")
}

fn offer(value: &str) -> MakerOfferId {
    MakerOfferId::new(value).expect("bounded offer ID")
}

fn xmr_route() -> MakerRouteV1 {
    MakerRouteV1::new(Pair::Monero, SwapDirection::TakerSellsLez).unwrap()
}

fn configure_and_publish(store: &mut SqliteSwapStore, prefix: &str, offer_id: &MakerOfferId) {
    let route = xmr_route();
    store
        .configure_maker_pair(
            &request(&format!("{prefix}-pair-create")),
            None,
            &MakerPairConfigurationV1::new(
                route,
                false,
                MakerPriceSourceKind::Local,
                XMR_PICONERO,
                XMR_PICONERO,
                300,
            )
            .unwrap(),
        )
        .unwrap();
    store
        .set_local_price(
            &request(&format!("{prefix}-price-create")),
            None,
            &LocalPriceV1::new(route, 1, 1_000_000_000).unwrap(),
        )
        .unwrap();
    store
        .configure_maker_pair(
            &request(&format!("{prefix}-pair-enable")),
            Some(1),
            &MakerPairConfigurationV1::new(
                route,
                true,
                MakerPriceSourceKind::Local,
                XMR_PICONERO,
                XMR_PICONERO,
                300,
            )
            .unwrap(),
        )
        .unwrap();
    store
        .publish_local_offer(&request(&format!("{prefix}-publish")), offer_id, route, 100)
        .unwrap();
}

struct ProofFixture {
    maker_wire: Vec<u8>,
    taker_wire: Vec<u8>,
    view_public: [u8; 32],
    spend_public: [u8; 32],
    address: String,
    maker_transcript_commitment: [u8; 32],
    taker_transcript_commitment: [u8; 32],
}

fn proofs() -> &'static ProofFixture {
    static FIXTURE: OnceLock<ProofFixture> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let maker = scalar(11);
        let taker = scalar(13);
        let maker_proof =
            CrossCurveDleqProofV1::prove(&maker, &mut ChaCha20Rng::from_seed([71; 32]))
                .expect("Maker proof");
        let taker_proof =
            CrossCurveDleqProofV1::prove(&taker, &mut ChaCha20Rng::from_seed([72; 32]))
                .expect("Taker proof");
        let view = MoneroPrivateViewKey::from_monero_little_endian(VIEW_KEY_BYTES)
            .expect("private view key");
        let address = MoneroSharedAddressV1::derive(
            MoneroAddressNetworkV1::Regtest,
            &maker_proof,
            &taker_proof,
            &view,
        )
        .expect("shared address");
        ProofFixture {
            maker_wire: maker_proof.to_wire_bytes().expect("Maker proof wire"),
            taker_wire: taker_proof.to_wire_bytes().expect("Taker proof wire"),
            view_public: address.public_view_key(),
            spend_public: address.public_spend_key(),
            address: address.address_string(),
            maker_transcript_commitment: maker_proof.transcript_commitment(),
            taker_transcript_commitment: taker_proof.transcript_commitment(),
        }
    })
}

fn scalar(value: u8) -> CrossCurveScalar {
    let mut bytes = [0; 32];
    bytes[0] = value;
    CrossCurveScalar::from_monero_little_endian(bytes).expect("fixture scalar")
}

fn public_key(secret: [u8; 32]) -> [u8; 33] {
    let secret = SecretKey::from_slice(&secret).expect("fixture secret");
    PublicKey::from_secret_key(&Secp256k1::new(), &secret).serialize()
}

fn sign(secret: [u8; 32], commitment: [u8; 32]) -> [u8; 64] {
    let secp = Secp256k1::new();
    let secret = SecretKey::from_slice(&secret).expect("fixture secret");
    secp.sign_schnorr_no_aux_rand(
        &Message::from_digest(commitment),
        &Keypair::from_secret_key(&secp, &secret),
    )
    .serialize()
}

fn agreement_body(
    swap_id: [u8; 32],
    direction: XmrSwapDirectionV1,
    xmr_piconero: u64,
    lez_units: u128,
    message_marker: u8,
) -> XmrAgreementBodyV1 {
    let proof = proofs();
    let participants = XmrParticipantsV1::new(
        XmrParticipantIdentityV1::new(
            [21; 32],
            public_key(MAKER_AGREEMENT_SECRET),
            public_key(MAKER_CLAIM_SECRET),
            public_key(MAKER_REFUND_SECRET),
        ),
        XmrParticipantIdentityV1::new(
            [22; 32],
            public_key(TAKER_AGREEMENT_SECRET),
            public_key(TAKER_CLAIM_SECRET),
            public_key(TAKER_REFUND_SECRET),
        ),
    );
    let claim_key = participants
        .claim_aggregate_x_only_key()
        .expect("claim aggregate");
    let refund_key = participants
        .refund_aggregate_x_only_key()
        .expect("refund aggregate");
    XmrAgreementBodyV1::new(
        direction,
        XmrNamedProfileV1::AcceleratedRegtest,
        swap_id,
        participants,
        XmrMoneroTermsV1::new(
            MoneroAddressNetworkV1::Regtest,
            [31; 32],
            xmr_piconero,
            10,
            proof.maker_wire.clone(),
            proof.taker_wire.clone(),
            proof.view_public,
            proof.spend_public,
            proof.address.clone(),
        ),
        XmrLezTermsV1::new(
            [40; 32],
            [41; 32],
            [42; 8],
            [43; 8],
            2,
            [44; 32],
            [45; 32],
            [22; 32],
            [21; 32],
            claim_key,
            XmrLezTermsV1::authority_account_for_key(claim_key),
            refund_key,
            XmrLezTermsV1::authority_account_for_key(refund_key),
            proof.maker_transcript_commitment,
            proof.taker_transcript_commitment,
            lez_units,
        ),
        XmrMessagesV1::new(
            [message_marker; 32],
            [message_marker.wrapping_add(1); 32],
            [message_marker.wrapping_add(2); 32],
        ),
        XmrWindowsV1::new(1_000_000, 2_000_000, 3_000_000),
    )
}

fn canonical_stage_a(
    offer_commitment: [u8; 32],
    reservation_id: &RequestId,
    xmr_piconero: u64,
    lez_units: u128,
    message_marker: u8,
) -> Vec<u8> {
    let body = agreement_body(
        maker_xmr_chat_swap_id(&offer_commitment, reservation_id),
        XmrSwapDirectionV1::TakerSellsLez,
        xmr_piconero,
        lez_units,
        message_marker,
    );
    let validated = ValidatedXmrAgreementBodyV1::validate(body).expect("valid XMR Stage A");
    let commitment = validated.commitment();
    validated
        .attach_signatures(
            sign(MAKER_AGREEMENT_SECRET, commitment),
            sign(TAKER_AGREEMENT_SECRET, commitment),
        )
        .expect("dual-signed XMR Stage A")
        .encode_wire()
        .expect("canonical XMR Stage-A wire")
}

fn primitive_stage_a(body: XmrAgreementBodyV1) -> Vec<u8> {
    let commitment = body.commitment();
    XmrAgreementRecordV1::from_parts(
        XMR_AGREEMENT_SCHEMA_V1,
        body,
        commitment,
        sign(MAKER_AGREEMENT_SECRET, commitment),
        sign(TAKER_AGREEMENT_SECRET, commitment),
    )
    .encode_wire()
    .expect("bounded primitive Stage-A wire")
}

#[test]
fn xmr_stage_a_is_quote_bound_one_winner_exact_replay_and_restart_safe() {
    let run = tempdir().expect("isolated XMR negotiation store");
    let database = run.path().join("xmr-maker-negotiation.sqlite3");
    let offer_id = offer("xmr-negotiation-offer-001");
    let reservation_id = request("xmr-negotiation-reservation-001");
    let stage_request = request("xmr-negotiation-stage-001");
    let wire = canonical_stage_a(
        OFFER_COMMITMENT,
        &reservation_id,
        XMR_PICONERO,
        LEZ_UNITS,
        51,
    );
    let negotiation = MakerXmrNegotiationV1::stage_a(
        reservation_id.clone(),
        OFFER_COMMITMENT,
        XMR_PICONERO,
        LEZ_UNITS,
        101,
        wire.clone(),
    )
    .expect("bounded untrusted Stage-A candidate");
    let debug = format!("{negotiation:?}");
    assert!(debug.contains("<redacted>"));
    assert!(debug.len() < 512, "Debug must not disclose Stage-A wire");

    let mut store = SqliteSwapStore::open(&database).unwrap();
    configure_and_publish(&mut store, "xmr-negotiation", &offer_id);
    let commit = store
        .stage_xmr_maker_negotiation(&stage_request, &offer_id, 1, &negotiation)
        .unwrap();
    assert_eq!(commit.revision(), 2);
    assert!(!commit.was_replay());
    assert!(
        store
            .stage_xmr_maker_negotiation(&stage_request, &offer_id, 1, &negotiation)
            .unwrap()
            .was_replay()
    );

    let conflicting_wire = canonical_stage_a(
        OFFER_COMMITMENT,
        &reservation_id,
        XMR_PICONERO,
        LEZ_UNITS,
        61,
    );
    let conflict = MakerXmrNegotiationV1::stage_a(
        reservation_id.clone(),
        OFFER_COMMITMENT,
        XMR_PICONERO,
        LEZ_UNITS,
        101,
        conflicting_wire,
    )
    .unwrap();
    assert!(matches!(
        store.stage_xmr_maker_negotiation(&stage_request, &offer_id, 1, &conflict),
        Err(StoreError::MakerOfferRequestConflict)
    ));

    drop(store);
    let mut reopened = SqliteSwapStore::open(&database).unwrap();
    let recovered = reopened
        .load_xmr_maker_negotiation(&offer_id)
        .unwrap()
        .expect("Stage A survives reopen");
    assert_eq!(
        recovered.status(),
        MakerXmrNegotiationStatus::StageAAccepted
    );
    assert_eq!(recovered.reservation_id(), &reservation_id);
    assert_eq!(recovered.offer_commitment(), &OFFER_COMMITMENT);
    assert_eq!(recovered.foreign_units(), XMR_PICONERO);
    assert_eq!(recovered.lez_units(), LEZ_UNITS);
    assert_eq!(recovered.stage_a_wire(), wire);
    assert_eq!(
        recovered.swap_id(),
        maker_xmr_chat_swap_id(&OFFER_COMMITMENT, &reservation_id)
    );
    let offer = &reopened.list_maker_offer_history(101).unwrap()[0];
    assert_eq!(offer.status(), MakerOfferStatus::Reserved);
    assert_eq!(offer.revision(), 2);
    assert_eq!(offer.reservation_id(), Some(&reservation_id));
    assert_eq!(offer.swap_id(), None);
    assert!(
        reopened
            .stage_xmr_maker_negotiation(&stage_request, &offer_id, 1, &negotiation)
            .unwrap()
            .was_replay()
    );
}

#[test]
fn malformed_wrong_direction_signature_identity_and_quote_are_zero_write() {
    let run = tempdir().expect("isolated invalid XMR negotiation store");
    let database = run.path().join("invalid-xmr-maker-negotiation.sqlite3");
    let offer_id = offer("xmr-invalid-offer-001");
    let reservation_id = request("xmr-invalid-reservation-001");
    let reusable_request = request("xmr-invalid-stage-001");
    let mut store = SqliteSwapStore::open(&database).unwrap();
    configure_and_publish(&mut store, "xmr-invalid", &offer_id);

    let valid = canonical_stage_a(
        OFFER_COMMITMENT,
        &reservation_id,
        XMR_PICONERO,
        LEZ_UNITS,
        71,
    );
    let mut malformed = valid.clone();
    malformed.truncate(malformed.len() - 1);
    let mut wrong_signature = valid;
    *wrong_signature.last_mut().unwrap() ^= 1;
    let wrong_direction = primitive_stage_a(agreement_body(
        maker_xmr_chat_swap_id(&OFFER_COMMITMENT, &reservation_id),
        XmrSwapDirectionV1::TakerSellsXmr,
        XMR_PICONERO,
        LEZ_UNITS,
        71,
    ));
    let wrong_identity = primitive_stage_a(agreement_body(
        [0xa5; 32],
        XmrSwapDirectionV1::TakerSellsLez,
        XMR_PICONERO,
        LEZ_UNITS,
        71,
    ));
    let wrong_quote = canonical_stage_a(
        OFFER_COMMITMENT,
        &reservation_id,
        XMR_PICONERO + 1,
        LEZ_UNITS,
        71,
    );

    for wire in [
        malformed,
        wrong_signature,
        wrong_direction,
        wrong_identity,
        wrong_quote,
    ] {
        let candidate = MakerXmrNegotiationV1::stage_a(
            reservation_id.clone(),
            OFFER_COMMITMENT,
            XMR_PICONERO,
            LEZ_UNITS,
            101,
            wire,
        )
        .expect("bounded candidate defers semantic wire validation to the store");
        assert!(
            store
                .stage_xmr_maker_negotiation(&reusable_request, &offer_id, 1, &candidate)
                .is_err()
        );
        assert_eq!(store.load_xmr_maker_negotiation(&offer_id).unwrap(), None);
        let durable = &store.list_maker_offer_history(101).unwrap()[0];
        assert_eq!(durable.status(), MakerOfferStatus::Active);
        assert_eq!(durable.revision(), 1);
    }

    let withdrawn = store
        .withdraw_maker_offer(&reusable_request, &offer_id, 1)
        .expect("every failed stage rolled back the global request identity");
    assert_eq!(withdrawn.revision(), 2);
}

#[test]
#[allow(clippy::too_many_lines)]
fn xmr_stage_a_transaction_rolls_back_and_concurrent_staging_has_one_winner() {
    let run = tempdir().expect("isolated rollback/concurrency XMR store");
    let rollback_database = run.path().join("xmr-stage-rollback.sqlite3");
    let rollback_offer = offer("xmr-rollback-offer-001");
    let reservation_id = request("xmr-rollback-reservation-001");
    let wire = canonical_stage_a(
        OFFER_COMMITMENT,
        &reservation_id,
        XMR_PICONERO,
        LEZ_UNITS,
        81,
    );
    let candidate = MakerXmrNegotiationV1::stage_a(
        reservation_id,
        OFFER_COMMITMENT,
        XMR_PICONERO,
        LEZ_UNITS,
        101,
        wire,
    )
    .unwrap();
    let mut store = SqliteSwapStore::open(&rollback_database).unwrap();
    configure_and_publish(&mut store, "xmr-rollback", &rollback_offer);
    let raw = Connection::open(&rollback_database).unwrap();
    raw.execute_batch(
        "CREATE TRIGGER fail_xmr_negotiation_stage
         BEFORE INSERT ON maker_application_mutations
         WHEN NEW.operation = 'xmr_negotiation_stage'
         BEGIN SELECT RAISE(ABORT, 'forced XMR Stage-A rollback'); END;",
    )
    .unwrap();
    assert!(matches!(
        store.stage_xmr_maker_negotiation(
            &request("xmr-rollback-stage-001"),
            &rollback_offer,
            1,
            &candidate,
        ),
        Err(StoreError::Sqlite(_))
    ));
    let rolled_back: (String, i64, i64) = raw
        .query_row(
            "SELECT state, revision,
                    (SELECT COUNT(*) FROM maker_xmr_negotiations)
               FROM maker_offers WHERE offer_id = ?1",
            params![rollback_offer.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(rolled_back, ("active".into(), 1, 0));
    drop(raw);
    drop(store);

    let concurrent_database = run.path().join("xmr-stage-concurrent.sqlite3");
    let concurrent_offer = offer("xmr-concurrent-offer-001");
    let mut store = SqliteSwapStore::open(&concurrent_database).unwrap();
    configure_and_publish(&mut store, "xmr-concurrent", &concurrent_offer);
    drop(store);
    let barrier = Arc::new(Barrier::new(2));
    let mut workers = Vec::new();
    for marker in [91_u8, 101_u8] {
        let database = concurrent_database.clone();
        let offer_id = concurrent_offer.clone();
        let barrier = Arc::clone(&barrier);
        workers.push(thread::spawn(move || {
            let reservation_id = request(&format!("xmr-concurrent-reservation-{marker}"));
            let wire = canonical_stage_a(
                OFFER_COMMITMENT,
                &reservation_id,
                XMR_PICONERO,
                LEZ_UNITS,
                marker,
            );
            let candidate = MakerXmrNegotiationV1::stage_a(
                reservation_id,
                OFFER_COMMITMENT,
                XMR_PICONERO,
                LEZ_UNITS,
                101,
                wire,
            )
            .unwrap();
            let mut store = SqliteSwapStore::open(&database).unwrap();
            barrier.wait();
            store.stage_xmr_maker_negotiation(
                &request(&format!("xmr-concurrent-stage-{marker}")),
                &offer_id,
                1,
                &candidate,
            )
        }));
    }
    let outcomes: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().expect("staging worker"))
        .collect();
    assert_eq!(outcomes.iter().filter(|outcome| outcome.is_ok()).count(), 1);
    assert_eq!(
        outcomes.iter().filter(|outcome| outcome.is_err()).count(),
        1
    );

    let store = SqliteSwapStore::open(&concurrent_database).unwrap();
    let winner = store
        .load_xmr_maker_negotiation(&concurrent_offer)
        .unwrap()
        .expect("one durable winner");
    let offer = &store.list_maker_offer_history(101).unwrap()[0];
    assert_eq!(offer.status(), MakerOfferStatus::Reserved);
    assert_eq!(offer.revision(), 2);
    assert_eq!(offer.reservation_id(), Some(winner.reservation_id()));
}

fn private_view_key() -> MoneroPrivateViewKey {
    MoneroPrivateViewKey::from_monero_little_endian(VIEW_KEY_BYTES).expect("private view key")
}

fn signer_round(
    context: &lez_adaptor_signature::AdaptorSessionContext,
    maker_secret: [u8; 32],
    taker_secret: [u8; 32],
) -> (XmrSessionTranscriptV1, [u8; 32], [u8; 32], [u8; 65]) {
    let mut maker = AdaptorSigner::new(context.clone(), SigningRole::Maker, maker_secret)
        .expect("Maker signer");
    let mut taker = AdaptorSigner::new(context.clone(), SigningRole::Taker, taker_secret)
        .expect("Taker signer");
    let maker_commitment = maker.nonce_commitment();
    let taker_commitment = taker.nonce_commitment();
    maker
        .accept_peer_commitment(taker_commitment)
        .expect("Maker accepts commitment");
    taker
        .accept_peer_commitment(maker_commitment)
        .expect("Taker accepts commitment");
    let maker_nonce = maker.public_nonce().expect("Maker nonce");
    let taker_nonce = taker.public_nonce().expect("Taker nonce");
    maker.accept_peer_nonce(taker_nonce).expect("Maker opening");
    taker.accept_peer_nonce(maker_nonce).expect("Taker opening");
    let maker_partial = maker.create_partial_signature().expect("Maker partial");
    let taker_partial = taker.create_partial_signature().expect("Taker partial");
    maker
        .accept_peer_partial_signature(taker_partial)
        .expect("Maker verifies Taker partial");
    taker
        .accept_peer_partial_signature(maker_partial)
        .expect("Taker verifies Maker partial");
    let presignature = maker.presignature().expect("aggregate presignature");
    (
        XmrSessionTranscriptV1::new(maker_commitment, taker_commitment, maker_nonce, taker_nonce),
        maker_partial,
        taker_partial,
        presignature,
    )
}

fn canonical_stage_b(agreement: &XmrAgreementV1) -> (XmrActivatedAgreementV1, Vec<u8>) {
    let claim_context = agreement
        .claim_session_descriptor()
        .context()
        .expect("claim context");
    let refund_context = agreement
        .refund_session_descriptor()
        .context()
        .expect("refund context");
    let (claim_transcript, maker_claim_partial, taker_claim_partial, _) =
        signer_round(&claim_context, MAKER_CLAIM_SECRET, TAKER_CLAIM_SECRET);
    let (refund_transcript, maker_refund_partial, taker_refund_partial, refund_presignature) =
        signer_round(&refund_context, MAKER_REFUND_SECRET, TAKER_REFUND_SECRET);
    let partial_context = agreement
        .claim_partial_context_binding(&claim_transcript, maker_claim_partial)
        .expect("claim partial context");
    let partial_commitment = agreement
        .commit_taker_claim_partial(&claim_transcript, maker_claim_partial, taker_claim_partial)
        .expect("Taker partial commitment");
    let body = XmrActivationBodyV1::new(
        agreement.agreement_commitment(),
        agreement.claim_context_binding(),
        claim_transcript,
        maker_claim_partial,
        partial_context,
        partial_commitment,
        agreement.refund_context_binding(),
        refund_transcript,
        maker_refund_partial,
        taker_refund_partial,
        refund_presignature,
    );
    let validated = ValidatedXmrActivationBodyV1::validate(agreement, body, &private_view_key())
        .expect("validated Stage B");
    let commitment = validated.commitment();
    let activation = validated
        .attach_signatures(
            sign(MAKER_AGREEMENT_SECRET, commitment),
            sign(TAKER_AGREEMENT_SECRET, commitment),
        )
        .expect("dual-signed Stage B");
    let wire = activation.encode_wire().expect("canonical Stage-B wire");
    (activation, wire)
}

#[test]
#[allow(clippy::too_many_lines)]
fn schema_20_xmr_stage_a_migrates_without_becoming_executable() {
    let run = tempdir().expect("isolated schema-20 XMR migration store");
    let database = run.path().join("xmr-schema-20.sqlite3");
    let offer_id = offer("xmr-schema-20-offer-001");
    let reservation_id = request("xmr-schema-20-reservation-001");
    let stage_request = request("xmr-schema-20-stage-001");
    let wire = canonical_stage_a(
        OFFER_COMMITMENT,
        &reservation_id,
        XMR_PICONERO,
        LEZ_UNITS,
        91,
    );
    let candidate = MakerXmrNegotiationV1::stage_a(
        reservation_id,
        OFFER_COMMITMENT,
        XMR_PICONERO,
        LEZ_UNITS,
        101,
        wire,
    )
    .unwrap();
    let mut store = SqliteSwapStore::open(&database).unwrap();
    configure_and_publish(&mut store, "xmr-schema-20", &offer_id);
    store
        .stage_xmr_maker_negotiation(&stage_request, &offer_id, 1, &candidate)
        .unwrap();
    drop(store);

    let raw = Connection::open(&database).unwrap();
    raw.execute_batch(
        "DROP INDEX maker_xmr_negotiations_state_reservation;
         ALTER TABLE maker_xmr_negotiations RENAME TO maker_xmr_negotiations_current;
         CREATE TABLE maker_xmr_negotiations (
             offer_id TEXT PRIMARY KEY NOT NULL,
             reservation_id TEXT NOT NULL UNIQUE,
             payload_version INTEGER NOT NULL CHECK (payload_version = 1),
             offer_commitment BLOB NOT NULL CHECK (length(offer_commitment) = 32),
             maker_agreement_identity BLOB NOT NULL CHECK (length(maker_agreement_identity) = 33),
             taker_agreement_identity BLOB NOT NULL CHECK (length(taker_agreement_identity) = 33),
             foreign_units INTEGER NOT NULL CHECK (foreign_units > 0),
             lez_units BLOB NOT NULL CHECK (length(lez_units) = 16),
             reserved_at_unix_seconds INTEGER NOT NULL CHECK (reserved_at_unix_seconds > 0),
             agreement_commitment BLOB NOT NULL CHECK (length(agreement_commitment) = 32),
             stage_a_wire BLOB NOT NULL CHECK (length(stage_a_wire) BETWEEN 1 AND 276480),
             state TEXT NOT NULL CHECK (state = 'stage_a_accepted'),
             updated_request_id TEXT NOT NULL,
             FOREIGN KEY (offer_id) REFERENCES maker_offers(offer_id) ON DELETE RESTRICT,
             CHECK (maker_agreement_identity != taker_agreement_identity)
         ) STRICT;
         INSERT INTO maker_xmr_negotiations (
             offer_id, reservation_id, payload_version, offer_commitment,
             maker_agreement_identity, taker_agreement_identity, foreign_units,
             lez_units, reserved_at_unix_seconds, agreement_commitment, stage_a_wire,
             state, updated_request_id
         )
         SELECT offer_id, reservation_id, payload_version, offer_commitment,
                maker_agreement_identity, taker_agreement_identity, foreign_units,
                lez_units, reserved_at_unix_seconds, agreement_commitment, stage_a_wire,
                state, updated_request_id
           FROM maker_xmr_negotiations_current;
         DROP TABLE maker_xmr_negotiations_current;
         CREATE INDEX maker_xmr_negotiations_state_reservation
             ON maker_xmr_negotiations (state, reservation_id);
         PRAGMA user_version = 20;",
    )
    .unwrap();
    drop(raw);

    let reopened = SqliteSwapStore::open(&database).expect("schema 20 migrates");
    let migrated = reopened
        .load_xmr_maker_negotiation(&offer_id)
        .unwrap()
        .expect("Stage A preserved");
    assert_eq!(migrated, candidate);
    assert_eq!(migrated.status(), MakerXmrNegotiationStatus::StageAAccepted);
    assert_eq!(migrated.activation_wire(), None);
    assert_eq!(migrated.coordinator_swap_id(), None);
    assert_eq!(
        reopened
            .load(&SwapId::new("0".repeat(64)).unwrap())
            .unwrap(),
        None
    );
}

#[test]
#[allow(clippy::too_many_lines)]
fn xmr_stage_b_completion_is_atomic_replay_safe_and_mints_one_actor() {
    let run = tempdir().expect("isolated XMR Stage-B completion store");
    let database = run.path().join("xmr-stage-b-completion.sqlite3");
    let offer_id = offer("xmr-stage-b-offer-001");
    let reservation_id = request("xmr-stage-b-reservation-001");
    let stage_request = request("xmr-stage-b-stage-001");
    let complete_request = request("xmr-stage-b-complete-001");
    let stage_a_wire = canonical_stage_a(
        OFFER_COMMITMENT,
        &reservation_id,
        XMR_PICONERO,
        LEZ_UNITS,
        111,
    );
    let agreement = XmrAgreementV1::from_wire(&stage_a_wire).expect("canonical Stage A");
    let (activation, activation_wire) = canonical_stage_b(&agreement);
    let initial = activation
        .initial_coordinator(&agreement)
        .expect("SDK-derived initial coordinator");
    let candidate = MakerXmrNegotiationV1::stage_a(
        reservation_id.clone(),
        OFFER_COMMITMENT,
        XMR_PICONERO,
        LEZ_UNITS,
        101,
        stage_a_wire,
    )
    .unwrap();
    let actor = MakerActorManifestV1::new(
        initial.id().clone(),
        MakerActorKindV1::Monero,
        run.path().join("maker-xmr-actor-config.json"),
        [0xa1; 32],
        run.path().join("xmr-reference-actor"),
        [0xb2; 32],
        run.path().join("maker-xmr-actor.sqlite3"),
    )
    .unwrap();
    MakerXmrActivationAcceptance::new(
        &initial,
        Participant::Maker,
        &agreement,
        &activation,
        activation_wire.clone(),
        1_000,
    )
    .expect("exact signed funding cutoff is inclusive");
    assert!(
        MakerXmrActivationAcceptance::new(
            &initial,
            Participant::Maker,
            &agreement,
            &activation,
            activation_wire.clone(),
            1_001,
        )
        .is_err(),
        "activation after the signed funding cutoff must fail closed"
    );
    let accepted = MakerXmrActivationAcceptance::new(
        &initial,
        Participant::Maker,
        &agreement,
        &activation,
        activation_wire.clone(),
        500,
    )
    .expect("validated Maker Stage-B acceptance after advertisement TTL");

    let mut store = SqliteSwapStore::open(&database).unwrap();
    configure_and_publish(&mut store, "xmr-stage-b", &offer_id);
    store
        .stage_xmr_maker_negotiation(&stage_request, &offer_id, 1, &candidate)
        .unwrap();
    assert_eq!(store.load(initial.id()).unwrap(), None);
    assert!(store.list_maker_actor_processes().unwrap().is_empty());

    let raw = Connection::open(&database).unwrap();
    raw.execute_batch(
        "CREATE TRIGGER fail_xmr_negotiation_completion
         BEFORE INSERT ON maker_application_mutations
         WHEN NEW.operation = 'xmr_negotiation_complete'
         BEGIN SELECT RAISE(ABORT, 'forced XMR Stage-B rollback'); END;",
    )
    .unwrap();
    assert!(matches!(
        store.complete_maker_xmr_negotiation_and_register_actor(
            &complete_request,
            &offer_id,
            2,
            &reservation_id,
            &accepted,
            &initial,
            &actor,
            103,
        ),
        Err(StoreError::Sqlite(_))
    ));
    let rolled_back: (String, i64, String, i64, i64, i64) = raw
        .query_row(
            "SELECT o.state, o.revision, n.state,
                    n.activation_wire IS NOT NULL,
                    (SELECT COUNT(*) FROM swaps WHERE id = ?2),
                    (SELECT COUNT(*) FROM maker_actor_processes WHERE swap_id = ?2)
               FROM maker_offers o
               JOIN maker_xmr_negotiations n USING (offer_id)
              WHERE o.offer_id = ?1",
            params![offer_id.as_str(), initial.id().as_str()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(
        rolled_back,
        ("reserved".into(), 2, "stage_a_accepted".into(), 0, 0, 0)
    );
    raw.execute_batch("DROP TRIGGER fail_xmr_negotiation_completion;")
        .unwrap();

    let committed = store
        .complete_maker_xmr_negotiation_and_register_actor(
            &complete_request,
            &offer_id,
            2,
            &reservation_id,
            &accepted,
            &initial,
            &actor,
            103,
        )
        .unwrap();
    assert_eq!(committed.offer_revision(), 3);
    assert!(!committed.was_replay());
    drop(store);

    let mut reopened = SqliteSwapStore::open(&database).unwrap();
    let completed = reopened
        .load_xmr_maker_negotiation(&offer_id)
        .unwrap()
        .unwrap();
    assert_eq!(completed.status(), MakerXmrNegotiationStatus::Activated);
    assert_eq!(
        completed.activation_wire(),
        Some(activation_wire.as_slice())
    );
    let offer = &reopened.list_maker_offer_history(1_000).unwrap()[0];
    assert_eq!(offer.status(), MakerOfferStatus::Consumed);
    assert_eq!(offer.revision(), 3);
    assert_eq!(offer.swap_id(), Some(initial.id().as_str()));
    assert_eq!(reopened.load(initial.id()).unwrap(), Some(initial.clone()));
    let actors = reopened.list_maker_actor_processes().unwrap();
    assert_eq!(actors.len(), 1);
    assert_eq!(actors[0].manifest(), &actor);

    let replay = reopened
        .complete_maker_xmr_negotiation_and_register_actor(
            &complete_request,
            &offer_id,
            2,
            &reservation_id,
            &accepted,
            &initial,
            &actor,
            10_000,
        )
        .unwrap();
    assert_eq!(replay.offer_revision(), 3);
    assert!(replay.was_replay());

    let changed_actor = MakerActorManifestV1::new(
        initial.id().clone(),
        MakerActorKindV1::Monero,
        run.path().join("changed-maker-xmr-actor-config.json"),
        [0xa1; 32],
        run.path().join("xmr-reference-actor"),
        [0xb2; 32],
        run.path().join("changed-maker-xmr-actor.sqlite3"),
    )
    .unwrap();
    assert!(matches!(
        reopened.complete_maker_xmr_negotiation_and_register_actor(
            &complete_request,
            &offer_id,
            2,
            &reservation_id,
            &accepted,
            &initial,
            &changed_actor,
            10_000,
        ),
        Err(StoreError::MakerOfferRequestConflict)
    ));

    let changed_acceptance = MakerXmrActivationAcceptance::new(
        &initial,
        Participant::Maker,
        &agreement,
        &activation,
        activation_wire.clone(),
        501,
    )
    .expect("changed but valid acceptance time");
    assert!(matches!(
        reopened.complete_maker_xmr_negotiation_and_register_actor(
            &complete_request,
            &offer_id,
            2,
            &reservation_id,
            &changed_acceptance,
            &initial,
            &actor,
            10_000,
        ),
        Err(StoreError::MakerOfferRequestConflict)
    ));

    raw.execute(
        "UPDATE maker_xmr_negotiations SET stage_a_wire = zeroblob(length(stage_a_wire)) WHERE offer_id = ?1",
        params![offer_id.as_str()],
    )
    .unwrap();
    assert!(matches!(
        reopened.complete_maker_xmr_negotiation_and_register_actor(
            &complete_request,
            &offer_id,
            2,
            &reservation_id,
            &accepted,
            &initial,
            &actor,
            10_000,
        ),
        Err(StoreError::CorruptMakerOffer)
    ));
    raw.execute(
        "UPDATE maker_xmr_negotiations SET stage_a_wire = ?1 WHERE offer_id = ?2",
        params![agreement.encode_wire().unwrap(), offer_id.as_str()],
    )
    .unwrap();
    raw.execute(
        "UPDATE maker_offers SET pair = 'bitcoin' WHERE offer_id = ?1",
        params![offer_id.as_str()],
    )
    .unwrap();
    assert!(matches!(
        reopened.complete_maker_xmr_negotiation_and_register_actor(
            &complete_request,
            &offer_id,
            2,
            &reservation_id,
            &accepted,
            &initial,
            &actor,
            10_000,
        ),
        Err(StoreError::CorruptMakerOffer)
    ));
}
