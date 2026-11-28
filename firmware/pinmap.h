#pragma once
// RSH pin map — config blob (SET_CFG, §3.6) se live pin table (Phase G, C3).
//
//   inputs.cpp compile-time PIN_* se parhta tha — SET_CFG push accept
//   hota tha, blob NVS mein store hota tha, lekin wheel purani pins par
//   chalta rehta tha (docs/PIN-MAP-PUSH.md ka EXACT BLOCKER). Ab blob
//   hi pin table hai: inputs.cpp is struct se parhta hai, COMMIT par
//   inputs_apply_config_blob() dobara lagata hai, boot par NVS blob se
//   load hota hai.
//
// Slot semantics (PROTOCOL-V2.1.md §3.6 — PC builder PinMapBoardPush
// BuildBlob hi producer hai; firmware factory blob bhi yahi scheme
// bolta hai):
//   AXIS_ADC/ADS1115/GENERIC (type 1/2/10): slot 0 steering, 1 accel,
//     2 brake, 3 clutch, 4 handbrake; slot > 4 ka koi consumer nahi.
//     I2C kinds (ADS1115 type 2 / AS5600 type 12) ke pins 0xFF hote
//     hain — axis `axisType[]` se pehchana jata hai, pin se nahi
//     (FW-01: I2C sampling; pehle pin 0xFF = record bekaar samjha jata
//     tha, is liye axis kabhi assign nahi hota tha).
//   BATTERY (type 7): slot 0.
//   BUTTON (type 3): slot 0 shifter-up, 1 shifter-down, 2 horn;
//     slot > 2 ka koi consumer nahi. NOTE: purana factory blob BUTTON
//     slots 0..3 = accel-btn/brake-btn/gear-up/gear-down istemal karta
//     tha — wo scheme KHATAM hai (neeche legacy detection).
//   LED_STRIP (type 6): slot 0.
//   SHIFTER (type 11): params[4] = shifter sensitivity (0..100, D-4);
//     gate mode v2.0.19 mein DELETE — shifter hamesha sequential hai,
//     is liye record ka params[0] (mode) 0 hona lazmi hai (validator
//     reject karta hai) aur pinmap is record se kuch nahi leta.
//   BUTTON_MATRIX (4): COMMIT par REJECT (P1-3) — koi firmware consumer
//     nahi, is liye accept karna silently dead buttons deta (expander
//     gap jaisa). Validator (config_transaction.h) err 3 deta hai.
//   EXPANDER (8/9): REJECTED at COMMIT (v4.0.4) — sampling not
//     implemented, is liye validator in records ko refuse karta hai.
//     Koi hasExpander field nahi — P3-3: wo sirf dead branches ko
//     khilata tha (valid blob mein kabhi 1 nahi hota tha), hata diya.
//   Accel/brake BUTTON pins (25/26) blob mein NAHI hain aur PC pin map
//   mein bhi nahi — wo pedal button-mode ke fixed pins hain, inputs.cpp
//   unhein hamesha PIN_ACCEL_BTN/PIN_BRAKE_BTN se parhta hai.
//
// Replace semantics: blob poori sach hai — jis function ka record NAHI
// hai, wo sample NAHI hota (pin 0xFF). Is liye factory blob POORA map
// hai (9 records — har function jo default on hai). PC builder bhi
// sirf assigned functions ke records bhejta hai; unassign = record
// gayab = board sample nahi karta (PIN-MAP-PUSH.md ka waada).
// Duplicate slot (do records, ek slot, alag pins): PEHLA jeetta hai
// (PC builder kabhi duplicate nahi bhejta; deterministic fallback).
//
// Ye file Arduino se free hai taake host par test ho sake.
#include <stdint.h>
#include <stddef.h>
#include <string.h>
#include "config.h"
#include "config_transaction.h"

// ---- Slot constants (§3.6, PC builder + factory blob dono yahi bolte hain) ----
#define RSH_PINMAP_AXIS_STEERING  0
#define RSH_PINMAP_AXIS_ACCEL     1
#define RSH_PINMAP_AXIS_BRAKE     2
#define RSH_PINMAP_AXIS_CLUTCH    3
#define RSH_PINMAP_AXIS_HANDBRAKE 4
#define RSH_PINMAP_BTN_SHIFT_UP   0
#define RSH_PINMAP_BTN_SHIFT_DOWN 1
#define RSH_PINMAP_BTN_HORN       2

