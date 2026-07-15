use std::sync::{
    Arc, Barrier,
    atomic::{AtomicUsize, Ordering},
};

use lez_swap_store::{
    AdaptorNonceCommitment, AdaptorPartialSignature, AdaptorPresignature, AdaptorPublicNonce,
    AdaptorSessionIdentity, AdaptorSessionJournalError, AdaptorSessionPhase,
    AdaptorSessionReservation, AdaptorSessionRole, SecretNonceBytes, SqliteAdaptorSessionJournal,
};
use tempfile::tempdir;

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

const SECRET: u8 = 0x71;
const OWN_NONCE: u8 = 0x72;
const OWN_COMMITMENT: u8 = 0x73;
const PEER_COMMITMENT: u8 = 0x74;
const PEER_NONCE: u8 = 0x75;
const OWN_PARTIAL: u8 = 0x76;
const PEER_PARTIAL: u8 = 0x77;
const PRESIGNATURE: u8 = 0x78;

fn identity(session: u8, role: AdaptorSessionRole) -> AdaptorSessionIdentity {
    AdaptorSessionIdentity::new(
        [session; 32],
        role,
        [0x21; 32],
        [0x22; 32],
        [0x23; 33],
        [[0x24; 33], [0x25; 33]],
    )
}

fn reservation(identity: AdaptorSessionIdentity, secret: u8) -> AdaptorSessionReservation {
    AdaptorSessionReservation::new(
        identity,
        SecretNonceBytes::new([secret; 97]),
        AdaptorPublicNonce::new([OWN_NONCE; 66]),
        AdaptorNonceCommitment::new([OWN_COMMITMENT; 32]),
    )
}

fn exchange_nonces(journal: &mut SqliteAdaptorSessionJournal, identity: &AdaptorSessionIdentity) {
    let _ = journal
        .record_peer_commitment(identity, AdaptorNonceCommitment::new([PEER_COMMITMENT; 32]))
        .expect("persist peer commitment");
    assert_eq!(
        journal
            .reveal_own_public_nonce(identity)
            .expect("reveal after commitment"),
        AdaptorPublicNonce::new([OWN_NONCE; 66])
    );
    let _ = journal
        .record_verified_peer_public_nonce(identity, AdaptorPublicNonce::new([PEER_NONCE; 66]))
        .expect("persist verified peer nonce");
}

