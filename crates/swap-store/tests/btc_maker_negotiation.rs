use bitcoin::hashes::Hash as _;
use bitcoin::secp256k1::{Keypair, Message, PublicKey, Secp256k1, SecretKey};
use bitcoin::{Amount, OutPoint, ScriptBuf, TxOut, Txid};
use lez_bridge_protocol::RequestId;
use lez_btc_swap_sdk::{
    AdaptorSessionContext, BtcAgreementBodyV1, BtcAgreementDraftV1, BtcAgreementV1,
    BtcChainPolicyV1, BtcClaimTermsV1, BtcFundingTermsV1, BtcLezTermsV1,
    BtcMakerAgreementProposalV1, BtcP2trTermsV1, BtcParticipantIdentityV1, BtcParticipantsV1,
    BtcRecoveryPlanV1, CooperativeKeyPathSpend, CsvBlockDelay, P2trSwapOutput, RefundXOnlyKey,
    TwoPartyAggregateKey,
};
use lez_swap_core::{Pair, Participant, SwapCoordinator, SwapDirection};
use lez_swap_store::{
    BtcAgreementAcceptance, LocalPriceV1, MakerActorKindV1, MakerActorManifestV1,
    MakerActorScheduleState, MakerBtcNegotiationStatus, MakerBtcNegotiationV1, MakerOfferId,
    MakerOfferStatus, MakerPairConfigurationV1, MakerPriceSourceKind, MakerRouteV1,
    SqliteSwapStore, StoreError, maker_btc_chat_swap_id,
};
use rusqlite::{Connection, params};
use tempfile::tempdir;

const OFFER_COMMITMENT: [u8; 32] = [0x61; 32];
const FUNDING_VALUE_SAT: u64 = 100_000;
const CLAIM_VALUE_SAT: u64 = 99_000;
const LEZ_UNITS: u128 = 5_000;

fn request(value: &str) -> RequestId {
    RequestId::new(value).expect("bounded request ID")
}

fn bitcoin_route() -> MakerRouteV1 {
    MakerRouteV1::new(Pair::Bitcoin, SwapDirection::TakerSellsForeign).unwrap()
}

struct NegotiationFixture {
    maker_identity: [u8; 33],
    taker_identity: [u8; 33],
    agreement_commitment: [u8; 32],
    proposal_wire: Vec<u8>,
    final_wire: Vec<u8>,
    alternate_final_wire: Vec<u8>,
    coordinator: SwapCoordinator,
}

fn secret(value: u8) -> SecretKey {
    SecretKey::from_slice(&[value; 32]).expect("valid deterministic secret")
}

fn compressed_public_key(secret: &SecretKey) -> [u8; 33] {
    PublicKey::from_secret_key(&Secp256k1::new(), secret).serialize()
}

fn x_only_public_key(secret: &SecretKey) -> [u8; 32] {
    Keypair::from_secret_key(&Secp256k1::new(), secret)
        .x_only_public_key()
        .0
        .serialize()
}

fn claim_destination(secret: &SecretKey) -> Vec<u8> {
    let key = Keypair::from_secret_key(&Secp256k1::new(), secret)
        .x_only_public_key()
        .0;
    ScriptBuf::new_p2tr(&Secp256k1::verification_only(), key, None).into_bytes()
}

fn signature(secret: &SecretKey, commitment: [u8; 32]) -> [u8; 64] {
    Secp256k1::new()
        .sign_schnorr_no_aux_rand(
            &Message::from_digest(commitment),
            &Keypair::from_secret_key(&Secp256k1::new(), secret),
        )
        .serialize()
}

fn alternate_signature(secret: &SecretKey, commitment: [u8; 32]) -> [u8; 64] {
    Secp256k1::new()
        .sign_schnorr_with_aux_rand(
            &Message::from_digest(commitment),
            &Keypair::from_secret_key(&Secp256k1::new(), secret),
            &[0x55; 32],
        )
        .serialize()
}

