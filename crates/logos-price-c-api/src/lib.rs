//! Versioned Logos-module price C-API protocol.
//!
//! This is the workspace's only foreign-function boundary. It is intended to
//! run in a short-lived helper process: native module aborts, segmentation
//! faults, and hangs therefore cannot corrupt the fund-owning maker daemon.

#[cfg(feature = "worker-host")]
use libloading::{Library, Symbol};
use serde::{Deserialize, Serialize};
#[cfg(feature = "worker-host")]
use std::{mem::size_of, path::Path};
use thiserror::Error;

/// The only ABI version understood by this adapter.
pub const ABI_VERSION_V1: u32 = 1;

/// The JSON schema emitted by the isolated worker.
pub const WORKER_SCHEMA_VERSION_V1: u32 = 1;

/// Maximum age accepted by the provisional ABI contract.
pub const MAX_QUOTE_AGE_SECONDS: u64 = 3_600;

/// Stable numeric asset-pair identifiers used by the C ABI.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[repr(u32)]
#[serde(rename_all = "snake_case")]
pub enum AbiPairV1 {
    /// LEZ/BTC.
    Bitcoin = 1,
    /// LEZ/XMR.
    Monero = 2,
    /// LEZ/ZEC.
    Zcash = 3,
}

impl TryFrom<u32> for AbiPairV1 {
    type Error = PriceCApiError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Bitcoin),
            2 => Ok(Self::Monero),
            3 => Ok(Self::Zcash),
            other => Err(PriceCApiError::UnsupportedPair(other)),
        }
    }
}

impl From<AbiPairV1> for u32 {
    fn from(value: AbiPairV1) -> Self {
        value as Self
    }
}

/// Stable numeric swap-direction identifiers used by the C ABI.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[repr(u32)]
#[serde(rename_all = "snake_case")]
pub enum AbiDirectionV1 {
    /// The taker pays LEZ and receives the foreign asset.
    TakerSellsLez = 1,
    /// The taker pays the foreign asset and receives LEZ.
    TakerSellsForeign = 2,
}

impl TryFrom<u32> for AbiDirectionV1 {
    type Error = PriceCApiError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::TakerSellsLez),
            2 => Ok(Self::TakerSellsForeign),
            other => Err(PriceCApiError::UnsupportedDirection(other)),
        }
    }
}

impl From<AbiDirectionV1> for u32 {
    fn from(value: AbiDirectionV1) -> Self {
        value as Self
    }
}

/// Typed result status emitted by the worker.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerPriceStatusV1 {
    /// A complete, validated quote is present.
    Ok,
    /// The module has no quote for the requested route.
    Missing,
    /// The module is temporarily unable to quote the route.
    Unavailable,
}

/// Bounded JSON response produced by the isolated worker.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WorkerQuoteV1 {
    schema_version: u32,
    status: WorkerPriceStatusV1,
    pair: AbiPairV1,
    direction: AbiDirectionV1,
    lez_units_per_lot: Option<u64>,
    foreign_units_per_lot: Option<u64>,
    source_revision: Option<u64>,
    as_of_unix_seconds: Option<u64>,
}

impl WorkerQuoteV1 {
    /// Worker output schema version.
    #[must_use]
    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }

    /// Typed quote status.
    #[must_use]
    pub const fn status(&self) -> WorkerPriceStatusV1 {
        self.status
    }

    /// Requested asset pair.
    #[must_use]
    pub const fn pair(&self) -> AbiPairV1 {
        self.pair
    }

    /// Requested swap direction.
    #[must_use]
    pub const fn direction(&self) -> AbiDirectionV1 {
        self.direction
    }

    /// Whether all exact quote fields are present.
    #[must_use]
    pub const fn has_quote(&self) -> bool {
        self.lez_units_per_lot.is_some()
            && self.foreign_units_per_lot.is_some()
            && self.source_revision.is_some()
            && self.as_of_unix_seconds.is_some()
    }

    /// Exact LEZ units in the quote ratio, or zero when no quote exists.
    #[must_use]
    pub fn lez_units_per_lot(&self) -> u64 {
        self.lez_units_per_lot.unwrap_or(0)
    }

    /// Exact foreign units in the quote ratio, or zero when absent.
    #[must_use]
    pub fn foreign_units_per_lot(&self) -> u64 {
        self.foreign_units_per_lot.unwrap_or(0)
    }

    /// Module-defined monotonic source revision, or zero when absent.
    #[must_use]
    pub fn source_revision(&self) -> u64 {
        self.source_revision.unwrap_or(0)
    }

    /// Source observation time, or zero when absent.
    #[must_use]
    pub fn as_of_unix_seconds(&self) -> u64 {
        self.as_of_unix_seconds.unwrap_or(0)
    }

    #[cfg(feature = "worker-host")]
    const fn without_quote(
        status: WorkerPriceStatusV1,
        pair: AbiPairV1,
        direction: AbiDirectionV1,
    ) -> Self {
        Self {
            schema_version: WORKER_SCHEMA_VERSION_V1,
            status,
            pair,
            direction,
            lez_units_per_lot: None,
            foreign_units_per_lot: None,
            source_revision: None,
            as_of_unix_seconds: None,
        }
    }
}

