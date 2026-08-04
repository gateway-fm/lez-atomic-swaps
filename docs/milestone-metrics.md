# Milestone delivery metrics

Last updated: 2026-08-04

This is the live evidence scorecard for ADR 0027's progressive milestone
delivery. It tracks measurements and explicit unknowns; it does not infer a
percentage from task counts. Update it whenever a phase changes, a reproducible
run is recorded, or an open item is closed or invalidated.

Status vocabulary:

- `in progress`: the owner-selected active phase;
- `awaiting owner transition`: work is intentionally not the active phase;
- `carried evidence`: relevant evidence created before the phase was selected,
  retained for later revalidation; and
- `gate met`: all documented exit evidence exists, subject to owner review.

## Milestone phase register

| Milestone | Active phase | Phase status | Completion tag | Owner transition |
|---|---|---|---|---|
| M1 | Historical completed milestone | Historical evidence predates ADR 0027; not retroactively reclassified | `m1-complete` and corrective tag `m1-complete.1` | No transition requested |
| M2 | Certified local-functional PoC | Canonical Docker-built/deployed artifact, both local LEZ/ZEC directions, and the exact-tree repository gates are GREEN; later hardening is deferred | `m2-complete` | M2 completion/tag directed; no QA or M3 transition requested |
| M3 | Certified local-functional PoC | All six issue-#112 outputs, including the literal RFP D1 three-video deliverable, and the underlying actual-node happy/refund/concurrent evidence are complete at the private functional boundary. The 2026-07-19 local lint/test/security/license/isolation/traceability and 150-diagram render closure gates are GREEN. No cross-system atomic commit, public live deployment, or production-hardening completion is claimed | `m3-complete` at `f7fb250` | Owner entered M3 on 2026-07-14 and directed completion. The exact commit and tag are pushed; the private Actions API was unavailable and no remote-green claim is made. Cutoff-race/process-kill/reorg/fee/chaos/formal review are later owner-selected phases |
| M4 | Certified progressive local-functional PoC | Exact clean replay `m4cert20260722an` completed the LEZ-first claim through isolated LEZ v0.2 and official Monero 0.18.5.1 Regtest, retained canonical cross-chain binding and exact cleanup, and was documented on the pushed tree | `m4-poc-complete.2` | Owner entered M5 on 2026-07-23; deferred M4 hardening and production scope remain explicit |
| M5 | Verified local-functional PoC | All seven literal outputs are reproducibly GREEN. Retained actual-node BTC, XMR, and ZEC application corridors are deliberately layered with the final role-lifecycle and coordinator control-plane matrices; marker evidence is never presented as fresh chain evidence | `m5-poc-complete` at `8586cce` | Owner entered M6 on 2026-08-03; public deployment, semantic receipt-v2 XMR workers, simultaneous actual-chain composition, and production hardening remain explicit later work |
| M6 | Certified local-functional PoC | The signed-off Maker/Taker prototypes are GREEN 6/6 in the sandboxed networkless browser. Both consumer-locked Basecamp `ui_qml` packages build module, LGX, installer, and official integration outputs; load in pinned Basecamp 0.2.0-RC3; fail closed without their owner service; and exercise real role services through typed process-isolated backends. Maker health, atomic route save, and history are product-GREEN. Taker health, offer browse, prepared initiation, exact UI replay, list, monitor, and the post-product registry assertion are product-GREEN. Layered fresh actual-node certificates retain one LEZ/Zcash Claim with no-effect replay in 33.330 seconds and one LEZ/Zcash Refund excluding Claim with no-effect replay in 211.530 seconds; they are not misrepresented as UI-emitted chain effects. Locked workspace tests, crash seams, warning-fatal formatting/Clippy/Rustdoc, dependency and Node vulnerability/license policy, CI hardening and quality, traceability/isolation, package contract, and 429/429 GitHub-compatible isolated Mermaid renders are GREEN. Exact M6 temp/build paths, volume, and unused pinned images were removed without global prune. No public RPC, faucet, public funds, public deployment, or remote-Actions-green claim is involved. Cross-restart rollback anchoring plus LOGOS-024/025 remain production hardening, not issue-#112 PoC blockers | `m6-poc-complete` | Owner approved the prototype and M6 certification; M7 entered separately on 2026-08-04 |
| M7 | Repository-controlled review readiness active | Live RFP/issue/template authority is pinned; ADR 0148 separates self-owned readiness from independent attestation. The S7 write-up, S12/S13 scope, finding policy, five doc packets, U9/U10 guides, XMR SDK facade, route-health control, generated SPEL custody freeze, sealed XMR application/plan custody, real Tag16 semantic child, sealed Tag14 service, schema-v2 release authority, authenticated zero-network Tag14 preflight, release-only FDs 220..223, and literal claim preflight/invoke/observe/Complete control flow are GREEN. Real worker 1/1, effect route 8/8, literal CLI 1/1 in 164.85s, release authority 39 tests, strict Clippy and warning-fatal Rustdoc pass. Joined actual-node Tag14 CLI replay, semantic finalized observer, Monero sweeps, hard-requirement remediation, QA/chaos/security/production gates and immutable handoff remain in progress | None; formal review is deliberately deferred until all self-owned work is exhausted | Current repository-controlled ETA is maintained after each pushed slice; external review calendar and policy-deferred public deployment excluded |

M5 schema-v3 checkpoint (2026-08-02): canonical role-fixed effect authority,
direct run binding, exact initialized workflow identity, full legacy Stage A/B
semantic revalidation, and atomic no-clobber publication are component-GREEN.
This prerequisite performs no RPC or chain effect and does not change the
literal M5 score of 4/7.

M5 receipt-v2 checkpoint (2026-08-02): replay-safe receipt-v2 publication and
selection now bind schema v3, the effect authority, initialized workflow
identity, run, and role. Locked monitor is implemented and performs no RPC or
chain effect; receipt v1 remains monitor-only. At that historical checkpoint,
receipt-v2 claim/refund still rejected.
Typed tool/RPC execution, at-use hashes, full workflow reconciliation, child
lock custody, Maker effects, and actual-runner proof remain open. Literal M5
stays 4/7; ETA remains 3 to 7 focused implementation hours.

M5 typed-plan/sealed-executable checkpoint (2026-08-02): both role-fixed
authorities now expose typed LEZ runtime/capability identity, four typed Monero
RPC and credential-path roles, and five exact program/SHA-256/ABI slots per
role. A reusable use-time verifier securely opens and revalidates one trusted
single-link executable, checks its pinned hash, copies it to an immutable
mode-0700 memfd, and executes only descriptor 197; later named-path replacement
cannot alter that snapshot and a fresh verification of drift fails closed.
Focused Taker tests are GREEN at 3/3, the Maker/Taker authority pair at 4/4,
both full package suites, strict Clippy, warning-fatal Rustdoc, and diff hygiene
are GREEN. No lifecycle route invokes the primitive. Credential and
runtime/capability use-time custody, complete workflow/reconciliation, dual-lock
child inheritance, Maker/Taker effect composition, and fresh two-devnet proof
remain open. No RPC or external runtime resource participated. Literal M5
stays 4/7; ETA remains 3 to 7 focused implementation hours.

M5 workflow-v2/dual-lock checkpoint (2026-08-02): schema v2 rejects v1 and
validates every durable row against an eight-step role/scope catalog. Role-local
predecessors and Common-before-branch gates pass, but do not prove cross-role
or global ordering; routes must bind finalized external evidence. One
Prepared-to-Started winner
gets InvokeOnce while Started/Unknown replay is ObserveOnly. Succeeded requires
nonzero canonical effect-evidence and exact tool-plan SHA-256 values plus a
LEZ-finalized or Monero-wallet source; exact replay passes and drift or legacy
evidence-free success fails closed. One command mapping now carries sealed
program FD 197, actor/state lock FD 198, and distinct workflow lock FD 199;
alias, collision, cross-swap, root, and identity drift fail before spawn, and the child holds
both locks through reap. Focused suites are GREEN at maker process 17/17, workflow
concurrency 2/2, hardening 1/1, restart/no-rearm regression 1/1, and workflow v2 3/3. The full
`lez-swap-store --all-targets` suite, strict Clippy, warning-fatal Rustdoc, rustfmt, and diff hygiene are GREEN. No
lifecycle route executes this boundary and no RPC, node, or external runtime
resource participated. Literal M5 stays 4/7; ETA remains 3 to 7 focused
implementation hours.

M5 schema-v3 effect-input custody checkpoint (2026-08-02): at-use no-symlink
secure-open now requires one stable mode-0700 owner parent and mode-0600
owner-only regular single-link sources. The runtime is bounded to 16 KiB and
authority-hash pinned; capability plus eight Monero RPC credential files are
bounded to 256 bytes and accept one raw/LF/CRLF runner-compatible graphic value.
Aliases and unstable or invalid content/storage fail closed. All nine secrets
become separate mode-0400 fully sealed memfds on unique descriptors at or above
200, exposed only by descriptor path, redacted length, and SHA-256 through
non-Clone redacted types. Existing snapshots survive named replacement; fresh
drift fails. Focused Taker authority tests are GREEN at 5/5, with strict
Clippy, warning-fatal Rustdoc, rustfmt, and diff hygiene GREEN. No route maps
these descriptors, opens an RPC or node, or executes an effect yet. Literal M5
stays 4/7; ETA remains 3 to 7 focused implementation hours.

M5 atomic child-exec descriptor checkpoint (2026-08-02): generic non-Clone
plans validate 1..64 unique non-aliased owned sources and unique child targets
in 200..1023. XMR consumes runtime plus nine secrets into fixed FDs 200..209
beside program 197 and lock FDs 198/199 in one mapping, with no secret argv/env.
The process proof executes exact pre-replacement program/input snapshots, sees
FD 210 absent, and retains both locks in the child after parent Command/lock
drop until exit/reap. Negative tests cover empty, reserved, duplicate-target,
and aliased-source plans plus redacted Debug. Full swap-store and XMR actor
all-target/all-feature regressions, strict Clippy, warning-fatal Rustdoc,
rustfmt, and diff hygiene are GREEN. No lifecycle route, RPC, node, or effect
participates. Literal M5 stays 4/7; current ETA is 2.5 to 5.5 focused
implementation hours.

