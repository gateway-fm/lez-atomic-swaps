# Reporting chain outages from a packaged Maker

Without this configuration a packaged Maker never reports an unreachable chain node. Every
route reads `disabled`, `maker_health` can only be degraded by Delivery or Chat, and an offer
whose chain is down stays published. This file is what turns that on.

## What a route is

A route is a pair **and** a direction — `{"pair": "Zcash", "direction": "TakerSellsLez"}`. The
pair names the **foreign** chain: Bitcoin, Monero or Zcash. **LEZ is never a route**; it is
always the other side of the pair, and a LEZ outage surfaces through Delivery, Chat and sidecar
health instead.

Monero exists only as `TakerSellsLez`, so there are five routes in total.

## Configure it

1. Write a probe for each foreign chain your Maker trades. It is an ordinary executable that
   exits `0` when the chain node is usable and non-zero when it is not. It runs with no shell,
   no inherited environment, no stdin and no output, from `/`, under the daemon's timeout:

   ```sh
   #!/bin/sh
   # /usr/local/lib/lez/route-health-zcash
   zcash-cli getblockchaininfo >/dev/null 2>&1
   ```

   It must be owned by `root` or by the service user, not group- or other-writable, executable,
   and a single hard link. Install it mode `0755` outside the state directory.

2. Copy `route-health.json.example` to `/etc/lez/maker/route-health.json`, mode `0600`, owned by
   the service user. Replace each `program` with your probe's absolute path and each
   `program_sha256` with its digest:

   ```sh
   sha256sum /usr/local/lib/lez/route-health-zcash
   ```

   The digest is re-checked before and after **every** observation, so replacing the probe
   without updating it makes that route unavailable.

3. Add the flag to `/etc/lez/maker/node.json`:

   ```json
   "--route-health-config", "/etc/lez/maker/route-health.json"
   ```

   Optionally `"--route-health-poll-milliseconds", "1000"` to change the cadence.

4. `systemctl daemon-reload && systemctl restart lez-maker-node`.

## Cover every route you trade

**A route you leave out of this file is treated as unavailable, not as unprobed.** That is
deliberate — configuring probing and forgetting a chain should fail closed — but it means the
Maker will refuse to quote or publish on that route, and will withdraw its live offers there.
List every pair and direction your Maker is configured for.

## What you should see

- `maker_health` reports `routes[].state` as `available` or `unavailable` instead of `disabled`,
  and `degraded` becomes true while any route is unavailable.
- Offers on an unavailable route are withdrawn automatically, and withdrawal is idempotent.
- Reserved offers are left alone — a swap in flight is not disturbed by its chain going away.

## When a probe is missing

The Maker still starts. It writes one line per affected route:

```
route health: MakerRouteV1 { pair: Zcash, direction: TakerSellsLez } has no usable probe
executable; this route is unavailable until it is installed
```

The route recovers on its own once the executable the digest names is in place — no restart.
This is deliberate: an operator who does not run every chain should still get a Maker that
serves the chains they do run.
