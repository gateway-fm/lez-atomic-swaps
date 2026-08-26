use bitcoin::ScriptBuf;
use bitcoin::secp256k1::{Keypair, Message, PublicKey, Secp256k1, SecretKey};
use lez_btc_swap_sdk::{
    BTC_ROLE_CONTRIBUTION_SCHEMA_V1, BtcChainPolicyV1, BtcLezChainIdentityV1,
    BtcParticipantIdentityV1, BtcRoleContributionBodyV1, BtcRoleContributionPairV1,
    BtcRoleContributionRecordV1, BtcRoleContributionV1, BtcRoleContributionV1Error,
    MAX_BTC_ROLE_CONTRIBUTION_RECORD_BYTES, derive_btc_pre_session_id_v1,
};
use lez_swap_core::{Participant, SwapDirection};

const OFFER_COMMITMENT: [u8; 32] = [31; 32];
const RESERVATION: &[u8] = b"role-separated-reservation";
const EXPIRES_AT: u64 = 1_900_000_000;

fn secret(value: u8) -> SecretKey {
    SecretKey::from_slice(&[value; 32]).expect("valid test secret")
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

fn body(
    role: Participant,
    direction: SwapDirection,
    pre_session_id: [u8; 32],
    entropy: u8,
) -> (BtcRoleContributionBodyV1, SecretKey) {
    let signing = secret(match role {
        Participant::Maker => 1,
        Participant::Taker => 2,
    });
    let refund = secret(match role {
        Participant::Maker => 3,
        Participant::Taker => 4,
    });
    let claim = secret(match role {
        Participant::Maker => 5,
        Participant::Taker => 6,
    });
    let funding = secret(match role {
        Participant::Maker => 8,
        Participant::Taker => 9,
    });
    let adaptor = secret(7);
    let identity = BtcParticipantIdentityV1::new(
        [match role {
            Participant::Maker => 11,
            Participant::Taker => 12,
        }; 32],
        public_key(&signing),
        x_only(&refund),
        destination(&claim),
    );
    let body = BtcRoleContributionBodyV1::new(
        pre_session_id,
        role,
        direction,
        BtcChainPolicyV1::new([21; 32], 6),
        BtcLezChainIdentityV1::new([22; 32], [23; 32], [24; 32], [25; 32]),
        identity,
        x_only(&funding),
        (role == Participant::Taker).then(|| public_key(&adaptor)),
        [entropy; 32],
        EXPIRES_AT,
    )
    .expect("valid role contribution body");
    (body, signing)
}

fn contribution(
    role: Participant,
    direction: SwapDirection,
    pre_session_id: [u8; 32],
    entropy: u8,
) -> BtcRoleContributionV1 {
    let (body, secret) = body(role, direction, pre_session_id, entropy);
    let commitment = body.commitment();
    let signature = Secp256k1::signing_only()
        .sign_schnorr_no_aux_rand(
            &Message::from_digest(commitment),
            &Keypair::from_secret_key(&Secp256k1::new(), &secret),
        )
        .serialize();
    BtcRoleContributionV1::validate(BtcRoleContributionRecordV1::from_parts(
        BTC_ROLE_CONTRIBUTION_SCHEMA_V1,
        body,
        commitment,
        signature,
    ))
    .expect("valid signed role contribution")
}

#[test]
fn canonical_role_contributions_round_trip_and_derive_one_joint_swap() {
    for direction in [
        SwapDirection::TakerSellsForeign,
        SwapDirection::TakerSellsLez,
    ] {
        let pre_session =
            derive_btc_pre_session_id_v1(&OFFER_COMMITMENT, RESERVATION, direction).unwrap();
        let maker = contribution(Participant::Maker, direction, pre_session, 41);
        let taker = contribution(Participant::Taker, direction, pre_session, 42);
        let maker_wire = maker.encode_wire().unwrap();
        let taker_wire = taker.encode_wire().unwrap();
        assert_eq!(
            BtcRoleContributionV1::from_wire(&maker_wire)
                .unwrap()
                .encode_wire()
                .unwrap(),
            maker_wire
        );
        assert_eq!(
            BtcRoleContributionV1::from_wire(&taker_wire)
                .unwrap()
                .encode_wire()
                .unwrap(),
            taker_wire
        );
        let pair = BtcRoleContributionPairV1::new(maker, taker).unwrap();
        assert_ne!(pair.swap_id(), &[0; 32]);
        assert_eq!(
            pair.participants()
                .for_participant(Participant::Maker)
                .musig2_public_key(),
            pair.maker()
                .body()
                .participant_identity()
                .musig2_public_key()
        );
        assert_eq!(
            pair.adaptor_point(),
            pair.taker().body().adaptor_point().unwrap()
        );
    }
}

#[test]
fn joint_swap_identity_changes_with_either_role_contribution() {
    let direction = SwapDirection::TakerSellsForeign;
    let pre_session =
        derive_btc_pre_session_id_v1(&OFFER_COMMITMENT, RESERVATION, direction).unwrap();
    let first = BtcRoleContributionPairV1::new(
        contribution(Participant::Maker, direction, pre_session, 41),
        contribution(Participant::Taker, direction, pre_session, 42),
    )
    .unwrap();
    let maker_changed = BtcRoleContributionPairV1::new(
        contribution(Participant::Maker, direction, pre_session, 43),
        contribution(Participant::Taker, direction, pre_session, 42),
    )
    .unwrap();
    let taker_changed = BtcRoleContributionPairV1::new(
        contribution(Participant::Maker, direction, pre_session, 41),
        contribution(Participant::Taker, direction, pre_session, 44),
    )
    .unwrap();
    assert_ne!(first.swap_id(), maker_changed.swap_id());
    assert_ne!(first.swap_id(), taker_changed.swap_id());
}

#[test]
fn contribution_pair_rejects_cross_wired_sessions_directions_and_roles() {
    let forward = derive_btc_pre_session_id_v1(
        &OFFER_COMMITMENT,
        RESERVATION,
        SwapDirection::TakerSellsForeign,
    )
    .unwrap();
    let reverse =
        derive_btc_pre_session_id_v1(&OFFER_COMMITMENT, RESERVATION, SwapDirection::TakerSellsLez)
            .unwrap();
    assert_eq!(
        BtcRoleContributionPairV1::new(
            contribution(
                Participant::Maker,
                SwapDirection::TakerSellsForeign,
                forward,
                41,
            ),
            contribution(
                Participant::Taker,
                SwapDirection::TakerSellsLez,
                reverse,
                42,
            ),
        ),
        Err(BtcRoleContributionV1Error::PreSessionMismatch)
    );
    assert_eq!(
        BtcRoleContributionPairV1::new(
            contribution(
                Participant::Taker,
                SwapDirection::TakerSellsForeign,
                forward,
                41,
            ),
            contribution(
                Participant::Maker,
                SwapDirection::TakerSellsForeign,
                forward,
                42,
            ),
        ),
        Err(BtcRoleContributionV1Error::PairRoleMismatch)
    );
}

#[test]
fn role_shape_requires_taker_only_adaptor_point() {
    let direction = SwapDirection::TakerSellsForeign;
    let pre_session =
        derive_btc_pre_session_id_v1(&OFFER_COMMITMENT, RESERVATION, direction).unwrap();
    let (maker, _) = body(Participant::Maker, direction, pre_session, 41);
    assert_eq!(maker.adaptor_point(), None);
    let (taker, _) = body(Participant::Taker, direction, pre_session, 42);
    assert!(taker.adaptor_point().is_some());
    let maker_identity = maker.participant_identity().clone();
    assert_eq!(
        BtcRoleContributionBodyV1::new(
            pre_session,
            Participant::Maker,
            direction,
            *maker.bitcoin_chain_policy(),
            *maker.lez_chain_identity(),
            maker_identity,
            x_only(&secret(8)),
            Some(public_key(&secret(7))),
            [41; 32],
            EXPIRES_AT,
        ),
        Err(BtcRoleContributionV1Error::InvalidAdaptorPoint)
    );
}

#[test]
fn bounded_wire_rejects_signature_drift_trailing_and_oversized_records() {
    let direction = SwapDirection::TakerSellsForeign;
    let pre_session =
        derive_btc_pre_session_id_v1(&OFFER_COMMITMENT, RESERVATION, direction).unwrap();
    let contribution = contribution(Participant::Maker, direction, pre_session, 41);
    let mut signature_drift = contribution.encode_wire().unwrap();
    let final_byte = signature_drift.last_mut().unwrap();
    *final_byte ^= 1;
    assert_eq!(
        BtcRoleContributionV1::from_wire(&signature_drift),
        Err(BtcRoleContributionV1Error::SignatureMismatch)
    );
    let mut trailing = contribution.encode_wire().unwrap();
    trailing.push(0);
    assert_eq!(
        BtcRoleContributionV1::from_wire(&trailing),
        Err(BtcRoleContributionV1Error::MalformedWireRecord)
    );
    assert_eq!(
        BtcRoleContributionV1::from_wire(&vec![0; MAX_BTC_ROLE_CONTRIBUTION_RECORD_BYTES + 1]),
        Err(BtcRoleContributionV1Error::OversizedWireRecord {
            actual: MAX_BTC_ROLE_CONTRIBUTION_RECORD_BYTES + 1,
            maximum: MAX_BTC_ROLE_CONTRIBUTION_RECORD_BYTES,
        })
    );
}

#[test]
fn pre_session_rejects_unbound_inputs_and_separates_directions() {
    assert_eq!(
        derive_btc_pre_session_id_v1(&[0; 32], RESERVATION, SwapDirection::TakerSellsForeign,),
        Err(BtcRoleContributionV1Error::InvalidOfferCommitment)
    );
    assert_eq!(
        derive_btc_pre_session_id_v1(&OFFER_COMMITMENT, &[], SwapDirection::TakerSellsForeign,),
        Err(BtcRoleContributionV1Error::InvalidReservationBinding)
    );
    assert_ne!(
        derive_btc_pre_session_id_v1(
            &OFFER_COMMITMENT,
            RESERVATION,
            SwapDirection::TakerSellsForeign,
        )
        .unwrap(),
        derive_btc_pre_session_id_v1(&OFFER_COMMITMENT, RESERVATION, SwapDirection::TakerSellsLez,)
            .unwrap()
    );
}
