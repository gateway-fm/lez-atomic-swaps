# Runtime build isolation

The fund-owning runtime is intentionally Linux-only because its secure process
and file contracts require `memfd` seals and `openat2`. On macOS or Windows,
run the supported container check:

```sh
./scripts/check-linux-runtime.sh
```

`lez-runtime-healthcheck` is independently distributed and therefore lives in
its own dependency-light package. Role-neutral RPC, configuration, secure-file,
Delivery, and Chat contracts live in `lez-node-common`. Maker authority and
executables live only in `lez-maker-node`; Taker authority and executables live
only in `lez-taker-node`. The corresponding Docker build contexts are likewise
role-exclusive, so compiling or packaging one role cannot pull in the other
role's executable surface.

On a Linux development host, collect isolated clean-target measurements with:

```sh
./scripts/measure-runtime-builds.sh
```

The TSV output records each canonical Node, CLI, and Chat target against its
own package and clean target directory. This keeps changes in one role
measurable without conflating them with the other role's build graph.
