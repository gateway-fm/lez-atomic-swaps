//! Durable maker application configuration and mutation audit.

use lez_bridge_protocol::RequestId;
use lez_swap_core::{Pair, SwapDirection};
use rusqlite::{OptionalExtension as _, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{SqliteSwapStore, StoreError};

const APPLICATION_PAYLOAD_VERSION: i64 = 1;
const MAXIMUM_OFFER_TTL_SECONDS: u64 = 86_400;

/// Invalid maker application configuration.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum MakerConfigurationError {
    /// The selected protocol construction does not support this direction.
    #[error("the selected pair does not support this swap direction")]
    UnsupportedDirection,
    /// Offer amount bounds were empty or reversed.
    #[error("offer amount bounds must satisfy 0 < minimum <= maximum")]
    InvalidAmountBounds,
    /// Offer validity was zero or exceeded the bounded one-day publication window.
    #[error("offer TTL must be 1..={MAXIMUM_OFFER_TTL_SECONDS} seconds")]
    InvalidOfferTtl,
    /// A fixed price ratio was zero, oversized, or not in canonical reduced form.
    #[error("local price lots must be nonzero, fit signed SQLite integers, and be reduced")]
    InvalidLocalPrice,
}

/// Configured source for one maker route's price.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MakerPriceSourceKind {
    /// Exact integer ratio stored in the owner database.
    Local,
    /// Quote supplied by the bounded Logos module C-API adapter.
    LogosCApi,
}

/// Pair plus direction used as one maker route key.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MakerRouteV1 {
    pair: Pair,
    direction: SwapDirection,
}

impl MakerRouteV1 {
    /// Constructs a supported route.
    ///
    /// # Errors
    ///
    /// Rejects the unsupported foreign-first Monero construction.
    pub fn new(pair: Pair, direction: SwapDirection) -> Result<Self, MakerConfigurationError> {
        let route = Self { pair, direction };
        route.validate()?;
        Ok(route)
    }

    /// Foreign-chain pair.
    #[must_use]
    pub const fn pair(self) -> Pair {
        self.pair
    }

    /// Taker-funded-first direction.
    #[must_use]
    pub const fn direction(self) -> SwapDirection {
        self.direction
    }

    fn validate(self) -> Result<(), MakerConfigurationError> {
        if self.pair == Pair::Monero && self.direction == SwapDirection::TakerSellsForeign {
            return Err(MakerConfigurationError::UnsupportedDirection);
        }
        Ok(())
    }
}

/// Durable pair policy used to derive bounded expiring offers.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MakerPairConfigurationV1 {
    route: MakerRouteV1,
    enabled: bool,
    price_source: MakerPriceSourceKind,
    minimum_foreign_units: u64,
    maximum_foreign_units: u64,
    offer_ttl_seconds: u64,
}

impl MakerPairConfigurationV1 {
    /// Constructs a bounded pair policy.
    ///
    /// # Errors
    ///
    /// Rejects unsupported routes, zero/reversed amount bounds, and an unbounded TTL.
    pub fn new(
        route: MakerRouteV1,
        enabled: bool,
        price_source: MakerPriceSourceKind,
        minimum_foreign_units: u64,
        maximum_foreign_units: u64,
        offer_ttl_seconds: u64,
    ) -> Result<Self, MakerConfigurationError> {
        let configuration = Self {
            route,
            enabled,
            price_source,
            minimum_foreign_units,
            maximum_foreign_units,
            offer_ttl_seconds,
        };
        configuration.validate()?;
        Ok(configuration)
    }

    /// Configured route.
    #[must_use]
    pub const fn route(&self) -> MakerRouteV1 {
        self.route
    }

    /// Whether new offers may be derived for this route.
    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    /// Price adapter selected for newly derived offers.
    #[must_use]
    pub const fn price_source(&self) -> MakerPriceSourceKind {
        self.price_source
    }

    /// Smallest accepted foreign-chain atomic-unit amount.
    #[must_use]
    pub const fn minimum_foreign_units(&self) -> u64 {
        self.minimum_foreign_units
    }

