# Provenance — full regression of `14b05e56` (0.3.5) on macOS arm64, 2026-11-23

Everything in this folder was produced on one Mac (Apple silicon, Docker
Desktop) by the Docker devnet of `deploy/`: every test of
`scripts/regress.sh full` (106: `regress.sh list full`), the Node scenarios
by `scripts/node-e2e.py` and the desk journeys by `scripts/ui-e2e.sh`, two at
a time on two regression lanes (`LEZ_LANES=2`). One operator ran both
parties, Maker and Taker, on one host. The tested commit is `14b05e56` on
`fix/m2-0.3.5`, the candidate of 0.3.5 with the fixes for #185, #186 and
#187.

## The count

- 106 tests, 106 passed, each on its first try, in one run of 153 minutes
  (11:13:13Z to 13:46:24Z). No test was rerun. 0 frozen tries, 0
  NOT-EXERCISED.
- The run started on a fresh chain: the stack before it was taken down with
  `deploy/scripts/down.sh --wipe` and rebuilt with
  `LEZ_TEST_HOOKS=1 deploy/scripts/from-scratch.sh --evidence`. The two
  `zec-balance-refusal` tests made their own chains, as they do; one
  timing-profile switch (fast -> refund at 13:12:59Z).

## Build source

Both Node images report `14b05e56d0383dcd5bc9d39fdd1c09b681448797` in
`/usr/local/share/lez/build-source.txt`, read before the run.
`logs/regress.log` prints the commit and "tree dirty: 0" at its start.

## The test hooks

The images carry the LEZ sidecar built with the feature `test-hooks`
(`LEZ_TEST_HOOKS=1`): `zec-kill-maker-claim` holds the Maker's LEZ claim in
the sidecar with them, and fails at once without them. Release images are
built without the feature: the release workflows set `LEZ_RELEASE_BUILD`,
`from-scratch.sh` refuses `LEZ_TEST_HOOKS=1` under it, and the sidecar built
with `test-hooks` under `LEZ_RELEASE_BUILD` does not compile
(`deploy/tests/test_release_workflow.py`, `NoTestHooksInARelease`).

## Where the `zec-kill-maker-claim` output is

- `logs/node-zec-zec-kill-maker-claim-TakerSellsLez-1.log`: its run in the
  full run (lane 1, 206 s, finished 12:04:47Z): two swaps, the Maker's
  container killed with SIGKILL at "claim preparation held" and at "claim
  submission in flight", each swap completed on both Nodes with the LEZ lot
  paid to the Maker; its summary is `e2e/zec-kill-maker-claim.json`.
- `negative-proof-pre-fix-sidecar/`: the same scenario against a Maker
  sidecar built from before the #187 fix, which fails (its `README.md`).

## Privacy

Nothing here is private. The logs carry step lines, swap and transaction
ids, account ids and balances, the paths and sizes of the files
`zec-maker-early-claim` searched, and the stack's start-up lines; the ZEC
exports carry swap views and durable actor status. No key, cookie,
capability file, `runtime/` or `market/` content is included; the swap
preimage is logged only as its SHA-256. Local paths are replaced by
`<checkout>`, `<workspace>`, `<home>` and `<scratch>`. Scanned before
commit with gitleaks and for key, cookie, password and preimage patterns.

## Times

`results.tsv`, `chain.tsv` and `logs/regress.log` are in UTC; the
per-test logs of `node-e2e.py` and `ui-e2e.sh` print the host's local time
(CET, UTC+1).
