#pragma once
// Host-test stub (TEST-ONLY): mbedTLS SHA-256 API ka naql — declarations
// sirf; implementation test_main.cpp mein hai (wahi independent SHA-256
// jo pw_obfuscate tests istemal karte hain). Buffer 512 B kaafi hai:
// keystore ka sha256_ one-shot chhote inputs par chalta hai (obfkey +
// password keystream — hamesha < 512 B).
#include <stdint.h>
#include <stddef.h>

typedef struct {
  uint8_t buf[512];
  size_t len;
} mbedtls_sha256_context;

void mbedtls_sha256_init(mbedtls_sha256_context *ctx);
int mbedtls_sha256_starts_ret(mbedtls_sha256_context *ctx, int is224);
int mbedtls_sha256_update_ret(mbedtls_sha256_context *ctx,
                              const unsigned char *input, size_t ilen);
int mbedtls_sha256_finish_ret(mbedtls_sha256_context *ctx,
                              unsigned char output[32]);
void mbedtls_sha256_free(mbedtls_sha256_context *ctx);
