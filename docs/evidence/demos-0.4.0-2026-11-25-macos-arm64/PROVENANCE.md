# Provenance: Monero demo set, 2026-11-25, macOS arm64

One operator ran both parties, Maker and Taker, on one host, an Apple-silicon Mac (linux/arm64
images), with the Docker devnet of `deploy/` and the Monero regtest chain up (`LEZ_MONERO=1`),
timing profile `fast` (C, R, P = +320, +720, +1,000 s after the take).

## Images

The stack ran the images of the full regression of `957c8016`
([regression-0.4.0-2026-11-24-macos-arm64](../regression-0.4.0-2026-11-24-macos-arm64/PROVENANCE.md)),
built there from the clean checkout of `957c8016` with `LEZ_MONERO=1 LEZ_TEST_HOOKS=1`; no image was
rebuilt. Read on 2026-11-25 before the runs:

- `/usr/local/share/lez/build-source.txt` in `lez-maker-node`, `lez-taker-node`,
  `lez-l2-maker-node` and `lez-l2-taker-node`: `957c8016a57500476ead4c12fce11a059463222f`.
- `lez-maker-node:local` `sha256:3f0376f8dfe75a5e9e21e9c4d7d36aa1c264a2f846aa5c029085d9b19aeb9507`,
  `lez-taker-node:local` `sha256:f5578d21a19333f4200e91bbddb11d956f6e71d6f816864a5bf1873fc333dbad`,
  `lez-basecamp-ui:local` `sha256:fe7dc01a1f7b873a368608d0416539bf8f9cf824ba6ddb14d3847873047ead92`:
  the ids that note lists.

## Chain

A new LEZ chain was made at 04:15Z (`up.sh --fresh-lez`, lanes 1 and 2); both runs and the
recovery example ran on it. Run 1 began at LEZ finalized height 416, run 2 ended at 3,259.

## Harness

The desk journeys ran from a checkout of `2680735c` (`test(ui): a Monero desk journey for the
Maker's punishment`), whose parent is the docs-only evidence commit `b2995dad` on top of
`957c8016`. `git diff --stat 957c8016 2680735c -- deploy/scripts deploy/ui-tests` lists
`deploy/scripts/regress.sh` and `deploy/scripts/ui-e2e.sh` only: the new journey, its entry in the
`full` tier and its expected length; `xmr-happy`, `xmr-taker-refund` and `xmr-concurrent` and the
recording code are unchanged. `ui-tests/` (`verify.mjs`, `record-step.sh`), which the Basecamps
mount, is the stack's own checkout of `957c8016`. Each `regress.log` prints "2680735c (tree dirty:
1)": the one untracked path is that checkout's link to the stack's `deploy/runtime`.

- Run 1: `LEZ_REGRESS_RECORD=1 LEZ_LANES=2 scripts/regress.sh run ui:xmr:xmr-punish:TakerSellsLez
  ui:xmr:xmr-taker-refund:TakerSellsLez ui:xmr:xmr-happy:TakerSellsLez
  ui:xmr:xmr-concurrent:TakerSellsLez`, 04:26:15Z to 04:50:33Z.
- Run 2: the same four tests with `LEZ_LANES=1` and `LEZ_STACK_COMPOSE_ARGS` naming the stack's
  deploy directory and `runtime.env`, 04:50:41Z to 05:33:51Z.

## Recovery example

`cargo run --locked -p xmr-reference-actor --example full-lifecycle` at `2680735c` (the example is
unchanged since `957c8016`), built on the host (`dev` profile), run from 05:34Z against lane 1's
`monerod` (127.0.0.1:18081) and funding wallet-rpc (127.0.0.1:18083) with the lane's Digest logins,
defaults otherwise (`LEZ_XMR_EXAMPLE_MONERO=live`, `LEZ_XMR_EXAMPLE_NETWORK=regtest`, seed
`full-lifecycle`).

## Files

- `README.md`: the index of the recordings.
- `run-1/`, `run-2/`: each run's `regress.log`, `results.tsv`, `chain.tsv` and test logs, colour
  codes removed.
- `recovery/`: the example's output (`full-lifecycle-live.txt`, the exit status line of the run's
  wrapper removed), its timed record (`full-lifecycle-live.cast`) and the two scripts that recorded
  and rendered it.
- `videos-kept-outside-git.sha256`: the SHA-256 of the five videos, which are not in git.
- `SHA256SUMS`: every other file of this folder.
