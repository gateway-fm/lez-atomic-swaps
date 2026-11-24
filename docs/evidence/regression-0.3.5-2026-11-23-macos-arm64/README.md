# Full regression of 0.3.5 (`14b05e56`), macOS arm64, 2026-11-23

106 of 106 tests of `regress.sh full`, each on its first try, 153 minutes
(11:13Z to 13:46Z UTC) on two lanes, from a fresh chain, on images whose
build-source stamp reads `14b05e56`. The images carry the sidecar's
`test-hooks` feature, which `zec-kill-maker-claim` needs; release images are
built without it, and a release build with it does not compile.

- `RESULTS.txt`: the count and the new tests of 0.3.5.
- `PROVENANCE.md`: build source, hooks, privacy, times.
- `logs/node-zec-zec-kill-maker-claim-TakerSellsLez-1.log`: the
  `zec-kill-maker-claim` run (issue #187), kept with the 0.3.5 record.
- `negative-proof-pre-fix-sidecar/`: the same scenario against a sidecar
  from before the #187 fix.
- `results.tsv`, `chain.tsv`, `logs/`, `e2e/`, `zec/`: the runner's output.
- `smoke/`: the smoke runs before the full run.
