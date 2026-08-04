# Requirements traceability

Last reconciled: 2026-07-31 against live RFP-003 master
`b59c620...16d` and Gateway's accepted replacement proposal #112. Issue #61 is
superseded. No authority or seven-output M5 scope drift was found. Exact pushed
run `m5-xmr-app-20260730-2c6aec1-h` recorded a clean cryptographic, finalized-chain, binding, and cleanup replay of the XMR application
corridor through isolated LEZ v0.2 and official Monero 0.18.5.1 Regtest: one
authenticated application identity crossed the synchronous cutoff, finalized
tag 13/14/15, adaptor extraction, a ten-confirmation sweep, cross-chain binding,
and schema-v2 exact cleanup. The binder claims conditional successful-claim
atomicity, not a distributed transaction or future-reorg immunity. M5 remains 3
of 7 literal outputs because Maker service control, Taker XMR lifecycle,
actual-application concurrency and automatic unavailable-chain
application behavior remain. Explicit selected-route disable/restart/re-enable
is GREEN under ADR 0115. A 2026-07-30 audit withdrew role-correct user-flow certification from H because the runner used provisioner funding, a Taker-hosted shared-wallet RPC, and a Maker destination. Corrected code requires Maker funding, a neutral shared-wallet RPC, and role-correct receipts. Exact pushed-commit refund run `m5xmrrefund45924caa` exercised those boundaries; the historical evidence remains immutable. Refund attempt `m5xmrrefund8c10cd7a` subsequently reached finalized tag 13 and verified Monero funding, then failed closed when the actual local finalized identity did not advance through empty-block polling. ADR 0123 records the contract-GREEN one-reservation, one-attempt terms-sealed Taker-to-Maker unit transfer, followed only by read-only verification and authoritative Maker finality classification. Its protocol, planner, client, runner, and live runtime path are GREEN. Run `m5xmrrefund45924caa` finalized the sealed clock effect and tag 16, confirmed the Maker recovery sweep, bound both chains conditionally, and cleaned exact resources. Literal M5 remains 3 of 7 and no tag is claimed. Pushed replay `m5xmrrefund827a5d4a` reached the signed refund threshold after both devnets, finalized tag 13, and Maker-funded Monero verification, then failed before any clock effect because a prefixed preparation request ID exceeded the 64-character protocol bound. The TDD repair uses versioned, operation-domain-separated SHA-256 IDs; 215 sidecar tests and strict gates pass, and exact run `m5xmrrefund45924caa` closes the actual-node replay gate.

IDs below follow the RFP's Functionality, Usability, Reliability, Performance,
Supportability, and Demo ordering. A row is not acceptance: `Passing` requires
the named evidence at the real actor and chain boundary described by the
requirement.

M4 working-tree delta on 2026-07-21: run `m4happy-40cbac3-20260721a`
executed one complete LEZ-first successful-claim journey through actual isolated
LEZ v0.2 and official Monero 0.18.5.1 Regtest processes. Initialize/Fund
finalized at heights 3953/3960, exact XMR funding reached ten confirmations,
tag 14 finalized at 4107, tag 15 finalized at 4208 with custody zero, and the
reconstructed-key sweep reached tip 130. The public packet deliberately binds
only the base commit plus working-tree status and omits execution-binary hashes;
exact committed-tree replay and scoped cleanup remain before certification.
Signed recovery, F7, U9, D1 XMR, and hardening statuses are unchanged.

The Taker actor subsequently produced one owner-private canonical
`lez_v02_m4_claim_cross_chain_binding_v1` packet. Its public digest is
`896d05d3178e3ff44b6ca010d4528835f5d796dc7e1004984ed78e853c083306`
(3203 bytes, mode `0600`, one link). It binds finalized LEZ Claim height 4208
under tip 4220 to Monero receipt height 121 under stable tip 130, the durable
claim transcript, extraction, and reconstructed key. The retained legacy-v1
sweep plus receipt-v2 path exposes `fee_piconero: null` and a
1808400000-piconero unreceived remainder. Current sweep-v2 exact-fee validation
is focused-tested only. Destination ownership remains an owner-private
Taker-wallet boundary rather than a Stage-A commitment; distributed atomicity
and future-reorg immunity are explicitly not claimed.

M4 delta at `afbd651`: ADR 0070 closes the repository-controlled pre-Fund
component gap. The sidecar now classifies only exact durable Initialize/Fund
targets with effect-specific historical state and stable finalized re-pins;
missing remains `Uncertain`. The concrete Taker adapter mints private-field
non-`Clone` exact finalized-Initialize evidence and consumes it before
authenticated Fund submission. Focused evidence uses synthetic/authenticated
loopback fixtures and no external chain resources. References below to a
remaining finalized-Initialize barrier mean actual-local indexer/sequencer and
actor execution, not missing component code or accepted admission as finality.

M4 deltas at `852b45e` and `d4f4019` supersede the older component counts in
F3/F5 below. The exact local-only deployer validates the pinned M4 manifest,
ELF/ImageID, channel/genesis/runtime, and built-ins before one send and bounded
canonical inclusion; its focused matrix makes zero RPC calls for 19
manifest/runtime mutations and three non-loopback endpoint classes. Four of
seven transaction-building routes are now functional: exact durable tag 13,
tag 14, and Maker tag-15 prepare/complete. ADR 0071 records tag-15's exact
nonce/ABI/account/hash and aggregate-BIP340 checks, separate owner-only records,
and restore-only startup revalidation. The current pinned sidecar component gates are GREEN with strict Clippy and
Rustdoc. Actual-local tag-13 execution, tag-14/tag-15 execution and finality,
adaptor
extraction, actors, and the swap remain. The inherited same-request-ID
concurrent journal overwrite race is tracked for post-PoC hardening and is not
presented as production-safe concurrency.

