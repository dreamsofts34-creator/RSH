#pragma once
// RSH inputs — debounce logic yahan saf (pure) hai taake host par test ho sake.
#include <stdint.h>
#include "config.h"

// Button debouncer: state tab badalta hai jab raw value lagataar
// `needed` samples tak nayi value par rahe (docs: 2 x 5 ms = 10 ms).
struct Debouncer {
  bool state = false;       // debounced state (true = pressed)
  uint8_t streak = 0;       // lagataar samples jo state se mukhtalif hain
  uint8_t needed = DEBOUNCE_SAMPLES;

  // Har sample par call karo. Debounced state return hota hai.
  bool update(bool rawPressed) {
    if (rawPressed == state) {
      streak = 0;
    } else {
      streak++;
      if (streak >= needed) {
        state = rawPressed;
        streak = 0;
      }
    }
    return state;
  }
};

// Trimmed mean: samples ko sort karke dono taraf se `trim` extreme values
// hata kar baqi ka average — ADC ki occasional spike readings steering ko
// hila nahi sakti (plain average spike ko ghaseet leta hai).
// n <= 32 hona chahiye; trim * 2 < n lazmi.
static inline uint16_t rsh_trimmed_mean(const uint16_t *v, uint8_t n, uint8_t trim) {
  uint16_t s[32];
  for (uint8_t i = 0; i < n; i++) s[i] = v[i];
  for (uint8_t i = 1; i < n; i++) {           // insertion sort (n chhota hai)
    uint16_t x = s[i];
    int8_t j = (int8_t)i - 1;
    while (j >= 0 && s[j] > x) { s[j + 1] = s[j]; j--; }
    s[j + 1] = x;
  }
  uint32_t sum = 0;
  uint8_t cnt = 0;
  for (uint8_t i = trim; i < (uint8_t)(n - trim); i++) { sum += s[i]; cnt++; }
  return cnt ? (uint16_t)(sum / cnt) : s[n / 2];
}

// Adaptive sampling governor: har window (2 s) ke overruns dekh kar
// samples ki tadaad khud adjust hoti hai — board sust nikle to raftar
// (200/sec) bachane ke liye samples kam, halat theek hon to wapas zyada.
// Levels: 0 = (16 steer, 8 pedal), 1 = (12, 6), 2 = (8, 4)
struct SampleGovernor {
  uint8_t level = 0;
  uint8_t goodWindows = 0;

  void update(uint32_t overrunsInWindow) {
    if (overrunsInWindow > 2) {
      if (level < 2) level++;
      goodWindows = 0;
    } else if (overrunsInWindow == 0) {
      if (goodWindows < 255) goodWindows++;
      if (goodWindows >= 5 && level > 0) {   // 5 saaf windows = ek step wapas
        level--;
        goodWindows = 0;
      }
    } else {
      goodWindows = 0;   // 1-2 overruns: na neeche, na upar
    }
  }
  uint8_t steerSamples() const { return level == 0 ? 16 : (level == 1 ? 12 : 8); }
  uint8_t pedalSamples() const { return level == 0 ? 8 : (level == 1 ? 6 : 4); }
};

// Ek cycle ki tamam readings
struct InputSample {
  uint16_t steering;       // mV ya raw counts (config par munhasir)
  uint16_t accelAnalog;    // raw counts 0-4095 (hall ke liye; button mode mein PC ignore karta hai)
  uint16_t brakeAnalog;    // raw counts 0-4095
  uint8_t buttons;         // RSH_BTN_* bits (protocol.h)
};

void inputs_begin();
InputSample inputs_read();   // ADC averages + debounced buttons, ek cycle
bool inputs_steering_is_mv();  // asal mode jo chal raha hai (fallback ke baad)
struct SampleGovernor;
SampleGovernor &inputs_governor();   // adaptive sampling (neeche dekho)
