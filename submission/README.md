# RFP-003 submission pack — M1 + M3 + M6

This is the reviewer entry point for Gateway's LEZ ⇄ Bitcoin vertical slice.
It packages the M1 protocol foundation, the M3 Bitcoin implementation evidence,
and the M6 product surface without claiming unrelated milestone completion.

## Start here

| Artifact | Purpose |
|---|---|
| [Actual UI swap demo](../media/lez-btc-ui-swap-demo.mp4) | 1:53.2 Basecamp walkthrough with click ripples, role handoffs, both independent chain checks, captions, and CC BY 4.0 music |
| [Single-file HTML presentation](../media/lez-btc-m1-m3-m6-submission.html) | The current 28-slide story, including the Delivery/Chat transport diagram, code x-rays, and both direction sequence diagrams, with all CSS, JavaScript, SVGs, and screenshots embedded for sharing |
| [PDF presentation](../media/lez-btc-m1-m3-m6-submission.pdf) | Print-ready 28-page offline export of the same deck |
| [`v0.1.1` release evidence map](RELEASE-v0.1.1.md) | Direct source layout, release assets, D1 happy/refund/concurrency evidence, verification gates, and issue-review links |
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

The actual UI recording is fresh run `m5arm-0825151914`: 0.01 BTC ⇄ 1,000
LEZ, terminal revision 4, two Bitcoin effects, three LEZ effects, reconciled
principal and fees, and zero replay submissions. Its exact secret-safe record
is
[`docs/evidence/m3-btc-ui-run-m5arm-0825151914.json`](../docs/evidence/m3-btc-ui-run-m5arm-0825151914.json).
The presentation screenshots and narrated vertical cut remain bound to the
earlier run `m5arm-0820121736` and its separate record:
[`docs/evidence/m3-btc-ui-run-m5arm-0820121736.json`](../docs/evidence/m3-btc-ui-run-m5arm-0820121736.json).

## Claim boundary

This is evidence from Bitcoin Core 31.1 regtest and a private LEZ v0.2 devnet.
It is submitted for review; the public milestone issues remain open and their
checkboxes remain unchecked. The pack does not claim public-network deployment,
production custody, a formal cryptographic audit, M2 Zcash, M4 Monero, complete
M5 operations, or M7 mainnet readiness.

The historical milestone checkpoints are recorded by commit identity. The
default branch now includes the complete buildable source workspace directly;
no source reconstruction or external source checkout is required. See
[Limitations](LIMITATIONS.md) for the remaining runtime and review boundaries.

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

Rebuild the PDF after rebuilding the standalone HTML with:

```sh
./submission/presentation/render-pdf.sh
```

The checked-in MP4 predates the code x-ray slides; rebuilding it produces a
new current-deck artifact whose digest and duration must be reviewed before the
submission manifest is updated. The renderer uses local Google Chrome and
ffmpeg, loads no CDN, burns all copy
into the frames, and writes an H.264/yuv420p 1920×1080 file with no audio
stream.

From the repository root, verify the packaged artifacts with:

```sh
shasum -a 256 -c submission/SHA256SUMS
```
