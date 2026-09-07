//! Two in-process seats converge on one adaptor presignature, and every step
//! replays without consuming a second nonce or changing any public byte.

use lez_adaptor_role_runner::{CeremonySeat, Role, ValidatedSession};
use lez_adaptor_signature::{
    AdaptorSessionContext, adapt_presignature, extract_adaptor_secret, verify_adaptor_presignature,
    verify_final_signature,
};
use lez_swap_store::AdaptorSessionPhase;
use secp256k1::{PublicKey, Secp256k1, SecretKey};
use zeroize::Zeroizing;

const MAKER_SECRET: [u8; 32] = [0x31; 32];
const TAKER_SECRET: [u8; 32] = [0x42; 32];
const ADAPTOR_SECRET: [u8; 32] = [0x53; 32];

fn public_key(secret: [u8; 32]) -> [u8; 33] {
    PublicKey::from_secret_key(
        &Secp256k1::signing_only(),
        &SecretKey::from_slice(&secret).expect("valid fixture key"),
    )
    .serialize()
}

fn context(taproot: bool) -> AdaptorSessionContext {
    let keys = [public_key(MAKER_SECRET), public_key(TAKER_SECRET)];
    if taproot {
        AdaptorSessionContext::taproot(
            keys,
            [0xb1; 32],
            [0x92; 32],
            public_key(ADAPTOR_SECRET),
            [0xa2; 32],
        )
    } else {
        AdaptorSessionContext::untweaked(keys, [0x91; 32], public_key(ADAPTOR_SECRET), [0xa1; 32])
    }
    .expect("valid context")
}

fn converge(taproot: bool) {
    let directory = tempfile::tempdir().expect("temporary directory");
    let session = ValidatedSession::from_context(context(taproot)).expect("validated session");
    let reloaded = ValidatedSession::from_canonical_bytes(&session.canonical_bytes().unwrap())
        .expect("canonical session reloads");
    assert_eq!(reloaded.context_binding(), session.context_binding());

    let mut maker = CeremonySeat::open(
        &directory.path().join("maker.sqlite"),
        session.clone(),
        Role::Maker,
    )
    .expect("maker seat");
    let mut taker = CeremonySeat::open(
        &directory.path().join("taker.sqlite"),
        session.clone(),
        Role::Taker,
    )
    .expect("taker seat");
    let maker_key = Zeroizing::new(MAKER_SECRET);
    let taker_key = Zeroizing::new(TAKER_SECRET);
    assert_eq!(maker.phase().unwrap(), None);

    // Round 1: commitments. Reserving twice returns the same commitment.
    let taker_commitment = taker.reserve(&taker_key).unwrap();
    assert_eq!(taker.reserve(&taker_key).unwrap(), taker_commitment);
    let maker_commitment = maker.reserve(&maker_key).unwrap();
    maker.accept_commitment(&taker_commitment).unwrap();
    maker.accept_commitment(&taker_commitment).unwrap();
    taker.accept_commitment(&maker_commitment).unwrap();
    assert_eq!(
        maker.phase().unwrap(),
        Some(AdaptorSessionPhase::CommitmentExchanged)
    );

    // A nonce cannot be revealed before the peer committed.
    let mut early = CeremonySeat::open(
        &directory.path().join("early.sqlite"),
        session.clone(),
        Role::Maker,
    )
    .unwrap();
    let _ = early.reserve(&maker_key).unwrap();
    assert!(early.reveal_nonce().is_err());

    // Round 2: the Taker reveals; the Maker verifies, reveals and signs.
    let taker_nonce = taker.reveal_nonce().unwrap();
    assert_eq!(taker.reveal_nonce().unwrap(), taker_nonce);
    let maker_nonce = maker.reveal_nonce().unwrap();
    let maker_partial = maker.accept_nonce_sign(&taker_nonce, &maker_key).unwrap();
    assert_eq!(
        maker.accept_nonce_sign(&taker_nonce, &maker_key).unwrap(),
        maker_partial,
        "a replayed sign step must return the durable partial"
    );
    assert_eq!(maker.replay_partial().unwrap(), maker_partial);

    // Round 3: the Taker signs and aggregates; the Maker aggregates.
    let taker_partial = taker.accept_nonce_sign(&maker_nonce, &taker_key).unwrap();
    let taker_presignature = taker.accept_peer_partial(&maker_partial).unwrap();
    let maker_presignature = maker.accept_peer_partial(&taker_partial).unwrap();
    assert_eq!(
        maker.accept_peer_partial(&taker_partial).unwrap(),
        maker_presignature
    );
    assert_eq!(
        taker_presignature, maker_presignature,
        "both seats converge"
    );
    assert_eq!(
        maker.phase().unwrap(),
        Some(AdaptorSessionPhase::PresignatureVerified)
    );

    let presignature = maker.presignature().unwrap();
    assert_eq!(taker.presignature().unwrap(), presignature);
    verify_adaptor_presignature(session.context(), presignature).expect("valid presignature");
    let final_signature = adapt_presignature(
        session.context(),
        presignature,
        Zeroizing::new(ADAPTOR_SECRET),
    )
    .expect("adapt");
    verify_final_signature(session.context(), final_signature).expect("final verifies");
    let extracted =
        extract_adaptor_secret(session.context(), presignature, final_signature).expect("extract");
    assert_eq!(*extracted, ADAPTOR_SECRET);

    // Crosswire: a packet from another session is refused.
    let other = ValidatedSession::from_context(context(!taproot)).unwrap();
    let mut stranger = CeremonySeat::open(
        &directory.path().join("stranger.sqlite"),
        other,
        Role::Taker,
    )
    .unwrap();
    let foreign = stranger.reserve(&taker_key).unwrap();
    assert!(maker.accept_commitment(&foreign).is_err());
}

#[test]
fn lez_untweaked_seats_converge_and_replay_idempotently() {
    converge(false);
}

#[test]
fn bitcoin_taproot_seats_converge_and_replay_idempotently() {
    converge(true);
}
