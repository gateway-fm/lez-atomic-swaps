# LEZ v0.2 sidecar verification

This crate is the process boundary between the swap actors and the exact
LEZ v0.2 (`LEE`) RPC/type generation. For milestone evidence, run
`scripts/verify-lez-v02-sidecar.sh`; that wrapper is authoritative. Direct
`cargo` commands do not certify this crate because Cargo's offline mode alone
does not stop the upstream `rust-rapidsnark` build script from attempting an
implicit release-asset download.

## Manual prerequisites

Install the pinned Rust 1.96.0 toolchain and `cargo-deny` 0.19.9. The machine also
needs Bash, `awk`, `rg`, `sha256sum`, the C/C++ and libclang prerequisites used
by bindgen, and a complete local Cargo/Git dependency cache. The verifier uses
`--locked`, `--offline`, and `CARGO_NET_OFFLINE=true`; it never downloads or
provisions a missing tool or dependency.

Provide an absolute path to the already-extracted rapidsnark v0.0.8 libraries.
The wrapper attests all four files against the identities in
`../lez-v0.2-provisional/local-stack.toml` before it invokes Cargo:

| File | Required SHA-256 |
| --- | --- |
| `librapidsnark.a` | `d4133227f845ff5bfa3672eb5b9c018a6a086bfa164b176bdaf76949c7d1f423` |
| `libgmp.a` | `0a910b420c3ad603c83c9dc2818c7ae05394c231ca23135c7b873e8e680ea41b` |
| `libfq.a` | `797b5d24bb8e8b088f811bddfff35f33973af9c797fb3812489cd42ba6a957d0` |
| `libfr.a` | `40f809394904682cb5517845cd3c2f936a5eb4609712534b573f552f2811fb82` |

From the repository root, run:

```bash
export RAPIDSNARK_LIB_DIR=/absolute/path/to/verified/rapidsnark-v0.0.8-libraries
export BINDGEN_EXTRA_CLANG_ARGS=-I/usr/lib/gcc/x86_64-linux-gnu/13/include
./scripts/verify-lez-v02-sidecar.sh
```

The wrapper then checks formatting, locked offline tests, strict Clippy,
rustdoc with warnings denied, and `check-dependency-policy.sh`. Missing,
relative, or hash-mismatched native inputs fail before Cargo starts.

## Scope and limitations

This gate verifies the isolated sidecar source and its dependency boundary. It
also verifies the first fail-closed native prepare foundation: the exact v0.2
escrow instruction and account types emitted by the pinned SPEL generator,
official LEE `Message` and `PublicTransaction` bytes, isolated role, runtime,
signer, and program bindings, one owned consecutive nonce pair, canonical byte,
hash, and signature validation, and exact in-memory replay. Tests assert the
generated initialize and fund account order and every instruction field.
Recovered pairs are validated against the complete original request; raw
transaction bytes alone do not prove an authenticated actor role.

The gate also verifies deterministic actor Vault Claim preparation against the
official `programs` and `vault_core` crates from LEZ v0.2.0. The maker fixture
(private-key byte `01`, allocation `100000`) and taker fixture (private-key byte
`02`, allocation `200000`) reproduce the public-key, owner-account, and Vault
PDA snapshots in ADR 0025. Each exact public transaction uses
`programs::vault().id()`, `vault_core::compute_vault_account_id`, ordered
accounts `[owner, owner_vault]`, one owner nonce, one owner witness, and
`vault_core::Instruction::Claim { amount }`. The complete request carries the
node-observed owner nonce; the configured nonce source must independently
confirm that same value before signing. Tests reject role, runtime, key,
allocation, program, account, order, amount, nonce, transaction-ID, canonical
byte, and signature substitutions.

Preparation is atomic at the planner boundary: its per-actor mutex serializes
nonce ownership, and it installs the active reservation only after the complete
official transaction has been constructed, signed, and converted to its exact
bounded representation. A failed preparation exposes and caches no partial
claim. This is local preparation atomicity only; finalized on-chain balance
conservation remains an actor-level end-to-end property.

The source-audited genesis pre-state is also part of effect eligibility. LEZ
v0.2 genesis routes each supplied allocation through the Vault program into an
owner-derived PDA controlled by the authenticated-transfer program; the owner
account is still the default account. An indexer with no finalized tip is
scanned from official `lee::GENESIS_BLOCK_ID` (block 1), never from block 0.
These are executable upstream facts, not fixture conventions.

Exact LEZ v0.2 still exposes a `PrivateKey` whose `Debug` and `Display` reveal
raw material and which does not implement `Zeroize`. This planner never formats
the key and keeps its retained byte copy in `Zeroizing<[u8; 32]>`, but upstream
constructor inputs and transient signing copies cannot be claimed fully
zeroized. LOGOS-008 tracks replacement or an upstream fix before production.

