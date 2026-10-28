# M2 on the Docker devnet: ZEC handover

Branch `feat/m2-zec-on-docker`. This is the engineering brief for finishing
the Zcash leg on the existing Docker stack, written by the reviewing session
for the session that changes code. Everything below was read from the tree
at `84334be3`; file references are exact.

## Goal

Full M2 readiness on the local devnet: every BTC scenario and desk journey
green, and ZEC scenarios + desk journeys that exist and pass. "Pass" means a
ZEC↔LEZ swap settles on the running zebra and LEZ chains through the two
Nodes, with claim and refund covered.

## Ground rules

- Work in `~/lez-m2` on this branch. Commit locally per step (see "Commit
  protocol"). Pushing is allowed. **Nothing private in a commit**: no keys,
  cookies, capability files, `runtime/`, `market/`, evidence with secrets.
  Fixtures that must move between machines go by `scp`, never git.
- tmux sessions `fast` (BTC fast-profile refund suite) and `recover` may be
  running. **Do not kill them, do not run `scripts/up.sh --fresh-lez`, and do
  not restart `lez-maker-node` / `lez-taker-node` while `/tmp/fast.log` has
  no `DONE` or `STALLED` line.** A chain recreation mid-suite throws the
  suite away. Other tenants' containers (`gasstorm-*`, `miden-*`) may be
  stopped if resources are short.
- The harnesses here were written for a human at a terminal. Every failure
  mode they have is a silent hang, not an error: `docker exec -i` with a
  stdin nobody closes, `docker compose run` without `-T`. If a run goes
  quiet, `ps -eo stat,args` and look for state `T` before reading logs.
  `timeout` does not rescue a suspended process; use `timeout -k 30`.
- The chain stalls (upstream logos-blockchain#3643). Signature: finalized
  height frozen (`getLastFinalizedBlockId` on `127.0.0.1:8779`) while the
  sequencer logs new blocks, and every desk shows `recovering (Lock window
  missed)` ~30 min later. A chain is good for roughly two hours of suite
  load. `scripts/ensure-chain.sh` samples it; a recreation can itself come
  up wedged, so judge a fresh chain by movement over ten minutes, not by
  `up.sh` returning 0 (`/tmp/chain-recover.sh` does this).

## What is already true

- BTC node-e2e: 11/11 local-profile scenarios pass (`TakerSellsForeign`).
  Fast profile (`taker-refund`, `maker-refund`, `refund-then-maker-lock`)
  is running in tmux `fast`.
- BTC desks: happy, restart-taker, restart-maker, survivor pass;
  `concurrent` failed only because the chain stalled mid-journey; it and
  `node-offline` are still owed on a fresh local-profile chain.
- Both deployed Node images are built with `pair-zec`: `lez-maker-node
  --help` lists the four `--zec-*` flags, `lez-taker-cli` has
  `--accept-zec-offer`. `lez-zec-maker-actor` / `lez-zec-taker-actor` are in
  `/usr/local/bin` of their containers. `lez-zebra` is healthy on the same
  Docker network (`chains`), regtest NU6.2, with 104+ mature coinbase
  outputs of 6.25 ZEC paid to the miner address whose key is `[0x04; 32]`
  (`deploy/assets/zebra/zebrad.toml`, identical to `tests/e2e/zebra/`).
- `deploy/profiles/local-btc-demo-v1.json` now lists `zebra`/`zebra-init`
  (`f09b8301`), which was the one CI failure that was ours.
- `84334be3` removed the provisioner's stale pin on the v0.2.0 transfer
  program; `07cb6d51` stages the three provisioner binaries into the Taker
  image (rebuild needed: `scripts/from-scratch.sh --only rust` then
  `--only build:taker-node`, or the image rebuild path `up.sh` takes).

## ZEC architecture, as the code has it

Read these before writing anything; they decide the design.

1. **A ZEC swap is prepared, not composed.** The Taker Node takes a ZEC
   offer only from a static `prepared_zec` catalog entry in its role config
   (`crates/taker-node/src/taker_service_config/prepared.rs:323-350`):
   `source_id, swap_id, offer_id, reservation_id, foreign_units, lez_units,
   signed_envelope{path,sha256}, unsigned_draft{path,sha256},
   signing_key{path}, source_config{path,sha256}, agreement_output,
   actor_root, receipt_output`. Canonical fixture:
   `crates/taker-node/tests/taker_initiation_config.rs:263-330`; the full
   Node-level flow with a live Maker daemon:
   `crates/maker-node/tests/zec_chat_process.rs:199-330`.
   ADR 0213 take-time preparation is Bitcoin-only
   (`taker_service/lifecycle.rs:1452`).
2. **The take path is wired.** `taker_swap_initiate_v1` →
   `execute_prepared_zec` (`lifecycle.rs:2062`) →
   `take_zec_with_authenticated_offer_and_actor_config`
   (`crates/taker-node/src/zec_taker_accept.rs`) drives
   `zec_chat_propose_v1` / `zec_chat_complete_v1` on the Maker's chat socket,
   validates the draft against the live offer, provisions and activates the
   Taker actor under `actor_root`, writes the receipt. The Maker half is
   `crates/maker-node/src/zec_chat.rs`.
3. **Direction.** Node-driven ZEC is proven for `TakerSellsLez` only (M6
   certificates `docs/evidence/m6-zec-service-{claim-regression,refund}-
   certificate-20260804.json`: LEZ first lock → Maker funds Zcash → Maker
   claims LEZ revealing the secret → Taker claims Zcash). The Taker Node's
   ZEC state map (`lifecycle.rs`, `zec_actor_progress`) offers the Taker only
   `claim` (`ClaimEvidenceAvailable` + `ClaimZcash`) and `refund`; in
   `TakerSellsForeign` the Taker's LEZ claim maps to `attention_required`.
   Build `TakerSellsLez` first; it is the direction the Maker's own
   supervisor drives (`actor_supervisor/runtime.rs:135,934-1011`:
   `zcash_fund`, `claim_lez`, `refund_zcash`, `zcash_followup_claim`).
4. **The Taker's first LEZ lock has no Node RPC for ZEC.**
   `taker_swap_lock_v1` is BTC-dynamic only (`lifecycle.rs`, `lock_swap`:
   `prepared.execution().dynamic()` + `dynamic_btc()`); the Node's
   `observe()`/`activate()` are no-ops for `Self::Zec`. The M6 runs drove the
   Taker actor directly. So the harness drives it:
   `lez-zec-taker-actor --config <actor_root>/…/actor-config.json drive`
   (`ActorCommand`: `activate|drive|claim|recover|status`,
   `crates/zec-reference-actor/src/lib.rs:43`). The actor's own
   `ZecLifecycleAction::CreateAndFundLez` is that first lock.
5. **The ZEC actors expect a running role-local LEZ sidecar** at
   `bridge.endpoint` (loopback only) with a bearer capability file, and
   **nothing in either Node launches one for ZEC** (grep
   `maker-node/src/zec_chat.rs`, `actor_supervisor/`, `taker-node/src/zec_taker_accept.rs`:
   no spawn). The BTC lifecycle's recipe is the one to copy,
   `crates/btc-role-lifecycle/src/sidecar.rs:150-178`:
   ```
   lez-v02-bridge-poc --listen-address 127.0.0.1:<port> --node-profile local
     --sequencer-url http://127.0.0.1:3040 --indexer-url http://127.0.0.1:8779
     --run-id <run_id> --runtime-file <RuntimeDescriptor json>
     --capability-file <64 hex chars + newline> --private-key-file <lez-signer.key>
     --state-directory <new 0700 dir>
     --authenticated-transfer-program-id <64 hex>
   ```
   The runtime file is the same `{sidecar_role, compatibility:"lee_v0_2_0",
   chain_id, channel_id, genesis_block_hash, escrow_program_id,
   signer_account_id}` the entrypoint already writes to
   `$btc_state/runtime.json`; the provisioner writes its own per role
   (`lez-runtime.json`) plus `sidecar.capability` and a private copy of
   `lez-signer.key`, which is exactly the sidecar's input set.
6. **Zebra must also be reachable on loopback inside the Node containers**
   (`deterministic_local` route = `LoopbackHttpEndpoint`,
   `crates/zec-reference-actor/src/config.rs`). The entrypoint already
   forwards 18443/3040/8779 with `socat` (`node-entrypoint.sh:33-35`);
   add `18232 → zebra:18232`.
7. **The corridor generator exists.** `zec-local-poc-provision
   --spec-file <json> --output-root <new abs dir>
   --maker-lez-signer-key-file <key> --taker-lez-signer-key-file <key>`
   (`crates/zec-reference-actor/src/bin/zec-local-poc-provision.rs`,
   `src/local_poc.rs:287`). Spec (schema 1, `deny_unknown_fields`):
   ```json
   {"schema_version":1,"run_id":"<8-64 [A-Za-z0-9._-]>","swap_id":"<string>",
    "direction":"taker_sells_lez",
    "lez_runtime":{"chain_id":"<hex32>","channel_id":"<hex32>",
      "genesis_block_hash":"<hex32>","escrow_program_id":"<hex32>",
      "authenticated_transfer_program_id_base58":"<b58 of fe96c4…>",
      "maker_signer_account_id_base58":"<identity.json account_id>",
      "taker_signer_account_id_base58":"<…>"},
    "bridge":{"maker_endpoint":"http://127.0.0.1:19101",
              "taker_endpoint":"http://127.0.0.1:19102"},
    "zebra_endpoint":"http://127.0.0.1:18232",
    "lez_discovery_start_height":1,"lez_discovery_max_blocks":256}
   ```
   Guards that matter (`local_poc.rs:953-1010`): `chain_id == channel_id`
   ("upstream exposes no chain-ID RPC"), zebra must be regtest NU6.2, the
   two signer keys must differ and match the account ids, and it selects a
   **mature coinbase output owned by key `[0x04;32]`** — do not mine to any
   other address. Output tree (`local_poc.rs:336-347, 775-783`):
   `shared/agreement-v2.borsh`, `<role>/actor-config.json`,
   `<role>/lez-runtime.json`, `<role>/claim-recovery.key`,
   `<role>/claim-preimage.key` (funder only), `<role>/zcash.key`,
   `<role>/lez-signer.key`, `<role>/sidecar.capability`,
   `<role>/state/{actor,bridge}.sqlite3`. **All paths inside
   `actor-config.json` are absolute under `--output-root`**, so provision
   with the root the containers will see.
   Then `zec-local-poc-chat-draft --source-agreement-file shared/agreement-v2.borsh
   --now-unix-seconds <now> --reservation-id <id> --offer-commitment <hex32>
   --offer-expires-at-unix-seconds <n> --output-file <unsigned draft>` rebinds
   the template to one live offer. `reservation_id` is Taker-chosen
   (`RequestId`, same 8-64 charset); the Maker derives the chat session from
   it (`maker_zec_chat_session_id`). `offer_commitment` is the
   `signed_envelope_sha256` the Taker's `taker_offer_list_v1` returns for
   the offer (a 32-byte array in JSON; hex-encode it);
   `offer_expires_at_unix_seconds` is the offer's expiry from the same entry.
   The Taker validates exactly those bindings
   (`zec_taker_accept.rs:540-549`): taker key, maker key, `foreign_units ==
   zcash_amount_zatoshis`, direction, `lez_units`, session id, commitment,
   expiry.
   Where the devnet facts live: `deploy/runtime/lez/<role>/identity.json`
   (`account_id` is the base58 the spec wants), `/var/lib/lez/<role>/btc/btc-role.json`
   in the containers (channel, genesis, escrow, transfer program), and the
   market manifest the entrypoint reads (`/run-market/market-bootstrap.env`,
   keys `M3_POC_LEZ_ESCROW_PROGRAM_ID`, `M3_POC_LEZ_GENESIS_BLOCK_HASH`).
   **Every one of these changes on `--fresh-lez`**; a catalog is pinned to
   the chain it was provisioned on.
8. **Maker side.** The daemon takes the authority group together
   (`crates/maker-node/src/bin/lez-maker-node.rs:240-284`):
   `--maker-claim-key-id <any id> --maker-claim-key-file <32 raw bytes>
   [--maker-claim-preimage-file <32 raw bytes>] --zec-source-maker-config
   <provisioned maker/actor-config.json> --zec-maker-actor-root <abs,
   canonical, 0700, existing> --zec-actor-program /usr/local/bin/lez-zec-maker-actor
   --zec-actor-program-sha256 <sha256 of that file>`. Reference launch:
   `crates/maker-node/tests/zec_chat_process.rs:2225-2250`. The preimage
   file is the Zcash funder's; in `TakerSellsLez` that is the Maker, so pass
   the provisioner's `maker/claim-preimage.key`. The claim key is
   `maker/claim-recovery.key`.
9. **Offers.** The Maker publishes a ZEC offer through the same owner RPCs
   the BTC harness uses (`deploy/scripts/node-e2e.py:153-171`):
   `maker_local_route_save_v1` with `route {pair:"Zcash",
   direction:"TakerSellsLez"}`, then `maker_offer_publish`. Units: the
   provisioner's template is `ZCASH_PRINCIPAL_ZATOSHIS = 100_000_000`
   (1 ZEC) against `LEZ_NATIVE_AMOUNT = 50_000` (`local_poc.rs:62-63`); the
   route's price must quote exactly those (`foreign_units_per_lot:
   100000000, lez_units_per_lot: 50000`, min = max = 100000000), and the
   catalog entry's `foreign_units`/`lez_units` must equal them.
10. **Desks.** The Taker's "Authenticated offers" form goes through Logos
    Chat (`apps/basecamp/taker/src/lez_atomic_swap_taker_backend.cpp:136-170`,
    `chat_->selectOffer`), and the devnet runs no Chat — that path is dead
    here. The order-book path is pair-agnostic: `takerTake`
    (`apps/basecamp/common/node_market.cpp:497-530`) sends whatever `route`
    the offer carries. What blocks ZEC in the desks is hard filtering and
    labelling on `"Bitcoin"` in `node_market.cpp` lines 45 (publish route
    constant), 132-135 and 195-196 (chain labels and action names), 292
    (explorer URL), 438, 450, 580, 629, 644, 701 (row filters), plus the
    QML pair combos (`apps/basecamp/taker/src/qml/Main.qml:763`,
    `model: ["Bitcoin"]`). The desk's action names are composed from the
    direction (`claim_btc`/`refund_lez`…) and dispatched by prefix
    (`node_market.cpp`, `takerAction`: `claim_*` → `taker_swap_claim_v1`,
    `refund_*` → `taker_swap_refund_v1`), so a `claim_zec`/`refund_zec` name
    works unchanged. Packages are Nix-built on the box
    (`scripts/from-scratch.sh --only nix`, store volume `lez-nix-store-arm`
    is present; then `--only build:basecamp-ui`).

## Design

### Volumes and paths

- One new named volume per role, `zec_maker` / `zec_taker`, mounted at
  `/var/lib/lez/zec` in the matching Node container, `0700 lez:lez` (init
  the same way `maker-init`/`taker-init` do). The provisioner runs with
  `--output-root /var/lib/lez/zec/corridor`; `shared/` + `maker/` are
  copied into `zec_maker`, `shared/` + `taker/` into `zec_taker`. Absolute
  paths stay canonical and the other role's secrets never enter a
  container. Also under the volume: `sidecar/` (sidecar state, new per
  start), `actors/` (Maker actor root), `taker-actors/` (Taker actor roots,
  one new dir per prepared swap), `prepared.json` (Taker catalog).
- The provisioner itself runs out of band from the host with the Taker
  image: `docker run --rm -v <tmp>:/work -v <host runtime/lez>:/identities:ro
  --entrypoint zec-local-poc-provision lez-taker-node:local …` where `/work`
  is later copied into the two volumes with `docker cp` / a helper
  container, and the spec is rendered by the script.

### Entrypoint (`deploy/images/{maker,taker}-node/node-entrypoint.sh`, identical files)

- Add `socat TCP-LISTEN:18232,bind=127.0.0.1,fork,reuseaddr TCP:${zebra_host:-zebra}:18232 &`.
- When `/var/lib/lez/zec/<role>/actor-config.json` exists: create
  `/var/lib/lez/zec/sidecar-<stamp>` (0700) and start the role sidecar in
  the background on `127.0.0.1:19101` (maker) / `19102` (taker) with the
  recipe in fact 5, `--run-id` taken from `jq -r .run_id` of the actor
  config, the three files from the provisioner's role dir, and logs to
  `/var/lib/lez/zec/sidecar.log`. Wait until the port answers before
  starting the Node.
- Maker: when the material exists, append the flags in fact 8 to `args`
  (`--maker-claim-key-id devnet-zec-claim-v1`, program sha from
  `sha256sum /usr/local/bin/lez-zec-maker-actor`, actor root
  `/var/lib/lez/zec/actors` created 0700 first).
- Taker: when `/var/lib/lez/zec/prepared.json` exists, render `role.json`
  with `initiation.prepared_zec` = that array (keep
  `execute_prepared:true`, `btc_role_config`). `jq --slurpfile`.

### `deploy/scripts/zec-provision.sh` (new)

Host-side, idempotent per chain. Steps, each verified before the next:

1. Preconditions: stack up, `ensure-chain.sh` says `ok`, zebra height ≥ 101
   (`getblockchaininfo` on `127.0.0.1:18232`); if not, `generate` to the
   miner address until it is — the miner address is the one in
   `deploy/assets/zebra/zebrad.toml`, key `[0x04;32]`, never another.
2. Render the spec from `runtime/lez/{maker,taker}/identity.json` and the
   facts in the Maker's `btc-role.json` (read with `docker exec`), base58
   of the transfer program computed in Python (alphabet
   `123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz`).
   `run_id = zec-devnet-<unix>`, `swap_id = zec-devnet-swap-<unix>`.
3. Provision (fact 7). Keep the provisioner's JSON summary; it is
   secret-free and is the evidence of what was built.
4. Distribute into the two volumes; `chmod -R go-rwx`.
5. Publish the ZEC offer (fact 9) via the Maker owner socket; discover it
   via `taker_offer_list_v1` on the Taker; hex-encode the commitment.
6. Chat draft (fact 7) with a fresh `reservation_id`
   (`zec-devnet-res-<unix>`), output `taker/unsigned-draft.borsh` in the
   Taker volume.
7. Write `prepared.json`: one entry, `source_id:"local-maker"` (the
   Taker's `delivery_sources[0].source_id` in the entrypoint), `swap_id`,
   `offer_id`, `reservation_id`, `foreign_units:100000000`,
   `lez_units:50000`, `signed_envelope` = the Delivery file the Maker wrote
   for that offer, copied into the Taker volume with its sha256 (check
   what `RunLocalDelivery` writes to `/delivery/<offer_id>.offer.json`
   against `authenticated.signed_envelope()` — the test writes exactly
   those bytes), `unsigned_draft` {path, sha256}, `signing_key` =
   `taker/zcash.key` (the test signs the agreement with the same key byte
   it uses for Zcash; confirm in `zec_taker_accept.rs` that
   `taker_signing_key_file` is the secp256k1 key whose pubkey is
   `draft.taker_zcash_key()`), `source_config` = `taker/actor-config.json`
   {path, sha256}, `agreement_output`, `actor_root` (new dir), `receipt_output`.
8. Restart both Nodes (`docker compose restart maker-node taker-node`) so
   the entrypoints pick up the material; check `maker_health` and that
   `lez-maker-node` did not exit on a flag error; check both sidecars
   answer.

Re-running must be safe: a second run on the same chain either reuses the
provisioned corridor or provisions a new swap id — the catalog may hold
several entries (bounded at 256), but one `source_id`+`offer_id` each.

### Harness

- `deploy/scripts/node-e2e.py`: a `--pair Zcash` switch that sets `ROUTE =
  {"pair":"Zcash","direction":"TakerSellsLez"}`, `FOREIGN_UNITS =
  100_000_000`, `LEZ_UNITS = 50_000`, and the ZEC scenarios:
  - `zec-happy`: publish (or reuse the provisioned offer), take, then drive
    the Taker's first LEZ lock with `lez-zec-taker-actor … drive` inside
    `lez-taker-node` (the actor config path is in the receipt the Node wrote
    at `receipt_output`), wait `awaiting_second_lock` → Maker funds Zcash
    (its supervisor) → `claim_available` on the Taker → `taker_swap_claim_v1`
    → `completed` on both. Mine zebra blocks as needed (`generate` to the
    miner address) and assert the Zcash claim tx is in a block.
  - `zec-taker-refund`, `zec-maker-refund` (fast profile): the ZEC refund
    plan is in the agreement (`ZecRefundPlanV1`), so the refund windows come
    from the provisioner's constants; read them from the provisioned
    agreement rather than guessing. Stop the Maker before it funds for the
    Taker refund; let the Taker lock and stop it for the Maker refund.
  - The take for a static catalog entry needs no `funding_transaction_hex`
    (`-32018` is BTC-only); keep the `-32010` retry loop.
- `deploy/scripts/ui-e2e.sh` + `deploy/ui-tests/verify.mjs`: `zec-happy`
  journey through the order-book path once the desks admit Zcash rows
  (below). Restore the deleted journey as a starting point:
  `git show 075d9202^:deploy/ui-tests/verify.mjs` lines 577-650, but
  replace the Chat-based "Browse authenticated offers" with the order-book
  take the BTC journeys use (`PREPARE_INTERACTIVE_BTC` flow in
  `ui-e2e.sh:164-173`).
- `scripts/ensure-chain.sh` before every scenario, `reset-swaps.sh` before
  every scenario (it already resets every pair, `c5060252`), `timeout -k 30`.

### Desks (last)

- `node_market.cpp`: replace the `== "Bitcoin"` filters with "pair is
  Bitcoin or Zcash"; derive chain labels, unit formatting (`formatBtc` →
  a per-pair formatter: 8 decimals + "ZEC" for zatoshis) and action names
  from `route.pair`; leave line 45 (desk-published offers stay Bitcoin).
- QML: `takerPair` model `["Bitcoin","Zcash"]`; the privacy label for a
  Zcash row: "After a transparent ZEC claim, move funds to a shielded
  wallet address." (the Node reports
  `ShieldReceivedTransparentZecSeparately`).
- Rebuild: `from-scratch.sh --only nix` (rebuilds the role packages when
  `apps/basecamp` changed), then `--only build:basecamp-ui`, then
  `docker compose up -d basecamp-ui`.

## Build order and acceptance

| # | Step | Done when |
|---|---|---|
| 1 | Rebuild the Taker image with the provisioners (`--only rust`, `--only build:taker-node`) | `docker run --rm --entrypoint zec-local-poc-provision lez-taker-node:local --help` prints usage |
| 2 | Volumes + entrypoint (socat 18232, sidecars, Maker flags, Taker catalog) | Both Nodes start with no material present exactly as before (no regression on BTC); with material present `lez-maker-node` stays up and `ss -ltn` shows 19101/19102 |
| 3 | `zec-provision.sh` | Ends with `maker_health` ready, catalog loaded (`taker_offer_list_v1` lists the Zcash offer; `lez-taker-cli … --help` is not enough — check the Node log says it loaded one prepared ZEC entry) |
| 4 | `node-e2e.py zec-happy` | Both Nodes report `completed`; zebra claim tx id printed and found in a block; LEZ claim finalized |
| 5 | `zec-taker-refund`, `zec-maker-refund` (fast) | `refunded` on both Nodes, chain evidence printed |
| 6 | Desk filter + QML + nix rebuild | 4/4 Basecamp smoke suites still pass; a Zcash row appears on the Taker desk |
| 7 | `ui-e2e.sh zec-happy` | Journey passes, evidence exported |
| 8 | BTC leftovers on a fresh local-profile chain: `ui/concurrent`, `ui/node-offline` | Both pass |

Keep `deploy/scripts/check-runtime-profiles.py`'s contract: any new Compose
service or volume that is a component must be in the profile; new volumes
alone are fine.

## Commit protocol

- One commit per step above, message in the house style (what/why, the
  failure it fixes, no "fix stuff"), ending with the Co-Authored-By line
  the session is given.
- Before every push: `git diff --cached | grep -iE "BEGIN (RSA|EC|OPENSSH)|cookie|capability|[0-9a-f]{64}"` and eyeball every hit; the only
  64-hex values allowed are chain ids and program ids already in the tree.
- After each step, append to `~/m2-work/M2-ON-DOCKER.md` (not committed) what
  was run and what it printed, and write one line to the tmux pane so the
  owner can read progress from the iPad.
- The reviewing session pulls each commit, reads the diff, and answers in
  `~/m2-work/REVIEW.md` on the box. Blocking findings are prefixed `BLOCK:`;
  address them before the next step.

## Things that will bite

- `docker compose run` needs `-T`; `docker exec` must not have `-i` unless
  stdin is really piped. Python: `stdin=subprocess.DEVNULL`.
- `reset-swaps.sh` feeds heredocs to `python3 -`: `-T` yes, `</dev/null` no.
- The catalog dies with the chain. After any `--fresh-lez`, re-run
  `zec-provision.sh` before any ZEC scenario.
- The Taker's `actor_root` / `agreement_output` / `receipt_output` must not
  exist before the take (`resolved_new_path`); one prepared entry is one
  take. A replay of the same `request_id` is served from the durable record.
- `lez-maker-node` refuses a duplicate authority (`duplicate swap or state
  identity`) if the same source config is passed twice.
- Zcash coinbase maturity is 100 confirmations; the provisioner selects a
  mature output and fails closed if there is none.
- `check-runtime-profiles.py` needs `cargo` on the host; run the drift part
  with the Python snippet in `f09b8301`'s message if cargo is absent.

## Part B — acceptance, in M3's shape

Added 2026-10-28 09:00Z. M3 was accepted on three things: recordings per
chain, a named SDK with a stage → API → test map plus a runnable offline
example, and a node guide that states routes, wallet creation, funding and
the direction each route serves. `docs/m2-acceptance-checklist.md` maps
rfp#122's six deliverables to artefacts, proofs and today's state. These
steps follow 1–8 and are reviewed the same way, one commit each.

| # | Step | Done when |
|---|---|---|
| 9 | ZEC scenario matrix in `node-e2e.py --pair Zcash`: beyond `zec-happy` and the two refunds, `replay`, `wrong-inputs`, `restart-taker`, `restart-maker`, `survivor`, `concurrent` (two prepared entries), `regenerated-catalog`, `tampered-catalog` — each the ZEC analogue of the BTC scenario of the same name, skipping only what the catalog model makes meaningless (say which, and why, in the scenario's docstring) | All pass on a fresh chain; `RESULTS.txt` lines in the M3 format |
| 10 | Desk ZEC journeys: `zec-happy`, `zec-concurrent`, one refund; all three recorded with `--record`; the Taker row shows the shield-after-swap label | Three MP4s under `runtime/evidence/videos/`, each from a passing run |
| 11 | `docs/sdk-lez-zec.md` (the five stages → `lez-zec-swap-sdk` public API → the `sdk_lifecycle.rs` test that drives each; the port / bridge-adapter boundary stated as in `sdk-lez-btc.md`), `crates/zec-swap-sdk/README.md`, `crates/zec-swap-sdk/examples/full-lifecycle.rs` offline against the in-memory store, printing each stage; the privacy-posture section | `cargo run -p lez-zec-swap-sdk --example full-lifecycle` reaches `Completed` and a second swap reaches `Refunded`; `cargo doc` warning-free; clippy pedantic clean on the example |
| 12 | CI: bring the Zebra regtest gate and the un-parked ZEC tests over from PR #116 (`8be5d829`, `2bf2e8f6`, `999bddfe`; check whether `25877a27` and the three `deps:` advisory commits are needed for green) | `ci` green on the pushed commit, Zebra job included |
| 13 | `docs/zcash-testnet-setup.md` checked against the five U8 topics; direction per route stated; traceability rows F4, U1, U6, U10, S1, S5, S8, D1 edited in place; `submission/MILESTONES.md` gains the M2 section; `m6-zec-reconciliation.md` updated | `scripts/check-requirements-traceability.sh` passes; every link resolves |
| 14 | Evidence set `docs/evidence/regression-m2-<date>/` with `PROVENANCE.md` (commit, image digests, chain recreations, corridor run ids per scenario) and `RESULTS.txt`; per-scenario Node evidence exported | Set committed; nothing private in it (run the scan) |
| 15 | Release tag with notes and the three MP4s; draft of the rfp#122 comment in `~/m2-work/RFP-122-COMMENT.md` for the owner (not posted) | Owner reviews |

Ground rules unchanged. The sequencing note on rfp#122 (#115 modules before
M2) is the owner's to address in the comment; do not write around it.
