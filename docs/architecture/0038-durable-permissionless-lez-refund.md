# ADR 0038: Durably prepare the permissionless LEZ refund before actor eligibility

Status: Accepted through the public actor one-attempt LEZ and Bitcoin recovery composition; deterministic tests are GREEN and fresh actual-node timeout/refund evidence remains active -- 2026-07-16

```mermaid
flowchart LR
    Terms["Countersigned refund terms"] --> Validate["Role, runtime, program,<br/>destination, and authority validation"]
    Validate --> Build["Official RefundNative message<br/>metadata, custody, depositor"]
    Build --> Unsigned["Zero nonces<br/>zero witnesses"]
    Unsigned --> Reserve["Owner-only durable<br/>exact-byte reservation"]
    Reserve --> BridgeCache["Authenticated role/run bridge<br/>durable request/result replay"]
    BridgeCache --> Admit["Submission boundary admits<br/>only the retained bytes"]
    FinalizedClock["Stable finalized LEZ clock<br/>and historical escrow state"] --> Eligible{"Deadline reached<br/>and still Funded?"}
    Eligible -->|"no"| NoEffect["No public effect"]
    Eligible -->|"yes"| Journal["Actor one-attempt<br/>public-effect journal"]
    Admit --> Journal
    Journal -->|"Prepared to Started CAS"| Sequencer["One sequencer send"]
    Journal -->|"Started or Unknown"| ObserveOnly["Observe only<br/>never resubmit"]
    Sequencer --> Indexer["Finalized indexer evidence"]
    ObserveOnly --> Indexer
    Indexer --> ObserveRpc["Authenticated repeatable observe<br/>no submit and no cached chain truth"]
    ObserveRpc --> Project["Project one finalized refund<br/>only from exact final evidence"]
    Project --> Lifecycle["Actor-local evidence replay<br/>maker refund revision 3"]
    Lifecycle --> Later["Only after durable projection<br/>drive the later taker refund"]
    Later --> Terminal["Taker refund revision 4<br/>terminal Refunded"]
```

## Context

The pinned LEZ v0.2 guest already implements `RefundNative` as a permissionless
instruction. It accepts exactly metadata, custody, and the immutable depositor
account; it consumes no signer nonce or witness. Once the guest clock reaches
`refund_at`, it transfers the complete custody balance only to that depositor,
zeros custody, and makes metadata terminal `Refunded`. Permissionless execution
therefore removes a liveness dependency without allowing the caller to choose a
beneficiary.

The generated upstream client constructs and submits immediately. That API does
not provide the repository's required prepare-before-effect durability,
request-id replay, exact-byte ownership, one-attempt ambiguity recovery, or
finalized evidence. Reusing the signed-transaction decoder would also be wrong:
it deliberately rejects empty witnesses, while an official refund must be
unsigned.

## Decision

The v0.2 sidecar planner constructs one canonical official public transaction
from the complete bridge request. The message contains the configured escrow
program, derived metadata and custody PDAs, the immutable depositor, no nonces,
and `RefundNative { swap_id }`. The planner supports both the legacy strict
hashlock terms and the strict M3 aggregate-witness terms. Witnessed requests
recompute the aggregate account from the supplied public key even though the
permissionless instruction does not consume that authority.

Before returning any bytes, a durable planner atomically creates one owner-only
`native-refund-reservation.v1.json`. Identical requests replay the exact bytes
after restart without consulting an account-nonce RPC. A distinct request, a
changed transaction ID, noncanonical encoding, account or instruction change,
injected nonce or witness, signer substitution, program substitution, or
aggregate-authority substitution fails closed. The generic submission boundary
accepts only the exact active reservation and uses a dedicated unsigned decoder.

The capability-authenticated, run/role/runtime-bound loopback bridge now exposes
that preparation. It durably records the canonical request and successful
result, restores the planner and compares the reconstructed result before
binding a restarted server, and replays an identical request ID without calling
the nonce source. Observations remain repeatable and are not cached as chain
truth. The finalized
witnessed observer now implements the state-only, exact-owned, and
discover-by-terms modes behind that same authenticated boundary.

