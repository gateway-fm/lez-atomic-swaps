# ADR 0126: Separate XMR workflow authority from adaptor and sidecar journals

Status: Accepted for the durable journal, schema-v3 authority, receipt-v2
locked-monitor boundary, typed effect-plan view, sealed-executable primitive,
workflow-v2 catalog, dual-lock command boundary, and schema-v3 effect-input
custody on 2026-08-02; lifecycle-route effect execution and actual-node application
replay remain in progress

## Context

The accepted XMR application boundary in ADR 0114 intentionally stops before
chain effects. Its schema-v2 actor manifest and receipt-v1 Taker handoff prove
activation and allow offline monitoring, but they must never gain claim or
refund authority by reinterpretation.

M5 still needs role-correct Maker and Taker lifecycle commands. Those commands
must survive process death without invoking a possibly completed effect twice,
and selecting the successful branch must permanently exclude the refund branch
and vice versa. The existing adaptor journal is an immutable Stage-A/Stage-B
transcript authority. The tag-15 and tag-16 sidecar journals already own their
respective one-attempt LEZ sends. Reusing either as an application workflow
journal would conflate different authorities and make restart reasoning
ambiguous.

## Decision

Introduce a separate owner-private schema-v2 SQLite workflow journal. Schema
v1 is rejected rather than migrated or reinterpreted. The schema-v3 actor
manifest now binds its normalized absolute path and the SHA-256 of a separate
immutable effect-authority manifest. Schema-v2 manifests and receipt-v1
handoffs remain monitor-only.

The journal binds one singleton identity: swap ID, local role, run ID, agreement
commitment, activation commitment, and effect-authority digest. It selects
exactly one claim or refund branch by an immediate SQLite compare-and-set.
Role-fixed steps are not free-form strings. Schema v2 contains the complete
eight-effect catalog:

| Protocol order | Step | Fixed role | Scope |
|---|---|---|---|
| 1 | Initialize LEZ tag 13 | Taker | Common |
| 2 | Fund LEZ tag 13 | Taker | Common |
| 3 | Fund Monero shared output | Maker | Common |
| 4 | Authorize LEZ tag 14 | Taker | Claim |
| 5 | Claim LEZ tag 15 | Maker | Claim |
| 6 | Sweep Monero claim | Taker | Claim |
| 7 | Refund LEZ tag 16 | Taker | Refund |
| 8 | Sweep Monero refund | Maker | Refund |

Preparation is role-local. Taker LEZ funding requires succeeded tag-13
initialization; tag-14 authorization and tag-16 refund require succeeded LEZ
funding. Maker tag-15 claim and Monero refund sweep require succeeded Monero
funding; the Taker Monero claim sweep requires succeeded tag-14 authorization.
The first Taker initialization and Maker Monero-funding steps have no local
predecessor. Before the irreversible branch CAS, every Common step for that
local role must already have a durable Prepared-or-later row. Thus a branch
cannot be selected while its role-local common plan is incomplete. These
predecessors do not prove cross-role or global protocol ordering. The future
route must bind each local transition to finalized LEZ evidence or confirmed
Monero wallet evidence before treating a counterparty effect as satisfied.

```mermaid
flowchart LR
    CLI["Maker or Taker lifecycle command"] --> R2["Receipt v2 selector"]
    R2 --> V3["Schema v3 actor manifest"]
    R2 --> W
    R2 --> A
    V3 --> W["Owner private XMR workflow journal"]
    V3 --> A["Immutable XMR effect authority v1"]
    W --> CI["Taker initialize tag 13"]
    CI --> CF["Taker fund tag 13"]
    W --> CM["Maker fund Monero"]
    CF --> TCAS{"Taker role-local branch CAS"}
    CM --> MCAS{"Maker role-local branch CAS"}
    TCAS --> CA["Taker authorize tag 14"]
    TCAS --> CR["Taker refund tag 16"]
    TCAS --> CS["Taker sweep Monero claim"]
    MCAS --> CC["Maker claim tag 15"]
    MCAS --> RS["Maker sweep Monero refund"]
    CA --> CS
    CM --> CC
    CM --> RS
    CF --> LR["LEZ sidecar loopback RPC"]
    CA --> LR
    CC --> LR
    CR --> LR
    CM --> OR["Official Monero loopback RPCs"]
    CS --> OR
    RS --> OR
    A --> LR
    A --> OR
    V2["Legacy schema v2 and receipt v1"] --> M["Monitor only"]
```