The current M4 session-descriptor delta closes a separate public-SDK
composition gap. Claim and refund descriptors are minted only by a fully
validated Stage-A agreement, rederive the purpose-separated session identity
inside the SDK, reconstruct the exact retained context, and reject every public
field mutation plus refund-into-claim cross-wiring. ADR 0072 requires actors to
derive protocol inputs from canonical validated Stage-A/Stage-B material. This
is now 16-test SDK evidence, including unsigned semantic validation before
role-indexed signature attachment. It is not an independent role-material
process, chain execution, finality, or an end-to-end swap.
The role-fixed Taker tag-13 executable retains 12 GREEN local tests around canonical
stage input, exact actor/deployment identity, stable finalized nonces,
no-clobber owner-only evidence, the finalized Initialize-before-Fund barrier,
and three finalized-consensus funding-cutoff checks. The actual-local run
finalized Initialize transaction `8013ad91...7676` in block 3008 before Fund
transaction `9b643629...da46` in block 3023 and before the signed cutoff. This
does not change the 0-of-1 M4 happy-swap count. Five focused reusable-finality
tests separately prove stable advancement, moving/regressing finalized views,
wrong genesis, and one-bit ProgramID rejection.
The role-fixed `xmr-reference-actor provision` boundary is GREEN in four tests,
including a true two-process CLI E2E. Separate Taker and Maker invocations each
receive one new root; Maker imports the Taker-generated private view key while
only canonical identity/DLEQ packets are public. Atomic no-replace directory
publication prevents partial private roots, and a private manifest binds role,
owner, and exact packet digest. Readers use one no-symlink descriptor and reject
zero owners plus compressed, x-only, and DLEQ/signing-key aliases. Provisioning
uses OS entropy and no RPC, node, Docker service, faucet, peer, or external
finality resource. It is not Stage-A/B composition or a chain effect.
The authenticated Taker-journal handoff closes the repository-controlled
plaintext-copy gap before tag 14. It opens only an existing durable claim
journal, derives the sole Taker session identity from validated Stage A,
requires the completed signing phase, exact-compares the transcript, nonces,
commitments, Maker partial, and withheld-partial commitment with Stage B, then
passes the independently revalidated partial directly to the existing typed
preparation route. Its 5 focused and 98 full package tests are GREEN; invalid
journals make zero RPC calls. This is still preparation, not publication,
finality, Maker observation, or a swap effect.
The role actor now closes the private Stage-A subpath. Each process validates
its manifest role/owner/packet digest, all three private keys, DLEQ share, view
key, both public packets, and canonical unsigned body before producing one
create-new BIP340 signature. Public assembly accepts only correctly indexed
Maker/Taker signatures and reparses the canonical agreement. Each role derives
the same purpose-separated claim/refund contexts into one exact owner-only
directory exposed by one no-replace rename; a canonical half-bundle cannot
appear. Four provisioning and two black-box Stage-A tests, default and
provision-only strict Clippy, warning-fatal Rustdoc, formatting, and diff gates
are GREEN. Public actual-local composition is now GREEN too: the read-only
sidecar composer used maintained typed Monero RPC plus official LEZ v0.2
sequencer/indexer clients, discovered nonzero height-zero identity, required
exact default escrow prestate and funded dedicated owners, cross-checked the
indexer finalized hash at sequencer block 2281, bracketed stable accounts and
nonces across monotonic live tips, and published one no-clobber canonical wire.
Separate Maker/Taker processes signed it and produced equal same-purpose atomic
session files. Adapter 17/17, composer 10/10, strict Clippy, formatting,
warning-fatal private Rustdoc, isolation policy, and the actual process replay
are GREEN. The canonical continuation is also GREEN. One long-lived SQLite
database per role carries both claim/refund sessions, retaining database-wide
nonce-reuse detection. Existing runner transitions persist commitments before
openings and consume nonces with exact partial outboxes. Only Maker claim/refund
and Taker refund partials enter the exchange. A Taker-only process reconstructs
both completed journal transcripts, commits its private claim partial into a
747-byte unsigned Stage B, and separate Maker/Taker processes countersign the
875-byte SDK-validated activation. The exact-current replay is byte-identical;
the focused black-box test rejects incomplete journals, crossed signatures, and
clobber while proving the exact private Taker claim partial is absent from both
wires. This is pre-effect evidence: every chain effect remains, so the M4 count
is still 0 of 1 swaps.
The 2026-07-28 authority refresh pins the live RFP repository at master commit
`b59c620d6ee8b8281657cbb6eca0381172a8e16d`, RFP-003 file blob
`ec6b602ea4f14627173c9a5236e021d8eff83e16`, and raw-file SHA-256
`a83d0b87ab32e459235a8fea7766519b7fe85ec99d7bcaf1dfe44d329bc3d498`.
Replacement issue #112 is open, retains the `accepted` and `RFP-003` labels,
still supersedes issue #61, and its newline-normalized body SHA-256 remains
`49356263a762307abc0f8dd2863ac5af8fe13d9b17b674f242d025de655f1c87`.
The intervening RFP changes do not alter the technical M5 output set. The
commit/blob
are immutable audit anchors; the issue state is mutable and must be reread
before milestone acceptance.

The executable guard `scripts/check-requirements-traceability.sh` requires every
hard-requirement ID to occur exactly once. RFP Supportability 4 ultimately
requires a corresponding test—not merely a row—for every F, U, R, and P item.

ADR 0023 records an owner-approved stealth M2 certification profile. The M2 tag
requires a fully functional private corridor across one actual
public-compatible local LEZ v0.2 devnet (source-verified immutable Bedrock,
indexer, and non-standalone sequencer pins/wiring, not standalone mock settlement)
and one actual local Zcash Regtest devnet with independent maker/taker
processes, while public deployment/testnet evidence remains visibly incomplete
in this matrix and moves to production readiness. The local and future public
routes must use the same binaries, ports, builders, and validators; only signed
configuration, funding, authentication, confirmation policy, and required LEZ
on-chain deployment may change.

The private happy-path portion of that profile is now GREEN in both accepted
directions. Runs `m2poc-corridor-fresh-20260714o` and
`m2poc-corridor-reverse-fresh-20260714c` used independent maker/taker
processes and stores, the pinned local LEZ v0.2 services, and Zebra Regtest;
both role stores reached revision 4 `Completed`. Their secret-safe evidence is
[`m2-taker-sells-lez-corridor-20260714.json`](evidence/m2-taker-sells-lez-corridor-20260714.json)
and
[`m2-taker-sells-foreign-corridor-20260714.json`](evidence/m2-taker-sells-foreign-corridor-20260714.json).
This meets the two-direction PoC gate. The dormant public-portability contract
is also locally GREEN: exact public-route identity contracts, schema-v3
actor/Zebra routes, and the exact official-public LEZ sidecar route are
contract-tested without public I/O. Two later schema-v3 certification runs also
repeat both accepted directions through the actual local nodes. Final canonical
runs `m2cert-canonical-forward-bb53daf-20260714a` and
`m2cert-canonical-reverse-bb53daf-20260714a` then repeated both directions
against the Docker-built/deployed ProgramId `5cf8c5a4...329c1`: all four actor
stores are terminal, six LEZ effects are finalized, both confirmed Zcash HTLC
outputs are spent after the LEZ reveal, and both LEZ custody accounts are zero.
The immutable evidence is
[`m2-canonical-local-certification-20260714.json`](evidence/m2-canonical-local-certification-20260714.json).
M2 is certified at the local-functional PoC boundary under `m2-complete`.
Actual-node refund/restart/reorg/chaos and live public execution retain their
explicitly deferred status; the tag itself did not imply a transition to QA or
M3. The owner subsequently entered M3 on 2026-07-14.