M5 complete XMR effect-input map checkpoint (2026-08-02): authority now requires
a normalized, non-overlapping shared-wallet file-password path. At use it is
validated and pinned as the tenth sealed secret on FD 210. The exact one-call
child map is now 197..210, the process sentinel is absent FD 211, and the child
verifies the pre-replacement runtime plus ten secrets. Validated Maker execution
authority retains the semantically validated public Stage-A/B paths and exact
wire SHA-256 values. Current authority/input validation, sealed custody, and
child-map gaps are closed; lifecycle route, RPC, node, classifier/reconciliation,
and chain-effect composition remain open. Literal M5 stays 4/7; ETA remains 2.5
to 5.5 focused implementation hours.

M5 XMR role-fixed invocation checkpoint (2026-08-02): schema-v3 execution load
retains exact effect-authority digest and workflow identity. The six sending
role/step slots pin the selected tool, runtime, ten secrets, exact actor/workflow
locks, and complete FD 197..210 command before workflow-v2 authorization.
InvokeOnce alone returns a Command plus stable domain-separated plan digest;
ObserveOnly and Complete return that digest without a Command. The real
schema-v3 Taker Tag14 fixture proves corrupt program and wrong-role failure
without burning Prepared, exact child FDs, one InvokeOnce, and restart
ObserveOnly with identical digest. No RPC, node, semantic tag-14 publication,
classifier, or funds participate. Literal M5 remains 4/7; current ETA is 2 to 5
focused implementation hours.

M5 receipt-v2 Taker Tag14 process checkpoint (2026-08-02): the real
`lez-taker claim --receipt` route validates schema v3 under separate actor and
workflow locks. The first claim wins one durable CAS, invokes and reaps the
hash-pinned Tag14 sender marker with FDs 197..210 exactly once, returns schema 3
`invoked_unreconciled` with a nonzero plan digest and false chain finality, and
leaves Started. The second claim starts no sender: it hash-pins the role-fixed
finalized observer, requires the exact original sending-plan identity, parses a
bounded step-exact result, derives `lez_finalized_event` locally, and reconciles
Succeeded. The third claim returns Complete without sender or observer. Only
Started and Unknown admit observation; Prepared and Succeeded reject it. Any
observer failure leaves the journal unchanged, while ambiguous sending stays
sticky Unknown and never rearms. The losing refund branch fails closed. The
real Maker-daemon/Delivery/Chat black-box is GREEN 1/1 in 133.16 seconds, the
focused effect-route suite is GREEN 5/5, and strict Clippy plus warning-fatal
Rustdoc are GREEN. Both programs are fixed local markers: no RPC, node, semantic
Tag14 transaction, actual-chain classification, or funds participated. This is
process-component evidence, not on-chain proof. Literal M5 remains 4/7; outputs
5/7, 6/7, and 7/7 remain.

Evidence correction recorded 2026-07-30: retained M4 run `m4cert20260722an` and M5 run `m5-xmr-app-20260730-2c6aec1-h` prove finalized LEZ effects, adaptor extraction, shared-key reconstruction, a confirmed Monero sweep, binding, and exact cleanup. A role audit proved their runner used provisioner funding, a Taker-hosted shared wallet, and a Maker sweep destination. Those runs no longer certify role-correct user economics. The code now enforces Maker funding and claim mining, a neutral provisioner shared-wallet RPC, and a Taker claim destination; a fresh role-correct claim replay remains separate and the historical evidence is unchanged. Authenticated tag-16 Taker prepare, aggregate completion, transaction-derived one-attempt submission, finalized Maker discovery, Maker ingestion/extraction, role-correct Maker recovery sweep, and the conditional refund binder became component-GREEN. Ambiguous submission is sticky across restart without resend and refund classification enforces `[refund_at, punish_at)`. Diagnostic run `m5xmrrefund8c10cd7a` proved read-only polling could not advance an idle local finalized clock; the later exact run below supersedes this historical refund-replay-open status.

Finality correction recorded 2026-07-31: run `m5xmrrefund842610ca` admitted one
terms-sealed clock effect, advanced sequencer height 193 to 194 with exact
accounting and unchanged escrow state, and obtained ten Bedrock descendants in
about 16 seconds. The failure was repeated classification of fixed block 120,
not stalled finality. The authenticated current-finalized-tip path is focused
GREEN and the runner now classifies exactly the returned finalized height. A
fresh pushed-commit refund replay was the next gate; the literal score stayed 3 of 7.

Corrected replay recorded 2026-07-31: exact pushed-commit run
`m5xmrrefund45924caa` finalized one sealed clock effect, tag 16 in block 198,
and a Maker-directed Monero sweep at ten confirmations, then passed conditional
binding and cleanup schema v2. The retained packet is
`docs/evidence/m5-xmr-application-refund-corridor-20260731.json`.

ZEC daemon certification recorded 2026-07-31: exact pushed-commit run
`m5zec432dapp1` at `432d1f7dabbb573b9642794155066e37ee95e75d`
completed in 25,030 milliseconds. Maker and Taker completed at revision 4; the
scheduler resolved `terminal` at generation 24 after 24 attempts with no child.
The post-lock cutover removed Delivery, Chat, and owner control while preserving
daemon-only Maker effect authority and a receipt-bound Taker claim. The terminal
owner restart made no chain RPC, and exact scoped cleanup passed. No public RPC,
faucet, testnet, external funds, or other public runtime dependency
participated. The retained packet is
`docs/evidence/m5-zec-daemon-supervisor-certification-20260731.json`.

Repository security re-attestation recorded 2026-07-31: every one of the ten
repository locks containing `ruint` resolves fixed `1.20.0`, and all 13
dependency graphs pass without a waiver. Provisional v0.2 ELF
`ade4af84...bbcee` / ImageID `b7f87278...b0433` passes five recursive cases.
Compatibility run `m5-ruint-v012-final-20260731`, using Risc0 3.0.5 and builder
`r0.1.94.1`, reproduced ELF `fe8ec116...c739f7` / ImageID
`5421868e...add62` and passed 6 ordinary + 2 actual lifecycle + 1 cost case.
The CI-required stable cost policy preserves exact identity, topology, totals,
budgets, and classification arithmetic while permitting only internally valid
volatile classification changes. The initial runner exit `1` was solely the
superseded byte-identical volatile cost diff. This remains historical context;
the later ZEC daemon certification moves M5 to 4 of 7 with an 8 to 16
focused-implementation-hour closure ETA.

## Historical M5 in-progress scorecard

Status: superseded by the verified 7/7 closure checkpoint at the end of this
document and tag `m5-poc-complete`. The measurements below preserve the
historical path to closure and are not the current milestone state.

