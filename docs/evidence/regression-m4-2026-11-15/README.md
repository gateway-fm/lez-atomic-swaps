# Full regression of `feat/m4-monero`, 2026-11-15

One run of every test of `deploy/scripts/regress.sh full` (157: `regress.sh list full`) on the
Docker devnet with the Monero regtest chain up (`LEZ_MONERO=1`), three lanes at a time
(`LEZ_LANES=3`), without desk recordings. 2026-11-15 16:17:35Z to 21:36:05Z (UTC). It
supersedes the [run of 2026-11-14](../regression-m4-2026-11-14/) (152 tests at `82e1b856`): the
143 Bitcoin and Zcash tests of the [Bitcoin/Zcash full run](../regression-m5-2026-11-12/) plus
14 Monero tests, 13 `node:xmr` scenarios (the eight of 2026-11-14, four outage and restart scenarios
and `mixed-three-pair`) and the desk journey `ui:xmr:xmr-happy`.

**Result: 157 of 157 passed in one run of 318 minutes.** 153 tests passed on their first try.
Three tries were FROZEN (the LEZ chain's finality stopped; below) and passed on try 2 on a new
chain; one, `node:zec:zec-claim-collision:TakerSellsForeign`, was NOT-EXERCISED on try 1 and
passed on try 2. 0 failed, 0 timeouts; 3 new LEZ chains (one after the freeze, one for each
`zec-balance-refusal` test); 3 timing-profile switches. By kind: node:btc 53, node:zec 51,
node:xmr 13, ui:btc 17, ui:zec 22, ui:xmr 1. Test time summed over lanes 43,775 s over 161
tries (43,655 s for the passing tries), of which the 14 Monero tests 11,408 s. The runner
records a FROZEN try as 0 s; the three voided tries had run 38, 38 and 14 minutes.

The same images ran, just before this run and on the same chain, the smoke tier
(`runtime/regress/20261115T145403Z`, 6 of 6) and two targeted runs of both Monero kill walks,
with `xmr-concurrent` (`20261115T150026Z`, 3 of 3) and with `mixed-three-pair`
(`20261115T153938Z`, 3 of 3); those runs are not committed here.

`results.tsv` is the runner's table (test, try, verdict, seconds, finished at, lane, LEZ chains
made so far by this run; the two tests on a chain of their own have no lane columns);
`regress.log` is the runner's log, every try's start and verdict line carrying the LEZ
finalized height and tip. Neither holds a local path. The per-test logs are not committed; the
lines quoted below are from them.

## The three FROZEN tries

The run started on the chain made before the smoke (finalized 2966). At 16:55:45 the runner's
finality watch declared it frozen and voided the three tests then in flight:

    [16:55:45]     finality has stood at 4078 for 60 s (sequencer tip 4113): the chain is frozen, every lane's test is void
    [16:56:07] === node:btc:kill-maker:TakerSellsLez (try 1, lane 3): FROZEN in 0 s; LEZ finalized 4078, tip 4123
    [16:56:09] === node:xmr:xmr-kill-maker:TakerSellsLez (try 1, lane 2): FROZEN in 0 s; LEZ finalized 4078, tip 4124
    [16:56:09] === node:xmr:xmr-kill-taker:TakerSellsLez (try 1, lane 1): FROZEN in 0 s; LEZ finalized 4078, tip 4124
    [16:56:09]     new LEZ chain (1)

The sequencer kept making blocks (tip 4113 → 4124) while finality stood still. All three
started again at 16:59:29 on the new chain and passed on try 2 (lanes 1, 2 and 3; 1,600 s,
2,256 s and 2,253 s).

The likely trigger is our own test. For its sixth kill point, "claim tool running",
`xmr-kill-maker` pauses the shared `lez-sequencer` container (`docker pause`) so the Maker's
tag-15 claim tool waits on it, kills the Maker Node there and unpauses the sequencer. Try 1 of
`xmr-kill-maker` reached that point 75 s before the freeze:

    [16:54:30]   the Maker's claim tool reserved tag15-claim-submission.json and waits on the paused sequencer
    [16:54:30]   kill point 'claim tool running': taker claim_in_progress (actor awaiting_maker_claim), maker claim_evidence_available
    [16:54:30]   killing lez-l2-maker-node with SIGKILL

