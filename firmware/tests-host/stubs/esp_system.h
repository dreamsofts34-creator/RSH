#pragma once
// Host-test stub (TEST-ONLY): esp_random() ka deterministic naql.
// xorshift — har call par alag bytes, kabhi all-zero/all-same nahi,
// taake keystore ka key_weak_ check kabhi trigger na ho.
#include <stdint.h>

static inline uint32_t esp_random() {
  static uint32_t s = 0x12345678u;
  s ^= s << 13;
  s ^= s >> 17;
  s ^= s << 5;
  return s;
}
