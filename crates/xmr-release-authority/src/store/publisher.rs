//! Transaction-scoped publication state machine.
//!
//! This boundary remains crate-private until a concrete, authenticated LEZ node
//! capability replaces the test transport. No generic sidecar submission route
//! may call it.
#![cfg_attr(not(test), allow(dead_code))]

use super::{
    PublicationDecision, PublicationProtectionKey, ReleaseError, ReleaseSnapshot, ReleaseState,
    ReleaseStore,
};
use async_trait::async_trait;
use thiserror::Error;

/// Exact node-mempool admission status; neither variant proves chain finality.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PublicationAdmissionStatus {
    Accepted,
    AlreadyKnown,
}

/// Node admission bound to the canonical submitted transaction identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PublicationAdmission {
    status: PublicationAdmissionStatus,
    transaction_id: [u8; 32],
}

/// Redacted transport failure after the journal has granted publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub(crate) enum PublicationTransportError {
    #[error("finalized LEZ clock or publication transport is unavailable")]
    Unavailable,
    #[error("LEZ node returned an invalid publication response")]
    InvalidResponse,
}

/// Authenticated finalized-clock and exact-publication capability.
///
/// Implementations must bind both calls to the supplied canonical target. The
/// concrete production implementation will be sealed inside the actor that owns
/// the authenticated LEZ node client.
#[async_trait]
pub(crate) trait XmrAuthorizationPublicationTransport: Send {
    /// Returns finalized LEZ consensus time in milliseconds.
    async fn finalized_lez_timestamp(
        &mut self,
        authenticated_target: &[u8],
    ) -> Result<u64, PublicationTransportError>;

    /// Submits the exact authenticated transaction and performs no retry.
    async fn submit_exact_authorization(
        &mut self,
        authenticated_target: &[u8],
        exact_publication: &[u8],
    ) -> Result<PublicationAdmission, PublicationTransportError>;
}

/// Durable publication result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReleasePublicationOutcome {
    /// The node accepted or already knew the exact bytes; finality is pending.
    Admitted(PublicationAdmissionStatus),
    /// Submission may have reached the node; no retry is permitted.
    Ambiguous,
    /// The post-CAS clock gate proved that no node call was made.
    Suppressed,
    /// Another process/restart owns or completed the only publication attempt.
    ObserveOnly,
}

/// Failure before a publication attempt or while persisting its terminal state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub(crate) enum ReleasePublicationError {
    #[error(transparent)]
    Journal(#[from] ReleaseError),
    #[error("finalized LEZ consensus time is unavailable")]
    FinalizedClockUnavailable,
    #[error("finalized LEZ consensus time is outside the release window")]
    OutsideWindow,
}