The validated authority is now exposed as a typed execution plan rather than
raw JSON. Its LEZ authority contains one literal-loopback sidecar root, an
absolute runtime-identity path plus pinned SHA-256, and an absolute capability
file path. Its Monero authority contains four distinct typed endpoint roles:
daemon, Maker funding wallet, neutral shared wallet, and local-role destination
wallet. Every endpoint is an HTTP literal-loopback root with an explicit
nonzero port and separate absolute username/password file paths. Monero
authority additionally requires one normalized absolute shared-wallet
file-password path, distinct from all eight RPC credential paths. URL userinfo,
queries, fragments, non-root paths, DNS names, and non-loopback addresses fail
closed. This checkpoint validates endpoint and credential-path structure; it
does not open a socket or read, snapshot, or authenticate a credential file.

The validated Maker execution authority now also retains the canonical
published Stage-A and Stage-B public paths and the SHA-256 of each exact wire
that the semantic loader already validated. This prevents the later route from
losing or re-deriving the public agreement/activation identity during the
application-to-effect handoff; it does not itself open an RPC or execute an
effect.

Each role has exactly five tool slots. Every slot carries one normalized
absolute program path, a nonzero lowercase SHA-256, and the fixed ABI below;
Maker and Taker profiles cannot cross or coexist in one authority.

| Role | Slot | Fixed ABI |
|---|---|---|
| Maker | Monero fund | `lez_xmr_monero_fund_v2` |
| Maker | LEZ claim | `lez_xmr_tag15_claim_v1` |
| Maker | finalized classifier | `lez_xmr_finalized_classifier_v1` |
| Maker | Monero refund sweep | `lez_xmr_monero_refund_sweep_v3` |
| Maker | Monero verify | `lez_xmr_monero_verify_v2` |
| Taker | tag-14 authorize | `lez_xmr_tag14_authorize_v1` |
| Taker | finalized classifier | `lez_xmr_finalized_classifier_v1` |
| Taker | Monero claim sweep | `lez_xmr_monero_claim_sweep_v2` |
| Taker | Monero verify | `lez_xmr_monero_verify_v2` |
| Taker | tag-16 refund | `lez_xmr_tag16_refund_v1` |

`XmrEffectToolV1::verify_program_at_use` supplies the first executable
TOCTOU-resistant primitive. It securely opens with no symlink traversal,
requires a canonical trusted non-writable parent and a single-link regular
root-or-euid-owned executable, bounds the bytes to 512 MiB, revalidates the
opened/named identity around the read, and checks the pinned SHA-256. It then
copies the verified bytes into a mode-0700 anonymous memfd carrying write,
grow, shrink, and seal seals. `PinnedExecutable::into_command` executes only
that snapshot through child FD 197; it does not reopen the named program.

```mermaid
sequenceDiagram
    participant R as Future lifecycle route
    participant T as Typed tool slot
    participant P as Named program
    participant M as Sealed memfd
    participant C as Child process
    R->>T: Select one role-fixed slot
    T->>P: Secure open and identity checks
    T->>P: Read bytes and verify SHA-256
    T->>M: Copy bytes and apply immutable seals
    Note over P,M: Later path replacement cannot alter this snapshot
    R->>C: Execute descriptor 197
    C->>M: Read exact verified bytes
```

The replay and race guarantee is exact but deliberately narrow. Once a tool is
pinned, replacement, unlink, or mutation of the named path cannot change the
bytes executed by that `PinnedExecutable`. A later independent verification
observes the then-current named path and fails on digest, symlink, mode, link,
owner, parent, size, or identity drift. The primitive does not itself decide
whether an effect may be retried, retain either application lock, validate the
runtime/capability bytes, read credentials, or reconcile an ambiguous external
effect. Those remain responsibilities of the workflow and lifecycle route.

The dual-lock command primitive closes the descriptor-custody half of that
boundary. It validates both held-lock path/device/inode identities immediately
before command construction, rejects aliases and descriptor collisions, and
installs the sealed executable as FD 197, actor/adaptor-state lock as FD 198,
and distinct workflow lock as FD 199 in one descriptor-mapping operation. The
child retains both lock descriptors for its lifetime, so competing processes
cannot acquire either lock until the child exits and is reaped. Changed named
locks, crossed-swap locks, unsafe lock-root changes, aliased kernel files, or
mapping drift fail before spawn.

