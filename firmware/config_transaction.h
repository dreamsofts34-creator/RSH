#pragma once
// RSH config transaction (FW-4, Phase D — Workstream 1).
// docs/PROTOCOL-V2.1.md §3.5/§3.6 + Plan v2.3 D28 authority hain.
//
//   stage (SET_CFG chunks) -> COMMIT -> CONFIRM (30 s) -> permanent.
//   Confirm na ho 30 s mein -> rollback (purani config wapas).
//
//   * Chunk <= 1024 B, total <= 2048 B (2 KB). Chunks kisi bhi order mein
//     aa sakte hain; dobara bheja gaya chunk idempotent hai (dobara
//     gin'a nahi jata). Naya session (offset 0, alag total) purana
//     staged buffer discard karta hai.
//   * Har chunk ka CRC16 (§3.5, chunkCrc sirf data par) verify hota hai;
//     poore blob ka CRC32 trailer (§3.6) COMMIT par verify hota hai.
//   * Confirm timer HAMESHA 30 s (D28) — reboot ho ya live apply, ek hi
//     value. Reboot ke waqt board ke paas wall clock nahi hoti (uptime
//     zero ho jata hai), is liye reboot ke doran confirm window mein
//     maujood commit ROLLBACK hota hai (rsh_cfg_boot_recover) — reboot
//     kabhi confirm nahi karta, aur baaqi waqt ka andaza nahi lagaya
//     jata. Ye fail-safe faisla hai: config kabhi brick nahi karti.
//   * COMMIT par blob validation fail ho to reply status + errCode (§3.6):
//       err 1 pin conflict       -> PIN_CONFLICT (5)
//       err 2 value out of range -> PIN_CONFLICT (5)
//       err 3 unknown module/type/schema/magic -> UNSUPPORTED (6)
//       err 4 chunk/blob CRC mismatch        -> BAD_CRC (3)
//       err 5 total size mismatch            -> TOO_BIG (4)
//   * Safe mode mein stage + commit dono BUSY (7) — config apply nahi hoti.
//   * REVERT idempotent hai: staged ya committed-lekin-unconfirmed config
//     discard karke purani config wapas; kuch pending na ho to no-op OK.
//
// NOTE (reconciliation, Phase E WS5 — FAISLA HO GAYA): waahid live
// engine ye file (`config_transaction.h`) hai. Parallel design
// `config_txn.h` (contiguous-only chunks, blob CRC16, koi
// live-apply/previous-copy nahi) Phase E mein HATA DIYA GAYA — us ke
// host tests (§12) live engine ki API par port kar diye gaye hain
// (tests-host/test_main.cpp §12, do semantic farq wahan darj hain:
// confirm boundary `>=` 30 s, COMMITTED mein stage = BUSY). Koi bhi
// include ab `config_txn.h` ki taraf ishara nahi karta.
//
// Ye file Arduino se bilkul free hai taake host par unit test ho sake.
// NVS persistence caller ka kaam hai (.ino chunked keys se current /
// previous / pending-confirm flag save karta hai — FW-8 control dispatcher
// inhi functions ko call karega). Koi static/global state yahan nahi.
#include <stdint.h>
#include <stddef.h>
#include <string.h>
#include "protocol.h"   // RSH_CTRL_* status codes (§3.9)

// ---------- Constants (contract) ----------
#define RSH_CFG_MAX_TOTAL            2048u  // poori config blob, zyada nahi
#define RSH_CFG_MAX_CHUNK            1024u  // ek SET_CFG chunk, zyada nahi
#define RSH_CFG_CONFIRM_TIMEOUT_MS   30000u // D28: hamesha 30 s
#define RSH_CFG_CONFIRM_DEADLINE_SEC 30     // COMMIT reply mein yehi jata hai
#define RSH_CFG_HEADER_SIZE          8      // 'RSHG' | schema | moduleCount | cfgVer u16
#define RSH_CFG_RECORD_SIZE          16     // type | slot | flags | pinA | pinB | pinC | params[10]
#define RSH_CFG_TRAILER_SIZE         4      // CRC32 (header + records par)
#define RSH_CFG_SCHEMA               2
#define RSH_CFG_MAX_MODULES          ((RSH_CFG_MAX_TOTAL - RSH_CFG_HEADER_SIZE - RSH_CFG_TRAILER_SIZE) / RSH_CFG_RECORD_SIZE)

// Blob module types (§3.6)
#define RSH_CFG_TYPE_AXIS_ADC        1
#define RSH_CFG_TYPE_AXIS_ADS1115    2
#define RSH_CFG_TYPE_BUTTON          3
#define RSH_CFG_TYPE_BUTTON_MATRIX   4
#define RSH_CFG_TYPE_ENCODER         5
#define RSH_CFG_TYPE_LED_STRIP       6
#define RSH_CFG_TYPE_BATTERY         7
#define RSH_CFG_TYPE_EXP_MCP23017    8
#define RSH_CFG_TYPE_EXP_PCF8574     9
#define RSH_CFG_TYPE_AXIS_GENERIC    10
// Phase F WS5: shifter gate-mode record (type 11). Layout (16 B record):
//   slot = 0 (reserved), flags = 0, pinA/pinB/pinC = gear pins 1..3,
//   params[0] = gate mode (0 sequential, 1 H-pattern),
//   params[1..3] = gear pins 4..6, params[4..9] = 0 (reserved).
// Sequential mode mein is record ki zaroorat nahi (default sequential);
// H-pattern ke liye ye record lazmi hai — baghair is ke mode sequential
// rehta hai. Gear pins 1..6 PIN_SHIFT_H1..H6 hone chahiye (validation
// range + uniqueness + pull-up (digital input) + H+encoder conflict
// check karti hai — pin numbers ki exact match caller/config ki
// zimmedari hai).
#define RSH_CFG_TYPE_SHIFTER         11
#define RSH_CFG_SHIFTER_GATE_PARAM   0   // params[0] = gate mode
#define RSH_CFG_SHIFTER_GEAR_PIN_PARAM_FIRST 1  // params[1..3] = gear pins 4..6
// Phase G WS4: AS5600 magnetic encoder axis (type 12, docs/AS5600-SPEC.md).
// Layout (16 B record): slot = axis slot, flags bit0 = multi-turn
// (0 = <=360° direct, 1 = turn counter on), pinA/pinB/pinC = 0xFF
// (koi GPIO nahi — I2C bus, addr 0x36 fixed), params[0] = 0 (reserved),
// params[1] = rotation (0 = 360°, 1 = 540°, 2 = 900°; default 0).
// Max 1 AS5600 per board (fixed I2C address) — doosra record PIN_CONFLICT.
#define RSH_CFG_TYPE_AXIS_AS5600     12
#define RSH_CFG_AS5600_MULTITURN_FLAG 0x01
#define RSH_CFG_AS5600_ROT_PARAM     1   // params[1] = rotation

