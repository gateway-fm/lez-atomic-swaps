//! Durable maker offers and one-winner acceptance transitions.

use lez_bridge_protocol::RequestId;
use lez_swap_core::{Pair, Phase, SwapCoordinator, SwapDirection, SwapId};
use rusqlite::{OptionalExtension as _, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{
    LocalPriceV1, MakerPairConfigurationV1, MakerPriceSourceKind, MakerRouteV1,
    SWAP_PAYLOAD_VERSION, SqliteSwapStore, StoreError,
};

const OFFER_PAYLOAD_VERSION: i64 = 1;

/// Invalid immutable maker-offer input.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum MakerOfferError {
    /// Offer identity was not bounded log-safe ASCII.
    #[error("offer ID must be 8..=64 safe ASCII bytes")]
    InvalidIdentifier,
    /// Trusted publication time or derived expiry was invalid or oversized.
    #[error("offer publication time or expiry is invalid")]
    InvalidTime,
    /// Price, policy, route, or revision snapshots were inconsistent.
    #[error("offer snapshot is internally inconsistent")]
    InvalidSnapshot,
}

/// Bounded log-safe durable offer identity.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct MakerOfferId(String);

impl MakerOfferId {
    /// Validates and constructs an offer identifier.
    ///
    /// # Errors
    ///
    /// Rejects values outside 8..=64 bytes or the safe ASCII grammar.
    pub fn new(value: impl Into<String>) -> Result<Self, MakerOfferError> {
        let value = value.into();
        if (8..=64).contains(&value.len())
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            Ok(Self(value))
        } else {
            Err(MakerOfferError::InvalidIdentifier)
        }
    }

    /// Borrows the validated identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for MakerOfferId {
    type Error = MakerOfferError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<MakerOfferId> for String {
    fn from(value: MakerOfferId) -> Self {
        value.0
    }
}

/// Effective offer lifecycle state returned to operator and discovery clients.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MakerOfferStatus {
    /// Published, unexpired, and eligible for exactly one reservation.
    Active,
    /// Never reserved and no longer discoverable at the caller's trusted time.
    Expired,
    /// Accepted by one negotiation identity but not yet bound to a swap.
    Reserved,
    /// Bound to one durable swap identity.
    Consumed,
    /// Explicitly removed before reservation.
    Withdrawn,
}

/// Immutable offer terms snapshotted from one enabled route and exact price.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MakerOfferV1 {
    id: MakerOfferId,
    pair_configuration: MakerPairConfigurationV1,
    price: LocalPriceV1,
    pair_configuration_revision: u64,
    price_source_revision: u64,
    price_observed_at_unix_seconds: u64,
    created_at_unix_seconds: u64,
    expires_at_unix_seconds: u64,
}

impl MakerOfferV1 {
    /// Durable offer identity.
    #[must_use]
    pub const fn id(&self) -> &MakerOfferId {
        &self.id
    }

    /// Exact pair and direction.
    #[must_use]
    pub const fn route(&self) -> MakerRouteV1 {
        self.pair_configuration.route()
    }

    /// Complete route policy snapshot used for publication.
    #[must_use]
    pub const fn pair_configuration(&self) -> &MakerPairConfigurationV1 {
        &self.pair_configuration
    }

    /// Inclusive smallest foreign atomic-unit amount.
    #[must_use]
    pub const fn minimum_foreign_units(&self) -> u64 {
        self.pair_configuration.minimum_foreign_units()
    }

    /// Inclusive largest foreign atomic-unit amount.
    #[must_use]
    pub const fn maximum_foreign_units(&self) -> u64 {
        self.pair_configuration.maximum_foreign_units()
    }

    /// Exact reduced-integer price snapshot.
    #[must_use]
    pub const fn price(&self) -> &LocalPriceV1 {
        &self.price
    }

    /// Route-policy revision used to publish this offer.
    #[must_use]
    pub const fn pair_configuration_revision(&self) -> u64 {
        self.pair_configuration_revision
    }

    /// Price-source revision used to publish this offer.
    #[must_use]
    pub const fn price_source_revision(&self) -> u64 {
        self.price_source_revision
    }

