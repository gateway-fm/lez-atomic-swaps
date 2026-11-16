# Monero desk recordings, `feat/m4-monero`, 2026-11-16

The three Monero desk journeys of `deploy/scripts/regress.sh full` run once on the Docker devnet with
the Monero regtest chain up (`LEZ_MONERO=1`), three lanes at a time (`LEZ_LANES=3`), recorded
(`LEZ_REGRESS_RECORD=1`, which passes `--record` to `ui-e2e.sh`): one run,
`runtime/regress/20261116T010314Z`, 2026-11-16 01:03:14Z to 01:21:00Z (UTC), on a LEZ chain made
for it at 00:23Z (`up.sh --fresh-lez`). Each desk is driven in its party's own Basecamp through
`ui-tests/verify.mjs`; each recording is a by-product of the test that passed while it was
recorded, as for the [Zcash set](../regression-2026-11-12/videos/).

**Result: 3 of 3 passed, each on its first try**, 0 frozen, 0 new chains, 0 profile switches
(`results.tsv`, `regress.log`). The tests ran at `20f8972a`; the images were built from
`b10f32bb`, whose `crates`, `compat`, `apps` and `modules` trees are the same (`20f8972a` changes
`deploy/` only: the two new journeys and their catalogue entries). Timing profile `fast`: the
Maker funds Monero by +260 s after the take, the Taker refunds from +700 s, the Maker punishes from
+1,000 s.

| test | lane | seconds | what it did |
|---|---|---:|---|
| `ui:xmr:xmr-happy:TakerSellsLez` | 3 | 436 | The Maker desk publishes Monero offers from its form; the Taker desk takes one; the Taker Node locks its LEZ and the Maker Node funds Monero on their own while the Maker desk shows the row funded; the Taker desk presses Claim; the Maker Node claims the LEZ, the Taker Node sweeps the Monero; both desks show `completed`. |
| `ui:xmr:xmr-concurrent:TakerSellsLez` | 2 | 646 | Two offers from the Maker desk's form, both taken from the Taker desk one after the other (the second while the first's LEZ lock ran); each claimed from the Taker desk once the Maker desk showed it funded; both completed on both desks. |
| `ui:xmr:xmr-taker-refund:TakerSellsLez` | 1 | 1,002 | The Maker funds, the Taker desk shows the claim (label "Your Monero claim is ready") and does not press it. Past the refund time the Taker desk shows "Refund window open"; halfway to the punish time it shows "Refund window closing" with the Refund button; the Taker desk presses Refund; the Maker desk shows "Refund sweep: recovering your Monero"; both desks show `refunded`. |

`logs/` holds each test's output as the runner kept it (colour codes removed): every desk step,
its captions and the labels each step asserted.

## Recordings

`videos/`, silent H.264 at 6 frames per second, 1680×1050, the desks' narration burned in as
subtitles (`ui-tests/record-step.sh`, joined and re-encoded once by `ui-e2e.sh`), kept in git as
the Zcash set is:

| file | bytes | length | sha256 |
|---|---:|---|---|
| `TakerSellsLez-xmr-happy.mp4` | 2,936,605 | 7 min 57 s | `f4f213a2ab0373357c6d54e2edcd7582469b5e86cd9f015a666d8c2d3aa320f9` |
| `TakerSellsLez-xmr-taker-refund.mp4` | 3,169,479 | 7 min 27 s | `7033da59c19dabddd176d7eebe51a6fa723509d02c81c38a2f9bd0a51140f979` |
| `TakerSellsLez-xmr-concurrent.mp4` | 4,585,820 | 10 min 31 s | `52a90b31049d30b7eaba80e6c79b1f1ae421c1b0727f9f39df747c2d2a443754` |

- `xmr-happy`: the Maker desk's offer form (Monero, 1 XMR for 50,000 LEZ), the take on the Taker
  desk, the Maker's row from "Taker's LEZ lock seen · funding Monero" to "Monero funded · waiting
  for the Taker's claim", the Taker's Claim press, its row through "Claim authorised" and "Sweeping
  the Monero", both rows `completed`.
- `xmr-taker-refund`: the abandonment and refund case of the pair's one direction. The waits up to
  the refund time and up to the warning are the host's, so the video holds no idle minutes:
  the funded rows, the unpressed Claim, "Refund window open", "Refund window closing", the Refund
  press, the Maker's "Refund sweep: recovering your Monero", both rows "Refunded".
- `xmr-concurrent`: two Monero rows open on both desks at once, both claimed, both completed. The
  second swap's Maker row reads "Monero funding in doubt" for about 80 s (01:10:41Z to 01:12:03Z)
  before it moves on to "Claiming LEZ": the Maker Node's funding of that swap went out at
  01:10:39Z, 11 s after the agreement's funding cutoff by the wall clock (the funding tool is gated
  by the finalized LEZ clock, which runs behind it), its observer then found the funding at depth,
  and the swap completed. This is the in-doubt row of the Maker desk on a real desk, reached by the
  timing of the run, not provoked.

Checked frame by frame at 30 s intervals and at every step boundary: nothing private is on screen.
The desks show what the Zcash recordings show: wallet labels, LEZ and BTC balances of the devnet
accounts, offer and swap ids, the activity list; no key, cookie, capability string, host user name
or host path. The bottom bar is the Basecamp display's own (workspace and clock).

## Earlier try

An earlier run of the same three tests at `7980ec77` (`runtime/regress/20261116T002355Z`, not
committed; it differs from `20f8972a` only in that the refund journey did not yet assert the
Maker's refund-sweep label) passed `xmr-happy` and `xmr-taker-refund` and **failed
`xmr-concurrent`**: the Maker Node's Monero funding of the first swap failed in its tool
("verify the exact shared Monero output: typed Monero RPC operation `get_block_by_hash` failed:
… error decoding response body", the `monero-rpc` Digest failure of
`crates/xmr-monero-adapter`'s topology client, which the read retries do not cover) and backed
off; the observer reconciled that funding 39 s later, but the supervisor reached the second
swap's funding only after that swap's funding cutoff, so the second swap ended `expired` on the
Maker, and the Maker desk's wait for that swap's funded row timed out after 30 minutes (the
journey does not refund; the Taker's LEZ of that swap stayed locked past its punish time, on that
devnet chain). Not retried within that run; the run above is the next one.

## Not covered

- The punish row ("Punish available: taking the Taker's LEZ", "Punished") and the Taker's
  "Refund window closed", "The Maker did not claim", "LEZ lock window closed" and "Monero sweep in
  doubt" rows were not seen on a desk. The Maker's "Monero funding in doubt" row was seen only as
  described above.
- The Maker desk's row past the refund time still reads "Monero funded · waiting for the Taker's
  claim", as the Maker Node reports the same phase (`both_legs_locked`) before and after it; the
  Taker can no longer claim then.
- The Maker desk's caption for the wait on `completed` ("claims LEZ with the secret the Taker
  revealed") is the Bitcoin and Zcash wording; for Monero the Maker's claim hands the Taker the
  Monero key. It is a caption of the test driver, not desk text.
- Only TakerSellsLez exists for Monero; regtest only; linux/amd64 only.
