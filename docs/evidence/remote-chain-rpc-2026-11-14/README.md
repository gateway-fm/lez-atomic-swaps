# Chains on another host: four defects, and what was run after fixing them

On 2026-11-14 both role Nodes and both desks ran natively on a Linux x86_64
host (`<x86_64-host>`) from the published, signed 0.3.1 packages in Basecamp
0.3.2. The chains were the devnet's containers on an Apple-silicon Mac,
reached through an SSH tunnel: each chain RPC took 30–120 ms (p90 about
220 ms), against 1–5 ms on one machine. That run is kept in
[`found-by/`](found-by/README.md). It found four defects, filed as issues
#149 to #152. This folder records what the fixes change and what was run
against them on the Mac. [`PROVENANCE.md`](PROVENANCE.md) says which commits
were tested, and what was not run.

## What the run found

| issue | what happened over the remote link | where |
|---|---|---|
| #149 | No ZEC swap got past its LEZ step at the devnet's 1 s LEZ blocks: every LEZ observation failed with `UnstableTip` (`actor_drive_unavailable LezFirstLock(Adapter(Canonical(UnstableTip)))`). Both directions passed only after the LEZ block interval was raised to 5 s, then 10 s. | [`found-by/results.tsv`](found-by/results.tsv), [`found-by/logs/e2e-zec-attempt2-unstable-tip/`](found-by/logs/e2e-zec-attempt2-unstable-tip/zec-happy-lez.log), [`found-by/latency-analysis.md`](found-by/latency-analysis.md) section 1 |
| #150 | Maker actor attempts ran past their 30 s deadline (`actor_timeout`, 17 times in one swap); a Maker in recovery never reconciled the Taker's refund. A counting proxy showed each attempt walking the LEZ discovery window one block at a time, `getBlockById` and `getBlockByHash` per block: about 560 calls. | [`found-by/logs/e2e-refund/`](found-by/logs/e2e-refund/btc-taker-refund.log), [`found-by/logs/rpc-calls.log.gz`](found-by/logs/rpc-calls.log.gz), [`found-by/latency-analysis.md`](found-by/latency-analysis.md) section 2 |
| #151 | Under Ubuntu's umask 002 the first swap failed after four minutes of retries: `actor_deployment_invalid` on the Maker (its `modules/lez_maker_node/bin` was 0775), and "sidecar state directory or lease file is unsafe" on both sidecars (a 0775 working directory above the Node's state). | [`found-by/directory-permissions.md`](found-by/directory-permissions.md), [`found-by/results.tsv`](found-by/results.tsv) |
| #152 | The harness failed on Linux with `./lez-zec-taker-actor` not found (`native_bin()` read `ps -o comm=`), and needed a `docker` stand-in to reach Bitcoin Core and Zebra over RPC. | [`found-by/scripts/docker-shim.py`](found-by/scripts/docker-shim.py), [`found-by/results.tsv`](found-by/results.tsv) |

## What changed

- **#149.** `crates/zec-swap-sdk/src/lez_observation.rs`: the observation is
  pinned at the tip read before it. The transaction must be included at or
  below that tip, and depth is counted from it. The tip read after it is
  accepted when it is the same block, or a higher block with another hash.
  The sidecar has checked, by hash, that a higher tip still holds the pinned
  block at the pinned height (`confirm_extends`, now through
  `pinned_block_still_canonical`). A replaced pinned block, a tip that went
  back, and a higher tip that carries the pinned hash are still
  `UnstableTip`.
- **#150.** `compat/lez-v0_2-sidecar`: `read_finalized_run` reads a
  finalized window with the pinned indexer's `getBlocks`, 64 blocks per call.
  Every block is still checked for its height, Bedrock `Finalized` and its
  hash link, and the run's last block is read again by hash. A boundary
  without the batch read, or a batch that is not exactly the run asked for,
  is read block by block as before. The claim, funding and asset observers
  and the refund observer use it.
- **#151.** `modules/lez-role-node/node-entrypoint.sh`: the launcher removes
  group and other write from its own package's directories and programs.
  Before the Node starts it applies the sidecars' ancestor rule to the
  directories above the swaps' and the ZEC sidecars' state, and exits 78
  with the directory, the reason and the fix. Both security checks are
  unchanged.