#[test]
fn poc_pure_callback_lifecycle_is_ordered_secret_free_and_exactly_replayable() {
    let directory = tempdir().expect("temporary directory");
    let path = directory.path().join("maker-adaptor.sqlite");
    let identity = identity(1, AdaptorSessionRole::Maker);
    let mut journal = SqliteAdaptorSessionJournal::open(&path).expect("open journal");

    let reserved = journal
        .reserve(reservation(identity.clone(), SECRET))
        .expect("reserve nonce before commitment exposure");
    assert!(!reserved.was_replay());
    assert_eq!(reserved.phase(), AdaptorSessionPhase::Reserved);
    assert_eq!(
        reserved.own_commitment(),
        AdaptorNonceCommitment::new([OWN_COMMITMENT; 32])
    );
    assert!(matches!(
        journal.reveal_own_public_nonce(&identity),
        Err(AdaptorSessionJournalError::InvalidPhase)
    ));
    let initial = journal
        .load(identity.session_id())
        .expect("load reserved session")
        .expect("reserved row");
    assert_eq!(initial.phase(), AdaptorSessionPhase::Reserved);
    assert_eq!(initial.own_public_nonce(), None);

    exchange_nonces(&mut journal, &identity);
    let signed = journal
        .sign_and_persist_partial(&identity, |material| {
            assert_eq!(material.identity(), &identity);
            assert_eq!(material.secret_nonce(), &[SECRET; 97]);
            assert_eq!(
                material.own_public_nonce(),
                AdaptorPublicNonce::new([OWN_NONCE; 66])
            );
            assert_eq!(
                material.peer_public_nonce(),
                AdaptorPublicNonce::new([PEER_NONCE; 66])
            );
            Ok(AdaptorPartialSignature::new([OWN_PARTIAL; 32]))
        })
        .expect("consume nonce and persist partial");
    assert!(!signed.was_replay());
    assert_eq!(
        signed.partial(),
        AdaptorPartialSignature::new([OWN_PARTIAL; 32])
    );

    let _ = journal
        .record_verified_peer_partial(&identity, AdaptorPartialSignature::new([PEER_PARTIAL; 32]))
        .expect("persist peer partial");
    let _ = journal
        .record_verified_presignature(&identity, AdaptorPresignature::new([PRESIGNATURE; 65]))
        .expect("persist aggregate presignature");

    let replay = journal
        .reserve(reservation(identity.clone(), SECRET))
        .expect("exact reservation replay cannot rearm nonce");
    assert!(replay.was_replay());
    assert_eq!(replay.phase(), AdaptorSessionPhase::PresignatureVerified);
    let signed_replay = journal
        .sign_and_persist_partial(&identity, |_| {
            panic!("persisted outbox replay must never invoke signer")
        })
        .expect("return exact persisted partial");
    assert!(signed_replay.was_replay());
    assert_eq!(signed_replay.partial(), signed.partial());

    let complete = journal
        .load(identity.session_id())
        .expect("load complete session")
        .expect("complete row");
    assert_eq!(complete.phase(), AdaptorSessionPhase::PresignatureVerified);
    assert_eq!(
        complete.peer_commitment(),
        Some(AdaptorNonceCommitment::new([PEER_COMMITMENT; 32]))
    );
    assert_eq!(
        complete.peer_public_nonce(),
        Some(AdaptorPublicNonce::new([PEER_NONCE; 66]))
    );
    assert_eq!(complete.own_partial(), Some(signed.partial()));
    assert_eq!(
        complete.peer_partial(),
        Some(AdaptorPartialSignature::new([PEER_PARTIAL; 32]))
    );
    assert_eq!(
        complete.presignature(),
        Some(AdaptorPresignature::new([PRESIGNATURE; 65]))
    );
}

#[test]
fn restart_recovers_exact_transcript_and_never_signs_a_persisted_partial_again() {
    let directory = tempdir().expect("temporary directory");
    let path = directory.path().join("taker-adaptor.sqlite");
    let identity = identity(2, AdaptorSessionRole::Taker);

    {
        let mut journal = SqliteAdaptorSessionJournal::open(&path).expect("open journal");
        let _ = journal
            .reserve(reservation(identity.clone(), SECRET))
            .expect("reserve nonce");
        let _ = journal
            .record_peer_commitment(
                &identity,
                AdaptorNonceCommitment::new([PEER_COMMITMENT; 32]),
            )
            .expect("persist peer commitment");
    }

    {
        let mut journal = SqliteAdaptorSessionJournal::open(&path).expect("restart journal");
        assert!(
            journal
                .record_peer_commitment(
                    &identity,
                    AdaptorNonceCommitment::new([PEER_COMMITMENT; 32]),
                )
                .expect("exact commitment replay")
                .was_replay()
        );
        assert_eq!(
            journal
                .reveal_own_public_nonce(&identity)
                .expect("recover exact own nonce"),
            AdaptorPublicNonce::new([OWN_NONCE; 66])
        );
        let _ = journal
            .record_verified_peer_public_nonce(&identity, AdaptorPublicNonce::new([PEER_NONCE; 66]))
            .expect("persist peer nonce");
        let _ = journal
            .sign_and_persist_partial(&identity, |_| {
                Ok(AdaptorPartialSignature::new([OWN_PARTIAL; 32]))
            })
            .expect("persist partial");
    }

    let invoked = AtomicUsize::new(0);
    let mut journal = SqliteAdaptorSessionJournal::open(&path).expect("second restart");
    let replay = journal
        .sign_and_persist_partial(&identity, |_| {
            invoked.fetch_add(1, Ordering::SeqCst);
            Ok(AdaptorPartialSignature::new([0xff; 32]))
        })
        .expect("recover exact partial after restart");
    assert!(replay.was_replay());
    assert_eq!(invoked.load(Ordering::SeqCst), 0);
    assert_eq!(
        replay.partial(),
        AdaptorPartialSignature::new([OWN_PARTIAL; 32])
    );

    let control = rusqlite::Connection::open(&path).expect("open control connection");
    let nonce_is_gone = control
        .query_row(
            "SELECT secret_nonce IS NULL FROM adaptor_sessions WHERE session_id = ?1",
            [identity.session_id().as_slice()],
            |row| row.get::<_, bool>(0),
        )
        .expect("inspect consumed nonce tombstone");
    assert!(nonce_is_gone);
}