The previous full run, `20261115T091025Z` at `48105e1d`, froze in the same sequence: that line
at 09:45:47, "finality has stood at 2984 for 60 s … the chain is frozen" at 09:47:00, the same
three tests voided. On try 2 of this run the same pause (17:34:43) did not freeze the chain. In
the 60 runs of the runner on 2026-11-14 and -15 this pause ran eleven times and the chain froze
after two of them; those two are the only freezes in those 60 runs. This is a correlation
from two runs, not a cause shown: the devnet's finality freeze is a known upstream defect of the
LEZ devnet (logos-blockchain#3643 in the deployment guide; it struck at random, with no pause,
in runs before the Monero scenarios existed), and the runner exists to void and retry a frozen
try. A change that holds the claim tool on the Maker's own sequencer forwarder instead of
pausing the shared sequencer (`test(node-e2e): xmr-kill-maker holds the claim tool on the
Maker's own sequencer forwarder`) was written during this run and is not in it. It landed after
this run: the three tests the freeze voided, run together on three lanes with that change, passed
three times in a row with all six Maker kill points reached and no frozen try (runs
`20261115T213634Z`, `20261115T221718Z`, `20261115T225347Z`; their results are not committed).
Three runs do not show that the freeze cannot recur.

## The NOT-EXERCISED try