// Validation err codes (§3.6)
#define RSH_CFG_ERR_PIN_CONFLICT 1
#define RSH_CFG_ERR_OUT_OF_RANGE 2
#define RSH_CFG_ERR_UNKNOWN_TYPE 3
#define RSH_CFG_ERR_CRC_MISMATCH 4
#define RSH_CFG_ERR_SIZE_MISMATCH 5

// ---------- CRC32 (ISO-HDLC: poly 0xEDB88320, init 0xFFFFFFFF, xorout 0xFFFFFFFF) ----------
// Table-driven (protocol.h ke CRC16 jaisa pattern). Check value:
// CRC32("123456789") = 0xCBF43926.
static uint32_t rsh_crc32_table_[256];
static int rsh_crc32_table_ready_ = 0;
static inline void rsh_crc32_table_init_(void) {
  if (rsh_crc32_table_ready_) return;
  for (int i = 0; i < 256; i++) {
    uint32_t c = (uint32_t)i;
    for (int b = 0; b < 8; b++)
      c = (c & 1) ? (0xEDB88320u ^ (c >> 1)) : (c >> 1);
    rsh_crc32_table_[i] = c;
  }
  rsh_crc32_table_ready_ = 1;
}

static inline uint32_t rsh_crc32(const uint8_t *data, size_t len) {
  rsh_crc32_table_init_();
  uint32_t crc = 0xFFFFFFFFu;
  for (size_t i = 0; i < len; i++)
    crc = rsh_crc32_table_[(crc ^ data[i]) & 0xFF] ^ (crc >> 8);
  return crc ^ 0xFFFFFFFFu;
}

// ---------- Blob build / validate ----------
// Ek module record bharo (16 B). pin 0xFF = istemal nahi. params NULL ho to zero.
static inline void rsh_cfg_record(uint8_t *rec16, uint8_t type, uint8_t slot,
                                  uint8_t flags, uint8_t pin_a, uint8_t pin_b,
                                  uint8_t pin_c, const uint8_t *params10) {
  rec16[0] = type;
  rec16[1] = slot;
  rec16[2] = flags;
  rec16[3] = pin_a;
  rec16[4] = pin_b;
  rec16[5] = pin_c;
  if (params10) memcpy(rec16 + 6, params10, 10);
  else memset(rec16 + 6, 0, 10);
}

// Blob banao: header + records + CRC32 trailer. out kam az kam
// 8 + 16*module_count + 4 bytes ka ho. Return kul length, ya 0 (ghalat input).
// records: module_count * 16 bytes, pehle se rsh_cfg_record se bhare hue.
// Header layout §3.6: magic(4) | schema(1) | moduleCount(1) | cfgVersion u16.
static inline size_t rsh_cfg_blob_build(uint8_t *out, const uint8_t *records,
                                        uint8_t module_count, uint16_t cfg_version) {
  if (!out) return 0;
  if (module_count > RSH_CFG_MAX_MODULES) return 0;
  if (module_count > 0 && !records) return 0;
  size_t total = (size_t)RSH_CFG_HEADER_SIZE
      + (size_t)RSH_CFG_RECORD_SIZE * module_count + RSH_CFG_TRAILER_SIZE;
  if (total > RSH_CFG_MAX_TOTAL) return 0;
  out[0] = 'R'; out[1] = 'S'; out[2] = 'H'; out[3] = 'G';
  out[4] = RSH_CFG_SCHEMA;
  out[5] = module_count;
  rsh_put_u16(out + 6, cfg_version);
  if (module_count > 0) memcpy(out + RSH_CFG_HEADER_SIZE, records,
                               (size_t)RSH_CFG_RECORD_SIZE * module_count);
  uint32_t crc = rsh_crc32(out, total - RSH_CFG_TRAILER_SIZE);
  rsh_put_u32(out + total - RSH_CFG_TRAILER_SIZE, crc);
  return total;
}

// Factory default blob — POORA pin map (§3.6 slot scheme, PC builder
// jaisa; Phase G C3). Replace semantics hai (pinmap.h): jis function ka
// record NAHI hai, board usay sample NAHI karta — is liye factory blob
// mein har default-on function ka record hai (11 records, 188 B):
// steering ADC GPIO34 (slot 0), accel 35 (1), brake 36 (2), handbrake
// 32 (slot 4), battery 39, gear-up BUTTON 27 (slot 0), gear-down 33
// (slot 1), horn 4 (slot 2), encoder 1 (23/19), encoder 2 (16/17),
// LED strip 21. Clutch NAHI hai (default off — AXIS slot 3 record
// sirf config se aata hai); SHIFTER record NAHI hai (sequential
// default). Accel/brake BUTTON pins (25/26) blob mein NAHI — wo pedal
// button-mode ke fixed pins hain (pinmap.h).
// Purana factory blob (7 records, BUTTON slots 0..3 = accel/brake/
// gear-up/gear-down) Phase G mein KHATAM — BUTTON slot scheme badal
// gaya hai; purane NVS blob ko boot par migrate kiya jata hai
// (rsh_pinmap_blob_is_legacy, rsh.ino).
// Return length (188 B), ya 0.
static inline size_t rsh_cfg_factory_blob(uint8_t *out) {
  uint8_t rec[11 * RSH_CFG_RECORD_SIZE];
  rsh_cfg_record(rec + 0 * 16, RSH_CFG_TYPE_AXIS_ADC, 0, 0, 34, 0xFF, 0xFF, NULL);
  rsh_cfg_record(rec + 1 * 16, RSH_CFG_TYPE_AXIS_ADC, 1, 0, 35, 0xFF, 0xFF, NULL);
  rsh_cfg_record(rec + 2 * 16, RSH_CFG_TYPE_AXIS_ADC, 2, 0, 36, 0xFF, 0xFF, NULL);
  rsh_cfg_record(rec + 3 * 16, RSH_CFG_TYPE_AXIS_ADC, 4, 0, 32, 0xFF, 0xFF, NULL);
  rsh_cfg_record(rec + 4 * 16, RSH_CFG_TYPE_BATTERY, 0, 0, 39, 0xFF, 0xFF, NULL);
  rsh_cfg_record(rec + 5 * 16, RSH_CFG_TYPE_BUTTON, 0, 0, 27, 0xFF, 0xFF, NULL);
  rsh_cfg_record(rec + 6 * 16, RSH_CFG_TYPE_BUTTON, 1, 0, 33, 0xFF, 0xFF, NULL);
  rsh_cfg_record(rec + 7 * 16, RSH_CFG_TYPE_BUTTON, 2, 0, 4, 0xFF, 0xFF, NULL);
  rsh_cfg_record(rec + 8 * 16, RSH_CFG_TYPE_ENCODER, 0, 0, 23, 19, 0xFF, NULL);
  rsh_cfg_record(rec + 9 * 16, RSH_CFG_TYPE_ENCODER, 1, 0, 16, 17, 0xFF, NULL);
  rsh_cfg_record(rec + 10 * 16, RSH_CFG_TYPE_LED_STRIP, 0, 0, 21, 0xFF, 0xFF, NULL);
  return rsh_cfg_blob_build(out, rec, 11, 1);
}

