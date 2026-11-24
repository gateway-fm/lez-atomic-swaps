# Milestone-to-artifact map

First written on 2026-08-26; the M2 and M4 sections were added with their
milestones. Logos accepted M3 at `v0.2.6`; the other upstream issues are open
and their checkboxes unchecked. `Included` below means the artifact is in the
repository; it does not mean Logos has accepted the deliverable.

## M1 — design, threat model, primitives, and SDK surface

Upstream: [logos-co/rfp#121](https://github.com/logos-co/rfp/issues/121)

| Official deliverable | Submission artifact | Status |
|---|---|---|
| Per-leg BTC/XMR/ZEC protocol design and atomicity arguments | [`docs/milestone-1/protocol-design.md`](../docs/milestone-1/protocol-design.md), [`threat-model.md`](../docs/milestone-1/threat-model.md), ADRs 0008–0011 | Included as the historical M1 design foundation |
| LEZ escrow design and SPEL IDL sketch | [`lez-escrow-design.md`](../docs/milestone-1/lez-escrow-design.md), [ADR 0012](../docs/architecture/0012-lez-escrow-custody.md) | Included |
| Threat model for extraction, malleability, timelocks, XMR recovery, and ZEC visibility | [`threat-model.md`](../docs/milestone-1/threat-model.md), [`parameter-profiles.md`](../docs/milestone-1/parameter-profiles.md) | Included |
| Written PR #48 answers and reproducer tests | [`lez-primitive-verification.md`](../docs/milestone-1/lez-primitive-verification.md), [`verify-lez-primitives.sh`](../scripts/verify-lez-primitives.sh), [`lez-v0.2-sequencer-reproducers`](../compat/lez-v0.2-sequencer-reproducers/README.md) | Included |
| Zcash node/wallet decision | [ADR 0004](../docs/architecture/0004-zcash-stack.md) | Included; outside the live BTC video claim |
| Common SDK trait plus per-pair sketches | [`sdk-trait-surface.md`](../docs/milestone-1/sdk-trait-surface.md), [ADR 0013](../docs/architecture/0013-sdk-layering.md) | Included |
| Embedded KV-store decision | [ADR 0003](../docs/architecture/0003-sqlite-persistence.md) | Included; SQLite selected |
| Logos Core daemon-mode integration design and standalone fallback | [ADR 0007](../docs/architecture/0007-maker-local-rpc.md), [ADR 0002](../docs/architecture/0002-ports-and-adapters.md) | Included |

Review entry: [`docs/milestone-1/README.md`](../docs/milestone-1/README.md).
Imported checkpoint: `m1-complete.1` / `96b7b229557e5084857e05bc0c34c03f40c73b66`.

## M2 — ZEC/LEZ leg

Upstream: [logos-co/rfp#122](https://github.com/logos-co/rfp/issues/122)

| Official deliverable | Submission artifact | Status |
|---|---|---|
| LEZ escrow program v1 on testnet 0.2: SHA-256 preimage path, block/timestamp validity windows for the refund | The LEZ v0.2 escrow the deployment manifest pins (ImageID `ca66e0dc…`, with the custody-credit fix of TM-0032; the program the BTC leg settles on); the ZEC agreement binds `secret_digest`, the Taker's first claim reveals it (on LEZ when the Taker sells ZEC, on Zcash when the Taker sells LEZ, [ADR 0216](../docs/architecture/0216-zec-taker-reveals-first-in-both-directions.md)), `ZecRefundPlanV1` carries both windows; settled and refunded on the Docker devnet in [the M2 evidence set](../docs/evidence/regression-m2-2026-11-11/RESULTS.txt), and in the ADR 0216 order in [the 0.3.3 regression](../docs/evidence/regression-0.3.3-2026-11-19-macos-arm64/RESULTS.txt) | Devnet evidence included. Public testnet: not applicable to this release. The public LEZ testnet has moved to LEZ 0.3, which this v0.2.4 stack cannot talk to; public-network evidence is postponed to the release that ports to LEZ 0.4 (S1) |
| BIP-199 HTLC scripts and Zcash integration tests in CI | `lez-zec-swap-sdk` (`bip199_contract`, `bip199_interpreter`, `zcash_transactions`, `zcash_funding`), `lez-zebra-node-adapter`, `zebra_regtest.rs` (fund → claim → refund through Zebra consensus) | Included: the `zebra-regtest` CI job funds, claims and refunds a real HTLC through Zebra on every push and gates the Zebra image on a HIGH/CRITICAL scan; the ZEC crates and the `pair-zec` Node tests run in the workspace jobs (from PR #116, `ff958ec8`, `6574eeea`, `055bb204`, base pin `b7251402`) |
| LEZ/ZEC SDK with full lifecycle coverage | [`lez-zec-swap-sdk` with `lez-swap-sdk-core`](../docs/sdk-lez-zec.md): each stage mapped to its public API and the test that drives it; [crate README](../crates/zec-swap-sdk/README.md); [`examples/full-lifecycle.rs`](../crates/zec-swap-sdk/examples/full-lifecycle.rs) runs offline | Included |
| Zcash node setup documentation | [`docs/zcash-testnet-setup.md`](../docs/zcash-testnet-setup.md): self-hosted Zebra and the public Zebrad provider, wallet creation, funding, the directions each route serves; the Docker devnet route in [`deploy/README.md`](../deploy/README.md) | Included; no fresh-machine run recorded |
| Privacy posture | [`docs/sdk-lez-zec.md#privacy-posture`](../docs/sdk-lez-zec.md#privacy-posture); the Taker desk shows the Node's shield-after-swap guidance on a completed ZEC row | Included |
| ZEC demo set per D1 | `zec-happy`, `zec-maker-refund`, `zec-concurrent` in both directions and `zec-stale-window` selling ZEC, in [`docs/evidence/regression-2026-11-12/videos/`](../docs/evidence/regression-2026-11-12/videos/) (seven recordings; the full set is attached to the v0.3.0 release); each a by-product of a desk journey that passed while being recorded | Included |

Additional functional cases in this pack, all in one run on one set of images
([`PROVENANCE.md`](../docs/evidence/regression-m2-2026-11-11/PROVENANCE.md)), with
the stack running as Logos modules in Basecamp (issue #115,
[`docs/submission-115.md`](../docs/submission-115.md)) and Zebra 7 regtest on
NU7: the ZEC Node matrix and BTC/ZEC parity scenarios in both directions (31
Node tests), the ZEC desk journeys in both directions (18), `zec-balance-refusal`
both ways, `zec-claim-collision` (a revealing LEZ claim superseded by the same
account's other LEZ transaction is re-planned and lands), the revealing-claim
protections, and the BTC regression on the same binaries (28 Node tests, 16
desk journeys): 93 tests, 92 passed in the run. The one failure, its cause and
its reruns are in
[`RESULTS.txt`](../docs/evidence/regression-m2-2026-11-11/RESULTS.txt).

Later full runs of the same suite with the #103 scenarios added (97 tests):
[`regression-2026-11-12`](../docs/evidence/regression-2026-11-12/RESULTS.txt)
(Linux x86_64, 97 of 97, 1 rerun),
[`regression-2026-11-12-macos-arm64`](../docs/evidence/regression-2026-11-12-macos-arm64/RESULTS.txt)
(Apple silicon, `f2f9b980`, 97 of 97),
[`regression-0.3.0-2026-11-12-macos-arm64`](../docs/evidence/regression-0.3.0-2026-11-12-macos-arm64/RESULTS.txt)
(Apple silicon, the v0.3.0 candidate `b4e17cf0`, 97 of 97),
[`regression-0.3.1-2026-11-13-macos-arm64`](../docs/evidence/regression-0.3.1-2026-11-13-macos-arm64/RESULTS.txt)
(Apple silicon, `538530eb`, the code of v0.3.1, 97 of 97),
[`regression-0.3.2-2026-11-14-macos-arm64`](../docs/evidence/regression-0.3.2-2026-11-14-macos-arm64/RESULTS.txt)
(Apple silicon, `961b0fe9`, `main` after PRs #148 and #153, 97 of 97),
[`regression-0.3.3-2026-11-16-macos-arm64`](../docs/evidence/regression-0.3.3-2026-11-16-macos-arm64/RESULTS.txt)
(Apple silicon, `0a42823f`, the fix for #156 and version 0.3.3, 97 of 97),
[`regression-0.3.3-2026-11-19-macos-arm64`](../docs/evidence/regression-0.3.3-2026-11-19-macos-arm64/RESULTS.txt)
(Apple silicon, `fab36d52`, ZEC selling LEZ in the Taker-reveals-first order, 97 of 97, 3 of them on a rerun),
[`regression-0.3.4-2026-11-19-macos-arm64`](../docs/evidence/regression-0.3.4-2026-11-19-macos-arm64/RESULTS.txt)
(Apple silicon, `3b73e65b`, the 0.3.4 candidate with `zec-maker-early-claim`, 98 of 98),
[`regression-0.3.5-2026-11-23-macos-arm64`](../docs/evidence/regression-0.3.5-2026-11-23-macos-arm64/RESULTS.txt)
(Apple silicon, `14b05e56`, the 0.3.5 candidate with the fixes for #185, #186 and #187, 106 of 106); and the 93 tests on Apple silicon,
[`regression-m2-2026-11-12-macos-arm64`](../docs/evidence/regression-m2-2026-11-12-macos-arm64/RESULTS.txt)
(`af3ac978`, 93 of 93). Each set's `PROVENANCE.md` names its commits. The
release record is the [v0.3.0 release page](https://github.com/gateway-fm/lez-atomic-swaps/releases/tag/v0.3.0).


## M4 — XMR/LEZ leg

Upstream: [logos-co/rfp#124](https://github.com/logos-co/rfp/issues/124).
Version 0.4.0. One direction, the Taker sells LEZ; Node-driven swaps on
Monero 0.18.5.1 regtest and the private LEZ devnet, and on public Monero
Stagenet once (a completed, a refunded and a punished swap); why the pair keeps its own order, and where
it departs from RFP-003 F5 and F6, is
[ADR 0217](../docs/architecture/0217-xmr-taker-sells-lez-deviates-from-the-common-flow.md).
Each deliverable with its proof and its limits:
[`docs/m4-acceptance-checklist.md`](../docs/m4-acceptance-checklist.md).

| Official deliverable | Submission artifact | Status |
|---|---|---|
| LEZ escrow program update for the XMR cross-curve DLEQ claim path | Escrow tags 13–17 in [`compat/lez-v0.2-provisional/escrow/src/lib.rs`](../compat/lez-v0.2-provisional/escrow/src/lib.rs) (ImageID `ca66e0dc…`, with the custody-credit fix of TM-0032), [protocol design](../docs/milestone-1/protocol-design.md#xmrlez), [compute units](../docs/lez-compute-units.md) | Included, devnet only. The escrow checks BIP-340 signatures of two-party MuSig2 keys; the DLEQ proofs are checked off chain by each party |
| LEZ/XMR SDK with full lifecycle coverage, including the spend-key-share recovery path under partial information loss | [`lez-xmr-swap-sdk`](../docs/sdk-lez-xmr.md), each area mapped to its public API and tests; [`examples/full-lifecycle.rs`](../crates/xmr-reference-actor/examples/full-lifecycle.rs) in `xmr-reference-actor`, the layer the Nodes run: a completed and a refunded swap, each recovering role rebuilding the spend key after losing its session state, Monero regtest live | Included; the example models the LEZ escrow in memory; its one recorded run, on Monero Stagenet, completed the claim swap and stopped the refund swap at its funding ([Stagenet guide](../docs/monero-stagenet-guide.md#the-sdk-examples-run-of-2026-11-21)) |
| DLEQ test-vector conformance against `comit-network/cross-curve-dleq` | [Erratum GW-M4-001](../docs/proposal-acceptance-errata.md): COMIT publishes no vectors and proves differently; the generators and the keys of a scalar are checked against COMIT's crate instead; self-generated vectors in [`tests/cross_curve_vectors.rs`](../crates/xmr-swap-sdk/tests/cross_curve_vectors.rs) | **Qualified deviation**, as GW-M3-001 was for M3; no Logos decision |
| `monerod` + `monero-wallet-rpc` stagenet setup documentation per U9 | [`docs/monero-stagenet-guide.md`](../docs/monero-stagenet-guide.md): the pinned release verified, a self-hosted pruned `monerod` and public nodes, a `monero-wallet-rpc` per role, faucet coins, the windows sized for 2-minute blocks, the devnet stack with both Nodes on Stagenet, what a Node checks of a daemon it does not run, the SDK example; every command run on 2026-11-21 | Included. On Stagenet the Maker and Taker Nodes completed swap `d9353027…` (Maker's funding at ten confirmations, its LEZ claim, the Taker's sweep at ten confirmations) refunded one and punished one under an earlier 30-minute P − R on 2026-11-21, and on 2026-11-22, with Nodes built from `f059d3fa` (the code of `7b4e46da`) and under the two-hour P − R, completed one, refunded one by the Taker Node's own refund at R, refunded one the Maker could not fund and punished one ([evidence](../docs/evidence/stagenet-0.4.0-2026-11-22/)), with LEZ on the devnet and public Stagenet nodes as daemons; the changes made after `7b4e46da` (the Logos Chat resend, the ZEC second-lock lookup, the sidecar's re-decision of interrupted submissions and `url` 2.5.9) were not run on Stagenet; the self-hosted node was syncing, not at the tip |
| XMR demo set per D1 | `TakerSellsLez-xmr-happy.mp4`, `TakerSellsLez-xmr-taker-refund.mp4`, `TakerSellsLez-xmr-concurrent.mp4` in [`docs/evidence/regression-m4-desks-2026-11-20/videos/`](../docs/evidence/regression-m4-desks-2026-11-20/videos/), each a by-product of a desk journey that passed while being recorded | Included; regtest, the one direction |
| Self-hosted stagenet `monerod` running in CI | The `monero-stagenet` job of [`ci.yml`](../.github/workflows/ci.yml): [`scripts/run-monero-stagenet-ci.sh`](../scripts/run-monero-stagenet-ci.sh) starts the verified `monerod --stagenet` and `monero-wallet-rpc --stagenet` and runs [`tests/stagenet_daemon.rs`](../crates/xmr-monero-adapter/tests/stagenet_daemon.rs) against them, then a bounded sync from public peers | Included; a 1,000-block prefix of the chain, no coins and no swap in CI |

The regression evidence set is
[`regression-0.4.0-2026-11-23-macos-arm64`](../docs/evidence/regression-0.4.0-2026-11-23-macos-arm64/README.md):
the full tier at `27526e2f`, 176 of 176, each on its first try, all 25 Monero
tests among them; an Apple-silicon Mac (linux/arm64 images), three lanes, 263
minutes, one operator running both parties on one host. The set before it,
[`regression-0.4.0-2026-11-22-macos-arm64`](../docs/evidence/regression-0.4.0-2026-11-22-macos-arm64/README.md),
is the full tier at `7b4e46da`, 175 of 175, 174 on their first try and
`ui:btc:logos-happy:TakerSellsForeign` on its second rerun; the one before that,
[`regression-m4-2026-11-21`](../docs/evidence/regression-m4-2026-11-21/README.md),
is the full tier at `d4475300` on x86_64: 160 of 160, each on its first try.

## M3 — BTC/LEZ leg

Upstream: [logos-co/rfp#123](https://github.com/logos-co/rfp/issues/123)

| Official deliverable | Submission artifact | Status |
|---|---|---|
| LEZ escrow update for Schnorr/Taproot adaptor-witness settlement | [ADRs 0042–0045](../docs/architecture/0042-bind-witnessed-token-claims-to-exact-atas.md), [both-direction evidence](../docs/evidence/m3-local-two-direction-poc-20260715.json), matching [UI evidence](../docs/evidence/m3-btc-ui-run-m5arm-0820121736.json), [`crates/`](../crates/) | Functional evidence, architecture, and directly browsable Rust source included |
| LEZ/BTC SDK with full lifecycle coverage | The artefact is [`lez-btc-swap-sdk` with `lez-swap-sdk-core`](../docs/sdk-lez-btc.md), which maps each lifecycle stage to its public API and to the test that drives it; [`examples/full-lifecycle.rs`](../crates/btc-swap-sdk/examples/full-lifecycle.rs) walks all five stages. Also [ADR 0013](../docs/architecture/0013-sdk-layering.md), [ADR 0046](../docs/architecture/0046-replay-btc-sdk-lifecycle-from-exact-transitions.md), [M3 review](../docs/milestone-3-review.md) | Included; discovery and negotiation are ports with no shipped transport, and LEZ transaction construction sits in `lez-bridge-adapter` |
| DLC `AdaptorSignature.md` vectors plus swap-specific vectors | [ADR 0050](../docs/architecture/0050-map-btc-adaptor-construction-to-security-properties.md), Bitcoin Core smoke/P2TR/MuSig2 JSON records, [proposal errata](../docs/proposal-acceptance-errata.md) | **Qualified deviation:** the cited DLC vector file does not exist; BIP-340/BIP-327 and swap-specific evidence is supplied without claiming literal DLC-file conformance |
| `bitcoind` testnet setup documentation | [`docs/bitcoin-testnet4-setup.md`](../docs/bitcoin-testnet4-setup.md) covers the self-hosted Testnet4 node, the exact-HTTPS gateway, and the keyless public provider on Testnet3, each with wallet creation, funding, and the directions it serves; [ADR 0051](../docs/architecture/0051-bind-bitcoin-testnet4-routes-to-chain-profile.md) | Included; public-network runs are recorded in [testnet4](../docs/evidence/testnet4-20260919/README.md) and [testnet3 provider](../docs/evidence/testnet3-provider-20260918/README.md), while the M3 certification itself remains regtest |
| BTC demo set per D1 | Three recordings on the [v0.2.5 release](https://github.com/gateway-fm/lez-atomic-swaps/releases/tag/v0.2.5): `TakerSellsForeign-happy.mp4`, `TakerSellsForeign-maker-refund.mp4`, `TakerSellsForeign-concurrent.mp4`. Earlier supporting material: the [UI walkthrough](../media/lez-btc-ui-swap-demo.mp4), the actual-node [two-lock refund](../docs/evidence/m3-local-two-direction-refund-poc-20260716.json) and [opposite-direction overlap](../docs/evidence/m3-overlapping-two-swap-poc-20260717.json) records, and the [manual walkthrough](../deploy/README.md) | Included; each recording is a by-product of a scenario that passed while being recorded. All three are the `TakerSellsForeign` direction on regtest plus a private LEZ devnet; the opposite direction has non-video evidence only |
| Inline Aumayr and Fournier grounding | [ADR 0050](../docs/architecture/0050-map-btc-adaptor-construction-to-security-properties.md) | Included as a security-property mapping, not a transferred formal proof |

Additional functional cases in this pack: both-direction happy settlement,
both-direction two-lock refunds, both-direction first-lock-only recovery, one
fresh-process post-reveal continuation, and one opposite-direction overlap run.

Imported checkpoint: `m3-complete` / `f7fb250f0491b9c33ed56f2ee02cdbc5ea5dcbb2`.

## M6 — Maker and Taker mini-app GUIs

Upstream: [logos-co/rfp#126](https://github.com/logos-co/rfp/issues/126)

| Official deliverable | Submission artifact | Status |
|---|---|---|
| Signed-off interactive HTML prototypes for both GUIs | [`apps/m6-prototypes/`](../apps/m6-prototypes/), [`m6-prototype-review.md`](../docs/m6-prototype-review.md), [6/6 revalidation evidence](../docs/evidence/m6-prototype-revalidation-20260804.json) | Included |
| Maker mini-app: pair/price, active monitoring, history | [`apps/basecamp/maker/`](../apps/basecamp/maker/), [Maker screenshot](../media/screenshots/maker-offer-desk.png), [package certificate](../docs/evidence/m6-basecamp-role-packages-20260804.json) | Included; current branch also adds the integrated BTC offer desk |
| Taker mini-app: browse, initiate, progress, ZEC shield guidance | [`apps/basecamp/taker/`](../apps/basecamp/taker/), [Taker screenshot](../media/screenshots/taker-market.png), prototype ZEC guidance | Included across prototype and package evidence; the video shows BTC, not the ZEC shielding journey |
| Basecamp-loadable repository, assets, and local-build instructions | [`apps/basecamp/`](../apps/basecamp/), `flake.nix`, `flake.lock`, metadata, icons, and [`apps/basecamp/README.md`](../apps/basecamp/README.md) | Build/package assets included; the integrated runnable route is documented under [`deploy/`](../deploy/) |

Imported checkpoint: `m6-poc-complete` / `78f9842465462715a913dde37105b77c4fd880b2`.

## Cross-milestone submission view

| Claim | Direct evidence in the presentation | Wider repository evidence |
|---|---|---|
| M1 | Design/checkpoint summary | Full M1 documents, verifier, reproducer, ADRs |
| M3 | One real completed BTC/LEZ run, explorer views, dated live validation | Both directions, refunds, first-lock recovery, restart survivor, overlap, cryptographic records |
| M6 | Current Basecamp BTC Maker/Taker journey | HTML prototypes, package builds, isolated journeys, ZEC service certificates |

M5-derived daemon, service, storage, and runner components are exercised by the
live stack. Complete M5 delivery is intentionally not claimed.
