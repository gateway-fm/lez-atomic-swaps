# AGENTS.md

Rules for coding agents working in this repository. `CONTRIBUTING.md` applies
as well (sign-off, tests with every behavioural change).

## What does not go into the tree

- **Nothing that exists only once a release is made.** Image digests, the
  names of attached files, what the module catalog holds: these go on the
  GitHub release page, not into a file here.
- **No release notes.** Release notes live on the GitHub release page, not
  in a file here.
- **No placeholders.** Do not commit `TBD`, "to be filled" or an empty table
  cell for a result that is not in. Say in a plain sentence what has run and
  what has not ("not run yet"), and replace it when the result exists.
- **No statements about a branch or a pull request as the present.** Docs
  describe `main`. "This branch", "this PR", "not merged yet" belong in the
  pull request description.

## Protocol design

- **Swap flows are the same for every pair.** For a given direction (Taker
  sells LEZ, or Taker sells the foreign asset), who locks first, who holds the
  secret, who claims first and which refund matures first are identical for
  BTC, ZEC, XMR and any later pair. A pair may differ only in the
  cryptographic mechanism that carries the secret (adaptor signature, hash
  preimage, key share). A pair that cannot follow the common flow in a
  direction does not offer that direction. XMR `TakerSellsLez` is the one
  accepted exception
  ([ADR 0217](docs/architecture/0217-xmr-taker-sells-lez-deviates-from-the-common-flow.md)):
  a Monero output has no hash lock and no time lock, so there the Taker
  authorises, the Maker claims LEZ first and reveals its Monero share, and
  the Taker's LEZ refund, open in [R, P), is the only refund; XMR offers no
  `TakerSellsForeign`. No other pair or direction may deviate without its own
  accepted ADR.

## Evidence

- Evidence of a commit is committed on top of it, in a docs-only commit that
  names the tested commit. A tag may sit on that docs-only commit; the
  release text says which commit the runs were made on.
- A run counts for a commit only if the Node images report that commit as
  their build source (`/usr/local/share/lez/build-source.txt`). Check it
  before the run and record it in `PROVENANCE.md`.
- Captured logs under `docs/evidence/` are kept as the tools printed them
  (`.gitattributes` exempts them from the whitespace check). Authored
  markdown is not exempt. Replace local paths, and scan for secrets, before
  committing.

## Versions

- Every component this project ships carries the release version: both
  desks, `lez_network`, `lez_swap_node`, the role packages and the sidecar,
  with the `.lidl` files that name them. Bump them together.
- The first pull request that changes something after a release carries the
  bump to the next version; there is no separate version pull request.
- Upstream components keep their own versions. Protocol, schema and profile
  identifiers are not release versions.

## Checks, merges and tags

- Run `scripts/check-all.sh` before every push. It runs the format, Clippy
  and rustdoc checks of every Cargo workspace with CI's flags; `--fast`
  covers formatting everywhere and the root workspace's lints.
- Before a push that changes code, also run what CI runs beyond the lints:
  the root workspace tests (`cargo test --locked --workspace --all-targets`,
  no excludes, as `ci.yml` runs them), the Node
  suites with `--features pair-zec` and with `--all-features`,
  `python3 -m unittest discover -s deploy/tests`,
  `scripts/test-check-all-workspaces.sh`, `scripts/test-ci-hardening-policy.sh`
  and `scripts/check-runtime-profiles.py`, and, when the Zcash adapter,
  SDK or contract changed, `scripts/run-zebra-e2e.sh` in its own project.
  When docs changed, run `scripts/check-requirements-traceability.sh`,
  `scripts/check-architecture-diagrams.sh` and `scripts/check-threat-model.py`.
  A check that cannot run on the machine (a Linux-only target, a job that
  needs a container the run may not start) is named in the pull request, not
  skipped in silence.
- A pull request is merged only when all its checks have finished green. A
  check that is still running is not green.
- A tag is cut only on a commit whose CI run has finished green; the release
  workflows refuse any other. A published tag is never moved: a further
  change gets a new version.

## Pull requests and issues

- The evidence of a full regression is committed to the pull request the
  regression ran on, not to a pull request of its own.
- A pull request description says what the change does, what was run and
  what was not. It carries no questions for the owner and no "to confirm"
  section; ask in the conversation or in a review comment.
- Close an issue when it is fixed on `main` with evidence; comment and leave
  it open when it is fixed in part.
