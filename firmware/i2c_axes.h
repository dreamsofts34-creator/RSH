#pragma once
// I2C axes sampler — pure layer (Round 3 WS-3, FW-01).
//
//   COMMIT ke baad axis LIVE hone ka rasta: config blob -> pinmap
//   (`RshPinMap.axisType[]`, pinmap.h) -> yahan configure -> har
//   `inputs_read()` cycle poll -> axis values (mV) + validity.
//   inputs.cpp sirf thin Arduino glue hai (Wire callbacks + bus
//   management); saara faisla yahan hai taake host tests mein mock
//   I2C se prove ho sake ke COMMIT ke baad axis sampled hota hai.
//
//   AS5600 (type 12): har cycle sample hota hai (fast axis — magnetic
//   encoder, `as5600.h` turn tracking + rotation mapping).
//   ADS1115 (type 2): slow axis, 20 ms cadence, round-robin channels
//   (`ads1115.h` — poll state machine; value tab tak cached rehta hai).
//
//   Fail behaviour (kabhi 0/garbage nahi jata):
//     * I2C NACK / magnet ghayab (AS5600 MD=0) -> valid=0, aakhri value
//       rehti hai; inputs.cpp axisInvalidMask bit lagata hai.
//     * ADS1115 poll fail -> valid=0, cached value rehti hai.
//     * Koi I2C axis configure nahi -> poll no-op hai.
#include <stdint.h>
#include <stddef.h>
#include <string.h>

#include "as5600.h"
#include "ads1115.h"
#include "pinmap.h"

typedef struct {
  uint8_t src[5];           // per axis slot: RSH_CFG_TYPE_* (1 ADC, 2 ADS1115,
                            // 10 GENERIC, 12 AS5600); 0xFF = koi axis record nahi
  uint8_t ads_ch[5];        // ADS1115 AIN channel per slot (0..3), 0xFF = n/a
  uint8_t as5600_slot;      // AS5600 wala slot, 0xFF = koi nahi (max 1/board)
  uint8_t as5600_multiturn; // flags bit0
  uint8_t as5600_rotation;  // 0=360°, 1=540°, 2=900°
  RshAs5600Turns as5600_turns;
  RshAds1115 ads;
  uint16_t value_mv[5];  // aakhri axis values (mV, 0..3300)
  uint8_t valid[5];      // 1 = value taaza/saheeh hai, 0 = fail/stale
  uint8_t has_value[5];  // 1 = kabhi value mili hai (valid=0 par bhi last value)
} RshI2cAxes;

static inline void rsh_i2c_axes_init(RshI2cAxes *a) {
  if (!a) return;
  memset(a, 0, sizeof(*a));
  for (int i = 0; i < 5; i++) {
    a->src[i] = 0xFF;
    a->ads_ch[i] = 0xFF;
  }
  a->as5600_slot = 0xFF;
  rsh_ads1115_init(&a->ads);
  rsh_as5600_turns_init(&a->as5600_turns);
}

// Pinmap se I2C axes configure karo (inputs_apply_config_blob changed
// path se bulata hai). Turn tracker + ADS state reset hote hain — nayi
// config = nayi session; purani config ki values naye axis par nahi jati.
static inline void rsh_i2c_axes_configure(RshI2cAxes *a,
                                          const RshPinMap *map) {
  if (!a || !map) return;
  memset(a, 0, sizeof(*a));
  for (int i = 0; i < 5; i++) {
    a->src[i] = 0xFF;
    a->ads_ch[i] = 0xFF;
  }
  a->as5600_slot = 0xFF;
  rsh_as5600_turns_init(&a->as5600_turns);
  uint8_t ads_channels[RSH_ADS1115_MAX_CH];
  uint8_t n_ads = 0;
  for (uint8_t slot = 0; slot < 5; slot++) {
    uint8_t t = map->axisType[slot];
    if (t == RSH_CFG_TYPE_AXIS_ADS1115) {
      a->src[slot] = t;
      a->ads_ch[slot] = (uint8_t)(map->adsCh[slot] & 0x03u);
      if (n_ads < RSH_ADS1115_MAX_CH)
        ads_channels[n_ads++] = a->ads_ch[slot];
    } else if (t == RSH_CFG_TYPE_AXIS_AS5600) {
      a->src[slot] = t;
      if (a->as5600_slot == 0xFF) {  // max 1/board (validator enforce karta hai)
        a->as5600_slot = slot;
        a->as5600_multiturn = map->as5600Multiturn;
        a->as5600_rotation = map->as5600Rotation;
      }
    } else if (t == RSH_CFG_TYPE_AXIS_ADC || t == RSH_CFG_TYPE_AXIS_GENERIC) {
      a->src[slot] = t;  // native axis — yahan sample nahi hota (inputs.cpp ADC)
    }
  }
  rsh_ads1115_configure(&a->ads, ads_channels, n_ads);
}