- **#152.** `deploy/scripts/node-e2e.py` reads `/proc/<pid>/exe` on Linux.
  `LEZ_CHAINS_RPC_ONLY=1` sends its `bitcoin-cli` calls as JSON-RPC, fails a
  scenario that stops a chain's container, and has `zec-provision.sh` check
  Zebra by its RPC. The runners in the repository already kept a scenario's
  exit status; the runner that logged `exit 0` for a failed scenario was a
  local copy.
- `deploy/scripts/rpc-delay-proxy.py`: a test tool. It adds a fixed delay to
  every round trip on a chain port and logs each JSON-RPC method.

## What was measured here

**A finalized window over a 100 ms link** ([`measured/`](measured/window-timing.txt)).
The official indexer client read one 280-block finalized window, the
devnet `fast` profile's, from a JSON-RPC indexer with the pinned indexer's
semantics. The read went through `rpc-delay-proxy.py --delay-ms 100`, which
counted every call.

| read | indexer calls | time |
|---|---|---|
| block by block (the code before #150) | 563: 281 `getBlockById`, 281 `getBlockByHash`, 1 `getLastFinalizedBlockId` | 59.1 s |
| batched (`read_finalized_run`) | 9: 5 `getBlocks`, 1 `getBlockById`, 2 `getBlockByHash`, 1 `getLastFinalizedBlockId` | 1.0 s |

The proxy's own log agrees: 572 calls in all, 563 plus 9
([`measured/window-calls-by-method.tsv`](measured/window-calls-by-method.tsv)).
At 100 ms a round trip, the block-by-block read alone is twice the actor's
30 s attempt deadline; the batched read is 1 s.

**Tests.** The test runs and their counts are in [`PROVENANCE.md`](PROVENANCE.md).
The tests added with the fixes:

- #149: `canonical_lez_observation_is_pinned_and_accepts_only_a_tip_that_grew`
  (`crates/zec-swap-sdk/tests/sdk_lifecycle.rs`) covers a tip that grew
  (accepted, at the pinned depth), a replaced pinned block, a tip that went
  back and a higher tip with the pinned hash (all `UnstableTip`). The claim
  matrix (`lez_claim_observations.rs`) adds the same cases, plus an inclusion
  above the pinned tip (`NoncanonicalInclusion`). In the sidecar,
  `a_later_tip_extends_the_pinned_one_only_while_the_pinned_block_stays`
  covers the by-hash check.
- #150: four unit tests in `finalized_claim_observation.rs` with a counting
  indexer (563 calls before, 9 after for 280 blocks; a broken link, a block
  that is not finalized, a wrong predecessor and an end the indexer does not
  serve by hash are each refused; a short batch falls back), and one against
  a JSON-RPC server for the `getBlocks` binding.
- #151: three launcher tests in `deploy/tests/test_role_node_launcher.py`.
  A package installed 0775 ends up 0755 and the Node starts; a state
  directory below a 0775 directory is refused by name (and accepted once the
  directory is sticky); a state directory below a symlink is refused by name.
- #152: three harness tests in `deploy/tests/test_node_e2e_harness.py`:
  `/proc/<pid>/exe` on Linux, `bitcoin-cli` as JSON-RPC with the cookie
  (wallet and named arguments, an error reply), and a chain container
  refused under `LEZ_CHAINS_RPC_ONLY`.

## Not run

The swaps through the delay proxy were not run. The plan was the
devnet's chains on the Mac (lane 1's LEZ network, lane 2's Bitcoin Core and
Zebra) with Node images built from the fixes, and every chain RPC of both
Nodes through `rpc-delay-proxy.py` at 100 ms. It would have covered ZEC
`zec-happy` in both directions at 1 s LEZ blocks, BTC `happy` in both
directions, and `taker-refund` with the Maker reconciling, each before and
after. Starting the devnet's stopped chain containers was refused by this
session's permission check, so none of these swaps ran. The fixed build has
not been run on the x86_64 host either. [`PROVENANCE.md`](PROVENANCE.md)
lists the images that were built for the run.