#[allow(clippy::too_many_lines)]
fn negotiation_fixture(reservation_id: &RequestId, key_offset: u8) -> NegotiationFixture {
    let maker_secret = secret(1 + key_offset);
    let taker_secret = secret(2 + key_offset);
    let maker_refund_secret = secret(3 + key_offset);
    let taker_refund_secret = secret(4 + key_offset);
    let maker_claim_secret = secret(5 + key_offset);
    let taker_claim_secret = secret(6 + key_offset);
    let adaptor_secret = secret(7 + key_offset);
    let maker_identity = compressed_public_key(&maker_secret);
    let taker_identity = compressed_public_key(&taker_secret);
    let maker = BtcParticipantIdentityV1::new(
        [10 + key_offset; 32],
        maker_identity,
        x_only_public_key(&maker_refund_secret),
        claim_destination(&maker_claim_secret),
    );
    let taker = BtcParticipantIdentityV1::new(
        [11 + key_offset; 32],
        taker_identity,
        x_only_public_key(&taker_refund_secret),
        claim_destination(&taker_claim_secret),
    );
    let participants = BtcParticipantsV1::new(maker, taker);
    let adaptor_point = compressed_public_key(&adaptor_secret);
    let aggregate = AdaptorSessionContext::untweaked(
        [maker_identity, taker_identity],
        [30 + key_offset; 32],
        adaptor_point,
        [31 + key_offset; 32],
    )
    .expect("valid aggregate context")
    .output_key();
    let contract = P2trSwapOutput::new(
        TwoPartyAggregateKey::from_bytes(aggregate).unwrap(),
        RefundXOnlyKey::from_bytes(x_only_public_key(&taker_refund_secret)).unwrap(),
        CsvBlockDelay::new(144).unwrap(),
    )
    .unwrap();
    let funding = BtcFundingTermsV1::new([21; 32], 1, FUNDING_VALUE_SAT);
    let spend = CooperativeKeyPathSpend::new(
        &contract,
        OutPoint {
            txid: Txid::from_byte_array(*funding.transaction_id()),
            vout: funding.output_index(),
        },
        Amount::from_sat(funding.value_sat()),
        vec![TxOut {
            value: Amount::from_sat(CLAIM_VALUE_SAT),
            script_pubkey: ScriptBuf::from_bytes(claim_destination(&maker_claim_secret)),
        }],
    )
    .unwrap();
    let lez = BtcLezTermsV1::new(
        [17; 32],
        [18; 32],
        [15; 32],
        [16; 32],
        [12; 32],
        [13; 32],
        [14; 32],
        *participants
            .for_participant(Participant::Maker)
            .lez_owner_account(),
        *participants
            .for_participant(Participant::Taker)
            .lez_owner_account(),
        LEZ_UNITS,
        1_700_000_100_000,
        [19; 32],
    );
    let body = BtcAgreementBodyV1::new(
        maker_btc_chat_swap_id(&OFFER_COMMITMENT, reservation_id),
        SwapDirection::TakerSellsForeign,
        BtcChainPolicyV1::new([8; 32], 6),
        participants,
        adaptor_point,
        lez,
        BtcP2trTermsV1::from_contract(&contract),
        funding,
        BtcClaimTermsV1::from_spend(&spend).unwrap(),
        BtcRecoveryPlanV1::new(
            1_000,
            1_144,
            1_699_999_800,
            1_700_000_100,
            1_700_000_500,
            300,
        ),
    );
    let draft = BtcAgreementDraftV1::validate(body).expect("validated BTC draft");
    let agreement_commitment = draft.commitment();
    let alternate_proposal = BtcMakerAgreementProposalV1::from_parts(
        draft.clone(),
        alternate_signature(&maker_secret, agreement_commitment),
    )
    .expect("alternate valid Maker signature");
    let alternate_final_wire = alternate_proposal
        .complete(signature(&taker_secret, agreement_commitment))
        .expect("alternate countersigned agreement")
        .encode_wire()
        .unwrap();
    let proposal = BtcMakerAgreementProposalV1::from_parts(
        draft,
        signature(&maker_secret, agreement_commitment),
    )
    .expect("maker-signed BTC proposal");
    let proposal_wire = proposal.encode_wire().expect("canonical proposal wire");
    let agreement = proposal
        .complete(signature(&taker_secret, agreement_commitment))
        .expect("taker-countersigned BTC agreement");
    let final_wire = agreement.encode_wire().expect("canonical agreement wire");
    assert_eq!(
        BtcAgreementV1::from_wire(&final_wire)
            .unwrap()
            .encode_wire()
            .unwrap(),
        final_wire
    );
    NegotiationFixture {
        maker_identity,
        taker_identity,
        agreement_commitment,
        proposal_wire,
        final_wire,
        alternate_final_wire,
        coordinator: agreement.coordinator().clone(),
    }
}

fn proposed(
    reservation_id: &RequestId,
    fixture: &NegotiationFixture,
    reserved_at_unix_seconds: u64,
) -> MakerBtcNegotiationV1 {
    MakerBtcNegotiationV1::proposed(
        reservation_id.clone(),
        OFFER_COMMITMENT,
        fixture.maker_identity,
        fixture.taker_identity,
        FUNDING_VALUE_SAT,
        LEZ_UNITS,
        reserved_at_unix_seconds,
        fixture.agreement_commitment,
        fixture.proposal_wire.clone(),
    )
    .unwrap()
}

