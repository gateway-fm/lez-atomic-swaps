# Smoke runs before the full run, 2026-11-23

`summary-raw.txt` is the runner's one line per test (result, wall seconds
including setup, Docker free disk, try). The first lines are from a run on
images of `7e9e4c3b` (same code as `0189c2e8`); three of them failed, none
in product code:

- `zec-slow-funding`, first run: the chain had no escrow program. The M4
  stack's volumes had been removed by hand instead of with `down.sh --wipe`,
  so the market root kept a deployment record whose genesis hash equals the
  new chain's (the devnet genesis is deterministic), and the bootstrap took
  the escrow as deployed. `up.sh --fresh-lez` retired it and deployed again.
- `zec-slow-funding`, second run: the scenario held the funding for a fixed
  75 s and the Maker's journal reached revision 21 of the 24 it wants on this
  host; fixed in `14b05e56` (it holds until the journal is past the minimum,
  at most 240 s).
- `btc-custody-credit-refund`, first run: `regress.sh` switched to the refund
  profile with an `up.sh` that had no `LEZ_WORKSPACE`/`LEZ_PROVISION` and
  found no escrow deployer.

The last four lines are the reruns, each passed on its first try. The full
run in this folder supersedes all of them.