// Koi slow (ADS1115) axis live hai? (GET_CAPS capsFlags bit0, FW-04.)
static inline uint8_t rsh_i2c_axes_has_slow_axis(const RshI2cAxes *a) {
  if (!a) return 0;
  for (int i = 0; i < 5; i++)
    if (a->src[i] == RSH_CFG_TYPE_AXIS_ADS1115) return 1;
  return 0;
}

// I2C bus chahiye? (AS5600 ya ADS1115 ya expander record ho.)
static inline uint8_t rsh_i2c_axes_needs_bus(const RshI2cAxes *a,
                                            const RshPinMap *map) {
  if (!a) return 0;
  if (a->as5600_slot != 0xFF || a->ads.n_channels > 0) return 1;
  if (map && map->hasExpander) return 1;  // FW-01: expander sampling pending
  return 0;
}

// Ek poll step (har inputs_read cycle). Return 0 = ok, -1 = kisi I2C
// read/write mein error (per-slot valid flags asal signal hain —
// caller values + valid parhta hai, return ko ignore kar sakta hai).
static inline int rsh_i2c_axes_poll(RshI2cAxes *a,
                                    rsh_as5600_i2c_read_fn read,
                                    rsh_ads1115_i2c_write_fn write,
                                    void *user, uint32_t now_ms) {
  if (!a || !read || !write) return -1;
  int rc = 0;
  // AS5600: har cycle (fast axis).
  if (a->as5600_slot != 0xFF) {
    RshAs5600Sample smp;
    uint8_t slot = a->as5600_slot;
    if (rsh_as5600_read(read, user, &smp) == 0 && smp.valid) {
      if (a->as5600_multiturn) {
        rsh_as5600_turn_update(&a->as5600_turns, smp.raw);
        uint16_t rot_steps = RSH_AS5600_ROT_360_STEPS;
        if (a->as5600_rotation == 1) rot_steps = RSH_AS5600_ROT_540_STEPS;
        else if (a->as5600_rotation == 2) rot_steps = RSH_AS5600_ROT_900_STEPS;
        a->value_mv[slot] =
            rsh_as5600_steps_to_mv(a->as5600_turns.total_steps, rot_steps);
      } else {
        a->value_mv[slot] = rsh_as5600_raw_to_mv(smp.raw);
      }
      a->valid[slot] = 1;
      a->has_value[slot] = 1;
    } else {
      a->valid[slot] = 0;  // aakhri value rehti hai — kabhi 0 nahi jata
      rc = -1;
    }
  }
  // ADS1115: scheduled poll (20 ms cadence, cached values).
  if (a->ads.n_channels > 0) {
    if (rsh_ads1115_poll(&a->ads, read, write, user, now_ms) != 0) {
      for (uint8_t slot = 0; slot < 5; slot++)
        if (a->src[slot] == RSH_CFG_TYPE_AXIS_ADS1115) a->valid[slot] = 0;
      rc = -1;
    } else {
      for (uint8_t slot = 0; slot < 5; slot++) {
        if (a->src[slot] == RSH_CFG_TYPE_AXIS_ADS1115) {
          uint8_t ch = a->ads_ch[slot];
          if (a->ads.has_value[ch]) {
            a->value_mv[slot] = a->ads.value_mv[ch];
            a->valid[slot] = 1;
            a->has_value[slot] = 1;
          }
        }
      }
    }
  }
  return rc;
}
