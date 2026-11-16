#pragma once
// RSH HORN_TEST — horn button ka synthetic test pulse (D-5, v4.0.2).
//
// Horn board par ek INPUT hai (button, GPIO4) — usay "bajaya" nahi ja
// sakta. HORN_TEST (control cmd 0x0E) is liye stream mein horn button
// bit ko ek muddat ke liye set karta hai, taake app/game ko horn press
// nazar aaye aur poora output chain (board → app → game) test ho sake.
// Duration 100..5000 ms (payload: duration_ms u16 LE). Kamyab test
// dobara bhejne par timer dobara shuru hota hai (idempotent-ish).
// Ye file Arduino se free hai; state net.cpp mein rehti hai, rsh.ino
// har sample par net_horn_test_active() poochta hai.
#include <stdint.h>

#define RSH_HORN_TEST_MIN_MS 100u
#define RSH_HORN_TEST_MAX_MS 5000u

typedef struct {
  uint32_t until_ms;  // test kab khatam hota hai (millis)
  uint8_t active;     // 1 = test chal raha hai
} RshHornTest;

static inline void rsh_horn_test_init(RshHornTest *t) {
  if (!t) return;
  t->until_ms = 0;
  t->active = 0;
}

// Test shuru karo. Return 1 = OK, 0 = duration had se bahar (100..5000).
static inline uint8_t rsh_horn_test_start(RshHornTest *t, uint16_t duration_ms,
                                          uint32_t now_ms) {
  if (!t) return 0;
  if (duration_ms < RSH_HORN_TEST_MIN_MS || duration_ms > RSH_HORN_TEST_MAX_MS)
    return 0;
  t->until_ms = now_ms + duration_ms;
  t->active = 1;
  return 1;
}

// Test abhi active hai? Expiry par active khud saaf ho jata hai.
static inline uint8_t rsh_horn_test_active(RshHornTest *t, uint32_t now_ms) {
  if (!t || !t->active) return 0;
  if ((int32_t)(now_ms - t->until_ms) >= 0) {
    t->active = 0;
    return 0;
  }
  return 1;
}
