# LEZ v0.2.0 sequencer reproducers

Executable answers to the milestone 1 sequencer questions, run against the
exact LEZ v0.2.0 crates the product is built from (`a58fbce2`). This is a
standalone Cargo workspace: nothing here is patched into upstream, and the
tests use only upstream's public API.

Each test starts a real `sequencer_core` with the mock block publisher, submits
transactions through the real mempool handle, and produces blocks with the real
builder. A test-only guest program, `windowed_transfer`, moves balance through
a chained call to the built-in authenticated-transfer program and stamps a
caller-chosen block and timestamp validity window on its output. It is compiled
with the RISC Zero Rust toolchain and deployed through the mempool at the start
of each windowed test.

| Test | Shows |
|---|---|
| `mempool_admits_then_block_rejects_insufficient_balance` | The mempool admits a stateless-valid overspend; the builder drops it. |
| `transaction_bytes_are_preserved_from_mempool_to_block` | Two identical submissions yield one inclusion, byte-identical to the submission. |
| `valid_when_queued_expires_before_inclusion` | A transfer valid for the next block is held back by a full block, expires, and is dropped with no state effect and no re-queue. |
| `block_window_start_is_inclusive` | Dropped one block before the start, included at exactly the start. |
| `block_window_end_is_exclusive` | Included at the last block before the end, dropped at exactly the end. |
| `timestamp_window_expires_while_queued` | A timestamp window that closes while queued expires the transfer. |
| `timestamp_window_bounds_apply_at_block_time` | Not-yet-open and already-closed windows are dropped from the block that includes an open one. |

Block heights are exact because the tests control block production.
Timestamps come from the builder's wall clock, so the timestamp cases use
margins rather than exact millisecond bounds.

## Running

```sh
rzup install rust          # RISC Zero Rust toolchain for the guest
rzup install r0vm 3.0.5
RISC0_DEV_MODE=1 cargo test --locked --manifest-path compat/lez-v0.2-sequencer-reproducers/Cargo.toml
```

`scripts/verify-lez-primitives.sh` runs the same tests by exact name when
`LEZ_VERIFY_SEQUENCER=1`, after checking that this crate pins the commit the
script verifies.
