# M3 local LEZ/BTC PoC operator guide

Last verified: 2026-07-17

This guide reproduces the current schema-4 public-actor M3 happy path and the
retained two-lock refund, absent-maker, and post-reveal survivor paths. Clean,
already-pushed run `m3schema4-20260717d` passed both happy directions at commit
`0e7635fc7e50cc6e0612745dcdaf6df8bbcf6f9a`. It uses Bitcoin Core 31.1
Regtest, the pinned LEZ v0.2 Bedrock/sequencer/indexer stack, the checked
witnessed-escrow guest, independent Maker and Taker sidecars, role-local stores
and signing journals, and actual transactions on both local chains.

This is a functional PoC recipe, not an automated full-lifecycle application.
The Taker externally submits the direction-selected first lock. Only the
schema-4 Maker config carries `maker_lock` material. A fresh
`btc-reference-actor` process reconstructs and durably journals the exact
direction-shaped plan, rechecks the first lock and signed cutoff, and submits
the Maker second lock. The controller may confirm, mine, wait for finality, and
invoke another one-shot process; it cannot submit that lock for the Maker.
LEZ initialization/funding are two ordered Maker steps and Bitcoin funding is
one exact Maker step. A fresh restart never rearms an accepted or ambiguous
step. Both roles project revision two only from current and
canonical/finalized evidence.

Run D reached revision 4 `completed` for both roles in both directions. Each
direction retained two unique Bitcoin effects and three exact LEZ effects,
exactly one Maker second-lock effect, and zero replay resubmissions. Historical
run `m3actor-20260716n` remains valid schema-3 evidence for observation of
operator-submitted locks plus actor-owned claims. Schema 3 is now legacy
observation-only compatibility and is not Maker-lock ownership evidence.
Retained runs `m3refund-20260716h`, `m3firstlock-20260716h`, and
`m3survivor-20260716c` remain the current timeout/refund and survivor evidence;
their historical schema/config boundaries are stated in their sections.

Run D closes the schema-4 actor-owned Maker-lock checkpoint, not all accepted
M3 scope. Two genuinely overlapping swaps, full-lifecycle SDK/custom-token
review, synchronized remaining architecture, final CI/security/Mermaid gates,
process-kill/reorg/chaos hardening, public deployment, and production readiness
remain open. No `m3-complete` tag is authorized by this run alone.

The earlier operator-composed reference result is
[m3-local-two-direction-poc-20260715.json](evidence/m3-local-two-direction-poc-20260715.json).
It records two completed directions, exact public transaction and block
identities, one Bitcoin confirmation per effect, finalized LEZ inclusion, exact
chain-witness equality, terminal recipients, and the explicit production
nonclaims. Do not publish a private run root: it contains keys, capabilities,
SQLite journals, exact signed transactions, and complete signatures.
Run D's audited terminal and cleanup packets remain locally below
`.e2e/m3schema4-20260717d/m3-actor-poc/evidence/`; the entire run root remains
owner-private. Its secret-safe retained checkpoint is
[m3-schema4-actor-owned-lock-poc-20260717.json](evidence/m3-schema4-actor-owned-lock-poc-20260717.json).
Run H's corresponding private evidence root is
`.e2e/m3refund-20260716h/m3-actor-poc/evidence/`.

## Topology and trust boundary

```mermaid
flowchart LR
    Operator["Local operator"] --> CoreProvisioner["Run-owned Core provisioner"]
    Operator --> LezProvisioner["Run-owned LEZ provisioner"]
    Operator --> AgreementGenerate["Agreement generate stage"]
    CoreProvisioner --> Core["Bitcoin Core 31.1 Regtest"]
    LezProvisioner --> Bedrock["LEZ v0.2 Bedrock"]
    LezProvisioner --> Sequencer["LEZ v0.2 sequencer RPC"]
    LezProvisioner --> Indexer["LEZ v0.2 indexer RPC"]
    Sequencer --> Bedrock
    Bedrock --> Indexer
    Maker["Maker role process and private store"] --> MakerBridge["Maker witnessed sidecar"]
    Taker["Taker role process and private store"] --> TakerBridge["Taker witnessed sidecar"]
    Maker --> MakerLockJournal["Schema 4 Maker lock journal"]
    Maker --> MakerSigners["Maker BTC and LEZ signer journals"]
    Taker --> TakerSigners["Taker BTC and LEZ signer journals"]
    AgreementGenerate --> PrivateFixture["Fresh signing refund claim and adaptor files"]
    AgreementGenerate --> AccountHelper["Pinned official NSSA account helper"]
    PrivateFixture --> FundingPrepare["Offline exact funding preparation"]
    CoreProvisioner --> FundingCredential["Owner-private rawtr credential"]
    FundingCredential --> FundingPrepare
    FundingPrepare --> FundingBytes["Persisted exact signed funding bytes"]
    FundingBytes -.->|"gettxout and read-only policy"| Core
    FundingBytes --> AgreementFinalize["Agreement finalize stage"]
    AccountHelper --> AgreementFinalize
    Sequencer -.->|"operator supplies runtime and deployment facts"| AgreementFinalize
    Indexer -.->|"operator supplies prepared claim facts"| AgreementFinalize
    AgreementFinalize --> MakerSigners
    AgreementFinalize --> TakerSigners
    AgreementFinalize -->|"schema 4 activation"| Maker
    AgreementFinalize -->|"schema 4 activation"| Taker
    MakerBridge --> Sequencer
    MakerBridge --> Indexer
    TakerBridge --> Sequencer
    TakerBridge --> Indexer
    Maker -->|"maker rpcauth"| Core
    Taker -->|"taker rpcauth"| Core
    Taker -->|"external first lock when Bitcoin"| Core
    Taker -->|"external first lock when LEZ"| TakerBridge
    MakerLockJournal -->|"one exact Bitcoin send"| Core
    MakerLockJournal -->|"ordered LEZ sends"| MakerBridge
    Core -->|"current unspent and canonical Maker lock"| MakerLockJournal
    Indexer -->|"current Funded and finalized Maker lock"| MakerLockJournal
    MakerLockJournal -->|"atomic intent close and revision 2"| Maker
    Core -->|"observe Maker Bitcoin lock"| Taker
    Indexer -->|"observe Maker LEZ lock"| Taker
    Maker <-->|"public commitments, nonces, partials"| Taker
    ClaimProjection["Reference actor durable BTC and LEZ claim effects"] -->|"one authorized exact send"| Core
    ClaimProjection -->|"one authorized exact send"| Sequencer
    Core -->|"finalized exact bytes at signed depth"| ClaimProjection
    Indexer -->|"bounded finalized presence or absence"| ClaimProjection
    MakerSigners --> ClaimProjection
    TakerSigners --> ClaimProjection
    ClaimProjection -->|"run D revision 4 complete"| Maker
    ClaimProjection -->|"run D revision 4 complete"| Taker
    Maker -->|"schema-3 recover"| RefundProjection["Deterministic actor refund recovery"]
    Taker -->|"schema-3 recover"| RefundProjection
    RefundProjection -->|"owner one-attempt exact refund"| Core
    RefundProjection -->|"owner one-attempt RefundNative"| Sequencer
    Core -->|"exact finalized refund bytes"| RefundProjection
    Indexer -->|"finalized refund state and position"| RefundProjection
    RefundProjection -->|"run H revision 3 then 4 refunded"| Maker
    RefundProjection -->|"run H revision 3 then 4 refunded"| Taker
    Indexer --> Finality["Sequential finality and witness auditor"]
    Core --> Finality
    Finality -->|"exact chain signature"| Recover["Point-checked scalar recovery"]
    Recover -->|"complete opposite persisted presignature"| Core
    Recover -->|"complete opposite persisted presignature"| Sequencer
```

Every listener is a dynamic literal-loopback endpoint. The Core P2P port is not
published, network activity is disabled, and the retained run had zero peers.
LEZ uses one run-owned Docker network with no public provider. Runtime funds are
deterministic Regtest coinbase outputs and deterministic local genesis Vault
allocations. No public RPC, faucet, public peer, or public fund participates.
The Core provisioner alone has wallet/mining authority. Maker and Taker actors
have distinct restricted `rpcauth` files; each LEZ sidecar has a different
run/role capability, signer, listener, and durable request store. The Taker's
first effect is deliberately external at this PoC boundary. The opposite lock
is a Maker actor effect, not a controller or provisioner shortcut.

## Prerequisites and builds

Repeat the public actor lifecycle component without Docker or chain nodes:

~~~sh
cargo test --locked -p btc-reference-actor --all-targets
~~~

The focused suite exercises the public `activate`, `drive`, `recover`, and
`status` surface through both lock projections and the injected actor
claim-observation seam through revisions three and four. It uses separate private maker/taker
configs, proves offline terminal status and idempotent activation, retains a
finalized LEZ ancestry tip, and covers both directions and roles. Revision
three reruns all activation material, validates the exact related public
signature, reproduces it from the taker's private scalar or extracts and
point-checks that scalar from the maker's persisted presignature, and stores
only its one-way commitment. Revision four reaches `Completed`; offline status
then reports next action `complete`. Deterministic observers and loopback-only
placeholder routes are used, so no public RPC, faucet, public funds, chain
peer, or external service participates. The current gate contains 49 library
cases and eight CLI integrations. Its refund cases use deterministic chain
ports and prove the same public command and durable journals without themselves
starting nodes; Run H separately proves the fresh actual-node composition. The
Bitcoin-effect cases additionally prove that the
taker alone owns a revealing Bitcoin claim at revision two and the maker alone
owns a follow-up Bitcoin claim at revision three. Exact public bytes are
persisted before one send, `Started` and `Unknown` remain observe-only across
restart, mismatched chain bytes are uncertain, and local projection waits for
exact finalized Core evidence. The maker derives its follow-up by re-extracting
the scalar from the durable revision-three revealing witness; no scalar is
persisted or logged.

Repeat the fixture-only generate/prepare/finalize boundary without a node or
Docker service:

~~~sh
cargo test --locked -p btc-local-poc-provision --all-targets
cargo clippy --locked -p btc-local-poc-provision --all-targets --all-features -- -D warnings
~~~

Eleven tests cover both `taker_sells_foreign` and `taker_sells_lez`, canonical
agreement replay, genuine rawtr funding authorization, public/private
crosswires, malformed or drifted funding, authority/key and recovery drift,
strict JSON, create-new output behavior, owner-only modes, unsafe-link
rejection, and stdout secret scanning. They use temporary files and supplied
deterministic facts; they do not call Core, LEZ,
the isolated NSSA helper process, a public endpoint, a faucet, or Docker. The
commands prove the provisioner boundary, not that an operator supplied truthful
node facts or that the combined actual-node harness invokes both stages.

Before starting either node, the durable Bitcoin lifecycle component can be
repeated independently:

~~~sh
cargo test --locked -p lez-swap-store --test btc_recovery -- --nocapture
~~~

The eleven cases create separate owner-private temporary SQLite databases for
maker and taker, project both directions through the four exact revisions,
close/reopen and reconstruct `Completed`, exercise replay/CAS/rollback and
mutation failures, and scan SQLite/WAL for the recovered-scalar sentinel. They
use synthetic evidence that represents already-validated chain-adapter output;
there is no RPC, node, Docker container, faucet, public endpoint, or external
availability dependency in this component gate. It does not substitute for
the separate actual-node proof; historical run-n supplies schema-3 claim proof,
run D supplies schema-4 Maker-lock happy proof, and Run H supplies the ordered
two-lock refund proof for both roles and directions.

Repeat the agreement-derived signing context and the new recovery seams:

~~~sh
cargo test --locked -p lez-btc-swap-sdk --test agreement_v1 \
  validated_agreement_derives_both_fresh_adaptor_session_contexts -- --exact
cargo test --locked -p lez-swap-store --test adaptor_session_journal \
  existing_only_open_never_creates_a_missing_signer_database -- --exact
cargo test --locked -p lez-swap-store --test public_effect_journal -- --nocapture
~~~

The claim actor retains distinct nonzero Bitcoin and LEZ session IDs and
role-local journal paths. It rederives keys, role order, messages, adaptor point,
and the Bitcoin Taproot tweak from the countersigned agreement, then requires
the existing journal identities to match. The role-runner session JSON is
manual ceremony input, not actor authority. Both Bitcoin and LEZ claim effects,
their exact observation paths, and terminal projection are composed in source
and deterministic actor tests. Run-n now proves the same boundary through fresh
two-direction actual-node public-actor execution.

The fourteen public-effect tests persist complete node-disclosable Bitcoin or
LEZ transaction bytes before authorization. For claims, only definitive
`Absent + Prepared` commits `Started`; refunds require affirmative stable
eligibility instead of treating absence as authority. A crash before or after
that RPC never re-arms `Started`; `Uncertain` and `Unknown` are observation-only. Exact
presence may reconcile `Prepared`, `Started`, or `Unknown` to `Accepted`.
Byte or effect-ID drift and contradictory terminal evidence fail closed. This
is a temporary-SQLite component gate with no RPC, Docker, faucet, peer, or
network, not an actor/submission E2E. “Public” describes node-disclosable bytes,
not an endpoint. Zcash is excluded by the typed chain enum, but raw bytes are not
secret-scanned; callers must never put secret-bearing material in this journal.

Repeat the typed Bitcoin Core boundary independently:

~~~sh
cargo test --locked -p lez-btc-core-adapter --all-targets
~~~

These 29 all-target test executions exercise the exact Core 31.1 typed DTOs,
consensus-byte cross-checks, stable-tip funding/claim observation, canonical scalar-free
evidence, wtxid/raw-byte-bound one-attempt submission semantics, already-known
and conflicting-witness outcomes, and a bounded authenticated
loopback HTTP server. They use deterministic RPC doubles and ephemeral loopback
servers, not the Dockerized Core service, a faucet, a public RPC, or public
funds. Run-n supplies actual-node happy integration; service-mode refund
integration remains a separate composed gate.

Repeat exact-ID and peerless finalized LEZ funding and claim observation
independently:

~~~sh
cargo test --locked -p lez-bridge-protocol --all-targets
cargo test --locked -p lez-bridge-client --all-targets
cargo +1.96.0 test --locked --manifest-path compat/lez-v0_2-sidecar/Cargo.toml --all-targets
~~~

The peerless cases omit the funding or completed-claim transaction ID and
discover one canonical public transaction from the pinned program,
terms-derived accounts, roles, aggregate authority, and bounded finalized
window. Funding requires canonical `FundNative`, historical `Funded` metadata,
and exact custody at its containing block before claim. Claim additionally
binds the exact prepared transcript and signature. They prove unique success
plus distinct absence, ambiguity, and conflicting-transcript failures through
the authenticated server/runtime path. Protocol 23/23, client 2 unit plus 26
integration and 3 CLI tests, and sidecar 78/78 are GREEN.
The public `validate_prepared_witnessed_claim` unit boundary checks only that
the message is nonempty and its bytes match the official domain-separated hash.
It performs no RPC and does not prove the signature, accounts, transcript,
inclusion, finality, or other chain truth.
The tests use deterministic in-process indexer doubles and ephemeral loopback
servers; they do not contact the local devnet or any public resource. The
manual flow below repeats the same request against the retained local indexer.

Repeat the agreement-to-signer and crash-safe public-effect boundaries without
any node, Docker service, faucet, peer, or network:

~~~sh
cargo test --locked -p lez-btc-swap-sdk --test agreement_v1 \
  validated_agreement_derives_both_fresh_adaptor_session_contexts -- --exact
cargo test --locked -p lez-swap-store --test adaptor_session_journal \
  existing_only_open_never_creates_a_missing_signer_database -- --exact
cargo test --locked -p lez-swap-store --test public_effect_journal -- --nocapture
~~~

The first two gates prove that the actor can reconstruct both exact signing
contexts from the countersigned agreement and cannot manufacture a missing
signer database. The public-effect tests use temporary SQLite only. They prove
durable complete public transaction bytes, one send authorization, and
observe-only ambiguous recovery as a component boundary; they do not prove an
actor RPC submission. `public` here means node-disclosable Bitcoin or LEZ
transaction bytes, not a public endpoint. Secret-bearing Zcash material is
excluded by type, and callers must still prevent other secrets from entering
the journal because its byte field is intentionally not a secret scanner.

Run from the repository root. Use a clean or intentionally reviewed worktree
and a fresh composed identifier:

~~~sh
export M3_RUN_ID=m3-manual-$(date -u +%Y%m%d%H%M%S)
export CORE_RUN_ID="${M3_RUN_ID}-core"
export LEZ_RUN_ID="${M3_RUN_ID}-lez"
export BUILD_RUN_ID="${M3_RUN_ID}-artifact"
export PRIVATE_ROOT="${TMPDIR:-/tmp}/lez-m3-${M3_RUN_ID}"
umask 077
install -d -m 0700 "$PRIVATE_ROOT" "$PRIVATE_ROOT/evidence"
~~~

One composed ID deliberately produces service-qualified Docker RUN_ID values.
The two retained-service scripts cannot safely receive an identical literal
RUN_ID at the same time: both reject any existing container carrying that run
label. CORE_RUN_ID and LEZ_RUN_ID preserve one composed lineage while avoiding
that intentional collision. Do not bypass either script's reuse check.

Required host tools include Bash, Rust/Cargo, Docker with Compose, curl, jq,
Python 3, OpenSSL, sha256sum, xxd, a C toolchain, libclang, GPG, and the tools
checked by the three repository scripts below. The LEZ v0.2 path uses Rust
1.96.0, Risc0 3.0.5, the pinned guest builder, and locally verified
Rapidsnark/GMP libraries.
A cold machine may need network access to populate signed Bitcoin release
material, Cargo/git sources, the digest-pinned Risc0 image, and the
checksum-pinned Logos circuits archive. Runtime execution after those caches
are warm has no external network dependency.

Build the root one-shot tools:

~~~sh
cargo build --locked -p lez-adaptor-role-runner
cargo build --locked -p btc-reference-actor
cargo build --locked -p lez-btc-swap-sdk --example btc-core-p2tr-fixture
cargo build --locked -p lez-bridge-client --example m3_witnessed_lez_operator
export ROLE_RUNNER="$PWD/target/debug/lez-adaptor-role-runner"
export BTC_ACTOR="$PWD/target/debug/btc-reference-actor"
export BTC_FIXTURE="$PWD/target/debug/examples/btc-core-p2tr-fixture"
export LEZ_OPERATOR="$PWD/target/debug/examples/m3_witnessed_lez_operator"
~~~

Build the official-wire sidecar and Vault Claim binaries in an isolated target.
Use the same verified Rapidsnark directory used by the full verifier:

