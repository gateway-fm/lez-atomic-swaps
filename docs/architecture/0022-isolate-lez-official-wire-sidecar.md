# ADR 0022: Isolate pinned LEZ official-wire code behind a sidecar

Status: Accepted for the M2 actual-node corridor; implementation in progress --
the official native and revealing-claim planners, native escrow observation,
revealing-claim owner/discovery observation, node-RPC core, authenticated bridge
server/client, executable role-isolated sidecars, signed-agreement
first-lock/observation adapter, and Zebra owner/counterparty claim/refund ports
are GREEN. Native refund sidecar/adapter composition, remaining SDK-port/actor
wiring, and the composed proof remain RED -- 2026-07-13

```mermaid
flowchart LR
    subgraph MakerActor["Maker actor process"]
        MakerSDK["Role-fixed SDK"]
        MakerLezAdapter["LEZ bridge adapter"]
        MakerZebraAdapter["Zebra adapter"]
        MakerState[("Maker SQLite")]
    end
    subgraph TakerActor["Taker actor process"]
        TakerSDK["Role-fixed SDK"]
        TakerLezAdapter["LEZ bridge adapter"]
        TakerZebraAdapter["Zebra adapter"]
        TakerState[("Taker SQLite")]
    end
    subgraph Sidecars["Pinned LEZ v0.1.2 sidecar processes"]
        MakerSidecar["Maker capability and signer"]
        TakerSidecar["Taker capability and signer"]
    end
    Zebra["Zebra Regtest JSON-RPC"]
    LezNode["LEZ standalone JSON-RPC"]

    MakerSDK --> MakerState
    MakerSDK --> MakerLezAdapter
    MakerSDK --> MakerZebraAdapter
    TakerSDK --> TakerState
    TakerSDK --> TakerLezAdapter
    TakerSDK --> TakerZebraAdapter
    MakerLezAdapter -->|"Bounded serde protocol and maker capability"| MakerSidecar
    TakerLezAdapter -->|"Bounded serde protocol and taker capability"| TakerSidecar
    MakerSidecar -->|"Official bytes and primitive facts"| LezNode
    TakerSidecar -->|"Official bytes and primitive facts"| LezNode
    MakerZebraAdapter -->|"Typed bounded JSON-RPC"| Zebra
    TakerZebraAdapter -->|"Typed bounded JSON-RPC"| Zebra
    MakerLezAdapter -->|"Primitive facts"| MakerSDK
    TakerLezAdapter -->|"Primitive facts"| TakerSDK
```

## Context

The M2 composed corridor must use the official pinned LEZ transaction/RPC types
and the canonical `librustzcash`/Zebra stack. The existing LEZ standalone actor
suite and actual Zebra suites prove each node independently, but they have not
yet been composed through production chain ports.

An executable dependency-resolution RED proved that the two pinned stacks
cannot inhabit one Cargo graph. The Zcash graph pins
`crypto-common = 0.2.0-rc.1`, while the LEZ v0.1.2 graph reaches
`chacha20 0.10` and `cipher 0.5.1`, which require stable
`crypto-common ^0.2`. Cargo cannot select one package version satisfying both
requirements. Relaxing a consensus dependency pin, patching a cryptographic
crate, or duplicating an official wire format merely to make the integration
compile would weaken the evidenced stacks.

## Decision

Keep the main swap workspace and Zcash adapter in one process. Place official
LEZ transaction construction, serialization, signing, nonce handling, RPC
decoding, and raw snapshot collection in a separately built, exactly pinned
sidecar. Connect them with a small serde-only protocol that contains primitive
requests, exact bytes, identifiers, inclusion facts, account facts, and
structured errors. The protocol makes no consensus or agreement-validity
judgments.

The SDK-facing LEZ adapter converts those primitive facts into SDK snapshot
types and invokes the SDK's agreement-bound validators. The sidecar cannot
declare a lock or claim canonical by assertion. The Zebra adapter similarly
uses typed and bounded RPC DTOs, assembles stable snapshots from bracketing tip
reads, and delegates transaction/output/spend validation to the existing SDK.

The signed agreement names the runtime family exactly. The deterministic
v0.1.2 corridor uses `DeterministicLocalV0_1_2Compatibility` and the official
`/NSSA/v0.2/AccountId/PDA/` domain; it must never emit or be recorded as
`DeterministicLocalV0_2`, whose deployed family uses the incompatible `/LEE/`
domain. The SDK derives all metadata, native-custody, and token accounts from
that signed selector, and a separate compatibility test compares the local
derivation source to the pinned official v0.1.2 types. Public v0.2 activation
remains fail-closed and is tracked as upstream production work.

For the deterministic M2 runner, the sidecar listener is ephemeral loopback,
requires a high-entropy run-scoped capability, rejects a different `RUN_ID`,
and is owned by the one runner that starts it. A production deployment should
prefer an owner-restricted Unix socket. Neither endpoint nor authentication
material is protocol authority; actor signatures, exact transaction identity,
canonical node observations, and the accepted agreement remain authoritative.

