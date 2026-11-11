# Logos-native acceptance on arm64, 2026-11-11: the same walk, the linux-arm64 release set

One run of `deploy/scripts/logos-native-acceptance.sh` per role against the
release set an Apple-silicon Mac built from nothing the same day
([../macos-arm64-smoke-2026-11-11](../macos-arm64-smoke-2026-11-11/PROVENANCE.md)).
It repeats the linux-amd64 run of
[../logos-native-2026-11-11-b](../logos-native-2026-11-11-b/README.md) on the
other architecture; that README classifies every FAIL, and nothing here
changes a classification.

| File | What |
|---|---|
| `maker/`, `taker/` | the run per role: `transcript.md`, `summary.tsv`, `inputs.tsv`, `session-layout.txt`, `daemon.log`. **115 steps: 93 PASS, 14 FAIL, 7 INFO, 1 NOT RUN** each |
| `packages.tsv` | the release set (name, version, variant, dependency ranges, sha256, root hash), as `from-scratch.sh` wrote it to `deploy/images/basecamp-ui/assets/packages/` |

## Result

Every step has the verdict it has in the amd64 run, for both roles (the first
two columns of each `summary.tsv` are identical to that run's). The 14 FAIL
are the same 14: the nine "signed by a signer that can be pinned" steps
(unsigned packages), S63 (a file install resolves no dependencies), S73, S80
(`logosctl` loads no `ui_qml` package), S95 and S100. S61 is NOT RUN for the
same reason (`lm` has no `verify`).

## Inputs

- Code: PR #120's head `c1d6f813`; the packages were built by
  `from-scratch.sh` on that tree with this branch's script fixes, which touch
  no package source.
- Packages: `packages.tsv`. The same names, versions and dependency ranges as
  the amd64 set; every variant is `linux-arm64`. None is signed.
- Tools: logosctl 0.3.2 and 0.3.0, logoscore 0.2.3 (the `aarch64` release
  tarballs, checked against the sha256 pinned in the script), lm/lgx/lgpm from
  the nix store of the build (`lez-nix-store-arm`); throwaway
  `debian:trixie-slim` containers on an internal network.
- Host: Apple M1 Max, macOS 26.7.3, Docker Desktop (engine 29.9.2). The
  containers are linux/arm64.

## What this does not show

The packages are `linux-arm64` and every command ran in a Linux container. No
darwin-arm64 package exists to walk: `crates/swap-store` refuses to compile
off Linux, the Node's process hardening and the launcher's tools are Linux's,
and of the modules the role packages need only `blockchain_module` ships a
darwin variant (docs/submission-115.md, R14).

## A note on running it

The two roles were first started side by side and the Taker's run stopped at
"could not unpack logosctl 0.3.2": both unpack the same AppImage into the
shared cache (`~/.cache/lez-logos-native`). Run one role after the other, or
give each its own `LOGOS_NATIVE_CACHE`. The Taker run kept here is the rerun.

Host paths in the transcripts are replaced by `<repo>` and `<out>`; nothing
else is edited.
