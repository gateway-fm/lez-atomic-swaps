# Evidence index

This index separates direct product evidence, M3 scenario certificates, and
historical M6 certificates. All committed JSON records are secret-safe by
design: they may include public transaction and block identities, but exclude
private keys, credentials, adaptor scalars, raw private journals, and wallet
seeds.

## Actual UI swap recording

| Field | Value |
|---|---|
| Video | [`lez-btc-ui-swap-demo.mp4`](../media/lez-btc-ui-swap-demo.mp4) |
| Record | [`m3-btc-ui-run-m5arm-0825141508.json`](../docs/evidence/m3-btc-ui-run-m5arm-0825141508.json) |
| Result | `passed` |
| Run | `m5arm-0825141508` |
| Direction | `TakerSellsForeign` / BTC → LEZ |
| Terms | 0.01000000 BTC for 1,000 LEZ |
| Terminal state | revision 4 / `completed` |
| Effects | 2 Bitcoin + 3 LEZ = 5 total |
| Replay submissions | 0 |
| Balance proof | BTC principal 1,000,000 sat; BTC fees 2,000 sat; LEZ principal 1,000; LEZ conserved |
| Networks | Bitcoin Core 31.1 regtest; LEZ v0.2 private local |
| Private material disclosed | `false` |

The recording visibly identifies this run and shows the same completion time,
wallets, five transaction identities, block heights, effect counts, balances,
and replay count carried by the exported record. Chain waits are accelerated;
the role actions and chain effects are from the actual local stack.

## Presentation and still-image run

| Field | Value |
|---|---|
| Record | [`m3-btc-ui-run-m5arm-0820121736.json`](../docs/evidence/m3-btc-ui-run-m5arm-0820121736.json) |
| Result | `passed` |
| Run | `m5arm-0820121736` |
| Direction | `TakerSellsForeign` / BTC → LEZ |
| Terms | 0.01000000 BTC for 1,000 LEZ |
| Terminal state | revision 4 / `completed` |
| Effects | 2 Bitcoin + 3 LEZ = 5 total |
| Replay submissions | 0 |
| Balance proof | BTC principal 1,000,000 sat; BTC fees 2,000 sat; LEZ principal 1,000; LEZ conserved |
| Networks | Bitcoin Core 31.1 regtest; LEZ v0.2 private local |
| Private material disclosed | `false` |

The six committed screenshots and the public vertical demo belong to this
earlier run:

- [`maker-offer-desk.png`](../media/screenshots/maker-offer-desk.png)
- [`taker-market.png`](../media/screenshots/taker-market.png)
- [`finalized-swap-proof.png`](../media/screenshots/finalized-swap-proof.png)
- [`bitcoin-claim-explorer.png`](../media/screenshots/bitcoin-claim-explorer.png)
- [`lez-claim-explorer.png`](../media/screenshots/lez-claim-explorer.png)
- [`lez-claim-evidence.png`](../media/screenshots/lez-claim-evidence.png)

The older `deploy/full-swap/evidence-m5arm-08180005*.json` files are a separate
passed run. Their transaction IDs must not be mixed with the screenshots above.

## M3 scenario matrix

