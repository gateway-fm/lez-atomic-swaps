# zec-kill-maker-claim against a sidecar from before the #187 fix

Not a run of the release code. It shows that `zec-kill-maker-claim` fails
without the fix it tests.

- The Maker's LEZ sidecar (`lez-v02-bridge-poc`) was built from `a77e9762`,
  the commit before the #187 fix on `fix/m2-0.3.5`, with only the test-hook
  code of `64e804ce` added (`hold-claim-preparations`,
  `hold-claim-submissions`) and `--features test-hooks`; linux-arm64 debug
  build, sha256
  `cc22c7ee950091c3aa0aab2389df006e6235a07f33cb9988d0a5bb287999c171`.
- It replaced the sidecar in the Maker Node's image, which was otherwise the
  one built from `7e9e4c3b` (its build-source stamp); the Taker's image and
  the harness (`14b05e56`, as `regress.log` prints) were unchanged. The
  original image was put back afterwards.
- Run on 2026-11-23 from 10:44:55Z, one try
  (`node-zec-zec-kill-maker-claim-TakerSellsLez-1.log`, local times CET):
  "claim preparation held" completed. At "claim submission in flight" the
  Maker was killed with its LEZ follow-up claim in flight; after the restart
  the swap did not complete, and the scenario failed after its 900 s wait
  with the Taker at `claim_in_progress` and the Maker at
  `claim_evidence_available`.
- Afterwards the Maker sidecar's request journal
  (`bridge-requests.v1.json`) still held the claim's submission as
  `submission_in_flight` / `unknown_submission_outcome`, with no later
  decision: the claim was never sent again.
