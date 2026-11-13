#pragma once
// RSH modules: Handbrake / Shifter / Horn — FW-11 (Plan v2.3 §4, F7).
// Authority: PROTOCOL-V2.1.md §3.4 (GET_CAPS reply body + kind/module
// codes), PIN-RULES.md (pin budget + presets), RSH-PLAN-v2.3.md §4
// (10 modules: Steering, Accelerator, Brake, Clutch, Handbrake, Shifter,
// Horn, Buttons, Encoders, LED).
//
// Ye file Arduino se free hai taake host par test ho sake.
//
// Pin map note: neeche wala DEFAULT table compile hua v3.3 map hai
// (config.h: steering 34, accel 35, brake 36, gear 27/33, buttons 25/26)
// + naye modules (handbrake ADC 32, horn button 4, encoders 23/19 + 16/17,
// LED strip 21). Ye board-esp32-devkitv1.json ke full-rig PRESET se
// mukhtalif hai (preset: steering 32, throttle 33, brake 34, clutch 35,
// handbrake 36) — preset ek alternate config hai, factory default nahi.
// GET_CAPS hamesha board ki CURRENT table batata hai, preset ki nahi;
// preset par switch karna FW-4 config transaction ka kaam hai.
// Clutch (Phase F WS5): compiled map mein clutch GPIO33 par hai — wo
// pin default map mein gear-down (digital) bhi hai, is liye clutch
// sirf config se on hota hai (factory blob mein clutch record nahi);
// GET_CAPS default table mein clutch ki row NAHI hai (pin 33 pehle se
// gear-down row mein hai — do rows ek pin par invalid hoti).
//
// Enable/disable (F7): band module ka ADC read NAHI hota (inputs.cpp
// read se pehle check karta hai) aur us ka data packet mein NAHI jata
// (axis mask bit clear / button bit clear — neeche helpers). GET_CAPS
// pin table mein capability rows phir bhi rehti hain (board kya kar
// sakta hai); on/off config blob ke module flags (§3.6) mein rehta hai.
#include <stdint.h>
#include <stddef.h>
#include <string.h>
#include "config.h"
#include "protocol.h"
#include "config_transaction.h"  // RSH_CFG_TYPE_SHIFTER + blob getters (Phase F WS5)
#include "pinmap.h"  // RshPinMap + slot semantics (Phase G, C3)

// ---- Module codes (PROTOCOL §3.4.1) — wire values, kabhi mat badlo ----
#define RSH_MOD_NONE        0
#define RSH_MOD_STEERING    1
#define RSH_MOD_ACCELERATOR 2
#define RSH_MOD_BRAKE       3
#define RSH_MOD_CLUTCH      4
#define RSH_MOD_HANDBRAKE   5
#define RSH_MOD_SHIFTER     6
#define RSH_MOD_HORN        7
#define RSH_MOD_BUTTONS     8
#define RSH_MOD_ENCODERS    9
#define RSH_MOD_LED         10
#define RSH_MOD_BATTERY     11

// ---- Kind codes (PROTOCOL §3.4.1) ----
#define RSH_KIND_NONE              0
#define RSH_KIND_ADC_NATIVE        1
#define RSH_KIND_ADC_ADS1115       2
#define RSH_KIND_DIGITAL_IN        3
#define RSH_KIND_DIGITAL_OUT       4
#define RSH_KIND_LED_STRIP         5
#define RSH_KIND_I2C_SDA           6
#define RSH_KIND_I2C_SCL           7
#define RSH_KIND_EXPANDER_MCP23017 8
#define RSH_KIND_EXPANDER_PCF8574  9
#define RSH_KIND_AXIS_GENERIC      10
#define RSH_KIND_RESERVED_PIN      11

// ---- Pin caps bits (PROTOCOL §3.4 pin table entry byte 3) ----
#define RSH_CAP_INPUT  0x01
#define RSH_CAP_OUTPUT 0x02
#define RSH_CAP_ADC    0x04
#define RSH_CAP_PWM    0x08
#define RSH_CAP_PULLUP 0x10

