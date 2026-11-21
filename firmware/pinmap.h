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
//   ENCODER (type 5): slot 0 encoder 1 (pinA/pinB), slot 1 encoder 2.
//   LED_STRIP (type 6): slot 0.
//   SHIFTER (type 11): params[0] gate mode (0 sequential, 1 H-pattern);
//     H-pattern gear pins 1..6 = pinA/pinB/pinC + params[1..3].
//   BUTTON_MATRIX (4): koi firmware consumer nahi — ignore.
//   EXPANDER (8/9): REJECTED at COMMIT (v4.0.4) — sampling not
//     implemented, is liye validator in records ko refuse karta hai;
//     `hasExpander` kisi valid blob mein kabhi 1 nahi hota (field
//     parser ke liye rehta hai).
//   Accel/brake BUTTON pins (25/26) blob mein NAHI hain aur PC pin map
//   mein bhi nahi — wo pedal button-mode ke fixed pins hain, inputs.cpp
//   unhein hamesha PIN_ACCEL_BTN/PIN_BRAKE_BTN se parhta hai.
//
// Replace semantics: blob poori sach hai — jis function ka record NAHI
// hai, wo sample NAHI hota (pin 0xFF). Is liye factory blob POORA map
// hai (11 records — har function jo default on hai). PC builder bhi
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
#define RSH_PINMAP_ENC_1          0
#define RSH_PINMAP_ENC_2          1

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
  uint8_t enc1A;      // ENCODER slot 0 pinA
  uint8_t enc1B;      // ENCODER slot 0 pinB
  uint8_t enc2A;      // ENCODER slot 1 pinA
  uint8_t enc2B;      // ENCODER slot 1 pinB
  uint8_t ledStrip;   // LED_STRIP slot 0
  uint8_t shiftH[6];  // H-pattern gear pins 1..6 (mode 1 mein)
  uint8_t shifterMode;  // 0 sequential, 1 H-pattern
  uint8_t handbrakeButtonMode;  // F-15 (v4.0.2): 1 = handbrake pin digital
                                // button hai (axis nahi) — inputs.cpp dekho
  // FW-01 (Round 3): I2C axes — pin 0xFF = I2C, is liye axis ka source
  // alag field mein hai (pin se nahi pehchana ja sakta).
  uint8_t axisType[5];  // per axis slot: RSH_CFG_TYPE_* (1 ADC, 2 ADS1115,
                        // 10 GENERIC, 12 AS5600); 0xFF = koi axis record nahi
  uint8_t adsCh[5];     // ADS1115 AIN channel (0..3) per slot; 0xFF = n/a
  uint8_t as5600Rotation;  // 0=360°, 1=540°, 2=900° (RSH_CFG_AS5600_ROT_PARAM)
  uint8_t as5600Multiturn;  // flags bit0 (RSH_CFG_AS5600_MULTITURN_FLAG)
  uint8_t hasExpander;  // koi EXP_MCP23017/PCF8574 record hai (v4.0.4:
                          // validator reject karta hai — valid blob mein hamesha 0)
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
  out->enc1A = PIN_ENC1_A;
  out->enc1B = PIN_ENC1_B;
  out->enc2A = PIN_ENC2_A;
  out->enc2B = PIN_ENC2_B;
  out->ledStrip = PIN_LED_STRIP;
  out->shiftH[0] = PIN_SHIFT_H1;
  out->shiftH[1] = PIN_SHIFT_H2;
  out->shiftH[2] = PIN_SHIFT_H3;
  out->shiftH[3] = PIN_SHIFT_H4;
  out->shiftH[4] = PIN_SHIFT_H5;
  out->shiftH[5] = PIN_SHIFT_H6;
  out->shifterMode = RSH_SHIFTER_MODE;
  out->handbrakeButtonMode = 0;  // F-15: default axis mode
  // FW-01: defaults native ADC hain (factory blob ke barabar) — clutch
  // slot 3 ka axisType 0xFF rehta hai (memset se), adsCh bhi 0xFF.
  out->axisType[0] = RSH_CFG_TYPE_AXIS_ADC;  // steering
  out->axisType[1] = RSH_CFG_TYPE_AXIS_ADC;  // accel
  out->axisType[2] = RSH_CFG_TYPE_AXIS_ADC;  // brake
  out->axisType[4] = RSH_CFG_TYPE_AXIS_ADC;  // handbrake
  out->as5600Rotation = 0;  // memset 0xFF ke baad saaf karo
  out->as5600Multiturn = 0;
  out->hasExpander = 0;
}

