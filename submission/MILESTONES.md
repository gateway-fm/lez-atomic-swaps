# Milestone-to-artifact map

Snapshot date: 2026-08-26. The upstream issues are open and all listed
checkboxes are currently unchecked. `Included` below means the artifact is in
this submission branch; it does not mean Logos has accepted the deliverable.

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
| LEZ escrow program v1 on testnet 0.2: SHA-256 preimage path, block/timestamp validity windows for the refund | The deployed LEZ v0.2 escrow (ImageID `c22d61fc…`, the program the BTC leg settles on); the ZEC agreement binds `secret_digest`, the revealing LEZ claim reveals it, `ZecRefundPlanV1` carries both windows; settled and refunded on the Docker devnet in [the M2 evidence set](../docs/evidence/regression-m2-2026-11-02/RESULTS.txt) | Devnet evidence included; a public testnet-0.2 smoke transaction is not (S1) |
| BIP-199 HTLC scripts and Zcash integration tests in CI | `lez-zec-swap-sdk` (`bip199_contract`, `bip199_interpreter`, `zcash_transactions`, `zcash_funding`), `lez-zebra-node-adapter`, `zebra_regtest.rs` (fund → claim → refund through Zebra consensus) | Code and tests included; the CI job that runs the Zebra regtest test on every push lives on PR #116 and is not on this branch yet |
| LEZ/ZEC SDK with full lifecycle coverage | [`lez-zec-swap-sdk` with `lez-swap-sdk-core`](../docs/sdk-lez-zec.md): each stage mapped to its public API and the test that drives it; [crate README](../crates/zec-swap-sdk/README.md); [`examples/full-lifecycle.rs`](../crates/zec-swap-sdk/examples/full-lifecycle.rs) runs offline | Included |
| Zcash node setup documentation | [`docs/zcash-testnet-setup.md`](../docs/zcash-testnet-setup.md): self-hosted Zebra and the public Zebrad provider, wallet creation, funding, the directions each route serves; the Docker devnet route in [`deploy/README.md`](../deploy/README.md) | Included; no fresh-machine run recorded |
| Privacy posture | [`docs/sdk-lez-zec.md#privacy-posture`](../docs/sdk-lez-zec.md#privacy-posture); the Taker desk shows the Node's shield-after-swap guidance on a completed ZEC row | Included |
| ZEC demo set per D1 | `zec-happy`, `zec-maker-refund`, `zec-concurrent` in both directions and `zec-stale-window` selling ZEC, in [`videos/`](../docs/evidence/regression-m2-2026-11-02/videos/); each a by-product of a desk journey that passed while being recorded | Included |

Additional functional cases in this pack, all on one set of images
([`PROVENANCE.md`](../docs/evidence/regression-m2-2026-11-02/PROVENANCE.md)): the ZEC Node matrix and BTC/ZEC parity
scenarios in both directions (28), the ZEC desk journeys in both directions (15),
`zec-balance-refusal` both ways, `zec-claim-collision` (a revealing LEZ claim
superseded by the same account's other LEZ transaction is re-planned and lands),
the revealing-claim protections (no refund once a revealing claim was submitted;
one revealing claim in flight per Node until final; no revealing claim after its
window), and the BTC regression on the same binaries (Node 14 + 14, desks 8 + 8).
Every failure in the first pass is listed with its cause and its rerun in
[`RESULTS.txt`](../docs/evidence/regression-m2-2026-11-02/RESULTS.txt).

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
