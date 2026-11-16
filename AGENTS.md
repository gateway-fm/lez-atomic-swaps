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
- Upstream components keep their own versions. Protocol, schema and profile
  identifiers are not release versions.

## Pull requests and issues

- A pull request description says what the change does, what was run and
  what was not. It carries no questions for the owner and no "to confirm"
  section; ask in the conversation or in a review comment.
- Close an issue when it is fixed on `main` with evidence; comment and leave
  it open when it is fixed in part.