#[test]
fn message_and_every_replayed_wire_mutation_fail_closed() {
    let directory = tempdir().expect("temporary directory");
    let path = directory.path().join("mutation-adaptor.sqlite");
    let identity = identity(3, AdaptorSessionRole::Maker);
    let mut journal = SqliteAdaptorSessionJournal::open(&path).expect("open journal");
    let _ = journal
        .reserve(reservation(identity.clone(), SECRET))
        .expect("reserve nonce");

    let mutated_identity = AdaptorSessionIdentity::new(
        *identity.session_id(),
        identity.local_role(),
        *identity.signing_domain(),
        [0xa1; 32],
        *identity.adaptor_point(),
        *identity.ordered_public_keys(),
    );
    assert!(matches!(
        journal.reserve(reservation(mutated_identity.clone(), SECRET)),
        Err(AdaptorSessionJournalError::SessionConflict)
    ));
    assert!(matches!(
        journal.record_peer_commitment(
            &mutated_identity,
            AdaptorNonceCommitment::new([PEER_COMMITMENT; 32]),
        ),
        Err(AdaptorSessionJournalError::SessionConflict)
    ));
    assert!(matches!(
        journal.record_verified_peer_public_nonce(
            &identity,
            AdaptorPublicNonce::new([PEER_NONCE; 66]),
        ),
        Err(AdaptorSessionJournalError::InvalidPhase)
    ));

    let _ = journal
        .record_peer_commitment(
            &identity,
            AdaptorNonceCommitment::new([PEER_COMMITMENT; 32]),
        )
        .expect("persist commitment");
    assert!(matches!(
        journal.record_peer_commitment(&identity, AdaptorNonceCommitment::new([0xa2; 32]),),
        Err(AdaptorSessionJournalError::SessionConflict)
    ));
    let _ = journal
        .record_verified_peer_public_nonce(&identity, AdaptorPublicNonce::new([PEER_NONCE; 66]))
        .expect("persist nonce");
    assert!(matches!(
        journal.record_verified_peer_public_nonce(&identity, AdaptorPublicNonce::new([0xa3; 66]),),
        Err(AdaptorSessionJournalError::SessionConflict)
    ));

    let durable_partial = journal
        .sign_and_persist_partial(&identity, |_| {
            Ok(AdaptorPartialSignature::new([OWN_PARTIAL; 32]))
        })
        .expect("persist own partial")
        .partial();
    let replay = journal
        .sign_and_persist_partial(&identity, |_| Ok(AdaptorPartialSignature::new([0xa4; 32])))
        .expect("mutation callback is skipped after persistence");
    assert!(replay.was_replay());
    assert_eq!(replay.partial(), durable_partial);

    let _ = journal
        .record_verified_peer_partial(&identity, AdaptorPartialSignature::new([PEER_PARTIAL; 32]))
        .expect("persist peer partial");
    assert!(matches!(
        journal.record_verified_peer_partial(&identity, AdaptorPartialSignature::new([0xa5; 32]),),
        Err(AdaptorSessionJournalError::SessionConflict)
    ));
    let _ = journal
        .record_verified_presignature(&identity, AdaptorPresignature::new([PRESIGNATURE; 65]))
        .expect("persist presignature");
    assert!(matches!(
        journal.record_verified_presignature(&identity, AdaptorPresignature::new([0xa6; 65]),),
        Err(AdaptorSessionJournalError::SessionConflict)
    ));
}

