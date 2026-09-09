# Third-party maker and taker extension APIs

Proposal and implementation scope for
[ecosystem #176](https://github.com/logos-co/ecosystem/issues/176) and its
[scope questions](https://github.com/logos-co/ecosystem/issues/176#issuecomment-5524168376).

Gateway supplies swap execution and documented APIs. Independent teams can
build maker strategies, taker automation, arbitrage tools, and operator kits on
those APIs. This change makes that boundary consumable; it does not commission
or implement those applications.

## Relationship to the accepted work

[RFP-003 #112](https://github.com/logos-co/rfp/issues/112) and
[M5 #125](https://github.com/logos-co/rfp/issues/125) already cover pair SDKs,
the maker daemon, CLIs, persistence, recovery, concurrent-swap isolation,
deployment, and the local/Logos C API price-source implementations. These remain
Gateway responsibilities. The current released product path is BTC↔LEZ on
private local chains; broader RFP pair scope is not a claim of availability in
this release. Testnet readiness must be established before an LP relies on it.

## Answers to the six questions

1. **Spreads, sizes, fees, inventory adjustments after a reference price?**
   The price-source interface returns an exact reference ratio. The raw maker
   snapshots a configured ratio, amount bounds, and TTL into an offer. It does
   not calculate trading spreads, inventory skew, or strategy quote sizes.
   Chain execution has its own configured fee and funding mechanics; a
   strategy fee model or separate trading-fee field is not supplied by this API.
   An independent strategy can compute its final ratio and bounds and submit
   them through the local route API.
2. **Automatic create, refresh, reprice, withdraw?** The node exposes the
   primitives. A third-party scheduler decides when to call them. Existing
   offers are immutable; changing route settings affects future publications.
   Repricing means withdrawing an unreserved offer and publishing a new ID.
   TTL expiry, signed-announcement rebroadcast, route-health withdrawal, and
   settlement progress already have node mechanisms, but those mechanisms are
   not a market-making loop reacting to prices or balances.
3. **Inventory/exposure limits or circuit breakers?** Existing protections
   include offer bounds/expiry, one-winner reservation, revision checks,
   authenticated commitments, route-health checks, and actor recovery gates.
   Portfolio limits, wallet-wide capital reservations, balance-triggered
   circuit breakers, and aggregate exposure management are not implemented or
   promised by this API change. A third party may implement conservative policy
   using independently obtained wallet information and node projections.
   Offer history is not spendable balance, and several offers can compete for
   the same capital. Hard capital guarantees would require a separately
   designed node-side admission/reservation API.
4. **Stable strategy interface separate from the price source?** Yes: the
   documented owner JSON-RPC contract is the extension boundary. Programs in
   any language can read public projections and submit reviewed decisions.
   They run as separate owner-authorized processes. The existing `PriceSource`
   trait/C API stays a source-of-prices boundary; no strategy plugin ABI or
   dynamic code loading inside the signer/daemon is introduced.
5. **Advanced strategies, simulation, rebalancing, hedging?** These are
   independent applications outside this implementation. The APIs provide
   execution building blocks, not a simulator, cross-venue execution service,
   historical market feed, or a profitability guarantee. Rebalancing and
   hedging need separate wallet/venue integrations and policy.
6. **Most useful independent extension?** A maker policy and operations
   application using these APIs: reference data → final quotes and sizes →
   offer scheduling → exposure monitoring, with simulation and operator
   feedback. Taker automation can use the same settlement service to select
   and execute exact authenticated offers. Neither needs to rebuild the swap
   protocol, daemon, or signing machinery.

## What this PR delivers

- A [public API reference](README.md) for maker configuration, publication,
  withdrawal, history and recovery, and taker discovery, initiation, lock,
  monitoring, claim and refund. It defines transport, compatibility, exact
  amounts, retries, error handling, and capability limits.
- `maker_offer_publish_v1`: publication guarded by the exact policy and local
  price revisions selected by the caller, checked in the same SQLite
  transaction as the offer insertion and durable replay record. This closes
  the review-to-publication race when a UI or another controller edits a route.
  The original `maker_offer_publish` remains available for its existing
  source-selected behavior.
- A small dependency-free [Python wire client](../../examples/owner-api/owner_rpc.py),
  runnable JSON request examples, and regression tests. This is an integration
  example rather than an arbitrage bot or operator starter kit.
- No new taker execution engine: the existing node-owned lifecycle already
  supplies the necessary calls. This PR documents how third parties use them.

## LP options

An **operator adoption LP** could retain the issue's proposed three independent
operators, a defined observation period, and setup/operation feedback, using
the existing Gateway services plus third-party packaging or policy. Its
testnet prerequisite and run duration need to be specified by the LP owner.

An **advanced feature LP** could instead fund policy, simulation, inventory
accounting, or venue adapters with explicit acceptance criteria, using these
APIs as its execution boundary. Funding another raw swap SDK, daemon, CLI, or
basic price adapter would overlap the accepted work. Whether either LP should
be awarded remains an ecosystem decision.

## Acceptance and remaining limits

An integrator can publish exactly reviewed terms, detect a configuration race,
replay a lost response without duplicating an offer, discover an authenticated
offer, initiate its exact amounts, and request only the actor-admissible actions
using opaque swap IDs. Integration tests cover guarded publication, persisted
replay, legacy separation, validation and migration. Existing lifecycle tests
remain the settlement evidence; this PR does not add public-network evidence.

Inventory/balance snapshots, atomic quote replacement, batch cancel, durable
event cursors, scoped remote credentials, per-strategy quotas, and hard capital
reservations are possible subsequent API proposals. They are explicitly not
endpoints third parties can depend on today.
