#pragma once
// RSH inputs — debounce / trimmed mean / governor / axis-health logic yahan
// saf (pure) hai taake host par test ho sake.
#include <stdint.h>
#include "config.h"
#include "protocol.h"   // InputSample.enc[RSH_V2_MAX_ENC] (FW-12) ke liye
#include "encoders.h"   // RshEncoder (quadrature decoder, FW-12)
#include "modules.h"    // RshModuleEnable, axis mask / button gating (FW-11)
#include "pinmap.h"     // RshPinMap — live pin table (Phase G, C3)

// Button debouncer — TIME-based (F12): state tab badalta hai jab raw value
// lagataar DEBOUNCE_MS tak nayi value par rahe. Sample-count wala tareeqa
// khatam: cycle time governor level ke saath badalta hai (5 ms -> 10 ms),
// is liye "2 samples" kabhi 10 ms, kabhi 20 ms hota tha.
struct Debouncer {
  bool state = false;            // debounced state (true = pressed)
  bool pendingValue = false;     // nayi value jis ka intezar hai
  bool hasPending = false;
  uint32_t pendingSinceMs = 0;   // pending kab se chal raha hai
  uint16_t debounceMs = DEBOUNCE_MS;

  // Har cycle par call karo, nowMs = millis(). Debounced state return hota hai.
  // (uint32 subtraction wrap-safe hai — millis ~49 din par wrap hota hai.)
  bool update(bool rawPressed, uint32_t nowMs) {
    if (rawPressed == state) {
      hasPending = false;
      return state;
    }
    if (!hasPending || pendingValue != rawPressed) {
      hasPending = true;
      pendingValue = rawPressed;
      pendingSinceMs = nowMs;
      return state;
    }
    if ((uint32_t)(nowMs - pendingSinceMs) >= debounceMs) {
      state = rawPressed;
      hasPending = false;
    }
    return state;
  }
};

// ---- Trimmed mean (F14 optimize) -------------------------------------------
// Pehle poori insertion sort hoti thi (O(n^2) comparisons + moves).
// Ab quickselect se sirf do order statistics nikale jate hain — trim-th
// smallest (L) aur trim-th largest (H) — phir ek pass mein beech wale
// elements ka sum. Average O(n), koi poora sort nahi, stack copy wahi 32.
// Duplicate values sahi handle hoti hain: trim sab se chhoti values ka sum
// (L se chhoti sab + L ki zaroori copies) aur trim sab se bari values ka sum
// (H se bari sab + H ki zaroori copies) kul sum se ghata kar bachta hai.
static inline uint16_t rsh_kth_smallest_(uint16_t *a, uint8_t n, uint8_t k) {
  uint8_t lo = 0, hi = (uint8_t)(n - 1);
  while (true) {
    if (lo == hi) return a[lo];
    // Median-of-three pivot, pivot ko hi par le jao.
    uint8_t mid = (uint8_t)((lo + hi) / 2);
    uint16_t x = a[lo], y = a[mid], z = a[hi];
    uint8_t piv = (x < y) ? ((y < z) ? mid : ((x < z) ? hi : lo))
                          : ((x < z) ? lo : ((y < z) ? hi : mid));
    uint16_t tmp = a[piv]; a[piv] = a[hi]; a[hi] = tmp;
    uint16_t pivot = a[hi];
    uint8_t i = lo;
    for (uint8_t j = lo; j < hi; j++) {
      if (a[j] <= pivot) { tmp = a[i]; a[i] = a[j]; a[j] = tmp; i++; }
    }
    tmp = a[i]; a[i] = a[hi]; a[hi] = tmp;
    if (k == i) return a[i];
    if (k < i) hi = (uint8_t)(i - 1);
    else lo = (uint8_t)(i + 1);
  }
}

