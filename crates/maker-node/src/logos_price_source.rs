//! Bounded parent process for the crash-isolated Logos price worker.

use std::{
    io::Read as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use lez_logos_price_c_api::{
    AbiDirectionV1, AbiPairV1, MAX_QUOTE_AGE_SECONDS, WORKER_SCHEMA_VERSION_V1,
    WorkerPriceStatusV1, WorkerQuoteV1,
};
use lez_node_common::secure_file::is_secure_file;
use lez_swap_core::{Pair, SwapDirection};
use lez_swap_store::{LocalPriceV1, MakerRouteV1};
use wait_timeout::ChildExt as _;

use crate::{PriceQuoteV1, PriceSource, PriceSourceError};

const MAX_WORKER_OUTPUT_BYTES: usize = 4_096;
const MAX_WORKER_OUTPUT_BYTES_U64: u64 = 4_096;
const MAX_WORKER_TIMEOUT: Duration = Duration::from_secs(5);

/// Crash-isolated parent adapter for one exact Logos price module artifact.
#[derive(Clone, Debug)]
pub struct ProcessLogosPriceSource {
    worker: PathBuf,
    module: PathBuf,
    module_sha256: [u8; 32],
    timeout: Duration,
    max_age_seconds: u64,
}

impl ProcessLogosPriceSource {
    /// Validates immutable process configuration for the external source.
    ///
    /// # Errors
    ///
    /// Rejects non-absolute, linked, wrong-owner, writable or oversized files,
    /// a wrong module hash, and zero or excessive time bounds.
    pub fn new(
        worker: PathBuf,
        module: PathBuf,
        module_sha256: [u8; 32],
        timeout: Duration,
        max_age_seconds: u64,
    ) -> Result<Self, PriceSourceError> {
        if timeout.is_zero()
            || timeout > MAX_WORKER_TIMEOUT
            || !(1..=MAX_QUOTE_AGE_SECONDS).contains(&max_age_seconds)
        {
            return Err(PriceSourceError::InvalidSource);
        }
        validate_secure_file(&worker, true, None)?;
        validate_secure_file(&module, false, Some(module_sha256))?;
        Ok(Self {
            worker,
            module,
            module_sha256,
            timeout,
            max_age_seconds,
        })
    }

    /// Pinned module identity committed into every external-price offer.
    #[must_use]
    pub const fn source_identity_sha256(&self) -> [u8; 32] {
        self.module_sha256
    }

    /// Maximum accepted age of an external observation at publication.
    #[must_use]
    pub const fn max_age_seconds(&self) -> u64 {
        self.max_age_seconds
    }

    fn invoke(
        &self,
        route: MakerRouteV1,
        now_unix_seconds: u64,
    ) -> Result<WorkerQuoteV1, PriceSourceError> {
        validate_secure_file(&self.worker, true, None)?;
        validate_secure_file(&self.module, false, Some(self.module_sha256))?;
        let pair = abi_pair(route.pair());
        let direction = abi_direction(route.direction());
        let mut child = Command::new(&self.worker)
            .arg("--library")
            .arg(&self.module)
            .arg("--pair")
            .arg(u32::from(pair).to_string())
            .arg("--direction")
            .arg(u32::from(direction).to_string())
            .arg("--now-unix-seconds")
            .arg(now_unix_seconds.to_string())
            .arg("--max-age-seconds")
            .arg(self.max_age_seconds.to_string())
            .env_clear()
            .current_dir("/")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| PriceSourceError::InvalidSource)?;
        let Some(status) = child
            .wait_timeout(self.timeout)
            .map_err(|_| PriceSourceError::InvalidSource)?
        else {
            let _ = child.kill();
            child.wait().map_err(|_| PriceSourceError::InvalidSource)?;
            return Err(PriceSourceError::SourceTimeout);
        };
        if !status.success() {
            return Err(PriceSourceError::InvalidSource);
        }
        let mut output = Vec::new();
        child
            .stdout
            .take()
            .ok_or(PriceSourceError::InvalidSource)?
            .take(MAX_WORKER_OUTPUT_BYTES_U64 + 1)
            .read_to_end(&mut output)
            .map_err(|_| PriceSourceError::InvalidSource)?;
        if output.len() > MAX_WORKER_OUTPUT_BYTES {
            return Err(PriceSourceError::InvalidSource);
        }
        validate_secure_file(&self.module, false, Some(self.module_sha256))?;
        serde_json::from_slice(&output).map_err(|_| PriceSourceError::InvalidSource)
    }
}

impl PriceSource for ProcessLogosPriceSource {
    fn quote(
        &self,
        route: MakerRouteV1,
        now_unix_seconds: u64,
    ) -> Result<PriceQuoteV1, PriceSourceError> {
        let expected_pair = abi_pair(route.pair());
        let expected_direction = abi_direction(route.direction());
        let response = self.invoke(route, now_unix_seconds)?;
        if response.schema_version() != WORKER_SCHEMA_VERSION_V1
            || response.pair() != expected_pair
            || response.direction() != expected_direction
        {
            return Err(PriceSourceError::InvalidSource);
        }
        match response.status() {
            WorkerPriceStatusV1::Missing => Err(PriceSourceError::MissingQuote),
            WorkerPriceStatusV1::Unavailable => Err(PriceSourceError::UnavailableQuote),
            WorkerPriceStatusV1::Ok => {
                let observed_at = response.as_of_unix_seconds();
                if !response.has_quote()
                    || response.source_revision() == 0
                    || observed_at == 0
                    || observed_at > now_unix_seconds
                    || now_unix_seconds - observed_at > self.max_age_seconds
                {
                    return Err(PriceSourceError::InvalidSource);
                }
                let price = LocalPriceV1::new(
                    route,
                    response.lez_units_per_lot(),
                    response.foreign_units_per_lot(),
                )
                .map_err(|_| PriceSourceError::InvalidSource)?;
                Ok(PriceQuoteV1::from_external(
                    price,
                    response.source_revision(),
                    observed_at,
                ))
            }
        }
    }
}

pub(crate) fn validate_secure_file(
    path: &Path,
    executable: bool,
    expected_sha256: Option<[u8; 32]>,
) -> Result<(), PriceSourceError> {
    if is_secure_file(path, executable, expected_sha256) {
        Ok(())
    } else {
        Err(PriceSourceError::InvalidSource)
    }
}

const fn abi_pair(pair: Pair) -> AbiPairV1 {
    match pair {
        Pair::Bitcoin => AbiPairV1::Bitcoin,
        Pair::Monero => AbiPairV1::Monero,
        Pair::Zcash => AbiPairV1::Zcash,
    }
}

const fn abi_direction(direction: SwapDirection) -> AbiDirectionV1 {
    match direction {
        SwapDirection::TakerSellsLez => AbiDirectionV1::TakerSellsLez,
        SwapDirection::TakerSellsForeign => AbiDirectionV1::TakerSellsForeign,
    }
}