Preparation is not deadline evidence and grants no send authority. The public
`btc-reference-actor recover` command now performs a role-bound state-only read
before preparation. Before the deadline it returns pending with zero prepare and
zero submit calls. At or after the signed deadline, only the LEZ depositor may
replay the deterministic witnessed refund preparation, persist its exact public
bytes, observe that exact ID, and ask the public-effect journal for send
authority. The claimant uses `DiscoverByTerms` only and never prepares or
submits. Only a fresh stable `Funded` account snapshot at or beyond the deadline
may let the journal consume one `Prepared` to `Started` CAS. The journal now encodes that distinction: refund `Absent` is invalid, only
affirmative `EligibleToAttempt` can authorize the refund operation, non-refund
effects cannot use that eligibility, and concurrent callers still produce one
durable winner.
After any possible call, `Started` and `Unknown` are permanently observation-only.
Projection to the lifecycle store requires later exact finalized `Refunded`
evidence with zero custody and the immutable depositor effect. Exact observation
is bound to the actor-owned durable bytes; claimant discovery is bound to the
signed witnessed terms. Both scan a fully covered bounded window through the
stable finalized tip, require equal block results by ID and hash plus intact
ancestry, accept only the canonical unsigned RefundNative account/instruction
shape, enforce the containing-block timestamp at or after `refund_at`, and check
historical and tip terminal accounts. Complete discovery misses may be Absent;
exact misses remain UnknownOrPending. Missing or partial account state fails
unavailable rather than becoming absence.

The existing compatible response wire exposes the stable finalized tip clock and
the refund transaction position, but it has no separate containing-block
finality/timestamp object. The observer enforces that fact internally. A future
additive finalized-refund result would be required if downstream consumers must
independently reverify the containing timestamp from the response alone.

The actor-local Bitcoin recovery store now admits an alternative exact four-record branch after the two locks: maker-funded refund evidence occupies revision three and taker-funded refund evidence occupies revision four. Each record carries a typed chain position, canonical transaction proof, and bounded adapter evidence. Replay calls the shared coordinator deadline checks and reaches `MakerLegRefunded` before terminal `Refunded`. Existing happy-path JSON omits the optional refund position, and the SQLite constraint migration copies those exact payload strings unchanged in one immediate transaction.

## Atomicity and failure analysis

There is no distributed transaction spanning Bitcoin, LEZ, and SQLite. Atomicity
is instead preserved to the extent available by the signed cross-chain deadline
order, immutable refund destinations, durable exact bytes before any send,
one-attempt public-effect authority, and observe-before-project recovery.

- A crash before durable reservation exposes no transaction.
- A crash after reservation but before send replays only the same exact bytes.
- A crash or timeout after `Started` cannot rearm submission authority.
- An early preparation cannot be submitted by the actor until finalized deadline
  eligibility is proven.
- A public transaction response cannot project state without matching finalized
  chain evidence.
- The later taker-funded refund cannot enter durable revision four until the
  earlier maker-funded refund is already exact durable revision three.

This decision now claims authenticated prepare reachability, exact restart
replay, finalized witnessed state/exact/discovery observation with internal
deadline enforcement, public one-shot actor composition, one-attempt LEZ and
Bitcoin authority, role-separated observer behavior, and ordered durable
lifecycle projection to `Refunded`. It does not yet claim fresh actual-node
timeout/refund execution, reorg/fee stress, or the final M3 closure run. Those
remain the next M3 gates.

## Evidence

`compat/lez-v0_2-sidecar/tests/native_refund_prepare.rs` covers the exact
official ABI, zero nonce/witness behavior, strict hashlock compatibility,
witnessed authority binding, transaction and identity mutations, byte-identical
restart recovery, distinct-request exclusion, owned-submission admission, and
zero nonce-source calls. `bridge_native_refund.rs` additionally proves one
authenticated loopback preparation and byte-identical replay after both the
server and planner restart. Its sequencer is an ephemeral loopback health stub;
no faucet, chain node, Docker service, or public network dependency is used.
`finalized_native_refund_observation.rs` adds nine cases for stable state-only
clocks at deadline minus one and deadline, exact owned presence, claimant
discovery, complete/incomplete absence, containing-block deadline enforcement,
historical and tip terminal state, zero custody, canonical unsigned bytes, block
identity and ancestry, moving tips, mutation, role and ID pre-read rejection,
ambiguity/conflict, and authenticated repeatable no-submit behavior. It uses an
in-memory finalized-indexer double plus an ephemeral loopback health server; it
does not use a faucet, chain node, Docker service, or public endpoint.

`swap-store/tests/public_effect_journal.rs` proves refund absence rejection,
operation separation, restart no-rearm, and one winner across eight concurrent
eligibility observers. `swap-store/tests/btc_recovery.rs` proves both-direction
and both-role refund replay, early/wrong-chain/zero-confirmation and happy-branch
collision rejection, terminal restart replay, and exact old-schema migration.
Those store tests use only private temporary SQLite files and no network resource.

`btc-reference-actor` deterministic recovery tests exercise the same public
actor observer boundaries for both refund transitions: pre-deadline LEZ reads
perform no preparation, eligible LEZ and Bitcoin owners consume one send,
`Started`/`Unknown`/`Accepted` restart states never rearm, exact finalized
evidence projects, and nonowners remain observation-only. They use temporary
owner-private SQLite files and injected chain ports; no RPC, faucet, Docker
service, or public endpoint is involved. Fresh actual Core/LEZ node evidence is
therefore still required before the M3 tag.
