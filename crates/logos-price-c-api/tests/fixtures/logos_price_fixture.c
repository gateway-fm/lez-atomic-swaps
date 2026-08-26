#include "lez_logos_price_api.h"

#include <stdlib.h>
#include <string.h>

#if defined(FIXTURE_MISSING_SYMBOL)
int32_t unrelated_symbol(void) { return 0; }
#else
uint32_t lez_logos_price_abi_version_v1(void) {
#if defined(FIXTURE_WRONG_VERSION_SYMBOL)
  return LEZ_LOGOS_PRICE_ABI_VERSION + 1u;
#else
  return LEZ_LOGOS_PRICE_ABI_VERSION;
#endif
}

int32_t lez_logos_price_quote_v1(
    const struct lez_logos_price_request_v1 *request,
    struct lez_logos_price_response_v1 *response) {
#if defined(FIXTURE_ABORT)
  abort();
#endif
  if (request == NULL || response == NULL ||
      request->struct_size != sizeof(*request) ||
      request->abi_version != LEZ_LOGOS_PRICE_ABI_VERSION) {
    return LEZ_LOGOS_PRICE_INVALID_REQUEST;
  }
  memset(response, 0, sizeof(*response));
  response->struct_size = sizeof(*response);
#if defined(FIXTURE_WRONG_ABI)
  response->abi_version = LEZ_LOGOS_PRICE_ABI_VERSION + 1u;
#else
  response->abi_version = LEZ_LOGOS_PRICE_ABI_VERSION;
#endif
  response->pair = request->pair;
  response->direction = request->direction;
  response->lez_units_per_lot = 5u;
  response->foreign_units_per_lot = 2u;
  response->source_revision = 7u;
  response->as_of_unix_seconds = request->now_unix_seconds - 5u;

#if defined(FIXTURE_STALE)
  response->as_of_unix_seconds = request->now_unix_seconds - 61u;
#endif
#if defined(FIXTURE_FUTURE)
  response->as_of_unix_seconds = request->now_unix_seconds + 1u;
#endif
#if defined(FIXTURE_ZERO_PRICE)
  response->lez_units_per_lot = 0u;
#endif
#if defined(FIXTURE_ZERO_REVISION)
  response->source_revision = 0u;
#endif
#if defined(FIXTURE_WRONG_ROUTE)
  response->pair = LEZ_LOGOS_PRICE_PAIR_BITCOIN;
#endif
#if defined(FIXTURE_RESERVED)
  response->reserved[0] = 1u;
#endif
#if defined(FIXTURE_MISSING)
  return LEZ_LOGOS_PRICE_MISSING;
#endif
#if defined(FIXTURE_UNAVAILABLE)
  return LEZ_LOGOS_PRICE_UNAVAILABLE;
#endif
  return LEZ_LOGOS_PRICE_OK;
}
#endif
