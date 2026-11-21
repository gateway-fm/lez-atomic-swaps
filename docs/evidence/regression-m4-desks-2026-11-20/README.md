# Monero desk recordings at `86bdb8a4`, 2026-11-20

The three Monero desk journeys of `deploy/scripts/regress.sh full` run once on the Docker devnet with
the Monero regtest chain up (`LEZ_MONERO=1`), three lanes at a time (`LEZ_LANES=3`), recorded
(`LEZ_REGRESS_RECORD=1`, which passes `--record` to `ui-e2e.sh`): one run,
`runtime/regress/20261120T233051Z`, 2026-11-20 23:30:51Z to 23:48:11Z (UTC), on a LEZ chain made
at 23:05Z (`up.sh --fresh-lez`). Then, on the same chain, one Zcash desk journey in ADR 0216's order,
recorded the same way: `runtime/regress/20261120T234825Z`, 23:48:25Z to 23:51:55Z. Each desk is
driven in its party's own Basecamp through `ui-tests/verify.mjs`; each recording is a by-product of
the test that passed while it was recorded.

These replace, for `86bdb8a4`, the recordings of `363df3cf` in
[regression-m4-desks-2026-11-16](../regression-m4-desks-2026-11-16/), which are the record of that
commit: since then the tree took two merges of `main` (the second with ADR 0216), the Monero windows
of `a83bae24` and the desk wording changes.

**Result: Monero 3 of 3 passed, each on its first try**, 0 frozen, 0 new chains, 0 profile switches
(`results.tsv`, `regress.log`); **Zcash 1 of 1 on its first try** (`results-zec.tsv`,
`regress-zec.log`). Timing profile `fast`; the refund journey's log gives "the refund time (720 s after the take)" and
warns "Halfway to the punish time (1000 s after the take)".

| test | lane | seconds | what it did |
|---|---|---:|---|
| `ui:xmr:xmr-happy:TakerSellsLez` | 2 | 237 | The Maker desk publishes Monero offers from its form; the Taker desk takes one; the Taker Node locks its LEZ and the Maker Node funds Monero on their own while the Maker desk's row goes from "Offered · waiting for the Taker's LEZ lock" through "Taker's LEZ lock seen · funding Monero" to "Monero funded · waiting for the Taker's claim"; the Taker desk presses Claim; its row reads "Claim authorised" ("Waiting for the Maker to claim the LEZ (tag 15), which hands you the Monero key"), the Maker's "Claiming LEZ", the Taker's "Sweeping the Monero"; both desks show `completed`. |
| `ui:xmr:xmr-concurrent:TakerSellsLez` | 3 | 328 | Two offers from the Maker desk's form, both taken from the Taker desk one after the other; each claimed from the Taker desk once the Maker desk showed it funded; both completed on both desks. |
| `ui:xmr:xmr-taker-refund:TakerSellsLez` | 1 | 997 | The Maker funds, the Taker desk shows "Your Monero claim is ready" and does not press Claim. Past the refund time the Taker desk shows "Refund window open" with "Refund 50,000 LEZ"; at the warning "Refund window closing"; the Taker desk presses Refund ("LEZ refund sent"); the Maker desk shows "Refund sweep: recovering your Monero"; both desks show `refunded`. |
| `ui:zec:zec-happy:TakerSellsLez` | 1 | 167 | Take-time agreement (no corridor; the Maker CLI publishes the offer on the running Node). The Taker desk takes it; the Taker Node locks its LEZ; the Maker Node funds Zcash on its own ("Locking Zcash", then "Waiting for the Taker's Zcash claim · The Taker's revealing claim is the next step"); the Taker desk presses Claim 1.00000000 ZEC, the revealing claim on Zcash; the Maker Node claims LEZ with the secret it read from that claim and its row shows `completed` while the Taker's row reads "Zcash claim submitted" and then "Waiting for the Maker's LEZ claim"; the Taker's row ends `completed`. |

`logs/` holds each test's output as the runner kept it (colour codes removed; one host path in the
Zcash log replaced by `<checkout>/`): every desk step, its captions and the labels each step
asserted.

## Recordings

