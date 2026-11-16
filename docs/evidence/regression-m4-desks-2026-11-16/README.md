# Monero desk recordings, `feat/m4-monero`, 2026-11-16

The three Monero desk journeys of `deploy/scripts/regress.sh full` run once on the Docker devnet with
the Monero regtest chain up (`LEZ_MONERO=1`), three lanes at a time (`LEZ_LANES=3`), recorded
(`LEZ_REGRESS_RECORD=1`, which passes `--record` to `ui-e2e.sh`): one run,
`runtime/regress/20261116T064112Z`, 2026-11-16 06:41:12Z to 07:01:07Z (UTC), on a LEZ chain made
for it at 06:39Z (`up.sh --fresh-lez`). Each desk is driven in its party's own Basecamp through
`ui-tests/verify.mjs`; each recording is a by-product of the test that passed while it was
recorded, as for the [Zcash set](../regression-2026-11-12/videos/).

**Result: 3 of 3 passed, each on its first try**, 0 frozen, 0 new chains, 0 profile switches
(`results.tsv`, `regress.log`). The tests and the images are at `363df3cf`. Timing profile `fast`:
the Maker funds Monero by +350 s after the take, the Taker refunds from +790 s, the Maker punishes
from +1,090 s (moved from +260, +700 and +1,000 s by `fede8171`, below).

| test | lane | seconds | what it did |
|---|---|---:|---|
| `ui:xmr:xmr-happy:TakerSellsLez` | 2 | 500 | The Maker desk publishes Monero offers from its form; the Taker desk takes one; the Taker Node locks its LEZ and the Maker Node funds Monero on their own while the Maker desk shows the row funded; the Taker desk presses Claim; the Maker Node claims the LEZ, the Taker Node sweeps the Monero; both desks show `completed`. |
| `ui:xmr:xmr-concurrent:TakerSellsLez` | 3 | 750 | Two offers from the Maker desk's form, both taken from the Taker desk one after the other (the second while the first's LEZ lock ran); each claimed from the Taker desk once the Maker desk showed it funded; both completed on both desks. |
| `ui:xmr:xmr-taker-refund:TakerSellsLez` | 1 | 1,113 | The Maker funds, the Taker desk shows the claim (label "Your Monero claim is ready") and does not press it. Past the refund time the Taker desk shows "Refund window open"; halfway to the punish time it shows "Refund window closing" with the Refund button; the Taker desk presses Refund; the Maker desk shows "Refund sweep: recovering your Monero"; both desks show `refunded`. |

`logs/` holds each test's output as the runner kept it (colour codes removed): every desk step,
its captions and the labels each step asserted.

## Recordings

`videos/`, silent H.264 at 6 frames per second, 1680×1050, the desks' narration burned in as
subtitles (`ui-tests/record-step.sh`, joined and re-encoded once by `ui-e2e.sh`), kept in git as
the Zcash set is:

| file | bytes | length | sha256 |
|---|---:|---|---|
| `TakerSellsLez-xmr-happy.mp4` | 3,889,391 | 9 min 17 s | `d7d220476af31f7b5e5e97bc92ac85a180b1fd4aea0f192774fe304ee4de2c61` |
| `TakerSellsLez-xmr-taker-refund.mp4` | 3,938,809 | 9 min 14 s | `91ddd661218441dcbed7434e6d194f17ddb31e272e0633276f097a194b5a3b4c` |
| `TakerSellsLez-xmr-concurrent.mp4` | 6,318,754 | 13 min 33 s | `c3f77ea48de1202860ab53bd98197a2242f1e5f87650aa6b88e6413e852eed1a` |

- `xmr-happy`: the Maker desk's offer form (Monero, 1 XMR for 50,000 LEZ), the take on the Taker
  desk, the Maker's row from "Taker's LEZ lock seen · funding Monero" to "Monero funded · waiting
  for the Taker's claim", the Taker's Claim press, its row through "Claim authorised" and "Sweeping
  the Monero", both rows `completed`.
- `xmr-taker-refund`: the abandonment and refund case of the pair's one direction. The waits up to
  the refund time and up to the warning are the host's, so the video holds no idle minutes:
  the funded rows, the unpressed Claim, "Refund window open", "Refund window closing", the Refund
  press, the Maker's "Refund sweep: recovering your Monero", both rows "Refunded".
- `xmr-concurrent`: two Monero rows open on both desks at once, both funded, both claimed, both
  completed; neither went through "Monero funding in doubt" in this run.