| Metric | Current measurement | Evidence or next measurement point |
|---|---|---|
| Live authorities reconciled | 2 of 2 | Re-read 2026-07-30: RFP master `b59c620...16d`, blob `ec6b602...e16`, raw SHA-256 `a83d0b87...3d498`; issue #112 remains open/accepted/RFP-003, retains normalized body SHA-256 `49356263...f1c87`, and supersedes #61. No scope drift; seven M5 outputs remain |
| Literal M5 outputs complete | 4 of 7 component-certified | The fuzz harness, both pricing modes, documented Delivery/Chat outage behavior, and daemon systemd/future-Core lifecycle are fully GREEN. The remaining outputs are Maker full supported-pair lifecycle, Taker full supported-pair lifecycle, and coordinator accepted-application actual-chain concurrency/restart/unavailable-XMR isolation |
| Application binaries | Maker daemon and CLI expose owner-local pair policy, exact local price, durable offer publish/list/withdraw, swap create/status/history, alerts, and generation-fenced monitor/claim/refund; daemon-owned authenticated Delivery, real taker acceptance, role-fixed provisioning, receipts, and offline monitoring are GREEN across the proven pair slices | BTC and XMR actual-node executions are clean-GREEN. Fixed packaged-system-service start/stop and receipt-only XMR Taker monitoring are GREEN. Exact run `m5xmrrefund45924caa` clean-GREEN proves the role-correct XMR refund corridor. Exact run `m5zec432dapp1` clean-GREEN proves daemon-only Maker ZEC authority and the receipt-bound Taker claim through terminal settlement and offline restart. Full Maker/Taker supported-pair lifecycles and coordinator actual-chain composition remain |
| Owner-local control | GREEN component slice | Mode-0700 owner runtime directory, disjoint mode-0600 owner and Chat Unix sockets, absolute/no-symlink paths, bounded HTTP/1 JSON-RPC, disabled batches, 16-connection cap, no-clobber readiness, exact-inode cleanup, and two black-box daemon/CLI journeys pass strict gates |
| Daemon supervision | Exact actual-chain ZEC application output GREEN | Exact-pinned notification, typed health, SIGTERM, mode-0400 credentials, process-lifetime database lease, hardened unit, staged actor install verification, and exact-child adapter tests pass. Historical clean-cache route run `lez-m5-systemd-1000-2947208-15620` retained one route across SIGKILL restart in 51 seconds; warm cache took nine seconds. Node-free actor crash run `lez-m5-systemd-1000-3497452-2505` retained one durable fixture effect across a ten-second daemon restart. Exact pushed run `m5zec432dapp1` then certified the actual Zcash-chain effect: both actors completed revision 4, the scheduler resolved terminal at generation 24 after 24 attempts with no child, transport stayed absent after lock, and the owner view restarted without chain RPC. Live Logos Core attachment remains LOGOS-019 |
| Persistent process coordination | Schema-v21 application store and scheduling, replay-safe manual actions, literal ZEC claim/recover routing, atomic secret-free progress, allowlisted owner monitor, generation-fenced Maker claim/refund RPC/CLI, held-lock recovery, physical artifact binding, atomic ZEC acceptance-registration, bounded per-swap authority registry, authority-bearing systemd restart, expiry-independent replay, both real BTC/ZEC sealed-config consumers, symmetric no-clobber Taker provisioning, pinned acceptance receipts, Delivery-independent persisted acceptance replay, and kernel-locked Taker lifecycle execution, exact-snapshot pair semantic comparison, bounded supervisor execution, prompt process-group cancellation, and node-free user-systemd crash recovery GREEN | The BTC process PoC adds Taker persist-before-completion, atomic final-wire/coordinator/offer/Maker-actor/replay commit, role-only Maker/Taker publication, receipt-after-completion, Delivery-free exact completion replay, inode preservation, receipt-only offline monitor, and an exact pushed actual-node corridor. XMR Stage A now reserves without authority; Stage B atomically derives its coordinator, consumes the offer, and registers one immutable Monero actor with restart, rollback, replay, cutoff, and schema-preservation checks. Fencing, peer isolation, strict output binding, restart durability, and exact replay remain GREEN. The clean XMR corridor closes the composed supervisor-to-legacy handoff. Receipt-only XMR Taker application monitoring is kernel-locked and process-GREEN. Exact run `m5zec432dapp1` closes daemon-owned ZEC claim authority, terminal reconciliation, and chain-RPC-free owner restart; full supported-pair lifecycles and concurrent live-node composition remain |
| Price-source runtime adapters | 2 of 2 application-path GREEN | The local adapter/CLI remain GREEN. Four actual-C worker tests plus four parent tests prove process isolation and validation. Schema-v15 tests prove replay-before-effect, policy CAS, module-epoch high-water, rollback/equivocation rejection, bounded freshness, and atomic immutable-offer binding. Daemon selection, no-fallback behavior, exact signed snapshots, replay-before-effect, failed-source rejection, and restart reconciliation pass real-process tests |
| Discovery and negotiation | ZEC application transport GREEN; BTC and XMR pre-effect process transports GREEN | ZEC retains signed Delivery, durable-first Chat, atomic acceptance, replay/loss recovery, and post-lock removal evidence. BTC now runs signed Delivery discovery and authenticated proposal/completion through the real Maker CLI, maker daemon, and Taker CLI. The Maker and Taker contribute their own Schnorr signatures, schema 19 atomically binds the consumed offer, coordinator, final wire, and Bitcoin Maker actor, and symmetric schema-6 provisioning publishes only the selected role. Exact completion replay succeeds from the durable final wire after Delivery removal, preserves agreement/config inodes, and supports receipt-only offline monitor. The focused 1-of-1 process run passed in 0.87 seconds with no RPC, chain, Docker, faucet, DNS, network, or public funds. The XMR process proof passed 1 of 1 in 307.71 seconds with canonical role material, crossed-reservation zero-write, reserve-only Stage A, atomic Stage B, Delivery removal, daemon reopen, immutable role journals and actor/receipt artifacts, and no chain resource or effect. BTC and XMR actual-node application corridors are clean-GREEN. LOGOS-020 remains an upstream production-parity caveat |
| XMR application process, semantic supervisor, and Taker monitor | Handoff process-GREEN at 1 of 1 in 307.71 seconds; exact schema-v2 supervisor process GREEN at 1 of 1 in 79.22 seconds; receipt-only Taker monitor and receipt-v2 Tag14 marker claim GREEN in the real-process journey; tag-16-to-Maker-refund runner actual-node GREEN; 4 exact functional actual-node completions including clean-certified claim and refund replays | Exact run `m5-xmr-app-20260730-2c6aec1-h` repeated plan → Stage A/B → real acceptance → typed no-effect `Blocked` → authenticated restart reconciliation → empty Delivery outage → Delivery-free replay → synchronous cutoff → finalized tag 13/14/15 → extraction → sweep → binding. Claim transaction `05cb9052...349fce` was in LEZ block 139 and finalized by tip 142. Monero sweep `37930570...1603c8` received 998191600000 of 1000000000000 piconero after a 1808400000 fee and reached 10 confirmations at tip 130. Cleanup schema v2 passed with source zero, exact absence, preserved sentinel/latch, no foreign/broad cleanup, and no failure reasons. After Delivery and Chat removal, `lez-taker monitor --receipt` validates the complete Taker application authority under the per-swap lock and emits fixed pre-effect status without RPC, chain inference, or state write. Receipt-v2 `lez-taker claim` now sends one durable Tag14 marker exactly once, observes a fixed finalized marker on restart with the same sending-plan digest, reconciles local evidence-shaped Succeeded, and returns process-free Complete on the third call. This makes no chain request and is process-component evidence, not semantic or actual-chain proof. Separately, the opt-in refund runner composes authenticated tag 16, Maker finality/ingestion/extraction, Maker-directed Monero recovery, receipt verification, and conditional binding. Exact pushed-commit run `m5xmrrefund45924caa` finalized one sealed clock transaction, tag 16 in block 198, and Maker sweep `252b922e...d4caf` at ten confirmations. It proved exact clock accounting with unchanged escrow custody and metadata, conditional atomicity, and cleanup schema v2; semantic actual-chain Taker CLI claim/refund controls and path-reopen ABA hardening remain |
| Persistent restart evidence | Exact packet-bearing replay `m5app6c3bbbe20260724a` retained pre-effect state and returned `Completed` from fresh post-terminal owner history/status while Chat/Delivery stayed absent; receipt `929c9a7c...c58e` binds source revision 4. Exact pushed run `m5zec432dapp1` repeated the post-lock transport cutover, completed Maker and Taker at revision 4, and projected the terminal owner view with no chain RPC | GREEN for the progressive local ZEC application and daemon-supervision PoC |
| Concurrent application swaps | 0 actual applications composed; 1 real-daemon simultaneous two-worker process composition GREEN; 10 of 10 deterministic repeats | One terminal actor completes while a disjoint child remains live and Leased; releasing that child changes only it to Backoff. Restart retains both exact manifests with one attempt and no replay. Distinct accepted application agreements, escrows, deadlines, and actual-chain effects remain |
| Missing-chain degradation | 1 explicit route-control process journey; 0 unavailable-node application compositions | Disabled Zcash quote/publication rejection, unaffected Bitcoin quote, restart durability, and revisioned Zcash re-enable are GREEN. Next add automatic health behavior and one actual unaffected-pair swap |
| Delivery/Chat removal after lock | 4 exact-tree actual-node runs GREEN | Exact pushed run `m5zec432dapp1` retained the restarted daemon through Zcash funding, removed Delivery, Chat, and the owner socket after the first confirmed lock, then reached terminal settlement and an offline terminal projection |
| Coordinator fuzzing | Literal cargo-fuzz target plus 7 retained seeds; local 512-run smoke GREEN at 1,161 covered counters and 4,926 features | BTC/ZEC both directions and LEZ-first XMR cover transition rejection, reorg/removal, claim/refund/recovery, immutable terms, terminal absorption, and restart after every action; the isolated graph audit is GREEN |
| Actual local application swaps | 9 functional happy/refund completions; 8 checked records including 2 packet-bearing or daemon-certified ZEC replays, clean BTC, clean XMR claim/refund, and 2 cleanup-uncertified XMR diagnostics | Run `m5zec432dapp1` adds one clean, 25,030-millisecond ZEC daemon-supervision completion and raises the literal output score to 4 of 7. Multi-application concurrency remains separate and open |
| Actual local one-leg recoveries | 1 intervention-assisted historical LEZ refund checkpoint plus 1 clean reproducible XMR refund | Run `m5xmrrefund45924caa` finalized tag 16 within the signed window, derived the refund share only after finality, and confirmed the Maker-directed Monero recovery with exact accounting and cleanup. The older `m5fresh-a390dd8-20260728a-app3` remains intervention-assisted historical evidence and is not counted as clean |
| Local finalized-clock liveness | 3 diagnostic REDs; finality-observer repair focused GREEN; 1 corrected complete replay | Run `m5xmrrefund45924caa` used exactly one terms-sealed clock effect, advanced authenticated finalized height 188 to 192 within 60 seconds, then classified exact tag 16 at finalized block 198. No second effect or weakened security parameter was used |
| Public runtime dependencies | None required for PoC | Record cold-download dependencies separately; runtime uses local nodes and deterministic funds |
| Cleanup leaks | 0 observed exact-resource leaks; two clean-certified XMR replays and one clean-certified ZEC daemon replay plus two retained failed-cleanup functional diagnostics | Claim run H, refund run `m5xmrrefund45924caa`, and ZEC run `m5zec432dapp1` passed exact scoped cleanup with every owned resource absent and no broad or foreign cleanup. Runs F/G remain fail-closed diagnostics; their resources were also absent |

The progressive local ZEC and exact pushed BTC application PoC gates are closed,
the exact pushed XMR claim and refund corridors are clean-certified, and exact
pushed run `m5zec432dapp1` certifies the daemon-driven ZEC deadline/cutover
output. Literal M5 completion is 4 of 7. The milestone-tag ETA is 2 to 5
focused implementation hours and is updated after every push. The remaining
outputs are Maker full supported-pair lifecycle, Taker full supported-pair
lifecycle, and coordinator accepted-application actual-chain
concurrency/restart/unavailable-XMR isolation. Evidence/document closure,
composite gates, and tag review remain closure gates rather than literal
outputs.

## M4 PoC scorecard

Status: active progressive local-PoC; successful-claim working-tree checkpoint reached, exact committed-tree certification pending. Counts below distinguish inherited
generic scaffolding from XMR-specific implementation and actual-node evidence.