~~~sh
export RAPIDSNARK_LIB_DIR=/absolute/path/to/verified/rapidsnark-v0.0.8-libraries
export BINDGEN_EXTRA_CLANG_ARGS=-I/usr/lib/gcc/x86_64-linux-gnu/13/include
export SIDECAR_TARGET="${TMPDIR:-/tmp}/lez-v02-sidecar-${BUILD_RUN_ID}"
CARGO_TARGET_DIR="$SIDECAR_TARGET" CARGO_NET_OFFLINE=true cargo +1.96.0 build --locked --offline --manifest-path compat/lez-v0_2-sidecar/Cargo.toml --bin lez-v02-bridge-poc --bin lez-v02-vault-claim-poc --example lez-v02-local-actor-identity
export BRIDGE_BIN="$SIDECAR_TARGET/debug/lez-v02-bridge-poc"
export VAULT_BIN="$SIDECAR_TARGET/debug/lez-v02-vault-claim-poc"
export IDENTITY_PROVISIONER="$SIDECAR_TARGET/debug/examples/lez-v02-local-actor-identity"
~~~

If the offline build reports a missing crate, git source, circuit, or native
library, populate it through the full pinned verifier rather than removing
locked/offline flags.

Provision fresh official LEZ actor identities before creating the LEZ genesis.
The helper accepts only new output directories, sources keys from the OS random
generator, writes `lez-signer.key` and `identity.json` under owner-only
permissions, and prints only the public descriptor:

~~~sh
"$IDENTITY_PROVISIONER" --output-directory "$PRIVATE_ROOT/maker" >"$PRIVATE_ROOT/evidence/maker-public-identity.json"
"$IDENTITY_PROVISIONER" --output-directory "$PRIVATE_ROOT/taker" >"$PRIVATE_ROOT/evidence/taker-public-identity.json"
chmod 0600 "$PRIVATE_ROOT/evidence/"*-public-identity.json
export LEZ_V02_MAKER_ACCOUNT_ID="$(jq -er '.account_id' "$PRIVATE_ROOT/maker/identity.json")"
export LEZ_V02_TAKER_ACCOUNT_ID="$(jq -er '.account_id' "$PRIVATE_ROOT/taker/identity.json")"
export LEZ_V02_MAKER_VAULT_ACCOUNT_ID="$(jq -er '.vault_account_id' "$PRIVATE_ROOT/maker/identity.json")"
export LEZ_V02_TAKER_VAULT_ACCOUNT_ID="$(jq -er '.vault_account_id' "$PRIVATE_ROOT/taker/identity.json")"
test "$(jq -er '.version' "$PRIVATE_ROOT/maker/identity.json")" = 2
test "$(jq -er '.version' "$PRIVATE_ROOT/taker/identity.json")" = 2
test "$LEZ_V02_MAKER_ACCOUNT_ID" != "$LEZ_V02_TAKER_ACCOUNT_ID"
test "$LEZ_V02_MAKER_VAULT_ACCOUNT_ID" != "$LEZ_V02_TAKER_VAULT_ACCOUNT_ID"
~~~

Never print either signer file. The provisioner refuses an existing output
path instead of reusing or overwriting an identity. Identity schema version 2
contains public `account_id`, `account_id_hex`, `vault_account_id`,
`vault_account_id_hex`, and `x_only_public_key` fields. Official LEZ code
derives each Vault ID from its owner and the Vault program. The stack must
receive each owner and derived Vault override as a pair before genesis.

## Generate the agreement fixture before funding

Run this whole three-command procedure once per direction with a fresh output
root. Generation belongs after the official maker/taker LEZ owner accounts exist but
before creating the Bitcoin funding output: its public specification contains
the direction-specific P2TR scripts that Core must actually fund. It contacts
no node and does not invent later chain facts.

~~~sh
export DIRECTION_NAME=taker_sells_foreign
case "$DIRECTION_NAME" in
  taker_sells_foreign|taker_sells_lez) ;;
  *) exit 2 ;;
esac
export REFUND_CSV_BLOCKS=144
install -d -m 0700 "$PRIVATE_ROOT/directions"
export DIRECTION="$(realpath -m "$PRIVATE_ROOT/directions/$DIRECTION_NAME")"
export AGREEMENT_PLANNING="$PRIVATE_ROOT/evidence/$DIRECTION_NAME-agreement-planning.json"
export AGREEMENT_GENERATE_SUMMARY="$PRIVATE_ROOT/evidence/$DIRECTION_NAME-agreement-generate.json"
test ! -e "$DIRECTION"
test ! -e "$AGREEMENT_PLANNING"
test ! -e "$AGREEMENT_GENERATE_SUMMARY"
(
  set -o noclobber
  jq -n \
    --arg maker "$LEZ_V02_MAKER_ACCOUNT_ID" \
    --arg taker "$LEZ_V02_TAKER_ACCOUNT_ID" \
    --argjson csv "$REFUND_CSV_BLOCKS" \
    '{schema_version:1,maker_lez_owner_account:$maker,taker_lez_owner_account:$taker,refund_csv_blocks:$csv}' \
    >"$AGREEMENT_PLANNING"
  chmod 0600 "$AGREEMENT_PLANNING"
  cargo run --locked -p btc-local-poc-provision -- generate \
    --planning-file "$AGREEMENT_PLANNING" \
    --output-root "$DIRECTION" \
    >"$AGREEMENT_GENERATE_SUMMARY"
  chmod 0600 "$AGREEMENT_GENERATE_SUMMARY"
)
export STAGE1_PUBLIC_SPEC="$(jq -er '.public_spec_file' "$AGREEMENT_GENERATE_SUMMARY")"
export STAGE1_PUBLIC_SHA256="$(jq -er '.public_spec_sha256' "$AGREEMENT_GENERATE_SUMMARY")"
test "$STAGE1_PUBLIC_SPEC" = "$DIRECTION/public-spec.json"
test "$(jq -er '.private_material_disclosed' "$AGREEMENT_GENERATE_SUMMARY")" = false
~~~

The normalized absolute output root must not exist; its parent must already be
canonical. Stage one creates the root and `private/` as mode `0700`, then uses
`create_new`, `O_NOFOLLOW`, mode `0600`, and a single-link check for the public
specification and seven fresh OS-random secret files: maker/taker signing,
refund, and claim-destination keys plus the adaptor scalar. The public one-line
summary contains no private material. Re-running `generate` against the same
root fails instead of reusing or overwriting any file. Create-new writes are
fail-safe but the multi-file stage is not one filesystem transaction: after an
interruption, retire the entire incomplete root and generate a fresh one rather
than deleting selected files or attempting repair.

Derive the aggregate LEZ authority with the exact helper named by that summary:

~~~sh
jq -e '
  .lez_authority_helper.manifest_path == "compat/lez-v0_2-sidecar/Cargo.toml" and
  .lez_authority_helper.package == "lez-v0-2-sidecar" and
  .lez_authority_helper.example == "lez-v02-account-id" and
  .lez_authority_helper.result_schema == "lez-v0.2-nssa-account-id" and
  .lez_authority_helper.result_version == 1
' "$AGREEMENT_GENERATE_SUMMARY"
export LEZ_AUTHORITY_X_ONLY="$(jq -er '.lez_authority_helper.argument' "$AGREEMENT_GENERATE_SUMMARY")"
export LEZ_AUTHORITY_MAPPING="$PRIVATE_ROOT/evidence/$DIRECTION_NAME-lez-authority.json"
test ! -e "$LEZ_AUTHORITY_MAPPING"
(
  set -o noclobber
  CARGO_TARGET_DIR="$SIDECAR_TARGET" CARGO_NET_OFFLINE=true \
    cargo +1.96.0 run --locked --offline \
      --manifest-path compat/lez-v0_2-sidecar/Cargo.toml \
      --package lez-v0-2-sidecar \
      --example lez-v02-account-id -- \
      "$LEZ_AUTHORITY_X_ONLY" >"$LEZ_AUTHORITY_MAPPING"
  chmod 0600 "$LEZ_AUTHORITY_MAPPING"
)
jq -e --arg key "$LEZ_AUTHORITY_X_ONLY" '
  .schema == "lez-v0.2-nssa-account-id" and
  .version == 1 and
  .x_only_public_key == $key and
  (.account_id | type == "string" and length == 64)
' "$LEZ_AUTHORITY_MAPPING"
~~~

This manifest-qualified Rust 1.96 command deliberately executes in
`compat/lez-v0_2-sidecar` with its separate target and official pinned `nssa`
graph. The root provisioner does not depend on `nssa` and must not copy its
derivation. Stage two binds the helper's exact schema, version, key, and account;
the live pinned LEZ prepare path independently rejects a wrong authority
account. Do not substitute a locally reimplemented hash or account mapping.

## Start both retained local chains

Use separate terminals. Start Core in service mode and retain it:

~~~sh
RUN_ID="$CORE_RUN_ID" BITCOIN_CORE_E2E_MODE=service BITCOIN_CORE_E2E_KEEP_RUNNING=1 BITCOIN_CORE_E2E_REQUIRE_CLEAN=0 ./scripts/run-bitcoin-core-e2e.sh
~~~

Require the service evidence to report Core 31.1, Regtest height 101, zero
peers, networkactive false, synced txindex and txospenderindex, a mature
5,000,000,000-sat local output, distinct maker/taker rpcauth files, and a
literal-loopback RPC.

Start and retain the exact LEZ v0.2 stack:

~~~sh
RUN_ID="$LEZ_RUN_ID" LEZ_V02_KEEP_RUNNING=1 \
  LEZ_V02_MAKER_ACCOUNT_ID="$LEZ_V02_MAKER_ACCOUNT_ID" \
  LEZ_V02_MAKER_VAULT_ACCOUNT_ID="$LEZ_V02_MAKER_VAULT_ACCOUNT_ID" \
  LEZ_V02_TAKER_ACCOUNT_ID="$LEZ_V02_TAKER_ACCOUNT_ID" \
  LEZ_V02_TAKER_VAULT_ACCOUNT_ID="$LEZ_V02_TAKER_VAULT_ACCOUNT_ID" \
  ./scripts/run-lez-v02-stack.sh
~~~

Require three run-owned services, an advancing signed channel, a non-genesis
finalized indexer tip, equal sequencer/indexer block identity at the readiness
height, and distinct maker/taker owner and Vault allocations. KEEP_RUNNING does
not make service readiness an actor-funding claim.

In the operator terminal, hand off only through the run-owned manifests:

~~~sh
export CORE_RUN_DIR="$PWD/.e2e/$CORE_RUN_ID/bitcoin-core"
export LEZ_RUN_DIR="$PWD/.e2e/$LEZ_RUN_ID/lez-v02"
test -f "$CORE_RUN_DIR/run.env"
test -f "$LEZ_RUN_DIR/run.env"
. "$CORE_RUN_DIR/run.env"
export CORE_RPC_URL="$BITCOIN_CORE_RPC_URL"
export MAKER_CORE_CONFIG="$BITCOIN_CORE_MAKER_CURL_CONFIG"
export TAKER_CORE_CONFIG="$BITCOIN_CORE_TAKER_CURL_CONFIG"
export MAKER_CORE_BASIC="$BITCOIN_CORE_MAKER_BASIC_CREDENTIALS"
export TAKER_CORE_BASIC="$BITCOIN_CORE_TAKER_BASIC_CREDENTIALS"
export CORE_FUNDING_FILE="$BITCOIN_CORE_FUNDING_CREDENTIALS"
. "$LEZ_RUN_DIR/run.env"
export SEQUENCER_URL="$LEZ_SEQUENCER_RPC_URL"
export INDEXER_URL="$LEZ_INDEXER_RPC_URL"
export BEDROCK_URL="$BEDROCK_RPC_URL"
export LEZ_CHAIN_ID="$LEZ_V02_CHANNEL_PUBLIC_KEY"
test "$LEZ_V02_MAKER_ACCOUNT_ID" = "$(jq -er '.account_id' "$PRIVATE_ROOT/maker/identity.json")"
test "$LEZ_V02_TAKER_ACCOUNT_ID" = "$(jq -er '.account_id' "$PRIVATE_ROOT/taker/identity.json")"
test "$LEZ_V02_MAKER_VAULT_ACCOUNT_ID" = "$(jq -er '.vault_account_id' "$PRIVATE_ROOT/maker/identity.json")"
test "$LEZ_V02_TAKER_VAULT_ACCOUNT_ID" = "$(jq -er '.vault_account_id' "$PRIVATE_ROOT/taker/identity.json")"
~~~

Never print or commit either curl config, Basic credential file, or the Core
funding credential file. The typed adapter consumes the role's Basic file and
never the provisioner's cookie or funding authority. Both Basic files must be
distinct owner-private regular files; the adapter rejects permissions other
than `0600`, hard links, symlinks, and file replacement or change while reading.
Read only the public txid, vout, value, and mining address fields needed by the
fixture. The same file also contains a local test secret.

## Verify and deploy the exact M3 guest

The full verifier is the authoritative build, test, lint, rustdoc, dependency,
and artifact-identity gate:

~~~sh
export LEZ_V02_ARTIFACT_TARGET_DIR="${TMPDIR:-/tmp}/lez-v02-artifact-${BUILD_RUN_ID}"
RUN_ID="$BUILD_RUN_ID" LEZ_V02_ARTIFACT_TARGET_DIR="$LEZ_V02_ARTIFACT_TARGET_DIR" RAPIDSNARK_LIB_DIR="$RAPIDSNARK_LIB_DIR" BINDGEN_EXTRA_CLANG_ARGS="$BINDGEN_EXTRA_CLANG_ARGS" ./scripts/verify-lez-v02-provisional.sh
export DEPLOYER="$LEZ_V02_ARTIFACT_TARGET_DIR/debug/lez-zec-escrow-v02-deployer"
~~~

Require ELF SHA-256
a199c5be062adcb27cf63c62d9f5688b37058b4699ce7e1767fd26eeceb5e293
and ImageID/ProgramId
39b6a4db85374de9359ea82164ef415019919475f656d597c5ab2231bc104dec.
The verifier must finish all guest, methods, deployer, Clippy, rustdoc, source,
feature, license, and advisory-policy checks.

Deploy once to the explicit local sequencer:

~~~sh
install -d -m 0700 "$PRIVATE_ROOT/deployment"
"$DEPLOYER" deploy-local --rpc-url "$SEQUENCER_URL" --channel-id "$LEZ_CHAIN_ID" --timeout-seconds 300 >"$PRIVATE_ROOT/deployment/deployment.json"
chmod 0600 "$PRIVATE_ROOT/deployment/deployment.json"
export ESCROW_PROGRAM_ID="$(jq -er '.preflight.image_id' "$PRIVATE_ROOT/deployment/deployment.json")"
jq -e --arg expected 39b6a4db85374de9359ea82164ef415019919475f656d597c5ab2231bc104dec '.preflight.image_id == $expected and .transaction_hash != null and .inclusion_block_id > 0 and .inclusion_block_hash != null' "$PRIVATE_ROOT/deployment/deployment.json" >/dev/null
~~~

deploy-local performs one submission attempt. If it times out or returns an
ambiguous failure after possible I/O, do not run it again on that chain.
Inspect the exact transaction through the sequencer and indexer. If its state
cannot be proved, abandon this chain and start a fresh service-qualified run.

Independently prove deployment finality through the indexer, sequentially:

~~~sh
rpc() {
  curl --fail --silent --show-error --connect-timeout 2 --max-time 30 -H 'content-type: application/json' --data "$2" "$1"
}
DEPLOY_TX="$(jq -er '.transaction_hash' "$PRIVATE_ROOT/deployment/deployment.json")"
DEPLOY_BLOCK="$(jq -er '.inclusion_block_id' "$PRIVATE_ROOT/deployment/deployment.json")"
DEPLOY_HASH="$(jq -er '.inclusion_block_hash' "$PRIVATE_ROOT/deployment/deployment.json")"
rpc "$INDEXER_URL" '{"jsonrpc":"2.0","id":1,"method":"getLastFinalizedBlockId","params":[]}' >"$PRIVATE_ROOT/deployment/finalized-tip.json"
jq -e --argjson block "$DEPLOY_BLOCK" '.result >= $block' "$PRIVATE_ROOT/deployment/finalized-tip.json" >/dev/null
rpc "$INDEXER_URL" "$(jq -cn --argjson block "$DEPLOY_BLOCK" '{jsonrpc:"2.0",id:1,method:"getBlockById",params:[$block]}')" >"$PRIVATE_ROOT/deployment/block-by-id.json"
rpc "$INDEXER_URL" "$(jq -cn --arg hash "$DEPLOY_HASH" '{jsonrpc:"2.0",id:1,method:"getBlockByHash",params:[$hash]}')" >"$PRIVATE_ROOT/deployment/block-by-hash.json"
jq -e --arg tx "$DEPLOY_TX" --arg hash "$DEPLOY_HASH" --argjson block "$DEPLOY_BLOCK" '.result.header.block_id == $block and .result.header.hash == $hash and .result.bedrock_status == "Finalized" and ([.result.body.transactions[] | select(.Public.hash == $tx)] | length) == 1' "$PRIVATE_ROOT/deployment/block-by-id.json" >/dev/null
test "$(jq -S -c '.result' "$PRIVATE_ROOT/deployment/block-by-id.json")" = "$(jq -S -c '.result' "$PRIVATE_ROOT/deployment/block-by-hash.json")"
~~~

Do not parallelize indexer block reads. The retained run saw intermittent local
timeouts under parallel heavy reads.

## Deterministic local Vault onboarding

The stack allocates funds to each actor's Vault, not directly to its owner
account. Both Vault Claims must be finalized before preparing either swap
transcript.

The fresh identities provisioned before stack startup already match the maker
and taker account IDs emitted in the LEZ run manifest. Keep those signer files
inside the private run root and create only the role-local state directories:

~~~sh
install -d -m 0700 "$PRIVATE_ROOT/maker/vault-state" "$PRIVATE_ROOT/taker/vault-state"
test "$(stat -c '%a' "$PRIVATE_ROOT/maker/lez-signer.key")" = 600
test "$(stat -c '%a' "$PRIVATE_ROOT/taker/lez-signer.key")" = 600
~~~

Run each Claim in a separate role process. The CLI durably records
AttemptStarted before one generated-RPC submission and reports only Admitted,
Rejected, Unknown, or observe-only replay. Never blind-retry Unknown.