#define RSH_PINMAP_ABSENT 0xFF  // function ka record nahi — sample nahi hota

// Live pin table — ek function, ek pin (ya 0xFF = absent).
typedef struct {
  uint8_t steer;      // AXIS slot 0
  uint8_t accel;      // AXIS slot 1
  uint8_t brake;      // AXIS slot 2
  uint8_t clutch;     // AXIS slot 3 (0xFF = clutch nahi — default)
  uint8_t handbrake;  // AXIS slot 4
  uint8_t battery;    // BATTERY slot 0
  uint8_t gearUp;     // BUTTON slot 0
  uint8_t gearDown;   // BUTTON slot 1
  uint8_t horn;       // BUTTON slot 2
  uint8_t ledStrip;   // LED_STRIP slot 0
  uint8_t handbrakeButtonMode;  // F-15 (v4.0.2): 1 = handbrake pin digital
                                // button hai (axis nahi) — inputs.cpp dekho
  // FW-01 (Round 3): I2C axes — pin 0xFF = I2C, is liye axis ka source
  // alag field mein hai (pin se nahi pehchana ja sakta).
  uint8_t axisType[5];  // per axis slot: RSH_CFG_TYPE_* (1 ADC, 2 ADS1115,
                        // 10 GENERIC, 12 AS5600); 0xFF = koi axis record nahi
  uint8_t adsCh[5];     // ADS1115 AIN channel (0..3) per slot; 0xFF = n/a
  uint8_t as5600Rotation;  // 0=360°, 1=540°, 2=900° (RSH_CFG_AS5600_ROT_PARAM)
  uint8_t as5600Multiturn;  // flags bit0 (RSH_CFG_AS5600_MULTITURN_FLAG)
} RshPinMap;

// Compiled defaults (config.h PIN_* + naye modules) — factory blob ke
// barabar; boot par NVS blob na mile to yahi chalta hai.
static inline void rsh_pinmap_defaults(RshPinMap *out) {
  if (!out) return;
  memset(out, RSH_PINMAP_ABSENT, sizeof(*out));
  out->steer = PIN_STEER;
  out->accel = PIN_ACCEL_ADC;
  out->brake = PIN_BRAKE_ADC;
  out->handbrake = PIN_HANDBRAKE_ADC;
  out->battery = PIN_BATTERY_ADC;
  out->gearUp = PIN_GEAR_UP;
  out->gearDown = PIN_GEAR_DOWN;
  out->horn = PIN_HORN_BTN;
  out->ledStrip = PIN_LED_STRIP;
  out->handbrakeButtonMode = 0;  // F-15: default axis mode
  // FW-01: defaults native ADC hain (factory blob ke barabar) — clutch
  // slot 3 ka axisType 0xFF rehta hai (memset se), adsCh bhi 0xFF.
  out->axisType[0] = RSH_CFG_TYPE_AXIS_ADC;  // steering
  out->axisType[1] = RSH_CFG_TYPE_AXIS_ADC;  // accel
  out->axisType[2] = RSH_CFG_TYPE_AXIS_ADC;  // brake
  out->axisType[4] = RSH_CFG_TYPE_AXIS_ADC;  // handbrake
  out->as5600Rotation = 0;  // memset 0xFF ke baad saaf karo
  out->as5600Multiturn = 0;
}

// Kya ye GPIO live pin table mein kisi function ko diya gaya hai?
// Round 2 W3 (FIX-2): pedal button-mode ke fixed pins GPIO25/26
// (PIN_ACCEL_BTN/PIN_BRAKE_BTN) blob ke kisi aur function ko bhi diye ja
// sakte hain (misal full-rig preset mein pins 23/25, 26/27 kisi axis ko) —
// inputs.cpp un pins ko fixed button ke tor par parhna band karta hai
// jab wo yahan maujood hon, warna ek hi press do cheezein chalata
// (pin 25 LOW = accel-button axis + us function ki reading).
// Factory/default map mein 25/26 kisi function ko nahi diye gaye, is
// liye default behaviour waisa hi.
static inline uint8_t rsh_pinmap_uses_pin(const RshPinMap *map, uint8_t pin) {
  if (!map || pin == RSH_PINMAP_ABSENT) return 0;
  if (map->steer == pin || map->accel == pin || map->brake == pin ||
      map->clutch == pin || map->handbrake == pin || map->battery == pin ||
      map->gearUp == pin || map->gearDown == pin || map->horn == pin ||
      map->ledStrip == pin)
    return 1;
  return 0;
}