| Metric | Current measurement | Evidence or next measurement point |
|---|---|---|
| Live authorities reconciled | 2 of 2 | The live RFP and accepted replacement issue #112 were re-read on 2026-07-19. The normalized issue body still hashes to `49356263a762307abc0f8dd2863ac5af8fe13d9b17b674f242d025de655f1c87`; issue #61 and the old ETH submission are excluded |
| Literal M4 outputs complete | 0 of 6 certified; 1 successful-claim working-tree checkpoint | The same-run claim closes the first functional vertical image but not a literal output on a clean pushed commit. Exact replay, cleanup, signed recovery, F7, U9, D1 XMR, and final conformance/documentation closure remain |
| XMR-specific executable crates/components | SDK, actor, signing, bridge, Monero, release, classifier, and role-lifecycle components compose one actual successful claim | The working-tree run exercised Stage A/B, tag 13, exact Monero funding, exclusive preparation, tag 14, Maker tag 15, Taker extraction, and official-wallet sweep. Recovery builders, definitive absence, rollback controls, and adversarial hardening remain |
| Isolated XMR release worker | Component gates GREEN and one actual-local working-tree admission executed | The separate release-only worker consumed only the fresh sealed Prepared state and admitted exact tag 14. The two failed preparer states remain quarantined. Different-UID/network isolation, exact committed replay, definitive absence, rollback anchoring, and cancellation-after-CAS hardening remain |
| Exclusive XMR release preparer | Component gates GREEN and one fresh actual-local `release3` preparation executed | It re-derived Stage A/B, recovered exact tag-13 bytes, proved finalized Fund, authenticated peerless Regtest topology and exact output, consumed the completed Taker journal, then exclusively created and reauthenticated mode-0600 state. Two earlier failed states exposed omitted-empty `connections` and remain quarantined; no failed state was deleted or reused |
| M4 v3 bridge-client contract | 9 of 9 strict protocol/server methods: 8 ordinary plus 1 release-intended typed surface; 53 of 53 package targets GREEN | The ordinary `BridgeClient` retains its eight Maker/Taker preparation, completion, and classification methods. `XmrReleaseClient` provides the Taker-bound authorization submission method through a type-narrowed Rust surface, rejects a Maker runtime before transport, validates response context/terms/ID, and never retries. Invalid local bindings make zero calls; a wrong mock response ID fails after one client call. The focused sidecar matrix is GREEN 3 of 3: accepted, exact byte-identical `AlreadyKnown`, and wrong official returned ID mapped to `UnknownSubmissionOutcome`, with exact lookup/send/replay counters. Raw bearer access can still reach the authenticated method, so this is client and official-type loopback validation, process wiring is checked separately, but this is not different-UID isolation, actual-node finality, or actor execution |
| Typed Stage-B authorization and pre-Fund finality barriers | 98 non-doc adapter tests plus 3 doctests; authenticated journal matrix 5 of 5; strict Clippy, Rustdoc, formatting, and diff gates GREEN | Only `LezBridgeAdapter<BridgeClient>` mints private-field non-`Clone` evidence. Stage-B authorization re-derives the committed partial and runtime binding. The Taker handoff opens only an existing completed role-bound claim journal, exact-compares its identity, transcript, commitments, nonces, Maker partial, and withheld-partial commitment with Stage B, and creates no secondary plaintext store; missing, incomplete, role-crossed, or transcript-crossed journals make zero RPC calls. The ADR-0070 barrier mints exact finalized-Initialize evidence and consumes it before authenticated Fund submission; mismatched evidence fails before transport. The official sidecar independently checks durable transaction semantics. These remain component capabilities, not actual-node or claim-PoC evidence |
| Native-XMR preparation, tag-13 execution, finalized effect classification, and authorization submission | Actual working-tree claim executed tag 13, tag 14, and tag 15 with role-local finality | Initialize/Fund finalized at 3953/3960, tag 14 at 4107, and tag 15 at 4208 with terminal custody zero. The Maker and Taker consumed only role-correct canonical evidence. Tag-16/tag-17 recovery builders and their actual effects remain |
| Pure Stage-A future-message plan | 3 of 3 focused tests GREEN; zero I/O by construction | One caller-supplied stable finalized snapshot drives the checked nonce schedule: Taker Initialize/Fund/Authorize, independent claim/refund aggregate authorities, and Maker punishment. The planner constructs exact generated official tag-15 claim, tag-16 signed-refund, and tag-17 punishment messages and their distinct NSSA hashes. The existing tag-15 prepare/complete path accepts the planned claim message and hash byte-identically. Planning performs no RPC, reservation, persistence, signing, or submission; tag-17 sidecar builder remains `Unavailable`; tag-16 submission and classification are covered separately below, and no chain or swap effect is claimed |
| Independent XMR role provisioning and actual-local Stage-A actor path | 4 provisioning plus 2 black-box Stage-A process tests GREEN; 2 fresh manual provisioning invocations and 1 actual-local two-devnet Stage-A replay GREEN | `xmr-reference-actor` atomically provisions no-clobber owner-only role bundles, validates every private/public binding, signs in separate Maker/Taker processes, assembles role-indexed signatures, and publishes each complete `0700` claim/refund session root by one no-replace rename. The read-only sidecar composer observed authenticated Monero genesis plus actual LEZ chain/account/finalized facts, cross-checked finalized block 2281 with the sequencer, and emitted canonical commitment `170c23ad...66009`; same-purpose session bytes matched across roles. Same-host evidence is not different-UID isolation, and a path-only same-UID unpublished-orphan residual remains. No composer/actor chain effect, public RPC, peer, faucet, public funds, external finality, or completed swap participates |
| Independent role journals and canonical Stage B | 1 of 1 actual pre-effect replay plus 1 focused black-box process test GREEN | Exactly one long-lived SQLite database per role carries claim and refund. Commitment/opening order is durable; Maker exposes both partials, Taker exposes only refund, and the two refund presignatures match. The Taker-only composer reads one journal path and emits 747 canonical bytes; separate role processes sign and assemble an 875-byte SDK-validated activation. Exact-current replay hashes are `85cee706...afaf4` unsigned and `df65d354...5da2` signed. Incomplete journals, crossed signatures, disclosure of the exact Taker claim partial, and output clobber are rejected. No RPC or chain effect occurs in this slice |
| Actual-local tag-13 Initialize/Fund | 2 of 2 ordered effects finalized once | The role-fixed Taker actor consumed the canonical Stage-A/B hashes and funded signer, anchored a stable four-account finalized nonce snapshot, then finalized Initialize transaction `8013ad91...7676` in block 3008 before Fund transaction `9b643629...da46` in block 3023. Both were before the signed cutoff. Raw owner-only evidence is mode `0600`, one link; the repository packet records no Monero lock, completion, or atomicity claim. No public RPC, peer, faucet, public funds, or external finality service participated. Observed local cadence was about 2.5 minutes per finalized effect and remains an iteration-speed measurement. The signed continuation ends at `2026-07-21T05:05:40Z`; this run remains historical tag-13 evidence and a fresh wider-window run will follow live-path composition |
| M4 request-journal concurrency residual | 1 inherited race; not exercised by the single-actor PoC | The generic server releases its journal lock before executing and later writes the outcome. Two concurrent different bodies under one request ID could both pass the first check and the later error could overwrite success. Tag-15 inherits this older behavior. The progressive PoC permits one actor and one in-flight request per swap; RED-GREEN CAS serialization plus adversarial concurrency belongs to post-PoC hardening and remains required for production readiness |
| Monero output observation boundary | 7 focused adapter tests plus 1 public issuer integration GREEN; non-cloneable result is durably consumed | Typed `monero-rpc` 0.5.1 calls bind network/genesis, transaction, standard address, amount, wallet-reported availability, canonical decoded block membership, at least ten confirmations, and a stable tip. The issuer integration binds the same run and authenticated topology, signed Stage A/B, finalized Fund, and exact publication before journal persistence. The test uses authenticated loopback fixtures, not an actor or actual chain. View-only spent status, discarded upstream header trust flags, upstream block-decode panic, and pre-decode response bounds remain explicit residuals |
| Typed Monero wallet effects | Component gates GREEN; one exact agreement funding and one reconstructed-key sweep executed in the working-tree claim | Official wallet RPC funded the exact Stage-A address with 1 XMR, confirmed it at tip 120, reconstructed the wallet only after finalized tag 15 extraction, and confirmed the sweep at tip 130. Exact clean-commit replay and ambiguous-outcome recovery remain |
| Finalized-Claim-to-sweep binder | Actor implementation, negative accounting matrix, and one retained owner-private invocation GREEN | The 3203-byte mode-`0600`, one-link packet at SHA-256 `896d05d3178e3ff44b6ca010d4528835f5d796dc7e1004984ed78e853c083306` revalidates Taker Stage A/B and journal, finalized LEZ Claim 4208/tip 4220, aggregate-signature extraction, reconstructed public key, and independent Monero receipt 121/tip 130. Retained provenance is `legacy_v1_plus_receipt_v2`: received 998191600000, exact fee null, remainder 1808400000. Current sweep-v2 exact-fee handling is focused-tested only. Destination ownership is the explicit owner-private Taker-wallet boundary, not a Stage-A commitment; distributed-transaction and future-reorg immunity claims are false |
| Typed XMR release issuer and journal | Component gates GREEN; actual working-tree Prepared-to-Admitted transition executed once | The successful `release3` journal cross-bound the same-run Stage A/B, finalized Fund, exact Monero output/topology, signed exclusive deadline, and tag-14 bytes. The release and sidecar journals are separate transactions. One-host custody, rollback, same-UID, definitive-absence, and cancellation-after-CAS hardening remain |
| Checked M4 LEZ guest artifact | 3 successful fresh builds; 5 of 5 recursive tests passed three times | Two historical digest-pinned executions reproduced ELF SHA-256 `dc370bc34b432317730c51b49342760dbc675fca700e300b30b5fadefe5b7292` and ImageID `4d6590332948743c2db88a183755815354ef92560550cd206ac27bddeea12c82`. The 2026-07-31 advisory-remediated rebuild reproduced new ELF `ade4af8426040b7e5c171b559a382a15a3fa72e27531a93fe89742689a1bbcee` and ImageID `b7f8727893174a29bd776eacbfdd9773e0510ebdac43102cb7e93ba4fa0b0433`. The serial suite preserves one native aggregate-witness compatibility case and covers XMR initialize/fund/claim, preauthorization rejection, wrong partial, exact Taker authorization, signed refund, punishment, and transfer-failure rollback in four XMR cases. This is a local checked artifact, not public deployment or actor runtime evidence |
| Checked M4 local deployer and actual deployment | 4 focused tests GREEN; one historical exact checked program (ELF dc370...; ImageID 4d659...) finalized on an isolated v0.2 stack | `deploy-m4-local` accepts only a literal-loopback HTTP sequencer URL, nonzero channel ID, and timeout. Before RPC it validates the pinned M4 manifest, append-only generated IDL, ELF SHA, ImageID, runtime health/channel/genesis/built-ins/tip; its code has one send per invocation and no automatic retry. Transaction `8bb883f1...63f9` finalized in block 86. A full finalized genesis-through-86 scan proves zero exact-ELF occurrences before submission and exactly one afterward, decoded ELF/ImageID equality, sequencer/indexer inclusion equality, and stable ID/hash/ID rereads. Runtime external resources are `[]`; no public RPC, peer, faucet, public funds, or external finality service participated. The retained packet does not independently prove a global RPC-attempt count or any swap effect |
| M4 actor Vault onboarding | 2 of 2 independent identities finalized once | Deterministic local genesis allocated 200000 to the Taker and 100000 to the Maker. Their separate Vault Claim transactions finalized once in blocks 228 and 240; both owner nonces became one while balances remained allocated and Vault balances remained zero. Owner-only state rejected a group-writable repository ancestor before any submission, then succeeded under a fresh mode-`0700` `/tmp` root without relaxing the check. This is funded identity/nonce readiness, not a lifecycle actor, tag-13 effect, Monero lock, or swap |
| M4 sidecar state-directory lease | Source/component GREEN; 2 library tests plus 1 binary lifecycle test | A fixed `bridge-state-lease.v1.lock` is opened relative to the held state directory and must be owner-only mode `0600`, single-link, empty, inode-stable, and exclusively nonblocking locked. The bridge acquires it immediately after argument validation and before config/node/store/server work, then holds it until shutdown. Parent launcher adopted-state support, typed tag-13-to-tag-14 export, and actual continuation replay remain pending |
| M4 artifact runtime/cold-cache boundary | Runtime external resources `[]`; four cold-setup dependency classes | Checked recursive execution uses no RPC, faucet, peer, public chain, or public finality service. A cold cache can require the pinned circuits release, crates.io/pinned Git sources, the digest-pinned Docker builder, and Risc0 release tools; DNS, registry, rate-limit, or availability failure can block setup. Default cleanup retained only the small checked ELF/evidence and removed about 3.49 GiB of exact run-owned build/tool state |
| Pair-neutral adaptor extraction | 10 leaf tests, 2 independent adaptor vectors, 16 BTC agreement tests, 32 BTC facade tests, and 4 role-process tests GREEN | ADR 0056 records the dependency-leaf boundary. Byte-exact durable-context and nonce-commitment assertions protect the `bitcoin::hashes` to `sha2` move; BTC top-level imports remain compatible and the role runner now depends directly on the leaf |
| Current focused M4 ETA | Runner/PoC implementation 3 to 7 focused hours; then warm replay 25 to 45 minutes or cold replay 1 to 3 hours; full functional closure remains 15 to 27 focused hours | The binder is complete. The runner source/contract now reaches the official Monero child, agreement and separate role journals, a durable no-retry latch, and exact one-shot finalized tag 13 before intentionally failing ahead of swap-specific Monero funding. That path is not clean-replayed from the current commit. The sidecar is still unwired; its exclusive state-directory lease is component-GREEN, while adopted-state launch, typed tag-13-to-tag-14 export, and actual continuation replay remain. The 15-to-27-hour closure estimate includes tag-16/tag-17 recovery, F7 parity, U9, D1 XMR, and closure gates; owner-selected QA, chaos, information-security, and production-readiness phases are excluded |
| Actual local LEZ/XMR happy swaps | 1 successful claim journey on a working tree; 0 clean-commit certified | Run `m4happy-40cbac3-20260721a` executed LEZ Initialize/Fund, exact XMR funding, sealed tag 14, Maker tag 15, Taker extraction, and confirmed sweep. The owner-private binder now directly ties finalized Claim to the receipt and reconstructed key, but exact committed replay remains required. It must be repeated from the exact committed tree before certification |
| Positive directions in reviewed scope | 1 of 1 executed on a working tree | LEZ-first is the only reviewed direction. XMR-first remains a zero-effect negative path unless a new construction and review supersedes ADR 0008 |
| M4 dependency groups accepted | 3 PoC groups: crypto/key slice, official node binaries, and isolated Rust RPC observation graph | Pinned `sigma_fun` 0.9.0, `monero` 0.22.0, postcard 1.1.3, and `monero-rpc` 0.5.1 pass advisories, bans, licenses, sources, strict Clippy and Rustdoc. `tiny-keccak` 2.0.2 has an exact CC0-only exception. Official Monero 0.18.5.1 retains its signed artifact identity. The RPC graph is accepted only for credential-configured literal-loopback observation with the recorded production residuals, not public RPC or release authority |
| Deterministic DLEQ/share spike | 2 successful proofs plus symmetric reconstruction | Maker scalar-one retains the expected basepoints, 56,611-byte proof, SHA-256 `0634e8a021bde0d9dd8461d0a8ccd1c56f85ec790b21ba78be27404d4121afe6`, and transcript `b9169740ae7b7a91b5c2e7971896a86b64286dbda218d711587109d2941852c8`. Both actors now supply bounded canonical proof envelopes; claim order `s_a + s_b` and signed-refund order `s_b + s_a` open the same shared address. Full vector/negative corpus remains post-happy-path work |
| Official-wallet reconstructed spend | 1 causal actual-claim sweep plus 1 earlier development success | The working-tree claim reconstructed the Stage-A wallet only after extracting the Maker share from finalized tag 15 and confirmed sweep `6c8c7bca...e21a` at tip 130. The earlier deterministic-share run remains behavior evidence only |
| Actual Monero topology bootstrap | 7 successful fresh runs after 1 assertion-defect run | The five earlier runs are joined by `m4stagea-fb67fe1-20260720a` and corrected-manifest run `20260720b`. Both new runs reached the official offline peerless fakechain with one daemon, three wallets, distinct Digest credentials/stores, real two-destination local funding, ten confirmations, and unlocked 10 XMR per role. Run `20b` directly supplied stable owner-only username/password files to the actual composer; `20a` was exactly cleaned after that handoff improvement. Run 19a failed closed only on the since-fixed final-height assertion and cleaned exactly |
| Monero bootstrap iteration time | 53 seconds before cleanup on run 19c | 30 seconds signed-release/source verification, 3 seconds cached image build plus four-process readiness, and 20 seconds wallet bootstrap and assertions. The 100 funding blocks and 10 confirmation blocks are locally generated at fixed difficulty; no wall-clock finality wait is used |
| Monero role isolation | 4 of 4 RPC bindings literal-loopback; 3 distinct wallet credentials/stores | Maker credentials against the Taker endpoint returned HTTP 401. All containers used UID/GID 65532, read-only roots, all capabilities dropped, `no-new-privileges`, tmpfs role stores, no published P2P/ZMQ, and a non-masquerading bridge |
| M4 actual-claim replay runner | Source/contract GREEN through exact one-shot finalized tag 13; 0 clean current-commit replays | `scripts/run-m4-actual-claim-poc.sh execute` now composes exact fresh Maker/Taker Vault Claims, starts the official Monero child, creates canonical Stage A/B and separate role journals, durably latches no-retry before invoking tag 13, and intentionally fails before swap-specific Monero funding. Exact cleanup is ledgered, run-label/PID revalidated, foreign-sentinel guarded, and broad cleanup forbidden. The role sidecars remain unwired; adopted-state launch, typed tag-13-to-tag-14 export, the rest of the claim tail, evidence publication, and a clean actual replay remain incomplete. It is not a one-command happy-claim replay |
| Monero cleanup evidence | Earlier topology runs cleaned exactly; current claim checkpoint cleanup not attested | The public claim packet intentionally records `cleanup_attested == false` while exact committed-tree replay and review are pending. Future cleanup must use only captured run-specific Compose, network, image, volume, and process identities and must preserve foreign sentinels |
| Crypto/protocol specification discrepancies | 3 open proposal errata | GW-M4-001 records the unlicensed archived DLEQ target; GW-M4-002 records the underspecified Ed25519-adaptor versus actual LEZ BIP-340/h4sh3d mapping; GW-M4-003 records literal RFP F6 two-leg refunds versus the cited COMIT punishment fallback after Taker abandonment. None permits a missing adaptor/DLEQ, signed-refund, or disclosed punishment path |
| XMR atomicity prerequisites | Successful claim branch executed conditionally atomically on a working tree; recovery branch unexecuted | Stage A/B committed exact future messages and kept the Taker claim partial private. Tag 14 released it only after finalized LEZ Fund and confirmed exact XMR funding. Maker tag 15 claimed LEZ and revealed the share Taker extracted before reconstructing and sweeping XMR. The binder now machine-checks that finalized-Claim-to-sweep snapshot and explicit destination/accounting boundaries. This is not one distributed commit; actual signed tag-16 refund/tag-17 punishment and recovery remain required |
| Public RPCs, peers, faucets, or funds used | 0 in the actual claim checkpoint | Actual isolated LEZ v0.2 and official Monero 0.18.5.1 processes communicated only through dynamic literal-loopback RPCs and deterministic local genesis/Regtest funds. Cold setup may require pinned Cargo/Git, circuits, Risc0, image, and Monero archive sources; those availability dependencies do not participate in runtime finality |
| QA / chaos / information security / production phases | Not active | Begin only after the reproducible happy PoC and owner phase transition; continuous repository lint, vulnerability, license, source, secret, and image gates remain mandatory |
| M4 completion tag | None | `m4-complete` is forbidden until all six literal outputs and synchronized closure gates are proven on the exact clean pushed commit |

