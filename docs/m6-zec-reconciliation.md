# M6: the planned ZEC flow against what exists

Statement 3 owed in the [12 September review](evidence/review-2026-09-12/README.md#statements-owed-by-gateway-not-test-output):
reconcile the planned ZEC flow with the M6 prototype and service evidence and
with what the apps cover today.

**In one line:** the ZEC flow was designed, prototyped and proven at the
service layer on 4 August; it was taken out of the desks and the prototypes on
26 August when the release was narrowed to BTC↔LEZ, and nothing recorded that.
The M6 apps we ask to be judged are BTC↔LEZ only. ZEC shield-after-swap
guidance is **not** delivered by them.

## What was planned

- Requirement U6 asked for a Taker mini-app covering "each pair including
  refund and ZEC shield-after-swap guidance"
  ([`requirements-traceability.md`](requirements-traceability.md)).
- The [prototype review](m6-prototype-review.md) made the ZEC claim the default
  Taker journey and asked whether it "clearly say[s] transparent-pool linkage is
  public and shielding is a separate wallet action". It was approved at
  `0abdbc2` on 4 August; that approval is internal (statement 4 of the same
  review).

## What the services do

- The ZEC pair is implemented end to end behind the `pair-zec` feature:
  `zec-swap-sdk`, `zec-reference-actor`, `zebra-node-adapter`, the Maker's
  `zec_chat_propose_v1` / `zec_chat_complete_v1`, and the Taker lifecycle, which
  admits `Pair::Zcash` only when the feature is on
  (`route_has_node_lifecycle`). A ZEC swap is *prepared* by the operator
  (`PreparedPrivateMaterial`), not composed by the user as a BTC swap is.
- Evidence, all of 4 August, against Zebra Regtest and a local LEZ v0.2:
  a [service-driven claim](evidence/m6-zec-service-claim-regression-certificate-20260804.json),
  a [service-driven refund](evidence/m6-zec-service-refund-certificate-20260804.json),
  and the [desk run](evidence/m6-basecamp-role-packages-20260804.json) that
  admitted a prepared ZEC swap through the Taker UI. That last record says so
  itself: `basecamp_click_claimed_to_create_chain_effect: false`. There has
  never been one run from a desk click to a ZEC chain effect.
- `pair-zec` is off by default, the shipped Nodes are built without it
  (`from-scratch.sh`), the stack has no Zebra, and CI keeps the ZEC crates
  compiling but parks their tests. **The released binaries contain no ZEC
  code.**

## What the apps do today

- Nothing for ZEC. Commit `ac02b41` (26 August) narrowed both desks' pair
  selectors to Bitcoin, removed the sentence "After a transparent ZEC claim,
  move funds to a shielded wallet address", and removed the ZEC journeys from
  `apps/m6-prototypes/`. The desks now skip every non-Bitcoin row the Node
  returns; a ZEC offer would be invisible rather than disabled.
- The `takerShielding` label survived with Bitcoin wording ("Bitcoin amounts and
  transaction linkage are public; use fresh wallet addresses"), and the package
  contract check only asserts that the label exists. It passing says nothing
  about ZEC.

## Where the records disagreed with the code

| Record | Said | Now |
|---|---|---|
| `requirements-traceability.md`, U6 | "Planned M6", each pair, ZEC guidance | BTC↔LEZ delivered; other pairs and ZEC guidance not delivered |
| `submission/MILESTONES.md`, Taker mini-app row | ZEC shield guidance "included across prototype and package evidence" | it was in the prototype reviewed on 4 August; it is in neither the prototype nor the packages at the submitted commit |
| `m6-prototype-revalidation-20260804.json` | 6/6 journeys, one of them the ZEC claim with shielding guidance | true of `0abdbc2`; the prototype on `main` no longer has that journey |
| `deploy/ui-tests/verify.mjs`, `REAL_ZEC=1` | a ZEC desk test | unreachable since `ac02b41` (it selected a combo entry that no longer exists); deleted with this document |

The first row is corrected in the same change. The submission pack is a
checksummed record of what was submitted, so its row stays and this document is
its correction. The 4 August records stay too: they are accurate about the
commit they name.

## What bringing ZEC back would take

Build the Nodes with `pair-zec`, add Zebra to the stack, restore the pair
selector and the shielding guidance in the Taker desk, give a ZEC swap a
user-driven initiation like Bitcoin's, and record one desk-to-chain run. That
is M2/M6 work for a later release, not part of v0.2.4.