// ---------- PIN-RULES (docs/PIN-RULES.md + docs/board-esp32-devkitv1.json) ----------
// Validator PC builder (software PinMap.ValidatePin) + reference validator
// (tests-host/pin_rules_ref.py) wali rules enforce karta hai — wire format
// waisa hi hai, sirf galat blob reject hota hai:
//   * ADC1-only axes/battery: AXIS_ADC/GENERIC + BATTERY pin sirf
//     {32, 33, 34, 35, 36, 39} (ADC2 pin Wi-Fi on par garbage deta hai).
//   * Barred pins kabhi nahi: flash 6-11, UART0 1/3, strapping 0/12/15,
//     status LED 2, maujood hi nahi 20/24/28-31/37/38.
//   * Digital input (BUTTON/ENCODER/MATRIX + H-pattern gear pins) ko
//     internal pull-up chahiye: input-only pins 34-39 reject (inputs.cpp
//     hamesha INPUT_PULLUP lagata hai — wahan pull-up maujood hi nahi).
//     NOTE (PC-builder farq): PC ValidatePin input-only digital par sirf
//     WARNING deta hai (ext_pullup user ki zimmedari), lekin blob mein
//     ext-pullup ka koi flag/field nahi — firmware farq nahi kar sakta,
//     is liye COMMIT par reject (fail closed). PC default blob aur
//     factory blob mein digital pins pull-up wale hain, wo pass hote hain.
//   * LED_STRIP ko digital-out chahiye: input-only pins reject.
//   * GPIO5 (caution, boot log) ALLOWED hai — PC + ref dono allow karte hain.
//   * I2C kinds (ADS1115/expander/AS5600) maujood hon to GPIO21/22 I2C
//     hain — direct pin istemal reject (R-L9).
//   * I2C kinds (ADS1115/expander) ka koi GPIO pin nahi (0xFF lazmi).
//   * Battery <= 1, LED strip (pin wala) <= 1. LED gin'ti sirf pin wale
//     records ki hoti hai — pinless (0xFF) LED records no-op hain (GET_CFG
//     clamp test 127 pinless LED records wala max-size blob banata hai).
// Err codes: pin-rule violation -> err 2 (OUT_OF_RANGE), duplicate/I2C
//   conflict -> err 1 (PIN_CONFLICT); dono ka status PIN_CONFLICT hai.
static inline uint8_t rsh_pin_is_adc1_(uint8_t pin) {
  return pin == 32 || pin == 33 || pin == 34 || pin == 35 || pin == 36 || pin == 39;
}
static inline uint8_t rsh_pin_is_input_only_(uint8_t pin) {
  return pin == 34 || pin == 35 || pin == 36 || pin == 39;
}
// Forbidden (flash 6-11, UART 1/3, nonexistent 20/24/28-31/37/38),
// avoid/strapping (0/12/15), reserved (status LED 2).
static inline uint8_t rsh_pin_is_barred_(uint8_t pin) {
  return pin == 0 || pin == 1 || pin == 2 || pin == 3 ||
         (pin >= 6 && pin <= 12) || pin == 15 || pin == 20 || pin == 24 ||
         (pin >= 28 && pin <= 31) || pin == 37 || pin == 38;
}

// Pin-rule kinds (rsh_cfg_pin_check_ ka `rule`).
#define RSH_PIN_RULE_CLASS_ONLY 0  // istemal nahi hone wala pin: sirf barred + I2C + dup
#define RSH_PIN_RULE_ADC1       1  // analog axis/battery
#define RSH_PIN_RULE_PULLUP     2  // digital input: internal pull-up lazmi
#define RSH_PIN_RULE_DIGOUT     3  // digital output (LED): input-only nahi

// Ek pin check karo. Return 0 = theek (used_pin mark ho gaya),
// RSH_CFG_ERR_PIN_CONFLICT (1) = duplicate ya I2C-reserved,
// RSH_CFG_ERR_OUT_OF_RANGE (2) = barred / galat kind.
// pin == 0xFF caller pehle hi skip karta hai (istemal nahi).
static inline uint8_t rsh_cfg_pin_check_(uint8_t pin, uint8_t rule,
                                         uint8_t has_i2c, uint8_t *used_pin) {
  if (pin > 39) return RSH_CFG_ERR_OUT_OF_RANGE;
  if (rsh_pin_is_barred_(pin)) return RSH_CFG_ERR_OUT_OF_RANGE;
  if (has_i2c && (pin == 21 || pin == 22)) return RSH_CFG_ERR_PIN_CONFLICT;
  if (rule == RSH_PIN_RULE_ADC1 && !rsh_pin_is_adc1_(pin))
    return RSH_CFG_ERR_OUT_OF_RANGE;
  if (rule == RSH_PIN_RULE_PULLUP && rsh_pin_is_input_only_(pin))
    return RSH_CFG_ERR_OUT_OF_RANGE;
  if (rule == RSH_PIN_RULE_DIGOUT && rsh_pin_is_input_only_(pin))
    return RSH_CFG_ERR_OUT_OF_RANGE;
  if (used_pin[pin]) return RSH_CFG_ERR_PIN_CONFLICT;
  used_pin[pin] = 1;
  return 0;
}

