# ADR 0053: Enter M4 through isolated Monero Regtest

Status: Accepted. The official-node Regtest topology and locally mined
wallet-to-wallet funding checkpoint are executable and evidenced. No atomic
LEZ/XMR swap is claimed yet.

## Context

The live RFP and accepted replacement issue #112 require a LEZ/XMR swap based
on the h4sh3d/COMIT spend-key-share construction. The proposal names six M4
outputs: an XMR LEZ claim update, a full LEZ/XMR SDK including partial-loss
recovery, DLEQ conformance evidence, the U9 stagenet node guide, three D1 XMR
videos, and a self-hosted stagenet `monerod` CI lane.

At M4 entry, the repository had only pair vocabulary, a generic LEZ-refund
event gate, SQLite phase replay, and CLI direction validation. The first
checkpoints now add bounded canonical DLEQ proofs for both spend-key shares, a
reproducible official `monerod` and three-wallet topology, symmetric shared-key
reconstruction, and a development spend from the reconstructed key through the
official wallet RPC. It still has no complete adaptor lifecycle, Monero node
adapter, role actor, revealing LEZ claim, signed LEZ refund/punish branches, or
atomic-swap evidence. Synthetic
`ChainProof` strings, arbitrary 32-byte `ClaimEvidence` markers, and the
bootstrap wallet transfer are not M4 swap evidence.

The named `comit-network/cross-curve-dleq` repository is archived, calls itself
a proof of concept, carries no license declaration or license file, and points
to the maintained 0BSD `sigma_fun` implementation. COMIT's full swap repository
is GPL-3.0. Neither codebase may be linked, vendored, or copied into this
MIT/Apache-2.0 delivery. GW-M4-001 records the literal conformance discrepancy.
GW-M4-002 separately records that the accepted text says “Ed25519 adaptor”
without specifying how that maps to LEZ v0.2's actual BIP-340 aggregate witness
and the h4sh3d scriptable-chain adaptor construction.

## Decision

M4 starts with one narrow, reproducible, actual-node happy path in the only
reviewed direction, `TakerSellsLez`. The PoC must create real effects on an
isolated LEZ v0.2 local chain and an offline Monero Regtest chain. It must use
independent one-shot Maker and Taker processes, role-local stores and keys, and
must continue after Delivery and Chat are absent.

The first implementation boundary is:

1. a pair-specific `lez-xmr-swap-sdk` with typed transcript, share, address,
   amount, output, confirmation, reveal, and recovery records;
2. a narrow Monero daemon/wallet RPC adapter using a maintained permissive RPC
   client and canonical Monero types, with the official wallet used for PoC
   transaction construction rather than custom ring-signature code;
3. a public `xmr-reference-actor` executable whose commands are role-fixed and
   persist complete effect material before one submission attempt;
4. the existing bounded LEZ v0.2 bridge, extended with an XMR-specific
   transcript commitment and exact revealing witnessed claim; and
5. a run-owned orchestrator which provisions both chains, invokes fresh actor
   processes, verifies the ordered public effects, emits secret-safe evidence,
   and deletes only its captured resources.

The cryptographic spike must pin the exact scalar width, byte order, point
encoding, subgroup/identity rejection, transcript domain, adaptor equations,
and extraction equation before the first result is called an atomic swap.
`sigma_fun` 0.9.0 is the leading PoC DLEQ candidate because the archived COMIT
repository redirects to it and it is 0BSD. It is not accepted for production
until its exact graph, known-answer compatibility, negative cases, and review
status are closed. The existing `musig2` adaptor machinery may be reused only
through an explicit XMR transcript mapping; no new curve arithmetic is written.

Official Monero CLI 0.18.5.1 supplies `monerod` and `monero-wallet-rpc`. The
runner verifies a retained copy of the canonical clearsigned hash list, its
pinned signer key and fingerprint, the archive SHA-256 and size, source tag
object and commit, exact binary members, and version output, then runs one
offline `monerod --regtest --fixed-difficulty 1` plus separate authenticated
funding, Maker, and Taker wallet RPC processes. Every port is selected
dynamically and published only on loopback. Wallet and chain directories are
run-scoped and distinct. Test funds are mined locally, so the PoC uses no peer,
public RPC, faucet, public funds, or external finality service.

Monero's own v0.18.5.1 functional harness starts ordinary-network wallets
against Regtest with `--allow-mismatched-daemon-version`. The runner uses the
equivalent config option and asserts `fakechain`, not mainnet, testnet, or
stagenet, before creating any wallet. The four loopback endpoints are
transports to real official processes; they do not emulate consensus or wallet
behavior. The daemon executes fakechain consensus rules and LMDB state, the
official wallets scan blocks and construct a real ring transaction, and the
runner requires daemon/wallet tip agreement, exact transaction confirmation,
and unlocked balances.

