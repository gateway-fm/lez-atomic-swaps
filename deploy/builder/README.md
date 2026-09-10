# Ephemeral builder (native arm64)

`Dockerfile` builds `lez-builder:local`. `scripts/from-scratch.sh` runs it only
as `docker run --rm`, one step at a time, to produce the artifacts the stack
cannot pull from a registry on arm64: the pinned LEZ v0.2 services, the
digest-pinned escrow artifact (deployer and guest ELF), the LEZ sidecar and
its identity tool, and the four wallet identities. The same image runs the
one-time market bootstrap against the stack's network.

The image carries the pinned prover toolchain under `/opt/lez-tools`: `rzup`,
`cargo-risczero` and `r0vm` at the risc0 tag and the Logos rapidsnark
libraries, the same pins `from-scratch.sh` and
`scripts/verify-lez-v02-provisional.sh` name. Nothing in that layer depends on
this repository, so it is built once per pin change by
`.github/workflows/builder-image.yml`, published as
`ghcr.io/gateway-fm/lez-atomic-swaps/lez-builder`, and consumed by the digest
in `image.lock`: `from-scratch.sh` pulls that reference and tags it
`lez-builder:local`, then seeds `provision/data/tools-arm` and
`rapidsnark-arm` from it, so the r0vm and cargo-risczero builds (about two
hours cold) are skipped while every version and digest check still runs. With
no lock, no registry access, or `LEZ_BUILDER_IMAGE=local`, the image is built
here from the same Dockerfile. Changing a pin means editing the Dockerfile
and the scripts together, letting the workflow publish, and committing the new
digest to `image.lock` in the same pull request.

Nothing persists in it. Registries and build targets live in named Docker
volumes (`lez-build-*`), outputs land in the provision directory, and the only
step that ever sees the host Docker socket is the reproducible Risc0 guest
build, for the duration of that one run. It takes part in no swap.
