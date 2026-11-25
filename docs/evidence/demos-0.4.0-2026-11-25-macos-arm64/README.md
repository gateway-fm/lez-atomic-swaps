# Monero demo set at `957c8016` (0.4.0), 2026-11-25, macOS arm64

The Monero (XMR) demo set per D1: four desk journeys recorded on the Docker devnet with the Monero
regtest chain up, and one terminal recording of the spend-key recovery after the adaptor journal
is lost. The Nodes, desks and sidecars run the images built from `957c8016`, the commit whose full
regression passed 179 of 179
([regression-0.4.0-2026-11-24-macos-arm64](../regression-0.4.0-2026-11-24-macos-arm64/)). The desk
journeys were driven by the harness of `2680735c`, which adds `ui:xmr:xmr-punish:TakerSellsLez` to
the harness of `957c8016` and changes nothing else that runs ([PROVENANCE.md](PROVENANCE.md)).

They replace, for 0.4.0, the Monero recordings of `86bdb8a4` in
[regression-m4-desks-2026-11-20](../regression-m4-desks-2026-11-20/), which were made before the
Taker Node refunded by itself and before the custody-credit fix.

**Result: 4 of 4 desk journeys passed while recorded, each on its first try** (`run-2/`), and the
recovery example ended with both swaps settled and both Monero outputs swept with rebuilt keys.

## Recordings

The video files are not in git. They are kept for the release page, and
`videos-kept-outside-git.sha256` holds their SHA-256. All are silent H.264, 1680×1050 at 6 frames
per second. Each desk recording is a by-product of the test that passed while it was recorded:
`LEZ_REGRESS_RECORD=1 scripts/regress.sh run …` passes `--record` to `ui-e2e.sh`, and every desk
step is screen-grabbed in its party's own Basecamp with the desk's narration burned in as subtitles
(`ui-tests/record-step.sh`, joined by `ui-e2e.sh`).

