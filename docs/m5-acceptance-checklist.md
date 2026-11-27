# M5 acceptance checklist — Maker daemon, CLIs, swap coordinator, fuzz harness

M5 is [rfp#125](https://github.com/logos-co/rfp/issues/125), "RFP-003 M5 —
Maker daemon, CLIs, swap coordinator, fuzz harness", tracked here as issue
#162. It is version 0.5.0. This page maps each of its deliverables, each issue
fixed in this version and each RFP-003 requirement the milestone touches to
the file, test or devnet scenario that proves it, and says where the proof
stops.

It follows the shape of the [M4 checklist](m4-acceptance-checklist.md) and the
[M2 checklist](m2-acceptance-checklist.md): each artefact named, the test or
scenario that exercises it, a regression evidence set with provenance, and no
claim Logos has not made ("accepted", "audited", "mainnet-ready").

The milestone text names two requirement numbers that differ from RFP-003's
own. It asks for the price-source trait "per F9 as amended by PR #108"; PR #108
of `logos-co/rfp` changed Functionality 8, the price sources, and this page
files the price source under F8 and the headless Maker under F9. It asks for
the Delivery and Chat outage handling "per R7"; R7 in RFP-003 is the Bitcoin
refund construction and the outage requirement is R8, where this page files
it.

A devnet scenario is named by its `regress.sh` test id
(`deploy/scripts/regress.sh list full`), run by `deploy/scripts/node-e2e.py`
(`node:`) or `deploy/scripts/ui-e2e.sh` (`ui:`) on the Docker devnet with
Monero regtest up (`LEZ_MONERO=1`). The full tier held 180 tests at 0.4.0 and
holds 200 at 0.5.0: 62 `node:btc`, 70 `node:zec`, 23 `node:xmr`, 17 `ui:btc`,
24 `ui:zec` and 4 `ui:xmr`, 27 of them Monero. Twenty joined in this version:
`node:btc:claim-send-fails` in both directions,
`node:zec:zec-maker-claim-collision:TakerSellsLez`, `zec-early-refund` in both
directions as a Node test and as a desk journey (four tests),
`node:btc:maker-cli-refund:TakerSellsForeign`,
`node:zec:zec-maker-cli-refund:TakerSellsForeign`,
`node:zec:zec-maker-cli-claim:TakerSellsLez`,
`node:btc:taker-cli:TakerSellsLez`,
`node:zec:zec-taker-cli-refund:TakerSellsLez`,
`node:xmr:xmr-taker-cli:TakerSellsLez`,
`node:btc:outage-before-take:TakerSellsLez`,
`node:zec:zec-outage-before-take:TakerSellsForeign`,
`node:btc:maker-price-module-stale:TakerSellsForeign`, and
`node:zec:zec-kill-maker-refund` and `node:zec:zec-kill-taker-refund` in both
directions. The four tests that were `zec-regenerated-catalog` and
`zec-tampered-catalog` are now `zec-regenerated-config` and
`zec-tampered-config`. A release bundle leaves 12 of the 200 out
(`node-e2e.py release-unavailable`): 10 need the devnet test hooks, which no
release image carries, and 2 compile the repository's test price module.
`deploy/scripts/from-scratch.sh` builds the devnet on an x86_64 Linux host as
well as on arm64 (`deploy/tests/test_from_scratch_host.py`).

**The state of the regression.** The full regression of the tree at
`6ad741bf` has not run yet. No test named on this page has a recorded run at
that commit, and no evidence set for 0.5.0 is committed. What exists are
pre-runs of 2026-11-27 on an x86_64 Linux host, three lanes, on images built
from `6a5535db`, an earlier commit of this version that has the fix for #174
and the thirteen CLI, outage, price-module and refund-path kill tests, and
has neither the fixes for #175, #176, #178 and #182 nor the removal of the
prepared Zcash catalog. On those images the thirteen tests passed 13 of 13,
each on its first try; `claim-send-fails` passed 2 of 2, each on its first
try; a rehearsal of the tier was stopped after 21 tests, all 21 passed on
their first try; and `node:btc:maker-cli:TakerSellsForeign` failed once on a
defect of the test (it enabled its route before pricing it) and passed after
the test was fixed. These pre-runs are not evidence of `6ad741bf` and are not
committed. The crate tests named below were not run for this page.

## Where M5 stands

- **The daemon runs in both modes.** `lez-maker-node` (`crates/maker-node`)
  is hosted by Logos core through the `lez_swap_node` module, in a Basecamp
  with the Maker desk or headless under `logosctl`, and it runs as a hardened
  systemd unit. Every devnet test runs each Node the first way, inside its
  role's Basecamp. The systemd run on record is of 2026-11-12, from commits
  before ADR 0216; a rerun on the 0.5.0 tree has not run yet.
- **Both CLIs drive swaps of all three pairs on the stack.** The Maker's CLI
  configures, prices and publishes for Bitcoin, Zcash and Monero, runs a
  whole Bitcoin swap's Maker side, and asks for a manual refund (Bitcoin,
  Zcash) and a manual claim (Zcash). The Taker's CLI takes, monitors, claims
  and refunds Bitcoin and Zcash swaps and takes and claims a Monero swap.