impl ReleaseStore {
    /// Permits one publication after decisive finalized-time validation.
    ///
    /// Failures before the compare-and-swap leave the journal prepared. The
    /// winner samples finalized time again and suppresses without a node call on
    /// regression, expiry, or clock failure. The future concrete issuer must
    /// also prove that the exact transaction's checked-guest validity predicate
    /// has the same exclusive end; the post-CAS sample narrows but cannot
    /// eliminate the final clock-to-node scheduling interval.
    pub(crate) async fn publish_or_observe<T: XmrAuthorizationPublicationTransport>(
        &self,
        snapshot: ReleaseSnapshot,
        key: &PublicationProtectionKey,
        transport: &mut T,
    ) -> Result<ReleasePublicationOutcome, ReleasePublicationError> {
        self.validate_for_publication(&snapshot, key)?;
        if snapshot.state() != ReleaseState::Prepared {
            return Ok(ReleasePublicationOutcome::ObserveOnly);
        }

        let target = snapshot.target().to_vec();
        let window = snapshot.window();
        let initial_timestamp = transport
            .finalized_lez_timestamp(&target)
            .await
            .map_err(|_| ReleasePublicationError::FinalizedClockUnavailable)?;
        if initial_timestamp < window.start() || initial_timestamp >= window.end() {
            return Err(ReleasePublicationError::OutsideWindow);
        }

        let attempt = match self.begin_publication(snapshot, key)? {
            PublicationDecision::Send(attempt) => attempt,
            PublicationDecision::ObserveOnly => {
                return Ok(ReleasePublicationOutcome::ObserveOnly);
            }
        };
        let decisive_timestamp = transport.finalized_lez_timestamp(&attempt.target).await;
        let Ok(decisive_timestamp) = decisive_timestamp else {
            self.mark_suppressed(*attempt, key)?;
            return Ok(ReleasePublicationOutcome::Suppressed);
        };
        if decisive_timestamp < initial_timestamp
            || decisive_timestamp < attempt.window.start()
            || decisive_timestamp >= attempt.window.end()
        {
            self.mark_suppressed(*attempt, key)?;
            return Ok(ReleasePublicationOutcome::Suppressed);
        }

        let exact_publication = attempt
            .opened_intent(key)
            .map_err(|_| ReleaseError::Authentication)?;
        if let Ok(admission) = transport
            .submit_exact_authorization(&attempt.target, &exact_publication)
            .await
            && admission.transaction_id == attempt.publication_id
        {
            self.mark_admitted(*attempt, key)?;
            Ok(ReleasePublicationOutcome::Admitted(admission.status))
        } else {
            self.mark_ambiguous(*attempt, key)?;
            Ok(ReleasePublicationOutcome::Ambiguous)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ReleasePlan, derive_activation_id};
    use std::{collections::VecDeque, fs, os::unix::fs::PermissionsExt, path::PathBuf};
    use tempfile::{TempDir, tempdir};
    use zeroize::Zeroizing;

    const PUBLICATION_ID: [u8; 32] = [38; 32];

    struct Transport {
        timestamps: VecDeque<Result<u64, PublicationTransportError>>,
        admission: Result<PublicationAdmission, PublicationTransportError>,
        clock_calls: usize,
        submission_calls: usize,
    }

    impl Transport {
        fn successful(timestamp: u64, status: PublicationAdmissionStatus) -> Self {
            Self {
                timestamps: VecDeque::from([Ok(timestamp), Ok(timestamp)]),
                admission: Ok(PublicationAdmission {
                    status,
                    transaction_id: PUBLICATION_ID,
                }),
                clock_calls: 0,
                submission_calls: 0,
            }
        }

        fn with_timestamps(
            timestamps: impl IntoIterator<Item = Result<u64, PublicationTransportError>>,
        ) -> Self {
            let mut transport = Self::successful(100, PublicationAdmissionStatus::Accepted);
            transport.timestamps = timestamps.into_iter().collect();
            transport
        }
    }

    #[async_trait]
    impl XmrAuthorizationPublicationTransport for Transport {
        async fn finalized_lez_timestamp(
            &mut self,
            authenticated_target: &[u8],
        ) -> Result<u64, PublicationTransportError> {
            assert_eq!(authenticated_target, b"lez-authenticated-target");
            self.clock_calls += 1;
            tokio::task::yield_now().await;
            self.timestamps.pop_front().expect("expected clock call")
        }

        async fn submit_exact_authorization(
            &mut self,
            authenticated_target: &[u8],
            exact_publication: &[u8],
        ) -> Result<PublicationAdmission, PublicationTransportError> {
            assert_eq!(authenticated_target, b"lez-authenticated-target");
            assert_eq!(exact_publication, b"exact-xmr-claim-authorization");
            self.submission_calls += 1;
            self.admission
        }
    }

    fn directory() -> TempDir {
        let directory = tempdir().unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
        directory
    }

    fn database_path(directory: &TempDir) -> PathBuf {
        directory.path().join("release.sqlite")
    }

    fn key() -> PublicationProtectionKey {
        PublicationProtectionKey::new("publisher-test-v1", [23; 32]).unwrap()
    }

    fn plan() -> ReleasePlan {
        let swap_id = [31; 32];
        let run_id = [32; 32];
        ReleasePlan {
            activation: derive_activation_id(&swap_id, &run_id),
            swap_id,
            run_id,
            lez_commitment: [33; 32],
            topology_commitment: [34; 32],
            resource_id: [35; 32],
            observation: vec![36; 48],
            claim_partial_commitment: [37; 32],
            target: b"lez-authenticated-target".to_vec(),
            publication_id: PUBLICATION_ID,
            window_start: 100,
            window_end: 200,
            publication: Zeroizing::new(b"exact-xmr-claim-authorization".to_vec()),
        }
    }

    #[tokio::test]
    async fn admitted_publication_is_terminal_across_restart() {
        for admission in [
            PublicationAdmissionStatus::Accepted,
            PublicationAdmissionStatus::AlreadyKnown,
        ] {
            let directory = directory();
            let path = database_path(&directory);
            let key = key();
            let store = ReleaseStore::open(&path).unwrap();
            let snapshot = store.prepare(plan(), &key).unwrap();
            let activation = snapshot.activation();
            let run_id = snapshot.run_id();
            let mut transport = Transport::successful(100, admission);

            assert_eq!(
                store
                    .publish_or_observe(snapshot, &key, &mut transport)
                    .await
                    .unwrap(),
                ReleasePublicationOutcome::Admitted(admission)
            );
            assert_eq!((transport.clock_calls, transport.submission_calls), (2, 1));
            drop(store);

            let reopened = ReleaseStore::open(&path).unwrap();
            let snapshot = reopened
                .load_by_activation_run(activation, run_id, &key)
                .unwrap();
            assert_eq!(snapshot.state(), ReleaseState::Admitted);
            assert_eq!(
                reopened
                    .publish_or_observe(snapshot, &key, &mut transport)
                    .await
                    .unwrap(),
                ReleasePublicationOutcome::ObserveOnly
            );
            assert_eq!((transport.clock_calls, transport.submission_calls), (2, 1));
        }
    }

    #[tokio::test]
    async fn clock_failure_and_exclusive_end_leave_prepared() {
        for timestamp in [Err(PublicationTransportError::Unavailable), Ok(200)] {
            let directory = directory();
            let key = key();
            let store = ReleaseStore::open(database_path(&directory)).unwrap();
            let snapshot = store.prepare(plan(), &key).unwrap();
            let activation = snapshot.activation();
            let run_id = snapshot.run_id();
            let mut transport = Transport::with_timestamps([timestamp]);

            let expected = if timestamp.is_err() {
                ReleasePublicationError::FinalizedClockUnavailable
            } else {
                ReleasePublicationError::OutsideWindow
            };
            assert_eq!(
                store
                    .publish_or_observe(snapshot, &key, &mut transport)
                    .await
                    .unwrap_err(),
                expected
            );
            assert_eq!((transport.clock_calls, transport.submission_calls), (1, 0));
            let loaded = store
                .load_by_activation_run(activation, run_id, &key)
                .unwrap();
            assert_eq!(loaded.state(), ReleaseState::Prepared);
        }
    }

    #[tokio::test]
    async fn submission_error_becomes_ambiguous_and_observe_only() {
        let directory = directory();
        let path = database_path(&directory);
        let key = key();
        let store = ReleaseStore::open(&path).unwrap();
        let snapshot = store.prepare(plan(), &key).unwrap();
        let activation = snapshot.activation();
        let run_id = snapshot.run_id();
        let mut transport = Transport::successful(199, PublicationAdmissionStatus::Accepted);
        transport.admission = Err(PublicationTransportError::InvalidResponse);

        assert_eq!(
            store
                .publish_or_observe(snapshot, &key, &mut transport)
                .await
                .unwrap(),
            ReleasePublicationOutcome::Ambiguous
        );
        assert_eq!((transport.clock_calls, transport.submission_calls), (2, 1));
        drop(store);

        let reopened = ReleaseStore::open(&path).unwrap();
        let snapshot = reopened
            .load_by_activation_run(activation, run_id, &key)
            .unwrap();
        assert_eq!(snapshot.state(), ReleaseState::Ambiguous);
        assert_eq!(
            reopened
                .publish_or_observe(snapshot, &key, &mut transport)
                .await
                .unwrap(),
            ReleasePublicationOutcome::ObserveOnly
        );
        assert_eq!((transport.clock_calls, transport.submission_calls), (2, 1));
    }

    #[tokio::test]
    async fn post_cas_expiry_is_suppressed_without_node_call() {
        let directory = directory();
        let path = database_path(&directory);
        let key = key();
        let store = ReleaseStore::open(&path).unwrap();
        let snapshot = store.prepare(plan(), &key).unwrap();
        let activation = snapshot.activation();
        let run_id = snapshot.run_id();
        let mut transport = Transport::with_timestamps([Ok(199), Ok(200)]);

        assert_eq!(
            store
                .publish_or_observe(snapshot, &key, &mut transport)
                .await
                .unwrap(),
            ReleasePublicationOutcome::Suppressed
        );
        assert_eq!((transport.clock_calls, transport.submission_calls), (2, 0));
        drop(store);

        let reopened = ReleaseStore::open(&path).unwrap();
        let snapshot = reopened
            .load_by_activation_run(activation, run_id, &key)
            .unwrap();
        assert_eq!(snapshot.state(), ReleaseState::Suppressed);
        assert_eq!(
            reopened
                .publish_or_observe(snapshot, &key, &mut transport)
                .await
                .unwrap(),
            ReleasePublicationOutcome::ObserveOnly
        );
        assert_eq!((transport.clock_calls, transport.submission_calls), (2, 0));
    }

    #[tokio::test]
    async fn wrong_returned_transaction_id_is_ambiguous_not_admitted() {
        let directory = directory();
        let key = key();
        let store = ReleaseStore::open(database_path(&directory)).unwrap();
        let snapshot = store.prepare(plan(), &key).unwrap();
        let activation = snapshot.activation();
        let run_id = snapshot.run_id();
        let mut transport = Transport::successful(150, PublicationAdmissionStatus::Accepted);
        transport.admission = Ok(PublicationAdmission {
            status: PublicationAdmissionStatus::Accepted,
            transaction_id: [99; 32],
        });

        assert_eq!(
            store
                .publish_or_observe(snapshot, &key, &mut transport)
                .await
                .unwrap(),
            ReleasePublicationOutcome::Ambiguous
        );
        assert_eq!((transport.clock_calls, transport.submission_calls), (2, 1));
        let loaded = store
            .load_by_activation_run(activation, run_id, &key)
            .unwrap();
        assert_eq!(loaded.state(), ReleaseState::Ambiguous);
    }

    #[tokio::test]
    async fn two_connections_have_one_submit_winner() {
        let directory = directory();
        let path = database_path(&directory);
        let key = key();
        let first_store = ReleaseStore::open(&path).unwrap();
        let first_snapshot = first_store.prepare(plan(), &key).unwrap();
        let activation = first_snapshot.activation();
        let run_id = first_snapshot.run_id();
        let second_store = ReleaseStore::open(&path).unwrap();
        let second_snapshot = second_store
            .load_by_activation_run(activation, run_id, &key)
            .unwrap();
        let mut first_transport = Transport::successful(150, PublicationAdmissionStatus::Accepted);
        let mut second_transport = Transport::successful(150, PublicationAdmissionStatus::Accepted);

        let (first, second) = tokio::join!(
            first_store.publish_or_observe(first_snapshot, &key, &mut first_transport),
            second_store.publish_or_observe(second_snapshot, &key, &mut second_transport),
        );
        let outcomes = [first.unwrap(), second.unwrap()];
        assert!(outcomes.contains(&ReleasePublicationOutcome::Admitted(
            PublicationAdmissionStatus::Accepted
        )));
        assert!(outcomes.contains(&ReleasePublicationOutcome::ObserveOnly));
        assert_eq!(
            first_transport.submission_calls + second_transport.submission_calls,
            1
        );

        drop((first_store, second_store));
        let reopened = ReleaseStore::open(path).unwrap();
        assert_eq!(
            reopened
                .load_by_activation_run(activation, run_id, &key)
                .unwrap()
                .state(),
            ReleaseState::Admitted
        );
    }
}