## Executable topology checkpoint

Run `m4-monero-poc-20260719c` exercised this exact component and RPC topology.
The bridge is non-masquerading so containers have no public egress, while only
authenticated RPC ports are published on kernel-selected literal-loopback
ports. P2P and ZMQ are not published.

```mermaid
flowchart LR
    Operator["Operator or CI"]
    Runner["M4 Monero runner"]
    Verify["Release verifier"]
    Archive["Official archive cache"]
    GitTag["Monero source tag"]

    subgraph Host["Literal loopback RPC boundary"]
        DaemonRpc["Daemon RPC"]
        FundingRpc["Funding wallet RPC"]
        MakerRpc["Maker wallet RPC"]
        TakerRpc["Taker wallet RPC"]
    end

    subgraph Bridge["Run-owned non-masquerading bridge"]
        Monerod["Official monerod 0.18.5.1"]
        FundingWallet["Official funding wallet RPC"]
        MakerWallet["Official Maker wallet RPC"]
        TakerWallet["Official Taker wallet RPC"]
        ChainStore[("Monero fakechain tmpfs")]
        FundingStore[("Funding wallet tmpfs")]
        MakerStore[("Maker wallet tmpfs")]
        TakerStore[("Taker wallet tmpfs")]
    end

    Operator --> Runner
    Runner --> Verify
    Archive --> Verify
    GitTag --> Verify
    Verify --> Runner
    Runner --> DaemonRpc
    Runner --> FundingRpc
    Runner --> MakerRpc
    Runner --> TakerRpc
    DaemonRpc --> Monerod
    FundingRpc --> FundingWallet
    MakerRpc --> MakerWallet
    TakerRpc --> TakerWallet
    FundingWallet --> Monerod
    MakerWallet --> Monerod
    TakerWallet --> Monerod
    Monerod --> ChainStore
    FundingWallet --> FundingStore
    MakerWallet --> MakerStore
    TakerWallet --> TakerStore
```

The bootstrap flow is an infrastructure gate, not the swap flow:

```mermaid
sequenceDiagram
    actor Operator
    participant Runner
    participant Verifier
    participant Daemon as monerod Regtest
    participant Funding as Funding wallet RPC
    participant Maker as Maker wallet RPC
    participant Taker as Taker wallet RPC

    Operator->>Runner: Start with unique run ID
    Runner->>Verifier: Verify signed release and source identity
    Verifier-->>Runner: Verified binaries and provenance
    Runner->>Daemon: Start offline fakechain
    Runner->>Funding: Start with provisioner credential
    Runner->>Maker: Start with Maker-only credential
    Runner->>Taker: Start with Taker-only credential
    Runner->>Taker: Try Maker credential
    Taker-->>Runner: HTTP 401
    Runner->>Funding: Create isolated funding wallet
    Runner->>Maker: Create isolated Maker wallet
    Runner->>Taker: Create isolated Taker wallet
    Runner->>Daemon: Mine 100 blocks to funding wallet
    Runner->>Funding: Transfer 10 XMR to Maker and Taker
    Runner->>Daemon: Mine policy of 10 confirmations
    Runner->>Funding: Refresh and require final height
    Runner->>Maker: Refresh and require unlocked 10 XMR
    Runner->>Taker: Refresh and require unlocked 10 XMR
    Runner->>Runner: Seal evidence and exact cleanup
```

The measured clean run passed at height 111 in 53 seconds before cleanup:
30 seconds release verification, 3 seconds image and topology readiness, and
20 seconds wallet bootstrap and assertions. All four processes used read-only
roots, UID/GID 65532, dropped capabilities, `no-new-privileges`, distinct
tmpfs stores, and distinct credentials. Maker credentials received HTTP 401
from the Taker endpoint. Cleanup removed the exact four containers, four
volumes, network, and image while a foreign sentinel survived.