The M3 local happy-path actor gate is now GREEN in both directions. Run
`m3actor-20260716n` at commit
`6ded2f9b8ba9ec8e0cfbf06287da92d34256f91a` used fresh one-shot maker
and taker processes against actual local Bitcoin Core 31.1 Regtest and LEZ
v0.2 services. Both roles reached revision 4 `Completed` in both directions;
replay added zero submissions and exact cleanup targeted no foreign resources.
No public RPC, faucet, or public funds were used. The schema-3 public `recover`
command is deterministic and actual-node GREEN for both ordered LEZ/Bitcoin
two-lock refund directions. Run `m3refund-20260716h` used fresh one-shot
roles, Core 31.1 Regtest, and private-local LEZ v0.2 with 3.0-second slots; all
four role/direction stores reached revision 4 `Refunded`, each direction
retained two Bitcoin and three LEZ effects, and replay added zero submissions.
The run used no public RPC, faucet, or public funds. The revision-one
`TakerLockConfirmed` to revision-two terminal `Refunded` store/projector is
GREEN in both roles and directions. The canonical agreement now also signs and
validates an explicit last-safe maker-second-lock cutoff: RED exposed the
missing safe signed bound, and GREEN binds and round-trips it with a nonzero,
overflow-checked reaction margin before the earlier refund bound. That
field is the selected implementation of RFP-derived race safety, not literal
RFP wording. The exact finalized LEZ maker-lock
presence/absence/uncertainty classifier and refund-side live actor admission are
GREEN. Run `m3firstlock-20260716h` proves the durable baseline and both
refund-side actual-node absent-maker journeys.

Clean run `m3schema4-20260717d` at pushed commit `0e7635f` closes live
schema-4 Maker second-lock admission in both directions. Only the Taker first
lock is externally submitted; the Maker actor submits the exact LEZ
initialize/fund pair or Bitcoin funding transaction, then both roles complete
through revision four with zero restart/replay submissions. Its retained
secret-safe packet is
[`m3-schema4-actor-owned-lock-poc-20260717.json`](evidence/m3-schema4-actor-owned-lock-poc-20260717.json).

Clean run `m3overlap-20260717a` at pushed commit `1e6d5f1` closes the accepted
opposite-direction two-swap PoC checkpoint. Two distinct mature coinbase
outpoints with anchors 103/104, agreements, actor databases, eight signer
journals, two sessions per domain, escrow pairs, and deadlines shared only the
run-owned Core/LEZ nodes and deterministic fixture custody key. Both swaps were
simultaneously revision 2 `both_legs_locked` before either settlement, then
all four roles reached revision 4 with disjoint effects, zero replay, and exact
cleanup. The retained packet is
[`m3-overlapping-two-swap-poc-20260717.json`](evidence/m3-overlapping-two-swap-poc-20260717.json).
Arbitrary-N and same-direction LEZ nonce scheduling, adversarial
cutoff/refund races, process-kill/reorg/chaos, public Testnet4/LEZ execution,
production custody, and formal review remain later work. The public durable BTC
SDK, F7 surface/repetitions, Testnet4 configuration contract, and BTC D1
recordings/private source and video bundles are GREEN. Exact closure commit
`f7fb250f...dcbb2` is published under `m3-complete`; the private Actions API
was unavailable and neither this row nor the tag claims remote CI green.

## Functionality

