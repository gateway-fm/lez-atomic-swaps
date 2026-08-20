# LEZ Atomic Swaps

Atomic swaps between Bitcoin and LEZ. No trusted third party. No collateral.
The secret that unlocks one side is the secret that unlocks the other — so
either both sides settle, or both sides refund.

Both directions work: **BTC → LEZ** and **LEZ → BTC**.

## Why you can trust it

Every claim in this repo is checked, not asserted.

- [**The atomicity argument**](docs/diagrams.md) — how adaptor signatures and
  two-party MuSig2 make the legs inseparable, grounded in the shipped code.
- [**Proofs of work**](deploy/full-swap/evidence-m5arm-08180005.json) — a
  completed run's public evidence: five transaction IDs on two chains, exact
  balances, zero private material. Machine-checkable.
- [**Scope, honestly stated**](docs/m3-local-poc-operator-guide.md) — the
  operator guide names what is *not* proven yet: reorgs, process-kill
  recovery, same-direction scheduling. Read the nonclaims before relying on
  anything.
- [**The security-ADR series**](deploy/full-swap/patches/) — the certified
  runner's 26-commit delta, including its 200+ architecture decisions
  (adaptor security properties, refund paths, at-most-once submission).

## Run it

A real swap, on real chains, on your machine:

```sh
cd deploy
./scripts/up.sh
./scripts/prepare-btc-m3-demo.sh
open vnc://127.0.0.1:5901   # password: lezswap
```

Two desks appear. Maker publishes. Taker takes. Then four gates, each owned by
one side:

**Lock → Lock → Claim → Claim.**

Nobody can move funds until both sides click. The swap settles only when both
agree — and refunds if they don't.

Closing everything: `./deploy/scripts/down.sh`.

## What's here

| Path | What it is |
|---|---|
| `apps/basecamp/` | The real Maker and Taker apps (QML over an owner-only socket) |
| `deploy/` | Dockerized LEZ + Bitcoin stack, market controller, VNC demo |
| `deploy/full-swap/` | Evidence packets, runner patches, export tooling |
| `docs/` | Diagrams, atomicity argument, the full operator guide |
| `apps/m6-prototypes/` | Clickable HTML prototypes of both journeys |

## Just want to look?

- **Prototypes, in seconds** — `scripts/run-prototypes.sh`, then open the URL it prints.
- **Real views, no chains** — `scripts/run-real-ui.sh` loads the production
  QML with stub backends.
- **Architecture first?** Start with the [diagrams](docs/diagrams.md).

## Verification

`./deploy/scripts/verify-all.sh` checks the stack end to end: controller
behavior, offer lifecycle, role gating, replay protection, the wallet ledger.

## License

MIT OR Apache-2.0.