    /// Trusted time at which the selected source was observed.
    #[must_use]
    pub const fn price_observed_at_unix_seconds(&self) -> u64 {
        self.price_observed_at_unix_seconds
    }

    /// Trusted daemon publication time.
    #[must_use]
    pub const fn created_at_unix_seconds(&self) -> u64 {
        self.created_at_unix_seconds
    }

    /// Exclusive trusted-time discovery/reservation boundary.
    #[must_use]
    pub const fn expires_at_unix_seconds(&self) -> u64 {
        self.expires_at_unix_seconds
    }

    fn validate(&self) -> Result<(), MakerOfferError> {
        MakerOfferId::new(self.id.as_str())?;
        let validated_policy = MakerPairConfigurationV1::new(
            self.pair_configuration.route(),
            self.pair_configuration.enabled(),
            self.pair_configuration.price_source(),
            self.pair_configuration.minimum_foreign_units(),
            self.pair_configuration.maximum_foreign_units(),
            self.pair_configuration.offer_ttl_seconds(),
        )
        .map_err(|_| MakerOfferError::InvalidSnapshot)?;
        if validated_policy != self.pair_configuration
            || !self.pair_configuration.enabled()
            || self.pair_configuration.price_source() != MakerPriceSourceKind::Local
            || self.price.route() != self.route()
            || self.pair_configuration_revision == 0
            || self.price_source_revision == 0
            || self.price_observed_at_unix_seconds != self.created_at_unix_seconds
        {
            return Err(MakerOfferError::InvalidSnapshot);
        }
        if self.created_at_unix_seconds >= self.expires_at_unix_seconds
            || self.expires_at_unix_seconds > i64::MAX as u64
            || self.expires_at_unix_seconds - self.created_at_unix_seconds
                != self.pair_configuration.offer_ttl_seconds()
        {
            return Err(MakerOfferError::InvalidTime);
        }
        LocalPriceV1::new(
            self.price.route(),
            self.price.lez_units_per_lot(),
            self.price.foreign_units_per_lot(),
        )
        .map_err(|_| MakerOfferError::InvalidSnapshot)?;
        Ok(())
    }
}

/// Durable offer view with its one-winner state and revision.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MakerOfferRecordV1 {
    revision: u64,
    status: MakerOfferStatus,
    offer: MakerOfferV1,
    reservation_id: Option<RequestId>,
    swap_id: Option<Box<str>>,
}

impl MakerOfferRecordV1 {
    /// Monotonic offer-local transition revision.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Effective state at the trusted time supplied to the read.
    #[must_use]
    pub const fn status(&self) -> MakerOfferStatus {
        self.status
    }

    /// Immutable published terms.
    #[must_use]
    pub const fn offer(&self) -> &MakerOfferV1 {
        &self.offer
    }

    /// Winning negotiation identity, if accepted.
    #[must_use]
    pub const fn reservation_id(&self) -> Option<&RequestId> {
        self.reservation_id.as_ref()
    }

    /// Swap identity bound after negotiation, if consumed.
    #[must_use]
    pub fn swap_id(&self) -> Option<&str> {
        self.swap_id.as_deref()
    }
}

/// Result of one atomic offer mutation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MakerOfferCommit {
    revision: u64,
    was_replay: bool,
}

impl MakerOfferCommit {
    /// Durable offer-local revision.
    #[must_use]
    pub const fn revision(self) -> u64 {
        self.revision
    }

    /// Whether the exact request and result were already committed.
    #[must_use]
    pub const fn was_replay(self) -> bool {
        self.was_replay
    }
}

#[derive(Deserialize, Serialize)]
struct StoredOfferCommitV1 {
    schema_version: u16,
    revision: u64,
}

#[derive(Serialize)]
struct PublishRequest<'a> {
    offer_id: &'a MakerOfferId,
    route: MakerRouteV1,
    now_unix_seconds: u64,
}

#[derive(Serialize)]
struct ReserveRequest<'a> {
    offer_id: &'a MakerOfferId,
    expected_revision: u64,
    reservation_id: &'a RequestId,
    now_unix_seconds: u64,
}

#[derive(Serialize)]
struct ConsumeRequest<'a> {
    offer_id: &'a MakerOfferId,
    expected_revision: u64,
    reservation_id: &'a RequestId,
    swap: &'a SwapCoordinator,
}