// Blob validate karo. err_codes_out (capacity 8 tak) mein err codes aur
// err_count_out mein kul ghaltiyon ki tadaad (8 se zyada bhi ho sakti hai).
// Return: control status (§3.9) — OK (0) ya pehli ghalti ka mapped status.
//   len < 12 ya > 2048 ya moduleCount se size na mile -> TOO_BIG, err 5
//   magic != "RSHG" ya schema != 2                 -> UNSUPPORTED, err 3
//   CRC32 trailer mismatch                          -> BAD_CRC, err 4
//   unknown type (1..11 ke bahar)                   -> UNSUPPORTED, err 3
//   pin 0xFF ke ilawa 0..39 ke bahar                -> PIN_CONFLICT, err 2
//   pin barred (flash/UART/strapping/LED/nonexistent)
//     ya kind-ghalat (ADC2 axis, input-only digital/LED)
//     ya I2C kind (ADS/expander/AS5600) par GPIO pin -> PIN_CONFLICT, err 2
//   I2C kinds maujood hon to direct pin 21/22       -> PIN_CONFLICT, err 1
//   ek GPIO do records mein                          -> PIN_CONFLICT, err 1
//   axes > 8 / encoders > 4 / batteries > 1 /
//     pin wale LED strips > 1                        -> PIN_CONFLICT, err 2
//   axes > 8 / encoders > 4                         -> PIN_CONFLICT, err 2
//   button records 65..128                          -> UNSUPPORTED, err 3 (N1)
//   button records > 128                            -> PIN_CONFLICT, err 2
//   SHIFTER record: mode params[0] > 1              -> PIN_CONFLICT, err 2
//   SHIFTER H-pattern + ENCODER record              -> PIN_CONFLICT, err 1
static inline uint8_t rsh_cfg_blob_validate(const uint8_t *blob, size_t len,
                                            uint8_t *err_codes_out,
                                            uint8_t *err_count_out) {
  uint8_t errs[8];
  uint8_t nerr = 0;
  unsigned int total_errs = 0;  // uint8 se zyada ho sakti hai (127 records)
#define RSH_CFG_ADD_ERR(code) do { \
    total_errs++; \
    if (nerr < 8) errs[nerr++] = (code); \
  } while (0)