// ---- GET_CAPS fixed fields (§3.4) ----
#define RSH_CAPS_FW_MAJOR 4
#define RSH_CAPS_FW_MINOR 0
#define RSH_CAPS_FW_PATCH 0
#define RSH_CAPS_PROTO_MAJOR 2
#define RSH_CAPS_PROTO_MINOR 1
#define RSH_CAPS_N_AXES_MAX 8
#define RSH_CAPS_N_BTN_MAX  64   // wire (§1.1 N1); config ceiling 128 hai
#define RSH_CAPS_N_ENC_MAX  4
#define RSH_CAPS_HEADER_SIZE 18  // status..pinCount
#define RSH_CAPS_ENTRY_SIZE  8

// ---- v2 stream axis slots (compiled map) + button bit indices ----
// Axis slot: 0 steering, 1 accel, 2 brake, 3 clutch (Phase F WS5),
// 4 handbrake. Slot order Plan v2.3 §4 module order + wheel.py AXES
// jaisa hai (steering, accel, brake, clutch, handbrake). NOTE (Phase F
// WS5 contract change): Phase E WS5 mein handbrake slot 3 par tha —
// clutch ke liye slot 3 khaali karna para, handbrake slot 4 par gaya.
// Core (ProcessingPipeline ClutchSlot/HandbrakeSlot) aur host tests §23
// isi map par hain; purana slot-3 handbrake packet ab valid nahi.
// Button bit (v2 stream buttons u32, §1.1): 0 shift-up, 1 shift-down,
// 2 accel-btn, 3 brake-btn, 4 horn. H-pattern mode mein gear 1..6
// bits 5..10 par aate hain (rsh_stream_button_bits) — D1 absolute
// numbering barqarar (bits 2/3/4 dono modes mein wahi hain).
#define RSH_AXIS_SLOT_STEERING  0
#define RSH_AXIS_SLOT_ACCEL     1
#define RSH_AXIS_SLOT_BRAKE     2
#define RSH_AXIS_SLOT_CLUTCH    3
#define RSH_AXIS_SLOT_HANDBRAKE 4
// Button bit (v2 stream buttons u32, §1.1): 0 shift-up, 1 shift-down,
// 2 accel-btn, 3 brake-btn, 4 horn.
#define RSH_BTN2_SHIFT_UP   0
#define RSH_BTN2_SHIFT_DOWN 1
#define RSH_BTN2_ACCEL      2
#define RSH_BTN2_BRAKE      3
#define RSH_BTN2_HORN       4

// ---- Shifter modes ----
#define RSH_SHIFTER_SEQUENTIAL 0  // 2 buttons: up/down (default)
#define RSH_SHIFTER_HPATTERN   1  // 6 digital pins: gears 1..6

// GET_CAPS pin table entry (§3.4, 8 B).
typedef struct {
  uint8_t pin;       // GPIO number
  uint8_t kind;      // RSH_KIND_*
  uint8_t module;    // RSH_MOD_*
  uint8_t caps;      // RSH_CAP_* bits
  uint8_t reserved[4];  // hamesha 0
} RshCapsEntry;

static inline RshCapsEntry rsh_caps_entry(uint8_t pin, uint8_t kind,
                                          uint8_t module, uint8_t caps) {
  RshCapsEntry e;
  e.pin = pin;
  e.kind = kind;
  e.module = module;
  e.caps = (uint8_t)(caps & 0x1F);   // bits 5-7 reserved 0 (§3.4)
  memset(e.reserved, 0, sizeof(e.reserved));
  return e;
}