| ID | Contract | Acceptance evidence | Status / milestone |
|---|---|---|---|
| F1 | No central server; Delivery advertisements and Chat coordination | Role E2E discovers and negotiates through real Delivery/Chat adapters; central services absent; UJ-006 repeats with both stopped after lock | Partial M5: ZEC, BTC, and XMR application paths use signed Delivery, durable Chat/application state, role-only authority, restart reconciliation, and Delivery-free replay. Clean BTC and XMR actual-node corridors continue after the transport cutoff using local state plus chain nodes. One-daemon concurrent application composition and automatic unavailable-route behavior remain |
| F2 | LEZ–BTC via BIP-340 adaptor signatures and Taproot key-path cooperative claim | Proposed GW-M3-001 evidence contract: official BIP-340/BIP-327 vectors, swap-specific adaptor fixtures, independent cross-check, tweak/parity-aware signatures under the P2TR output key, Core consensus, and both-direction actual-node happy/refund/concurrency role E2E. The nonexistent DLC `AdaptorSignature.md` is a nonblocking Gateway/Logos production-review erratum | M3 private-functional evidence GREEN: schema-4 actor-owned Maker locks complete both directions; happy, both ordered two-lock refunds, both absent-maker refunds, both post-reveal survivor continuations, and the opposite-direction overlap barrier reach terminal actual-node state with zero replay. Pushed `0c78f3d` adds the full public durable lifecycle boundary and immutable official plus independently cross-checked adaptor vectors. The three D1 recordings, MP4s, and sealed private source/video bundles are GREEN, and `946208a` adds explicit self-hosted/exact-HTTPS Testnet4 route/readiness configuration. Arbitrary-N/same-direction scheduling, adversarial cutoff/refund races, process-kill/reorg, live public execution, production authority, formal review, and GW-M3-001 replacement acceptance remain later hardening/review work rather than missing local M3 output evidence |
| F3 | LEZ–XMR via Ed25519 adaptor signatures, cross-curve DLEQ, and spend-key share | LEZ-first COMIT/DLEQ vectors; `monerod`/wallet RPC stagenet happy/refund/concurrency role E2E; spend-key recovery fault case; XMR-first rejected | Partial across milestones: M4 clean-certified the local successful-claim protocol, and M5 run `m5-xmr-app-20260730-2c6aec1-h` clean-certified its real Maker/daemon/Taker application composition through finalized Claim, extraction, reconstructed-key sweep, binding, and exact cleanup. Receipt-only XMR Taker application monitoring is process-GREEN without claiming chain progress. The opt-in application runner now composes tag-16 prepare/complete/one-attempt submission, finalized Maker discovery, Maker ingestion/extraction, honest Maker recovery sweep, independent receipt, and conditional binder; the diagnostic attempts exposed local-finality and request-identity REDs. ADR 0123 and the request-ID repair close them; exact run `m5xmrrefund45924caa` finalized tag 16 and a ten-confirmation Maker recovery sweep with conditional binding and exact cleanup. Tag-17 punishment, semantic receipt-v2 Taker XMR claim/refund beyond the process-GREEN Tag14 sender plus local observer/reconciliation checkpoint, concurrency/fault recovery, Stagenet trust, and production hardening remain |
| F4 | LEZ-transparent-ZEC via BIP-199 HTLC; LEZ claim/refund precedes ZEC and the ZEC refund strictly outlives LEZ by the documented margin | Exact vectors, profiles, agreement validation, canonical funding observations, and fresh maker eligibility pass. Separate role-fixed maker/taker SDKs and schema-v10 SQLite stores complete and reopen the direction-fixed claim or refund lifecycles. The authenticated eight-method client/server protocol, official native/revealing-claim/refund preparation and observation, executable role-isolated sidecars, main escrow/claim/refund validation adapters, role-correct both-direction Zcash funding submission and unknown-ID discovery, and production Zebra claim/refund boundaries pass isolated suites. The context-owning LEZ SDK ports persist caller-owned IDs/windows and share one role-local journal. The fresh-client factory rereads a private capability without retaining it, and the actor context source generates fresh OS-random IDs with only protocol-required bounded windows. Reopened SQLite now derives the exact LEZ claim funding in both directions without primitive caller IDs; reverse direction additionally refuses until the durable Zcash second lock exists. The zeroizing production Zcash signer produces byte-identical canonical funding/claim/refund transactions and rejects foreign roles or keys. The agreement-committed exact-outpoint planner validates every candidate and stable Zebra identity before producing the only durable funding plan. A one-shot maker/taker CLI and private-config boundary now proves separate roles, runs, stores, journals, capabilities, keys, and config files without path aliasing. The local v0.2/Zebra corridor now completes both claim directions through independent role processes: run 14o finalized LEZ blocks 264/265/266 and Zcash heights 106/108; reverse 14c finalized LEZ blocks 641/642/643 and Zcash heights 113/115. In both runs Zcash funding had two confirmations before the LEZ reveal, the exact `:0` HTLC output was spent afterward, and both stores reached revision 4 `Completed`. Signed public LEZ identities, schema-v3 Zebra routes, exact official-public sidecar routing, and pre-persistence runtime checks pass local nonconnecting contract suites with the same binaries and validators. Current-schema runs `m2cert-schema3-forward-2d09997-20260714a` and `m2cert-schema3-reverse-2d09997-20260714a` repeat both role/effect orderings through actual local nodes. Canonical Docker-target runs `m2cert-canonical-forward-bb53daf-20260714a` and `m2cert-canonical-reverse-bb53daf-20260714a` then bind that same order to ProgramId `5cf8...29c1`, finalized LEZ blocks 2594/2595/2596 and 2605/2606/2607, Zcash heights 122/124 and 125/127, zero terminal LEZ custody, and four revision-4 `Completed` actor stores. | Partial: the private actual-node happy-claim and configuration-portability gates are GREEN in both directions, including direction-correct funding and LEZ-before-Zcash reveal order. Lower lanes retain ordered refund, restart, and fault evidence, but their composed actual-node repetition and the remaining post-lock/margin matrix are owner-gated hardening. The M2 local-functional PoC is certified under `m2-complete`; composed actual-node recovery and the post-lock/margin hardening matrix remain owner-gated, and public-testnet execution is deferred to production readiness under ADR 0023 |
| F5 | Risc0 LEZ escrow validates pair proof and supports claim/refund | Per-pair guest instruction tests plus standalone-sequencer initialise/claim/refund E2E | Historical M2/M3 checked and deployed artifacts remain certified. The privately deployed historical M4 v0.2 identity is ELF `dc370bc3...b7292` / ImageID `4d659033...12c82`; tags 13–15 and the later clean refund replay used that deployed program. The advisory-remediated current v0.2 guest is separately checked at ELF `ade4af84...bbcee` / ImageID `b7f87278...b0433`, with all five recursive cases GREEN, but it has not yet been freshly deployed and must not be conflated with the historical deployed identity. The v0.1.2 compatibility re-attestation reproduced ELF `fe8ec116...c739f7` / ImageID `5421868e...add62` and passed six ordinary, two actual deployment/lifecycle, and one stable-cost case under builder `r0.1.94.1`. Exact run `m5xmrrefund45924caa` finalized tag 16 and the Maker recovery path against the historical deployment. Tag 17 punishment remains absent. Public deployment stays deferred under ADR 0023; Logos-owned disclosures remain tracked under ADR 0018 |
| F6 | Atomic outcome: both claim or both refund | Six scenario tests and 512 generated transition sequences pass. M2/M3 retain their certified conditional claim/refund evidence. For M4, Stage A/B precommitted exact claim, signed-refund, and punishment messages; the Taker partial stayed private until exact XMR funding; finalized Maker claim disclosed the share used for the Taker sweep | Partial: the M4 successful claim branch executed conditionally atomically, not as one distributed cross-chain transaction. The claim binder directly cross-checks finalized LEZ height 4208/tip 4220 against Monero receipt height 121/tip 130, but is a historical snapshot and not future-reorg immunity. The refund runner/binder now requires finalized tag 16 in the signed half-open window, derives the Taker share only for the Maker after that effect, and verifies the Maker-directed Monero receipt with honest roles and exact accounting. Exact run `m5xmrrefund45924caa` proved the clock effect leaves escrow byte-identical, finalized tag 16 in the signed window, disclosed the refund share only afterward, and confirmed the Maker Monero sweep. This is conditional both-refund evidence, not a distributed commit or future-reorg guarantee. Tag 17, adversarial races, process-kill, reorg, and concurrency remain. GW-M4-003 preserves the punishment/economic-safety discrepancy |
| F7 | Native and custom LEZ tokens through ATAs | Parameterized guest and sequencer E2E for native token plus two independent token mints/accounts | Partial M3: the checked BTC guest now appends aggregate-witness token initialize/claim tags 11/12 without changing tags 0-10, recursively proves two definitions plus substitution rollback, and has one artifact/IDL/deployer identity across the verifier and active M3 runner. An additive strict v2 protocol binds native or exact token definition/program/ATA/authority terms and seven transaction lifecycles. Four more methods classify finalized initialization, token-only custody creation, funding, and claim through exact or discovered `Found`/`Absent`/`Uncertain`/`Unavailable` outcomes; 44 tests cover native parity, two definitions, three-step order, containing blocks, state, cross-field drift, and unchanged v1 wire. A separate bounded extension countersigns the unchanged agreement-v1 commitment plus explicit asset kind and every custom program/definition/owner/ATA/amount/deadline/authority field; independent custom custody, exact local policy, cross-agreement/signature/field/alias rejection, and native byte stability pass 16 agreement tests. The loopback bridge client maps all eleven v2 operations exactly once, enforces the depositor/claimant/either-participant role matrix, and rejects context/runtime/terms/target/window/transcript/effect/placement drift; 46 all-target checks preserve the v1 contracts and cover native plus two token definitions, four classifier states, timeouts, and zero-wire outsider rejection. The official v0.2 sidecar planner now rederives the exact Token/ATA programs and all three ATAs, prepares tags 11/7/8/12/10 with exact signer/nonce ordering, and durably replays four distinct escrow/claim/completion/refund v2 reservations. The server exposes all eleven authenticated v2 calls, restores the four exact reservations in dependency order, and produces fork-safe finalized evidence for initialization, custody creation, funding, claim, and refund. Lifecycle-aware discovery validates and skips only legitimate different same-swap steps while retaining malformed, same-kind, and duplicate-match conflicts; all 127 sidecar tests pass, including same-height fork rejection and the official three-effect funding-discovery regression. The main-process adapter rebinds the countersigned extension plus exact local policy, performs runtime/chain/program/signer/role preflight, maps all eleven no-submit calls once, and preserves the four classifier outcomes; 79 total adapter tests and strict gates pass without dependency changes. Runs P and Q are retained bounded REDs for the now-fixed actor/client timeout-contract mismatches. Fresh run `m3f7compose20260718r` at clean pushed commit `7fd84fa` reached uniquely finalized forward Bitcoin and custom-token initialization/custody/funding effects, and the Maker reached revision two. The Taker then made 120 read-only legacy v1 native-funding observations, which could not match the exact four-account token funding; no claim/refund followed and exact cleanup targeted no foreign resource. Run R is bounded RED evidence, not an F7 PoC pass. RED-GREEN coverage routes schema-5 nonowners through v2 peerless `DiscoverByTerms`, proves exact `Found` token funding projects in both role/direction shapes without peer-private material or a submission surface, and keeps `Absent`, `Uncertain`, and `Unavailable` pending; schema 4 remains on v1. Its request identity binds runtime, full terms, target, agreement, asset, run, role, and window. Run `m3f7compose20260718s` at clean pushed `ba17e3b` exercised the corrected v2 route after repeating deployment, bootstraps, fixture, Bitcoin lock, and finalized token initialize/custody/fund. Maker reached revision two; Taker exposed the pre-fix lifecycle-scan conflict on the valid earlier initialization. No claim/refund followed and exact non-foreign cleanup passed. Run S is bounded RED, not an F7 pass. All 85 actor tests, 127 sidecar tests, formatting, strict Clippy, and the M3 pre-Docker actor contract pass. Overs
| F8 | Pluggable local and Logos-module C-API price sources | Price-port contract suite; config/CLI mutation E2E; fake and real C-ABI adapter tests including stale/unavailable feeds | Partial M5: local daemon/CLI quoting is GREEN. The actual-C worker and bounded parent are GREEN for validation and isolation. Schema v15 atomically binds the policy, module SHA epoch, monotonic quote head, immutable offer, and replay record; retry-before-effect, rollback/equivocation, and stale quote cases pass. The real daemon selects without fallback, invokes outside the store mutex, signs exact quote/module fields into Delivery, exact-replays with the failed module, rejects fresh requests while failed, and reconciles after restart. Eventual upstream compatibility remains LOGOS-021 |
| F9 | Headless maker covers configuration, pricing, advertisement, execution, monitoring, and full CLI operation | UJ-007 actual CLI/daemon suite plus restart, history, manual claim/refund, pricing, advertisement, and fixed-service lifecycle cases | Partial M5: owner-local configuration/pricing/offers/create/status/history/alerts and generation-fenced actions survive restart; clean BTC/XMR application corridors exist. The daemon now runs bounded simultaneous workers with failure isolation and restart/no-replay evidence. Fixed packaged-system-service start/stop is GREEN. The receipt-only XMR monitor and receipt-v2 Tag14 sender/observer checkpoint close no Maker effect gap: fixed local markers perform evidence-shaped reconciliation but no semantic transaction or actual-chain observation, and actual-application plus unavailable-node composition remain |