// Trimmed mean: samples ko sort karke dono taraf se `trim` extreme values
// hata kar baqi ka average — ADC ki occasional spike readings steering /
// pedals ko hila nahi sakti (plain average spike ko ghaseet leta hai — F6:
// pedals par bhi trimmed mean, plain mean khatam).
// n <= 32 hona chahiye; trim * 2 < n lazmi.
// T14 guards: n=0 par `s[n/2]` uninitialized read tha; n>32 par s[32] stack
// buffer overflow tha; trim*2>=n par koi element nahi bachta tha. Teeno
// ab defined hain — caller ki ghalti kabhi UB nahi banegi.
static inline uint16_t rsh_trimmed_mean(const uint16_t *v, uint8_t n, uint8_t trim) {
  if (!v || n == 0) return 0;             // khaali/null input — koi read nahi
  if (n > 32) n = 32;                     // buffer se zyada kabhi nahi parhte
  if (trim * 2 >= n) trim = (uint8_t)((n - 1) / 2);   // kam se kam ek element bache
  uint16_t s[32];
  uint32_t sumAll = 0;
  for (uint8_t i = 0; i < n; i++) { s[i] = v[i]; sumAll += v[i]; }
  if (trim == 0) return (uint16_t)(sumAll / n);
  uint16_t loVal = rsh_kth_smallest_(s, n, trim);
  uint16_t hiVal = rsh_kth_smallest_(s, n, (uint8_t)(n - trim - 1));
  // Trim sab se chhoti values ka sum...
  uint32_t sumSmall = 0;
  uint16_t cntLess = 0;
  for (uint8_t i = 0; i < n; i++) {
    if (s[i] < loVal) { sumSmall += s[i]; cntLess++; }
  }
  sumSmall += (uint32_t)(trim - cntLess) * loVal;
  // ...aur trim sab se bari values ka sum.
  uint32_t sumLarge = 0;
  uint16_t cntGreater = 0;
  for (uint8_t i = 0; i < n; i++) {
    if (s[i] > hiVal) { sumLarge += s[i]; cntGreater++; }
  }
  sumLarge += (uint32_t)(trim - cntGreater) * hiVal;
  uint32_t mid = sumAll - sumSmall - sumLarge;
  uint8_t cnt = (uint8_t)(n - 2 * trim);
  return (uint16_t)(mid / cnt);
}

// Adaptive sampling governor: har window (2 s) ke overruns dekh kar
// samples ki tadaad khud adjust hoti hai — board sust nikle to raftar
// (200/sec) bachane ke liye samples kam, halat theek hon to wapas zyada.
// Levels (D16): 0 = (16 steer, 8 pedal), 1 = (12, 6), 2 = (8, 4),
//               3 = (4, 4) + 100 pps fallback (SEND_PERIOD 10 ms).
struct SampleGovernor {
  uint8_t level = 0;
  uint8_t goodWindows = 0;
  static const uint8_t kMaxLevel = 3;

  void update(uint32_t overrunsInWindow) {
    if (overrunsInWindow > 2) {
      if (level < kMaxLevel) level++;
      goodWindows = 0;
    } else {
      // F5 fix: 0 ya 1 overrun wali window bhi goodWindows gine.
      // Pehle 1 overrun goodWindows reset kar deta tha, is liye halka
      // dabao bhi recovery ko hamesha ke liye rok deta tha.
      if (goodWindows < 255) goodWindows++;
      if (goodWindows >= 5 && level > 0) {   // 5 saaf windows = ek step wapas
        level--;
        goodWindows = 0;
      }
    }
  }
  uint8_t steerSamples() const {
    return level == 0 ? 16 : (level == 1 ? 12 : (level == 2 ? 8 : 4));
  }
  uint8_t pedalSamples() const {
    return level == 0 ? 8 : (level == 1 ? 6 : 4);
  }
  // Level 3 par loop 100 packets/sec par gir jata hai (D16) — 200 Hz
  // zabardasti rakhne se overrun storm hota hai; raftar bachana behtar hai.
  uint32_t sendPeriodUs() const { return level >= 3 ? 10000u : 5000u; }
  bool fallback100pps() const { return level >= 3; }
};

// ---- Floating / disconnected axis detection (F2, corrected criteria D12) ----
// Invalid SIRF tab jab:
//   (a) value 10 s tak exact rail (0 ya railHigh) par stuck ho AUR doosre
//       axes move kar rahe hon (board zinda hai, ye axis mara hua hai), ya
//   (b) jitter (lagataar samples ke darmiyan |diff| ka EMA) threshold se
//       zyada ho — floating pin ka be-tarteeb noise.
// Sirf variance≈0 (still wheel, pot beech mein ruka hua) KABHI invalid nahi.
// Jitter EMA alpha 1/16 hai: tez lekin mumkin steering sweep (~40 counts/
// sample lock-to-lock 0.5 s mein) threshold 150 se neeche rehta hai, jabke
// floating noise (|diff| aam tor par 300-1000) 10 samples mein pakra jata hai.
struct AxisHealth {
  uint16_t lastValue = 0;
  bool hasLast = false;
  uint32_t emaJitterX256 = 0;  // |diff| ka EMA, x256 fixed-point
  uint32_t railSinceMs = 0;    // rail par kab se (sirf onRail ke saath valid)
  bool onRail = false;         // railSinceMs==0 bhi valid waqt hai (t=0),
                               // is liye alag flag lazmi hai — sentinel nahi