// Board ki current pin table (compiled map + naye modules). out mein
// likhta hai, entries ki tadaad return karta hai (0 = out NULL/chhota).
// Order: axes, battery, shifter, horn, buttons, encoders, LED.
static inline uint8_t rsh_caps_default_table(RshCapsEntry *out, uint8_t maxEntries) {
  static const RshCapsEntry kTable[] = {
      // pin, kind, module, caps
      {PIN_STEER, RSH_KIND_ADC_NATIVE, RSH_MOD_STEERING, RSH_CAP_INPUT | RSH_CAP_ADC, {0, 0, 0, 0}},
      {PIN_ACCEL_ADC, RSH_KIND_ADC_NATIVE, RSH_MOD_ACCELERATOR, RSH_CAP_INPUT | RSH_CAP_ADC, {0, 0, 0, 0}},
      {PIN_BRAKE_ADC, RSH_KIND_ADC_NATIVE, RSH_MOD_BRAKE, RSH_CAP_INPUT | RSH_CAP_ADC, {0, 0, 0, 0}},
      {PIN_HANDBRAKE_ADC, RSH_KIND_ADC_NATIVE, RSH_MOD_HANDBRAKE,
       RSH_CAP_INPUT | RSH_CAP_ADC | RSH_CAP_PULLUP, {0, 0, 0, 0}},
      {PIN_BATTERY_ADC, RSH_KIND_ADC_NATIVE, RSH_MOD_BATTERY, RSH_CAP_INPUT | RSH_CAP_ADC, {0, 0, 0, 0}},
      {PIN_GEAR_UP, RSH_KIND_DIGITAL_IN, RSH_MOD_SHIFTER, RSH_CAP_INPUT | RSH_CAP_PULLUP, {0, 0, 0, 0}},
      {PIN_GEAR_DOWN, RSH_KIND_DIGITAL_IN, RSH_MOD_SHIFTER,
       RSH_CAP_INPUT | RSH_CAP_ADC | RSH_CAP_PULLUP, {0, 0, 0, 0}},
      {PIN_HORN_BTN, RSH_KIND_DIGITAL_IN, RSH_MOD_HORN, RSH_CAP_INPUT | RSH_CAP_PULLUP, {0, 0, 0, 0}},
      {PIN_ACCEL_BTN, RSH_KIND_DIGITAL_IN, RSH_MOD_BUTTONS, RSH_CAP_INPUT | RSH_CAP_PULLUP, {0, 0, 0, 0}},
      {PIN_BRAKE_BTN, RSH_KIND_DIGITAL_IN, RSH_MOD_BUTTONS, RSH_CAP_INPUT | RSH_CAP_PULLUP, {0, 0, 0, 0}},
      {PIN_ENC1_A, RSH_KIND_DIGITAL_IN, RSH_MOD_ENCODERS, RSH_CAP_INPUT | RSH_CAP_PULLUP, {0, 0, 0, 0}},
      {PIN_ENC1_B, RSH_KIND_DIGITAL_IN, RSH_MOD_ENCODERS, RSH_CAP_INPUT | RSH_CAP_PULLUP, {0, 0, 0, 0}},
      {PIN_ENC2_A, RSH_KIND_DIGITAL_IN, RSH_MOD_ENCODERS, RSH_CAP_INPUT | RSH_CAP_PULLUP, {0, 0, 0, 0}},
      {PIN_ENC2_B, RSH_KIND_DIGITAL_IN, RSH_MOD_ENCODERS, RSH_CAP_INPUT | RSH_CAP_PULLUP, {0, 0, 0, 0}},
      {PIN_LED_STRIP, RSH_KIND_LED_STRIP, RSH_MOD_LED, RSH_CAP_OUTPUT, {0, 0, 0, 0}},
  };
  uint8_t n = (uint8_t)(sizeof(kTable) / sizeof(kTable[0]));
  if (!out || maxEntries < n) return 0;
  memcpy(out, kTable, sizeof(kTable));
  return n;
}

// Pin ki capability bits (GET_CAPS entry byte 3): ADC bit sirf ADC1
// pins (32,33,34,35,36,39) par; PULLUP bit sirf un pins par jin mein
// internal pull-up hai — input-only pins (34,35,36,39) par pull-up NAHI
// hota, is liye un par button ko bahari pull-up chahiye (PC warning).
static inline uint8_t rsh_caps_pin_caps_(uint8_t pin, uint8_t isAnalog) {
  uint8_t caps = RSH_CAP_INPUT;
  if (isAnalog || pin == 32 || pin == 33 || pin == 34 || pin == 35 ||
      pin == 36 || pin == 39)
    caps |= RSH_CAP_ADC;
  if (pin != 34 && pin != 35 && pin != 36 && pin != 39)
    caps |= RSH_CAP_PULLUP;
  return caps;
}