// FW-01 helpers — I2C axes pin se nahi, axisType se pehchane jate hain
// (I2C records ke pins hamesha 0xFF hote hain).
// Axis slot mein koi axis record hai? (pin ho ya I2C kind — dono present.)
static inline uint8_t rsh_pinmap_axis_present(const RshPinMap *map,
                                              uint8_t slot) {
  if (!map || slot > RSH_PINMAP_AXIS_HANDBRAKE) return 0;
  return map->axisType[slot] != RSH_PINMAP_ABSENT;
}
// Koi I2C kind (ADS1115/AS5600) configure hai? (Wire.begin
// sirf tab — warna GPIO21 LED strip ka pin rehta hai, I2C nahi.)
// P3-3: hasExpander check hata diya — expander records COMMIT par
// reject hote hain, is liye valid map mein expander kabhi nahi hota.
static inline uint8_t rsh_pinmap_has_i2c(const RshPinMap *map) {
  if (!map) return 0;
  for (int i = 0; i < 5; i++)
    if (map->axisType[i] == RSH_CFG_TYPE_AXIS_ADS1115 ||
        map->axisType[i] == RSH_CFG_TYPE_AXIS_AS5600)
      return 1;
  return 0;
}
// Koi slow (ADS1115) axis live hai? (GET_CAPS capsFlags bit0, FW-04.)
static inline uint8_t rsh_pinmap_has_slow_axis(const RshPinMap *map) {
  if (!map) return 0;
  for (int i = 0; i < 5; i++)
    if (map->axisType[i] == RSH_CFG_TYPE_AXIS_ADS1115) return 1;
  return 0;
}