## M3 PoC scorecard

Status: progressive local PoC evidence gate met. Counts below deliberately measure
evidence instead of assigning a percentage to unlike work items.

| Metric | Current measurement | Evidence or next measurement point |
|---|---|---|
| Live authorities reconciled | 2 of 2 | RFP master commit `121da225...5542a` / blob `d0fa52b`; accepted issue #112 is open, retains the `accepted` and `RFP-003` labels, and has body SHA-256 `49356263...f1c87`; issue #61 excluded. Re-fetched 2026-07-18 |
| Executable BTC-specific crates/components | 3 BTC crates, 1 actual-node runner, 4 fixture examples/CLIs, 1 role-local journal, 1 role-runner crate, and 2 of 2 live actor directions GREEN | Source tests remain GREEN. Run `m3actor-20260716n` binds commit `6ded2f9`, certified script hashes, fresh one-shot actor processes, four terminal role stores, and exact replay |
| Typed Bitcoin Core adapter | 37 of 37 all-target test executions plus 2 of 2 happy and 2 of 2 refund actual-node actor integrations GREEN | Run H exercised exact Core 31.1 signed-anchor maturity, next-block eligibility, canonical three-item refund witness, txid/wtxid readback, one-attempt submission, containing-height confirmation, and terminal replay. Five focused tests additionally prove exact Testnet4 chain/genesis/index readiness plus self-hosted loopback and exact HTTPS route/profile composition without public I/O. Fee stress, bounded RBF/CPFP, live Testnet4, and reorg remain later hardening |
| LEZ BTC witnessed path | Fresh checked deployment/onboarding plus 2 of 2 happy and 2 of 2 refund actor directions GREEN | Run-n retains both witnessed claims. Run H separately finalized maker `RefundNative` `a5cbb48a...97e41` in block 111 and taker `RefundNative` `64e1005b...9a6b4` in block 292 under private-local 3.0-second slots. Bounded scans and finite 30-second reads are live-proven; upstream historical-account proof/snapshot limits remain |
| Durable signer, public-effect, and BTC recovery-state boundaries | Schema-3 actor: 49 library cases and 8 CLI integrations GREEN; actual-node happy and refund actors: 2 of 2 directions each | Run H covers rev2 to 3 to 4 direction mappings, role authority, pre-deadline no-send, exact bytes before one CAS/attempt, accepted-restart observation, owner/nonowner finality projection, terminal `Refunded`, and unchanged replay counts. Process-kill timing, cutoff/race stress, and malicious database-owner authentication remain pending; no chain/database atomic commit is claimed |
| Actual local timeout/refund compositions | 2 of 2 directions | `m3refund-20260716h` completed both ordered two-lock paths on fresh Core 31.1 Regtest and LEZ v0.2 actual nodes. All four role/direction stores are revision 4 `Refunded`. Each direction retains exactly 2 Bitcoin and 3 LEZ effects, one actor-owned refund per chain, no cooperative claim, zero replay resubmissions, no public RPC/faucet/funds, and exact non-foreign cleanup |
| Actual local refund runtime | 54 minutes 5 seconds from first retained evidence file through cleanup | The directions are sequential and wait for signed five- and fifteen-minute bounds with 3.0-second LEZ slots. This is deadline evidence, not a throughput baseline. Scheduling, finality, and moving-tip retries can extend it |
| Dependency groups accepted | 2 of 5 entry candidates | Core 31.1 and exact-pinned `bitcoin` 0.32.101 graphs passed their acceptance gates. The exact `musig2` 0.4.1 graph is locked, policy-gated, exercised through Core, the real LEZ guest, and independent crash-safe role processes, but remains an unaccepted beta/unaudited candidate pending stronger secret handling and review; `miniscript` and `corepc` also remain unaccepted |
| Fresh identity, guest, and pre-lock orchestration | Actual-node effect-bearing run GREEN | `m3actor-20260716n` generated fresh owners/Vaults, deployed exact guest `a199c5be...e293` / ProgramId `39b6a4db...4dec`, finalized onboarding, pre-admitted exact Bitcoin funding, finalized agreement and journals before effects, and hit planned anchors 102/104 |
| Actual local M3 happy compositions | Repository-owned actor: 2 of 2 fresh directions on 1 isolated Core/LEZ tuple | Run used Core `32913`, Bedrock `32914`, sequencer `32915`, indexer `32916`, and dynamic role sidecars. No public RPC/faucet/funds; exact cleanup attestation passed without targeting foreign resources |
| Supported happy directions completed | Fresh repo-owned actor composition: 2 of 2 | `TakerSellsForeign` and `TakerSellsLez` both reached revision 4 for maker and taker. Each Bitcoin contract outpoint was spent once and each LEZ custody account ended zero |
| Completed native happy E2E executions | 6 per direction actually executed; 3 per direction retained in committed secret-safe packets plus 3 owner-private runs | The earlier fourth pair is `m3actor-20260716n`. Fresh D1 happy and concurrent recordings add the fifth and sixth executions in each direction at evidence commit `a6eb1ad`, with exact hashes, terminal replay, and cleanup retained in the private bundle. Public packets cover the initial composition, schema-4 actor-owned locks, and earlier overlapping pair |
| F7 custom-token actual-node composition | Four complete pairs GREEN on clean pushed commits; required 3 of 3 repetitions per direction exceeded | Runs X (`422c72e`, 20m52s), Z (`1555749`, 19m10.95s), AA (`df7ed86`, 18m13.61s), and AD (`0826dd5`, 16m06.52s) each completed both actual-node directions at one-second cadence. Every direction ended revision 4 with exactly 2 Bitcoin and 4 LEZ effects, one Maker second lock, zero replay resubmission, zero custody, conserved total 250, and exact balances `175/75/0` or `75/175/0`; exact cleanup removed only captured resources. Runs Y, AB, and AC failed closed and count as no repetition. The F7 repeatability gate is closed; this is not the M3 tag |
| M3 official-wallet repeat preparation | Hardened cold 202.42 s; exact hit 10.35 s; certified Z/AA/AD hits 10.32/7.81/7.370 s | Same host and production inputs under policy 2, input key `6607d474...ded208`, wallet `28245d5f...f96e6`, 118,659,320 bytes. Cold/hit peak RSS was 856,824/33,844 KiB, about 804 MiB lower on hit. Contract tests cover miss/hit, expected-output pin, effective legacy Cargo config, production test-var rejection, dirty source, fingerprint failure, tamper/mode/missing object, no-overwrite, and concurrent one-build publication. Runs Z, AA, and AD retained production-mode hits on exact pushed code without changing finality, effects, balances, replay, or cleanup |
| Early artifact-drift rejection | RED-GREEN; late 1m58s Run-Y failure path moved before builds/nodes | Outer preflight pins canonical regular non-symlink F7 guest `bc2ea18e...67fd7` and deployer `a7f1e259...191c`. Bootstrap still independently pins the guest and requires the deployer hash to remain exact at entry, point of use, and evidence publication. This is avoided failure latency, not a claimed successful-run speedup |
| Runnable manual BTC flows | Happy and two-lock timeout/refund paths actual-node evidenced | The guide gives a fresh-ID `M3_ACTOR_POC_JOURNEY=refund` command, both direction orders, exact terminal/effect/replay/cleanup assertions, Core 31.1 and LEZ v0.2 local inventory, the observed deadline runtime, flakiness, and Run H's explicit absent-maker/survivor/race/concurrency/process-kill/reorg nonclaims |
| Public BTC lifecycle SDK boundary | 15 unit, 32 external facade, and 2 doctests GREEN; combined SDK all-target/all-feature total 75 | Pushed `0c78f3d` supplies canonical bounded secret-free records, full-range decimal `u128`, exact create/CAS port, role-fixed resume, typed Bitcoin/LEZ runtime, both claim and ordered-refund directions, restart every transition, zero-write replay, substitution rejection, and a dedicated wiring example. Production supplies the durable store and persist-before-send port journals |
| Official and swap-specific cryptographic vectors | 9 of 9 focused groups GREEN; immutable corpus checksum gate GREEN | All 19 official BIP-340 rows cross-check `musig2` and `k256`, with applicable rows also checked by rust-bitcoin. Applicable stateful BIP-327 key/nonce/sign/verify/tweak/aggregate operations execute valid and error vectors. The exact swap fixture adapts, extracts, independently verifies, and rejects context/key/order/tweak/point/signature/secret substitutions. The unused newer deterministic-signing extension is checksum/structure validated but is not a production SDK path |
| Testnet4 configuration portability | 5 of 5 focused tests and 37 adapter tests GREEN; live public calls 0 | Exact Core 31.1, `chain=testnet4`, three-way genesis, synchronized indexes, literal-loopback self-host, and exact allowlisted HTTPS Basic composition pass. The setup guide covers release verification, wallet/funding, readiness, role credentials, SDK composition, external dependencies, and flakiness. Live peers/gateway/faucet/funds remain deliberately unclaimed |
| D1 BTC demo videos | 3 of 3 source recordings and 3 of 3 required MP4s GREEN | Happy, refund, and concurrent owner-private actual-node recordings bind evidence commit `a6eb1ad`; source verifier commit `946208a` sealed mode-`0600` source bundle `3d7d7adc...a86c7cc`. Renderer/verifier commit `846ba56` produced 21.640/20.360/20.360-second H.264 1280x720 MP4s; regenerated-source, complete-decode, frame-sampling, and private bundle verification passed at SHA-256 `7697a27c...f101ba8`. No public RPC, faucet, public funds, or external-network success dependency participated |
| M3 local closure gates | GREEN on 2026-07-19 | Pinned quality runner, Rust format/strict Clippy/all-target tests/warning-free docs, focused vectors/Testnet4, Node audit/license, repository and chain-container isolation, traceability, and action/CI policy passed. All 11 cargo-deny graphs passed advisories/bans/licenses/sources; all 150 Mermaid diagrams passed conservative GitHub parsing and exact rendering. Remote Trivy and actual-node lanes remain push-CI evidence, not local substitutions |
| Concurrent actual-node startup | Certified actual-node saving 31 s; full AD runtime 16m06.52s | Run AA's pre-change service logs measured 39 seconds Core, 58 seconds LEZ, and a 98-second sequential window. AB exposed the unparsed actor handoff; AC crossed it but exposed false cleanup-boolean status. Both failed closed and count as no benchmark. Run AD on clean pushed `0826dd5` completed both directions and cleanup with exit zero. Core took 38 seconds and LEZ 67 seconds in one 67-second overlapped window, certifying a 31-second startup saving. AD was 127.09 seconds faster end-to-end than AA, but only 31 seconds is attributed to startup until structured phase timings explain the remaining variance |
| Structured M3 phase evidence | Clean pushed Run AF: 16m40.17s outer with 510 ms unattributed; 16m47.57s through exact cleanup | ADR 0049 fixes monotonic outer and child phases, secret-safe exact schemas, effect binding, parent containment, and five-file pre/post-publication rehash. `m3f7compose20260718af` at `0b54ab6` passed both real custom-token directions and cleanup. Forward/reverse children were 346.06/386.06 s inside 346.28/386.31 s parents. Forward LEZ second lock/revealing claim were 243.62/99.48 s; reverse Bitcoin second lock, LEZ first lock, and LEZ follow-up claim were 141.01/126.64/116.11 s. Every other child phase was below one second. Exact effects, balances, conservation, zero replay/custody, and no foreign cleanup remain GREEN. An unrelated host workload makes the 22-second Run-AE wall-time difference non-certifiable as a speedup |
| Gateway proposal acceptance errata | 1 nonblocking upstream production/review item | GW-M3-001 records the nonexistent DLC Schnorr adaptor-vector path and the proposed replacement evidence contract. It does not block local milestone certification under the owner policy, but remains visible for Logos/Gateway review and production readiness |
| QA / chaos / information security / production phases | Not active | Each phase begins only after its owner transition; continuous CI/security baselines remain enforced |
| M3 completion tag | `m3-complete` | Annotated tag object `3768c2d...9e51fcf2ed1` peels remotely to exact closure commit `f7fb250f...dcbb2`. Its evidence statement records source/video bundle hashes, local gates, private-local scope, deferred hardening, and the unavailable Actions API without claiming remote green |