// Kya ye GPIO live pin table mein kisi function ko diya gaya hai?
// Round 2 W3 (FIX-2): pedal button-mode ke fixed pins GPIO25/26
// (PIN_ACCEL_BTN/PIN_BRAKE_BTN) blob ke kisi aur function ko bhi diye ja
// sakte hain (misal full-rig preset mein encoder B pins 23/25, 26/27) —
// inputs.cpp un pins ko fixed button ke tor par parhna band karta hai
// jab wo yahan maujood hon, warna ek hi press do button bits deta
// (encoder B LOW = accel-button bit). Factory/default map mein 25/26
// kisi function ko nahi diye gaye, is liye default behaviour waisa hi.
static inline uint8_t rsh_pinmap_uses_pin(const RshPinMap *map, uint8_t pin) {
  if (!map || pin == RSH_PINMAP_ABSENT) return 0;
  if (map->steer == pin || map->accel == pin || map->brake == pin ||
      map->clutch == pin || map->handbrake == pin || map->battery == pin ||
      map->gearUp == pin || map->gearDown == pin || map->horn == pin ||
      map->enc1A == pin || map->enc1B == pin || map->enc2A == pin ||
      map->enc2B == pin || map->ledStrip == pin)
    return 1;
  for (int i = 0; i < 6; i++)
    if (map->shiftH[i] == pin) return 1;
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
// Koi I2C kind (ADS1115/AS5600/expander) configure hai? (Wire.begin
// sirf tab — warna GPIO21 LED strip ka pin rehta hai, I2C nahi.)
static inline uint8_t rsh_pinmap_has_i2c(const RshPinMap *map) {
  if (!map) return 0;
  if (map->hasExpander) return 1;
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
  m.shifterMode = RSH_SHIFTER_MODE;  // SHIFTER record na ho to sequential
  m.handbrakeButtonMode = 0;  // F-15: default axis mode (memset 0xFF ke baad saaf karo)
  m.as5600Rotation = 0;  // FW-01: memset 0xFF ke baad saaf karo (default 360°)
  m.as5600Multiturn = 0;
  m.hasExpander = 0;  // axisType/adsCh 0xFF hi rehte hain (memset se)
  // H-pattern gear pins bhi default rakho — sequential mode mein istemal
  // nahi hote, lekin GET_CAPS/debug mein kabhi kachra na dikhe.
  m.shiftH[0] = PIN_SHIFT_H1;
  m.shiftH[1] = PIN_SHIFT_H2;
  m.shiftH[2] = PIN_SHIFT_H3;
  m.shiftH[3] = PIN_SHIFT_H4;
  m.shiftH[4] = PIN_SHIFT_H5;
  m.shiftH[5] = PIN_SHIFT_H6;
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
      case RSH_CFG_TYPE_EXP_MCP23017:
      case RSH_CFG_TYPE_EXP_PCF8574:
        // Round 3 (v4.0.4): expander records COMMIT par reject hote
        // hain (validator, config_transaction.h) — ye case kabhi live
        // blob par nahi chalta; hasExpander sirf parser completeness
        // ke liye set hota hai (I2C axes axisType se pehchane jate hain).
        m.hasExpander = 1;
        break;
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
      case RSH_CFG_TYPE_ENCODER: {
        uint8_t *dstA = NULL, *dstB = NULL;
        if (slot == RSH_PINMAP_ENC_1) { dstA = &m.enc1A; dstB = &m.enc1B; }
        else if (slot == RSH_PINMAP_ENC_2) { dstA = &m.enc2A; dstB = &m.enc2B; }
        if (dstA && *dstA == RSH_PINMAP_ABSENT) *dstA = pinA;
        if (dstB && *dstB == RSH_PINMAP_ABSENT) *dstB = pinB;
        break;
      }
      case RSH_CFG_TYPE_LED_STRIP:
        if (slot == 0 && pinA != RSH_PINMAP_ABSENT && m.ledStrip == RSH_PINMAP_ABSENT)
          m.ledStrip = pinA;
        break;
      case RSH_CFG_TYPE_SHIFTER: {
        // mode params[0] mein hai (record offset 6). Validator mode
        // 0/1 + H-pattern mein 6 gear pins pehle hi check kar chuka hai.
        uint8_t mode = r[6];
        m.shifterMode = mode <= 1 ? mode : RSH_SHIFTER_MODE;
        if (m.shifterMode == 1) {  // H-pattern (RSH_SHIFTER_HPATTERN, modules.h)
          m.shiftH[0] = r[3];   // gear 1 (pinA)
          m.shiftH[1] = r[4];   // gear 2 (pinB)
          m.shiftH[2] = r[5];   // gear 3 (pinC)
          m.shiftH[3] = r[7];   // gear 4 (params[1])
          m.shiftH[4] = r[8];   // gear 5 (params[2])
          m.shiftH[5] = r[9];   // gear 6 (params[3])
        }
        break;
      }
      default:
        break;  // BUTTON_MATRIX: koi consumer nahi (EXPANDER ka case upar hai)
    }
  }
  // Quadrature decoder ko dono channels chahiye — ek channel akela ho
  // to poora encoder absent (aadha encoder step nahi karta, count
  // ghalat hota — fail closed).
  if (m.enc1A == RSH_PINMAP_ABSENT || m.enc1B == RSH_PINMAP_ABSENT) {
    m.enc1A = RSH_PINMAP_ABSENT;
    m.enc1B = RSH_PINMAP_ABSENT;
  }
  if (m.enc2A == RSH_PINMAP_ABSENT || m.enc2B == RSH_PINMAP_ABSENT) {
    m.enc2A = RSH_PINMAP_ABSENT;
    m.enc2B = RSH_PINMAP_ABSENT;
  }
  *out = m;
  return 1;
}

