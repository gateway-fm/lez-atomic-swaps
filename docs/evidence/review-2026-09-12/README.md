# Evidence for the RFP-003 review index (logos-co/ecosystem#182)

Prepared 2026-09-12 against the reviewer's index comment of 2026-09-10
(<https://github.com/logos-co/ecosystem/issues/182#issuecomment-5616692159>).
Each item of that comment that asks Gateway for something, or names a run the
reviewer intends to make, is answered below with the artefact that answers it.
Where an item needs a statement rather than test output, it says so.

## The build this evidence was produced on

| | |
|---|---|
| Repository | `gateway-fm/lez-atomic-swaps` |
| Branch / PR | `integration/all-open`, PR #42 "Pre-release fixes for v0.2.2" |
| Commit under test | `963cf12` for the first full pass of the matrix (2026-09-11); the clean recordings then exposed two Node defects (below), fixed on `4adac34` and `d3bb481`; every scenario a fix touches was run again on the fixed code, and the summaries in `e2e/` are those of the latest run of each scenario. Final commit: `d3bb481` |
| Intended release | v0.2.2, tagged from `main` once #42 is merged |
| Images | `ghcr.io/gateway-fm/lez-atomic-swaps/lez-{maker-node,taker-node,lez-services,bitcoin-core,btc-miner,btc-explorer,lez-explorer,basecamp-ui}:<tag>` (linux/arm64) |
| Startup bundle | `lez-swap-stack-<tag>-arm64.tar.gz`, attached to the GitHub release; `./scripts/start.sh` pulls the images, mints the wallet identities, renders the configuration, starts the stack, bootstraps the market and runs both desk suites; `./scripts/start.sh --swap` adds one complete swap through the two desks |
| Basecamp modules | published to the module catalog `mandrigin/logos-modules-release-base` from the release tag (the catalog checkout pins its `lez-atomic-swaps` submodule to the tag and runs `./scripts/catalog.sh release-all --force --watch`) |
| Supported Mac architecture | arm64 (Apple silicon) with Docker Desktop; arm64 Linux equally. No amd64 build of the stack images |
| Supported test path | the supplied VNC environment: the `basecamp-ui` container runs both desks on an Xvfb display, reachable at `vnc://127.0.0.1:5901` (password `lezswap`). Every desk result below was produced by pressing those desks' buttons. A native Mac Basecamp with the catalog modules is published but was **not** exercised by these runs; how it receives the Node sockets and chain configuration is a Gateway statement still owed (see "Statements owed") |
| Timing profile | `fast` (Maker lock cutoff 600 s, earlier refund 900 s, later refund 1200 s, Bitcoin CSV 6 blocks); the `local` profile differs only in these constants |

## The scenario matrix

Nine scenarios, each in both Bitcoin directions (the Taker sells BTC, the
Taker sells LEZ), each run twice on the same code paths: through the Nodes'
owner APIs (`deploy/scripts/node-e2e.py`) and by pressing the real desks
(`deploy/scripts/ui-e2e.sh`, which drives the Basecamp desks through the QML
inspector and reads the APIs only to learn the new swap's id). The desks call
exactly the Node methods the API run calls; there are no test-only methods.

| Scenario | e2e · Taker sells BTC | e2e · Taker sells LEZ | Desks · sells BTC | Desks · sells LEZ |
|---|---|---|---|---|
| happy | passed | passed | passed | passed |
| replay | passed | passed | n/a | n/a |
| wrong-inputs | passed | passed | n/a | n/a |
| restart-taker | passed | passed | passed | passed |
| restart-maker | passed | passed | passed | passed |
| survivor | passed | passed | passed | passed |
| concurrent | passed | passed | passed | passed |
| taker-refund | passed | passed | passed | passed |
| maker-refund | passed | passed | passed | passed |

replay and wrong-inputs have no desk equivalent by design: a desk sends one
request per click and offers only the actions the Node reports.

Per-scenario JSON summaries of the API runs, as written by the harness, are in
[`e2e/`](e2e/) (`<scenario>.json` for the Taker-sells-BTC direction,
`<scenario>-lez.json` for the Taker-sells-LEZ direction). Each carries the swap
id(s), the direction, the timing profile in force, the wall-clock duration and
the result. The desk column was passed twice: once with the accumulated swap
history of the API runs still on the Nodes, and once more on a stack whose swap
history was reset before each scenario (the recorded runs below); the second
pass is where the two defects listed under "Defects found" surfaced, and the
affected scenarios (concurrent, taker-refund, happy) were run a third time, in
both directions and both ways, on the fixed code.

## Videos of every desk scenario

Fourteen silent screen recordings with burned-in narration, one per scenario
and direction, recorded on a stack whose swap history was reset before each
scenario so that a video shows only its own swap(s). Delivered alongside this
document as `videos/<direction>-<scenario>.mp4` (1.6 to 5.2 MB each, five to
twenty-one minutes), with `direction` one of `TakerSellsForeign` (the Taker
sells BTC) and `TakerSellsLez`. The swaps in the recordings are the ones whose
exported on-chain evidence sits in [`exports/`](exports/) (thirteen files; the
first recorded swap's export was lost to a reset before the collector ran).

| Video | What it shows |
|---|---|
| `*-happy` | offer published on the Maker desk, taken on the Taker desk, first lock pressed, the Maker Node's own second lock, the revealing claim pressed, the Maker Node's follow-up claim, both desks at "completed" |
| `*-restart-taker` | as happy, with the Taker Node restarted between its lock and its claim |
| `*-restart-maker` | as happy, with the Maker Node restarted between the Taker's lock and its own |
| `*-survivor` | the Taker Node stopped right after its revealing claim; the Maker desk shows the swap completed by the Maker Node alone; the Taker desk catches up after the restart |
| `*-concurrent` | two offers taken, two first locks, two claims, both swaps completed, each with its own row and state |
| `*-taker-refund` | the Maker Node stopped before it can lock; after the cutoff the Taker desk offers Refund; the Taker's leg comes back; the restarted Maker reconciles |
| `*-maker-refund` | the Taker never claims; after its deadline the Maker Node refunds its own lock and its desk shows "refunded"; the Taker desk then offers and completes its refund |

Each recorded run was itself a passing desk scenario (the runner asserts every
step), so the recordings are a third pass over the desk column above.

## M3: swaps and recovery

**Run a completed BTC/LEZ swap on the selected build.** `e2e/happy.json`
(Taker sells BTC, swap `394ad56e5905…`) and `e2e/happy-lez.json` (Taker sells
LEZ, swap `e6635c6af3e4…`); the desk videos `*-happy`; and, for the recorded
swaps, the exported on-chain evidence in [`exports/`](exports/): the five public
transactions of a completed swap (the Taker's first lock, the escrow
initialization, the second lock, the revealing claim, the follow-up claim),
each with chain, block, finality and the local explorer address.

**Run both refund cases on that build.** `e2e/taker-refund.json` and
`e2e/taker-refund-lez.json` (the Maker Node stopped before it locks; the Taker
refunds after the cutoff; the Node's own wallet balances before and after are
recorded), `e2e/maker-refund.json` and `e2e/maker-refund-lez.json` (the Taker
never claims; the Maker refunds its leg after its deadline; the Taker refunds
afterwards); the videos `*-taker-refund` and `*-maker-refund`.

**Run overlapping swaps and verify that each keeps its own state and
outcome.** `e2e/concurrent.json` and `e2e/concurrent-lez.json` list both swap
ids of each run; each swap completed on its own; the videos `*-concurrent`
show two rows advancing independently on both desks.

**Late-observed claim follow-up (PR #32 comment).** Addressed on this branch,
commit `4c9212b`. The Taker Node's observer stopped observing a submitted claim
once the actor no longer offered the claim (the claim window had closed), and
waited for a Maker refund that could not come while the Maker completed the
swap. The observer now keeps observing a submitted claim across the window's
close (observing never sends) and additionally watches the Maker's refund once
the window has closed. Regression test:
`crates/btc-reference-actor/src/tests.rs::late_observed_revealing_claim_still_completes_both_roles_without_another_send`
freezes the wall clock past the earlier refund's deadline, asserts the actor
routes to recovery, then presents a canonical revealing claim and asserts it is
projected for both roles and both directions with exactly one observation and no
send, and that the follow-up claim completes the swap. The admitted-but-expired
claim/refund case is kept (`closed_claim_window_routes_revision_two_to_recovery_through_the_maker_leg`).

**Restart/configuration regression (PR #28 comment).** Addressed as two
automated scenarios of the live harness on this branch, commit `4c9212b`:
`node-e2e.py regenerated-config` (a swap is taken and locked, the runtime
configuration is rendered again with the same chain and identities, the Taker
Node is restarted onto it, the swap must reload from its own saved copy and
complete) and `node-e2e.py tampered-config` (the swap's copied
`taker-role-config.json` is altered by one byte, the Taker Node is restarted,
the swap must be refused: its lock is rejected and it lists as needing
attention). Results: see "Results of the follow-up scenarios" at the end of this
document.

**Align the milestone/evidence map with the review build.** The historical
evidence under `docs/evidence/` predates the Node-owned lifecycle (ADR 0213) and
the v0.2 line; the runs in this document are the ones that apply to the review
build. Everything here was produced on the commit named above, by the scripts
named above, and can be reproduced with the commands in "How to reproduce".

**Replacement tests (proposal-acceptance-errata).** No change from this work;
the acceptance decision is the reviewer's.

## M1: design and safety

**Enforced claim cutoff against the documented delay/finality assumptions.**
What the code does on this build:

- Both roles derive one countersigned schedule per swap (Maker lock cutoff,
  earlier refund deadline, later refund opening, Bitcoin refund height). The
  Taker Node offers the refund only once the Maker's cutoff has passed (#43),
  and every desk countdown is read from that same schedule.
- Past the earlier refund's deadline the actor routes both roles to recovery
  through the Maker's leg, because a revealing claim sent late is admitted to
  the mempool and dropped at block build (reproduced against the real v0.2.0
  block builder in PR #18). A claim already submitted before the deadline stays
  observed (this document's #32 follow-up).
- The Maker acts on the revealing claim only from finalized evidence (a
  finalized witnessed claim on LEZ; the signed confirmation policy on Bitcoin).
  It does not read a secret out of an unconfirmed transaction.

The case the reviewer names, a secret visible in a transaction that never
becomes canonical or final, is exactly the "reveal before finality" question the
reviewer has routed to the M7 audit. This build does not claim to close it: the
Node's automation never acts on an unfinalized reveal, but nothing in the
protocol prevents a counterparty from reading a mempool transaction by other
means. This document records that as open, for M7.

**XMR penalty fallback (ADR 0174).** Not exercised by this build (the swaps
under test are BTC/LEZ). The explicit disposition the reviewer asks for is a
Gateway statement; see "Statements owed".

**When the SDK design was published to Logos for review.** A Gateway statement;
see "Statements owed".

## M6: Maker and Taker apps

**Price/amount controls, progress, history and recovery in the apps.** The
controls of PR #21 and the timelines, fill and spread of PR #26 are the desks
in the videos: the Maker composes an offer (sell side, amounts, TTL) and
publishes it; the Taker takes it; every swap row shows its phase, a progress
bar, the countersigned deadlines as countdowns, the amounts and fill, and a
details popup with the swap's own on-chain transactions; recovery paths are the
`*-taker-refund` and `*-maker-refund` videos. Each desk also shows its Node's
own wallet balances (Bitcoin and LEZ) under the account selector.

**The 4 August prototype sign-off.** `docs/m6-prototype-review.md`, "Sign-off
record": decision approved, reviewer "repository owner (explicit chat
approval)", reviewed commit `0abdbc2`, revalidated evidence commit `17573cd`,
date 2026-08-04. That is an internal approval by the repository owner, not an
external review; Gateway should confirm that scope (see "Statements owed").

**Planned ZEC flow against the prototype/service evidence.** A Gateway
statement; see "Statements owed".

## Setup and API follow-up

**Published packages, exact commit, Mac architecture, startup bundle.** The
table at the top. The v0.2.2 release will carry the same shapes from the merged
`main`.

**Basecamp setup path.** The verified path is the supplied VNC environment (the
`basecamp-ui` container). The native path is not covered by this evidence.

**Maker/Taker API feedback (PR #16).** The documented owner APIs are the ones
the desks call. Wallet and capital visibility: `maker_wallet_balances_v1` and
`taker_wallet_balances_v1` report each Node's own Bitcoin wallet (trusted,
pending, immature) and LEZ owner account (balance, nonce); `taker_swap_list_v1`
and `maker_actor_monitor_v1` carry every swap's effects. Operator behaviour:
locks and claims are owner actions on the Taker; the Maker Node locks, claims
and refunds on its own schedule; a refund the Taker admits is driven by the
Node until it lands. `docs/api/README.md` describes the methods and states.

## Defects found and fixed by these runs

All on PR #42, each reproduced on the stack before the fix and re-run after it.

| Area | Defect | Fix |
|---|---|---|
| LEZ v0.2.0 indexer | reset its last-breakpoint id on every open, so after any restart historical reads past the next 100-block boundary failed and a Maker never observed its own escrow | carried source patch `deploy/builder/patches/lez-v0.2.0/0001-indexer-keep-breakpoints-across-reopen.patch` |
| Taker Node | two sell-LEZ locks back to back were prepared at the same account nonce; the sequencer skipped the second pair | one LEZ escrow submission in flight per Node |
| Taker Node | that gate waited for the previous lock in the *finalized* view (the indexer), which the devnet's L1 pauses for minutes at a time; a second sell-LEZ lock pressed during a pause was refused after 100 s | the gate reads the sequencer's account state, the one a submission's nonce is checked against |
| Actor | the exact lookup of an own LEZ refund, and the discovery of the counterparty's, scanned the swap-start window sized for the Maker's lock; the later-refunding leg's refund lands past it | lookups trail the finalized tip, with a full-span fallback |
| Actor | a Maker lock already spent by the Taker's claim read as pending (Bitcoin), and a Maker LEZ funding whose escrow the Taker had claimed could not complete (sidecar refused the "claimed" state) | both count as canonical; the sidecar reports a claimed escrow; an after-funding read on the adapter |
| Actor | a Maker past its cutoff only recovered and never re-observed a lock it had sent in time | the lock is projected before recovery |
| Actor | a Maker restarted just after its cutoff still sent its lock: the projection above could submit a never-sent lock because its freshness read consults the chain clock, which trails the wall clock (Bitcoin's median time by about an hour on mainnet, ten minutes on the local regtest); the Taker then saw a late lock it could neither claim nor refund against and both legs stayed locked | an unsent Maker lock is never sent once the wall clock is past the cutoff, in both directions and for both lock chains; regression test `unsent_maker_lock_stays_unsent_once_the_wall_clock_passes_the_cutoff` |
| Taker Node | a submitted claim was no longer observed once the claim window closed (#32 follow-up) | observed across the window's close |
| Nodes | owner RPC replies capped at 64 KiB; the swap list outgrew it | 4 MiB replies |
| Nodes | the actor printed nothing about a decision not to act, so a stalled refund left no evidence | with `LEZ_BTC_ACTOR_TRACE=1` (passed through by the compose file, off by default) the actor names every effect decision, submission outcome, refused Maker lock and uncertain safety or refund read |
| Node ↔ sidecar | one transport failure refused the Taker's lock | prepare and submit repeated on transport failure |
| Desks | the Maker desk queued a market read every two seconds regardless of the previous one and fell minutes behind the Node; an explicit refresh could be dropped; a refunded Maker leg was labelled "Lock window missed" | one silent read in flight, explicit reads always sent; refunded leg reported first |
| Scripts | the timing profile silently reverted on a rerun; base images pruned with the build cache failed the image build; a desk change did not reach the stack | `gen-config.sh`, `up.sh`, `from-scratch.sh` fixed |

## Known limitations of the local environment

- A Node that has served many swaps answers a Maker market read in one call per
  swap; a desk on such a Node refreshes in seconds rather than instantly. A
  fresh stack (`up.sh --fresh`) or `scripts/reset-swaps.sh` clears the history.
- The LEZ devnet's L1 (bedrock) pauses finality for a few minutes at a time once
  it has run for about a day, then catches up in one step; swaps complete
  through it, and the harness allows five minutes without finalized progress.

## How to reproduce

On an arm64 host with Docker, from a checkout of the commit above:

```
cd deploy
LEZ_TIMING_PROFILE=fast bash scripts/up.sh --fresh      # a clean stack, one unattended run
python3 scripts/node-e2e.py all                          # nine scenarios, Taker sells BTC
python3 scripts/node-e2e.py all --direction TakerSellsLez
bash scripts/ui-e2e.sh all                               # seven scenarios on the real desks
bash scripts/ui-e2e.sh all --direction TakerSellsLez
bash scripts/ui-e2e.sh happy --record                    # the same, filmed with narration
python3 scripts/node-e2e.py regenerated-config           # the #28 follow-ups
python3 scripts/node-e2e.py tampered-config
```

Summaries land in `runtime/e2e/<scenario>[-lez].json`, exported swap evidence
in `runtime/evidence/<swap-id>.json`, videos in `runtime/evidence/videos/`.

## Statements owed by Gateway (not test output)

1. When the SDK design was published to Logos for review before
   implementation (a link to that record).
2. The explicit disposition of the XMR penalty fallback of ADR 0174 for the
   audit (it pays the Maker LEZ without recovering the XMR).
3. Reconciliation of the planned ZEC flow with the M6 prototype/service
   evidence and the current app coverage.
4. Confirmation of the scope of the 4 August prototype sign-off (internal
   approval by the repository owner; who reviewed).
5. Whether a native Mac Basecamp with the catalog modules is a supported test
   path, and if so how it receives the Node sockets and chain configuration.

## Results of the follow-up scenarios

Run on 2026-09-12 on the same stack, after the recordings:

| Scenario | Result | Summary |
|---|---|---|
| regenerated-config (Taker sells BTC) | passed in 229 s: swap taken and locked, configuration rendered again, Taker Node restarted, swap reloaded and completed on both Nodes | `e2e/regenerated-config.json` |
| regenerated-config (Taker sells LEZ) | passed | `e2e/regenerated-config-lez.json` |
| tampered-config | passed in 21 s: one byte appended to the swap's saved `taker-role-config.json`, Taker Node restarted, swap listed as `attention_required`, lock refused as `lock_swap_unknown` | `e2e/tampered-config.json` |