// Board ki CURRENT pin table — pinmap (blob) se (Phase G, C3: "board ki
// table hi jeet-ti hai"). Order default table jaisa hai: axes, battery,
// shifter, horn, buttons, encoders, LED. Factory blob se bani table
// default table ke barabar hoti hai (host test § pinmap mein compare
// hoti hai). Accel/brake BUTTON rows (25/26) hamesha shamil hain — wo
// fixed pedal button-mode pins hain, blob mein nahi aate (pinmap.h).
// H-pattern mode mein gear-up/down + encoder rows ki jagah 6 H-gear
// rows aati hain. Return entries ki tadaad (0 = map/out NULL ya out chhota).
static inline uint8_t rsh_caps_table_from_pinmap(const RshPinMap *map,
                                                 RshCapsEntry *out,
                                                 uint8_t maxEntries) {
  if (!map || !out) return 0;
  RshCapsEntry tmp[19];  // zyada se zyada rows 16 hain (5 axes + battery
                         // + 6 H-gear ya 2 gear + horn + 2 fixed buttons
                         // + 4 enc + LED); 19 mein headroom hai
  uint8_t n = 0;
#define RSH_CAPS_ADD(pin_, kind_, module_, caps_) do { \
    if (n < (uint8_t)(sizeof(tmp) / sizeof(tmp[0]))) { \
      tmp[n++] = rsh_caps_entry((pin_), (kind_), (module_), (caps_)); \
    } \
  } while (0)
  if (map->steer != RSH_PINMAP_ABSENT)
    RSH_CAPS_ADD(map->steer, RSH_KIND_ADC_NATIVE, RSH_MOD_STEERING,
                 rsh_caps_pin_caps_(map->steer, 1));
  if (map->accel != RSH_PINMAP_ABSENT)
    RSH_CAPS_ADD(map->accel, RSH_KIND_ADC_NATIVE, RSH_MOD_ACCELERATOR,
                 rsh_caps_pin_caps_(map->accel, 1));
  if (map->brake != RSH_PINMAP_ABSENT)
    RSH_CAPS_ADD(map->brake, RSH_KIND_ADC_NATIVE, RSH_MOD_BRAKE,
                 rsh_caps_pin_caps_(map->brake, 1));
  // Clutch row sirf tab jab clutch record ho (factory blob mein nahi —
  // is liye factory table default table ke barabar rehti hai). Default
  // table mein clutch row is liye nahi thi ke pin 33 gear-down row mein
  // tha — blob mein clutch aur gear-down ek pin par kabhi nahi hote
  // (validator PIN_CONFLICT), is liye yahan dono rows aa sakti hain.
  if (map->clutch != RSH_PINMAP_ABSENT)
    RSH_CAPS_ADD(map->clutch, RSH_KIND_ADC_NATIVE, RSH_MOD_CLUTCH,
                 rsh_caps_pin_caps_(map->clutch, 1));
  if (map->handbrake != RSH_PINMAP_ABSENT)
    RSH_CAPS_ADD(map->handbrake, RSH_KIND_ADC_NATIVE, RSH_MOD_HANDBRAKE,
                 rsh_caps_pin_caps_(map->handbrake, 1));
  if (map->battery != RSH_PINMAP_ABSENT)
    RSH_CAPS_ADD(map->battery, RSH_KIND_ADC_NATIVE, RSH_MOD_BATTERY,
                 rsh_caps_pin_caps_(map->battery, 1));
  if (map->shifterMode == RSH_SHIFTER_HPATTERN) {
    for (int i = 0; i < 6; i++) {
      if (map->shiftH[i] != RSH_PINMAP_ABSENT)
        RSH_CAPS_ADD(map->shiftH[i], RSH_KIND_DIGITAL_IN, RSH_MOD_SHIFTER,
                     rsh_caps_pin_caps_(map->shiftH[i], 0));
    }
  } else {
    if (map->gearUp != RSH_PINMAP_ABSENT)
      RSH_CAPS_ADD(map->gearUp, RSH_KIND_DIGITAL_IN, RSH_MOD_SHIFTER,
                   rsh_caps_pin_caps_(map->gearUp, 0));
    if (map->gearDown != RSH_PINMAP_ABSENT)
      RSH_CAPS_ADD(map->gearDown, RSH_KIND_DIGITAL_IN, RSH_MOD_SHIFTER,
                   rsh_caps_pin_caps_(map->gearDown, 0));
  }
  if (map->horn != RSH_PINMAP_ABSENT)
    RSH_CAPS_ADD(map->horn, RSH_KIND_DIGITAL_IN, RSH_MOD_HORN,
                 rsh_caps_pin_caps_(map->horn, 0));
  // Fixed pedal button-mode pins (blob mein nahi — pinmap.h).
  RSH_CAPS_ADD(PIN_ACCEL_BTN, RSH_KIND_DIGITAL_IN, RSH_MOD_BUTTONS,
               RSH_CAP_INPUT | RSH_CAP_PULLUP);
  RSH_CAPS_ADD(PIN_BRAKE_BTN, RSH_KIND_DIGITAL_IN, RSH_MOD_BUTTONS,
               RSH_CAP_INPUT | RSH_CAP_PULLUP);
  if (map->shifterMode != RSH_SHIFTER_HPATTERN) {
    if (map->enc1A != RSH_PINMAP_ABSENT)
      RSH_CAPS_ADD(map->enc1A, RSH_KIND_DIGITAL_IN, RSH_MOD_ENCODERS,
                   rsh_caps_pin_caps_(map->enc1A, 0));
    if (map->enc1B != RSH_PINMAP_ABSENT)
      RSH_CAPS_ADD(map->enc1B, RSH_KIND_DIGITAL_IN, RSH_MOD_ENCODERS,
                   rsh_caps_pin_caps_(map->enc1B, 0));
    if (map->enc2A != RSH_PINMAP_ABSENT)
      RSH_CAPS_ADD(map->enc2A, RSH_KIND_DIGITAL_IN, RSH_MOD_ENCODERS,
                   rsh_caps_pin_caps_(map->enc2A, 0));
    if (map->enc2B != RSH_PINMAP_ABSENT)
      RSH_CAPS_ADD(map->enc2B, RSH_KIND_DIGITAL_IN, RSH_MOD_ENCODERS,
                   rsh_caps_pin_caps_(map->enc2B, 0));
  }
  if (map->ledStrip != RSH_PINMAP_ABSENT)
    RSH_CAPS_ADD(map->ledStrip, RSH_KIND_LED_STRIP, RSH_MOD_LED, RSH_CAP_OUTPUT);
