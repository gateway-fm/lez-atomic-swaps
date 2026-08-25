# M3+ submission scope and provenance

This branch is a focused proposal package for the LEZ/BTC vertical slice. It
combines the accepted design foundation, the Bitcoin implementation evidence,
and the Basecamp product surface without presenting unrelated milestone work as
part of this submission.

## Included checkpoints

The milestone artifacts were imported from immutable local tags, not copied
from an uncommitted worktree.

| Area | Source checkpoint | Commit | Included here |
|---|---|---|---|
| M1 design foundation | `m1-complete.1` | `96b7b229557e5084857e05bc0c34c03f40c73b66` | Design/review packet, ADRs 0001–0013, traceability, and document/verifier support files |
| M3 Bitcoin PoC | `m3-complete` | `f7fb250f0491b9c33ed56f2ee02cdbc5ea5dcbb2` | Review, operator guide, ADRs 0029–0052, both-direction happy paths/refunds, first-lock recovery, one fresh-process continuation, and one opposite-direction overlap run |
| M6 product work | `m6-poc-complete` | `78f9842465462715a913dde37105b77c4fd880b2` | Review, ADRs 0128–0147, evidence, prototype contract, and isolated UI test |
| M3+ UI/deployment | branch baseline | `93dd323ca01b396e30bdc04a75073419c737ea2c` | Updated Basecamp BTC UX, Docker stack, explorers, and evidence exporter |
| Bidirectional runner | 34 patches over `5c384a5` | `2888e8cf818143a7dce903f343fdbe70de9e267a` | Exact committed application runner used by the live stack; reconstructed tree `c3bc49ad802328455ef7c8843d3c7a4ee81bade9` |

The M3+ Basecamp files intentionally remain at the branch's integrated state;
they were not replaced with the older M6 snapshot. Historical milestone
documents remain milestone-scoped, while the root README and this file explain
what the current runnable product does.

The small M2 evidence/setup dependency set referenced directly by the M3
architecture and manual-flow documents is included so every repository-local
Markdown link resolves. It is supporting provenance, not an M2 delivery claim.

## What a reviewer can inspect

1. Open the [reviewer submission pack](../submission/README.md), the
   [single-file HTML presentation](../media/lez-btc-m1-m3-m6-submission.html), or
   the [silent overview video](../media/lez-btc-m1-m3-m6-submission-silent.mp4),
   then watch the [fresh actual UI swap](../media/lez-btc-ui-swap-demo.mp4).
2. Read the [M1 entry point](milestone-1/README.md) and [M1 review](milestone-1/review.md).
3. Read the [M3 review](milestone-3-review.md) and inspect the `m3-*.json`
   records under [`docs/evidence/`](evidence/).
4. Review the [atomic success/refund flow](diagrams.md) and
   [security-property mapping](architecture/0050-map-btc-adaptor-construction-to-security-properties.md).
5. Inspect the [M6 review](m6-prototype-review.md), the current
   [`apps/basecamp/`](../apps/basecamp/) packages, and the
   [`apps/m6-prototypes/`](../apps/m6-prototypes/) journeys.
6. Run the [private-local stack](../deploy/) and open the Bitcoin explorer, LEZ
   explorer, service logs, and exported evidence from the UI.

## Milestone mapping

| Milestone area | Submission statement |
|---|---|
| M1 | The protocol, threat model, primitives, parameters, and SDK boundaries are the documented foundation used by the implementation. |
| M3 | Direct implementation and evidence for the LEZ/BTC pair, including both directions, happy settlement, ordered refunds, restart survival, and overlapping swaps. |
| M6 | M6 review, evidence, and prototypes plus the branch's current Basecamp packages; the video demonstrates the branch's BTC journey, not the complete M6 acceptance surface. |
| M5 | Daemon, service, storage, and runner components are integrated transitively in the live stack; complete M5 delivery is not claimed by this package. |

M2 Zcash, M4 Monero, and M7 audit/mainnet readiness are outside the package's
claim. Some M6 milestone records necessarily mention Zcash because the
historical M6 review covered that journey; those records are provenance, not a
claim that the current BTC video demonstrates M2.

## Evidence boundary

Committed screenshots, videos, and JSON records contain public identities from
isolated local chains. They do not include wallet seeds, signing keys, adaptor
secrets, private journals, databases, or raw `.e2e` directories. The media is
evidence of a private-local protocol run, not a public-testnet or mainnet
deployment.

## Quick validation

```sh
./scripts/check-architecture-diagrams.sh
./scripts/check-requirements-traceability.sh
node scripts/check-m6-prototype-contract.mjs
cd deploy && ./scripts/verify-all.sh
```

The first three checks are repository-local. The full deployment check expects
the Docker stack and its pinned runner prerequisites described in
[`deploy/README.md`](../deploy/README.md).

The historical M6 browser check is available as
`./scripts/run-m6-prototype-e2e-isolated.sh`. Its digest-pinned Chromium image
is amd64; the runner deliberately refuses emulation or a `--no-sandbox`
fallback on arm64. The committed M6 evidence records the native-amd64 6/6 run,
while the dependency-free prototype contract above is portable.