`node:zec:zec-claim-collision:TakerSellsForeign` needs two Taker swaps whose LEZ transactions
collide on the owner account's nonce. On try 1 (lane 3, 120 s) the collision did not happen and
the scenario said so instead of passing:

    FAILED: NO-COLLISION: A completed without a superseded claim (C's lock was in a block before
    A's claim was planned, or C's lock lost the nonce instead: … try again

The runner recorded it NOT-EXERCISED and ran it again; try 2 (lane 1, 115 s) produced the
collision ("revealing LEZ claim superseded (planned nonce 92, live 93); re-plan 1 at nonce 93")
and passed. The test cannot force its race; the 2026-11-14 run needed a second try as well.

## The Monero tests

All 14 are TakerSellsLez (the only Monero direction), against Monero regtest on the devnet
(per lane a `monerod` with funding, Maker and Taker `monero-wallet-rpc`), `fast` profile. Each
swap's windows, as every log prints them: "the Maker funds by +260 s, the Taker refunds from
+700 s, the Maker punishes from +1000 s". Lot 1 XMR against 50,000 LEZ units. The two kill walks
are their passing try 2.

| test | lane | s | what its log shows |
|---|---|---|---|
| `node:xmr:xmr-happy` | 1 | 264 | One swap through both Nodes: "settled completed: LEZ moved {'maker': 50000, 'taker': -50000} as both Nodes say"; the Taker's wallet received 0.9981868 XMR (the lot less fees). |
| `node:xmr:xmr-taker-refund` | 2 | 860 | The Maker funds, "the Taker does not claim it"; "Taker refund admitted", "settled refunded: LEZ moved {'maker': 0, 'taker': 0}"; the Maker's wallet swept its Monero back (received 0.9981868 XMR), the Taker's received nothing. |
| `node:xmr:xmr-maker-never-funds` | 1 | 804 | "stopping the Maker Node before its Monero funding"; "Taker refund admitted with the Maker down", LEZ back to the Taker, "no Monero moved: the Maker's wallet sent nothing, the Taker's received nothing". |
| `node:xmr:xmr-punish` | 2 | 1,110 | The Taker Node is stopped once it has verified the Maker's Monero and neither claims nor refunds. "the Maker's LEZ owner HwzsKWbe4kNt… has nonce 13 before the swap", then "the Maker punished; its LEZ owner's nonce moved 13 → 14"; LEZ moved {'maker': 50000, 'taker': -50000}; neither wallet received Monero, the funding "stays at the shared address: neither party received it". The Taker, back up, reads `attention_required` (`xmr_refund_window_closed`, actor `punish_open`). |
| `node:xmr:xmr-concurrent` | 3 | 422 | Two offers taken one after the other, swaps `c4c2ca7ec983` and `9f9782000d69` live together; both completed, "settled both: LEZ moved {'maker': 100000, 'taker': -100000}", the Taker's wallet received two outputs of 0.9981904 XMR. |
| `node:xmr:xmr-kill-taker` | 3 | 2,253 | The Taker Node SIGKILLed at 7 points, a fresh swap each: "taker killed at 7 points: after the take → completed; lock in flight → completed; locked → completed; Monero verified → completed; claim authorised → completed; waiting for the Maker's claim → completed; sweeping → completed". Each swap: Taker 3 LEZ transactions, Maker none, one Maker funding, one Taker sweep. |
| `node:xmr:xmr-kill-maker` | 2 | 2,256 | The Maker Node SIGKILLed at 6 points: "maker killed at 6 points: after the take → completed; lock observed → completed; funding in flight → completed; funded → completed; claim in flight → completed; claim tool running → completed". At "claim tool running" the tool held its reserved, empty evidence file on the paused sequencer when killed; "the Maker drove its stopped claim again and recorded its one tag 15". Each swap: Taker 3 LEZ transactions, Maker none, one funding, one sweep. No point ended refunded and no funding was read in doubt in this run (on 2026-11-14 "funding in flight" ended refunded). |
| `node:xmr:xmr-taker-refund-kill` | 3 | 877 | The Taker Node SIGKILLed right after "Taker refund admitted"; back up at `refund_available`, the refund settled ("LEZ moved {'maker': 0, 'taker': 0}"), the Maker's wallet swept its Monero back (0.998188 XMR), the Taker's received nothing. |
| `node:xmr:xmr-monerod-down` | 1 | 695 | Before a take: the lane's `monerod` stopped for 4 s; "the Maker withdrew offer … and refuses to publish on Monero: chain dependency is unavailable for route", the Taker refuses a take (`chain_unavailable`, pair Monero), "Bitcoin's and Zcash's available"; started again, both Nodes read the route available and a swap completes. Mid-swap: stopped right after a take, "the Maker sees the lock final with monerod down (taker_lock_confirmed)", "monerod was down 113 s (the funding cutoff is 146 s away)"; the swap completed, LEZ {'maker': 50000, 'taker': -50000}, the Taker received 0.9981856 XMR. |
| `node:xmr:xmr-wallet-restart` | 2 | 540 | Two swaps. The Maker's `monero-wallet-rpc` restarted after the take (back with "No wallet file" open), the swap completed; the Taker's restarted twice, while watching for the Monero and before the sweep, the swap completed. Each: LEZ {'maker': 50000, 'taker': -50000}, the Taker received 0.9981856 XMR. |
| `node:xmr:xmr-outage-before-take` | 1 | 319 | Logos Chat and Delivery cut before the take (7 s): both Nodes degraded, "the Taker refused the take: Taker dependency unavailable (authenticated_delivery_unavailable)", no swap on either Node, the offer unchanged; restored, "the same request e2e-take-1794768111 took offer-e2e-xmr-1794768111" and the swap completed. |
| `node:xmr:xmr-cut-after-lock` | 2 | 296 | "the Taker's owner nonce moved 29 → 30: its LEZ lock is out; cutting Logos Chat and Delivery"; cut 175 s, through the funding, the claim, the Maker's LEZ claim and the sweep: "the cut held: 80 samples of the Maker's Chat socket, none with a connection", "the Monero swap completed with both off-chain dependencies cut". |
| `node:xmr:mixed-three-pair` | 3 | 442 | One Maker and one Taker, three TakerSellsLez swaps at once: Monero `68256968d7b2`, Bitcoin `9e8f02f16420`, Zcash `zec-dead542d` (take-time agreement). "all three taken, all three in flight", then "all three completed; LEZ transactions signed by the owner accounts meanwhile: {'maker': 1, 'taker': 7}"; the Taker received 0.9981892 XMR. |
| `ui:xmr:xmr-happy` | 2 | 270 | A Monero swap clicked through both desks: the Maker desk publishes Monero offers from its form; the Taker desk takes one (the Taker Node reserves, plans, signs and activates); the Taker Node locks its LEZ while the Maker desk shows the row funded; the Taker desk performs `claim_xmr`; the Maker Node claims the LEZ and the Taker Node sweeps the Monero; both desks show the swap `completed`. Its evidence export is skipped ("a Monero swap's record is node-e2e.py's"). |

## How the Zcash agreements were made

Of the 73 Zcash tests, 69 formed their agreement at take time (`zec_reserve_v1`: the Maker
publishes, the Taker takes, the two Nodes compose and sign it, each with its own key; their logs
say "take-time agreement"), which is the harness default on this branch
(`LEZ_ZEC_AGREEMENT=take-time`); both tries of `zec-claim-collision:TakerSellsForeign` did, and
so did the Zcash swap of `mixed-three-pair`. Four used the test fixture
`deploy/tests/fixtures/zec-catalog-fixture.sh`, which holds both parties' keys:
`zec-regenerated-catalog` and `zec-tampered-catalog` in both directions, because they test the
prepared catalog itself (their logs say "take-time agreement not used here").

## What changed since the 2026-11-14 run and why

`82e1b856` is now `6c3bf812` (the same commit rebased; `git diff 82e1b856 6c3bf812` is exactly
`git diff 88d00005 a6d24501`, the two Zcash commits below). `git log 6c3bf812..305b32e0` holds
34 commits, plus the two on `feat/m5-gaps`. Two full runs came between, each found failures, and
each failure was fixed before this run:

- `20261115T033448Z` at `7068b323`: 154 of 157, 0 frozen. Not passed:
  `node:zec:zec-kill-maker:TakerSellsLez`, `node:xmr:mixed-three-pair:TakerSellsLez` (its Zcash
  swap) and `node:zec:zec-offline-after-lock:TakerSellsForeign`.
- `20261115T091025Z` at `48105e1d` (the same branch rebased onto the two Zcash fixes): 156 of
  157, 3 frozen tries (the freeze above). Not passed: `node:xmr:xmr-kill-taker:TakerSellsLez`.

The changes, by theme, with the run that exposed each where one did
(the run directories and the supervisor's log that diagnosed them are not in the repository):

- **Monero route health.** The probe asked the role's `monero-wallet-rpc`, which the Node's own
  tools keep busy for seconds; one slow answer read the route unavailable and the Maker withdrew
  a fresh offer for good while it was being taken. It asks `monerod` only (`98d4d269`; found in
  `20261114T123914Z`, listed as not landed in the 2026-11-14 note).
- **`xmr-kill-maker` and a funding left in the pool.** A funding tool killed after its transfer
  and before its mining left the funding unmined; the scenario now counts fundings per swap and
  mines one its kill left (`3a8dd9d6`; `20261114T123914Z`, `20261114T125704Z`).
- **New scenarios.** `xmr-monerod-down`, `xmr-wallet-restart`, `xmr-outage-before-take`,
  `xmr-cut-after-lock` and `mixed-three-pair` (`e38038a1`…`efcf3d52`, `7e33c03d`). The first of
  them found that a regtest `monerod` deletes its chain when restarted without `keep-fakechain`
  (`a05a99d3`; `20261114T185956Z`).
- **Owner-account nonces and the shared lock gate.** Each role signs all its pairs' LEZ
  transactions with one owner account at the live nonce. `mixed-three-pair` showed a Bitcoin lock
  taking the nonce of the Monero lock's funding (`20261114T191308Z`): the Taker now sends the
  funding as soon as the initialisation is in a block (`ef8218be`). Then the Bitcoin lock,
  planned while the Monero pair was pending, never landed (`20261114T200648Z`): the Monero lock is
  planned under the Taker Node's LEZ lock gate that Bitcoin locks take (`527eaf5f`), only at a
  nonce that stood still since the gate's last look, since a Zcash lock moves the account without
  the gate (`42f97483`), and a Monero pair that dies gives the gate back at its cutoff
  (`1226ea90`); these two from reading the code, no run recorded. What is still uncoordinated is
  in the deployment guide's "Owner nonces" (`70af6c56`).
- **The Maker's claim margin and wider Monero windows.** The Maker claims only on a claim
  authorization it sees final before R − (R − C)/2. Under three lanes one swap's Maker drives came
  37 to 84 s apart, and `xmr-concurrent` and `mixed-three-pair` fell back to the refund
  (`20261114T205631Z`). The Maker now looks for the claim in the drive that reconciles its funding
  (`8d39a3f7`), and the windows hold two drives at 90 s each: `fast` +260/+700/+1000 s (was
  +260/+360/+480), `refund` +120/+560/+815, `local` +1820/+2260/+2800 (`8574da70`).
- **The Taker's first Monero balance read.** Under load the first read after a lane was
  recreated failed once in the client (`20261114T225353Z`, `mixed-three-pair` before its take);
  the read is made once more and the adapter's errors keep their cause (`ee1fa64a`).
- **The lost-punish re-plan.** A tag 17 whose owner nonce another transaction took could never
  land, and after P nothing else moves the escrow. The sidecar now plans the same punishment
  again at the live nonce, at most five times (`d6f15ee1`; found by reading the code).
- **Scan cursors.** With P at +1000 s the Maker's tag-17 observer scanned from the Taker's lock,
  more than one 30 s run gets through, and `xmr-punish` never ended punished
  (`20261115T010526Z`): it starts at the refund scan's cursor (`a149fdc4`), and the tag-15 and
  tag-17 observers resume where the last run stopped (`6a3502b3`, `0a507dc1`).
- **CI and packaging on the Monero head** (`28844a06`, `f03788e9`, `dfaa1627`, `78a41ad0`,
  `98827388`, `d6f37f28`, `48105e1d`): the Maker recovery fixture, a licence allowance, Monero's
  image built in `up.sh`, no Monero image in a release bundle, Monero executables in the Linux
  role packages only, a gitleaks allowance for the test vectors' fixed keys, the fuzz matrix.
- **The Zcash actor's database binding** (on `feat/m5-gaps`, `a6d24501`). The Zcash actor bound
  the role state database and the bridge journal by their size and times, which a legitimate
  writer changes between two `lstat`s; it compares the device and inode now. Exposed in
  `20261115T033448Z`: `mixed-three-pair`'s Zcash swap read `actor_deployment_invalid`, and
  `zec-offline-after-lock:TakerSellsForeign` read `attention_required` once.
- **The settlement read** (on `feat/m5-gaps`, `f766c9d0`). The harness read LEZ balances from the
  indexer (finalized state) right after both Nodes said completed, which on the devnet they say
  on inclusion; it now waits until LEZ is final through the sequencer's tip. Exposed in
  `20261115T033448Z`: `zec-kill-maker:TakerSellsLez` read "{'maker': 0, 'taker': -50000}"
  although the Maker's claim had landed. A harness defect, not a product one.
- **The Digest two-request cause of transient Monero RPC failures, and its workaround.** In
  `20261115T091025Z` `xmr-kill-taker` failed: the Maker's funding was broadcast, its one
  `generateblocks` call failed in the client ("error decoding response body: expected value at
  line 1 column 1"), nothing else mines on regtest, and the swap ran into the refund window. With
  two earlier single-call failures (a `get_address`, a `get_balance`) it fits one cause, read from
  the code and not reproduced on the stack: `monero-rpc` 0.5.1 sends each Digest call as two
  requests through a connection pool, `monerod` keeps the Digest nonce per TCP connection, and
  the second leg on another connection gets the HTML 401 page, which the client decodes as JSON.
  The crate builds its own client, so the cause is worked around, not removed: `generateblocks`
  is made up to three times (`2f194443`); read-only calls once more after a client failure, never
  a transfer or sweep, which could pay twice (`abbccd6c`); on regtest the Maker's funding and
  refund-sweep observers mine a transfer they find unconfirmed in the pool (`c89a5d17`); and a
  scenario that needs the funding fails 90 s after the Maker reads it in doubt, saying so
  (`305b32e0`).

## Source and images

Tests and images from `305b32e0` (branch `feat/m4-monero`: 122 commits on `feat/m5-gaps` at
`a6d24501`), tree clean (`regress.log` prints the commit and "tree dirty: 0"). Trees at that
commit (`git rev-parse 305b32e0:<directory>`):

| directory | tree |
|---|---|
| `crates` | `95c159fff9ca10f6fa9448973a6507eff01e8f48` |
| `compat` | `1bf92ed1ea6063d39caa233d097290b5e4cba590` |
| `apps` | `c434c3d87ceaa23293012962d5d7ffe794590e65` |
| `modules` | `36bcbc0f2c8115fe97953163fab70ede08126ef3` |
| `deploy` | `3f3ce542fffa8bee3e9ca294c8e86a4fb74616d4` |
| `packaging` | `9e605b7dfb43be866bf8c4f855a4f7469811c871` |

Built just before the smoke, from that commit (the Node binaries, the staged packages, then the
`maker-node`, `taker-node`, `basecamp-ui` and `network` images), and not rebuilt during this run;
local linux/amd64 ids with each image's creation time, as the build log printed them. The
`basecamp-ui` and `network` images keep the creation time of an earlier build; their ids
differ from the 2026-11-14 run's.

| image | id | created |
|---|---|---|
| `lez-maker-node:local` | `sha256:63d5977be71d8c342298afcdb447e4bf62baf6b3032ad64310af338825d9756e` | 2026-11-15 14:50Z |
| `lez-taker-node:local` | `sha256:952f034722b106f1416d891090ee950346a2d498b700e37cb79823dad5e46a5f` | 2026-11-15 14:50Z |
| `lez-basecamp-ui:local` | `sha256:6cce4abe9ee5c566c59ca7d260d4e2fc7a1d0eb189aa177997a97afc6584c2ef` | 2026-11-14 11:55Z |
| `lez-network-module:local` | `sha256:829bd8ad0ae6f87f42aa9c7bc3f658c1e71c84d8a0d868b05e1ceb1ef4c505fb` | 2026-11-14 13:37Z |
| `lez-services:local` | `sha256:021a7928e64602ce785ce464a83df7e9c27588391c24f6fd3d048489f7d52bef` | 2026-11-13 18:33Z |
| `lez-zebra:local` | `sha256:42d64e9a9e85dbd24911f26b2676f4ca07f3c7d910e80dfb99dfb4ca49ecc101` | 2026-11-11 11:50Z |
| `lez-bitcoin-core:local` | `sha256:dadb2fd79b64a6e20612440c90146d6501a1b3d2039408760e93d48c33619cc5` | 2026-11-11 15:01Z |
| `lez-btc-miner:local` | `sha256:4a33449ce116b239641c7932c63845e2d7c9a54e0d00f39d19688e7c3e403473` | 2026-11-11 15:01Z |
| `lez-monerod:local` | `sha256:db4f4b5620a34a5634bd31d4ab9fc8f92b3ce00139f586828287f41e3ce1d9cf` | 2026-11-13 15:13Z |

`lez-services`, `lez-zebra`, `lez-bitcoin-core`, `lez-btc-miner` and `lez-monerod` are the same
images as in the 2026-11-14 run; `lez-services` and `lez-network-module` carry the same
write-ahead-log bound patches (`deploy/builder/patches/README.md`).

## Not covered

- Monero beyond an offline regtest chain: no stagenet or mainnet; x86_64 (linux/amd64) only.
- No systemd packaging, macOS package or desk recordings for Monero. (Since this run the Maker's
  systemd unit takes a Monero role from an optional drop-in, checked statically and with stub
  programs, not run in a VM; the Taker unit has none.)
- Compute units for the five Monero escrow instructions (tags 13–17) were not measured in this
  run; they were measured afterwards against the deployed program, outside the stack, in
  [docs/lez-compute-units.md](../../lez-compute-units.md#the-xmr-path-tags-1317).
- On a real desk only the Monero happy journey is clicked through; the refund, punish and
  in-doubt rows were not seen on a desk.
- Proven by unit tests only, not on the stack: the lost-nonce punish re-plan (no scenario makes
  another owner transaction take tag 17's nonce); the switch of a Maker's stopped claim to the
  refund or the punishment (Claim → Refund, Claim → Punish); a claim submitted but not recorded;
  the regtest observer mining a funding or refund sweep left in the pool (no transfer was left
  unmined in this run). Whether the read and mining retries fired in this run was not checked:
  the Node logs are not kept with the run.
- A transfer or sweep that hits the Digest failure is not made again; the swap is left in doubt
  (on regtest the observer mines what reached the pool). The topology check's own Monero client
  does not go through the retry.
- Owner-account nonces: the Taker's tag 14 is signed without coordination with its other pairs'
  transactions (a lost one falls back to the refund; tag 16 is signed by the swap's own refund
  authority and cannot collide); the Maker's tag 17 is not coordinated beyond the re-plan; a
  Zcash lock sent within the same LEZ block as a Monero lock is planned can still take its nonce.
- Two Bitcoin-adjacent defects have fixes prepared but not landed, so they are not in this run:
  the Maker's Bitcoin claim journaled `unknown` before its broadcast
  (`node:btc:offline-after-claim:TakerSellsForeign`, 2026-11-13; `agent/m5-btc-ready-retry`), and
  the Maker's check of its Bitcoin actor's state database by size and times
  (`actor_deployment_invalid`, `ui:btc:happy:TakerSellsLez`, 2026-11-13;
  `agent/m5-actor-state-bind`). Both tests passed here, which does not show the races are gone.
- Two hosts: the Logos-transport tests (`logos-happy`, Bitcoin and Zcash) run both Basecamps on
  one host with a private Delivery network; the public Logos network (`logos.test`) did not
  deliver Chat reliably when measured (`deploy/README.md`, "Two transports").
- A swap under the systemd units is a separate run (`docs/evidence/maker-systemd-2026-11-12/`).
- macOS, linux/arm64, and any public network are untested on this head.
