# Proposal media

These assets were captured from the running private-local LEZ/BTC stack. They
are not generated product mockups.

## Video

[`lez-btc-rfp003-proposal-vertical.mp4`](lez-btc-rfp003-proposal-vertical.mp4)
is the narrated proposal cut: 102.5 seconds, 1080 × 1920, 30 fps, H.264 video
with AAC audio and burned captions.

It demonstrates one successful direction through the Basecamp Maker/Taker UI:
offer publication, offer acceptance, Taker Bitcoin lock, Maker LEZ funding,
Taker revealing claim, Maker Bitcoin claim, five public effects, explorer
inspection, balance reconciliation, and zero replay submissions.

## Screenshots

- `maker-offer-desk.png` — wallet-owned Maker inventory and offer publication.
- `taker-market.png` — Taker offer discovery and acceptance.
- `finalized-swap-proof.png` — completed revision, five effects, transaction
  identities, balances, fees, and replay count.
- `bitcoin-claim-explorer.png` — the follow-up P2TR claim in the Bitcoin
  regtest explorer.
- `lez-claim-explorer.png` — the revealing claim in the LEZ explorer.
- `lez-claim-evidence.png` — the corresponding LEZ evidence detail.

The transaction identifiers are public identities from disposable local
chains. No private signing material, wallet seed, adaptor secret, or raw runner
state is included.

## Scope

The video is happy-path evidence for M3 Bitcoin settlement surfaced through the
branch's current Basecamp BTC mini-app flow and grounded in the M1 design.
Refund, first-lock recovery, restart-survivor, and overlapping-swap cases are
recorded separately under
[`../docs/evidence/`](../docs/evidence/); they are not visually demonstrated by
this cut. Bitcoin is regtest and LEZ is a private v0.2 devnet.
