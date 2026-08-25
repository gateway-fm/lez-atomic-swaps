# Live private-local validation — 2026-08-20

This is a secret-free summary of the read-only validation used by the
submission presentation. It is not a replacement for the transaction evidence
JSON and intentionally does not publish raw runtime directories or private
service state.

## Context

| Field | Value |
|---|---|
| Observed at | `2026-08-20T18:01:15Z` |
| Submission branch base | `m3-plus` |
| Git commit at validation | `049fb475a5d340ab76a9255fc875f5795a656660` |
| Bitcoin | Core 31.1 regtest |
| LEZ | v0.2 private local devnet |

Command, run from `deploy/`:

```sh
./scripts/verify-all.sh
```

## Result

All verification stages passed.

| Stage | Result |
|---|---|
| Compose services | 11/11 up; Bitcoin Core and Basecamp reported healthy |
| Bitcoin chain | height 7,872; spender index present |
| LEZ chain | finalized block 6,980 |
| Explorer transaction display | 110/110 checks passed across 20 discovered certified runs |
| Wallet market controller | 31/31 checks passed |
| Basecamp Maker suite | 3/3 passed |
| Basecamp Taker suite | 4/4 passed |

Maker owner-local health was separately probed:

```json
{
  "chat": "disabled",
  "degraded": false,
  "delivery": "available",
  "ready": true,
  "routes": [],
  "schema_version": 1
}
```

Selected live node log lines at the same observation window showed:

```text
indexer_core       Indexed L2 block 6994
sequencer_core     Created block with 1 transactions in 0 seconds
Bitcoin Core       UpdateTip ... height=7872 ... progress=1.000000
```

The LEZ sequencer and indexer continued producing/indexing blocks after the
verification command, so the displayed heights are expected to advance. They
are runtime-health observations rather than immutable evidence anchors.

## Safety and publication boundary

No raw `.e2e` root, signing journal, database, key, seed, capability,
credential, or adaptor scalar is included here. The presentation's matching
completed-run record is
[`docs/evidence/m3-btc-ui-run-m5arm-0820121736.json`](../docs/evidence/m3-btc-ui-run-m5arm-0820121736.json),
where exact public transaction/block identities and the
`private_material_disclosed=false` assertion are retained.
