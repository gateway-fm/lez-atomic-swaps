# LEZ Atomic Swap Suite

Trustless swaps between Logos Execution Zone (LEZ) and Bitcoin, Monero, and
Zcash's transparent pool.

The accepted delivery scope is Gateway's replacement proposal
[logos-co/rfp#112](https://github.com/logos-co/rfp/issues/112), interpreted
together with the live
[RFP-003](https://github.com/logos-co/rfp/blob/master/RFPs/RFP-003-atomic-swaps.md).
The earlier issue #61 is superseded and Ethereum is not an in-scope pair.

## Current status

M2 is certified at its private local-functional PoC boundary under
`m2-complete`. M3's progressive private local-devnet PoC is now **2 of 2
directions complete through the public actor and actual nodes**. Its authority, Bitcoin Core 31.1
Regtest topology, dependency candidates, actor flows, and acceptance gate are
audited in
[ADR 0029](docs/architecture/0029-m3-bitcoin-local-poc-entry.md). The
nonexistent DLC Schnorr-vector reference is separately tracked as
[Gateway erratum GW-M3-001](docs/proposal-acceptance-errata.md), with no accepted
replacement yet.

Run `m3actor-20260716n` passed at pushed `origin/main` commit `6ded2f9` on
2026-07-16. It drove fresh one-shot maker and taker actor processes through
`TakerSellsForeign` and `TakerSellsLez` against isolated Bitcoin Core 31.1
Regtest and the exact local LEZ v0.2 stack. Both roles in both directions ended
at revision 4 `completed` with next action `complete`; terminal replay caused
zero resubmissions. Each direction retained two unique confirmed Bitcoin
effects and three exact durable LEZ submissions, including both actor-owned
claims. The terminal packet reports no public RPC, faucet, public funds, or
private-material disclosure, and cleanup proves all exact run containers,
networks, volumes, images, and secure reservation state absent without targeting
foreign resources. This completes the progressive M3 local PoC, not the later
QA, chaos, infosec, public-Testnet, or production-readiness phases.
The Core release verifier also stops the exact run-owned `gpg-agent` on every
exit; the run-n post-cleanup audit found no matching agent process.
Commit `650d94e` also sets every actor-to-LEZ finalized scan to a finite
30-second request timeout. The bound is long enough for the exact local
finalized-window scan that completed run-n, while a timeout remains retryable
read-only unavailability and never grants another submission.

Run `m3refund-20260716h` then passed the two-lock timeout/refund journey
from base HEAD `ef5f306` with a dirty pre-commit source tree and the packet's
independently validated runner hashes. It is hash-bound functional evidence,
not clean exact-commit certification.
Fresh one-shot maker and taker processes executed both directions against
Bitcoin Core 31.1 Regtest and LEZ v0.2 private-local
Bedrock/sequencer/indexer services using a 3.0-second slot. In both directions,
both role stores reached revision 4 `refunded` with next action `complete`.
Each direction retained exactly two confirmed Bitcoin effects and three exact
durable LEZ submissions: one Bitcoin refund and one LEZ `RefundNative` were
actor-owned, and no cooperative claim effect was present. Terminal `recover`
replay changed no effect count. The packet records no public RPC, faucet,
public funds, certification-time network dependency, or private-material
disclosure; exact cleanup removed only captured run resources and targeted no
foreign activity. The retained evidence span from its first emitted file to
cleanup was 54 minutes 5 seconds. Most elapsed time is the deliberate signed
deadline wait across two sequential directions, so this is not a throughput
benchmark. It proves ordered recovery after both locks, not a first-lock-only
absent-maker or survivor-only journey.

Run `m3firstlock-20260716h` passed at clean, already-pushed commit `cefcd07`.
In both economic directions the taker funded the first leg, the maker stayed
offline after activation and never submitted the second lock, and fresh actor
processes recovered only the taker-funded leg after the countersigned cutoff
and chain-specific refund boundary. `TakerSellsForeign` retained exactly the
Bitcoin lock and BIP-342 refund; `TakerSellsLez` retained exactly LEZ
initialize, fund, and `RefundNative`. Both roles reconstructed terminal
revision 2 `refunded`, replay added zero effects, and exact cleanup targeted no
foreign resource. The secret-safe retained packet is
[M3 first-lock absent-maker evidence](docs/evidence/m3-local-two-direction-first-lock-refund-poc-20260716.json).
This closes the refund-side absent-maker journey, not the still-open live
maker-lock admission race or post-reveal survivor journey.

Run `m3poc-live2-20260715a` used one isolated Bitcoin Core 31.1 Regtest node,
one exact local LEZ v0.2 Bedrock/sequencer/indexer stack, and separate
capability-authenticated maker/taker sidecars, signing processes, keys,
SQLite journals, and state roots. The digest-pinned aggregate-witness guest
ELF `a199c5be...e293` / ProgramId `39b6a4db...4dec` was deployed and finalized
before either direction. Both actor Vault allocations were independently
claimed and re-read at finalized tips before swap preparation.
Fresh local identity schema version 2 publishes each actor's owner account and
the official owner-derived Vault account in base58 and hex, plus the x-only
public key; signer material remains owner-private. The local stack requires
owner and derived-Vault overrides as a pair before it creates genesis.

`TakerSellsForeign` confirmed the taker Bitcoin lock `ca0ae641...a4c75`, then
finalized maker LEZ initialize/fund transactions in blocks 540/544. Only after
both locks were proven did the taker finalize LEZ claim
`ef77099e...2cde3` in block 570. The maker matched the exact finalized LEZ
witness, extracted and point-checked the committed adaptor scalar from its own
persisted session, and confirmed Bitcoin claim `0ee99753...6a5aa` with exactly
one 64-byte key-path witness. `TakerSellsLez` finalized taker LEZ
initialize/fund transactions in blocks 617/620, then confirmed maker Bitcoin
lock `c5dd0f85...752a3`. The taker confirmed Bitcoin claim
`66255398...054f4`; the maker matched that exact Core witness, recovered the
scalar from its own persisted session, and finalized LEZ claim
`834c67e9...d3033` in block 644. Both terminal LEZ custody accounts are zero,
and both Bitcoin contract outpoints are spent exactly once to the
direction-correct recipient.

The PoC uses a declared one-confirmation Regtest policy and independently
requires LEZ indexer `Finalized` membership, exact-once bounded block
membership, and equal by-ID/by-hash block bodies. No scalar is used before both
locks, and after reveal the opposite claim uses only persisted local state and
chain observations. The secret-safe retained packet is
[M3 local two-direction evidence](docs/evidence/m3-local-two-direction-poc-20260715.json).
No public RPC, faucet, peer, credential, public funds, or public deployment is
used.

The public `btc-reference-actor` now closes the first cohesive lifecycle slice.
Each fresh role-fixed process accepts `activate`, `drive`, `recover`, or `status`
with a strict owner-private schema-3 config. The agreement-selected Bitcoin
funder alone supplies a lowercase-hex mode-`0600` refund-key file converted
without stdout from its raw provisioner key; the other role must have neither
the converted file nor that authority. Activation rederives and compares the countersigned x-only key. Before
`activate` may insert agreement
acceptance or create revision zero, it binds the complete prepared LEZ claim
result plus two distinct completed role-local signer journals to contexts
rederived from the countersigned agreement. The taker must also provide an
owner-only adaptor scalar that is point-checked without creating a signature;
maker configs are forbidden from naming that secret. Missing, cross-wired,
incomplete, unsafe, or changed authority fails without creating actor state.
Absent or
empty/no-acceptance state remains `not_activated`, while corrupt or conflicting
state fails closed; `status` may migrate an existing schema but creates no
acceptance and performs no RPC. At revision zero or one, `drive`
selects respectively the taker-funded or maker-funded chain from the validated agreement, observes exact
Bitcoin funding through the typed Core adapter or finalized witnessed LEZ
funding through the role sidecar, binds LEZ accounts to the signed agreement,
returns from the observation, and only then performs the SQLite predecessor
CAS. Finalized LEZ evidence retains its complete finalized tip. Before funding, LEZ
finalized-observer errors are retryable unavailability, not proof of absence.
Exact retries retain their deterministic request ID; a deliberate bounded-
window change receives a distinct ID and remains evidence-bound. A
valid concurrent revision-one or revision-two winner is reconstructed without overwrite; other
projection conflicts fail closed. This is a read-only-observation-to-local-
projection boundary, not a cross-system atomic commit. At revision two or
three, claim projection reruns the complete activation-material gate before
using signer state. The injected exact-canonical observation seam reaches
revision three and terminal revision four for both roles and directions. The
taker must reproduce the revealing signature from its private scalar; the maker
must extract and point-check the same scalar from its persisted presignature.
Only one-way `ClaimEvidence` enters lifecycle state, signed Bitcoin confirmation
policy and finalized LEZ policy units are enforced, and terminal status is
reconstructed offline. The typed live Bitcoin finalized-claim observer is
wired.

The public-effect journal and exact one-attempt Bitcoin send primitive are now
composed for actor-owned Bitcoin claims. The taker alone owns a revealing
Bitcoin claim at revision two; the maker alone owns a follow-up Bitcoin claim
at revision three and re-extracts its scalar from the durable revealing witness
plus its persisted LEZ presignature. Complete public bytes are durable before
the only fresh send authority. `Started` and `Unknown` remain observe-only
across restart, exact-byte drift is uncertain, and even an accepted send does
not project local state. Projection waits for the same exact bytes finalized by
Core at the signed confirmation policy. Pushed `66d352f` composes the matching
actor-owned LEZ completion, bounded finalized presence/absence observation,
one-attempt send, and terminal projection path. Both claim paths are GREEN in
source, deterministic actor tests, and the retained run-n actual-node evidence.
See
[ADR 0031](docs/architecture/0031-one-shot-btc-actor-observe-before-project.md),
[ADR 0034](docs/architecture/0034-gate-actor-activation-on-signing-material.md),
and [ADR 0035](docs/architecture/0035-project-claims-only-from-canonical-public-evidence.md).

For timeout recovery, call the same binary with `recover`. The maker-funded leg
must reach durable revision 3 before the taker-funded leg can reach revision 4
`Refunded`. Deterministic actor tests cover both direction mappings, owner and
nonowner roles, pre-deadline no-send, persist-before-send, one-winner/one-attempt
authority, observe-only ambiguity, exact finalized projection, and terminal
restart. The [manual timeout/refund procedure](docs/m3-local-poc-operator-guide.md#manual-actor-timeoutrefund-recovery)
uses only isolated Core 31.1 Regtest and local LEZ v0.2
Bedrock/sequencer/indexer plus role sidecars; no public RPC, faucet, public
deployment, or public funds are needed. Run `m3refund-20260716h` now retains
fresh actual-node execution for both ordered two-lock refund directions.
First-lock-only absent-maker recovery is now actual-node GREEN in
`m3firstlock-20260716h`. Survivor-specific continuation, the maker-lock side of
the deadline-cutoff race, concurrency, process-kill, and reorg journeys remain
later gates.

Pushed `a8688a3` replaces the unsafe post-confirmation recipe with an exact
pre-effect funding ceremony. For each direction the operator runs `generate`,
reads the candidate input with Core `gettxout`, builds one exact signed funding
transaction with `prepare-funding`, asks Core for read-only
`testmempoolaccept` evidence, finalizes the
countersigned agreement with a planned next-block anchor, completes both the
Bitcoin and LEZ signer journals, and only then permits the first chain effect:

```sh
cargo run --locked -p btc-local-poc-provision -- generate \
  --planning-file "$AGREEMENT_PLANNING" \
  --output-root "$DIRECTION"

cargo run --locked -p btc-local-poc-provision -- prepare-funding \
  --spec-file "$FUNDING_PREPARE_SPEC" \
  --output-root "$DIRECTION"

# Read-only: submit the exact persisted hex to Core testmempoolaccept here.

cargo run --locked -p btc-local-poc-provision -- finalize \
  --spec-file "$AGREEMENT_FINALIZE_SPEC" \
  --output-root "$DIRECTION"
```

The strict prepare document contains `schema_version`, the stage-one public
SHA-256, `direction`, one `service_input` object (`transaction_id`,
`output_index`, `value_sat`, `script_pubkey`, and the path to a raw mode-`0600`
32-byte `signing_secret_key_file`), `contract_value_sat`, and `fee_sat`. The
secret is extracted from the owner-private Core funding credential directly to
that file without stdout. `prepare-funding` emits no raw transaction on stdout;
it creates mode-`0600` `funding-transaction.hex` and a secret-free summary with
the exact txid/wtxid, input, contract, change, fee, BIP-341 sighash, Merkle root,
and `node_state_asserted: false` facts.

The strict finalize document binds `genesis_block_hash`,
`required_confirmations`, `funding_signed_transaction`, its SHA-256, the input
value and script, funding txid/vout/value, `claim_value_sat`, LEZ deployment and
prepared-claim facts, and the recovery plan. The recovery plan contains
`refund_csv_blocks`, `planned_bitcoin_funding_anchor_height`,
`bitcoin_refund_height`, both typed cross-chain deadlines, and their safety
margin. It rejects the former observed-confirmation/observed-anchor/broadcast
fields. The signed anchor must be the isolated Core tip plus one; when Bitcoin
funding is due, the harness broadcasts the persisted exact bytes, mines exactly
one block, and requires the containing height to equal that plan.

Stage one creates fresh OS-random maker/taker signing, refund, and claim keys
plus the adaptor scalar under mode-`0700` directories and single-link,
create-new mode-`0600` files. The provisioner verifies the rawtr service-key
relation and exact BIP-341 authorization, canonical bytes/hash/txid, fee,
contract output, and one-item `SIGHASH_DEFAULT` witness. It performs no RPC and
therefore does not prove that the input is unspent/mature, current Core policy,
broadcast, confirmation, or finality. `gettxout`, `testmempoolaccept`, and later
exact block reads supply those separate node-state facts; policy acceptance is
not a reservation.

The multi-file generate, funding, and agreement output groups are create-new
and fail safe but are not distributed or filesystem-atomic transactions. Before
any effect, retire an interrupted direction root and restart from fresh stage
one. After any possible effect, preserve the exact root and reconcile or refund;
never regenerate authority for already-locked funds. Cross-chain atomicity is
maximized by requiring one agreement and both chains' presignatures before the
first effect, but no distributed atomic commit exists across files, journals,
actor stores, Core, and LEZ.

Eleven all-target provisioner tests cover both directions, genuine rawtr
signing, drift and malformed inputs, no-clobber recovery, and stdout secret
scanning without RPC, Docker, faucet, or public endpoint use. The combined
run-owned harness must retain the policy response, journal-completion evidence,
and actual planned-anchor equality while driving fresh public actor processes
through both directions. See the exact
[operator recipe](docs/m3-local-poc-operator-guide.md#generate-the-agreement-fixture-before-funding)
and [ADR 0037](docs/architecture/0037-finalize-exact-bitcoin-funding-before-first-effect.md).


M3 now has an actual-Core, two-party MuSig2/adaptor P2TR vertical slice.
Exact-pinned `bitcoin` 0.32.101 constructs the aggregate-internal-key plus
CSV-refund commitment, while exact-pinned `musig2` 0.4.1 aggregates the ordered
maker/taker public fixture keys, applies the Taproot tweak, and matches the
rust-bitcoin output key `Q` and parity. One helper process computes two
role-bound nonce commitments, creates and verifies both partial adaptor
signatures and their 65-byte aggregate presignature, adapts it with a labeled
public Regtest scalar, verifies the resulting 64-byte signature under `Q`, and
re-extracts the scalar and checks its point. The isolated runner then executes
the `TakerSellsForeign` Bitcoin-leg ordering through distinct actor `rpcauth`
capabilities. Core policy-checks, decodes, mines, and re-reads the taker funding
at height 102 and maker key-path claim at height 103, including the exact
one-item witness and spent-once outpoint; the final mempool and peer set are
empty and cleanup is exact.

The public BTC SDK now also validates a bounded canonical agreement signed by
both roles. It binds the exact LEZ runtime/program/accounts/amount/deadline and
claim message, Bitcoin genesis and confirmation policy, ordered MuSig2 keys and
adaptor point, reconstructed P2TR/CSV output, exact funding outpoint/value,
cooperative transaction and BIP-341 sighash, and the direction-correct recovery
schedule. Derived Taproot and transaction fields are reconstructed with the
pinned libraries before either role signature is accepted. The reference actor
now activates this agreement and uses it through lock and injected claim
projection. Both its exact Bitcoin and LEZ claim effects are actor-owned,
persist-before-submit, and GREEN in deterministic tests. Run
`m3actor-20260716n` also proves their combined two-direction actual-node
boundary through the public actor.

Pushed commit `0177151` adds the next protocol slice behind canonical byte
boundaries. Separate maker and taker signer-state objects create fresh
OS-random nonces, exchange and verify transcript-bound commitments before nonce
reveal, reject phase/reuse/message/adaptor mismatches, verify both partials, and
derive the same aggregate presignature for the exact BTC message and a
placeholder LEZ message domain. A runnable dual-session fixture proves that
either domain's completed signature reveals the same scalar and completes the
other signature.

Pushed `e3f2938` adds a role-local SQLite adaptor-session journal. It reserves
the exact one-use nonce before exposing its commitment, permits nonce reveal
only after the peer commitment is durable, and atomically consumes the nonce
with an exact replayable partial-signature outbox. Six focused restart,
mutation, reuse, concurrency, and private-file tests plus the 68-test store
suite pass. This PoC journal deliberately stores the nonce plaintext in an
owner-only database until consumption; encryption or HSM custody remains a
production-readiness requirement.

Pushed `8a7ea55` bridges that durability boundary into the BTC SDK. Fresh
MuSig2 material can be reserved before commitment exchange, reconstructed after
restart, checked against the complete session and both role-bound nonce
commitments, consumed once to produce a verified partial, and aggregated into a
verified presignature. Pushed `ca524ff` then moves every maker and taker phase
into a fresh one-shot OS process with a separate owner-only SQLite journal and
canonical public packets. Pushed `96f2a31` adds journal-bound adaptation and
point-checked scalar extraction as separate processes with create-new `0600`
scalar files. Four process journeys cover both the untweaked LEZ and tweaked
Bitcoin domains, restart/replay, cross-wires, unsafe permissions, and
secret-free output.

Pushed `6935acd` adds the actual pinned LEZ v0.2 aggregate-witness guest. A
distinct two-party aggregate authority signs the exact public transaction, but
the escrowed asset is transferred only to the separately committed claimant.
The digest-pinned Risc0 build reproduces ELF `a199c5be...e293` and
ImageID/ProgramId `39b6a4db...4dec`; recursive execution proves the two-party
claim and rejects one-share, wrong-message, mismatched-authority, and preimage
bypass attempts without moving custody.

Pushed `79735dd` exposes that path through the official-wire sidecar. The
claimant role durably reserves the exact canonical LEZ message and official
hash, external maker/taker signing supplies one aggregate signature, and the
sidecar verifies and durably completes the canonical transaction before the
existing one-shot submission boundary can accept it. Preparation survives a
fresh process without rereading the authority nonce. Live local deployment and
submission remain part of the composed runner, not this component claim.

Pushed `3862dde` adds the other side of the witnessed LEZ lock: the depositor
role prepares the exact generated `InitializeNativeWitnessed` account ordering
and a separate `FundNative` transaction, signs only with the depositor key, and
survives restart without combining preparation and submission. Pushed
`f827dad` makes the Bitcoin transaction helper emit the exact public spend plan
and canonical role-runner session, then verify an externally completed
signature before emitting the broadcast-ready one-item-witness transaction.

Pushed `3d7386b` adds conservative live observation of that witnessed LEZ
initialize/fund pair. It validates canonical transaction bytes, signer/account
order, aggregate authority/key, metadata/custody effects, and one stable tip;
an exact miss remains `unknown_or_pending` instead of inventing absence or
finality. Pushed `a3da09e` exposes the witnessed prepare, observe, submit, and
complete calls through a strict typed one-shot operator CLI with a private
capability file. Pushed `bf5bdbd` delegates x-only-key to LEZ-account mapping to
the pinned official `nssa` implementation rather than duplicating it in shell.

The same operator/client/sidecar boundary now has an explicit read-only
`observe-finalized-witnessed-claim` call. Either correctly bound participant
can scan one bounded window only after the indexer finalized tip covers it,
using either an exact transaction ID or peerless discovery from the signed
terms and prepared transcript. The sidecar requires equal block-by-ID and
block-by-hash results plus parent-hash continuity from the window start through
the stable finalized tip, exactly one canonical public claim under the pinned
program and derived accounts, the exact aggregate signature/message/authority, and
`Claimed` metadata plus zero custody read at that containing numeric block ID.
The client independently enforces the requested window and verifies BIP-340
with exact-pinned `secp256k1` 0.29.1. The official indexer supplies historical
account DTOs but no account proof or atomic multi-read snapshot token; that is
recorded as an upstream production trust limitation, not hidden as local
consensus proof. This closes peer-independent typed LEZ claim evidence.

The separate `observe-finalized-witnessed-funding` call is now GREEN. It keeps
the earlier stable-tip progress observer intact, accepts an exact transaction
ID or bounded unique discovery by signed terms, and proves canonical
`FundNative` inclusion plus historical `Funded` metadata and exact custody at
the containing finalized block. The reference actor now uses this evidence for
whichever of the agreement-derived taker or maker funding transitions is LEZ,
before the corresponding local projection. Claim projection now requires both
lock projections and reruns the complete activation-material gate before any
adaptor use; the read-only bridge method still does not itself block its
independent claim methods. The pinned sidecar is the
canonical official-wire decoder and PDA
validator; the graph-isolated client validates the bounded result and role
binding but does not claim an independent official LEZ decode. The actor compares the returned accounts and
transaction evidence
with the signed agreement before durably advancing the lock state.

The next durable M3 component is now implemented locally as
`SqliteBtcRecoveryStore`. Each maker or taker opens a different owner-private
database bound to its exact accepted agreement, role, and agreement-derived
initial coordinator. Four exact public-evidence revisions project the taker
lock, maker lock, revealing claim, and follow-up claim through `BEGIN
IMMEDIATE`, predecessor CAS, and a versioned domain-separated SHA-256 evidence
chain. Reopen recomputes the chain and reconstructs terminal `Completed`
offline. The store retains the exact 64-byte public revealing witness but never
the recovered scalar; focused mutation, rollback, replay, actor-separation,
and scalar-absence checks are 11/11, and all 84 store tests pass. This is a
GREEN persistence component. The reference actor now validates the canonical
agreement and both typed funding observations before projecting revisions one
and two, then projects exact canonical revealing and follow-up claims through
revisions three and four in its injected seam. Chain and SQLite cannot commit
atomically, and the hash chain detects
accidental/tampered history but does not authenticate a database rewritten in
full by its filesystem owner.

The typed `lez-btc-core-adapter` component now validates the other public
evidence source, including the Bitcoin timeout path. It requires exact Bitcoin
Core 31.1, Regtest genesis from the
countersigned agreement, an unpruned synced tip, and synced `txindex` plus
`txospenderindex`. The selected readiness policy distinguishes the fully
disconnected local node from an explicitly network-enabled Regtest node. Exact
funding and key-path claim observations are reconstructed from consensus bytes
and cross-checked against Core's typed vin, vout, identity, size, weight,
confirmation, block, spender, and stable-tip facts. Canonical bounded evidence
records bind the agreement, transaction bytes/IDs, block/tip context, exact
64-byte public claim witness without a scalar, and finalized refund containing height. Submission is one-attempt only:
an owner-provided durable CAS binds txid, wtxid, and the exact raw-byte digest
before mempool policy and one broadcast. Broadcast success is accepted only
after the spender index returns the same complete witness bytes; same-txid but
different-wtxid races are terminal `Unknown` for claims and refunds. Already-known or ambiguous results
become terminal `Unknown`, while conflicting witness bytes fail before another
RPC mutation. The HTTP
transport is literal-loopback, Basic-file authenticated, bounded, one-request
concurrent, and rejects non-`0600`, symlinked, hard-linked, replaced, or changed
credential files. The full 29 all-target test executions plus strict dependency, Clippy, and
rustdoc gates are GREEN. Refund observation additionally binds the
stable-tip-derived funding height to the signed anchor, applies BIP-68 at the
next-block boundary, and emits finalized evidence at the refund containing
height rather than the tip.
Run `m3actor-20260716n` exercises that typed adapter through both actual-node
actor directions. Core 31.1 requires `gettxspendingprevout`'s second parameter
to be the options object
`{"mempool_only":false,"return_spending_tx":true}`; commit `2233964` fixes the
former positional booleans and checks exact spending bytes plus the absent or
present containing block hash. Current `Networked` still means network-enabled
Regtest, not Testnet4 portability.

The v0.2 native-refund planner now closes the next no-RPC component boundary.
It produces the official permissionless `RefundNative` transaction with exactly
metadata, custody, and immutable depositor accounts and with no nonce or
witness. Strict legacy hashlock and M3 witnessed terms are supported; the
witnessed aggregate account is recomputed even though it does not sign the
refund. Owner-only durable reservation precedes byte exposure, identical
restart replay performs no nonce lookup, and transaction or identity mutations
fail closed. Five planner tests, one authenticated prepare/restart test, and
nine finalized-observer tests are GREEN. Prepare persists its canonical request
and result, restores the planner before bind, and replays byte-identically with
no nonce lookup. State-only observation brackets Funded or Refunded accounts at
one stable finalized clock; exact/discovery scan only fully covered finalized
ancestry, require equal by-ID/by-hash blocks, enforce the containing timestamp at
or after the signed deadline, and prove historical plus tip Refunded state with
zero custody. The authenticated observation is repeatable and never submits.
The actor-local recovery store now replays the ordered maker- then taker-funded
refund branch to terminal `Refunded` in both directions without changing old
happy-path evidence bytes. Public `recover` composes both LEZ and Bitcoin
refund legs through revision 2 to 3 to 4 in deterministic role tests. Exact
bytes are durable before the one `Prepared` to `Started` CAS; only affirmative
stable eligibility grants one attempt, and `Started`, `Unknown`, or `Accepted`
never rearm. Owner and nonowner roles project only later exact finalized
evidence. Run `m3refund-20260716h` now closes the fresh two-direction
actual-node refund evidence boundary recorded by ADR 0038; the separate
refund-side absent-maker boundary is GREEN in `m3firstlock-20260716h`, while
maker-lock cutoff admission, concurrency, process-kill, and reorg remain open.

The older retained actual-Core run remains a one-process public deterministic
cryptographic and consensus fixture. The operator-composed run closes live
witnessed submission, both happy directions, and the PoC atomicity/recovery
order through separate role processes. The public actor source now owns both
claim effects, and run-n now retains their fresh actual-node composition. This
closes the progressive private local PoC, but does **not** close the accepted
proposal's production-ready scope: native/custom-token parity,
survivor-specific recovery, maker-lock cutoff/race and concurrent demos, Testnet4
setup/execution, production key custody/Core adapter, QA/chaos/infosec
campaigns, and GW-M3-001 disposition remain. There is no `m3-complete` tag. CI
runs the same P2TR funding/claim
composition and
fail-hard scans
the exact Core image for HIGH/CRITICAL vulnerabilities. The earlier clean
infrastructure run remains available as
[secret-safe Core evidence](docs/evidence/m3-bitcoin-core-smoke-a7393df-20260714.json);
the historical clean known-key P2TR run remains as
[secret-safe funding/claim evidence](docs/evidence/m3-bitcoin-core-p2tr-4f7b6b3-20260715.json),
and strict clean run `m3-musig-exact-f5a9caa` on pushed commit `f5a9caa` is
retained as
[MuSig2/adaptor/extraction Core evidence](docs/evidence/m3-bitcoin-core-musig2-f5a9caa-20260715.json).
Runtime uses no public RPC, faucet, public funds, public peers, or public chain.
Cold setup still depends on checksum-verified Core release assets, the pinned
base image, vulnerability data, and locked Rust registries, so availability and
scan flakiness remain explicit. GnuPG uses a run-owned home, and the verifier
terminates only that home's agent rather than using broad host cleanup.

Development has started with protocol and real-node acceptance tests. The
current executable slices enforce:

- the taker-funded lock is confirmed before the maker can lock the second leg;
- claim completion after the first lock needs only on-chain evidence; and
- pair-specific claim and recovery ordering, including LEZ-before-ZEC claim and
  refund in both ZEC trade directions;
- independent fixed-role SDK actors with separate schema-v10 SQLite databases
  now complete both ZEC directions through a preimage-revealing LEZ claim and
  the counterparty's Zcash follow-up, then independently
  `resume_claim_capable` at `Completed`. The same externally supplied claim key
  is required when each role reopens its own database; neither plaintext
  preimages nor plaintext exact claim bytes are stored in SQLite or its WAL;
- immutable local/public-testnet ZEC profiles with network/branch binding,
  checked deadlines, required calibration, and exact margin enforcement;
- typed ZEC observations that re-decode canonical transaction bytes and bind
  network, branch, block, outpoint, value, exact BIP-199 scripts, and depth
  before projecting evidence into the chain-independent coordinator, populated
  from stable actual Zebra RPC queries in the actor E2E;
- exact BIP-199 P2SH plus canonical Zcash V5 funding, claim, and refund
  transactions; and
- actor-keyed funding/claim/refund acceptance and rejection through pinned
  Zebra NU6.2 Regtest consensus, including a two-node conflicting
  four-over-three-block canonical fork replacement; and
- checked-guest deployment plus real-key native LEZ initialize/fund/claim and
  permissionless-refund execution in an isolated standalone sequencer; and
- two-definition official-ATA claim/refund lifecycles with real owner keys,
  immutable destinations, and cross-definition substitution rejection; and
- machine-checked recursive native/authenticated-transfer and token/ATA/Token
  Risc0 session costs with setup and Clock noise excluded; and
- a bounded dual-signed LEZ/ZEC agreement integrated through role-fixed
  negotiation, persistence-before-activation, and adversarial resume, without
  exposing transport, raw chain, or recovery-store handles after activation;
  plus exact first-lock intent staged before node effects, observe-before-exact
  rebroadcast after restart, and separately recoverable LEZ initialize/fund
  steps; confirmed evidence is applied only after an atomic store commit or an
  exact unknown-outcome probe, and is replayed on resume. A role-fixed
  schema-v10 SQLite adapter now proves exact replay, role isolation, retained
  closed-intent validation, atomic rollback, corruption rejection, and
  close/reopen recovery. Its ordered maker journal durably replays canonical
  Zcash evidence, atomic reorg replacement, same-inclusion depth changes, and
  affirmative removal through the exact canonical tracker. Replacement halves
  must share one stable tip, unchanged polls write nothing, and the store
  rejects orphan/holey histories, individually valid but
  history-incompatible appends, and stale-instance divergence. The maker
  independently observes only the
  agreement-selected taker-lock chain and replays that role-local projection
  without taker intent or negotiation state. Forward Zcash rejects a weak
  transaction-ID/depth assertion and durably revalidates the complete canonical
  transaction/block/tip/output record against the signed agreement's exact
  HTLC output binding. Role-local input/change/fee/expiry policy constrains this
  SDK's own builder and is not a remote-wallet acceptance condition. These
  first-lock observations remain non-authorizing on their own. A distinct fresh
  eligibility call replays the durable head, re-queries the exact canonical
  tracker head, writes nothing when unchanged, and returns a non-cached
  revision-bound result. The maker effect now consumes that result internally,
  persists the direction-fixed opposite-chain plan before submission, and
  atomically projects confirmed Maker funding. Both directions reach
  `BothLegsLocked` and survive schema-v10 SQLite close/reopen; `next_action`
  still caches no permission.
  Reverse deterministic-local LEZ accepts a depth-sufficient exact head.
  The public-v0.2 policy seam additionally defines and unit-tests typed
  awaiting-finality outcomes until Bedrock reports Finalized, but public
  agreement activation remains fail-closed pending a reviewed deployment.
  Reverse LEZ requires a stable canonical
  escrow snapshot bound to the signed execution channel/genesis, public fund
  transaction, generated account order, full metadata, exact custody, depth,
  and finality policy; that primitive snapshot is revalidated after SQLite
  close/reopen. A dependency-free two-phase LEZ tracker now proves duplicate
  suppression, monotonic Pending/Safe/Finalized updates, affirmative same-tip
  replacement, stale/tip-regressing evidence rejection, and fatal
  finalized-history changes.
  Revealing LEZ claims now have the same primitive-evidence discipline: the SDK
  binds the node-reported ID to the official-decoder hash, claimant signature,
  generated accounts, exact claim/preimage, terminal metadata, empty custody,
  canonical inclusion, and depth. New secret-free schema-v2 snapshots are fully
  revalidated on SQLite replay with the separately protected preimage; legacy
  opaque v1 rows are read-compatible but cannot be created by live adapters.
  The active SDK and schema-v10 SQLite journal now fold the agreement-selected
  LEZ tracker: exact duplicates write no row and same-inclusion finality/depth
  updates survive close/reopen. Affirmative nonfinal removal and atomic same-tip
  replacement now use complete primitive records, reject stale old-head
  evidence, consume one revision, and replay through SQLite. Official-wire LEZ
  native escrow, revealing-claim, and native-refund conversion is implemented;
  the context-owning SDK-port wrapper, independent actor processes, and the
  completed real-node corridor remain. Schema-v10 now also persists exact
  refund owner intents before broadcast and atomically commits owner/observer
  transitions through `Refunded` in both directions, including rollback,
  conflict, corruption, and close/reopen replay.
  The M2 lane introduced a bounded authenticated eight-method LEZ sidecar
  client; the current M3 lane extends the same protocol to fourteen methods,
  a signed-agreement native first-lock bridge adapter, typed Zebra
  owner/counterparty claim and refund ports, and the public crash-safe
  timeout-refund SDK contract. The bridge client binds every request
  and response to one run, role, runtime, and one-use request ID; the Zebra
  adapter converts compatibility-selected signed native terms into exact
  initialize/fund SDK bytes without retrying randomized preparation. The Zebra
  adapter derives exact follow-up claims and refunds from the accepted
  agreement, delegates only signing to a role-local capability, revalidates
  stable canonical funding and signed transaction policy, observes before
  byte-identical rebroadcast, and treats ambiguous submission outcomes
  conservatively. Counterparty discovery scans a bounded canonical Zebra
  horizon and treats unresolved or older spends as unstable, never absent. The
  refund driver fixes LEZ-before-Zcash order in both directions, persists exact
  owner bytes before broadcast, distinguishes unknown outcomes, and uses
  observation-only transitions for the other role.
  These are isolated contract tests, not yet a composed maker/taker user flow.
  The sidecar server library now authenticates one run/role capability before
  parsing, restores exact official prepared bytes, and durably guards unknown
  submissions before the node call. Official revealing-claim preparation now
  binds the signed role, runtime, signer, terms, preimage, and funding identity,
  restores the exact randomized bytes after restart, and admits only that
  cached transaction for submission. Native escrow observation now decodes
  official transactions, signatures, instructions, metadata, custody, block
  links, genesis, and stable tip brackets for exact owners and bounded
  counterparty discovery; the main adapter independently revalidates those
  primitive facts against the signed agreement. Bounded or old misses remain
  unknown, never false absence. Official revealing-claim observation now
  validates the canonical Risc0 instruction, message, witness, ordered accounts,
  transaction placement, terminal metadata, and zero custody for exact owners
  or bounded counterparty discovery. Only a complete stable window is absent;
  partial coverage is unknown and ambiguity or a moving tip fails closed. The
  executable runner starts concurrent maker and taker sidecars with separate
  private keys, capabilities, runtime descriptors, durable stores, and
  ephemeral loopback listeners. All eight M2 methods execute, and the six M3
  witnessed preparation, finalized-funding, and finalized-claim methods are
  separately covered. Native
  refunds are official permissionless `RefundNative` transactions with no
  nonce or witness; exact-owner and bounded counterparty observations require
  a stable clock, terminal refunded accounts, zero custody, canonical bytes,
  and restart-safe cache membership. The main native-refund and revealing-claim
  adapters validate both signed directions, exact caller-owned IDs/windows,
  complete primitive facts, durable identities/bytes, and conservative
  one-attempt outcomes. Zebra now also discovers agreement-bound unknown-ID
  funding for both role directions from stable canonical block and mempool
  evidence. A reusable external exact-v0.1.2 standalone-node implementation
  verifies the tracked guest ELF SHA-256 and Risc0 ImageID before creating any
  state, starts from a fresh mode-0700 home on a dynamic port, deploys the
  checked guest, and publishes a no-clobber mode-0600 readiness manifest. The
  private handoff binds the loopback endpoint, channel, genesis ID/hash,
  ELF/ImageID/ProgramId, canonical deployment transaction and containing block,
  the advertised authenticated-transfer built-in, and two official-RPC-verified
  funded actor accounts and signing keys; tampered guests and pre-existing homes
  fail without mutation. Its first exact process run exposed and rejected an
  incorrect assumption that `getProgramIds` listed custom deployments. The
  last corrected full exact runner was GREEN: schema-v2 transaction/block evidence,
  process rejection paths, native/two-definition actor lifecycles, strict
  Clippy, and recursive cost reproduction all pass. A subsequent actor-contract
  RED found that its all-zero deterministic channel could not enter a signed
  SDK agreement; the node/config/readiness source now uses one nonempty fixed
  channel and its focused locked-graph suite is GREEN. The exact full runner is
  rechecked before corridor evidence or M2 certification. This completes the
  local node handoff, but not its consumption by independent corridor actors.
  Context-owning SDK-port composition is GREEN. The exact-outpoint Zebra
  funding planner is GREEN. The Unix-only one-shot maker/taker boundary now
  loads a deny-unknown-fields schema-v3 private configuration that fixes the
  run, swap, role, signed-agreement SHA-256, LEZ runtime, Zebra identity,
  typed Zebra route, discovery window, exact funding outpoints, and every
  role-local persistence and credential path. Thirty boundary tests reject
  unsafe permissions, symlinks, hard-link/path aliases, same-inode rewrites,
  late alias creation, wrong roles/identities/routes, unsafe credential
  combinations, and secret-bearing diagnostics. `status` remains deliberately offline: terminal
  SDK replay has no LEZ/Zebra trait bound and needs only the role store plus
  claim-recovery key; effect credentials and chain endpoints may be unavailable.
  `activate` and `drive` now compose descriptor-bound SQLite, the authenticated
  loopback role sidecar, and the selected local or public-capable Zebra
  transport. Both local LEZ-plus-Zebra directions are completed evidence;
  public execution is not.

See the living [implementation plan](docs/implementation-plan.md), the
[milestone delivery metrics](docs/milestone-metrics.md), the
[whole-system actor and flow architecture](docs/architecture/system-architecture.md),
the [deployment component and RPC inventory](docs/architecture/deployment-components-and-rpcs.md),
the [architecture decision log](docs/architecture/README.md), the living
[manual reproduction guide](docs/manual-user-flows.md), and the first
[acceptance tests](crates/swap-core/tests/e2e_swap_lifecycle.rs). The
[upstream Logos production-blocker register](docs/upstream-production-blockers.md)
separates disclosed external release risks from repository-controlled milestone
acceptance. The
[progressive milestone delivery decision](docs/architecture/0027-progressive-jpeg-milestone-delivery.md)
puts the active milestone reproducible local-devnet happy path first, then
enters QA with RED-GREEN-REFACTOR, chaos, information-security, and production-
readiness hardening only when the repository owner ends each phase. M2 is
currently in the PoC phase; earlier hardening remains carried evidence, not a
claim that those later phases are complete. The
[private local M2 certification decision](docs/architecture/0023-private-local-m2-certification.md)
requires one actual public-compatible local LEZ v0.2 devnet, one actual local
Zcash Regtest devnet, and
independent maker/taker processes while deferring public evidence without
claiming it exists. The
[source-audited local-stack decision](docs/architecture/0024-source-audited-lez-v0-2-local-stack.md)
binds the exact Bedrock image/source labels, LEZ source, toolchain, native
inputs, service flows, and service-binary hashes. Retained run
`m2poc-vertical-20260714a` proves the three official local v0.2 services, both
finalized actor Vault Claims, checked escrow deployment, and a role-separated
native initialize/fund/claim lifecycle in finalized blocks 219/220/223. Fresh
isolated chain runs `m2poc-fresh-lez-20260714a` and
`m2poc-fresh-zebra-20260714a` then supported both completed
reference-actor corridors. In the first run,
`m2poc-corridor-fresh-20260714o`, the
`TakerSellsLez` role order, the taker initialized and funded LEZ, the maker
observed it and funded the Zcash HTLC, the maker waited for two Zcash
confirmations and revealed the preimage by claiming LEZ, and the taker used that
reveal to claim Zcash. Both independent actor stores reached `Completed`
revision 4 after 39 drive rounds and 78 actor events in 25.370 seconds. One
payload-free `moving_tip` observation failure was retried once within the
maximum-eight same-run policy and then succeeded.

LEZ initialize/fund/claim finalized in blocks 264/265/266 and ended `Claimed`
with custody 0, depositor balance 100000, and claimant balance 150000. Zebra
funding transaction `255b991f...dceab` entered block 106, received the required
second confirmation in block 107, and claim transaction `a2b41c5f...be16e`
spent its `:0` HTLC output in block 108. No public RPC, faucet, or public funds
were used. Exact secret-safe facts are in the
[first-direction corridor evidence](docs/evidence/m2-taker-sells-lez-corridor-20260714.json);
the earlier [local-onboarding evidence](docs/evidence/m2-local-onboarding-20260714.json)
remains the component baseline. Failed fresh attempts 14i and 14k through 14n
made no chain effect. Attempt 14j stopped after only one Zcash confirmation and
retains 50000 LEZ in its distinct failed swap; its files and funds must never be
reused.

Reverse run `m2poc-corridor-reverse-fresh-20260714c` then completed
`TakerSellsForeign`. The taker funded Zcash at height 113, the maker funded LEZ
in finalized blocks 641/642, the taker revealed by claiming LEZ in finalized
block 643, and the maker spent the exact Zcash `:0` output at height 115. Both
actors reached revision 4 `Completed` in 26.960 seconds. Terminal LEZ state was
`Claimed` with custody 0, maker depositor balance 0, and taker claimant balance
150000. Two prior fresh reverse attempts are retained and never reused; they
exposed and reproduced a forward-only canonical LEZ validator, now corrected
to bind the signer to the agreement-derived depositor. Exact secret-safe facts
are in the
[reverse-direction corridor evidence](docs/evidence/m2-taker-sells-foreign-corridor-20260714.json).
The M2 local-functional PoC is certified **2 of 2** under the annotated
`m2-complete` tag. The tag binds the exact closure tree to the canonical
evidence packet; it does not claim that the owner has entered QA, M3, or the
deferred recovery, chaos, public-execution, and production-readiness phases. The
schema-v3 Zebra route selection, public HTTPS `x-api-key` transport, and LEZ
`official_public` sidecar route are now locally verified portability contracts;
they have not made a public call.
Current-schema certification runs
`m2cert-schema3-forward-2d09997-20260714a` and
`m2cert-schema3-reverse-2d09997-20260714a` also repeated both directions through
the actual pinned local LEZ v0.2 and Zebra Regtest nodes. Both independent actors
reached `completed`, the atomic effect order was observed, and no public RPC or
faucet was used. The secret-safe aggregate is in the
[schema-v3 corridor evidence](docs/evidence/m2-schema-v3-local-corridors-20260714.json).
Those earlier runs are retained as historical behavior evidence. Final local
certification rebuilt the guest through the exact digest-pinned Risc0 Docker
builder, and the independently Docker-backed methods embedding produced the
same ELF `c85055f6...c9d2e` and ImageID/ProgramId `5cf8c5a4...329c1`. That
artifact was deployed once and finalized in local LEZ block 2582; canonical
runs `m2cert-canonical-forward-bb53daf-20260714a` and
`m2cert-canonical-reverse-bb53daf-20260714a` then completed both directions
against that deployment and Zebra Regtest. The new immutable
[canonical certification packet](docs/evidence/m2-canonical-local-certification-20260714.json)
binds the builder, artifact, deployment, actors, exact chain effects, terminal
states, and absence of public resources without rewriting earlier evidence.
PoC-to-hardening and milestone
transitions remain repository-owner decisions. The
[Zcash public-testnet setup guide](docs/zcash-testnet-setup.md) records the
selected self-hosted and Tatum Testnet Zebrad routes, optional funding wallet,
external dependencies, and the still-missing public credentials, funded
accounts, deployment, and live method evidence without claiming a completed
testnet run.

## Development

Prerequisites: Rust 1.96.0. Docker is needed for the isolated Zebra consensus
suite, pinned Risc0 guest builder, and full local LEZ v0.2 lane; Docker Compose
v2 is used by both local-chain suites. Building the exact upstream v0.2
sequencer/indexer artifacts additionally uses upstream Rust 1.94.0 plus the
hash-checked r0vm and Rapisnark inputs; the repository-owned sidecar remains on
Rust 1.96.0. Direct Cargo commands do not certify that v0.2 sidecar because the
upstream Rapisnark build script can download native libraries even with Cargo
offline; use the hash-attesting wrapper documented in the manual guide. The
[manual reproduction guide](docs/manual-user-flows.md) lists the complete
per-run prerequisites, isolation rules, commands, expected evidence, and
cleanup behavior.

### Local LEZ v0.2 service-readiness quick start

From a clean host, provision a clean exact LEZ `v0.2.0` checkout, the two
locked service binaries, and verified `r0vm 3.0.5` as described in the
[manual flow](docs/manual-user-flows.md#flow-0b2-run-the-isolated-lez-v02-service-stack).
Then run:

```sh
export LEZ_V02_SOURCE_DIR=/absolute/path/to/clean/logos-execution-zone-v0.2.0
export LEZ_V02_SERVICES_DIR=/absolute/path/to/locked/release-binaries
export LEZ_V02_R0VM=/absolute/path/to/verified/r0vm
RUN_ID=manual-v02-stack-001 ./scripts/run-lez-v02-stack.sh
```

The command creates unique run-scoped containers and a no-masquerade bridge,
uses dynamic `127.0.0.1` RPC ports, writes evidence below
`.e2e/manual-v02-stack-001/lez-v02`, and removes plus asserts absence of only
its exact containers, network, and image. It uses no public chain RPC, faucet,
or public funds. A cold setup can still depend on GHCR/GCR for the two exact
digest-pinned images and on GitHub/Rust/crates distribution while provisioning
source and binaries. This proves LEZ service readiness only; it is not yet the
manual atomic-swap corridor.

### M3 public-actor local PoC quick start

First complete the M3 guide's pinned LEZ verifier so the checked guest deployer
exists, and provision the clean LEZ v0.2 source, exact service binaries, r0vm,
four verified Rapidsnark libraries, and offline Cargo graphs. Then let the
repository-owned runner create both local chains and both fresh directions:

```sh
export RUN_ID=m3actor-manual-20260715a
export LEZ_V02_SOURCE_DIR=/absolute/path/to/clean/logos-execution-zone-v0.2.0
export LEZ_V02_SERVICES_DIR=/absolute/path/to/locked/release-binaries
export LEZ_V02_R0VM=/absolute/path/to/verified/r0vm
export LEZ_V02_ARTIFACT_TARGET_DIR=/absolute/path/to/verified/lez-artifact-target
export RAPIDSNARK_LIB_DIR=/absolute/path/to/verified/rapidsnark-v0.0.8-libraries
export BINDGEN_EXTRA_CLANG_ARGS=-I/usr/lib/gcc/x86_64-linux-gnu/13/include
./scripts/run-m3-actor-local-poc.sh

export M3_EVIDENCE=".e2e/${RUN_ID}/m3-actor-poc/evidence"
jq -e '.result == "passed" and
  (.directions | map(.direction) ==
    ["taker_sells_foreign", "taker_sells_lez"]) and
  all(.directions[];
    .terminal_revision == 4 and .terminal_phase == "completed") and
  .actor_process_model == "fresh_one_shot_process_per_command" and
  .replay_resubmission_count == 0 and
  .public_rpc_used == false and .faucet_used == false and
  .public_funds_used == false and .private_material_disclosed == false' \
  "$M3_EVIDENCE/m3-actor-local-poc.json"
jq -e '.result == "passed" and .all_exact_run_resources_absent == true and
  .foreign_resources_targeted == false and .broad_cleanup_used == false' \
  "$M3_EVIDENCE/cleanup-attestation.json"
```

To reproduce the actual-node timeout/refund journey, keep the same verified
prerequisite variables but select `refund` and a different, never-before-used run
ID. Change the example ID for every attempt; a failed or successful root is
spent and the runner refuses to reuse it.

```sh
export RUN_ID=m3refund-manual-001
export M3_ACTOR_POC_JOURNEY=refund
./scripts/run-m3-actor-local-poc.sh

export M3_EVIDENCE=".e2e/$RUN_ID/m3-actor-poc/evidence"
jq -e '.kind == "m3_actor_two_direction_refund_local_poc" and
  .journey == "refund" and .result == "passed" and
  all(.directions[];
    .terminal_revision == 4 and .terminal_phase == "refunded") and
  .expected_unique_effects_by_direction == {
    taker_sells_foreign:{bitcoin:2,lez:3},
    taker_sells_lez:{bitcoin:2,lez:3}} and
  .replay_command == "recover" and .replay_resubmission_count == 0 and
  .services.bitcoin_core == {run_id: (.run_id + "-btc"),
    version: "31.1", network: "regtest"} and
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
```

The refund runner waits for the countersigned deadlines and executes the two
directions sequentially. Run H's retained evidence-to-cleanup span was 54
minutes 5 seconds with 3.0-second LEZ slots; local load, finality progress, and
moving-tip retries can extend that. A timeout is uncertain observation and
never grants another submission.

To reproduce the first-lock absent-maker journey, keep the verified
prerequisites, choose another fresh run ID, and select `first_lock_refund`:

```sh
export RUN_ID=m3firstlock-manual-001
export M3_ACTOR_POC_JOURNEY=first_lock_refund
./scripts/run-m3-actor-local-poc.sh

export M3_EVIDENCE=".e2e/$RUN_ID/m3-actor-poc/evidence"
jq -e '.kind == "m3_actor_two_direction_first_lock_refund_local_poc" and
  .journey == "first_lock_refund" and .result == "passed" and
  all(.directions[];
    .terminal_revision == 2 and .terminal_phase == "refunded" and
    .maker_second_lock_effect_count == 0) and
  .expected_unique_effects_by_direction == {
    taker_sells_foreign:{bitcoin:2,lez:0},
    taker_sells_lez:{bitcoin:0,lez:3}} and
  .first_lock_refund_admission.two_fresh_cross_chain_reads == true and
  .first_lock_refund_admission.fresh_maker_observer == true and
  .replay_resubmission_count == 0 and
  .external_resources.certification_success_depends_on_external_network == false' \
  "$M3_EVIDENCE/m3-actor-local-poc.json"
jq -e '.journey == "first_lock_refund" and .result == "passed" and
  .all_exact_run_resources_absent == true and
  .foreign_resources_targeted == false and .broad_cleanup_used == false' \
  "$M3_EVIDENCE/cleanup-attestation.json"
```

The directions run sequentially and intentionally wait for signed boundaries.
Advancing LEZ finality can yield typed `moving_tip`; only that read-only
condition is retried with a fresh request ID, and retained retry evidence proves
the durable LEZ submission count did not change.

The audited run used the same entry point at `6ded2f9`; these are its exact
run-owned and native-artifact inputs. This is an audit record, not a command to
reuse—the runner must reject the existing run ID and root:

```sh
RUN_ID=m3actor-20260716n \
LEZ_V02_SOURCE_DIR=/tmp/lez-v020-native-investigation \
LEZ_V02_SERVICES_DIR=/tmp/lez-v02-services-a58fbce2-20260713/release \
LEZ_V02_R0VM=/tmp/lez-atomic-swaps-tools/risc0-3.0.5/home/extensions/v3.0.5-cargo-risczero-x86_64-unknown-linux-gnu/r0vm \
LEZ_V02_ARTIFACT_TARGET_DIR=/tmp/lez-m3-artifact-20260715a \
RAPIDSNARK_LIB_DIR=/tmp/lez-atomic-swaps-tools/rapidsnark-v0.0.8/d4133227 \
BINDGEN_EXTRA_CLANG_ARGS=-I/usr/lib/gcc/x86_64-linux-gnu/13/include \
./scripts/run-m3-actor-local-poc.sh
```

Its terminal and cleanup packets remain at
`.e2e/m3actor-20260716n/m3-actor-poc/evidence/`. The containing run root remains
owner-private because it also retains credentials, keys, signed transactions,
and actor/signer state; publish only separately reviewed secret-safe summaries.

Use a never-before-used 8–48 character lowercase run ID. The runner refuses
pre-existing run roots or same-ID Docker resources, uses dynamic literal-
loopback RPC ports, executes the two directions sequentially, and cleans only
captured exact IDs on success or failure. Its root and sidecar builds are
offline by design; populate their pinned caches before starting. Core release
and Guix provenance, the LEZ/Risc0 artifacts, Docker images, and cold Cargo/git
inputs are setup dependencies and can fail because of DNS, registry, or host
availability. Runtime chain I/O uses only local Core Regtest and the local LEZ
Bedrock/sequencer/indexer; funds are fresh local Regtest/genesis outputs, with
no public chain RPC, peer, faucet, public deployment, or public funds. The
pinned Bedrock binary also makes best-effort UDP NTP attempts to
`pool.ntp.org:123`; certification does not depend on success, and the M3 runner
records the observed timeout count. Thus the chain boundary is fully local, but
the Bedrock process is not claimed to make zero egress attempts. See the
[M3 operator guide](docs/m3-local-poc-operator-guide.md) for exact builds,
proof boundaries, private evidence handling, and failure recovery.

### M2 corridor and route-selection quick start

After provisioning fresh isolated LEZ v0.2 and Zebra Regtest nodes with the
manual guide's Flow 0 prerequisites, build and run the same local user boundary
used by the retained PoC:

```sh
cargo build --locked -p zec-reference-actor --bin zec-reference-actor
export RUN_ID=manual-m2-corridor-001
export POC_DIRECTION=taker_sells_lez # or: taker_sells_foreign
export POC_OUTPUT_ROOT="${TMPDIR:-/tmp}/lez-atomic-swaps-${RUN_ID}"
export LEZ_SEQUENCER_URL=http://127.0.0.1:<sequencer-port>
export LEZ_INDEXER_URL=http://127.0.0.1:<indexer-port>
export ZEBRA_RPC_URL=http://127.0.0.1:<zebra-port>
export ESCROW_PROGRAM_ID=5cf8c5a4eedb3c2873956cb7898eb33a495407c9746fb1a065c99638159329c1
export RAPIDSNARK_LIB_DIR=/absolute/path/to/verified/rapidsnark-v0.0.8-libraries
./scripts/run-m2-taker-sells-lez-poc.sh
```

The historical script name covers both directions. It refuses a reused output
root, serializes access to the exact node tuple, and uses fresh local
genesis/Regtest funds. See
[Flow 0G](docs/manual-user-flows.md#flow-0g-run-either-development-m2-corridor-direction)
for all prerequisites, evidence assertions, and cleanup rules.

The role-private `zec-reference-actor` schema is version 3. Its `zebra.route`
is exactly one of these deny-unknown-fields objects:

```json
{
  "kind": "deterministic_local",
  "endpoint": "http://127.0.0.1:18232",
  "cookie_file": null
}
```

```json
{
  "kind": "self_hosted_cookie",
  "endpoint": "http://127.0.0.1:8232",
  "cookie_file": "/absolute/private/run/maker-zebra.cookie"
}
```

```json
{
  "kind": "tatum_testnet_x_api_key",
  "endpoint": "https://zcash-testnet-zebrad.gateway.tatum.io",
  "api_key_file": "/absolute/private/run/maker-tatum-api-key"
}
```

`deterministic_local` requires the Regtest identity; `self_hosted_cookie`
requires a matching public Mainnet or Testnet identity; and the exact Tatum
route requires Testnet. The two role configs select the same route kind and
endpoint. Any cookie or API-key file, each actor config, signer key, claim key,
preimage, and sidecar capability must be a regular owner-only mode-`0600` file
below a mode-`0700` role directory. Never put a credential in a URL, JSON
value, command line, log, or committed file. The actor loads credentials only
for `drive`; `status` remains offline.

The LEZ v0.2 sidecar independently selects one complete outbound node profile.
Local runs use `--node-profile local` with distinct literal-loopback sequencer
and indexer URLs. The dormant public route uses
`--node-profile official_public` and requires the exact URL
`https://testnet.lez.logos.co/` for both `--sequencer-url` and `--indexer-url`.
In either profile, `--listen-address` and each actor's `bridge.endpoint` remain
dedicated `127.0.0.1:<role-port>` listeners protected by a role/run capability;
the actor-to-sidecar hop is never public.

Moving from the proved local route to public Testnet requires only route
selection under the signed agreement/runtime configuration plus the expected
on-chain deployment and account/key/fund provisioning. It does not require a
different actor, sidecar, or chain adapter. No automated test, retained M2 run,
or manual command in this repository has called either public chain endpoint,
used a faucet, or spent public funds. Live public deployment and method evidence
remain deliberately deferred under the progressive-PoC boundary.

### External dependencies and flakiness

The current automated and retained local PoC flows use no public blockchain
RPC, faucet, credential, or public funds. The pinned LEZ Bedrock component can
make best-effort UDP NTP attempts to `pool.ntp.org:123`; local certification
requires no successful reply and records observed timeout attempts. This is an
external egress attempt, not a public chain dependency. Public-route parsing,
TLS client
construction, credential loading/redaction, and strict LEZ profile selection
are tested without connecting. The successful corridor used dynamic-loopback
Bedrock, sequencer, indexer, Zebra Regtest, and two independently authenticated
role-sidecar processes. Its retained ports `32831` through `32834`, maker
sidecar port `52289`, and taker sidecar port `49643` belong only to the named evidence
runs; manual runs must allocate fresh dynamic ports and a fresh output root. The
official LEZ v0.2 endpoint
`https://testnet.lez.logos.co` is selected and its health/block/program methods
were checked on 2026-07-12, but no repository user flow submits to it yet.
Maker, Zebra-adapter, and sidecar host endpoints are ephemeral loopback
services. The LEZ
test client uses loopback, but pinned upstream v0.1.2 binds its ephemeral server
to the host wildcard address; it is short-lived and collision-isolated, not
loopback/network-namespace isolated. The reusable external node refuses an
existing home, creates its own mode-0700 directory, and publishes only a
dynamic `127.0.0.1` client endpoint in a mode-0600 readiness file. That file is
secret-bearing because it carries the two deterministic genesis signing keys;
it must remain run-local and must never be logged or committed. Test funds are
deterministic local genesis/Regtest outputs whose account IDs, key derivations,
authenticated-transfer ownership, and positive balances are re-read through
the official LEZ RPC before readiness. Upstream `getProgramIds` is a static map
of built-ins, not a deployed-program registry: the process uses it only to bind
the authenticated-transfer owner. Custom guest deployment is proved by exact
`getTransaction` bytes plus the containing canonical `getBlock` ID/hash stored
in readiness. Cold builds still depend on
rustup/crates.io, locked GitHub sources, digest-pinned Docker Hub/GCR images,
the checksum-pinned Logos circuits release, and `rzup`'s pinned Risc0 tools.
Availability, DNS, proxy, registry throttling, or GitHub/CDN outages can block
an uncached run, but cannot relax the lockfile, digest, checksum, ELF, ImageID,
or consensus checks. Warm verified caches reduce this availability risk.

The M3 root agreement provisioner is not a network service and performs no RPC
or Docker action. On warm caches its only external runtime input is the OS
random source; the official account helper also performs no RPC but an uncached
build shares the separate pinned sidecar graph's Cargo/git/native-library
availability. The actual run-owned Core and LEZ observations surround the
three local commands. Core `gettxout` precedes `prepare-funding`; read-only
`testmempoolaccept` of its exact persisted bytes and finalized LEZ preparation
facts precede agreement finalization; exact broadcast and the planned one-block
mine occur only after both chain presignatures are durable. Policy/finality
delay, moving tips, local readiness, or operator transcription can make the
ceremony fail closed, but must never be treated as permission to fabricate a
fact, relax a policy, reuse the output root, or overwrite its create-new files.

These are real local on-chain executions, not mocks: pinned Zebra
validates/mempools/mines signed Zcash
transactions and chooses a higher-work fork; the pinned LEZ sequencer deploys
the checked guest, executes production state transitions, and persists
canonical actor/custody state. Loopback supplies safe isolation while the real
consensus/state-transition implementations supply fidelity. Regtest/standalone
do not prove public peer
propagation, fee markets, organic timing/reorg behavior, provider quirks, or LEZ
testnet 0.2 compatibility. Both composed private local directions are now
proved through independent actor processes. Public deployment and
public-testnet execution are explicitly deferred
to production readiness under ADR 0023; the same binaries and adapters must
switch routes through signed configuration/provisioning only.

The in-memory and schema-v10 SQLite actor lifecycle tests are a separate,
deterministic lower lane. They start no node or service and use no RPC, Docker,
faucet, public endpoint, or network access. Their only runtime resources are
temporary local maker/taker databases and an explicitly supplied deterministic
test claim key. Consequently, public-chain availability cannot make those
tests flaky; actual Zebra and LEZ node execution remains covered by the
separate node suites and is not implied by the contract-double corridor.

CI also refreshes RustSec and Trivy vulnerability data. A database outage may
block scanning; a newly published advisory may deliberately turn a prior pass
red. Do not bypass that failure as “flaky.” The LEZ v0.2 RPC, self-hosted Zebra
6.0.0, and Tatum's API-key-authenticated Testnet Zebrad gateway are selected.
The Tatum route is a third-party authoritative-node service, not an official
Zcash Foundation endpoint. Its bounded HTTPS `x-api-key` adapter and schema-v3
actor wiring are locally GREEN, while its live method contract has no evidence
yet. Zcash funding may use a community faucet, Discord request, or controlled
pre-funded wallet, all with explicit availability risk. The role-keyed signer
is wired, but no public key, TAZ funding, broadcast, or confirmation has been
exercised. Provider limits, fallback routes, and funding assumptions remain
production-readiness evidence; M2 retains no live public-execution requirement
under ADR 0023. See
the [full resource/flakiness table](docs/manual-user-flows.md#external-resources-and-flakiness).

    cargo test --locked --workspace --all-targets
    cargo fmt --all --check
    cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
    cargo deny check advisories bans licenses sources
    npm ci
    npm audit --audit-level=moderate
    npm run audit:licenses
    npm run test:mermaid
    RUN_ID=local-lez-v02-a ./scripts/verify-lez-v02-provisional.sh
    RUN_ID=local-zebra-1 ./scripts/run-zebra-e2e.sh

To repeat the proven ZEC claim happy path alone:

```sh
cargo build --locked -p lez-zec-swap-sdk -p lez-swap-store
cargo test --locked -p lez-zec-swap-sdk --test sdk_lifecycle \
  independent_actors_complete_lez_then_zcash_claims_in_both_directions \
  -- --exact --nocapture
cargo test --locked -p lez-swap-store --test zec_sdk_recovery \
  schema_v9_claim_journal_completes_and_reopens_independent_actors_in_both_directions \
  -- --exact --nocapture
```

The second test creates different temporary SQLite files for maker and taker.
Each file is opened and reopened with the same external key ID and key material
for that run; the key itself is never written to either database. The expected
terminal evidence is LEZ reveal, Zcash follow-up, and both role-local journals
replaying revision 4 as `Completed` via `resume_claim_capable`.

The provisional LEZ v0.2 command compiles exact SPEL PR #238 head
`df17acd98436be4f09c55877dae1fe2e73cbcdca` against official LEZ `v0.2.0`
at `a58fbce2ff48c58b7bb5001b1a27e64b9596ee3a`. It uses two Cargo jobs and
separate run-local root, guest, artifact, tool, and Docker-source paths derived
from the lowercase `RUN_ID`. It builds the deployment ELF with a digest-pinned
official Risc0 guest-builder image, but starts no sequencer, listener, or fixed
port. A cold run needs Docker plus crates.io/GitHub and circuit/image
distribution access, `unzip`, and working libclang C headers, and compiles the
large official graph; do not overlap it with another Docker-heavy or native
build on the same host.

The lane now proves the v0.2 standalone config and `LeeTransaction` API compile,
locks one tag-based `lee_core` identity to the exact LEZ commit, and matches
SPEL public PDA bytes to LEZ's fixed `/LEE/` vector. It also builds the Risc0
escrow guest and generated client, binds exact ELF SHA-256/ImageID/ProgramId,
executes recursive native and two-definition token claim/refund lifecycles, and
proves full rollback when a child transfer fails. Its exact-once official-RPC
deployer accepts evidence only after immutable endpoint/channel/built-in,
genesis, transaction-byte, transaction, block, and artifact checks. Before
printing retained public evidence, `deploy` authenticates it with a separate
owner-only 32-byte HMAC-SHA256 key. Its offline `provision-identity` command
requires that same zeroized key, verifies the authentication tag and bounded
evidence, then atomically writes a no-clobber public runtime identity in a
non-shared-writable directory containing the exact
chain/channel/genesis/program/deployment fields consumed by signed
provisioning. Public-testnet
deployment and deployed-runtime costs are deferred under ADR 0023. The
public-compatible local v0.2 node corridor and independent actors are GREEN in
both directions. Dormant schema-v3 Zebra routes, the public HTTPS transport,
and the LEZ `official_public` profile are locally GREEN; live deployment,
credentials, funds, method smoke, and public transactions remain deferred.
PR #238 remains unmerged and unreviewed. That status is a production-release
blocker under ADR 0018, not a private M2 blocker. The final private M2
repository certification gate is GREEN and bound by `m2-complete`; the public
and production gates remain explicitly deferred.

Cargo-deny reports that the exact official LEZ graph forces Hickory DNS
`0.25.0-alpha.5` (`RUSTSEC-2026-0118` and `RUSTSEC-2026-0119`) through
Logos-owned common/libp2p dependencies. Graph-local policy permits only those
exact advisories; tests bind the pins, exclude the generated wallet graph, keep
the sequencer future unpolled, and reject DNSSEC features. Under ADR 0018 this
disclosed upstream exception does not block private local M2 certification, but
it remains a production-release blocker pending an upstream fix or explicit security
acceptance.

`npm run test:mermaid` scans every tracked Markdown Mermaid block, rejects
GitHub-host-sensitive configuration, beta/new-shape, and interactive syntax,
then renders every diagram with the exact Mermaid CLI 11.16.0 pin. GitHub's
live Viewscreen renderer also reported 11.16.0 on 2026-07-12; the exact asset
and SHA-256 are recorded in
[`docs/evidence/github-mermaid-renderer.json`](docs/evidence/github-mermaid-renderer.json).
GitHub controls that renderer, so the repository deliberately retains a
conservative syntax subset and requires a visual check after documentation is
pushed.

On a hardened Linux host where Chromium cannot create its own user namespace,
keep the browser download isolated and opt into the repository's no-sandbox
Puppeteer profile only inside an already isolated test account/container:

```sh
PUPPETEER_CACHE_DIR=/tmp/lez-mermaid-browser \
  npx puppeteer browsers install chrome-headless-shell
PUPPETEER_CACHE_DIR=/tmp/lez-mermaid-browser \
  MERMAID_ALLOW_NO_SANDBOX=1 npm run test:mermaid
```

Do not set `MERMAID_ALLOW_NO_SANDBOX=1` for general web browsing or an
untrusted checkout. CI uses its own ephemeral runner and the default command
whenever the runner's Chromium sandbox is available.

The Zebra suite uses a unique `lez-atomic-swaps-${RUN_ID}` Compose project. It
copies the binary from the digest-pinned official Zebra 5.2.0 image into a
digest-pinned distroless nonroot runtime, then runs two disconnected nodes on a
project-only network with read-only filesystems, independent tmpfs state,
resource caps, no Linux capabilities, and separate ephemeral localhost RPC
ports. Before Compose starts it allocates an absolute run-scoped maker SQLite
database and refuses any pre-existing manifest, database, WAL, or SHM. The suite
first proves real canonical funding, close/reopen/requery, deeper-fork removal,
second restart, and exact replay through the maker runtime; it then runs the
actor fund/claim/refund/concurrent-fork consensus fixture. Cleanup addresses that
exact project and never prunes or stops resources it did not create.

## Licensing

Licensed under either the Apache License, Version 2.0 or the MIT License, at
your option.