The Linux foundation now includes a narrow Vault Claim submission state
machine. Each planner optionally binds one pre-existing owner-only `0700` actor
state directory through `new_durable` using `openat2(NO_SYMLINKS)` and persists
its single reservation as an owner-only `0600` fsynced create-exclusive file.
Restart recovers the originally signed exact bytes
without calling the nonce source or re-signing; recovery revalidates the
complete stored request and result and rejects role, runtime, signer,
allocation, program, transaction-ID, canonical-byte, and signature drift. A
different request against an existing reservation fails closed, as do crash
partials, corruption, unknown fields, future schemas, symlinked or foreign-owned
directories, runtime directory/file permission drift, and hardlink aliases;
diagnostics never reveal the store path. The effect journal additionally binds
one run, role, runtime, and signer to canonical typed preparation plus duplicate
exact bytes in SQLite. It commits attempt one and revision one before the only
`LeeTransaction::Public` call; reopen, crash, concurrency loss, transport
ambiguity, or a returned-hash mismatch can never restore send permission.
Unexpected schemas/triggers, binding drift, noncanonical revisions, unsafe
ancestors, symlinks, hard links, inode replacement, and parent permission drift
fail closed. The coordinator alone classifies raw official `jsonrpsee`
`ClientError` values. Active same-UID/root writers are outside this filesystem
boundary, so mutually untrusted roles require separate users or containers.

The crate now also provides two narrow local-PoC executables. The
`lez-v02-vault-claim-poc` command submitted the distinct maker and taker Vault
Claims to the retained official v0.2 node. The
`lez-v02-native-escrow-poc` `deposit`, `claim`, and keyless `observe`
subcommands then drove the checked escrow through `Absent -> Empty -> Funded ->
Claimed`: maker alone initialized/funded, taker alone supplied the revealing
preimage and claimed, and each process used a different key file and owner-only
state directory. The exact transactions, blocks, balances, PDAs, runtime, and
limitations are in
[`docs/evidence/m2-local-onboarding-20260714.json`](../../docs/evidence/m2-local-onboarding-20260714.json),
with manual commands in
[`docs/manual-user-flows.md`](../../docs/manual-user-flows.md#flow-0d-run-the-role-separated-native-lez-v02-slice).

The native executable observes canonical sequencer inclusion and stable
same-tip account facts. Separate sequential indexer reads established that the
three exact transactions were in finalized blocks 219, 220, and 223; that
manual proof does not make the CLI result an indexer-finality result. The CLI
persists exact signed bytes and observes before submit, but honestly emits
`crash_atomic_submission=false`: ambiguous multi-effect crash reconciliation
and integrated journal-to-finality transitions remain post-PoC hardening.
Forty-two existing integration tests and the full wrapper gate stayed GREEN; no
new PoC feature test is represented as a RED-GREEN-REFACTOR phase transition.

The crate now also contains the exact source-complete PoC bridge process,
`lez-v02-bridge-poc`. It accepts only an explicitly provisioned nonzero
literal-loopback listener, official sequencer and indexer loopback URLs, and
file-backed runtime, capability, signer, and private state inputs. Before it
binds, it verifies the runtime/signer relationship and both official node
health gates. Bearer capability, exact run ID, fixed role, runtime, signer, and
state identity are checked at the process boundary. The bounded bridge
implements describe, native prepare, escrow observation, revealing-claim
prepare/observation, and exact transaction submission. Refund methods are
registered but return a typed unavailable result.

Successful PREPARE results are durably replayed. Observation results and
transient PREPARE failures re-execute instead of becoming stale facts. A submit
request persists an unknown-outcome marker before node I/O, so restart cannot
silently resubmit an ambiguous transaction. The sequencer observation contract
is deliberately limited to bounded canonical inclusion plus stable same-tip
account reads. Readiness requires a non-genesis finalized indexer tip, but this
process does not itself assert effect finality. Both role processes completed
the live `m2poc-corridor-fresh-20260714o` run: taker initialize/fund and maker
revealing-claim effects were accepted exactly once, both actors reached
`Completed`, and a separate indexer audit located those transactions in
finalized blocks 264, 265, and 266 with claimed metadata and zero custody. The
exact claim-absence bug exposed by 14d and the unbounded indexer startup wait
were fixed in pushed commit `0861117`; the readiness gate now uses
`getLastFinalizedBlockId` instead of upstream `checkHealth`, whose full-state
recalculation was unsuitable for a bounded corridor startup check.

Live authenticated reference-actor composition and Zebra HTLC effects are now
GREEN for the first of two required directions. The reverse
`TakerSellsForeign` corridor and integrated terminal-evidence collection remain
PoC work under ADR 0026. PoC-to-hardening and milestone transitions remain an
explicit repository-owner decision.

This package verification gate itself does not start Bedrock, the LEZ
sequencer/indexer, Zebra Regtest, or Docker. The separate run14o composed runner
consumed explicitly configured isolated local nodes and proves one corridor
direction only. It does not prove the reverse direction, public-runtime parity,
public deployment, refund recovery, or post-PoC hardening.

The native archive is outside Cargo license analysis. Upstream metadata
identifies
rapidsnark as LGPL-3.0-or-later and GMP as LGPLv3-or-GPLv2. This local/CI
gate publishes no artifact. Before distributing a statically linked binary or
image, production release review must determine and satisfy the applicable
source, relinking, and notice obligations. This records the upstream metadata
boundary; it is not legal advice.

The original rapidsnark archive is an external GitHub release asset, and Cargo
also normally resolves crates and pinned Git sources from external services.
Acquiring those inputs can be unavailable or flaky. Provision them out of
band, verify them with the local-stack source contract, and retain a trusted
local cache. A cold build can additionally be slow and disk/CPU intensive.
Once the inputs are cached, this wrapper deliberately avoids the network and
fails closed instead of falling back to any upstream download.
