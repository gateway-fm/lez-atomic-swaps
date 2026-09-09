# Repeat the concurrent-swap and refund evidence

These commands run actual BTC → LEZ swaps through the Maker/Taker owner APIs
on Bitcoin regtest and the LEZ v0.2 devnet. They produce new recordings and
verify the public chain effects. They do not require our wallet identities,
credentials, staged executables, Basecamp, or screen-recording software.

## Prepare a dedicated ARM64 machine

Use Apple Silicon with Docker Desktop and Homebrew, or ARM64 Linux with Docker
Engine, Compose v2, Git, Python 3, jq, curl, OpenSSL, xxd and shasum
(`libdigest-sha-perl` on Debian/Ubuntu). Allow several hours
for the first source build and about one hour for the three scenarios.
Internet access is needed for pinned sources, container images and dependencies;
the running chains are local. x86 hosts are not supported by these payloads.

Use a fresh checkout and an empty workspace. The Compose stack uses fixed
container and network names and localhost ports 18443, 3040 and 8779.
Only one instance can run on a Docker daemon. For an existing installation,
stop it using its own Compose directory. Evidence mode derives separate volume
names from the checkout and workspace paths, so it preserves the previous
stack's wallets and Node stores. Do not delete existing volumes to follow this guide.

```sh
git clone https://github.com/gateway-fm/lez-atomic-swaps.git
cd lez-atomic-swaps
# Select the reviewed commit containing this guide, before building:
git checkout <reviewed-commit>

bash deploy/scripts/from-scratch.sh --evidence --workspace "$PWD/../evidence-workspace"
python3 deploy/scripts/record-evidence.py
```

`--evidence` skips Nix, Basecamp and the explorers, selects `fast` refund timing,
builds the Nodes, Bitcoin actors and LEZ sidecar from this checkout, and prepares
the settlement market. Four new LEZ identities receive genesis allocations.
The bootstrap creates or loads the two Core wallets and mines mature test coins
to the Taker if its spendable balance is below 1 BTC. It refuses wallet seeding
on any chain other than regtest. Ordinary restarts retain identities and funds.

After deployment, bootstrap publishes the public market manifest into the
runtime and recreates both Nodes so their Bitcoin lifecycle is enabled. The
private market directory is not mounted into the Nodes.

The volume namespace is persisted in `runtime/runtime.env` for subsequent
Compose commands. Reusing the same checkout and workspace resumes its state;
a new checkout and workspace create independent state.

The checked-out Node and sidecar sources are submitted to Cargo on every build;
existing staged binaries do not bypass compilation. Cargo dependency/target
caches are reusable. Upstream chain/tool payloads in the provision directory
are reused on subsequent runs. For a build without those payloads, use a new
workspace. An entirely empty Docker build cache is considerably slower.

The recorder refuses a stack belonging to another checkout, tracked source edits, the wrong timing profile, stale
source receipts, running binaries that differ from the built images, and an
existing output directory. Run the scenarios sequentially and avoid other swaps
while recording: refund tests stop/restart the Maker, and historical LEZ balance
checks assume there are no competing transactions for the Maker account.

## Inspect and share the results

A [checked-in example capture](../docs/evidence/swap-20260908-c7c5f2d/README.md)
contains the public recordings and transaction records from this machine. Its
README identifies the exact capture commit and distinguishes it from earlier
release evidence. Download its `index.html` to play the recordings locally.

The command prints its output directory under `deploy/runtime/recordings/`.
Open `index.html` directly in a browser; it contains all terminal recordings,
with playback, seeking and speed controls, and requires no network access.
These are API execution recordings, not videos of manually clicking the UI.
Each scenario also has an asciinema v2 `execution.cast` and plain `execution.log`.

* `concurrent`: both swaps lock BTC before either is claimed; both finish. Each
  has five confirmed/finalized public effects.
* `taker-refund`: the Maker is stopped before the BTC broadcast, so it cannot
  fund LEZ; the Taker refunds BTC after the recovery conditions mature.
* `maker-refund`: the Maker funds LEZ and the Taker leaves it unclaimed; the
  Maker refunds LEZ, then the Taker refunds BTC.

Success requires the scenario assertions and the public evidence checks. Both
BTC refunds must spend their recorded lock, pay the Taker's contribution
script, and reconcile 1,000,000 sats to 999,000 sats plus a 1,000-sat fee. Maker
LEZ funding/refund are checked against historical account balances at their
actual blocks: 1,000 units leave and return. Funding-wallet balance alone is
not treated as proof of a BTC refund.

`result.json` records the overall result. A failed attempt returns a nonzero
exit code and retains its logs; it is never silently presented as passing.
`provenance.json` records the checkout, running executable hashes, image IDs and
timing. `source.tar.gz` contains that commit's tracked source. The output does
not copy wallet databases, signer keys, RPC credentials or runtime env files.

```sh
cd deploy/runtime/recordings/<printed-directory>
shasum -a 256 -c SHA256SUMS
# Share this output directory, not the surrounding runtime or workspace.
```

Checksums check the supplied files' integrity; independently executing the
scenarios checks the behavior. New identities, genesis, transaction IDs, swap
IDs and elapsed times will differ. Compare outcomes, destinations, amounts,
ordering and finality, not identical hashes or playback timing. A Node can
observe a finalized refund later than its chain inclusion; the log preserves
that wait.

To repeat just one scenario on the prepared stack:

```sh
python3 deploy/scripts/record-evidence.py --scenario maker-refund
# Or specify a new output directory:
python3 deploy/scripts/record-evidence.py --output /tmp/my-swap-evidence
```

## Stop or recover

```sh
cd deploy
docker compose --env-file runtime/runtime.env down
```

This preserves the state. A failed/interrupted scenario can leave a role
stopped; `docker compose --env-file runtime/runtime.env up -d maker-node taker-node`
restarts it. Inspect retained swaps before running another attempt. Do not reset
the chain to recover a swap or change its timing profile mid-flight.

The build is resumable with the same workspace and `--evidence` arguments;
`--only sources|rust|build|stage|stack` selects one phase. Keep the selected
commit unchanged through build and capture. These tests cover the BTC → LEZ
API paths, not XMR, reverse-direction swaps or unresolved protocol threat-model
questions. Older release recordings remain historical evidence.
