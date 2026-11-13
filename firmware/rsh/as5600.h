#pragma once
// RSH AS5600 magnetic encoder — FW-12 groundwork (Phase G Workstream 4).
// Authority: docs/AS5600-SPEC.md (ams OSRAM AS5600 datasheet v1-06),
// Plan v2.3 D34 (AXIS_AS5600 sirf <= 360°; 900° ke liye turn counter lazmi),
// PROTOCOL-V2.1.md §1.1 (sab axes mV) + §3.4.1 (kind 12 AXIS_AS5600).
//
// Ye file Arduino se free hai taake host par test ho sake (encoders.h jaisa
// pattern): I2C backend caller-supplied callbacks se aata hai — host tests
// mein mock, firmware mein Wire. RSH kabhi BURN (0xFF) nahi bhejta (OTP,
// irreversible) aur ANGLE (0x0E) nahi parhta — RAWANGLE (0x0C) + PC-side
// calibration hi kaafi hai (D10 model).
//
// Limits (datasheet + D34):
//   - I2C address 0x36 FIXED — ek bus par sirf ek AS5600 (max 1/board).
//   - 12-bit raw 0..4095 = 0..359.912°, 0.0879°/LSB.
//   - mV mapping: raw * 3300 / 4095 (D6 — sab axes mV, 0..3300).
//   - Turn counter: i12 wrap-aware delta (encoders.h ke i16 delta jaisa);
//     boot par turns = 0 (power loss se count bhool jata hai).
//   - Glitch rule: ek 5 ms tick mein > 90° (1024 steps) ka jump namumkin
//     hai (90°/tick @200 Hz = 3000 RPM) — aisa jump glitch hai, count nahi.
#include <stdint.h>
#include <stddef.h>

// ---- Register map (datasheet v1-06, § Register Description) ----
#define RSH_AS5600_I2C_ADDR        0x36
#define RSH_AS5600_REG_ZMCO        0x00
#define RSH_AS5600_REG_ZPOS_H      0x01
#define RSH_AS5600_REG_MPOS_H      0x03
#define RSH_AS5600_REG_MANG_H      0x05
#define RSH_AS5600_REG_CONF_H      0x07
#define RSH_AS5600_REG_STATUS      0x0B
#define RSH_AS5600_REG_RAWANGLE_H  0x0C
#define RSH_AS5600_REG_RAWANGLE_L  0x0D
#define RSH_AS5600_REG_ANGLE_H     0x0E
#define RSH_AS5600_REG_ANGLE_L     0x0F
#define RSH_AS5600_REG_AGC         0x1A
#define RSH_AS5600_REG_MAGNITUDE_H 0x1B
#define RSH_AS5600_REG_MAGNITUDE_L 0x1C
#define RSH_AS5600_REG_BURN        0xFF  // kabhi mat likho (OTP, irreversible)

#define RSH_AS5600_RAW_MAX 4095  // 12-bit full scale
#define RSH_AS5600_MV_FULL 3300  // D6: axes mV, 0..3300

// STATUS bits (0x0B): MD bit5 magnet detected, ML bit4 too weak,
// MH bit3 too strong.
#define RSH_AS5600_STATUS_MD 0x20
#define RSH_AS5600_STATUS_ML 0x10
#define RSH_AS5600_STATUS_MH 0x08

// AGC @3.3 V: range 0..128, ideal ~64 (mid-range). Acceptable band 16..112 —
// is se bahar ho to magnet gap ghalat hai (USER-GUIDE mein gap adjust karo).
#define RSH_AS5600_AGC_MAX_3V3  128
#define RSH_AS5600_AGC_OK_LO    16
#define RSH_AS5600_AGC_OK_HI    112

// Rotation presets (steps: 1 step = 360/4096°; 360° = 4096 steps).
#define RSH_AS5600_ROT_360_STEPS 4096
#define RSH_AS5600_ROT_540_STEPS 6144
#define RSH_AS5600_ROT_900_STEPS 10240
// Glitch threshold: ek tick mein 1024 steps (90°) se bara jump namumkin hai.
#define RSH_AS5600_GLITCH_STEPS 1024

// ---- STATUS decode ----
typedef struct {
  uint8_t magnet_detected;  // MD: magnet mila
  uint8_t too_weak;         // ML: AGC max overflow, magnet too weak
  uint8_t too_strong;       // MH: AGC min overflow, magnet too strong
} RshAs5600Status;

static inline RshAs5600Status rsh_as5600_status_decode(uint8_t v) {
  RshAs5600Status s;
  s.magnet_detected = (uint8_t)((v & RSH_AS5600_STATUS_MD) != 0);
  s.too_weak = (uint8_t)((v & RSH_AS5600_STATUS_ML) != 0);
  s.too_strong = (uint8_t)((v & RSH_AS5600_STATUS_MH) != 0);
  return s;
}

// AGC mid-range mein hai? (3.3 V supply; 5 V par range 0..255 hoti hai —
// RSH 3.3 V par chalta hai, is liye yehi band authority hai.)
static inline int rsh_as5600_agc_ok(uint8_t agc) {
  return agc >= RSH_AS5600_AGC_OK_LO && agc <= RSH_AS5600_AGC_OK_HI;
}

// ---- Raw angle -> mV (<= 360° direct mode, D34) ----
// raw 0..4095 -> 0..3300 mV. 4095 -> 3300 exact; 2048 -> 1650.
static inline uint16_t rsh_as5600_raw_to_mv(uint16_t raw) {
  if (raw > RSH_AS5600_RAW_MAX) raw = RSH_AS5600_RAW_MAX;
  return (uint16_t)(((uint32_t)raw * (uint32_t)RSH_AS5600_MV_FULL) /
                    (uint32_t)RSH_AS5600_RAW_MAX);
}