~~~sh
"$VAULT_BIN" --role maker --run-id "$M3_RUN_ID" --request-id maker-vault-claim-001 --state-directory "$PRIVATE_ROOT/maker/vault-state" --private-key-file "$PRIVATE_ROOT/maker/lez-signer.key" --sequencer-url "$SEQUENCER_URL" --chain-id "$LEZ_CHAIN_ID" --escrow-program-id "$ESCROW_PROGRAM_ID" --allocation 100000 >"$PRIVATE_ROOT/evidence/maker-vault-claim.json"
"$VAULT_BIN" --role taker --run-id "$M3_RUN_ID" --request-id taker-vault-claim-001 --state-directory "$PRIVATE_ROOT/taker/vault-state" --private-key-file "$PRIVATE_ROOT/taker/lez-signer.key" --sequencer-url "$SEQUENCER_URL" --chain-id "$LEZ_CHAIN_ID" --escrow-program-id "$ESCROW_PROGRAM_ID" --allocation 200000 >"$PRIVATE_ROOT/evidence/taker-vault-claim.json"
chmod 0600 "$PRIVATE_ROOT/evidence/"*-vault-claim.json
jq -e '.submission.decision == "admitted" and .durable_state == "admitted" and .before.owner.balance == 0 and .before.vault.balance == .allocation' "$PRIVATE_ROOT/evidence/maker-vault-claim.json" >/dev/null
jq -e '.submission.decision == "admitted" and .durable_state == "admitted" and .before.owner.balance == 0 and .before.vault.balance == .allocation' "$PRIVATE_ROOT/evidence/taker-vault-claim.json" >/dev/null
~~~

For each returned transaction ID, apply the sequential finality recipe below.
At that exact finalized block query getAccountAtBlock with the base58 owner and
Vault IDs from run.env. Require owner balance equal to its allocation, owner
nonce 1, authenticated-transfer program ownership, Vault balance 0, and Vault
nonce 0. The indexer account API expects base58 account IDs; the sequencer and
bridge protocol use their canonical hex forms. Do not interchange them.

## Create role runtimes and start sidecars

Query indexer block zero once and save its hash:

~~~sh
rpc "$INDEXER_URL" '{"jsonrpc":"2.0","id":1,"method":"getBlockById","params":[0]}' >"$PRIVATE_ROOT/evidence/lez-genesis.json"
export LEZ_GENESIS_HASH="$(jq -er '.result.header.hash' "$PRIVATE_ROOT/evidence/lez-genesis.json")"
export MAKER_ACCOUNT_HEX="$(jq -er '.before.owner.account_id' "$PRIVATE_ROOT/evidence/maker-vault-claim.json")"
export TAKER_ACCOUNT_HEX="$(jq -er '.before.owner.account_id' "$PRIVATE_ROOT/evidence/taker-vault-claim.json")"
~~~

Create strict public runtime descriptors and private capabilities:

~~~sh
for role in maker taker; do
  openssl rand -hex 32 >"$PRIVATE_ROOT/$role/sidecar.capability"
  chmod 0600 "$PRIVATE_ROOT/$role/sidecar.capability"
  install -d -m 0700 "$PRIVATE_ROOT/$role/bridge-state"
done
jq -n --arg chain "$LEZ_CHAIN_ID" --arg genesis "$LEZ_GENESIS_HASH" --arg program "$ESCROW_PROGRAM_ID" --arg signer "$MAKER_ACCOUNT_HEX" '{sidecar_role:"maker",compatibility:"lee_v0_2_0",chain_id:$chain,channel_id:$chain,genesis_block_hash:$genesis,escrow_program_id:$program,signer_account_id:$signer}' >"$PRIVATE_ROOT/maker/runtime.json"
jq -n --arg chain "$LEZ_CHAIN_ID" --arg genesis "$LEZ_GENESIS_HASH" --arg program "$ESCROW_PROGRAM_ID" --arg signer "$TAKER_ACCOUNT_HEX" '{sidecar_role:"taker",compatibility:"lee_v0_2_0",chain_id:$chain,channel_id:$chain,genesis_block_hash:$genesis,escrow_program_id:$program,signer_account_id:$signer}' >"$PRIVATE_ROOT/taker/runtime.json"
chmod 0600 "$PRIVATE_ROOT/maker/runtime.json" "$PRIVATE_ROOT/taker/runtime.json"
export AUTH_TRANSFER_PROGRAM_ID=dcbbfebcd59399961ed9973b8307dc475fd4c5ca5779aacfe7588f7dbc3f4a71
~~~

Reserve two unused loopback ports. The reference run used 32857 and 32858, but
they are evidence, not defaults. Start one role per terminal and record its PID
and readiness line:

~~~sh
export MAKER_LISTEN=127.0.0.1:<fresh-maker-port>
export TAKER_LISTEN=127.0.0.1:<fresh-taker-port>
export MAKER_BRIDGE_URL="http://$MAKER_LISTEN/"
export TAKER_BRIDGE_URL="http://$TAKER_LISTEN/"
"$BRIDGE_BIN" --listen-address "$MAKER_LISTEN" --node-profile local --sequencer-url "$SEQUENCER_URL" --indexer-url "$INDEXER_URL" --run-id "$M3_RUN_ID" --runtime-file "$PRIVATE_ROOT/maker/runtime.json" --capability-file "$PRIVATE_ROOT/maker/sidecar.capability" --private-key-file "$PRIVATE_ROOT/maker/lez-signer.key" --state-directory "$PRIVATE_ROOT/maker/bridge-state" --authenticated-transfer-program-id "$AUTH_TRANSFER_PROGRAM_ID"
~~~

Run the analogous command with taker paths and TAKER_LISTEN in the other
terminal. Each must emit one ready JSON line whose role, run ID, endpoint, and
runtime match, whose indexer health is getLastFinalizedBlockId_non_genesis, and
whose finality field is not_observed_by_this_poc_bridge. Sidecar observation
proves bounded canonical inclusion and stable same-tip account effects. It does
not prove Bedrock finality; the separate indexer audit remains mandatory.

## Prepare and finalize exact funding before either chain effect

Do not broadcast the contract funding after `generate`. First deploy/finalize
the exact LEZ guest and prepare the deterministic witnessed escrow/claim pair without
submitting either transaction. Derive the direction-correct
metadata/custody/depositor/claimant accounts and retain the prepared claim hash.
Then use one actual mature Core rawtr service output to construct the exact
funding transaction offline.

The service key is stored as hexadecimal in the owner-private Core credential,
while `prepare-funding` requires a raw 32-byte mode-`0600` file. Extract it with
a direct file-to-file conversion that emits nothing to stdout and never print,
log, or commit either credential:

~~~sh
export CORE_SERVICE_INPUT_KEY="$DIRECTION/private/core-service-input.key"
test ! -e "$CORE_SERVICE_INPUT_KEY"
python3 - "$CORE_FUNDING_FILE" "$CORE_SERVICE_INPUT_KEY" <<'PY'
import os
import re
import sys

source, destination = sys.argv[1:]
values = []
with open(source, encoding="ascii") as credential:
    for line in credential:
        name, separator, value = line.rstrip("\n").partition("=")
        if separator and name == "BITCOIN_CORE_FUNDING_SECRET_KEY_HEX":
            values.append(value)
if len(values) != 1 or re.fullmatch(r"[0-9a-f]{64}", values[0]) is None:
    raise SystemExit("Core funding credential contains no unique canonical key")
secret = bytes.fromhex(values[0])
descriptor = os.open(
    destination,
    os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
    0o600,
)
try:
    if os.write(descriptor, secret) != len(secret):
        raise SystemExit("short private-key write")
finally:
    os.close(descriptor)
PY
test "$(stat -c '%a' "$CORE_SERVICE_INPUT_KEY")" = 600
test "$(stat -c '%s' "$CORE_SERVICE_INPUT_KEY")" = 32
~~~

For the first direction, take the public service outpoint/value from the same
credential. For a later direction in the same isolated Core run, use the
previous prepared funding summary's exact rawtr change output instead. In both
cases query the current Core UTXO view and derive the script from that response:

~~~sh
credential_field() {
  awk -F= -v key="$1" '$1 == key {print substr($0, length(key) + 2)}' "$CORE_FUNDING_FILE"
}
export SERVICE_INPUT_TXID="$(credential_field BITCOIN_CORE_FUNDING_TXID)"
export SERVICE_INPUT_VOUT="$(credential_field BITCOIN_CORE_FUNDING_VOUT)"
export SERVICE_INPUT_VALUE_SAT="$(credential_field BITCOIN_CORE_FUNDING_VALUE_SAT)"
# For direction two, override these three values from direction one's summary.

case "$DIRECTION_NAME" in
  taker_sells_foreign)
    export FUNDING_ROLE_CONFIG="$TAKER_CORE_CONFIG"
    ;;
  taker_sells_lez)
    export FUNDING_ROLE_CONFIG="$MAKER_CORE_CONFIG"
    ;;
esac

core_role_rpc() {
  local config="$1" method="$2" params="$3"
  curl --fail --silent --show-error --noproxy '*' --config "$config" \
    --connect-timeout 2 --max-time 30 -H 'content-type: application/json' \
    --data "$(jq -cn --arg method "$method" --argjson params "$params" \
      '{jsonrpc:"2.0",id:1,method:$method,params:$params}')" "$CORE_RPC_URL"
}

export SERVICE_INPUT_EVIDENCE="$PRIVATE_ROOT/evidence/$DIRECTION_NAME-service-input.json"
core_role_rpc "$FUNDING_ROLE_CONFIG" gettxout \
  "[\"$SERVICE_INPUT_TXID\",$SERVICE_INPUT_VOUT,true]" >"$SERVICE_INPUT_EVIDENCE"
export SERVICE_INPUT_SCRIPT="$(jq -er '.result.scriptPubKey.hex' "$SERVICE_INPUT_EVIDENCE")"
jq -e --argjson value "$SERVICE_INPUT_VALUE_SAT" \
  '.result != null and ((.result.value * 100000000 | round) == $value)' \
  "$SERVICE_INPUT_EVIDENCE" >/dev/null
~~~

Build the strict prepare document. The provisioner creates the raw transaction
only in `funding-transaction.hex`; stdout contains the same secret-free summary
written separately below, never the raw transaction or service key:

~~~sh
export CONTRACT_VALUE_SAT=1000000
export FUNDING_FEE_SAT=1000
export FUNDING_PREPARE_SPEC="$PRIVATE_ROOT/evidence/$DIRECTION_NAME-funding-prepare.json"
export FUNDING_PREPARE_SUMMARY="$PRIVATE_ROOT/evidence/$DIRECTION_NAME-funding-prepared.json"
jq -n \
  --arg stage1 "$STAGE1_PUBLIC_SHA256" \
  --arg direction "$DIRECTION_NAME" \
  --arg tx "$SERVICE_INPUT_TXID" \
  --argjson vout "$SERVICE_INPUT_VOUT" \
  --argjson value "$SERVICE_INPUT_VALUE_SAT" \
  --arg script "$SERVICE_INPUT_SCRIPT" \
  --arg secret "$CORE_SERVICE_INPUT_KEY" \
  --argjson contract "$CONTRACT_VALUE_SAT" \
  --argjson fee "$FUNDING_FEE_SAT" \
  '{schema_version:1,stage1_public_sha256:$stage1,direction:$direction,
    service_input:{transaction_id:$tx,output_index:$vout,value_sat:$value,
      script_pubkey:$script,signing_secret_key_file:$secret},
    contract_value_sat:$contract,fee_sat:$fee}' >"$FUNDING_PREPARE_SPEC"
chmod 0600 "$FUNDING_PREPARE_SPEC"
cargo run --locked -p btc-local-poc-provision -- prepare-funding \
  --spec-file "$FUNDING_PREPARE_SPEC" \
  --output-root "$DIRECTION" >"$FUNDING_PREPARE_SUMMARY"
chmod 0600 "$FUNDING_PREPARE_SUMMARY"
test "$(jq -er '.private_material_disclosed' "$FUNDING_PREPARE_SUMMARY")" = false
test "$(jq -er '.node_state_asserted' "$FUNDING_PREPARE_SUMMARY")" = false
test "$(stat -c '%a' "$DIRECTION/funding-transaction.hex")" = 600
~~~

Ask Core for current read-only policy on those exact persisted bytes. Keep the
raw bytes out of terminal output by using a request file. `allowed: true` is
not a reservation, submission, confirmation, or finality statement:

~~~sh
export FUNDING_POLICY_REQUEST="$PRIVATE_ROOT/evidence/$DIRECTION_NAME-funding-policy-request.json"
export FUNDING_POLICY_RESPONSE="$PRIVATE_ROOT/evidence/$DIRECTION_NAME-funding-policy-response.json"
jq -n --rawfile raw "$DIRECTION/funding-transaction.hex" \
  '{jsonrpc:"2.0",id:1,method:"testmempoolaccept",params:[[($raw | gsub("[\\r\\n]";""))]]}' \
  >"$FUNDING_POLICY_REQUEST"
chmod 0600 "$FUNDING_POLICY_REQUEST"
curl --fail --silent --show-error --noproxy '*' --config "$FUNDING_ROLE_CONFIG" \
  --connect-timeout 2 --max-time 30 -H 'content-type: application/json' \
  --data-binary "@$FUNDING_POLICY_REQUEST" "$CORE_RPC_URL" >"$FUNDING_POLICY_RESPONSE"
chmod 0600 "$FUNDING_POLICY_RESPONSE"
jq -e --arg txid "$(jq -er '.transaction_id' "$FUNDING_PREPARE_SUMMARY")" \
  --arg wtxid "$(jq -er '.witness_transaction_id' "$FUNDING_PREPARE_SUMMARY")" \
  '.result[0].allowed == true and .result[0].txid == $txid and .result[0].wtxid == $wtxid' \
  "$FUNDING_POLICY_RESPONSE" >/dev/null

core_role_rpc "$FUNDING_ROLE_CONFIG" getblockhash '[0]' \
  >"$PRIVATE_ROOT/evidence/$DIRECTION_NAME-core-genesis.json"
core_role_rpc "$FUNDING_ROLE_CONFIG" getblockcount '[]' \
  >"$PRIVATE_ROOT/evidence/$DIRECTION_NAME-core-tip-before-funding.json"
export CORE_GENESIS_HASH="$(jq -er '.result' "$PRIVATE_ROOT/evidence/$DIRECTION_NAME-core-genesis.json")"
export PLANNED_BTC_FUNDING_ANCHOR_HEIGHT="$((
  $(jq -er '.result' "$PRIVATE_ROOT/evidence/$DIRECTION_NAME-core-tip-before-funding.json") + 1
))"
export BTC_REFUND_HEIGHT="$((PLANNED_BTC_FUNDING_ANCHOR_HEIGHT + REFUND_CSV_BLOCKS))"
~~~

Now finalize the agreement using the exact prepared funding bytes, the planned
next-block anchor, finalized LEZ deployment identity, prepared claim facts, and
typed recovery schedule. The provisioner still performs no RPC; its
cryptographic proof and Core's node-state proof remain deliberately separate.

