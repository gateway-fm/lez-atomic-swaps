# M2 acceptance checklist — follow what M3 was judged on

M3 was accepted at `v0.2.6` on 2026-10-09 ([rfp#123](https://github.com/logos-co/rfp/issues/123)).
The reviewer's bar, in their own words, was: three recordings per chain (one
direction is enough if the other is backed by regression and text records,
and you say so); the SDK **named**, with a doc that maps the five lifecycle
stages to the public API and to the tests that drive each, plus a runnable
offline example they can build from the tag; a node setup guide with every
route, wallet creation, funding, and the direction each route serves stated
plainly. Behind that sat the regression evidence set
(`docs/evidence/regression-v0.2.5-20260928/`: `PROVENANCE.md` + `RESULTS.txt`,
binaries rebuilt from the tagged commit, chain recreations logged) and the
claim discipline (no "mainnet-ready", no "audited", nothing Logos has not
said).

M2 is [rfp#122](https://github.com/logos-co/rfp/issues/122). Its six official
deliverables, each mapped to the artefact that answers it, the proof the
reviewer can run, and where it stands on `feat/m2-zec-on-docker` today.

## State on 2026-11-11 (PR #120)

Read this first; the tables below were written on 2026-11-03 for the Docker
stack and are kept as the record of that submission shape.

- **The stack moved.** PR #120 runs the stack as Logos modules (issue #115):
  one network Basecamp, each role's Node in a Logos package inside the role's
  Basecamp. `docs/submission-115.md` is the traceability and the limitations.
  The sequencing decision of 2026-10-28 below ("M2 on the Docker stack first")
  is superseded: M2's evidence is produced on the modules stack.
- **Zcash is on NU7.** Public Zcash Testnet activated NU7; the engine signs
  for it (checked against mined Testnet transactions) and the regtest devnet
  runs Zebra 7.0.0 with NU7 from height 1, so the regression exercises the
  same consensus branch (deliverable 2).
- **Public testnet (deliverable 1's public half, and any public corridor).**
  Not applicable to this release. The public LEZ testnet has moved to LEZ 0.3, which this LEZ v0.2.4 stack cannot talk to.
  Public-network evidence is postponed to the release that ports to LEZ 0.4 (#107). The devnet evidence stands for
  the escrow: deployed, funded, claimed and refunded in both directions on
  every recreated chain. (A legacy v0.2 endpoint still answers; exploratory
  work against it is on a follow-up branch and is not part of this
  submission.)
- **Deliverable 6, recordings.** `regress.sh` records every desk test of a
  full run (`LEZ_REGRESS_RECORD=1`); the release candidate's run is the source
  of the demo set.
- **Review findings due with the ZEC submission** (issue #103 items 1 and 16,
  #112, #113): fixed in PR #120; `docs/issue-closing-notes-2026-11-11.md`.
- **The evidence set** is `docs/evidence/regression-m2-2026-11-11/`: all 93
  tests in one run on three lanes (92 passed; the one failure and its reruns
  are explained in its `RESULTS.txt`), with provenance and the ZEC demo
  recordings.
- **The module catalog** is under the organisation (#106):
  `gateway-fm/logos-modules-release-base`. The address under a personal
  account keeps serving until the next release.
- **Still open for the submission:** the release workflow green on the commit
  to be tagged (on `main` at `af3ac978`, `start.sh` could not finish; the
  fixes and the runs that show them are in
  `docs/evidence/macos-arm64-release-gates-2026-11-11/`), then the tag and
  release (0.3.0), the rfp#122 comment, the fresh-machine walk of the Zcash
  guide (U10).

## The six deliverables

| # | Official deliverable (rfp#122) | Artefact | Proof | Today |
|---|---|---|---|---|
| 1 | LEZ escrow program v1 deployed on testnet 0.2, SHA-256 HTLC preimage path, `BlockValidityWindow` / `TimestampValidityWindow` for the refund | The deployed LEZ v0.2 escrow (devnet ImageID `c22d61fc…`, the same program the BTC leg settles on); the ZEC agreement binds `secret_digest`, the Maker's `lez_revealing_claim` reveals it, `ZecRefundPlanV1` carries both windows | M6 service certificates (claim + refund, 2026-08-04); the devnet `zec-happy` / refund scenarios below; **S1 needs a version-pinned deployment manifest and a public testnet-0.2 smoke transaction** | Devnet: yes, settled and refunded in both directions in `docs/evidence/regression-m2-2026-11-02/`. Public testnet 0.2: open (S1) |
| 2 | BIP-199 HTLC scripts and transaction construction on the transparent pool, integration tests against Zcash testnet **in CI** | `lez-zec-swap-sdk` (`bip199_contract`, `bip199_interpreter`, `zcash_transactions`, `zcash_funding` tests), `lez-zebra-node-adapter`, `zebra_regtest.rs` (fund → claim → refund through Zebra consensus) | A CI job that runs the Zebra regtest test on every push to the M2 branch. **On this branch CI parks the ZEC tests and excludes `lez-zebra-node-adapter`; the gate exists only on PR #116** (`8be5d829`, `2bf2e8f6`, `999bddfe`) | Done 2026-11-03: the three commits are on this branch (`ff958ec8`, `6574eeea`, `055bb204`), the Zebra test image's base moved to a clean distroless (`b7251402`); every new step ran green locally before the push |
| 3 | LEZ/ZEC SDK with full lifecycle coverage (discovery, negotiation, escrow creation, claim, refund) | Name it: **`lez-zec-swap-sdk`** on `lez-swap-sdk-core` (confirm the layering from `docs/architecture/0013-sdk-layering.md`); `docs/sdk-lez-zec.md` stage → API → test map; `crates/zec-swap-sdk/README.md`; `crates/zec-swap-sdk/examples/full-lifecycle.rs` offline, compiled under `--all-targets` | The reviewer builds and runs the example from the tag; the named tests in `tests/sdk_lifecycle.rs` (e.g. `first_claimant_activation_persists_agreement_and_claim_preimage_in_both_directions`, `maker_happy_path_locks_the_opposite_chain_and_restarts_in_both_legs_locked`, `taker_reorg_between_lez_maker_steps_suspends_funding_until_replacement`) cited per stage; traceability U1 / S8 rows updated | Done: `docs/sdk-lez-zec.md`, `crates/zec-swap-sdk/README.md`, `crates/zec-swap-sdk/examples/full-lifecycle.rs`; U1 / S8 rows updated |
| 4 | Step-by-step Zcash node setup (the M1-chosen node plus the public-node option): configuration, wallet setup, obtaining testnet funds | `docs/zcash-testnet-setup.md` (258 lines: self-hosted Zebra route, public Zebrad provider route, disposable transparent wallet and funds, corridor rehearsal) | Read it against the five topics the M3 reviewer checked on U8: each route, wallet creation, funding, **which direction each route serves and why**; a fresh-machine walk (U10) | Guide done (both routes, wallet, funding, directions). A fresh-machine record (U10) is still open |
| 5 | Documented privacy posture: transparent-pool visibility and the shield-after-swap user pattern | One named section the reviewer can link (`docs/sdk-lez-zec.md` or `docs/zec-privacy-posture.md`), citing `docs/milestone-1/protocol-design.md`, ADR 0004, and the Node's `ShieldReceivedTransparentZecSeparately` guidance; the Taker desk's label for a Zcash row restored ("After a transparent ZEC claim, move funds to a shielded wallet address.") | The doc; the desk label visible in the ZEC desk recording | Done: `docs/sdk-lez-zec.md#privacy-posture`; the Taker desk shows the guidance on a completed ZEC row again (`takerShielding`, `node_market.cpp`) |
| 6 | ZEC demo set per D1: happy path, refund/timeout, concurrent swaps | `ui-e2e.sh <zec-scenario> --record` → `runtime/evidence/videos/TakerSellsLez-zec-happy.mp4`, `…-zec-maker-refund.mp4` (or taker), `…-zec-concurrent.mp4`, attached to the release | Each recording is a by-product of a desk scenario that passed while being recorded, as for BTC | Done: happy, maker-refund and concurrent in both directions plus stale-window selling ZEC, in `docs/evidence/regression-m2-2026-11-02/videos/` |

## The evidence set, in M3's shape

`docs/evidence/regression-m2-<date>/`:

- `PROVENANCE.md`: commit under test, binaries rebuilt from it (image digests
  of `lez-maker-node`, `lez-taker-node`, `lez-zebra`, the bedrock/sequencer/
  indexer images), chain recreations during the run with their timestamps and
  the reason (#3643 stall), what was reset (`reset-swaps.sh`) and when, which
  corridor run id was live for which scenario.
- `RESULTS.txt` in the `##### E2E <direction> <scenario> attempt N <time>` /
  `##### E2E-RESULT … PASSED|FAILED <time>` format `node-e2e.py` already
  prints, one block per check.
- The check matrix M3 ran was 45 (node-e2e both directions, desk suites, desk
  scenarios both directions). For M2 state the matrix and its scope plainly:

| Phase | Checks | Scope statement |
|---|---|---|
| `node-e2e.py --pair Zcash`, **both directions** (handover Part D) | happy, replay, wrong-inputs, restart-taker, restart-maker, survivor, concurrent, taker-refund, maker-refund, regenerated-catalog, tampered-catalog (fast profile for the refunds) | Decided 2026-10-28: both directions through the Nodes. Done 2026-11-01: 28/28 in both directions (the eleven plus early-lock, offline-after-lock and refund-then-maker-lock for BTC parity), profile `deterministic-local-devnet-v3` |
| desk scenarios, Zcash | happy, concurrent, one refund (+ restart-taker if the catalog survives a restart, which it should) | Done 2026-11-01/02: 15 desk journeys in both directions (taker-refund, maker-refund, concurrent, concurrent-staggered, restart-taker, restart-maker, survivor, node-offline, happy, stale-window) plus the recordings; the Taker Node drives its own first LEZ lock after the take |
| BTC regression on the same binaries | `node-e2e.py` 14 × TakerSellsForeign **and** 14 × TakerSellsLez, the desk suites, desk scenarios forward (8) and reversed (7): the full M3-shaped 45 | Decided 2026-10-28: the full set, both directions. Done on the M2 images: Node 28/28 and desks 16/16, eleven of them on a rerun after a devnet finality freeze (see the set's `RESULTS.txt`) |
| CI | the Zebra regtest gate green on the pushed commit | On this branch since 2026-11-03; green locally (HTLC tests, Trivy 0 HIGH/CRITICAL) |

Every scenario exports its Node evidence (`export-node-evidence.py`) under
the set, as the BTC scenarios do today.

## Repository changes the submission needs

State on 2026-11-03: items 1 to 6 done (4, the CI gate, on 2026-11-03); 7 done in
`m6-zec-reconciliation.md`; 8 and 9 wait for the owner (no tag yet).

1. `docs/sdk-lez-zec.md`, `crates/zec-swap-sdk/README.md`,
   `crates/zec-swap-sdk/examples/full-lifecycle.rs` (deliverable 3).
2. The privacy-posture section and the restored desk label (5).
3. `docs/zcash-testnet-setup.md` reviewed against the U8 standard (4).
4. CI: the Zebra regtest gate and un-parked ZEC tests on this branch (2).
5. `docs/requirements-traceability.md`: F4, U1, U6, U10, S1, S5, S8, D1 rows
   edited in place (one row per ID — the guard counts them), "Last
   reconciled" refreshed.
6. `submission/MILESTONES.md`: an `## M2 — ZEC/LEZ leg` section in the M3
   section's shape (official deliverable → artefact → status), upstream
   rfp#122.
7. `docs/milestone-metrics.md` and `docs/m6-zec-reconciliation.md`: the
   reconciliation's "what bringing ZEC back would take" is now done or
   scoped — update it rather than leave it contradicting the tree.
8. **No tag until everything is done and the evidence is in** (owner,
   2026-10-28). The release will be `0.3.0`; until then a draft PR from
   `feat/m2-zec-on-docker` to `main` so CI runs on every push, with the
   release notes drafted alongside.
9. The rfp#122 comment, in the shape of the accepted rfp#123 one: point at
   the tag, map the six deliverables to links, state the direction and
   desk-lock scope in the body, do not tick Logos's boxes.

## Sequencing against the rfp#122 note — decided

The only comment on rfp#122 (2026-10-16) records the 13 October call:
*"Gateway moves the Docker stack into Logos modules before starting M2
(gateway-fm/lez-atomic-swaps#115). M2 remains unchanged otherwise."* Decided
2026-10-28: **a clean M2 on the Docker stack first, the modules migration
(#115) afterwards.** The LEZ modules gave trouble when tried, and the owner
does not want M2's evidence to depend on them. The submission comment says
this in its first lines — M2 is delivered on the Docker stack that M3 was
accepted on, #115 follows as its own change and is tracked separately —
rather than leaving the reviewer to ask the question already written
down on the issue. No claim is made about the modules' readiness.

## Claim discipline

As in M3: no "mainnet-ready", no "audited", no "accepted" until Logos says
so; conditional atomicity stated as the M6 certificates state it
("…under canonical local-chain observations, secret-gated claiming, durable
one-winner authorization…; not a distributed atomic commit or future-reorg
proof"); public-RPC / faucet / public-funds usage declared per evidence set.