fn configure_route(store: &mut SqliteSwapStore, route: MakerRouteV1, prefix: &str) {
    store
        .configure_maker_pair(
            &request(&format!("{prefix}-pair-create")),
            None,
            &MakerPairConfigurationV1::new(
                route,
                false,
                MakerPriceSourceKind::Local,
                FUNDING_VALUE_SAT,
                FUNDING_VALUE_SAT,
                300,
            )
            .unwrap(),
        )
        .unwrap();
    store
        .set_local_price(
            &request(&format!("{prefix}-price-create")),
            None,
            &LocalPriceV1::new(route, 1, 20).unwrap(),
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
                FUNDING_VALUE_SAT,
                FUNDING_VALUE_SAT,
                300,
            )
            .unwrap(),
        )
        .unwrap();
}

#[test]
#[allow(clippy::too_many_lines)]
fn btc_maker_negotiation_is_one_winner_restart_safe_and_completes_atomically() {
    let run = tempdir().expect("isolated BTC maker negotiation store");
    let database = run.path().join("btc-maker-negotiation.sqlite3");
    let offer_id = MakerOfferId::new("btc-negotiation-offer-001").unwrap();
    let stage_request = request("btc-negotiation-stage-001");
    let reservation_id = request("btc-negotiation-reservation-001");
    let completion_request = request("btc-negotiation-complete-001");

    let mut store = SqliteSwapStore::open(&database).unwrap();
    configure_route(&mut store, bitcoin_route(), "btc-negotiation-forward");
    let reverse_route = MakerRouteV1::new(Pair::Bitcoin, SwapDirection::TakerSellsLez).unwrap();
    configure_route(&mut store, reverse_route, "btc-negotiation-reverse");
    store
        .publish_local_offer(
            &request("btc-negotiation-publish-001"),
            &offer_id,
            bitcoin_route(),
            100,
        )
        .unwrap();
    let wrong_direction_offer_id = MakerOfferId::new("btc-negotiation-reverse-offer").unwrap();
    store
        .publish_local_offer(
            &request("btc-negotiation-publish-reverse"),
            &wrong_direction_offer_id,
            reverse_route,
            100,
        )
        .unwrap();
    let history = store.list_maker_offer_history(100).unwrap();
    let published = history
        .iter()
        .find(|record| record.offer().id() == &offer_id)
        .unwrap();
    assert_eq!(published.offer().route(), bitcoin_route());
    assert_eq!(published.offer().created_at_unix_seconds(), 100);
    assert_eq!(published.offer().expires_at_unix_seconds(), 400);

    let winner = negotiation_fixture(&reservation_id, 0);
    let conflicting = negotiation_fixture(&reservation_id, 10);
    let losing_reservation = request("btc-negotiation-reservation-loser");
    let losing = negotiation_fixture(&losing_reservation, 20);
    assert!(matches!(
        store.stage_btc_maker_negotiation(
            &request("btc-negotiation-wrong-direction"),
            &wrong_direction_offer_id,
            1,
            &proposed(&reservation_id, &winner, 101),
        ),
        Err(StoreError::MakerOfferSwapMismatch)
    ));
    let staged = proposed(&reservation_id, &winner, 101);
    let stage_commit = store
        .stage_btc_maker_negotiation(&stage_request, &offer_id, 1, &staged)
        .unwrap();
    assert_eq!(stage_commit.revision(), 2);
    assert!(!stage_commit.was_replay());
    assert!(
        store
            .stage_btc_maker_negotiation(&stage_request, &offer_id, 1, &staged)
            .unwrap()
            .was_replay()
    );
    assert!(matches!(
        store.stage_btc_maker_negotiation(
            &stage_request,
            &offer_id,
            1,
            &proposed(&reservation_id, &conflicting, 101),
        ),
        Err(StoreError::MakerOfferRequestConflict)
    ));
    assert!(matches!(
        store.stage_btc_maker_negotiation(
            &request("btc-negotiation-stage-loser"),
            &offer_id,
            1,
            &proposed(&losing_reservation, &losing, 101),
        ),
        Err(StoreError::StaleMakerOffer {
            expected: 1,
            actual: 2
        })
    ));
    drop(store);

    let mut store = SqliteSwapStore::open(&database).unwrap();
    let recovered = store
        .load_btc_maker_negotiation(&offer_id)
        .unwrap()
        .expect("staged BTC proposal survives restart");
    assert_eq!(recovered, staged);
    assert_eq!(recovered.status(), MakerBtcNegotiationStatus::Proposed);
    assert_eq!(recovered.final_agreement_wire(), None);
    assert_eq!(recovered.swap_id(), None);

    let raw = Connection::open(&database).unwrap();
    raw.execute(
        "UPDATE maker_btc_negotiations SET updated_request_id = 'stage-drift' WHERE offer_id = ?1",
        params![offer_id.as_str()],
    )
    .unwrap();
    assert!(matches!(
        store.stage_btc_maker_negotiation(&stage_request, &offer_id, 1, &staged),
        Err(StoreError::CorruptMakerOffer)
    ));
    raw.execute(
        "UPDATE maker_btc_negotiations SET updated_request_id = ?1 WHERE offer_id = ?2",
        params![stage_request.as_str(), offer_id.as_str()],
    )
    .unwrap();
    raw.execute(
        "UPDATE maker_btc_negotiations SET reserved_at_unix_seconds = 400 WHERE offer_id = ?1",
        params![offer_id.as_str()],
    )
    .unwrap();
    assert!(matches!(
        store.stage_btc_maker_negotiation(&stage_request, &offer_id, 1, &staged),
        Err(StoreError::CorruptMakerOffer)
    ));
    raw.execute(
        "UPDATE maker_btc_negotiations SET reserved_at_unix_seconds = 101 WHERE offer_id = ?1",
        params![offer_id.as_str()],
    )
    .unwrap();
    raw.execute(
        "UPDATE maker_offers SET updated_request_id = 'offer-stage-drift' WHERE offer_id = ?1",
        params![offer_id.as_str()],
    )
    .unwrap();
    assert!(matches!(
        store.stage_btc_maker_negotiation(&stage_request, &offer_id, 1, &staged),
        Err(StoreError::CorruptMakerOffer)
    ));
    raw.execute(
        "UPDATE maker_offers SET updated_request_id = ?1 WHERE offer_id = ?2",
        params![stage_request.as_str(), offer_id.as_str()],
    )
    .unwrap();

    let initial = winner.coordinator.clone();
    let final_wire = winner.final_wire.clone();
    let accepted = BtcAgreementAcceptance::new(
        &initial,
        Participant::Maker,
        final_wire.clone(),
        winner.agreement_commitment,
        102,
    )
    .unwrap();
    let actor = MakerActorManifestV1::new(
        initial.id().clone(),
        MakerActorKindV1::Bitcoin,
        run.path().join("maker-btc-actor-config.json"),
        [0xa1; 32],
        run.path().join("btc-reference-actor"),
        [0xb2; 32],
        run.path().join("maker-btc-actor.sqlite3"),
    )
    .unwrap();

    assert!(
        store
            .preflight_maker_btc_scheduled_completion_replay(
                &completion_request,
                &offer_id,
                2,
                &reservation_id,
                &final_wire,
            )
            .unwrap()
            .is_none()
    );
    let early = BtcAgreementAcceptance::new(
        &initial,
        Participant::Maker,
        final_wire.clone(),
        winner.agreement_commitment,
        100,
    )
    .unwrap();
    assert!(matches!(
        store.complete_maker_btc_negotiation_and_register_actor(
            &request("btc-negotiation-too-early"),
            &offer_id,
            2,
            &reservation_id,
            &early,
            &initial,
            &actor,
            103,
        ),
        Err(StoreError::InvalidBtcApplicationState)
    ));
    let expired = BtcAgreementAcceptance::new(
        &initial,
        Participant::Maker,
        final_wire.clone(),
        winner.agreement_commitment,
        400,
    )
    .unwrap();
    assert!(matches!(
        store.complete_maker_btc_negotiation_and_register_actor(
            &request("btc-negotiation-at-expiry"),
            &offer_id,
            2,
            &reservation_id,
            &expired,
            &initial,
            &actor,
            400,
        ),
        Err(StoreError::InvalidBtcApplicationState)
    ));
    let alternate = BtcAgreementAcceptance::new(
        &initial,
        Participant::Maker,
        winner.alternate_final_wire.clone(),
        winner.agreement_commitment,
        102,
    )
    .unwrap();
    assert!(matches!(
        store.complete_maker_btc_negotiation_and_register_actor(
            &request("btc-negotiation-alternate-maker-signature"),
            &offer_id,
            2,
            &reservation_id,
            &alternate,
            &initial,
            &actor,
            103,
        ),
        Err(StoreError::InvalidBtcApplicationState)
    ));

    raw.execute_batch(
        "CREATE TRIGGER fail_btc_negotiation_completion
         BEFORE INSERT ON maker_application_mutations
         WHEN NEW.operation = 'btc_negotiation_complete'
         BEGIN SELECT RAISE(ABORT, 'forced BTC negotiation rollback'); END;",
    )
    .unwrap();
    assert!(matches!(
        store.complete_maker_btc_negotiation_and_register_actor(
            &completion_request,
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
    let rolled_back: (String, i64, String, i64, i64, i64, i64) = raw
        .query_row(
            "SELECT o.state, o.revision, n.state,
                    n.final_agreement_wire IS NOT NULL, n.swap_id IS NOT NULL,
                    (SELECT COUNT(*) FROM swaps
                      WHERE id = ?2),
                    (SELECT COUNT(*) FROM maker_actor_processes
                      WHERE swap_id = ?2)
               FROM maker_offers o
               JOIN maker_btc_negotiations n USING (offer_id)
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
                    row.get(6)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(
        rolled_back,
        ("reserved".into(), 2, "proposed".into(), 0, 0, 0, 0)
    );

    raw.execute_batch("DROP TRIGGER fail_btc_negotiation_completion;")
        .unwrap();
    let committed = store
        .complete_maker_btc_negotiation_and_register_actor(
            &completion_request,
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

    let mut store = SqliteSwapStore::open(&database).unwrap();
    let negotiation = store
        .load_btc_maker_negotiation(&offer_id)
        .unwrap()
        .unwrap();
    assert_eq!(negotiation.status(), MakerBtcNegotiationStatus::Completed);
    assert_eq!(
        negotiation.final_agreement_wire(),
        Some(final_wire.as_slice())
    );
    assert_eq!(negotiation.swap_id(), Some(initial.id().as_str()));
    let offer = &store.list_maker_offer_history(1_000).unwrap()[0];
    assert_eq!(offer.status(), MakerOfferStatus::Consumed);
    assert_eq!(offer.revision(), 3);
    assert_eq!(offer.swap_id(), Some(initial.id().as_str()));
    assert_eq!(store.load(initial.id()).unwrap(), Some(initial.clone()));
    let actors = store.list_maker_actor_processes().unwrap();
    assert_eq!(actors.len(), 1);
    assert_eq!(actors[0].manifest(), &actor);
    assert_eq!(actors[0].schedule_state(), MakerActorScheduleState::Queued);

    let preflight = store
        .preflight_maker_btc_scheduled_completion_replay(
            &completion_request,
            &offer_id,
            2,
            &reservation_id,
            &final_wire,
        )
        .unwrap()
        .expect("committed completion is recoverable before provisioning");
    assert_eq!(preflight.offer_revision(), 3);
    assert_eq!(preflight.swap_id(), initial.id());
    assert_eq!(preflight.actor(), &actor);

    raw.execute(
        "UPDATE maker_btc_negotiations SET maker_proposal_wire = ?1 WHERE offer_id = ?2",
        params![conflicting.proposal_wire, offer_id.as_str()],
    )
    .unwrap();
    assert!(matches!(
        store.preflight_maker_btc_scheduled_completion_replay(
            &completion_request,
            &offer_id,
            2,
            &reservation_id,
            &final_wire,
        ),
        Err(StoreError::MakerOffer(_))
    ));
    raw.execute(
        "UPDATE maker_btc_negotiations SET maker_proposal_wire = ?1 WHERE offer_id = ?2",
        params![winner.proposal_wire, offer_id.as_str()],
    )
    .unwrap();

    let replay = store
        .complete_maker_btc_negotiation_and_register_actor(
            &completion_request,
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
        MakerActorKindV1::Bitcoin,
        run.path().join("changed-btc-actor-config.json"),
        [0xa1; 32],
        run.path().join("btc-reference-actor"),
        [0xb2; 32],
        run.path().join("changed-btc-actor.sqlite3"),
    )
    .unwrap();
    assert!(matches!(
        store.complete_maker_btc_negotiation_and_register_actor(
            &completion_request,
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

    raw.execute(
        "UPDATE maker_btc_negotiations SET final_agreement_wire = ?1 WHERE offer_id = ?2",
        params![winner.alternate_final_wire, offer_id.as_str()],
    )
    .unwrap();
    assert!(matches!(
        store.load_btc_maker_negotiation(&offer_id),
        Err(StoreError::MakerOffer(_))
    ));
}
