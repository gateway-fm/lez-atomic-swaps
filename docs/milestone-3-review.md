# Milestone 3 private-local functional review

Status: closure candidate. All six issue-#112 outputs, including the literal
RFP D1 three-video deliverable, and the underlying private actual-node evidence
are GREEN.
The fresh repository-wide local gates are GREEN. The closure-evidence push,
remote CI, and the annotated tag remain pending.

Review date: 2026-07-19

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
| Three BTC demo videos | Happy, both ordered refunds, and opposite-direction concurrency have hash-bound private actual-node source recordings at evidence commit `a6eb1ad`; three decode-verified, sampled private MP4 walkthroughs are sealed at renderer/verifier commit `846ba56` in bundle `7697a27c...f101ba8` | [recording and video procedure](m3-local-poc-operator-guide.md#private-d1-btc-recording-bundle), [traceability D1](requirements-traceability.md) |
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

The derived private video bundle is retained separately under
`.e2e/m3-private-demo-videos-20260719c/`:

| Scenario | MP4 SHA-256 | Duration | Manifest SHA-256 |
| --- | --- | --- | --- |
| Happy | `4924404f03b944f108b02c07c6b0555e83c5fef5d18fd45accbe305aa1dcf6bd` | 21.640 s | `e2b353b7a98a1812aed490bcdad8f051c96209963ec042238d9175d400e6db16` |
| Refund | `e9fd9fa305bd72bed890a9cccf560d5ae31b4c105844e102be0dd1998e07f4b6` | 20.360 s | `9a8c7d0586ff1b3d2d8906a08da31d820928160ad63b5f9ca12ef4468b638af8` |
| Concurrent | `343a705aebc9270175128c63aa91d154ee9e7acc052fa8373bffd28d48da5c9b` | 20.360 s | `a5bc70a70fa6995ebd518658a33abf6cb7b7464401789d1de9b19f23fc1e4e56` |

The mode-`0600` bundle was recorded at `2026-07-19T00:30:49Z`, binds source
commit `a6eb1ad` to renderer/verifier commit `846ba56`, passed regenerated
source verification and complete H.264/1280x720 stream decode, and has SHA-256
`7697a27c80c8f90856d6592051805a8923fe564aa01b0dff4109bd5c5f101ba8`.
Operator frame sampling covered each introduction, both directions, the
refund/concurrency-specific panels, conditional-atomicity panel, and stable
tail. No public RPC, faucet, public funds, or external-network success
dependency participated.

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
| Private D1 MP4s and three-video bundle | GREEN; 3 of 3 mode-`0600` MP4s pass regenerated source verification, complete decode, frame sampling, and sealed bundle verification at `7697a27c...f101ba8` |
| Repository diff, traceability, isolation, action-pin, and CI-hardening policy | GREEN on 2026-07-19: clean diff, traceability, repository/Core/LEZ isolation, pinned Actions, spin remediation, and fail-hard CI security policy passed |
| Rust format, strict workspace Clippy, all-target tests, and warning-free docs | GREEN on 2026-07-19. The root all-target matrix completed with zero failures; its declared Zebra Docker case remains ignored here and separately required by the remote actual-node job |
| Cryptographic-vector and Testnet4 focused gates | GREEN on 2026-07-19: 9 of 9 BIP-340/BIP-327/adaptor groups plus 5 of 5 nonconnecting Testnet4 route/profile cases passed |
| npm vulnerability and license gates | GREEN on 2026-07-19: exact lock installed in the isolated browser cache, `npm audit --audit-level=moderate` found zero vulnerabilities, and the license allowlist passed |
| Rust advisory, ban, license, and source policy | GREEN on 2026-07-19: all 11 `cargo-deny 0.19.9` graphs passed all four checks. Only policy-accepted duplicate/unused-policy and version-scoped upstream SPEL license-file warnings remain |
| Static GitHub Mermaid compatibility and one exact final render pass | GREEN on 2026-07-19: all 150 diagrams passed the conservative GitHub contract and exact Chromium rendering |
| Exact pushed-commit remote CI, or explicit API-unavailable record | pending |

The first Node install attempt found a stale shared Puppeteer directory with no
Chrome executable. It changed no tracked file and was not treated as a code
pass. The exact lock then installed successfully with
`PUPPETEER_CACHE_DIR=/tmp/lez-mermaid-browser`; that isolated cache also
rendered all 150 diagrams. The local closure run does not substitute for the
remote CI's Trivy and actual-node jobs.

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
