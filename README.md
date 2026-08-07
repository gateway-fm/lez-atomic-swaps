# LEZ Atomic Swap Suite

Trustless swaps between Logos Execution Zone (LEZ) and Bitcoin, Monero, and
Zcash's transparent pool.

The accepted delivery scope is Gateway's replacement proposal
[logos-co/rfp#112](https://github.com/logos-co/rfp/issues/112), interpreted
together with the live
[RFP-003](https://github.com/logos-co/rfp/blob/master/RFPs/RFP-003-atomic-swaps.md).
The earlier issue #61 is superseded and Ethereum is not an in-scope pair.

M7 local F5 is GREEN on exact pushed run `m7tag17a23a314a`: the current
five-of-five Risc0 guest was freshly deployed to isolated LEZ v0.2, one
post-boundary Tag17 punishment finalized, Maker and Taker independently agreed
on the canonical terminal state, and exact cleanup passed. Reproduction and
resource details are in [manual Flow 1ZB](docs/manual-user-flows.md#flow-1zb-repeat-the-actual-local-tag17-punishment-poc),
with checked evidence in
[`m7-actual-tag17-a23a314-20260804.json`](docs/evidence/m7-actual-tag17-a23a314-20260804.json).
F3/F6 still require the joined two-devnet abandonment economics and adverse
recovery races; public deployment remains deliberately deferred.

ADR [0174](docs/architecture/0174-join-tag17-to-the-funded-monero-output.md)
now defines the isolated joined-abandonment runner checkpoint. Its opt-in mode
funds the exact Stage-A Monero output before Tag17 and re-observes the same
fresh output after the terminal LEZ punishment. The implementation contract is
GREEN, while the exact pushed-commit two-devnet replay is pending. This is the
disclosed COMIT penalty fallback, not literal both-leg refund conformance; the
view-only observation is not composite-key-image unspent authority.

The application-owned Maker Tag17 recovery boundary is also GREEN under
[ADR 0163](docs/architecture/0163-supervise-maker-tag17-recovery.md): the
normal supervisor runs the real schema-3 role actor, transfers its existing
actor lock safely, submits once, then observes and terminalizes on the next
cycle without resending. ADR [0164](docs/architecture/0164-select-maker-recovery-from-durable-branch.md)
now makes the same command select only the durable Refund or Punish branch; the
real process proof covers both one-shot routes and keeps the private spend share
invocation-only. ADR [0165](docs/architecture/0165-seal-finalized-refund-signature-for-in-memory-extraction.md)
also seals finalized Tag16 on invocation-only FD 219 and verifies extraction
against the exact durable presignature without creating a plaintext scalar
handoff. ADR [0166](docs/architecture/0166-submit-maker-monero-refund-without-mining.md)
now provides the semantic Maker refund child: it reconstructs in memory and
submits once through role-correct local wallet RPCs without mining, waiting, or
retrying. ADR [0167](docs/architecture/0167-observe-maker-monero-refund-without-spend-authority.md)
now supplies its read-only restart observer: exact transaction finality is
re-proved through typed Maker-wallet and daemon RPCs without either refund
secret, and the receipt is atomically published. ADR
[0168](docs/architecture/0168-activate-maker-refund-from-finalized-evidence.md)
removes operator branch selection from exact funding and finalized Maker-side
Tag16 evidence. ADR
[0169](docs/architecture/0169-preserve-pinned-adaptor-journal-through-refund-activation.md)
keeps the schema-3 byte-pinned adaptor journal immutable until activation.
ADR
[0170](docs/architecture/0170-drive-refund-confirmations-from-durable-submission.md)
now starts the separate local confirmation driver from validated durable send
evidence instead of a transient scheduler sample. Exact run
`m7refund-e7016d8-a` proved that fix with one real Maker refund and exactly ten
local confirmation blocks, then exposed a restart rejection caused by treating
mutable SQLite representation bytes as immutable. ADR
[0171](docs/architecture/0171-validate-mutable-role-journals-semantically.md)
keeps the raw digest as provisioning provenance and revalidates complete stable
session semantics on restart. Its RED-to-GREEN physical-rewrite regression and
complete XMR actor suite pass. Fresh pushed run `m7refund-d6ebaaf-a` then
completed the joined refund through one wallet send, exactly ten local blocks,
read-only finality, workflow revision 2, completed manual Refund, terminal
scheduler state and exact cleanup. The audit found that cleanup also removed the
validated secret-free finality receipt from its private effect directory. ADR
[0172](docs/architecture/0172-retain-refund-finality-before-scoped-cleanup.md)
now publishes that receipt create-once into retained evidence before cleanup.
Exact pushed-commit run `m7refund-7cd3a9c-a` certifies the corrected boundary:
Tag16 finalized Refund, the normal Maker supervisor submitted one semantic
Monero refund, a separate driver mined exactly ten official Regtest blocks, the
observer terminalized revision 2 without spend authority, the owner action
completed, and exact cleanup left the mode-`0600`, single-link finality receipt
intact while removing every run-owned Docker resource. The checked secret-safe
packet is
[`m7-actual-maker-refund-7cd3a9c-20260805.json`](docs/evidence/m7-actual-maker-refund-7cd3a9c-20260805.json).
This run does not claim a daemon restart after submission. Reproduce it with [manual Flow
1ZF](docs/manual-user-flows.md#flow-1zf-repeat-the-joined-supervised-maker-refund).
[Manual Flow 1ZC](docs/manual-user-flows.md#flow-1zc-repeat-the-supervised-maker-tag17-recovery-checkpoint)
reproduces that networkless control-plane proof. It does not close the joined
two-devnet F3/F6 corridor.

### M6 certified local-functional Basecamp mini-app PoC

M5 is verified at the local-functional PoC boundary by tag
`m5-poc-complete`. M6 is certified by `m6-poc-complete` under
[ADR 0128](docs/architecture/0128-enter-m6-through-current-basecamp-qml.md):
first, deterministic local clickable Maker and Taker prototypes for journey
sign-off; then two current Basecamp 0.2.0 `ui_qml` packages over typed,
role-correct backend boundaries. The Maker journey covers pair and price
configuration, active monitoring, and history. The Taker journey covers offer
browsing, initiation, progress, terminal action, and ZEC shield-after-swap
guidance. Prototype state is explicitly simulated and makes no RPC, chain,
faucet, DNS, or public-network request. The implementation history and deferred
production limitations are tracked in the
[implementation plan](docs/implementation-plan.md#m6-active-work-package-maker-and-taker-basecamp-mini-apps).

The prototype gate and the Basecamp implementation are now GREEN. Commits
`149cb84`, `0141e60`, and `e3e6907` build two consumer-locked role packages,
load them in pinned Basecamp 0.2.0-RC3, fail closed without their owner service,
and exercise real role services through typed process-isolated backends. Maker
health, atomic route save, and history pass through `lez-maker-daemon`. Taker
health, offer browse, prepared initiation, exact UI replay, list, monitor, and a
post-product durable registry assertion pass through `lez-taker-service`.
The [Basecamp package guide](apps/basecamp/README.md),
[manual Flow 1X2](docs/manual-user-flows.md#flow-1x2-build-and-use-the-maker-and-taker-basecamp-packages),
[ADR 0147](docs/architecture/0147-isolate-basecamp-role-packages-over-owner-services.md),
and [machine-readable evidence](docs/evidence/m6-basecamp-role-packages-20260804.json)
record the exact builds, component/RPC schema, role flows, per-pair sequence
diagrams, atomicity arguments, external resources, flakiness, and cleanup.

The product runtime is networkless and uses no public RPC, faucet, public funds,
or public deployment. Cold Nix construction can contact immutable GitHub flake
inputs and `cache.nixos.org`; availability of those setup services is a disclosed
flakiness boundary. Terminal actual-node Claim and Refund certificates remain a
separate service/actor evidence layer. They are not represented as transactions
caused by the retained Basecamp product run. Local repository, security,
quality, architecture, package, and product gates are GREEN; exact M6 resources
were removed without global prune. No remote-Actions-green claim is made.
The first nonvisual backend prerequisite is GREEN at `8c6a7db`: strict
`maker_local_route_save_v1` atomically stores one same-route pair policy,
exact local price, and replay result in schema v22. It opens no external
resource and does not bypass the explicit sign-off gate before production QML.
ADR [0129](docs/architecture/0129-save-maker-local-route-atomically.md) records
the component, success/failure flows, and atomicity scope.

The local actor-browser proof is GREEN 6/6 through the digest-pinned,
networkless, read-only Docker runner at
`scripts/run-m6-prototype-e2e-isolated.sh`; no chain, wallet, faucet, public
RPC, or public network is involved. An unchanged-input replay on 2026-08-04
passed all six journeys in 19.34 seconds against commit `1afc0db`, then removed its exact
784 MB runner image. Owner-local RPC calls are now bounded end
to end at `eb7e147`. The strict typed Taker facade contract is GREEN at
`6161e35`: it exposes seven versioned methods, no caller-selected paths, keys,
commands, or raw evidence, and reports current Monero terminal routes honestly
as effect checkpoints rather than completed swaps. Reusable owner-only server
custody is GREEN at `270c5ef`; real authenticated Delivery health and bounded
offer listing are GREEN at `1584b76`. ADR
[0130](docs/architecture/0130-expose-a-strict-role-fixed-taker-facade.md)
records the typed boundary. ADR
[0131](docs/architecture/0131-isolate-taker-facade-on-owner-service.md)
records the separate process/socket decision and read-only flows. The actual
`lez-taker-service` is GREEN at `8826836`: an owner-private startup file
configures only pinned Delivery sources, an optional metadata-only Chat socket
probe, and the bounded offer result limit; its dedicated owner-only Unix socket
registers only health and authenticated offer listing. Commit `0ef38b0`
binds zeroizing configuration bytes to the same descriptor's device, inode,
and length, revalidates them around the read, and rejects path replacement; an
exact owner-owned single-link regular mode-0400 or mode-0600 file is accepted.
Commit `ad088f8` makes health report the exact registered method set. The
schema-v1 Taker registry foundation is GREEN through `9820400`, and
`28006dc` adds the strict prepared-ZEC context loader. Commit `5536dd0` advances that boundary from admission-only to a reproducible
real ZEC acceptance happy path. With `execute_prepared_zec: true`, a new
request still commits its exact public facts, full private authority, and
global replay result first; the service then performs the real bounded Chat
proposal/completion exchange, countersigns and no-clobber persists the
agreement, and provisions the role-fixed Taker actor before returning
`NotActivated` generation zero. Maker negotiation is durably `Completed`
and its actor is queued, but neither actor is started and no Zebra or LEZ
effect occurs.

Restart replay uses the immutable original admission time, re-admits the
current full prepared authority against the private durable row, and rejects
even a same-byte signing-key replacement with a different inode. A valid
completion receipt permits the exact retry after both the Delivery offer and
Chat endpoint are unavailable, without rewriting the agreement, actor config,
or receipt. The digest-pinned `ActorConfig` object is passed directly into
provisioning, closing its prior check/use path race. The affected real-service,
configuration, admission, restart, and legacy Chat set is GREEN 14/14, with
strict all-target Clippy, warning-fatal Rustdoc, formatting, and diff hygiene
also GREEN.

Commit `e9393cf` adds the first receipt-bound lifecycle reads. A service with a
validated prepared-ZEC catalog now truthfully registers swap list and monitor
beside health, offer list, and initiate. Both reads resolve only service-owned
prepared authority, require the exact private registry admission, and derive
actor status under the same per-swap kernel lock used by workers. After
Delivery removal and Chat outage, list and monitor reproduce the accepted swap
as `NotActivated` generation zero without rewriting its agreement, actor
configuration, or receipt. Unknown swap IDs and offer-ID substitution receive
the same fixed redacted error. The read path starts no actor and contacts no
chain, wallet, Delivery, or Chat endpoint. Pushed hardening `3307dca` captures
the receipt digest and inode at startup or acceptance, rejects same-byte inode
replacement and live actor-lock contention, and recovers after restoration. Pushed `9cf1a34` distinguishes a
never-published
receipt from disappeared accepted custody, fails the whole list on invalid local
state, and proves exact process-restart `Initiating` reads before any receipt exists.

This is negotiation and local handoff atomicity, not cross-chain completion.
The service commits admission before any transport effect; deterministic Chat
request IDs plus Maker transactions and Taker no-clobber publication make
response-loss retries converge. No chain or wallet RPC is called at this
checkpoint. Draft and signing-key bytes are revalidated before execution, but
their path-based acceptance reread still leaves a same-process replacement
hardening item; use-time inode-preserving direct-byte handoff remains required
for production readiness.

ADR [0132](docs/architecture/0132-persist-taker-initiation-admission-separately.md)
records registry atomicity and limitations; ADR
[0133](docs/architecture/0133-bind-prepared-zec-service-authority.md) records
the prepared-authority boundary; ADR
[0134](docs/architecture/0134-admit-taker-initiation-before-effects.md) records
the admission ordering; ADR
[0135](docs/architecture/0135-complete-prepared-zec-acceptance-before-response.md)
records the service-connected components, fresh/restart sequences, and exact
atomicity argument. ADR
[0136](docs/architecture/0136-project-admitted-zec-swaps-under-actor-lock.md)
records the receipt-bound read topology, fresh and restart/offline sequences,
and its lock-scoped read-atomicity argument. ADR
[0137](docs/architecture/0137-authorize-one-taker-terminal-action-before-effects.md)
records generation-fenced terminal authorization and exact effect replay.
Fresh regression `m6claim0ba41aba` now proves the service-driven
`TakerSellsLez` Claim on wholly fresh LEZ and Zebra stacks after the shared
timeout change. Exact replay preserved the same one Zcash transaction,
`0da6b4c2...d2abf`; LEZ Claim `f865903e...14d0cc` finalized in block
127; the Zcash Claim is canonical at height 107; and both actors plus the
service completed in 33.330 seconds with zero drive retries. Together with
fresh Refund certificate `m6refund8f76d87a`, this closes the nonvisual M6
Claim/Refund regression boundary. Both used deterministic local funds with no
public RPC, faucet, public funds, or public deployment. The retained Claim
packet is
[m6-zec-service-claim-regression-certificate-20260804.json](docs/evidence/m6-zec-service-claim-regression-certificate-20260804.json).
Maker and Taker Basecamp packages and the prepared actor-real UI product journey
are GREEN through the retained `e3e6907` product capture and the final
`m6-poc-complete` certification tag. Prototype sign-off was explicitly approved
on 2026-08-04.
Cross-restart receipt/state rollback
anchoring remains production hardening, not an issue-#112 PoC gate.

The current Basecamp 0.2 C++/QML toolchain is independently preflight-GREEN
for the default plugin package, core and UI `.lgx` artifacts, and exact `lgpm`
0.2.0 installation/dependency discovery. The package manager inventory resolves
`calc_ui_cpp` as `ui_qml`, depends on `calc_module`, and retains the QML view,
plugin, and process-isolated replica factory. The unsigned official tutorial
artifacts were allowed only for this local rehearsal; production signature
policy is not satisfied. The rehearsal uses a repository-consumer lock,
module-builder 0.2.0 at exact commit `92ef691e...fc9690`, package manager at
`7a1f1cf...1be584`, and digest-pinned Nix 2.35.1. It opens the public Nix and
GitHub caches only to fetch immutable inputs; it does not contact a chain,
wallet, faucet, swap service, or public deployment.

Exact Basecamp tag 0.2.0 resolves to `48b26c0d...79e5c6` and reports internal
version `0.2.0-RC3`. A fresh isolated build of its official `smoke-test` output
now certifies the Basecamp binary/runtime itself: the capability, package
manager, and package downloader modules connected over Qt Remote Objects, the
main UI loaded offscreen, and the upstream five-second smoke passed. Its exact
2,749,148,608-byte closure is recorded in the evidence packet. The later role
evidence separately loads both Maker/Taker packages and drives their real owner
services; the issue-#112 prototype gate was released on 2026-08-04. The first
disk-constrained attempt remains
recorded as a safe stop, and both attempts used exact-name isolation without
touching unrelated Docker activity. The warmed networkless product replay is
now GREEN. Realized-closure SBOM, vulnerability, signature, and license review
remain production-release gates.
The upstream broad all-output integration
evaluation has a missing-store-source defect, and five direct Logos UI sources
have no explicit license grant; LOGOS-025 records both as Logos-owned release
blockers that do not waive repository-owned M6 tests. [ADR 0146](docs/architecture/0146-pin-basecamp-builds-behind-consumer-locks.md)
records the pinned build, install versus load boundary, component flow, test
split, and exact artifact hashes. The machine-readable result is
[m6-basecamp-toolchain-preflight-20260804.json](docs/evidence/m6-basecamp-toolchain-preflight-20260804.json).

The first Refund attempts exposed a local liveness edge now fixed at the
component boundary by
[ADR 0138](docs/architecture/0138-pin-refund-snapshots-across-forward-finality.md).
At that checkpoint generated local ZEC actors moved from 10 to 30 seconds for
the bridge; ADR 0145 now sets the generated local budget to 60 seconds after
measuring multi-phase historical reads. Refund observations still return one
explicitly pinned finalized clock; a forward finalized-height change is
accepted only after bounded
ID/hash, ancestry, and repeated-pin verification. The full 26-test refund
observer suite is GREEN. At that checkpoint a fresh actual-node service Refund
certificate was still required; `m6refund8f76d87a` subsequently closed it.

A fresh clean-chain attempt then exposed a second bounded liveness mismatch:
service terminal calls stopped at 15 seconds while the invoked actor may spend
up to 30 seconds on its bridge request at that checkpoint. ADR
[0139](docs/architecture/0139-bound-service-actions-above-actor-bridges.md)
records the historical 15-second query and 40-second action split and is
superseded by ADR 0145 for the current Refund path. That effect-bearing run is
quarantined. At that checkpoint a new fresh-chain Refund replay was required.

A following fresh run returned the durable Refund commit within that budget,
but its deliberate opposite Claim check exposed transient actor availability
before the already durable terminal winner. ADR
[0140](docs/architecture/0140-prefer-durable-terminal-conflicts.md) now resolves
exact replay and any existing Claim/Refund winner before actor availability,
while SQLite admission remains the final one-winner authority. Process and race
regressions are GREEN; the effect-bearing discovery run is quarantined; fresh run
`m6refund8f76d87a` subsequently closed the certificate.

The next fresh run proved the corrected conflict response on actual local nodes:
Refund committed and the opposite Claim returned `-32017`. The runner then
stopped on its own stale assertion because it expected a scalar
`error.data`, while every service error uses the documented
`error.data.category` envelope. The runner contract now locks that envelope;
the effect-bearing run is quarantined; fresh run `m6refund8f76d87a`
subsequently closed the certificate.

ADR [0141](docs/architecture/0141-certify-terminal-refund-replay.md) closes the
remaining evidence-design gap before that run: Zebra refund confirmation now
requires exact-once membership in a canonical block, and an exact Refund replay
after both actors are terminal must leave the ordered successful LEZ submission
trace, Zebra height, and empty mempool unchanged. Both finalized refund blocks
are re-read after replay. The focused runner contract and fresh actual-node
evidence are GREEN through `m6refund8f76d87a`.

A fresh run on `8e0ed10` reached finalized LEZ Refund and Maker recovery but
exhausted the old 130-second outer runner ceiling before the Zcash refund. The
fixed 60-second protocol deadline and all per-call/finality rules remain
unchanged. That checkpoint raised the fail-safe test ceiling to 190 seconds,
covering the two
measured service-to-actor reconciliations plus terminal no-effect replay; this
adds no wait to a successful path. The effect-bearing run is quarantined.

A second fresh run on the 190-second ceiling proved that time was not the
remaining cause: the Maker reached durable Refund while the Taker service
remained in progress. The runner invoked its Taker driver through Bash command
substitution, so admitted generation, pinned LEZ start tip, finalized refund
identity, and supervisor-restart state were changed only in the child shell.
ADR [0142](docs/architecture/0142-handoff-refund-control-state-to-parent.md)
now makes that boundary explicit. The child emits a strictly validated,
monotonic control envelope; the parent restores it and alone starts Maker
recovery once. Executable regressions cover pending, finalized, exact replay,
replacement, and regression cases. The contract is GREEN. Run
`m6refund734db82a` is quarantined, the ceiling was not raised again, and at that checkpoint one
new fresh-node Refund certificate was required.

Fresh clean run `m6refund5320572a` then proved that handoff on actual nodes:
the LEZ Refund finalized, the parent started Maker recovery, and the exact
service replay evidence advanced. A subsequent replay encountered the
sidecar's bounded `moving_tip` observation guard, which the service correctly
reported as `-32010 / taker_action_execution_unavailable`; the runner
incorrectly treated that documented transient as terminal. ADR
[0143](docs/architecture/0143-retry-admitted-refund-reconciliation.md) now
persists that exact object-shaped response and continues only the already
admitted request in a later bounded corridor round. The registry winner,
generation, request ID, parent handoff, and actor journal stay unchanged; all
other response shapes fail closed. The executable contract is GREEN. The run
is quarantined; at that checkpoint one further wholly fresh Refund
certificate was required.

That fresh discovery run exposed a distinct state-projection gap rather than a
retry or timeout fault. The Maker's Zcash funding was canonical and confirmed,
but the daemon was cut over before the Maker actor durably recorded its own
lock. ADR
[0144](docs/architecture/0144-reconcile-confirmed-maker-lock-before-refund.md)
now permits one bounded observation-only Maker step while normal Maker
authority and transports are suppressed. It must reach `both_legs_locked` while
Zebra height stays unchanged and both before/after mempools stay empty. Final
acceptance validates and SHA-binds that evidence into the result. Exact-call,
deadline, live-authority, changed-tip, dirty-mempool, and evidence regressions
are GREEN. Run `m6refund7be4428a` is quarantined; at that checkpoint a fresh
Refund certificate was required before claiming both service-driven legs.

Fresh run `m6refund43f2cbca` then proved ADR 0144 on actual nodes and
finalized one LEZ Refund, but Maker and Taker refund observers contended on the
Logos LEZ v0.2 historical-account path. Each `getAccountAtBlock` rebuilds
state from genesis; measured reads at block 157 took 10.84 and 11.39 seconds.
ADR
[0145](docs/architecture/0145-serialize-refund-observation-and-layer-timeouts.md)
therefore pauses redundant Taker action reconciliation only after LEZ Refund
finality and while parent-owned Maker recovery is active. Local actor bridge,
refund-only Maker attempt, and service action budgets are now 60, 75, and 90
seconds respectively, all inside the unchanged 300-second corridor. The
executable test proves this branch makes zero Taker actor/service action calls,
preserves the strict parent handoff, and fails open to normal reconciliation
when any predicate edge changes. The run is quarantined and cannot certify
either leg. At that checkpoint a wholly fresh Refund certificate was still required.

Fresh pushed-commit run `m6refund8f76d87a` now closes that certificate.
It used new LEZ deployment/onboarding run `m6lez8f76d87a` and new Zebra
run `m6refundzec8f76d87b`, all with zero restarts and deterministic local
funds. LEZ Refund `c43df1bb...dcf5ad` finalized exactly once in block 129
before Maker's Zcash Refund `db066a94...5ab470` appeared exactly once in
canonical block 110. Maker, Taker, and service reached `refunded`; opposite
Claim was rejected; the transient log was empty; exact terminal replay changed
neither the LEZ submission trace nor Zebra height 110 and empty mempool. The
run completed in 211.530 seconds without a public RPC, faucet, public funds, or
public deployment. The retained secret-free packet is
[the M6 Refund certificate](docs/evidence/m6-zec-service-refund-certificate-20260804.json).
Fresh Claim regression `m6claim0ba41aba` is also GREEN; owner prototype
signoff, literal Basecamp UI outputs, and actor-real prepared flow are complete.
Only final certification gates, exact cleanup, push, and tag remain.

Earlier run `m6refund7be4428a` consumed the 190-second
provision-to-completion ceiling when the transient response arrived, leaving no
later bounded round. The current outer
fail-safe is therefore 300 seconds. This changes no timelock, block cadence,
per-call budget, finality rule, or success-path delay.

To repeat the proven nonvisual Claim boundary, start uniquely named isolated
LEZ v0.2 and primary-only Zebra Regtest stacks, deploy/onboard the checked LEZ
artifacts, and export their exact dynamic loopback endpoints, chain identities,
role accounts, signer files, and current deployment evidence as documented in
[Flow 1Y](docs/manual-user-flows.md#reproduce-the-service-driven-zec-claim-on-actual-local-nodes). Then run:

```sh
cargo build --locked -p lez-maker-node --bins
./scripts/test-m6-zec-service-runner-contract.sh
./scripts/run-m6-zec-taker-service-poc.sh
```

The runner cleans only the application processes it starts and retains its private
`/tmp/lez-atomic-swaps-${RUN_ID}` evidence root; the operator stops only the node
containers recorded in each run manifest. Runtime funds are deterministic
local genesis/Regtest outputs, with no public RPC, faucet, or public funds. The
pinned Bedrock process may make a best-effort UDP NTP request through
`pool.ntp.org` during stack startup, and cold Cargo/Docker acquisition may use
registries; neither is swap-chain evidence.

### M5 verified progressive application PoC — historical implementation record

This section preserves the implementation path to the verified M5 PoC. The
owner-local maker application currently provides a mode-0600
Unix-socket daemon, maker CLI, durable schema-v22
pair/price/offer/negotiation/swap history, exact local pricing, expiring
one-winner offers, daemon-owned signed bounded run-local Delivery publication,
global request replay, and restart reconciliation. Final ZEC acceptance atomically
persists the countersigned agreement, coordinator, immutable binding, encrypted
maker claim material, offer consumption, and replay result. The separate taker
CLI discovers the daemon's key-pinned signed offers and now owns its ZEC
proposal validation, local countersignature, atomic Chat completion, and
no-clobber final-wire persistence. A disjoint taker-facing Chat socket now
authenticates the exact Delivery envelope and unsigned canonical ZEC draft,
signs with the Delivery-pinned maker identity, and atomically stages the
one-winner proposal before responding. The separate taker role then validates
and countersigns that proposal, and the daemon reuses the atomic schema-v15 final
acceptance transaction with only daemon-local claim authority; delayed replay
and kill/reopen durability are process-GREEN. Exact final-wire actor
configuration and scheduling are now component-GREEN: the daemon holds a
startup-pinned Maker template and authority identities, revalidates every chain
fact, key, funder role, and preimage, durably publishes only a Maker bundle with
no-clobber rename, and commits its immutable scheduler manifest in
the same SQLite transaction as acceptance. The opt-in M5 runner now composes
that handoff with the stable LEZ/ZEC actor corridor, retains the restarted
daemon through the first confirmed Zcash lock, and then removes Chat and
Delivery before settlement. Schema v15 then offline-replays the stopped Maker
actor with unit chain ports, binds its terminal coordinator to the exact Chat
agreement, and imports a display-only projection before a fresh owner-only
daemon becomes ready. Owner `status` and `history` overlay that record while
ordinary lifecycle loads remain unchanged and cannot gain effect authority.
Schema v17 now adds the replay-safe manual-action foundation: global request-ID
binding, one open explicit `claim` or `refund` per swap, current-generation
admission, exact lease attachment, same-transaction process/action resolution,
and kernel-locked crash transfer. Focused restart, stale-generation,
wrong-owner, global-conflict, and exact-replay tests are GREEN. ADR
[0103](docs/architecture/0103-persist-replay-safe-manual-actor-actions.md)
records the component and atomicity flows. The ZEC actor now also exposes a
literal claim-only command. The supervisor validates offline status, attaches
an action only under the exact process lease and kernel lock, routes claim only
to `claim` and refund only to `recover`, and atomically completes the process
and action rows. Command-specific outcome and absorbing-phase allowlists reject
cross-action output. The actor boundary is 34 of 34 GREEN and the supervisor
integration suite is 12 of 12 GREEN. The owner-local Maker CLI now exposes
`monitor`, `claim`, and `refund` through `maker_actor_monitor_v1`,
`maker_actor_claim_v1`, and `maker_actor_refund_v1` on the existing owner Unix
socket. Every action requires the generation shown by `monitor`; exact replay
of the request ID, swap ID, action, and generation returns the original
admission, while stale generations and changed payloads fail closed. ZEC Maker
actors support claim and refund. BTC Maker actors support refund only; manual
BTC claim is rejected. No fresh actual-node run uses this supervisor path yet,
so this component does not complete M5.
Schema v19 now carries that authority into a read-only progress projection
without creating a second actor reader or worker. The strict supervisor parses
the actual pair-specific BTC and ZEC status/effect vocabularies, accepts their
real revision-zero activation state, enforces terminal phase/action/outcome
coherence, and commits validated progress in the same fenced transaction as
process and optional action resolution. A rejected effect preserves only the
last validated status. BTC effect output now obtains `next_action` from the
same actor-local derivation used by offline status. ADR
[0104](docs/architecture/0104-commit-actor-progress-with-fenced-resolution.md)
records the updated component and sequence diagrams. The monitor response
allowlists actor kind, scheduler state, generation, attempt count, validated
progress, and latest action state. It never serializes actor paths, hashes,
lease-owner identity, child PID, or private role state.
The real Taker CLI now exposes ZEC `monitor`, `claim`, and `refund` from a
post-completion owner-private acceptance receipt. The receipt pins the exact
role-fixed config bytes, Taker role, swap, state path, and agreement digest from
one identified config read. These commands need no Delivery or Chat arguments
and hold the same per-swap kernel lock as the Maker supervisor while reusing the
existing actor journals. Seven lifecycle cases cover receipt and direct-config
offline status, secret-free output, tamper and unknown-field rejection, role
rejection, command availability, lock contention, and recovery after release. ADR [0105](docs/architecture/0105-run-taker-lifecycle-from-role-state.md)
records the receipt-aware component, sequence, and atomicity diagrams;
[Flow 1K](docs/manual-user-flows.md#flow-1k-monitor-claim-or-refund-as-the-zec-taker)
gives the manual commands and external-resource boundary. The real Chat process
additionally proves no-clobber receipt publication, all seven bound fields,
inode-stable exact replay, Delivery-independent persisted completion replay, and
receipt-only monitor after both application transports are removed. The composed
application runner now uses that acceptance-provisioned Taker
config and state, validates it against the queued Maker bundle before activation,
pins receipt identity and SHA-256 around every receipt-based monitor or claim
invocation, and permits raw
drive only for exact non-claim phase/action pairs. The happy-path Zcash
follow-up routes only through `lez-taker claim --receipt`; accepted-swap monitor
and claim traces bind the swap and receipt digest. The focused runner contract
is GREEN, while a fresh isolated actual-node execution and receipt-bound refund
remain before the final user journey.

The BTC application path now has a reproducible pre-effect process PoC. The real
Maker CLI publishes a signed Delivery offer, the real Taker CLI discovers it,
and a separate real maker daemon runs BTC Chat proposal and completion. A
Delivery-only daemon can publish before any Chat, signing, provisioning, or
actor authority exists. The Taker planning command authenticates the envelope
and derives its reservation-bound swap ID without private material; the daemon
then restarts with only the selected BTC authority. The canonical draft is
exported from the finalized actual-node fixture under its exact Bitcoin policy
using owner-private no-clobber storage, so the forthcoming node splice does not
retype executable terms in shell. The bounded canonical unsigned draft runs the
same executable Bitcoin, LEZ, role,
and recovery checks as the final agreement and must match the exact local
Bitcoin genesis and confirmation policy before signing. The daemon supplies the
Maker Schnorr signature; the Taker validates and countersigns it; schema 19
atomically commits the exact dual-signed wire, agreement-derived coordinator,
consumed offer, immutable Bitcoin Maker actor, and replay result. The two
process roles publish only their own role-fixed actor bundles through private
staging and no-replace rename. The Taker persists the final agreement before
completion, publishes its receipt only after durable Maker completion, and can
repeat exact completion and monitor offline after Delivery is removed without
replacing the final agreement, actor config, or their inodes. The focused
[Flow 1N](docs/manual-user-flows.md#flow-1n-repeat-the-btc-application-process-poc)
passes 1 of 1 in 0.87 seconds and uses no chain RPC, node, Docker service,
faucet, DNS lookup, network, or public funds. It proves the application and
crash-safe pre-effect handoff.

The opt-in BTC application runner now composes that same handoff into the actual
M3 node lifecycle: schema-6 source configs bind the exact finalized agreement,
the real daemon/Taker publish role-only no-clobber bundles before activation,
all actor commands select those accepted bundles, Delivery and both application
sockets are removed before chain effects, and later scan-window changes cannot
replace runtime authority. Exact pushed run
`m5-btc-app-20260730-992b6d4-e` completed both role actors at revision 4 against
Bitcoin Core 31.1 Regtest and LEZ v0.2. It retained exactly two Bitcoin effects
and three LEZ effects, submitted nothing on terminal replay, and removed only
its exact run-scoped resources. The checked
[BTC application evidence packet](docs/evidence/m5-btc-application-corridor-20260730.json)
records the pushed-clean provenance, exact effect IDs, local-node versions,
runtime external-resource boundary, timings, and cleanup. The XMR application
corridor is also clean-certified below. At that checkpoint, the all-pair and
unavailable-route control-plane closures were still open; M5 was not tagged.

The XMR application path has now entered its first schema-v20 store slice. An
exact canonical dual-signed Stage-A agreement can reserve one authenticated
Delivery offer only when its domain-separated swap ID, role identities,
piconero amount, LEZ amount, direction, quote, and acceptance window all match.
The agreement is intentionally non-executable: the same transaction creates no
coordinator, actor, effect journal, or chain call. Exact replay rechecks the
complete offer and negotiation rows; malformed wires, wrong signatures,
direction, identity, or quote leave no write; concurrent reservations have one
winner; and forced final-write failure rolls everything back. Three focused
XMR tests and the complete 148-test store suite pass with warning-fatal Clippy
and Rustdoc. [ADR 0111](docs/architecture/0111-reserve-dual-signed-xmr-stage-a-before-activation.md)
records the component, reservation, replay, and atomicity diagrams.

Schema v21 now completes the Stage-B store boundary. Only canonical
countersigned Stage B can derive the lowercase-hex Monero coordinator and its
signed LEZ/Monero confirmation plus recovery policy. One immediate transaction
creates the coordinator, changes the negotiation to activated, consumes the
reserved offer, registers one immutable Monero Maker actor, and records the
global replay result. Forced failure restores the Stage-A-only reservation;
restart and exact replay retain one coordinator and actor. Acceptance is valid
through the signed whole-second Maker funding cutoff, fails afterward, and does
not incorrectly reapply the already-linearized advertisement TTL. Replay binds
the acceptance time and rechecks the canonical Stage A, complete offer route and
quote, activation, coordinator, actor, and mutation rows. Schema 20 to 21 keeps
existing process, manual-action, and progress rows while widening their actor
kind checks. Maker-node now admits Monero execution only through the exact
`xmr-maker-actor` schema-v2 pre-effect ABI described below. [ADR 0112](docs/architecture/0112-activate-xmr-stage-b-atomically.md)
records the component, commit/replay sequence, and atomicity argument. Role-only
process handoff and semantic pre-effect scheduler execution are process-GREEN;
the clean isolated Monero plus LEZ corridor described below now certifies their
composition through the existing one-shot chain-effect owner.

The M5 XMR application path now has a process-GREEN real-process pre-effect checkpoint. It runs the actual Maker CLI, `lez-maker-daemon`, and Taker CLI around role-generated canonical Stage A/B material. Stage A advances only to reserved revision 2; Stage B is the sole transaction that creates the coordinator, consumes the offer, registers one Maker-only Monero actor, and records revision 3 replay. The Taker publishes only its own no-clobber actor bundle and acceptance receipt. A crossed reservation must leave the active offer at revision 1 with no negotiation, coordinator, actor, or public effect. After the exact Delivery advertisement is removed and the daemon is restarted, the durable Taker actor bypasses discovery and exact replay must preserve every captured actor/receipt byte and inode.

The legacy XMR acceptance receipt also drives a real, receipt-only
`lez-taker monitor` after Delivery and Chat are gone. The command pins the
receipt and manifest, takes the same per-swap kernel lock as the actor worker,
and validates the complete Taker application authority under that lock before
emitting a fixed, secret-free application status. It starts no node, opens no
RPC, performs no chain effect, and does not infer current or enduring chain
progress. Legacy receipt-v1 `claim` and `refund` remain explicitly unsupported. The
manual command, exact JSON, stable failures, atomicity boundary, and external-
resource declaration are in
[Flow 1T](docs/manual-user-flows.md#flow-1t-monitor-an-accepted-xmr-application-as-the-taker).
Inherited ABA hardening for paths reopened while validating the authority
remains production work.

The follow-on XMR receipt-v2 checkpoint adds a replay-safe, effect-shaped
authority handoff and one process-level Taker Tag14 invocation. During an otherwise identical
accepted-XMR Taker replay, these four flags are all-or-none:
`--xmr-effect-authority-file`, `--xmr-effect-manifest-file`,
`--xmr-workflow-journal`, and `--xmr-run-id`; the existing
`--xmr-acceptance-receipt` output becomes a new schema-v2 receipt. The writer
publishes a no-clobber schema-v3 manifest and initialized role-local workflow
journal, and the selector later revalidates the receipt, schema-v3 manifest,
immutable effect authority, workflow identity, and legacy application
authority under both owner locks. Legacy receipt v1 remains monitor-only.

`lez-taker monitor --receipt /absolute/private/acceptance-receipt-v2.json`
returns schema 2 with the bound run ID and `effect_authority:"validated"`.
It reads private authority files but contacts neither chain. The authority
contains literal-loopback LEZ and Monero URLs, credential-file paths,
runtime/capability paths, and role-fixed program/hash/ABI slots; at this
checkpoint those values are canonical syntax and identity commitments only.
The monitor does not open the URLs, read RPC credentials, invoke tools, or
check the at-use executable/capability hashes.

`lez-taker claim --receipt /absolute/private/acceptance-receipt-v2.json` now
validates the schema-v3 execution under separate actor and workflow locks,
prepares the exact Taker `AuthorizeLezTag14` plan, wins one durable workflow
CAS, and invokes and reaps one hash-pinned child with FDs 197 through 210. A
successful marker child returns schema 3 state `invoked_unreconciled` with
`chain_effect_finalized:false` and leaves the workflow `Started`.

The second claim starts no sending child. It hash-pins the role-fixed finalized
observer, rederives and exact-compares the original nonzero sending-plan
identity, accepts observation only from `Started` or `Unknown`, and parses one
bounded step-exact result. Finalized marker evidence is reconciled atomically as
`lez_finalized_event`, so the command returns `complete` with
`chain_effect_finalized:true`. The third claim reads durable `Succeeded` and
returns `complete` without starting either sender or observer. Observer spawn,
timeout, exit, output, parse, digest, or evidence failure leaves the journal
unchanged. The result cannot choose its source: role plus step derive it
locally. `Prepared` and `Succeeded` cannot start an observer. Sending
ambiguity still makes `Unknown` sticky and never rearms; the losing receipt-v2
`refund` branch fails closed.

This is process-component evidence only. The sender and finalized classifier
are fixed local marker programs, not semantic Tag14 or chain-observer workers.
They open no RPC, construct or submit no LEZ transaction, and prove no on-chain
finality. The exact Maker-daemon/Delivery/Chat black-box test is GREEN 1 of 1
in 133.16 seconds; the focused effect-route suite is GREEN 5 of 5, and strict
Clippy plus warning-fatal Rustdoc are GREEN.
Both transports are removed and the daemon is stopped before the lifecycle
action. It uses deterministic private files and no node, Docker service,
faucet, DNS, public RPC, peer, or funds. Exact flags, outputs, private-file
inventory, and cold-build, hashing, lock-contention, and host-scheduling
flakiness notes are maintained in
[Flow 1T](docs/manual-user-flows.md#flow-1t-monitor-an-accepted-xmr-application-as-the-taker).

M7 now extends that generic custody boundary with the first real semantic
sender. The no-argument `xmr-reference-tag16` child receives the exact runtime,
capability, Stage A/B, view key, canonical plan, and Taker share on sealed FDs;
requires its live durable refund presignature to equal Stage B; adapts and
verifies the final signature in memory; and supports a prepare-only preflight
before the parent consumes its one-attempt CAS. Rejected preparation cannot
complete, submit, publish evidence, or change workflow state. Successful
preflight is followed by repinning, CAS, and one authenticated local prepare,
complete, and exact submission; restart states skip preflight and cannot
rearm. Tag16 process tests are GREEN 6 of 6, effect routing is GREEN 7 of 7,
and the literal receipt-v2 refund journey is GREEN 1 of 1 in 106.26 seconds.
These tests include journal-drift-before-RPC, least-privilege FD 218, and
one-preflight/one-invocation/restart-reconciliation checks. They use an
authenticated in-process
loopback sidecar, sealed memfds, temporary SQLite, and deterministic material.
They use no Docker, external node, public RPC, DNS, faucet, public funds, or
deployment, and therefore do not claim actual-node finality or the subsequent
Maker Monero recovery. Reproduction and resource/flakiness details are in
[Flow 1V](docs/manual-user-flows.md#flow-1v-repeat-the-role-correct-xmr-refund-continuation-checkpoint),
with components, sequence, and conditional atomicity in
[ADR 0154](docs/architecture/0154-derive-tag16-in-the-sealed-effect-child.md).

M7 also composes the existing safe Tag14 release service through that
supervisor boundary. Its no-argument mode accepts only a typed invocation,
release-only capability, and journal protection key on fully sealed FDs 220
through 222, plus the already-open owner-private journal directory on FD 223.
Mutable or unsealed inputs fail before journal or RPC use; two fresh process
invocations produce exactly one accepted release and an observe-only restart.
The service still consumes the separately prepared encrypted journal that binds
finalized LEZ Fund, the exact confirmed Monero output, and authenticated wallet
topology, and still rechecks finalized time after its publication CAS. No
Docker, node, public RPC, faucet, or funds participate in this component proof.
Reproduction, components, sequence, and the conditional-atomicity limit are in
[ADR 0155](docs/architecture/0155-invoke-the-xmr-release-worker-through-sealed-descriptors.md).

Receipt authority is now versioned before that worker can be selected. Schema
1 remains the existing marker-only profile. Schema 2 is Taker-only and requires
the release-worker v2 ABI, a distinct release-only sidecar/capability, local or
exact-pinned finalized indexer, encrypted-journal directory, protection-key
file, and key identifier as one canonical authority. Downgrade, omission,
public-local endpoints, capability/path aliasing, and a changed public origin
fail closed. See
[ADR 0156](docs/architecture/0156-version-the-tag14-release-authority.md).

The schema-v2 route is now connected to `lez-taker claim`. A non-sending
preflight opens, decrypts, authenticates, and exact-binds the release journal
before the parent consumes its workflow CAS; it makes zero indexer or sidecar
calls and accepts only Prepared or already-Admitted state. The parent then
repins and derives the canonical release invocation from the exact validated
Stage A/B, private view key, runtime, run, and release profile. Only FDs
220..223 reach the worker; general LEZ/Monero credentials, application-private
material, and the spend share are absent. Real worker process proof, the
eight-case route suite, and the literal claim flow with rejected-preflight
retry/invoke/observe/Complete are GREEN. A joined actual-node CLI replay,
semantic finalized observer, and subsequent Monero sweep remain open. See
[ADR 0157](docs/architecture/0157-preflight-and-compose-tag14-release.md).

That first checkpoint is deliberately zero-effect: it starts no Monero or LEZ
node, opens no chain RPC, and uses no Docker service, faucet, DNS, network, or
funds. [Flow 1P](docs/manual-user-flows.md#flow-1p-repeat-the-xmr-role-process-pre-effect-checkpoint)
reproduces its real Maker/daemon/Taker handoff; the exact black-box proof passed
1 of 1 in 307.71 seconds.

The follow-on schema-v2 semantic-supervisor checkpoint is also GREEN. The
supervisor runs the real installed `xmr-maker-actor` from its digest-pinned
single-link executable, supplies only fully sealed config FD 196, and requires
the exact `xmr-maker-actor` program identity,
`lez_maker_xmr_pre_effect_v1` ABI, and nine-key status object. Execution-time
validation rehashes and semantically revalidates Stage A, Stage B, both public
packets, the Maker private manifest/view-key authority, and an immutable
snapshot of the external role journal. The only accepted result is typed
`Blocked` with `chain_effect_executed:false` and
`xmr_chain_effects_not_yet_composed`; it invokes no activate, drive, claim, or
refund effect and waits at least 60 seconds before another authority
observation. The exact real-process supervisor proof passed 1 of 1 in 79.22
seconds. The optimized complete authority replay took 29.02 seconds, down from
194.75 seconds, without changing protocol or validation semantics.
[Flow 1Q](docs/manual-user-flows.md#flow-1q-repeat-the-xmr-schema-v2-semantic-supervisor-checkpoint)
gives the focused reproduction. It uses no chain node, RPC, Docker service,
faucet, DNS, network, or funds and therefore does not certify a swap or chain
effect by itself. The separately composed isolated official Monero 0.18.5.1
Regtest plus LEZ v0.2 application corridor is clean-certified below.

The opt-in XMR application runner is now **CLEAN LOCAL HAPPY-PATH GREEN**. Four
pre-runtime attempts repaired stale graphs,
an artifact hash, and the Risc0 handoff. Fifth run
`m5-xmr-app-20260730-58e1ee1-e` reached real local nodes and exposed the
harness's incorrect rejection of intended consumed-offer reconciliation before
tag 13. Exact pushed-tree run `m5-xmr-app-20260730-da9be26-f` then completed
the corrected flow: signed Delivery, canonical Stage A/B, real Maker/Taker
acceptance, typed no-effect `Blocked`, authenticated restart reconciliation,
empty Delivery outage, inode/hash-stable replay, synchronous application
cutoff, finalized tag 13/14/15, adaptor extraction, confirmed Monero sweep, and
cross-chain binding. LEZ Claim finalized in block 141 with finalized tip 146;
the 1 XMR lock swept 998191600000 piconero after a 1808400000-piconero fee and
reached 10 confirmations at Monero tip 130. The source path returned zero and
all exact resources are absent, but cleanup certification failed because one
exact cleanup command returned nonzero. The existing evidence schema did not
identify which command, so this run is not the clean certifying replay and its
one-shot ID must never be reused. Cleanup evidence is now versioned and records
stable failure reason codes without relaxing fail-closed behavior. Exact
pushed-tree run `m5-xmr-app-20260730-9067ba3-g` then repeated the complete
functional corridor. LEZ Claim finalized in block 140 at tip 143; the same
1 XMR amount swept 998191600000 piconero after a 1808400000-piconero fee and
reached 10 confirmations at Monero tip 130. Source status was zero, binding
completed, every exact resource was absent, and the foreign sentinel survived.
Schema v2 isolated cleanup failure to exactly three
`ephemeral_path_boundary_failed` reasons. All three were nested directories
under the exact run-owned private namespace: the guard admitted the namespace
itself but not its children. Commit `fb4e279` fixes that boundary by
canonicalizing and admitting only descendants of the run-owned private root;
focused tests retain rejection of traversal, symlinks, and foreign paths.
Correction recorded 2026-07-30: a post-run role audit proved that historical run H used the provisioner as funder, the Taker RPC as the shared-wallet process, and the Maker address as sweep destination. H remains genuine finalized-chain, key-reconstruction, sweep, binding, and cleanup evidence, but it does not certify the intended user-economic roles. The runner now enforces Maker funding, a neutral provisioner shared-wallet process, Maker-mined claim confirmations, and a Taker destination; fresh exact-commit replay evidence is pending.

Exact pushed-tree run `m5-xmr-app-20260730-2c6aec1-h` then repeated the entire
corridor from commit `2c6aec1` and passed cleanup schema v2. Its application
cutoff, finalized tag 13/14/15, extraction, sweep, and binding all completed for
swap `9d627d18...abfeb7c`. Claim transaction `05cb9052...349fce` was included
at LEZ height 139 and observed at finalized tip 142. Monero sweep transaction
`37930570...1603c8` received 998191600000 of 1000000000000 funded piconero
after a 1808400000-piconero fee and reached 10 confirmations at tip 130.
Cleanup returned `passed`: source status zero, every exact resource/process/port
absent, the foreign sentinel and tag-13 latch preserved, no foreign target, no
broad cleanup, and no failure reason. The binding claims conditional
successful-claim atomicity, not a distributed transaction or future-reorg
immunity. Runtime is entirely local:
official Monero 0.18.5.1 Regtest and LEZ v0.2 with deterministic
genesis/Regtest funds and ephemeral loopback RPCs; no public RPC, faucet, peer,
or public funds are used.
[Flow 1R](docs/manual-user-flows.md#flow-1r-run-the-xmr-application-to-chain-corridor)
documents the exact operator command, resources, evidence, cleanup, and
non-retry boundary. A fresh RFP/issue acceptance audit confirms the literal M5 score remains 3
of 7. Explicit selected-route disable is now process-GREEN: disabled Zcash quote
and publication fail before price or Delivery I/O, an enabled Bitcoin quote is
unaffected across restart, and revisioned Zcash re-enable restores quotes. Full
automatic route health is now process-GREEN: bounded hash-pinned semantic
commands run periodically off the RPC loop, unavailable routes reject new work,
active offers withdraw, reserved negotiations survive, and another route stays
available. [Flow 1Z](docs/manual-user-flows.md#flow-1z-configure-and-verify-automatic-maker-route-health)
documents configuration, local proof, RPC resources, and flakiness. F1/R3 are
now actual-node GREEN through the same operator-visible control.

`scripts/run-m7-unaffected-pair-outage-poc.sh` starts a unique real Bitcoin Core
31.1 Regtest service, verifies its genesis, stops only that labelled container,
then drives the ordinary actual-node Zcash Maker/Taker corridor with semantic
genesis-bound health checks for both routes. Clean pushed run
`m7outage-2c63218-a` rejected the stopped Bitcoin route before and after Maker
restart while the independent Zebra/LEZ claim journey completed both roles in
36.920 seconds with zero same-run retries. Confirmed Zcash funding preceded the
revealing LEZ claim, which preceded the confirmed Zcash claim. The secret-free
certificate is
`docs/evidence/m7-unaffected-pair-outage-2c63218-20260804.json`; no public RPC,
faucet, public funds, peer, or runtime external resource participated.
Fixed packaged-system-service start/stop and receipt-bound
XMR Taker monitoring are now GREEN. XMR Taker claim/refund effect composition
and actual-application concurrency remain. The tag-16 sidecar checkpoint is now
component-GREEN: authenticated Taker preparation and completion feed only the
transaction-derived one-attempt submission identity; an ambiguous send remains
unknown across restart without resend; and Taker-exact plus Maker-discovery
classification require canonical finalized refund facts in
`[refund_at, punish_at)`. That lower component checkpoint used controlled local fixtures; the exact actual-node replay below closes Maker extraction, reconstructed-key sweep, and binding. ADR 0158 and run `m7tag17a23a314a` make Tag-17 preparation, one-attempt release, and two-role finalized classification actual-node GREEN under the isolated Maker claimant key; joined abandonment economics and adverse races remain open. The next role-correct component is now GREEN: the real
Taker process cryptographically verifies and publishes tag 16 through the
transaction-derived one-attempt identity; the Maker accepts only canonical
finalized discovery into the precommitted refund session; and one sweep engine
selects the opposite reconstruction, destination, and confirmation roles for
claim versus refund. The opt-in application runner now composes that same
refund path through isolated local LEZ v0.2 and Monero Regtest services and an
owner-private binder that cross-checks finalized tag 16, Maker extraction, the
Maker-directed Monero receipt, honest refund roles, and exact fee accounting.
This runner and binder are now actual-node GREEN on exact pushed commit
`45924ca8ed2f76cdcb5befad25b54c5ccf37dbea`. Clean run
`m5xmrrefund45924caa` completed the role-correct refund branch through both
local devnets and exact cleanup.
The first clean attempt, `m5xmrrefund8c10cd7a`, reached finalized tag 13 and
verified Maker-funded Monero output, then repeatedly classified one fixed LEZ
block. Later evidence proved Bedrock finality was progressing: the classifier
was correctly reporting its requested discovery window, not the current tip.
The correction in
[ADR 0123](docs/architecture/0123-drive-the-local-finalized-clock-with-one-sealed-effect.md)
permits exactly one local-only, activated-terms-sealed Taker-to-Maker transfer
of one native unit through the authenticated Taker sidecar. It uses one durable
reservation, canonical transaction-derived one-attempt submission, and
read-only before/after verification; escrow metadata and custody must remain
byte-identical. Authenticated `observe_finalized_clock` then polls the official
genesis-bound finalized tip without submission, and the Maker classifier scans
exactly that returned block to decide the signed refund window. Fresh request
IDs are bounded SHA-256 derivations and the driver waits at most 60 seconds.
This path is GREEN across strict protocol, client, live-runtime, driver, and
runner contracts plus the complete root and sidecar test suites, warning-fatal
Clippy and Rustdoc, repository CI/security policy, Docker isolation, and
dependency policy. All ten repository lockfiles containing `ruint` now resolve fixed `1.20.0` for `RUSTSEC-2026-0220`, with no waiver. The remediated Risc0 rebuild passes all five recursive cases at ELF `ade4af84...bbcee` and ImageID `b7f87278...b0433`; [`docs/evidence/m5-ruint-remediation-20260731.json`](docs/evidence/m5-ruint-remediation-20260731.json) records the 13-graph audit and isolated artifact proof. Official upstream LEZ remains separately tracked as `LOGOS-023`.
The v0.1.2 compatibility artifact was also rebuilt with Risc0 3.0.5 and the
digest-pinned Rust builder `r0.1.94.1` because fixed `ruint 1.20.0` requires
Rust 1.90 or newer. Exact run `m5-ruint-v012-final-20260731` reproduced ELF
`fe8ec116...c739f7` and ImageID `5421868e...add62`, passed six ordinary tests,
two actual deployment/native-plus-two-token lifecycle tests, and one recursive
cost case. Its initial top-level exit was `1` only because the former
byte-identical cost comparison included volatile cycle classifications after
every functional and budget gate had passed. The CI-required stable policy now
accepts the exact generated output only when immutable artifact identity,
operation order/session topology, total cycles, and budgets match and every
classification sum and budget check remains valid.

Removing the approved local `.e2e` run cache reduced this build's Docker context
from 6.37 GB to about 64 KB. That is a temporary, non-durable iteration saving:
the pinned Risc0 Dockerfile-specific ignore file overrides the repository root
`.dockerignore`, so future retained `.e2e` data can enlarge the context again.
Pushed replay `m5xmrrefund827a5d4a` retained the request-ID RED before any clock
effect. Run `m5xmrrefund842610ca` then admitted exactly one clock effect, proved
accounting and unchanged escrow state, and obtained ten Bedrock descendants
within about 16 seconds before exposing the fixed-window observation bug. The
current-finalized-tip TDD correction supersedes that diagnosis; neither partial
run is completion evidence. Clean run `m5xmrrefund45924caa` then submitted one
sealed clock transaction at height 192, observed finalized height 188 advance
to 192 in 107 read-only attempts, preserved escrow metadata and custody,
finalized tag 16 in block 198, and bound Maker extraction to the confirmed
Maker-directed Monero sweep `252b922e...d4caf`. Cleanup schema v2 passed with
source exit zero, all exact resources absent, and no broad or foreign cleanup.
The retained secret-safe packet is
[`docs/evidence/m5-xmr-application-refund-corridor-20260731.json`](docs/evidence/m5-xmr-application-refund-corridor-20260731.json).
[Flow 1U](docs/manual-user-flows.md#flow-1u-repeat-the-tag-16-one-attempt-component-checkpoint)
and [Flow 1V](docs/manual-user-flows.md#flow-1v-repeat-the-role-correct-xmr-refund-continuation-checkpoint)
reproduce the lower component boundaries; Flow 1W gives the exact clean replay
command and resource/flakiness boundary. At that checkpoint, literal M5 was 3
of 7: the refund proof closed a prerequisite, while daemon-owned accepted-
application effects, complete Maker/Taker lifecycle surfaces, and concurrent
accepted-application isolation remained. The current score and ETA are updated
by the ZEC evidence below.


The persistent coordinator now runs 1 to 32 independent actor workers with one
SQLite connection each, one shared daemon lease identity, per-row CAS and
generation fences, per-swap kernel locks, and joined cancellation. A real-daemon
two-swap journey proves one terminal actor completes while a disjoint actor is
simultaneously live and leased; releasing the peer to a typed failure changes
only it to Backoff. Restart preserves both exact manifests and performs no new
invocation. The deterministic journey passed 10 of 10 repetitions in 0.49 to
0.54 seconds. It uses only owner-private files, SQLite, Unix sockets, and local
child processes: no chain RPC, Docker service, faucet, DNS, public network, or
funds participate. ADR 0116 records the worker, sequence, and isolation model.
Distinct accepted application agreements, escrows, deadlines, and actual-chain
overlap remain before full R5 closure.

Exact pushed-tree run `m5appee8424520260724a` completed the earlier direct-actor local application
corridor in
33.400 protocol seconds with no retry. Exact packet-bearing replay
`m5app6c3bbbe20260724a` then repeated it from pushed commit `6c3bbbe` in 27.860
seconds and 56 drive rounds with zero retry: both actors reached revision 4,
Zebra advanced exactly 104 to 107, Chat and Delivery stayed absent after the
first lock, a fresh daemon reported `Completed`, exact scoped cleanup passed,
and no public RPC or faucet participated. See the
[terminal-projection evidence packet](docs/evidence/m5-zec-application-terminal-projection-20260724.json)
and the preceding
[corridor checkpoint](docs/evidence/m5-zec-application-corridor-20260724.json).
The progressive local ZEC application PoC gate is certified for that earlier
corridor.

Current daemon-supervised run `m5zec432dapp1` replayed exact pushed commit
`432d1f7dabbb573b9642794155066e37ee95e75d` against a fresh LEZ v0.2
deployment, fresh Zebra 5.2.0 Regtest state, and fresh Maker/Taker identities.
Both role actors reached revision 4 `completed` in 25.030 protocol seconds; the
Maker scheduler ended `terminal` with no child, the daemon supervisor was the
only Maker effect authority, and the Taker claim was acceptance-receipt bound.
Delivery, Chat, and the owner socket remained absent after the first confirmed
lock. A fresh owner daemon projected the terminal state without either chain
RPC. Exact cleanup removed the four containers, two networks, two tagged
images, private run roots, ports, and processes without a global prune. No
public RPC, faucet, peer, or public funds participated.

That checkpoint closed the daemon-owned accepted-application output and
raised literal M5 to 4 of 7. The remaining outputs at that point were complete
Maker lifecycle control for every supported pair, complete Taker lifecycle control for every supported pair, and accepted-
application actual-chain coordinator concurrency/restart isolation including
proof that unavailable XMR does not stall BTC/ZEC. See the
[daemon-supervisor certification packet](docs/evidence/m5-zec-daemon-supervisor-certification-20260731.json).

The current XMR schema-v3 execution boundary now selects only the six
role-fixed sending slots and pins the exact executable, runtime, ten secrets,
actor lock, workflow lock, and FD 197..210 child map before consuming the
workflow-v2 one-attempt CAS. Only the Prepared winner receives a Command;
Started/Unknown prepare only a role-fixed observer; Succeeded returns Complete
without either process. A genuine signed Stage-A/B Taker fixture proves one
Tag14 marker invocation, restart-only finalized-marker observation with the
same sending-plan digest, durable evidence-bound reconciliation, and a
process-free third call. Strict bounded output parsing rejects source injection
and step/digest drift without journal mutation. This checkpoint uses temporary
local files/processes only: no RPC listener, node, Docker service, faucet, DNS,
public network, funds, or finality wait participates, so it proves process
orchestration rather than semantic Tag14 chain behavior. At that process checkpoint M5 remained 4 of 7.

Those historical runs are not evidence of the current receipt-bound claim route.
The current M5 working tree also contains an intervention-assisted actual-node
one-leg recovery checkpoint. In isolated run
`m5fresh-a390dd8-20260728a-app3`, the Taker refunded its only locked LEZ leg
once after expiry; transaction `3a7ffaa5...16e25` occurs once in finalized block
608, the by-ID and by-hash indexer reads agree, custody is zero at that block,
and both role-fixed actors finish at `Refunded` revision 2. ADR
[0102](docs/architecture/0102-observe-refunds-from-finalized-window-prefixes.md)
documents why finalized-prefix discovery preserves atomicity while a partial
absence remains non-terminal. The original observation window ended before the
refund block, so that historical run required manual actor-window rotation and
retirement of an old active bridge-journal row. Current code removes that gap:
a restart-safe SQLite cursor advances only validated fully covered pages, keeps
partial/ambiguous/typed-error polls on the exact page, and restores the active
page despite unchanged actor config. Both owner and counterparty paths pass a
RED-GREEN reopen test, but the retained actual-node evidence remains
intervention-assisted until a fresh recovery replay. At that recovery
checkpoint literal M5 was 4 of 7 and incomplete. The remaining accepted outputs
at that point were complete supported-pair Maker lifecycle, complete supported-pair Taker lifecycle, and actual-chain
accepted-application coordinator concurrency/restart/unavailable-XMR
isolation. Clean sidecar builds
should set the documented
absolute `RAPIDSNARK_LIB_DIR` only after verifying the four pinned v0.0.8
library hashes and should use Cargo offline rather than the upstream download
fallback.

The real `zec_chat_process` boundary now also proves lost-completion-response
recovery: it fully observes a successful durable Maker completion through a
bounded local Unix HTTP proxy, drops the response before the Taker sees it,
verifies that no acceptance receipt exists, and exact-retries without replacing
the agreement or role config before publishing the first receipt. This test
uses only temporary local process, filesystem, Unix socket, and SQLite resources;
there is no chain RPC, Docker service, faucet, DNS, or public network.

Full swap-store, maker-process, strict Clippy, and Rustdoc gates are GREEN. The
literal pinned
coordinator fuzz target, seven-seed corpus, bounded CI smoke, and separate
dependency audit are also GREEN locally; reproduce them with
`./scripts/run-m5-coordinator-fuzz-smoke.sh`. The same maker daemon now has a
hardened `Type=notify` systemd package, encrypted runtime credentials, typed
health, SIGTERM cleanup, a process-lifetime database lease, an actual
crash/restart rehearsal, and a bounded future Logos Core lifecycle contract.
Use [Flow 1D](docs/manual-user-flows.md#flow-1d-install-and-rehearse-the-maker-systemd-service)
to repeat it. The provisional versioned Logos price C-API and one-shot worker
are actual-C fixture GREEN. Its parent adapter also bounds time/output, reaps an
aborted or hung exact child, and pins owner/path/mode/link/module-hash inputs.
Schema v15 commits per-module revision high-water, policy revalidation, the
immutable signed offer snapshot, and request replay together; exact replay
returns before a source call. The real daemon now selects the durable route's
local or Logos source without fallback, invokes the bounded worker outside the
SQLite mutex, and signs the exact module SHA, revision, observation, and ratio
into Delivery. A black-box daemon/maker/taker journey proves replay can recreate
a deleted advertisement after the module fails without contacting it, while a
fresh request fails closed and restart discovery reconciles from SQLite. See
[Flow 1E](docs/manual-user-flows.md#flow-1e-repeat-the-logos-price-daemon-and-signed-offer-path).
The real ZEC process journey now also makes Delivery unavailable after startup,
reports `ready: true` plus explicit degraded dependency state, repairs and
exact-replays the one durable offer, removes Chat after proposal staging, proves
no final agreement appears, and completes exactly once after daemon restart.
Reserved or consumed unexpired envelopes remain projected only so the winning
deterministic retry can resume. Completed maker/taker lifecycle commands,
other-pair application composition, and hardening remain.

The short-TTL ZEC process proof now also waits until the consumed Delivery
envelope is stale. The daemon remains `ready: true` but correctly reports
`degraded: true` and Delivery `unavailable` until startup/operator
reconciliation removes that stale projection; SQLite, not the mailbox, remains
the committed swap authority.

Schema v16 now also persists pair-bound, immutable maker-actor scheduling
metadata with stable due order, owner/generation fencing, restart enumeration,
and peer-isolated backoff. Time alone cannot steal a live lease. Registration
and lease races plus the feature-gated marker helper are unit-GREEN. Secure
per-state-database kernel locks now survive child exec, exclude live children,
and authorize atomic owner/generation recovery without exposing an unleased row.
Config and program files are secure-opened, identity/mode/link/hash checked, and
copied to write-sealed child FDs 196/197; path replacement cannot change the
bytes read or executed. State paths are rebound as the same private inode or the
same absence immediately before command construction, and lock FD 198 remains
the process-liveness fence. The production Chat completion path now validates a
Maker-only source template and pinned actor program, publishes an owner-private
agreement/config bundle through `RENAME_NOREPLACE`, reloads its role/swap/state/
agreement/authority binding, and only then atomically accepts the swap with one
immutable queued actor manifest. Forced late failure rolls back the offer, swap,
agreement, binding, encrypted claim material, actor row, and replay record
together. Exact/delayed replay preserves one row and the same config inode;
changed or partial artifacts conflict. Files and containing directories are
synced bottom-up before acceptance, and exact replay repeats the durability
barrier. Six focused tests cover Maker-only creation/replay, Taker rejection,
corrupt collision, unsafe mutable artifacts, and concurrent publishers. A
filesystem bundle left by a database rollback is inert without its scheduler
row and is exact-replayable. Exact committed completion replay is now
independent of the current wall clock: before live agreement parsing or actor
provisioning, the daemon verifies the exact request, offer revision,
reservation, final-wire digest, protected preimage digest, completed
negotiation bytes, swap ID, and immutable scheduled actor row. The taker first
persists the countersigned agreement; a rerun validates that private agreement,
the executable unsigned draft, pinned Maker, local Taker role/key, and amount,
then retries only completion without Delivery discovery or proposal replay.
The real CLI/process proof uses a three-second offer TTL and completes this
exact retry after expiry. Chat now requires a bounded registry of one or more
startup-pinned Maker templates. Each accepted agreement selects the template by
its exact application swap ID; duplicate swap or role-state identities fail
before sockets or SQLite are opened, and missing authority fails before
acceptance. The same requirement is carried through the packaged systemd unit,
which installs the real ZEC actor, requires its exact SHA-256, and names the
private authority and actor roots. Actual user-systemd run
`lez-m5-systemd-1000-2947208-15620` reached notification readiness, preserved
configuration across one exact SIGKILL restart, and removed its runtime on
SIGTERM in 51 seconds from a clean Cargo cache with no external resources; the
same flow previously took nine seconds with a warm cache. The preceding RED
proved a Cargo debug binary is correctly rejected for group-writable parent/file and
multiple-link metadata; the rehearsal deploys a single-link mode-0500 copy and
does not relax the artifact policy. Exact config bytes are now hash-verified
once, compared with their ZEC/BTC Maker role, swap, and role-state manifest
fields, then sealed into FD 196; wrong-swap or wrong-state configs fail before
spawn. The opt-in long-running daemon supervisor is now local-process GREEN. It
opens a dedicated SQLite connection so actor waits cannot block owner RPC,
creates one nonzero 128-bit lease owner from the OS CSPRNG per daemon lifetime,
and scans abandoned leases before readiness. A replacement generation may
recover a lease only after acquiring its per-swap kernel lock; the CAS transfers
owner and generation plus one while the row stays leased. A live old lock is
left untouched and does not stop a distinct due peer. Exact sealed `status` and
effect processes retain the lock through durable resolution, record and
exact-clear PID/start-time identity only after reap, and run under finite
process-group, time, and output bounds. SIGTERM cancels and reaps an in-flight
group before socket/readiness cleanup. The packaged systemd unit and transient
rehearsal enable this supervisor. Focused evidence is 12/12 store cases and 11/11
supervisor cases, and one local-only actual-daemon E2E: health stayed responsive
while the actor ran, and cancellation, reap, durable lease release, socket
cleanup, and readiness cleanup completed in under two seconds. The focused
runtime uses no node, RPC, Docker, faucet, DNS, public network, or public funds;
a cold Cargo build may use its pinned registry cache or download.
Both real one-shot ZEC and BTC actors now accept exactly one private path or
fixed inherited config FD 196. Each synchronously requires an anonymous,
euid-owned, mode-0600, unlinked memfd with all immutable seals before Tokio
exists. Black-box binary tests replace the deployment configs after sealing and
prove only the inherited snapshots are read; ordinary files, incomplete seals,
legacy BTC schemas, or any other FD number fail without actor JSON. The BTC FD
route additionally requires schema 6: it binds the exact agreement SHA-256 and
exposes a secret-free role/state/digest/signed-swap validation surface for the
supervisor while keeping path schemas 3–5 compatible. Pair-specific leased-
manifest comparison is GREEN over the exact sealed snapshot, and the persistent
daemon supervisor is process-GREEN. Actual user-systemd run
`lez-m5-systemd-1000-3497452-2505` then proved the node-free scheduler crash
boundary in 10 seconds: one sealed, hash-pinned memfd actor persisted an exact
fixture effect, the daemon was killed, generation 2 adopted the abandoned lease
without changing the effect inode or SHA-256, and a disjoint queued peer reached
terminal generation 1. No actor row or child identity remained leased. This
proof uses only local process, kernel, filesystem, and SQLite resources; it does
not certify a Zcash transaction or replace the still-open actual-node supervisor
composition and concurrent disjoint live-process composition.

The application runner now also installs one private mode-0500, single-link ZEC
actor and verifies that Chat acceptance atomically exposes the exact queued
daemon-provisioned manifest. Its current settlement still drives a separately
finalized Maker actor directly. Component-level supervisor routing is GREEN;
owner-local Maker monitor/claim/refund controls are process-GREEN. Actual-node
supervisor evidence, acceptance-receipt-bound Taker lifecycle effects, and
concurrent disjoint live-node
execution remain open. This checkpoint does not make M5 complete.

Build and repeat the current real process boundary with:

```sh
cargo build --locked -p lez-maker-node --bins
cargo test --locked -p lez-maker-node --test operator_journey -- --nocapture
cargo test --locked --offline -p lez-maker-node --test zec_chat_process -- --nocapture
cargo test --locked -p lez-logos-price-c-api --test worker_process
cargo test --locked -p lez-maker-node --test logos_price_offer_process -- --nocapture
```

The complete manual configure, price, quote, publish, restart, inspect, and
withdraw flow is [Flow 1 in the operator guide](docs/manual-user-flows.md#flow-1-maker-operator-cli-and-daemon-restart).
After starting fresh isolated LEZ v0.2 and primary Zebra Regtest services and
deploying the checked escrow, run the composed application path with identities
and dynamic loopback endpoints from those run manifests:

```sh
RUN_ID=m5zec-$(date -u +%Y%m%d%H%M%S) \
LEZ_SEQUENCER_URL=http://127.0.0.1:PORT \
LEZ_INDEXER_URL=http://127.0.0.1:PORT \
ZEBRA_RPC_URL=http://127.0.0.1:PORT \
LEZ_CHAIN_ID=HEX LEZ_GENESIS_HASH=HEX ESCROW_PROGRAM_ID=HEX \
MAKER_ACCOUNT_BASE58=FRESH_BASE58 TAKER_ACCOUNT_BASE58=FRESH_BASE58 \
M5_LEZ_DEPLOYMENT_EVIDENCE_FILE=/absolute/current/deployment.json \
M5_LEZ_FINALITY_EVIDENCE_FILE=/absolute/current/finality.json \
M5_LEZ_ONBOARDING_EVIDENCE_FILE=/absolute/current/onboarding/summary.json \
M5_LEZ_MAKER_SIGNER_KEY_FILE=/absolute/private/maker/lez-signer.key \
M5_LEZ_TAKER_SIGNER_KEY_FILE=/absolute/private/taker/lez-signer.key \
./scripts/run-m5-zec-application-poc.sh
```

See [Flow 1B](docs/manual-user-flows.md#flow-1b-composed-m5-zec-application-poc)
for prerequisites, evidence, and cleanup.
These component flows use only owner-private local Unix sockets, SQLite,
Delivery, signing, raw claim-recovery and preimage files; they use no chain RPC, Docker, faucet,
public funds, public price feed, or external network at runtime. The composed
application PoC uses only isolated local LEZ
v0.2 and Zebra Regtest services with fresh OS-random actor identities,
deterministic local genesis/Regtest funds, and current finalized deployment and
onboarding evidence. It records every endpoint and scoped cleanup operation.
No public RPC or faucet participates; cold pinned dependency or image acquisition
can fail independently and is not runtime swap evidence.

### M4 progressive local PoC

Correction recorded 2026-07-30: the retained M4 replay used the same role-inverted runner topology. Its cryptographic reconstruction, finalized chain effects, sweep, binder, and cleanup remain evidence, but the role-correct Taker receipt claim is withdrawn pending a fresh replay with Maker funding, neutral shared-wallet hosting, and Taker receipt. The historical evidence files remain immutable.

The exact clean replay `m4cert20260722an` on commit `5ec6521` certifies the
local native-XMR successful-claim path: LEZ v0.2 deployment/readiness, fresh
Maker/Taker actors, finalized tag 13/14/15, adaptor extraction, Maker-destination
post-fee receipt verification, canonical cross-chain binding evidence, and exact
run-scoped cleanup all passed. It used only isolated loopback services and
deterministic local genesis/Regtest funds—no public RPC, faucet, peer, or public
funds. This is a progressive local-functional PoC checkpoint; signed refund and
punishment branches, F7 parity, U9/D1 outputs, independent review, chaos, and
production-readiness hardening remain explicitly deferred.

The current BTC slice uses strict schema 4. The Taker externally submits the
direction-selected first lock. Only the Maker config contains `maker_lock`
material, and a fresh `btc-reference-actor` process reconstructs the exact
direction-shaped `BtcPairSdk` plan, persists it in
`SqliteBtcMakerLockJournal`, rechecks the first lock and signed cutoff, and owns
the second-lock submission. LEZ initialization and funding are two ordered,
durable steps; Bitcoin is one exact signed funding step. `Accepted`, `Unknown`,
timeouts, and a moving tip never create another send authority. Exact current
and canonical/finalized observation is required before the Maker intent and
lifecycle revision two close in one local SQLite transaction.

Run `m3schema4-20260717d` passed this boundary in both directions at clean,
already-pushed commit
`0e7635fc7e50cc6e0612745dcdaf6df8bbcf6f9a`. In
`TakerSellsForeign`, the Taker submitted Bitcoin and the Maker actor submitted
exactly one LEZ initialize/fund pair. In `TakerSellsLez`, the Taker submitted
the LEZ pair and the Maker actor submitted exactly one Bitcoin funding
transaction. Fresh Maker restarts added no effect; both actors reached revision
4 `completed`; terminal replay resubmitted nothing. The retained secret-safe
checkpoint is
[m3-schema4-actor-owned-lock-poc-20260717.json](docs/evidence/m3-schema4-actor-owned-lock-poc-20260717.json).

Clean run `m3overlap-20260717a` at already-pushed commit `1e6d5f1` then ran
both economic directions as two overlapping swaps on the same local Core and
LEZ nodes. Both swaps and all four role stores were simultaneously at revision
2 `both_legs_locked` before either settlement was released. The swaps used two
distinct mature coinbase outpoints, agreements, escrow account pairs,
deadlines, actor databases, and two signer sessions per domain backed by eight
distinct journals. Both reached revision 4, their Bitcoin and LEZ effect IDs
were disjoint, replay added no effect, and exact cleanup passed. The retained
secret-safe checkpoint is
[m3-overlapping-two-swap-poc-20260717.json](docs/evidence/m3-overlapping-two-swap-poc-20260717.json).

The earlier run `m3actor-20260716n` remains valid schema-3 evidence for
two-direction lock observation plus actor-owned claims. Schema 3 is now legacy
observation-only compatibility: it may project an already submitted Maker lock
but cannot submit one. It is not evidence for schema-4 Maker-lock ownership.
Run D closes that specific actual-node checkpoint; it does not complete all
accepted M3 scope or production readiness by itself. The later public SDK,
official-vector, Testnet4-route, and private-recording/video outputs are summarized
below; final repository-wide gates still precede any `m3-complete` tag.

## Current status

M3 is complete under `m3-complete`; the M4 local-functional PoC is certified
under `m4-poc-complete.2`. M5 is the active progressive phase. Its current
component evidence and remaining PoC path are summarized above and tracked in
[the live milestone scorecard](docs/milestone-metrics.md). The paragraphs below
retain the detailed historical M4 execution record.

Run `m4happy-40cbac3-20260721a` used the checked M4 LEZ guest, an isolated
source-audited LEZ v0.2 Bedrock/sequencer/indexer stack, separate authenticated
Maker and Taker sidecars, and official Monero 0.18.5.1 Regtest daemon and wallet
processes. Independent role material produced canonical Stage A and Stage B.
The Taker then finalized LEZ Initialize transaction `a85d7850...e234d388` at
height 3953 and Fund transaction `324cbbc4...ff34eb0e` at height 3960.

Only after that scriptable LEZ lock was canonical did the Maker-side funding
boundary pay exactly 1 XMR to the Stage-A shared address. Monero transaction
`de02209c...a8ef8017` was contained at height 111 and reached the ten-confirmation
local policy at tip 120. The exclusive preparer proved the exact finalized Fund,
peerless authenticated Monero topology, confirmed output, and completed Taker
claim journal. A fresh sealed release database reached `Prepared`; the one-shot
worker admitted dedicated tag 14, which Maker-side role discovery found as
transaction `13f9d56e...d37e7f1f` finalized at height 4107.

The Maker adapted its retained claim presignature, completed the exact durable
tag-15 claim, and submitted transaction `32c0135b...2585f8d`, finalized at
height 4208 with terminal LEZ custody zero. Taker-side role discovery recovered
that canonical aggregate signature, the Taker extracted the Maker share only
from that finalized evidence, combined it with its retained share, restored the
exact Stage-A wallet, and confirmed Monero sweep
`6c8c7bca...70e8e21a` at tip 130. The public
[working-tree evidence packet](docs/evidence/m4-actual-claim-poc-20260721.json)
contains no credentials, capability, wallet password, or private scalar. It
deliberately omits execution-binary hashes because post-run rebuilds changed
the evidence schemas; that omission and the explicit source limitation prevent
the run name from being misread as clean commit `40cbac3` evidence.

The Taker actor has now also produced one owner-private, mode-`0600`,
one-link cross-chain binder. It revalidates the exact Taker Stage A/B and
durable claim session, canonical finalized tag 15 at LEZ height 4208 under tip
4220, the observed aggregate signature and extracted Maker share, the
reconstructed public spend key, and the independently observed Monero sweep at
height 121 under tip 130. The public packet records the binder schema and
public facts without a private path. The final 3203-byte mode-`0600`, one-link
packet has SHA-256
`896d05d3178e3ff44b6ca010d4528835f5d796dc7e1004984ed78e853c083306`.

The retained sweep input is legacy v1 paired with receipt v2. It proves
998191600000 piconero at the evidenced destination and an unreceived remainder
of 1808400000 piconero, but it did not record exact fee fields; therefore the
public fee is `null`. The current sweep-v2 path is focused-tested and records
and cross-checks the exact fee, but the retained CLI invocation used legacy v1
plus receipt v2.

The destination is authenticated by the owner-private Taker-wallet execution
boundary but is not countersigned in Stage A, so the binder claims a confirmed
sweep to the evidenced destination, not an independent cryptographic proof of
Taker address ownership. It claims successful-path conditional atomicity, not
a distributed transaction or immunity from a later reorganization.

This executes the successful-claim branch of the conditional atomicity
argument: the Maker cannot receive the LEZ custody balance without finalizing
the signature that reveals Maker share `s_a`, which the Taker combines with
retained `s_b` to spend the exact confirmed XMR output. It is not a distributed
cross-chain transaction. The tag-16 signed-refund path is actual-node GREEN.
Tag-17 exact preparation, durable custody, one-attempt release, and two-role
finalized classification are actual-node GREEN under ADR 0158 and run
`m7tag17a23a314a`. Joined abandonment economics remain open alongside native
plus two custom-token F7
parity, U9 Stagenet guide/CI, D1 XMR videos, repeatability, QA, chaos,
information-security, production readiness, and independent review.

The implementation retains the two-stage XMR SDK, pair-neutral adaptor leaf,
checked guest tags 13 through 17, nine-method strict bridge boundary, durable
tag-13/tag-14/tag-15 planners and journals, exclusive release preparer, sealed
publisher, role-local finalized classifier, and typed Monero funding and sweep
effects. The two failed preparer databases from the live audit remain
quarantined; only a genuinely fresh third database was published. Official
Monero 0.18.5.1 may omit `connections` when its connection list is empty, so
the typed decoder accepts omission as empty only while `get_info` independently
proves zero incoming and outgoing peers; any nonempty list or nonzero count is
rejected.

The exact component/RPC topology and role sequence are in
[the deployment inventory](docs/architecture/deployment-components-and-rpcs.md)
and [system architecture](docs/architecture/system-architecture.md). The
conditional claim argument and evidence boundary are recorded in
[ADR 0075](docs/architecture/0075-complete-xmr-claim-from-finalized-role-evidence.md).
The complete fresh-ID operator procedure, external resources, inspection, and
scoped-cleanup rules are in
[Flow 0](docs/manual-user-flows.md#flow-0-m4-official-monero-regtest-topology).

`scripts/run-m4-actual-claim-poc.sh` remains a deliberately incomplete replay
runner, but its source and contract now compose through finalized tag 13. After
the checked artifact, identities, LEZ stack, deployment, and exact finalized
Maker/Taker Vault Claims, `execute` starts the run-scoped official Monero child,
composes canonical Stage A and countersigned Stage B through separate role
journals, publishes a durable no-retry latch, and invokes the exact one-shot
tag-13 Initialize/Fund actor. It then funds the local shared Monero output and verifies it against both role wallets,
then intentionally fails before release/Tag14. The cleanup contract uses an exact resource ledger, child/run
labels revalidated immediately before deletion, PID start-time/binary binding,
and a foreign sentinel; broad cleanup is forbidden. These paths are
**contract-GREEN but have not been cleanly replayed from the current commit**,
so they do not replace the retained working-tree checkpoint and the script is
not yet a one-command happy-claim replay.

The role-sidecar launcher now supports exact Taker adoption of the Tag-13 state,
requires the owner-private typed receipt plus its fixed runtime and terms
siblings, and rejects cross-swap terms, aliases, and state/output overlap before
creating a supervisor root. The bridge lease, typed exporter, receipt gate, and adversarial tests are
source/component-GREEN. The parent `execute` runner now builds and stages the exporter, bridge, and
Monero fund/verify binaries, exports the four exact artifacts, starts Taker
adoption and fresh Maker sidecars, records exact cleanup identities, funds the
local shared output, verifies it against both role wallets, and stops
intentionally before release/Tag14. The exact committed-tree replay remains
pending. The
agreement receipt calls the CLI inputs `requested_terms`; it does not claim that
the helper decoded and rebound those terms from Stage A. The remaining runner/PoC slice is estimated at 1 to 3 focused hours; after it exists, a warm replay is
expected to take 25 to 45 minutes and a cold replay 1 to 3 hours.
Full functional M4 remains 15 to 27 focused hours; later owner-selected
hardening is separate.

The checked guest and focused host components can be repeated independently of
the actual two-devnet journey. The deployer, focused component commands, and
full role-correct claim procedure are in the linked Flow 0:

```sh
RUN_ID=m4-readme-artifact-20260719a \
LEZ_M4_TOOL_DIR=/tmp/lez-atomic-swaps-tools/risc0-3.0.5 \
  ./scripts/run-m4-lez-artifact-tests.sh

./scripts/run-m4-lez-local-deployment.sh contract

cargo test --locked -p lez-bridge-client -p lez-xmr-monero-adapter \
  --all-targets --all-features

CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=2 cargo test --locked \
  -p lez-xmr-release-authority --all-targets --all-features
```

The release-authority suite uses authenticated literal-loopback HTTP fixtures
and a temporary owner-private SQLite journal. It makes no chain, public RPC,
faucet, peer, or external-network call. The checked-artifact run likewise opens
no chain RPC, faucet, peer, or public endpoint after
setup. A cold cache can still require pinned circuits, Cargo/Git sources, the
digest-pinned Docker builder, and Risc0 release tools. The shared tool directory
above must already contain the exact verified tools; omit it for an isolated
cold run that cleans its own tools. The observation component accepts only
distinct credential-configured literal-loopback daemon/wallet origins and
public RPC is rejected. Its upstream and view-only-wallet limitations, plus the
required consume-once actor gate, are in
[ADR 0059](docs/architecture/0059-separate-monero-observation-from-release-authority.md).

M2 is certified at its private local-functional PoC boundary under
`m2-complete`. M3 completed with **2 of 2 schema-4 happy directions with actor-owned
Maker second locks through actual local nodes**, including one clean
opposite-direction overlapping execution at a shared revision-two barrier.
Its authority, Bitcoin Core
31.1 Regtest topology, dependency candidates, actor flows, and acceptance gate
are audited in
[ADR 0029](docs/architecture/0029-m3-bitcoin-local-poc-entry.md). The
nonexistent DLC Schnorr-vector reference is separately tracked as
[Gateway erratum GW-M3-001](docs/proposal-acceptance-errata.md), with no accepted
replacement yet.

All six issue-#112 M3 outputs, including the literal RFP D1 three-video
requirement, and their underlying BTC happy/refund/concurrent actual-node
evidence are implemented at the private functional-PoC boundary. The source
recordings and their private MP4 projections are GREEN; the sealed video bundle
has SHA-256
`7697a27c80c8f90856d6592051805a8923fe564aa01b0dff4109bd5c5f101ba8`.
The 2026-07-19 local closure run passed the exact lint, test, vulnerability,
license, source, isolation, traceability, and 150-diagram render gates.
`m3-complete` now certifies exact closure commit `f7fb250`. The private
Actions result was not observable because this environment has SSH push access
but no Actions API identity; the annotation makes no remote-green claim. Pushed
`0c78f3d`
adds the public canonical durable lifecycle codec,
CAS store port, typed Bitcoin/LEZ runtime, restart/replay coverage, dedicated
example, official BIP-340/BIP-327 corpora, and independently checked
swap-specific adaptor vectors. Pushed `946208a` adds exact Core 31.1 Testnet4
readiness plus self-hosted loopback and exact allowlisted HTTPS routes, and
makes those focused gates mandatory in CI. The
[Testnet4 setup guide](docs/bitcoin-testnet4-setup.md) documents release
verification, node/index setup, wallet/funding, SDK composition, external
resources, and flakiness without claiming a public run. ADR 0050 supplies the
Aumayr/Fournier mapping and conditional atomicity argument.
The evidence inventory, claim boundary, remaining hardening, and exact tag rule
are collected in the [M3 review packet](docs/milestone-3-review.md).

Three owner-private actual-node source recordings at evidence commit `a6eb1ad` cover
happy, both ordered refunds, and the opposite-direction concurrent barrier.
Their mode-`0600` source bundle was sealed by verifier commit `946208a` with SHA-256
`3d7d7adc12571a610be21a18b746e68cb17311ea1224191fcdcdf1b39a86c7cc`.
It records Core 31.1 Regtest and private LEZ v0.2, no public RPC/faucet/funds,
and no certification dependency on an external network. The bundle remains
owner-private under `.e2e`; the reproduction and independent hash/mode checks
below are public.

Run `m3schema4-20260717d` passed at clean pushed `origin/main` commit `0e7635f`
on 2026-07-17. It used schema-4 configs, separate role state and signer
journals, fresh one-shot processes, restricted Maker/Taker Core identities,
Bitcoin Core 31.1 Regtest, and the exact local LEZ v0.2
Bedrock/sequencer/indexer plus role sidecars. In each direction the Taker
externally submitted the first lock and the Maker actor submitted exactly one
second lock. Both roles ended revision 4 `completed`; each direction retained
two unique Bitcoin effects and three exact LEZ effects; terminal replay added
zero effects. Run-scoped cleanup removed only its captured containers,
networks, volumes, images, and secure reservation state.

The local finalized LEZ tip advanced during Run D. The bounded reconciliation
retried only typed read/observation unavailability while checking the durable
LEZ count or exact Bitcoin mempool identity on every attempt. The successful
`TakerSellsLez` Maker action followed nine typed moving-tip attempts, yet the
Maker Bitcoin lock appeared exactly once and its fresh restart resubmitted
nothing. This can add latency; it cannot authorize a new send. The actor bridge
timeout remains 30 seconds, and timeout is uncertain observation rather than
absence.

Historical run `m3actor-20260716n` at `6ded2f9` remains the schema-3
two-direction actor-owned-claim proof. It reached revision 4 for both roles and
replayed with zero resubmissions, but its run operator submitted both locks.
It must not be cited as schema-4 Maker-lock ownership evidence. Run D closes
that ownership checkpoint. Run `m3overlap-20260717a` separately closes the
accepted two-swap opposite-direction overlap checkpoint. Arbitrary-N and
same-direction nonce scheduling, production SDK/custom-token hardening, public
live execution, and final milestone-wide gates remain open.

Fresh isolated Run `m3f7compose20260718x` on clean pushed `422c72e`
closes the functional custom-token checkpoint in both directions against actual
Bitcoin Core 31.1 Regtest and LEZ v0.2 nodes. Each direction reached revision 4
`completed` with exactly two Bitcoin and four LEZ effects, zero replay
resubmission, zero custody balance, and conserved owner balances:
`175/75/0` when the Taker sold Bitcoin and `75/175/0` when the Taker sold
LEZ. The run used no public RPC, faucet, or public funds, disclosed no private
material, and removed only its exact captured resources. This is a private local
PoC and the first clean F7 repetition, not the M3 tag or a production-readiness
claim.

Fresh Run `m3f7compose20260718z` on clean pushed `1555749` repeated the
same two actual-node directions in 19 minutes 10.95 seconds and is the second
of three requested clean F7 repetitions per direction. It retained the same
effects, balances, zero replay, finality, and exact cleanup while recording a
production-mode verified-wallet cache hit. Run `m3f7compose20260718y` is not a
swap repetition: an accidentally supplied pre-F7 guest target failed its
pinned ELF check before deployment, and exact cleanup passed.

Fresh Run `m3f7compose20260718aa` on clean pushed `df7ed86` completed the
third pair in 18 minutes 13.61 seconds. It preserved the same terminal
revisions, effects, directional balances, finality, zero replay, and exact
cleanup, while certifying the fail-fast guest/deployer hardening and a
7.81-second production cache hit. The requested F7 repeatability gate is now
3 of 3 per direction; this still does not by itself complete or tag M3.

The official LEZ v0.2 wallet preparation is now content-addressed and
fail-closed for repeat runs. A hardened production-input cold build measured
202.42 seconds; the exact validated hit measured 10.35 seconds, saving 192.07
seconds (94.9%) while still binding the source/origin/archive, lockfile, Cargo
metadata and effective config, program artifacts, Rust/Cargo and target-library
tree, build tools, bindgen headers, native libraries, validation helper/policy,
runtime dependencies, and pinned wallet SHA-256. Only the executable and its
manifest persist; wallet state, keys, actor databases, journals, agreements,
transactions, ports, node state, and evidence never enter the cache. Runs Z
and AA certified integrated hits in 10.32 and 7.81 seconds on exact pushed
code; the earlier
cold/hit comparison remains development performance evidence. Manual commands,
evidence checks, cache trust and
repair guidance are in the
[M3 operator guide](docs/m3-local-poc-operator-guide.md#reproduce-the-custom-token-f7-happy-pair-with-the-verified-wallet-cache).
Timing-enabled M3 runs also publish an owner-private monotonic phase packet and
two owner-private actor-direction packets. The main run packet binds all three
paths and SHA-256 values; each child also binds its current actual-effect
manifest and must fit inside the correct outer actor-flow or overlap duration.
All five bound files are rehashed before and after main publication. The
[manual timing checks](docs/m3-local-poc-operator-guide.md#inspect-bound-monotonic-phase-timings)
show how to validate and compare phases without exposing private actor data.
Clean pushed Run AF measured 16m40.17s before main publication with only
510 ms unattributed and 16m47.57s through exact cleanup. Its forward and
reverse actor children took 346.06s and 386.06s. The dominant child phases are
the forward LEZ second lock at 243.62s, forward LEZ revealing claim at 99.48s,
reverse Bitcoin second lock at 141.01s, reverse LEZ first lock at 126.64s,
and reverse LEZ follow-up claim at 116.11s; every other child phase is below
one second. The complete pinned CI quality suite and both actual-node
directions are GREEN. Run AF overlapped an unrelated host workload, so its
22-second wall-time difference from Run AE is not a certified speedup and does
not justify weaker finality.

The public BTC SDK now deterministically validates application-owned
discovery/negotiation, drops those peer capabilities at activation, and replays
exact agreement-bound claim or ordered-refund transitions through revisions one
to four in both directions and roles. A separate two-role-signed asset extension
binds the unchanged agreement-v1 commitment to an explicit native or exact
custom-token selection without aliasing custom custody to native custody. These
are component gates, not new actual-node evidence. The bounded loopback bridge
client now maps all eleven additive v2 asset operations with one-call/no-retry
semantics, depositor/claimant/permissionless role checks, and strict
term/target/window/effect validation. Public process-durable store/chain
composition was the remaining public-SDK gap at this checkpoint; pushed
`0c78f3d` now closes it with the canonical durable codec, exact CAS store
port, typed chain ports/runtime, restart/replay coverage, and application
wiring example. Production applications still supply the concrete durable
store and persist-before-send journals. The official v0.2 sidecar planner now prepares the
ordered witnessed-token initialize, permissionless custody creation, funding,
aggregate-witness claim, and fixed-destination refund transactions. It
rederives the pinned Token/ATA programs and every owner/custody ATA, preserves
consecutive signed nonces around the unsigned custody step, and restores four
distinct v2 reservations byte-for-byte without nonce rereads. The main-process
adapter now maps the countersigned BTC asset extension and exact local policy
into all eleven v2 calls, checks chain/program/signer/role before transport, and
preserves all four conservative classifier outcomes without submit authority.
The sidecar exposes all eleven capability-authenticated routes, restores those
reservations in dependency order, and scans finalized initialization, custody,
funding, claim, and refund evidence without joining state across a moving or
same-height replacement fork. Runs X, Z, AA, and AD subsequently close durable
actor composition and both-direction actual-node token balance/effect evidence
at the private functional boundary.

Run `m3refund-20260716h` then passed the two-lock timeout/refund journey
from base HEAD `ef5f306` with a dirty pre-commit source tree and the packet's
independently validated runner hashes. It is hash-bound functional evidence,
not clean exact-commit certification.
Fresh one-shot maker and taker processes executed both directions against
Bitcoin Core 31.1 Regtest and LEZ v0.2 private-local
Bedrock/sequencer/indexer services using a 3.0-second slot. In both directions,
both role stores reached revision 4 `refunded` with next action `complete`.
Each direction retained exactly two confirmed Bitcoin effects and three exact
durable LEZ submissions: one Bitcoin refund and one LEZ `RefundNative` were
actor-owned, and no cooperative claim effect was present. Terminal `recover`
replay changed no effect count. The packet records no public RPC, faucet,
public funds, certification-time network dependency, or private-material
disclosure; exact cleanup removed only captured run resources and targeted no
foreign activity. The retained evidence span from its first emitted file to
cleanup was 54 minutes 5 seconds. Most elapsed time is the deliberate signed
deadline wait across two sequential directions, so this is not a throughput
benchmark. It proves ordered recovery after both locks, not a first-lock-only
absent-maker or survivor-only journey.

Run `m3firstlock-20260716h` passed at clean, already-pushed commit `cefcd07`.
In both economic directions the taker funded the first leg, the maker stayed
offline after activation and never submitted the second lock, and fresh actor
processes recovered only the taker-funded leg after the countersigned cutoff
and chain-specific refund boundary. `TakerSellsForeign` retained exactly the
Bitcoin lock and BIP-342 refund; `TakerSellsLez` retained exactly LEZ
initialize, fund, and `RefundNative`. Both roles reconstructed terminal
revision 2 `refunded`, replay added zero effects, and exact cleanup targeted no
foreign resource. The secret-safe retained packet is
[M3 first-lock absent-maker evidence](docs/evidence/m3-local-two-direction-first-lock-refund-poc-20260716.json).
This closes the refund-side absent-maker journey. Run D separately proves the
timely actor-owned Maker-lock branch; overlapping timing/race, reorg, and chaos
hardening remain open. The post-reveal survivor journey is certified by run C
below.

Run `m3survivor-20260716c` passed the direct post-reveal
survivor journey in both directions. After both locks, the taker published the
direction-correct reveal and the journey barred every further harnessed taker
actor invocation until maker terminality. A fresh
maker observed canonical chain disclosure into nonterminal revision 3
`claim_evidence_available`, exited, and another fresh maker completed the
opposite claim before the signed refund boundary. Only after maker terminality
did the taker return for observation-only catch-up. Each direction retained
exactly two Bitcoin and three LEZ effects, replay added zero effects, and exact
cleanup targeted no foreign resource. The run used clean, already-pushed commit
`6e8b065`; the retained secret-safe packet is
[M3 post-reveal survivor evidence](docs/evidence/m3-local-two-direction-survivor-claim-poc-20260716.json).

Run `m3poc-live2-20260715a` used one isolated Bitcoin Core 31.1 Regtest node,
one exact local LEZ v0.2 Bedrock/sequencer/indexer stack, and separate
capability-authenticated maker/taker sidecars, signing processes, keys,
SQLite journals, and state roots. The digest-pinned aggregate-witness guest
ELF `a199c5be...e293` / ProgramId `39b6a4db...4dec` was deployed and finalized
before either direction. Both actor Vault allocations were independently
claimed and re-read at finalized tips before swap preparation.
Fresh local identity schema version 2 publishes each actor's owner account and
the official owner-derived Vault account in base58 and hex, plus the x-only
public key; signer material remains owner-private. The local stack requires
owner and derived-Vault overrides as a pair before it creates genesis.

`TakerSellsForeign` confirmed the taker Bitcoin lock `ca0ae641...a4c75`, then
finalized maker LEZ initialize/fund transactions in blocks 540/544. Only after
both locks were proven did the taker finalize LEZ claim
`ef77099e...2cde3` in block 570. The maker matched the exact finalized LEZ
witness, extracted and point-checked the committed adaptor scalar from its own
persisted session, and confirmed Bitcoin claim `0ee99753...6a5aa` with exactly
one 64-byte key-path witness. `TakerSellsLez` finalized taker LEZ
initialize/fund transactions in blocks 617/620, then confirmed maker Bitcoin
lock `c5dd0f85...752a3`. The taker confirmed Bitcoin claim
`66255398...054f4`; the maker matched that exact Core witness, recovered the
scalar from its own persisted session, and finalized LEZ claim
`834c67e9...d3033` in block 644. Both terminal LEZ custody accounts are zero,
and both Bitcoin contract outpoints are spent exactly once to the
direction-correct recipient.

The PoC uses a declared one-confirmation Regtest policy and independently
requires LEZ indexer `Finalized` membership, exact-once bounded block
membership, and equal by-ID/by-hash block bodies. No scalar is used before both
locks, and after reveal the opposite claim uses only persisted local state and
chain observations. The secret-safe retained packet is
[M3 local two-direction evidence](docs/evidence/m3-local-two-direction-poc-20260715.json).
No public RPC, faucet, peer, credential, public funds, or public deployment is
used.

The public `btc-reference-actor` accepts `activate`, `drive`, `recover`, or
`status` in a fresh role-fixed process. Current happy flows use strict
owner-private schema-4 configs. Only the Maker config carries the exact
direction-shaped `maker_lock`; schema 3 remains historical observation-only
compatibility. The agreement-selected Bitcoin funder alone supplies a
lowercase-hex mode-`0600` refund-key file converted
without stdout from its raw provisioner key; the other role must have neither
the converted file nor that authority. Activation rederives and compares the countersigned x-only key. Before
`activate` may insert agreement
acceptance or create revision zero, it binds the complete prepared LEZ claim
result plus two distinct completed role-local signer journals to contexts
rederived from the countersigned agreement. The taker must also provide an
owner-only adaptor scalar that is point-checked without creating a signature;
maker configs are forbidden from naming that secret. Missing, cross-wired,
incomplete, unsafe, or changed authority fails without creating actor state.
Absent or
empty/no-acceptance state remains `not_activated`, while corrupt or conflicting
state fails closed; `status` may migrate an existing schema but creates no
acceptance and performs no RPC. At revision zero, both roles observe the exact
Taker first lock before the predecessor CAS. At revision one, the Maker actor
persists and drives its exact opposite-chain plan while the Taker remains
observation-only. Every possible send follows a fresh first-lock/cutoff check;
only exact current and canonical/finalized Maker-lock evidence can close the
Maker journal and revision two in one local SQLite transaction. A fresh Taker
process then independently observes that same lock. Finalized LEZ evidence
retains its complete finalized tip. Before funding, LEZ observer errors are
retryable unavailability, not proof of absence. Exact actor retries retain
their deterministic operation identity and never rearm durable send authority.
A valid concurrent revision-one or revision-two winner is reconstructed without
overwrite; other projection conflicts fail closed. Chain RPC and SQLite are
not one transaction. At revision two or
three, claim projection reruns the complete activation-material gate before
using signer state. The injected exact-canonical observation seam reaches
revision three and terminal revision four for both roles and directions. The
taker must reproduce the revealing signature from its private scalar; the maker
must extract and point-check the same scalar from its persisted presignature.
Only one-way `ClaimEvidence` enters lifecycle state, signed Bitcoin confirmation
policy and finalized LEZ policy units are enforced, and terminal status is
reconstructed offline. The typed live Bitcoin finalized-claim observer is
wired.

The public-effect journal and exact one-attempt Bitcoin send primitive are now
composed for actor-owned Bitcoin claims. The taker alone owns a revealing
Bitcoin claim at revision two; the maker alone owns a follow-up Bitcoin claim
at revision three and re-extracts its scalar from the durable revealing witness
plus its persisted LEZ presignature. Complete public bytes are durable before
the only fresh send authority. `Started` and `Unknown` remain observe-only
across restart, exact-byte drift is uncertain, and even an accepted send does
not project local state. Projection waits for the same exact bytes finalized by
Core at the signed confirmation policy. Pushed `66d352f` composes the matching
actor-owned LEZ completion, bounded finalized presence/absence observation,
one-attempt send, and terminal projection path. Both claim paths are GREEN in
source, deterministic actor tests, historical run-n claim evidence, and the
current schema-4 run-D actual-node evidence.
See
[ADR 0031](docs/architecture/0031-one-shot-btc-actor-observe-before-project.md),
[ADR 0034](docs/architecture/0034-gate-actor-activation-on-signing-material.md),
[ADR 0035](docs/architecture/0035-project-claims-only-from-canonical-public-evidence.md),
and [ADR 0040](docs/architecture/0040-continue-post-reveal-from-canonical-evidence.md).

For timeout recovery, call the same binary with `recover`. The maker-funded leg
must reach durable revision 3 before the taker-funded leg can reach revision 4
`Refunded`. Deterministic actor tests cover both direction mappings, owner and
nonowner roles, pre-deadline no-send, persist-before-send, one-winner/one-attempt
authority, observe-only ambiguity, exact finalized projection, and terminal
restart. The [manual timeout/refund procedure](docs/m3-local-poc-operator-guide.md#manual-actor-timeoutrefund-recovery)
uses only isolated Core 31.1 Regtest and local LEZ v0.2
Bedrock/sequencer/indexer plus role sidecars; no public RPC, faucet, public
deployment, or public funds are needed. Run `m3refund-20260716h` now retains
fresh actual-node execution for both ordered two-lock refund directions.
First-lock-only absent-maker recovery is now actual-node GREEN in
`m3firstlock-20260716h`. Survivor-specific continuation, the maker-lock side of
the deadline-cutoff race, and opposite-direction two-swap overlap are now
separately GREEN in `m3survivor-20260716c`, `m3schema4-20260717d`, and
`m3overlap-20260717a`. Adversarial cutoff/refund races,
arbitrary-N/same-direction scheduling, process-kill, and reorg remain later
gates.

Pushed `a8688a3` replaces the unsafe post-confirmation recipe with an exact
pre-effect funding ceremony. For each direction the operator runs `generate`,
reads the candidate input with Core `gettxout`, builds one exact signed funding
transaction with `prepare-funding`, asks Core for read-only
`testmempoolaccept` evidence, finalizes the
countersigned agreement with a planned next-block anchor, completes both the
Bitcoin and LEZ signer journals, and only then permits the first chain effect:

```sh
cargo run --locked -p btc-local-poc-provision -- generate \
  --planning-file "$AGREEMENT_PLANNING" \
  --output-root "$DIRECTION"

cargo run --locked -p btc-local-poc-provision -- prepare-funding \
  --spec-file "$FUNDING_PREPARE_SPEC" \
  --output-root "$DIRECTION"

# Read-only: submit the exact persisted hex to Core testmempoolaccept here.

cargo run --locked -p btc-local-poc-provision -- finalize \
  --spec-file "$AGREEMENT_FINALIZE_SPEC" \
  --output-root "$DIRECTION"
```

The strict prepare document contains `schema_version`, the stage-one public
SHA-256, `direction`, one `service_input` object (`transaction_id`,
`output_index`, `value_sat`, `script_pubkey`, and the path to a raw mode-`0600`
32-byte `signing_secret_key_file`), `contract_value_sat`, and `fee_sat`. The
secret is extracted from the owner-private Core funding credential directly to
that file without stdout. `prepare-funding` emits no raw transaction on stdout;
it creates mode-`0600` `funding-transaction.hex` and a secret-free summary with
the exact txid/wtxid, input, contract, change, fee, BIP-341 sighash, Merkle root,
and `node_state_asserted: false` facts.

The strict finalize document binds `genesis_block_hash`,
`required_confirmations`, `funding_signed_transaction`, its SHA-256, the input
value and script, funding txid/vout/value, `claim_value_sat`, LEZ deployment and
prepared-claim facts, and the recovery plan. The recovery plan contains
`refund_csv_blocks`, `planned_bitcoin_funding_anchor_height`,
`bitcoin_refund_height`, both typed cross-chain deadlines, and their safety
margin. It rejects the former observed-confirmation/observed-anchor/broadcast
fields. The signed anchor must be reserved before stage-two finalization from a
stable, empty-mempool isolated Core tip. Sequential execution reserves `tip +
1` just before each direction is countersigned; overlap execution atomically
reserves `tip + 1` and `tip + 2` before either agreement. An anchor is never
rebased after finalization. When Bitcoin funding is due, the harness broadcasts
the persisted exact bytes, mines exactly one block, and requires the containing
height to equal that plan.

Stage one creates fresh OS-random maker/taker signing, refund, and claim keys
plus the adaptor scalar under mode-`0700` directories and single-link,
create-new mode-`0600` files. The provisioner verifies the rawtr service-key
relation and exact BIP-341 authorization, canonical bytes/hash/txid, fee,
contract output, and one-item `SIGHASH_DEFAULT` witness. It performs no RPC and
therefore does not prove that the input is unspent/mature, current Core policy,
broadcast, confirmation, or finality. `gettxout`, `testmempoolaccept`, and later
exact block reads supply those separate node-state facts; policy acceptance is
not a reservation.

The multi-file generate, funding, and agreement output groups are create-new
and fail safe but are not distributed or filesystem-atomic transactions. Before
any effect, retire an interrupted direction root and restart from fresh stage
one. After any possible effect, preserve the exact root and reconcile or refund;
never regenerate authority for already-locked funds. Cross-chain atomicity is
maximized by requiring one agreement and both chains' presignatures before the
first effect, but no distributed atomic commit exists across files, journals,
actor stores, Core, and LEZ.

Eleven all-target provisioner tests cover both directions, genuine rawtr
signing, drift and malformed inputs, no-clobber recovery, and stdout secret
scanning without RPC, Docker, faucet, or public endpoint use. The combined
run-owned harness must retain the policy response, journal-completion evidence,
and actual planned-anchor equality while driving fresh public actor processes
through both directions. See the exact
[operator recipe](docs/m3-local-poc-operator-guide.md#generate-the-agreement-fixture-before-funding)
and [ADR 0037](docs/architecture/0037-finalize-exact-bitcoin-funding-before-first-effect.md).


M3 completed with an actual-Core, two-party MuSig2/adaptor P2TR vertical slice.
Exact-pinned `bitcoin` 0.32.101 constructs the aggregate-internal-key plus
CSV-refund commitment, while exact-pinned `musig2` 0.4.1 aggregates the ordered
maker/taker public fixture keys, applies the Taproot tweak, and matches the
rust-bitcoin output key `Q` and parity. One helper process computes two
role-bound nonce commitments, creates and verifies both partial adaptor
signatures and their 65-byte aggregate presignature, adapts it with a labeled
public Regtest scalar, verifies the resulting 64-byte signature under `Q`, and
re-extracts the scalar and checks its point. The isolated runner then executes
the `TakerSellsForeign` Bitcoin-leg ordering through distinct actor `rpcauth`
capabilities. Core policy-checks, decodes, mines, and re-reads the taker funding
at height 102 and maker key-path claim at height 103, including the exact
one-item witness and spent-once outpoint; the final mempool and peer set are
empty and cleanup is exact.

The public BTC SDK now also validates a bounded canonical agreement signed by
both roles. It binds the exact LEZ runtime/program/accounts/amount/deadline and
claim message, Bitcoin genesis and confirmation policy, ordered MuSig2 keys and
adaptor point, reconstructed P2TR/CSV output, exact funding outpoint/value,
cooperative transaction and BIP-341 sighash, and the direction-correct recovery
schedule. Derived Taproot and transaction fields are reconstructed with the
pinned libraries before either role signature is accepted. The reference actor
now activates this agreement and uses it through lock and injected claim
projection. Both its exact Bitcoin and LEZ claim effects are actor-owned,
persist-before-submit, and GREEN in deterministic tests. Run
`m3actor-20260716n` also proves their combined two-direction actual-node
boundary through the public actor.

Pushed commit `0177151` adds the next protocol slice behind canonical byte
boundaries. Separate maker and taker signer-state objects create fresh
OS-random nonces, exchange and verify transcript-bound commitments before nonce
reveal, reject phase/reuse/message/adaptor mismatches, verify both partials, and
derive the same aggregate presignature for the exact BTC message and a
placeholder LEZ message domain. A runnable dual-session fixture proves that
either domain's completed signature reveals the same scalar and completes the
other signature.

Pushed `e3f2938` adds a role-local SQLite adaptor-session journal. It reserves
the exact one-use nonce before exposing its commitment, permits nonce reveal
only after the peer commitment is durable, and atomically consumes the nonce
with an exact replayable partial-signature outbox. Six focused restart,
mutation, reuse, concurrency, and private-file tests plus the 68-test store
suite pass. This PoC journal deliberately stores the nonce plaintext in an
owner-only database until consumption; encryption or HSM custody remains a
production-readiness requirement.

Pushed `8a7ea55` bridges that durability boundary into the BTC SDK. Fresh
MuSig2 material can be reserved before commitment exchange, reconstructed after
restart, checked against the complete session and both role-bound nonce
commitments, consumed once to produce a verified partial, and aggregated into a
verified presignature. Pushed `ca524ff` then moves every maker and taker phase
into a fresh one-shot OS process with a separate owner-only SQLite journal and
canonical public packets. Pushed `96f2a31` adds journal-bound adaptation and
point-checked scalar extraction as separate processes with create-new `0600`
scalar files. Four process journeys cover both the untweaked LEZ and tweaked
Bitcoin domains, restart/replay, cross-wires, unsafe permissions, and
secret-free output.

Pushed `6935acd` adds the actual pinned LEZ v0.2 aggregate-witness guest. A
distinct two-party aggregate authority signs the exact public transaction, but
the escrowed asset is transferred only to the separately committed claimant.
The digest-pinned Risc0 build reproduces ELF `a199c5be...e293` and
ImageID/ProgramId `39b6a4db...4dec`; recursive execution proves the two-party
claim and rejects one-share, wrong-message, mismatched-authority, and preimage
bypass attempts without moving custody.

Pushed `79735dd` exposes that path through the official-wire sidecar. The
claimant role durably reserves the exact canonical LEZ message and official
hash, external maker/taker signing supplies one aggregate signature, and the
sidecar verifies and durably completes the canonical transaction before the
existing one-shot submission boundary can accept it. Preparation survives a
fresh process without rereading the authority nonce. Live local deployment and
submission remain part of the composed runner, not this component claim.

Pushed `3862dde` adds the other side of the witnessed LEZ lock: the depositor
role prepares the exact generated `InitializeNativeWitnessed` account ordering
and a separate `FundNative` transaction, signs only with the depositor key, and
survives restart without combining preparation and submission. Pushed
`f827dad` makes the Bitcoin transaction helper emit the exact public spend plan
and canonical role-runner session, then verify an externally completed
signature before emitting the broadcast-ready one-item-witness transaction.

Pushed `3d7386b` adds conservative live observation of that witnessed LEZ
initialize/fund pair. It validates canonical transaction bytes, signer/account
order, aggregate authority/key, metadata/custody effects, and one stable tip;
an exact miss remains `unknown_or_pending` instead of inventing absence or
finality. Pushed `a3da09e` exposes the witnessed prepare, observe, submit, and
complete calls through a strict typed one-shot operator CLI with a private
capability file. Pushed `bf5bdbd` delegates x-only-key to LEZ-account mapping to
the pinned official `nssa` implementation rather than duplicating it in shell.

The same operator/client/sidecar boundary now has an explicit read-only
`observe-finalized-witnessed-claim` call. Either correctly bound participant
can scan one bounded window only after the indexer finalized tip covers it,
using either an exact transaction ID or peerless discovery from the signed
terms and prepared transcript. The sidecar requires equal block-by-ID and
block-by-hash results plus parent-hash continuity from the window start through
the stable finalized tip, exactly one canonical public claim under the pinned
program and derived accounts, the exact aggregate signature/message/authority, and
`Claimed` metadata plus zero custody read at that containing numeric block ID.
The client independently enforces the requested window and verifies BIP-340
with exact-pinned `secp256k1` 0.29.1. The official indexer supplies historical
account DTOs but no account proof or atomic multi-read snapshot token; that is
recorded as an upstream production trust limitation, not hidden as local
consensus proof. This closes peer-independent typed LEZ claim evidence.

The separate `observe-finalized-witnessed-funding` call is now GREEN. It keeps
the earlier stable-tip progress observer intact, accepts an exact transaction
ID or bounded unique discovery by signed terms, and proves canonical
`FundNative` inclusion plus historical `Funded` metadata and exact custody at
the containing finalized block. The reference actor now uses this evidence for
whichever of the agreement-derived taker or maker funding transitions is LEZ,
before the corresponding local projection. Claim projection now requires both
lock projections and reruns the complete activation-material gate before any
adaptor use; the read-only bridge method still does not itself block its
independent claim methods. The pinned sidecar is the
canonical official-wire decoder and PDA
validator; the graph-isolated client validates the bounded result and role
binding but does not claim an independent official LEZ decode. The actor compares the returned accounts and
transaction evidence
with the signed agreement before durably advancing the lock state.

The next durable M3 component is now implemented locally as
`SqliteBtcRecoveryStore`. Each maker or taker opens a different owner-private
database bound to its exact accepted agreement, role, and agreement-derived
initial coordinator. Four exact public-evidence revisions project the taker
lock, maker lock, revealing claim, and follow-up claim through `BEGIN
IMMEDIATE`, predecessor CAS, and a versioned domain-separated SHA-256 evidence
chain. Reopen recomputes the chain and reconstructs terminal `Completed`
offline. The store retains the exact 64-byte public revealing witness but never
the recovered scalar; focused mutation, rollback, replay, actor-separation,
and scalar-absence checks are 11/11, and all 84 store tests pass. This is a
GREEN persistence component. The reference actor now validates the canonical
agreement and both typed funding observations before projecting revisions one
and two, then projects exact canonical revealing and follow-up claims through
revisions three and four in its injected seam. Chain and SQLite cannot commit
atomically, and the hash chain detects
accidental/tampered history but does not authenticate a database rewritten in
full by its filesystem owner.

The typed `lez-btc-core-adapter` component now validates the other public
evidence source, including the Bitcoin timeout path. It requires exact Bitcoin
Core 31.1, Regtest genesis from the
countersigned agreement, an unpruned synced tip, and synced `txindex` plus
`txospenderindex`. The selected readiness policy distinguishes the fully
disconnected local node from an explicitly network-enabled Regtest node. Exact
funding and key-path claim observations are reconstructed from consensus bytes
and cross-checked against Core's typed vin, vout, identity, size, weight,
confirmation, block, spender, and stable-tip facts. The fresh-maker variant
adds the spender-index read inside the same stable-tip bracket and distinguishes
confirmed-unspent, confirmed-spent, pending exact bytes, and affirmative
absence. Canonical bounded evidence
records bind the agreement, transaction bytes/IDs, block/tip context, exact
64-byte public claim witness without a scalar, and finalized refund containing height. Submission is one-attempt only:
an owner-provided durable CAS binds txid, wtxid, and the exact raw-byte digest
before mempool policy and one broadcast. Broadcast success is accepted only
after the spender index returns the same complete witness bytes; same-txid but
different-wtxid races are terminal `Unknown` for claims and refunds. Funding
submission uses the same single-send preflight but requires an exact
`getrawtransaction` byte readback rather than a spender read. Already-known or ambiguous results
become terminal `Unknown`, while conflicting witness bytes fail before another
RPC mutation. The HTTP
transport is literal-loopback, Basic-file authenticated, bounded, one-request
concurrent, and rejects non-`0600`, symlinked, hard-linked, replaced, or changed
credential files. The full 37 all-target test executions plus strict dependency, Clippy, and
rustdoc gates are GREEN. Refund observation additionally binds the
stable-tip-derived funding height to the signed anchor, applies BIP-68 at the
next-block boundary, and emits finalized evidence at the refund containing
height rather than the tip.
Run `m3actor-20260716n` exercises that typed adapter through both actual-node
actor directions. Core 31.1 requires `gettxspendingprevout`'s second parameter
to be the options object
`{"mempool_only":false,"return_spending_tx":true}`; commit `2233964` fixes the
former positional booleans and checks exact spending bytes plus the absent or
present containing block hash. Current `Networked` still means network-enabled
Regtest, not Testnet4 portability.

The v0.2 native-refund planner now closes the next no-RPC component boundary.
It produces the official permissionless `RefundNative` transaction with exactly
metadata, custody, and immutable depositor accounts and with no nonce or
witness. Strict legacy hashlock and M3 witnessed terms are supported; the
witnessed aggregate account is recomputed even though it does not sign the
refund. Owner-only durable reservation precedes byte exposure, identical
restart replay performs no nonce lookup, and transaction or identity mutations
fail closed. Five planner tests, one authenticated prepare/restart test, and
nine finalized-observer tests are GREEN. Prepare persists its canonical request
and result, restores the planner before bind, and replays byte-identically with
no nonce lookup. State-only observation brackets Funded or Refunded accounts at
one stable finalized clock; exact/discovery scan only fully covered finalized
ancestry, require equal by-ID/by-hash blocks, enforce the containing timestamp at
or after the signed deadline, and prove historical plus tip Refunded state with
zero custody. The authenticated observation is repeatable and never submits.
The actor-local recovery store now replays the ordered maker- then taker-funded
refund branch to terminal `Refunded` in both directions without changing old
happy-path evidence bytes. Public `recover` composes both LEZ and Bitcoin
refund legs through revision 2 to 3 to 4 in deterministic role tests. Exact
bytes are durable before the one `Prepared` to `Started` CAS; only affirmative
stable eligibility grants one attempt, and `Started`, `Unknown`, or `Accepted`
never rearm. Owner and nonowner roles project only later exact finalized
evidence. Run `m3refund-20260716h` now closes the fresh two-direction
actual-node refund evidence boundary recorded by ADR 0038; the separate
refund-side absent-maker boundary is GREEN in `m3firstlock-20260716h`. Commit
`3d202f7` closes canonical containing-time cutoff enforcement. Commits
`4fb6950` and `79d7e68` add exact unspent eligibility, one-shot Bitcoin
funding, role-fixed exact plans, current LEZ first-lock state, and durable
ordered Maker authority. Commit `8870910` composes those pieces through the
schema-4 typed actor seam and keeps schema 3 observation-only. Live CLI
composition and its actual-node admission packet are GREEN in both directions
in `m3schema4-20260717d`. The LEZ path uses exact-idempotent
same-ID/same-bytes initialization, ordered funding, finalized transaction
evidence, and current `Funded` custody. The Bitcoin path joins finalized/current
Taker LEZ evidence to exact Core observation, one actor-authorized send, and
canonical readback. Pending-level LEZ absence remains unavailable and is not
claimed; the idempotent path grants one attempt without calling uncertainty
absence. The opposite-direction two-swap concurrency checkpoint is GREEN in
`m3overlap-20260717a`; arbitrary-N/same-direction nonce scheduling,
process-kill, reorg, and broader hardening remain open.

The older retained actual-Core run remains a one-process public deterministic
cryptographic and consensus fixture. The operator-composed run closes live
witnessed submission, both happy directions, and the PoC atomicity/recovery
order through separate role processes. The public actor source owns both claim
effects, and schema-4 run D additionally owns both direction-selected Maker
locks. This closes that private-local happy checkpoint. Subsequent pushed work
closes the accepted native/custom-token, public full-lifecycle SDK, Testnet4
setup contract, vector, recording, and construction-mapping outputs at the
private functional boundary. It does **not** claim arbitrary-N/same-direction
nonce scheduling, live Testnet4 execution, production key custody,
QA/chaos/infosec, formal review, production readiness, or accepted disposition
of GW-M3-001. The exact private-functional boundary is certified by
`m3-complete`; later hardening remains outside that tag. CI
runs the same P2TR funding/claim
composition and
fail-hard scans
the exact Core image for HIGH/CRITICAL vulnerabilities. The earlier clean
infrastructure run remains available as
[secret-safe Core evidence](docs/evidence/m3-bitcoin-core-smoke-a7393df-20260714.json);
the historical clean known-key P2TR run remains as
[secret-safe funding/claim evidence](docs/evidence/m3-bitcoin-core-p2tr-4f7b6b3-20260715.json),
and strict clean run `m3-musig-exact-f5a9caa` on pushed commit `f5a9caa` is
retained as
[MuSig2/adaptor/extraction Core evidence](docs/evidence/m3-bitcoin-core-musig2-f5a9caa-20260715.json).
Runtime uses no public RPC, faucet, public funds, public peers, or public chain.
Cold setup still depends on checksum-verified Core release assets, the pinned
base image, vulnerability data, and locked Rust registries, so availability and
scan flakiness remain explicit. GnuPG uses a run-owned home, and the verifier
terminates only that home's agent rather than using broad host cleanup.

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
  The M2 lane introduced a bounded authenticated eight-method LEZ sidecar
  client; the current M3 lane extends the same protocol to fourteen methods,
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
  ephemeral loopback listeners. All eight M2 methods execute, and the six M3
  witnessed preparation, finalized-funding, and finalized-claim methods are
  separately covered. Native
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

### M3 schema-4 actor-owned Maker-lock local PoC quick start

First complete the M3 guide's pinned LEZ verifier so the checked guest deployer
exists, and provision the clean LEZ v0.2 source, exact service binaries, r0vm,
four verified Rapidsnark libraries, and offline Cargo graphs. Then let the
repository-owned runner create both local chains and both fresh directions.

The exact root and sidecar build commands are retained under
[Prerequisites and builds](docs/m3-local-poc-operator-guide.md#prerequisites-and-builds).

```sh
export RUN_ID=m3schema4-manual-001
export LEZ_V02_SOURCE_DIR=/absolute/path/to/clean/logos-execution-zone-v0.2.0
export LEZ_V02_SERVICES_DIR=/absolute/path/to/locked/release-binaries
export LEZ_V02_R0VM=/absolute/path/to/verified/r0vm
export LEZ_V02_ARTIFACT_TARGET_DIR=/absolute/path/to/verified/lez-artifact-target
export RAPIDSNARK_LIB_DIR=/absolute/path/to/verified/rapidsnark-v0.0.8-libraries
export BINDGEN_EXTRA_CLANG_ARGS=-I/usr/lib/gcc/x86_64-linux-gnu/13/include
./scripts/run-m3-actor-local-poc.sh

export M3_EVIDENCE=".e2e/${RUN_ID}/m3-actor-poc/evidence"
jq -e '.result == "passed" and
  .journey == "claim" and
  (.directions | map(.direction) ==
    ["taker_sells_foreign", "taker_sells_lez"]) and
  all(.directions[];
    .terminal_revision == 4 and .terminal_phase == "completed" and
    .maker_second_lock_effect_count == 1 and
    .expected_unique_effects == {bitcoin:2,lez:3}) and
  .actor_process_model == "fresh_one_shot_process_per_command" and
  .replay_resubmission_count == 0 and
  .services.bitcoin_core.version == "31.1" and
  .services.bitcoin_core.network == "regtest" and
  .services.lez.version == "v0.2.0" and
  .services.lez.network == "private_local" and
  .external_resources.certification_success_depends_on_external_network == false and
  .public_rpc_used == false and .faucet_used == false and
  .public_funds_used == false and .private_material_disclosed == false' \
  "$M3_EVIDENCE/m3-actor-local-poc.json"

M3_DRIVER_CONTRACT="$(M3_POC_JOURNEY=claim \
  ./scripts/run-m3-actor-direction.sh contract)"
jq -e '
  .runtime_backend == "repository_owned_actual_node_implementation" and
  .actor_config_schema_version == 4 and
  .taker_first_lock_external_runner_submission == true and
  .actor_owned_maker_lock_effects == true and
  .maker_lock_submission_actor_output == "awaiting_observation" and
  .maker_lock_restart_never_resubmits == true and
  .runner_only_confirms_actor_submitted_maker_locks == true and
  .bounded_read_only_observation_retries_never_resubmit == true' \
  <<<"$M3_DRIVER_CONTRACT"
unset M3_DRIVER_CONTRACT

jq -e '.result == "passed" and .all_exact_run_resources_absent == true and
  .foreign_resources_targeted == false and .broad_cleanup_used == false' \
  "$M3_EVIDENCE/cleanup-attestation.json"
```

These assertions distinguish the user roles. The harness uses Taker authority
only for the external first lock. The schema-4 Maker actor owns the opposite
lock, returns `awaiting_observation` after a possible send, and must later close
revision two only from exact current and canonical/finalized evidence. The
controller may mine or wait for local finality and invoke fresh actor processes;
it may not submit a Maker lock on the actor's behalf.

To reproduce the opposite-direction overlapping checkpoint, keep the same
verified prerequisite variables, select the overlap schedule, and use a fresh
run ID:

```sh
export RUN_ID=m3overlap-manual-001
export M3_ACTOR_POC_SCHEDULE=overlap
./scripts/run-m3-actor-local-poc.sh

export M3_EVIDENCE=".e2e/$RUN_ID/m3-actor-poc/evidence"
jq -e '.kind == "m3_actor_overlapping_two_swap_local_poc" and
  .journey == "claim" and .schedule == "overlap" and .result == "passed" and
  all(.directions[];
    .terminal_revision == 4 and .terminal_phase == "completed" and
    .maker_second_lock_effect_count == 1 and
    .expected_unique_effects == {bitcoin:2,lez:3}) and
  .concurrency == {
    simultaneous_in_flight:true,overlap_revision:2,
    overlap_phase:"both_legs_locked",distinct_funding_outpoints:true,
    distinct_agreements:true,distinct_actor_state_dbs:true,
    distinct_signing_journals:true,distinct_signer_sessions_per_domain:true,
    distinct_escrows:true,distinct_deadlines:true,
    chain_mutations_serialized_for_exact_observation:true,
    shared_local_nodes:true,shared_fixture_custody_key:true,
    arbitrary_n_or_same_direction_scheduler_proven:false} and
  .replay_resubmission_count == 0 and
  .external_resources.certification_success_depends_on_external_network == false' \
  "$M3_EVIDENCE/m3-actor-local-poc.json"
jq -e '.result == "passed" and .all_exact_run_resources_absent == true and
  .foreign_resources_targeted == false and .broad_cleanup_used == false' \
  "$M3_EVIDENCE/cleanup-attestation.json"
unset M3_ACTOR_POC_SCHEDULE
```

The local funding fixture uses one deterministic test-custody key, but assigns
the swaps two distinct mature coinbase outpoints and distinct planned Bitcoin
anchors. The shared key is fixture-only custody; it is not a shared outpoint,
agreement, actor store, escrow, deadline, or signing session. The runner keeps
both swaps at revision 2 until it has proved four distinct actor databases,
eight distinct signer journals, two sessions per signing domain, and two
escrows, then releases settlement. Chain mutations are deliberately serialized
so exact mempool and finalized-history assertions stay strict; this proves two
overlapping in-flight swaps, not arbitrary-N throughput or same-direction LEZ
nonce scheduling.

The retained secret-safe overlap packet can be checked independently in one
command:

```sh
jq -e '.result == "passed" and .run_id == "m3overlap-20260717a" and
  .provenance.repository_commit == "1e6d5f1b9205aafb2df427f5285ff0920406b7d1" and
  .milestone_boundary.simultaneously_in_flight_swaps == 2 and
  .milestone_boundary.terminal_swaps == 2 and
  .concurrency_contract.actor_state_database_count == 4 and
  .concurrency_contract.signing_journal_count == 8 and
  .concurrency_contract.distinct_bitcoin_sessions == 2 and
  .concurrency_contract.distinct_lez_sessions == 2 and
  .cross_swap_effect_isolation.bitcoin_effect_ids_pairwise_disjoint == true and
  .cross_swap_effect_isolation.lez_effect_ids_pairwise_disjoint == true and
  .cross_swap_effect_isolation.terminal_replay_resubmission_count == 0 and
  .topology.isolation.all_exact_run_resources_absent_after_run == true' \
  docs/evidence/m3-overlapping-two-swap-poc-20260717.json
```

The runner signs a journey-specific maker-second-lock window so the actual-node
lock can be prepared, finalized, and admitted reproducibly without weakening the
refund reaction margin. `claim` and `survivor_claim` use a cutoff 1,800 seconds
after agreement preparation; `refund` uses 300 seconds and moves its earlier
refund bound to 900 seconds; `first_lock_refund` intentionally fixes the cutoff
at preparation time because no maker lock is permitted. Bitcoin admission uses
the canonical containing block's median time, while LEZ admission uses the
finalized containing block timestamp. Inclusion after the signed cutoff must
remain revision 1 and fail closed.

To reproduce the actual-node timeout/refund journey, keep the same verified
prerequisite variables but select `refund` and a different, never-before-used run
ID. Change the example ID for every attempt; a failed or successful root is
spent and the runner refuses to reuse it.

```sh
export RUN_ID=m3refund-manual-001
export M3_ACTOR_POC_JOURNEY=refund
./scripts/run-m3-actor-local-poc.sh

export M3_EVIDENCE=".e2e/$RUN_ID/m3-actor-poc/evidence"
jq -e '.kind == "m3_actor_two_direction_refund_local_poc" and
  .journey == "refund" and .result == "passed" and
  all(.directions[];
    .terminal_revision == 4 and .terminal_phase == "refunded") and
  .expected_unique_effects_by_direction == {
    taker_sells_foreign:{bitcoin:2,lez:3},
    taker_sells_lez:{bitcoin:2,lez:3}} and
  .replay_command == "recover" and .replay_resubmission_count == 0 and
  .services.bitcoin_core == {run_id: (.run_id + "-btc"),
    version: "31.1", network: "regtest"} and
  .services.lez.version == "v0.2.0" and
  .services.lez.network == "private_local" and
  .services.lez.slot_duration_seconds == "3.0" and
  .public_rpc_used == false and .faucet_used == false and
  .public_funds_used == false and .private_material_disclosed == false' \
  "$M3_EVIDENCE/m3-actor-local-poc.json"
jq -e '.journey == "refund" and .result == "passed" and
  .all_exact_run_resources_absent == true and
  .foreign_resources_targeted == false and .broad_cleanup_used == false' \
  "$M3_EVIDENCE/cleanup-attestation.json"
```

The refund runner waits for the countersigned deadlines and executes the two
directions sequentially. Run H's retained evidence-to-cleanup span was 54
minutes 5 seconds with 3.0-second LEZ slots; local load, finality progress, and
moving-tip retries can extend that. A timeout is uncertain observation and
never grants another submission.

To reproduce the first-lock absent-maker journey, keep the verified
prerequisites, choose another fresh run ID, and select `first_lock_refund`:

```sh
export RUN_ID=m3firstlock-manual-001
export M3_ACTOR_POC_JOURNEY=first_lock_refund
./scripts/run-m3-actor-local-poc.sh

export M3_EVIDENCE=".e2e/$RUN_ID/m3-actor-poc/evidence"
jq -e '.kind == "m3_actor_two_direction_first_lock_refund_local_poc" and
  .journey == "first_lock_refund" and .result == "passed" and
  all(.directions[];
    .terminal_revision == 2 and .terminal_phase == "refunded" and
    .maker_second_lock_effect_count == 0) and
  .expected_unique_effects_by_direction == {
    taker_sells_foreign:{bitcoin:2,lez:0},
    taker_sells_lez:{bitcoin:0,lez:3}} and
  .first_lock_refund_admission.two_fresh_cross_chain_reads == true and
  .first_lock_refund_admission.fresh_maker_observer == true and
  .replay_resubmission_count == 0 and
  .external_resources.certification_success_depends_on_external_network == false' \
  "$M3_EVIDENCE/m3-actor-local-poc.json"
jq -e '.journey == "first_lock_refund" and .result == "passed" and
  .all_exact_run_resources_absent == true and
  .foreign_resources_targeted == false and .broad_cleanup_used == false' \
  "$M3_EVIDENCE/cleanup-attestation.json"
```

The directions run sequentially and intentionally wait for signed boundaries.
Advancing LEZ finality can yield typed `moving_tip`; only that read-only
condition is retried with a fresh request ID, and retained retry evidence proves
the durable LEZ submission count did not change.

To reproduce the post-reveal survivor journey, keep the same verified
prerequisites, use another fresh ID, and select `survivor_claim`:

```sh
export RUN_ID=m3survivor-manual-001
export M3_ACTOR_POC_JOURNEY=survivor_claim
./scripts/run-m3-actor-local-poc.sh

export M3_EVIDENCE=".e2e/$RUN_ID/m3-actor-poc/evidence"
jq -e '.kind == "m3_actor_two_direction_survivor_claim_local_poc" and
  .journey == "survivor_claim" and .result == "passed" and
  .survivor.revealer == "taker" and .survivor.follower_role == "maker" and
  .survivor.protected_absence.revealer_actor_invocation_count == 0 and
  .survivor.intermediate.phase == "claim_evidence_available" and
  .survivor.intermediate.lifecycle_disposition == "recovering" and
  .survivor.intermediate.terminal == false and
  .survivor.intermediate.remaining_leg_canonical_and_claimable == true and
  .survivor.delayed_revealer_catchup.observation_only == true and
  .survivor.delayed_revealer_catchup.bitcoin_successful_resubmission_count == 0 and
  .survivor.delayed_revealer_catchup.lez_successful_resubmission_count == 0 and
  .survivor.delayed_revealer_catchup.successful_resubmission_count == 0 and
  all(.survivor.direction_evidence[];
    .completion_boundary.completed_before_signed_refund_boundary == true and
    (.completion_evidence_sha256 | test("^[0-9a-f]{64}$")) and
    (.recovering_evidence_sha256 | test("^[0-9a-f]{64}$"))) and
  all(.directions[];
    .terminal_revision == 4 and .terminal_phase == "completed") and
  .expected_unique_effects_by_direction == {
    taker_sells_foreign:{bitcoin:2,lez:3},
    taker_sells_lez:{bitcoin:2,lez:3}} and
  .replay_resubmission_count == 0' \
  "$M3_EVIDENCE/m3-actor-local-poc.json"
jq -e '.journey == "survivor_claim" and .result == "passed" and
  .all_exact_run_resources_absent == true and
  .foreign_resources_targeted == false and .broad_cleanup_used == false' \
  "$M3_EVIDENCE/cleanup-attestation.json"
```

This is the actual-user role split: taker reveals and disappears, one fresh
maker process commits revision 3, a later fresh maker process completes, and
the taker catches up only after maker terminality. The remaining Bitcoin or LEZ
leg must be independently canonical and claimable before its refund boundary.
Runtime uses only the same isolated literal-loopback nodes and deterministic
local funds described below; bounded finalized-tip retries can extend runtime
but never grant another send.

The audited run used the same entry point at `6ded2f9`; these are its exact
run-owned and native-artifact inputs. This is an audit record, not a command to
reuse—the runner must reject the existing run ID and root:

```sh
RUN_ID=m3actor-20260716n \
LEZ_V02_SOURCE_DIR=/tmp/lez-v020-native-investigation \
LEZ_V02_SERVICES_DIR=/tmp/lez-v02-services-a58fbce2-20260713/release \
LEZ_V02_R0VM=/tmp/lez-atomic-swaps-tools/risc0-3.0.5/home/extensions/v3.0.5-cargo-risczero-x86_64-unknown-linux-gnu/r0vm \
LEZ_V02_ARTIFACT_TARGET_DIR=/tmp/lez-m3-artifact-20260715a \
RAPIDSNARK_LIB_DIR=/tmp/lez-atomic-swaps-tools/rapidsnark-v0.0.8/d4133227 \
BINDGEN_EXTRA_CLANG_ARGS=-I/usr/lib/gcc/x86_64-linux-gnu/13/include \
./scripts/run-m3-actor-local-poc.sh
```

Its terminal and cleanup packets remain at
`.e2e/m3actor-20260716n/m3-actor-poc/evidence/`. The containing run root remains
owner-private because it also retains credentials, keys, signed transactions,
and actor/signer state; publish only separately reviewed secret-safe summaries.

Use a never-before-used 8–48 character lowercase run ID. The runner refuses
pre-existing run roots or same-ID Docker resources, uses dynamic literal-loopback RPC ports, defaults to sequential directions, and uses the explicit
`M3_ACTOR_POC_SCHEDULE=overlap` revision-two barrier only when selected. It
cleans only captured exact IDs on success or failure. Its root and sidecar builds are
offline by design; populate their pinned caches before starting. Core release
and Guix provenance, the LEZ/Risc0 artifacts, Docker images, and cold Cargo/git
inputs are setup dependencies and can fail because of DNS, registry, or host
availability. Runtime chain I/O uses only local Core Regtest and the local LEZ
Bedrock/sequencer/indexer; funds are fresh local Regtest/genesis outputs, with
no public chain RPC, peer, faucet, public deployment, or public funds. The
pinned Bedrock binary also makes best-effort UDP NTP attempts to
`pool.ntp.org:123`; certification does not depend on success, and the M3 runner
records the observed timeout count. Thus the chain boundary is fully local, but
the Bedrock process is not claimed to make zero egress attempts. See the
[M3 operator guide](docs/m3-local-poc-operator-guide.md) for exact builds,
proof boundaries, private evidence handling, and failure recovery.

### Private M3 terminal-recording quick start

Commits `a3c6b21` and `269bbad` provide the fail-closed recorder and complete
three-scenario bundle verifier. Three live actual-node source recordings are GREEN at
clean pushed evidence commit `a6eb1ad`; verifier commit `946208a` sealed their
private bundle. The exact reference run IDs are
`m3record-happy-20260718ag`, `m3record-refund-20260718ag`, and
`m3record-concurrent-20260718ag`. The runner's JSON evidence packet is not a
recording: it is a hash-bound input to the replayable terminal output and timing
stream. They are inputs to, not substitutes for, the RFP's three demo videos.

Run from a clean committed checkout on a Linux host with the M3 runner's Docker,
Rust/Cargo, LEZ, and native-library prerequisites. The recording layer also
requires Git, jq, util-linux `script`/`scriptreplay`, `sha256sum`, `stat`, and
`realpath`. Keep the verified values below unchanged across all three runs.
Replace only the absolute paths with the locally verified pinned inputs:

```sh
export LEZ_V02_SOURCE_DIR=/absolute/path/to/clean/logos-execution-zone-v0.2.0
export LEZ_V02_SERVICES_DIR=/absolute/path/to/locked/release-binaries
export LEZ_V02_R0VM=/absolute/path/to/verified/risc0-3.0.5-r0vm
export LEZ_V02_ARTIFACT_TARGET_DIR=/absolute/path/to/verified/lez-artifact-target
export RAPIDSNARK_LIB_DIR=/absolute/path/to/verified/rapidsnark-v0.0.8-libraries
export BINDGEN_EXTRA_CLANG_ARGS=-I/usr/lib/gcc/x86_64-linux-gnu/13/include

unset M3_RECORDING_TESTING M3_RECORDING_TEST_DRIVER
unset M3_RECORDING_TEST_EVIDENCE_FILE M3_RECORDING_BUNDLE_TESTING
test -z "$(git status --porcelain=v1 --untracked-files=normal)"
export D1_COMMIT="$(git rev-parse --verify HEAD)"
export D1_STAMP="$(date -u +%Y%m%d%H%M%S)"
export HAPPY_RUN_ID="m3record-happy-${D1_STAMP}"
export REFUND_RUN_ID="m3record-refund-${D1_STAMP}"
export CONCURRENT_RUN_ID="m3record-concurrent-${D1_STAMP}"
```

Use these exact three live commands. Each fresh ID is valid for one attempt
only; a failed run deliberately retains its private diagnostics and must not be
overwritten or reused.

```sh
RUN_ID="$HAPPY_RUN_ID" M3_RECORDING_SCENARIO=happy ./scripts/record-m3-private-demo.sh
RUN_ID="$REFUND_RUN_ID" M3_RECORDING_SCENARIO=refund ./scripts/record-m3-private-demo.sh
RUN_ID="$CONCURRENT_RUN_ID" M3_RECORDING_SCENARIO=concurrent ./scripts/record-m3-private-demo.sh
```

The scenario bindings are exact: `happy` runs the sequential two-direction
claim journey, `refund` runs the sequential two-lock timeout/refund journey,
and `concurrent` runs the claim journey with the opposite-direction
revision-two overlap barrier. The corresponding packet kinds are
`m3_actor_two_direction_local_poc`,
`m3_actor_two_direction_refund_local_poc`, and
`m3_actor_overlapping_two_swap_local_poc`.

By default each private directory is
`.e2e/<RUN_ID>/m3-recordings/<scenario>/` with mode `0700`. It contains
`terminal.typescript`, `terminal.timing`, and `recording.json`, each mode
`0600`. The separately retained and hash-bound actual-node packet is
`.e2e/<RUN_ID>/m3-actor-poc/evidence/m3-actor-local-poc.json`. Keep the whole
`.e2e` tree private; it can also contain credentials, signer material,
transactions, and actor databases. An optional `M3_RECORDING_PRIVATE_ROOT`
must be an absolute non-symlink path; an in-repository root must be ignored by
Git.

Replay from each recording directory, for example:

```sh
(cd ".e2e/$HAPPY_RUN_ID/m3-recordings/happy" && scriptreplay --log-timing terminal.timing --log-out terminal.typescript)
(cd ".e2e/$REFUND_RUN_ID/m3-recordings/refund" && scriptreplay --log-timing terminal.timing --log-out terminal.typescript)
(cd ".e2e/$CONCURRENT_RUN_ID/m3-recordings/concurrent" && scriptreplay --log-timing terminal.timing --log-out terminal.typescript)
```

Do not change commits or tracked files between runs. After all three pass, this
exact command verifies modes, replayability, all hashes, scenario/evidence
bindings, node versions, three unique run IDs, and one shared repository commit
before atomically creating a private mode-`0600` bundle index:

```sh
export D1_BUNDLE="$PWD/.e2e/m3record-bundle-${D1_STAMP}/recording-bundle.json"
M3_RECORDING_BUNDLE_OUTPUT="$D1_BUNDLE" \
  ./scripts/verify-m3-private-recording-bundle.sh \
  "$PWD/.e2e/$HAPPY_RUN_ID/m3-recordings/happy/recording.json" \
  "$PWD/.e2e/$REFUND_RUN_ID/m3-recordings/refund/recording.json" \
  "$PWD/.e2e/$CONCURRENT_RUN_ID/m3-recordings/concurrent/recording.json"
test "$(jq -er '.repository_commit' "$D1_BUNDLE")" = "$D1_COMMIT"
```

Those source captures are not the literal D1 video deliverable. With the same
clean checkout, pull the exact MIT-licensed renderer once, generate an animated
role-flow MP4 from each verified actual-node packet, and seal the three-video
bundle:

```sh
export M3_VHS_IMAGE='ghcr.io/charmbracelet/vhs@sha256:9d5fc3dc0c160b0fb1d2212baff07e6bdf3fa9438c504a3237484567302fcf93'
docker pull "$M3_VHS_IMAGE"
export D1_VIDEO_ROOT="$PWD/.e2e/m3-private-demo-videos-${D1_STAMP}"

./scripts/render-m3-private-demo-video.sh \
  "$PWD/.e2e/$HAPPY_RUN_ID/m3-recordings/happy/recording.json" "$D1_VIDEO_ROOT"
./scripts/render-m3-private-demo-video.sh \
  "$PWD/.e2e/$REFUND_RUN_ID/m3-recordings/refund/recording.json" "$D1_VIDEO_ROOT"
./scripts/render-m3-private-demo-video.sh \
  "$PWD/.e2e/$CONCURRENT_RUN_ID/m3-recordings/concurrent/recording.json" "$D1_VIDEO_ROOT"

M3_PRIVATE_DEMO_VIDEO_BUNDLE_OUTPUT="$D1_VIDEO_ROOT/video-bundle.json" \
  ./scripts/verify-m3-private-demo-video-bundle.sh \
  "$D1_VIDEO_ROOT/happy/video.json" \
  "$D1_VIDEO_ROOT/refund/video.json" \
  "$D1_VIDEO_ROOT/concurrent/video.json"
```

Rendering uses the already-pulled digest with no container network, a read-only
root, dropped capabilities, no-new-privileges, bounded resources, and only the
private output mounted. The final verifier regenerates the proof from current
terminal, role, effect, refund, and concurrency packets; re-hashes all inputs;
and decode-probes every live MP4. Any source or video tamper fails. The image
pull alone depends on registry/DNS/TLS availability; rendering and milestone
evidence use no public RPC, faucet, public funds, or chain network.

The retained reference render is private under
`.e2e/m3-private-demo-videos-20260719c/`. It binds source commit `a6eb1ad` to
renderer/verifier commit `846ba56`; its mode-`0600` bundle passed with SHA-256
`7697a27c80c8f90856d6592051805a8923fe564aa01b0dff4109bd5c5f101ba8`.
Happy, refund, and concurrent MP4s are respectively 21.64, 20.36, and 20.36
seconds. Their intro, both-direction, scenario-specific, atomicity, and stable
tail frames were sampled after complete stream decode. The surrounding
`.e2e` evidence remains owner-private and must not be published.

The bundle JSON is a verifier-produced index, not a recording. Live verification
rejects fixture/test-contract manifests, a dirty or different current HEAD,
mixed commits or node versions, duplicate scenarios/runs, changed bytes,
missing evidence, unsafe modes, non-replayable output, and an existing bundle
path. A failed driver leaves the terminal stream and timing file as private
diagnostics but creates no passing `recording.json`; a failed bundle creates no
passing bundle.

The retained private bundle is mode `0600`, result `passed`, binds evidence
commit `a6eb1ada739f8fcd671feb8fbb41cfc682e5d651` to verifier commit
`946208a887709d9b8422f51f8152a3008c6d745a`, and has SHA-256
`3d7d7adc12571a610be21a18b746e68cb17311ea1224191fcdcdf1b39a86c7cc`.
It is intentionally ignored rather than published; operators can reproduce
and verify the same schema with fresh unique run IDs using the commands above.

All three journeys start actual run-owned Bitcoin Core 31.1 Regtest and LEZ
v0.2 private-local Bedrock/sequencer/indexer services on isolated loopback
endpoints. Funds are deterministic local Regtest coinbase and LEZ genesis/Vault
outputs. No public chain RPC, peer, faucet, deployment, or public funds are
used. Pinned Bedrock may make best-effort UDP NTP requests to
`pool.ntp.org:123`; success is neither required nor trusted, and finalized
chain timestamps remain authoritative. The refund recording intentionally
waits through both signed recovery schedules. The retained reference took
54 minutes 5 seconds at 3.0-second LEZ slots; host load, finality, and bounded
moving-tip retries can extend it without authorizing another send.

The detailed failure contract and operator checks are in
[the M3 local PoC operator guide](docs/m3-local-poc-operator-guide.md#private-d1-btc-recording-bundle).

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
RPC, faucet, credential, or public funds. The pinned LEZ Bedrock component can
make best-effort UDP NTP attempts to `pool.ntp.org:123`; local certification
requires no successful reply and records observed timeout attempts. This is an
external egress attempt, not a public chain dependency. Public-route parsing,
TLS client
construction, credential loading/redaction, and strict LEZ profile selection
are tested without connecting.

The M4 Monero runner's **runtime** resource list is also empty: official
`monerod` and three official wallet RPCs run on a non-masquerading project
bridge, only authenticated RPC is bound to random literal-loopback ports, and
P2P/ZMQ are not published. All 110 bootstrap/confirmation blocks and funds are
local Regtest effects. **Cold setup** can require the exact official 0.18.5.1
archive, the pinned distroless image digest, and a live Monero source-tag
identity lookup. DNS, TLS, download host, registry, or Git host outages can
therefore delay a cold run. The clearsigned hash manifest and signer key are
retained locally; the 85 MB verified archive cache avoids repeated downloads
without bypassing provenance checks. Public stagenet peers, sync, funding, and
reorg behavior remain unmeasured and are not inferred from Regtest. The actual
M4 claim checkpoint used those real official local processes through loopback,
not daemon or wallet mocks; its LEZ and Monero runtime resource list is empty.
The [Monero Stagenet setup guide](docs/monero-stagenet-setup.md) separately
documents pinned verification, self-hosted and untrusted public-node routes,
role wallet RPCs, funding, security, cleanup, and flakiness without claiming a
public run.

The M4 checked-artifact runner also records runtime resources as empty. Its
fresh recursive test opens no RPC or public service, but cold setup can require
the pinned circuits release, crates.io and pinned Git sources, the
digest-pinned guest-builder image, and Risc0 tool releases. Those DNS,
registry, rate-limit, and availability inputs can delay setup without changing
the checked ELF identity. Default cleanup retains the small evidence ELF and
removes the exact run-owned target/tools; the two certification runs reclaimed
about 3.49 GiB. The bridge-client and Monero-adapter unit/contract suites use
no node, faucet, or public endpoint after dependencies are present.

Schema-4 run D used one run-owned Bitcoin Core 31.1 Regtest daemon and one
run-owned LEZ v0.2 Bedrock/sequencer/indexer tuple, all on allocated literal
loopback endpoints. Regtest coinbase outputs and fresh local genesis/Vault
allocations supplied deterministic funds. It used no public RPC, public peer,
faucet, public deployment, or public funds. Bedrock attempted optional NTP and
recorded 45 timeouts, but certification did not require external-network
success. An advancing local finalized tip produced bounded typed `moving_tip`
reconciliation. Every retry checked the exact durable LEZ count or Bitcoin
mempool transaction; it could delay the run but could not rearm a send.

Bitcoin Testnet4 support is locally contract-tested but was not contacted.
Self-hosting introduces public P2P synchronization, initial-index time, peer
partitions, organic reorgs, and disk/network availability. The exact HTTPS
route introduces DNS, platform CA roots/system clock, credentials, provider
quota/method/index policy, lag, outage, and ambiguous-broadcast risk. A faucet
or donor wallet has no SLA and its returned txid is untrusted until confirmed
through the selected node. The adapter allows no redirect, automatic retry,
proxy, or route failover; every route must report exact Core 31.1,
`chain=testnet4`, Testnet4 genesis, and synchronized `txindex` plus
`txospenderindex`. See the
[Bitcoin Testnet4 setup guide](docs/bitcoin-testnet4-setup.md). None of these
external resources can make the private Regtest/LEZ certification pass or fail.

The successful M2 corridor used dynamic-loopback
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

The M3 root agreement provisioner is not a network service and performs no RPC
or Docker action. On warm caches its only external runtime input is the OS
random source; the official account helper also performs no RPC but an uncached
build shares the separate pinned sidecar graph's Cargo/git/native-library
availability. The actual run-owned Core and LEZ observations surround the
three local commands. Core `gettxout` precedes `prepare-funding`; read-only
`testmempoolaccept` of its exact persisted bytes and finalized LEZ preparation
facts precede agreement finalization; exact broadcast and the planned one-block
mine occur only after both chain presignatures are durable. Policy/finality
delay, moving tips, local readiness, or operator transcription can make the
ceremony fail closed, but must never be treated as permission to fabricate a
fact, relax a policy, reuse the output root, or overwrite its create-new files.

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

## M5 local-functional PoC closure candidate

Current status on 2026-08-02: **verified 7 of 7 under the progressive-JPEG
local-functional PoC policy; tag `m5-poc-complete` binds this
closure.** This supersedes
the earlier 4-of-7 current-status statements above without rewriting their
historical evidence. It does not claim production readiness or public
deployment.

The literal accepted RFP-003 issue #112 deliverables are now composed as follows:

| Deliverable | PoC evidence |
|---|---|
| Long-running Maker daemon | Retained hardened service and daemon-supervised ZEC actual-chain corridor |
| Maker CLI | Real CLI/daemon all-pair claim/refund admission matrix, GREEN 1 of 1 in 0.64 seconds |
| Taker CLI | Retained BTC/ZEC lifecycle corridors plus receipt-v2 XMR Tag14 claim and Tag16 refund process routes |
| Coordinator persistence, crash, and concurrency | One daemon/database/worker pool runs three pair-correct rows; unavailable then failing XMR does not prevent BTC and ZEC Terminal, GREEN 1 of 1 in 16.31 seconds |
| Price sources | Retained local and Logos-module C-API pricing evidence |
| Delivery and Chat degradation | Retained post-lock transport removal, replay, and degraded-state evidence |
| Coordinator fuzzing | Retained literal fuzz target, seeds, and smoke evidence |

The XMR receipt-v2 Tag16 user path is GREEN 1 of 1 in 106.26 seconds: an
injected rejected preflight sends nothing, leaves the CAS available, and is
successfully retried; the accepted attempt then sends once and leaves Started;
the second runs the role-fixed observer and reconciles Succeeded; the third is
Complete without a process; and the losing claim fails closed. The all-pair
Maker matrix exposed a real Bitcoin
manual-claim RED at JSON-RPC code `-32602`; production now preserves the user
claim intent while mapping it to Bitcoin's semantic `drive` command. The
focused mapping unit is GREEN 1 of 1.

Evidence remains layered. M2/M3/M4 retain real local-devnet chain effects, and
M5 retains clean accepted-application BTC, ZEC, and XMR local-chain corridors.
The new all-pair lifecycle and three-pair overlap tests use fixed marker actors:
they prove real CLI, daemon, SQLite, scheduling, fencing, child custody,
failure isolation, and restart/no-replay composition, but no new chain effect.
Semantic receipt-v2 XMR worker adapters and a fresh simultaneous
accepted-application actual-chain composite are post-PoC QA and production
hardening. See [the manual closure-candidate flow](docs/manual-user-flows.md#m5-poc-closure-candidate-reproduction).


## Licensing

Licensed under either the Apache License, Version 2.0 or the MIT License, at
your option.
