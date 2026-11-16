#pragma once
// RSH pairing window — transport-independent state (F-02/F-03, v4.0.2).
//
// Purana model: window sirf AP-up clock (s_apUpMs) se chalti thi — 60 s,
// sirf boot par, dobara kabhi nahi khulti thi (Phase 0 gate §4: user ka
// "No pairing window" bug isi liye tha). Naya model: ek deadline
// (`open_until_ms`) jo koi bhi trigger set kar sakta hai:
//
//   * AP-up (pehli dafa): now + 60 s (purana behaviour, barqarar).
//   * Pair button (BOOT 3 s hold, release < 5 s): now + 120 s (F-02).
//   * Serial HELO, unpaired board par: now + 120 s, har HELO par refresh
//     (F-03 — USB cable = physical access, trust anchor).
//
// Window sirf unpaired board par khulti hai; paired board par open_*
// no-op hai. Sab comparisons uint32 wrap-safe hain (millis ~49 din).
// Ye file Arduino se free hai — keystore.cpp is struct ko hold karta
// hai, host tests yahin se logic cover karte hain.
#include <stdint.h>

#define RSH_PAIR_WINDOW_BOOT_MS    60000u   // AP-up window (purana, §2.3)
#define RSH_PAIR_WINDOW_BUTTON_MS  120000u  // pair button / serial window (F-02/F-03)

typedef struct {
  uint32_t open_until_ms;  // window kab khatam hoti hai (millis)
  uint8_t is_set;          // 1 = kabhi kholi gayi (deadline valid)
} RshPairWindow;

static inline void rsh_pair_window_init(RshPairWindow *w) {
  if (!w) return;
  w->open_until_ms = 0;
  w->is_set = 0;
}

// Window kholo (ya extend karo): deadline = now + duration_ms.
// Paired board par caller ko bulana hi nahi chahiye — yahan bhi guard
// hai (unpaired==0 → no-op), taake ghalat caller bhi window na khole.
static inline void rsh_pair_window_open(RshPairWindow *w, uint32_t now_ms,
                                        uint32_t duration_ms, uint8_t unpaired) {
  if (!w || !unpaired) return;
  w->open_until_ms = now_ms + duration_ms;
  w->is_set = 1;
}

// Window abhi khuli hai? Sirf unpaired + deadline ke andar.
static inline uint8_t rsh_pair_window_is_open(const RshPairWindow *w,
                                              uint32_t now_ms, uint8_t unpaired) {
  if (!w || !unpaired || !w->is_set) return 0;
  return (int32_t)(w->open_until_ms - now_ms) > 0 ? 1 : 0;
}

// Baqi waqt, poore seconds mein (0 = band). GET_DIAG (F-10) aur app ka
// conservative countdown isi se banta hai — HELLO mein jagah NAHI hai
// (F-16: byte 17 reserved, parser reject karta hai — protocol frozen).
static inline uint16_t rsh_pair_window_remaining_s(const RshPairWindow *w,
                                                   uint32_t now_ms,
                                                   uint8_t unpaired) {
  if (!rsh_pair_window_is_open(w, now_ms, unpaired)) return 0;
  uint32_t left_ms = w->open_until_ms - now_ms;  // is_open ne wrap-safe check kiya
  uint32_t s = left_ms / 1000u;
  return s > 0xFFFFu ? 0xFFFF : (uint16_t)s;
}