    /// Largest accepted foreign-chain atomic-unit amount.
    #[must_use]
    pub const fn maximum_foreign_units(&self) -> u64 {
        self.maximum_foreign_units
    }

    /// Validity assigned to one derived offer.
    #[must_use]
    pub const fn offer_ttl_seconds(&self) -> u64 {
        self.offer_ttl_seconds
    }

    fn validate(&self) -> Result<(), MakerConfigurationError> {
        self.route.validate()?;
        if self.minimum_foreign_units == 0
            || self.maximum_foreign_units < self.minimum_foreign_units
        {
            return Err(MakerConfigurationError::InvalidAmountBounds);
        }
        if !(1..=MAXIMUM_OFFER_TTL_SECONDS).contains(&self.offer_ttl_seconds) {
            return Err(MakerConfigurationError::InvalidOfferTtl);
        }
        Ok(())
    }
}

/// Exact integer lot ratio for a locally configured price.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LocalPriceV1 {
    route: MakerRouteV1,
    lez_units_per_lot: u64,
    foreign_units_per_lot: u64,
}

impl LocalPriceV1 {
    /// Constructs one exact rational quote without floating-point rounding.
    ///
    /// # Errors
    ///
    /// Rejects unsupported routes and zero-sized lots.
    pub fn new(
        route: MakerRouteV1,
        lez_units_per_lot: u64,
        foreign_units_per_lot: u64,
    ) -> Result<Self, MakerConfigurationError> {
        let price = Self {
            route,
            lez_units_per_lot,
            foreign_units_per_lot,
        };
        price.validate()?;
        Ok(price)
    }

    /// Route priced by this quote.
    #[must_use]
    pub const fn route(&self) -> MakerRouteV1 {
        self.route
    }

    /// LEZ atomic units in the exact price lot.
    #[must_use]
    pub const fn lez_units_per_lot(&self) -> u64 {
        self.lez_units_per_lot
    }

    /// Foreign-chain atomic units in the exact price lot.
    #[must_use]
    pub const fn foreign_units_per_lot(&self) -> u64 {
        self.foreign_units_per_lot
    }

    fn validate(&self) -> Result<(), MakerConfigurationError> {
        self.route.validate()?;
        if self.lez_units_per_lot == 0
            || self.foreign_units_per_lot == 0
            || self.lez_units_per_lot > i64::MAX as u64
            || self.foreign_units_per_lot > i64::MAX as u64
            || greatest_common_divisor(self.lez_units_per_lot, self.foreign_units_per_lot) != 1
        {
            return Err(MakerConfigurationError::InvalidLocalPrice);
        }
        Ok(())
    }
}

/// One versioned durable maker record.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct VersionedMakerRecord<T> {
    revision: u64,
    value: T,
}

impl<T> VersionedMakerRecord<T> {
    /// Monotonic revision local to this route and record family.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Validated semantic value.
    #[must_use]
    pub const fn value(&self) -> &T {
        &self.value
    }
}

/// Result of one atomic configuration mutation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MakerConfigurationCommit {
    revision: u64,
    was_replay: bool,
}

impl MakerConfigurationCommit {
    /// Durable route-local revision.
    #[must_use]
    pub const fn revision(self) -> u64 {
        self.revision
    }

    /// Whether the exact request ID and payload were already committed.
    #[must_use]
    pub const fn was_replay(self) -> bool {
        self.was_replay
    }
}

#[derive(Deserialize, Serialize)]
struct StoredCommitV1 {
    schema_version: u16,
    revision: u64,
}

#[derive(Serialize)]
struct PairMutationRequest<'a> {
    expected_revision: Option<u64>,
    configuration: &'a MakerPairConfigurationV1,
}

#[derive(Serialize)]
struct PriceMutationRequest<'a> {
    expected_revision: Option<u64>,
    price: &'a LocalPriceV1,
}