LEZ initialize and fund must be prepared and durably recorded before the first
submission. The sidecar obtains one account nonce under an exclusive signer
lease, reserves the required consecutive nonces, signs exact transactions, and
returns their identities and bytes without logging secrets. Restart
reconciliation observes exact identities before any byte-identical rebroadcast.

The sidecar retains its own lockfile, source allowlist, license/advisory policy,
and exact LEZ/SPEL pins. It does not depend on the swap SDK. The main workspace
does not import the LEZ standalone/sequencer/Risc0 server graph.

The first implementation slice now provides the separately locked official
transaction planner. It constructs native initialize/fund instructions with
the official NSSA/SPEL APIs, reserves one checked consecutive nonce pair under
an exclusive mutex, preserves the first randomized BIP340 signatures for exact
retry, and accepts only its cached pair for submission. Construction binds the
complete configured runtime descriptor; decoding checks canonical bytes,
official hash, witness validity, signer, role, and escrow program. The local
official node adapter now implements nonce reads, exact cached-byte submission,
and bounded exact-owner block-window scans through upstream generated RPC
types. It brackets scans with validated tips, recomputes block hashes and links,
checks genesis when covered, and never treats a bounded miss as global absence.
Only the proven stateless invalid-params error is definitive; returned-hash,
server, parse, timeout, and transport ambiguity remain unknown outcomes. The
local server library now authenticates capability, run, and role before parsing;
uses an ephemeral literal-loopback listener; restores exact randomized prepare
results through the official decoder; and persists a submission-in-flight
marker before the node call so a crash cannot cause blind resubmission. The six
implemented executable methods are registered; the shared protocol and client
also define the two native-refund methods whose sidecar handlers remain RED.
Revealing-claim preparation validates the exact
claimant role, runtime, signer, signed terms, preimage, and request-bound funding
transaction before nonce use; restart restores the exact randomized official
bytes, and submission requires cache membership. The pinned guest ABI carries
only swap ID and preimage, so the funding transaction identity is enforced by
the request/cache boundary but cannot be embedded in the on-chain instruction
without an upstream ABI change. Native escrow observation now decodes official
transactions, signatures, instructions, block links, metadata, and custody,
brackets account reads with an identical tip, rejects ambiguity, and reports
absence only after a complete stable discovery window. Exact-owner lookup is
limited to the latest 4,096 canonical blocks because the upstream transaction
lookup has no chain position; an older miss is `UnknownOrPending`, never
absence. The main adapter independently validates the primitive pair against
the signed agreement and conservative local-depth policy. Revealing-claim
observation now decodes the official Risc0 instruction ABI and canonical
message, witness, ordered accounts, transaction bytes, inclusion position,
terminal metadata, and zero custody. The exact claimant path is cache-bound;
the depositor discovers the counterparty claim by signed terms in a caller-owned
window. Ambiguity and moving tips fail closed, incomplete scans remain
`UnknownOrPending`, and only complete stable windows produce absence. Native
and claim reservations coexist across restart while rejecting nonce reuse.

The executable starts one sidecar with private capability/signer files, an
exact runtime descriptor, a 0600 durable idempotency store, and a
`127.0.0.1:0` listener. Its process contract runs maker and taker
concurrently with distinct identities and proves wrong capability, run, and
role rejection plus graceful cleanup. Actor composition and the composed-chain
proof remain pending.

## Atomicity preservation

No implementation can make LEZ and Zcash commit in one shared database or
consensus transaction. This corridor therefore preserves atomic-swap safety by
making every externally visible transition satisfy the same hashlock and a
strictly ordered timeout protocol:

1. Both signatures bind the same secret digest, amounts, actor destinations,
   chain identities, programs, transaction policy, and asymmetric deadlines.
2. The taker submits the first lock. The maker cannot prepare or submit the
   second lock until fresh canonical evidence satisfies the signed identity,
   role, output/account state, depth, and environment-specific finality policy.
3. Neither claim path starts before both locks are durably projected. As the
   final awaited check before releasing the preimage, the LEZ claimant must
   freshly prove that the exact agreement-bound Zcash output is still
   canonical, unspent, sufficiently deep, and claimable before its CLTV. The
   LEZ claim is always the revealing action in both trade directions. The later
   Zcash claimant learns the preimage only from canonical LEZ transaction
   evidence and spends that exact coordinator-pinned outpoint at vout zero.
4. The LEZ refund deadline is earlier than the Zcash refund deadline by the
   signed safety margin. This gives an honest actor time to use a revealed
   preimage on Zcash before the later refund becomes valid. Local timing proves
   protocol ordering; public-testnet latency calibration remains an explicit M2
   evidence gate.
5. Exact prepared bytes and their expected identities are persisted before any
   broadcast. After timeout, crash, or unknown submission outcome, a restarted
   actor observes that identity before any byte-identical rebroadcast. It never
   substitutes a newly signed transaction or advances from RPC success alone.
