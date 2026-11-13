#pragma once
#include <stdint.h>

enum class LedState : uint8_t {
  ApUpWaitingPc,   // 2 Hz blink
  PcConnected,     // solid on
  Error,           // 8 Hz tez blink
  Updating         // 5 Hz blink (OTA chal raha hai)
};

void led_begin();
void led_set(LedState s);
void led_update(uint32_t nowMs);   // loop mein har dafa call karo (non-blocking)
