# Ephemeral builder (native arm64)

`Dockerfile` builds `lez-builder:local`. `scripts/from-scratch.sh` runs it only
as `docker run --rm`, one step at a time, to produce the artifacts the stack
cannot pull from a registry on arm64: the pinned LEZ v0.2 services and `r0vm`,
the digest-pinned escrow artifact (deployer and guest ELF), the LEZ sidecar and
its identity tool, and the four wallet identities. The same image runs the
one-time market bootstrap against the stack's network.

Nothing persists in it. Registries and build targets live in named Docker
volumes (`lez-build-*`), outputs land in the provision directory, and the only
step that ever sees the host Docker socket is the reproducible Risc0 guest
build, for the duration of that one run. It takes part in no swap.