#undef RSH_CAPS_ADD
  if (maxEntries < n) return 0;
  memcpy(out, tmp, (size_t)n * sizeof(tmp[0]));
  return n;
}

// ---- GET_CAPS reply body builder (§3.4) ----
// buf mein likhta hai: status + mac + fw + proto + limits + wifiChannel +
// capsFlags + pinCount + entries. Total length return karta hai
// (18 + 8*N), 0 = buf chhota ya pinCount > 40.
static inline size_t rsh_caps_build_body(uint8_t *buf, size_t bufLen,
                                         const uint8_t *mac6, uint8_t wifiChannel,
                                         uint8_t capsFlags,
                                         const RshCapsEntry *entries, uint8_t nEntries) {
  if (!buf || !mac6 || !entries) return 0;
  if (nEntries > 40) return 0;
  size_t total = (size_t)RSH_CAPS_HEADER_SIZE + (size_t)RSH_CAPS_ENTRY_SIZE * nEntries;
  if (bufLen < total) return 0;
  buf[0] = RSH_CTRL_OK;
  memcpy(buf + 1, mac6, 6);
  buf[7] = RSH_CAPS_FW_MAJOR;
  buf[8] = RSH_CAPS_FW_MINOR;
  buf[9] = RSH_CAPS_FW_PATCH;
  buf[10] = RSH_CAPS_PROTO_MAJOR;
  buf[11] = RSH_CAPS_PROTO_MINOR;
  buf[12] = RSH_CAPS_N_AXES_MAX;
  buf[13] = RSH_CAPS_N_BTN_MAX;
  buf[14] = RSH_CAPS_N_ENC_MAX;
  buf[15] = wifiChannel;
  buf[16] = (uint8_t)(capsFlags & 0x0F);   // bits 4-7 reserved 0
  buf[17] = nEntries;
  for (uint8_t i = 0; i < nEntries; i++) {
    uint8_t *e = buf + RSH_CAPS_HEADER_SIZE + (size_t)i * RSH_CAPS_ENTRY_SIZE;
    e[0] = entries[i].pin;
    e[1] = entries[i].kind;
    e[2] = entries[i].module;
    e[3] = (uint8_t)(entries[i].caps & 0x1F);
    memset(e + 4, 0, 4);
  }
  return total;
}

