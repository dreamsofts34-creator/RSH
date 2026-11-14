#pragma once
// RSH LED strip driver abstraction — Phase G WS5 (FW-12 groundwork).
// Authority: PROTOCOL-V2.1.md §3.7 (LED_TEST), led_patterns.h (pattern
// engine), PIN-RULES.md §5.3 (R-L9 pin rule).
//
// Ye file Arduino se free hai: pattern engine (led_patterns.h) pure
// timing logic hai, aur ye driver us ke output (RGB per pixel) ko ek
// hardware sink tak pahunchata hai. Sink hi asal WS2812 wire protocol
// hai — driver ko is se koi matlab nahi.
//
//   pattern (LED_TEST request) -> rsh_led_driver_start()
//   main loop (200 Hz)         -> rsh_led_driver_tick(nowMs)
//   tick                       -> sink(user, pixels, ledCount)
//
// Sink kab likhta hai (change detection): tick har loop mein chalta hai,
// lekin sink sirf tab bulaya jata hai jab frame pichle frame se badla ho.
// SOLID ek dafa likhta hai phir khamosh; BLINK sirf phase badalne par;
// CHASE sirf lit pixel badalne par; RAINBOW har tick (hue musalsal
// badalta hai). Is se 200 Hz loop mein WS2812 ko be-wajah dobara likhna
// nahi hota (har write ~30 us/pixel × 16 = ~0.5 ms hota — har tick
// likhna loop budget khata).
//
// Phase H WS2 mein lag gaya (docs/PHASE-H-WS2-HANDOFF.md):
//   1. Asal sink: RMT driver — led_sink_rmt.h/.cpp (ESP-IDF legacy
//      RMT, hardware timing; bit-bang nahi). Bitstream encode
//      ws2812_rmt.h mein hai (host-testable).
//   2. net.cpp wiring: `fw34Ctx.led_test_request` driver-start adapter
//      par hai (sink init fail ho to NULL rehta hai — fail closed);
//      loop mein `net_led_tick()` (rsh.ino se) driver tick chalata hai.
//   3. A7 board run: paanchon patterns strip par aankh se (A7-GATE §25).
//
// Fail-closed rules (driver khud):
//   - start() pehle validate karta hai (rsh_led_pattern_validate) —
//     ghalat request chalta hua pattern nahi bigarti (state untouched).
//   - sink NULL ho to init() fail hota hai — driver kabhi aise state
//     mein nahi jata jahan pattern "chal raha" ho lekin likha na jaye.
//   - Pattern khatam hone par driver aakhri frame ke tor par all-off
//     ek dafa likhta hai (strip purani state par — §3.7), phir inactive.
#include <stdint.h>
#include <stddef.h>
#include <string.h>
#include "led_patterns.h"  // RshLedPattern(State), validate, pixel, done

// Validator limit (PIN-RULES.md §4): strip mein zyada se zyada 60 LEDs.
#define RSH_LED_DRIVER_MAX 60

typedef struct {
  uint8_t r, g, b;
} RshLedRgb;

// Hardware sink: pixels ko strip par likho (Phase H: NeoPixel/RMT).
// `pixels` ledCount lambi hoti hai. Sink ko frame ke andar kabhi
// block nahi hona chahiye (WS2812 timing sakht hai — RMT/NeoPixel
// interrupt-safe write istemal karo).
typedef void (*rsh_led_sink_fn)(void *user, const RshLedRgb *pixels,
                                uint8_t count);

typedef struct {
  RshLedPatternState state;   // chalta hua pattern (led_patterns.h)
  RshLedRgb last[RSH_LED_DRIVER_MAX];  // aakhri likha hua frame
  uint8_t ledCount;           // strip mein LEDs (1..60)
  uint8_t frameValid;         // last[] mein frame hai (pehli write ke baad 1)
  uint8_t finalOffSent;       // done hone par all-off likhi ja chuki
  rsh_led_sink_fn sink;       // hardware sink (NULL kabhi nahi hota)
  void *user;                 // sink ka user pointer
} RshLedDriver;

// Driver tayyar karo. Return 1 (OK) / 0 (drv NULL, ledCount 0 ya > 60,
// sink NULL). Init ke baad driver inactive hota hai (koi pattern nahi).
static inline uint8_t rsh_led_driver_init(RshLedDriver *drv, uint8_t ledCount,
                                          rsh_led_sink_fn sink, void *user) {
  if (!drv || !sink || ledCount == 0 || ledCount > RSH_LED_DRIVER_MAX) return 0;
  memset(drv, 0, sizeof(*drv));
  drv->ledCount = ledCount;
  drv->sink = sink;
  drv->user = user;
  return 1;
}

