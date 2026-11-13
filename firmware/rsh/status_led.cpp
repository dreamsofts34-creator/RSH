#include "status_led.h"
#include "config.h"
#include <Arduino.h>

static LedState cur = LedState::ApUpWaitingPc;

static inline void led_write(bool on) {
  digitalWrite(PIN_LED, LED_ACTIVE_LOW ? (on ? LOW : HIGH) : (on ? HIGH : LOW));
}

void led_begin() {
  pinMode(PIN_LED, OUTPUT);
  led_write(false);
}

void led_set(LedState s) { cur = s; }

void led_update(uint32_t nowMs) {
  switch (cur) {
    case LedState::PcConnected:
      led_write(true);
      break;
    case LedState::ApUpWaitingPc:            // 2 Hz: 250 ms on / 250 ms off
      led_write((nowMs / 250) % 2 == 0);
      break;
    case LedState::Error:                    // 8 Hz: ~62 ms on / ~62 ms off
      led_write((nowMs / 62) % 2 == 0);
      break;
    case LedState::Updating:                 // 5 Hz: 100 ms on / 100 ms off
      led_write((nowMs / 100) % 2 == 0);
      break;
  }
}
