# Writing and integrating a Logos price module for the Maker

A Maker route (a pair and a direction) gets its price from one of two price sources. This page
says how to write the second kind, a Logos price module, and how to run it. Everything here is
what the code in this tree does; each statement names the file it comes from. Requirement F8
(`docs/requirements-traceability.md`): "Pluggable local and Logos-module C-API price sources".

Verified: the example module in section 6 was compiled with `cc` on an Ubuntu 24.04 host (gcc
13.3) and run through the `lez-logos-price-worker` shipped in the `lez-maker-node:local` image on
2026-11-13 (outputs below are from that run), as were the repository fixture's failure modes. The
Maker-side behaviour (the Node's checks, error codes, the quote history) is from reading the code
and from the stack scenario named in section 8; it was not re-run for this page.

## 1. What a price source is

A price source answers one question: for this route, how many LEZ atomic units per how many
foreign atomic units, now. The boundary is the `PriceSource` trait
(`crates/maker-node/src/price_source.rs`), which returns a `PriceQuoteV1`: the price, a source
revision and an observation time. There are two implementations:

- **local**: the price the owner saved with `lez-maker-cli set-local-price --pair … --direction …
  --lez-units-per-lot N --foreign-units-per-lot M` (`LocalPriceSource`, in the same file). Its
  revision is the stored row's revision; its observation time is
  the moment of the call.
- **logos-c-api**: a shared library with the C ABI of
  `crates/logos-price-c-api/include/lez_logos_price_api.h`, run by a separate worker process
  (`ProcessLogosPriceSource`, `crates/maker-node/src/logos_price_source.rs`).

Each route chooses its source in its pair configuration (`lez-maker-cli configure-pair …
--price-source local|logos-c-api`; default `local`).

The Maker consults the source at two moments only (`crates/maker-node/src/lib.rs`):

- `maker_price_quote` (`lez-maker-cli quote`), through `quote_selected_price_source`. It only
  reads; it records nothing.
- `maker_offer_publish` (`lez-maker-cli publish-offer`), through `publish_offer`: the module is asked once, and its price, revision and observation time are
  written into the new offer by `publish_logos_offer` (`crates/swap-store/src/maker_offer.rs:1552`).

After that the price is part of the signed offer. Reserving and taking an offer read the stored
offer (`reserve_maker_offer`, maker_offer.rs:1662); no code path from take onward calls the price
source (the only callers are the two above). A module that changes its price, or stops answering,
does not reprice or remove offers already out.

The guarded `maker_offer_publish_v1` (`crates/maker-node/src/extension_api.rs:38`) publishes
local-price routes only; on a `logos-c-api` route it is refused with "maker route does not use the
local price source" (maker_offer.rs:1483-1484). Use `maker_offer_publish` / `publish-offer`.

## 2. The C ABI

The whole interface, from `crates/logos-price-c-api/include/lez_logos_price_api.h` (verbatim,
include guards and `extern "C"` omitted):

```c
#define LEZ_LOGOS_PRICE_ABI_VERSION 1u

enum lez_logos_price_pair_v1 {
  LEZ_LOGOS_PRICE_PAIR_BITCOIN = 1u,
  LEZ_LOGOS_PRICE_PAIR_MONERO = 2u,
  LEZ_LOGOS_PRICE_PAIR_ZCASH = 3u
};

enum lez_logos_price_direction_v1 {
  LEZ_LOGOS_PRICE_TAKER_SELLS_LEZ = 1u,
  LEZ_LOGOS_PRICE_TAKER_SELLS_FOREIGN = 2u
};

enum lez_logos_price_status_v1 {
  LEZ_LOGOS_PRICE_OK = 0,
  LEZ_LOGOS_PRICE_MISSING = 1,
  LEZ_LOGOS_PRICE_UNAVAILABLE = 2,
  LEZ_LOGOS_PRICE_INVALID_REQUEST = 3
};

struct lez_logos_price_request_v1 {
  uint32_t struct_size;
  uint32_t abi_version;
  uint32_t pair;
  uint32_t direction;
  uint64_t now_unix_seconds;
  uint64_t reserved[2];
};

struct lez_logos_price_response_v1 {
  uint32_t struct_size;
  uint32_t abi_version;
  uint32_t pair;
  uint32_t direction;
  uint64_t lez_units_per_lot;
  uint64_t foreign_units_per_lot;
  uint64_t source_revision;
  uint64_t as_of_unix_seconds;
  uint64_t reserved[2];
};

uint32_t lez_logos_price_abi_version_v1(void);
int32_t lez_logos_price_quote_v1(
    const struct lez_logos_price_request_v1 *request,
    struct lez_logos_price_response_v1 *response);
```

The header also declares function-pointer typedefs for the two symbols
(`lez_logos_price_abi_version_v1_fn`, `lez_logos_price_quote_v1_fn`).

**Symbols.** A module exports exactly these two C symbols. The worker looks them up by name
(`crates/logos-price-c-api/src/lib.rs:283-298`); a missing one fails the call ("required ABI v1
symbol is absent").

- `lez_logos_price_abi_version_v1()` must return `1`. Any other value fails ("module ABI version
  N is not supported"); the quote function is then not called.
- `lez_logos_price_quote_v1(request, response)` answers one quote and returns a status.

**Request** (filled by the worker, lib.rs:301-308):

| field | value |
|---|---|
| `struct_size` | `sizeof(struct lez_logos_price_request_v1)`, 40 on the host's layout |
| `abi_version` | 1 |
| `pair` | 1 Bitcoin, 2 Monero, 3 Zcash: the route's foreign asset |
| `direction` | 1 the Taker pays LEZ and receives the foreign asset; 2 the Taker pays the foreign asset and receives LEZ |
| `now_unix_seconds` | the Maker's clock when it asked; never 0 |
| `reserved` | zero |

**Response** (the module fills it; the worker zero-initialises it first, lib.rs:309):

| field | rule (lib.rs `validate_quote`, 331-385) |
|---|---|
| `struct_size` | exactly `sizeof(struct lez_logos_price_response_v1)` (64), else "wrong response size" |
| `abi_version` | 1 |
| `pair`, `direction` | copied from the request; a different route fails ("route does not match request") |
| `lez_units_per_lot` | LEZ atomic units in one lot; nonzero |
| `foreign_units_per_lot` | foreign atomic units (satoshi, zatoshi) in one lot; nonzero |
| `source_revision` | the module's identifier of this observation; nonzero; see section 3 |
| `as_of_unix_seconds` | when the module's data was observed; nonzero, not after `now_unix_seconds`, not older than the max age |
| `reserved` | must be zero, else "reserved response fields are nonzero" |

**The price is a ratio of integers, not a number.** "5 LEZ units per 2 sat" is
`lez_units_per_lot = 5`, `foreign_units_per_lot = 2`. A lot is the smallest exchange the ratio
describes. The Maker stores the ratio as a `LocalPriceV1`, which requires both values nonzero, at
most `i64::MAX`, and **reduced** (greatest common divisor 1)
(`crates/swap-store/src/maker_application.rs:213-220`). The worker does not check the reduction;
the Maker does (`logos_price_source.rs:160-165`), so `10/4` passes a hand run of the worker but
fails in the Maker (-32603). Return `5/2`. The module may quote a different ratio per direction.

**Status codes** (lib.rs:313-327): `0` OK, the response is validated as above; `1` MISSING, the
module has no price for this route; `2` UNAVAILABLE, it has one in principle but cannot give it
now; `3` INVALID_REQUEST, the module refuses the request (the call fails as a contract error);
any other value fails ("module returned invalid status N"). On MISSING and UNAVAILABLE the
response fields are ignored.

**Ownership and lifetime.** Both structs belong to the worker and live on its stack for the one
synchronous call (lib.rs:301-312). The module reads the request, writes the response, and must
not keep either pointer. Nothing is allocated across the boundary and nothing is freed by the
other side. The worker loads the library, makes one version call and one quote call, prints the
result and exits (`src/bin/lez-logos-price-worker.rs`): every quote is a fresh process, so
**memory in the module does not survive between quotes**. A module that needs state (a counter, a
cache) keeps it in a file or derives it from its data. The worker runs with an empty environment,
`/` as its working directory and no stdin (`logos_price_source.rs:98-102`): a module cannot read
configuration from environment variables; compile it in or read an absolute path.

**ABI version.** `LEZ_LOGOS_PRICE_ABI_VERSION` is 1, the only version the worker accepts
(`ABI_VERSION_V1`, lib.rs:15). The `_v1` suffix on every name and the `struct_size` field are
there so a later version can coexist; no later version exists.

## 3. Semantics the module must honour

**One revision is one observation, and revisions increase.** When an offer is published, the
Maker records the quote as the head of that source's history, keyed by pair, direction and module
sha256 (`update_external_price_head`, `crates/swap-store/src/maker_offer.rs:4176-4246`, table
`maker_external_price_heads`). Against the stored head a new quote is refused when:

- its revision is lower ("maker price source revision rolled back", `MakerPriceRevisionRollback`);
- its revision is higher but its `as_of` is earlier (also rollback);
- its revision is equal but its `as_of`, `lez_units_per_lot` or `foreign_units_per_lot` differs
  ("maker price source revision conflicts with durable quote history",
  `MakerPriceRevisionConflict`).

Both answer RPC error -32009 (`application_store_error` in `crates/maker-node/src/lib.rs`). The same revision with the
same observation and price is accepted. The head is stored in the Maker's SQLite database, so it
survives restarts and is kept for as long as the module's sha256 stays the same; a rebuilt module
with a different hash starts a new history. Only publish updates the head; `quote` does not.

So: give every new observation a new, larger revision, and never reuse a revision for different
data. **The repository's own test fixture got this wrong once.** It reported revision 7 on every
call with `as_of = now - 5`; its build is deterministic, so a second run against the same Maker
state repeated revision 7 with another observation time, and the stack scenario failed in a full
regression with -32009 (commit e0204908, which added `FIXTURE_REVISION_FROM_TIME`). Because every
quote is a new process (section 2), a counter held in memory cannot work. If your data source has
its own sequence number, use it; otherwise the observation's Unix second works as long as the
price does not change twice within one second.

**Freshness.** The observation must be no older than the Node's max age and not in the future.
The bound is `--logos-price-max-age-seconds`, default 30, allowed 1-3600
(`logos_price_max_age_seconds` and `configured_logos_price_source` in
`crates/maker-node/src/bin/lez-maker-node.rs`; `MAX_QUOTE_AGE_SECONDS`, logos-price-c-api
lib.rs:21). It is checked three times: in the worker (`validate_quote`, "quote
is stale" / "observation time is in the future"), in the Maker on the worker's answer
(`logos_price_source.rs:150-159`), and at publish against the commit time, which is a little later
than the quote time (`publish_logos_offer`, maker_offer.rs:1575-1586; a quote right at the limit
can therefore fail at publish with -32602 "maker offer is invalid"). Report the time your data was
observed, not the time you were asked, and leave margin under 30 s.

**Missing, unavailable, invalid.** The Maker fails closed in each case; there is no fallback to a
local price and no zero price (`quote_selected_price_source`, `publish_offer`):

| module answers | Maker error (`price_source_error` in the Maker's lib.rs) |
|---|---|
| MISSING | -32004 "price source has no quote for the requested route" |
| UNAVAILABLE | -32003 "price source is temporarily unavailable" |
| worker past its deadline | -32003 "price source worker timed out" |
| anything invalid: INVALID_REQUEST, unknown status, malformed or stale response, crash, failed file check, unreduced ratio | -32603 "external price source failed validation" |

A refused publish leaves no offer behind. Offers already published keep their price and stay
active: the Maker withdraws offers automatically only when a route's chain dependency is
unavailable (`reconcile_unhealthy_route_offers` in the Maker's lib.rs, driven by route health, not by the
price source), when the owner withdraws them, or when they expire. The stack scenario checks this:
with the module answering UNAVAILABLE, the offer already out is still listed active and still
offered to the Taker (`scenario_maker_price_module`, `deploy/scripts/node-e2e.py`).

**Integer lots and the offer's bounds.** The offer carries the route's
`--minimum-foreign-units`/`--maximum-foreign-units`. At take, the Taker's foreign amount must lie
within them, and `foreign_amount × lez_units_per_lot / foreign_units_per_lot` must be a whole
number, else "selected amount is not exactly representable by the offer price" (no rounding;
`quote_foreign_amount`, maker_offer.rs:224-238). With a reduced ratio that means the foreign
amount is a multiple of `foreign_units_per_lot`: the scenario uses 200 000 sat at 5/2, giving
500 000 LEZ units. A ratio with a large `foreign_units_per_lot` makes most amounts untakeable.

## 4. Isolation: the worker process

The module never runs inside the Maker. For every quote and publish the Maker starts
`lez-logos-price-worker` (`crates/logos-price-c-api/src/bin/lez-logos-price-worker.rs`), which
loads the module, asks once and prints one JSON line. The parent side
(`logos_price_source.rs:78-129`):

- deadline `--logos-price-timeout-milliseconds`, default 1000, at most 5000 (`MAX_WORKER_TIMEOUT`,
  :23); past it the worker is killed and the call fails with -32003;
- at most 4096 bytes of output (:21-22, :116-126), else invalid;
- a non-zero exit (including a crash or `abort()`) is invalid; the worker's stderr is discarded.

A crash, hang or memory corruption in the module therefore ends one short-lived process; the
Maker answers a typed error and keeps running. `crates/logos-price-c-api` is the only crate in the
workspace allowed `unsafe` code (its `Cargo.toml`).

The worker's command line and output, for running it by hand:

```
lez-logos-price-worker --library /abs/path/module.so --pair 1 --direction 2 \
  --now-unix-seconds "$(date +%s)" [--max-age-seconds 30]
```

On success it exits 0 and prints
`{"schema_version":1,"status":"ok"|"missing"|"unavailable","pair":"bitcoin","direction":"taker_sells_foreign","lez_units_per_lot":5,"foreign_units_per_lot":2,"source_revision":…,"as_of_unix_seconds":…}`
(the four quote fields are `null` unless `ok`). On a contract violation it exits 1 and writes
`price worker failed closed: <reason>` to stderr, the reason being one of the messages quoted in
sections 2-3. The worker's own `--max-age-seconds` default is 30; the Maker always passes its own.

## 5. File rules the Maker enforces

The Node checks the module file when it starts (`ProcessLogosPriceSource::new`,
`logos_price_source.rs:42-64`), again before every worker run and again after it
(`:83-84`, `:127`), with `is_secure_file`
(`crates/node-common/src/secure_file.rs:257-297`). Any failure is -32603 (or a start failure):

- the path is absolute;
- the path itself is a regular file: it is not followed, so a symlink is refused;
- exactly one hard link;
- owned by root or by the Node's user;
- not group- or other-writable (`mode & 0o022 == 0`); the module needs no execute bit;
- at most 64 MiB (`MAX_NON_EXECUTABLE_BYTES`, secure_file.rs:249);
- its parent directory is a directory owned by root or the Node's user and not group- or
  other-writable;
- its sha256 equals the pinned `--logos-price-module-sha256` (64 hex characters, decoded in
  `configured_logos_price_source`, `lez-maker-node.rs`).

The worker binary passes the same checks except the size and hash, and must be executable.

The module path and its sha256 are read once, at start (`configured_logos_price_source`,
`lez-maker-node.rs`). Replacing the file with a new build changes its hash, so every
quote and publish fails with -32603 until the Node is restarted with the new sha256. To switch
modules: install the new file, restart the Node with its path and hash.

## 6. A minimal module

[`docs/examples/fixed_price_module.c`](examples/fixed_price_module.c) quotes 5 LEZ units per 2
foreign units for Bitcoin and Zcash and treats every call as a new observation:

```c
#include "lez_logos_price_api.h"

#include <stddef.h>
#include <string.h>

uint32_t lez_logos_price_abi_version_v1(void) {
  return LEZ_LOGOS_PRICE_ABI_VERSION;
}

int32_t lez_logos_price_quote_v1(
    const struct lez_logos_price_request_v1 *request,
    struct lez_logos_price_response_v1 *response) {
  if (request == NULL || response == NULL ||
      request->struct_size != sizeof(*request) ||
      request->abi_version != LEZ_LOGOS_PRICE_ABI_VERSION) {
    return LEZ_LOGOS_PRICE_INVALID_REQUEST;
  }
  if (request->pair != LEZ_LOGOS_PRICE_PAIR_BITCOIN &&
      request->pair != LEZ_LOGOS_PRICE_PAIR_ZCASH) {
    return LEZ_LOGOS_PRICE_MISSING;
  }

  memset(response, 0, sizeof(*response));
  response->struct_size = sizeof(*response);
  response->abi_version = LEZ_LOGOS_PRICE_ABI_VERSION;
  response->pair = request->pair;
  response->direction = request->direction;
  response->lez_units_per_lot = 5u;
  response->foreign_units_per_lot = 2u;
  response->as_of_unix_seconds = request->now_unix_seconds;
  response->source_revision = request->now_unix_seconds;
  return LEZ_LOGOS_PRICE_OK;
}
```

Line by line:

- The header gives the structs, enums and prototypes; nothing else is needed.
- `lez_logos_price_abi_version_v1` returns the header's version, 1.
- The first check refuses a request whose size or version this build does not know. Comparing
  `struct_size` with your own `sizeof` is what keeps an old module from misreading a newer
  request.
- Monero is answered MISSING: the Maker reports -32004 for that route, with no price.
- `memset` clears the response, which also zeroes `reserved` as required.
- `struct_size`, `abi_version`, `pair` and `direction` are filled from the header and the
  request: the worker rejects any other route.
- The ratio is reduced (5 and 2 share no divisor).
- `as_of_unix_seconds` is the request's time: this module "observes" when asked. A real module
  puts the time its data was observed.
- `source_revision` is the same second, so revisions increase with time, and two calls in one
  second give the same revision with the same observation and price, which the Maker accepts. A
  real module whose price can change within a second needs its own sequence number.

The repository's fixture, `crates/logos-price-c-api/tests/fixtures/logos_price_fixture.c`, is
the same shape (5/2, revision 7, `as_of = now - 5`) with compile-time modes that break one rule
each: `FIXTURE_MISSING`, `FIXTURE_UNAVAILABLE`, `FIXTURE_ABORT`, `FIXTURE_STALE` (61 s old),
`FIXTURE_FUTURE`, `FIXTURE_ZERO_PRICE`, `FIXTURE_ZERO_REVISION`, `FIXTURE_WRONG_ROUTE`,
`FIXTURE_RESERVED`, `FIXTURE_WRONG_ABI`, `FIXTURE_WRONG_VERSION_SYMBOL`, `FIXTURE_MISSING_SYMBOL`,
and `FIXTURE_REVISION_FROM_TIME` (revision = request time, used by the stack scenario).

Build, with the flags of the crate test (`compile_fixture` in
`crates/logos-price-c-api/tests/worker_process.rs`):

```sh
cc -shared -fPIC -std=c11 -Wall -Wextra -Werror \
  -I crates/logos-price-c-api/include \
  docs/examples/fixed_price_module.c -o libfixed_price.so
chmod 644 libfixed_price.so
sha256sum libfixed_price.so
```

Run it through the worker the Maker image ships (read-only use of the image):

```sh
docker run --rm --network none -v "$PWD:/m:ro" --entrypoint \
  /var/lez/basecamp-maker-user/modules/lez_maker_node/bin/lez-logos-price-worker \
  lez-maker-node:local --library /m/libfixed_price.so --pair 1 --direction 2 \
  --now-unix-seconds "$(date +%s)"
```

Output of that run (2026-11-13):

```
{"schema_version":1,"status":"ok","pair":"bitcoin","direction":"taker_sells_foreign","lez_units_per_lot":5,"foreign_units_per_lot":2,"source_revision":1794576528,"as_of_unix_seconds":1794576528}
```

`--pair 2` printed `"status":"missing"` with null quote fields; `--pair 9` exited 1 with
"unsupported pair identifier 9"; a relative `--library` path exited 1 with "module path must be an
absolute regular file". The fixture's modes, run the same way, printed the reasons quoted in
sections 2-3; `FIXTURE_ABORT` ended the worker with signal 6 (exit 134).

## 7. Integrating it

**In the role package / containers.** The launcher `modules/lez-role-node/node-entrypoint.sh`
(the block that tests `LEZ_LOGOS_PRICE_MODULE`) adds `--logos-price-worker <package bin>/lez-logos-price-worker --logos-price-module
"$LEZ_LOGOS_PRICE_MODULE" --logos-price-module-sha256 "$LEZ_LOGOS_PRICE_MODULE_SHA256"` when
either variable is set, and refuses to start with only one of them. `deploy/compose.yaml`
passes both to the `maker-node` service; empty means no module. The worker is in the
Maker's package (`deploy/scripts/package-role-node.sh:26`). The file must be inside the container
and satisfy section 5 for the Node's user (`lez`, uid 4713, in the image). The stack scenario
writes it into the Maker's state volume and recreates the service:

```sh
docker exec -i lez-maker-node sh -c 'umask 022 && mkdir -p /var/lib/lez/maker/price-module &&
  cat > /var/lib/lez/maker/price-module/libfixed_price.so' < libfixed_price.so
cd deploy
LEZ_LOGOS_PRICE_MODULE=/var/lib/lez/maker/price-module/libfixed_price.so \
LEZ_LOGOS_PRICE_MODULE_SHA256=<sha256> \
  docker compose --env-file runtime/runtime.env up -d --no-build --force-recreate --no-deps maker-node
```

(`install_test_price_module` and `recreate_maker_with_price_module` in
`deploy/scripts/node-e2e.py`.) The timeout and max-age flags are not set by the launcher, so the
defaults (1000 ms, 30 s) apply there.

**Under systemd.** `scripts/install-maker-node-service.sh` installs the worker as
`/usr/bin/lez-logos-price-worker` and the same launcher as `/usr/bin/lez-role-node-launcher`, which
the unit runs with `EnvironmentFile=/etc/lez/maker/env` (`packaging/systemd/lez-maker-node.service`).
The launcher's worker path is its own directory, `/usr/bin`, so setting the two variables in
`/etc/lez/maker/env` gives the Node the three flags; then `systemctl restart lez-maker-node`. The
unit has `ProtectHome=yes`, so keep the module out of `/home`; a root-owned file in a root-owned
directory such as `/usr/local/lib/lez/` satisfies section 5. Running `lez-maker-node` directly,
pass `--logos-price-worker`, `--logos-price-module`, `--logos-price-module-sha256` and optionally
`--logos-price-timeout-milliseconds`, `--logos-price-max-age-seconds`
(the `logos_price_*` fields of `Arguments`, `lez-maker-node.rs`); the three must be given together. (This systemd path was read, not
run, for this page.)

**Selecting it for a route and publishing** (in the container prefix with
`docker exec lez-maker-node`):

```sh
lez-maker-cli configure-pair --request-id price-pair-1 --pair bitcoin \
  --direction taker-sells-foreign --enabled true --price-source logos-c-api \
  --minimum-foreign-units 200000 --maximum-foreign-units 200000 --offer-ttl-seconds 3600
  # add --expected-revision N when the route already exists
lez-maker-cli quote --pair bitcoin --direction taker-sells-foreign
  # {"price":{"route":…,"lez_units_per_lot":5,"foreign_units_per_lot":2},
  #  "source_revision":…,"observed_at_unix_seconds":…}
lez-maker-cli publish-offer --request-id price-publish-1 --offer-id offer-price-1 \
  --pair bitcoin --direction taker-sells-foreign
lez-maker-cli offers
```

A route set to `logos-c-api` on a Node started without a module answers -32003 to quote and
publish (`quote_selected_price_source` and `publish_offer` map a missing source to
`PriceSourceError::UnavailableQuote`). The published offer records `price` (the module's ratio),
`price_source_identity_sha256` (the module's pinned sha256), `price_source_revision` and
`price_observed_at_unix_seconds` (the module's revision and `as_of`), next to
`created_at_unix_seconds` and `expires_at_unix_seconds` (maker_offer.rs:1613-1623). A local-price
offer has no identity and its observation time equals its creation time (maker_offer.rs:256-271).

## 8. Testing a module

Crate tests to copy (they build Rust: run them only where builds are allowed):

- `crates/logos-price-c-api/tests/worker_process.rs` compiles the C fixture in each mode and runs
  the real worker on it: `cargo test -p lez-logos-price-c-api --test worker_process`. To test
  your module, point `compile_fixture` at your source.
- `crates/maker-node/tests/logos_price_process.rs` (the Maker's parent side: max age, typed
  unavailability, abort, timeout, oversized output, writable, hard-linked and wrong-hash modules)
  and `crates/maker-node/tests/logos_price_offer_process.rs` (a running daemon signs an offer at
  the source's quote and replays it): `cargo test -p lez-maker-node --test logos_price_process
  --test logos_price_offer_process`. Both use a shell script in place of the worker, so they test
  the Maker's handling of worker output, not a C module.

On the devnet stack, `node:btc:maker-price-module:TakerSellsForeign`
(`scenario_maker_price_module` in `deploy/scripts/node-e2e.py`) compiles the fixture with
`FIXTURE_REVISION_FROM_TIME`, installs it in the Maker's container, quotes, publishes two offers,
completes a swap on one, then installs the `UNAVAILABLE` build and checks quote and publish answer
-32003 while the other offer stays active:

```sh
cd deploy && bash scripts/regress.sh run node:btc:maker-price-module:TakerSellsForeign
```

`node:btc:maker-price-module-stale:TakerSellsForeign` (`scenario_maker_price_module_stale`) is
the staleness bound of section 4 on the stack, and a second pair. With the same build it selects
`logos-c-api` for the Bitcoin route and for Zcash `taker-sells-lez`, checks each route's quote and
publishes one offer on each, which the Taker lists at the module's price. Then it installs the
`STALE` build (`FIXTURE_STALE`, a quote 61 s old against the Node's default 30 s) and checks that
`quote` and `publish-offer` on both routes answer -32603 "external price source failed
validation", that a refused publish leaves no offer and that the two offers already out stay
active at their price. With the first build back the quote answers again. It completes no swap;
the scenario above does. It puts the Zcash route back as it found it:

```sh
cd deploy && bash scripts/regress.sh run node:btc:maker-price-module-stale:TakerSellsForeign
```

To try your own module on the stack, follow section 7 by hand; the scenario only runs the
fixture.

## 9. Troubleshooting

The Maker discards the worker's stderr, so a -32603 does not say why. Run the worker by hand
(section 4) with the same module and `--now-unix-seconds "$(date +%s)"`: its stderr names the rule.

| code | message | cause | check |
|---|---|---|---|
| -32004 | price source has no quote for the requested route | module returned MISSING | the route's pair/direction are ones your module prices |
| -32003 | price source is temporarily unavailable | module returned UNAVAILABLE, or the route uses `logos-c-api` and the Node runs without a module | the module's data source; the Node's `--logos-price-*` flags (`ps`), the two variables |
| -32003 | price source worker timed out | worker ran past the deadline (default 1 s) | blocking I/O in the module; `--logos-price-timeout-milliseconds` (max 5000) |
| -32603 | external price source failed validation | a file check failed (section 5), wrong sha256, the worker exited non-zero (crash, INVALID_REQUEST, malformed, stale or future quote, wrong route, nonzero reserved), output over 4096 bytes, or an unreduced ratio | `ls -ln` of the file and its directory, `sha256sum`, then a hand run of the worker |
| -32009 | maker price source revision conflicts with durable quote history | same revision as the stored head with a different `as_of` or price | revisions must change with every observation (section 3) |
| -32009 | maker price source revision rolled back | revision lower than the head, or higher with an earlier `as_of` | revision source restarted from a lower number; clocks |
| -32602 | maker offer is invalid | the quote aged past the max age between quote and commit, or other offer validation | `as_of` close to the limit; report the observation time honestly and keep it fresh |
| -32602 | maker route does not use the local price source | `maker_offer_publish_v1` on a `logos-c-api` route | use `maker_offer_publish` / `publish-offer` |
| -32602 | chain dependency is unavailable for route | route health says the route's chain is down (not price related) | `lez-maker-cli health` |
| Node does not start | "Logos price worker, module, and module SHA-256 must be configured together" / "validate Logos price source" | missing flag, bad hex, or the file fails section 5 at start | the three flags; file rules |

## 10. The boundary, and what does not exist

The external price source is a shared object that exports the two symbols of this repository's
C ABI (`crates/logos-price-c-api/include/lez_logos_price_api.h`). The Maker never loads it:
`lez-logos-price-worker`, a separate process started for one quote and then gone, does. A route
selects it with `--price-source logos-c-api`, and the Node accepts only the one file whose
SHA-256 it was started with.

- **How a Logos module provides a price.** By being, or shipping, such a shared object. The
  Maker does not call a price module through Logos core: there is no `logosAPI` call, no `.lidl`
  interface and no `.lgx` package on this path. A Logos module that has prices exports
  `lez_logos_price_abi_version_v1` and `lez_logos_price_quote_v1`, from its own library or from
  a small adapter library built beside it, and the operator gives the Maker that file's absolute
  path and SHA-256 (section 7). The object is loaded by `dlopen` in a process with an empty
  environment that lives for one call (section 2), so it must answer on its own, from a file or
  from its own I/O within the deadline; it cannot rely on a running Logos core or on state held
  in memory.
- **The only module exercised is the repository's fixture.** No real Logos price module is
  shipped, built or referenced anywhere in this repository. The crate tests and the stack
  scenario of section 8 run `crates/logos-price-c-api/tests/fixtures/logos_price_fixture.c`, and
  the hand run of section 6 ran the example beside this page; F8's evidence is that fixture. No
  real feed, its staleness or its failures are exercised.
- The ABI is this repository's own definition (`crates/logos-price-c-api`). No Logos-side
  specification of a price module was found in the tree: `third_party/` (logos-execution-zone,
  logos-execution-zone-module, lez-indexer-module, logos-zebrad-module) and `docs/research/`
  contain nothing about price modules or oracles. The crate's own comments call the contract
  "provisional" (lib.rs:20, 217).
- There is no spread or strategy layer: the module's ratio is published as it is
  (`docs/api/README.md`, the paragraph on the legacy `maker_offer_publish`).
