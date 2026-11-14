#pragma once
// RSH LED patterns — FW-12 (Plan v2.3 R-L9 + PROTOCOL-V2.1.md §3.7 LED_TEST).
// Authority: PROTOCOL-V2.1.md §3.7 — pattern format (8 B request payload):
// patternId 0 OFF / 1 SOLID / 2 BLINK / 3 CHASE / 4 RAINBOW; r/g/b;
// periodMs u16 (100-5000, blink/chase period); repeats u8 (0 = 5 s tak
// chalao phir stop); reserved 0. Unknown patternId -> UNSUPPORTED.
// LED module configured na ho -> UNSUPPORTED.
//
// R-L9 (PIN-RULES.md §5.3): LED test FAIL hoga jab tak GPIO21 digital ke
// tor par use na ho — direct full rig mein LED sirf GPIO21 par fit hoti
// hai (baaqi digital pins liye hue hain). Expanders hon to 21/22 I2C hain
// aur LED freed digital pin par jati hai (preset mein GPIO4).
//
// Ye file Arduino se free hai: pattern engine pure timing logic hai
// (nowMs caller deta hai). Asal WS2812 strip drive karna is file ka kaam
// NAHI — us ke liye strip driver (NeoPixel lib ya RMT) chahiye, jo
// platformio.ini mein abhi maujood nahi; driver FW-8 control channel
// (LED_TEST dispatch) ke saath aayega. Engine ka output (RGB per pixel)
// `rsh_led_pattern_pixel()` se milta hai — driver sirf wohi likhega.
#include <stdint.h>
#include "protocol.h"   // RSH_CTRL_* status codes

// Pattern IDs (§3.7) — wire values, kabhi mat badlo.
#define RSH_LED_OFF     0
#define RSH_LED_SOLID   1
#define RSH_LED_BLINK   2
#define RSH_LED_CHASE   3
#define RSH_LED_RAINBOW 4

// LED_TEST ke liye 5 s default run (repeats == 0, §3.7).
#define RSH_LED_TEST_DEFAULT_MS 5000u
// SEC-H-10 (Phase H W7): kisi bhi LED_TEST pattern ki zyada se zyada
// total run time — is se zyada repeats hon to runtime 60 s par kata
// jata hai (clamp, reject nahi).
#define RSH_LED_TEST_MAX_MS 60000u

// Ek LED_TEST request (wire layout §3.7, 8 B).
typedef struct {
  uint8_t patternId;
  uint8_t r, g, b;
  uint16_t periodMs;   // 100..5000 (SOLID/RAINBOW par bhi cycle ke liye)
  uint8_t repeats;     // 0 = 5 s tak chalao phir stop
  uint8_t reserved;    // hamesha 0
} RshLedPattern;

// Chalta hua pattern — ek hi instance firmware mein rehta hai.
typedef struct {
  RshLedPattern cfg;
  uint32_t startMs;
  uint32_t totalMs;    // kab khatam hona hai (startMs + totalMs par done)
  uint8_t active;
  uint8_t done;
} RshLedPatternState;

// R-L9: LED test is pin par chal sakta hai?
// expanders==0 -> sirf GPIO21 digital (direct full rig). expanders==1 ->
// 21/22 I2C hain, is liye LED kisi aur digital-out pin par honi chahiye
// (GPIO2 status LED kabhi nahi).
static inline uint8_t rsh_led_test_allowed(uint8_t ledPin, uint8_t expanders) {
  if (!expanders) return ledPin == 21;
  if (ledPin == 21 || ledPin == 22 || ledPin == 2) return 0;
  // Digital-out capable pins (PIN-RULES.md §2 table).
  switch (ledPin) {
    case 4: case 5: case 13: case 14: case 16: case 17: case 18: case 19:
    case 23: case 25: case 26: case 27: case 32: case 33:
      return 1;
    default:
      return 0;
  }
}

// 8 B wire payload (§3.7) -> RshLedPattern. Return 1 (OK) / 0 (len != 8).
static inline uint8_t rsh_led_pattern_parse(const uint8_t *payload, uint16_t len,
                                            RshLedPattern *out) {
  if (!payload || !out || len != 8) return 0;
  out->patternId = payload[0];
  out->r = payload[1];
  out->g = payload[2];
  out->b = payload[3];
  out->periodMs = (uint16_t)((uint16_t)payload[4] | ((uint16_t)payload[5] << 8));
  out->repeats = payload[6];
  out->reserved = payload[7];
  return 1;
}

// Pattern request validate karo. Return RSH_CTRL_OK ya error code (§3.9).
// ledConfigured==0 -> UNSUPPORTED (LED module hi nahi hai).
static inline uint8_t rsh_led_pattern_validate(const RshLedPattern *cfg,
                                               uint8_t ledPin, uint8_t expanders,
                                               uint8_t ledConfigured) {
  if (!cfg) return RSH_CTRL_UNSUPPORTED;
  if (!ledConfigured) return RSH_CTRL_UNSUPPORTED;
  if (cfg->patternId > RSH_LED_RAINBOW) return RSH_CTRL_UNSUPPORTED;
  if (cfg->reserved != 0) return RSH_CTRL_UNSUPPORTED;
  if (!rsh_led_test_allowed(ledPin, expanders)) return RSH_CTRL_PIN_CONFLICT;
  if (cfg->patternId == RSH_LED_BLINK || cfg->patternId == RSH_LED_CHASE ||
      cfg->patternId == RSH_LED_RAINBOW) {
    if (cfg->periodMs < 100 || cfg->periodMs > 5000) return RSH_CTRL_UNSUPPORTED;
  }
  return RSH_CTRL_OK;
}

