# M6 clickable prototype review

Status: Awaiting owner sign-off

Prototype checkpoint: `0abdbc2`

This is the acceptance record for the interactive HTML prototypes required by
accepted Gateway proposal issue #112. Approval here authorizes the production
Basecamp QML implementation to follow the reviewed journeys; it does not certify
the QML packages, backend bridge, chain effects, or production readiness.

## Reproduce

From the repository root:

```bash
node apps/m6-prototypes/server.mjs
```

Open the run-unique `http://127.0.0.1:<port>/` URL printed by the command.
The server chooses an ephemeral loopback port and serves only allowlisted local
files. No dependency installation, Docker service, RPC, node, faucet, DNS,
wallet, public funds, or public network is used.

## Review checklist

| Requirement | Review surface | Acceptance question |
|---|---|---|
| Maker pair and price configuration | Maker → Pair & price | Are pair, direction, integer price, limits, TTL, and sample-only confirmation clear? |
| Maker active monitoring | Maker → Active swaps | Is secret-free progress and the claim/refund intent boundary understandable? |
| Maker history | Maker → History | Are completed/refunded outcomes and filtering useful? |
| Taker offer browsing | Taker → Browse offers | Are pair, direction, amount, price, Maker identity, and expiry visible before initiation? |
| Taker initiation and progress | Review exact terms → Initiate → Progress | Is explicit consent followed by receipt-shaped progress and mutually exclusive terminal action? |
| ZEC privacy guidance | Complete the default ZEC claim | Does it clearly say transparent-pool linkage is public and shielding is a separate wallet action? |
| Refund journey | Choose Refund at the terminal-action step | Is recovery distinct from a successful claim and free of false chain-effect claims? |
| Role separation | Switch between Maker and Taker | Does each actor see only controls appropriate to that role? |
| Prototype boundary | Persistent amber banner and notices | Is it impossible to mistake browser sample state for daemon, wallet, or chain state? |
| Responsive and keyboard use | Narrow viewport; Tab, Enter, Escape | Are controls usable without a pointer and without horizontal layout loss? |

## Sign-off record

- Decision: pending
- Reviewer: pending
- Reviewed commit: `0abdbc2`
- Date: pending
- Required changes: pending

After an explicit `approved` decision, update this record with the reviewer,
date, exact accepted commit, and any conditions. Production QML must not be
described as accepted before that update.