```mermaid
flowchart LR
    P["Sealed executable"] --> F197["FD 197"]
    ALOCK["Actor state lock"] --> F198["FD 198"]
    WLOCK["Workflow lock"] --> F199["FD 199"]
    F197 --> CHILD["Future effect child"]
    F198 --> CHILD
    F199 --> CHILD
    CHILD --> REAP["Exit and reap release custody"]
```

Schema-v3 effect authority now also has an explicit at-use input-custody
boundary. `pin_effect_inputs_at_use` opens each source with `openat2` and no
symlink traversal beneath its exact mode-0700 euid-owned parent. Every source
must be a mode-0600 euid-owned regular single-link file. The parent identity and
the source device, inode, length, owner, mode, link count, modification time,
and change time must remain stable across the bounded read and named-file
recheck. Cross-source device/inode aliases fail closed.

The LEZ runtime is bounded to 16 KiB and must match the authority-pinned
SHA-256. The LEZ capability, all eight Monero RPC username/password sources, and the
shared-wallet file-password source are bounded to 256 bytes apiece. To match
the actual runner, a secret may be one
nonempty ASCII-graphic value stored raw, with one trailing LF, or with one
trailing CRLF; the snapshot preserves those exact source bytes. Empty values,
embedded or repeated newlines, stray carriage returns, NULs, non-graphic bytes,
and oversized values fail closed.

Each of the ten secrets is copied into its own mode-0400 memfd carrying write,
grow, shrink, and seal seals, then duplicated with close-on-exec to a distinct
collision-free parent descriptor. The runtime now receives the same immutable
mode-0400 execution snapshot in addition to its bounded hash-checked in-memory
view. The non-Clone custody types expose only a descriptor path, redacted
length, and SHA-256, and their Debug output redacts values. Named replacement
after pinning cannot alter an existing snapshot; a fresh pin observes unsafe
mode, alias, or runtime digest drift and fails.

```mermaid
flowchart LR
    AUTH["Validated schema v3 authority"] --> PIN["Pin inputs at use"]
    RUN["LEZ runtime source"] --> PIN
    CAP["LEZ capability source"] --> PIN
    RPC["Nine Monero secret sources"] --> PIN
    PIN --> RB["Hash checked runtime snapshot"]
    PIN --> SF["Runtime plus ten sealed secret memfds"]
    SF --> PLAN["Fixed input FDs 200 through 210"]
    PROG["Sealed program FD 197"] --> MAP["One descriptor mapping"]
    LOCKS["Actor and workflow locks FDs 198 and 199"] --> MAP
    PLAN --> MAP
    MAP --> CHILD["Future role fixed effect child"]
```

The generic non-Clone `PinnedChildFdPlan` consumes between 1 and 64 owned
source descriptors. It accepts only unique, non-aliased sources and unique child
targets in 200 through 1023; empty, reserved/out-of-range, duplicate-target, and
aliased-source plans fail closed. Its redacted Debug reports only the descriptor
count. XMR specializes that plan to runtime FD 200, capability FD 201, daemon
username/password FDs 202/203, funding-wallet FDs 204/205, shared-wallet FDs
206/207, role-wallet FDs 208/209, and shared-wallet file-password FD 210.

The XMR command consumes the runtime and all ten secret snapshots alongside
the program and both held locks. Program FD 197, actor/state lock FD 198,
workflow lock FD 199, and input FDs 200 through 210 are installed by exactly one
`fd_mappings` call. No capability, username, password, or runtime bytes enter
argv or the environment. The process RED/GREEN executes the already pinned
program after every named program/input is replaced, observes the exact original
runtime and secret hashes after exec, proves FD 211 absent, then drops the
parent Command and lock handles. The live child alone retains both locks until
it exits and is reaped; only then can the parent reacquire them.

The schema-v3 execution loader now retains both the exact effect-authority
SHA-256 and the fully reconstructed workflow identity beside the validated
effect authority. `prepare_effect_invocation` accepts only the six role-fixed
external-effect slots that have one sending tool: Maker Monero fund, Maker LEZ
tag-15 claim, Maker Monero refund sweep, Taker LEZ tag-14 authorization, Taker
Monero claim sweep, and Taker LEZ tag-16 refund. Wrong-role steps and observation
helpers cannot acquire invocation authority through this API.

Preparation deliberately orders every fallible local custody gate before the
workflow CAS: select the exact role/step tool, compute its plan identity, hash-pin
the executable, pin runtime and all ten secrets, validate the exact actor-state
and workflow locks against the loaded swap/state paths, and compose the complete
FD 197 through 210 command. Only then does it open and validate workflow v2 and
call `authorize_once`. Therefore corrupt program/input/lock/plan state cannot
burn a Prepared row.

