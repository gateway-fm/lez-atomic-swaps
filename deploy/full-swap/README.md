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
Docker socket. The runner-work root is mounted at the same absolute path on
both sides because nested Docker bind mounts are resolved by the host daemon.
From a workspace containing `submission/`, `runner-work/`, and provisioned
`provision/data/` directories:

```sh
workspace_root="$(pwd -P)"
runner_work_root="$workspace_root/runner-work"
docker build -t lez-runner-arm:latest \
  -f submission/deploy/full-swap/runner-arm.Dockerfile submission/deploy
docker run -d --name lez-runner-arm --network host \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -v "$runner_work_root:$runner_work_root" \
  -v "$workspace_root/provision/data:/provision" \
  lez-runner-arm:latest sleep infinity
# After provisioning the pinned LEZ source, arm64 rapidsnark libraries, and
# risc0 v3.0.5 r0vm in those roots:
docker cp submission/deploy/full-swap/run-full-swap.sh \
  lez-runner-arm:/tmp/lez-run-full-swap.sh
docker exec \
  -e LEZ_M3_RUNNER_REPO_IN_CONTAINER="$runner_work_root/repo" \
  lez-runner-arm bash /tmp/lez-run-full-swap.sh
```

`run-full-swap.sh` builds the escrow artifact through the repository's own
`verify-lez-v02-provisional.sh` (digest-enforced) and then drives
`run-m3-actor-local-poc.sh` in M5 application mode.

## The local patch series

`patches/` contains the 38 commits applied on top of `main` (`5c384a5`) that
make the pinned verification lane work on an Apple-Silicon host:

1. **arm64 pins** (0001–0007): native rebuild hashes for the LEZ services,
   r0vm, rapidsnark libs, and the official Core 31.1 aarch64 archive (hash
   from the same signed SHA256SUMS / Guix attestation set); a
   `LEZ_NATIVE_TOOLS` lane with source-built `cargo-risczero` (upstream ships
   no aarch64-linux release assets) and an aarch64 circuits release.
2. **host quirks** (0008–0016): per-run `/tmp` GPG home (virtiofs cannot
   host gpg-agent locks), container creation through compose so published
   ports use `mode: host` (this engine's docker-proxy ports are unreachable
   from `--network host` processes), and an exec-wait for the 344 MB
   mount-backed binaries before the `/proc/<pid>/exe` drift check.
3. **prepared ZEC UI corridor** (0017–0023): deterministic fixtures, isolated
   Taker authority, and offer-TTL handling for the optional prepared-service
   exercise.
4. **long-standing-chain attach mode** (0024–0026): injectable chain run IDs,
   persistent wallet identities, idempotent bootstrap reuse, cumulative
   opening balances, and shared-chain Bitcoin confirmation handling.
5. **bidirectional application runs** (0027–0034): direction selection,
   direction-aware funding sources, replay assertions, and evidence maps for
   both bounded LEZ/BTC economic routes.
6. **independent negotiation** (0035–0037): independently signed role
   contributions, real Logos Chat transport, and signed Delivery offer
   discovery with deterministic one-winner conflict resolution.
7. **publication dependency closure** (0038): all seven independent locked
   HTTP/2 dependency graphs are advanced to the advisory-fixed patch release
   and refreshed with Cargo 1.96. That refresh also repairs missing
   already-declared path dependencies in the stale isolated XMR lock and
   normalizes target-specific `windows-sys`/`socket2` selections; no manifest
   or application-code dependency changes are included.

Guest ELF/ImageID digests and all on-chain assertions remain exactly as
upstream pinned them.

## Surfacing the swap in the UI

Export the public, secret-free evidence from a completed run:

```sh
deploy/full-swap/export-ui-evidence.sh \
  .e2e/<run>/m3-actor-poc/evidence \
  deploy/runtime/m3-btc-ui-evidence.json
```

The exporter fails closed unless the application result passed, the pair is
Bitcoin, the direction is `taker_sells_foreign`, terminal revision 4 is
`completed`, the exact effect cardinality is two Bitcoin plus three LEZ, and no
private material was disclosed. It emits the five transaction/block identities
and no keys or preimages.

From `deploy/`, `scripts/prepare-btc-m3-demo.sh --from-run <evidence-dir>`
exports and publishes in one command. With the provisioned runner,
`scripts/prepare-btc-m3-demo.sh --rerun` first creates a fresh real run. In
Basecamp (VNC :5901), open **LEZ / BTC Maker** to publish wallet-owned offers
and **LEZ / BTC Taker** to take them. The same runner is gated at the four
role-owned actor steps: Taker Bitcoin lock, Maker LEZ funding, Taker LEZ claim,
and Maker Bitcoin claim. On completion, the mounted evidence is replaced with
the five proofs plus per-wallet opening/closing balance reconciliation. The
same evidence is available at
`http://127.0.0.1:3003/#/evidence`.