// Pattern shuru karo (LED_TEST handler se — callback ki jagah).
// Pehle validate hota hai; ghalat request RSH_CTRL_* error deti hai aur
// chalta hua pattern waisa ka waisa rehta hai (fail closed). Kamyabi par
// purana pattern (agar chal raha ho) naye se badal jata hai.
static inline uint8_t rsh_led_driver_start(RshLedDriver *drv,
                                           const RshLedPattern *cfg,
                                           uint32_t nowMs, uint8_t ledPin,
                                           uint8_t expanders,
                                           uint8_t ledConfigured) {
  if (!drv || !drv->sink) return RSH_CTRL_UNSUPPORTED;
  uint8_t st = rsh_led_pattern_validate(cfg, ledPin, expanders, ledConfigured);
  if (st != RSH_CTRL_OK) return st;
  rsh_led_pattern_start(&drv->state, cfg, nowMs);
  drv->frameValid = 0;      // pehla frame hamesha likho (OFF bhi — ek dafa)
  drv->finalOffSent = 0;
  return RSH_CTRL_OK;
}

// Ek tick (main loop se, 200 Hz): maujooda frame render karo; badla ho to
// sink likho. Return 1 (sink likha) / 0 (frame waisa hi, ya inactive).
// Pattern khatam ho to aakhri frame all-off hoti hai (ek dafa), phir
// driver inactive ho jata hai — agle ticks kuch nahi karte.
static inline uint8_t rsh_led_driver_tick(RshLedDriver *drv, uint32_t nowMs) {
  if (!drv || !drv->sink || !drv->state.active) return 0;
  uint8_t done = rsh_led_pattern_done(&drv->state, nowMs);
  RshLedRgb frame[RSH_LED_DRIVER_MAX];
  for (uint8_t i = 0; i < drv->ledCount; i++)
    rsh_led_pattern_pixel(&drv->state, nowMs, i, drv->ledCount,
                          &frame[i].r, &frame[i].g, &frame[i].b);
  uint8_t changed = !drv->frameValid ||
      (memcmp(frame, drv->last, (size_t)drv->ledCount * sizeof(RshLedRgb)) != 0);
  if (changed) {
    drv->sink(drv->user, frame, drv->ledCount);
    memcpy(drv->last, frame, (size_t)drv->ledCount * sizeof(RshLedRgb));
    drv->frameValid = 1;
  }
  if (done) {
    // Aakhri frame all-off hai (pixel() done par (0,0,0) deta hai) —
    // wo upar likhi ja chuki (changed ho to) ya pehle se off thi.
    drv->finalOffSent = 1;
    drv->state.active = 0;
  }
  return changed ? 1 : 0;
}

// Pattern abhi roko: strip all-off likho (agar pehle se off nahi), phir
// inactive. Return 1 (all-off likha) / 0 (kuch likhne ki zaroorat nahi
// ya inactive tha).
static inline uint8_t rsh_led_driver_stop(RshLedDriver *drv) {
  if (!drv || !drv->sink || !drv->state.active) return 0;
  drv->state.active = 0;
  drv->finalOffSent = 1;
  uint8_t allOff = 1;
  if (drv->frameValid) {
    for (uint8_t i = 0; i < drv->ledCount; i++)
      if (drv->last[i].r || drv->last[i].g || drv->last[i].b) { allOff = 0; break; }
  } else {
    allOff = 0;  // kabhi frame likhi hi nahi — off likhna banta hai
  }
  if (!allOff) {
    RshLedRgb frame[RSH_LED_DRIVER_MAX];
    memset(frame, 0, sizeof(frame));
    drv->sink(drv->user, frame, drv->ledCount);
    memcpy(drv->last, frame, (size_t)drv->ledCount * sizeof(RshLedRgb));
    drv->frameValid = 1;
    return 1;
  }
  return 0;
}

// Pattern chal raha hai? (start ke baad, done/stop se pehle 1.)
static inline uint8_t rsh_led_driver_active(const RshLedDriver *drv) {
  return (drv && drv->state.active) ? 1 : 0;
}

// Pattern khatam hua? (inactive driver hamesha done hai.)
static inline uint8_t rsh_led_driver_done(const RshLedDriver *drv, uint32_t nowMs) {
  if (!drv || !drv->state.active) return 1;
  return rsh_led_pattern_done(&drv->state, nowMs);
}