~~~sh
export PLANNED_BTC_SCRIPT="$(jq -er --arg direction "$DIRECTION_NAME" '.contracts[$direction].script_pubkey' "$STAGE1_PUBLIC_SPEC")"
: "${SWAP_ID:?64-hex fresh swap ID}"
: "${CORE_GENESIS_HASH:?actual Core getblockhash 0}"
: "${REQUIRED_BTC_CONFIRMATIONS:?signed confirmation policy}"
: "${BTC_CLAIM_VALUE_SAT:?planned fee-subtracted claim value}"
: "${LEZ_CHAIN_ID:?actual LEZ chain ID}"
: "${LEZ_CHANNEL_ID:?actual LEZ channel ID}"
: "${LEZ_GENESIS_BLOCK_HASH:?actual LEZ genesis hash}"
: "${ESCROW_PROGRAM_ID:?finalized checked escrow ProgramId}"
: "${AUTH_TRANSFER_PROGRAM_ID:?actual authenticated-transfer ProgramId}"
: "${LEZ_METADATA_ACCOUNT:?direction-derived metadata account}"
: "${LEZ_CUSTODY_ACCOUNT:?direction-derived custody account}"
: "${LEZ_DEPOSITOR_ACCOUNT:?direction-derived depositor account}"
: "${LEZ_CLAIMANT_ACCOUNT:?direction-derived claimant account}"
: "${LEZ_AMOUNT:?signed integer amount}"
: "${LEZ_REFUND_AT_MS:?signed LEZ refund time in milliseconds}"
: "${LEZ_PREPARED_CLAIM_MESSAGE_HASH:?official prepared claim message hash}"
: "${BTC_REFUND_HEIGHT:?funding anchor plus CSV}"
: "${EARLIER_REFUND_LATEST_UNIX_SECONDS:?earlier refund bound}"
: "${LATER_REFUND_EARLIEST_UNIX_SECONDS:?later refund bound}"
: "${REQUIRED_REFUND_MARGIN_SECONDS:?minimum cross-chain margin}"
test "$(jq -er '.contract_script_pubkey' "$FUNDING_PREPARE_SUMMARY")" = "$PLANNED_BTC_SCRIPT"
export AGREEMENT_FINALIZE_SPEC="$PRIVATE_ROOT/evidence/$DIRECTION_NAME-agreement-finalize.json"
export AGREEMENT_FINALIZE_SUMMARY="$PRIVATE_ROOT/evidence/$DIRECTION_NAME-agreement-finalized.json"
test ! -e "$AGREEMENT_FINALIZE_SPEC"
test ! -e "$AGREEMENT_FINALIZE_SUMMARY"
test ! -e "$DIRECTION/agreement.borsh"
test ! -e "$DIRECTION/agreement-summary.json"
(
  set -o noclobber
  jq -n \
    --arg stage1 "$STAGE1_PUBLIC_SHA256" \
    --arg swap "$SWAP_ID" \
    --arg direction "$DIRECTION_NAME" \
    --arg core_genesis "$CORE_GENESIS_HASH" \
    --argjson required_btc "$REQUIRED_BTC_CONFIRMATIONS" \
    --rawfile funding_signed "$DIRECTION/funding-transaction.hex" \
    --arg funding_sha "$(jq -er '.signed_transaction_sha256' "$FUNDING_PREPARE_SUMMARY")" \
    --argjson funding_input_value "$(jq -er '.input_value_sat' "$FUNDING_PREPARE_SUMMARY")" \
    --arg funding_input_script "$(jq -er '.input_script_pubkey' "$FUNDING_PREPARE_SUMMARY")" \
    --arg funding_txid "$(jq -er '.transaction_id' "$FUNDING_PREPARE_SUMMARY")" \
    --argjson funding_vout "$(jq -er '.contract_output_index' "$FUNDING_PREPARE_SUMMARY")" \
    --argjson funding_value "$(jq -er '.contract_value_sat' "$FUNDING_PREPARE_SUMMARY")" \
    --argjson claim_value "$BTC_CLAIM_VALUE_SAT" \
    --arg lez_chain "$LEZ_CHAIN_ID" \
    --arg lez_channel "$LEZ_CHANNEL_ID" \
    --arg lez_genesis "$LEZ_GENESIS_BLOCK_HASH" \
    --arg escrow "$ESCROW_PROGRAM_ID" \
    --arg transfer "$AUTH_TRANSFER_PROGRAM_ID" \
    --arg metadata "$LEZ_METADATA_ACCOUNT" \
    --arg custody "$LEZ_CUSTODY_ACCOUNT" \
    --arg depositor "$LEZ_DEPOSITOR_ACCOUNT" \
    --arg claimant "$LEZ_CLAIMANT_ACCOUNT" \
    --argjson amount "$LEZ_AMOUNT" \
    --argjson refund_ms "$LEZ_REFUND_AT_MS" \
    --arg claim_hash "$LEZ_PREPARED_CLAIM_MESSAGE_HASH" \
    --argjson csv "$REFUND_CSV_BLOCKS" \
    --argjson funding_anchor "$PLANNED_BTC_FUNDING_ANCHOR_HEIGHT" \
    --argjson btc_refund "$BTC_REFUND_HEIGHT" \
    --argjson earlier "$EARLIER_REFUND_LATEST_UNIX_SECONDS" \
    --argjson later "$LATER_REFUND_EARLIEST_UNIX_SECONDS" \
    --argjson margin "$REQUIRED_REFUND_MARGIN_SECONDS" \
    --slurpfile authority "$LEZ_AUTHORITY_MAPPING" \
    '{
      schema_version: 1,
      stage1_public_sha256: $stage1,
      swap_id: $swap,
      direction: $direction,
      bitcoin: {
        genesis_block_hash: $core_genesis,
        required_confirmations: $required_btc,
        funding_signed_transaction: ($funding_signed | gsub("[\\r\\n]";"")),
        funding_signed_transaction_sha256: $funding_sha,
        funding_input_value_sat: $funding_input_value,
        funding_input_script_pubkey: $funding_input_script,
        funding_transaction_id: $funding_txid,
        funding_output_index: $funding_vout,
        funding_value_sat: $funding_value,
        claim_value_sat: $claim_value
      },
      lez_runtime: {
        compatibility: "lee_v0_2_0",
        chain_id: $lez_chain,
        channel_id: $lez_channel,
        genesis_block_hash: $lez_genesis,
        escrow_program_id: $escrow,
        authenticated_transfer_program_id: $transfer
      },
      lez_terms: {
        aggregate_authority_mapping: $authority[0],
        metadata_account: $metadata,
        custody_account: $custody,
        depositor_account: $depositor,
        claimant_account: $claimant,
        amount: $amount,
        refund_at_ms: $refund_ms,
        prepared_claim_message_hash: $claim_hash
      },
      recovery: {
        refund_csv_blocks: $csv,
        planned_bitcoin_funding_anchor_height: $funding_anchor,
        bitcoin_refund_height: $btc_refund,
        earlier_refund_latest_unix_seconds: $earlier,
        later_refund_earliest_unix_seconds: $later,
        required_margin_seconds: $margin
      }
    }' >"$AGREEMENT_FINALIZE_SPEC"
  chmod 0600 "$AGREEMENT_FINALIZE_SPEC"
  cargo run --locked -p btc-local-poc-provision -- finalize \
    --spec-file "$AGREEMENT_FINALIZE_SPEC" \
    --output-root "$DIRECTION" \
    >"$AGREEMENT_FINALIZE_SUMMARY"
  chmod 0600 "$AGREEMENT_FINALIZE_SUMMARY"
)
test "$(jq -er '.direction' "$AGREEMENT_FINALIZE_SUMMARY")" = "$DIRECTION_NAME"
test "$(jq -er '.agreement_revalidated' "$AGREEMENT_FINALIZE_SUMMARY")" = true
test "$(jq -er '.private_material_disclosed' "$AGREEMENT_FINALIZE_SUMMARY")" = false
test "$(jq -er '.bitcoin_funding_authorization' "$AGREEMENT_FINALIZE_SUMMARY")" = verified
test "$(jq -er '.bitcoin_node_state' "$AGREEMENT_FINALIZE_SUMMARY")" = not_asserted
test "$(jq -er '.planned_bitcoin_funding_anchor_height' "$AGREEMENT_FINALIZE_SUMMARY")" = "$PLANNED_BTC_FUNDING_ANCHOR_HEIGHT"
export AGREEMENT_FILE="$(jq -er '.agreement_file' "$AGREEMENT_FINALIZE_SUMMARY")"
test "$AGREEMENT_FILE" = "$DIRECTION/agreement.borsh"
test "$(stat -c '%a' "$AGREEMENT_FILE")" = 600
~~~

Finalization stable-reads the stage-one public/private set and exact persisted
funding bytes, verifies the public SHA-256, and reconstructs every public key
and both P2TR/CSV contracts. It independently verifies canonical transaction
encoding/hash/txid, the rawtr prevout key/script/value, exact contract output,
bounded positive fee, one 64-byte `SIGHASH_DEFAULT` witness, and the BIP-341
Schnorr signature before it signs the agreement. It then binds the exact
official LEZ authority result, deployment/accounts/amount/refund/prepared-
message facts, planned anchor, direction-derived roles, and recovery margin,
and re-decodes a byte-identical canonical Borsh agreement.

That is cryptographic reconstruction, not node truth: finalization never
contacts Core, the sequencer, or the indexer. `gettxout` establishes the local
node's current UTXO view, `testmempoolaccept` its current read-only policy, and
post-send transaction/block reads the actual containing height and later signed
confirmation depth. None is a cross-chain proof or distributed transaction.

The generate set, funding hex/summary, and agreement/summary are create-new
owner-private groups, but no group is one filesystem transaction. Before any
effect, an interruption or partial output retires the whole direction root;
restart at `generate` with fresh secrets. After a possible broadcast, preserve
the exact root and reconcile from node evidence or execute the signed refund.
Never delete selected survivors or re-sign an already locked swap.

After finalization, complete and verify both roles' Bitcoin and LEZ journals
before actor activation or either chain submission. This maximizes atomicity:
after the first lock, either direction's opposite claim requires only persisted
local material and canonical public reveal evidence. It is not a distributed
atomic commit across the filesystem, signer journals, actor databases, Core,
and LEZ; ambiguity is handled by durable exact bytes, one-attempt authority,
observation, and fail-closed recovery.

When Bitcoin funding is direction-correctly due, re-run
`testmempoolaccept`, send only `$DIRECTION/funding-transaction.hex`, mine exactly
one isolated block, and require its containing height to equal
`$PLANNED_BTC_FUNDING_ANCHOR_HEIGHT`. A mismatch invalidates certification and
the agreement's absolute recovery schedule even though the actual Taproot CSV
leaf remains consensus-valid. Preserve evidence, stop the lifecycle, and
recover through the actual relative-CSV script when eligible; never patch the
signed agreement after the effect.

## Run the combined actual-node public-actor flow

The repository-owned composition enters through
`scripts/run-m3-actor-local-poc.sh` and delegates each direction to
`scripts/run-m3-actor-direction.sh`. Do not start the retained manual services
above when using this entry point; it creates fresh Core and LEZ child runs,
executes the directions sequentially, and cleans only resources whose exact IDs
it captured. After the pinned verifier has produced the deployer target and all
offline graphs are cached, run:

~~~sh
export RUN_ID=m3schema4-manual-001
export LEZ_V02_SOURCE_DIR=/absolute/path/to/clean/logos-execution-zone-v0.2.0
export LEZ_V02_SERVICES_DIR=/absolute/path/to/locked/release-binaries
export LEZ_V02_R0VM=/absolute/path/to/verified/r0vm
export LEZ_V02_ARTIFACT_TARGET_DIR=/absolute/path/to/verified/lez-artifact-target
export RAPIDSNARK_LIB_DIR=/absolute/path/to/verified/rapidsnark-v0.0.8-libraries
export BINDGEN_EXTRA_CLANG_ARGS=-I/usr/lib/gcc/x86_64-linux-gnu/13/include
./scripts/run-m3-actor-local-poc.sh
~~~

For audit only, retained run D used the following exact already-verified
inputs:

~~~sh
RUN_ID=m3schema4-20260717d \
LEZ_V02_SOURCE_DIR=/tmp/lez-v020-native-investigation \
LEZ_V02_SERVICES_DIR=/tmp/lez-v02-services-a58fbce2-20260713/release \
LEZ_V02_R0VM=/tmp/lez-atomic-swaps-tools/risc0-3.0.5/home/extensions/v3.0.5-cargo-risczero-x86_64-unknown-linux-gnu/r0vm \
LEZ_V02_ARTIFACT_TARGET_DIR=/tmp/lez-m3-artifact-20260715a \
RAPIDSNARK_LIB_DIR=/tmp/lez-atomic-swaps-tools/rapidsnark-v0.0.8/d4133227 \
BINDGEN_EXTRA_CLANG_ARGS=-I/usr/lib/gcc/x86_64-linux-gnu/13/include \
./scripts/run-m3-actor-local-poc.sh
~~~

Never rerun that literal command: `m3schema4-20260717d` and its root are spent
identities. A portable reproduction uses equivalent verified prerequisites and
a fresh run ID with the generic command above.

The run ID must be a fresh 8–48 character lowercase identifier. The runner
refuses an existing outer root, either child root, or same-ID Docker resource.
It validates the canonical absolute Rapidsnark directory and all four pinned
library hashes before its offline builds; `LEZ_V02_ARTIFACT_TARGET_DIR` must
already contain the verified deployer and exact checked guest. A successful
process exit is necessary but not sufficient. Check both the terminal packet
and the cleanup attestation:

~~~sh
export M3_EVIDENCE="$PWD/.e2e/${RUN_ID}/m3-actor-poc/evidence"
jq -e '.result == "passed" and
  .journey == "claim" and
  (.directions | map(.direction) ==
    ["taker_sells_foreign", "taker_sells_lez"]) and
  all(.directions[];
    .terminal_revision == 4 and .terminal_phase == "completed" and
    .maker_second_lock_effect_count == 1 and
    .expected_unique_effects == {bitcoin:2,lez:3}) and
  .actor_process_model == "fresh_one_shot_process_per_command" and
  .replay_resubmission_count == 0 and
  .services.bitcoin_core.version == "31.1" and
  .services.bitcoin_core.network == "regtest" and
  .services.lez.version == "v0.2.0" and
  .services.lez.network == "private_local" and
  .external_resources.certification_success_depends_on_external_network == false and
  .public_rpc_used == false and .faucet_used == false and
  .public_funds_used == false and .private_material_disclosed == false' \
  "$M3_EVIDENCE/m3-actor-local-poc.json"

M3_DRIVER_CONTRACT="$(M3_POC_JOURNEY=claim \
  ./scripts/run-m3-actor-direction.sh contract)"
jq -e '
  .runtime_backend == "repository_owned_actual_node_implementation" and
  .actor_config_schema_version == 4 and
  .taker_first_lock_external_runner_submission == true and
  .actor_owned_maker_lock_effects == true and
  .maker_lock_submission_actor_output == "awaiting_observation" and
  .maker_lock_restart_never_resubmits == true and
  .runner_only_confirms_actor_submitted_maker_locks == true and
  .bounded_read_only_observation_retries_never_resubmit == true' \
  <<<"$M3_DRIVER_CONTRACT"
unset M3_DRIVER_CONTRACT

jq -e '.result == "passed" and .all_exact_run_resources_absent == true and
  .foreign_resources_targeted == false and .broad_cleanup_used == false' \
  "$M3_EVIDENCE/cleanup-attestation.json"
for direction in taker_sells_foreign taker_sells_lez; do
  for role in maker taker; do
    jq -e '.state == "active" and .phase == "completed" and
      .revision == 4 and .next_action == "complete"' \
      "$M3_EVIDENCE/${direction}-${role}-terminal.json"
  done
  jq -e '.bitcoin == 2 and .lez == 3' \
    "$M3_EVIDENCE/${direction}-actual-submission-counts.json"
  jq -e '(.bitcoin_effect_ids | length) == 2 and
    (.lez_effect_ids | length) == 3 and
    .expected_unique_effects.bitcoin == 2 and
    .expected_unique_effects.lez == 3 and
    (.actor_owned_claims.bitcoin | length) > 0 and
    (.actor_owned_claims.lez | length) > 0' \
    "$M3_EVIDENCE/${direction}-actual-effects.json"
done
~~~

The contract assertion is part of the user-flow proof, not decoration. It
prevents an apparently successful terminal packet from being interpreted as
schema 4 if the direction driver instead lets the controller submit a Maker
lock. The runner itself validates the same contract before starting either
service and records the exact direction-driver hash in its terminal packet.

The default journey is `claim`. To reproduce Run H's actual-node refund
boundary, keep the same verified prerequisite variables, select `refund`, and
use a different never-before-used run ID. Change the example ID for every
attempt; successful and failed roots are both spent.

~~~sh
export RUN_ID=m3refund-manual-001
export M3_ACTOR_POC_JOURNEY=refund
./scripts/run-m3-actor-local-poc.sh

export M3_EVIDENCE="$PWD/.e2e/$RUN_ID/m3-actor-poc/evidence"
jq -e '.kind == "m3_actor_two_direction_refund_local_poc" and
  .journey == "refund" and .result == "passed" and
  (.directions | map(.direction) ==
    ["taker_sells_foreign", "taker_sells_lez"]) and
  all(.directions[];
    .terminal_revision == 4 and .terminal_phase == "refunded") and
  .actor_process_model == "fresh_one_shot_process_per_command" and
  .actor_owned_effect_semantics == "refund" and
  .expected_unique_effects_by_direction == {
    taker_sells_foreign:{bitcoin:2,lez:3},
    taker_sells_lez:{bitcoin:2,lez:3}} and
  .replay_command == "recover" and .replay_resubmission_count == 0 and
  .services.bitcoin_core.version == "31.1" and
  .services.bitcoin_core.network == "regtest" and
  .services.lez.version == "v0.2.0" and
  .services.lez.network == "private_local" and
  .services.lez.slot_duration_seconds == "3.0" and
  .public_rpc_used == false and .faucet_used == false and
  .public_funds_used == false and .private_material_disclosed == false' \
  "$M3_EVIDENCE/m3-actor-local-poc.json"
jq -e '.journey == "refund" and .result == "passed" and
  .all_exact_run_resources_absent == true and
  .foreign_resources_targeted == false and .broad_cleanup_used == false' \
  "$M3_EVIDENCE/cleanup-attestation.json"
for direction in taker_sells_foreign taker_sells_lez; do
  for role in maker taker; do
    jq -e '.state == "active" and .phase == "refunded" and
      .revision == 4 and .next_action == "complete"' \
      "$M3_EVIDENCE/${direction}-${role}-terminal.json"
  done
  jq -e '.bitcoin == 2 and .lez == 3' \
    "$M3_EVIDENCE/${direction}-actual-submission-counts.json"
  jq -e '.journey == "refund" and
    (.bitcoin_effect_ids | length) == 2 and
    (.lez_effect_ids | length) == 3 and
    (.actor_owned_refunds.bitcoin | type) == "string" and
    (.actor_owned_refunds.lez | type) == "string" and
    .cooperative_claim_effects_present == false' \
    "$M3_EVIDENCE/${direction}-actual-effects.json"
  jq -s -e '.[0] == .[1]' \
    "$M3_EVIDENCE/${direction}-submission-counts-before-replay.json" \
    "$M3_EVIDENCE/${direction}-submission-counts-after-replay.json"
done
unset M3_ACTOR_POC_JOURNEY
~~~

Run H's evidence files span 54 minutes 5 seconds from the first retained file
through cleanup. The two directions run sequentially and intentionally wait for
signed five- and fifteen-minute recovery bounds while LEZ advances in
3.0-second slots; this duration is not a performance baseline. Scheduling,
indexer finality, a moving LEZ tip, and bounded read retries can extend it.
Unavailability or timeout remains uncertain observation and never creates a new
send authority.

Keep the entire `.e2e/$RUN_ID` tree private: the public terminal packet is
secret-safe, but sibling evidence and `private/` contain credentials, keys,
complete transactions, and signer/actor databases. If the run fails, the
cleanup attestation should still prove exact cleanup; no terminal success packet
exists, and the same run ID and run root must never be reused.

## Historical schema-3 observation-only actor inspection

This section retains the exact lower-level recipe used to inspect the
historical schema-3 path. It is not the current happy-path certification
recipe. Schema 3 observes and projects an already submitted Maker lock with
zero actor attempts; it cannot own that submission. Use the one-command
schema-4 flow above, and the two direction-specific schema-4 sequences below,
when verifying current role ownership. Do not convert these historical configs
to schema 4 by changing only the version number: schema 4 requires complete
direction-shaped `maker_lock` material and its dedicated durable journal.

The actor consumes an already canonical countersigned Borsh agreement; it does
not negotiate or create one. Finalization emits that exact agreement as
`$DIRECTION/agreement.borsh`; do not copy, rewrite, or re-sign it. Require its
Bitcoin genesis, confirmation policy, LEZ channel/genesis/program/accounts,
direction, and swap terms to match the run artifacts below. Use different state
databases, role Basic files, capabilities, runtimes, and configs for maker and
taker.

This recipe is chronological only after the exact LEZ claim has been prepared,
the funding transaction has passed read-only Core policy, the agreement has
been finalized, and both fresh-process signing ceremonies below have reached
verified presignatures, but still before the first chain submission. Preparation uses
the deterministic planned LEZ funding transaction ID, so it need not wait for
that transaction to be submitted. Return here after completing direction step
4 for `TakerSellsForeign` or step 3 for `TakerSellsLez`.

For this historical inspection only, create strict private schema-3 configs
with normalized absolute paths.
`cookie_file` must point to the role's restricted Basic file, never the
provisioner cookie or funding/mining credential. Each config binds its own
existing Bitcoin and LEZ signer journal, the two distinct nonzero session IDs,
and the complete persisted `{context,claim}` prepare result—not only its
`.claim` member. The taker config additionally requires the owner-only adaptor
scalar; the maker config must omit it:

~~~sh
: "${DIRECTION:?set the private direction root}"
: "${ACTOR_DISCOVERY_START_HEIGHT:?inclusive finalized LEZ window start}"
: "${ACTOR_DISCOVERY_MAX_BLOCKS:?complete covered window length}"
test "$ACTOR_DISCOVERY_MAX_BLOCKS" -ge 1
test "$ACTOR_DISCOVERY_MAX_BLOCKS" -le 4096
export AGREEMENT_FILE="$(realpath "$DIRECTION/agreement.borsh")"
export ACTOR_ACCEPTED_AT="$(date -u +%s)"
: "${BTC_SESSION_ID:?retain the 64-hex Bitcoin session ID}"
: "${LEZ_SESSION_ID:?retain the 64-hex LEZ session ID}"
test "${#BTC_SESSION_ID}" -eq 64
test "${#LEZ_SESSION_ID}" -eq 64
test "$BTC_SESSION_ID" != "$LEZ_SESSION_ID"
test -s "$DIRECTION/prepared-claim.json"

