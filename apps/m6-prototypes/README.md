# M6 clickable role prototypes

Deterministic, secret-free HTML prototypes for the Maker and Taker mini-app
journeys. Every value is sample state. Buttons change only in-memory browser
state; the prototypes do not contact a daemon, chain node, Delivery, Chat, or
any other network service.

## Run locally

From the repository root:

```bash
node apps/m6-prototypes/server.mjs
```

Open the run-unique loopback URL printed by the command. The server asks the
kernel for an ephemeral port, binds loopback only, and serves an allowlisted
set of files from this directory. For an explicit isolated port, set
`M6_PROTOTYPE_PORT`; the server fails instead of taking over a busy port.

You can also open `index.html` directly from disk. The local server is useful
for reproducing the same URLs during review.

## Prototype journeys

- Maker: configure a pair and sample price, inspect active swap progress, and
  filter durable-history-shaped sample rows.
- Taker: browse sample offers, review and initiate a swap, advance receipt-bound
  sample progress, choose a terminal claim/refund action, and review the ZEC
  shield-after-swap privacy guidance.

Keyboard users can tab through all controls. `Escape` closes open dialogs.

## External resources and effects

- Runtime network requests: none.
- External fonts, scripts, images, analytics, or CDNs: none.
- Persistent browser storage: none.
- Chain, wallet, daemon, Delivery, or Chat effects: none.
- Dependencies: a local Node.js runtime only when using `server.mjs`.

The SVG artwork in `assets/` is original code-native source maintained with the
prototype.
