#pragma once
// RSH v2.1 stream emission (Phase E WS5, PROTOCOL-V2.1.md §1.1) — PURE
// (Arduino-free) taake host par unit test ho sake.
//
// 200 Hz loop (rsh.ino) ka faisla (SEC-E-14, Phase F):
//   peer active + session authenticated ho -> v2 / v1 (HELO proto se)
//   session authenticated na ho            -> kuch nahi (HELO akela kaafi nahi)
//   koi PC active na ho                    -> kuch nahi
// "Authenticated session" = is boot mein is peer se kam az kam ek
// HMAC-verified control command ACCEPT hua ho (net.cpp flag; GET_CAPS
// aam tor par pehla hota hai). Status packet (1 Hz) is se alag hai —
// waisa hi rehta hai.
//
// Wire rules (§1.1, D6): sab axes mV mein hain. Steering mV mode mein
// analogReadMilliVolts se seedha mV aata hai; pedals/clutch/handbrake
// raw counts (0-4095) aate hain — unhein yahin nominal ADC scale se mV
// mein badla jata hai (calibration PC-side hoti hai; ye scale sirf
// wire unit hai). Clutch slot 3, handbrake slot 4 (Phase F WS5).
// Encoders cumulative i16 hain (D1). Buttons v1 byte (RSH_BTN_* bits)
// ki bit positions v2 bits 0..4 se milti hain (gear up/down, accel,
// brake, horn) — module gating rsh_stream_button_bits karta hai.
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

/// Emission faisla: sirf ACTIVE + AUTHENTICATED session ko bhejo;
/// version HELO proto se. SEC-E-14: HELO (unauthenticated UDP
/// bootstrap) se stream nahi khulta — pehle is session mein kam az kam
/// ek HMAC-verified control command ACCEPT hona lazmi hai (aam tor par
/// GET_CAPS; board net.cpp mein sessionAuthenticated flag rakhta hai).
/// Unauthenticated peer ko v1 bhi nahi milta (v1 ke paas auth ka rasta
/// hi nahi — v1-only peer ko stream band hai; v2.1 app GET_CAPS se
/// authenticate hoti hai). Status packet (1 Hz) is se alag hai.
static inline rsh_emit_kind_t rsh_stream_emit_kind(uint8_t pc_active,
                                                   uint8_t pc_is_v2,
                                                   uint8_t session_authenticated) {
  if (!pc_active || !session_authenticated) return RSH_EMIT_NONE;
  return pc_is_v2 ? RSH_EMIT_V2 : RSH_EMIT_V1;
}

/// v2 stream packet banao (InputSample jaisi values se — .ino se call
/// hota hai). Axes mask-order mein bhare jate hain (rsh_v2_build_stream
/// ka contract: axes[] ascending slot order mein).
///   steering: steering value; steering_is_mv=0 ho to raw->mV convert.
///   n_enc: encoders enabled hon to 2 (enc[0..1]), warna 0.
///   buttons_v1: RSH_BTN_* byte.
///   clutch_raw: clutch raw counts (slot 3, Phase F WS5 — module band
///     ho to ignore hota hai, mask mein bit hi nahi hoti).
///   shifter_gear: H-pattern gear 0..6 (sequential mein 0) — button
///     bits 5..10 mein jata hai (rsh_stream_button_bits).
/// Return packet length (20 + 2*axes + 2*n_enc + 2), ya 0 (ghalat input).
static inline size_t rsh_build_v2_stream_packet(
    uint8_t *buf, size_t buf_len, uint16_t seq, uint32_t timestamp_us,
    uint8_t flags, uint16_t steering, uint8_t steering_is_mv,
    uint16_t accel_raw, uint16_t brake_raw, uint16_t handbrake_raw,
    const int16_t *enc, uint8_t n_enc, uint8_t buttons_v1,
    const RshModuleEnable *en, uint16_t clutch_raw, uint8_t shifter_gear) {
  if (!buf || !enc) return 0;
  if (n_enc > 2) return 0;  // sirf 2 encoders maujood hain (enc[0..1])
  if (flags & 0xF0) return 0;  // flags bits 4-7 reserved 0 (§1.3)
  uint8_t mask = rsh_stream_axis_mask(en);
  uint16_t axes[5];  // Phase F WS5: 5 axes max (clutch + handbrake)
  uint8_t n = 0;
  // Slots ascending: 0 steering, 1 accel, 2 brake, 3 clutch, 4 handbrake.
  // (Phase F WS5: handbrake slot 3 se 4 par gaya — clutch ke liye slot 3.)
  if (mask & (uint8_t)(1u << RSH_AXIS_SLOT_STEERING))
    axes[n++] = steering_is_mv ? steering : rsh_raw_to_mv(steering);
  if (mask & (uint8_t)(1u << RSH_AXIS_SLOT_ACCEL))
    axes[n++] = rsh_raw_to_mv(accel_raw);
  if (mask & (uint8_t)(1u << RSH_AXIS_SLOT_BRAKE))
    axes[n++] = rsh_raw_to_mv(brake_raw);
  if (mask & (uint8_t)(1u << RSH_AXIS_SLOT_CLUTCH))
    axes[n++] = rsh_raw_to_mv(clutch_raw);
  if (mask & (uint8_t)(1u << RSH_AXIS_SLOT_HANDBRAKE))
    axes[n++] = rsh_raw_to_mv(handbrake_raw);
  uint32_t buttons = rsh_stream_button_bits(
      en, (uint8_t)((buttons_v1 & RSH_BTN_GEAR_UP) != 0),
      (uint8_t)((buttons_v1 & RSH_BTN_GEAR_DOWN) != 0),
      (uint8_t)((buttons_v1 & RSH_BTN_ACCEL) != 0),
      (uint8_t)((buttons_v1 & RSH_BTN_BRAKE) != 0),
      (uint8_t)((buttons_v1 & RSH_BTN_HORN) != 0), shifter_gear);
  size_t total = (size_t)RSH_V2_STREAM_HEADER_SIZE + 2u * n + 2u * n_enc + 2u;
  if (buf_len < total) return 0;
  return rsh_v2_build_stream(buf, seq, timestamp_us, flags, mask, axes,
                             n_enc, enc, buttons);
}