write_actor_config() {
  local role="$1"
  local core_basic="$2"
  local bridge_url="$3"
  local runtime="$PRIVATE_ROOT/$role/runtime.json"
  local capability="$PRIVATE_ROOT/$role/sidecar.capability"
  local state_db="$DIRECTION/$role-btc-actor.sqlite3"
  local config="$DIRECTION/$role-btc-actor.json"
  local btc_journal="$DIRECTION/$role/btc-journal.sqlite"
  local lez_journal="$DIRECTION/$role/lez-journal.sqlite"
  local prepared_result="$DIRECTION/prepared-claim.json"
  local adaptor_secret=""
  local refund_source=""
  local refund_key=""

  test ! -e "$state_db"
  test -s "$btc_journal"
  test -s "$lez_journal"
  test -s "$prepared_result"
  case "$role" in
    taker)
      test -s "$DIRECTION/taker/adaptor-secret.key"
      adaptor_secret="$(realpath "$DIRECTION/taker/adaptor-secret.key")"
      ;;
    maker)
      test ! -e "$DIRECTION/maker/adaptor-secret.key"
      ;;
    *) return 2 ;;
  esac
  case "$DIRECTION_NAME:$role" in
    taker_sells_foreign:taker)
      refund_source="$DIRECTION/private/taker-refund.key"
      refund_key="$DIRECTION/taker/bitcoin-refund.hex"
      ;;
    taker_sells_lez:maker)
      refund_source="$DIRECTION/private/maker-refund.key"
      refund_key="$DIRECTION/maker/bitcoin-refund.hex"
      ;;
  esac
  if test -n "$refund_source"; then
    test -f "$refund_source"
    test "$(wc -c <"$refund_source")" -eq 32
    test ! -e "$refund_key"
    test ! -L "$refund_key"
    (
      set -o noclobber
      xxd -p -c 32 "$refund_source" >"$refund_key"
    )
    chmod 0600 "$refund_key"
    test "$(wc -c <"$refund_key")" -eq 65
    refund_key="$(realpath "$refund_key")"
  fi
  jq -n \
    --arg role "$role" \
    --arg agreement "$AGREEMENT_FILE" \
    --arg state "$(realpath -m "$state_db")" \
    --argjson accepted "$ACTOR_ACCEPTED_AT" \
    --arg core "$CORE_RPC_URL" \
    --arg basic "$(realpath "$core_basic")" \
    --arg bridge "$bridge_url" \
    --arg capability "$(realpath "$capability")" \
    --arg run "$M3_RUN_ID" \
    --argjson timeout 30000 \
    --argjson start "$ACTOR_DISCOVERY_START_HEIGHT" \
    --argjson blocks "$ACTOR_DISCOVERY_MAX_BLOCKS" \
    --arg btc_session "$BTC_SESSION_ID" \
    --arg btc_journal "$(realpath "$btc_journal")" \
    --arg lez_session "$LEZ_SESSION_ID" \
    --arg lez_journal "$(realpath "$lez_journal")" \
    --arg prepared_result "$(realpath "$prepared_result")" \
    --arg adaptor_secret "$adaptor_secret" \
    --arg refund_key "$refund_key" \
    --slurpfile runtime "$runtime" \
    '{
      schema_version: 3,
      role: $role,
      agreement_file: $agreement,
      state_db: $state,
      accepted_at_unix_seconds: $accepted,
      bitcoin_core: {
        endpoint: $core,
        cookie_file: $basic,
        connectivity: "isolated_local"
      },
      lez_bridge: {
        endpoint: $bridge,
        capability_file: $capability,
        run_id: $run,
        runtime: $runtime[0],
        request_timeout_millis: $timeout,
        discovery_start_height: $start,
        discovery_max_blocks: $blocks
      },
      signing: ({
          bitcoin: {
            session_id: $btc_session,
            journal_db: $btc_journal
          },
          lez: {
            session_id: $lez_session,
            journal_db: $lez_journal
          },
          prepared_witnessed_claim_result_file: $prepared_result
        } + (if $role == "taker"
             then {adaptor_secret_file: $adaptor_secret}
             else {}
             end)),
      refund: (if $refund_key == ""
               then {}
               else {bitcoin_refund_key_file: $refund_key}
               end)
    }' >"$config"
  chmod 0600 "$config"
}

write_actor_config maker "$MAKER_CORE_BASIC" "$MAKER_BRIDGE_URL"
write_actor_config taker "$TAKER_CORE_BASIC" "$TAKER_BRIDGE_URL"
export MAKER_BTC_ACTOR_CONFIG="$DIRECTION/maker-btc-actor.json"
export TAKER_BTC_ACTOR_CONFIG="$DIRECTION/taker-btc-actor.json"
~~~

The 30,000 ms value is a finite timeout for a read-only LEZ bridge scan, added
in commit `650d94e`. It prevents an unavailable or moving local indexer from
hanging the actor indefinitely. Timeout is uncertain observation: preserve the
persisted effect bytes and state, then retry only the observation under the
rules below. It never grants authority to resubmit initialize, fund, or claim.

The single LEZ discovery window is used by whichever direction-derived funding
transition observes LEZ. Choose an inclusive window that the finalized tip
can
fully cover and that will contain the funding transaction. Before funding
exists, the v0.2 finalized observer commonly returns an absence/window error;
the actor reports `actor first-lock observation is unavailable`. Treat that as
retryable local observation unavailability, not proof that funding is absent
and never as authorization to submit a replacement effect.

An exact retry uses the unchanged private config, deterministic observation ID,
and request. If the chosen window already contains the funding height, wait for
finality and invoke a fresh process without changing it. A deliberate bounded-
window change produces a distinct deterministic observation ID because the ID
binds the run, role, runtime, signed terms, target, and window. The complete
request is also retained and revalidated in the evidence. Change the window
only as an intentional new bounded observation, for example when the funded
block is outside the earlier window; do not mutate it merely to retry the same
request. Keep maker and taker configs on the same deliberate window.

Only `activate` inserts the agreement acceptance. Before creating revision
zero, it strict-decodes the complete prepared result, binds its run, claimant,
request, and official message hash to the agreement, opens both signer journals
existing-only, derives both contexts from the agreement and configured session
IDs, and independently verifies their local-role identities and retained
presignatures. For the taker only, it stable-reads an exact lowercase 32-byte
hex scalar from a mode-0600, single-link regular file and point-checks it against
the agreement without producing a signature. Maker configs carrying a secret
path and taker configs missing one are rejected. Missing, cross-wired,
incomplete, unsafe, or changed material fails without creating the actor
database. Private schema-1 and schema-2 configs are rejected.

An absent state path or an
empty/migrated database with no acceptance produces `not_activated` from
`status` and `actor is not activated` from `drive` or `recover`. `status` may
migrate the
schema of an existing SQLite file, but it performs no RPC and never creates an
acceptance. A corrupt database or an acceptance conflicting with role,
agreement, timestamp, or initial coordinator fails closed. Do not precreate the
state database in the normal flow.

Repeat each one-shot command and retain only its secret-free stdout:

~~~sh
for config in "$MAKER_BTC_ACTOR_CONFIG" "$TAKER_BTC_ACTOR_CONFIG"; do
  "$BTC_ACTOR" --config "$config" status
  "$BTC_ACTOR" --config "$config" activate
  "$BTC_ACTOR" --config "$config" status
done
~~~

The first status must be `not_activated`. Activation returns revision `0`,
phase `offered`, and `was_replay:false`; exact repetition returns
`was_replay:true`. Post-activation status is still revision `0` with next action
`observe_taker_first_lock` and does not require either node. `status` does not
open signer journals or the prepared-result file; activation does.

After the agreement-derived taker lock reaches the signed Bitcoin confirmation
policy or finalized LEZ funding is inside the complete configured window, run
one fresh process for each role:

~~~sh
"$BTC_ACTOR" --config "$MAKER_BTC_ACTOR_CONFIG" drive
"$BTC_ACTOR" --config "$TAKER_BTC_ACTOR_CONFIG" drive
"$BTC_ACTOR" --config "$MAKER_BTC_ACTOR_CONFIG" status
"$BTC_ACTOR" --config "$TAKER_BTC_ACTOR_CONFIG" status
~~~

Affirmative output is `observed_then_projected`, revision `1`, phase
`taker_lock_confirmed`, on the direction-correct `bitcoin` or `lez` chain.
Bitcoin evidence is the typed, stable-tip agreement funding record at the
signed depth. LEZ evidence contains the agreement commitment, complete
finalized tip, canonical funding transaction, containing block, historical
`Funded` metadata, and exact custody; the actor also compares metadata and
custody accounts with the signed agreement. The RPC observation returns before
SQLite begins the predecessor-zero projection. This is restartable local CAS,
not cross-system atomicity. If concurrent drives
race with different valid evidence, one projection wins and the other may
return `converged_on_existing_projection` after reconstructing the valid
revision-one winner; it never overwrites the winner. Other conflicts fail
closed.

Status now reports next action `observe_maker_second_lock`. Use the operator
flow below to submit and confirm/finalize the agreement-derived maker lock, then
run one fresh process for each role again:

~~~sh
"$BTC_ACTOR" --config "$MAKER_BTC_ACTOR_CONFIG" drive
"$BTC_ACTOR" --config "$TAKER_BTC_ACTOR_CONFIG" drive
"$BTC_ACTOR" --config "$MAKER_BTC_ACTOR_CONFIG" status
"$BTC_ACTOR" --config "$TAKER_BTC_ACTOR_CONFIG" status
~~~

Affirmative output is `observed_then_projected`, revision `2`, phase
`both_legs_locked`, on the opposite direction-correct chain. Observation again
returns before the predecessor-one SQLite CAS. Exact replay is idempotent;
different concurrent valid observations may only converge on an already valid
revision-two winner and never overwrite it.

Status at durable revision `2` reports `observe_revealing_claim`. The live
Bitcoin and LEZ claim branches, plus the injected deterministic seams used by
their tests, advance exact canonical evidence to revision `3`
`claim_evidence_available`; the direction-correct follow-up reaches revision
`4` `completed`, and fresh offline `status` reports `complete`. Both branches
rerun strict activation material before claim use. The taker must reproduce the
revealing signature from its private scalar, while the maker must extract and
point-check the same scalar from its persisted presignature; only
`ClaimEvidence` survives in lifecycle state.

For a Bitcoin revealing claim at revision `2`, only the taker prepares and may
submit. For a Bitcoin follow-up at revision `3`, only the maker prepares and may
submit, after re-extracting the scalar from the durable revision-three
revealing witness and its matching persisted LEZ presignature. The other role
is observation-only. Before chain I/O the actor records the exact transaction
bytes, transaction ID, agreement, role, operation, and predecessor revision in
its public-effect journal. Only a definitive unspent observation plus fresh
`Prepared` state grants one authorized Core send. `Started` or `Unknown` is
observe-only forever across restart; an exact-byte mismatch is uncertain, not
absence and not authority. An accepted send response does not advance local
lifecycle state. Projection occurs only after Core returns those exact bytes as
finalized at the agreement's signed confirmation depth.

For a LEZ revealing or follow-up claim, the direction- and revision-owning role
completes the exact aggregate signature, persists the exact transaction before
classifying presence, and may submit once only after stable bounded
`NotFound`. `Started`, `Unknown`, conflicting presence, and uncertain or moving
history grant no fresh send authority. An accepted response alone does not
project lifecycle state; a later fresh `drive` must obtain exact finalized
evidence. The other role discovers the same claim from signed terms and
transcript without accepting a peer transaction ID. This claim path is GREEN in
source, deterministic actor tests, historical run-n, and schema-4 run D. The refund path is GREEN in
deterministic tests and in Run H's fresh actual-node two-lock journey. The
lower-level operator flow below remains useful for inspecting each exact
request/effect; refund-side absent-maker and clean post-reveal survivor
execution are GREEN. Canonical containing-time enforcement is also GREEN;
schema-4 same-action Maker admission is actual-node GREEN in run D. Concurrent,
process-kill, and reorg journeys remain later gates.

## Manual actor timeout/refund recovery

Status: the public schema-3 command and both ordered refund directions are GREEN
with deterministic chain ports and fresh actual nodes in
`m3refund-20260716h`. Run `m3actor-20260716n` followed the claim path;
both retained roots and their configs are spent audit inputs, so do not modify
or reuse them. For manual inspection, start a fresh direction, use the schema-3
config recipe above, stop after both actors project revision 2
`both_legs_locked`, and do not submit either cooperative claim.

The agreement fixes the recovery order. The maker-funded leg must be durably
projected at revision 3 before the taker-funded leg can project revision 4:

| Direction | Revision 2 to 3 | Revision 3 to 4 |
|---|---|---|
| `taker_sells_foreign` | Maker LEZ refund | Taker Bitcoin refund |
| `taker_sells_lez` | Maker Bitcoin refund | Taker LEZ refund |

Run H retained that exact order. The foreign direction finalized maker LEZ
refund `a5cbb48a...97e41` in block 111 before confirming taker Bitcoin
refund `ef48682a...72886`. The LEZ direction confirmed maker Bitcoin
refund `17c652fb...56c96` before finalizing taker LEZ refund
`64e1005b...9a6b4` in block 292. In each direction the final effect manifest
contains two unique Bitcoin effects and three unique LEZ effects, names exactly
one actor-owned refund on each chain, and reports no cooperative claim. All four
role/direction terminal files are revision 4 `refunded` with next action
`complete`, and the before/after replay count packets are identical.

The provisioner output `private/<role>-refund.key` is raw 32-byte owner-private
material. The recipe converts only the agreement-selected Bitcoin funder key
with `xxd -p -c 32`, without stdout, into the distinct role-owned
`<role>/bitcoin-refund.hex` mode-`0600` file that its config names. The file
contains exactly 64 lowercase hexadecimal characters plus one newline. The
nonfunder gets no converted file and its config carries `refund:{}`. Activation
rederives the x-only public key and compares it with the countersigned participant key, so a missing, wrong,
cross-role, aliased, or unsafe key fails before recovery authority is used. LEZ
`RefundNative` remains permissionless on chain, but the actor owner/nonowner
split prevents both local roles from preparing or submitting the same effect.

Before the first deadline, invoke a fresh process for each role:

~~~sh
for config in "$MAKER_BTC_ACTOR_CONFIG" "$TAKER_BTC_ACTOR_CONFIG"; do
  "$BTC_ACTOR" --config "$config" recover
  "$BTC_ACTOR" --config "$config" status
done
~~~

At revision 2 the expected pre-deadline result is `awaiting_observation` with no
projection. For LEZ this performs only the stable state/clock read: it does not
prepare or submit. For Bitcoin the typed Core adapter does not declare the
refund eligible until the next block satisfies the signed funding anchor plus
CSV rule.

For a Bitcoin refund, advance only the isolated run-owned Core node to the exact
next-block eligibility boundary. With the guide variables above, the eligible
tip is the signed funding anchor plus the CSV delay minus one:

~~~sh
TARGET_REFUND_TIP=$((PLANNED_BTC_FUNDING_ANCHOR_HEIGHT + REFUND_CSV_BLOCKS - 1))
CURRENT_REFUND_TIP="$(docker exec "$CORE_CONTAINER" bitcoin-cli \
  -conf=/run-config/bitcoin.conf -datadir=/var/lib/bitcoin getblockcount)"
test "$CURRENT_REFUND_TIP" -le "$TARGET_REFUND_TIP"
REFUND_BLOCKS=$((TARGET_REFUND_TIP - CURRENT_REFUND_TIP))
if test "$REFUND_BLOCKS" -gt 0; then
  docker exec "$CORE_CONTAINER" bitcoin-cli -conf=/run-config/bitcoin.conf \
    -datadir=/var/lib/bitcoin generatetoaddress "$REFUND_BLOCKS" \
    "$MINING_ADDRESS" >"$PRIVATE_ROOT/evidence/$DIRECTION_NAME-refund-maturity.json"
fi
~~~

Invoke `recover` for both roles. Only the agreement-selected Bitcoin funder can
persist the exact canonical BIP-342 refund and consume the single send
authority; the other role observes. Mine the local profile confirmation block
with the same exact run-owned `generatetoaddress` command already used above,
then invoke `recover` for both roles again. For the one-confirmation local
profile, the confirmation command is:

~~~sh
docker exec "$CORE_CONTAINER" bitcoin-cli -conf=/run-config/bitcoin.conf \
  -datadir=/var/lib/bitcoin generatetoaddress 1 "$MINING_ADDRESS" \
  >"$PRIVATE_ROOT/evidence/$DIRECTION_NAME-refund-confirmation.json"
~~~

Projection requires Core to return the same canonical bytes, recomputed txid and wtxid, agreement confirmation
depth, and finalized containing height. An accepted send response alone leaves
the lifecycle revision unchanged.

For a LEZ refund, allow the isolated Bedrock/sequencer/indexer stack to advance
to a stable finalized clock at or after the countersigned `refund_at_ms`, then
invoke `recover` for both roles. Before the deadline the owner only reads state.
At eligibility it prepares the deterministic official `RefundNative` bytes,
persists them, performs exact observation, and may consume one send authority;
the other role uses bounded discovery by signed terms and never prepares or
submits. After the transaction is finalized, invoke both fresh processes again.
Only exact finalized inclusion plus historical and tip `Refunded` metadata, zero
custody, immutable depositor, and the configured window can project. The stable
wire exposes the finalized clock and transaction position; the v0.2 sidecar
checks the actual containing-block timestamp internally before returning the
result.

After both role stores report revision 3 `maker_leg_refunded`, repeat the same
procedure for the direction-correct second chain. Do not start the second leg
from a role store still at revision 2. Offline `status` reports
`next_action=recover_taker_leg` at this boundary. Completion is both stores at
revision 4, phase `refunded`; preserve both configs, stores, public-effect
journals, exact chain transaction/block identities, command outputs, and
cleanup evidence.

`Started`, `Unknown`, and `Accepted` journal states never grant a second send. A
transport ambiguity therefore trades liveness for at-most-once safety: rerun
`recover` only to observe the exact persisted effect. Never delete the journal,
regenerate a key, replace bytes, or manually rebroadcast. This does not create a
distributed atomic transaction across Core, LEZ, and SQLite. Atomicity is
preserved by pre-lock recovery authority, maker-funded-before-taker-funded
ordering, durable exact bytes before send, one-winner CAS, finalized-only
projection, and the rule that revision 4 cannot precede durable revision 3.