| file | bytes | length | test, try | what it shows |
|---|---:|---|---|---|
| `TakerSellsLez-xmr-happy.mp4` | 2,345,421 | 3 min 51 s | `ui:xmr:xmr-happy:TakerSellsLez`, passed on try 1 (202 s) | The Maker desk publishes two Monero offers (1 XMR for 50,000 LEZ) from its form; the Taker desk takes one; the Taker Node locks its LEZ on its own ("LEZ lock pending", "Waiting for the Maker's Monero"); the Maker row goes "Offered · waiting for the Taker's LEZ lock", "Taker's LEZ lock seen · funding Monero", "Monero funded · waiting for the Taker's claim"; the Taker desk shows "Your Monero claim is ready" and presses Claim 1.000000000000 XMR; the Taker row reads "Claiming: authorising the Maker's LEZ claim", "Claim authorised", "Sweeping the Monero", the Maker row "Claiming LEZ"; both desks end `Completed`. |
| `TakerSellsLez-xmr-taker-refund.mp4` | 2,295,979 | 3 min 11 s | `ui:xmr:xmr-taker-refund:TakerSellsLez`, passed on try 1 (963 s) | The Maker funds the Monero; the Taker desk shows "Your Monero claim is ready" and does not press it. After the refund time (720 s after the take) the Taker desk shows "Refund window open" with "Refund 50,000 LEZ"; halfway to the punish time (1,000 s), "Refund window closing"; the Taker desk presses Refund ("LEZ refund sent"); the Maker row shows "Refund sweep: recovering your Monero": the Maker Node learns the Taker's Monero share from the final LEZ refund and sweeps its Monero back; both desks end `Refunded`. The waits up to R and up to the warning are the host's, so they are not on the video. |
| `TakerSellsLez-xmr-concurrent.mp4` | 3,130,515 | 3 min 20 s | `ui:xmr:xmr-concurrent:TakerSellsLez`, passed on try 1 (278 s) | Two offers from the Maker desk's form, both taken from the Taker desk one after the other; the Maker desk shows both rows (one funded, one offered); each is claimed from the Taker desk once the Maker desk shows it funded; the Taker desk shows both rows sweeping, then both `Completed`. |
| `TakerSellsLez-xmr-punish.mp4` | 2,199,837 | 4 min 5 s | `ui:xmr:xmr-punish:TakerSellsLez`, passed on try 1 (1,110 s) | The Maker funds the Monero; the Taker desk shows "Your Monero claim is ready" and does not press it. The Taker's Basecamp (desk, LEZ signer and Node) is then stopped and stays down through the refund window. Shortly before the punish time the Maker desk is opened: its row reads "Monero funded · waiting for the Taker's claim", then "Punish available: taking the Taker's LEZ", then "Punished" ("Your Node took the Taker's LEZ; your Monero stays at the swap's shared address for good"), and the Maker's LEZ balance on the desk goes from 2,150,000 to 2,200,000. The Taker's Basecamp is started again; its desk shows the swap "Punished" ("The refund window closed without a refund …"). The 14 minutes the Taker was down are the host's wait, not on the video. |
| `xmr-recovery-full-lifecycle-live.mp4` | 1,716,579 | 1 min 32 s | `cargo run --locked -p xmr-reference-actor --example full-lifecycle`, exit 0 on its only run | The terminal of the actor layer's full-lifecycle example run against this devnet's Monero regtest (lane 1, `LEZ_XMR_EXAMPLE_MONERO=live`, the default). See [the recovery recording](#the-recovery-recording). |

`run-2/` is the run these recordings come from: `regress.log`, `results.tsv`, `chain.tsv` (LEZ
finality sampled during the run) and each test's log in `logs/` as the runner kept it, colour codes
removed: every desk step, its captions and the labels it asserted ("Your Monero claim is ready",
"Refund window open", "Refund window closing", "Refund sweep: recovering your Monero", "Punished" on
both desks).

How the videos were checked: each was cut into frames every 10 to 15 s, and the last 24 to 40 s of
the punish and happy videos every 2 to 4 s, tiled and read. Nothing private is on screen: wallet
labels ("Munich Vault 01 · Maker Node", "Zurich Wallet 01 · Taker Node"), the devnet accounts' LEZ
and BTC balances, offer and swap ids, the offer form, the activity list; no key, seed, login or host
path.

What the desk recordings do not show, or show in passing:

- In the refund journey the Taker Node's own refund is switched off for the journey (as the journey
  has always done), so that the desk's Refund button and the closing warning appear; with it on, the
  Taker Node refunds by itself at R and neither is shown. The Node's own refund is covered by
  `node:xmr:xmr-maker-never-funds` in the full regression.
- The punish journey asserts the desks only. The LEZ moving to the Maker, no Monero received by
  either party and the Taker's `punished` view are asserted by `node:xmr:xmr-punish`, which passed in
  the full regression of `957c8016`. On this chain the lane-1 Maker's LEZ owner had already signed
  run 1's punishment and LEZ claims, so this punishment was not signed at nonce 0; the journey
  itself does not check the nonce (`node:xmr:xmr-punish` does).
- At times the Taker desk's "My orders" reads "This wallet has not taken an offer yet" for one frame
  between two rows of the same swap (in the happy, concurrent and punish videos): a refresh of the
  desk's list, seen before and after with the swap's row.
- The Taker's "Refund window closed", "The Maker did not claim", "LEZ lock window closed" and
  "Monero sweep in doubt" rows and the Maker's "Monero funding in doubt" row are on no recording.

## The recovery recording