#[test]
fn secret_nonce_fingerprint_is_one_use_across_session_ids() {
    let directory = tempdir().expect("temporary directory");
    let path = directory.path().join("nonce-reuse.sqlite");
    let first = identity(4, AdaptorSessionRole::Maker);
    let second = identity(5, AdaptorSessionRole::Maker);
    let mut journal = SqliteAdaptorSessionJournal::open(&path).expect("open journal");

    let _ = journal
        .reserve(reservation(first, SECRET))
        .expect("reserve first secret nonce");
    assert!(matches!(
        journal.reserve(reservation(second.clone(), SECRET)),
        Err(AdaptorSessionJournalError::SecretNonceReuse)
    ));
    let _ = journal
        .reserve(reservation(second, SECRET.wrapping_add(1)))
        .expect("fresh secret nonce remains usable");
}

#[test]
fn concurrent_signing_uses_one_callback_and_one_exact_outbox() {
    let directory = tempdir().expect("temporary directory");
    let path = directory.path().join("concurrent-adaptor.sqlite");
    let identity = identity(6, AdaptorSessionRole::Taker);
    {
        let mut journal = SqliteAdaptorSessionJournal::open(&path).expect("open journal");
        let _ = journal
            .reserve(reservation(identity.clone(), SECRET))
            .expect("reserve nonce");
        exchange_nonces(&mut journal, &identity);
    }

    let first_journal = SqliteAdaptorSessionJournal::open(&path).expect("open first connection");
    let second_journal = SqliteAdaptorSessionJournal::open(&path).expect("open second connection");
    let barrier = Arc::new(Barrier::new(3));
    let invocation_count = Arc::new(AtomicUsize::new(0));

    let spawn_signer = |mut journal: SqliteAdaptorSessionJournal| {
        let barrier = Arc::clone(&barrier);
        let invocation_count = Arc::clone(&invocation_count);
        let identity = identity.clone();
        std::thread::spawn(move || {
            barrier.wait();
            journal
                .sign_and_persist_partial(&identity, |_| {
                    invocation_count.fetch_add(1, Ordering::SeqCst);
                    Ok(AdaptorPartialSignature::new([OWN_PARTIAL; 32]))
                })
                .expect("concurrent exact signing attempt")
        })
    };
    let first = spawn_signer(first_journal);
    let second = spawn_signer(second_journal);
    barrier.wait();

    let first = first.join().expect("first signer thread");
    let second = second.join().expect("second signer thread");
    assert_eq!(invocation_count.load(Ordering::SeqCst), 1);
    assert_eq!(first.partial(), second.partial());
    assert_ne!(first.was_replay(), second.was_replay());
}

#[cfg(unix)]
#[test]
fn poc_plaintext_at_rest_journal_is_owner_private_and_secret_debug_is_redacted() {
    let directory = tempdir().expect("temporary directory");
    let path = directory.path().join("private-adaptor.sqlite");
    let identity = identity(7, AdaptorSessionRole::Maker);
    let secret_marker = 247_u8;
    let secret = SecretNonceBytes::new([secret_marker; 97]);
    let secret_debug = format!("{secret:?}");
    assert!(secret_debug.contains("[REDACTED]"));
    assert!(!secret_debug.contains(&secret_marker.to_string()));

    let candidate = reservation(identity.clone(), secret_marker);
    let candidate_debug = format!("{candidate:?}");
    assert!(candidate_debug.contains("[REDACTED]"));
    assert!(!candidate_debug.contains(&secret_marker.to_string()));

    let mut journal = SqliteAdaptorSessionJournal::open(&path).expect("open private journal");
    let _ = journal.reserve(candidate).expect("reserve redacted secret");
    exchange_nonces(&mut journal, &identity);
    let mut signing_debug = String::new();
    assert!(matches!(
        journal.sign_and_persist_partial(&identity, |material| {
            signing_debug = format!("{material:?}");
            Err(())
        }),
        Err(AdaptorSessionJournalError::SigningFailed)
    ));
    assert!(signing_debug.contains("[REDACTED]"));
    assert!(!signing_debug.contains(&secret_marker.to_string()));

    let metadata = std::fs::symlink_metadata(&path).expect("inspect database");
    assert!(metadata.file_type().is_file());
    assert_eq!(metadata.permissions().mode() & 0o7777, 0o600);
    assert_eq!(metadata.nlink(), 1);
}
