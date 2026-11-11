# Provenance — release-gate checks of main on an Apple-silicon Mac

- Host: Apple M1 Max (10 cores, 64 GB), macOS 26.7.3, Docker Desktop with
  engine 29.9.2, Homebrew bash 5.3 and coreutils.
- Code: `main` at `af3ac978` plus the two fix commits of this branch,
  `2ab39a39` and `cbeba3a2`. Sections 1 to 4 of `RESULTS.txt` ran at
  `2ab39a39`; section 5's passing run ran on the tree `cbeba3a2` commits (its
  four files were still uncommitted then; `start-path/summary-start.log`
  names them).
- Images: every `lez-*:local` image was rebuilt from `af3ac978` by
  `deploy/scripts/from-scratch.sh` at 21:00Z–21:03Z, all `linux/arm64`. For
  section 5 `lez-wallet-module:local` was rebuilt at 21:52Z from the
  Dockerfile `cbeba3a2` commits (it adds `libpcsclite1`).
- Stack: the local devnet (LEZ v0.2.4, Bitcoin regtest, Zebra regtest), one
  lane. Sections 1 to 3 ran on one LEZ chain; section 4 on a second; section
  5's passing run made two more, one per `start.sh`.
- Section 5 is the path of a release bundle's user, on local images:
  `start.sh` mints the six wallet identities with the tools service, starts
  the stack through `up.sh` without building, bootstraps the market, runs the
  desk suites and `verify-all.sh`. Its market root is `deploy/market`.
- Contents: `RESULTS.txt`; `gates/` (sections 1, 3 and 4), `smoke/` (the two
  runs of section 2, as `regress.sh` printed them: their per-test logs went
  with the runtime directory the wipe of section 4 removed), `start-path/`
  (section 5's passing run). Host paths are replaced by `<repo>`,
  `<workspace>` and `<out>` and terminal colour codes are removed; nothing
  else is edited. Public chain data only; no keys, cookies or capability
  files.

## What this does not show

- Nothing ran on Linux. Three of the four start.sh faults are not about the
  platform, so they are expected there too, but that is not shown here.
- The images are local builds, not a release's: the published bundle path
  (pull, then `start.sh`) is the release workflow's smoke job.
- The three smoke failures of section 2's first run are not explained.
- This is not the regression: six smoke tests, one finality test, four desk
  trades.
