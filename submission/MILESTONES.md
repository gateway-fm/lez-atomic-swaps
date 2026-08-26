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
| Written PR #48 answers and reproducer tests | [`lez-primitive-verification.md`](../docs/milestone-1/lez-primitive-verification.md), [`verify-lez-primitives.sh`](../scripts/verify-lez-primitives.sh), [`lez-sequencer-reproducers.patch`](../tests/upstream/lez-sequencer-reproducers.patch) | Included |
| Zcash node/wallet decision | [ADR 0004](../docs/architecture/0004-zcash-stack.md) | Included; outside the live BTC video claim |
| Common SDK trait plus per-pair sketches | [`sdk-trait-surface.md`](../docs/milestone-1/sdk-trait-surface.md), [ADR 0013](../docs/architecture/0013-sdk-layering.md) | Included |
| Embedded KV-store decision | [ADR 0003](../docs/architecture/0003-sqlite-persistence.md) | Included; SQLite selected |
| Logos Core daemon-mode integration design and standalone fallback | [ADR 0007](../docs/architecture/0007-maker-local-rpc.md), [ADR 0002](../docs/architecture/0002-ports-and-adapters.md) | Included |

Review entry: [`docs/milestone-1/README.md`](../docs/milestone-1/README.md).
Imported checkpoint: `m1-complete.1` / `96b7b229557e5084857e05bc0c34c03f40c73b66`.

## M3 — BTC/LEZ leg

Upstream: [logos-co/rfp#123](https://github.com/logos-co/rfp/issues/123)

| Official deliverable | Submission artifact | Status |
|---|---|---|
| LEZ escrow update for Schnorr/Taproot adaptor-witness settlement | [ADRs 0042–0045](../docs/architecture/0042-bind-witnessed-token-claims-to-exact-atas.md), [both-direction evidence](../docs/evidence/m3-local-two-direction-poc-20260715.json), matching [UI evidence](../docs/evidence/m3-btc-ui-run-m5arm-0820121736.json), [`crates/`](../crates/) | Functional evidence, architecture, and directly browsable Rust source included |
| LEZ/BTC SDK with full lifecycle coverage | [M3 operator guide](../docs/m3-local-poc-operator-guide.md), [M3 review](../docs/milestone-3-review.md), [ADR 0046](../docs/architecture/0046-replay-btc-sdk-lifecycle-from-exact-transitions.md), [`crates/btc-swap-sdk/`](../crates/btc-swap-sdk/) | Historical review/evidence and the complete buildable implementation are included directly; see portability limits |
| DLC `AdaptorSignature.md` vectors plus swap-specific vectors | [ADR 0050](../docs/architecture/0050-map-btc-adaptor-construction-to-security-properties.md), Bitcoin Core smoke/P2TR/MuSig2 JSON records, [proposal errata](../docs/proposal-acceptance-errata.md) | **Qualified deviation:** the cited DLC vector file does not exist; BIP-340/BIP-327 and swap-specific evidence is supplied without claiming literal DLC-file conformance |
| `bitcoind` testnet setup documentation | [`docs/bitcoin-testnet4-setup.md`](../docs/bitcoin-testnet4-setup.md), [ADR 0051](../docs/architecture/0051-bind-bitcoin-testnet4-routes-to-chain-profile.md) | Included; the demonstrated run remains regtest |
| BTC demo set per D1 | Public [actual-UI happy-path walkthrough](../media/lez-btc-ui-swap-demo.mp4); actual-node [two-lock refund](../docs/evidence/m3-local-two-direction-refund-poc-20260716.json) and [opposite-direction overlap](../docs/evidence/m3-overlapping-two-swap-poc-20260717.json) records; exact reproduction procedures in the [M3 operator guide](../docs/m3-local-poc-operator-guide.md) | Included as public walkthrough plus runnable procedures and passed secret-safe evidence; refund/overlap are not represented as public execution footage |
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
