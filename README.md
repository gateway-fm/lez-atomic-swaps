# LEZ Atomic Swap Suite

Trustless swaps between Logos Execution Zone (LEZ) and Bitcoin, Monero, and
Zcash's transparent pool.

The accepted delivery scope is Gateway's replacement proposal
[logos-co/rfp#112](https://github.com/logos-co/rfp/issues/112), interpreted
together with the live
[RFP-003](https://github.com/logos-co/rfp/blob/master/RFPs/RFP-003-atomic-swaps.md).
The earlier issue #61 is superseded and Ethereum is not an in-scope pair.

## Current status

M2 is certified at its private local-functional PoC boundary under
`m2-complete`. M3 is active. Its authority, Bitcoin Core 31.1 Regtest
topology, dependency candidates, actor flows, and acceptance gate are
entry-audited in
[ADR 0029](docs/architecture/0029-m3-bitcoin-local-poc-entry.md). The
nonexistent DLC Schnorr-vector reference is separately tracked as
[Gateway erratum GW-M3-001](docs/proposal-acceptance-errata.md), with no accepted
replacement yet.

M3 now has an actual-Core P2TR vertical slice. Exact-pinned `bitcoin` 0.32.101
constructs the aggregate-internal-key plus CSV-refund commitment, signs a
one-input cooperative key-path spend under tweaked output key `Q`, and emits
the exact one-item witness. The isolated runner verifies the official Core
release/signers/Guix attestations, then executes the `TakerSellsForeign`
Bitcoin-leg ordering: the taker client signs and submits a normal funding
transaction to the exact contract, the maker observes its confirmation, the
maker submits the cooperative claim, and the taker observes the exact outpoint
spent once. Core independently policy-checks, decodes, mines, and re-reads both
transactions through distinct actor `rpcauth` capabilities. The successful
local validation reached height 103, an empty final mempool, zero peers, and
exact run-owned cleanup.

This is deliberately a transaction/consensus fixture, not a complete swap:
the known Regtest key is labeled fixture-only, and MuSig2/adaptor sessions,
durable nonces, scalar extraction, the LEZ BTC guest, the second direction, and
end-to-end atomicity remain. There is no executable full BTC swap command yet.
CI runs the same P2TR funding/claim composition and fail-hard scans the exact
Core image for HIGH/CRITICAL vulnerabilities. The earlier clean infrastructure
run remains available as
[secret-safe Core evidence](docs/evidence/m3-bitcoin-core-smoke-a7393df-20260714.json);
the clean pushed-commit P2TR run is retained as
[secret-safe funding/claim evidence](docs/evidence/m3-bitcoin-core-p2tr-4f7b6b3-20260715.json). Runtime uses
no public RPC, faucet, public funds, public peers, or public chain. Cold setup
still depends on checksum-verified Core release assets, the pinned base image,
vulnerability data, and locked Rust registries, so availability and scan
flakiness remain explicit.

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
  evidence, consume one revision, and replay through SQLite. Official-wire LEZ
  native escrow, revealing-claim, and native-refund conversion is implemented;
  the context-owning SDK-port wrapper, independent actor processes, and the
  completed real-node corridor remain. Schema-v10 now also persists exact
  refund owner intents before broadcast and atomically commits owner/observer
  transitions through `Refunded` in both directions, including rollback,
  conflict, corruption, and close/reopen replay.
  The main workspace now also has a bounded authenticated eight-method LEZ
  sidecar client,
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
  cached transaction for submission. Native escrow observation now decodes
  official transactions, signatures, instructions, metadata, custody, block
  links, genesis, and stable tip brackets for exact owners and bounded
  counterparty discovery; the main adapter independently revalidates those
  primitive facts against the signed agreement. Bounded or old misses remain
  unknown, never false absence. Official revealing-claim observation now
  validates the canonical Risc0 instruction, message, witness, ordered accounts,
  transaction placement, terminal metadata, and zero custody for exact owners
  or bounded counterparty discovery. Only a complete stable window is absent;
  partial coverage is unknown and ambiguity or a moving tip fails closed. The
  executable runner starts concurrent maker and taker sidecars with separate
  private keys, capabilities, runtime descriptors, durable stores, and
  ephemeral loopback listeners. All eight sidecar methods now execute. Native
  refunds are official permissionless `RefundNative` transactions with no
  nonce or witness; exact-owner and bounded counterparty observations require
  a stable clock, terminal refunded accounts, zero custody, canonical bytes,
  and restart-safe cache membership. The main native-refund and revealing-claim
  adapters validate both signed directions, exact caller-owned IDs/windows,
  complete primitive facts, durable identities/bytes, and conservative
  one-attempt outcomes. Zebra now also discovers agreement-bound unknown-ID
  funding for both role directions from stable canonical block and mempool
  evidence. A reusable external exact-v0.1.2 standalone-node implementation
  verifies the tracked guest ELF SHA-256 and Risc0 ImageID before creating any
  state, starts from a fresh mode-0700 home on a dynamic port, deploys the
  checked guest, and publishes a no-clobber mode-0600 readiness manifest. The
  private handoff binds the loopback endpoint, channel, genesis ID/hash,
  ELF/ImageID/ProgramId, canonical deployment transaction and containing block,
  the advertised authenticated-transfer built-in, and two official-RPC-verified
  funded actor accounts and signing keys; tampered guests and pre-existing homes
  fail without mutation. Its first exact process run exposed and rejected an
  incorrect assumption that `getProgramIds` listed custom deployments. The
  last corrected full exact runner was GREEN: schema-v2 transaction/block evidence,
  process rejection paths, native/two-definition actor lifecycles, strict
  Clippy, and recursive cost reproduction all pass. A subsequent actor-contract
  RED found that its all-zero deterministic channel could not enter a signed
  SDK agreement; the node/config/readiness source now uses one nonempty fixed
  channel and its focused locked-graph suite is GREEN. The exact full runner is
  rechecked before corridor evidence or M2 certification. This completes the
  local node handoff, but not its consumption by independent corridor actors.
  Context-owning SDK-port composition is GREEN. The exact-outpoint Zebra
  funding planner is GREEN. The Unix-only one-shot maker/taker boundary now
  loads a deny-unknown-fields schema-v3 private configuration that fixes the
  run, swap, role, signed-agreement SHA-256, LEZ runtime, Zebra identity,
  typed Zebra route, discovery window, exact funding outpoints, and every
  role-local persistence and credential path. Thirty boundary tests reject
  unsafe permissions, symlinks, hard-link/path aliases, same-inode rewrites,
  late alias creation, wrong roles/identities/routes, unsafe credential
  combinations, and secret-bearing diagnostics. `status` remains deliberately offline: terminal
  SDK replay has no LEZ/Zebra trait bound and needs only the role store plus
  claim-recovery key; effect credentials and chain endpoints may be unavailable.
  `activate` and `drive` now compose descriptor-bound SQLite, the authenticated
  loopback role sidecar, and the selected local or public-capable Zebra
  transport. Both local LEZ-plus-Zebra directions are completed evidence;
  public execution is not.

See the living [implementation plan](docs/implementation-plan.md), the
[milestone delivery metrics](docs/milestone-metrics.md), the
[whole-system actor and flow architecture](docs/architecture/system-architecture.md),
the [deployment component and RPC inventory](docs/architecture/deployment-components-and-rpcs.md),
the [architecture decision log](docs/architecture/README.md), the living
[manual reproduction guide](docs/manual-user-flows.md), and the first
[acceptance tests](crates/swap-core/tests/e2e_swap_lifecycle.rs). The
[upstream Logos production-blocker register](docs/upstream-production-blockers.md)
separates disclosed external release risks from repository-controlled milestone
acceptance. The
[progressive milestone delivery decision](docs/architecture/0027-progressive-jpeg-milestone-delivery.md)
puts the active milestone reproducible local-devnet happy path first, then
enters QA with RED-GREEN-REFACTOR, chaos, information-security, and production-
readiness hardening only when the repository owner ends each phase. M2 is
currently in the PoC phase; earlier hardening remains carried evidence, not a
claim that those later phases are complete. The
[private local M2 certification decision](docs/architecture/0023-private-local-m2-certification.md)
requires one actual public-compatible local LEZ v0.2 devnet, one actual local
Zcash Regtest devnet, and
independent maker/taker processes while deferring public evidence without
claiming it exists. The
[source-audited local-stack decision](docs/architecture/0024-source-audited-lez-v0-2-local-stack.md)
binds the exact Bedrock image/source labels, LEZ source, toolchain, native
inputs, service flows, and service-binary hashes. Retained run
`m2poc-vertical-20260714a` proves the three official local v0.2 services, both
finalized actor Vault Claims, checked escrow deployment, and a role-separated
native initialize/fund/claim lifecycle in finalized blocks 219/220/223. Fresh
isolated chain runs `m2poc-fresh-lez-20260714a` and
`m2poc-fresh-zebra-20260714a` then supported both completed
reference-actor corridors. In the first run,
`m2poc-corridor-fresh-20260714o`, the
`TakerSellsLez` role order, the taker initialized and funded LEZ, the maker
observed it and funded the Zcash HTLC, the maker waited for two Zcash
confirmations and revealed the preimage by claiming LEZ, and the taker used that
reveal to claim Zcash. Both independent actor stores reached `Completed`
revision 4 after 39 drive rounds and 78 actor events in 25.370 seconds. One
payload-free `moving_tip` observation failure was retried once within the
maximum-eight same-run policy and then succeeded.

LEZ initialize/fund/claim finalized in blocks 264/265/266 and ended `Claimed`
with custody 0, depositor balance 100000, and claimant balance 150000. Zebra
funding transaction `255b991f...dceab` entered block 106, received the required
second confirmation in block 107, and claim transaction `a2b41c5f...be16e`
spent its `:0` HTLC output in block 108. No public RPC, faucet, or public funds
were used. Exact secret-safe facts are in the
[first-direction corridor evidence](docs/evidence/m2-taker-sells-lez-corridor-20260714.json);
the earlier [local-onboarding evidence](docs/evidence/m2-local-onboarding-20260714.json)
remains the component baseline. Failed fresh attempts 14i and 14k through 14n
made no chain effect. Attempt 14j stopped after only one Zcash confirmation and
retains 50000 LEZ in its distinct failed swap; its files and funds must never be
reused.

Reverse run `m2poc-corridor-reverse-fresh-20260714c` then completed
`TakerSellsForeign`. The taker funded Zcash at height 113, the maker funded LEZ
in finalized blocks 641/642, the taker revealed by claiming LEZ in finalized
block 643, and the maker spent the exact Zcash `:0` output at height 115. Both
actors reached revision 4 `Completed` in 26.960 seconds. Terminal LEZ state was
`Claimed` with custody 0, maker depositor balance 0, and taker claimant balance
150000. Two prior fresh reverse attempts are retained and never reused; they
exposed and reproduced a forward-only canonical LEZ validator, now corrected
to bind the signer to the agreement-derived depositor. Exact secret-safe facts
are in the
[reverse-direction corridor evidence](docs/evidence/m2-taker-sells-foreign-corridor-20260714.json).
The M2 local-functional PoC is certified **2 of 2** under the annotated
`m2-complete` tag. The tag binds the exact closure tree to the canonical
evidence packet; it does not claim that the owner has entered QA, M3, or the
deferred recovery, chaos, public-execution, and production-readiness phases. The
schema-v3 Zebra route selection, public HTTPS `x-api-key` transport, and LEZ
`official_public` sidecar route are now locally verified portability contracts;
they have not made a public call.
Current-schema certification runs
`m2cert-schema3-forward-2d09997-20260714a` and
`m2cert-schema3-reverse-2d09997-20260714a` also repeated both directions through
the actual pinned local LEZ v0.2 and Zebra Regtest nodes. Both independent actors
reached `completed`, the atomic effect order was observed, and no public RPC or
faucet was used. The secret-safe aggregate is in the
[schema-v3 corridor evidence](docs/evidence/m2-schema-v3-local-corridors-20260714.json).
Those earlier runs are retained as historical behavior evidence. Final local
certification rebuilt the guest through the exact digest-pinned Risc0 Docker
builder, and the independently Docker-backed methods embedding produced the
same ELF `c85055f6...c9d2e` and ImageID/ProgramId `5cf8c5a4...329c1`. That
artifact was deployed once and finalized in local LEZ block 2582; canonical
runs `m2cert-canonical-forward-bb53daf-20260714a` and
`m2cert-canonical-reverse-bb53daf-20260714a` then completed both directions
against that deployment and Zebra Regtest. The new immutable
[canonical certification packet](docs/evidence/m2-canonical-local-certification-20260714.json)
binds the builder, artifact, deployment, actors, exact chain effects, terminal
states, and absence of public resources without rewriting earlier evidence.
PoC-to-hardening and milestone
transitions remain repository-owner decisions. The
[Zcash public-testnet setup guide](docs/zcash-testnet-setup.md) records the
selected self-hosted and Tatum Testnet Zebrad routes, optional funding wallet,
external dependencies, and the still-missing public credentials, funded
accounts, deployment, and live method evidence without claiming a completed
testnet run.

## Development

Prerequisites: Rust 1.96.0. Docker is needed for the isolated Zebra consensus
suite, pinned Risc0 guest builder, and full local LEZ v0.2 lane; Docker Compose
v2 is used by both local-chain suites. Building the exact upstream v0.2
sequencer/indexer artifacts additionally uses upstream Rust 1.94.0 plus the
hash-checked r0vm and Rapisnark inputs; the repository-owned sidecar remains on
Rust 1.96.0. Direct Cargo commands do not certify that v0.2 sidecar because the
upstream Rapisnark build script can download native libraries even with Cargo
offline; use the hash-attesting wrapper documented in the manual guide. The
[manual reproduction guide](docs/manual-user-flows.md) lists the complete
per-run prerequisites, isolation rules, commands, expected evidence, and
cleanup behavior.

### Local LEZ v0.2 service-readiness quick start

From a clean host, provision a clean exact LEZ `v0.2.0` checkout, the two
locked service binaries, and verified `r0vm 3.0.5` as described in the
[manual flow](docs/manual-user-flows.md#flow-0b2-run-the-isolated-lez-v02-service-stack).
Then run:

```sh
export LEZ_V02_SOURCE_DIR=/absolute/path/to/clean/logos-execution-zone-v0.2.0
export LEZ_V02_SERVICES_DIR=/absolute/path/to/locked/release-binaries
export LEZ_V02_R0VM=/absolute/path/to/verified/r0vm
RUN_ID=manual-v02-stack-001 ./scripts/run-lez-v02-stack.sh
```

The command creates unique run-scoped containers and a no-masquerade bridge,
uses dynamic `127.0.0.1` RPC ports, writes evidence below
`.e2e/manual-v02-stack-001/lez-v02`, and removes plus asserts absence of only
its exact containers, network, and image. It uses no public chain RPC, faucet,
or public funds. A cold setup can still depend on GHCR/GCR for the two exact
digest-pinned images and on GitHub/Rust/crates distribution while provisioning
source and binaries. This proves LEZ service readiness only; it is not yet the
manual atomic-swap corridor.

### M2 corridor and route-selection quick start

After provisioning fresh isolated LEZ v0.2 and Zebra Regtest nodes with the
manual guide's Flow 0 prerequisites, build and run the same local user boundary
used by the retained PoC:

```sh
cargo build --locked -p zec-reference-actor --bin zec-reference-actor
export RUN_ID=manual-m2-corridor-001
export POC_DIRECTION=taker_sells_lez # or: taker_sells_foreign
export POC_OUTPUT_ROOT="${TMPDIR:-/tmp}/lez-atomic-swaps-${RUN_ID}"
export LEZ_SEQUENCER_URL=http://127.0.0.1:<sequencer-port>
export LEZ_INDEXER_URL=http://127.0.0.1:<indexer-port>
export ZEBRA_RPC_URL=http://127.0.0.1:<zebra-port>
export ESCROW_PROGRAM_ID=5cf8c5a4eedb3c2873956cb7898eb33a495407c9746fb1a065c99638159329c1
export RAPIDSNARK_LIB_DIR=/absolute/path/to/verified/rapidsnark-v0.0.8-libraries
./scripts/run-m2-taker-sells-lez-poc.sh
```

The historical script name covers both directions. It refuses a reused output
root, serializes access to the exact node tuple, and uses fresh local
genesis/Regtest funds. See
[Flow 0G](docs/manual-user-flows.md#flow-0g-run-either-development-m2-corridor-direction)
for all prerequisites, evidence assertions, and cleanup rules.

The role-private `zec-reference-actor` schema is version 3. Its `zebra.route`
is exactly one of these deny-unknown-fields objects:

```json
{
  "kind": "deterministic_local",
  "endpoint": "http://127.0.0.1:18232",
  "cookie_file": null
}
```

```json
{
  "kind": "self_hosted_cookie",
  "endpoint": "http://127.0.0.1:8232",
  "cookie_file": "/absolute/private/run/maker-zebra.cookie"
}
```

```json
{
  "kind": "tatum_testnet_x_api_key",
  "endpoint": "https://zcash-testnet-zebrad.gateway.tatum.io",
  "api_key_file": "/absolute/private/run/maker-tatum-api-key"
}
```

`deterministic_local` requires the Regtest identity; `self_hosted_cookie`
requires a matching public Mainnet or Testnet identity; and the exact Tatum
route requires Testnet. The two role configs select the same route kind and
endpoint. Any cookie or API-key file, each actor config, signer key, claim key,
preimage, and sidecar capability must be a regular owner-only mode-`0600` file
below a mode-`0700` role directory. Never put a credential in a URL, JSON
value, command line, log, or committed file. The actor loads credentials only
for `drive`; `status` remains offline.

The LEZ v0.2 sidecar independently selects one complete outbound node profile.
Local runs use `--node-profile local` with distinct literal-loopback sequencer
and indexer URLs. The dormant public route uses
`--node-profile official_public` and requires the exact URL
`https://testnet.lez.logos.co/` for both `--sequencer-url` and `--indexer-url`.
In either profile, `--listen-address` and each actor's `bridge.endpoint` remain
dedicated `127.0.0.1:<role-port>` listeners protected by a role/run capability;
the actor-to-sidecar hop is never public.

Moving from the proved local route to public Testnet requires only route
selection under the signed agreement/runtime configuration plus the expected
on-chain deployment and account/key/fund provisioning. It does not require a
different actor, sidecar, or chain adapter. No automated test, retained M2 run,
or manual command in this repository has called either public chain endpoint,
used a faucet, or spent public funds. Live public deployment and method evidence
remain deliberately deferred under the progressive-PoC boundary.

### External dependencies and flakiness

The current automated and retained local PoC flows use no public blockchain
RPC, faucet, credential, or public funds. Public-route parsing, TLS client
construction, credential loading/redaction, and strict LEZ profile selection
are tested without connecting. The successful corridor used dynamic-loopback
Bedrock, sequencer, indexer, Zebra Regtest, and two independently authenticated
role-sidecar processes. Its retained ports `32831` through `32834`, maker
sidecar port `52289`, and taker sidecar port `49643` belong only to the named evidence
runs; manual runs must allocate fresh dynamic ports and a fresh output root. The
official LEZ v0.2 endpoint
`https://testnet.lez.logos.co` is selected and its health/block/program methods
were checked on 2026-07-12, but no repository user flow submits to it yet.
Maker, Zebra-adapter, and sidecar host endpoints are ephemeral loopback
services. The LEZ
test client uses loopback, but pinned upstream v0.1.2 binds its ephemeral server
to the host wildcard address; it is short-lived and collision-isolated, not
loopback/network-namespace isolated. The reusable external node refuses an
existing home, creates its own mode-0700 directory, and publishes only a
dynamic `127.0.0.1` client endpoint in a mode-0600 readiness file. That file is
secret-bearing because it carries the two deterministic genesis signing keys;
it must remain run-local and must never be logged or committed. Test funds are
deterministic local genesis/Regtest outputs whose account IDs, key derivations,
authenticated-transfer ownership, and positive balances are re-read through
the official LEZ RPC before readiness. Upstream `getProgramIds` is a static map
of built-ins, not a deployed-program registry: the process uses it only to bind
the authenticated-transfer owner. Custom guest deployment is proved by exact
`getTransaction` bytes plus the containing canonical `getBlock` ID/hash stored
in readiness. Cold builds still depend on
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
testnet 0.2 compatibility. Both composed private local directions are now
proved through independent actor processes. Public deployment and
public-testnet execution are explicitly deferred
to production readiness under ADR 0023; the same binaries and adapters must
switch routes through signed configuration/provisioning only.

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
Zcash Foundation endpoint. Its bounded HTTPS `x-api-key` adapter and schema-v3
actor wiring are locally GREEN, while its live method contract has no evidence
yet. Zcash funding may use a community faucet, Discord request, or controlled
pre-funded wallet, all with explicit availability risk. The role-keyed signer
is wired, but no public key, TAZ funding, broadcast, or confirmation has been
exercised. Provider limits, fallback routes, and funding assumptions remain
production-readiness evidence; M2 retains no live public-execution requirement
under ADR 0023. See
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
separate run-local root, guest, artifact, tool, and Docker-source paths derived
from the lowercase `RUN_ID`. It builds the deployment ELF with a digest-pinned
official Risc0 guest-builder image, but starts no sequencer, listener, or fixed
port. A cold run needs Docker plus crates.io/GitHub and circuit/image
distribution access, `unzip`, and working libclang C headers, and compiles the
large official graph; do not overlap it with another Docker-heavy or native
build on the same host.

The lane now proves the v0.2 standalone config and `LeeTransaction` API compile,
locks one tag-based `lee_core` identity to the exact LEZ commit, and matches
SPEL public PDA bytes to LEZ's fixed `/LEE/` vector. It also builds the Risc0
escrow guest and generated client, binds exact ELF SHA-256/ImageID/ProgramId,
executes recursive native and two-definition token claim/refund lifecycles, and
proves full rollback when a child transfer fails. Its exact-once official-RPC
deployer accepts evidence only after immutable endpoint/channel/built-in,
genesis, transaction-byte, transaction, block, and artifact checks. Before
printing retained public evidence, `deploy` authenticates it with a separate
owner-only 32-byte HMAC-SHA256 key. Its offline `provision-identity` command
requires that same zeroized key, verifies the authentication tag and bounded
evidence, then atomically writes a no-clobber public runtime identity in a
non-shared-writable directory containing the exact
chain/channel/genesis/program/deployment fields consumed by signed
provisioning. Public-testnet
deployment and deployed-runtime costs are deferred under ADR 0023. The
public-compatible local v0.2 node corridor and independent actors are GREEN in
both directions. Dormant schema-v3 Zebra routes, the public HTTPS transport,
and the LEZ `official_public` profile are locally GREEN; live deployment,
credentials, funds, method smoke, and public transactions remain deferred.
PR #238 remains unmerged and unreviewed. That status is a production-release
blocker under ADR 0018, not a private M2 blocker. The final private M2
repository certification gate is GREEN and bound by `m2-complete`; the public
and production gates remain explicitly deferred.

Cargo-deny reports that the exact official LEZ graph forces Hickory DNS
`0.25.0-alpha.5` (`RUSTSEC-2026-0118` and `RUSTSEC-2026-0119`) through
Logos-owned common/libp2p dependencies. Graph-local policy permits only those
exact advisories; tests bind the pins, exclude the generated wallet graph, keep
the sequencer future unpolled, and reject DNSSEC features. Under ADR 0018 this
disclosed upstream exception does not block private local M2 certification, but
it remains a production-release blocker pending an upstream fix or explicit security
acceptance.

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