## M2 current scorecard

### PoC

| Metric | Current measurement | Evidence or next measurement point |
|---|---|---|
| Full corridor reproductions | 2 successful directions: `m2poc-corridor-fresh-20260714o` and `m2poc-corridor-reverse-fresh-20260714c` | Run 14o completed `TakerSellsLez`; reverse run 14c completed `TakerSellsForeign`. The checked-in secret-safe evidence packets retain exact transactions, blocks, actors, and limitations |
| Completed happy E2E executions | 3 per direction, 6 total | The foundational, schema-v3 recertification, and canonical deployed-artifact runs each created fresh effects in both directions. The requested post-certification repeatability target adds two clean canonical executions per direction after the M3 PoC boundary, reaching 5 per direction without rewriting the historical `m2-complete` claim |
| Current-schema exact-tree replay | 2 of 2 actual-node directions GREEN: `m2cert-schema3-forward-2d09997-20260714a` and `m2cert-schema3-reverse-2d09997-20260714a` | Schema-v3 typed local routes crossed the retained pinned LEZ v0.2 and Zebra Regtest nodes. Forward completed in 46 rounds with 0 retries; reverse completed in 33 rounds with 2 bounded retries. Both actors reached `completed`, atomic order was observed, and no public RPC/faucet was used |
| Canonical Docker artifact and corridor replay | 2 of 2 actual-node directions GREEN after exact local deployment | Direct Docker and Docker-backed methods builds agree on ELF `c85055f6...c9d2e` and ImageID/ProgramId `5cf8c5a4...329c1`. Deployment transaction `bd16808e...733f` finalized in local LEZ block 2582. Canonical forward/reverse runs completed in 38/47 rounds with 2/0 bounded retries and no public resources; see `m2-canonical-local-certification-20260714.json` |
| Clean-host reproductions | 0 | Both successes used fresh run-owned actor state and isolated retained devnets on a host with verified caches. A cold clean-host repeat remains not measured and is not inferred from the two successful directions |
| Setup duration | Run 14o entered effects after 400 ms of provisioning; reverse 14c entered effects after 300 ms | Prebuild happens before the protocol clock. Earlier partial baselines were 6 seconds in 14d, 17 seconds in 14e, and 5590 ms in 14f |
| Happy-path execution duration | 25.370 seconds for 14o; 26.960 seconds for reverse 14c, each measured from provisioning through both terminal actor states | The cap is 49 seconds, preserving a true minimum 10-second margin against the 60-second LEZ delay despite whole-second deadline truncation |
| Required local chain environments | 2: pinned LEZ v0.2 and pinned Zebra Regtest | Both successful directions crossed the same retained endpoint tuple serially; the runner now holds an endpoint-tuple advisory lock so effect-bearing corridor runs cannot overlap |
| LEZ processes in the target environment | 3: Bedrock, non-standalone sequencer, indexer | All three remained live while Vault onboarding, checked deployment, native initialize/fund/claim, same-tip state reads, and manual indexer finality completed |
| Effect-bearing swap actors | 2 independent reference actors and 2 role bridges completed each direction | Run 14o recorded 78 actor events across 39 rounds; reverse 14c recorded 100 events across 50 rounds. Maker and taker independently reached revision 4 `Completed` in both runs |
| Exact v0.2 PoC role bridge | 1 executable; both role processes completed the direction-correct method sequence in both runs | Run 14o used taker LEZ deposit then maker reveal; reverse 14c used maker LEZ deposit then taker reveal. Both crossed initialize, fund, bounded observe, revealing claim, and exact submit |
| Same-run retry evidence | Retained schema-v2 runs: 1 successful retry in 14o and 0 in reverse 14c; current-schema runs: 0 in forward and 2 in reverse; configured ceiling is 8 exact same-run retries within the unchanged absolute deadline | Taker round 2 in 14o retried `lez_bridge.v1.observe_escrow` once after payload-free `moving_tip`, then completed. Reverse 14c completed without a same-run retry. Current-schema forward completed without retries; current-schema reverse completed after two bounded same-run retries |
| Supported happy directions | 2 of 2 composed | `TakerSellsLez` and `TakerSellsForeign` are GREEN; `m2-complete` binds this PoC boundary without entering a later phase |
| Actual maker/taker Vault Claims | 2 of 2 finalized on the retained local LEZ run | Maker block 29 and taker block 30 are exact finalized indexer evidence; this onboards the LEZ actors but is not a swap corridor |
| Checked LEZ escrow lifecycles | 2 canonical plus 2 retained historical composed initialize/fund/claim lifecycles, and the earlier local-only slice | Canonical forward effects finalized in blocks 2594/2595/2596; canonical reverse effects finalized in 2605/2606/2607, all under ProgramId `5cf8...29c1`. Both ended `Claimed` with custody 0. Blocks 264/265/266 and 641/642/643 remain immutable pre-canonical behavior evidence |
| Zcash/reference-actor fixture readiness | 2 successful just-in-time pairs were provisioned and consumed; 0 retained actor pairs are advertised as reusable | Stable Zebra identity/output checks ran before each corridor. Every repetition must select fresh current inputs and a fresh LEZ window; saved or failed-run files and candidates are never reused |
| Actual Zcash HTLC lifecycle | 2 canonical terminal composed funding and claim lifecycles plus retained historical evidence | Canonical forward funding `0d041be6...b64c:0` at height 122 was spent by `8555c3d7...77d7` at 124. Canonical reverse funding `1cbb5923...4785:0` at 125 was spent by `bfbd4379...9b2a` at 127. Both had a second confirmation before LEZ reveal; the older height 106/108 and 113/115 runs remain historical |
| Final state and balance proof | 2 canonical cross-chain terminal proofs plus retained historical and LEZ-only proofs | Canonical forward block 2596 ended custody/depositor/claimant at 0/100000/50000 from 0/150000/0; canonical reverse block 2607 ended 0/0/150000 from 0/50000/100000. Each conserves 150000 LEZ and both pairs of actor stores are revision 4 `Completed` |
| Public RPCs, faucets, or public funds used | 0 | Both successes used only isolated local LEZ and Zebra endpoints and deterministic local Vault/Regtest funds; cold artifact provisioning remains an external availability dependency |
| Dormant public route contract | 5 composed boundaries, 0 public calls | Signed public LEZ agreement activation, actor schema-v3 routes, Zebra HTTPS/API-key transport, the sidecar's exact official-public outbound profile, and the authenticated deployment-evidence-to-runtime-identity handoff pass local executable contract tests. The actor-facing sidecar listener stays loopback-only. Provisioning uses domain-separated HMAC-SHA256 and covers one happy case, no-clobber output, eight authenticated evidence mutations, wrong-key plus unauthenticated semantic/envelope chain-fact tampering, bounded/non-regular input, and exact owner-only key-file validation. Live LEZ finalized-tip availability and provider rate limits remain unmeasured |
| Cleanup and retained state | Bridge processes are exact-PID/start-time/executable scoped; endpoint tuples are serialized; failure roots are retained; chain funds are not rolled back | Successful runs stopped only their role bridges. Failed 14j and reverse attempts 14a/14b retain effects in distinct nonretryable swaps; never reuse their actor files, swaps, candidates, or funds |
| PoC defect evidence | 1 directionality defect reproduced in 2 effect-bearing reverse attempts, then corrected | Reverse attempts 14a/14b exposed a forward-only canonical LEZ validator. The correction binds validation to the agreement-derived LEZ depositor; its focused regression and all 35 SDK lifecycle tests passed before reverse 14c |
| Manual reproduction path | One direction-aware runner and expected evidence for both directions are documented | Requires already-running explicit fresh local nodes, a unique run ID/output root per attempt, and serialized runs. The retained evidence endpoints and run IDs are examples, never defaults |
| Exact LEZ v0.2 closure verifier | GREEN | Root compatibility, escrow and local-stack tests; strict Clippy and rustdoc; canonical Docker guest artifact/ProgramId equality; recursive native/refund/rollback/two-definition suites; deployer tests; and dependency source/feature checks all passed |
| Fresh Zebra closure E2E | 2 of 2 GREEN | Isolated `m2cert-final-bc31373-zebra-20260714b` passed restart/requery/actual-fork removal and real actor-key fund/claim/refund through Zebra consensus; the schema-v10 expectation fix followed a RED-to-GREEN defect audit |
| Supply-chain and image vulnerability closure | GREEN | All 11 Rust dependency graphs pass advisories/bans/licenses/sources, npm audit reports zero vulnerabilities, and fail-hard Trivy 0.70.0 with a fresh database reports zero HIGH/CRITICAL findings in the exact Zebra image |
| Architecture and repository-policy closure | GREEN | All 95 tracked Mermaid diagrams render with the repository harness; traceability, CI hardening, formatting, strict Clippy, tests, and docs gates pass. Remote-hosted CI status is not inferred from this checked-in local evidence |