`videos/`, silent H.264 at 6 frames per second, 1680×1050, the desks' narration burned in as
subtitles (`ui-tests/record-step.sh`, joined and re-encoded once by `ui-e2e.sh` with the same
settings as the 2026-11-16 set: `record-step.sh` and `ui-e2e.sh`'s join are unchanged since `363df3cf`), kept in git as the
earlier sets are:

| file | bytes | length | sha256 |
|---|---:|---|---|
| `TakerSellsLez-xmr-happy.mp4` | 2,281,234 | 3 min 58 s | `d631da8fae4f7e3cb1a1f5e8b829e471256182274f42b2f6187b3af4a2c77ee2` |
| `TakerSellsLez-xmr-taker-refund.mp4` | 2,561,829 | 4 min 26 s | `7bd4275cabffcbc80dfa91c18b2fa2b4b40c7d4e37fdef4666d6b44e539542df` |
| `TakerSellsLez-xmr-concurrent.mp4` | 3,477,150 | 4 min 38 s | `fecb04592896c28a53da6ea3a1ac651229a01593d3fb06bd42781c3871cc4629` |
| `TakerSellsLez-zec-happy.mp4` | 1,227,774 | 2 min 15 s | `504739babff6e844a003e01224cb7970eb50da0ebfa18fe3b19300e6c2a83585` |

The Monero videos are shorter than those of 2026-11-16 (9 to 14 minutes) because the journeys took
237 to 997 s against 500 to 1,113 s, and the refund journey's waits are the host's, as before.

The Monero captions: the Taker's Claim step reads "Claim 1.000000000000 XMR: the Taker authorises
the Maker's LEZ claim (it releases its withheld claim signature share); the Maker's claim signature
then lets the Taker recover the Monero spend key"; the Maker's wait on `completed` reads "The Taker
authorised the claim on LEZ (it released its withheld claim signature share): the Maker Node claims
the LEZ, and its claim signature lets the Taker recover the Monero spend key; the desk waits for
'completed'"; the refund steps read "The Taker refunded its LEZ: the Maker Node learns the Taker's
Monero share from the final refund and sweeps its Monero back" and "The Maker Node sweeps its Monero
back once the Taker's LEZ refund is final". They match the protocol: the Taker authorises, the Maker
claims the LEZ, the Maker's claim gives the Taker the Monero key. Each renders whole and legible on
one to three subtitle lines.

The Zcash captions follow ADR 0216's order: "Claim 1.00000000 ZEC: the revealing claim spends the
Maker's Zcash HTLC on the transparent pool and discloses the hashlock secret", then "The Taker's
Zcash claim revealed the secret; once it is in a zebra block the Maker Node claims LEZ with it".
Two captions of this journey still name the dealer's corridor although it ran at take time: "Take
offer on the Zcash row: the Taker Node runs the prepared ZEC ceremony with the Maker over Chat and
provisions its Taker actor" and "Inventory refreshed: … the Zcash lot came from the prepared
corridor, nothing is published from the desk" (the log shows the Maker CLI publishing the offer,
"take-time agreement: no corridor"). They are the test driver's narration, not desk text, and are
left as they are in this note.

How the frames were checked: each video was cut into frames every 15 s (Zcash every 12 s) plus the
Zcash video's last frame, tiled four to an image at half size, and every tile was read; the
captions were also read in full from the logs. Nothing private is on screen: the desks show wallet
labels ("Munich Vault 01 · Maker Node", "Zurich Wallet 01 · Taker Node"), the devnet accounts' LEZ
and BTC balances, offer and swap ids, the Maker's offer form (amounts, offer lifetime), the activity
list; no key, seed, cookie, capability string, host user name or host path. The bottom bar is the
Basecamp display's own (workspace and clock). Both desks' activity lists show, at the start of each
journey, the "Node channel unavailable" and "Owner-local Node endpoint is unavailable" lines of the
desks' first reads while their Nodes come up after the recording's store reset, followed by "Node
replied over the owner-local channel". In the Zcash journey the Taker's activity list shows
"Refund 50,000 LEZ is yours to run" while its swap is at `lock_pending → awaiting_maker_lock`; the
recording does not show whether the desk offered a Refund button then.

## Attempts

- `20261120T230801Z` and `20261120T232512Z`, the first recorded runs on this chain: Monero 3 of 3
  (230, 335 and 974 s) and Zcash 1 of 1 (178 s), each on its first try. Not kept: the Maker desks
  listed four Bitcoin and LEZ offers left open on the Maker Nodes by the full regression of the same
  day (the reset before a recording forgets swaps, not offers), which pushed the Maker desk's "My
  orders" row below the visible part of the window in the Monero recordings. The open offers of the
  three Makers were then withdrawn through each Maker Node's `maker_offer_withdraw` (what the
  desk's Withdraw button calls), and both sets were recorded again; before the Zcash run the Monero
  run's own leftover offers were withdrawn the same way. In the kept recordings the Maker desk lists
  only the journey's own two Monero offers (none in the Zcash journey) and shows its rows.
- No try failed.

## Not covered

- The punish row ("Punish available: taking the Taker's LEZ", "Punished") and the Taker's
  "Refund window closed", "The Maker did not claim", "LEZ lock window closed" and "Monero sweep in
  doubt" rows were not seen on a desk; nor was the Maker's "Monero funding in doubt" row.
- The Taker's "Claim queued" row of #130 was not recorded: no desk journey asserts it (the scenario
  that presses Claim at one Zcash confirmation, `LEZ_ZEC_CLAIM_AT_ONE_CONFIRMATION=1`, is a Node
  scenario of `node-e2e.py`), and recording it would need a desk journey written for it.
- One Zcash journey only (happy, TakerSellsLez); the other Zcash desk journeys were not recorded at
  this commit.
- Only TakerSellsLez exists for Monero; regtest only; linux/amd64 only.
