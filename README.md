# LEZ Atomic Swap Suite

Trustless swaps between Logos Execution Zone (LEZ) and Bitcoin, Monero, and
Zcash's transparent pool.

The accepted delivery scope is Gateway's replacement proposal
[logos-co/rfp#112](https://github.com/logos-co/rfp/issues/112), interpreted
together with the live
[RFP-003](https://github.com/logos-co/rfp/blob/master/RFPs/RFP-003-atomic-swaps.md).
The earlier issue #61 is superseded and Ethereum is not an in-scope pair.

## Current status

Development has started with protocol and real-node acceptance tests. The
current executable slices enforce:

- the taker-funded lock is confirmed before the maker can lock the second leg;
- claim completion after the first lock needs only on-chain evidence; and
- pair-specific claim and recovery ordering, including LEZ-before-ZEC claim and
  refund in both ZEC trade directions;
- immutable local/public-testnet ZEC profiles with network/branch binding,
  checked deadlines, required calibration, and exact margin enforcement;
- typed ZEC observations that re-decode canonical transaction bytes and bind
  network, branch, block, outpoint, value, exact BIP-199 scripts, and depth
  before projecting evidence into the chain-independent coordinator, populated
  from stable actual Zebra RPC queries in the actor E2E;
- exact BIP-199 P2SH plus canonical Zcash V5 funding, claim, and refund
  transactions; and
- actor-keyed funding/claim/refund acceptance and rejection through pinned
  Zebra NU6.2 Regtest consensus, including a two-node conflicting
  four-over-three-block canonical fork replacement; and
- checked-guest deployment plus real-key native LEZ initialize/fund/claim and
  permissionless-refund execution in an isolated standalone sequencer; and
- two-definition official-ATA claim/refund lifecycles with real owner keys,
  immutable destinations, and cross-definition substitution rejection; and
- machine-checked recursive native/authenticated-transfer and token/ATA/Token
  Risc0 session costs with setup and Clock noise excluded; and
- a bounded dual-signed LEZ/ZEC agreement integrated through role-fixed
  negotiation, persistence-before-activation, and adversarial resume, without
  exposing transport, raw chain, or recovery-store handles after activation;
  plus exact first-lock intent staged before node effects, observe-before-exact
  rebroadcast after restart, and separately recoverable LEZ initialize/fund
  steps; confirmed evidence is applied only after an atomic store commit or an
  exact unknown-outcome probe, and is replayed on resume. A role-fixed
  schema-v5 SQLite adapter now proves exact replay, role isolation, retained
  closed-intent validation, atomic rollback, corruption rejection, and
  close/reopen recovery. The maker independently observes only the
  agreement-selected taker-lock chain and replays that role-local projection
  without taker intent or negotiation state. Forward Zcash rejects a weak
  transaction-ID/depth assertion and durably revalidates the complete canonical
  transaction/block/tip/output record against the signed agreement's exact
  HTLC output binding. Role-local input/change/fee/expiry policy constrains this
  SDK's own builder and is not a remote-wallet acceptance condition. It remains non-authorizing:
  next action is `Wait` until canonical evidence and a fresh reorg-safe
  eligibility check exist. Production adapters, later effects, and the
  completed corridor remain.

See the living [implementation plan](docs/implementation-plan.md), the
[whole-system actor and flow architecture](docs/architecture/system-architecture.md),
the [deployment component and RPC inventory](docs/architecture/deployment-components-and-rpcs.md),
the [architecture decision log](docs/architecture/README.md), the living
[manual reproduction guide](docs/manual-user-flows.md), and the first
[acceptance tests](crates/swap-core/tests/e2e_swap_lifecycle.rs). The
[upstream Logos production-blocker register](docs/upstream-production-blockers.md)
separates disclosed external release risks from repository-controlled milestone
acceptance. The
[Zcash public-testnet setup guide](docs/zcash-testnet-setup.md) records the
selected self-hosted route, optional funding wallet, external dependencies, and
the still-missing transparent signer without claiming a completed testnet run.

## Development

Prerequisites: Rust 1.96.0. Docker is needed for the isolated Zebra consensus
suite and the pinned Risc0 guest builder; Docker Compose v2 is used by Zebra.
The [manual reproduction guide](docs/manual-user-flows.md) lists the complete
per-run prerequisites, isolation rules, commands, expected evidence, and
cleanup behavior.

### External dependencies and flakiness

The current executable operator, Zebra, and LEZ flows use no public blockchain
RPC or faucet. The official LEZ v0.2 endpoint
`https://testnet.lez.logos.co` is selected and its health/block/program methods
were checked on 2026-07-12, but no repository user flow submits to it yet. Maker
and Zebra host endpoints are ephemeral loopback services. The LEZ
test client uses loopback, but pinned upstream v0.1.2 binds its ephemeral server
to the host wildcard address; it is short-lived and collision-isolated, not
loopback/network-namespace isolated. Test funds are
deterministic local genesis/Regtest outputs. Cold builds still depend on
rustup/crates.io, locked GitHub sources, digest-pinned Docker Hub/GCR images,
the checksum-pinned Logos circuits release, and `rzup`'s pinned Risc0 tools.
Availability, DNS, proxy, registry throttling, or GitHub/CDN outages can block
an uncached run, but cannot relax the lockfile, digest, checksum, ELF, ImageID,
or consensus checks. Warm verified caches reduce this availability risk.

These are real local on-chain executions, not mocks: pinned Zebra
validates/mempools/mines signed Zcash
transactions and chooses a higher-work fork; the pinned LEZ sequencer deploys
the checked guest, executes production state transitions, and persists
canonical actor/custody state. Loopback supplies safe isolation while the real
consensus/state-transition implementations supply fidelity. Regtest/standalone
do not prove public peer
propagation, fee markets, organic timing/reorg behavior, provider quirks, or LEZ
testnet 0.2 compatibility. A composed local corridor and self-hosted/public
testnet corridor with real funded accounts remain mandatory M2 evidence.

CI also refreshes RustSec and Trivy vulnerability data. A database outage may
block scanning; a newly published advisory may deliberately turn a prior pass
red. Do not bypass that failure as “flaky.” The LEZ v0.2 RPC and self-hosted
Zebra 6.0.0 public-testnet route are selected; no official public Zebra
JSON-RPC route was found. Zcash funding may use a community faucet, Discord
request, or controlled pre-funded wallet, all with explicit availability risk.
The project-owned transparent testnet signer remains unimplemented. Provider
limits, fallback routes, and funding assumptions remain an M2 evidence gate. See
the [full resource/flakiness table](docs/manual-user-flows.md#external-resources-and-flakiness).

    cargo test --locked --workspace --all-targets
    cargo fmt --all --check
    cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
    cargo deny check advisories bans licenses sources
    npm ci
    npm audit --audit-level=moderate
    npm run audit:licenses
    npm run test:mermaid
    RUN_ID=local-lez-v02-a ./scripts/verify-lez-v02-provisional.sh
    RUN_ID=local-zebra-1 ./scripts/run-zebra-e2e.sh

The provisional LEZ v0.2 command compiles exact SPEL PR #238 head
`df17acd98436be4f09c55877dae1fe2e73cbcdca` against official LEZ `v0.2.0`
at `a58fbce2ff48c58b7bb5001b1a27e64b9596ee3a`. It uses two Cargo jobs and
unique `/tmp` target/tool paths derived from the lowercase `RUN_ID`. It starts
no Docker workload, network namespace, port, service, or sequencer. A cold run
still needs crates.io/GitHub access, `unzip`, and working libclang C headers,
and compiles the large standalone dependency graph; avoid overlapping it with
another heavy build on the same host.

That seam proves the v0.2 standalone config and `LeeTransaction` API compile,
one tag-based `lee_core` identity is locked to the exact LEZ commit, and SPEL's
public PDA matches LEZ's fixed `/LEE/` vector. A second test compiles the SDK's
dependency-light derivation source directly in this pinned fixture and proves
its swap metadata, native multi-seed custody, and associated-token-account bytes
match exact upstream `lee_core`, SPEL, and ATA-core types. It does not build or
deploy the escrow guest/client, execute actor lifecycles, measure costs, or
contact the public testnet. PR #238 remains unmerged and unreviewed, so a pass
is explicitly not M2 completion or final release approval.

Cargo-deny also reports that exact LEZ graph as forcing vulnerable Hickory DNS
`0.25.0-alpha.5` (`RUSTSEC-2026-0118` and `RUSTSEC-2026-0119`). The provisional
fixture carries compile-only exceptions guarded by a hash-locked test plus
checks that no standalone future is polled and no DNSSEC feature is enabled.
This graph is prohibited for runtime and testnet use. The next slice remains
security-blocked until upstream supplies a safe graph or a separate explicit
review accepts a narrowly defined runtime risk.

`npm run test:mermaid` scans every tracked Markdown Mermaid block, rejects
GitHub-host-sensitive configuration, beta/new-shape, and interactive syntax,
then renders every diagram with the exact Mermaid CLI 11.16.0 pin. GitHub's
live Viewscreen renderer also reported 11.16.0 on 2026-07-12; the exact asset
and SHA-256 are recorded in
[`docs/evidence/github-mermaid-renderer.json`](docs/evidence/github-mermaid-renderer.json).
GitHub controls that renderer, so the repository deliberately retains a
conservative syntax subset and requires a visual check after documentation is
pushed.

On a hardened Linux host where Chromium cannot create its own user namespace,
keep the browser download isolated and opt into the repository's no-sandbox
Puppeteer profile only inside an already isolated test account/container:

```sh
PUPPETEER_CACHE_DIR=/tmp/lez-mermaid-browser \
  npx puppeteer browsers install chrome-headless-shell
PUPPETEER_CACHE_DIR=/tmp/lez-mermaid-browser \
  MERMAID_ALLOW_NO_SANDBOX=1 npm run test:mermaid
```

Do not set `MERMAID_ALLOW_NO_SANDBOX=1` for general web browsing or an
untrusted checkout. CI uses its own ephemeral runner and the default command
whenever the runner's Chromium sandbox is available.

The Zebra suite uses a unique `lez-atomic-swaps-${RUN_ID}` Compose project. It
copies the binary from the digest-pinned official Zebra 5.2.0 image into a
digest-pinned distroless nonroot runtime, then runs two disconnected nodes on a
project-only network with read-only filesystems, independent tmpfs state,
resource caps, no Linux capabilities, and separate ephemeral localhost RPC
ports. Before Compose starts it allocates an absolute run-scoped maker SQLite
database and refuses any pre-existing manifest, database, WAL, or SHM. The suite
first proves real canonical funding, close/reopen/requery, deeper-fork removal,
second restart, and exact replay through the maker runtime; it then runs the
actor fund/claim/refund/concurrent-fork consensus fixture. Cleanup addresses that
exact project and never prunes or stops resources it did not create.

## Licensing

Licensed under either the Apache License, Version 2.0 or the MIT License, at
your option.