The local-functional PoC boundary is certified under `m2-complete`; the owner
has not entered QA or M3. Run 14o and reverse 14c live-prove the no-round-cap loop,
0.10-second polling, fail-closed millisecond clock, KILL-bounded calls,
maximum-eight exact same-run retry policy, direction-derived effect owners,
two-confirmation Zcash reveal gate, exact claim/follow-up order, and terminal
LEZ indexer/account evidence. The runner prebuilds, provisions at a fresh tip,
starts run-owned bridge ports, mines only after a reported Zcash effect, locks
the exact shared endpoint tuple against concurrent corridor use, and fails on
deadline/headroom. Forward failures 14i and 14k through 14n made no effect;
14j and reverse 14a/14b retain effects in distinct nonretryable swaps. Cross-
chain atomicity remains protocol ordering and recoverability rather than one
database transaction. The configuration-portability contract is locally GREEN without public I/O,
and the exact local repository closure gates are GREEN. Recovery/refund,
restart, reorg,
ambiguity, concurrency, and broader hardening wait for owner transition unless
needed to protect correctness. Logos-owned production issues remain nonblocking
for this local phase and stay in the upstream register.

### QA

Status: awaiting owner transition. Extensive unit, property, persistence,
adapter, role-boundary, and real-node regression evidence already exists and is
carried forward. It is not a claim that the M2 QA phase is complete.

