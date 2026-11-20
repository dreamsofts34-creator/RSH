#include "status_led.h"
#include "config.h"
#include "diag_log.h"  // F-10: rsh_error_blink_on() (pure blink pattern)
#include <Arduino.h>

static LedState cur = LedState::ApUpWaitingPc;
static uint8_t errorBlinks = 4;  // ErrorCode state ka blink count

static inline void led_write(bool on) {
  digitalWrite(PIN_LED, LED_ACTIVE_LOW ? (on ? LOW : HIGH) : (on ? HIGH : LOW));
}

void led_begin() {
  pinMode(PIN_LED, OUTPUT);
  led_write(false);
}

void led_set(LedState s) { cur = s; }

void led_set_error_blinks(uint8_t blinks) {
  errorBlinks = (blinks == 0 || blinks > 9) ? 4 : blinks;
  cur = LedState::ErrorCode;
}

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
    case LedState::SafeMode:                 // 1 Hz: 500 ms on / 500 ms off (§4.4)
      led_write((nowMs / 500) % 2 == 0);
      break;
    case LedState::ErrorCode:                // F-10: N blinks + 1 s pause
      led_write(rsh_error_blink_on(errorBlinks, nowMs) != 0);
      break;
  }
}