#[derive(Serialize)]
struct WithdrawRequest<'a> {
    offer_id: &'a MakerOfferId,
    expected_revision: u64,
}

impl SqliteSwapStore {
    /// Publishes one local-price offer and snapshots policy and price revisions atomically.
    ///
    /// # Errors
    ///
    /// Returns an error for a disabled/non-local route, missing/corrupt source
    /// records, duplicate identity, request conflict, time overflow, or `SQLite` failure.
    pub fn publish_local_offer(
        &mut self,
        request_id: &RequestId,
        offer_id: &MakerOfferId,
        route: MakerRouteV1,
        now_unix_seconds: u64,
    ) -> Result<MakerOfferCommit, StoreError> {
        let request_json = serde_json::to_string(&PublishRequest {
            offer_id,
            route,
            now_unix_seconds,
        })?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(commit) =
            replay_offer_mutation(&transaction, request_id, "offer_publish", &request_json)?
        {
            transaction.commit()?;
            return Ok(commit);
        }
        if now_unix_seconds > i64::MAX as u64 {
            return Err(MakerOfferError::InvalidTime.into());
        }
        let offer_exists: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM maker_offers WHERE offer_id = ?1)",
            params![offer_id.as_str()],
            |row| row.get(0),
        )?;
        if offer_exists {
            return Err(StoreError::MakerOfferAlreadyExists);
        }
        let (policy, policy_revision) =
            load_pair(&transaction, route)?.ok_or(StoreError::MissingMakerPair)?;
        if !policy.enabled() {
            return Err(StoreError::MakerRouteDisabled);
        }
        if policy.price_source() != MakerPriceSourceKind::Local {
            return Err(StoreError::MakerPriceSourceMismatch);
        }
        let (price, price_revision) =
            load_price(&transaction, route)?.ok_or(StoreError::MissingMakerLocalPrice)?;
        let expires_at_unix_seconds = now_unix_seconds
            .checked_add(policy.offer_ttl_seconds())
            .filter(|value| i64::try_from(*value).is_ok())
            .ok_or(MakerOfferError::InvalidTime)?;
        let offer = MakerOfferV1 {
            id: offer_id.clone(),
            pair_configuration: policy,
            price,
            pair_configuration_revision: policy_revision,
            price_source_revision: price_revision,
            price_observed_at_unix_seconds: now_unix_seconds,
            created_at_unix_seconds: now_unix_seconds,
            expires_at_unix_seconds,
        };
        offer.validate()?;
        transaction.execute(
            "INSERT INTO maker_offers (
                 offer_id, pair, direction, payload_version, payload_json,
                 expires_at_unix_seconds, state, revision, updated_request_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active', 1, ?7)",
            params![
                offer_id.as_str(),
                pair_name(route.pair()),
                direction_name(route.direction()),
                OFFER_PAYLOAD_VERSION,
                serde_json::to_string(&offer)?,
                u64_to_sql(expires_at_unix_seconds)?,
                request_id.as_str(),
            ],
        )?;
        persist_offer_mutation(&transaction, request_id, "offer_publish", &request_json, 1)?;
        transaction.commit()?;
        Ok(MakerOfferCommit {
            revision: 1,
            was_replay: false,
        })
    }

    /// Atomically reserves one still-active and unexpired offer for one negotiation.
    ///
    /// # Errors
    ///
    /// Fails closed on expiry, non-active state, stale revision, request conflict,
    /// missing/corrupt state, time overflow, or `SQLite` failure.
    pub fn reserve_maker_offer(
        &mut self,
        request_id: &RequestId,
        offer_id: &MakerOfferId,
        expected_revision: u64,
        reservation_id: &RequestId,
        now_unix_seconds: u64,
    ) -> Result<MakerOfferCommit, StoreError> {
        let request_json = serde_json::to_string(&ReserveRequest {
            offer_id,
            expected_revision,
            reservation_id,
            now_unix_seconds,
        })?;
        transition_offer(
            self,
            OfferTransitionContext {
                request_id,
                operation: "offer_reserve",
                request_json: &request_json,
                offer_id,
                expected_revision,
                swap_to_insert: None,
            },
            |record| {
                if now_unix_seconds > i64::MAX as u64
                    || now_unix_seconds >= record.offer.expires_at_unix_seconds()
                {
                    return Err(StoreError::MakerOfferExpired);
                }
                if record.status != MakerOfferStatus::Active {
                    return Err(StoreError::MakerOfferUnavailable);
                }
                Ok(("reserved", Some(reservation_id.as_str().to_owned()), None))
            },
        )
    }

    /// Atomically binds the winning reservation to one validated swap identity.
    ///
    /// Expiry is intentionally not rechecked: reservation is the acceptance
    /// linearization point, so time passing cannot revoke already accepted terms.
    ///
    /// # Errors
    ///
    /// Fails on a wrong reservation, non-reserved state, stale revision,
    /// request conflict, missing/corrupt state, or `SQLite` failure.
    pub fn consume_maker_offer(
        &mut self,
        request_id: &RequestId,
        offer_id: &MakerOfferId,
        expected_revision: u64,
        reservation_id: &RequestId,
        swap: &SwapCoordinator,
    ) -> Result<MakerOfferCommit, StoreError> {
        let request_json = serde_json::to_string(&ConsumeRequest {
            offer_id,
            expected_revision,
            reservation_id,
            swap,
        })?;
        transition_offer(
            self,
            OfferTransitionContext {
                request_id,
                operation: "offer_consume",
                request_json: &request_json,
                offer_id,
                expected_revision,
                swap_to_insert: Some(swap),
            },
            |record| {
                if record.status != MakerOfferStatus::Reserved
                    || record.reservation_id.as_ref() != Some(reservation_id)
                {
                    return Err(StoreError::MakerOfferReservationConflict);
                }
                if record.offer.route().pair() != swap.pair()
                    || record.offer.route().direction() != swap.direction()
                    || swap.phase() != Phase::Offered
                {
                    return Err(StoreError::MakerOfferSwapMismatch);
                }
                Ok((
                    "consumed",
                    Some(reservation_id.as_str().to_owned()),
                    Some(swap.id().as_str().to_owned()),
                ))
            },
        )
    }

    /// Atomically withdraws one active offer before reservation.
    ///
    /// # Errors
    ///
    /// Fails on non-active state, stale revision, request conflict,
    /// missing/corrupt state, or `SQLite` failure.
    pub fn withdraw_maker_offer(
        &mut self,
        request_id: &RequestId,
        offer_id: &MakerOfferId,
        expected_revision: u64,
    ) -> Result<MakerOfferCommit, StoreError> {
        let request_json = serde_json::to_string(&WithdrawRequest {
            offer_id,
            expected_revision,
        })?;
        transition_offer(
            self,
            OfferTransitionContext {
                request_id,
                operation: "offer_withdraw",
                request_json: &request_json,
                offer_id,
                expected_revision,
                swap_to_insert: None,
            },
            |record| {
                if record.status != MakerOfferStatus::Active {
                    return Err(StoreError::MakerOfferUnavailable);
                }
                Ok(("withdrawn", None, None))
            },
        )
    }

    /// Lists active, unexpired offers in stable identity order.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid trusted time, corrupt state, or `SQLite` failure.
    pub fn list_discoverable_maker_offers(
        &self,
        now_unix_seconds: u64,
    ) -> Result<Vec<MakerOfferRecordV1>, StoreError> {
        if now_unix_seconds > i64::MAX as u64 {
            return Err(MakerOfferError::InvalidTime.into());
        }
        list_offers(
            &self.connection,
            "WHERE state = 'active' AND expires_at_unix_seconds > ?1",
            Some(now_unix_seconds),
            now_unix_seconds,
        )
    }

    /// Lists complete offer history with expiry projected at trusted caller time.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid trusted time, corrupt state, or `SQLite` failure.
    pub fn list_maker_offer_history(
        &self,
        now_unix_seconds: u64,
    ) -> Result<Vec<MakerOfferRecordV1>, StoreError> {
        if now_unix_seconds > i64::MAX as u64 {
            return Err(MakerOfferError::InvalidTime.into());
        }
        list_offers(&self.connection, "", None, now_unix_seconds)
    }
}

