# Full regression of `6ad741bf` (0.5.0), 2026-11-27/28, Linux x86_64

This is the full regression of release 0.5.0, and the first full regression on an x86_64 Linux
host. Its tier holds 200 tests, against the 180 of
[regression-0.4.0-2026-11-25-macos-arm64-zebra-module](../regression-0.4.0-2026-11-25-macos-arm64-zebra-module/).
Twenty-four tests are new:

- `claim-send-fails` (issue #174), `zec-maker-claim-collision` (#176), `zec-kill-maker-refund`
  and `zec-kill-taker-refund` (#178), and `zec-early-refund` as a Node scenario and as a desk
  journey (#182);
- the CLI journeys `taker-cli`, `xmr-taker-cli`, `zec-taker-cli-refund`, `maker-cli-refund`,
  `zec-maker-cli-claim` and `zec-maker-cli-refund`;
- `outage-before-take` and `zec-outage-before-take` (Delivery and Chat outages), and
  `maker-price-module-stale` (the price source);
- `zec-regenerated-config` and `zec-tampered-config`, which replace the four
  `zec-regenerated-catalog` and `zec-tampered-catalog` tests. A Zcash swap is now agreed at take
  time, and no prepared catalog exists to regenerate or tamper with.

Every test of `deploy/scripts/regress.sh full` (200: `regress.sh list full`) ran on the Docker
devnet, with the Monero regtest chain up (`LEZ_MONERO=1`) and the devnet test hooks built in
(`LEZ_TEST_HOOKS=1`). They ran three at a time on three lanes (`LEZ_LANES=3`), without desk
recordings. One operator ran both parties, Maker and Taker, on one host: a virtual machine with
8 cores and 31 GB of memory (Ubuntu 24.04.5, kernel 6.8.0-147), Docker Engine 29.1.3, local
linux/amd64 dev-profile images. No second operator and no second host took part.

**Result:**

- **On `6ad741bf`:** 199 of 200 passed, each on its first try, in one run of 295 minutes
  (2026-11-27 19:16:59Z to 2026-11-28 00:12:17Z). There were 0 frozen tries, 0 NOT-EXERCISED
  and no timeout.
- **The 1 failure:** `node:zec:zec-maker-claim-collision:TakerSellsLez`, the scenario of issue
  #176. Its last check was wrong, not the Node (below).
- **On `f98908ec`:** that test was run again, in a separate runner invocation labelled as such.
  `f98908ec` differs from `6ad741bf` in the scenario's check, in the systemd VM script and in
  documents (`diff-6ad741bf-f98908ec.txt`). It passed on its first try.
- **Nothing else was rerun.**

| invocation | head | command | UTC | tests | result |
|---|---|---|---|---:|---|
| 1 (`run-6ad741bf-full/`) | `6ad741bf` | `regress.sh full` | 19:16:59Z to 00:12:17Z (295 min) | 200 | 199 passed, 1 failed |
| 2 (`run-f98908ec-collision/`) | `f98908ec` | `regress.sh run node:zec:zec-maker-claim-collision:TakerSellsLez` | 00:15:06Z to 00:17:34Z (2 min) | 1 | 1 passed |

| kind | tests | minutes in invocation 1 (summed over lanes) |
|---|---:|---:|
| `node:btc` | 62 | 192.9 |
| `node:zec` | 70 | 211.8 |
| `node:xmr` | 23 | 286.5 |
| `ui:btc` | 17 | 39.8 |
| `ui:zec` | 24 | 59.7 |
| `ui:xmr` | 4 | 39.7 |
| all | 200 | 830.4 |

The median test took 107 s. Invocation 1 had 3 timing-profile switches: `zec-v6` at 22:49:47Z,
`refund` at 23:43:13Z, and back to `fast` for the own-chain pair at 00:08:08Z. It made one new
LEZ chain, for both `zec-balance-refusal` tests (lanes 1 and 2 at once). Before invocation 2,
`up.sh --fresh-lez` with `LEZ_LANES=3` made a funded chain again (00:12:58Z to 00:15:06Z),
because the last chain had been that pair's own, which leaves two wallets unfunded.

`results.tsv` holds the two invocations' tables one after the other (201 rows: 200, then 1).

## The claim-collision test

`zec-maker-claim-collision` is the scenario of issue #176. After the Taker claims Zcash, a test
hook (`claim-nonce-thefts`) makes six other transactions of the Maker's own account take the
nonce the Maker's follow-up LEZ claim was planned on, one after the other. The Maker must
re-plan the claim each time, report `lez_claim_nonce_contention`, and still claim.

**What the Node did on `6ad741bf`.** The test log
(`run-6ad741bf-full/logs/node-zec-zec-maker-claim-collision-TakerSellsLez-1.log`) shows all of
it:

- six lines "follow-up LEZ claim superseded (planned nonce 20, live 21); re-plan 1 at nonce 21"
  through "(planned nonce 25, live 26); re-plan 6 at nonce 26";
- "the LEZ claim was re-planned 6 times: the account's nonce keeps being taken; it is
  re-planned until the LEZ refund time; needs attention (lez_claim_nonce_contention)".

Both Nodes then reported the swap completed.

**Why the test failed.** The scenario's last check compares the Maker's account nonce before and
after: six thefts and the claim move it by 7. It read the nonce from the indexer once, as soon
as the Maker reported the claim, and the indexer serves final blocks only. The claim's block was
not final yet, so it read 6: "FAILED: the Maker's account nonce moved by 6; 6 thefts and the
claim move it by 7" (lane 2, 86 s).

**On `f98908ec`** the check waits up to 180 s for the indexer's nonce to reach the expected
value (`d2e736be`). The rerun's log shows the same six re-plans and the same
`lez_claim_nonce_contention` line, the wait for finality, and "settled completed: LEZ moved
{'maker': 49994, 'taker': -49994} as both Nodes say".

The scenario had not run against a stack before this regression, so this run was its first.

## What each folder holds

| folder | what |
|---|---|
| `run-6ad741bf-full/` | invocation 1: `regress.log`, `results.tsv`, `chain.tsv` (LEZ heights during the run), `logs/` (one log per try, the profile switches, the lanes), `e2e/` (each Node scenario's summary), `zec/` (each Zcash swap's export), `lanes/` (each lane's last reset and verdict) |
| `run-f98908ec-collision/` | invocation 2, the same files |
| `results.tsv` | both tables |
| `RESULTS.txt` | the result in plain text |
| `PROVENANCE.md` | source, images, host, commands, privacy |
| `diff-6ad741bf-f98908ec.txt` | `git diff --name-only 6ad741bf f98908ec` |
| `OUTSIDE-GIT.sha256` | the digest of the failed try's Node logs, which are kept outside git |
| `SHA256SUMS` | the digest of every file here |

The columns of `results.tsv`: test, try, verdict, seconds, finished (UTC), lane, new chains.