| Metric | Current classification | QA-phase measurement |
|---|---|---|
| Requirement/invariant coverage | Carried evidence exists; composed matrix not measured | Map every M2 happy and negative behavior to executable actor-level evidence |
| RED-GREEN-REFACTOR cases | Historical cases exist; no count assigned to the new phase | Count new failing cases, fixes, and refactors from QA entry |
| Restart, boundary, reorg, refund, concurrency cases | Proven in multiple lower lanes; 0 composed phase cases | Revalidate around the completed PoC using real roles and required nodes |
| Pass/fail/ignored totals | Not baselined for the phase | Record exact commands, totals, and justified ignores on phase entry and exit |
| Flake rate | Not measured | Repeated isolated runs must report attempts, intermittent failures, and causes |
| Open QA defects | Not baselined | Maintain severity, owner, reproduction, and disposition |

### Chaos

Status: awaiting owner transition. Zebra fork/reorg, restart, ambiguous effect,
and store recovery tests are carried evidence, not a composed chaos campaign.

| Metric | Current measurement | Chaos-phase target |
|---|---|---|
| Composed fault cases injected | 0 | Catalogue process, RPC, node, network, reorg, storage, and timing faults |
| Successful recoveries | 0 composed | Record result and observed recovery time per fault |
| Duplicate external effects | Not measured in a composed run | 0 unexplained duplicates |
| Lost funds or state corruption | Not measured in a composed run | 0 |
| Run-owned resource leaks | Not measured in a composed run | 0 after exact cleanup |

### Information security

Status: awaiting owner transition. Security is still a continuous baseline:
formatting, strict linters, tests/docs, RustSec, dependency bans/licenses/sources,
ShellCheck, traceability, Mermaid policy, and pinned image scanning remain CI
requirements. Prior green results are carried evidence and must be freshly
recorded when this phase is active.

| Metric | Current measurement | Information-security phase target |
|---|---|---|
| Repository-controlled critical/high vulnerabilities | All eleven independently checked-in Rust lockfiles resolve non-yanked `spin 0.9.9`; exact advisory/bans/licenses/sources audits are GREEN for the root and all ten nested graphs with no `spin` exception. Final-image scanning remains in the exact-commit certification pass. | 0 unresolved |
| Logos-owned advisory exceptions | Present and enumerated in the upstream production-blocker register | Exact, narrow, reviewed, and non-expanding for local evidence |
| Threat-model findings | Not rebaselined for the composed corridor | Count by severity with disposition and regression evidence |
| Secret exposure findings | No composed-run measurement | 0; logs, evidence, configs, stores, and process arguments included |
| License/source-policy violations | Prior gates carried; fresh phase result pending | 0 undisclosed violations or license bombs |
| Lint/static-analysis/image gates | CI gates exist; fresh phase result pending | All required jobs GREEN on the exact evidence commit |

### Production readiness

Status: awaiting owner transition.

| Metric | Current measurement | Production-readiness target |
|---|---|---|
| Public configuration portability | Locally contract-proven, including authenticated offline evidence provisioning and no-clobber exact identity output; not live-exercised | Same actor binaries, SDK, builders, and validators; route changes through signed configuration, credentials, funding, and verified LEZ deployment provisioning only |
| Public deployment/execution | Intentionally absent under ADR 0023 | Remains explicit until owner authorizes evidence or scope changes |
| Latency/resource envelope | Not measured for composed corridor | Report setup/runtime latency, CPU, memory, storage, chain compute/fees, and concurrency envelope |
| Availability/recovery objectives | Not defined | Define and verify operator-facing objectives |
| Observability and alert coverage | Partial lower-layer diagnostics; not measured | Every external effect, wait, recovery, and terminal failure is diagnosable without exposing secrets |
| Operator runbooks | Partial manual component flows | Clean setup, normal operation, recovery, upgrade, backup/restore, and teardown paths complete |
| Release artifacts and provenance | Partial pinned inputs | Reproducible package, SBOM/provenance, signatures, scans, and release notes complete |
| Upstream production blockers | Living register exists | Every Logos-owned dependency risk has owner/status/impact/workaround and release disposition |

## Update rules

1. Record the exact commit, command, run ID, cache/network assumptions, and
   result behind every numeric improvement.
2. Never count a lower-lane unit or contract test as a composed PoC run.
3. Never count carried evidence as phase completion until it is revalidated
   against the working vertical path.
4. Record failed attempts and flakes as well as successes.
5. Update the implementation plan, manual guide, architecture diagrams, and
   external-resource inventory in the same change when their facts change.

## M4 progressive local PoC certification (2026-07-22)

Exact clean replay `m4cert20260722an` on commit `5ec6521` passed the native XMR
claim path and bounded cleanup. The run exercised both actor roles, LEZ v0.2
deployment/readiness, official Monero 0.18.5.1 Regtest, finalized tag 13/14/15,
adaptor extraction, Maker-destination post-fee receipt verification, and the
canonical `lez_v02_m4_claim_cross_chain_binding_v1` evidence packet. Its ledger
records source exit 0, evidence completed, cleanup passed, exact run resources
absent, sidecar ports closed, and foreign-sentinel survival; it used no public
RPC, faucet, peer, or public funds. This is the progressive local-functional PoC
checkpoint, not a production-readiness claim; refund/punishment, F7 parity,
U9/D1, independent review, chaos, and QA/security hardening remain deferred.


## M5 PoC closure-candidate scorecard (2026-08-02)

This current scorecard supersedes the earlier progressive snapshots above.
Status is **verified local-functional PoC 7/7**, bound by `m5-poc-complete`; it is
neither a production-readiness nor public-deployment certification.

| Literal issue #112 output | Current measurement | Evidence boundary |
|---|---|---|
| Daemon | PoC complete | Retained system-service, supervision, restart, and daemon-owned ZEC local-chain evidence |
| Maker CLI | PoC complete; all-pair matrix 1/1 in 0.64s | Real CLI/daemon and durable rows; marker actors add no chain effect |
| Taker CLI | PoC complete; receipt-v2 Tag16 refund 1/1 in 106.26s and schema-v2 Tag14 claim 1/1 in 164.85s | Both LEZ terminal routes cover rejected-preflight retry, send once, observe/reconcile, Complete and losing-branch exclusion. Tag14 grants only FDs 220..223. Fixed process observers are not actual-chain proof |
| Coordinator persistence/crash/concurrency | PoC complete; three-pair overlap 1/1 in 16.31s | One daemon/database; XMR failure isolated from BTC/ZEC Terminal; child reap and restart exact/no replay; markers only |
| Price sources | PoC complete | Retained local and Logos-module C-API evidence |
| Delivery/Chat degradation | PoC complete | Retained durable replay and post-lock transport-removal evidence |
| Fuzz | PoC complete | Retained literal target, seven seeds, and 512-run smoke |

Bitcoin manual claim first returned `-32602`, producing useful RED evidence.
The supervisor now maps the user intent Claim to Bitcoin Drive; the focused
mapping unit is GREEN 1/1. This is a semantic command correction, not a new
Bitcoin transaction result.

Evidence composition is explicit: M2/M3/M4 real local-devnet runs and clean M5
BTC/ZEC/XMR accepted-application corridors retain chain-effect evidence. The new
Maker matrix and three-pair overlap certify control-plane composition only.
Semantic receipt-v2 XMR workers and fresh simultaneous accepted-application
actual-chain overlap remain QA/production-hardening measurements after PoC.
