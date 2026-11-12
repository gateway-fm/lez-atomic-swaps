# Coordinator fuzz run, 2026-11-12

One hour of `cargo fuzz run coordinator` against the swap coordinator state machine
(`lez-swap-core`), from the checked-in seed corpus. No crash, timeout or out-of-memory.

## What ran
- Source: commit `6710b8e9997a722cb32f63bb8b4f9dac0b26f966`; tree of `crates/swap-core`
  `03b737f8e39093c7f98383b0e143ce854e873e25`, tree of `fuzz` `1ed4b32af0af88c8940e7c4de3c66abca1571e40`.
  `scripts/run-coordinator-fuzz.sh` was added afterwards and changes neither tree.
- Container: `rust:1.96.0-bookworm`, image `sha256:5e2214abe154fe26e39f64488952e5c991eeed1d6d6da7cc8381ae83927f0cfc`,
  linux/amd64, repository mounted read-only. The nightly and cargo-fuzz ran only inside it.
- Toolchain: `nightly-2026-07-01` (rustc 1.98.0-nightly f46ec5218 2026-06-30, LLVM 22.1.8),
  `cargo-fuzz 0.13.2` (`cargo install --locked`), `libfuzzer-sys =0.4.13`; the versions of the
  former CI job. cargo-fuzz defaults: release profile with debug assertions, AddressSanitizer.
- Command: `scripts/run-coordinator-fuzz.sh 3600` (FUZZ_JOBS=6), which runs
  `cargo fuzz run --fuzz-dir fuzz coordinator <corpus> -- -fork=6 -max_total_time=3600 -max_len=512 -timeout=2`
  and then `cargo fuzz cmin coordinator <copy of the grown corpus> -- -max_len=512`.
  `-max_len=512` is the harness's input format: one profile byte and at most 511 action bytes.
- Host: shared 8-core amd64; 6 parallel libFuzzer jobs (fork mode). Fuzzing from
  2026-11-12T09:44Z; script wall time 1:00:44 including the build and cmin; libFuzzer: 3634 s.

## Results
- Executions: 6,213,892 in 3634 s, 1,710 exec/s across the 6 jobs (the `exec/s` column of
  fork-mode lines is the last job's own rate, 218–480).
- Coverage: 1118 edges / 2124 features after the first job on the seeds; 1306 edges by 300 s,
  unchanged for the remaining 55 minutes; features 8411 at the end (of 9803 inline counters).
- Corpus: 9 seeds before, 861 files (15,776 bytes) after; `cmin` keeps 729 files (13,450 bytes,
  8410 features, 1306 edges). That is over the 50-file bound for checking in, so
  `fuzz/corpus/coordinator` is unchanged; the stable replay test
  (`cargo test -p lez-swap-core --test fuzz_corpus_replay`) passes on it.
- Crashes, timeouts, out-of-memory: 0 / 0 / 0. `fuzz-summary.txt` holds the libFuzzer start,
  one line every ~300 s, the end and the cmin result.

## What the oracles check (fuzz/fuzz_targets/coordinator_harness.rs)
1. A rejected transition leaves the coordinator exactly as it was.
2. Completed and Refunded are absorbing: no later input changes the coordinator.
3. Completed always holds public claim evidence; no refunded phase ever holds revealing-claim evidence.
4. Swap ID, pair, direction, recovery schedule, funded chains and confirmation policies never change.
5. After every action the coordinator serialises and deserialises to an equal value (restart safety).

## What this does not cover
One target only: the coordinator state machine, driven by a fixed alphabet of observations with
fixed proofs and heights. No network or RPC decoders, no store formats, no chain adapters or
actors, no cryptography; edge coverage plateaued at 1306 within five minutes.