Current F7 execution update: clean pushed Runs X (`422c72e`), Z (`1555749`),
AA (`df7ed86`), and AD (`0826dd5`) each completed both actual-node custom-token
directions at the one-second local cadence. Every direction reached revision
four with exactly two Bitcoin and four LEZ effects, one Maker second lock, zero
replay resubmission, zero custody, conserved total 250, direction-correct
`175/75/0` or `75/175/0` balances, and exact scoped cleanup. Runs X, Z, and AA
close the requested three-repetitions-per-direction gate; Run AD supplies a
fourth complete pair and the measured concurrent-startup result. Failed-closed
Runs Y, AB, and AC count as no repetition. This closes the reproducible private
local F7 functional and repeatability checkpoint; it does not claim public
execution, production hardening, or an M3 completion tag.

## Usability

| ID | Contract | Acceptance evidence | Status / milestone |
|---|---|---|---|
| U1 | Dedicated full-lifecycle SDK for each pair | Shared SDK contract suite instantiated for BTC/XMR/ZEC; public API doctests cover discovery through refund | M7 GREEN across all three pairs. BTC and ZEC retain their role-fixed durable facades. XMR now adds a bounded canonical Stage-A/Stage-B negotiation envelope, shared Delivery/Chat traits, role-fixed publish/discover/negotiate/activate/resume/advance/claim/refund APIs, public lifecycle identity and structured errors. The post-lock `ActiveXmrSwap` erases Delivery/Chat and delegates private validation, persist-before-effect journals and chain actions to `XmrRoleActorPort`. Twenty-two tests cover claim, absent-Maker refund, replay, revision/branch/identity substitution; strict Clippy, warning-fatal Rustdoc and a dedicated external-wiring example pass. Actual role processes remain the concrete effect authority rather than being duplicated in the SDK. |
| U2 | Long-running autonomous maker daemon plus systemd unit/install guide | Packaged daemon runs under hardened systemd in an isolated VM, restarts, advertises, prices, and executes without GUI | Partial M5: hardened systemd lifecycle, exact identity, staged verification, crash recovery, and bounded 1..=32 worker supervision are GREEN. One daemon/database proves simultaneous terminal/live overlap, peer failure isolation, joined cancellation, restart equality, and no replay. Accepted-application/actual-chain concurrency and actual-chain actor crash recovery remain. Immutable live Logos Core integration stays an upstream production-parity exception |
| U3 | Maker CLI configures pairs/prices, controls daemon, queries history, and triggers claim/refund over IPC/RPC | Black-box CLI-to-daemon command matrix under owner/wrong-owner roles plus fixed-system-service contract | Partial M5: configure/price/offers/create/status/history/monitor and supported generation-fenced actions pass through the real daemon/CLI with replay and restart evidence. Literal start/stop controls only the packaged system unit with bounded exact-state verification. Receipt-v2 XMR Taker Tag14 marker invocation is intentionally a separate CLI boundary and does not satisfy the missing daemon-owned XMR effect actions; complete the supported-pair lifecycle matrix |
| U4 | Taker CLI covers discovery, initiation, monitoring, claim, and refund | Actual taker CLI drives each pair’s happy and abandoned-counterparty journey | Partial M5: ZEC discovery/initiation/receipt-bound monitor/claim/refund, BTC discovery/initiation/offline monitor, and XMR discovery/initiation/receipt-only monitoring plus the receipt-v2 Tag14 marker claim are process-GREEN; clean actual-node BTC and composed XMR application corridors exist. The role-correct XMR refund effect is composed in the application runner but not yet exposed through the literal Taker lifecycle CLI. The literal receipt-v2 claim invokes the Tag14 sender marker exactly once, uses a role-fixed finalized observer on the second call to reconcile locally derived evidence-shaped Succeeded, and returns Complete without either process on the third. It still constructs no semantic transaction and proves no actual chain evidence. XMR monitor validates pre-effect application authority under the per-swap lock and explicitly does not infer chain progress. Abandonment journeys remain hardening unless required to expose a PoC defect |
| U5 | Basecamp maker mini-app configures and monitors | Signed-off prototype plus consumer-locked Maker `ui_qml`, typed process backend, official standalone load, missing-service fail-closed run, and real-daemon Basecamp product run. Product health, atomic schema-v22 `maker_local_route_save_v1`, and isolated history are GREEN | M6 local-functional PoC certified by `m6-poc-complete` under ADRs 0128, 0129, 0146, and 0147; later QA/production hardening requires an owner transition |
| U6 | Basecamp taker mini-app browses and executes swaps | Signed-off prototype plus consumer-locked Taker `ui_qml`, seven typed slots, official standalone load, missing-service fail-closed run, and real-service product flow. Basecamp browses, admits a prepared offer, exact-replays the fixed UI request, lists, monitors, and is followed by a durable registry assertion. Fresh layered certificates `m6claim0ba41aba` and `m6refund8f76d87a` separately prove service-driven actual-local Claim/Refund, opposite-action exclusion, and no-effect replay | M6 local-functional PoC certified by `m6-poc-complete` under ADRs 0128 and 0130 through 0147; the UI run is not misrepresented as the source of retained chain effects |
| U7 | SPEL IDL for LEZ escrow program(s) | IDL generation/validation test against pinned compatible SPEL/LEZ versions | Generated IDL/client and signer roles pass; custody ABI is being regenerated around authenticated transfer and official ATAs |
| U8 | Bitcoin Core testnet setup guide, self-hosted and public | Fresh-machine documentation test reaches funded wallet and SDK connectivity for both routes | M3 configuration/documentation GREEN under the private-delivery policy. `docs/bitcoin-testnet4-setup.md` covers exact Core 31.1 release verification, Testnet4 node/index/readiness configuration, separate operator wallet/funding authority, confirmed-outpoint checks, literal-loopback and exact allowlisted HTTPS SDK composition, role credential boundaries, no failover/retry, main claim/refund flow, external resources, and flakiness. Focused tests exercise both client/profile shapes without public I/O. The existing operator guide retains reproducible local Regtest/LEZ happy/refund/concurrent flows. A cold live public sync, gateway, faucet, funded transaction, and provider availability measurement are deliberately deferred and unclaimed. |
| U9 | Monero stagenet and wallet-RPC setup guide, self-hosted and public | Fresh-machine documentation test reaches funded wallet and SDK connectivity for both routes | M7 guide and CI documentation contract GREEN: pinned 0.18.5.1 verification, authenticated self-hosted daemon, separate role wallet RPCs, funding/faucet flow, untrusted public-node policy, rehearsal, security, cleanup and flakiness are documented without public calls. Live self-hosted/public Stagenet smoke remains deliberately unclaimed |
| U10 | Zebra/Zcash testnet transparent-wallet guide, self-hosted and public | Fresh-machine documentation test reaches funded transparent wallet and SDK connectivity for both routes | M7 repository capability and guide contract GREEN: the guide selects self-hosted Zebra 6.0.0 and Tatum's API-key-authenticated Testnet Zebrad gateway; schema-v3 tests validate both exact routes without public I/O. Separate mode-0600 role keys feed the implemented project-owned signer and actual-node-tested funding, claim, and refund builders. Optional Zallet funding plus faucet/Discord fallback, privacy, cleanup, and external flakiness are documented. Under ADR 0023, no live public key, funded TAZ, broadcast, provider smoke, or clean-machine public rehearsal is claimed; those remain production evidence rather than a missing implementation. |

