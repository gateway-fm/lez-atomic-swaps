# Fuzz runs of the six targets, version 0.5.0, 2026-11-28

Each of the six `cargo-fuzz` targets ran once with `scripts/run-fuzz.sh`, from its checked-in
seed corpus. No target reported a crash, a timeout or an out-of-memory, and no artifact was
written.

| target | what it drives | libFuzzer time | executions | edges | features | crash / timeout / oom |
|---|---|---:|---:|---:|---:|---|
| `coordinator_store` | two swaps through the coordinator and one `SqliteSwapStore`, with restarts | 1 230 s | 46 000 | 4 337 | 19 400 | 0 / 0 / 0 |
| `coordinator` | the swap coordinator state machine | 1 216 s | 1 084 141 | 1 246 | 7 558 | 0 / 0 / 0 |
| `offer_announcement` | the offer announcement decoder | 316 s | 3 283 951 | 3 580 | 10 678 | 0 / 0 / 0 |
| `chat_frame` | the Chat frame decoder | 316 s | 7 457 536 | 3 332 | 7 299 | 0 / 0 / 0 |
| `zec_agreement_wire` | the Zcash take-time agreement records | 314 s | 18 924 618 | 1 500 | 2 798 | 0 / 0 / 0 |
| `xmr_reserve_wire` | the Monero reserve records | 298 s | 3 087 659 | 7 023 | 14 914 | 0 / 0 / 0 |

`fuzz-summary.txt` holds, for each target, the libFuzzer command, the first line, one line about
every 300 s, the last line, the end and the `cmin` result.

## What ran

- **Source.** Five targets ran on `f98908ec782d1b939b97420fa07575356b604063`. `xmr_reserve_wire`
  ran on `37d5467e4048693db3b046223a693f3990443700`. `git diff f98908ec 37d5467e` changes nothing
  under `fuzz/` or `crates/` and not `Cargo.lock`. The tree of `fuzz` is
  `d330b0e6a05528c4c78b904db0e2b854476e2be1`, of `crates/swap-core`
  `7cb19e0d60108d78f5d19a7dcbe6a70082bb5020` and of `crates/swap-store`
  `5fa08abab19f684319d13d6ccc86a8919f9674af`. The store holds the journal change of issue #174
  and the place rule of #178.
- **Container.** `rust:1.96.0-bookworm`, image
  `sha256:5e2214abe154fe26e39f64488952e5c991eeed1d6d6da7cc8381ae83927f0cfc`, linux/amd64, the
  repository mounted read-only. The nightly and cargo-fuzz ran only inside it.
- **Toolchain.** `nightly-2026-06-01` (rustc 1.98.0-nightly 14210df0e 2026-05-31, LLVM 22.1.6),
  `cargo-fuzz 0.13.2` (`cargo install --locked`), `libfuzzer-sys =0.4.13`. cargo-fuzz defaults:
  release profile with debug assertions, AddressSanitizer.
- **Command.** `FUZZ_JOBS=2 scripts/run-fuzz.sh <target> <seconds>`: 1200 s for the two
  coordinator targets and 300 s for the four decoders. The script sets each target's `-max_len`
  and `-timeout` (10 s for `coordinator_store`, 20 s for `xmr_reserve_wire`, 2 s for the others)
  and runs `cargo fuzz cmin` on a copy of the grown corpus afterwards.
- **Host.** A shared virtual machine with 8 x86_64 cores, 2 libFuzzer jobs in fork mode. The
  devnet stack, the systemd VM runs and the workspace's test suites ran on the same host during
  these runs, so the execution rates are lower bounds for this host.

## Starts that did not run

- The first start of `coordinator_store` (00:18Z) and the first start of `xmr_reserve_wire`
  (01:25Z) stopped inside the script's copy of the checkout, before any build: `tar` reported
  "file changed as we read it". The checkout held a running devnet stack's state under
  `deploy/runtime`, and the stack rewrote it during the copy. Neither start reached libFuzzer.
- `coordinator_store` then ran from the same checkout (01:26Z). `xmr_reserve_wire` ran from a
  second worktree that holds no `deploy/runtime` (01:54Z).
- `5f0da4ac` leaves `deploy/runtime` out of the script's copy. No fuzz target reads it.

## Corpus

No corpus was checked in from these runs. After `cmin` each grown corpus holds 185 to 1 448
files, over the 50-file bound for checking in. The stable replay tests of the checked-in
corpora (`cargo test -p lez-swap-core --test fuzz_corpus_replay` and `-p lez-swap-store --test
fuzz_corpus_replay`) passed in the workspace test run on `f98908ec`.

## What this does not cover

- The runs are 5 and 20 minutes long. The earlier one-hour run of `coordinator` is
  [coordinator-fuzz-2026-11-12](../coordinator-fuzz-2026-11-12/README.md).
- The coordinator model drives two swaps through the coordinator and its store. It does not
  drive the actors, the sidecar or a chain.
- Runs of 2026-11-27 on an earlier commit of this version are not part of this set. One of
  them, `xmr_reserve_wire` with a 2 s timeout, wrote a timeout artifact equal to the target's
  own seed `stage-a`, which takes about 4 s in the instrumented build on a loaded host. The
  script's timeout for that target has been 20 s since `c7e7f3ea`, and this set's run of it
  reports no timeout.
