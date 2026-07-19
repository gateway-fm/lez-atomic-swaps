# Milestone 3 private-local functional review

Status: closure candidate under correction. Five outputs and the underlying
private actual-node evidence are GREEN; the literal D1 three-video deliverable
is being rendered from that retained evidence. Fresh repository-wide gates,
the exact push, remote CI, and the annotated tag remain pending.

Review date: 2026-07-18

## Authority and claim boundary

The review uses RFP-003 at master commit
`121da225de1930c5ba693ebbef80ee788d55542a`, file blob `d0fa52b`,
and accepted replacement issue #112 with newline-normalized body SHA-256
`49356263a762307abc0f8dd2863ac5af8fe13d9b17b674f242d025de655f1c87`.
Issue #112 was reread on 2026-07-18, remained open with `accepted` and
`RFP-003` labels, and superseded issue #61.

The proposed certification is deliberately narrow: a reproducible,
private-local, functional M3 using actual Bitcoin Core 31.1 Regtest and the
pinned private LEZ v0.2 stack. It does not claim a public Testnet4 or LEZ
deployment, production custody, production readiness, formal cryptographic
review, one cross-chain database transaction, or literal conformance to the
nonexistent DLC Schnorr vector file.

## Accepted issue #112 outputs

| Output | Repository result | Evidence |
| --- | --- | --- |
| Witness-gated BTC escrow | Both native and witnessed custom-token paths use the agreement-bound aggregate BIP-340 authority; actual-node happy, refund, survivor, overlap, and four F7 pairs reach exact terminal state with zero replay sends | [M3 operator guide](m3-local-poc-operator-guide.md), [traceability F2/F7](requirements-traceability.md), [ADR 0042](architecture/0042-bind-witnessed-token-claims-to-exact-atas.md) |
| Full-lifecycle LEZ/BTC SDK | Pushed `0c78f3d` exposes the bounded canonical secret-free codec, exact create/CAS store port, role-fixed stored SDK, typed Bitcoin/LEZ ports, both claim/refund directions, restart/replay checks, and a wiring example | [SDK architecture](architecture/0013-sdk-layering.md), [deployment inventory](architecture/deployment-components-and-rpcs.md) |
| Conformance and swap vectors | Nine focused groups bind official BIP-340/BIP-327 corpora and swap-specific positive/negative adaptor fixtures to immutable checksums and an independent `k256` verifier | [ADR 0050](architecture/0050-map-btc-adaptor-construction-to-security-properties.md), [metrics](milestone-metrics.md) |
| Bitcoin testnet setup | Pushed `946208a` binds exact Core 31.1 Testnet4 chain/genesis/index readiness to literal-loopback self-hosting or one exact allowlisted HTTPS Basic origin without public I/O | [Testnet4 setup](bitcoin-testnet4-setup.md), [ADR 0051](architecture/0051-bind-bitcoin-testnet4-routes-to-chain-profile.md) |
| Three BTC demo videos | Happy, both ordered refunds, and opposite-direction concurrency have hash-bound private actual-node source recordings at evidence commit `a6eb1ad`; the required private MP4 walkthroughs and three-video bundle are pending | [recording and video procedure](m3-local-poc-operator-guide.md#private-d1-btc-recording-bundle), [traceability D1](requirements-traceability.md) |
| Aumayr/Fournier explanation | Pushed `a0f19ac` maps the implemented nonce, adaptor, tweak/parity, extraction, ordering, and recovery conditions to the two constructions without claiming their proofs transfer automatically | [ADR 0050](architecture/0050-map-btc-adaptor-construction-to-security-properties.md) |

## User flows and conditional atomicity

The system and role boundaries are diagrammed in
[system architecture](architecture/system-architecture.md) and the exact
local nodes, RPCs, credentials, and public route shapes are diagrammed in
[deployment components and RPCs](architecture/deployment-components-and-rpcs.md).
The [manual user-flow guide](manual-user-flows.md) gives build, happy, refund,
concurrent, SDK/vector, recording, and Testnet4 procedures.

Both trade directions enforce the same safety order:

1. the Taker persists and submits the first lock;
2. the Maker observes the exact canonical first lock, persists its second-lock
   intent, and submits the second lock once;
3. no adaptor witness is revealed before both locks are canonical;
4. the revealing claim publishes first, then the counterparty extracts and
   point-checks the adaptor scalar before the follow-up claim; and
5. without a canonical reveal, the Maker-funded leg refunds at the earlier
   signed bound and the Taker-funded leg at the later bound.

The two direction-specific claim sequences are in
[system architecture](architecture/system-architecture.md), the two exact
refund sequences are in
[ADR 0044](architecture/0044-presign-btc-recovery-and-project-revealing-leg-first.md),
and the construction map plus precise conditional atomicity argument are in
[ADR 0050](architecture/0050-map-btc-adaptor-construction-to-security-properties.md).
Atomicity is protocol ordering plus recoverability: each chain commits
independently, so no distributed atomic-commit claim is made. Safety depends on
the countersigned agreement, pre-signed recovery, durable persist-before-send
journals, canonical/finalized observation, conservative deadline separation,
the stated cryptographic assumptions, and an honest participant acting before
its signed bound.

The happy path has been executed six times per direction. Four complete F7
custom-token pairs executed both directions. The refund run executed both
ordered two-lock recovery directions, and the D1 concurrent run held both
opposite-direction swaps at revision two before either settlement.

## Private D1 bundle

The retained bundle is intentionally outside Git under `.e2e` because its
sibling run roots contain actor-private state. Its public, secret-safe identity
is:

| Field | Value |
| --- | --- |
| Happy run | `m3record-happy-20260718ag` |
| Refund run | `m3record-refund-20260718ag` |
| Concurrent run | `m3record-concurrent-20260718ag` |
| Evidence commit | `a6eb1ada739f8fcd671feb8fbb41cfc682e5d651` |
| Verifier commit | `946208a887709d9b8422f51f8152a3008c6d745a` |
| Bundle result and mode | `passed`, `0600` |
| Bundle SHA-256 | `3d7d7adc12571a610be21a18b746e68cb17311ea1224191fcdcdf1b39a86c7cc` |
| Public dependencies | none; Core 31.1 Regtest and private LEZ v0.2 only |

## Quality, security, and supply-chain policy

CI requires Rust formatting, strict Clippy, tests, and warning-free rustdoc;
ShellCheck, actionlint, Hadolint, and Compose validation; fail-hard
`cargo-deny` advisory/ban/license/source checks across every checked-in Rust
graph; `npm audit --audit-level=moderate` and the Node license allowlist;
and fail-hard Trivy HIGH/CRITICAL scans for repository runtime bases, Zebra,
and Bitcoin Core. Exact Logos/RISC Zero build inputs that are upstream-owned
and not shipped as runtime images remain visible report-only scans under the
documented owner policy; they are not silently waived or represented as
production-clean.

The tag is not authorized until all three private videos and their sealed
bundle pass, the fresh exact closure gates pass on the resulting tree, that
commit is pushed, and remote CI is verified when observable. Current results:

| Gate | Closure result |
| --- | --- |
| Private D1 MP4s and three-video bundle | pending; the renderer and verifier contract tests are GREEN, but live rendering is deliberately blocked until the implementation is committed on a clean tree |
| Repository diff, traceability, isolation, action-pin, and CI-hardening policy | prior pass carried; fresh exact-tree pass pending after video-pipeline documentation |
| Rust format, strict workspace Clippy, all-target tests, and warning-free docs | prior pass carried; fresh exact-tree pass pending |
| Cryptographic-vector and Testnet4 focused gates | prior pass carried; fresh exact-tree pass pending |
| npm vulnerability and license gates | prior pass carried; fresh exact-tree pass pending |
| Rust advisory, ban, license, and source policy | prior pass carried; fresh exact-tree pass pending |
| Static GitHub Mermaid compatibility and one exact final render pass | prior 148-diagram pass carried; fresh final pass pending after the evidence ADR is added |
| Exact pushed-commit remote CI, or explicit API-unavailable record | pending |

## External resources and deferred work

The retained private runtime certification depends on no public RPC, faucet,
peer, public funds, DNS, CA service, or provider. Final closure tooling may
still need registries, release assets, and current vulnerability databases;
those availability failures stop the gate rather than alter runtime evidence.
Testnet4 self-hosting introduces archive/signature downloads, P2P
synchronization, disk and network availability, organic reorgs, and operator
funding. The HTTPS shape additionally introduces DNS/TLS,
credentials, quotas, method/index policy, provider lag/outage, and ambiguous
broadcasts. A faucet has no SLA and its result is untrusted until confirmed
through the selected node. None can turn the private milestone result green or
red.

The owner-selected QA, chaos, information-security, and production-readiness
phases remain later work: adversarial cutoff/refund races, operating-system
process-kill campaigns, reorg and fee pressure, arbitrary-N and same-direction
nonce scheduling, production custody, live public execution, monitoring, and
formal review. They are not missing private-functional M3 outputs.

Current public-provider discovery uses token-in-path or header-key auth shapes
that the deliberately narrow root-origin Basic adapter does not admit directly.
Self-hosted Core 31.1 Testnet4 remains actionable; a reviewed translating
gateway or later adapter work is required for those provider shapes. This is a
repository-owned later portability item, not a Logos blocker and not part of
the owner-approved private-local milestone boundary.

`GW-M3-001` remains the nonblocking proposal erratum: the accepted DLC
Schnorr vector path does not exist, while the implemented replacement evidence
has not been accepted upstream. The proposed tag therefore cannot claim literal
DLC conformance. Logos-owned production items remain in
[the upstream production blocker register](upstream-production-blockers.md);
under the owner policy they stay visible without blocking this local
certification. Non-Logos findings retain their ordinary fail-hard treatment.

## Tag rule

Create and push annotated tag `m3-complete` only after this packet records
all closure gates GREEN, the worktree is clean, `HEAD` equals
`origin/main`, and the tag message repeats the private-local claim boundary,
`GW-M3-001`, and the upstream production disposition.