- **The coordinator has three fixes from the review of v0.3.3.** A Bitcoin
  swap's LEZ claim that did not land is sent again (#174, ADR 0218); every
  LEZ transaction of the Maker's account passes one gate (#176, ADR 0219);
  an unfunded Zcash swap gives its capacity place up (#178, ADR 0220). What
  remains: a claim the node accepted and the sequencer dropped is not resent
  by the Bitcoin actor, a Bitcoin LEZ lock that loses its nonce is not
  planned again, and #178's place rule has no devnet scenario.
- **A Zcash swap is agreed at take time, and the catalog is gone.** The
  Taker's Node makes the swap secret and sends only its SHA-256, each Node
  signs with its own Zcash key, and only public keys cross. The preparation
  tools, the prepared catalog, its test fixture and the options that named a
  prepared swap are removed.
- **Private devnet only.** Bitcoin Core regtest, Zebra regtest, Monero
  regtest and the private LEZ v0.2.4 devnet. RISC Zero 3.0 is yanked
  (GHSA-g6qg-9j8p-7j87) and LEZ v0.2.4 pins it, so every graph's `cargo-deny`
  policy carries the exemption the [M4 checklist](m4-acceptance-checklist.md#where-m4-stands)
  describes (issue #184). No public-network release is made while it stands.
- **The price module boundary.** The external price source is a shared
  object that exports this repository's C ABI and is loaded by a one-shot
  worker process. No call goes through Logos core, and the only module
  exercised is the repository's test fixture.

## The deliverables

D-1 to D-7 are the seven deliverables of rfp#125 in its wording. D-8 is the
item issue #162 adds and the reviewer asked to count under the
swap-coordinator deliverable
([comment on rfp#125](https://github.com/logos-co/rfp/issues/125#issuecomment-7008071240)):
in v0.3.3 one operator prepared a Zcash swap, created the swap secret and
signed for both parties, so such a swap was safe only where the Taker trusted
whoever ran the set-up; the two Nodes are to prepare the swap at take time,
the Taker's Node creating the secret and sending only its hash, and each Node
creating its own Zcash key.

No scenario in the Proof column has a recorded run at `6ad741bf`; the column
says what each scenario asserts when it runs.

| # | Deliverable (rfp#125) | Artefact | Proof | Where it stops |
|---|---|---|---|---|
| D-1 | Maker daemon (Tokio + Rust) runnable under Logos core daemon mode (remote-controllable from the maker mini-app), with a standalone systemd-managed fallback until that Logos capability is delivered, plus installation docs (U2) | The daemon `lez-maker-node` ([`crates/maker-node`](../crates/maker-node/)). Logos core mode: the module [`modules/lez-swap-node`](../modules/lez-swap-node/) (`start_node`, `stop_node`, `node_status`, `node_stats`, `serve_signer`), the payload package [`modules/lez-maker-node`](../modules/lez-maker-node/) and the launcher [`modules/lez-role-node/node-entrypoint.sh`](../modules/lez-role-node/node-entrypoint.sh). The Maker desk [`apps/basecamp/maker`](../apps/basecamp/maker/). systemd: [`packaging/systemd/`](../packaging/systemd/), [`scripts/install-maker-node-service.sh`](../scripts/install-maker-node-service.sh), [`scripts/run-maker-node-systemd-vm.sh`](../scripts/run-maker-node-systemd-vm.sh). Installation: [maker-install.md](maker-install.md) (both modes) and [maker-systemd-install.md](maker-systemd-install.md) | Every devnet test runs each Node under Logos core inside its role's Basecamp, started by the desk through `lez_swap_node`. Headless: [`deploy/scripts/logos-native-acceptance.sh`](../deploy/scripts/logos-native-acceptance.sh) loads the module with the Logos command-line tools and calls `start_node`, `stop_node` and `node_status` ([logos-native-2026-11-11-c](evidence/logos-native-2026-11-11-c/); on macOS, swaps under one `logosctl` session, [darwin-native-node-2026-11-12-b](evidence/darwin-native-node-2026-11-12-b/); both before 0.4.0). systemd: both Nodes under their hardened units in an isolated Ubuntu VM executed a Bitcoin and a Zcash swap with the Maker killed and restarted mid-swap ([maker-systemd-2026-11-12](evidence/maker-systemd-2026-11-12/README.md)) | The systemd evidence is from commits before ADR 0216 and before 0.3.5; the VM run on the 0.5.0 tree has not run yet. The desk's view calls `health`, `btcMarket`, `btcPublishOffer`, `btcWithdrawOffer`, `btcSwapAction`, `chatStatus` and `resetChat`; history, monitor, claim and refund exist on its backend only and the operator uses the CLI for them; the desk starts the Node when it loads and has no control that stops it ([maker-install.md](maker-install.md#what-the-maker-desk-controls)). No Linux run of a swap under `logosctl` without a Basecamp. Monero under the Taker's systemd unit is not packaged |
| D-2 | Maker CLI per U3 (configure pairs/prices, start/stop, query history, manual claims/refunds) | `lez-maker-cli`, 21 commands, each in the [CLI reference](cli-reference.md#maker-u3-lez-maker-cli); `deploy/tests/test_cli_reference.py` fails when a command or flag is not named there. Crate tests [`operator_journey.rs`](../crates/maker-node/tests/operator_journey.rs) and [`maker_service_cli.rs`](../crates/maker-node/tests/maker_service_cli.rs) | `node:btc:maker-cli:TakerSellsForeign`: a happy swap with every Maker call made by the CLI (`configure-pair`, `set-local-price`, `publish-offer`, `offers`, `monitor`, `history`), `health` and `balances` as the owner, and 18 owner commands run as another uid refused at the 0600 socket. `node:btc:maker-cli-refund:TakerSellsForeign` and `node:zec:zec-maker-cli-refund:TakerSellsForeign`: the Maker's refund asked for with `lez-maker-cli refund`; `monitor` and `history` show the request completed. `node:zec:zec-maker-cli-claim:TakerSellsLez`: with the Maker's own claim held in its sidecar by a devnet test hook, `lez-maker-cli claim` is admitted, its repeat is a replay, a second request is refused with `-32009`, and the swap completes once. `configure-pair`, `set-local-price` and `publish-offer` for Zcash in `node:zec:zec-dynamic-happy` (both directions) and for Monero in every Monero scenario; `quote` and `withdraw-offer` in `node:btc:maker-price-module:TakerSellsForeign` | `start` and `stop` act on the systemd unit only ([`crates/node-common/src/service_control.rs`](../crates/node-common/src/service_control.rs)); under Logos core the Node is stopped and started with the hold file or `lez_swap_node`'s `stop_node` and `start_node`. `zec-maker-cli-claim` does not show a claim the Node would not have made by itself: the request decides when and under whose name the claim runs. The Maker has no Monero claim or refund to ask for (its Node runs them). As the owner, `alerts`, `acknowledge-alert`, `delivery-identity` and `create-swap` run in crate tests only; on the stack `alerts`, `acknowledge-alert` and `create-swap` appear only among the commands refused for another uid |
| D-3 | Taker CLI per U4 (offer discovery, swap initiation, progress monitoring, claim/refund) | `lez-taker-cli`: `swap offers`, `take`, `list`, `monitor`, `lock`, `claim`, `refund`, with `balances` and `health`, in the [CLI reference](cli-reference.md#taker-u4-lez-taker-cli). Crate tests [`taker_service_process.rs`](../crates/taker-node/tests/taker_service_process.rs) and [`taker_lifecycle_process.rs`](../crates/taker-node/tests/taker_lifecycle_process.rs) | `node:btc:taker-cli` in both directions: a happy swap with every Taker call made by the CLI, then `swap list`, `health` and `balances` as the owner and 9 owner commands refused for another uid. `node:btc:taker-cli-refund:TakerSellsForeign`, `node:zec:zec-taker-cli:TakerSellsLez`, `node:zec:zec-taker-cli-refund:TakerSellsLez` and `node:xmr:xmr-taker-cli:TakerSellsLez`: the named journey with the Taker driven by the CLI. `node:zec:zec-dynamic-happy` in both directions: the Maker's CLI publishes and the Taker's CLI takes | The flags `--accept-zec-offer` and those that went with it, a Zcash draft made ahead of the take, are removed. No scenario drives a Zcash swap selling ZEC through `zec-taker-cli`; that direction runs through the Taker CLI in `zec-dynamic-happy` only. The commands that do not use the Node (the in-process `monitor`, `claim` and `refund`) are not run against chains |
| D-4 | Swap-coordinator state machine: persistence, crash recovery, concurrent-swap isolation per R4 / R5 | [swap-coordinator.md](swap-coordinator.md): `lez_swap_core::SwapCoordinator`, `lez_swap_store::SqliteSwapStore` (schema version 22), the journals, recovery for each kind of step, isolation, each with its crate tests and scenarios. New in this version: ADRs [0218](architecture/0218-resend-an-exact-lez-claim-that-did-not-land.md), [0219](architecture/0219-one-gate-for-the-maker-accounts-lez-transactions.md) and [0220](architecture/0220-an-unfunded-zec-swap-gives-its-place-up.md) and the endpoint check of #175 ([the issues table](#the-issues-closed-in-this-version)) | New kill points: `node:zec:zec-kill-maker-refund` and `node:zec:zec-kill-taker-refund`, both directions, SIGKILL the role while both legs wait for the refund times and again once the Taker's refund is admitted; both legs still refund and no LEZ moves. Existing: `kill-maker`, `kill-taker`, `zec-kill-maker`, `zec-kill-taker`, `xmr-kill-maker` and `xmr-kill-taker` (7 points each), `restart-maker`, `restart-taker`, `survivor`, `zec-corrupt-state`, `regenerated-config` and `tampered-config` for Bitcoin and, on swaps agreed at take time, for Zcash, `concurrent`, `concurrent-kill-maker`, `zec-concurrent`, `zec-concurrent-kill-maker`, `mixed-pair`, `xmr-concurrent` and `mixed-three-pair`. The new scenarios of #174, #176 and #182 are in the issues table | No multi-user run: a lane has one Taker identity. No migration on the stack. No Bitcoin corruption scenario at the Node level. The adaptor nonce journal's key derives from the role's signing key. The limits of the three fixes are in the issues table |
| D-5 | Pluggable price-source trait per F9 as amended by PR #108, with two reference impls: local configuration, and an external-feed adapter sourcing prices from another Logos module via its C API | The trait `PriceSource` ([`crates/maker-node/src/price_source.rs`](../crates/maker-node/src/price_source.rs)); `LocalPriceSource`; `ProcessLogosPriceSource` ([`logos_price_source.rs`](../crates/maker-node/src/logos_price_source.rs)) over [`crates/logos-price-c-api`](../crates/logos-price-c-api/) (the header [`lez_logos_price_api.h`](../crates/logos-price-c-api/include/lez_logos_price_api.h), the worker `lez-logos-price-worker`); [price-module-guide.md](price-module-guide.md) | `node:btc:maker-price-module:TakerSellsForeign`: the Maker prices a Bitcoin route through the worker, publishes at the module's price under its SHA-256, a swap completes on that offer, and with the module answering unavailable `quote` and `publish-offer` fail closed with `-32003` while the offer already out stays active. `node:btc:maker-price-module-stale:TakerSellsForeign`: the module prices the Bitcoin route and Zcash `TakerSellsLez`; a quote 61 s old, past the Node's 30 s bound, makes `quote` and `publish-offer` fail closed with `-32603` on both routes, no offer is left by a refused publish, and the two offers already out stay active | The external source is a shared object that exports this repository's C ABI, loaded by an isolated one-shot worker, selected per route and pinned by SHA-256. No call goes through Logos core. The only module exercised is the repository's test fixture: no real Logos price module exists, so no real feed is exercised ([guide, section 10](price-module-guide.md#10-the-boundary-and-what-does-not-exist)). A release bundle does not run these two scenarios |
| D-6 | Graceful Logos Delivery / Chat unavailability handling per R7, behaviour documented | [delivery-chat-outage.md](delivery-chat-outage.md): the health states, what each role can do before a take, who retries, what recovers by itself, and the proof | Before the take: `node:btc:outage-before-take` and `node:zec:zec-outage-before-take` in both directions and `node:xmr:xmr-outage-before-take:TakerSellsLez`. Under the cut both Nodes report Delivery and Chat unavailable and are degraded, which `lez-maker-cli health` and `lez-taker-cli health` say too; the take is refused and leaves no swap; `lez-maker-cli publish-offer` answers `-32603` with the offer stored, and the offer reaches the Taker after the restore with no second publish; the same take request with the same request id then takes the offer and the swap completes. After the lock: `offline-after-lock`, `offline-after-second-lock`, `offline-after-claim` and `taker-refund-offline` for Bitcoin and Zcash, and `node:xmr:xmr-cut-after-lock:TakerSellsLez` | The cut is made on the shared Delivery directory and Chat socket of the devnet, not on the Logos modules of `deploy/compose.logos.yaml`. The Node does not buffer a take: the caller replays the same request id |
| D-7 | Fuzz harness against the swap-coordinator state machine using `cargo-fuzz` or equivalent | `cargo-fuzz` targets [`fuzz/fuzz_targets/coordinator.rs`](../fuzz/fuzz_targets/coordinator.rs) (the pure state machine) and [`coordinator_store.rs`](../fuzz/fuzz_targets/coordinator_store.rs) (two swaps through one `SqliteSwapStore`: save, reopen, a crash without a save, a stale save, revision-checked commits, with oracles for persistence, crash recovery and isolation), on the shared model [`coordinator_model.rs`](../fuzz/fuzz_targets/coordinator_model.rs); nine seeds each in `fuzz/corpus/coordinator` and `fuzz/corpus/coordinator_store`; [`scripts/run-fuzz.sh`](../scripts/run-fuzz.sh); the weekly workflow [`fuzz.yml`](../.github/workflows/fuzz.yml), six targets | Stable replays of every seed in the ordinary test run: `coordinator_fuzz_corpus_replays_on_stable` ([`crates/swap-core/tests/fuzz_corpus_replay.rs`](../crates/swap-core/tests/fuzz_corpus_replay.rs)) and `coordinator_store_fuzz_corpus_replays_on_stable` ([`crates/swap-store/tests/fuzz_corpus_replay.rs`](../crates/swap-store/tests/fuzz_corpus_replay.rs)). Runs of 2026-11-27 on x86_64, 2 jobs: `coordinator_store` 1,212 s, 34,104 executions, no crash, timeout or out-of-memory; `coordinator` 1,212 s, 1,348,701 executions, none ([the evidence set](#the-evidence-set) has every number). An earlier one-hour run of `coordinator`: [coordinator-fuzz-2026-11-12](evidence/coordinator-fuzz-2026-11-12/README.md) | The evidence set of the 2026-11-27 runs is not committed yet. They were made before the #174 journal change was in the store the `coordinator_store` target links. The model drives two swaps; it does not drive the actors, the sidecar or a chain |
| D-8 | Issue #162 and the reviewer's request on rfp#125: ZEC swaps are prepared by the two Nodes at take time, as BTC swaps are; the Taker's Node creates the swap secret and sends only its hash; each Node creates its own Zcash key | The rounds `zec_reserve_v1`, `zec_chat_propose_v1` and `zec_chat_complete_v1`. The Taker's Node makes the preimage at take time and sends only its SHA-256 ([`crates/taker-node/src/taker_service/zec_dynamic.rs`](../crates/taker-node/src/taker_service/zec_dynamic.rs), [`crates/zec-reference-actor/src/reserve.rs`](../crates/zec-reference-actor/src/reserve.rs)); the Maker composes its draft around that digest and holds no preimage ([`crates/maker-node/src/zec_reserve.rs`](../crates/maker-node/src/zec_reserve.rs)); each Node uses its own Zcash key and the Taker's launcher creates one when none is supplied. Removed: the Taker's `initiation.prepared_zec` (refused by name at start), the dealer tools and their Cargo feature, the catalog test fixture, the launcher's corridor wiring, the Maker's `--zec-source-maker-config` and the Taker CLI's `--accept-zec-offer`. A Chat round for a reservation the Maker does not hold is refused | Every Zcash test takes at take time. `node:zec:zec-dynamic-happy` (both directions) asserts on the stack that the Taker's reservation record carries the digest and not the preimage and that the Maker's swap directory holds no `claim-preimage.key`. `node:zec:zec-regenerated-config` and `node:zec:zec-tampered-config` (both directions) run the configuration scenarios on swaps agreed at take time. `node:zec:zec-maker-early-claim:TakerSellsLez` searches the Maker's files for the preimage, finds none, and shows two claims by the Maker without it refused. Crate test `startup_refuses_the_removed_prepared_zec_list_by_name` ([`taker_service_process.rs`](../crates/taker-node/tests/taker_service_process.rs)) | The devnet's `gen-config.sh` still mints both lane-1 Zcash keys, because Zebra's miner address is the Maker's. The Maker's Zcash key is also its Delivery key and its funding key. The public Zcash lane is converted to take time and has not run on a public network |

## The issues closed in this version

Each is fixed in the tree. A devnet scenario named here has no recorded run
at `6ad741bf` yet.

| Issue | What was wrong | The fix | Tests | Devnet scenario | What remains |
|---|---|---|---|---|---|
| #174 | A Bitcoin swap's LEZ claim whose send failed before the sequencer, ended ambiguously or was cut by a stopped process was never sent again; for the Maker's follow-up claim the Taker then held the Bitcoin and could refund its LEZ too | [ADR 0218](architecture/0218-resend-an-exact-lez-claim-that-did-not-land.md): the exact claim is sent again as the same bytes while the chain shows it absent (`SqlitePublicEffectJournal::resend_exact_lez_claim`); conflicting presence, recorded in the table `public_effect_conflicting_presence`, ends every send; the revealing claim is resent only inside its claim window; the sidecar decides a witnessed claim's submission on the chain again | [`crates/btc-reference-actor/src/issue174_tests.rs`](../crates/btc-reference-actor/src/issue174_tests.rs), six tests: `a_lez_claim_that_never_reached_the_sequencer_is_sent_by_the_next_drive`, `a_lez_claim_with_an_ambiguous_answer_is_sent_again_until_it_is_accepted`, `a_lez_claim_left_in_flight_by_a_stopped_process_is_sent_again`, `a_lez_claim_contradicted_on_the_chain_is_never_sent_again`, `a_revealing_lez_claim_is_not_sent_again_once_its_claim_window_closed`, `a_token_claim_with_a_failed_send_is_sent_by_the_next_drive`. [`crates/swap-store/tests/public_effect_journal.rs`](../crates/swap-store/tests/public_effect_journal.rs): `an_exact_lez_claim_left_unknown_or_started_is_sent_again_while_still_absent`, `a_resend_is_lez_claim_only_evidence_bound_and_barred_by_conflicting_presence`. [`compat/lez-v0_2-sidecar/tests/bridge_witnessed_claim_resend.rs`](../compat/lez-v0_2-sidecar/tests/bridge_witnessed_claim_resend.rs): `a_witnessed_claim_refused_before_its_send_is_sent_under_the_same_request_id`, `a_witnessed_claim_with_an_unknown_outcome_is_decided_on_the_chain_again`, `a_witnessed_claim_in_flight_at_a_kill_is_decided_again_after_the_restart` | `node:btc:claim-send-fails`, both directions: the claim's submission is refused twice before its send in its owner's sidecar (the devnet test hook `fail-claim-submissions`); the swap still completes on both Nodes | A claim the node accepted and the sequencer later dropped is not sent again by the Bitcoin actor. The actor does not bound a claim's first send in time. The scenario fails the send before the sequencer; the ambiguous answer and the stopped process are covered by crate tests only |
| #175 | Two Node readers connected to `lez.sequencer_url` without the route check the sidecar applies | [`crates/btc-role-lifecycle/src/lez_endpoint.rs`](../crates/btc-role-lifecycle/src/lez_endpoint.rs): `lez.sequencer_url` and `lez.indexer_url` are checked at configuration load against `sequencer_route` (literal loopback HTTP with a port for `local`, the exact official origin for `official_public`, the indexer literal loopback under both); the readers (`getAccount`, `getLastBlockId`, `getLastFinalizedBlockId`, the Monero supervisor's `getTransaction`) take only the checked type | In `lez_endpoint.rs`: `the_local_route_accepts_only_literal_loopback_http_with_a_port`, `the_official_public_route_accepts_only_the_exact_official_origin`, `the_indexer_is_literal_loopback_under_every_route`, `a_refusal_never_repeats_the_url`, `the_official_origin_is_the_one_the_sidecar_allowlists`. In [`tests/lez_endpoint_routes.rs`](../crates/btc-role-lifecycle/tests/lez_endpoint_routes.rs): `the_local_route_loads_only_a_literal_loopback_sequencer`, `the_official_public_route_loads_only_the_exact_official_origin`, `the_indexer_loads_only_as_literal_loopback_under_both_routes`, `the_two_sequencer_readers_connect_only_to_a_url_the_route_accepted`, `a_finality_watch_takes_only_checked_endpoints`, `no_node_source_connects_to_the_configured_url_text` | None: a configuration check. Every devnet test loads a `local` route | The `official_public` route is checked by crate tests only; no Node has run against the official origin |
| #176 | The Maker's LEZ locks shared its account with its Zcash follow-up claims and skipped the claim gate, so a lock could take a claim's nonce; a submitted claim held the next one until it was final; the public-testnet capacity came from a formula that gave zero and was raised to one | [ADR 0219](architecture/0219-one-gate-for-the-maker-accounts-lez-transactions.md): one gate in the Maker's actor supervisor ([`account_gate.rs`](../crates/maker-node/src/actor_supervisor/account_gate.rs)) for every LEZ transaction of the Maker's account (Zcash follow-up claims, Zcash and Bitcoin LEZ locks, the Monero tag 17). Claims go one at a time and never behind a lock; no lock is planned or sent while a claim waits, is admitted or is within its hold; locks go one at a time; a submitted claim holds the next for one admission bound from its submission. The capacity is the queue bound, `1 + ⌊(M − 2 × landing) / slot⌋`: 3 under the devnet profiles, 1 under `deterministic-local-v3`, and held at 1 under `public-testnet-v3` by an explicit constant for a network on which no claim was measured | [`crates/maker-node/tests/maker_actor_supervisor.rs`](../crates/maker-node/tests/maker_actor_supervisor.rs): `the_makers_lez_locks_wait_for_a_pending_lez_claim_and_for_each_other_and_no_claim_waits_for_a_lock`, `a_submitted_lez_claim_holds_the_next_claim_for_its_hold_not_until_it_is_final`, `a_makers_zcash_funding_drive_passes_the_lez_account_gate`. [`crates/maker-node/tests/zec_followup_claim_contention.rs`](../crates/maker-node/tests/zec_followup_claim_contention.rs): `a_followup_lez_claim_re_planned_more_than_five_times_across_attempts_still_lands` (the real `lez-zec-maker-actor` under the supervisor's requeue: seven re-plans, the claim included in the eighth attempt, a competing LEZ lock held back with `lez_lock_waits_for_claim` until the claim is final). Unit tests in `account_gate.rs` and in `crates/maker-node/src/zec_chat.rs` (`the_capacity_is_the_queue_bound_and_never_a_zero_raised_to_one`) | `node:zec:zec-maker-claim-collision:TakerSellsLez`: the sidecar's test hook `claim-nonce-thefts` takes the claim's nonce six times with other transactions of the Maker's account; the claim is planned again each time through the actor and the Node's requeue, and lands | A Bitcoin LEZ lock that loses its nonce is not planned again: that swap expires with nothing locked by the Maker. The locks in flight are kept in memory, so a restart can cost the next transaction one re-plan. Two Nodes configured with one LEZ account are not coordinated. The capacity under `public-testnet-v3` rests on no measurement |
| #178 | Item 1: the Maker's Zcash refund after a lost LEZ claim was recorded as an actor failure. Item 2: an accepted Zcash `TakerSellsLez` swap the Taker never funded held a capacity place for the whole LEZ refund delay | Item 1 was fixed in 0.3.4; in this version the Node's test reads the actor's own serialized output. Item 2, [ADR 0220](architecture/0220-an-unfunded-zec-swap-gives-its-place-up.md): the swap holds a place only for its Taker-lock window, `(LEZ refund delay − M) / 2` from the acceptance (300 s under `deterministic-local-devnet-v7`, 180 s under `-v8`, 45 s under `deterministic-local-v3`, 3,600 s under `public-testnet-v3`), unless its funding was admitted (`SqliteSwapStore::admit_maker_zec_funding`, table `maker_zec_funding_admissions`); a lock confirmed later is funded only while a place is free, and otherwise the Taker refunds at the LEZ refund time | Item 1: `the_lost_claim_refund_output_is_the_one_the_maker_node_is_tested_with` (`crates/zec-reference-actor/src/command.rs`, the fixture compared byte for byte) and `a_maker_zcash_refund_after_a_lost_lez_claim_ends_the_swap_refunded` (`maker_actor_supervisor.rs`). Item 2: `an_unfunded_taker_sells_lez_swap_gives_its_place_up_and_a_late_lock_needs_a_free_place` ([`crates/swap-store/tests/zec_sdk_recovery.rs`](../crates/swap-store/tests/zec_sdk_recovery.rs)) and `a_taker_lock_confirmed_after_its_window_is_funded_only_once_a_place_is_free` (`maker_actor_supervisor.rs`) | None | No devnet scenario lets a window pass. A Taker whose lock is confirmed after its window while the Maker is at capacity waits and may have to refund; nothing tells it before it locks |
| #182 | The Taker desk offered **Refund** on a Zcash swap from the Taker's confirmed lock on, while the Maker was still in time to fund and before the refund could be sent | The Zcash actor names the Taker's refund between the locks only once the Taker's claim window has closed; the Taker Node's projection offers Refund only then; an earlier `taker_swap_refund_v1` is refused with `-32016` `taker_action_unavailable` and the Node keeps observing; the desk's row carries no Refund while it waits | `before_the_makers_lock_the_taker_is_told_to_refund_only_once_its_claim_window_closed` (`crates/zec-reference-actor/src/command.rs`); `zec_refund_is_offered_only_when_the_actors_next_action_is_the_takers_refund` and `an_early_zcash_refund_is_refused_and_the_swap_goes_on_to_its_claim` (`crates/taker-node/src/taker_service/lifecycle.rs`); the supervisor tests of `crates/taker-node/src/taker_service/zec_supervisor.rs`; the desk contract in `scripts/check-m6-basecamp-package-contract.mjs` | `node:zec:zec-early-refund`, both directions: with the Maker's lock withheld the row reads `awaiting_second_lock` with no action and the refund request answers `taker_action_unavailable`; the Maker then locks and the swap completes. `ui:zec:zec-early-refund`, both directions: the desk journey of the same state | Nothing known |

## The RFP-003 requirements M5 touches

The Status column says what 0.5.0 adds. The whole row of each requirement is
in the [traceability matrix](requirements-traceability.md).

| Req | What it asks | Proof | Status |
|---|---|---|---|
| F8 | Pluggable local and Logos-module C-API price sources | D-5: `node:btc:maker-price-module:TakerSellsForeign`, `node:btc:maker-price-module-stale:TakerSellsForeign`; the process tests `crates/maker-node/tests/logos_price_*` | A stale quote and an unavailable module fail closed on the stack, on a Bitcoin and a Zcash route. The module is the repository's test fixture; no real Logos price module exists |
| F9 | Headless Maker: configuration, pricing, advertisement, execution, monitoring, full CLI operation | D-2: `node:btc:maker-cli:TakerSellsForeign`, `node:btc:maker-cli-refund:TakerSellsForeign`, `node:zec:zec-maker-cli-refund:TakerSellsForeign`, `node:zec:zec-maker-cli-claim:TakerSellsLez`, `node:zec:zec-dynamic-happy` | The CLI runs a swap's Maker side, a manual refund and a manual claim on the stack. No prepared catalog and no fixture: every Zcash swap is agreed at take time |
| U2 | Long-running autonomous Maker daemon plus systemd unit and install guide | D-1 | Both modes documented in [maker-install.md](maker-install.md). The systemd VM run on the 0.5.0 tree has not run yet; the run on record is of 2026-11-12 |
| U3 | Maker CLI: pairs and prices, daemon control, history, manual claim and refund | D-2 | Manual refund on the stack for Bitcoin and Zcash, manual claim for Zcash; 18 owner commands refused for another uid. `start` and `stop` are systemd only |
| U4 | Taker CLI: discovery, initiation, monitoring, claim, refund | D-3 | The Taker CLI drives a happy swap of each pair and a refund for Bitcoin and Zcash on the stack |
| R4 | Persisted state survives crash and restart without fund loss | D-4, the issues table (#174); `node:zec:zec-kill-maker-refund`, `node:zec:zec-kill-taker-refund`, `node:btc:claim-send-fails` | Zcash has refund-path kill points; a claim that did not land is sent again; a corrupt swap is listed as `attention_required` beside the others (`a_swap_whose_state_cannot_be_read_does_not_hide_the_others`). No migration on the stack |
| R5 | Concurrent swaps have independent state, escrow and deadlines | D-4, the issues table (#176, #178); `node:zec:zec-maker-claim-collision:TakerSellsLez` | The Maker's LEZ transactions of every pair pass one gate; an unfunded Zcash swap gives its place up (crate tests only). One Taker identity per lane |
| R8 | Delivery and Chat outage retried, buffered or degraded, and documented | D-6 | The outage before a take is on the stack for Bitcoin and Zcash in both directions and for Monero; documented in [delivery-chat-outage.md](delivery-chat-outage.md). A take is not buffered |

R1 and R2 keep the proof of the earlier milestones; M5 changes neither. The
fuzz harness (D-7) is a deliverable of the milestone with no RFP-003
requirement of its own.

## Known limits

The limits of the milestone are stated in the normative
[M5 boundary of the submission's limitations](../submission/LIMITATIONS.md#m5-boundary).
In short:

- The full regression of `6ad741bf` and the systemd VM run on the 0.5.0 tree
  have not run yet.
- Private devnet only; no public-network run of any pair is added by M5.
- The desk has no history, claim or refund panel for the Maker and no control
  that stops the Node; `start` and `stop` of the CLIs act on systemd only.
- No real Logos price module exists; the price source is not called through
  Logos core.
- A claim the node accepted and the sequencer dropped is not resent by the
  Bitcoin actor; a Bitcoin LEZ lock that loses its nonce is not planned
  again; the place rule of ADR 0220 has no devnet scenario.
- The outage is cut on the devnet's shared directory and socket, not on the
  Logos modules; a take is not buffered.
- The fuzz model drives two swaps through the coordinator and its store, not
  the actors, the sidecar or a chain.
- The devnet mints both lane-1 Zcash keys; the Maker's Zcash key is also its
  Delivery key and its funding key.

## The evidence set

A regression set has the shape M4's have:
`docs/evidence/regression-<version>-<date>-<host>/`, with `PROVENANCE.md` (the
tested commit, checked against each Node container's
`/usr/local/share/lez/build-source.txt` before and after the run; the images,
the chains made during the run), `README.md` (what the tree is, the result,
every try, what is not covered) and `results.tsv` with `regress.log`.

The set for 0.5.0 is not committed yet, because the full regression of
`6ad741bf` has not run yet. The pre-runs of 2026-11-27 named at the top of
this page were made on images of `6a5535db`, are not evidence of `6ad741bf`
and are not committed. The latest committed regression set is M4's,
[regression-0.4.0-2026-11-25-macos-arm64-zebra-module](evidence/regression-0.4.0-2026-11-25-macos-arm64-zebra-module/)
at `23e54486`; it holds none of the twenty tests that joined the tier in this
version.

The fuzz runs of 2026-11-27 were made with `scripts/run-fuzz.sh` on an x86_64
host with 2 jobs, on the harness as it is at `6a337127` (the same `fuzz/`
tree; the store the `coordinator_store` target links did not yet hold the
#174 journal change). `coordinator_store`: 1,212 s, 34,104 executions, 4,327
edges, 18,915 features, no crash, timeout or out-of-memory. `coordinator`:
1,212 s, 1,348,701 executions, 1,246 edges, none. `offer_announcement`: 191 s,
2,084,438 executions. `chat_frame`: 191 s, 3,790,783. `zec_agreement_wire`:
188 s, 11,022,293. `xmr_reserve_wire`: 182 s, 1,291,722, with one timeout
artifact that is the target's own seed `stage-a`: an execution of it takes
about 4 s in the instrumented build against the 2 s timeout the run used, and
`run-fuzz.sh` now runs that target with 20 s. The evidence set of these runs
is not committed yet. The committed fuzz sets are of 2026-11-12:
[coordinator-fuzz-2026-11-12](evidence/coordinator-fuzz-2026-11-12/README.md)
and
[untrusted-input-fuzz-2026-11-12](evidence/untrusted-input-fuzz-2026-11-12/README.md).

The systemd set is
[maker-systemd-2026-11-12](evidence/maker-systemd-2026-11-12/README.md), from
commits before ADR 0216 and before 0.3.5. The VM run on the 0.5.0 tree has not
run yet.

## Claim discipline

As for M2, M3 and M4: no "accepted" until Logos says so, no "audited" (the
construction has had no third-party review; S12 and S13 are M7), no
"mainnet-ready". A scenario is said to pass only where a committed evidence
set records the run; for 0.5.0 none does yet. A fix is said to be in the tree
with its tests, and its limit is stated beside it.
