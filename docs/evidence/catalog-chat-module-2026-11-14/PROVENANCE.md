# Provenance — the catalog's Chat module, 2026-11-14

Everything here ran on one Apple-silicon Mac (macOS 26) and, for Linux, in an
arm64 container on it (Docker Desktop). Local paths are replaced by
`<checkout>` (the repository), `<scratch>` (the run's working directory,
`<scratch>/previous` an earlier run's), `<workspace>` (the devnet's deploy
directory), `<home>` and `<tmp>`; in `scripts/` they are `${SCRATCH}`,
`${CHECKOUT}`, `${WORKSPACE}` and `$HOME`. No key, cookie, wallet or identity file is
included. Scanned before commit with gitleaks (the repository's
`.gitleaks.toml`): no leaks found.

## Source

The scripts are those of `477bcb06` (the commit this one documents): `deploy/scripts/package-chat-module.sh`
and `deploy/scripts/pin-package-dependencies.sh` as committed, run from the
checkout. The run's helpers are in `scripts/` (the Linux image's Dockerfile
as `scripts/linux.Dockerfile`).

The desks, `lez_core`, `lez_swap_node`, `lez_indexer_module`, `lez_network`
and both role packages are the merged v0.3.1 set (variants `darwin-arm64`,
`linux-amd64`, `linux-arm64`), unsigned, the files that were signed and
published as v0.3.1; their dependencies were pinned again by this commit's
script. This commit changes nothing the packages are built from except a
comment in `apps/basecamp/common/lez_role_node.h`, so no package was
rebuilt. The desks therefore read 0.3.1. `chat_module` 0.2.2-delivery.1 is
the script's output from the release asset
`chat_module-v0.2.2/chat_module-0.2.2.lgx` (sha256
`ab55abc49771f9bc52d689c463b22228241ffdb98150cfc3d45071d5d373dac1`).
`delivery_module` 0.2.0 and `blockchain_module` 0.2.4 entered the set only
for their versions; their release assets matched the official index's
sha256.

## Catalog

`catalog/`: a local HTTPS catalog (`https://localhost:8473`, a test CA the
clients trusted through `SSL_CERT_FILE`) with the two desks, `lez_core`,
`lez_swap_node`, both role packages and `chat_module`, its `index.json`
written by the Logos catalog's `index.py`, `trustedSigners` empty (the
packages are unsigned). It was added beside the default official catalog in
every session. In the Linux container `localhost:8473` was forwarded to the
host. `probe/` used a second one on port 8474.

## Tools

| tool | version |
|---|---|
| logosctl | 0.3.2, commit `96e704df` (macOS aarch64 release; Linux aarch64 AppImage, `scripts/appimages.sha256`) |
| Basecamp | 0.3.2 portable, `logos-basecamp` `f2fae630` (macOS app; Linux aarch64 AppImage `LogosBasecamp-Desktop-v0.3.2-f2fae6`) |
| lgx | logos-package `4bdd904` (`lgx verify`); the release workflow's `ab885df3` was not run |
| Linux | Debian trixie arm64 (`scripts/linux.Dockerfile`), Xvfb 1600×1000, xdotool |

The Linux installs of `install/linux-arm64-maker.log` and `-taker.log` ran in
a first container of that image; the Basecamp runs, and the installs they
used (`install/linux-arm64-*-for-basecamp.log`), in a second one after
`libharfbuzz0b` and `libfribidi0`, which the Basecamp AppImage needs, were
added to the image. logosctl is the same in both.

On macOS, Basecamp was driven through System Events: the sidebar's desk
buttons by their accessibility action, the Applications page's tile,
Install and Launch by mouse events at the buttons' positions
(`scripts/ax-mouse-click.js`); `screencapture` is not permitted on that Mac,
so the macOS evidence is the log and accessibility text, not screenshots.
For the Applications page on macOS, the repository was added by writing the
package downloader's `repositories.json` between a first start and the run
(what Settings → Package Repositories writes); on Linux it was added in
Settings by typing the URL, and "HTTP only" chosen there.

## What ran

- The cut Chat built twice from the release asset, compared member by
  member, `lgx verify` (`chat-module/build.log`).
- `deploy/tests/` (unittest, all modules), `scripts/check-m6-basecamp-package-contract.mjs`
  (also against two deliberate breakages, which it reported), shellcheck
  0.11.0 on the two scripts, actionlint 1.7.12 on `release-packages.yml`,
  and the release workflow's verify checks (`jq`) on the pinned set, which
  pass and refuse the v0.3.1 desk.
- One install per desk in a fresh logosctl session: macOS, Linux arm64.
- The desk opened in Basecamp 0.3.2 after one install, by logosctl and by the
  Applications page, each desk, macOS and Linux arm64.
- The pre-release probe and the `chat_ui` sequences (logosctl, macOS).
- The images' set before and after (`image-set.txt`).

## What did not run

- The release workflow (`release-packages.yml`) on GitHub's runners.
- linux-amd64: the set carries the variant; no install ran on x86_64.
- Signed packages: everything here is unsigned.
- The Compose stack: its images were not rebuilt; `image-set.txt` shows their
  input set is unchanged.
- Swaps: not run yet. The devnet's chains on this Mac were in use by another
  run (a reverse tunnel to a remote host) for the whole session, checked
  from 11:13Z to 12:45Z.