The deterministic actor gate uses temporary files and in-process chain ports; it
uses no RPC, Docker, faucet, peer, funds, or external network. The manual
actual-node flow uses only the isolated Core 31.1 Regtest node, LEZ v0.2
Bedrock/sequencer/indexer, and the two role sidecars described in this guide.
Bitcoin funds come from deterministic local Regtest outputs and LEZ funds from
local genesis/Vault allocations. No public chain RPC, faucet, public deployment,
or public funds are required. The pinned Bedrock process can make best-effort UDP NTP attempts to `pool.ntp.org:123`; certification does not require a
successful reply, and the combined runner records observed timeout attempts.
Runtime flakiness can still come from local process
scheduling, moving LEZ tips, indexer readiness, the 30-second read timeout, and
the fixed maximum 4096-block discovery window ageing past an old transaction.
Use a fresh isolated run, bounded sequential reads, and a window that covers the
expected refund height; an unavailable or aged observation is never absence or
new send authority. The separately missing containing-block timestamp field and
fixed window are recorded as nonblocking Logos production caveats.

## Strict witnessed operator requests

Every operator request is strict JSON with unknown fields denied. Every call
also requires the explicit endpoint, composed run ID, sidecar role, private
capability file, matching runtime file, and request file:

~~~sh
"$LEZ_OPERATOR" <command> --endpoint "$MAKER_BRIDGE_URL" --run-id "$M3_RUN_ID" --sidecar-role maker --capability-file "$PRIVATE_ROOT/maker/sidecar.capability" --runtime-file "$PRIVATE_ROOT/maker/runtime.json" --request-file <request.json>
~~~

Use the taker endpoint and files for taker calls. The eight commands are
describe-runtime, prepare-witnessed-escrow, observe-witnessed-escrow,
observe-finalized-witnessed-funding, submit-transaction,
prepare-witnessed-claim, complete-witnessed-claim, and
observe-finalized-witnessed-claim.

Each context has exactly schema_version, run_id, request_id, and sidecar_role.
Runtime has exactly sidecar_role, compatibility, chain_id, channel_id,
genesis_block_hash, escrow_program_id, and signer_account_id. Witnessed terms
have exactly swap_id, terms_hash, depositor, depositor_account_id, claimant,
claimant_account_id, aggregate_authority_account_id,
aggregate_x_only_public_key, amount as a decimal string, refund_at_ms as an
integer, and authenticated_transfer_program_id.

Construct requests with jq rather than hand-editing signed identifiers:

~~~sh
new_request_id() { openssl rand -hex 16; }
ROLE=maker
RUNTIME="$PRIVATE_ROOT/$ROLE/runtime.json"
REQ="$(new_request_id)"
jq -n --arg run "$M3_RUN_ID" --arg req "$REQ" --arg role "$ROLE" --slurpfile runtime "$RUNTIME" --slurpfile terms "$DIRECTION/terms.json" '{context:{schema_version:1,run_id:$run,request_id:$req,sidecar_role:$role},runtime:$runtime[0],terms:$terms[0]}' >"$DIRECTION/prepare-escrow-request.json"
~~~

prepare-witnessed-claim adds exactly funding_transaction_id. Its context role is
the LEZ claimant:

~~~sh
jq -n --arg run "$M3_RUN_ID" --arg req "$(new_request_id)" --arg role "$CLAIMANT_ROLE" --arg funding "$LEZ_FUND_TX" --slurpfile runtime "$CLAIMANT_RUNTIME" --slurpfile terms "$DIRECTION/terms.json" '{context:{schema_version:1,run_id:$run,request_id:$req,sidecar_role:$role},runtime:$runtime[0],terms:$terms[0],funding_transaction_id:$funding}' >"$DIRECTION/prepare-claim-request.json"
~~~

An exact observation adds target with mode exact plus both exact transaction
IDs. Counterparty discovery adds target with mode discover_by_terms and window
containing start_height and max_blocks:

~~~sh
jq -n --arg run "$M3_RUN_ID" --arg req "$(new_request_id)" --arg role "$ROLE" --arg init "$LEZ_INIT_TX" --arg fund "$LEZ_FUND_TX" --slurpfile runtime "$RUNTIME" --slurpfile terms "$DIRECTION/terms.json" '{context:{schema_version:1,run_id:$run,request_id:$req,sidecar_role:$role},runtime:$runtime[0],terms:$terms[0],target:{mode:"exact",initialization_transaction_id:$init,funding_transaction_id:$fund}}' >"$DIRECTION/observe-exact-request.json"
jq -n --arg run "$M3_RUN_ID" --arg req "$(new_request_id)" --arg role "$ROLE" --argjson start "$START_HEIGHT" --argjson blocks "$MAX_BLOCKS" --slurpfile runtime "$RUNTIME" --slurpfile terms "$DIRECTION/terms.json" '{context:{schema_version:1,run_id:$run,request_id:$req,sidecar_role:$role},runtime:$runtime[0],terms:$terms[0],target:{mode:"discover_by_terms",window:{start_height:$start,max_blocks:$blocks}}}' >"$DIRECTION/observe-discover-request.json"
~~~

The stable live observation above is a progress/recovery hint, not the
dual-lock finality checkpoint. After the indexer finalized tip covers the funding
window, either bound participant must run the distinct finalized funding
observation. The bridge returns evidence but does not retain a prerequisite
across its independent claim methods. The public actor enforces the ordering by
observing and projecting both locks before it permits claim projection. In this
lower-level operator recipe, the operator must persist the same checkpoint
before permitting any independent bridge claim method. Peerless mode accepts no
transaction ID from the counterparty:

~~~sh
FUNDING_OBSERVER_ROLE=maker
FUNDING_OBSERVER_RUNTIME="$PRIVATE_ROOT/$FUNDING_OBSERVER_ROLE/runtime.json"
FUNDING_OBSERVER_ENDPOINT="$MAKER_BRIDGE_URL"
: "${FUNDING_START_HEIGHT:?save the pre-funding finalized height here}"
rpc "$INDEXER_URL" '{"jsonrpc":"2.0","id":1,"method":"getLastFinalizedBlockId","params":[]}' >"$DIRECTION/funding-finalized-tip.json"
FUNDING_FINALIZED_TIP="$(jq -er '.result' "$DIRECTION/funding-finalized-tip.json")"
FUNDING_MAX_BLOCKS="$((FUNDING_FINALIZED_TIP - FUNDING_START_HEIGHT + 1))"
test "$FUNDING_MAX_BLOCKS" -ge 1
test "$FUNDING_MAX_BLOCKS" -le 4096
jq -n --arg run "$M3_RUN_ID" --arg req "$(new_request_id)" --arg role "$FUNDING_OBSERVER_ROLE" --argjson start "$FUNDING_START_HEIGHT" --argjson blocks "$FUNDING_MAX_BLOCKS" --slurpfile runtime "$FUNDING_OBSERVER_RUNTIME" --slurpfile terms "$DIRECTION/terms.json" '{context:{schema_version:1,run_id:$run,request_id:$req,sidecar_role:$role},runtime:$runtime[0],terms:$terms[0],target:{mode:"discover_by_terms"},window:{start_height:$start,max_blocks:$blocks}}' >"$DIRECTION/observe-finalized-funding-request.json"
"$LEZ_OPERATOR" observe-finalized-witnessed-funding --endpoint "$FUNDING_OBSERVER_ENDPOINT" --run-id "$M3_RUN_ID" --sidecar-role "$FUNDING_OBSERVER_ROLE" --capability-file "$PRIVATE_ROOT/$FUNDING_OBSERVER_ROLE/sidecar.capability" --runtime-file "$FUNDING_OBSERVER_RUNTIME" --request-file "$DIRECTION/observe-finalized-funding-request.json" >"$DIRECTION/finalized-funding.json"
test "$(jq -er '.funding.metadata.status' "$DIRECTION/finalized-funding.json")" = funded
test "$(jq -er '.funding.custody.balance' "$DIRECTION/finalized-funding.json")" = "$(jq -er '.amount' "$DIRECTION/terms.json")"
test "$(jq -er '.funding.transaction.transaction_id' "$DIRECTION/finalized-funding.json")" = "$LEZ_FUND_TX"
~~~

The last transaction-ID comparison uses the locally retained submission result
after peerless discovery; it is not discovery input. Persist this complete
result before claim completion. No adaptor reveal, claim completion, or claim
submission is eligible from `observe-witnessed-escrow` alone.

submit-transaction adds exactly transaction, containing transaction_id and
exact_bytes copied unchanged from a prepared result:

~~~sh
jq -n --arg run "$M3_RUN_ID" --arg req "$(new_request_id)" --arg role "$ROLE" --slurpfile runtime "$RUNTIME" --slurpfile transaction "$PREPARED_TRANSACTION_FILE" '{context:{schema_version:1,run_id:$run,request_id:$req,sidecar_role:$role},runtime:$runtime[0],transaction:$transaction[0]}' >"$DIRECTION/submit-request.json"
~~~

complete-witnessed-claim adds exactly claim and aggregate_signature. Copy claim
from the prepared claim result and payload from the role-runner final-signature
packet without logging either:

~~~sh
jq -n --arg run "$M3_RUN_ID" --arg req "$(new_request_id)" --arg role "$CLAIMANT_ROLE" --arg signature "$(jq -er '.payload' "$LEZ_FINAL_PACKET")" --slurpfile runtime "$CLAIMANT_RUNTIME" --slurpfile prepared "$DIRECTION/prepared-claim.json" '{context:{schema_version:1,run_id:$run,request_id:$req,sidecar_role:$role},runtime:$runtime[0],claim:$prepared[0].claim,aggregate_signature:$signature}' >"$DIRECTION/complete-claim-request.json"
~~~

After submission, either participant may independently discover the exact
finalized claim through its own role endpoint. Supply the same strict witnessed
terms and prepared unsigned claim plus the inclusive bounded window that
contains it; do not supply a peer transaction ID:

~~~sh
OBSERVER_ROLE=maker
OBSERVER_RUNTIME="$PRIVATE_ROOT/$OBSERVER_ROLE/runtime.json"
OBSERVER_ENDPOINT="$MAKER_BRIDGE_URL"
: "${CLAIM_START_HEIGHT:?save the pre-claim finalized height here}"
rpc "$INDEXER_URL" '{"jsonrpc":"2.0","id":1,"method":"getLastFinalizedBlockId","params":[]}' >"$DIRECTION/claim-finalized-tip.json"
CLAIM_FINALIZED_TIP="$(jq -er '.result' "$DIRECTION/claim-finalized-tip.json")"
CLAIM_MAX_BLOCKS="$((CLAIM_FINALIZED_TIP - CLAIM_START_HEIGHT + 1))"
test "$CLAIM_MAX_BLOCKS" -ge 1
test "$CLAIM_MAX_BLOCKS" -le 4096
jq -n --arg run "$M3_RUN_ID" --arg req "$(new_request_id)" --arg role "$OBSERVER_ROLE" --argjson start "$CLAIM_START_HEIGHT" --argjson blocks "$CLAIM_MAX_BLOCKS" --slurpfile runtime "$OBSERVER_RUNTIME" --slurpfile terms "$DIRECTION/terms.json" --slurpfile prepared "$DIRECTION/prepared-claim.json" '{context:{schema_version:1,run_id:$run,request_id:$req,sidecar_role:$role},runtime:$runtime[0],terms:$terms[0],claim:$prepared[0].claim,target:{mode:"discover_by_terms"},window:{start_height:$start,max_blocks:$blocks}}' >"$DIRECTION/observe-finalized-claim-request.json"
"$LEZ_OPERATOR" observe-finalized-witnessed-claim --endpoint "$OBSERVER_ENDPOINT" --run-id "$M3_RUN_ID" --sidecar-role "$OBSERVER_ROLE" --capability-file "$PRIVATE_ROOT/$OBSERVER_ROLE/sidecar.capability" --runtime-file "$OBSERVER_RUNTIME" --request-file "$DIRECTION/observe-finalized-claim-request.json" >"$DIRECTION/finalized-claim.json"
~~~

Use the observer's matching maker or taker endpoint, capability, and runtime;
the example endpoint is maker-specific. Success means the whole window was
covered by one stable finalized tip, the containing block lies inside it, all
blocks agree by ID/hash and parent-link through that tip, and terminal metadata
and zero custody were read at that exact
numeric block ID, and the client independently verified the aggregate BIP-340
signature. This call is read-only and never authorizes a replacement submit.
Require the returned transaction ID to equal the locally retained
`$LEZ_CLAIM_TX`; this is an evidence comparison after discovery, not an input
that can be supplied by the counterparty.

Moving-tip observations must use a new request ID on every attempt. Reusing an
ID returns the sidecar's durable at-most-once result and can replay Unknown
after the chain has advanced. Preparation may intentionally replay an exact
successful result under its original ID. Never retry submit after an ambiguous
response; reconcile the exact transaction ID through node reads.

## Fresh-process two-session signing ceremony

Each direction has two distinct session files: one BTC Taproot session over the
exact BIP-341 sighash and one untweaked LEZ session over the exact official
Message::hash(). They share one adaptor point but have fresh 32-byte session
IDs, separate nonces, and separate journals.

The BTC helper emits the public fixed PoC contract, funding transaction, spend
plan, and session:

~~~sh
"$BTC_FIXTURE" fund "$SOURCE_TXID" "$SOURCE_VOUT" "$SOURCE_VALUE_SAT" >"$DIRECTION/btc-fund.json"
"$BTC_FIXTURE" plan-spend "$(jq -er '.txid' "$DIRECTION/btc-fund.json")" 0 "$(jq -er '.contract_value_sat' "$DIRECTION/btc-fund.json")" "$BTC_RECIPIENT_ADDRESS" >"$DIRECTION/btc-plan.json"
BTC_SESSION_ID="$(openssl rand -hex 32)"
"$BTC_FIXTURE" btc-session "$(jq -er '.txid' "$DIRECTION/btc-fund.json")" 0 "$(jq -er '.contract_value_sat' "$DIRECTION/btc-fund.json")" "$BTC_RECIPIENT_ADDRESS" "$BTC_SESSION_ID" >"$DIRECTION/btc-session.json"
LEZ_SESSION_ID="$(openssl rand -hex 32)"
"$BTC_FIXTURE" lez-session "$LEZ_SESSION_ID" "$LEZ_CLAIM_MESSAGE_HASH" "$(jq -er '.adaptor_point' "$DIRECTION/btc-plan.json")" >"$DIRECTION/lez-session.json"
~~~

The helper and signer keys are deterministic local-only fixture authority. The
two owner-private signer files must match the public maker/taker keys emitted by
the helper. Do not substitute random keys. Do not print the fixture keys or the
adaptor scalar; provision them into mode-0600 files with a non-logging local
fixture provisioner. This is test authority, not production custody.

For each session, run every phase as a fresh process. Set SESSION to either
btc-session.json or lez-session.json, PREFIX to btc or lez, and use a distinct
role journal for that session:

~~~sh
"$ROLE_RUNNER" --journal "$DIRECTION/maker/$PREFIX-journal.sqlite" --session "$SESSION" maker reserve --secret-key-file "$DIRECTION/maker/signing.key" --output "$DIRECTION/public/$PREFIX-maker-commitment.json"
"$ROLE_RUNNER" --journal "$DIRECTION/taker/$PREFIX-journal.sqlite" --session "$SESSION" taker reserve --secret-key-file "$DIRECTION/taker/signing.key" --output "$DIRECTION/public/$PREFIX-taker-commitment.json"
"$ROLE_RUNNER" --journal "$DIRECTION/maker/$PREFIX-journal.sqlite" --session "$SESSION" maker accept-commitment --input "$DIRECTION/public/$PREFIX-taker-commitment.json"
"$ROLE_RUNNER" --journal "$DIRECTION/taker/$PREFIX-journal.sqlite" --session "$SESSION" taker accept-commitment --input "$DIRECTION/public/$PREFIX-maker-commitment.json"
"$ROLE_RUNNER" --journal "$DIRECTION/maker/$PREFIX-journal.sqlite" --session "$SESSION" maker reveal-nonce --output "$DIRECTION/public/$PREFIX-maker-nonce.json"
"$ROLE_RUNNER" --journal "$DIRECTION/taker/$PREFIX-journal.sqlite" --session "$SESSION" taker reveal-nonce --output "$DIRECTION/public/$PREFIX-taker-nonce.json"
"$ROLE_RUNNER" --journal "$DIRECTION/maker/$PREFIX-journal.sqlite" --session "$SESSION" maker accept-nonce-sign --input "$DIRECTION/public/$PREFIX-taker-nonce.json" --secret-key-file "$DIRECTION/maker/signing.key" --output "$DIRECTION/public/$PREFIX-maker-partial.json"
"$ROLE_RUNNER" --journal "$DIRECTION/taker/$PREFIX-journal.sqlite" --session "$SESSION" taker accept-nonce-sign --input "$DIRECTION/public/$PREFIX-maker-nonce.json" --secret-key-file "$DIRECTION/taker/signing.key" --output "$DIRECTION/public/$PREFIX-taker-partial.json"
"$ROLE_RUNNER" --journal "$DIRECTION/maker/$PREFIX-journal.sqlite" --session "$SESSION" maker accept-peer-partial --input "$DIRECTION/public/$PREFIX-taker-partial.json" --output "$DIRECTION/public/$PREFIX-maker-presignature.json"
"$ROLE_RUNNER" --journal "$DIRECTION/taker/$PREFIX-journal.sqlite" --session "$SESSION" taker accept-peer-partial --input "$DIRECTION/public/$PREFIX-maker-partial.json" --output "$DIRECTION/public/$PREFIX-taker-presignature.json"
cmp "$DIRECTION/public/$PREFIX-maker-presignature.json" "$DIRECTION/public/$PREFIX-taker-presignature.json"
~~~

Repeat all ten lines for both sessions before the first chain effect. Require
owner-only journal permissions, two distinct context bindings, commitment
acceptance before nonce reveal, both exact partials persisted, and identical
verified presignatures. Retain hashes of public packets, not their complete
contents, in publishable evidence. Retain the exact `BTC_SESSION_ID` and
`LEZ_SESSION_ID`; both historical schema-3 and current schema-4 configs name
them and the role-local journals. Schema 4 additionally gives only the Maker
exact direction-shaped lock material. Role-runner session
JSON and packet files remain ceremony transport,
not actor authority. The actor rederives keys, role order, exact messages,
adaptor point, and the Bitcoin Taproot tweak from the countersigned agreement.

## Restricted Bitcoin actor RPC and mining