The proposal says the spend-key recovery under partial information loss is demonstrated in the
refund-timeout demo. In the Nodes a role keeps its adaptor journal; no Node and no CLI rebuilds a
spend key without it (the Nodes recover from their journals after a restart, which the kill
scenarios of the full regression cover). The recovery without the journal is the SDK's
`XmrActivatedAgreementV1::recover_claim_spend_key` and `recover_refund_spend_key`. Rebuilding the
key of a devnet desk swap after deleting its journal would need a tool that reads a Node's role root
and that does not ship; so this set records the strongest run that uses shipped code: the actor
layer's example (`crates/xmr-reference-actor/examples/full-lifecycle.rs`), on the types the Nodes
run, with the Monero leg live on this devnet's regtest.

What `xmr-recovery-full-lifecycle-live.mp4` shows, line by line in `recovery/full-lifecycle-live.txt`
(its output) and `recovery/full-lifecycle-live.cast` (the same output with the time of each line,
asciicast v2):

- The example reaches the lane's `monerod` and funding wallet-rpc ("Monero leg: LIVE Regtest",
  genesis `418015bb9ae982a1…`, tip 620).
- Swap 1, completed: both roles' DLEQ proofs, Stage A composed by each role and countersigned, both
  MuSig2 adaptor sessions in each role's SQLite adaptor journal with the Taker's claim partial
  withheld, Stage B; tag 13 locks LEZ; the Maker funds 0.2 XMR to the shared 2-of-2 address (tx
  `84b715fe…`, mined); the Taker's view-only wallet sees the exact output at 10 confirmations; tag
  14 with a wrong partial is refused, the right one accepted; tag 15 claims the LEZ. The Taker
  rebuilds the spend key from its journal; then it **drops its session state and deletes its
  adaptor journal**, keeping only its share, the view key and the Stage A and Stage B bytes, rebuilds
  the key again from those and the public tag-14 partial and tag-15 signature
  (`recover_claim_spend_key`), gets the same key, and sweeps the shared output with it (tx
  `b9b3f2e4…`, 0.1981904 XMR received, mined at height 640).
- Swap 2, refunded: the same set-up with 0.1 XMR (tx `f467a3e6…`); tag 16 before R and tag 17
  before P are refused; tag 16 in [R, P) refunds the LEZ; tag 17 after it is refused. The Maker
  rebuilds the key from its journal, then **deletes its adaptor journal and drops its session
  state**, rebuilds the key from its share, the view key, Stage B's refund presignature and the public
  tag-16 signature (`recover_refund_spend_key`), and sweeps its Monero back with it (tx `282df4a5…`,
  0.0981904 XMR, mined at height 660).
- It prints no private key byte (`private_key_bytes_printed=false`) and exits 0.

What it does not show: the LEZ escrow is the example's in-memory model of the guest program
(states, windows on a model LEZ clock, the tag-14 commitment check and the BIP-340 checks of tags 15
and 16), not the devnet's LEZ chain; Chat is a function call; the two swaps are the example's own,
not the desk swaps above. The Monero leg (funding, the view-only check, both sweeps) is real on the
devnet's regtest chain. Its earlier recorded run, on Monero Stagenet on 2026-11-21, stopped at the
funding; this run went to the end of both swaps.

How it was recorded: the machine has no asciinema. `recovery/record_cast.py` ran the command with
its output piped and wrote each line with its time to the asciicast file;
`recovery/render_cast.py` replays that file as a 150-column, 46-row screen into ASS subtitles, which
the devnet's `lez-maker-node:local` image's `ffmpeg` (libass) drew onto a plain background at real
speed, with 8 s held at the end. Leading spaces of cargo's two status lines are not drawn.

## Attempts

- Run 1 (`run-1/`, 04:26:15Z to 04:50:33Z, two lanes): the same four journeys, 4 of 4 passed on
  their first try (965, 1,106, 208 and 296 s), but no video was joined: the runner ran from a second
  checkout of the tree, where `docker compose run` for the join tried to build the Maker image and
  failed on a build input that is not in git ("video join failed"), and the join then deleted the
  segments. Not kept as recordings. Run 2 ran the journeys again one at a time on lane 1, with
  `LEZ_STACK_COMPOSE_ARGS` naming the stack's own deploy directory for the join.
- No try failed.