// ---- Cumulative steps -> mV (multi-turn mode, D34) ----
// total_steps: turn tracker se cumulative steps (0..rotation_steps).
// rotation_steps: 4096 (360°) / 6144 (540°) / 10240 (900°).
// Range se bahar clamp hota hai — kabhi 0..3300 se bahar mV nahi jata.
static inline uint16_t rsh_as5600_steps_to_mv(int32_t total_steps,
                                              int32_t rotation_steps) {
  if (rotation_steps <= 0) rotation_steps = RSH_AS5600_ROT_360_STEPS;
  if (total_steps < 0) total_steps = 0;
  if (total_steps > rotation_steps) total_steps = rotation_steps;
  return (uint16_t)(((int64_t)total_steps * (int64_t)RSH_AS5600_MV_FULL) /
                    (int64_t)rotation_steps);
}

// ---- Turn tracking (900° steering, D34) ----
typedef struct {
  uint16_t prev_raw;    // pichla raw angle (0..4095)
  uint8_t seen;         // pehla sample mil gaya?
  int32_t total_steps;  // boot se cumulative steps (manfi bhi ho sakta hai)
  uint32_t glitches;    // reject kiye gaye jumps (diagnostics)
} RshAs5600Turns;

static inline void rsh_as5600_turns_init(RshAs5600Turns *t) {
  t->prev_raw = 0;
  t->seen = 0;
  t->total_steps = 0;
  t->glitches = 0;
}

// Ek raw sample do. Pehla sample sirf state set karta hai (count nahi
// badalta — boot par aadhi position se start ho to bhi drift nahi).
// Wapas: is sample ka accepted delta (steps), glitch par 0.
static inline int32_t rsh_as5600_turn_update(RshAs5600Turns *t, uint16_t raw) {
  if (raw > RSH_AS5600_RAW_MAX) raw = RSH_AS5600_RAW_MAX;
  if (!t->seen) {
    t->prev_raw = raw;
    t->seen = 1;
    return 0;
  }
  int32_t diff = (int32_t)raw - (int32_t)t->prev_raw;
  if (diff > 2048) diff -= 4096;       // wrap: 4090 -> 5 = +11, -4085 nahi
  else if (diff < -2048) diff += 4096;  // wrap: 5 -> 4090 = -11
  if (diff > RSH_AS5600_GLITCH_STEPS || diff < -RSH_AS5600_GLITCH_STEPS) {
    // Namumkin jump (glitch / I2C corrupt read): state resync karo
    // (encoders.h ke invalid-transition rule jaisa), count na badhao.
    t->glitches++;
    t->prev_raw = raw;
    return 0;
  }
  t->total_steps += diff;
  t->prev_raw = raw;
  return diff;
}

// ---- I2C backend (caller-supplied; host par mock, firmware mein Wire) ----
// read(dev_addr, reg, buf, len, user): reg se len bytes parho.
// Wapas 0 = kamyab, nonzero = NACK/timeout.
typedef int (*rsh_as5600_i2c_read_fn)(uint8_t dev_addr, uint8_t reg,
                                      uint8_t *buf, uint8_t len, void *user);

typedef struct {
  uint16_t raw;       // 0..4095 (RAWANGLE)
  uint8_t status;     // STATUS byte (raw)
  uint8_t agc;        // AGC byte
  uint8_t valid;      // 1 = I2C theek + MD=1 (angle par bharosa karo)
  uint8_t magnet_ok;  // MD bit (STATUS decode)
} RshAs5600Sample;

// Ek poora sample parho: STATUS + RAWANGLE (2 B) + AGC.
// Koi bhi transaction fail ho to valid=0, raw/agc 0 — caller axis-invalid
// (F2/D12) ka rasta le (aakhri valid value + invalid flag, 0 mat bhejo).
static inline int rsh_as5600_read(rsh_as5600_i2c_read_fn read, void *user,
                                  RshAs5600Sample *out) {
  uint8_t sb[1];
  uint8_t ab[2];
  uint8_t gb[1];
  out->raw = 0;
  out->status = 0;
  out->agc = 0;
  out->valid = 0;
  out->magnet_ok = 0;
  if (read == NULL || out == NULL) return -1;
  if (read(RSH_AS5600_I2C_ADDR, RSH_AS5600_REG_STATUS, sb, 1, user) != 0)
    return -1;
  if (read(RSH_AS5600_I2C_ADDR, RSH_AS5600_REG_RAWANGLE_H, ab, 2, user) != 0)
    return -1;
  if (read(RSH_AS5600_I2C_ADDR, RSH_AS5600_REG_AGC, gb, 1, user) != 0)
    return -1;
  out->status = sb[0];
  out->raw = (uint16_t)(((uint16_t)(ab[0] & 0x0F) << 8) | (uint16_t)ab[1]);
  out->agc = gb[0];
  RshAs5600Status st = rsh_as5600_status_decode(sb[0]);
  out->magnet_ok = st.magnet_detected;
  out->valid = st.magnet_detected;
  return 0;
}

// Device maujood hai? (boot probe — STATUS parh kar dekho.)
static inline int rsh_as5600_probe(rsh_as5600_i2c_read_fn read, void *user) {
  uint8_t sb[1];
  if (read == NULL) return 0;
  return read(RSH_AS5600_I2C_ADDR, RSH_AS5600_REG_STATUS, sb, 1, user) == 0;
}
