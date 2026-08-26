# Live private-local validation — 2026-08-26

This is a secret-free release-candidate summary. The verification started no
swap and submitted no chain effect. Its market test created and withdrew one
uniquely named offer, leaving the long-running Compose services online.

## Context

| Field | Value |
|---|---|
| Observed at | `2026-08-26T16:00:01Z` |
| Release candidate | `v0.1.0` on `m3-plus` |
| Validated branch base | `ef585e0c93180ee55f3833b06d1e42100edb4d0c` |
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
| Compose services | 11/11 up; Bitcoin Core and Basecamp healthy |
| Bitcoin chain | height 10,212; spender index present |
| LEZ chain | finalized block 34,736 |
| Explorer transaction display | 120/120 checks passed across 22 certified runs |
| Wallet market controller | 31/31 checks passed |
| Basecamp Maker suite | 3/3 passed |
| Basecamp Taker suite | 4/4 passed |

Maker owner-local health was separately read without changing state:

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

Chat identity and sessions live in the Basecamp apps, so `chat=disabled` in the
standalone Maker-daemon health record is expected; it is not a report that the
Basecamp Chat transport is unavailable.

## Public-source and offline gates

The public patch series reconstructed exact tree
`c747dafbdf39ed4615d92b005e63552a32bb60bf`. The Chat/Delivery E2E then ran in
a task-unique read-only Docker container with `--network none` and
`CARGO_NET_OFFLINE=true`: 40 Maker-node unit tests, 3 BTC Chat process tests,
4 signed offer-discovery tests, and 5 local Delivery tests passed (52/52).

The architecture-diagram, requirements-traceability, M6 prototype, M3 evidence,
video decode, standalone-deck, PDF-render, and public-repository checks also
passed. PDF sampling covered the cover, transport, refund, and closing pages;
the final export has 28 pages at 960 × 540 pt.

## Safety and publication boundary

No raw `.e2e` root, signing journal, database, key, seed, capability,
credential, or adaptor scalar is included. Chain heights are health observations
and will advance; exact public transaction and block identities remain in the
committed JSON evidence records.
