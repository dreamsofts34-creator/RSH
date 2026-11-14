#pragma once
// RSH WS2812 strip — RMT bitstream encoder (Phase H WS2, FW-12).
// Authority: PROTOCOL-V2.1.md §3.7 (LED_TEST), led_patterns.h (pattern
// engine), led_driver.h (sink boundary), docs/PHASE-G-WS5-HANDOFF.md §3
// (sink choice: RMT — hardware timing exact, CPU free).
//
// Sink choice (W7 note): RMT, bit-bang NAHI. Wire timing WS2812 ke liye
// sakht hai (sub-microsecond) aur 200 Hz loop starve nahi hona chahiye —
// RMT peripheral frame hardware se bhejta hai, CPU sirf items likhta
// hai aur wapas loop mein chala jata hai. Koi external library nahi:
// ye encoder + led_sink_rmt.cpp mein ESP-IDF ka built-in legacy RMT
// driver (driver/rmt.h — installed Arduino core 2.0.17 / IDF 4.4 mein
// maujood; koi lib_deps, koi version/SHA pin karne wali dependency
// nahi) istemal hota hai. NeoPixel lib bhi nahi li — us ki timing
// interrupt-sensitive hoti aur ek aur external dependency hoti.
//
// Ye file Arduino se free hai: sirf pure encode logic hai, taake host
// tests (tests-host/test_main.cpp §27) bitstream/timing assert kar
// sakein. Asal RMT channel setup + transmit led_sink_rmt.cpp mein hai.
//
// Timing (WS2812B datasheet, 800 kHz):
//   RMT tick 100 ns (APB 80 MHz / clk_div 8 — led_sink_rmt.cpp).
//   T0: high 400 ns / low 800 ns  (datasheet 400±150 / 850±150)
//   T1: high 800 ns / low 400 ns  (datasheet 800±150 / 450±150)
//   Bit period 1200 ns (datasheet 1250±600 ns) — dono bits ek jaisi.
//   Reset: line ≥ 50 us LOW (yahan aakhri item 500 ticks = 50 us).
// Byte order: WS2812 GRB leti hai (pehle Green, phir Red, phir Blue),
// har byte MSB-first. Frame: count*24 items + 1 reset item.
#include <stdint.h>
#include <stddef.h>

// RMT tick (led_sink_rmt.cpp: clk_div 8 @ 80 MHz APB).
#define RSH_WS2812_TICK_NS 100u

// Bit timing — ticks (× 100 ns = ns) aur ns dono, taake host test
// datasheet range ke khilaf assert kar sake.
#define RSH_WS2812_T0H_TICKS 4u    // 400 ns (datasheet 250..550)
#define RSH_WS2812_T0L_TICKS 8u    // 800 ns (datasheet 700..1000)
#define RSH_WS2812_T1H_TICKS 8u    // 800 ns (datasheet 650..950)
#define RSH_WS2812_T1L_TICKS 4u    // 400 ns (datasheet 300..600)
#define RSH_WS2812_T0H_NS (RSH_WS2812_T0H_TICKS * RSH_WS2812_TICK_NS)
#define RSH_WS2812_T0L_NS (RSH_WS2812_T0L_TICKS * RSH_WS2812_TICK_NS)
#define RSH_WS2812_T1H_NS (RSH_WS2812_T1H_TICKS * RSH_WS2812_TICK_NS)
#define RSH_WS2812_T1L_NS (RSH_WS2812_T1L_TICKS * RSH_WS2812_TICK_NS)
// Reset: line LOW, kam az kam 50 us (datasheet ≥ 50 us).
#define RSH_WS2812_RESET_TICKS 500u  // 50 us

// Strip limits (led_driver.h RSH_LED_DRIVER_MAX se milti hain).
#define RSH_WS2812_MAX_LEDS 60
// Ek frame ke items: har LED 24 bits + aakhri reset item.
#define RSH_WS2812_MAX_ITEMS (RSH_WS2812_MAX_LEDS * 24 + 1)

// Ek RMT item (rmt_item32_t ka Arduino-free mirror: high hissa phir
// low hissa; level hamesha 1 phir 0 hota hai — WS2812 non-inverted).
typedef struct {
  uint16_t duration0;  // high hissa (ticks)
  uint8_t level0;      // hamesha 1
  uint16_t duration1;  // low hissa (ticks)
  uint8_t level1;      // hamesha 0
} RshWs2812Item;

// Ek LED ke liye kitne items (24) + reset (1).
static inline uint16_t rsh_ws2812_item_count(uint8_t count) {
  return (uint16_t)((uint16_t)count * 24u + 1u);
}

// Poori frame bhejne ka takhmina waqt (us, integer math):
// har bit 1200 ns + reset 50 us. 16 LEDs -> ~511 us; 60 LEDs -> 1778 us.
// 200 Hz loop budget (3500 us) se neeche — aur RMT hardware bhejta hai,
// CPU block nahi hota (aakhri chunk wait_tx_done=false, led_sink_rmt.cpp).
static inline uint32_t rsh_ws2812_frame_us(uint8_t count) {
  return (uint32_t)(((uint32_t)count * 24u * 12u + 500u) / 10u);
}

// RGB pixels -> RMT items (GRB, MSB-first) + aakhri reset item.
// pixels: RshLedRgb jaisi layout {r, g, b} — struct yahan define nahi
// (led_driver.h se aati hai); is liye pixels ko byte triples ke tor
// par lo: har pixel 3 bytes (r, g, b). Return: likhe gaye items, ya 0
// (pixels/items NULL, count 0 ya > 60, capacity kam).
// NOTE: `pixels` ko RshLedRgb* ke tor par bhi pass kar sakte ho —
// layout ek jaisi hai (3 bytes, r/g/b tarteeb).
static inline uint16_t rsh_ws2812_encode(const uint8_t *pixels, uint8_t count,
                                         RshWs2812Item *items, uint16_t capacity) {
  if (!pixels || !items || count == 0 || count > RSH_WS2812_MAX_LEDS) return 0;
  uint16_t need = rsh_ws2812_item_count(count);
  if (capacity < need) return 0;
  uint16_t n = 0;
  for (uint8_t i = 0; i < count; i++) {
    // GRB order: pehle G, phir R, phir B; har byte MSB-first.
    uint8_t grb[3] = {pixels[(size_t)i * 3 + 1], pixels[(size_t)i * 3 + 0],
                      pixels[(size_t)i * 3 + 2]};
    for (uint8_t c = 0; c < 3; c++) {
      for (int8_t bit = 7; bit >= 0; bit--) {
        uint8_t one = (uint8_t)((grb[c] >> bit) & 1u);
        items[n].level0 = 1;
        items[n].level1 = 0;
        if (one) {
          items[n].duration0 = RSH_WS2812_T1H_TICKS;
          items[n].duration1 = RSH_WS2812_T1L_TICKS;
        } else {
          items[n].duration0 = RSH_WS2812_T0H_TICKS;
          items[n].duration1 = RSH_WS2812_T0L_TICKS;
        }
        n++;
      }
    }
  }
  // Reset item: line 50 us LOW (is ke baad RMT idle level bhi LOW hai).
  items[n].duration0 = RSH_WS2812_RESET_TICKS;
  items[n].level0 = 0;
  items[n].duration1 = 0;
  items[n].level1 = 0;
  n++;
  return n;
}
