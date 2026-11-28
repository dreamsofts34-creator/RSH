#pragma once
// RSH v2.2 stream emission (Phase E WS5, PROTOCOL-V2.2.md §1.1) — PURE
// (Arduino-free) taake host par unit test ho sake.
//
// 200 Hz loop (rsh.ino) ka faisla (v2.2, D3 — plain GET_CAPS OK):
//   peer active + GET_CAPS OK ho -> v2 / v1 (HELO proto se)
//   GET_CAPS OK na ho            -> kuch nahi (HELO akela kaafi nahi)
//   koi PC active na ho         -> kuch nahi
// "GET_CAPS OK" = is session mein GET_CAPS control command OK hua ho
// (net.cpp per-transport flag; naya session / naya peer use saaf karta
// hai). Status packet (1 Hz) is se alag hai — waisa hi rehta hai.
//
// Wire rules (§1.1, D6): sab axes mV mein hain. Steering mV mode mein
// analogReadMilliVolts se seedha mV aata hai; native pedals/clutch/
// handbrake raw counts (0-4095) aate hain — unhein yahin nominal ADC
// scale se mV mein badla jata hai (calibration PC-side hoti hai; ye scale
// sirf wire unit hai). I2C axes (ADS1115/AS5600) ki values PEHLE SE mV
// mein hoti hain (inputs.cpp read_axis_value_ / i2c_axes.h value_mv) —
// un par dobara raw->mV NAHI lagta (P1-1: pehle har non-steering axis
// par lagta tha — 1650 mV -> 1330 mV, full travel kabhi 100% nahi).
// Caller har slot ke liye `axes_is_mv` bit deta hai (inputs_axis_is_mv).
// Clutch slot 3, handbrake slot 4 (Phase F WS5).
// Encoders v2.0.19 mein DELETE — wire slots barqarar hain (protocol
// v2.2 frozen), values hamesha 0 (n_enc hamesha 2, enc = {0, 0}).
// Buttons v1 byte (RSH_BTN_* bits) ki bit positions v2 bits 0/1/4 se
// milti hain (gear up/down, horn) — module gating
// rsh_stream_button_bits karta hai; bits 2/3/5..10 hamesha 0 hain.
//
// Flags (§1.3): axis-invalid (F2) aur tick-skipped (D5, seqGap badla ho
// to) yahan se aate hain. config-changed / low-supply ke liye abhi koi
// tracker nahi hai — bits 0 rehte hain (GET_CFG flow config change ko
// cover karta hai; low-supply Phase D F2/F10 ka kaam hai).
#include <stdint.h>
#include <stddef.h>
#include "protocol.h"
#include "modules.h"

// Raw ADC counts (12-bit, 0-4095, 11 dB attenuation) -> mV (0-3300).
// 4095 se zyada clamp hota hai (kabhi aana nahi chahiye — defensive).
static inline uint16_t rsh_raw_to_mv(uint16_t raw) {
  if (raw > 4095) raw = 4095;
  return (uint16_t)(((uint32_t)raw * 3300u + 2047u) / 4095u);
}

typedef enum {
  RSH_EMIT_NONE = 0,  // koi PC active nahi — kuch mat bhejo
  RSH_EMIT_V1,         // legacy v1 peer — 18 B v1 packet
  RSH_EMIT_V2          // v2.1 peer — v2 stream packet (§1.1)
} rsh_emit_kind_t;

/// Emission faisla: sirf ACTIVE + GET_CAPS-OK session ko bhejo;
/// version HELO proto se. D3: HELO (unauthenticated UDP bootstrap) se
/// stream nahi khulta — pehle is session mein GET_CAPS OK hona lazmi
/// hai (board net.cpp mein per-transport capsOk flag rakhta hai).
/// GET_CAPS ke baghair peer ko v1 bhi nahi milta. Status packet (1 Hz)
/// is se alag hai.
static inline rsh_emit_kind_t rsh_stream_emit_kind(uint8_t pc_active,
                                                   uint8_t pc_is_v2,
                                                   uint8_t caps_ok) {
  if (!pc_active || !caps_ok) return RSH_EMIT_NONE;
  return pc_is_v2 ? RSH_EMIT_V2 : RSH_EMIT_V1;
}