```mermaid
sequenceDiagram
    participant R as Future lifecycle route
    participant E as Schema v3 execution authority
    participant P as Program inputs and locks
    participant W as Workflow v2 journal
    R->>E: Prepare one role fixed effect step
    E->>P: Pin tool inputs and compose FDs 197 through 210
    E->>W: Authorize once only after complete plan exists
    alt Prepared winner
        W-->>E: InvokeOnce
        E-->>R: Command and stable plan digest
    else Started or Unknown
        W-->>E: ObserveOnly
        E-->>R: Digest only and no command
    else Succeeded
        W-->>E: Complete
        E-->>R: Digest only and no command
    end
```

The plan SHA-256 uses the stable `lez-xmr-effect-tool-plan-v1\0` domain and binds
role, fixed step name, ABI, pinned program SHA-256, and exact effect-authority
SHA-256. It is stable across restart. InvokeOnce alone owns the prepared
Command; ObserveOnly and Complete drop the locally composed command and return
only the same digest.

This component performs no chain or network I/O. It uses the already locked
rusqlite dependency and SQLite WAL, FULL synchronous writes, foreign keys, and
secure deletion. Creation is exclusive and mode 0600; an existing safe file
returns a typed no-clobber error and an unsafe alias fails closed. Existing
open never creates or migrates a database. The dedicated application ID,
schema version, exact canonical CREATE TABLE definitions, STRICT checks,
foreign-key check, and quick_check must all match.

Before invoking a process, the application commits Prepared to Started and
increments the only attempt counter. Only the transaction that wins that
compare-and-set receives InvokeOnce. Reopened Started and Unknown rows return
ObserveOnly and can never be rearmed. Evidence-free `mark_succeeded` is always
rejected. Only `reconcile_succeeded` may move Started or Unknown to Succeeded,
and it must atomically persist a nonzero SHA-256 of canonical external-effect
evidence, a nonzero SHA-256 of the exact tool-plan identity, and either the
`lez_finalized_event` or `monero_wallet_transaction` source. Exact succeeded
replay is idempotent; any evidence, plan, or source drift is rejected. Every
persisted step row is parsed and checked against its fixed catalog role, scope,
state, counters, revisions, and reconciliation fields whenever the journal is
opened or revalidated.

```mermaid
sequenceDiagram
    participant C as Future lifecycle route
    participant W as Workflow v2 journal
    participant X as Role fixed effect worker
    participant N as Local chain authority
    C->>W: Prepare role legal common steps
    C->>W: Select one branch after common plan exists
    C->>W: Prepare step after predecessor succeeded
    C->>W: CAS Prepared to Started
    alt This process won
        W-->>C: InvokeOnce
        C->>X: Invoke sealed worker
        X->>N: At most one external submission
        X-->>C: Exact result or ambiguity
    else Restart or losing process
        W-->>C: ObserveOnly
        C->>N: Classify without resubmission
    end
    C->>W: Reconcile with evidence and tool-plan hashes
```

Atomicity is deliberately layered. SQLite makes common-plan presence, branch
choice, predecessor gating, local invocation authority, and evidence-bound
reconciliation atomic within the journal. A tag-specific or wallet journal
remains authoritative for its actual send, and canonical finalized-chain or
wallet-history observation remains authoritative for completion. No transaction
is claimed across those layers.

```mermaid
stateDiagram-v2
    [*] --> Prepared
    Prepared --> Started: one winning CAS gives InvokeOnce
    Started --> Started: restart gives ObserveOnly
    Started --> Unknown: ambiguous result
    Unknown --> Unknown: restart gives ObserveOnly
    Started --> Succeeded: exact evidence reconciliation
    Unknown --> Succeeded: exact evidence reconciliation
    Succeeded --> Succeeded: exact replay only
```

## Evidence and limits

The focused RED first failed because the journal API did not exist. GREEN proves
restart-sticky Started and Unknown states, losing-branch rejection, and
role-crossed step rejection. A second RED proved that copied application
headers and table names could forge the initial validator; exact schema
comparison made it GREEN. Eight concurrent creators now produce one new
journal, and eight concurrent authorizers produce one InvokeOnce plus seven
ObserveOnly decisions. The complete lez-swap-store all-target/all-feature suite
is GREEN at 156 tests, with strict Clippy and warning-fatal Rustdoc.

