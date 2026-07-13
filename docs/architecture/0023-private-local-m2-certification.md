# ADR 0023: Certify M2 with a private actual-node corridor

Status: Accepted by the repository owner on 2026-07-13

```mermaid
flowchart LR
    Maker["Independent maker actor"]
    Taker["Independent taker actor"]

    subgraph LocalLez["Public-compatible local LEZ v0.2 devnet"]
        Bedrock["Bedrock node<br/>digest and OCI revision source-bound"]
        Indexer["LEZ v0.2 indexer<br/>binary attested; execution RED"]
        Sequencer["Non-standalone v0.2 sequencer RPC<br/>binary attested; execution RED"]
        Escrow["Checked escrow deployment"]
    end

    subgraph LocalZcash["Pinned local Zcash Regtest devnet"]
        Zebra["Primary Zebra"]
        ZebraFork["Temporary fork Zebra"]
    end

    LocalEvidence["Private happy, restart, refund, reorg, and concurrency evidence"]
    M2["M2 local-functional certification"]
    PublicLez["Public LEZ v0.2 deployment"]
    PublicZec["Public Zcash testnet run"]
    Release["Production-readiness backlog"]

    Maker -->|"v0.2 official-wire sidecar"| Sequencer
    Taker -->|"v0.2 official-wire sidecar"| Sequencer
    Maker -->|"Typed Zebra adapter"| Zebra
    Taker -->|"Typed Zebra adapter"| Zebra
    Sequencer -.->|"Zone SDK block publish"| Bedrock
    Indexer -.->|"Poll finalized LEZ channel"| Bedrock
    Sequencer --> Escrow
    ZebraFork -.->|"Explicit reorg relay in fault tests"| Zebra
    Escrow --> LocalEvidence
    Zebra --> LocalEvidence
    LocalEvidence --> M2
    PublicLez -.-> Release
    PublicZec -.-> Release
    M2 -.->|"Does not assert public evidence"| PublicLez
    M2 -.->|"Does not assert public evidence"| PublicZec
```

## Context

Gateway proposal #112 includes public LEZ v0.2 deployment and Zcash-testnet
evidence in its M2 outputs. The repository owner has selected an unpublished
private-evidence delivery mode ("stealth" here is a disclosure policy, not a
shielded or stealth-address transaction claim): finish and certify a fully
functional corridor without publishing a
deployment, transaction identifier, address, externally hosted recording, or
public actor activity.

Local contract doubles are insufficient for that decision. The private
certification must still execute the actual pinned LEZ and Zebra node
implementations, use independent role-fixed maker and taker processes, submit
real locally signed on-chain transactions, and preserve the protocol's durable
atomicity and recovery boundaries.

## Decision

M2 certification will require private actual-node evidence for both supported
ZEC directions: taker-first ordering, both locks before reveal, canonical LEZ
reveal before the exact Zcash spend, restart recovery, ordered refund, reorg
handling, and concurrent isolation. All endpoints, funds, keys, databases,
Compose projects, ports, and retained evidence remain run-local. Manual
instructions must reproduce this corridor without a public RPC or faucet.

The normal corridor contains exactly two isolated local chain environments: one
pinned public-compatible LEZ v0.2 local devnet and one pinned Zcash Regtest
devnet using the same canonical builders and validators as the future public
route; the signed configuration selects the exact network and active consensus
branch for each environment. The LEZ devnet runs the full source-verified
Bedrock node,
indexer, and non-standalone sequencer; the `standalone` feature's mock block
publisher may be fast lower coverage but cannot be the sole certification
proof. The existing LEZ v0.1.2 standalone lane likewise remains useful
lower-level compatibility evidence but cannot certify this portability gate. A
second
Zebra process is a temporary member of the same Zcash test environment only for
fork/reorg cases; it is not a third swap leg or a shared actor. Maker and taker
remain independent operating-system processes with different configs, keys,
funds, stores, journals, sidecars, and restart lifecycles.

ADR 0024 now attests the clean v0.2 source contract, Bedrock OCI source mapping,
correct sequencer-to-Bedrock publication and indexer-to-Bedrock polling flows,
and attested sequencer/indexer output hashes. Independent clean rebuild
reproducibility, container assembly, ordered
Bedrock/indexer/sequencer startup, restart-state preservation, same-ID finalized
block readiness, Vault Claim onboarding, escrow deployment, and actor use remain
RED; this ADR still makes no running-stack claim.

The M2 implementation must produce local and future public routes in the same
actor binaries, SDK state machine, chain-port traits, transaction builders, and
validation adapters. A
public move may change only provisioning and signed configuration: RPC
endpoints/authentication, chain/genesis/channel/branch identities, confirmation
profiles, signer and funding material, and the deployed LEZ escrow program ID.
Deploying that program on the selected public LEZ network is an expected
on-chain provisioning action. A devnet-only protocol branch, fake evidence
adapter, alternate transaction format, or code rebuild selected by environment
does not satisfy M2 portability. This is an open implementation gate today:
public LEZ activation still fails closed, the actor endpoint schema is
loopback-only, and the public Zebra HTTPS/signing route is incomplete. Public
execution evidence is deferred, but the dormant public-capable configuration
and adapters must exist and be locally contract-tested before M2 is tagged.

Public LEZ v0.2 deployment, public Zcash-testnet execution, public transaction
identifiers, and public recordings are deferred to the production-readiness
backlog. They remain visibly incomplete in RFP traceability. The M2 tag and
release notes must say that they certify the owner-approved local-functional
scope and do not assert those public outputs.

Logos-owned public-runtime limitations continue under ADR 0018. This ADR does
not relax repository-controlled correctness, security, dependency, lint,
vulnerability, license, documentation, or actual-node gates.

## Consequences

- Public endpoint, provider, rate-limit, faucet, and public-fund availability
  cannot make the private M2 corridor flaky.
- Regtest and local v0.2 execution prove the pinned consensus/state-transition
  implementations and real signed effects, but not public propagation, fee
  markets, organic reorgs, public service behavior, or provider behavior.
- The evidence packet and annotated tag must retain this deviation so local M2
  completion cannot be mistaken for full public-testnet compliance with
  proposal #112.
- Once the open portability gate is implemented, public activation is a
  configuration plus funding/deployment operation, subject to the signed
  compatibility checks; it is not a later rewrite of the corridor.
- Production readiness still requires an explicit decision to run and disclose
  the deferred public evidence or to obtain an accepted scope amendment.