// Blob se pin table banao (replace semantics — upar dekho).
// Return 1 = theek (out bhari hui), 0 = blob invalid (out ko haath nahi
// lagta — caller purani table rakhta hai, fail closed). NULL blob/len 0
// bhi 0 hai. Validation rsh_cfg_blob_validate se hoti hai (magic/schema/
// CRC32/dup pins/range) — kharab blob se pin kabhi nahi nikala jata.
static inline uint8_t rsh_pinmap_from_blob(const uint8_t *blob, size_t len,
                                           RshPinMap *out) {
  if (!blob || len == 0 || !out) return 0;
  if (rsh_cfg_blob_validate(blob, len, NULL, NULL) != RSH_CTRL_OK) return 0;
  RshPinMap m;
  memset(&m, RSH_PINMAP_ABSENT, sizeof(m));
  m.handbrakeButtonMode = 0;  // F-15: default axis mode (memset 0xFF ke baad saaf karo)
  m.as5600Rotation = 0;  // FW-01: memset 0xFF ke baad saaf karo (default 360°)
  m.as5600Multiturn = 0;
  uint8_t mc = blob[5];
  for (uint8_t i = 0; i < mc; i++) {
    const uint8_t *r = blob + RSH_CFG_HEADER_SIZE + (size_t)i * RSH_CFG_RECORD_SIZE;
    uint8_t type = r[0], slot = r[1];
    uint8_t pinA = r[3], pinB = r[4];
    switch (type) {
      case RSH_CFG_TYPE_AXIS_ADC:
      case RSH_CFG_TYPE_AXIS_ADS1115:
      case RSH_CFG_TYPE_AXIS_GENERIC: {
        uint8_t *dst = NULL;
        if (slot == RSH_PINMAP_AXIS_STEERING) dst = &m.steer;
        else if (slot == RSH_PINMAP_AXIS_ACCEL) dst = &m.accel;
        else if (slot == RSH_PINMAP_AXIS_BRAKE) dst = &m.brake;
        else if (slot == RSH_PINMAP_AXIS_CLUTCH) dst = &m.clutch;
        else if (slot == RSH_PINMAP_AXIS_HANDBRAKE) dst = &m.handbrake;
        if (!dst) break;  // slot > 4 ka koi consumer nahi
        // Pehla record jeetta hai (pin ki tarah) — axisType sirf usi se.
        if (m.axisType[slot] != RSH_PINMAP_ABSENT) break;
        if (type == RSH_CFG_TYPE_AXIS_ADS1115) {
          // FW-01: I2C axis — pins 0xFF lazmi (validator bhi check karta
          // hai); channel params[0] mein hai (0..3, validator range
          // check karta hai). Pehle yahan `pinA == ABSENT` par break
          // hota tha — I2C records ke pins hamesha 0xFF hote hain, is
          // liye axis kabhi assign nahi hota tha (FW-01 bug).
          if (pinA != RSH_PINMAP_ABSENT) break;
          m.axisType[slot] = type;
          m.adsCh[slot] = r[6];  // params[0] = AIN channel
          break;
        }
        if (pinA == RSH_PINMAP_ABSENT) break;  // pin nahi — record bekaar
        m.axisType[slot] = type;
        // F-15: handbrake button mode — pehla slot-4 record jeet-ta hai
        // (pin ki tarah); mode sirf usi record se aata hai.
        if (slot == RSH_PINMAP_AXIS_HANDBRAKE)
          m.handbrakeButtonMode =
              (r[2] & RSH_CFG_HANDBRAKE_BUTTON_FLAG) ? 1 : 0;
        *dst = pinA;  // pehla jeetta hai
        break;
      }
      case RSH_CFG_TYPE_AXIS_AS5600: {
        // FW-01: AS5600 magnetic encoder — pins 0xFF lazmi (I2C bus,
        // addr 0x36 fixed); flags bit0 = multi-turn, params[1] =
        // rotation (0=360°, 1=540°, 2=900°). Validator max 1/board +
        // rotation range pehle hi check kar chuka hai.
        if (pinA != RSH_PINMAP_ABSENT || r[4] != RSH_PINMAP_ABSENT ||
            r[5] != RSH_PINMAP_ABSENT)
          break;
        if (slot > RSH_PINMAP_AXIS_HANDBRAKE) break;
        if (m.axisType[slot] != RSH_PINMAP_ABSENT) break;  // pehla jeetta hai
        m.axisType[slot] = type;
        m.as5600Multiturn =
            (r[2] & RSH_CFG_AS5600_MULTITURN_FLAG) ? 1 : 0;
        m.as5600Rotation = r[6 + RSH_CFG_AS5600_ROT_PARAM];
        break;
      }
      case RSH_CFG_TYPE_BATTERY:
        if (slot == 0 && pinA != RSH_PINMAP_ABSENT && m.battery == RSH_PINMAP_ABSENT)
          m.battery = pinA;
        break;
      case RSH_CFG_TYPE_BUTTON: {
        if (pinA == RSH_PINMAP_ABSENT) break;
        uint8_t *dst = NULL;
        if (slot == RSH_PINMAP_BTN_SHIFT_UP) dst = &m.gearUp;
        else if (slot == RSH_PINMAP_BTN_SHIFT_DOWN) dst = &m.gearDown;
        else if (slot == RSH_PINMAP_BTN_HORN) dst = &m.horn;
        // slot > 2 (purana factory scheme ka slot 3 samet) ka koi
        // consumer nahi — ignore (legacy detection alag se hoti hai).
        if (dst && *dst == RSH_PINMAP_ABSENT) *dst = pinA;
        break;
      }
      case RSH_CFG_TYPE_LED_STRIP:
        if (slot == 0 && pinA != RSH_PINMAP_ABSENT && m.ledStrip == RSH_PINMAP_ABSENT)
          m.ledStrip = pinA;
        break;
      default:
        break;  // BUTTON_MATRIX (P1-3: COMMIT par reject) aur EXPANDER
                // (v4.0.4: COMMIT par reject) ka koi consumer nahi —
                // valid blob mein ye records aate hi nahi (from_blob
                // pehle validate karta hai), is liye default ignore.
                // ENCODER records (type 5) bhi yahin girte hain —
                // v2.0.19 mein validator inhein reject karta hai.
                // SHIFTER record (type 11) se pinmap kuch nahi leta —
                // sensitivity rsh_cfg_shifter_sensitivity() blob se
                // seedha parhta hai; gate mode DELETE hai (sequential
                // hi hai, validator mode != 0 reject karta hai).
    }
  }
  *out = m;
  return 1;
}