// Ek pin PIN-RULES se check karo (rsh_cfg_pin_check_): ghalti ho to err
// record karo + pehla status PIN_CONFLICT. has_i2c/used_pin neeche ke
// record-scan scope mein hain (macro use site par expand hota hai).
#define RSH_CFG_CHECK_PIN(pin, rule) do { \
    uint8_t _perr = rsh_cfg_pin_check_((pin), (rule), has_i2c, used_pin); \
    if (_perr) { \
      RSH_CFG_ADD_ERR(_perr); \
      if (status == RSH_CTRL_OK) status = RSH_CTRL_PIN_CONFLICT; \
    } \
  } while (0)
  uint8_t status = RSH_CTRL_OK;

  if (!blob || len < (size_t)(RSH_CFG_HEADER_SIZE + RSH_CFG_TRAILER_SIZE)
      || len > RSH_CFG_MAX_TOTAL) {
    RSH_CFG_ADD_ERR(RSH_CFG_ERR_SIZE_MISMATCH);
    status = RSH_CTRL_TOO_BIG;
    goto done;
  }
  if (blob[0] != 'R' || blob[1] != 'S' || blob[2] != 'H' || blob[3] != 'G'
      || blob[4] != RSH_CFG_SCHEMA) {
    RSH_CFG_ADD_ERR(RSH_CFG_ERR_UNKNOWN_TYPE);
    status = RSH_CTRL_UNSUPPORTED;
    goto done;
  }
  {
    uint8_t mc = blob[5];
    size_t expect = (size_t)RSH_CFG_HEADER_SIZE
        + (size_t)RSH_CFG_RECORD_SIZE * mc + RSH_CFG_TRAILER_SIZE;
    if (expect != len) {
      RSH_CFG_ADD_ERR(RSH_CFG_ERR_SIZE_MISMATCH);
      status = RSH_CTRL_TOO_BIG;
      goto done;
    }
    if (rsh_crc32(blob, len - RSH_CFG_TRAILER_SIZE)
        != rsh_get_u32(blob + len - RSH_CFG_TRAILER_SIZE)) {
      RSH_CFG_ADD_ERR(RSH_CFG_ERR_CRC_MISMATCH);
      status = RSH_CTRL_BAD_CRC;
      goto done;
    }
    // Records scan karo — tamaam ghaltiyan gino (pehli ka status milta hai).
    // Counters uint16_t hain: 127 records * 16 buttons (matrix) uint8_t se
    // overflow kar jata (T14 jaisa wrap bug) — yahan wrap nahi hona chahiye.
    uint8_t used_pin[40];
    memset(used_pin, 0, sizeof(used_pin));
    uint16_t n_axes = 0, n_enc = 0, n_btn = 0, n_as5600 = 0, n_bat = 0, n_led = 0;
    uint8_t has_hpattern = 0;
    // Pre-scan: I2C kinds (ADS1115/expander/AS5600) hon to GPIO21/22 I2C
    // hain (R-L9) — direct pin istemal neeche reject hota hai.
    uint8_t has_i2c = 0;
    for (uint8_t j = 0; j < mc; j++) {
      uint8_t tj = blob[RSH_CFG_HEADER_SIZE + (size_t)j * RSH_CFG_RECORD_SIZE];
      if (tj == RSH_CFG_TYPE_AXIS_ADS1115 || tj == RSH_CFG_TYPE_EXP_MCP23017 ||
          tj == RSH_CFG_TYPE_EXP_PCF8574 || tj == RSH_CFG_TYPE_AXIS_AS5600) {
        has_i2c = 1;
        break;
      }
    }
    for (uint8_t i = 0; i < mc; i++) {
      const uint8_t *r = blob + RSH_CFG_HEADER_SIZE + (size_t)i * RSH_CFG_RECORD_SIZE;
      uint8_t type = r[0];
      if (type < 1 || type > 12) {
        RSH_CFG_ADD_ERR(RSH_CFG_ERR_UNKNOWN_TYPE);
        if (status == RSH_CTRL_OK) status = RSH_CTRL_UNSUPPORTED;
        continue;
      }
      if (type == RSH_CFG_TYPE_AXIS_ADC || type == RSH_CFG_TYPE_AXIS_ADS1115
          || type == RSH_CFG_TYPE_AXIS_GENERIC) n_axes++;
      else if (type == RSH_CFG_TYPE_AXIS_AS5600) {
        // Phase G WS4: AS5600 axis hai (axes limit mein gina jata hai);
        // fixed I2C addr 0x36 ki wajah se max 1 per board.
        n_axes++;
        n_as5600++;
        if (n_as5600 > 1) {
          RSH_CFG_ADD_ERR(RSH_CFG_ERR_PIN_CONFLICT);
          if (status == RSH_CTRL_OK) status = RSH_CTRL_PIN_CONFLICT;
        }
        if ((r[2] & (uint8_t)~RSH_CFG_AS5600_MULTITURN_FLAG) != 0 ||
            r[6 + RSH_CFG_AS5600_ROT_PARAM] > 2 ||
            r[3] != 0xFF || r[4] != 0xFF || r[5] != 0xFF) {
          // flags sirf bit0, rotation 0..2, pins 0xFF (koi GPIO nahi — I2C).
          RSH_CFG_ADD_ERR(RSH_CFG_ERR_OUT_OF_RANGE);
          if (status == RSH_CTRL_OK) status = RSH_CTRL_PIN_CONFLICT;
        }
      }
      else if (type == RSH_CFG_TYPE_ENCODER) n_enc++;
      else if (type == RSH_CFG_TYPE_BUTTON) n_btn++;
      else if (type == RSH_CFG_TYPE_BUTTON_MATRIX) n_btn += 16;  // default 4x4
      else if (type == RSH_CFG_TYPE_BATTERY) n_bat++;
      else if (type == RSH_CFG_TYPE_LED_STRIP) {
        if (r[3] != 0xFF) n_led++;  // pinless LED record no-op hai (upar dekho)
      }
      else if (type == RSH_CFG_TYPE_SHIFTER) {
        // Phase F WS5: gate mode params[0] mein (record offset 6).
        uint8_t mode = r[6];
        if (mode > 1) {
          RSH_CFG_ADD_ERR(RSH_CFG_ERR_OUT_OF_RANGE);
          if (status == RSH_CTRL_OK) status = RSH_CTRL_PIN_CONFLICT;
          continue;
        }
        if (mode == 1) {
          has_hpattern = 1;
          // Gear pins 1..3 (pinA/B/C) 0xFF nahi ho sakte; maujood hon to
          // digital input hain (INPUT_PULLUP — PIN-RULES pullup rule).
          for (int gp = 3; gp <= 5; gp++) {
            if (r[gp] == 0xFF) {
              RSH_CFG_ADD_ERR(RSH_CFG_ERR_OUT_OF_RANGE);
              if (status == RSH_CTRL_OK) status = RSH_CTRL_PIN_CONFLICT;
            } else {
              RSH_CFG_CHECK_PIN(r[gp], RSH_PIN_RULE_PULLUP);
            }
          }
          // Gear pins 4..6 (params[1..3], record offsets 7..9):
          // generic loop inhein nahi dekhta — yahin check karo.
          for (int gp = 7; gp <= 9; gp++) {
            uint8_t pin = r[gp];
            if (pin == 0xFF) {
              RSH_CFG_ADD_ERR(RSH_CFG_ERR_OUT_OF_RANGE);
              if (status == RSH_CTRL_OK) status = RSH_CTRL_PIN_CONFLICT;
            } else {
              RSH_CFG_CHECK_PIN(pin, RSH_PIN_RULE_PULLUP);
            }
          }
          continue;  // generic loop skip — gear pins pehle hi check ho chuke
        }
      }
      // I2C kinds (ADS1115/expander): koi GPIO pin nahi (0xFF lazmi,
      // PIN-RULES I2C kind rule). AS5600 ke pins upar wali branch mein
      // pehle hi check ho chuke — generic loop bhi skip karo.
      if (type == RSH_CFG_TYPE_AXIS_ADS1115 ||
          type == RSH_CFG_TYPE_EXP_MCP23017 ||
          type == RSH_CFG_TYPE_EXP_PCF8574) {
        for (int p = 3; p <= 5; p++) {
          if (r[p] != 0xFF) {
            RSH_CFG_ADD_ERR(RSH_CFG_ERR_OUT_OF_RANGE);
            if (status == RSH_CTRL_OK) status = RSH_CTRL_PIN_CONFLICT;
          }
        }
        continue;
      }
      if (type == RSH_CFG_TYPE_AXIS_AS5600) continue;
      // Har maujood pin PIN-RULES se check karo — rule type + position
      // par munhasir hai (pinA = asal function pin; pinB/pinC aksar
      // istemal nahi hote, un par sirf barred/I2C/dup check).
      for (int p = 3; p <= 5; p++) {
        uint8_t pin = r[p];
        if (pin == 0xFF) continue;
        uint8_t rule = RSH_PIN_RULE_CLASS_ONLY;
        if (p == 3) {
          if (type == RSH_CFG_TYPE_AXIS_ADC ||
              type == RSH_CFG_TYPE_AXIS_GENERIC ||
              type == RSH_CFG_TYPE_BATTERY)
            rule = RSH_PIN_RULE_ADC1;
          else if (type == RSH_CFG_TYPE_BUTTON ||
                   type == RSH_CFG_TYPE_BUTTON_MATRIX ||
                   type == RSH_CFG_TYPE_ENCODER)
            rule = RSH_PIN_RULE_PULLUP;
          else if (type == RSH_CFG_TYPE_LED_STRIP)
            rule = RSH_PIN_RULE_DIGOUT;
        } else if (p == 4) {
          if (type == RSH_CFG_TYPE_ENCODER ||
              type == RSH_CFG_TYPE_BUTTON_MATRIX)
            rule = RSH_PIN_RULE_PULLUP;
        } else {  // p == 5 (pinC)
          if (type == RSH_CFG_TYPE_BUTTON_MATRIX)
            rule = RSH_PIN_RULE_PULLUP;
        }
        RSH_CFG_CHECK_PIN(pin, rule);
      }
    }
    if (n_axes > 8 || n_enc > 4) {
      RSH_CFG_ADD_ERR(RSH_CFG_ERR_OUT_OF_RANGE);
      if (status == RSH_CTRL_OK) status = RSH_CTRL_PIN_CONFLICT;
    }
    if (n_bat > 1 || n_led > 1) {
      RSH_CFG_ADD_ERR(RSH_CFG_ERR_OUT_OF_RANGE);
      if (status == RSH_CTRL_OK) status = RSH_CTRL_PIN_CONFLICT;
    }
    // Phase F WS5: H-pattern + encoders ek saath nahi (shared pins
    // 16/17/19 — modules.h rsh_modules_hpattern_encoder_conflict).
    // COMMIT par reject — board par dono ek saath kabhi live nahi hote.
    if (has_hpattern && n_enc > 0) {
      RSH_CFG_ADD_ERR(RSH_CFG_ERR_PIN_CONFLICT);
      if (status == RSH_CTRL_OK) status = RSH_CTRL_PIN_CONFLICT;
    }
    if (n_btn > 128) {
      RSH_CFG_ADD_ERR(RSH_CFG_ERR_OUT_OF_RANGE);
      if (status == RSH_CTRL_OK) status = RSH_CTRL_PIN_CONFLICT;
    } else if (n_btn > 64) {
      RSH_CFG_ADD_ERR(RSH_CFG_ERR_UNKNOWN_TYPE);  // N1: wire slot nahi hai
      if (status == RSH_CTRL_OK) status = RSH_CTRL_UNSUPPORTED;
    }
  }