static inline void rsh_led_pattern_start(RshLedPatternState *st,
                                         const RshLedPattern *cfg, uint32_t nowMs) {
  st->cfg = *cfg;
  st->startMs = nowMs;
  // Ek repeat ka matlab: BLINK/CHASE/RAINBOW ke liye ek poora period/cycle;
  // SOLID ke liye 1 s. repeats==0 -> 5 s (§3.7).
  uint32_t oneRepeatMs;
  if (cfg->patternId == RSH_LED_SOLID) oneRepeatMs = 1000u;
  else if (cfg->patternId == RSH_LED_OFF) oneRepeatMs = 0;
  else oneRepeatMs = cfg->periodMs ? cfg->periodMs : 1000u;
  uint32_t reps = cfg->repeats ? cfg->repeats : 0;
  st->totalMs = reps ? oneRepeatMs * reps : RSH_LED_TEST_DEFAULT_MS;
  if (cfg->patternId == RSH_LED_OFF) st->totalMs = 0;
  // SEC-H-10 cap (RSH_LED_TEST_MAX_MS upar): 255 x 5000 ms ~21 min
  // nahi — 60 s ke baad pattern khud khatam (all-off).
  if (st->totalMs > RSH_LED_TEST_MAX_MS) st->totalMs = RSH_LED_TEST_MAX_MS;
  st->active = 1;
  st->done = 0;
}

// Hue (0..255) -> RGB (poora wheel, integer math, koi float nahi).
static inline void rsh_led_hue_rgb(uint8_t hue, uint8_t *r, uint8_t *g, uint8_t *b) {
  uint8_t region = (uint8_t)(hue / 43);        // 0..5
  uint8_t rem = (uint8_t)((hue - region * 43) * 6);  // 0..255
  switch (region) {
    case 0: *r = 255; *g = rem; *b = 0; break;
    case 1: *r = (uint8_t)(255 - rem); *g = 255; *b = 0; break;
    case 2: *r = 0; *g = 255; *b = rem; break;
    case 3: *r = 0; *g = (uint8_t)(255 - rem); *b = 255; break;
    case 4: *r = rem; *g = 0; *b = 255; break;
    default: *r = 255; *g = 0; *b = (uint8_t)(255 - rem); break;
  }
}

// Is waqt pixel `index` (0..ledCount-1) ka rang nikalo. done==1 ho to
// (0,0,0) milta hai — test khatam hone par strip purani state par wapas
// (caller purani state restore karta hai, §3.7).
static inline void rsh_led_pattern_pixel(const RshLedPatternState *st, uint32_t nowMs,
                                         uint8_t index, uint8_t ledCount,
                                         uint8_t *r, uint8_t *g, uint8_t *b) {
  *r = 0; *g = 0; *b = 0;
  if (!st || !st->active) return;
  if (st->cfg.patternId == RSH_LED_OFF) return;
  uint32_t elapsed = nowMs - st->startMs;   // wrap-safe (uint32 subtraction)
  if (st->totalMs > 0 && elapsed >= st->totalMs) return;  // khatam — off
  const RshLedPattern *c = &st->cfg;
  switch (c->patternId) {
    case RSH_LED_SOLID:
      *r = c->r; *g = c->g; *b = c->b;
      break;
    case RSH_LED_BLINK: {
      // period/2 on, period/2 off.
      uint32_t phase = elapsed % (uint32_t)c->periodMs;
      if (phase < (uint32_t)c->periodMs / 2u) { *r = c->r; *g = c->g; *b = c->b; }
      break;
    }
    case RSH_LED_CHASE: {
      // Ek lit pixel poori strip par ghoomta hai; ek full cycle = periodMs.
      if (ledCount == 0) break;
      uint32_t stepMs = (uint32_t)c->periodMs / ledCount;
      if (stepMs == 0) stepMs = 1;
      uint8_t lit = (uint8_t)((elapsed / stepMs) % ledCount);
      if (index == lit) { *r = c->r; *g = c->g; *b = c->b; }
      break;
    }
    case RSH_LED_RAINBOW: {
      // Hue poora wheel periodMs mein; har pixel hue mein offset rakhta hai.
      uint8_t hue = (uint8_t)((elapsed * 256u / (uint32_t)c->periodMs) & 0xFF);
      if (ledCount > 1)
        hue = (uint8_t)(hue + (uint8_t)((uint16_t)index * 256u / ledCount));
      rsh_led_hue_rgb(hue, r, g, b);
      break;
    }
    default:
      break;
  }
}

// Pattern khatam hua? (caller is par purani LED state restore karta hai.)
static inline uint8_t rsh_led_pattern_done(const RshLedPatternState *st, uint32_t nowMs) {
  if (!st || !st->active) return 1;
  if (st->cfg.patternId == RSH_LED_OFF) return 1;
  if (st->totalMs == 0) return 0;
  return (uint32_t)(nowMs - st->startMs) >= st->totalMs;
}
