# Build and bootstrap runner (native arm64)

`runner-arm.Dockerfile` builds `lez-runner-arm`, the container
`scripts/from-scratch.sh` provisions with the pinned LEZ v0.2 services, r0vm,
rapidsnark and the escrow artifact, and in which `scripts/market-bootstrap.sh`
deploys the escrow program and funds the four wallet identities once.

It takes part in no swap. Since ADR 0213 the Maker and Taker Nodes settle
BTC↔LEZ swaps themselves; the runner is not on the Compose stack's path and
holds no authority over it.
