# v0.1.0 — RFP-003 M1 + M3 + M6 review release

This is Gateway's public reviewer release for the LEZ ⇄ Bitcoin vertical slice.
It binds the M1 protocol foundation, the M3 BTC/LEZ implementation and evidence,
and the M6 Maker/Taker product surface to one versioned repository state.

Release: <https://github.com/gateway-fm/lez-atomic-swaps/releases/tag/v0.1.0>

## Release assets

| Asset | Purpose |
|---|---|
| [`lez-btc-m1-m3-m6-submission.html`](../media/lez-btc-m1-m3-m6-submission.html) | Self-contained, interactive 28-slide deck with no external runtime dependency |
| [`lez-btc-m1-m3-m6-submission.pdf`](../media/lez-btc-m1-m3-m6-submission.pdf) | Print-ready 28-page export of the deck |
| [`lez-btc-ui-swap-demo.mp4`](../media/lez-btc-ui-swap-demo.mp4) | 1:53 actual Basecamp BTC→LEZ walkthrough, from offer publication through both chain claims and reconciliation |
| [`lez-btc-ui-swap-demo.en.vtt`](../media/lez-btc-ui-swap-demo.en.vtt) | English captions for the walkthrough |
| [`SHA256SUMS`](SHA256SUMS) | Digests for release media, evidence, and reviewer documents |

## Milestone review map

### M1 — protocol foundation

The [M1 issue](https://github.com/logos-co/rfp/issues/121) maps to the per-leg
protocol and atomicity design, LEZ escrow/SPEL sketch, threat model, PR #48
reproducers, Zcash stack ADR, common SDK surface, persistence ADR, and Logos
daemon integration/fallback design listed in [`MILESTONES.md`](MILESTONES.md).
The complete entry point is [`docs/milestone-1/README.md`](../docs/milestone-1/README.md).

### M3 — BTC/LEZ leg

The [M3 issue](https://github.com/logos-co/rfp/issues/123) maps to the witnessed
LEZ settlement ADRs, full-lifecycle SDK/runner source, cryptographic fixtures,
Bitcoin Testnet4 guide, and inline security references listed in
[`MILESTONES.md`](MILESTONES.md).

The D1 behaviours are reviewable as:

| Behaviour | Public evidence | Reproduction |
|---|---|---|
| Happy settlement | [Actual Basecamp walkthrough](../media/lez-btc-ui-swap-demo.mp4) and [matching passed record](../docs/evidence/m3-btc-ui-run-m5arm-0825151914.json) | [`REPRODUCE.md`](REPRODUCE.md#lane-5--full-private-local-btclez-product-stack) |
| Refund/timeout | [Both-direction two-lock refund record](../docs/evidence/m3-local-two-direction-refund-poc-20260716.json) | [Timeout/refund procedure](../docs/m3-local-poc-operator-guide.md#manual-actor-timeoutrefund-recovery) |
| Concurrent swaps | [Opposite-direction overlap record](../docs/evidence/m3-overlapping-two-swap-poc-20260717.json) | [Overlap procedure](../docs/m3-local-poc-operator-guide.md#reproduce-two-overlapping-opposite-direction-swaps) |

All three are private-local functional claims using Bitcoin Core regtest and a
private LEZ v0.2 devnet; no public funds or private key material are published.

### M6 — Maker and Taker mini-apps

The [M6 issue](https://github.com/logos-co/rfp/issues/126) maps to signed-off
interactive HTML prototypes, Maker pair/price/active/history surfaces, Taker
offer/initiation/progress/ZEC-shield guidance, and Basecamp-loadable role
packages with local-build instructions. Start with
[`apps/m6-prototypes/`](../apps/m6-prototypes/),
[`apps/basecamp/`](../apps/basecamp/), and the M6 section of
[`MILESTONES.md`](MILESTONES.md).

## Verification gates

The release candidate is accepted only after:

```sh
./scripts/verify-public-repository.sh
./scripts/run-public-offline-e2e.sh
./scripts/check-architecture-diagrams.sh
./scripts/check-requirements-traceability.sh
node scripts/check-m6-prototype-contract.mjs
shasum -a 256 -c submission/SHA256SUMS
```

The running-stack verification is read-only for settlement: it starts no swap
and submits no chain effect; its market check creates and withdraws one uniquely
named offer. The dated result is recorded under `submission/`.

## Claim boundary

This release is submitted for Logos review; it does not mark the upstream issue
checkboxes or claim formal acceptance. It does not claim public-network
deployment, production custody, independent cryptographic audit, M2, M4,
complete M5 operations, or M7 mainnet readiness. The normative boundary is
[`LIMITATIONS.md`](LIMITATIONS.md).
