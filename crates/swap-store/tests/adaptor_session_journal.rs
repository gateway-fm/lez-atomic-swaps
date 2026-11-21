use std::sync::{
    Arc, Barrier,
    atomic::{AtomicUsize, Ordering},
};

use lez_swap_store::{
    AdaptorNonceCommitment, AdaptorNonceSealingKey, AdaptorPartialSignature, AdaptorPresignature,
    AdaptorPublicNonce, AdaptorSessionIdentity, AdaptorSessionJournalError, AdaptorSessionPhase,
    AdaptorSessionReservation, AdaptorSessionRole, SecretNonceBytes, SqliteAdaptorSessionJournal,
    StoreError,
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

fn key() -> AdaptorNonceSealingKey {
    AdaptorNonceSealingKey::derive(&[0x5a; 32])
}

fn reservation(identity: AdaptorSessionIdentity, secret: u8) -> AdaptorSessionReservation {
    AdaptorSessionReservation::new(
        identity,
        SecretNonceBytes::new([secret; 97]),
        AdaptorPublicNonce::new([OWN_NONCE; 66]),
        AdaptorNonceCommitment::new([OWN_COMMITMENT; 32]),
    )
}

#[test]
fn existing_only_open_never_creates_a_missing_signer_database() {
    let directory = tempdir().expect("temporary directory");
    let missing = directory.path().join("missing-adaptor.sqlite");
    assert!(matches!(
        SqliteAdaptorSessionJournal::open_existing(&missing),
        Err(AdaptorSessionJournalError::Store(
            StoreError::DatabaseFileUnavailable
        ))
    ));
    assert!(
        !missing.exists(),
        "existing-only open must not create a file"
    );

    let existing = directory.path().join("existing-adaptor.sqlite");
    let expected = identity(0x19, AdaptorSessionRole::Maker);
    {
        let mut journal =
            SqliteAdaptorSessionJournal::open(&existing).expect("create signer journal");
        let _ = journal
            .reserve(reservation(expected.clone(), SECRET), &key())
            .expect("persist exact signer transcript");
    }
    let reopened = SqliteAdaptorSessionJournal::open_existing(&existing)
        .expect("reopen only the existing signer journal");
    assert_eq!(
        reopened
            .load(expected.session_id())
            .expect("load exact transcript")
            .expect("persisted transcript")
            .identity(),
        &expected
    );
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
        .reserve(reservation(identity.clone(), SECRET), &key())
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
        .sign_and_persist_partial(&identity, &key(), |material| {
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
        .reserve(reservation(identity.clone(), SECRET), &key())
        .expect("exact reservation replay cannot rearm nonce");
    assert!(replay.was_replay());
    assert_eq!(replay.phase(), AdaptorSessionPhase::PresignatureVerified);
    let signed_replay = journal
        .sign_and_persist_partial(&identity, &key(), |_| {
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
            .reserve(reservation(identity.clone(), SECRET), &key())
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
            .sign_and_persist_partial(&identity, &key(), |_| {
                Ok(AdaptorPartialSignature::new([OWN_PARTIAL; 32]))
            })
            .expect("persist partial");
    }

    let invoked = AtomicUsize::new(0);
    let mut journal = SqliteAdaptorSessionJournal::open(&path).expect("second restart");
    let replay = journal
        .sign_and_persist_partial(&identity, &key(), |_| {
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
        .reserve(reservation(identity.clone(), SECRET), &key())
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
        journal.reserve(reservation(mutated_identity.clone(), SECRET), &key()),
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
        .sign_and_persist_partial(&identity, &key(), |_| {
            Ok(AdaptorPartialSignature::new([OWN_PARTIAL; 32]))
        })
        .expect("persist own partial")
        .partial();
    let replay = journal
        .sign_and_persist_partial(&identity, &key(), |_| {
            Ok(AdaptorPartialSignature::new([0xa4; 32]))
        })
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
        .reserve(reservation(first, SECRET), &key())
        .expect("reserve first secret nonce");
    assert!(matches!(
        journal.reserve(reservation(second.clone(), SECRET), &key()),
        Err(AdaptorSessionJournalError::SecretNonceReuse)
    ));
    let _ = journal
        .reserve(reservation(second, SECRET.wrapping_add(1)), &key())
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
            .reserve(reservation(identity.clone(), SECRET), &key())
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
                .sign_and_persist_partial(&identity, &key(), |_| {
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
fn the_journal_file_is_owner_private_and_secret_debug_is_redacted() {
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
    let _ = journal
        .reserve(candidate, &key())
        .expect("reserve redacted secret");
    exchange_nonces(&mut journal, &identity);
    let mut signing_debug = String::new();
    assert!(matches!(
        journal.sign_and_persist_partial(&identity, &key(), |material| {
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

/// A secret nonce no other test bytes repeat, so a byte search is meaningful.
fn distinctive_secret() -> [u8; 97] {
    std::array::from_fn(|index| u8::try_from(index).expect("index fits") ^ 0xc5)
}

fn contains_secret(path: &std::path::Path, secret: &[u8; 97]) -> bool {
    std::fs::read(path)
        .is_ok_and(|bytes| bytes.windows(secret.len()).any(|window| window == secret))
}

#[test]
fn sealed_nonce_round_trips_and_never_reaches_the_database_or_wal_in_clear() {
    let directory = tempdir().expect("temporary directory");
    let path = directory.path().join("sealed-adaptor.sqlite");
    let wal = directory.path().join("sealed-adaptor.sqlite-wal");
    let identity = identity(0x31, AdaptorSessionRole::Taker);
    let secret = distinctive_secret();
    let mut journal = SqliteAdaptorSessionJournal::open(&path).expect("open journal");
    let _ = journal
        .reserve(
            AdaptorSessionReservation::new(
                identity.clone(),
                SecretNonceBytes::new(secret),
                AdaptorPublicNonce::new([OWN_NONCE; 66]),
                AdaptorNonceCommitment::new([OWN_COMMITMENT; 32]),
            ),
            &key(),
        )
        .expect("reserve sealed nonce");
    exchange_nonces(&mut journal, &identity);

    let stored: Vec<u8> = rusqlite::Connection::open(&path)
        .expect("inspect database")
        .query_row("SELECT secret_nonce FROM adaptor_sessions", [], |row| {
            row.get(0)
        })
        .expect("stored nonce");
    assert_eq!(stored.len(), 138);
    assert_eq!(stored[0], 1, "sealed record version");
    assert!(
        !contains_secret(&path, &secret),
        "database holds the nonce in clear"
    );
    assert!(
        !contains_secret(&wal, &secret),
        "WAL holds the nonce in clear"
    );

    let signed = journal
        .sign_and_persist_partial(&identity, &key(), |material| {
            assert_eq!(material.secret_nonce(), &secret);
            Ok(AdaptorPartialSignature::new([OWN_PARTIAL; 32]))
        })
        .expect("open sealed nonce and sign once");
    assert!(!signed.was_replay());
    assert!(!contains_secret(&path, &secret));
    assert!(!contains_secret(&wal, &secret));
}

#[test]
fn wrong_key_or_tampered_sealed_nonce_fails_closed_without_a_new_nonce() {
    let directory = tempdir().expect("temporary directory");
    let path = directory.path().join("tampered-adaptor.sqlite");
    let identity = identity(0x32, AdaptorSessionRole::Maker);
    let mut journal = SqliteAdaptorSessionJournal::open(&path).expect("open journal");
    let _ = journal
        .reserve(reservation(identity.clone(), SECRET), &key())
        .expect("reserve sealed nonce");
    exchange_nonces(&mut journal, &identity);

    let calls = AtomicUsize::new(0);
    let wrong_key = AdaptorNonceSealingKey::derive(&[0x5b; 32]);
    assert!(matches!(
        journal.sign_and_persist_partial(&identity, &wrong_key, |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(AdaptorPartialSignature::new([OWN_PARTIAL; 32]))
        }),
        Err(AdaptorSessionJournalError::CorruptSession)
    ));

    let inspector = rusqlite::Connection::open(&path).expect("inspect database");
    let original: Vec<u8> = inspector
        .query_row("SELECT secret_nonce FROM adaptor_sessions", [], |row| {
            row.get(0)
        })
        .expect("stored nonce");
    let mut tampered = original.clone();
    tampered[60] ^= 1;
    let _ = inspector
        .execute("UPDATE adaptor_sessions SET secret_nonce = ?1", [&tampered])
        .expect("tamper ciphertext");
    assert!(matches!(
        journal.sign_and_persist_partial(&identity, &key(), |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(AdaptorPartialSignature::new([OWN_PARTIAL; 32]))
        }),
        Err(AdaptorSessionJournalError::CorruptSession)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0, "signer never saw a nonce");

    // The session keeps its one durable nonce: a replay returns it, a fresh
    // nonce for the same session conflicts, and nothing was consumed.
    let replay = journal
        .reserve(reservation(identity.clone(), SECRET), &key())
        .expect("exact replay");
    assert!(replay.was_replay());
    assert!(matches!(
        journal.reserve(
            reservation(identity.clone(), SECRET.wrapping_add(9)),
            &key()
        ),
        Err(AdaptorSessionJournalError::SessionConflict)
    ));
    let snapshot = journal
        .load(identity.session_id())
        .expect("load")
        .expect("session");
    assert_eq!(snapshot.phase(), AdaptorSessionPhase::NoncesExchanged);
    assert_eq!(snapshot.own_partial(), None);
}

#[cfg(unix)]
#[test]
fn plaintext_record_from_older_code_is_read_and_its_file_refuses_new_reservations() {
    use sha2::{Digest as _, Sha256};

    let directory = tempdir().expect("temporary directory");
    let current = directory.path().join("current.sqlite");
    let legacy = directory.path().join("legacy.sqlite");
    drop(SqliteAdaptorSessionJournal::open(&current).expect("create current journal"));
    let schema: String = rusqlite::Connection::open(&current)
        .expect("inspect current schema")
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name = 'adaptor_sessions'",
            [],
            |row| row.get(0),
        )
        .expect("current schema");
    // The older code's table admitted only a 97-byte plaintext nonce.
    let old_schema = schema.replace("IN (97, 138)", "= 97");
    assert_ne!(old_schema, schema);

    let fresh = identity(0x34, AdaptorSessionRole::Taker);
    let identity = identity(0x33, AdaptorSessionRole::Taker);
    let secret = distinctive_secret();
    let mut hasher = Sha256::new();
    hasher.update(b"lez-atomic-swaps/adaptor-secret-nonce-fingerprint/v1");
    hasher.update(secret);
    let fingerprint: [u8; 32] = hasher.finalize().into();
    {
        let connection = rusqlite::Connection::open(&legacy).expect("create legacy journal");
        connection.execute_batch(&old_schema).expect("old schema");
        connection
            .execute(
                "INSERT INTO adaptor_sessions (
                    session_id, local_role, signing_domain, exact_message,
                    adaptor_point, maker_public_key, taker_public_key,
                    secret_nonce, secret_nonce_fingerprint, own_commitment,
                    own_public_nonce, peer_commitment, peer_public_nonce, phase
                ) VALUES (?1, 'taker', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                          'nonces_exchanged')",
                rusqlite::params![
                    identity.session_id().as_slice(),
                    identity.signing_domain().as_slice(),
                    identity.exact_message().as_slice(),
                    identity.adaptor_point().as_slice(),
                    identity.ordered_public_keys()[0].as_slice(),
                    identity.ordered_public_keys()[1].as_slice(),
                    secret.as_slice(),
                    fingerprint.as_slice(),
                    [OWN_COMMITMENT; 32].as_slice(),
                    [OWN_NONCE; 66].as_slice(),
                    [PEER_COMMITMENT; 32].as_slice(),
                    [PEER_NONCE; 66].as_slice(),
                ],
            )
            .expect("plaintext record as older code wrote it");
    }
    std::fs::set_permissions(&legacy, std::fs::Permissions::from_mode(0o600))
        .expect("owner-only legacy journal");
    assert!(
        contains_secret(&legacy, &secret),
        "the byte search finds a plaintext nonce"
    );

    let mut journal =
        SqliteAdaptorSessionJournal::open_existing(&legacy).expect("open legacy journal");
    assert_eq!(
        journal
            .load(identity.session_id())
            .expect("load legacy record")
            .expect("legacy session")
            .phase(),
        AdaptorSessionPhase::NoncesExchanged
    );
    let _ = journal
        .sign_and_persist_partial(&identity, &key(), |material| {
            assert_eq!(material.secret_nonce(), &secret);
            Ok(AdaptorPartialSignature::new([OWN_PARTIAL; 32]))
        })
        .expect("in-flight legacy session still signs");
    assert!(matches!(
        journal.reserve(reservation(fresh, SECRET), &key()),
        Err(AdaptorSessionJournalError::Store(_))
    ));
}