// Legacy factory blob pehchano: Phase D/E/F firmware ka factory blob
// BUTTON slots 0..3 = accel-btn/brake-btn/gear-up/gear-down bolta tha.
// PC builder BUTTON slot 3 KABHI nahi bhejta (slots sirf 0/1/2), is
// liye slot-3 BUTTON record sirf purane factory blob ki nishani hai.
// Aisa blob replace semantics mein horn/encoders/LED/handbrake band kar
// deta (un ke records purane blob mein hain hi nahi) — boot par is
// blob ko naye factory blob se migrate karna hai (rsh.ino). Return 1 =
// legacy (valid blob + BUTTON slot-3 record), warna 0.
static inline uint8_t rsh_pinmap_blob_is_legacy(const uint8_t *blob, size_t len) {
  if (!blob || len == 0) return 0;
  if (rsh_cfg_blob_validate(blob, len, NULL, NULL) != RSH_CTRL_OK) return 0;
  uint8_t mc = blob[5];
  for (uint8_t i = 0; i < mc; i++) {
    const uint8_t *r = blob + RSH_CFG_HEADER_SIZE + (size_t)i * RSH_CFG_RECORD_SIZE;
    if (r[0] == RSH_CFG_TYPE_BUTTON && r[1] == 3) return 1;
  }
  return 0;
}