done:
  if (err_codes_out && nerr > 0) memcpy(err_codes_out, errs, nerr);
  // errCount reply mein u8 hai — 255 par clamp (kabhi wrap nahi).
  if (err_count_out) *err_count_out = total_errs > 255 ? 255 : (uint8_t)total_errs;
  return status;
#undef RSH_CFG_ADD_ERR
#undef RSH_CFG_CHECK_PIN
}

// ---------- Blob queries (Phase F WS5) ----------
// Dono getters pehle blob validate karte hain (magic/schema/CRC) —
// kharab blob se mode/pin kabhi nahi nikala jata (default milta hai).

// Shifter gate mode (§3.6, type-11 record params[0]): 0 sequential
// (default — record na ho to bhi), 1 H-pattern. Record maujood magar
// mode > 1 ho to bhi 0 (validation COMMIT par reject karti hai — ye
// getter live apply ke liye mehfooz default deta hai).
static inline uint8_t rsh_cfg_shifter_mode(const uint8_t *blob, size_t len) {
  if (!blob) return 0;
  if (rsh_cfg_blob_validate(blob, len, NULL, NULL) != RSH_CTRL_OK) return 0;
  uint8_t mc = blob[5];
  for (uint8_t i = 0; i < mc; i++) {
    const uint8_t *r = blob + RSH_CFG_HEADER_SIZE + (size_t)i * RSH_CFG_RECORD_SIZE;
    if (r[0] == RSH_CFG_TYPE_SHIFTER) return r[6] <= 1 ? r[6] : 0;
  }
  return 0;
}

// LED strip pin (§3.6, type-6 record pinA): LED_STRIP record ho to 1 +
// pin_out mein pinA; na ho to 0 (pin_out untouched). R-L9 rule ke liye
// (LED_TEST validation) net.cpp har delegate par ye poochta hai.
static inline uint8_t rsh_cfg_led_pin(const uint8_t *blob, size_t len,
                                      uint8_t *pin_out) {
  if (!blob) return 0;
  if (rsh_cfg_blob_validate(blob, len, NULL, NULL) != RSH_CTRL_OK) return 0;
  uint8_t mc = blob[5];
  for (uint8_t i = 0; i < mc; i++) {
    const uint8_t *r = blob + RSH_CFG_HEADER_SIZE + (size_t)i * RSH_CFG_RECORD_SIZE;
    if (r[0] == RSH_CFG_TYPE_LED_STRIP && r[3] != 0xFF) {
      if (pin_out) *pin_out = r[3];
      return 1;
    }
  }
  return 0;
}

// Expanders maujood hain? MCP23017 (type 8) ya PCF8574 (type 9) record
// ho to 1 — R-L9 rule badal jata hai (21/22 I2C hain).
static inline uint8_t rsh_cfg_has_expanders(const uint8_t *blob, size_t len) {
  if (!blob) return 0;
  if (rsh_cfg_blob_validate(blob, len, NULL, NULL) != RSH_CTRL_OK) return 0;
  uint8_t mc = blob[5];
  for (uint8_t i = 0; i < mc; i++) {
    const uint8_t *r = blob + RSH_CFG_HEADER_SIZE + (size_t)i * RSH_CFG_RECORD_SIZE;
    if (r[0] == RSH_CFG_TYPE_EXP_MCP23017 || r[0] == RSH_CFG_TYPE_EXP_PCF8574)
      return 1;
  }
  return 0;
}

// Clutch maujood hai? AXIS_ADC/ADS1115/GENERIC record jis ka slot == 3
// (RSH_AXIS_SLOT_CLUTCH) ho to 1 + pin_out mein pinA (pinA==0xFF ho to
// bhi 0 — clutch ke liye pin lazmi hai). Na ho to 0.
static inline uint8_t rsh_cfg_has_clutch(const uint8_t *blob, size_t len,
                                         uint8_t *pin_out) {
  if (!blob) return 0;
  if (rsh_cfg_blob_validate(blob, len, NULL, NULL) != RSH_CTRL_OK) return 0;
  uint8_t mc = blob[5];
  for (uint8_t i = 0; i < mc; i++) {
    const uint8_t *r = blob + RSH_CFG_HEADER_SIZE + (size_t)i * RSH_CFG_RECORD_SIZE;
    uint8_t type = r[0];
    if ((type == RSH_CFG_TYPE_AXIS_ADC || type == RSH_CFG_TYPE_AXIS_ADS1115 ||
         type == RSH_CFG_TYPE_AXIS_GENERIC) &&
        r[1] == 3 && r[3] != 0xFF) {
      if (pin_out) *pin_out = r[3];
      return 1;
    }
  }
  return 0;
}

// ---------- Transaction state machine ----------
typedef enum {
  RSH_TXN_IDLE = 0,       // kuch staged nahi, koi confirm window nahi
  RSH_TXN_STAGING = 1,    // chunks aa rahe hain (poora blob abhi nahi)
  RSH_TXN_STAGED = 2,     // poora blob mil gaya (CRC abhi COMMIT par check hoga)
  RSH_TXN_COMMITTED = 3   // nayi config live hai, confirm window khuli hai
} rsh_txn_state_t;

