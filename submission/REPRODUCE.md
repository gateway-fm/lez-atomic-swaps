# Reproduction guide

Choose the narrowest lane that answers the review question. The first three
lanes run from a clean submission clone without private runtime state. The full
stack needs the separately provisioned runner described below.

## Lane 1 — inspect the pack with no services

Open these files directly:

1. [`README.md`](README.md)
2. [`MILESTONES.md`](MILESTONES.md)
3. [`EVIDENCE.md`](EVIDENCE.md)
4. [`lez-btc-m1-m3-m6-submission.html`](../media/lez-btc-m1-m3-m6-submission.html), the single-file offline deck
5. [`lez-btc-m1-m3-m6-submission.pdf`](../media/lez-btc-m1-m3-m6-submission.pdf), the print-ready 28-page deck
6. [`presentation/index.html`](presentation/index.html), the editable source deck
7. [`lez-btc-ui-swap-demo.mp4`](../media/lez-btc-ui-swap-demo.mp4), the actual Basecamp swap walkthrough, with its exact [`m5arm-0825151914` evidence](../docs/evidence/m3-btc-ui-run-m5arm-0825151914.json)

Check the actual UI recording and fully decode it:

```sh
ffprobe -v error \
  -show_entries format=duration,size:stream=index,codec_type,codec_name,width,height,r_frame_rate,pix_fmt \
  -of json media/lez-btc-ui-swap-demo.mp4
ffmpeg -v error -i media/lez-btc-ui-swap-demo.mp4 -f null -
```

Expected UI-demo properties: H.264, 1920×1080, 30 fps, `yuv420p`,
113.200000 seconds, 3,396 frames, 15,067,133 bytes, and one stereo AAC audio
stream. The burned-in run ID must be `m5arm-0825151914`, matching the linked
evidence record.

## Lane 2 — repository-local document and prototype contracts

From the repository root:

```sh
./scripts/check-architecture-diagrams.sh
./scripts/check-requirements-traceability.sh
node scripts/check-m6-prototype-contract.mjs
```

Validate that every M3 JSON certificate in the public evidence directory
reports `passed`:

```sh
for evidence in docs/evidence/m3-*.json; do
  jq -e '.result == "passed"' "$evidence" >/dev/null
done
```

The LEZ primitive verifier clones/checks its exact upstream input and therefore
has a setup-time network dependency:

```sh
./scripts/verify-lez-primitives.sh
```

## Lane 3 — inspect the clickable M6 journeys

The prototype server is dependency-free and loopback-only:

```sh
node apps/m6-prototypes/server.mjs
```

Open the ephemeral `http://127.0.0.1:<port>/` URL printed by the command. The
pages explicitly identify themselves as prototypes; they create no daemon,
wallet, or chain effects.

The historical isolated browser evidence is committed at
[`docs/evidence/m6-prototype-revalidation-20260804.json`](../docs/evidence/m6-prototype-revalidation-20260804.json).
Its pinned browser image is amd64; the wrapper deliberately refuses an
unsandboxed or emulated fallback on arm64.

## Lane 4 — rebuild the HTML-derived silent video

Requirements: a Chromium-compatible browser and ffmpeg with `libx264` and
`xfade`.

```sh
./submission/presentation/render-video.sh
```

Override tool paths if required:

```sh
CHROME_BIN=/absolute/path/to/chromium \
FFMPEG_BIN=/absolute/path/to/ffmpeg \
./submission/presentation/render-video.sh /absolute/path/to/output.mp4
```

The renderer creates temporary frames under `mktemp`, writes the poster and
MP4, and removes only that verified temporary directory. It performs no network
fetch and references only repository-local assets.

Build the single-file HTML export with Node:

```sh
node submission/presentation/build-standalone.mjs
```

The builder embeds the source CSS, JavaScript, SVGs, and PNGs as local data
URIs. The resulting `media/lez-btc-m1-m3-m6-submission.html` has no external
runtime dependency.

Build the PDF from that standalone export with local Chrome/Chromium:

```sh
./submission/presentation/render-pdf.sh
pdfinfo media/lez-btc-m1-m3-m6-submission.pdf
```

Expected PDF properties: 28 pages, 960 × 540 pt (16:9), with no network fetch.

## Lane 5 — full private-local BTC/LEZ product stack

### Important prerequisite

The complete Rust workspace and application sources are checked in directly on
Gateway's default `main` branch. Use the repository root itself as the M3
application runner; no second checkout, patch application, or reconstruction
step is required. Confirm the direct source contract before provisioning the
runtime:

```sh
./scripts/verify-public-repository.sh
cargo metadata --locked --no-deps
```

The runner-work root must be mounted at the same absolute path on the host and
inside `lez-runner-arm`; the runner talks to the host Docker socket and nested
bind paths are resolved by the host daemon. Pinned LEZ v0.2, RISC Zero, and
arm64 prover inputs must also be provisioned as described in
[`deploy/full-swap/README.md`](../deploy/full-swap/README.md).

After provisioning:

```sh
export LEZ_M3_RUNNER_REPO="$PWD"
export LEZ_M3_RUNNER_REPO_IN_CONTAINER="$PWD"

cd deploy
./scripts/up.sh
./scripts/prepare-btc-m3-demo.sh
```

Open:

- Basecamp over `vnc://127.0.0.1:5901`;
- Bitcoin explorer at <http://127.0.0.1:3002>;
- LEZ explorer/evidence at <http://127.0.0.1:3003/#/evidence>.

Drive the product path:

1. Maker publishes one wallet-owned LEZ/BTC offer.
2. Taker accepts the exact offer.
3. Taker locks BTC.
4. Maker funds LEZ.
5. Taker claims LEZ.
6. Maker claims BTC.
7. Open all five transaction proofs and the wallet reconciliation.

Then run:

```sh
./scripts/verify-all.sh
docker compose --env-file runtime/runtime.env logs --tail=200 \
  maker-node taker-service btc-demo-controller
```

Stop only this compose project:

```sh
./scripts/down.sh
```

Use `./scripts/down.sh --wipe` only when intentionally removing the generated
local state. Never publish the external runner's `.e2e` roots, identity keys,
databases, signing journals, or generated credentials.

## M6 package builds

The Nix package/build outputs are documented in
[`apps/basecamp/README.md`](../apps/basecamp/README.md). The immutable M6
certificate records the exact historical builds and integration tests. The
current submission branch does not include the historical root `package.json`
or `scripts/m6-basecamp-launch-wrapper.sh`; use the integrated Docker lane above
for the runnable BTC product, or reconstruct the full historical source
checkpoint before following those two older commands verbatim.