The journal trusts its owner and the owner-private filesystem hierarchy; it is
not an authenticated rollback anchor against a hostile same-UID writer or
backup restore. The current path check enforces a normalized absolute path,
owner-private immediate parent, and a single-link owner-only terminal file;
deployment must also keep every ancestor private. Schema-v3 now directly binds
the run ID, immutable effect-authority path and
digest, and separate workflow path while reconstructing schema-v2 only through
its original parser. The semantic loader rereads and validates the complete
Stage A/B role authority, exact effect bytes, role/swap/agreement/activation/run,
and the already initialized workflow identity. Publication is owner-private,
atomic create-new, and never overwrites schema v2 or an existing schema-v3 file.
The full role-process integration test proves digest tamper, run crossing, legacy
v2 execution, workflow drift, and output collision fail closed.

Receipt v2 now digest-pins schema v3, the effect authority, workflow identity,
and run. The selector semantically revalidates those bytes under the per-swap
and workflow locks, and locked monitor is implemented without chain I/O.
Receipt v1 remains monitor-only. Claim and refund still reject before effect
execution. Focused Taker authority tests are GREEN at 3 of 3 and the combined
Maker/Taker authority pair is GREEN at 4 of 4. Both full package suites,
`lez-swap-store --all-targets` and
`xmr-reference-actor --all-targets --all-features`, strict all-target/all-
feature Clippy, warning-fatal Rustdoc, rustfmt, and diff hygiene are GREEN.

The workflow-v2 and dual-lock focused suites are GREEN: maker process 17 of 17,
workflow concurrency 2 of 2, workflow hardening 1 of 1, restart/no-rearm regression 1 of 1, and workflow-v2 catalog/reconciliation 3 of 3. They prove schema-v1
rejection, catalog and row validation, predecessor and branch gates, one-winner
invocation, exact evidence replay, drift rejection, descriptor separation,
lock-alias, cross-swap, root, and identity rejection, and child custody through
reap.

The expanded Taker authority suite is GREEN at 7 of 7. It proves exact runtime
hashing, raw/LF/CRLF secret preservation, ten distinct sealed descriptors,
redacted metadata-only views, named replacement isolation, fresh drift failure,
and rejection of invalid content, unsafe storage, symlinks, hard links,
oversize inputs, unsafe parents, and cross-source aliases. Strict all-target
Clippy, warning-fatal Rustdoc, rustfmt, and diff hygiene are GREEN.

The generic negative-plan test proves empty, reserved/out-of-range, duplicate
target, and aliased-source rejection plus redacted Debug. The XMR process test
proves exact pre-replacement program, runtime, and secrets after exec; exact FDs
197 through 210; absent FD 211; and both locks held by the child after parent
Command and lock handles are dropped until child exit/reap. Full
`lez-swap-store` and `xmr-reference-actor` all-target/all-feature regressions,
strict Clippy, warning-fatal Rustdoc, rustfmt, and diff hygiene are GREEN.

The current checkpoint makes the normalized shared-wallet file-password path
mandatory and disjoint from every other credential path, pins it as the tenth
sealed secret, maps it to FD 210, and moves the absence sentinel to FD 211.
Focused authority/custody/process coverage also proves missing, relative,
unsafe, and aliased file-password rejection. Maker authority coverage proves
the validated Stage-A/B public paths and exact wire SHA-256 values survive the
execution-authority handoff.

The real schema-v3 Taker Tag14 process fixture proves a corrupted pinned program
fails while the journal remains Prepared, a Maker-only step fails under Taker
authority, the valid child sees exact FDs 197 through 210, and precisely one
caller receives InvokeOnce with a Command. Reload then returns ObserveOnly with
no Command and the identical nonzero domain-separated plan digest. This worker
is only the descriptor/process fixture: it makes no RPC and does not publish a
semantic tag 14.

The typed authority, sealed program/input snapshots, workflow-v2 journal, and
one-map executable/lock/input command boundary and role-fixed invocation
preparation are component-GREEN. The
current effect-input validation, custody, and child-map gaps are closed, but no
Maker or Taker lifecycle route calls it. Actual classifier-to-evidence
composition, Maker effects, receipt-v2 claim/refund routing, and fresh isolated
LEZ plus official Monero Regtest proof remain open. This checkpoint opens no
RPC or node and does not authorize a chain send. Literal M5 therefore remains 4
of 7.