| Scenario | Record | Run ID | Result and scope |
|---|---|---|---|
| Bitcoin Core smoke | [`m3-bitcoin-core-smoke-a7393df-20260714.json`](../docs/evidence/m3-bitcoin-core-smoke-a7393df-20260714.json) | `m3-core-exact-a7393df` | Passed exact-node smoke evidence |
| P2TR construction | [`m3-bitcoin-core-p2tr-4f7b6b3-20260715.json`](../docs/evidence/m3-bitcoin-core-p2tr-4f7b6b3-20260715.json) | `m3-p2tr-exact-4f7b6b3` | Passed Taproot construction evidence |
| MuSig2/adaptor fixtures | [`m3-bitcoin-core-musig2-f5a9caa-20260715.json`](../docs/evidence/m3-bitcoin-core-musig2-f5a9caa-20260715.json) | `m3-musig-exact-f5a9caa` | Passed exact cryptographic fixture record |
| Both-direction settlement | [`m3-local-two-direction-poc-20260715.json`](../docs/evidence/m3-local-two-direction-poc-20260715.json) | `m3poc-live2-20260715a` | Passed BTC→LEZ and LEZ→BTC actual-node settlement |
| Both-direction two-lock timeout | [`m3-local-two-direction-refund-poc-20260716.json`](../docs/evidence/m3-local-two-direction-refund-poc-20260716.json) | `m3refund-20260716h` | Passed ordered refunds; terminal replay produced zero resubmissions |
| First-lock-only recovery | [`m3-local-two-direction-first-lock-refund-poc-20260716.json`](../docs/evidence/m3-local-two-direction-first-lock-refund-poc-20260716.json) | `m3firstlock-20260716h` | Passed both directions with no Maker second lock |
| Post-reveal fresh process | [`m3-local-two-direction-survivor-claim-poc-20260716.json`](../docs/evidence/m3-local-two-direction-survivor-claim-poc-20260716.json) | `m3survivor-20260716c` | Passed fresh Maker-controlled continuation in both directions |
| Opposite-direction overlap | [`m3-overlapping-two-swap-poc-20260717.json`](../docs/evidence/m3-overlapping-two-swap-poc-20260717.json) | `m3overlap-20260717a` | Passed two overlapping swaps with disjoint role state/effects |
| Actor-owned lock/replay | [`m3-schema4-actor-owned-lock-poc-20260717.json`](../docs/evidence/m3-schema4-actor-owned-lock-poc-20260717.json) | `m3schema4-20260717d` | Passed actor-owned second-lock and replay boundary |

The overlap record proves one pair of opposite-direction swaps, not arbitrary-N
concurrency or the same-direction scheduler. The refund records are
machine-readable actual-node evidence; the silent overview does not pretend to
be execution footage of every recovery case.

## M6 evidence

| Record | What it supports | Important boundary |
|---|---|---|
| [`m6-prototype-revalidation-20260804.json`](../docs/evidence/m6-prototype-revalidation-20260804.json) | 6/6 isolated clickable journeys; networkless browser; repository read-only | Prototype state creates no wallet or chain effects |
| [`m6-basecamp-role-packages-20260804.json`](../docs/evidence/m6-basecamp-role-packages-20260804.json) | Two QML role packages, 13 typed slots, one owner-local transport, package/integration checks | Historical package certificate; current BTC integration is the M3+ branch layer |
| [`m6-basecamp-toolchain-preflight-20260804.json`](../docs/evidence/m6-basecamp-toolchain-preflight-20260804.json) | Pinned Basecamp/Nix toolchain preflight | Cold builds depend on immutable public fetches/caches |
| [`m6-zec-service-claim-regression-certificate-20260804.json`](../docs/evidence/m6-zec-service-claim-regression-certificate-20260804.json) | Layered ZEC service claim behavior | Not a continuous Basecamp-click-to-chain video |
| [`m6-zec-service-refund-certificate-20260804.json`](../docs/evidence/m6-zec-service-refund-certificate-20260804.json) | Layered ZEC service refund behavior | Not a claim that the BTC demo proves M2 |

## Dated live validation

[`runtime-validation-20260820.md`](runtime-validation-20260820.md) records the
successful read-only verification run used in the presentation:

- 11 compose services up;
- Bitcoin and LEZ chain heads readable;
- explorer transaction display 110/110;
- wallet market controller 31/31;
- Basecamp Maker checks 3/3;
- Basecamp Taker checks 4/4;
- Maker health `ready=true`, `degraded=false`.

Chain heights are observations, not stable identifiers. Exact transaction and
block identities live in the JSON records.

## Evidence interpretation

These records support a private-local functional claim. They do not establish
public-network availability, deep-reorg immunity, production fee-bumping,
production key custody, independent cryptographic review, or mainnet readiness.
See [`LIMITATIONS.md`](LIMITATIONS.md) and the M3
[security-property mapping](../docs/architecture/0050-map-btc-adaptor-construction-to-security-properties.md).
