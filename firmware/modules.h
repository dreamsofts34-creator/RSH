#pragma once
// RSH modules: Handbrake / Shifter / Horn — FW-11 (Plan v2.3 §4, F7).
// Authority: PROTOCOL-V2.1.md §3.4 (GET_CAPS reply body + kind/module
// codes), PIN-RULES.md (pin budget + presets), RSH-PLAN-v2.3.md §4
// (v2.0.19: 7 inputs — Steering, Accelerator, Brake, Clutch, Handbrake,
// Shifter, Horn; Buttons + Encoders modules v2.0.19 mein DELETE hue,
// H-pattern bhi — sequential-only shifter).
//
// Ye file Arduino se free hai taake host par test ho sake.
//
// Pin map note: neeche wala DEFAULT table compile hua v3.3 map hai
// (config.h: steering 34, accel 35, brake 36, gear 27/33, horn 4)
// + naye modules (handbrake ADC 32, horn button 4, LED strip 21).
// Ye board-esp32-devkitv1.json ke full-rig PRESET se mukhtalif hai
// (preset: steering 32, throttle 33, brake 34, clutch 35,
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
// v2.0.19: RSH_MOD_BUTTONS (8) + RSH_MOD_ENCODERS (9) DELETE hue —
// Buttons/Encoders modules khatam, GET_CAPS mein ye codes kabhi nahi
// aate. Baqi codes ki values waisi ki waisi hain (gap rehta hai).
#define RSH_MOD_NONE        0
#define RSH_MOD_STEERING    1
#define RSH_MOD_ACCELERATOR 2
#define RSH_MOD_BRAKE       3
#define RSH_MOD_CLUTCH      4
#define RSH_MOD_HANDBRAKE   5
#define RSH_MOD_SHIFTER     6
#define RSH_MOD_HORN        7
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
#define RSH_KIND_AXIS_AS5600       12  // FW-01 (Round 3): AS5600 I2C axis (§3.4.1)

// ---- Pin caps bits (PROTOCOL §3.4 pin table entry byte 3) ----
#define RSH_CAP_INPUT  0x01
#define RSH_CAP_OUTPUT 0x02
#define RSH_CAP_ADC    0x04
#define RSH_CAP_PWM    0x08
#define RSH_CAP_PULLUP 0x10

// ---- GET_CAPS fixed fields (§3.4) ----
// F-12: version triple version.h se aata hai (FIRMWARE_VERSION jaisa hi
// source) — HELLO/GET_CAPS kabhi dobara alag version nahi batayenge.
#define RSH_CAPS_FW_MAJOR RSH_FW_VERSION_MAJOR
#define RSH_CAPS_FW_MINOR RSH_FW_VERSION_MINOR
#define RSH_CAPS_FW_PATCH RSH_FW_VERSION_PATCH
#define RSH_CAPS_PROTO_MAJOR 2
#define RSH_CAPS_PROTO_MINOR 2  // v2.2
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
// 4 horn. v2.0.19: bits 2/3 (accel/brake pedal buttons) + bits 5..10
// (H-pattern gears) DELETE hue — wire par hamesha 0 (reserved).
// D1 absolute numbering barqarar (horn bit dono taraf wahi hai).
#define RSH_AXIS_SLOT_STEERING  0
#define RSH_AXIS_SLOT_ACCEL     1
#define RSH_AXIS_SLOT_BRAKE     2
#define RSH_AXIS_SLOT_CLUTCH    3
#define RSH_AXIS_SLOT_HANDBRAKE 4
// Button bit (v2 stream buttons u32, §1.1): 0 shift-up, 1 shift-down,
// 4 horn. Bits 2/3/5..10 reserved-zero hain (v2.0.19).
#define RSH_BTN2_SHIFT_UP   0
#define RSH_BTN2_SHIFT_DOWN 1
#define RSH_BTN2_HORN       4

