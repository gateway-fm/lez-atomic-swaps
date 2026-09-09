# Record and share swap evidence

Run concurrent BTC → LEZ swaps and both refund scenarios through the existing
Maker/Taker APIs on Bitcoin regtest and the local LEZ v0.2 devnet. The output
includes offline playback, public transaction evidence and source provenance.

## Build and run

Use Apple Silicon with Docker Desktop and Homebrew, or ARM64 Linux with Docker
Engine, Compose v2, Git, Python 3, jq, curl, OpenSSL, xxd and shasum
(`libdigest-sha-perl` on Debian/Ubuntu). x86 is not supported by these payloads.
Allow several hours for the first source build and about one hour for all three
scenarios. Building needs internet access; the running chains are local.

Use a fresh checkout and an empty workspace. Only one stack can run on a Docker
daemon because container/network names and ports 18443, 3040 and 8779 are fixed.
Stop an existing stack from its own Compose directory, without deleting volumes.
Evidence setup gives each checkout/workspace separate persisted state.

```sh
git clone https://github.com/gateway-fm/lez-atomic-swaps.git
cd lez-atomic-swaps
git checkout <commit-to-test>
bash deploy/scripts/from-scratch.sh --evidence --workspace "$PWD/../evidence-workspace"
python3 deploy/scripts/record-evidence.py
```

`--evidence` skips Basecamp, Nix and explorers, selects fast local refund timing,
builds the Nodes/actors/sidecar, creates LEZ identities and prepares the market.
It creates or loads the Core wallets and funds the Taker with mature regtest
coins when needed. Reusing the same checkout/workspace resumes its state;
Cargo caches make subsequent builds faster.

Keep the selected commit unchanged during build and capture. The recorder
rejects tracked edits, another checkout's stack, stale source receipts, binary
mismatches, the wrong network/timing and existing output directories. Run one
recording at a time without other swaps: refund scenarios stop/restart the Maker,
and historical LEZ balance checks assume no competing account transactions.

## Inspect and share

The command prints a directory under `deploy/runtime/recordings/`. Open its
`index.html` locally for offline playback, seeking and speed controls. Each
scenario also has an asciinema v2 `execution.cast` and plain `execution.log`.
These are API execution recordings; Basecamp UI interaction is not exercised.
A [public example capture](../docs/evidence/swap-20260908-c7c5f2d/README.md)
includes all three recordings and their exact source identity.

| Scenario | Required outcome |
| --- | --- |
| `concurrent` | Both swaps lock BTC before either claim; both complete with five confirmed/finalized public effects each. |
| `taker-refund` | Maker never funds LEZ; Taker recovers BTC after the recovery conditions mature. |
| `maker-refund` | Maker recovers its unclaimed LEZ, then Taker recovers BTC. |

BTC refunds must spend the recorded lock and pay the Taker's contribution
script: 1,000,000 sats becomes 999,000 sats plus a 1,000-sat fee. Historical LEZ
balances must show 1,000 units funded and returned. Core wallet balance alone
is not refund proof because mining also produces coinbase rewards.

`result.json` records success or failure; failed/interrupted attempts retain
logs and return a nonzero exit code. `provenance.json` identifies the Git commit,
running binary hashes, images and timing. Wallet databases, keys, credentials
and runtime env files are excluded.

```sh
cd deploy/runtime/recordings/<printed-directory>
shasum -a 256 -c SHA256SUMS
# Share this directory, not the surrounding runtime or workspace.
```

Checksums verify file integrity; rerunning verifies behavior. New identities,
genesis, transaction IDs and timings will differ. Compare outcomes, amounts,
destinations, ordering and finality. These BTC → LEZ tests do not revalidate
XMR, reverse-direction swaps or unresolved protocol threat-model questions.

## Repeat or stop

```sh
# From the repository root, run just one scenario or choose a new output path:
python3 deploy/scripts/record-evidence.py --scenario maker-refund --output /tmp/my-swap-evidence

# Stop while retaining chain and wallet state:
cd deploy
docker compose --env-file runtime/runtime.env down
```

If an interrupted scenario leaves a role stopped, restart it with
`docker compose --env-file runtime/runtime.env up -d maker-node taker-node`.
Inspect retained swaps before repeating. Do not reset the chain or change timing
mid-swap. Resume a failed build with the same workspace and `--evidence` arguments;
`--only sources|rust|build|stage|stack` selects one build phase.