// ---- Module enable/disable (F7) ----
typedef struct {
  uint8_t handbrake;  // 1 = on (analog read + axis bit)
  uint8_t shifter;    // 1 = on (sequential buttons ya H-pattern)
  uint8_t horn;       // 1 = on (button bit)
  uint8_t encoders;   // 1 = on (quadrature reads + enc values)
  uint8_t led;        // 1 = on (strip present, LED_TEST allowed)
  uint8_t clutch;     // Phase F WS5: 1 = on (analog read PIN_CLUTCH_ADC + axis slot 3)
                      // Default 0 — factory blob mein clutch record nahi;
                      // sirf config blob se on hota hai (apply_config_blob).
  uint8_t shifterMode;  // Phase F WS5: runtime gate mode (0 sequential,
                        // 1 H-pattern) — default RSH_SHIFTER_MODE (config.h).
                        // SET_CFG blob type-11 record se aata hai.
} RshModuleEnable;

static inline RshModuleEnable &rsh_modules_state_(void) {
  // Function-local static: inline function ki wajah se poore program mein
  // EK hi instance (har TU mein alag copy wala `static` global nahi —
  // warna .ino se set kiya gaya state inputs.cpp ko nazar na aata).
  static RshModuleEnable state = {1, 1, 1, 1, 1, 0, RSH_SHIFTER_MODE};  // default: sab on, clutch off
  return state;
}

static inline void rsh_modules_set_enabled(const RshModuleEnable *en) {
  if (en) rsh_modules_state_() = *en;
}
static inline const RshModuleEnable *rsh_modules_enabled(void) { return &rsh_modules_state_(); }
static inline uint8_t rsh_modules_handbrake_enabled(void) { return rsh_modules_state_().handbrake; }
static inline uint8_t rsh_modules_shifter_enabled(void) { return rsh_modules_state_().shifter; }
static inline uint8_t rsh_modules_horn_enabled(void) { return rsh_modules_state_().horn; }
static inline uint8_t rsh_modules_encoders_enabled(void) { return rsh_modules_state_().encoders; }
static inline uint8_t rsh_modules_led_enabled(void) { return rsh_modules_state_().led; }
static inline uint8_t rsh_modules_clutch_enabled(void) { return rsh_modules_state_().clutch; }
static inline uint8_t rsh_modules_shifter_mode(void) { return rsh_modules_state_().shifterMode; }

// Config blob (SET_CFG, §3.6) se module state lagao — presence-driven
// (Phase G, C3 — pinmap.h replace semantics): blob mein jis function ka
// record hai, wo module ON hai; jis ka record NAHI hai, wo OFF hai
// (inputs.cpp usay sample nahi karta, stream mein us ka data nahi jata).
// Blob invalid ho to kuch nahi badalta (state waisi ki waisi — fail
// closed). .ino boot par aur cfg version badalne par bulata hai.
// Clutch kisi bhi ADC1 pin par ho sakta hai — inputs.cpp ab pinmap se
// parhta hai, is liye purani pabandi (sirf PIN_CLUTCH_ADC 33) KHATAM
// hai; full-rig preset ka clutch pin 35 bhi ab kaam karta hai (PC
// ValidatePin Analog ke liye ADC1 lazmi karta hai).
static inline void rsh_modules_apply_config_blob(const uint8_t *blob, size_t len) {
  if (!blob || len == 0) return;
  RshPinMap map;
  // Kharab blob (bad magic/schema/CRC/validation) state nahi badalta —
  // warna ek corrupt blob modules band ya gate mode reset kar deta (host
  // test §25 ne pakda: CRC-toda blob shifterMode 1 -> 0 kar raha tha).
  if (!rsh_pinmap_from_blob(blob, len, &map)) return;
  rsh_modules_state_().clutch = (map.clutch != RSH_PINMAP_ABSENT) ? 1 : 0;
  rsh_modules_state_().handbrake = (map.handbrake != RSH_PINMAP_ABSENT) ? 1 : 0;
  rsh_modules_state_().encoders =
      (map.enc1A != RSH_PINMAP_ABSENT || map.enc2A != RSH_PINMAP_ABSENT) ? 1 : 0;
  rsh_modules_state_().led = (map.ledStrip != RSH_PINMAP_ABSENT) ? 1 : 0;
  rsh_modules_state_().horn = (map.horn != RSH_PINMAP_ABSENT) ? 1 : 0;
  rsh_modules_state_().shifter =
      (map.gearUp != RSH_PINMAP_ABSENT || map.gearDown != RSH_PINMAP_ABSENT ||
       map.shifterMode == RSH_SHIFTER_HPATTERN) ? 1 : 0;
  rsh_modules_state_().shifterMode = map.shifterMode;
}

