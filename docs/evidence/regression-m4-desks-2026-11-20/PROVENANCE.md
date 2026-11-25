# Provenance — desk recordings at `86bdb8a4`, 2026-11-20

Everything in this folder was produced on one host by the Docker devnet of `deploy/`: the three
Monero desk journeys of `scripts/regress.sh full` and `ui:zec:zec-happy:TakerSellsLez`, run by
`scripts/ui-e2e.sh --record` through `regress.sh` with `LEZ_REGRESS_RECORD=1`, on three regression
lanes with the Monero regtest chain up (`LEZ_MONERO=1`).

Nothing here is private. `results*.tsv` and `regress*.log` carry test names, verdicts, times, lanes
and LEZ heights; `logs/` the tests' output with colour codes removed and one host path replaced by
`<checkout>/`; `videos/` the desks as recorded. No key, cookie, capability file, database,
`runtime/` or `market/` content is included. How the frames were checked is in the README. Scanned
before commit with the repository's gitleaks configuration.

## Source

`86bdb8a4c7766eee34df0f7b30090a606fd49d29`, the commit of the full regression in
[regression-m4-2026-11-20](../regression-m4-2026-11-20/) (its trees are listed there). The checkout
the stack ran from was clean and detached at `86bdb8a4` (`regress.log` and `regress-zec.log` print
"86bdb8a4 (tree dirty: 0)").

## Images

The images of that full regression, not rebuilt: `lez-maker-node:local`
`sha256:1beb2f304224031fe90f2f15657e0317ef3fe51e3b385c806bae5303570013ad` and
`lez-taker-node:local` `sha256:b5209386af6daaecbbdc405499b00e501bbe63629fcbbadaa6ad63b7ab0daa1c`
(`docker image inspect`; the other images as listed in that note). The desks run in the Node
containers. Build source: `/usr/local/share/lez/build-source.txt` read
`86bdb8a4c7766eee34df0f7b30090a606fd49d29` in all six Node containers right after `up.sh` and
again after the last recorded run, each container running the two ids above (`docker inspect`).
Bitcoin Core: the three lanes' Cores held `-checkblockindex=0` (`docker inspect`, after `up.sh`).

## The runs

- Host: Linux 6.8 x86_64, 8 cores, Docker 29.1.3. Other projects' containers share it.
- From the checkout's `deploy/`:
  `SKIP_UI_VERIFY=1 LEZ_NO_BANNER=1 LEZ_MONERO=1 LEZ_LANES=3 bash scripts/up.sh --fresh-lez`
  (new LEZ chain, genesis 2026-11-20 23:05:04Z; lanes 2 and 3 up at 23:07:16Z).
- `LEZ_MONERO=1 LEZ_LANES=3 LEZ_REGRESS_RECORD=1 bash scripts/regress.sh run
  ui:xmr:xmr-happy:TakerSellsLez ui:xmr:xmr-taker-refund:TakerSellsLez
  ui:xmr:xmr-concurrent:TakerSellsLez` twice: `20261120T230801Z` (3 of 3, not kept, README
  "Attempts") and `20261120T233051Z` (3 of 3, kept). Between them the open offers of the three
  Maker Nodes were withdrawn with `maker_offer_withdraw` over each Node's owner socket.
- `LEZ_MONERO=1 LEZ_LANES=3 LEZ_REGRESS_RECORD=1 bash scripts/regress.sh run
  ui:zec:zec-happy:TakerSellsLez` twice: `20261120T232512Z` (1 of 1, not kept) and
  `20261120T234825Z` (1 of 1, kept), the Makers' open offers withdrawn before the second.
- In this order: Monero `20261120T230801Z`, Zcash `20261120T232512Z`, the withdrawal, Monero
  `20261120T233051Z`, the withdrawal of its leftover offers, Zcash `20261120T234825Z`.
- All on the one chain `up.sh` made; 0 frozen tries, 0 new chains, 0 profile switches. The console
  output is not committed: it holds host paths.
