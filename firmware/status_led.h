#pragma once
#include <stdint.h>

enum class LedState : uint8_t {
  ApUpWaitingPc,   // 2 Hz blink
  PcConnected,     // solid on
  Error,           // 8 Hz tez blink
  Updating,        // 5 Hz blink (OTA chal raha hai)
  SafeMode,        // 1 Hz slow blink (FW-3: BOOT-hold 5 s — §4.4)
  ErrorCode        // F-10 (v4.0.2): blink count se error code (diag_log.h)
// v2.2 DELETE: KeyResetArmed (koi armed state nahi — 10 s hold seedha
// password clear + reboot hai, D7) aur PairWindow (pairing khatam).
};

void led_begin();
void led_set(LedState s);
// Error code set karo (blink count 1..9): led_set(ErrorCode) jaisa,
// lekin blink count bhi. Mapping diag_log.h mein (heap-low 2, config
// rollback 3, net/AP fail 4, OTA fail 5).
void led_set_error_blinks(uint8_t blinks);
void led_update(uint32_t nowMs);   // loop mein har dafa call karo (non-blocking)