typedef struct {
  uint8_t state;                              // rsh_txn_state_t
  uint8_t safe_mode;                          // 1 = safe mode: stage/commit BUSY
  uint8_t staged[RSH_CFG_MAX_TOTAL];          // stage ho raha blob
  uint8_t received[RSH_CFG_MAX_TOTAL / 8];    // bitmap: har blob byte ke liye 1 bit
  uint16_t received_count;                    // naye bytes jo mil chuke hain
  uint16_t total;                             // expected kul length
  uint8_t has_total;
  uint8_t current[RSH_CFG_MAX_TOTAL];         // live config (confirmed ya committed)
  uint16_t current_len;
  uint8_t previous[RSH_CFG_MAX_TOTAL];        // COMMIT se pehle wali (rollback copy)
  uint16_t previous_len;
  uint8_t has_previous;
  uint32_t commit_ms;                         // COMMIT kab hua (millis)
  uint32_t confirm_deadline_ms;               // commit_ms + 30000 (wrap-safe compare)
  uint16_t cfg_version;                       // current config ki version
} rsh_cfg_txn_t;

static inline void rsh_cfg_txn_init(rsh_cfg_txn_t *t) {
  memset(t, 0, sizeof(*t));
  t->state = RSH_TXN_IDLE;
}

// Safe mode flag set karo (safemode.h se). Stage + commit BUSY ho jate hain.
static inline void rsh_cfg_set_safe_mode(rsh_cfg_txn_t *t, uint8_t active) {
  if (t) t->safe_mode = active ? 1 : 0;
}

// Live config load karo (boot par: NVS slot ya factory blob). Blob ko
// validate kiya jata hai — ghalat blob load nahi hota (caller factory
// fallback le). Return OK ya validation status.
static inline uint8_t rsh_cfg_txn_load_current(rsh_cfg_txn_t *t, const uint8_t *blob,
                                               size_t len) {
  if (!t || !blob) return RSH_CTRL_UNSUPPORTED;
  uint8_t st = rsh_cfg_blob_validate(blob, len, NULL, NULL);
  if (st != RSH_CTRL_OK) return st;
  memcpy(t->current, blob, len);
  t->current_len = (uint16_t)len;
  t->cfg_version = rsh_get_u16(blob + 6);
  t->state = RSH_TXN_IDLE;
  t->has_previous = 0;
  return RSH_CTRL_OK;
}

static inline uint8_t rsh_cfg_is_complete(const rsh_cfg_txn_t *t) {
  return (t && t->has_total && t->total > 0 && t->received_count == t->total) ? 1 : 0;
}

static inline uint16_t rsh_cfg_staged_bytes(const rsh_cfg_txn_t *t) {
  return t ? t->received_count : 0;
}

// ---- SET_CFG chunk (§3.5) ----
// staged_bytes_out / total_out: reply ke stagedBytes/totalExpected.
// Return control status: OK / BAD_CRC (chunk CRC) / TOO_BIG / BUSY
// (safe mode ya confirm window mein conflicting op).
static inline uint8_t rsh_cfg_stage_chunk(rsh_cfg_txn_t *t, uint16_t offset,
                                          uint16_t total, uint16_t chunk_len,
                                          uint16_t chunk_crc, const uint8_t *data,
                                          uint16_t *staged_bytes_out,
                                          uint16_t *total_out) {
  if (!t) return RSH_CTRL_UNSUPPORTED;
  if (t->safe_mode) return RSH_CTRL_BUSY;
  if (t->state == RSH_TXN_COMMITTED) return RSH_CTRL_BUSY;
  if (chunk_len == 0 || chunk_len > RSH_CFG_MAX_CHUNK) return RSH_CTRL_TOO_BIG;
  if (total == 0 || total > RSH_CFG_MAX_TOTAL) return RSH_CTRL_TOO_BIG;
  if ((uint32_t)offset + chunk_len > total) return RSH_CTRL_TOO_BIG;
  if (!data) return RSH_CTRL_BAD_CRC;
  if (rsh_crc16_ccitt(data, chunk_len) != chunk_crc) return RSH_CTRL_BAD_CRC;

  // Naya session (offset 0, ALAG total) purana staged buffer discard karta
  // hai (§3.5). Same total par offset-0 chunk dobara bhejna sirf retry hai
  // (idempotent) — STAGED state mein bhi discard NAHI hota, warna
  // complete hone ke baad chunk-0 retry poora blob phenk deta.
  if (offset == 0 && (!t->has_total || t->total != total)) {
    memset(t->staged, 0, sizeof(t->staged));
    memset(t->received, 0, sizeof(t->received));
    t->received_count = 0;
    t->total = total;
    t->has_total = 1;
  }
  if (!t->has_total || t->total != total) return RSH_CTRL_TOO_BIG;
  for (uint16_t i = 0; i < chunk_len; i++) {
    uint16_t pos = (uint16_t)(offset + i);
    uint8_t bit = (uint8_t)(t->received[pos / 8] & (uint8_t)(1u << (pos % 8)));
    if (!bit) {
      t->received[pos / 8] |= (uint8_t)(1u << (pos % 8));
      t->received_count++;
    }
    t->staged[pos] = data[i];
  }
  t->state = rsh_cfg_is_complete(t) ? RSH_TXN_STAGED : RSH_TXN_STAGING;
  if (staged_bytes_out) *staged_bytes_out = t->received_count;
  if (total_out) *total_out = t->total;
  return RSH_CTRL_OK;
}

// ---- COMMIT (§3.6) ----
// Poora blob staged hona chahiye. Validation fail -> mapped status +
// err codes (err_codes_out capacity 8, err_count_out kul tadaad).
// Kamyabi par: previous = current, current = staged (LIVE apply, reboot
// nahi), confirm deadline = now_ms + 30 s. Dobara COMMIT (retry) wahi
// deadline wapas deta hai — timer dobara shuru NAHI hota (idempotent).
static inline uint8_t rsh_cfg_commit(rsh_cfg_txn_t *t, uint32_t now_ms,
                                     uint8_t *err_codes_out, uint8_t *err_count_out) {
  if (!t) return RSH_CTRL_UNSUPPORTED;
  if (t->safe_mode) return RSH_CTRL_BUSY;
  if (t->state == RSH_TXN_COMMITTED) return RSH_CTRL_OK;  // retry: deadline wahi
  if (t->state == RSH_TXN_IDLE) return RSH_CTRL_UNSUPPORTED;
  if (t->state == RSH_TXN_STAGING) {
    if (err_count_out) *err_count_out = 1;
    if (err_codes_out) err_codes_out[0] = RSH_CFG_ERR_SIZE_MISMATCH;
    return RSH_CTRL_TOO_BIG;
  }
  uint8_t st = rsh_cfg_blob_validate(t->staged, t->total, err_codes_out, err_count_out);
  if (st != RSH_CTRL_OK) return st;  // state STAGED rehti hai — fix ya REVERT
  memcpy(t->previous, t->current, t->current_len);
  t->previous_len = t->current_len;
  t->has_previous = 1;
  memcpy(t->current, t->staged, t->total);
  t->current_len = t->total;
  t->cfg_version = rsh_get_u16(t->staged + 6);
  t->commit_ms = now_ms;
  t->confirm_deadline_ms = now_ms + RSH_CFG_CONFIRM_TIMEOUT_MS;
  t->state = RSH_TXN_COMMITTED;
  return RSH_CTRL_OK;
}