## Reliability

| ID | Contract | Acceptance evidence | Status / milestone |
|---|---|---|---|
| R1 | Taker locks first; maker waits for required confirmations | Core rejects early lock and uses independent immutable policies for each funded leg; reverse ZEC proves maker-funded ZEC below-threshold accumulation, promotion, 10→9-style depth regression suspension, exact depth recovery, removal pinning, conflict rejection, and preserved refunds. Both actual-node happy directions now show the direction-derived taker effect first and prohibit the LEZ reveal until the Zcash lock has two confirmations. Watcher tests still require stable affirmative evidence; runtime requires both role policies to match the immutable named profile and commits conflicting chain replacements without substituting the protocol-pinned ID. | Passing core/reconciliation boundaries and both private local composed happy directions; actual-process reorg/replacement and refund hardening remain owner-gated |
| R2 | After first submission, only local state and chain nodes are required | BTC retains happy/refund/survivor evidence. XMR run `m5-xmr-app-20260730-2c6aec1-h` archives authenticated reconciliation, installs an empty Delivery outage, replays from durable application state, proves process cutoff, and completes using only role-local journals plus local LEZ/Monero nodes | Passing at the clean private-local happy-path boundary for BTC, ZEC, and XMR; process-kill, recovery, different-UID isolation, and adverse node behavior remain hardening |
| R3 | Missing chain dependency does not disable other pairs | Dependency matrix starts daemon with each node absent/unhealthy and completes unaffected-pair swaps with clear CLI/GUI status | Partial M5: explicit route control is process-GREEN. Disabled Zcash quote and publication both fail before price/Delivery I/O; an enabled Bitcoin quote remains available across daemon restart; revisioned Zcash re-enable restores quotes. Automatic node-health detection, active-offer withdrawal, mid-negotiation behavior, and an actual unaffected-pair swap with another chain absent remain |
| R4 | Persisted state survives crash/restart without fund loss | Schema v9 retains the schema-v8 lock journal and adds protected claim material, encrypted exact claim intents, owner/observer transitions, and two-direction replay to `Completed` (`add5d98`). Coordinator snapshots retain only a SHA-256 claim marker; AAD binds agreement/role/step/revision/expected identity. The crash-safe v8→v9 migration rewrites exact legacy plaintext arrays, enables secure deletion, truncates WAL, and reopens the marker (`5ed04ec`). Existing lock rollback/corruption/unknown-submission coverage remains green. | Partial: claim happy-path restart and legacy scrub proven; claim-specific unknown submission, atomic rollback, wrong-key/AAD/ciphertext/future-version/orphan corruption, process-kill, and actual-node restart gates remain |
| R5 | Concurrent swaps have independent state, escrow, and deadlines | M3 clean run `m3overlap-20260717a` proves the underlying opposite-direction actor/chain boundary with distinct agreements, stores, escrows, deadlines, effects, replay, and cleanup. M5 now has one real-daemon simultaneous two-row worker journey: a terminal peer completes while the other child remains live and Leased, then only the released failing peer enters Backoff; restart preserves exact manifests and no replay | M5 daemon concurrency component GREEN; full R5 remains Partial until the same daemon/database runs distinct accepted application agreements with independent escrows/deadlines and actual-chain effects, restart/replay, and scoped cleanup |
| R6 | Timelocks cover variance, congestion, and clock drift per chain | Typed pair-specific domains and ordering pass; LEZ seconds/milliseconds conversion is explicit, conservatively rounded, overflow-checked, and boundary-tested; named runtime profiles and public calibration remain | Partial: `m3refund-20260716h` executed both ordered recovery mappings, exact Bitcoin next-block CSV eligibility, and finalized LEZ timestamps with private-local 3.0-second slots. `m3firstlock-20260716h` additionally enforced the signed maker cutoff before two fresh admission reads and waited for the chain-specific sole-leg refund boundary in both directions. Margin validation and the refund-side live gate are GREEN. Commit `3d202f7` now also requires canonical Bitcoin containing-block median time or finalized LEZ containing-block time no later than the cutoff, with late presence quarantined as uncertain. SDK-owned same-action admission, public calibration, cutoff/race stress, congestion, and actual reorg margins remain. |
| R7 | Bitcoin refund construction is chosen and justified with failure analysis | ADR 0009 selects script-path CSV; Bitcoin Core tests cover exact boundary, key, reorg, current fees, and RBF/CPFP | Partial M3: the canonical agreement commits the exact CSV leaf, control block, destination, funding outpoint/value, fee, and direction-correct schedule. `m3refund-20260716h` exercised the two-lock Bitcoin refund in each direction. In `m3firstlock-20260716h`, `TakerSellsForeign` additionally executed the sole Bitcoin first-lock refund at exact signed height 246 with the countersigned three-item witness and txid/wtxid readback. Maker-lock admission, fee stress, bounded RBF/CPFP, and reorg remain |
| R8 | Delivery/Chat outage is retried/buffered/degraded and documented | Pre-lock outage/recovery matrix plus post-lock UJ-006 proves on-chain independence and user-visible degraded state | M5 local output GREEN: real Maker Delivery and Chat failure/recovery, response-loss replay, restart, and three post-first-lock actual-node transport removals remain proven. M6 `e9393cf` additionally proves that after service-connected Maker completion and Taker receipt publication, restart/offline list and monitor succeed with the Delivery offer removed and Chat unavailable while agreement, actor config, and receipt bytes and inodes remain unchanged. LOGOS-020 remains upstream production parity |

