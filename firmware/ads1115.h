#pragma once
// ADS1115 16-bit I2C ADC — pure driver (Round 3 WS-3, FW-01).
//
//   ADS1115 axis record (type 2, PROTOCOL-V2.2 §3.6): slot = axis slot,
//   pins 0xFF (koi GPIO nahi — I2C bus, addr 0x48 fixed, PC builder
//   `PinModule::axis_ads1115` channel `params[0]` 0..3), flags 0.
//   Slow axis hai (≤50 Hz — GET_CAPS capsFlags bit0, FW-04): har
//   channel 20 ms cadence par single-shot poll hota hai, round-robin.
//   Conversion → mV: raw (0..32767) * 6144 / 32767, clamp 0..3300 mV
//   (PGA ±6.144 V; pedals/axes 0..3.3 V range mein hain).
//
//   AS5600 (`as5600.h`) ki tarah callback-based hai — koi Arduino
//   dependency nahi, host tests mein mock I2C se test hota hai.
//   Config register (0x01) layout (datasheet §9.6.3):
//     [15] OS=1 (start single conversion), [14:12] MUX (100+ch =
//     single-ended AIN0..3), [11:9] PGA=000 (±6.144 V), [8] MODE=1
//     (single-shot), [7:5] DR=100 (128 SPS — conversion ~7.8 ms,
//     20 ms cadence se pehle hamesha poori), [4:0] comparator off
//     (COMP_MODE 0, COMP_POL 0, COMP_LAT 0, COMP_QUE 11 disable).
//   Canonical config word ch0 ke liye 0xC183 hai.
//
//   Poll state machine (pipelined — ek transaction per poll):
//     poll 1: conversion start (config write, ch N)
//     poll 2 (≥20 ms baad): result read (ch N) + conversion start (ch N+1)
//   I2C fail ho to converting channel waisa hi rehta hai — agla poll
//   dobara read karta hai (result register mein rehta hai); write fail
//   ho to wahi channel dobara start hota hai. Kabhi garbage value nahi.
#include <stdint.h>
#include <stddef.h>
#include <string.h>

#define RSH_ADS1115_I2C_ADDR 0x48  // fixed (PC builder addr 0x48 set karta hai)
#define RSH_ADS1115_REG_CONV   0x00
#define RSH_ADS1115_REG_CONFIG 0x01
#define RSH_ADS1115_POLL_MS    20  // 50 Hz max (slowAxis, PROTOCOL-V2.2 §3.4)
#define RSH_ADS1115_MAX_CH     4   // AIN0..AIN3

typedef int (*rsh_ads1115_i2c_read_fn)(uint8_t dev, uint8_t reg, uint8_t *buf,
                                       uint8_t len, void *user);
typedef int (*rsh_ads1115_i2c_write_fn)(uint8_t dev, uint8_t reg,
                                        const uint8_t *buf, uint8_t len,
                                        void *user);

// Config word banao: single-shot, single-ended AIN`channel`, PGA ±6.144 V,
// 128 SPS, comparator off. channel 0..3 (caller validate karta hai).
static inline uint16_t rsh_ads1115_config_word(uint8_t channel) {
  return (uint16_t)(0x8000u | ((uint16_t)(0x04u + (channel & 0x03u)) << 12) |
                    0x0100u | 0x0080u | 0x0003u);
}

// Raw conversion (signed, big-endian register) -> mV (0..3300).
// Single-ended readings >= 0 hote hain; negative clamp 0, full-scale
// 32767 = 6144 mV -> 3300 clamp (axes 0..3.3 V range mein hain).
static inline uint16_t rsh_ads1115_raw_to_mv(int16_t raw) {
  if (raw <= 0) return 0;
  int32_t mv = ((int32_t)raw * 6144) / 32767;
  if (mv > 3300) mv = 3300;
  return (uint16_t)mv;
}

typedef struct {
  uint8_t channels[RSH_ADS1115_MAX_CH];  // poll order (AIN channels)
  uint8_t n_channels;
  uint8_t next_idx;     // agla channel jo start hoga
  int8_t converting;    // conversion in-flight wala channel, -1 = koi nahi
  uint32_t last_poll_ms;
  uint8_t has_value[RSH_ADS1115_MAX_CH];
  uint16_t value_mv[RSH_ADS1115_MAX_CH];
} RshAds1115;

