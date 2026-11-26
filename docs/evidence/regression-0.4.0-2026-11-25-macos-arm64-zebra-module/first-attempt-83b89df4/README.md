# The stopped first attempt at `83b89df4`, 2026-11-25

This is the first run of `regress.sh full` for this set, at `83b89df4` (180 tests). It ran on
four lanes, with Monero regtest up and the devnet test hooks built in, from 20:06:17Z. It was
stopped at a test boundary at about 21:59Z, once a defect in the tested code had shown itself.
These are its files as the runner wrote them (`runtime/regress/20261125T200616Z`), with local
paths replaced. Its results are not counted in the run of `23e54486` that the set's README
reports.

## What it did

- 33 tests passed, each on its first try. 29 are in `results.tsv`. The other 4 finished after
  the runner's scheduler was paused, so the runner never wrote their rows; they are in
  `finished-after-pause.tsv`:
  - `node:xmr:xmr-custody-credit:TakerSellsLez`, 238 s
  - `node:zec:zec-bitcoin-down:TakerSellsForeign`, 269 s
  - `ui:xmr:xmr-concurrent:TakerSellsLez`, 321 s
  - `node:zec:zec-kill-maker:TakerSellsForeign`, 338 s

  Their logs are under `logs/`.
- 1 test failed: `node:zec:zec-maker-early-claim:TakerSellsLez` (lane 4, 14 s, 21:32:56Z). It
  failed before any swap, at step 3/8 of `zec-catalog-fixture.sh` ("provision"), with this
  error:

  ```
  Error: provisioned actor pair is not isolated
  Caused by: actor pair isolation is invalid
  zec-local-poc-provision failed for swap 0
  ```

  This is a defect of `83b89df4`, in `zec-reference-actor`. The provisioner names Zebra's live
  cookie, where Zebra writes it, for both roles (`local_poc.rs`, since `13dd690b`). The cookie
  is one of each role's local path bindings (`config.rs`, `PathBindings::local`). The pair
  check (`validate_actor_pair`, `actor_pair_invariants_differ`) then rejects any Maker and
  Taker whose local bindings share a file. So no dealer-provisioned corridor could pass:
  `zec-maker-early-claim`, `zec-tampered-catalog` and `zec-regenerated-catalog`. `23e54486`
  fixes the rule.
- 146 tests did not run. No try froze and no other test failed.

## Why it was stopped

To save time once the defect was found. A fix was ready, and it changes `zec-reference-actor`,
which every ZEC test uses, so the whole tier had to run again on the fixed head. These steps
stopped the run:

1. At about 21:54Z the runner's scheduler process was paused (`SIGSTOP`), so it started no new
   test.
2. The four tests then running finished and wrote their verdicts (all PASSED).
3. The scheduler and its finality watcher were then killed.

No test was cut short.

## Bringing up the four lanes

This happened before the run.

- **Address pools.** The first bring-up of lanes 3 and 4 failed with "all predefined address
  pools have been fully subnetted". The host held 119 bridge networks, 97 of them other
  projects' stopped stacks. Lanes 3 and 4 came up once two unused networks of this project had
  been removed. This is the environment, not a defect.
- **Lane 2.** Lane 2 failed its first start inside `up.sh --fresh-lez` with `LEZ_LANES=4`. Afterwards its
  Node containers stood "Created", never started, and `runtime/lanes/2/lez/maker/identity.json`,
  `runtime/lanes/2/lez/taker/identity.json` and `runtime/lanes/2/lez/maker/zcash.key` were
  directories. This is what Docker makes at a bind source that does not exist when a container
  is created, so the containers were most likely created before those files were written. `gen-config.sh` then copied each
  identity into the directory (`identity.json/identity.json`), its `chmod 0644` took the
  directories' execute bit, and its `install` of the Zcash key failed with "Inappropriate file
  type or format". `lanes.sh down 4 --wipe` could not delete those directories, and the next
  `lanes.sh up 2` failed with "cp: …/identity.json: Permission denied". It was recovered by
  `chmod -R u+rwx` on the lane's runtime directory, removing that directory, and `lanes.sh up 2`
  (healthy). It happened together with the address-pool failure and not again.

## Files

`results.tsv`, `regress.log`, `chain.tsv`, `logs/` (each try's log and the lane logs), `e2e/`,
`zec/` and `lanes/` are as the runner wrote them. `finished-after-pause.tsv` was written by the
operator from the four lanes' verdict files. The Node logs the runner kept for the failed try
(`logs/node-zec-zec-maker-early-claim-TakerSellsLez-1.nodes/`) are not committed. They are kept
outside git; the SHA-256 of their archive is in the set's `OUTSIDE-GIT.sha256`.