## Performance

| ID | Contract | Acceptance evidence | Status / milestone |
|---|---|---|---|
| P1 | Compute units documented for initialise, claim, and refund against a named LEZ testnet version | Reproducible benchmark records each operation/pair and fails CI thresholds for the pinned release | Partial: exact local v0.1.2 native/authenticated-transfer and token/ATA/Token cycles, segments, invariants, and CI budgets are recorded. v0.2 recursive behavior is executable, but deployed-runtime CU evidence against the named public testnet is deferred to production readiness under ADR 0023 |

## Supportability

| ID | Contract | Acceptance evidence | Status / milestone |
|---|---|---|---|
| S1 | LEZ escrow deployed/tested on testnet 0.2 | Version-pinned deployment manifest and public smoke-test transaction evidence | Partial local GREEN: certified M2/M3 local artifacts remain, and the checked M4 ELF/ImageID was deployed once to private local v0.2. The M4 working-tree claim then executed tags 13–15 against an isolated local v0.2 stack. Public deployment and smoke evidence are intentionally deferred under ADR 0023, and the claim packet is pending exact committed-tree replay; no public-testnet or production claim is made |
| S2 | Standalone LEZ sequencer E2E is included in CI | Isolated CI job boots sequencer on ephemeral resources and runs guest lifecycle suites | Partial local GREEN: the existing isolated CI lanes cover the certified v0.1.2 and local v0.2 component/deployment boundaries. One M4 actual local claim now exists as working-tree evidence, but the full M4 two-devnet journey has not been replayed from a clean commit or added as an isolated CI lane. Public deployment remains deferred under ADR 0023 |
| S3 | Default-branch CI is green | Required checks run format, strict Clippy, workspace tests/docs, traceability, dependencies, strict final-image vulnerability scanning, and isolated E2E | Fresh repository-local closure gates are GREEN on 2026-07-19: quality/security/isolation policies, root Rust tests/docs, Node vulnerability/license policy, all 11 Rust advisory/ban/license/source graphs, traceability, and 150 exact-rendered Mermaid diagrams passed. Exact commit `f7fb250f...dcbb2` and `m3-complete` are pushed. The private Actions API was unavailable because this environment has SSH but no API identity, so separate remote Trivy/actual-node job results were not observed and no default-branch remote-green claim is made |
| S4 | Every F/U/R/P hard requirement has a corresponding test | Traceability completeness guard plus test-report manifest rejects missing or skipped requirement IDs | Matrix guard passing; acceptance tests incomplete |
| S5 | Complete reference integration for every chain | UJ-001/UJ-002/UJ-004 role E2E and runnable reference package for BTC, XMR, and ZEC | Partial across all pairs: ZEC and BTC retain certified private local references. XMR now has clean exact-commit application claim and refund corridors through real local LEZ v0.2 and official Monero Regtest processes, role-correct funding/recovery, conditional binding, manual repetition, and scoped cleanup. Complete daemon-owned Maker/Taker lifecycle, accepted-application concurrency, unavailable-route composition, public execution, and hardening remain |
| S6 | README covers deployment, addresses, prerequisites, and maker/taker CLI/mini-app use | Fresh-machine documentation tests plus link/command validation | M6 documentation covers prototype review, package/LGX/installer builds, pinned Basecamp install, separate role user directories and sockets, real Maker/Taker UI steps, external build/runtime resources, flakiness, exact cleanup, prepared acceptance/replay/monitor, and layered actual-node Claim/Refund. Traceability, conservative link targets, package commands, and all 429 Mermaid renders are GREEN at `m6-poc-complete` |
| S7 | Write-up covers protocols, escrow, atomicity, timelocks, assumptions, limitations | M1 design packet reviewed, then updated from executable evidence and audit findings | M7 repository write-up covers every named section plus an operations runbook and links the detailed per-pair/component/RPC sequences. It explicitly denies mainnet readiness while independent review, public calibration, rollback and upstream release gates remain |
| S8 | Every pair SDK public API has docs/errors/examples for full lifecycle | `cargo doc -D warnings`, doctests, and public-API coverage check for all three SDKs | M7 GREEN across all pairs. The XMR public facade documents bounded negotiation, role/process authority, lifecycle phases, restart identity, commands and every structured error; `lifecycle-wiring` type-checks an external Delivery/Chat/actor composition. Twenty-two all-target tests, strict Clippy, warning-fatal Rustdoc and the facade CI contract pass. Concrete daemon/CLI adapters are tracked separately under U3/U4 and do not make the SDK API incomplete. |
| S9 | Logos doc packet for each pair SDK | Template validation and reviewer acceptance for BTC, XMR, and ZEC packets | M7 local packet contract GREEN for BTC, XMR and ZEC against pinned official template commit `63ecf397...`; command rehearsals and external submission/Docs-team acceptance remain |
| S10 | Separate maker-CLI and taker-CLI doc packets | Template validation plus clean operator/user journey rehearsals | M7 local packet contract GREEN for separate Maker and Taker journeys against pinned official template commit `63ecf397...`; command rehearsals and external submission/Docs-team acceptance remain |
| S11 | Figma designs or equivalent for both mini-apps | Clickable HTML prototypes, isolated 6/6 actor-browser rehearsal, explicit 2026-08-04 owner approval, and both implemented Basecamp QML views exist | M6 design and implementation evidence GREEN under ADRs 0128 and 0147 |
| S12 | Third-party review of all on-chain programs/scripts and remediation | Agreed reputable reviewer report covers LEZ, BTC, ZEC, and other locking logic; findings tracked to closure | M7 review scope and severity/remediation register are repository-GREEN; immutable bundle and all self-owned closure work remain before external review is requested |
| S13 | Third-party review of protocol implementation and remediation | Reviewer report covers atomicity, taker-first, timelocks, adaptor/HTLC constructions, with findings tracked | M7 review scope includes atomicity, ordering, timelocks, all three pair constructions, coordinator, daemon auth and IPC; immutable bundle and all self-owned closure work remain before external review is requested |