static inline void rsh_ads1115_init(RshAds1115 *s) {
  if (!s) return;
  memset(s, 0, sizeof(*s));
  s->converting = -1;
}

// Poll karne wale channels set karo (slot order mein, distinct —
// validator duplicate channel reject karta hai). Pehle ki values saaf
// hoti hain (nayi config = nayi session).
static inline void rsh_ads1115_configure(RshAds1115 *s,
                                         const uint8_t *channels,
                                         uint8_t n) {
  if (!s) return;
  memset(s, 0, sizeof(*s));
  s->converting = -1;
  if (!channels || n == 0) return;
  if (n > RSH_ADS1115_MAX_CH) n = RSH_ADS1115_MAX_CH;
  for (uint8_t i = 0; i < n; i++) s->channels[i] = (uint8_t)(channels[i] & 0x03u);
  s->n_channels = n;
}

// Ek poll step (caller har inputs_read cycle bulata hai; andar 20 ms
// cadence hai — jald-bazi mein bulaya jaye to kuch nahi hota).
// Return 0 = ok / is poll par kuch nahi karna tha, -1 = I2C error
// (value nahi badli — caller last value + invalid flag istemal kare).
// P3-1: read kamyab ho kar taaza value store ho chuki ho to agle
// conversion-start ki write fail par bhi 0 milta hai — taaza value
// ek frame ke liye invalid nahi hoti (write fail sirf AGLI conversion
// ko rokta hai; channel dobara start hone ke liye converting -1 aur
// next_idx waisi ki waisi rehti hai). Read fail ho to hamesha -1.
static inline int rsh_ads1115_poll(RshAds1115 *s, rsh_ads1115_i2c_read_fn read,
                                   rsh_ads1115_i2c_write_fn write, void *user,
                                   uint32_t now_ms) {
  if (!s || !read || !write) return -1;
  if (s->n_channels == 0) return 0;
  if ((uint32_t)(now_ms - s->last_poll_ms) < RSH_ADS1115_POLL_MS) return 0;
  s->last_poll_ms = now_ms;
  uint8_t fresh = 0;  // is poll par taaza value store hui?
  // Pehle pichli conversion ka result parho (128 SPS par 20 ms mein
  // poori ho chuki hoti hai; register mein last completed result rehta
  // hai — jald read bhi purana result deta hai, garbage nahi).
  if (s->converting >= 0) {
    uint8_t buf[2];
    if (read(RSH_ADS1115_I2C_ADDR, RSH_ADS1115_REG_CONV, buf, 2, user) != 0)
      return -1;  // converting waisa hi — agla poll dobara read karega
    int16_t raw = (int16_t)((uint16_t)((uint16_t)buf[0] << 8) | buf[1]);
    uint8_t ch = (uint8_t)s->converting;
    s->value_mv[ch] = rsh_ads1115_raw_to_mv(raw);
    s->has_value[ch] = 1;
    s->converting = -1;
    fresh = 1;
  }
  // Agli conversion start karo (round-robin).
  uint8_t ch = s->channels[s->next_idx];
  uint16_t cfg = rsh_ads1115_config_word(ch);
  uint8_t wbuf[2] = {(uint8_t)(cfg >> 8), (uint8_t)(cfg & 0xFF)};
  if (write(RSH_ADS1115_I2C_ADDR, RSH_ADS1115_REG_CONFIG, wbuf, 2, user) != 0) {
    // P3-1: taaza value pehle hi store ho chuki — is poll ko kamyab
    // mano (next_idx aage nahi — agla poll wahi channel dobara start karega).
    // Koi taaza value na ho to -1 (value nahi badli).
    return fresh ? 0 : -1;
  }
  s->converting = (int8_t)ch;
  s->next_idx = (uint8_t)((s->next_idx + 1) % s->n_channels);
  return 0;
}

// Presence probe: config write ACK = chip maujood hai (ADS1115 ka koi
// ID register nahi — yehi standard check hai).
static inline uint8_t rsh_ads1115_probe(rsh_ads1115_i2c_write_fn write,
                                        void *user) {
  if (!write) return 0;
  uint16_t cfg = rsh_ads1115_config_word(0);
  uint8_t wbuf[2] = {(uint8_t)(cfg >> 8), (uint8_t)(cfg & 0xFF)};
  return write(RSH_ADS1115_I2C_ADDR, RSH_ADS1115_REG_CONFIG, wbuf, 2, user) == 0
             ? 1
             : 0;
}