// ---- Shifter modes ----
// v2.0.19: sequential-only — H-pattern DELETE hua (RSH_SHIFTER_HPATTERN
// + gear pins + rsh_shifter_hpattern_gear khatam). Shifter hamesha
// 2 buttons (up/down) hai; koi mode field nahi rehta.

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
// Order: axes, shifter, horn, LED. v2.0.19: 8 rows (buttons 25/26 +
// encoders 23/19/16/17 ke rows DELETE — wo inputs ab parhe hi nahi jate).
static inline uint8_t rsh_caps_default_table(RshCapsEntry *out, uint8_t maxEntries) {
  static const RshCapsEntry kTable[] = {
      // pin, kind, module, caps
      {PIN_STEER, RSH_KIND_ADC_NATIVE, RSH_MOD_STEERING, RSH_CAP_INPUT | RSH_CAP_ADC, {0, 0, 0, 0}},
      {PIN_ACCEL_ADC, RSH_KIND_ADC_NATIVE, RSH_MOD_ACCELERATOR, RSH_CAP_INPUT | RSH_CAP_ADC, {0, 0, 0, 0}},
      {PIN_BRAKE_ADC, RSH_KIND_ADC_NATIVE, RSH_MOD_BRAKE, RSH_CAP_INPUT | RSH_CAP_ADC, {0, 0, 0, 0}},
      {PIN_HANDBRAKE_ADC, RSH_KIND_ADC_NATIVE, RSH_MOD_HANDBRAKE,
       RSH_CAP_INPUT | RSH_CAP_ADC | RSH_CAP_PULLUP, {0, 0, 0, 0}},
      // F-12: BATTERY row nahi (GPIO39 kabhi sample nahi hota) — 15 -> 14.
      {PIN_GEAR_UP, RSH_KIND_DIGITAL_IN, RSH_MOD_SHIFTER, RSH_CAP_INPUT | RSH_CAP_PULLUP, {0, 0, 0, 0}},
      {PIN_GEAR_DOWN, RSH_KIND_DIGITAL_IN, RSH_MOD_SHIFTER,
       RSH_CAP_INPUT | RSH_CAP_ADC | RSH_CAP_PULLUP, {0, 0, 0, 0}},
      {PIN_HORN_BTN, RSH_KIND_DIGITAL_IN, RSH_MOD_HORN, RSH_CAP_INPUT | RSH_CAP_PULLUP, {0, 0, 0, 0}},
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
// table hi jeet-ti hai"). Order default table jaisa hai: axes, shifter,
// horn, LED (F-12: battery row nahi; v2.0.19: buttons/encoders rows
// nahi). Factory blob se bani table default table ke barabar hoti hai
// (host test § pinmap mein compare hoti hai). Shifter hamesha
// sequential hai (H-pattern v2.0.19 mein DELETE — koi mode branch nahi).
// Return entries ki tadaad (0 = map/out NULL ya out chhota).
static inline uint8_t rsh_caps_table_from_pinmap(const RshPinMap *map,
                                                 RshCapsEntry *out,
                                                 uint8_t maxEntries) {
  if (!map || !out) return 0;
  RshCapsEntry tmp[12];  // zyada se zyada rows 9 hain (5 axes + 2 gear
                         // + horn + LED); 12 mein headroom hai
  uint8_t n = 0;
#define RSH_CAPS_ADD(pin_, kind_, module_, caps_) do { \
    if (n < (uint8_t)(sizeof(tmp) / sizeof(tmp[0]))) { \
      tmp[n++] = rsh_caps_entry((pin_), (kind_), (module_), (caps_)); \
    } \
  } while (0)
// FW-01: ek axis slot ki row — kind/pin/caps axisType se. I2C axis ka
// pin 0xFF hai (koi GPIO nahi), kind 2 (ADS1115) / 12 (AS5600); native
// axis ka pin GPIO hai, kind 1 (ADC) / 10 (GENERIC). axisType 0xFF ho
// to koi row nahi (slot ka record nahi).
#define RSH_CAPS_AXIS_ROW(pin_, type_, module_) do { \
    if ((type_) == RSH_CFG_TYPE_AXIS_AS5600) { \
      RSH_CAPS_ADD(RSH_PINMAP_ABSENT, RSH_KIND_AXIS_AS5600, (module_), \
                   RSH_CAP_INPUT); \
    } else if ((type_) == RSH_CFG_TYPE_AXIS_ADS1115) { \
      RSH_CAPS_ADD(RSH_PINMAP_ABSENT, RSH_KIND_ADC_ADS1115, (module_), \
                   RSH_CAP_INPUT | RSH_CAP_ADC); \
    } else if ((pin_) != RSH_PINMAP_ABSENT) { \
      RSH_CAPS_ADD((pin_), \
                   (type_) == RSH_CFG_TYPE_AXIS_GENERIC \
                       ? RSH_KIND_AXIS_GENERIC : RSH_KIND_ADC_NATIVE, \
                   (module_), rsh_caps_pin_caps_((pin_), 1)); \
    } \
  } while (0)
  RSH_CAPS_AXIS_ROW(map->steer, map->axisType[0], RSH_MOD_STEERING);
  RSH_CAPS_AXIS_ROW(map->accel, map->axisType[1], RSH_MOD_ACCELERATOR);
  RSH_CAPS_AXIS_ROW(map->brake, map->axisType[2], RSH_MOD_BRAKE);
  // Clutch row sirf tab jab clutch record ho (factory blob mein nahi —
  // is liye factory table default table ke barabar rehti hai). Default
  // table mein clutch row is liye nahi thi ke pin 33 gear-down row mein
  // tha — blob mein clutch aur gear-down ek pin par kabhi nahi hote
  // (validator PIN_CONFLICT), is liye yahan dono rows aa sakti hain.
  RSH_CAPS_AXIS_ROW(map->clutch, map->axisType[3], RSH_MOD_CLUTCH);
  // F-15: button mode mein handbrake digital input hai (ADC nahi) —
  // GET_CAPS mein wahi kind/caps aate hain (sirf native pin par).
  if (map->handbrakeButtonMode && map->handbrake != RSH_PINMAP_ABSENT)
    RSH_CAPS_ADD(map->handbrake, RSH_KIND_DIGITAL_IN, RSH_MOD_HANDBRAKE,
                 rsh_caps_pin_caps_(map->handbrake, 0));
  else
    RSH_CAPS_AXIS_ROW(map->handbrake, map->axisType[4], RSH_MOD_HANDBRAKE);
  // F-12 (polish round): BATTERY row HATA DI — GPIO39 kabhi sample
  // nahi hota tha (status packet mein battery_pct=255 / supply_mv=0),
  // is liye GET_CAPS mein is ka hona misleading tha (app battery
  // capability dikha sakta tha jo board measure hi nahi karta). Jab
  // GPIO39 sampling implement ho (divider ratio ke saath), row wapas
  // aayegi. Pinmap slot (blob compatibility) waisa hi rehta hai.
  (void)map;
  if (map->gearUp != RSH_PINMAP_ABSENT)
    RSH_CAPS_ADD(map->gearUp, RSH_KIND_DIGITAL_IN, RSH_MOD_SHIFTER,
                 rsh_caps_pin_caps_(map->gearUp, 0));
  if (map->gearDown != RSH_PINMAP_ABSENT)
    RSH_CAPS_ADD(map->gearDown, RSH_KIND_DIGITAL_IN, RSH_MOD_SHIFTER,
                 rsh_caps_pin_caps_(map->gearDown, 0));
  if (map->horn != RSH_PINMAP_ABSENT)
    RSH_CAPS_ADD(map->horn, RSH_KIND_DIGITAL_IN, RSH_MOD_HORN,
                 rsh_caps_pin_caps_(map->horn, 0));
  if (map->ledStrip != RSH_PINMAP_ABSENT)
    RSH_CAPS_ADD(map->ledStrip, RSH_KIND_LED_STRIP, RSH_MOD_LED, RSH_CAP_OUTPUT);
#undef RSH_CAPS_AXIS_ROW
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
  uint8_t shifter;    // 1 = on (sequential buttons up/down)
  uint8_t horn;       // 1 = on (button bit)
  uint8_t led;        // 1 = on (strip present, LED_TEST allowed)
  uint8_t clutch;     // Phase F WS5: 1 = on (analog read PIN_CLUTCH_ADC + axis slot 3)
                      // Default 0 — factory blob mein clutch record nahi;
                      // sirf config blob se on hota hai (apply_config_blob).
} RshModuleEnable;

