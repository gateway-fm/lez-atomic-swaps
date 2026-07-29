# ADR 0102: observe refunds from finalized window prefixes

- Status: Accepted; finalized-page observer and restart-safe durable cursor
  component-GREEN; clean actual-node application replay remains
- Date: 2026-07-29

## Context

An owner can identify its prepared refund transaction exactly. Its counterparty
cannot know that transaction ID before submission and instead discovers the
unique refund whose program, swap ID, ordered accounts, and authority match the
agreement. The LEZ observer previously refused every counterparty discovery
request until the complete configured window had finalized. That made a refund
already finalized near the start of a long window temporarily invisible and
prevented the other role from converging to the same terminal state.

Returning an incomplete absence as terminal would be unsafe. Returning an exact
matching finalized refund is different: later blocks cannot remove that result
inside the finality model already accepted for the local v0.2 indexer.

## Decision

Both exact and terms-based refund observation scan the available finalized
prefix after the window start. A unique matching refund can be returned
immediately. An absence is terminal only when the adapter proves that the whole
window is covered; a prefix-only absence remains `Unstable` and cannot advance
the SDK.

The actor config supplies only the initial page and fixed page size. The
role-local bridge journal owns the active page after first reservation. On a
validated full-page miss, one SQLite transaction completes that request and
inserts a fresh request ID for the next contiguous page. A partial miss,
transport ambiguity, or typed validation error retains the exact current page.
Restart restores the active journal page and ignores the original seed. Page
start arithmetic is checked and exhaustion fails closed. The public SDK result
does not expose or infer this progression bit; only a raw validated sidecar
`Absent` covering the full page may move the cursor. In particular, the stable
funded exact-lookup shortcut does not advance it.

The observer validates the refund variant as well as its common fields.
Hashlock terms require the exact SHA-256 preimage authority and digest.
Witnessed terms require the exact aggregate authority key and account. Mixed
variants fail closed. Terminal evidence requires a finalized containing block,
stable by-ID and by-hash identity, valid ancestry, the deadline, exact program
and accounts, terminal metadata, and zero custody at that block. Historical
block reads remain bounded by the active page even when the current finalized
tip is far ahead.

An exact owner observation may classify a stable finalized `Funded` snapshot as
absent without scanning the entire window. This permits the existing durable
at-most-once submit path to act. Discovery absence never receives that shortcut.

```mermaid
flowchart LR
    OwnerActor["Refund-owning actor"]
    PeerActor["Counterparty actor"]
    SDK["ZEC swap SDK"]
    Adapter["LEZ bridge adapter"]
    Sidecar["Role-isolated LEZ sidecar"]
    Indexer["Finalized LEZ v0.2 indexer"]
    Journal["Encrypted recovery and bridge journals"]
    Escrow["Escrow metadata and custody"]

    OwnerActor --> SDK
    PeerActor --> SDK
    SDK --> Journal
    SDK --> Adapter
    Adapter --> Sidecar
    Sidecar --> Indexer
    Indexer --> Escrow
    Sidecar --> Adapter
    Adapter -->|"full covered miss"| Journal
    Journal -->|"next contiguous page"| Adapter
    Adapter --> SDK
    SDK --> OwnerActor
    SDK --> PeerActor
```

## Recovery sequence and atomicity

```mermaid
sequenceDiagram
    participant T as Taker actor
    participant J as Durable journals
    participant L as LEZ sequencer
    participant I as Finalized indexer
    participant M as Maker actor
    T->>J: Persist exact refund attempt before send
    T->>I: Read deadline and stable funded state
    T->>L: Submit owned refund once
    L-->>I: Finalized refund and terminal accounts
    T->>I: Observe exact transaction and zero custody
    T->>J: Commit Refunded revision 2
    loop Bounded pages until the refund is found
        M->>J: Reserve durable active page
        M->>I: Discover matching refund in finalized page prefix
        I-->>M: Full-page miss or unique finalized refund
        M->>J: On full miss atomically reserve next page
    end
    I-->>M: Unique transaction terminal metadata zero custody
    M->>J: Commit Refunded revision 2 without submission
```

