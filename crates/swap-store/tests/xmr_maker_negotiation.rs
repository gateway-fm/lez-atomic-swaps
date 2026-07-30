use std::sync::{Arc, Barrier, OnceLock};
use std::thread;

use lez_bridge_protocol::RequestId;
use lez_swap_core::{Pair, SwapDirection};
use lez_swap_store::{
    LocalPriceV1, MakerOfferId, MakerOfferStatus, MakerPairConfigurationV1, MakerPriceSourceKind,
    MakerRouteV1, MakerXmrNegotiationStatus, MakerXmrNegotiationV1, SqliteSwapStore, StoreError,
    maker_xmr_chat_swap_id,
};
use lez_xmr_swap_sdk::{
    CrossCurveDleqProofV1, CrossCurveScalar, MoneroAddressNetworkV1, MoneroPrivateViewKey,
    MoneroSharedAddressV1, ValidatedXmrAgreementBodyV1, XMR_AGREEMENT_SCHEMA_V1,
    XmrAgreementBodyV1, XmrAgreementRecordV1, XmrLezTermsV1, XmrMessagesV1, XmrMoneroTermsV1,
    XmrNamedProfileV1, XmrParticipantIdentityV1, XmrParticipantsV1, XmrSwapDirectionV1,
    XmrWindowsV1,
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
        XmrWindowsV1::new(10_000, 20_000, 30_000),
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
