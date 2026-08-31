//! Pair-neutral owner RPC request and operational health contracts.

use lez_swap_store::MakerRouteV1;
use serde::{Deserialize, Serialize};

/// Empty parameters for bounded owner-local list methods.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct ListRequest {}

/// Versioned read-only daemon health response.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MakerHealthV1 {
    schema_version: u16,
    ready: bool,
    degraded: bool,
    delivery: MakerDependencyStateV1,
    chat: MakerDependencyStateV1,
    #[serde(default)]
    routes: Vec<MakerRouteHealthV1>,
}

/// Read-only route-scoped chain dependency health.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MakerRouteHealthV1 {
    route: MakerRouteV1,
    state: MakerDependencyStateV1,
}

/// Read-only state of one optional maker application dependency.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MakerDependencyStateV1 {
    /// The optional dependency was not configured for this daemon.
    Disabled,
    /// The configured dependency is reachable and internally consistent.
    Available,
    /// The daemon is ready, but this configured dependency needs operator action.
    Unavailable,
}

impl MakerHealthV1 {
    pub(crate) fn ready(
        delivery: MakerDependencyStateV1,
        chat: MakerDependencyStateV1,
        routes: Vec<MakerRouteHealthV1>,
    ) -> Self {
        Self {
            schema_version: 1,
            ready: true,
            degraded: matches!(delivery, MakerDependencyStateV1::Unavailable)
                || matches!(chat, MakerDependencyStateV1::Unavailable)
                || routes
                    .iter()
                    .any(|route| route.state == MakerDependencyStateV1::Unavailable),
            delivery,
            chat,
            routes,
        }
    }

    /// Returns the health schema version.
    #[must_use]
    pub const fn schema_version(&self) -> u16 {
        self.schema_version
    }

    /// Returns true only for the daemon's ready state.
    #[must_use]
    pub const fn is_ready(&self) -> bool {
        self.schema_version == 1 && self.ready
    }

    /// Returns true when a configured application dependency is unavailable.
    #[must_use]
    pub const fn is_degraded(&self) -> bool {
        self.degraded
    }

    /// Returns the current Delivery projection state.
    #[must_use]
    pub const fn delivery(&self) -> MakerDependencyStateV1 {
        self.delivery
    }

    /// Returns the current Chat endpoint state.
    #[must_use]
    pub const fn chat(&self) -> MakerDependencyStateV1 {
        self.chat
    }

    /// Returns the current state for an exact configured route, or disabled when absent.
    #[must_use]
    pub fn route_state(&self, route: MakerRouteV1) -> MakerDependencyStateV1 {
        self.routes
            .iter()
            .find(|health| health.route == route)
            .map_or(MakerDependencyStateV1::Disabled, |health| health.state)
    }

    /// Returns every configured route's chain dependency state.
    #[must_use]
    pub fn routes(&self) -> &[MakerRouteHealthV1] {
        &self.routes
    }
}

impl MakerRouteHealthV1 {
    pub(crate) const fn new(route: MakerRouteV1, state: MakerDependencyStateV1) -> Self {
        Self { route, state }
    }

    /// Exact route represented by this health row.
    #[must_use]
    pub const fn route(self) -> MakerRouteV1 {
        self.route
    }

    /// Current dependency state for the route.
    #[must_use]
    pub const fn state(self) -> MakerDependencyStateV1 {
        self.state
    }
}