The Monero captions are the corrected ones (`05e46694`): the Taker's Claim step reads "the Taker
authorises the Maker's LEZ claim (it releases its withheld claim signature share); the Maker's
claim signature then lets the Taker recover the Monero spend key", the Maker's wait on `completed`
reads "the Maker Node claims the LEZ, and its claim signature lets the Taker recover the Monero
spend key". Both render whole and legible on two or three subtitle lines.

Checked at 30 s intervals and on the claim and refund steps: nothing private is on screen. The
desks show what the Zcash recordings show: wallet labels, LEZ and BTC balances of the devnet
accounts, offer and swap ids, the activity list; no key, cookie, capability string, host user name
or host path. The bottom bar is the Basecamp display's own (workspace and clock). The Taker desk
shows "Node degraded · LEZ indexer unreachable" for a moment in the refund journey (the Node's
health read; cleared on its next poll).

## Findings, and what was done about them

- **The Maker desk's caption** for the wait on `completed` used the Bitcoin and Zcash wording
  ("claims LEZ with the secret the Taker revealed"). Fixed in the test driver (`05e46694`); these
  recordings carry the Monero wording. The Maker desk's own row text for the claim ("…which hands
  the Taker the Monero key") is desk text and unchanged.
- **A Maker funding sent after the funding cutoff C** (the earlier recording: the transfer left
  0.37 s after C) is the documented design, not a defect: only the Maker enforces C, the Taker
  takes the exact output whenever it appears before R, and a late funding shortens only the
  Maker's claim window ([protocol design](../../milestone-1/protocol-design.md#xmrlez), the
  paragraph on C).
- **The first try of 2026-11-16 (`20261116T002355Z`) failed `xmr-concurrent`** on a Monero block
  read whose text error the read retry did not recognise (monero-rpc 0.5.1's `get_block` turns the
  Digest decode failure into "Can not fetch block: …"): the first swap's funding tool failed after
  its transfer and the Maker reached the second swap's funding after its cutoff. Fixed in
  `a1c0b915` (that text counts as one transport failure; the topology client's reads are retried
  too).
- **Two Monero swaps at once failed often under three-lane load** (`ui:` and
  `node:xmr:xmr-concurrent` failed in three of four runs on 2026-11-16 before these fixes, run
  beside `node:xmr:mixed-three-pair`). Causes, each fixed in its own commit:
  - The sidecar's Monero Stage-A planner verified both role packets' DLEQ proofs unoptimized:
    17 s of CPU per run, run once by the Maker's R1 and once by the Taker's R2. The Maker's R1 took
    24 to 41 s, past the Taker's 30 s Chat deadline ("initiation_execution_unavailable"), and a
    take used 60 to 114 s of the funding window. With the crypto kernels optimized in the
    sidecar's dev build (`ebf98df5`; 2.5 s a run, identical output) R1 took 6 to 13 s and takes 33
    to 86 s.
  - The second of two swaps needs three of the Maker's actor drives before its funding (the Maker
    Node's one worker alternates the two swaps; one swap's drives came 69 to 103 s apart), and a
    funding drive started as late as +258 s against C = +260 s. C is now +350 s on `fast`
    (`fede8171`); R and P move with it; Bitcoin and Zcash values are unchanged.
  - Under that load the Maker's activation round (Stage B, the swap's sidecar, its actor) outlasted
    the Taker's 30 s deadline twice (`20261116T043912Z`, `20261116T061219Z`): the Taker gave the
    take up while the Maker activated the swap. The Taker now sends a timed-out take round once
    more and gets the Maker's replayed answer (`363df3cf`).

  The failing combination (`ui:` and `node:xmr:xmr-concurrent` with `node:xmr:mixed-three-pair`,
  three lanes) then passed 3 of 3 in each of three runs at `fede8171`, and 3 of 3, 3 of 3 and 2 of
  3 with a first form of the resend that covered the reserve and ceremony rounds only, where the
  failure was
  mixed-three-pair's Bitcoin swap: it missed its own 240 s Maker lock cutoff while the Maker's one
  worker ran the Monero swap's drives first. The worker's scheduling is shared by every pair and
  was left as it is.

## Not covered

- The punish row ("Punish available: taking the Taker's LEZ", "Punished") and the Taker's
  "Refund window closed", "The Maker did not claim", "LEZ lock window closed" and "Monero sweep in
  doubt" rows were not seen on a desk; nor was the Maker's "Monero funding in doubt" row in this
  run.
- The Maker desk's row past the refund time still reads "Monero funded · waiting for the Taker's
  claim", as the Maker Node reports the same phase (`both_legs_locked`) before and after it; the
  Taker can no longer claim then.
- Only TakerSellsLez exists for Monero; regtest only; linux/amd64 only.
