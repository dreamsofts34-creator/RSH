#pragma once
// RSH heap + restart policy — FW-10 (Plan v2.3 D17, F10).
// Authority: RSH-PLAN-v2.3.md D17 — "Restart jab largest free block < 8 KB
// lagatar 10 cycles tak ho, ya free heap 5 min mein 20% se zyada gire.
// Threshold NVS/config mein hoga, hard-code nahi."
//
// Ye file Arduino se bilkul free hai (protocol.h / inputs.h ki tarah) taake
// host par unit test ho sake — ESP.getFreeHeap()/getMaxAllocHeap() ki values
// caller deta hai, policy sirf faisla karti hai.
//
// "Cycle" = heap-check cycle: rsh.ino har 10 s mein ek dafa update() bulata
// hai, is liye 10 cycles = 100 s musalsal fragmentation. OTA ke doran bhi
// check chalta hai (F10) — ye .ino ki wiring hai (heap check OTA early
// return se PEHLE hota hai); policy ko is se koi farq nahi parta.
//
// Purana rule (RSH_MIN_FREE_HEAP 15000, config.h) KHATAM: 15k magic number
// free heap par tha, largest free block par nahi — fragmented heap mein
// free heap kaafi dikh sakta hai jab ke koi 8 KB block allocate na ho sake.
#include <stdint.h>
#include "config.h"

#ifndef RSH_HEAP_MIN_LARGEST_BLOCK
#define RSH_HEAP_MIN_LARGEST_BLOCK 8192   // D17: largest free block < 8 KB
#endif
#ifndef RSH_HEAP_LOW_CYCLES
#define RSH_HEAP_LOW_CYCLES 10           // D17: lagatar 10 cycles
#endif
#ifndef RSH_HEAP_DROP_PCT
#define RSH_HEAP_DROP_PCT 20             // D17: 5 min mein 20% girawat
#endif
#ifndef RSH_HEAP_DROP_WINDOW_MS
#define RSH_HEAP_DROP_WINDOW_MS 300000u  // D17: 5 min window
#endif

// Policy state — ek hi instance rsh.ino mein static rehti hai.
struct HeapPolicy {
  uint32_t lowStreak = 0;      // lagatar cycles jin mein largest < 8 KB
  uint32_t baselineFree = 0;   // window ke shuru mein free heap
  uint32_t windowStartMs = 0;  // window ka shuru (millis)
  uint8_t hasBaseline = 0;     // pehla sample mil gaya?
  uint8_t restartLatched = 0;  // ek dafa trigger ho to latched rehta hai
};

static inline void heap_policy_reset(HeapPolicy *p) {
  p->lowStreak = 0;
  p->baselineFree = 0;
  p->windowStartMs = 0;
  p->hasBaseline = 0;
  p->restartLatched = 0;
}

// Har heap-check cycle par call karo. true = restart karo (ESP.restart()).
// freeHeap = ESP.getFreeHeap(), largestBlock = ESP.getMaxAllocHeap(),
// nowMs = millis(). Koi allocation nahi, koi ESP call nahi — host-testable.
static inline uint8_t heap_policy_update(HeapPolicy *p, uint32_t freeHeap,
                                         uint32_t largestBlock, uint32_t nowMs) {
  if (!p) return 0;
  if (p->restartLatched) return 1;

  // Rule 1 (D17): largest free block < 8 KB lagatar 10 cycles.
  if (largestBlock < (uint32_t)RSH_HEAP_MIN_LARGEST_BLOCK) {
    if (p->lowStreak < 0xFFFFFFFFu) p->lowStreak++;
    if (p->lowStreak >= (uint32_t)RSH_HEAP_LOW_CYCLES) {
      p->restartLatched = 1;
      return 1;
    }
  } else {
    p->lowStreak = 0;   // ek saaf cycle streak tor deta hai
  }

  // Rule 2 (D17): free heap 5-min window mein 20% se zyada gire.
  if (!p->hasBaseline) {
    // Pehla sample: baseline banao, koi faisla nahi (freeHeap==0 bhi
    // baseline hi hai — agle sample se mawazna hoga, divide-by-zero nahi).
    p->baselineFree = freeHeap;
    p->windowStartMs = nowMs;
    p->hasBaseline = 1;
    return 0;
  }
  if ((uint32_t)(nowMs - p->windowStartMs) >= (uint32_t)RSH_HEAP_DROP_WINDOW_MS) {
    // Window khatam: dobara baseline banao (slow legit girawat bhi
    // eventually trigger na kare — sirf ek window ke andar ki girawat
    // dekhi jati hai).
    p->baselineFree = freeHeap;
    p->windowStartMs = nowMs;
    return 0;
  }
  if (p->baselineFree > 0) {
    // freeHeap < 80% of baseline  <=>  drop > 20%. 64-bit se overflow nahi.
    if ((uint64_t)freeHeap * 100u < (uint64_t)p->baselineFree * (100u - (uint32_t)RSH_HEAP_DROP_PCT)) {
      p->restartLatched = 1;
      return 1;
    }
  }
  return 0;
}
