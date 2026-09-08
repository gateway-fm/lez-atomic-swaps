# Reviewer evidence recorded on 2026-09-08

This is a fresh execution on Bitcoin Core 31.1 regtest and the LEZ v0.2.0 local
devnet, captured at source commit
[`c7c5f2d820c6221066721595fcc838dbf1354156`](https://github.com/gateway-fm/lez-atomic-swaps/tree/c7c5f2d820c6221066721595fcc838dbf1354156).
It uses the v0.2.0 swap implementation with the reviewer bootstrap/recorder fixes
in PR #17. Subsequent changes in that PR register the initialization service in
the runtime inventory and publish these records; they do not change the swap
implementation or recorder used here. The playback page was regenerated with
the seek-slider fix at renderer commit `de0aeb9`, recorded separately in
`provenance.json`. Execution logs and transaction evidence are unchanged.

Download [index.html](index.html) and open it locally to play, seek, or accelerate
all three terminal recordings. The HTML embeds the recordings and works offline.
These are Node owner-API executions. The inherited five-effect export's
`m3_btc_ui_evidence` kind and optional localhost explorer URLs do not mean that
Basecamp or an explorer was exercised in this run.

| Scenario | Result | Harness elapsed time |
| --- | --- | --- |
| Two concurrent swaps | passed | 8m03s |
| Taker BTC refund; Maker never funds | passed | 22m08s |
| Maker LEZ refund followed by Taker BTC refund | passed | 22m03s |

The concurrent swaps both locked BTC before either LEZ claim. Both roles reached
`completed`, with ten distinct confirmed/finalized transaction IDs across the
two swaps. Each refund spent its recorded BTC lock and returned 999,000 sats to
the Taker's contribution destination, from a 1,000,000-sat principal with a
1,000-sat fee. The Maker refund also verifies historical LEZ account balances:
1,000 units funded, then 1,000 units returned. The large Core wallet balance
increases in the logs include mined regtest coinbase rewards; those increases
are not used as proof of the refunded principal.

`summary.json` in each scenario records the harness duration above;
`recording-result.json` also includes the subsequent public-chain verification.
The logs retain the complete waits, including the time for a Node to reconcile
a finalized chain event. Harness timestamps use the host's Europe/Zurich time;
heartbeat timestamps explicitly use UTC.

To repeat with new identities and transactions, follow
[the reviewer guide](../../../deploy/REVIEWER.md). This machine used Apple
Silicon and Docker Desktop, a new checkout and empty provision directory, fresh
wallet identities and isolated chain state. Node/actor, LEZ services, r0vm,
cargo-risczero, escrow and sidecar builds completed from source. Docker base
images and compiler/dependency caches were reused; this was not a test with an
entirely empty Docker cache. The complete bootstrap and a subsequent rerun
both passed before capture.

These new records validate the three BTC → LEZ API scenarios on this v0.2
checkout. They do not retroactively certify every result in the older v0.1.2
evidence map, or revalidate XMR, reverse-direction, UI, or threat-model claims.
Earlier release recordings remain historical evidence.

## Integrity and source

The public logs, casts, JSON, provenance and playback page are copied byte for
byte from the capture. Wallet databases, signer material and runtime credentials
are not included. The original 60 MB `source.tar.gz` is omitted from Git because
its exact source commit is linked above. `FULL_CAPTURE_SHA256SUMS` preserves the
original capture manifest, including that archive's digest.

`SHA256SUMS` covers the files in this checked-in subset, including this README
and the original manifest. Verify it from this directory:

```sh
shasum -a 256 -c SHA256SUMS
```

Checksums verify file integrity. Independent execution of the reviewer commands
verifies the behavior; new transaction IDs and timings will differ.