Maker and taker use only their own curl config. A convenient wrapper is:

~~~sh
core_actor_rpc() {
  role="$1"
  method="$2"
  params="$3"
  case "$role" in
    maker) config="$MAKER_CORE_CONFIG" ;;
    taker) config="$TAKER_CORE_CONFIG" ;;
    *) return 2 ;;
  esac
  curl --fail --silent --show-error --config "$config" -H 'content-type: application/json' --data "$(jq -cn --arg method "$method" --argjson params "$params" '{jsonrpc:"2.0",id:1,method:$method,params:$params}')"
}
~~~

The actor allowlist includes chain/network reads, raw transaction and outpoint
observation, mempool reads, testmempoolaccept, and sendrawtransaction. Actors
cannot mine. Identify the one exact run-owned Core container and its mining
address now, but do not advance its tip until the direction recipe explicitly
submits the planned funding bytes:

Bitcoin Core 31.1 changed the accepted second parameter for
`gettxspendingprevout`. Every spender observation must send the options object
`{"mempool_only":false,"return_spending_tx":true}`; the former positional
booleans are invalid. The typed adapter additionally requires the returned
spending transaction's exact bytes, requires `blockhash` absent while the
transaction is only in the mempool, and requires the exact containing block
hash once confirmed. Commit `2233964` introduced this compatibility fix and
its contract tests.

~~~sh
mapfile -t CORE_CONTAINERS < <(docker container ls --quiet --filter "label=org.logos-co.atomic-swaps.run=$CORE_RUN_ID")
test "${#CORE_CONTAINERS[@]}" -eq 1
CORE_CONTAINER="${CORE_CONTAINERS[0]}"
MINING_ADDRESS="$(awk -F= '$1 == "BITCOIN_CORE_FUNDING_ADDRESS" {print $2}' "$CORE_FUNDING_FILE")"
~~~

For the Bitcoin funding lock, repeat `testmempoolaccept` under the
direction-correct role immediately before the first send. Submit only the exact
persisted funding file, mine one block, and bind the actual containing height to
the agreement's plan:

~~~sh
export BTC_FUNDING_ROLE=taker # use maker for taker_sells_lez
export BTC_FUNDING_RAW="$(tr -d '\r\n' <"$DIRECTION/funding-transaction.hex")"
core_actor_rpc "$BTC_FUNDING_ROLE" testmempoolaccept "[[\"$BTC_FUNDING_RAW\"]]" \
  >"$PRIVATE_ROOT/evidence/$DIRECTION_NAME-funding-policy-before-send.json"
jq -e '.result[0].allowed == true' \
  "$PRIVATE_ROOT/evidence/$DIRECTION_NAME-funding-policy-before-send.json" >/dev/null
core_actor_rpc "$BTC_FUNDING_ROLE" sendrawtransaction "[\"$BTC_FUNDING_RAW\"]" \
  >"$PRIVATE_ROOT/evidence/$DIRECTION_NAME-funding-send.json"
test "$(jq -er '.result' "$PRIVATE_ROOT/evidence/$DIRECTION_NAME-funding-send.json")" = \
  "$(jq -er '.transaction_id' "$FUNDING_PREPARE_SUMMARY")"
docker exec "$CORE_CONTAINER" bitcoin-cli -conf=/run-config/bitcoin.conf \
  -datadir=/var/lib/bitcoin generatetoaddress 1 "$MINING_ADDRESS" \
  >"$PRIVATE_ROOT/evidence/$DIRECTION_NAME-funding-mine.json"
core_actor_rpc "$BTC_FUNDING_ROLE" getrawtransaction \
  "[\"$(jq -er '.transaction_id' "$FUNDING_PREPARE_SUMMARY")\",true]" \
  >"$PRIVATE_ROOT/evidence/$DIRECTION_NAME-funding-confirmed.json"
export BTC_FUNDING_BLOCK_HASH="$(jq -er '.result.blockhash' "$PRIVATE_ROOT/evidence/$DIRECTION_NAME-funding-confirmed.json")"
core_actor_rpc "$BTC_FUNDING_ROLE" getblockheader "[\"$BTC_FUNDING_BLOCK_HASH\",true]" \
  >"$PRIVATE_ROOT/evidence/$DIRECTION_NAME-funding-block.json"
test "$(jq -er '.result.height' "$PRIVATE_ROOT/evidence/$DIRECTION_NAME-funding-block.json")" = \
  "$PLANNED_BTC_FUNDING_ANCHOR_HEIGHT"
unset BTC_FUNDING_RAW
~~~

For each later Bitcoin claim, apply the same role policy check and exact
send/observation discipline, then mine exactly one block and require the signed
confirmation policy. One confirmation is an intentional local Regtest happy-
path policy only, never a production policy.

## Direction 1: TakerSellsForeign with an actor-owned Maker LEZ lock

Here the Taker locks Bitcoin and ultimately receives LEZ. The Maker actor owns
the ordered LEZ initialize/fund second lock and ultimately receives Bitcoin.

```mermaid
sequenceDiagram
    actor Taker
    participant TakerActor as Taker actor
    participant MakerActor as Maker actor
    participant MakerJournal as Maker lock journal
    participant Core as Bitcoin Core
    participant Sidecar as Maker LEZ sidecar
    participant Sequencer as LEZ sequencer
    participant Indexer as LEZ indexer

    Note over TakerActor,MakerActor: Agreement and both signing sessions are durable
    Taker->>Core: Submit exact Bitcoin first lock
    Core-->>TakerActor: Confirmed exact unspent lock
    Core-->>MakerActor: Confirmed exact unspent lock
    TakerActor->>TakerActor: Project revision 1
    MakerActor->>MakerActor: Project revision 1
    MakerActor->>MakerJournal: Persist exact initialize and fund plan
    MakerActor->>Sidecar: Submit exact initialization once
    Sidecar->>Sequencer: Signed initialization
    Indexer-->>MakerActor: Finalized exact initialization
    MakerActor->>Sidecar: Submit exact funding once
    Sidecar->>Sequencer: Signed funding
    Indexer-->>MakerActor: Finalized exact funding and current Funded custody
    MakerActor->>MakerJournal: Atomically close intent with revision 2
    Indexer-->>TakerActor: Observe exact Maker lock
    TakerActor->>TakerActor: Project revision 2
    TakerActor->>Sequencer: Submit revealing LEZ claim once
    Indexer-->>MakerActor: Finalized reveal evidence
    MakerActor->>Core: Submit follow-up Bitcoin claim once
    Core-->>TakerActor: Confirm exact follow-up
    Note over TakerActor,MakerActor: Both stores reach revision 4 Completed
```

Use the exact builds, node bootstrap, agreement finalization, and signing
ceremony above. The repository runner performs this check before its first
effect; an operator auditing the retained private root can repeat it with:

~~~sh
export DIRECTION_NAME=taker_sells_foreign
export DIRECTION_ROOT="$PWD/.e2e/$RUN_ID/m3-actor-poc/private/directions/$DIRECTION_NAME"
export MAKER_BTC_ACTOR_CONFIG="$DIRECTION_ROOT/actors/maker/actor-config.json"
export TAKER_BTC_ACTOR_CONFIG="$DIRECTION_ROOT/actors/taker/actor-config.json"
jq -e '.schema_version == 4 and .role == "maker" and
  .maker_lock.chain == "lez" and
  (.maker_lock.preparation_request_file | type) == "string" and
  (.maker_lock.preparation_result_file | type) == "string"' \
  "$MAKER_BTC_ACTOR_CONFIG"
jq -e '.schema_version == 4 and .role == "taker" and
  (has("maker_lock") | not)' "$TAKER_BTC_ACTOR_CONFIG"
~~~

Then repeat the role flow:

1. Complete both Bitcoin and LEZ signing ceremonies before any public effect.
2. Under the restricted Taker Core identity, policy-check and broadcast only
   the exact persisted Bitcoin first-lock bytes. The local provisioner mines
   the planned block. Invoke one fresh process per role to project revision 1:

   ~~~sh
   "$BTC_ACTOR" --config "$MAKER_BTC_ACTOR_CONFIG" drive
   "$BTC_ACTOR" --config "$TAKER_BTC_ACTOR_CONFIG" drive
   ~~~

3. Invoke the Maker actor, never the sidecar submit command directly. Success
   at each pre-finality phase is `awaiting_observation` at revision 1:

   ~~~sh
   "$BTC_ACTOR" --config "$MAKER_BTC_ACTOR_CONFIG" drive
   ~~~

   The first successful phase adds exactly the initialization. A fresh repeat
   must leave the count at one. After exact initialization finality, another
   Maker process observes it and a later process adds exactly the funding. A
   fresh repeat must leave the count at two. After both exact transactions are
   finalized inside the actor's bounded window and the escrow is currently
   `Funded`, a final Maker `drive` atomically closes its journal intent and
   lifecycle revision 2. Typed observation unavailability may be retried with
   the same durable plan; every failed retry must retain empty stdout and keep
   the count inside `0 -> 1 -> 2`. It never grants a third
   submission.
4. Invoke the Taker actor only after the Maker projection. It independently
   observes the exact finalized Maker lock and reaches revision 2:

   ~~~sh
   "$BTC_ACTOR" --config "$TAKER_BTC_ACTOR_CONFIG" drive
   ~~~

5. Continue with fresh `drive` processes. The Taker owns the revealing LEZ
   claim at revision 2; the Maker observes it and owns the Bitcoin follow-up at
   revision 3. Accepted sends do not project. Exact finalized/confirmed
   observation must take both stores to revision 4 `completed`.

Why this is atomic: both adaptor presignatures, refund paths, and the exact
agreement are durable before the Taker's Bitcoin effect. The Maker risks LEZ
only after a fresh exact, confirmed, currently unspent Bitcoin check and before
the signed cutoff. The Taker cannot reveal until both locks are canonical; its
LEZ claim reveals the material for the Maker's Bitcoin claim. Without reveal,
the Maker LEZ refund precedes the Taker Bitcoin refund. This is economic and
protocol atomicity under the stated chain, cryptographic, and durability
assumptions, not one transaction across Core, LEZ, and SQLite.

## Direction 2: TakerSellsLez with an actor-owned Maker Bitcoin lock

Do not even prepare this direction until the preceding LEZ claim is finalized.
The shared aggregate-authority account nonce advances when a witnessed claim is
accepted. Preparing direction 2 earlier can reserve a stale exact claim message
and invalidate its presignature.

Here the Taker deposits LEZ and ultimately receives BTC. The Maker locks BTC
and ultimately receives LEZ.

```mermaid
sequenceDiagram
    actor Taker
    participant TakerActor as Taker actor
    participant MakerActor as Maker actor
    participant MakerJournal as Maker lock journal
    participant TakerSidecar as Taker LEZ sidecar
    participant Sequencer as LEZ sequencer
    participant Indexer as LEZ indexer
    participant Core as Bitcoin Core

    Note over TakerActor,MakerActor: Agreement and both signing sessions are durable
    Taker->>TakerSidecar: Submit exact LEZ initialize and fund pair
    TakerSidecar->>Sequencer: Signed Taker first-lock effects
    Indexer-->>TakerActor: Finalized exact Funded escrow
    Indexer-->>MakerActor: Finalized exact Funded escrow
    TakerActor->>TakerActor: Project revision 1
    MakerActor->>MakerActor: Project revision 1
    MakerActor->>MakerJournal: Persist exact Bitcoin funding bytes
    MakerActor->>Core: Submit exact Bitcoin funding once
    Core-->>MakerActor: Exact mempool identity then confirmation
    MakerActor->>MakerJournal: Atomically close intent with revision 2
    Core-->>TakerActor: Observe exact Maker lock
    TakerActor->>TakerActor: Project revision 2
    TakerActor->>Core: Submit revealing Bitcoin claim once
    Core-->>MakerActor: Confirmed reveal witness
    MakerActor->>Sequencer: Submit follow-up LEZ claim once
    Indexer-->>TakerActor: Finalized exact follow-up
    Note over TakerActor,MakerActor: Both stores reach revision 4 Completed
```

Use a fresh direction root because the aggregate-authority account nonce moves
after the preceding LEZ claim. Inspect the schema-4 configs:

~~~sh
export DIRECTION_NAME=taker_sells_lez
export DIRECTION_ROOT="$PWD/.e2e/$RUN_ID/m3-actor-poc/private/directions/$DIRECTION_NAME"
export MAKER_BTC_ACTOR_CONFIG="$DIRECTION_ROOT/actors/maker/actor-config.json"
export TAKER_BTC_ACTOR_CONFIG="$DIRECTION_ROOT/actors/taker/actor-config.json"
jq -e '.schema_version == 4 and .role == "maker" and
  .maker_lock.chain == "bitcoin" and
  (.maker_lock.exact_funding_transaction_file | type) == "string"' \
  "$MAKER_BTC_ACTOR_CONFIG"
jq -e '.schema_version == 4 and .role == "taker" and
  (has("maker_lock") | not)' "$TAKER_BTC_ACTOR_CONFIG"
~~~

Then repeat the role flow:

1. Prepare the exact Taker LEZ initialization/funding pair and Maker Bitcoin
   funding bytes, countersign the agreement, and complete both domain signing
   sessions before a public effect.
2. Through the restricted Taker sidecar, externally submit LEZ initialization
   and funding. Prove each transaction exactly once in sequential finalized
   blocks and require historical plus current `Funded` metadata and exact
   custody. Invoke both actors to project revision 1:

   ~~~sh
   "$BTC_ACTOR" --config "$MAKER_BTC_ACTOR_CONFIG" drive
   "$BTC_ACTOR" --config "$TAKER_BTC_ACTOR_CONFIG" drive
   ~~~

3. Policy-check the exact Maker Bitcoin bytes without sending them. Then invoke
   the Maker actor; do not call `sendrawtransaction` as the operator:

   ~~~sh
   "$BTC_ACTOR" --config "$MAKER_BTC_ACTOR_CONFIG" drive
   ~~~

   On the first successful send boundary it returns `awaiting_observation` at
   revision 1 and the mempool must contain exactly the planned transaction. A
   fresh identical Maker process must return without another send and preserve
   that one transaction. The provisioner may now mine the local block. After
   the signed depth, another Maker `drive` revalidates the current Taker LEZ
   lock and exact canonical Bitcoin bytes, then atomically closes the Maker
   intent and revision 2.
4. Invoke the Taker actor to observe the confirmed Maker lock and reach
   revision 2:

   ~~~sh
   "$BTC_ACTOR" --config "$TAKER_BTC_ACTOR_CONFIG" drive
   ~~~

5. Continue with fresh `drive` processes. The Taker owns the revealing Bitcoin
   claim and the Maker owns the follow-up witnessed LEZ claim. Require exact
   chain-witness equality before scalar extraction, and require confirmed or
   finalized evidence before either local projection. Both stores must finish
   at revision 4 `completed`; replay must leave both effect counts unchanged.

Why this is atomic: the Maker's exact Bitcoin transaction is durable before its
sole send, and the actor freshly rechecks that the Taker's exact LEZ escrow is
currently `Funded`, fully collateralized, finalized, and before the signed
cutoff. The Taker's Bitcoin claim reveals the material needed for the Maker's
LEZ claim. Without reveal, the Maker Bitcoin refund precedes the Taker LEZ
refund. A moving tip, ambiguous mempool, late inclusion, or changed custody
withholds progress rather than permitting conflicting histories. This remains
a conditional protocol guarantee, not a distributed transaction.

### Exact atomicity boundary

There is no atomic transaction spanning Bitcoin, LEZ, either sidecar, or either
SQLite database. The chain read returns before a local transaction begins, and
a chain submission may become public before the actor can record a later
observation. The implementation contains that gap by persisting the exact plan
and one-attempt authority before I/O, never rearming `Started`, `Unknown`, or
accepted work, and reconciling only the same public effect. Within the Maker's
own database, the final exact lock evidence, Maker intent close, and lifecycle
revision-two CAS are one SQLite transaction; that local atomic commit is not a
cross-chain commit.

The swap's economic atomicity instead comes from the countersigned transcript,
both domain presignatures and recovery material being durable before the first
effect, Taker-first funding, fresh Maker eligibility and cutoff enforcement,
the both-locks-before-reveal gate, adaptor disclosure linking the two claims,
and the direction-specific earlier-Maker/later-Taker refund order. It is
conditional on the pinned cryptography, honest key custody, canonical chain
views, configured finality, and transaction inclusion. RPC loss or permanent
actor abandonment can stop progress; it cannot justify a false terminal state
or a second blind send.

## Sequential LEZ finality recipe

For deployment, both Vault Claims, and every initialize, fund, and claim
transaction:

1. Save the start height before submission and the exact expected transaction
   ID.
2. Query getLastFinalizedBlockId. Wait locally until its result covers the
   bounded scan window; do not infer finality from sequencer admission or
   sidecar observation.
3. Query getTransaction with the exact transaction ID and require the returned
   Public.hash to match.
4. Query getBlockById one height at a time from start through finalized tip.
   Count exact Public.hash occurrences across body.transactions. Require count
   exactly one.
5. Require that containing block's bedrock_status is Finalized.
6. Query getBlockByHash for its header hash and require canonical JSON equality
   with the getBlockById result.
7. For state assertions, call getAccountAtBlock with the correct base58 account
   ID and that containing/finalized height. Keep calls sequential.

An empty result means not proved, not safe to continue. A moving finalized tip
is normal; a moving sequencer tip during a multi-read sidecar observation may
return typed observation unavailability. A direct operator audit uses a fresh
request ID for a deliberately new read. A schema-4 actor retry preserves its
deterministic operation identity and durable plan while starting a fresh
one-shot process. In both cases retry only the bounded read/reconciliation
category and prove the durable submission count or exact mempool identity did
not change. Never turn an uncertain submit into a second send.

`getAccountAtBlock` exposes end-of-block state, not a transaction-index
snapshot. Funding and claim in one block would therefore expose only terminal
Claimed state and cannot prove the intermediate Funded lock. Always complete
and persist finalized funding observation before submitting the claim; the
claim must finalize in a later block. This is deterministic local ordering, and
the missing proof/snapshot API remains `LOGOS-016` in the production register.

## Exact witness and terminal checks

For a finalized LEZ witnessed claim, require:

~~~sh
CHAIN_LEZ_SIGNATURE="$(jq -er '.result.Public.witness_set.signatures_and_public_keys[0][0]' "$INDEXER_CLAIM_TRANSACTION")"
PACKET_LEZ_SIGNATURE="$(jq -er '.payload' "$LEZ_FINAL_PACKET")"
test "${#CHAIN_LEZ_SIGNATURE}" -eq 128
test "$CHAIN_LEZ_SIGNATURE" = "$PACKET_LEZ_SIGNATURE"
jq -e '.result.Public.witness_set.signatures_and_public_keys | length == 1' "$INDEXER_CLAIM_TRANSACTION" >/dev/null
~~~

