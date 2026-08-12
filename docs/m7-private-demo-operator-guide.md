# M7 private demo operator guide

This guide reproduces the six M7 XMR/ZEC presentation videos and their
verification bundle. Together with the three certified BTC videos documented
in the M3 operator guide, they satisfy D1's nine-video inventory.

The reference M7 bundle was rendered and verified from exact pushed commit
`1b47a158140c3336f8d5e0ac2e2b3e7a8ce12876`. Its secret-free certificate
is [m7-private-demo-video-bundle-1b47a15-20260812.json](evidence/m7-private-demo-video-bundle-1b47a15-20260812.json).
The MP4s, proofs, and manifests remain private beneath `.e2e/`.

## What this repeats

| Pair | Scenario | Bound functional source |
|---|---|---|
| XMR | Happy | Joined actual-node Taker claim and Monero sweep |
| XMR | Refund | Joined actual-node Maker refund with process-kill recovery |
| XMR | Concurrent | Two joined actual-node applications through one Maker daemon |
| ZEC | Happy | Joined actual-node accepted application with process-kill recovery |
| ZEC | Refund | Joined actual-node reverse first-lock refund after Maker absence |
| ZEC | Concurrent | Real one-daemon actor overlap plus separately bound actual-node Claim and Refund effects |

The last row is intentionally layered. It does not claim that two ZEC swaps
were run concurrently against Zebra in one joined recording.

## Prerequisites

Use a Linux checkout with Git, Bash, Cargo, jq, sha256sum, stat, and Docker.
The checked source proofs need the existing Rust build cache or the normal
locked Cargo dependencies. Rendering needs this exact VHS image:

```bash
export M7_VHS_IMAGE='ghcr.io/charmbracelet/vhs@sha256:9d5fc3dc0c160b0fb1d2212baff07e6bdf3fa9438c504a3237484567302fcf93'
docker pull "$M7_VHS_IMAGE"
```

The image pull is the only required external call. Registry, DNS, or TLS
availability can make a cold pull flaky. Rendering itself uses a networkless,
read-only, capability-dropped container with bounded CPU, memory, and PIDs.
No public chain RPC, peer, faucet, funds, or deployment participates.

## Verify the retained certificate

From any current checkout:

```bash
./scripts/test-m7-private-demo-video-actual-certificate.sh
```

This regenerates all six source proofs and checks the retained video hashes,
durations, sizes, commit, source map, evidence model, and resource disclosure.
It does not need Docker or start chain nodes.

## Re-render all six videos

Use an exact clean checkout of the reference renderer commit. Do this in a
worktree where switching commits cannot disturb other work:

```bash
git switch --detach 1b47a158140c3336f8d5e0ac2e2b3e7a8ce12876
test -z "$(git status --porcelain=v1 --untracked-files=normal)"
export M7_VIDEO_ROOT="$PWD/.e2e/m7-private-demo-videos-1b47a15-manual"
for pair in xmr zec; do
  for scenario in happy refund concurrent; do
    ./scripts/verify-m7-private-demo-source.sh "$pair" "$scenario"
    ./scripts/render-m7-private-demo-video.sh "$pair" "$scenario" "$M7_VIDEO_ROOT"
  done
done
```

Every scenario directory is mode `0700`. Its `proof.json`,
`walkthrough.txt`, `demo.sh`, `demo.tape`, `demo.mp4`,
and `video.json` are mode `0600`. Do not publish the private tree.

Seal and fully decode-probe the six videos:

```bash
export M7_VIDEO_BUNDLE="$M7_VIDEO_ROOT/video-bundle.json"
M7_PRIVATE_DEMO_VIDEO_BUNDLE_OUTPUT="$M7_VIDEO_BUNDLE" ./scripts/verify-m7-private-demo-video-bundle.sh "$M7_VIDEO_ROOT/xmr/happy/video.json" "$M7_VIDEO_ROOT/xmr/refund/video.json" "$M7_VIDEO_ROOT/xmr/concurrent/video.json" "$M7_VIDEO_ROOT/zec/happy/video.json" "$M7_VIDEO_ROOT/zec/refund/video.json" "$M7_VIDEO_ROOT/zec/concurrent/video.json"
jq '{result,source_repository_commit,videos,zec_concurrent_joined_actual_node_run}' "$M7_VIDEO_BUNDLE"
```

The verifier rejects dirty or different live source, duplicate or missing
scenarios, changed evidence, changed proofs, unsafe modes, artifact tampering,
non-MP4 output, non-H.264 video, wrong dimensions, and decode failure. A fresh
render may have different encoded bytes; its own manifest and bundle bind the
exact output. The retained reference bundle SHA-256 is
`a23b7d32b11ce91a44875750c82bccc470659cac380de90d61c9e7b2e743bf5b`.

Open the six `demo.mp4` files with any local MP4 player. The videos show
role order, local nodes, terminal result, and the conditional-atomicity
boundary; they are a presentation of checked evidence rather than a new chain
execution.

## Repeat the underlying user journeys

To start fresh local nodes and repeat the actual role flows rather than only
re-render them, follow these existing procedures:

- XMR happy: [Flow 1R](manual-user-flows.md#flow-1r-run-the-xmr-application-to-chain-corridor).
- XMR refund: [Flow 1W](manual-user-flows.md#flow-1w-run-the-role-correct-xmr-application-refund-locally).
- XMR concurrent: [Flow M7-XMR-2](manual-user-flows.md#flow-m7-xmr-2-two-xmr-application-workers-across-daemon-restart).
- ZEC happy: [Flow 1ZK](manual-user-flows.md#flow-1zk-recover-an-accepted-zec-application-after-process-kill).
- ZEC refund: [Flow M7-ZEC-1](manual-user-flows.md#flow-m7-zec-1-reverse-first-lock-refund-after-maker-absence).
- ZEC concurrent scheduler layer: run
  `./scripts/test-m7-zec-concurrent-demo-baseline.sh`; its actual-node
  Claim and Refund layers are the two preceding ZEC procedures.

Those runners allocate unique run IDs, dynamic loopback ports, isolated Docker
networks, and deterministic local genesis/Regtest funds. Cold image or
dependency downloads, host CPU/disk pressure, Docker readiness, local finality
lag, and port exhaustion can cause setup delay. Public-chain finality,
third-party RPC quotas, faucet availability, and public peers cannot affect
the certified flows. Always use each runner's exact scoped cleanup; never
broad-prune while unrelated workloads are active.
