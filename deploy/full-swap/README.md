# Full swap flow (native arm64)

The complete M5 application swap — real maker daemon, real CLIs, Bitcoin Core
31.1 regtest, LEZ v0.2 devnet, pinned Risc0 escrow artifacts — executed
end-to-end and then surfaced in the Basecamp UI.

## What a successful run proves

`evidence-m5arm-08180005.json` (run `m5arm-08180005`, 2026-08-18):

- direction `taker_sells_foreign`, terminal revision **4 `completed`**
- exactly **1** maker second-lock effect; unique on-chain effects
  `{bitcoin: 2, lez: 3}` (real transactions on both local chains)
- Bitcoin Core **31.1** / LEZ **v0.2.0**, zero public RPC / faucet usage
- replay resubmission count **0**; cleanup attestation **passed**
- escrow deployed through the checked guest artifact whose ELF digest and
  ImageID match the repository pins (`b7f8727…`)

## How to reproduce

Everything runs inside a native arm64 runner container against the host's
docker socket:

```sh
docker build -t lez-runner-arm:latest full-swap/runner-arm.Dockerfile   # see file for context path
docker run -d --name lez-runner-arm --network host \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -v <this-repo-checkout>:/repo \
  lez-runner-arm:latest sleep infinity
# provision pinned prerequisites (LEZ v0.2 source at /tmp paths, arm64
# rapidsnark libs, native r0vm from the risc0 v3.0.5 tag), then:
docker exec lez-runner-arm bash full-swap/run-full-swap.sh
```

`run-full-swap.sh` builds the escrow artifact through the repository's own
`verify-lez-v02-provisional.sh` (digest-enforced) and then drives
`run-m3-actor-local-poc.sh` in M5 application mode.

## The local patch series

`patches/` contains the 16 commits applied on top of `main` (5c384a5) that
make the pinned verification lane work on an Apple-Silicon host:

1. **arm64 pins** (0001–0007): native rebuild hashes for the LEZ services,
   r0vm, rapidsnark libs, and the official Core 31.1 aarch64 archive (hash
   from the same signed SHA256SUMS / Guix attestation set); a
   `LEZ_NATIVE_TOOLS` lane with source-built `cargo-risczero` (upstream ships
   no aarch64-linux release assets) and an aarch64 circuits release.
2. **host quirks** (0008–0016): per-run `/tmp` gpg homedir (virtiofs cannot
   host gpg-agent locks), container creation through compose so published
   ports use `mode: host` (this engine's docker-proxy ports are unreachable
   from `--network host` processes), and an exec-wait for the 344 MB
   mount-backed binaries before the `/proc/<pid>/exe` drift check.

Guest ELF/ImageID digests and all on-chain assertions remain exactly as
upstream pinned them.

## Surfacing the swap in the UI

After a successful run, the maker database contains the real swap:

```sh
sqlite3 .e2e/<run>/m3-actor-poc/private/directions/taker_sells_foreign/application/maker.sqlite3 \
  'select id, revision from swaps;'
```

Checkpoint it, drop it into the compose stack's maker volume (uid 4713), and
restart `maker-node`. In Basecamp (VNC :5901): **Refresh swap history** lists
the swap; entering its id and pressing **Monitor** returns the live actor
projection (`actor=bitcoin`). `deploy/ui-tests/verify.mjs maker` asserts the
whole journey end-to-end.