For a confirmed Bitcoin claim, require:

~~~sh
CHAIN_BTC_SIGNATURE="$(jq -er '.result.vin[0].txinwitness[0]' "$CORE_CLAIM_TRANSACTION")"
PACKET_BTC_SIGNATURE="$(jq -er '.payload' "$BTC_FINAL_PACKET")"
test "${#CHAIN_BTC_SIGNATURE}" -eq 128
test "$CHAIN_BTC_SIGNATURE" = "$PACKET_BTC_SIGNATURE"
jq -e '.result.vin[0].txinwitness | length == 1' "$CORE_CLAIM_TRANSACTION" >/dev/null
~~~

Perform equality before extraction. Comparing a locally prepared signature to
itself is not chain evidence.

Terminal certification for each direction additionally requires:

- Bitcoin gettxout for the contract outpoint is null.
- gettxspendingprevout names exactly the expected confirmed claim once.
- The claim's recipient output is present and belongs to the direction-correct
  maker or taker.
- LEZ finalized-window observation reports the exact aggregate-signature-verified
  claim and containing block, with metadata Claimed and custody balance 0 read
  at that same containing block ID.
- The LEZ claimant balance received the direction amount.
- Both recipient roles match the signed direction.
- Recovered scalar matches the committed point, but its value is not recorded.
- Both locks preceded the first scalar adaptation and the opposite claim used
  only persisted role state.

## Failed-onboarding diagnostic and atomic refusal

The earlier diagnostic m3poc-live-20260715a confirmed a BTC lock, then saw LEZ
initialize admitted but dropped with program error 6003 because the genesis
allocation remained in the Vault while the owner account was unfunded. LEZ
funding was not submitted and the adaptor scalar was never revealed.

It would have been possible to onboard the actor and prepare a new LEZ claim
transcript after that Bitcoin lock. The operator correctly refused: replacing
the exact message or presignature after the first effect would violate the
pre-lock signing invariant and could destroy atomicity. The failed swap was
excluded from certification. Both Vaults were onboarded and a completely fresh
swap, with both presignatures complete before either effect, produced the
successful run.

Apply the same rule to every failure: never repair a live locked swap by
changing terms, exact claim bytes, account nonce, session, key aggregation,
adaptor point, or presignature. Reconcile only the exact persisted transcript;
otherwise abandon the isolated chain funds and start a fresh run.

## External resources and flakiness

Audited schema-4 run `m3schema4-20260717d` and the retained recovery runs used
no public chain RPC, peer, faucet, public funds, or successful external-network
service as a certification dependency. The narrower chain claim remains true;
“runtime external resources were empty” would be too broad. Run D's pinned
Bedrock process made best-effort UDP NTP attempts to `pool.ntp.org:123` and
recorded 45 timeouts. Certification neither requires nor trusts a successful
NTP reply.

- Bitcoin Core is local Regtest with zero peers and deterministic local mining.
- LEZ is the run-owned local v0.2 Bedrock, sequencer, and indexer. Run D used
  the private-local 1.0-second slot; Run H used 3.0 seconds.
- Funds come from local genesis/Vault and Regtest outputs.
- No faucet, public chain RPC, public Testnet, public peer, or public
  funds are used.
- Pinned Bedrock may attempt `pool.ntp.org:123/udp`; this optional time-sync
  egress can fail or be blocked without invalidating the local proof. The proof
  waits for and verifies canonical finalized block timestamps instead.

Loopback describes reachability, not a node double. The combined runner starts
the pinned Core daemon and the exact LEZ Bedrock/sequencer/indexer services,
then checks Core version, Regtest genesis, policy, canonical transaction bytes,
containing blocks, confirmation depth, and spent-once outpoints. Run H also
checks the exact next-block CSV boundary, three-item refund witness, txid/wtxid
readback, and refund containing height. On LEZ it
checks the source/binary/runtime identities, deploys the checked guest, requires
sequential finalized ancestry, re-reads Vault ownership and balances, and binds
the exact initialize/fund/claim or refund transactions to historical account
state. The deterministic genesis and Regtest funds make the setup reproducible;
these node
and chain assertions, rather than the loopback address itself, establish the
local behavior under test.

The root `generate`, `prepare-funding`, and `finalize` commands and their eleven
tests perform no RPC,
start no service, and use no Docker. With a warm Cargo cache they depend only on
the OS random source and local owner-private files. The official
`lez-v02-account-id` invocation also performs no RPC, but its first build uses
the separate locked Rust 1.96 LEZ sidecar graph and can be delayed by the same
cold Cargo/git/native-library availability as that graph. Actual Core funding,
LEZ deployment/finality, prepared-claim facts, Core UTXO state, and read-only
policy are collected around those commands. Local node readiness, moving-tip
observations, or manual transcription can delay or fail the ceremony. Such
failure is not permission to invent a fact, reuse a fixture root, relax a
policy, or overwrite output.

Cold setup can depend on crates.io/git caches, official Bitcoin release and
Guix-signature URLs, the digest-pinned Risc0 builder image, and the
checksum-pinned Logos circuits release. Treat download, DNS, registry, and
registry-rate failures as setup flakiness, not chain evidence.

The combined actor runner invokes all root and sidecar Cargo builds with
`--offline`, so a missing locked source fails before either service starts. It
still inherits the exact LEZ checkout/service/r0vm paths and the Core release
provenance workflow; provision those inputs first. Docker daemon availability,
image construction, CPU, memory, and disk pressure are local execution
dependencies. None is a public chain dependency, and none may be converted into
a looser finality, policy, identity, or cleanup assertion.

The Core release verifier creates a run-owned GnuPG home and, on normal exit or
signal, calls `gpgconf --homedir <that-home> --kill gpg-agent`. It fails closed
if that exact cleanup fails and never kills agents outside the run. Run-n's
post-cleanup process audit found no agent for its GnuPG home.

Remaining certification-sensitive runtime flakiness is local: deliberate
recovery-deadline waits,
process scheduling, an advancing LEZ tip
across multi-read observations, indexer/sequencer readiness, heavy parallel
indexer reads, port selection, and manual ordering. Use unique ports, sequential
indexer calls, bounded waits, deterministic actor operation identities, and
retained exact evidence. The actor's LEZ scan is bounded to 30 seconds; expiry
is uncertain read-only observation, not absence and not new send authority.
Never mask a timeout by resubmitting an effect.

Run D exercised this rather than merely documenting it. The Maker LEZ path
reconciled each initialize/fund phase while keeping its durable count within
`0 -> 1 -> 2`. The Maker Bitcoin path saw nine typed moving-tip attempts before
success; every attempt required exactly two Taker LEZ effects and a Bitcoin
mempool containing either nothing or only the exact planned funding txid. The
successful attempt produced that one txid, and a fresh Maker restart preserved
it without resubmission. Only this typed, bounded reconciliation is retryable;
authentication, transport ambiguity, malformed evidence, foreign mempool
content, count drift, or nonempty failed stdout aborts.

The combined runner now makes its maker-lock timing policy explicit. Happy and
survivor journeys sign the cutoff 1,800 seconds after agreement preparation;
the two-lock refund journey uses 300 seconds, with its earlier refund boundary
at 900 seconds so the required 600-second reaction margin is unchanged; the
absent-maker journey uses the preparation time itself. These are local PoC
parameters, not inferred public-chain calibration. Admission compares Bitcoin's
canonical containing-block median time or LEZ's finalized containing-block
timestamp to the signed cutoff. A host clock, mempool sighting, sequencer tip,
or runner-side preflight cannot substitute for that chain evidence.

## Manual actor first-lock absent-maker recovery

Status: both actual-node directions are GREEN at clean pushed commit `cefcd07`
in run `m3firstlock-20260716h`. Use the same verified prerequisites as the
combined happy path, a new run ID, and the dedicated journey:

~~~sh
export RUN_ID=m3firstlock-manual-001
export M3_ACTOR_POC_JOURNEY=first_lock_refund
./scripts/run-m3-actor-local-poc.sh

export M3_EVIDENCE="$PWD/.e2e/$RUN_ID/m3-actor-poc/evidence"
jq -e '.kind == "m3_actor_two_direction_first_lock_refund_local_poc" and
  .journey == "first_lock_refund" and .result == "passed" and
  all(.directions[];
    .terminal_revision == 2 and .terminal_phase == "refunded" and
    .maker_second_lock_effect_count == 0) and
  .expected_unique_effects_by_direction == {
    taker_sells_foreign:{bitcoin:2,lez:0},
    taker_sells_lez:{bitcoin:0,lez:3}} and
  .first_lock_refund_admission.signed_cutoff == true and
  .first_lock_refund_admission.two_fresh_cross_chain_reads == true and
  .first_lock_refund_admission.owner_restart_without_resubmission == true and
  .first_lock_refund_admission.fresh_maker_observer == true and
  .replay_resubmission_count == 0 and
  .external_resources.certification_success_depends_on_external_network == false' \
  "$M3_EVIDENCE/m3-actor-local-poc.json"
jq -e '.journey == "first_lock_refund" and .result == "passed" and
  .all_exact_run_resources_absent == true and
  .foreign_resources_targeted == false and .broad_cleanup_used == false' \
  "$M3_EVIDENCE/cleanup-attestation.json"
~~~

The runner creates one countersigned agreement per direction and activates
separate maker/taker stores and signing journals before the first effect. The
taker submits only the direction-correct first lock. The maker is not invoked
again until refund finality and therefore cannot submit the second lock. The
taker must remain at revision 1 while it waits for the signed maker-lock cutoff
and its chain-specific recovery boundary. Two fresh, complete observations
must show the other lock absent and the taker lock canonical and unspent. The
actor's persisted admission is authoritative; the runner reads corroborate it
but cannot grant send authority.

`TakerSellsForeign` then spends the exact Bitcoin first lock through its
countersigned BIP-342 CSV branch. `TakerSellsLez` submits the official
`RefundNative` only after a finalized LEZ block timestamp crosses the signed
deadline. Exact confirmed/finalized evidence projects the taker to revision 2;
a new maker process must then reconstruct the first lock and refund from chain
state before it also reports revision 2 `refunded`. Terminal restart and both
role replays must leave the effect manifests byte-identical.

All chain RPCs are ephemeral literal-loopback Core, LEZ sequencer, and LEZ
indexer endpoints. Funds are deterministic local Regtest coinbase or genesis
Vault allocations. No public RPC, faucet, peer, deployment, or public funds are
used. Cold provisioning can still depend on pinned artifact/image sources.
Pinned Bedrock makes best-effort `pool.ntp.org:123/udp` attempts, but the final
packet must say certification has no external-network success dependency.

An advancing finalized LEZ tip may return typed `moving_tip` during the exact
witnessed-funding read. The runner retries only that read-only category, at
most 120 attempts over 30 seconds, with a fresh request ID and retained empty
failed stdout. Timeout, transport ambiguity, malformed evidence, authentication
failure, and every other remote category abort. Retry evidence must show the
durable LEZ submission count unchanged. Run H's first sample succeeded once;
the second converged after three moving-tip retries. This local finality race
can extend elapsed time but can never authorize a duplicate effect.

The retained secret-safe result is
`docs/evidence/m3-local-two-direction-first-lock-refund-poc-20260716.json`.
Keep the full `.e2e` root private because it contains credentials, keys, raw
transactions, and role state. This journey proves the refund side when the
maker is absent. Run D separately proves timely schema-4 Maker-lock admission;
neither run proves overlapping admission/refund races, concurrent swaps, reorg,
or production readiness.

## Manual actor post-reveal survivor continuation

Use the same pinned prerequisites as the combined happy flow, a new run ID,
and the survivor journey. It starts fresh isolated services; do not point it at
another operator's Core or LEZ nodes.

~~~sh
export RUN_ID=m3survivor-manual-001
export M3_ACTOR_POC_JOURNEY=survivor_claim
./scripts/run-m3-actor-local-poc.sh

export M3_EVIDENCE="$PWD/.e2e/$RUN_ID/m3-actor-poc/evidence"
jq -e '.kind == "m3_actor_two_direction_survivor_claim_local_poc" and
  .journey == "survivor_claim" and .result == "passed" and
  .survivor.revealer == "taker" and .survivor.follower_role == "maker" and
  .survivor.protected_absence.revealer_actor_invocation_count == 0 and
  .survivor.intermediate == {
    phase:"claim_evidence_available",lifecycle_disposition:"recovering",
    terminal:false,remaining_leg_canonical_and_claimable:true,
    followup_effect_present:false} and
  .survivor.delayed_revealer_catchup.observation_only == true and
  .survivor.delayed_revealer_catchup.bitcoin_successful_resubmission_count == 0 and
  .survivor.delayed_revealer_catchup.lez_successful_resubmission_count == 0 and
  .survivor.delayed_revealer_catchup.successful_resubmission_count == 0 and
  all(.survivor.direction_evidence[];
    .completion_boundary.completed_before_signed_refund_boundary == true and
    (.completion_evidence_sha256 | test("^[0-9a-f]{64}$")) and
    (.recovering_evidence_sha256 | test("^[0-9a-f]{64}$"))) and
  all(.directions[];
    .terminal_revision == 4 and .terminal_phase == "completed") and
  .expected_unique_effects_by_direction == {
    taker_sells_foreign:{bitcoin:2,lez:3},
    taker_sells_lez:{bitcoin:2,lez:3}} and
  .replay_resubmission_count == 0 and
  .external_resources.public_rpc == false and
  .external_resources.faucet == false and
  .external_resources.public_funds == false' \
  "$M3_EVIDENCE/m3-actor-local-poc.json"

jq -e '.result == "passed" and .all_exact_run_resources_absent == true and
  .foreign_resources_targeted == false and .broad_cleanup_used == false' \
  "$M3_EVIDENCE/cleanup-attestation.json"
~~~

The two directions deliberately mirror real role behavior. After both locks,
the taker submits the revealing claim and the journey blocks every further
harnessed taker actor process until maker terminality. A fresh maker observes the public witness, reconstructs
and point-checks the adaptor scalar against its persisted presignature, commits
revision 3 `ClaimEvidenceAvailable`, and exits. A separate fresh maker resumes,
submits the opposite claim, and reaches revision 4 `Completed`. Only then does
the taker return for observation-only catch-up. Revision 3 is evidence-layer
`recovering`, not a new protocol phase and never terminal.

The direction packet proves that the still-funded leg is claimable before its
refund boundary: exact canonical/unspent Core outpoint for
`TakerSellsForeign`, exact finalized `Funded` LEZ custody for
`TakerSellsLez`. The run uses ephemeral literal-loopback Core, sequencer, and
indexer RPCs plus deterministic Regtest/genesis funds. There is no public RPC,
faucet, peer, Delivery, Chat, public deployment, or public funds. Bedrock may
attempt `pool.ntp.org:123/udp`, but success is not required. `moving_tip` and
temporarily unavailable finalized reads are bounded, read-only, and can extend
runtime without granting another submission.

Clean pushed-commit run `m3survivor-20260716c` passed both directions and exact
cleanup. Its secret-safe retained packet is
`docs/evidence/m3-local-two-direction-survivor-claim-poc-20260716.json`. Keep
the full `.e2e` run root private;
it contains credentials, actor stores, capabilities, and raw transactions.

## Public configuration switch and production nonclaims

The LEZ sidecar has a dormant official_public node profile. Switching LEZ
routes changes the node profile, HTTPS URLs, runtime chain/channel/genesis,
deployed program identity, actor accounts, funding, and finality policy. The
checked protocol messages and sidecar binary remain the same. A public escrow
deployment and public account provisioning are still on-chain actions, not a
URL-only switch.

Bitcoin is not yet a configuration-only public switch. The current Core runner
is Regtest-only and btc-core-p2tr-fixture uses deterministic local keys,
Regtest addresses, and fixed fixture transaction construction. A production
Core adapter, public/Testnet funding path, non-fixture signing authority,
production confirmation/reorg policy, and key custody must replace that test
surface.

Run `m3refund-20260716h` proves ordered actual-node refund after both locks, and
`m3firstlock-20260716h` proves refund-side first-lock absent-maker abandonment.
Run `m3survivor-20260716c` separately certifies clean post-reveal survivor
recovery. Run `m3schema4-20260717d` proves the complementary timely Maker-lock
branch through schema-4 actors in both directions. It does not prove two
genuinely overlapping swaps, adversarial admission/refund races, reorg handling,
process-kill or crash recovery across the full lifecycle, chaos behavior,
denial-of-service resistance, secure transport between actors, HSM custody, key
rotation, backup, formal cryptographic audit, public deployment, or production
readiness.
musig2 remains a PoC dependency subject to the recorded audit/zeroization
review, and Logos-owned upstream disclosures remain tracked separately.

## Exact run-scoped cleanup

Stop only the two recorded bridge PIDs and wait for them. Preserve the
secret-safe evidence you intend to retain, then remove the private direction
roots. Never use docker system prune, docker volume prune, wildcard container
removal, or any cleanup keyed to another run.

For Core, read exact identities from its retained runtime evidence:

~~~sh
CORE_EVIDENCE="$CORE_RUN_DIR/evidence/runtime.json"
CORE_CONTAINER="$(jq -er '.isolation.container_id' "$CORE_EVIDENCE")"
CORE_VOLUME="$(jq -er '.isolation.data_volume' "$CORE_EVIDENCE")"
CORE_NETWORK="$(jq -er '.isolation.network' "$CORE_EVIDENCE")"
CORE_IMAGE="$(jq -er '.core.image' "$CORE_EVIDENCE")"
docker container rm --force "$CORE_CONTAINER"
docker volume rm "$CORE_VOLUME"
docker network rm "$CORE_NETWORK"
docker image rm "$CORE_IMAGE"
~~~

For LEZ, enumerate only the exact service-qualified run label, require exactly
three containers, then remove the named network and image:

~~~sh
mapfile -t LEZ_CONTAINERS < <(docker container ls --all --quiet --filter "label=org.logos-co.atomic-swaps.run=$LEZ_RUN_ID")
test "${#LEZ_CONTAINERS[@]}" -eq 3
docker container rm --force "${LEZ_CONTAINERS[@]}"
docker network rm "lez-atomic-swaps-lez-v02-${LEZ_RUN_ID}-private"
docker image rm "lez-atomic-swaps-lez-v02:${LEZ_RUN_ID}"
~~~

Finally verify no container retains either exact run label. Remove only
PRIVATE_ROOT and the isolated build targets you created after any required
private audit is complete. Leave every unrelated container, image, network,
volume, cache, and worktree file untouched.
