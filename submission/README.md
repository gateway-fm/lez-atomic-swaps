# RFP-003 submission pack — M1 + M3 + M6

This is the reviewer entry point for Gateway's LEZ ⇄ Bitcoin vertical slice.
It packages the M1 protocol foundation, the M3 Bitcoin implementation evidence,
and the M6 product surface without claiming unrelated milestone completion.

## Start here

| Artifact | Purpose |
|---|---|
| [Actual UI swap demo](../media/lez-btc-ui-swap-demo.mp4) | 3:40.7 Basecamp walkthrough of a fresh offer, swap, both independent route checks, settlement, and proof |
| [Silent video presentation](../media/lez-btc-m1-m3-m6-submission-silent.mp4) | 99-second pre-discovery milestone overview with no audio stream |
| [Single-file HTML presentation](../media/lez-btc-m1-m3-m6-submission.html) | The current 22-slide story, including the Delivery/Chat transport diagram, with all CSS, JavaScript, SVGs, and screenshots embedded for sharing |
| [Editable presentation source](presentation/index.html) | Dependency-free source deck with keyboard, touch, fullscreen, and autoplay controls |
| [Milestone map](MILESTONES.md) | Every current issue #121/#123/#126 deliverable mapped to repository artifacts and its submission status |
| [Evidence index](EVIDENCE.md) | Human-readable map of M3 scenarios, the matching UI run, explorers, and M6 certificates |
| [Reproduction guide](REPRODUCE.md) | Inspection-only, lightweight, prototype, and full-stack verification lanes |
| [Limitations](LIMITATIONS.md) | Explicit nonclaims, known deviations, and Gateway portability boundaries |
| [Machine-readable manifest](manifest.json) | Checkpoints, generated media properties, hashes, and live-validation summary |

The presentation poster is
[`media/screenshots/lez-btc-m1-m3-m6-submission-cover.png`](../media/screenshots/lez-btc-m1-m3-m6-submission-cover.png).

## Submission statement

- **M1:** the documented protocol, threat model, LEZ primitive checks, parameter
  profiles, escrow model, and SDK boundaries used as the implementation
  foundation.
- **M3:** private-local functional evidence for the BTC/LEZ pair: both economic
  directions, successful settlement, two-lock and first-lock recovery, one
  post-reveal fresh-process continuation, and one opposite-direction overlap
  run.
- **M6:** signed-off clickable journeys, separate Maker and Taker Basecamp QML
  packages, and the branch's current BTC product journey with real local chain
  effects.

The actual UI recording is fresh run `m5arm-0825141508`: 0.01 BTC ⇄ 1,000
LEZ, terminal revision 4, two Bitcoin effects, three LEZ effects, reconciled
principal and fees, and zero replay submissions. Its exact secret-safe record
is
[`docs/evidence/m3-btc-ui-run-m5arm-0825141508.json`](../docs/evidence/m3-btc-ui-run-m5arm-0825141508.json).
The presentation screenshots and narrated vertical cut remain bound to the
earlier run `m5arm-0820121736` and its separate record:
[`docs/evidence/m3-btc-ui-run-m5arm-0820121736.json`](../docs/evidence/m3-btc-ui-run-m5arm-0820121736.json).

## Claim boundary

This is evidence from Bitcoin Core 31.1 regtest and a private LEZ v0.2 devnet.
It is submitted for review; the public milestone issues remain open and their
checkboxes remain unchecked. The pack does not claim public-network deployment,
production custody, a formal cryptographic audit, M2 Zcash, M4 Monero, complete
M5 operations, or M7 mainnet readiness.

The historical milestone checkpoints are recorded by commit identity. Those
tags are not currently published on the Gateway remote, and this branch is a
curated submission snapshot rather than the full historical Rust workspace.
See [Limitations](LIMITATIONS.md) before attempting the full reproduction lane.

## Presentation controls

Open `presentation/index.html` in a browser, then use:

- `←` / `→` or swipe to navigate;
- `Space` to start or pause deterministic autoplay;
- `F` to toggle fullscreen;
- `Home` / `End` to jump to the first or last slide.

Rebuild the silent MP4 from the current editable deck with:

```sh
./submission/presentation/render-video.sh
```

Rebuild the standalone HTML with:

```sh
node submission/presentation/build-standalone.mjs
```

The checked-in MP4 predates the Delivery/Chat slide; rebuilding it produces a
new current-deck artifact whose digest and duration must be reviewed before the
submission manifest is updated. The renderer uses local Google Chrome and
ffmpeg, loads no CDN, burns all copy
into the frames, and writes an H.264/yuv420p 1920×1080 file with no audio
stream.

From the repository root, verify the packaged artifacts with:

```sh
shasum -a 256 -c submission/SHA256SUMS
```