// ---- CONFIRM (§3.6) ----
// Sirf COMMITTED state mein. Waqt par confirm -> permanent (previous
// discard, staged clear). Der se confirm (deadline guzar gayi) -> pehle
// rollback karo, phir UNSUPPORTED wapas (transaction khatam ho chuki thi).
// Kuch pending na ho -> UNSUPPORTED (exact retransmit FW-8 session cache
// sambhalta hai — ye layer dobara execute nahi karti).
static inline uint8_t rsh_cfg_confirm(rsh_cfg_txn_t *t, uint32_t now_ms) {
  if (!t || t->state != RSH_TXN_COMMITTED) return RSH_CTRL_UNSUPPORTED;
  if ((int32_t)(now_ms - t->confirm_deadline_ms) >= 0) {
    // Der ho gayi — rollback, phir batao ke transaction khatam thi.
    if (t->has_previous) {
      memcpy(t->current, t->previous, t->previous_len);
      t->current_len = t->previous_len;
      t->cfg_version = rsh_get_u16(t->current + 6);
    }
    t->has_previous = 0;
    memset(t->staged, 0, sizeof(t->staged));
    memset(t->received, 0, sizeof(t->received));
    t->received_count = 0;
    t->has_total = 0;
    t->state = RSH_TXN_IDLE;
    return RSH_CTRL_UNSUPPORTED;
  }
  t->has_previous = 0;
  memset(t->staged, 0, sizeof(t->staged));
  memset(t->received, 0, sizeof(t->received));
  t->received_count = 0;
  t->has_total = 0;
  t->state = RSH_TXN_IDLE;
  return RSH_CTRL_OK;
}

// ---- REVERT (§3.6) ----
// Staged ya committed-lekin-unconfirmed config discard karke purani
// config wapas. Kuch pending na ho to no-op OK (idempotent).
static inline uint8_t rsh_cfg_revert(rsh_cfg_txn_t *t) {
  if (!t) return RSH_CTRL_UNSUPPORTED;
  if (t->state == RSH_TXN_COMMITTED && t->has_previous) {
    memcpy(t->current, t->previous, t->previous_len);
    t->current_len = t->previous_len;
    t->cfg_version = rsh_get_u16(t->current + 6);
  }
  t->has_previous = 0;
  memset(t->staged, 0, sizeof(t->staged));
  memset(t->received, 0, sizeof(t->received));
  t->received_count = 0;
  t->has_total = 0;
  t->state = RSH_TXN_IDLE;
  return RSH_CTRL_OK;
}

// ---- Confirm-deadline tick (har loop mein call karo) ----
// Return 1 = deadline guzar gayi, rollback ho gaya (current = previous).
// Return 0 = kuch nahi hua. Caller rollback par persisted current wapas
// likhe aur pending-confirm flag clear kare.
static inline uint8_t rsh_cfg_tick(rsh_cfg_txn_t *t, uint32_t now_ms) {
  if (!t || t->state != RSH_TXN_COMMITTED) return 0;
  if ((int32_t)(now_ms - t->confirm_deadline_ms) < 0) return 0;
  if (t->has_previous) {
    memcpy(t->current, t->previous, t->previous_len);
    t->current_len = t->previous_len;
    t->cfg_version = rsh_get_u16(t->current + 6);
  }
  t->has_previous = 0;
  memset(t->staged, 0, sizeof(t->staged));
  memset(t->received, 0, sizeof(t->received));
  t->received_count = 0;
  t->has_total = 0;
  t->state = RSH_TXN_IDLE;
  return 1;
}

// ---- Boot recover: reboot confirm nahi karta (D28) ----
// Boot par caller txn mein current = NVS "current" (nayi, unconfirmed)
// aur previous = NVS "previous" load kare, phir ye call kare agar NVS
// pending-confirm flag set ho. Return 1 = rollback hua (current = purani).
// pending na ho to 0, kuch nahi badla.
// INVARIANT (caller ki zimmedari): pending flag set ho to previous copy
// bhi NVS mein maujood honi chahiye — flag aur previous hamesha ek saath
// likhe jayen. Agar previous na ho to 0 milta hai aur current waisa hi
// rehta hai (is surat mein caller factory fallback le).
static inline uint8_t rsh_cfg_boot_recover(rsh_cfg_txn_t *t, uint8_t pending_confirm) {
  if (!t || !pending_confirm) return 0;
  uint8_t had_previous = t->has_previous;
  if (had_previous) {
    memcpy(t->current, t->previous, t->previous_len);
    t->current_len = t->previous_len;
    t->cfg_version = rsh_get_u16(t->current + 6);
  }
  t->has_previous = 0;
  memset(t->staged, 0, sizeof(t->staged));
  memset(t->received, 0, sizeof(t->received));
  t->received_count = 0;
  t->has_total = 0;
  t->state = RSH_TXN_IDLE;
  return had_previous ? 1 : 0;
}

// ---- GET_CFG read (committed/current config se) ----
// offset se want_len bytes current config se out mein copy karo.
// Return copy ki gayi tadaad (0 = offset bahar hai).
static inline size_t rsh_cfg_get_chunk(const rsh_cfg_txn_t *t, uint16_t offset,
                                       uint16_t want_len, uint8_t *out) {
  if (!t || !out || offset >= t->current_len) return 0;
  size_t avail = (size_t)(t->current_len - offset);
  size_t n = want_len < avail ? want_len : avail;
  memcpy(out, t->current + offset, n);
  return n;
}
