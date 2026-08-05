# ADR 0160: Version Maker Tag17 effect authority

- Status: Accepted as an M7 application-composition prerequisite
- Date: 2026-08-05

## Context

ADR 0159 made Tag17 a durable Maker workflow branch. The role-fixed effect
authority still had no executable slot for that step. Reusing a generic LEZ
sender or overloading the Tag15 tool would make the executable identity
ambiguous and could silently widen older application manifests.

Schema 1 is the deployed Maker profile. Schema 2 is the Taker-only Tag14
release profile. Both canonical encodings must keep their current meaning.

## Decision

Introduce effect-authority schema 3 only for Maker. It requires one explicit
`lez_punish` executable with the fixed ABI `lez_xmr_tag17_punish_v1`. The slot
is absent from canonical schema-1 and schema-2 documents. Schema 3 without the
slot, schema 1 with the slot, a Taker schema-3 profile, or ABI drift fails
closed before executable or RPC access.

The authority still pins the executable path and SHA-256 and reopens it only
through the existing secure executable descriptor at use. This decision adds
authority, not invocation: route selection and the sealed worker are separate
RED/GREEN slices.

## Components

```mermaid
flowchart LR
    Manifest["Canonical effect authority"] --> Version{"Schema and role"}
    Version -->|"Schema 1 Maker"| LegacyMaker["Existing five Maker tools"]
    Version -->|"Schema 2 Taker"| Tag14["Existing Tag14 release profile"]
    Version -->|"Schema 3 Maker"| MakerV3["Existing tools plus lez_punish"]
    MakerV3 --> Abi["Exact lez_xmr_tag17_punish_v1 ABI"]
    Abi --> Digest["Pinned executable SHA-256"]
    Digest --> Router["Role-fixed effect router next slice"]
```

## Validation flow

```mermaid
sequenceDiagram
    participant App as Maker application
    participant Loader as Authority loader
    participant Profile as Schema-role validator
    participant Tool as Tag17 tool slot
    App->>Loader: Load canonical owner-private bytes
    Loader->>Loader: Re-encode and compare exact bytes
    Loader->>Profile: Validate schema 3 and Maker role
    Profile->>Tool: Require lez_punish path digest and ABI
    alt Exact schema-3 Maker profile
        Tool-->>App: Typed optional accessor is present
    else Missing crossed legacy or drifted profile
        Tool-->>App: Reject before executable or RPC use
    end
```

## Security and compatibility argument

The new tool cannot appear through an old schema and an old Maker manifest
cannot gain Tag17 authority by parser defaulting. Conversely, schema-1 Maker
and schema-2 Taker retain their prior canonical field sets. The exact ABI and
digest make later route selection deterministic, while at-use secure open
continues to protect against named-path replacement.

Focused tests cover valid schema 3, missing tool, wrong ABI, cross-version
injection, and unchanged schema-1 loading. They are network-free and use no
Docker, RPC, faucet, funds, or external service.