## Demos

| ID | Contract | Acceptance evidence | Status / milestone |
|---|---|---|---|
| D1 | Happy, abandonment/refund, and concurrent-swap recorded demo video for each pair | Nine videos generated from passing role E2E runs, with commit/testnet/version metadata | BTC M3 GREEN: happy `m3record-happy-20260718ag`, refund `m3record-refund-20260718ag`, and concurrent `m3record-concurrent-20260718ag` are replayable mode-`0600` actual-node captures bound to clean pushed evidence commit `a6eb1ad`, Core 31.1 Regtest/LEZ v0.2 identities, zero replay sends, and no public RPC/faucet/funds. Refund covers both ordered timeout legs; concurrent proves simultaneous revision two and disjoint authority. Source verifier commit `946208a` sealed source bundle `3d7d7adc...a86c7cc`; RED-GREEN renderer/verifier commit `846ba56` then produced three H.264 1280x720 MP4s. Regenerated-source verification, full decode, scenario/atomicity/tail frame sampling, and mode-`0600` bundle verification passed at SHA-256 `7697a27c80c8f90856d6592051805a8923fe564aa01b0dff4109bd5c5f101ba8`. Other-pair videos remain M4/M7 work; public execution stays deferred under ADR 0023. |

## M4 PoC evidence checkpoint

The progressive local-functional M4 exit gates are evidenced by clean replay
`m4cert20260722an` on commit `5ec6521` (documented and tagged as
`m4-poc-complete.1`). The replay binds finalized LEZ Claim, transcript
extraction, reconstructed spend key, Maker-destination Monero sweep, independent
post-fee receipt, and canonical cross-chain binding; its evidence and cleanup
ledgers are green with exact resources absent. This checkpoint deliberately does
not claim the deferred signed-recovery branches, F7/U9/D1 outputs, or production
hardening gates.


## M5 closure-candidate traceability reconciliation (2026-08-02)

This mapping uses the seven literal deliverables in the current accepted issue
#112. It supersedes earlier 3/7 and 4/7 current-score snapshots while preserving
their historical evidence.

| Deliverable | Requirements advanced | Closure-candidate evidence and limit |
|---|---|---|
| Daemon | F9, U2, R2, R4 | Retained service/actual-corridor evidence plus responsive three-pair supervision |
| Maker CLI | F9, U3 | Real all-pair CLI/daemon claim/refund matrix 1/1 in 0.64s; Bitcoin Claim-to-Drive unit 1/1 after `-32602` RED |
| Taker CLI | F3, U4 | Retained pair corridors plus receipt-v2 Tag16 refund 1/1 in 84.21s with send-once, observe/reconcile, Complete, and losing-claim exclusion |
| Coordinator persistence/crash/concurrency | R3, R4, R5 | Three pair-correct rows under one daemon/database 1/1 in 16.31s; XMR live/failure isolation, BTC/ZEC Terminal, reap, restart equality, no replay |
| Price sources | F8 | Retained local and Logos-module C-API paths |
| Delivery/Chat degradation | F1, R2, R8 | Retained post-lock removal, degraded state, and durable replay |
| Fuzz | S4 | Retained coordinator target, seeds, and smoke counters |

Literal M5 PoC status is verified 7/7, bound by `m5-poc-complete`. The all-pair
matrix and overlap use marker actors and prove control-plane behavior only.
Actual-chain claims remain grounded in the retained M2/M3/M4 local-devnet
evidence and clean M5 accepted-application BTC, ZEC, and XMR corridors. Semantic
receipt-v2 XMR worker adapters, a fresh simultaneous accepted-application
actual-chain composite, public deployment, and production hardening are not
claimed by this candidate.
