//! Pair-neutral owner RPC request and operational health contracts.

use lez_btc_role_lifecycle::LezChainProgressV1;
use lez_swap_store::MakerRouteV1;
use serde::{Deserialize, Serialize};

/// Empty parameters for bounded owner-local list methods.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct ListRequest {}

/// Parameters of `maker_offer_list`. A Node keeps every offer it ever
/// published, so a caller that renders only some of them names the states it
/// wants; without `states` the whole history is returned, as before.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct MakerOfferListRequest {
    #[serde(default)]
    pub states: Option<Vec<lez_swap_store::MakerOfferStatus>>,
}

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
    /// The LEZ chain's progress as this Node reads it (#112): the last
    /// finalized block id, when it advanced, and a named reason once it
    /// stalls, which also sets `degraded`. Absent on a Node without the
    /// Node-owned lifecycle (it has no indexer to read) and on Nodes that
    /// predate it; additive, so an older client ignores it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lez_chain: Option<LezChainProgressV1>,
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
            lez_chain: None,
        }
    }

    /// Adds the LEZ chain's progress; a degraded chain degrades the Node.
    #[must_use]
    pub(crate) fn with_lez_chain(mut self, lez_chain: Option<LezChainProgressV1>) -> Self {
        self.degraded |= lez_chain.as_ref().is_some_and(|chain| chain.degraded);
        self.lez_chain = lez_chain;
        self
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

    /// Returns the LEZ chain's progress, when this Node reads the chain.
    #[must_use]
    pub const fn lez_chain(&self) -> Option<&LezChainProgressV1> {
        self.lez_chain.as_ref()
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

#[cfg(test)]
mod tests {
    use lez_btc_role_lifecycle::LezFinalityRule;

    use super::*;

    fn stalled_chain() -> LezChainProgressV1 {
        let mut rule = LezFinalityRule::new(60);
        rule.observe(1_000, Some(84), Some(100));
        rule.observe(1_100, Some(84), Some(160))
    }

    #[test]
    fn a_stalled_lez_finality_degrades_a_node_whose_dependencies_are_fine() {
        let available = MakerDependencyStateV1::Available;
        let healthy = MakerHealthV1::ready(available, available, Vec::new());
        assert!(!healthy.is_degraded());
        let stalled = healthy.clone().with_lez_chain(Some(stalled_chain()));
        assert!(stalled.is_ready());
        assert!(stalled.is_degraded());
        let value = serde_json::to_value(&stalled).unwrap();
        assert_eq!(value["degraded"], true);
        assert_eq!(value["lez_chain"]["reason"], "lez_finality_stalled");
        assert_eq!(value["lez_chain"]["finalized_block_id"], 84);
        assert_eq!(value["lez_chain"]["finalized_flat_seconds"], 100);
        // A moving chain leaves `degraded` to the other dependencies.
        let mut rule = LezFinalityRule::new(60);
        let moving = rule.observe(1_000, Some(84), Some(100));
        let unavailable =
            MakerHealthV1::ready(MakerDependencyStateV1::Unavailable, available, Vec::new());
        assert!(
            unavailable
                .with_lez_chain(Some(moving.clone()))
                .is_degraded()
        );
        assert!(!healthy.with_lez_chain(Some(moving)).is_degraded());
    }

    #[test]
    fn health_without_the_chain_omits_it_and_old_replies_still_parse() {
        #[derive(Deserialize)]
        struct OldClientHealth {
            ready: bool,
            degraded: bool,
        }
        let available = MakerDependencyStateV1::Available;
        let health = MakerHealthV1::ready(available, available, Vec::new()).with_lez_chain(None);
        let value = serde_json::to_value(&health).unwrap();
        assert!(value.get("lez_chain").is_none(), "{value}");
        // A reply from a Node that predates `lez_chain` (and `routes`).
        let old: MakerHealthV1 = serde_json::from_value(serde_json::json!({
            "schema_version": 1, "ready": true, "degraded": false,
            "delivery": "available", "chat": "disabled"
        }))
        .unwrap();
        assert!(old.lez_chain().is_none());
        // A client that knows only the old fields reads a new reply.
        let new = serde_json::to_value(health.with_lez_chain(Some(stalled_chain()))).unwrap();
        let read: OldClientHealth = serde_json::from_value(new.clone()).unwrap();
        assert!(read.ready && read.degraded);
        let round: MakerHealthV1 = serde_json::from_value(new).unwrap();
        assert_eq!(round.lez_chain(), Some(&stalled_chain()));
    }
}
