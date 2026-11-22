# Limitations and nonclaims

This file is normative for the submission pack. Historical milestone documents
are retained as provenance and may describe the status that was true at their
checkpoint; they do not widen the current claim.

## Review status

- Upstream issues [#121](https://github.com/logos-co/rfp/issues/121),
  [#123](https://github.com/logos-co/rfp/issues/123), and
  [#126](https://github.com/logos-co/rfp/issues/126) remain open and their
  deliverable checkboxes remain unchecked as of 2026-08-26.
- This branch is **submitted for review**. `Included`, `passed`, and historical
  repository-owner approval do not mean Logos has formally accepted a
  milestone.

## Environment and product boundary

- The effect-bearing demonstration uses Bitcoin Core 31.1 **regtest** and LEZ
  v0.2 on a **private local devnet**. It uses no public funds.
- It does not claim public-testnet/mainnet deployment, production custody,
  production signing, liquidity, operational monitoring, or mainnet readiness.
- The current BTC mini-app route uses an owner-local demo controller with
  Docker-socket authority and fixed wallet/amount presets. That controller is
  trusted local-demo orchestration outside the intended production boundary.
- The stack integrates M5-derived daemon, service, storage, and runner
  components, but complete M5 delivery is not claimed.

## Source and Gateway portability

- Gateway's default `main` branch contains the complete Rust workspace,
  compatibility packages, Basecamp source, tests, manifests, and build scripts
  at their conventional paths. A clean Gateway clone needs no source
  reconstruction or external source repository.
- Historical checkpoint tags `m1-complete.1`, `m3-complete`, and
  `m6-poc-complete` are named by commit identity but are not currently
  published on the Gateway remote.
- Cold builds may still require network access for upstream crates, Nix inputs,
  node images, or toolchains. The certified offline lane requires the documented
  caches to be populated first.
- The integrated BTC runtime path remains [`deploy/`](../deploy/); generated
  binaries, node data, Docker volumes, and private evidence roots are not source
  artifacts and are intentionally excluded.

## M3 media and evidence boundary

- The public product footage demonstrates one successful BTC→LEZ economic
  direction. The repository evidence covers the reverse direction, refunds,
  first-lock recovery, restart survival, and overlap, but those cases are not
  all shown as public execution footage.
- The public happy-path walkthrough is paired with passed actual-node refund
  and opposite-direction overlap records plus their detailed reproduction
  procedures. Those two recovery/concurrency cases are not presented as public
  execution footage.
- The committed screenshots and matching JSON belong to run
  `m5arm-0820121736`. The two older `m5arm-08180005` JSON files under
  `deploy/full-swap/` are a separate passed run with different transaction IDs.
- Secret-safe JSON records intentionally omit the bytes and role-private state
  needed for an independent third party to recompute every internal equality.
  They preserve public transaction/block identities and the asserted verifier
  outcomes.

## Cryptographic and atomicity boundary

- ADR 0050 maps the implementation to BIP-340, BIP-327, Aumayr et al., and
  Fournier one-time VES security properties. It is not a formal proof of the
  exact aggregate two-party MuSig2 adaptor composition; independent review
  remains an M7 gate.
- The proposal's requested DLC `AdaptorSignature.md` test-vector path does not
  exist. The repository supplies BIP-340/BIP-327 and swap-specific replacement
  evidence under `GW-M3-001` without claiming literal DLC-vector conformance.
- Cross-chain atomicity is conditional protocol ordering plus recovery, not one
  ACID transaction spanning both ledgers.
- Safety assumes durable role state, key secrecy, exact canonical chain
  observation, conservative deadline separation, viable fees, chain finality,
  and an authorized party able to act during each recovery window.
- After Bitcoin CSV maturity, its key-path claim and refund can be competing
  spends. Fee starvation, deep reorganization, and action at arbitrary deadline
  boundaries are not closed production claims.

## M4 boundary

The XMR/LEZ pair (version 0.4.0). Each limit below is open unless it says
otherwise; the deliverables and their proof are in
[`docs/m4-acceptance-checklist.md`](../docs/m4-acceptance-checklist.md).

Scope:

- Monero 0.18.5.1 **regtest** on the private LEZ devnet for the regression
  and the recordings. On public Monero **Stagenet** the Nodes completed a
  swap, refunded one (the Maker's Monero swept back) and punished one, once,
  on 2026-11-21, with LEZ on the devnet and under an earlier 30-minute P − R
  (the profile now has two hours; those swaps were not run again)
  ([Stagenet guide](../docs/monero-stagenet-guide.md#the-node-driven-run-of-2026-11-21)).
  No Monero testnet or mainnet swap has run; the LEZ testnet waits for the
  LEZ 0.4 port (#107). LEZ v0.2.4 pins RISC Zero 3.0, which is yanked
  (GHSA-g6qg-9j8p-7j87); its `cargo-deny` exemption is an accepted risk on
  devnet-only software (#184), and no public-network release is made while it
  stands.
- One direction, the Taker sells LEZ. Native LEZ only, no tokens (F7).
- One operator ran both parties on one host for every recorded run.
- No third-party review of the construction (DLEQ and adaptor composition);
  S12 and S13 are M7.

Protocol, by design ([ADR 0217](../docs/architecture/0217-xmr-taker-sells-lez-deviates-from-the-common-flow.md)):

- The Taker's refund is open only in [R, P) (a departure from F5). From P the
  Maker's punishment takes the LEZ and the Monero stays locked for good (a
  departure from F6).
- TM-0031: a Maker running other software can punish from P a swap it never
  funded; a Taker Node down through all of [R, P) loses its LEZ, funded or
  not.
- TM-0019: the Taker's claim partial is committed, not verifiably encrypted;
  a Taker that never authorises leaves the Maker's Monero to its refund.
- The escrow verifies no DLEQ proof on chain; each party verifies both proofs
  off chain before it signs.
- The Taker makes the per-swap private view key and sends it to the Maker
  over Chat; each role's own `monero-wallet-rpc` receives the full spend key
  when it sweeps.
- The single LEZ sequencer sees every refund in its mempool, and with it the
  Taker's share. An operator colluding with the Maker could include a
  backdated claim after seeing a refund; a refund it drops until P leaves the
  Maker the LEZ.

The escrow (TM-0032):

- The custody-credit fix holds only for escrows under ImageID `ca66e0dc…`.
  An escrow made under the program of v0.3.4 (`c22d61fc…`) can still be
  locked for good by a stranger's credit to its custody.
- A token escrow whose custody ATA a stranger creates in the same block as
  its `InitializeToken` is refused by the sidecar's initialization
  observation, which requires the ATA to be absent at that block. (Token
  escrows are BTC and ZEC; XMR is native only.)

The Maker Node:

- It checks the Monero network at take time (genesis and `get_info`), and on
  Regtest again before it funds (the mining capability needs the daemon's
  `fakechain`); on Stagenet the funding transfer itself is not preceded by
  another network check, and the topology check (synchronized, a fresh tip)
  comes when the funding is recorded.
- Its Monero RPC reads refuse a wallet with more than 4,096 available outputs
  (`MAX_AVAILABLE_OUTPUTS`): past that the Maker cannot find its output and
  stops, failing closed.
- Its funding tool makes three attempts five seconds apart; a wallet-rpc
  that answers none of them ends the swap `expired` (refunded, nothing lost).
- A funding it sent and cannot record (its daemon refused as not
  synchronized or stale, or the output missed by its view-only wallet)
  still lets it find and record the Taker's refund from R; the sweep back
  runs once the funding is recorded. A funding never recorded leaves its
  Monero at the shared address with the refund signature on LEZ, recovered
  by hand: no tool does it.
- A Maker may set the agreement's restore height up to three blocks above
  the Taker's tip; one that funds fast at that height can have the Taker's
  view-only wallet miss the output, and the swap falls back to the refund
  (griefing at the Maker's own cost, as on regtest).
- The id of its Monero refund sweep is written after the sweep is submitted;
  a crash between the two loses it, and the re-run finds no unspent output to
  sweep. Recovery is by hand.

The Taker Node:

- A sweep it has started and cannot prove unsent is marked in doubt and never
  sent again; recovering the Monero from it is by hand, and no tool does it.
- Its read-only Monero steps wait at most 30 s for the role's wallet-rpc and
  take at most 2 minutes; a claim sweep has no overall bound, as one cut off
  after its transfer would read as never sent. Every swap's refund runs
  before any of them.

On Monero Stagenet (the [Stagenet guide](../docs/monero-stagenet-guide.md#a-daemon-the-node-does-not-run)
and TM-0033 to TM-0036):

- The recorded run read public Stagenet nodes, behind a forwarder, as both
  roles' daemons; the self-hosted node was not at the tip. Each Node
  verifies its daemon's network, sync and tip age before the decisions that
  rest on it (the Maker's funding receipt; the Taker's record of the
  funding, its tag 14 and its claim sweep), and each output by its view
  key, but not that the chain is the heaviest one: a daemon that lies
  consistently is not detected, and `monero-rpc` 0.5.1 bounds no response
  body. The Maker's funding transfer and its sweeps are not preceded by the
  sync and tip-age check.
- Ten confirmations is a constant of the agreement, not sized to the value;
  a reorganisation past it after the Taker's authorisation takes the Monero
  side. No test reorganises a funded swap.
- P − R on Stagenet is two hours (`PublicStagenet`'s minimum): a Taker
  Node down for longer than that from R loses its LEZ (TM-0031).
- The role wallets have no password; the Maker's needs one unlocked output
  per swap in flight (a funding's change is locked ten blocks), and a funding
  that finds none ends the swap `expired`. Fees are the wallet-rpc's default.
- A Maker Node pins each swap's actor program: a swap taken under one build
  does not run under another, so a fix of the Maker's actor does not reach
  swaps in flight (three swaps of the run stayed unfinished on the Maker
  side that way, their Monero at their shared addresses).
- The restore height of the Maker's wallets comes from block timestamps
  (the last block six hours before C); a chain whose timestamps lie by more
  than that leaves a wallet that misses the funding, and the swap falls back
  to the refund.

The sidecar:

- Its request store holds at most 4,096 entries (`MAX_STORE_ENTRIES`). When
  full it evicts a read, then the oldest settled submission; a store full of
  preparations and submissions in flight refuses new requests.
- Its request store's records refuse unknown fields, so a downgraded sidecar
  refuses a store written by a newer one; entries from before the eviction
  order all read as the same age, so the newest settled one may be evicted
  first (the chain refuses a replay by its nonce).

The launcher and the desks:

- The Taker's `monero_network` in its swap view comes from its role
  configuration, not from the swap's Stage A; it is only displayed, and the
  Node refuses a Stage A of another network at take time.

## M6 boundary

- The signed-off HTML prototypes create no daemon, wallet, or chain effects.
- The historical M6 package certificate covers separate Maker/Taker QML
  packages and product tests. The current M3+ branch adds the effect-bearing BTC
  journey; the video does not by itself prove every historical M6 acceptance
  surface.
- ZEC shield-after-swap guidance exists in the prototype flow and the ZEC
  claim/refund certificates are layered service evidence. They are not proof
  that this BTC video demonstrates M2.
- The isolated browser certificate was produced on native amd64. The wrapper
  deliberately refuses emulation or an unsafe `--no-sandbox` fallback on
  arm64.

## Historical status language

Some imported milestone records say `closure candidate`, `tag pending`, or
describe production packages as not yet implemented. Those statements are
historical checkpoint text. Use [`MILESTONES.md`](MILESTONES.md),
[`EVIDENCE.md`](EVIDENCE.md), and [`docs/submission.md`](../docs/submission.md)
as the current reviewer map.
