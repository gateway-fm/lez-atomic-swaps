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
- A manual refund (`maker_actor_refund_v1`) is queued without a check that
  it applies. Queued on a Monero swap whose actor is still watching for the
  Taker's authorisation or refund (`observe_claim_or_refund`), it becomes a
  recovery the actor refuses, as no branch is chosen yet; the request stays
  queued and is retried for good, the actor's drive does not run again, and
  the Maker can miss its claim window or the Taker's refund.
- Its swap history (`swap_history`) shows `Offered` for a Monero swap that is
  punished, expired or whose funding is in doubt: the swap phases have no
  such state, and only an active actor's phase replaces the aggregate's.

The Taker Node:

- A sweep it has started and cannot prove unsent is marked in doubt and never
  sent again; recovering the Monero from it is by hand, and no tool does it.
- Its read-only Monero steps wait at most 30 s for the role's wallet-rpc and
  take at most 2 minutes; a claim sweep has no overall bound, as one cut off
  after its transfer would read as never sent. Every swap's refund runs
  before any of them.
- It records a claim sweep as in doubt before it takes the wallet lock and
  creates the sweep's wallet file; a crash in between leaves a sweep that
  never went out marked in doubt, recovered by hand like one that did.

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

- Its request store answers a repeated request from what it journaled,
  except for submissions whose caller asks again under the same request id
  only after it found the transaction absent and only inside its own window:
  the XMR tag-13 pair, tags 14 to 17, the native escrow pair and its
  re-plans, the revealing claim and its re-plans (the Maker's follow-up claim
  too) and the native refund (`lez_refund`). For those, an unknown outcome,
  or the in-flight marker a sidecar killed mid-submission leaves, is decided
  on the chain again: the exact bytes are looked up and, absent, sent once
  more (one transaction id and one nonce, included at most once). A definite
  refusal is still replayed, except for tags 16 and 17. The BTC reference
  actor's witnessed and token effects keep one attempt (its own journal sends
  each once and never asks again; a lost one falls back to observation and
  recovery), and so does the reference clock transaction. Unit tests only;
  no devnet scenario kills a sidecar mid-submission except
  `xmr-kill-maker`'s "claim tool running" (tag 15).
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
- The Node launcher fixes the Monero endpoints a Node reads at loopback
  ports 18081 (monerod) and 18083 (its wallet-rpc)
  (`modules/lez-role-node/node-entrypoint.sh`: the role configuration it
  writes and the loopback forwards it starts). In a container each Node has
  its own loopback, so the devnet's lanes are unaffected; outside containers
  two Nodes on one host, a Maker and a Taker or two of either, share one
  loopback: both read the same two ports, and the launcher of each would
  forward them to its own wallet-rpc, so they cannot each have a wallet-rpc
  of their own (`LEZ_XMR_WALLET_HOST` only names where the forward goes,
  not the port the Node reads). The ports are
  not configurable; running both roles' Monero wallets on one host outside
  containers is not supported.

The Logos transport:

- A Logos Delivery node hands a received message to its Chat module only
  for a content topic it has subscribed to, and a Maker subscribes to a new
  direct conversation's topic only after its Chat client has processed the
  Taker's invite (chat_module queues the subscription and forwards it from a
  50 ms poll loop). A Taker request that reaches the Maker's node sooner is
  dropped: the relay received it, the Chat module never saw it, and neither
  Delivery (store off) nor Chat sends it again. On the devnet this is the
  first request of a take, sent about 40 ms after the conversation is
  created; in 15 diagnostic runs on an Apple-silicon Mac it arrived 2 and
  10 ms before the Maker's subscription in the two that failed and 7 to
  552 ms after it in the 13 that passed. Both sides had their relay peer in
  every run, and message size did not matter (the failures were 7.8 KB ZEC
  requests, while 9.9 KB BTC ones passed).
- Workaround in our gateway: the Taker queues an unanswered request frame
  again, the same bytes, 1, 2, 4, 8 and 16 s after its first send, inside the
  25 s wait (`crates/node-common/src/logos_chat_gateway.rs`,
  `REQUEST_RESEND_AFTER`). The Maker's gateway answers a repeated frame from
  its in-flight set or its response cache, and the Maker daemon replays every
  Chat round by its request digest, so no round runs twice; a lost answer is
  sent again on the next resend. A take whose first request was dropped now
  goes on at the next resend that arrives after the Maker's subscription
  instead of failing: on the devnet at `5002e330`, with the Maker's Basecamp
  paused for 5 s while the Taker took, the first three copies arrived before
  the subscription and were dropped, the fourth (sent 4 s after the first)
  was answered and the swap completed; 20 runs of `ui:*:logos-happy` without
  the pause passed with no resend needed.
- What remains upstream: the race itself, in chat_module and Delivery. A
  message on a conversation topic that arrives before its subscription is
  still lost; the resend only makes the exchange recover from it. Any other
  sender of a first message on a fresh conversation topic would meet it too;
  in this project the Maker never sends first (it only answers), and the
  Taker subscribes to the conversation's topic when it creates the
  conversation, before its first request.

Found in the M4 review, outside the Monero protocol:

- A swap's LEZ sidecar keeps running, and keeps its loopback port, after the
  swap ends, on both Nodes and for every pair; a recorded port is never
  reused, so a Node takes at most as many swaps over its life as its role
  configuration's sidecar port range holds.
- The Taker desk offers **Refund** on a Zcash swap from the Taker's
  confirmed lock on, while the Maker is still in time to fund and before the
  refund can be sent (issue #182). A refund requested then waits for its
  time; if the Maker funds meanwhile, the swap can end refunded on both
  sides where it could have completed. No funds are at risk. Bitcoin and
  Monero swaps offer Refund only once it is the Taker's next step.
- The Docker stack's Basecamp containers default their VNC password to
  `lezswap` unless `VNC_PASSWORD` is set (`deploy/images/basecamp-ui/entrypoint.sh`,
  `deploy/compose.yaml`; the testnet overlay `deploy/compose.testnet.yaml`
  uses the same image); the VNC ports are bound to `127.0.0.1`. The systemd
  and macOS packages run no VNC.

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