#[derive(Clone, Copy)]
struct OfferTransitionContext<'a> {
    request_id: &'a RequestId,
    operation: &'static str,
    request_json: &'a str,
    offer_id: &'a MakerOfferId,
    expected_revision: u64,
    swap_to_insert: Option<&'a SwapCoordinator>,
}

fn transition_offer<F>(
    store: &mut SqliteSwapStore,
    context: OfferTransitionContext<'_>,
    transition: F,
) -> Result<MakerOfferCommit, StoreError>
where
    F: FnOnce(
        &MakerOfferRecordV1,
    ) -> Result<(&'static str, Option<String>, Option<String>), StoreError>,
{
    let transaction = store
        .connection
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(commit) = replay_offer_mutation(
        &transaction,
        context.request_id,
        context.operation,
        context.request_json,
    )? {
        transaction.commit()?;
        return Ok(commit);
    }
    let record =
        load_offer(&transaction, context.offer_id, 0)?.ok_or(StoreError::MissingMakerOffer)?;
    if record.revision != context.expected_revision {
        return Err(StoreError::StaleMakerOffer {
            expected: context.expected_revision,
            actual: record.revision,
        });
    }
    let (state, reservation_id, swap_id) = transition(&record)?;
    if let Some(swap) = context.swap_to_insert {
        transaction.execute(
            "INSERT INTO swaps (id, schema_version, state_json) VALUES (?1, ?2, ?3)",
            params![
                swap.id().as_str(),
                SWAP_PAYLOAD_VERSION,
                serde_json::to_string(swap)?,
            ],
        )?;
    }
    let revision = context
        .expected_revision
        .checked_add(1)
        .ok_or(StoreError::RevisionOverflow)?;
    let updated = transaction.execute(
        "UPDATE maker_offers SET state = ?1, revision = ?2,
             reservation_id = ?3, swap_id = ?4, updated_request_id = ?5
         WHERE offer_id = ?6 AND revision = ?7",
        params![
            state,
            u64_to_sql(revision)?,
            reservation_id.as_deref(),
            swap_id.as_deref(),
            context.request_id.as_str(),
            context.offer_id.as_str(),
            u64_to_sql(context.expected_revision)?,
        ],
    )?;
    if updated != 1 {
        return Err(StoreError::StaleMakerOffer {
            expected: context.expected_revision,
            actual: record.revision,
        });
    }
    persist_offer_mutation(
        &transaction,
        context.request_id,
        context.operation,
        context.request_json,
        revision,
    )?;
    transaction.commit()?;
    Ok(MakerOfferCommit {
        revision,
        was_replay: false,
    })
}

fn replay_offer_mutation(
    transaction: &rusqlite::Transaction<'_>,
    request_id: &RequestId,
    operation: &str,
    request_json: &str,
) -> Result<Option<MakerOfferCommit>, StoreError> {
    let row = transaction
        .query_row(
            "SELECT operation, request_json, result_json FROM maker_application_mutations
             WHERE request_id = ?1",
            params![request_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((stored_operation, stored_request, stored_result)) = row else {
        return Ok(None);
    };
    if stored_operation != operation || stored_request != request_json {
        return Err(StoreError::MakerOfferRequestConflict);
    }
    let result: StoredOfferCommitV1 = serde_json::from_str(&stored_result)?;
    if result.schema_version != 1 || result.revision == 0 {
        return Err(StoreError::CorruptMakerOffer);
    }
    Ok(Some(MakerOfferCommit {
        revision: result.revision,
        was_replay: true,
    }))
}

fn persist_offer_mutation(
    transaction: &rusqlite::Transaction<'_>,
    request_id: &RequestId,
    operation: &str,
    request_json: &str,
    revision: u64,
) -> Result<(), StoreError> {
    let result_json = serde_json::to_string(&StoredOfferCommitV1 {
        schema_version: 1,
        revision,
    })?;
    transaction.execute(
        "INSERT INTO maker_application_mutations (
             request_id, operation, request_payload_version, request_json, result_json
         ) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            request_id.as_str(),
            operation,
            OFFER_PAYLOAD_VERSION,
            request_json,
            result_json,
        ],
    )?;
    Ok(())
}

fn load_pair(
    transaction: &rusqlite::Transaction<'_>,
    route: MakerRouteV1,
) -> Result<Option<(MakerPairConfigurationV1, u64)>, StoreError> {
    transaction
        .query_row(
            "SELECT payload_version, payload_json, revision FROM maker_pair_configurations
             WHERE pair = ?1 AND direction = ?2",
            params![pair_name(route.pair()), direction_name(route.direction())],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()?
        .map(|(version, json, revision)| {
            check_version(version, "maker pair configuration")?;
            let value: MakerPairConfigurationV1 = serde_json::from_str(&json)?;
            let validated = MakerPairConfigurationV1::new(
                value.route(),
                value.enabled(),
                value.price_source(),
                value.minimum_foreign_units(),
                value.maximum_foreign_units(),
                value.offer_ttl_seconds(),
            )?;
            if validated.route() != route || validated != value {
                return Err(StoreError::CorruptMakerConfiguration);
            }
            Ok((value, sql_to_u64(revision)?))
        })
        .transpose()
}

fn load_price(
    transaction: &rusqlite::Transaction<'_>,
    route: MakerRouteV1,
) -> Result<Option<(LocalPriceV1, u64)>, StoreError> {
    transaction
        .query_row(
            "SELECT payload_version, payload_json, revision FROM maker_local_prices
             WHERE pair = ?1 AND direction = ?2",
            params![pair_name(route.pair()), direction_name(route.direction())],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()?
        .map(|(version, json, revision)| {
            check_version(version, "maker local price")?;
            let value: LocalPriceV1 = serde_json::from_str(&json)?;
            let validated = LocalPriceV1::new(
                value.route(),
                value.lez_units_per_lot(),
                value.foreign_units_per_lot(),
            )?;
            if validated.route() != route || validated != value {
                return Err(StoreError::CorruptMakerConfiguration);
            }
            Ok((value, sql_to_u64(revision)?))
        })
        .transpose()
}

fn list_offers(
    connection: &rusqlite::Connection,
    predicate: &str,
    time_parameter: Option<u64>,
    now_unix_seconds: u64,
) -> Result<Vec<MakerOfferRecordV1>, StoreError> {
    let sql = format!(
        "SELECT offer_id, pair, direction, payload_version, payload_json,
                expires_at_unix_seconds, state, revision, reservation_id, swap_id
         FROM maker_offers {predicate} ORDER BY offer_id"
    );
    let mut statement = connection.prepare(&sql)?;
    let mut rows = match time_parameter {
        Some(value) => statement.query(params![u64_to_sql(value)?])?,
        None => statement.query([])?,
    };
    let mut records = Vec::new();
    while let Some(row) = rows.next()? {
        records.push(decode_offer_tuple(
            (
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
            ),
            now_unix_seconds,
        )?);
    }
    Ok(records)
}

fn load_offer(
    transaction: &rusqlite::Transaction<'_>,
    offer_id: &MakerOfferId,
    now_unix_seconds: u64,
) -> Result<Option<MakerOfferRecordV1>, StoreError> {
    transaction
        .query_row(
            "SELECT offer_id, pair, direction, payload_version, payload_json,
                    expires_at_unix_seconds, state, revision, reservation_id, swap_id
             FROM maker_offers WHERE offer_id = ?1",
            params![offer_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<String>>(9)?,
                ))
            },
        )
        .optional()?
        .map(|row| decode_offer_tuple(row, now_unix_seconds))
        .transpose()
}

type OfferRow = (
    String,
    String,
    String,
    i64,
    String,
    i64,
    String,
    i64,
    Option<String>,
    Option<String>,
);

fn decode_offer_tuple(row: OfferRow, now: u64) -> Result<MakerOfferRecordV1, StoreError> {
    let (offer_id, pair, direction, version, json, expires, state, revision, reservation, swap_id) =
        row;
    check_version(version, "maker offer")?;
    let offer: MakerOfferV1 = serde_json::from_str(&json)?;
    offer.validate()?;
    if offer.id.as_str() != offer_id
        || pair_name(offer.route().pair()) != pair
        || direction_name(offer.route().direction()) != direction
        || offer.expires_at_unix_seconds() != sql_to_u64(expires)?
    {
        return Err(StoreError::CorruptMakerOffer);
    }
    let reservation_id = reservation
        .map(RequestId::new)
        .transpose()
        .map_err(|_| StoreError::CorruptMakerOffer)?;
    let swap_id = swap_id
        .map(|value| SwapId::new(value.clone()).map(|_| value))
        .transpose()
        .map_err(|_| StoreError::CorruptMakerOffer)?;
    let (mut status, valid_shape) = match state.as_str() {
        "active" => (
            MakerOfferStatus::Active,
            reservation_id.is_none() && swap_id.is_none(),
        ),
        "reserved" => (
            MakerOfferStatus::Reserved,
            reservation_id.is_some() && swap_id.is_none(),
        ),
        "consumed" => (
            MakerOfferStatus::Consumed,
            reservation_id.is_some() && swap_id.is_some(),
        ),
        "withdrawn" => (
            MakerOfferStatus::Withdrawn,
            reservation_id.is_none() && swap_id.is_none(),
        ),
        _ => return Err(StoreError::CorruptMakerOffer),
    };
    if !valid_shape {
        return Err(StoreError::CorruptMakerOffer);
    }
    if status == MakerOfferStatus::Active && now >= offer.expires_at_unix_seconds() {
        status = MakerOfferStatus::Expired;
    }
    Ok(MakerOfferRecordV1 {
        revision: sql_to_u64(revision)?,
        status,
        offer,
        reservation_id,
        swap_id: swap_id.map(Into::into),
    })
}

fn check_version(version: i64, kind: &'static str) -> Result<(), StoreError> {
    if version != OFFER_PAYLOAD_VERSION {
        return Err(StoreError::UnsupportedPayloadVersion { kind, version });
    }
    Ok(())
}

fn sql_to_u64(value: i64) -> Result<u64, StoreError> {
    let value = u64::try_from(value).map_err(|_| StoreError::CorruptMakerOffer)?;
    if value == 0 {
        return Err(StoreError::CorruptMakerOffer);
    }
    Ok(value)
}

fn u64_to_sql(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| MakerOfferError::InvalidTime.into())
}