There is no cross-chain database transaction and this ADR does not claim one.
The recovery safety argument is instead:

1. only the depositor for a leg may submit that leg's refund, and only after its
   signed deadline;
2. the exact intent is durable before network submission, so an ambiguous
   restart observes rather than creating a second effect;
3. when both legs are funded, the agreement orders LEZ recovery before Zcash
   recovery; a one-leg abandonment refunds only the funded leg;
4. the counterparty has observation authority only and cannot submit the
   owner's refund;
5. both roles accept only the same unique finalized transaction and the same
   terminal metadata and zero-custody snapshot; and
6. a missing transaction in a partial window is never terminal evidence.

This preserves value atomicity under the documented chain-finality and
authoritative-indexer assumptions. It does not make LEZ and Zcash one atomic
state machine, and it does not remove the upstream lack of proof-bearing LEZ
historical accounts recorded as LOGOS-016.

The bounded old-page liveness claim trusts the official indexer `Finalized`
status, exact by-ID and by-hash equality, within-page ancestry, and stable
bracketed current tip. The v0.2 wire does not carry a proof from an old page to
the current finalized tip. Removing that trust would require an upstream
protocol extension with durable hash checkpoints or proof-bearing historical
reads. This is a Logos-owned production caveat, not a reason to block local RFP
milestone certification.

## Verified local result

The isolated local run `m5fresh-a390dd8-20260728a-app3` began from a deliberate
one-leg state: only the Taker-owned LEZ amount 50000 was locked. After expiry,
the Taker submitted refund transaction
`3a7ffaa55817e0dfe19e5aef35bae678c78cc01220638dff58f65c4bdc116e25`.
It occurs exactly once in finalized block 608, whose by-ID and by-hash indexer
responses agree. Historical state at that block has metadata balance zero and
custody balance zero. The Taker then reached `Refunded` revision 2. With this
decision implemented, the Maker discovered the same finalized refund and also
reached `Refunded` revision 2 without submitting a transaction.

This is an intervention-assisted checkpoint, not a reproducible application
recovery. The originally provisioned observation window was 193 through 448,
while the refund finalized at block 608. The retained run therefore required
manual rotation of both actor observation windows to 590 through 845 and manual
retirement of an older active bridge-journal row before the two actor commands
could converge. Those operations are neither a supported operator procedure nor
daemon/CLI evidence, so the retained run remains intervention-assisted.

The current component removes that implementation gap. A RED integration test
showed both owner exact lookup and counterparty discovery reopening SQLite on
the unchanged configured page 10 through 12 and rejecting a refund at height
14. GREEN durably advances to 13 through 15, restores that page after reopen,
finds the same refund for both roles, and uses fresh request IDs. The test also
proves checked non-overlapping arithmetic and fail-closed height exhaustion. No
schema migration is required because the existing bridge-operation journal
already stores poll sequence, request ID, and window. A fresh actual-node replay
through the daemon and application CLIs is still required before upgrading the
retained evidence claim.

The focused observer test was RED on the old `Unavailable` guard, then GREEN.
The full finalized-refund observer suite is 26 of 26 GREEN across exact and
discovery, hashlock and witnessed authorities, deadlines, ancestry, moving
tips, ambiguity, custody, old pages, and bounded windows. The full bridge-adapter
integration suite is 47 of 47 GREEN.

## Consequences

- A finalized recovery is visible as soon as it exists, rather than after an
  unrelated future window boundary.
- Prefix absence remains a polling state and cannot be misreported as a refund.
- The same rule works for the legacy ZEC hashlock and witnessed asset corridors
  without weakening either authority shape.
- Durable contiguous-page progress is automatic and restart-safe; operators do
  not edit actor config or bridge-journal rows.
- The historical actual-node slice remains intervention-assisted. Application
  manual-action intents, a clean daemon-supervised actual-node replay, concurrent
  composed swaps, and BTC and XMR application lifecycle completion remain M5
  work, so no M5 completion tag is authorized by this result.
