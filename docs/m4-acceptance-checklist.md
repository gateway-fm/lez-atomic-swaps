# M4 acceptance checklist — the XMR/LEZ pair

M4 is [rfp#124](https://github.com/logos-co/rfp/issues/124), "Milestone 4:
XMR-LEZ leg", tracked here as issue #161. It is version 0.4.0. This page maps
each of its deliverables, and each RFP-003 requirement the Monero pair
touches, to the file, test or devnet scenario that proves it, and says where
the proof stops.

It follows the shape M3 was accepted on at `v0.2.6`
([rfp#123](https://github.com/logos-co/rfp/issues/123)) and M2 was submitted
on ([M2 checklist](m2-acceptance-checklist.md)): the SDK named, with a doc that
maps the lifecycle to the public API and to the tests, and a runnable
example; three recordings for the pair's direction; a regression evidence set
with provenance; and no claim Logos has not made ("accepted", "audited",
"mainnet-ready").

A devnet scenario is named by its `regress.sh` test id
(`deploy/scripts/regress.sh list full`), run by `deploy/scripts/node-e2e.py`
(`node:`) or `deploy/scripts/ui-e2e.sh` (`ui:`) on the Docker devnet with
Monero regtest up (`LEZ_MONERO=1`). The full tier holds 179 tests, 25 of them
Monero. The last full run is at `27526e2f`
([regression-0.4.0-2026-11-23-macos-arm64](evidence/regression-0.4.0-2026-11-23-macos-arm64/)):
176 of 176, each on its first try, among them all 25 Monero tests,
`node:zec:zec-slow-funding:TakerSellsLez` and `xmr-kill-maker` with seven kill
points. One operator ran both parties on one host, an Apple-silicon Mac
(linux/arm64 images), three lanes, 263 minutes. The run before it, at
`7b4e46da`
([regression-0.4.0-2026-11-22-macos-arm64](evidence/regression-0.4.0-2026-11-22-macos-arm64/)),
passed 175 of 175, 174 on their first try;
`ui:btc:logos-happy:TakerSellsForeign` failed on its first try and on its
first rerun alone and passed on its second rerun. The run before that, at `d4475300`
([regression-m4-2026-11-21](evidence/regression-m4-2026-11-21/)), passed 160
of 160, each on its first try, among them 16 Monero tests, on x86_64; the 15
tests that joined the tier after it, nine Monero (`xmr-taker-clock-ahead`,
`xmr-punish-replans`, `xmr-maker-claim-without-authorization`,
`xmr-maker-early-punish`, `xmr-unfunded-punish`, `xmr-taker-cheats`,
`xmr-shared-address-dust`, `xmr-custody-credit`, `xmr-custody-credit-refund`)
and six BTC and ZEC custody-credit tests, ran first in the `7b4e46da` run.

## Where M4 stands

- **One direction.** XMR offers only `TakerSellsLez`: the Taker locks LEZ
  first, authorises the Maker's claim, and the Maker's LEZ claim reveals its
  Monero share. `TakerSellsXmr` is refused at every entry. Why, and how this
  order departs from the BTC/ZEC common flow and from RFP-003 F5 and F6:
  [ADR 0217](architecture/0217-xmr-taker-sells-lez-deviates-from-the-common-flow.md),
  accepted.
- **Node swaps on regtest, and on Stagenet once.** The regression and the
  recordings ran against an offline Monero 0.18.5.1 regtest chain on the
  private LEZ v0.2.4 devnet. The Nodes also take swaps on public Monero
  Stagenet (`LEZ_MONERO_NETWORK=stagenet`, the `public-stagenet` profile,
  windows +10 min, +2 h, +4 h): on 2026-11-22, under those windows, Nodes
  built from `f059d3fa` (the code of `7b4e46da`) completed a swap there,
  refunded one by the Taker Node's own refund at R (the Maker's Monero swept
  back), refunded one the Maker could not fund and punished one, with LEZ on
  the devnet and Monero through public nodes ([Stagenet guide](monero-stagenet-guide.md#the-node-driven-run-of-2026-11-22),
  [evidence](evidence/stagenet-0.4.0-2026-11-22/)); the changes made after
  `7b4e46da` (the Logos Chat resend, the ZEC second-lock lookup, the sidecar's re-decision of interrupted submissions and `url` 2.5.9) were not run on Stagenet; a first run on
  2026-11-21 had used an earlier 30-minute P − R.
  CI runs a self-hosted Stagenet `monerod` (D-6). No Monero testnet or
  mainnet run exists. The Monero lane runs on linux/x86_64 and linux/arm64
  hosts.
- **The escrow program is the one of v0.3.5 and v0.3.6.** Its source, guest
  lock and manifests are identical between v0.3.6 and 0.4.0, and the ImageID
  is `ca66e0dc4118597e0a9010670fd49aec923bba9c332e9e6a26d9208dbdf6af98`. Its
  last change, the custody-credit fix of v0.3.5 (TM-0032, issue #185), makes
  every ending pay the whole custody balance and require at least the amount.
  Release v0.3.4 and LEZ testnet 0.2 run the program before the fix
  (`c22d61fc…`).
- **RISC Zero 3.0 is yanked** (GHSA-g6qg-9j8p-7j87). LEZ v0.2.4 pins it, so
  every graph's `cargo-deny` policy carries an exemption for exactly those
  versions, as an accepted risk on devnet-only software
  ([compat README](../compat/lez-v0.2-provisional/README.md), issue #184). No
  public-network release is made while the exemption stands.

## The deliverables

| # | Deliverable (rfp#124) | Artefact | Proof | Where it stops |
|---|---|---|---|---|
| D-1 | LEZ escrow program update for the XMR cross-curve DLEQ claim path (Ed25519 adaptor + secp256k1 ↔ Ed25519 DLEQ) | Escrow tags 13–17 (`InitializeNativeXmr`, `AuthorizeNativeXmrClaim`, `ClaimNativeXmr`, `RefundNativeXmr`, `PunishNativeXmr`) in [`compat/lez-v0.2-provisional/escrow/src/lib.rs`](../compat/lez-v0.2-provisional/escrow/src/lib.rs); the construction in [protocol design](milestone-1/protocol-design.md#xmrlez); compute units in [lez-compute-units.md](lez-compute-units.md) | Guest tests `xmr_native_claim_refund_and_punish_are_atomic_and_disjoint` and `native_endings_of_every_pair_sweep_a_credited_custody` (CI, `lez-v02-provisional`); on the devnet, tags 13–16 in `node:xmr:xmr-happy:TakerSellsLez` and `node:xmr:xmr-taker-refund:TakerSellsLez`, tag 17 in `node:xmr:xmr-punish:TakerSellsLez` (passed at `d4475300`, `7b4e46da` and `27526e2f`) | Built differently from the wording: the claim and refund are BIP-340 signatures of two-party secp256k1 MuSig2 keys, completed with a Monero share as adaptor secret; the escrow verifies no DLEQ proof and no Ed25519 value, each party verifies both DLEQ proofs off chain before it signs (traceability F3, ADR 0217 step 2). Devnet only, no testnet 0.2 deployment (S1) |
| D-2 | LEZ/XMR SDK with full lifecycle coverage, including the Monero spend-key-share recovery path under partial information loss | Named: **`lez-xmr-swap-sdk`** ([crate README](../crates/xmr-swap-sdk/README.md); [SDK guide](sdk-lez-xmr.md): each area mapped to its public API and the tests that prove it), with the Nodes' actor layer `xmr-reference-actor` and the Monero RPC boundary `lez-xmr-monero-adapter`; [`crates/xmr-swap-sdk/examples/full-lifecycle.rs`](../crates/xmr-swap-sdk/examples/full-lifecycle.rs), offline; [`crates/xmr-reference-actor/examples/full-lifecycle.rs`](../crates/xmr-reference-actor/examples/full-lifecycle.rs), on the actor layer | `cargo run -p lez-xmr-swap-sdk --example full-lifecycle`: four swaps for both roles, a completed one (the Maker claims after losing its adaptor journal, the Taker rebuilds the spend key after losing its own), a refunded one (the Taker's journal lost before it authorises, so it refunds from Stage B; the Maker rebuilds the key after losing its journal), one refunded after an unanswered tag 14, and a punished one (no key can be rebuilt). [`tests/full_lifecycle.rs`](../crates/xmr-swap-sdk/tests/full_lifecycle.rs) asserts each outcome, that every rebuilt key equals the shared address's private spend key, and that a wrong share, a partial Stage B does not commit to, the other session's signature and a missing LEZ signature are refused. `cargo run -p xmr-reference-actor --example full-lifecycle`: a completed and a refunded swap for both roles on the actor layer's SQLite journals, each recovering role rebuilding the spend key after it has dropped its session state and deleted its journal, against the devnet's Monero regtest. CI runs both examples, the actor layer's with `LEZ_XMR_EXAMPLE_MONERO=off`, and the tests. The SDK unit tests and `tests/cross_curve_vectors.rs`: `claim_and_signed_refund_reveal_orders_reconstruct_the_same_spend_key`, `a_role_without_its_journal_recovers_the_spend_key_from_public_data`, `recovery_vector_reconstructs_the_same_key_on_both_paths`, `share_that_does_not_match_is_rejected` | In both examples the LEZ escrow is modelled in memory (the sidecars are not reachable from the host); in the SDK's the Monero output is modelled too. CI runs the actor layer's example without its Monero leg, which needs a funded Monero wallet. That example's one run with the Monero leg live is on Monero Stagenet, 2026-11-21 ([Stagenet guide](monero-stagenet-guide.md#the-sdk-examples-run-of-2026-11-21)): the completed swap ran end to end, the refunded swap stopped at its Monero funding, refused by a public node. Recovery after a lost journal is the SDK's `XmrActivatedAgreementV1::{recover_claim_spend_key, recover_refund_spend_key}`; the Nodes recover from their journal. The facade `XmrPairSdk` is not what the Nodes run |
| D-3 | DLEQ test-vector conformance against `comit-network/cross-curve-dleq` | Erratum [GW-M4-001](proposal-acceptance-errata.md); [`crates/xmr-swap-sdk/tests/cross_curve_vectors.rs`](../crates/xmr-swap-sdk/tests/cross_curve_vectors.rs) and its vectors; [SDK guide](sdk-lez-xmr.md#the-dleq-proof-and-key-share-recovery) | In CI: the self-generated vectors and negative cases, `alternate_generators_are_the_published_nums_points` (COMIT's two generators from their definitions), `proofs_interoperate_with_the_eigenwallet_parameters`. Outside the repository, recorded in GW-M4-001: COMIT's tests 5 of 5, the same generators, the same keys for 5 of 5 scalars, proofs refused both ways | Not conformance as worded: COMIT publishes no vectors and proves differently, so no proof crosses. The replacement has no Logos decision |
| D-4 | `monerod` + `monero-wallet-rpc` stagenet setup documentation per U9 | [Monero Stagenet guide](monero-stagenet-guide.md): the pinned release verified, a self-hosted pruned `monerod` and the public-node route, a `monero-wallet-rpc` per role, Stagenet coins from a faucet, the window sizing for 2-minute blocks, the devnet stack with both Nodes on Stagenet (`LEZ_MONERO_NETWORK=stagenet`, `deploy/compose.stagenet.yaml`), what a Node checks of a daemon it does not run, refund and punish; the SDK and adapter alone through `xmr-reference-actor`'s `examples/full-lifecycle.rs` | Every command in the guide was run on 2026-11-21 (Apple-silicon Mac, Docker Desktop): the Maker and Taker Nodes completed swap `d9353027…` on Stagenet (funding `807a7d6c…` at ten confirmations, the Maker's LEZ claim, the Taker's sweep `e27ca307…` at ten confirmations, 55 minutes from the take), refunded one and punished one, under an earlier 30-minute P − R; on 2026-11-22 the Nodes of `f059d3fa`, without test hooks, under the `PublicStagenet` windows +10 min, +2 h, +4 h, completed swap `13711985…` (funding `007ccb71…`, sweep `f2e6b550…`, 60 minutes), refunded `fc596d6b…` by the Taker Node's own tag 16 101 s after R (the Maker's sweep back `5b1ea967…`), refunded the unfunded `5a696c9f…` 7 s after R and punished `353a4758…` (Taker Node stopped through [R, P), tag 17 23 s after P) ([run](monero-stagenet-guide.md#the-node-driven-run-of-2026-11-22), [evidence](evidence/stagenet-0.4.0-2026-11-22/)); the example's completed swap (funding `0e81a263…`, sweep `59fdd682…`); the adapter's Stagenet tests against the self-hosted node during its sync | LEZ is the devnet's, not LEZ testnet; the Stagenet daemons were public nodes behind a forwarder (TM-0033), not the operator's own: the self-hosted node did not reach the tip in the session; one operator ran both Nodes on one host; three earlier swaps of the session, on images before `8f66d11b`, could not finish on the Maker side (the guide says why) |
| D-5 | XMR demo set per D1 | `TakerSellsLez-xmr-happy.mp4`, `TakerSellsLez-xmr-taker-refund.mp4`, `TakerSellsLez-xmr-concurrent.mp4` in [regression-m4-desks-2026-11-20](evidence/regression-m4-desks-2026-11-20/videos/) | Each a by-product of `ui:xmr:xmr-happy:TakerSellsLez`, `ui:xmr:xmr-taker-refund:TakerSellsLez` and `ui:xmr:xmr-concurrent:TakerSellsLez` passing while recorded, at `86bdb8a4`, 3 of 3 on the first try | Regtest; one direction; recorded at `86bdb8a4`, before the Taker Node's own refund and the custody-credit fix; the Maker desk's punish rows are on no recording |
| D-6 | Self-hosted stagenet `monerod` running in CI | The `monero-stagenet` job of [`ci.yml`](../.github/workflows/ci.yml), [`scripts/run-monero-stagenet-ci.sh`](../scripts/run-monero-stagenet-ci.sh), [`crates/xmr-monero-adapter/tests/stagenet_daemon.rs`](../crates/xmr-monero-adapter/tests/stagenet_daemon.rs) | Every CI run: the release verified, `monerod --stagenet` and `monero-wallet-rpc --stagenet` started, network, genesis and logins checked; the Stagenet identity and network check, their refusal by the Regtest checks a Node runs, the SDK's Stagenet 2-of-2 address restored as a view-only wallet and its mainnet-byte form refused; then 1,000 blocks synced from public peers. Run locally in an arm64 Linux container on 2026-11-21: passed, 3,101 blocks in 47 s | A bounded sync, not the tip; no coins and no swap in CI; it depends on public Stagenet peers answering |

## The RFP-003 requirements the pair touches

| Req | What it asks | Proof | Status |
|---|---|---|---|
| F3 | LEZ–XMR swaps with Ed25519 adaptor signatures, cross-curve DLEQ, spend-key share | `node:xmr:xmr-happy:TakerSellsLez`, `node:xmr:xmr-taker-refund:TakerSellsLez`, `node:xmr:xmr-maker-never-funds:TakerSellsLez`, `node:xmr:xmr-punish:TakerSellsLez` (passed at `d4475300`, `7b4e46da` and `27526e2f`); the SDK tests of D-2 | Devnet, regtest, `TakerSellsLez` only; secp256k1 adaptors plus DLEQ instead of Ed25519 adaptors ([protocol design](milestone-1/protocol-design.md#xmrlez), traceability F3) |
| F5 | Escrow claims on a valid proof, refunds after the timelock | Guest test `xmr_native_claim_refund_and_punish_are_atomic_and_disjoint`; `node:xmr:xmr-taker-refund:TakerSellsLez`, `node:xmr:xmr-taker-refund-kill:TakerSellsLez` (passed at `d4475300`, `7b4e46da` and `27526e2f`); the re-sent refund on LEZ time: `node:xmr:xmr-taker-clock-ahead:TakerSellsLez` (a Taker clock 60 s ahead of LEZ time sends no refund before the finalized LEZ time reaches R; passed at `7b4e46da` and `27526e2f`) and the Taker supervisor unit tests named in ADR 0217 | Departure: the refund is open only in [R, P) ([ADR 0217](architecture/0217-xmr-taker-sells-lez-deviates-from-the-common-flow.md#relation-to-rfp-003)) |
| F6 | Both complete or both refund | `node:xmr:xmr-punish:TakerSellsLez` (passed at `d4475300`, `7b4e46da` and `27526e2f`); TM-0031 on the devnet: `node:xmr:xmr-unfunded-punish:TakerSellsLez` (tag 17 from P on an unfunded swap pays the Maker with the Taker Node stopped through [R, P), and is refused once the running Taker Node has refunded; passed at `7b4e46da` and `27526e2f`) | Departure by design: the punishment from P pays the Maker the LEZ and leaves the Monero locked (ADR 0217); TM-0031 and TM-0019 open |
| F7 | Native and custom tokens through ATAs | `InitializeNativeXmr` binds the native authenticated-transfer program | Native LEZ only for XMR; no token variant exists |
| R1 | The Maker locks only after the Taker's lock is confirmed | The Maker actor funds Monero only after the Taker's lock is final and while the finalized LEZ time is before C (`drive_once` in [`xmr-maker-actor.rs`](../crates/maker-node/src/bin/xmr-maker-actor.rs)); `node:xmr:xmr-maker-never-funds:TakerSellsLez` | Code and the scenarios above; no scenario has the Maker try to fund early |
| R2 | After the lock, on-chain data alone completes or refunds | `node:xmr:xmr-cut-after-lock:TakerSellsLez` (passed at `d4475300`, `7b4e46da` and `27526e2f`) | One cut, after the lock |
| R3 | Graceful degradation with no Monero node | `node:xmr:xmr-monerod-down:TakerSellsLez` (passed at `d4475300`, `7b4e46da` and `27526e2f`) | Done on the devnet |
| R4 | State survives restarts | `node:xmr:xmr-kill-taker:TakerSellsLez` (7 kill points), `node:xmr:xmr-kill-maker:TakerSellsLez` (7, two of them with the claim held in the Maker's sidecar), `node:xmr:xmr-taker-refund-kill:TakerSellsLez`, `node:xmr:xmr-wallet-restart:TakerSellsLez` (passed at `d4475300`, `7b4e46da` and `27526e2f`; `xmr-kill-maker` had 6 points in the first two) | A Taker sweep left in doubt is recovered by hand (see the known limits) |
| R5 | Concurrent swaps keep independent state | `node:xmr:xmr-concurrent:TakerSellsLez`, `node:xmr:mixed-three-pair:TakerSellsLez` (passed at `d4475300`, `7b4e46da` and `27526e2f`) | One Taker identity per lane |
| R6 | Timelocks allow for block variance and clock drift | The windows are LEZ times fixed in Stage A; the Taker sends its refund on finalized LEZ time, not its own clock (ADR 0217 step 7); `node:xmr:xmr-taker-clock-ahead:TakerSellsLez` (passed at `7b4e46da` and `27526e2f`) | Regtest windows +320/+720/+1000 s on `fast`; Stagenet windows +10 min, +2 h, +4 h, derived in the [Stagenet guide](monero-stagenet-guide.md#the-windows) and admitted by `PublicStagenet` (R − C ≥ 100 min, P − R ≥ 2 h, the Taker's safety window); the Node-driven Stagenet run of 2026-11-22 ran under them (its punish 23 s after a P two hours after R), after a first run of 2026-11-21 under an earlier P − R of 30 min |
| R8 | Delivery/Chat outage | `node:xmr:xmr-outage-before-take:TakerSellsLez`, `node:xmr:xmr-cut-after-lock:TakerSellsLez` (passed at `d4475300`, `7b4e46da` and `27526e2f`) | Done on the devnet |
| U3, U4 | Maker and Taker CLIs per pair | The Maker CLI configures, prices and publishes the Monero route in every Monero scenario; the Taker CLI's Monero rows and `balances --monero` are unit-tested in `crates/taker-node/src/bin/lez-taker-cli.rs` | No scenario drives a Monero swap through the Taker CLI |
| U5, U6 | Maker and Taker desks per pair | `ui:xmr:xmr-happy:TakerSellsLez`, `ui:xmr:xmr-taker-refund:TakerSellsLez`, `ui:xmr:xmr-concurrent:TakerSellsLez` (passed at `d4475300`, `7b4e46da` and `27526e2f`) | No desk journey punishes |
| P1 | Compute units per operation | [lez-compute-units.md](lez-compute-units.md): tags 13, 2 and 14–17 under ImageID `ca66e0dc…` | Done |
| S1 | Escrow on LEZ testnet 0.2 | — | Not for XMR: devnet only; the public network waits for the LEZ 0.4 port (issue #107) |
| — | Escrow custody credited by a stranger (TM-0032) | Executor tests `compat/lez-v0.2-provisional/escrow/methods/tests/custody_dust.rs`; `node:xmr:xmr-custody-credit:TakerSellsLez`, `node:xmr:xmr-custody-credit-refund:TakerSellsLez` (a stranger's unit in the custody: the claim and the refund each pay the lot and the unit; passed at `7b4e46da` and `27526e2f`, as did the six BTC and ZEC custody-credit tests); a third party's outputs at the swap's Monero shared address: `node:xmr:xmr-shared-address-dust:TakerSellsLez` (the Taker's sweep spends only the swap's own output; passed at `7b4e46da` and `27526e2f`) | Holds only under the new ImageID; v0.3.4 escrows can still be locked |
| — | A cheating counterparty | `node:xmr:xmr-maker-claim-without-authorization:TakerSellsLez`, `node:xmr:xmr-maker-early-punish:TakerSellsLez`, `node:xmr:xmr-taker-cheats:TakerSellsLez`, `node:xmr:xmr-punish-replans:TakerSellsLez` (tag 15 without tag 14, tag 17 before P and after the refund, tag 16 before R and after tag 15: each refused; a punishment whose nonce is taken six times still lands; passed at `7b4e46da` and `27526e2f`); the guest test of D-1 | The same rules are checked by guest and unit tests |

## Known limits

The limits of the pair are stated in the normative
[M4 boundary of the submission's limitations](../submission/LIMITATIONS.md#m4-boundary):
scope, the protocol's departures and open threats, the escrow, and what each
Node and the sidecar still do not handle.

## The evidence set

`docs/evidence/regression-m4-<date>/`, or for a run on a release version
`docs/evidence/regression-<version>-<date>-<host>/` as for 0.3.0 to 0.3.4
(`regression-0.4.0-<date>-macos-arm64/` on an Apple-silicon Mac), in M3's and
M2's shape:
`PROVENANCE.md` (the tested commit, checked against each Node container's
`/usr/local/share/lez/build-source.txt` before and after the run; the images,
the chains made during the run), `README.md` (what the tree is, the result,
every try, what is not covered) and `results.tsv` with `regress.log`. The
latest set is
[regression-0.4.0-2026-11-23-macos-arm64](evidence/regression-0.4.0-2026-11-23-macos-arm64/)
at `27526e2f`, on arm64; the one before it,
[regression-0.4.0-2026-11-22-macos-arm64](evidence/regression-0.4.0-2026-11-22-macos-arm64/)
at `7b4e46da`, on arm64 too, and
[regression-m4-2026-11-21](evidence/regression-m4-2026-11-21/) at `d4475300`
on x86_64.

## Claim discipline

As for M2 and M3: no "accepted" until Logos says so, no "audited" (the
construction has had no third-party review; S12 and S13 are M7), no
"mainnet-ready". Atomicity is stated as the protocol gives it: conditional on
each party acting in its window, with the punishment as the one outcome in
which only one side is paid.
