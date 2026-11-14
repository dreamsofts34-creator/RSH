#pragma once
// RSH WS2812 strip — RMT hardware sink (Phase H WS2, FW-12).
// Authority: PROTOCOL-V2.1.md §3.7, ws2812_rmt.h (bitstream encoder),
// led_driver.h (rsh_led_sink_fn signature), docs/PHASE-G-WS5-HANDOFF.md §3.
//
// Ye sink `rsh_led_sink_fn` hai — RshLedDriver is se jurta hai
// (net.cpp). RMT peripheral frame hardware se bhejta hai:
//   - 16 LEDs ki frame ~511 us (ws2812_rmt.h: rsh_ws2812_frame_us),
//     60 LEDs ~1778 us — 200 Hz loop budget (3500 us) se neeche.
//   - CPU block nahi hota: aakhri chunk wait_tx_done=false se jata hai;
//     beech wale chunks (sirf > 21 LEDs par) wait karte hain, kul
//     block bhi budget ke andar rehta hai (60 LEDs: ~1.3 ms).
//   - RMT memory: mem_block_num 8 = 512 items; frame chunked likhi
//     jati hai (512 items/chunk). Chunk ke beech line LOW rehti hai
//     chand us — 50 us reset threshold se bohat neeche, is liye WS2812
//     stream ko musalsal samajhta hai.
//   - In-flight guard: pichli frame abhi bhej raha ho to nayi frame
//     skip hoti hai (dobara likhne se memory corrupt hoti). 200 Hz loop
//     mein (5 ms tick vs 0.5 ms frame) ye kabhi trigger nahi hota —
//     sirf burst ticks par safety hai; agla badla hua frame phir bhi
//     jata hai (driver change-detection har tick chalata hai).
//
// Arduino-only file hai (driver/rmt.h + micros): host tests is ko
// compile nahi karte — encode logic ws2812_rmt.h mein host-testable hai.
// GPIO: init jis pin par ho, RMT channel 0 usi par bandhta hai.
// Config se strip pin badle (expanders -> freed pin, R-L9) to init
// dobara us pin par hota hai (net.cpp adapter) — channel dobara lagta
// hai, driver ko chhoona nahi parta.
#include <stdint.h>
#include "led_driver.h"  // RshLedRgb + rsh_led_sink_fn

#ifdef __cplusplus
extern "C" {
#endif

// RMT sink tayyar karo (ya dobara lagao — pin badla ho to).
// Return 1 (OK) / 0 (pin invalid / RMT config/install fail). Fail ho
// to sink ready nahi hota aur led_test_request NULL rehta hai
// (fail closed — net.cpp). Dobara call mehfooz hai (pehle uninstall).
uint8_t rsh_led_rmt_sink_init(uint8_t pin);

// Sink ready hai? (init kamyab hua ho aur write error na aaya ho.)
uint8_t rsh_led_rmt_sink_ready(void);

// Sink is waqt kis pin par hai (0xFF = kabhi init nahi hua).
uint8_t rsh_led_rmt_sink_pin(void);

// Asal sink — rsh_led_sink_fn signature (led_driver.h). user ignore
// hota hai (static channel state). Ready na ho, pixels NULL hon, ya
// count 0/>60 ho to kuch nahi likhta. Write error aaye to sink
// mustaqil not-ready ho jata hai (dobara hammer nahi karta); A7
// visual check is ko pakarta hai (strip andheri rehti hai).
void rsh_led_rmt_sink(void *user, const RshLedRgb *pixels, uint8_t count);

#ifdef __cplusplus
}
#endif