const fn pair_name(pair: Pair) -> &'static str {
    match pair {
        Pair::Bitcoin => "bitcoin",
        Pair::Monero => "monero",
        Pair::Zcash => "zcash",
    }
}

const fn direction_name(direction: SwapDirection) -> &'static str {
    match direction {
        SwapDirection::TakerSellsForeign => "taker_sells_foreign",
        SwapDirection::TakerSellsLez => "taker_sells_lez",
    }
}

pub(super) fn migrate(transaction: &rusqlite::Transaction<'_>) -> Result<(), StoreError> {
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS maker_offers (
             offer_id                    TEXT PRIMARY KEY NOT NULL,
             pair                        TEXT NOT NULL CHECK (pair IN ('bitcoin', 'monero', 'zcash')),
             direction                   TEXT NOT NULL CHECK (direction IN ('taker_sells_foreign', 'taker_sells_lez')),
             payload_version             INTEGER NOT NULL CHECK (payload_version = 1),
             payload_json                TEXT NOT NULL,
             expires_at_unix_seconds     INTEGER NOT NULL CHECK (expires_at_unix_seconds > 0),
             state                       TEXT NOT NULL CHECK (state IN ('active', 'reserved', 'consumed', 'withdrawn')),
             revision                    INTEGER NOT NULL CHECK (revision > 0),
             reservation_id              TEXT,
             swap_id                     TEXT,
             updated_request_id          TEXT NOT NULL,
             CHECK (pair != 'monero' OR direction = 'taker_sells_lez'),
             FOREIGN KEY (swap_id) REFERENCES swaps(id) ON DELETE RESTRICT,
             CHECK (
                 (state IN ('active', 'withdrawn') AND reservation_id IS NULL AND swap_id IS NULL)
                 OR (state = 'reserved' AND reservation_id IS NOT NULL AND swap_id IS NULL)
                 OR (state = 'consumed' AND reservation_id IS NOT NULL AND swap_id IS NOT NULL)
             )
         ) STRICT;
         CREATE INDEX IF NOT EXISTS maker_offers_discovery
             ON maker_offers (state, expires_at_unix_seconds, pair, direction, offer_id);
",
    )?;
    Ok(())
}
