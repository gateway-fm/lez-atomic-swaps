//! Pluggable maker price-source boundary.

use lez_swap_store::{
    LocalPriceV1, MakerRouteV1, SqliteSwapStore, StoreError, VersionedMakerRecord,
};
use serde::{Deserialize, Serialize};

/// One exact price observed from a named source.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PriceQuoteV1 {
    price: LocalPriceV1,
    source_revision: u64,
    observed_at_unix_seconds: u64,
}

impl PriceQuoteV1 {
    /// Exact reduced integer price.
    #[must_use]
    pub const fn price(&self) -> &LocalPriceV1 {
        &self.price
    }

    /// Revision of the source record used for this quote.
    #[must_use]
    pub const fn source_revision(&self) -> u64 {
        self.source_revision
    }

    /// Trusted daemon time at which the source was read.
    #[must_use]
    pub const fn observed_at_unix_seconds(&self) -> u64 {
        self.observed_at_unix_seconds
    }
}

/// Structured price-source failure.
#[derive(Debug, thiserror::Error)]
pub enum PriceSourceError {
    /// Durable local configuration could not be read or revalidated.
    #[error("local price store is unavailable or corrupt")]
    Store(#[from] StoreError),
    /// The source has no quote for the requested route.
    #[error("price source has no quote for the requested route")]
    MissingQuote,
    /// A supposedly route-unique source returned more than one quote.
    #[error("price source returned duplicate route quotes")]
    DuplicateQuote,
}

/// Synchronous quote boundary used inside the daemon's persistence actor.
///
/// External adapters must copy and validate C-owned values before returning a
/// [`PriceQuoteV1`]. They receive no signing keys or fund-moving authority.
pub trait PriceSource {
    /// Returns one exact route quote at trusted daemon time.
    ///
    /// # Errors
    ///
    /// Returns a structured unavailable, missing, stale, or invalid-source error.
    fn quote(
        &self,
        route: MakerRouteV1,
        now_unix_seconds: u64,
    ) -> Result<PriceQuoteV1, PriceSourceError>;
}

/// Local price source backed by the daemon's authoritative `SQLite` owner.
#[derive(Debug)]
pub struct LocalPriceSource<'a> {
    store: &'a SqliteSwapStore,
}

impl<'a> LocalPriceSource<'a> {
    /// Borrows the already-locked maker store for one bounded quote operation.
    #[must_use]
    pub const fn new(store: &'a SqliteSwapStore) -> Self {
        Self { store }
    }
}

impl PriceSource for LocalPriceSource<'_> {
    fn quote(
        &self,
        route: MakerRouteV1,
        now_unix_seconds: u64,
    ) -> Result<PriceQuoteV1, PriceSourceError> {
        let mut matches = self
            .store
            .list_local_prices()?
            .into_iter()
            .filter(|record| record.value().route() == route);
        let record = matches.next().ok_or(PriceSourceError::MissingQuote)?;
        if matches.next().is_some() {
            return Err(PriceSourceError::DuplicateQuote);
        }
        Ok(quote_from_record(&record, now_unix_seconds))
    }
}

fn quote_from_record(
    record: &VersionedMakerRecord<LocalPriceV1>,
    observed_at_unix_seconds: u64,
) -> PriceQuoteV1 {
    PriceQuoteV1 {
        price: record.value().clone(),
        source_revision: record.revision(),
        observed_at_unix_seconds,
    }
}

#[cfg(test)]
mod tests {
    use lez_bridge_protocol::RequestId;
    use lez_swap_core::{Pair, SwapDirection};
    use lez_swap_store::{MakerPairConfigurationV1, MakerPriceSourceKind};
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn local_source_returns_exact_current_revision_and_trusted_time() {
        let run = tempdir().expect("isolated price source");
        let mut store = SqliteSwapStore::open(run.path().join("price.sqlite3")).unwrap();
        let route = MakerRouteV1::new(Pair::Zcash, SwapDirection::TakerSellsLez).unwrap();
        let policy =
            MakerPairConfigurationV1::new(route, false, MakerPriceSourceKind::Local, 1, 1_000, 300)
                .unwrap();
        store
            .configure_maker_pair(
                &RequestId::new("price-source-pair-001").unwrap(),
                None,
                &policy,
            )
            .unwrap();
        let price = LocalPriceV1::new(route, 5, 2).unwrap();
        store
            .set_local_price(
                &RequestId::new("price-source-quote-001").unwrap(),
                None,
                &price,
            )
            .unwrap();

        let quote = LocalPriceSource::new(&store)
            .quote(route, 1_700_000_000)
            .unwrap();
        assert_eq!(quote.price(), &price);
        assert_eq!(quote.source_revision(), 1);
        assert_eq!(quote.observed_at_unix_seconds(), 1_700_000_000);
    }

    #[test]
    fn local_source_reports_an_unconfigured_route_without_substitution() {
        let run = tempdir().expect("isolated price source");
        let store = SqliteSwapStore::open(run.path().join("empty.sqlite3")).unwrap();
        let route = MakerRouteV1::new(Pair::Bitcoin, SwapDirection::TakerSellsForeign).unwrap();
        assert!(matches!(
            LocalPriceSource::new(&store).quote(route, 1_700_000_000),
            Err(PriceSourceError::MissingQuote)
        ));
    }
}