impl SqliteSwapStore {
    /// Atomically configures one maker route and records its idempotency result.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid policy, conflicting request-ID reuse, corrupt
    /// prior state, revision overflow, or a `SQLite` failure.
    pub fn configure_maker_pair(
        &mut self,
        request_id: &RequestId,
        expected_revision: Option<u64>,
        configuration: &MakerPairConfigurationV1,
    ) -> Result<MakerConfigurationCommit, StoreError> {
        configuration.validate()?;
        let payload_json = serde_json::to_string(configuration)?;
        let request_json = serde_json::to_string(&PairMutationRequest {
            expected_revision,
            configuration,
        })?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(commit) = replay_configuration_mutation(
            &transaction,
            request_id,
            "pair_configure",
            &request_json,
        )? {
            transaction.commit()?;
            return Ok(commit);
        }
        let route = configuration.route();
        let revision = next_revision(
            &transaction,
            "maker_pair_configurations",
            route,
            expected_revision,
        )?;
        if configuration.enabled()
            && configuration.price_source() == MakerPriceSourceKind::Local
            && !route_exists(&transaction, "maker_local_prices", route)?
        {
            return Err(StoreError::MissingMakerLocalPrice);
        }
        transaction.execute(
            "INSERT INTO maker_pair_configurations (
                 pair, direction, payload_version, payload_json, revision, updated_request_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(pair, direction) DO UPDATE SET
                 payload_version = excluded.payload_version,
                 payload_json = excluded.payload_json,
                 revision = excluded.revision,
                 updated_request_id = excluded.updated_request_id",
            params![
                pair_name(route.pair()),
                direction_name(route.direction()),
                APPLICATION_PAYLOAD_VERSION,
                payload_json,
                revision_to_sql(revision)?,
                request_id.as_str(),
            ],
        )?;
        persist_configuration_mutation(
            &transaction,
            request_id,
            "pair_configure",
            &request_json,
            revision,
        )?;
        transaction.commit()?;
        Ok(MakerConfigurationCommit {
            revision,
            was_replay: false,
        })
    }

    /// Atomically sets one exact local price and records its idempotency result.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid price, conflicting request-ID reuse, corrupt
    /// prior state, revision overflow, or a `SQLite` failure.
    pub fn set_local_price(
        &mut self,
        request_id: &RequestId,
        expected_revision: Option<u64>,
        price: &LocalPriceV1,
    ) -> Result<MakerConfigurationCommit, StoreError> {
        price.validate()?;
        let payload_json = serde_json::to_string(price)?;
        let request_json = serde_json::to_string(&PriceMutationRequest {
            expected_revision,
            price,
        })?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(commit) = replay_configuration_mutation(
            &transaction,
            request_id,
            "local_price_set",
            &request_json,
        )? {
            transaction.commit()?;
            return Ok(commit);
        }
        let route = price.route();
        let pair = load_pair_record(&transaction, route)?.ok_or(StoreError::MissingMakerPair)?;
        if pair.value().price_source() != MakerPriceSourceKind::Local {
            return Err(StoreError::MakerPriceSourceMismatch);
        }
        let revision = next_revision(&transaction, "maker_local_prices", route, expected_revision)?;
        transaction.execute(
            "INSERT INTO maker_local_prices (
                 pair, direction, payload_version, payload_json, revision, updated_request_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(pair, direction) DO UPDATE SET
                 payload_version = excluded.payload_version,
                 payload_json = excluded.payload_json,
                 revision = excluded.revision,
                 updated_request_id = excluded.updated_request_id",
            params![
                pair_name(route.pair()),
                direction_name(route.direction()),
                APPLICATION_PAYLOAD_VERSION,
                payload_json,
                revision_to_sql(revision)?,
                request_id.as_str(),
            ],
        )?;
        persist_configuration_mutation(
            &transaction,
            request_id,
            "local_price_set",
            &request_json,
            revision,
        )?;
        transaction.commit()?;
        Ok(MakerConfigurationCommit {
            revision,
            was_replay: false,
        })
    }

    /// Lists all durable maker pair policies in stable route order.
    ///
    /// # Errors
    ///
    /// Returns an error for unsupported/corrupt records or a `SQLite` failure.
    pub fn list_maker_pairs(
        &self,
    ) -> Result<Vec<VersionedMakerRecord<MakerPairConfigurationV1>>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT pair, direction, payload_version, payload_json, revision
             FROM maker_pair_configurations ORDER BY pair, direction",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?;
        rows.map(|row| decode_pair_record(row?)).collect()
    }

    /// Lists all durable local prices in stable route order.
    ///
    /// # Errors
    ///
    /// Returns an error for unsupported/corrupt records or a `SQLite` failure.
    pub fn list_local_prices(&self) -> Result<Vec<VersionedMakerRecord<LocalPriceV1>>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT pair, direction, payload_version, payload_json, revision
             FROM maker_local_prices ORDER BY pair, direction",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?;
        rows.map(|row| decode_price_record(row?)).collect()
    }
}