static inline RshModuleEnable &rsh_modules_state_(void) {
  // Function-local static: inline function ki wajah se poore program mein
  // EK hi instance (har TU mein alag copy wala `static` global nahi —
  // warna .ino se set kiya gaya state inputs.cpp ko nazar na aata).
  static RshModuleEnable state = {1, 1, 1, 1, 0};  // default: sab on, clutch off
  return state;
}

static inline void rsh_modules_set_enabled(const RshModuleEnable *en) {
  if (en) rsh_modules_state_() = *en;
}
static inline const RshModuleEnable *rsh_modules_enabled(void) { return &rsh_modules_state_(); }
static inline uint8_t rsh_modules_handbrake_enabled(void) { return rsh_modules_state_().handbrake; }
static inline uint8_t rsh_modules_shifter_enabled(void) { return rsh_modules_state_().shifter; }
static inline uint8_t rsh_modules_horn_enabled(void) { return rsh_modules_state_().horn; }
static inline uint8_t rsh_modules_led_enabled(void) { return rsh_modules_state_().led; }
static inline uint8_t rsh_modules_clutch_enabled(void) { return rsh_modules_state_().clutch; }

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
  // warna ek corrupt blob modules band kar deta (host test §25 ne
  // pakda: CRC-toda blob shifter band kar raha tha).
  if (!rsh_pinmap_from_blob(blob, len, &map)) return;
  // FW-01: clutch/handbrake I2C axis (ADS1115/AS5600) par bhi on hote
  // hain — un ka pin 0xFF hota hai, is liye axis_present (pin ya I2C
  // kind) se check karo, sirf pin se nahi.
  rsh_modules_state_().clutch =
      rsh_pinmap_axis_present(&map, RSH_AXIS_SLOT_CLUTCH) ? 1 : 0;
  rsh_modules_state_().handbrake =
      rsh_pinmap_axis_present(&map, RSH_AXIS_SLOT_HANDBRAKE) ? 1 : 0;
  rsh_modules_state_().led = (map.ledStrip != RSH_PINMAP_ABSENT) ? 1 : 0;
  rsh_modules_state_().horn = (map.horn != RSH_PINMAP_ABSENT) ? 1 : 0;
  rsh_modules_state_().shifter =
      (map.gearUp != RSH_PINMAP_ABSENT || map.gearDown != RSH_PINMAP_ABSENT) ? 1 : 0;
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
// v2.0.19: SIRF bits 0/1/4 kabhi set hote hain — bits 2/3 (pedal buttons)
// + bits 5..10 (H-pattern gears) DELETE hue, wire par hamesha 0. Is liye
// function ke sirf 4 arguments hain — koi caller dobara bit 2/3/5..10
// set nahi kar sakta.
static inline uint32_t rsh_stream_button_bits(const RshModuleEnable *en,
                                              uint8_t shiftUp, uint8_t shiftDown,
                                              uint8_t horn) {
  uint32_t b = 0;
  if (en && en->shifter) {
    if (shiftUp) b |= (1u << RSH_BTN2_SHIFT_UP);
    if (shiftDown) b |= (1u << RSH_BTN2_SHIFT_DOWN);
  }
  if (en && en->horn && horn) b |= (1u << RSH_BTN2_HORN);
  return b;
}
