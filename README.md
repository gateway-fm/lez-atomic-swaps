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
- independent fixed-role SDK actors with separate schema-v10 SQLite databases
  now complete both ZEC directions through a preimage-revealing LEZ claim and
  the counterparty's Zcash follow-up, then independently
  `resume_claim_capable` at `Completed`. The same externally supplied claim key
  is required when each role reopens its own database; neither plaintext
  preimages nor plaintext exact claim bytes are stored in SQLite or its WAL;
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
  schema-v10 SQLite adapter now proves exact replay, role isolation, retained
  closed-intent validation, atomic rollback, corruption rejection, and
  close/reopen recovery. Its ordered maker journal durably replays canonical
  Zcash evidence, atomic reorg replacement, same-inclusion depth changes, and
  affirmative removal through the exact canonical tracker. Replacement halves
  must share one stable tip, unchanged polls write nothing, and the store
  rejects orphan/holey histories, individually valid but
  history-incompatible appends, and stale-instance divergence. The maker
  independently observes only the
  agreement-selected taker-lock chain and replays that role-local projection
  without taker intent or negotiation state. Forward Zcash rejects a weak
  transaction-ID/depth assertion and durably revalidates the complete canonical
  transaction/block/tip/output record against the signed agreement's exact
  HTLC output binding. Role-local input/change/fee/expiry policy constrains this
  SDK's own builder and is not a remote-wallet acceptance condition. These
  first-lock observations remain non-authorizing on their own. A distinct fresh
  eligibility call replays the durable head, re-queries the exact canonical
  tracker head, writes nothing when unchanged, and returns a non-cached
  revision-bound result. The maker effect now consumes that result internally,
  persists the direction-fixed opposite-chain plan before submission, and
  atomically projects confirmed Maker funding. Both directions reach
  `BothLegsLocked` and survive schema-v10 SQLite close/reopen; `next_action`
  still caches no permission.
  Reverse deterministic-local LEZ accepts a depth-sufficient exact head.
  The public-v0.2 policy seam additionally defines and unit-tests typed
  awaiting-finality outcomes until Bedrock reports Finalized, but public
  agreement activation remains fail-closed pending a reviewed deployment.
  Reverse LEZ requires a stable canonical
  escrow snapshot bound to the signed execution channel/genesis, public fund
  transaction, generated account order, full metadata, exact custody, depth,
  and finality policy; that primitive snapshot is revalidated after SQLite
  close/reopen. A dependency-free two-phase LEZ tracker now proves duplicate
  suppression, monotonic Pending/Safe/Finalized updates, affirmative same-tip
  replacement, stale/tip-regressing evidence rejection, and fatal
  finalized-history changes.
  Revealing LEZ claims now have the same primitive-evidence discipline: the SDK
  binds the node-reported ID to the official-decoder hash, claimant signature,
  generated accounts, exact claim/preimage, terminal metadata, empty custody,
  canonical inclusion, and depth. New secret-free schema-v2 snapshots are fully
  revalidated on SQLite replay with the separately protected preimage; legacy
  opaque v1 rows are read-compatible but cannot be created by live adapters.
  The active SDK and schema-v10 SQLite journal now fold the agreement-selected
  LEZ tracker: exact duplicates write no row and same-inclusion finality/depth
  updates survive close/reopen. Affirmative nonfinal removal and atomic same-tip
  replacement now use complete primitive records, reject stale old-head
  evidence, consume one revision, and replay through SQLite. The official-wire
  LEZ observation/refund conversion, independent actor processes, and the
  completed real-node corridor remain. Schema-v10 now also persists exact
  refund owner intents before broadcast and atomically commits owner/observer
  transitions through `Refunded` in both directions, including rollback,
  conflict, corruption, and close/reopen replay.
  The main workspace now also has a bounded authenticated LEZ sidecar client,
  a signed-agreement native first-lock bridge adapter, typed Zebra
  owner/counterparty claim and refund ports, and the public crash-safe
  timeout-refund SDK contract. The bridge client binds every request
  and response to one run, role, runtime, and one-use request ID; the Zebra
  adapter converts compatibility-selected signed native terms into exact
  initialize/fund SDK bytes without retrying randomized preparation. The Zebra
  adapter derives exact follow-up claims and refunds from the accepted
  agreement, delegates only signing to a role-local capability, revalidates
  stable canonical funding and signed transaction policy, observes before
  byte-identical rebroadcast, and treats ambiguous submission outcomes
  conservatively. Counterparty discovery scans a bounded canonical Zebra
  horizon and treats unresolved or older spends as unstable, never absent. The
  refund driver fixes LEZ-before-Zcash order in both directions, persists exact
  owner bytes before broadcast, distinguishes unknown outcomes, and uses
  observation-only transitions for the other role.
  These are isolated contract tests, not yet a composed maker/taker user flow.
  The sidecar server library now authenticates one run/role capability before
  parsing, restores exact official prepared bytes, and durably guards unknown
  submissions before the node call. Official revealing-claim preparation now
  binds the signed role, runtime, signer, terms, preimage, and funding identity,
  restores the exact randomized bytes after restart, and admits only that
  cached transaction for submission. Escrow and revealing-claim observation
  still fail closed as unavailable. The sidecar executable/runner, those
  observation cores, actor-process integration, and completed LEZ-plus-Zebra
  corridor remain.

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
selected self-hosted and Tatum Testnet Zebrad routes, optional funding wallet,
external dependencies, and the still-missing transparent signer/provider
transport without claiming a completed testnet run.

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

The in-memory and schema-v10 SQLite actor lifecycle tests are a separate,
deterministic lower lane. They start no node or service and use no RPC, Docker,
faucet, public endpoint, or network access. Their only runtime resources are
temporary local maker/taker databases and an explicitly supplied deterministic
test claim key. Consequently, public-chain availability cannot make those
tests flaky; actual Zebra and LEZ node execution remains covered by the
separate node suites and is not implied by the contract-double corridor.

CI also refreshes RustSec and Trivy vulnerability data. A database outage may
block scanning; a newly published advisory may deliberately turn a prior pass
red. Do not bypass that failure as “flaky.” The LEZ v0.2 RPC, self-hosted Zebra
6.0.0, and Tatum's API-key-authenticated Testnet Zebrad gateway are selected.
The Tatum route is a third-party authoritative-node service, not an official
Zcash Foundation endpoint, and its HTTPS adapter/method contract has not passed
live evidence yet. Zcash funding may use a community faucet, Discord request,
or controlled pre-funded wallet, all with explicit availability risk. The
project-owned transparent testnet signer remains unimplemented. Provider limits,
fallback routes, and funding assumptions remain an M2 evidence gate. See
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

To repeat the proven ZEC claim happy path alone:

```sh
cargo build --locked -p lez-zec-swap-sdk -p lez-swap-store
cargo test --locked -p lez-zec-swap-sdk --test sdk_lifecycle \
  independent_actors_complete_lez_then_zcash_claims_in_both_directions \
  -- --exact --nocapture
cargo test --locked -p lez-swap-store --test zec_sdk_recovery \
  schema_v9_claim_journal_completes_and_reopens_independent_actors_in_both_directions \
  -- --exact --nocapture
```

The second test creates different temporary SQLite files for maker and taker.
Each file is opened and reopened with the same external key ID and key material
for that run; the key itself is never written to either database. The expected
terminal evidence is LEZ reveal, Zcash follow-up, and both role-local journals
replaying revision 4 as `Completed` via `resume_claim_capable`.

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
