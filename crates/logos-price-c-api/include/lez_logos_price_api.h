#ifndef LEZ_LOGOS_PRICE_API_H
#define LEZ_LOGOS_PRICE_API_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

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

typedef uint32_t (*lez_logos_price_abi_version_v1_fn)(void);
typedef int32_t (*lez_logos_price_quote_v1_fn)(
    const struct lez_logos_price_request_v1 *request,
    struct lez_logos_price_response_v1 *response);

uint32_t lez_logos_price_abi_version_v1(void);
int32_t lez_logos_price_quote_v1(
    const struct lez_logos_price_request_v1 *request,
    struct lez_logos_price_response_v1 *response);

#ifdef __cplusplus
}
#endif

#endif