#[derive(Clone, Copy)]
#[repr(C)]
#[cfg(feature = "worker-host")]
struct PriceRequestV1 {
    struct_size: u32,
    abi_version: u32,
    pair: u32,
    direction: u32,
    now_unix_seconds: u64,
    reserved: [u64; 2],
}

#[derive(Clone, Copy, Default)]
#[repr(C)]
#[cfg(feature = "worker-host")]
struct PriceResponseV1 {
    struct_size: u32,
    abi_version: u32,
    pair: u32,
    direction: u32,
    lez_units_per_lot: u64,
    foreign_units_per_lot: u64,
    source_revision: u64,
    as_of_unix_seconds: u64,
    reserved: [u64; 2],
}

#[cfg(feature = "worker-host")]
type AbiVersionFn = unsafe extern "C" fn() -> u32;
#[cfg(feature = "worker-host")]
type QuoteFn = unsafe extern "C" fn(*const PriceRequestV1, *mut PriceResponseV1) -> i32;
/// Errors at the provisional native-module boundary.
#[derive(Debug, Error)]
pub enum PriceCApiError {
    /// The requested pair identifier is not defined by ABI v1.
    #[error("unsupported pair identifier {0}")]
    UnsupportedPair(u32),
    /// The requested direction identifier is not defined by ABI v1.
    #[error("unsupported direction identifier {0}")]
    UnsupportedDirection(u32),
    /// The module path was not an absolute regular-file path.
    #[error("module path must be an absolute regular file")]
    UnsafeModulePath,
    /// The module could not be loaded.
    #[error("native module load failed")]
    Load,
    /// A required v1 symbol was not exported.
    #[error("required ABI v1 symbol is absent")]
    MissingSymbol,
    /// The module advertises an unsupported ABI version.
    #[error("module ABI version {actual} is not supported")]
    WrongAbiVersion { actual: u32 },
    /// The module returned a status outside the v1 contract.
    #[error("module returned invalid status {0}")]
    InvalidStatus(i32),
    /// The module rejected a structurally valid request.
    #[error("module rejected the price request")]
    InvalidRequest,
    /// A successful response violated the v1 structural contract.
    #[error("module returned malformed quote: {0}")]
    MalformedQuote(&'static str),
}

/// Loads one module, performs one quote call, and validates its output.
///
/// # Errors
///
/// Fails closed for unsafe paths, ABI/symbol mismatches, invalid statuses,
/// malformed ratios, route substitution, revision errors, or stale/future data.
#[cfg(feature = "worker-host")]
pub fn query_module_once(
    module: &Path,
    pair: AbiPairV1,
    direction: AbiDirectionV1,
    now_unix_seconds: u64,
    max_age_seconds: u64,
) -> Result<WorkerQuoteV1, PriceCApiError> {
    if !module.is_absolute() || !module.is_file() {
        return Err(PriceCApiError::UnsafeModulePath);
    }
    if now_unix_seconds == 0 {
        return Err(PriceCApiError::MalformedQuote("request time is zero"));
    }
    if !(1..=MAX_QUOTE_AGE_SECONDS).contains(&max_age_seconds) {
        return Err(PriceCApiError::MalformedQuote(
            "maximum quote age is out of bounds",
        ));
    }

    // SAFETY: This call happens only in the disposable worker process. The
    // module path is an absolute regular file and no `Library` crosses a thread
    // or process boundary.
    let module = module.to_str().ok_or(PriceCApiError::UnsafeModulePath)?;
    // SAFETY: See the containment and path validation immediately above.
    let library = unsafe { Library::new(module) }.map_err(|_| PriceCApiError::Load)?;
    // SAFETY: Symbol bytes are NUL-terminated and the function type is the
    // fixed ABI v1 signature from the checked-in C header.
    let abi_version: Symbol<'_, AbiVersionFn> =
        unsafe { library.get(b"lez_logos_price_abi_version_v1\0") }
            .map_err(|_| PriceCApiError::MissingSymbol)?;
    // SAFETY: The symbol takes no arguments and returns a fixed-width integer.
    // A lying native module can only terminate this disposable worker process.
    let advertised_version = unsafe { abi_version() };
    if advertised_version != ABI_VERSION_V1 {
        return Err(PriceCApiError::WrongAbiVersion {
            actual: advertised_version,
        });
    }

    // SAFETY: This exact symbol and function type are defined by the checked-in
    // C ABI v1 header.
    let quote: Symbol<'_, QuoteFn> = unsafe { library.get(b"lez_logos_price_quote_v1\0") }
        .map_err(|_| PriceCApiError::MissingSymbol)?;
    let request_size = u32::try_from(size_of::<PriceRequestV1>())
        .map_err(|_| PriceCApiError::MalformedQuote("host request structure is too large"))?;
    let request = PriceRequestV1 {
        struct_size: request_size,
        abi_version: ABI_VERSION_V1,
        pair: pair.into(),
        direction: direction.into(),
        now_unix_seconds,
        reserved: [0; 2],
    };
    let mut response = PriceResponseV1::default();
    // SAFETY: Both pointers reference initialized, correctly aligned `repr(C)`
    // values for this synchronous call. Native code runs only in the worker.
    let status = unsafe { quote(&raw const request, &raw mut response) };
    match status {
        0 => validate_quote(response, pair, direction, now_unix_seconds, max_age_seconds),
        1 => Ok(WorkerQuoteV1::without_quote(
            WorkerPriceStatusV1::Missing,
            pair,
            direction,
        )),
        2 => Ok(WorkerQuoteV1::without_quote(
            WorkerPriceStatusV1::Unavailable,
            pair,
            direction,
        )),
        3 => Err(PriceCApiError::InvalidRequest),
        other => Err(PriceCApiError::InvalidStatus(other)),
    }
}

#[cfg(feature = "worker-host")]
fn validate_quote(
    response: PriceResponseV1,
    pair: AbiPairV1,
    direction: AbiDirectionV1,
    now_unix_seconds: u64,
    max_age_seconds: u64,
) -> Result<WorkerQuoteV1, PriceCApiError> {
    let response_size = u32::try_from(size_of::<PriceResponseV1>())
        .map_err(|_| PriceCApiError::MalformedQuote("host response structure is too large"))?;
    if response.struct_size != response_size {
        return Err(PriceCApiError::MalformedQuote("wrong response size"));
    }
    if response.abi_version != ABI_VERSION_V1 {
        return Err(PriceCApiError::WrongAbiVersion {
            actual: response.abi_version,
        });
    }
    if response.pair != u32::from(pair) || response.direction != u32::from(direction) {
        return Err(PriceCApiError::MalformedQuote(
            "route does not match request",
        ));
    }
    if response.reserved != [0; 2] {
        return Err(PriceCApiError::MalformedQuote(
            "reserved response fields are nonzero",
        ));
    }
    if response.lez_units_per_lot == 0 || response.foreign_units_per_lot == 0 {
        return Err(PriceCApiError::MalformedQuote("price ratio contains zero"));
    }
    if response.source_revision == 0 {
        return Err(PriceCApiError::MalformedQuote("source revision is zero"));
    }
    if response.as_of_unix_seconds == 0 {
        return Err(PriceCApiError::MalformedQuote("observation time is zero"));
    }
    let age = now_unix_seconds
        .checked_sub(response.as_of_unix_seconds)
        .ok_or(PriceCApiError::MalformedQuote(
            "observation time is in the future",
        ))?;
    if age > max_age_seconds {
        return Err(PriceCApiError::MalformedQuote("quote is stale"));
    }
    Ok(WorkerQuoteV1 {
        schema_version: WORKER_SCHEMA_VERSION_V1,
        status: WorkerPriceStatusV1::Ok,
        pair,
        direction,
        lez_units_per_lot: Some(response.lez_units_per_lot),
        foreign_units_per_lot: Some(response.foreign_units_per_lot),
        source_revision: Some(response.source_revision),
        as_of_unix_seconds: Some(response.as_of_unix_seconds),
    })
}