fn replay_configuration_mutation(
    transaction: &rusqlite::Transaction<'_>,
    request_id: &RequestId,
    operation: &str,
    request_json: &str,
) -> Result<Option<MakerConfigurationCommit>, StoreError> {
    let existing = transaction
        .query_row(
            "SELECT operation, request_json, result_json
             FROM maker_application_mutations WHERE request_id = ?1",
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
    let Some((stored_operation, stored_request, stored_result)) = existing else {
        return Ok(None);
    };
    if stored_operation != operation || stored_request != request_json {
        return Err(StoreError::MakerConfigurationRequestConflict);
    }
    let result: StoredCommitV1 = serde_json::from_str(&stored_result)?;
    if result.schema_version != 1 || result.revision == 0 {
        return Err(StoreError::CorruptMakerConfiguration);
    }
    Ok(Some(MakerConfigurationCommit {
        revision: result.revision,
        was_replay: true,
    }))
}

fn persist_configuration_mutation(
    transaction: &rusqlite::Transaction<'_>,
    request_id: &RequestId,
    operation: &str,
    request_json: &str,
    revision: u64,
) -> Result<(), StoreError> {
    let result_json = serde_json::to_string(&StoredCommitV1 {
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
            APPLICATION_PAYLOAD_VERSION,
            request_json,
            result_json,
        ],
    )?;
    Ok(())
}

fn next_revision(
    transaction: &rusqlite::Transaction<'_>,
    table: &str,
    route: MakerRouteV1,
    expected_revision: Option<u64>,
) -> Result<u64, StoreError> {
    let sql = format!("SELECT revision FROM {table} WHERE pair = ?1 AND direction = ?2");
    let current = transaction
        .query_row(
            &sql,
            params![pair_name(route.pair()), direction_name(route.direction())],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .map(revision_from_sql)
        .transpose()?;
    if current != expected_revision {
        return Err(StoreError::StaleMakerConfiguration {
            expected: expected_revision,
            actual: current,
        });
    }
    current
        .unwrap_or(0)
        .checked_add(1)
        .ok_or(StoreError::RevisionOverflow)
}

fn route_exists(
    transaction: &rusqlite::Transaction<'_>,
    table: &str,
    route: MakerRouteV1,
) -> Result<bool, StoreError> {
    let sql = format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE pair = ?1 AND direction = ?2)");
    transaction
        .query_row(
            &sql,
            params![pair_name(route.pair()), direction_name(route.direction())],
            |row| row.get(0),
        )
        .map_err(StoreError::from)
}

fn load_pair_record(
    transaction: &rusqlite::Transaction<'_>,
    route: MakerRouteV1,
) -> Result<Option<VersionedMakerRecord<MakerPairConfigurationV1>>, StoreError> {
    transaction
        .query_row(
            "SELECT pair, direction, payload_version, payload_json, revision
             FROM maker_pair_configurations WHERE pair = ?1 AND direction = ?2",
            params![pair_name(route.pair()), direction_name(route.direction())],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()?
        .map(decode_pair_record)
        .transpose()
}

fn decode_pair_record(
    (pair, direction, version, json, revision): (String, String, i64, String, i64),
) -> Result<VersionedMakerRecord<MakerPairConfigurationV1>, StoreError> {
    check_payload_version(version, "maker pair configuration")?;
    let value: MakerPairConfigurationV1 = serde_json::from_str(&json)?;
    value.validate()?;
    validate_route_columns(value.route(), &pair, &direction)?;
    Ok(VersionedMakerRecord {
        revision: revision_from_sql(revision)?,
        value,
    })
}

fn decode_price_record(
    (pair, direction, version, json, revision): (String, String, i64, String, i64),
) -> Result<VersionedMakerRecord<LocalPriceV1>, StoreError> {
    check_payload_version(version, "maker local price")?;
    let value: LocalPriceV1 = serde_json::from_str(&json)?;
    value.validate()?;
    validate_route_columns(value.route(), &pair, &direction)?;
    Ok(VersionedMakerRecord {
        revision: revision_from_sql(revision)?,
        value,
    })
}

fn check_payload_version(version: i64, kind: &'static str) -> Result<(), StoreError> {
    if version != APPLICATION_PAYLOAD_VERSION {
        return Err(StoreError::UnsupportedPayloadVersion { kind, version });
    }
    Ok(())
}

fn validate_route_columns(
    route: MakerRouteV1,
    pair: &str,
    direction: &str,
) -> Result<(), StoreError> {
    if pair_name(route.pair()) != pair || direction_name(route.direction()) != direction {
        return Err(StoreError::CorruptMakerConfiguration);
    }
    Ok(())
}

fn revision_from_sql(value: i64) -> Result<u64, StoreError> {
    let revision = u64::try_from(value).map_err(|_| StoreError::RevisionOverflow)?;
    if revision == 0 {
        return Err(StoreError::CorruptMakerConfiguration);
    }
    Ok(revision)
}

fn revision_to_sql(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::RevisionOverflow)
}

const fn greatest_common_divisor(mut left: u64, mut right: u64) -> u64 {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
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
        "CREATE TABLE IF NOT EXISTS maker_pair_configurations (
             pair               TEXT NOT NULL CHECK (pair IN ('bitcoin', 'monero', 'zcash')),
             direction          TEXT NOT NULL CHECK (
                 direction IN ('taker_sells_foreign', 'taker_sells_lez')
             ),
             payload_version    INTEGER NOT NULL CHECK (payload_version > 0),
             payload_json       TEXT NOT NULL,
             revision           INTEGER NOT NULL CHECK (revision > 0),
             updated_request_id TEXT NOT NULL,
             PRIMARY KEY (pair, direction),
             CHECK (pair != 'monero' OR direction = 'taker_sells_lez')
         ) STRICT;
         CREATE TABLE IF NOT EXISTS maker_local_prices (
             pair               TEXT NOT NULL CHECK (pair IN ('bitcoin', 'monero', 'zcash')),
             direction          TEXT NOT NULL CHECK (
                 direction IN ('taker_sells_foreign', 'taker_sells_lez')
             ),
             payload_version    INTEGER NOT NULL CHECK (payload_version > 0),
             payload_json       TEXT NOT NULL,
             revision           INTEGER NOT NULL CHECK (revision > 0),
             updated_request_id TEXT NOT NULL,
             PRIMARY KEY (pair, direction),
             FOREIGN KEY (pair, direction)
                 REFERENCES maker_pair_configurations(pair, direction) ON DELETE RESTRICT,
             CHECK (pair != 'monero' OR direction = 'taker_sells_lez')
         ) STRICT;
         CREATE TABLE IF NOT EXISTS maker_application_mutations (
             sequence                INTEGER PRIMARY KEY AUTOINCREMENT,
             request_id              TEXT NOT NULL UNIQUE,
             operation               TEXT NOT NULL CHECK (
                 operation IN ('pair_configure', 'local_price_set', 'offer_publish', 'offer_reserve', 'offer_consume', 'offer_withdraw')
             ),
             request_payload_version INTEGER NOT NULL CHECK (request_payload_version = 1),
             request_json            TEXT NOT NULL,
             result_json             TEXT NOT NULL
         ) STRICT;",
    )?;
    let legacy_exists: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'maker_configuration_mutations')",
        [],
        |row| row.get(0),
    )?;
    if legacy_exists {
        transaction.execute_batch(
            "INSERT INTO maker_application_mutations (
                 sequence, request_id, operation, request_payload_version, request_json, result_json
             ) SELECT sequence, request_id, operation, request_payload_version, request_json, result_json
               FROM maker_configuration_mutations ORDER BY sequence;
             DROP TABLE maker_configuration_mutations;",
        )?;
    }
    Ok(())
}