/// v2 stream packet banao (InputSample jaisi values se — .ino se call
/// hota hai). Axes mask-order mein bhare jate hain (rsh_v2_build_stream
/// ka contract: axes[] ascending slot order mein).
///   axes_is_mv: bit per slot (bit 0 steering … bit 4 handbrake) — bit
///     set ho to value pehle se mV mein hai (I2C axis, ya mV-mode
///     steering), clear ho to raw counts hain aur raw->mV convert hota
///     hai. P1-1: pehle sirf steering ka flag tha — I2C pedals do
///     dafa convert hote thay (mV -> dobara mV scale).
///   n_enc: hamesha 2 (enc values hamesha 0 — v2.0.19, encoders DELETE;
///     wire slots barqarar hain, protocol v2.2 frozen).
///   buttons_v1: RSH_BTN_* byte (bits 0/1/4 — shifter up/down, horn).
///   clutch_raw: clutch raw counts (slot 3, Phase F WS5 — module band
///     ho to ignore hota hai, mask mein bit hi nahi hoti).
/// Return packet length (20 + 2*axes + 2*n_enc + 2), ya 0 (ghalat input).
static inline size_t rsh_build_v2_stream_packet(
    uint8_t *buf, size_t buf_len, uint16_t seq, uint32_t timestamp_us,
    uint8_t flags, uint16_t steering, uint8_t axes_is_mv,
    uint16_t accel_raw, uint16_t brake_raw, uint16_t handbrake_raw,
    const int16_t *enc, uint8_t n_enc, uint8_t buttons_v1,
    const RshModuleEnable *en, uint16_t clutch_raw) {
  if (!buf || !enc) return 0;
  if (n_enc > 2) return 0;  // wire slots: 2 enc values (hamesha 0 — v2.0.19)
  if (flags & 0xF0) return 0;  // flags bits 4-7 reserved 0 (§1.3)
  uint8_t mask = rsh_stream_axis_mask(en);
  uint16_t axes[5];  // Phase F WS5: 5 axes max (clutch + handbrake)
  uint8_t n = 0;
  // Slots ascending: 0 steering, 1 accel, 2 brake, 3 clutch, 4 handbrake.
  // (Phase F WS5: handbrake slot 3 se 4 par gaya — clutch ke liye slot 3.)
  // P1-1: har slot ka mV bit alag — I2C value pehle se mV hai, us par
  // dobara raw->mV NAHI lagta; native raw counts convert hote hain.
  if (mask & (uint8_t)(1u << RSH_AXIS_SLOT_STEERING))
    axes[n++] = (axes_is_mv & (uint8_t)(1u << RSH_AXIS_SLOT_STEERING))
                    ? steering
                    : rsh_raw_to_mv(steering);
  if (mask & (uint8_t)(1u << RSH_AXIS_SLOT_ACCEL))
    axes[n++] = (axes_is_mv & (uint8_t)(1u << RSH_AXIS_SLOT_ACCEL))
                    ? accel_raw
                    : rsh_raw_to_mv(accel_raw);
  if (mask & (uint8_t)(1u << RSH_AXIS_SLOT_BRAKE))
    axes[n++] = (axes_is_mv & (uint8_t)(1u << RSH_AXIS_SLOT_BRAKE))
                    ? brake_raw
                    : rsh_raw_to_mv(brake_raw);
  if (mask & (uint8_t)(1u << RSH_AXIS_SLOT_CLUTCH))
    axes[n++] = (axes_is_mv & (uint8_t)(1u << RSH_AXIS_SLOT_CLUTCH))
                    ? clutch_raw
                    : rsh_raw_to_mv(clutch_raw);
  if (mask & (uint8_t)(1u << RSH_AXIS_SLOT_HANDBRAKE))
    axes[n++] = (axes_is_mv & (uint8_t)(1u << RSH_AXIS_SLOT_HANDBRAKE))
                    ? handbrake_raw
                    : rsh_raw_to_mv(handbrake_raw);
  uint32_t buttons = rsh_stream_button_bits(
      en, (uint8_t)((buttons_v1 & RSH_BTN_GEAR_UP) != 0),
      (uint8_t)((buttons_v1 & RSH_BTN_GEAR_DOWN) != 0),
      (uint8_t)((buttons_v1 & RSH_BTN_HORN) != 0));
  size_t total = (size_t)RSH_V2_STREAM_HEADER_SIZE + 2u * n + 2u * n_enc + 2u;
  if (buf_len < total) return 0;
  return rsh_v2_build_stream(buf, seq, timestamp_us, flags, mask, axes,
                             n_enc, enc, buttons);
}
