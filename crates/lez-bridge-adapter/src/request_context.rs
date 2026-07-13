//! Actor-owned one-use bridge request contexts.

use std::{error::Error, fmt};

use lez_bridge_protocol::{DiscoveryWindow, RequestId};
use lez_swap_store::{BridgeOperationKey, BridgeOperationKind, BridgeRequestSpec};

use crate::BridgeRequestContextSource;

/// Supplies a bounded scan window from chain or orchestration authority.
///
/// The complete operation key is provided so an implementation can derive a
/// role-, swap-, run-, and operation-specific canonical range. This capability
/// is never invoked for operations whose wire contract forbids a scan window.
pub trait BridgeDiscoveryWindowSource: Send + Sync {
    /// Structured authority failure, redacted at the actor context boundary.
    type Error: Error + Send + Sync + 'static;

    /// Returns the authoritative bounded window for one window-bearing operation.
    ///
    /// # Errors
    ///
    /// Returns an authority-specific error when a safe range is unavailable.
    fn discovery_window(&self, key: &BridgeOperationKey) -> Result<DiscoveryWindow, Self::Error>;
}

/// Generates one OS-random request ID and attaches an authoritative scan window.
///
/// This source retains no generated identifier and performs no collision check.
/// The caller must immediately offer each returned specification to the durable
/// bridge operation journal, which remains the reuse and collision authority.
#[derive(Clone)]
pub struct ActorBridgeRequestContextSource<Windows> {
    windows: Windows,
}

impl<Windows> ActorBridgeRequestContextSource<Windows> {
    /// Binds request-ID generation to one actor's discovery-window authority.
    #[must_use]
    pub const fn new(windows: Windows) -> Self {
        Self { windows }
    }
}

impl<Windows> fmt::Debug for ActorBridgeRequestContextSource<Windows> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ActorBridgeRequestContextSource")
            .field("windows", &"[REDACTED]")
            .finish()
    }
}

/// Redacted failure category for actor-owned request context generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActorBridgeRequestContextError {
    /// The operating system could not supply cryptographic randomness.
    RandomnessUnavailable,
    /// The fixed random request-ID encoding unexpectedly failed validation.
    InvalidRequestId,
    /// Chain or orchestration authority could not supply a safe scan window.
    DiscoveryWindowUnavailable,
}

impl fmt::Display for ActorBridgeRequestContextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::RandomnessUnavailable => "OS randomness is unavailable for LEZ bridge request",
            Self::InvalidRequestId => "generated LEZ bridge request identity is invalid",
            Self::DiscoveryWindowUnavailable => {
                "authoritative LEZ bridge discovery window is unavailable"
            }
        })
    }
}

impl Error for ActorBridgeRequestContextError {}

impl<Windows> BridgeRequestContextSource for ActorBridgeRequestContextSource<Windows>
where
    Windows: BridgeDiscoveryWindowSource,
{
    type Error = ActorBridgeRequestContextError;

    fn next_request(&self, key: &BridgeOperationKey) -> Result<BridgeRequestSpec, Self::Error> {
        let mut random = [0_u8; 16];
        getrandom::fill(&mut random).map_err(|_| Self::Error::RandomnessUnavailable)?;
        let request_id = RequestId::new(format!("req-{:032x}", u128::from_be_bytes(random)))
            .map_err(|_| Self::Error::InvalidRequestId)?;
        let discovery_window = if window_bearing(key.operation()) {
            Some(
                self.windows
                    .discovery_window(key)
                    .map_err(|_| Self::Error::DiscoveryWindowUnavailable)?,
            )
        } else {
            None
        };
        Ok(BridgeRequestSpec::new(request_id, discovery_window))
    }
}

const fn window_bearing(operation: BridgeOperationKind) -> bool {
    matches!(
        operation,
        BridgeOperationKind::NativeEscrowDiscoveryObserve
            | BridgeOperationKind::RevealingClaimDiscoveryObserve
            // Exact native-refund observation still performs the bounded scan
            // required by the existing bridge protocol and durable journal.
            | BridgeOperationKind::NativeRefundExactObserve
            | BridgeOperationKind::NativeRefundDiscoveryObserve
    )
}