  bool update(uint16_t value, uint16_t railHigh, bool othersMoving, uint32_t nowMs) {
    bool railed = (value == 0 || value >= railHigh);
    if (railed) {
      if (!onRail) { onRail = true; railSinceMs = nowMs; }
    } else {
      onRail = false;
      railSinceMs = 0;
    }
    if (hasLast) {
      uint32_t d = value > lastValue ? (uint32_t)(value - lastValue)
                                     : (uint32_t)(lastValue - value);
      emaJitterX256 = (emaJitterX256 * 15 + d * 256) / 16;
    }
    lastValue = value;
    hasLast = true;
    if (onRail && (uint32_t)(nowMs - railSinceMs) >= RSH_AXIS_RAIL_MS
        && othersMoving)
      return true;
    if (emaJitterX256 / 256 > RSH_AXIS_JITTER_THRESHOLD) return true;
    return false;
  }
};

// ---- mV fallback decisions (F13) — pure helpers, inputs.cpp istemal karta hai
// Boot par bhi aur periodic recheck par bhi yahi faisla hota hai.
static inline bool rsh_mv_should_fallback(uint32_t rawSum, uint32_t mvSum, uint8_t n) {
  return n > 0 && mvSum == 0 && rawSum > (uint32_t)n * 200;
}
static inline bool rsh_mv_should_restore(uint32_t mvSum, uint8_t n) {
  return n > 0 && mvSum > (uint32_t)n * 50;
}

// ---- Pedal analog button mode (F7 / D14) ------------------------------------
// Button mode mein analog read skip hota hai — sirf har 20th cycle (10 Hz
// @ 200 Hz) ek diagnostic read hota hai, baqi cycles mein pichli value.
static inline bool rsh_pedal_should_read(bool analogMode, uint32_t cycle) {
  return analogMode || (cycle % 20) == 0;
}

// Ek cycle ki tamam readings
struct InputSample {
  uint16_t steering;       // mV ya raw counts (config par munhasir)
  uint16_t accelAnalog;    // raw counts 0-4095 (hall ke liye; button mode mein PC ignore karta hai)
  uint16_t brakeAnalog;    // raw counts 0-4095
  uint16_t clutch;         // Phase F WS5: raw counts 0-4095; module band ho to 0
                           // (band module ka ADC read hi nahi hota — F7)
  uint16_t handbrake;      // FW-11: raw counts 0-4095; module band ho to 0
                           // (band module ka ADC read hi nahi hota — F7)
  int16_t enc[RSH_V2_MAX_ENC];  // FW-12: cumulative i16 counts (C3 fix);
                                 // enc[0..1] istemal hote hain (2 encoders),
                                 // enc[2..3] hamesha 0
  uint8_t shifterGear;     // FW-11: H-pattern gear 0=neutral, 1..6
                           // (sequential mode mein hamesha 0)
  uint8_t buttons;         // RSH_BTN_* bits (protocol.h)
};

void inputs_begin();
InputSample inputs_read();   // ADC averages + debounced buttons, ek cycle
bool inputs_steering_is_mv();  // asal mode jo chal raha hai (fallback ke baad)
// Config blob (SET_CFG, §3.6) se live pin table lagao — COMMIT / boot /
// rollback par .ino bulata hai (Phase G, C3). Blob invalid ho to 0 aur
// pin table waisi ki waisi (fail closed); valid ho to 1. Pin badle hon
// to pinMode dobara + debouncers/encoders/health reset hote hain (purane
// pin ki state naye pin par ghalat trigger na kare); map waisa hi ho to
// kuch nahi hota (stream mein glitch nahi).
uint8_t inputs_apply_config_blob(const uint8_t *blob, size_t len);
// Active pin table (defaults, phir aakhri valid blob) — tests + diagnostics.
const RshPinMap *inputs_pin_map();
// Pedal analog mode: false = button mode (analog skip, sirf 10 Hz diagnostic)
void inputs_set_pedal_analog(bool accelAnalog, bool brakeAnalog);
// Axis validity mask: bit0 steering, bit1 accel, bit2 brake,
// bit3 clutch (Phase F WS5, 1 = invalid, F2)
uint8_t inputs_axis_invalid_mask();
bool inputs_axis_invalid();
SampleGovernor &inputs_governor();   // adaptive sampling (neeche dekho)
