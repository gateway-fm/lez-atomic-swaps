/* A minimal Logos price module for the Maker (docs/price-module-guide.md).
   It quotes every route at 5 LEZ units per 2 foreign units and treats each
   call as a new observation: the observation time is the request's time and
   the revision is that same second, so revisions increase with time. */
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