// v2 stream axisMask (§1.1): band module ka axis bit clear -> us ka data
// packet mein nahi jata. Steering/accel/brake hamesha (core modules).
static inline uint8_t rsh_stream_axis_mask(const RshModuleEnable *en) {
  uint8_t mask = (uint8_t)((1u << RSH_AXIS_SLOT_STEERING) |
                           (1u << RSH_AXIS_SLOT_ACCEL) |
                           (1u << RSH_AXIS_SLOT_BRAKE));
  if (en && en->clutch) mask |= (uint8_t)(1u << RSH_AXIS_SLOT_CLUTCH);
  if (en && en->handbrake) mask |= (uint8_t)(1u << RSH_AXIS_SLOT_HANDBRAKE);
  return mask;
}

// v2 stream buttons u32 (§1.1, bit n = button n): band module ke bits
// clear. shifter band ho to up/down dono clear; horn band ho to horn clear.
// H-pattern mode (shifterMode==1): gear 1..6 bits 5..10 par (bit 5 = gear 1
// ... bit 10 = gear 6); up/down bits clear (sequential switches maujood
// nahi — un ke pins H-pattern gear pins hain). Gear 0 (neutral) ya
// invalid (>6) ho to koi gear bit nahi. D1 absolute numbering barqarar:
// accel/brake/horn bits dono modes mein wahi hain.
static inline uint32_t rsh_stream_button_bits(const RshModuleEnable *en,
                                              uint8_t shiftUp, uint8_t shiftDown,
                                              uint8_t accelBtn, uint8_t brakeBtn,
                                              uint8_t horn, uint8_t shifterGear) {
  uint32_t b = 0;
  if (en && en->shifter) {
    if (en->shifterMode == RSH_SHIFTER_HPATTERN) {
      if (shifterGear >= 1 && shifterGear <= 6)
        b |= (1u << (4u + shifterGear));   // gear 1 -> bit 5 ... gear 6 -> bit 10
    } else {
      if (shiftUp) b |= (1u << RSH_BTN2_SHIFT_UP);
      if (shiftDown) b |= (1u << RSH_BTN2_SHIFT_DOWN);
    }
  }
  if (accelBtn) b |= (1u << RSH_BTN2_ACCEL);
  if (brakeBtn) b |= (1u << RSH_BTN2_BRAKE);
  if (en && en->horn && horn) b |= (1u << RSH_BTN2_HORN);
  return b;
}

// H-pattern shifter decode: bits6 bit i = gear i+1 daba hua (active-HIGH,
// caller digitalRead LOW ko invert karke deta hai). 0 = neutral / koi gear
// nahi; ek se zyada bits = invalid -> 0 (neutral, kabhi ghalat gear nahi).
static inline uint8_t rsh_shifter_hpattern_gear(uint8_t bits6) {
  bits6 &= 0x3F;
  if (bits6 == 0) return 0;
  if (bits6 & (uint8_t)(bits6 - 1)) return 0;   // 2+ bits — invalid
  uint8_t gear = 0;
  while (bits6 >>= 1) gear++;
  return (uint8_t)(gear + 1);   // bit 0 -> gear 1 ... bit 5 -> gear 6
}

// H-pattern mode encoders se takrata hai: H-pattern pins (13,14,16,17,18,19)
// mein se 16/17/19 encoder pins bhi hain. 1 = conflict (FW-4 config
// validation is mode + encoders ko ek saath reject kare).
static inline uint8_t rsh_modules_hpattern_encoder_conflict(uint8_t shifterMode,
                                                            uint8_t encodersEnabled) {
  return (shifterMode == RSH_SHIFTER_HPATTERN) && encodersEnabled;
}