6. Maker and taker use separate stores, claim keys, sidecar processes,
   capabilities, and signers. A sidecar returns primitive facts only; the SDK
   independently validates them against the dual-signed agreement before an
   atomic local state-and-journal commit.

The implemented claim lifecycle is designed to remain peer-independent after
both locks: it uses protected local state plus chain observations, not a peer
message. The exact follow-up Zcash outpoint is restart-safe. Commit `166d3e5`
also makes a fresh observation of that exact canonical, unspent, sufficiently
deep, pre-CLTV Zcash output the final awaited port call before a retained LEZ
reveal is submitted. Its two-direction restart matrix proves that absent,
spent, unstable, replaced, under-depth, expired, and field-mutated observations
release nothing, while restoration submits the same retained bytes once.
Durable post-lock reorg ingestion and the composed actual-node proof remain M2
gates, so this safety claim is not yet certified for the complete corridor.

This ordering minimizes but cannot eliminate the cross-chain time-of-check to
time-of-use window: Zcash state can change after the final observation while
the LEZ transaction is in flight. Eliminating that interval would require a
native atomic primitive spanning both chains. The implementation therefore
combines the narrowest available check-to-submit interval with conservative
confirmation depth, ordered refunds, continued chain observation, and durable
recovery rather than claiming impossible absolute atomicity.

Refund ordering, exact owner intents, observe-before-rebroadcast, and
owner/observer transition contracts are implemented in the SDK. The Zebra
refund port now revalidates canonical funding, maturity, exact signed policy,
prepared bytes, and counterparty spends with conservative unknown outcomes.
The schema-v10 SQLite refund journal is implemented with atomic owner/observer
replay. The production LEZ refund port remains required before peer-independent
timeout recovery is an M2 implementation claim. Neither path
can guarantee liveness during indefinite node outage, censorship, or a
reorganization deeper than the signed policy.
The deterministic v0.1.2 lane also has only depth-qualified `Pending` blocks;
that weaker upstream finality model is isolated to its explicitly named local
compatibility environment and is not production-v0.2 evidence.

## Rejected alternatives

- Relax or patch either cryptographic dependency pin: this invalidates the
  already evidenced LEZ or Zcash stack and introduces an unaudited combination.
- Copy LEZ wire structures into the main runtime: this can silently drift from
  upstream signing and hashing semantics.
- Treat the current in-memory claim ports as node evidence: they prove SDK and
  recovery ordering, not actual consensus execution.
- Let the sidecar return already trusted SDK evidence: this moves agreement and
  consensus policy outside the SDK and makes adapter assertions authoritative.

## Consequences and verification

The process boundary adds one local component, lifecycle, capability, and
failure mode to the actor corridor. Transport loss, malformed or oversized
responses, wrong run identity, unstable tips, unavailable signers, unknown
submission outcomes, exact-hash mismatches, and node rejections must remain
distinct errors and be exercised across restart.

The implemented main-workspace bridge client exercises all eight typed protocol
methods, accepts only literal loopback HTTP, sends a sensitive capability plus
exact run and role headers, validates the echoed run/role/runtime context, and
permits each request ID once per client instance. It makes one attempt with no
redirect, proxy, or automatic retry. The sidecar currently serves six of those
methods; native-refund handlers are the next TDD slice. The first main-process
adapter accepts a
caller-owned durable request ID, verifies the signed compatibility environment,
channel, genesis, escrow program, role, and signer, and converts one official
native prepare response into the exact two-step SDK first-lock plan. Its
observation path accepts caller-owned exact IDs or a bounded discovery window,
validates full initialization/funding facts, and invokes the SDK canonical
agreement validator without importing official wire types. Durable
replay protection and idempotent responses across client or sidecar restart are
server-owned. The implemented Zebra claim/refund ports independently derive
signed terms, delegate only transaction signing, validate exact retained V5
bytes and durable identity, sample stable chain facts, observe before
byte-identical rebroadcast, and treat every post-send identity or chain drift as
an unknown outcome. Bounded canonical block/mempool counterparty discovery
returns `Unstable` for unresolved or exhausted searches rather than false
absence.

The executable sidecar process contract is GREEN, but the composed corridor
test still requires
distinct loopback LEZ-sidecar and Zebra endpoints, distinct role funding,
separate maker/taker databases and claim keys, both signed directions, the
fixed `locks -> LEZ reveal -> Zcash follow-up` order, and restart after every
effect. It remains RED until the remaining claim/refund/funding adapters and
reference actors satisfy that contract. No broken commit is published while
this slice is being driven GREEN.

The official claim observation boundary is now source-correct and independently
decoded, but the main revealing-claim adapter, the native-refund sidecar and
adapter, and composed post-lock actor evidence remain repository-controlled
prerequisites. The Zcash taker-first observation path must still receive its
previous canonical head so removal/replacement can be assembled after process
restart; the maker-lock SDK boundary needs a later durable post-lock history
extension rather than an invented adapter assertion.