```mermaid
flowchart LR
    subgraph Host["Run-owned host boundary"]
        Run["M4 orchestrator"]
        Maker["Fresh Maker actor"]
        Taker["Fresh Taker actor"]
        MakerDb[("Maker role store")]
        TakerDb[("Taker role store")]
    end

    subgraph Lez["Isolated LEZ v0.2 project"]
        Bridge["Role LEZ bridge sidecars"]
        Sequencer["Sequencer JSON-RPC"]
        Indexer["Indexer finality RPC"]
        Bedrock["Bedrock settlement"]
    end

    subgraph Xmr["Isolated Monero 0.18.5.1 Regtest project"]
        Daemon["monerod daemon RPC"]
        FundingWallet["Funding wallet RPC"]
        MakerWallet["Maker wallet RPC"]
        TakerWallet["Taker wallet RPC"]
        XmrState[("Run-scoped fakechain and wallets")]
    end

    Run --> Maker
    Run --> Taker
    Maker --> MakerDb
    Taker --> TakerDb
    Maker --> Bridge
    Taker --> Bridge
    Bridge --> Sequencer
    Bridge --> Indexer
    Sequencer --> Bedrock
    Maker --> MakerWallet
    Taker --> TakerWallet
    Run --> FundingWallet
    FundingWallet --> Daemon
    MakerWallet --> Daemon
    TakerWallet --> Daemon
    Daemon --> XmrState
```

The positive-path sequence is deliberately chain-real and user-shaped:

```mermaid
sequenceDiagram
    actor Maker as Maker operator
    actor Taker as Taker user
    participant TakerActor as Fresh Taker actor
    participant Lez as LEZ sequencer and indexer
    participant MakerActor as Fresh Maker actor
    participant Monero as monerod and wallet RPC

    Maker->>MakerActor: Accept signed LEZ-first terms and durable transcript
    Taker->>TakerActor: Accept the same terms and durable transcript
    TakerActor->>Lez: Submit exact taker-owned LEZ lock
    Lez-->>MakerActor: Exact lock is finalized
    Note over MakerActor,TakerActor: Delivery and Chat may now remain offline
    MakerActor->>Monero: Fund exact shared Monero address and amount
    Monero-->>TakerActor: Exact output reaches signed confirmation policy
    MakerActor->>Lez: Submit exact adaptor-completed witnessed claim
    Lez-->>TakerActor: Canonical claim bytes reveal the bound scalar share
    TakerActor->>TakerActor: Verify extraction and both DLEQ public points
    TakerActor->>Monero: Reconstruct spend authority and spend exact output
    Monero-->>TakerActor: Exact spend is canonically confirmed
    TakerActor-->>Taker: Terminal completion and destination balance
    MakerActor-->>Maker: Terminal completion and LEZ balance
```

## Atomicity boundary

For the supported happy path, each role proves that its secp256k1 adaptor point
and Ed25519 Monero public spend-key share represent the same nonzero canonical
scalar. The Maker claim is adapted with Maker share `s_a`; its canonical final
signature lets the Taker extract `s_a`, add retained `s_b`, and spend XMR. The
Taker must keep its claim partial private until the exact Maker-funded Monero
output reaches the signed confirmation policy, otherwise the Maker could claim
LEZ before funding XMR.

If no canonical Maker claim appears, the Taker's timeout refund must be a
distinct signed branch adapted with Taker share `s_b`. Its canonical final
signature lets the Maker extract `s_b`, add retained `s_a`, and recover XMR.
The current generic permissionless refund is unsigned and reveals no share; it
cannot prove this recovery path. A Maker punishment branch is also required if
the Taker disappears through the signed refund window. ADR 0055 specifies these
branches and their sequence. Refund, punishment, partial-loss, survivor,
restart, reorg, and concurrency execution are required for literal M4 closure
but follow the first linked happy path under the progressive delivery policy.

This is not a cross-chain database transaction. The safety claim is conditional
on adaptor extractability, DLEQ soundness, exact transcript binding, durable
secret custody, canonical LEZ and Monero observations, and transaction
inclusion. The local PoC proves concrete effects and ordering, not production
cryptographic review, public stagenet availability, or deep-reorg tolerance.

## Evidence and phase gates

The local happy PoC is GREEN only when one command on a clean pushed commit
proves exact source/artifact identities, independent actors, one LEZ lock, one
Monero funding output, one revealing LEZ claim, one reconstructed Monero spend,
both terminal stores, expected balances, zero LEZ custody, zero replay
resubmissions, no public resources, and exact isolated cleanup. A local wallet
transfer without the proven adaptor/DLEQ link is not an atomic-swap PoC.

After the PoC, QA proceeds RED-GREEN-REFACTOR through vector mutations and
subgroup cases, forbidden-direction zero-wire rejection, exact agreement and
output binding, restart at every transition, post-reveal survivor completion,
refund/event-gated recovery, same-direction concurrency, secret-at-rest and
zeroization checks, node/wallet disagreement, scan lag, reorgs, process kills,
timeouts, fees, and flake/coverage measurement. The literal M4 gate then adds
the U9 self-hosted/public stagenet guide, self-hosted stagenet CI lane, three D1
videos, synchronized traceability/review evidence, all security/license/image
gates, clean push, and annotated `m4-complete` tag.
