#pragma once
// RSH wired USB-serial transport — serial framing (Phase G, D26 / OPEN-20).
// PURE (Arduino-free) taake host par unit test ho sake.
//
// Board: ESP32 DevKit V1 — is board par native USB NAHI hai; USB-serial
// ka matlab CP2102 USB-UART bridge (Serial) hai. S3 ka native USB OTG
// future path hai (Phase G/H HIDMaestro evaluation ke saath), is spec ka
// hissa nahi.
//
// Frame (dono directions, §10 PROTOCOL-V2.1.md):
//   magic "RSHS" (4 B) | kind u8 | len u16 LE | payload (len B) | CRC16 (2 B)
//   CRC16-CCITT (§0: poly 0x1021, init 0xFFFF) — magic + kind + len +
//   payload par, yani CRC field se pehle ke tamam bytes par (D2).
//   Max payload 1053 B (bara control frame, §3.1) → max frame 1062 B.
//
// Kinds (payload wahi bytes hain jo UDP par jate — koi naya wire format
// nahi, sirf serial envelope hai):
//   1 STREAM  — v2.1 "RSH2" stream packet (§1.1) ya v1 18 B packet
//   2 STATUS  — v2.1 status packet (§1.2, 34/38 B)
//   3 HELLO   — HELLO reply (§2.2, 92 B)
//   4 CONTROL — "RSHC" control frame (§3.1, max 1053 B)
//   5 PING    — "RSHN" ping frame (§9, 34 B)
//   6 HELO    — HELO request (§2.1, 5 B "HELO" + proto + sessionFlag)
//   7 PROBE   — "RSHP" probe frame (§3.8)
//
// Baud: 921600, 8N1 (config.h RSH_SERIAL_BAUD). 115200 kaafi NAHI:
// 200 Hz x (36 B stream + 9 B envelope) = ~9000 B/s = 115200 baud ka
// ~78% — control replies/status ke liye koi headroom nahi. 921600 par
// ~10% utilization hai. CP2102 921600 support karta hai.
//
// Decoder byte-at-a-time hai (Serial se jese bytes aain): partial frame,
// chunked delivery aur beech mein garbage (misal boot log / reset par
// bootloader output) sab sambhalta hai — ghalat magic/CRC par resync
// hota hai, frame kabhi aadha deliver nahi hota.
#include <stdint.h>
#include <stddef.h>
#include <string.h>
#include "protocol.h"   // rsh_crc16_ccitt, RSH_V2_CTRL_MAX_FRAME

#define RSH_SERIAL_MAGIC_0 'R'
#define RSH_SERIAL_MAGIC_1 'S'
#define RSH_SERIAL_MAGIC_2 'H'
#define RSH_SERIAL_MAGIC_3 'S'
#define RSH_SERIAL_HEADER_SIZE 7    // magic(4) + kind(1) + len(2)
#define RSH_SERIAL_CRC_SIZE 2
#define RSH_SERIAL_MAX_PAYLOAD RSH_V2_CTRL_MAX_FRAME  // 1053 — bara control frame
#define RSH_SERIAL_MAX_FRAME (RSH_SERIAL_HEADER_SIZE + RSH_SERIAL_MAX_PAYLOAD + RSH_SERIAL_CRC_SIZE)  // 1062

// Kind bytes (§10).
#define RSH_SERIAL_KIND_STREAM  1
#define RSH_SERIAL_KIND_STATUS  2
#define RSH_SERIAL_KIND_HELLO   3
#define RSH_SERIAL_KIND_CONTROL 4
#define RSH_SERIAL_KIND_PING    5
#define RSH_SERIAL_KIND_HELO    6
#define RSH_SERIAL_KIND_PROBE   7

typedef enum {
  RSH_SERIAL_OK = 0,
  RSH_SERIAL_TOO_SHORT,    // frame header (7 B) bhi poora nahi
  RSH_SERIAL_BAD_MAGIC,    // pehle 4 bytes "RSHS" nahi
  RSH_SERIAL_BAD_LENGTH,   // len > 1053, ya total length match nahi, ya kind unknown
  RSH_SERIAL_BAD_CRC       // CRC16 mismatch
} rsh_serial_result_t;

/// Kind valid hai? (1..7 — §10 table.)
static inline uint8_t rsh_serial_kind_valid(uint8_t kind) {
  return kind >= RSH_SERIAL_KIND_STREAM && kind <= RSH_SERIAL_KIND_PROBE;
}

/// Ek frame banao. buf kam az kam RSH_SERIAL_MAX_FRAME (1062) bytes ka
/// ho. Return total frame length, ya 0 (kind unknown / payload > 1053 /
/// buf chhota). CRC header+payload par banta hai (§0/D2).
static inline size_t rsh_serial_build_frame(uint8_t *buf, size_t buf_len,
                                            uint8_t kind,
                                            const uint8_t *payload,
                                            uint16_t payload_len) {
  if (!buf) return 0;
  if (!rsh_serial_kind_valid(kind)) return 0;
  if (payload_len > RSH_SERIAL_MAX_PAYLOAD) return 0;
  size_t total = (size_t)RSH_SERIAL_HEADER_SIZE + payload_len + RSH_SERIAL_CRC_SIZE;
  if (buf_len < total) return 0;
  buf[0] = RSH_SERIAL_MAGIC_0; buf[1] = RSH_SERIAL_MAGIC_1;
  buf[2] = RSH_SERIAL_MAGIC_2; buf[3] = RSH_SERIAL_MAGIC_3;
  buf[4] = kind;
  rsh_put_u16(buf + 5, payload_len);
  if (payload_len > 0) memcpy(buf + RSH_SERIAL_HEADER_SIZE, payload, payload_len);
  rsh_put_u16(buf + total - RSH_SERIAL_CRC_SIZE,
              rsh_crc16_ccitt(buf, total - RSH_SERIAL_CRC_SIZE));
  return total;
}

/// Ek poora frame ek dafa mein parse karo (tests / vectors ke liye).
/// payload_out caller ka buffer hai (kam az kam len bytes); payload_len_out
/// mein payload length milti hai. TOO_SHORT sirf tab jab 7 B header bhi
/// poora na ho; header parh liya gaya ho to har length mismatch
/// (truncation samet) → BAD_LENGTH (control §3.1 jaisa rule).
static inline rsh_serial_result_t rsh_serial_parse_frame(
    const uint8_t *data, size_t len, uint8_t *kind_out,
    uint8_t *payload_out, uint16_t *payload_len_out) {
  if (!data) return RSH_SERIAL_TOO_SHORT;
  if (len < RSH_SERIAL_HEADER_SIZE) return RSH_SERIAL_TOO_SHORT;
  if (data[0] != RSH_SERIAL_MAGIC_0 || data[1] != RSH_SERIAL_MAGIC_1 ||
      data[2] != RSH_SERIAL_MAGIC_2 || data[3] != RSH_SERIAL_MAGIC_3)
    return RSH_SERIAL_BAD_MAGIC;
  uint8_t kind = data[4];
  if (!rsh_serial_kind_valid(kind)) return RSH_SERIAL_BAD_LENGTH;
  uint16_t payload_len = rsh_get_u16(data + 5);
  if (payload_len > RSH_SERIAL_MAX_PAYLOAD) return RSH_SERIAL_BAD_LENGTH;
  if (len != (size_t)RSH_SERIAL_HEADER_SIZE + payload_len + RSH_SERIAL_CRC_SIZE)
    return RSH_SERIAL_BAD_LENGTH;
  if (rsh_crc16_ccitt(data, len - RSH_SERIAL_CRC_SIZE) !=
      rsh_get_u16(data + len - RSH_SERIAL_CRC_SIZE))
    return RSH_SERIAL_BAD_CRC;
  if (kind_out) *kind_out = kind;
  if (payload_len_out) *payload_len_out = payload_len;
  if (payload_out && payload_len > 0)
    memcpy(payload_out, data + RSH_SERIAL_HEADER_SIZE, payload_len);
  return RSH_SERIAL_OK;
}

// ---- Incremental decoder (Serial stream se) ----
//
// Har byte feed karo; jab poora valid frame tayyar ho to feed 1 return
// karta hai aur frame decoder ke fields mein hota hai (caller use foran
// copy kare — agla byte aate hi buffer dobara istemal hota hai).
// Ghalat frame (bad magic / bad CRC / bad length) khamoshi se gira kar
// resync hota hai — counters mein gin'a jata hai, kabhi aadha frame
// deliver nahi hota. Garbage bytes (boot log) magic scan mein girte hain.
typedef struct {
  uint8_t buf[RSH_SERIAL_MAX_FRAME];  // header + payload + CRC
  uint16_t pos;        // buf mein ab tak kitne bytes hain
  uint16_t want;       // poore frame ke liye kitne bytes chahiye (0 = header ka intezar)
  uint8_t frame_kind;  // aakhri complete frame ka kind
  uint16_t frame_len;  // aakhri complete frame ki payload length
  // Counters (diagnostics — status packet mein nahi jate, host tests + debug):
  uint32_t frames_ok;
  uint32_t bad_magic;   // magic scan mein gira hua byte / ghalat magic
  uint32_t bad_crc;     // CRC mismatch par gira frame
  uint32_t bad_length;  // len > 1053 ya kind unknown
} rsh_serial_decoder_t;

static inline void rsh_serial_decoder_init(rsh_serial_decoder_t *d) {
  memset(d, 0, sizeof(*d));
}

/// Ek byte feed karo. Return 1 = poora valid frame tayyar hai
/// (d->frame_kind / d->buf + RSH_SERIAL_HEADER_SIZE / d->frame_len se
/// parho); 0 = abhi frame poora nahi (ya ghalat frame gira diya gaya).
static inline uint8_t rsh_serial_decoder_feed(rsh_serial_decoder_t *d, uint8_t b) {
  if (!d) return 0;
  if (d->want == 0) {
    // Header ka intezar: 4-byte magic window slide karo.
    if (d->pos < 4) {
      d->buf[d->pos++] = b;
      if (d->pos < 4) return 0;
    } else {
      d->buf[0] = d->buf[1]; d->buf[1] = d->buf[2]; d->buf[2] = d->buf[3];
      d->buf[3] = b;
    }
    if (d->buf[0] != RSH_SERIAL_MAGIC_0 || d->buf[1] != RSH_SERIAL_MAGIC_1 ||
        d->buf[2] != RSH_SERIAL_MAGIC_2 || d->buf[3] != RSH_SERIAL_MAGIC_3) {
      // Magic nahi mila — byte gira; window ke aakhri 3 bytes agle
      // byte ke saath dobara check honge (overlap wala magic na chhoote).
      // pos=4 rehta hai (window full), bad_magic sirf tab gino jab
      // window mein koi bhi magic byte na ho? Nahi — har gira byte gino:
      // diagnostics ke liye seedha hai (garbage storm mein bara hota hai,
      // wahi matlab hai).
      d->bad_magic++;
      return 0;
    }
    // Magic mil gaya — ab kind + len (3 bytes) ka intezar.
    d->pos = 4;
    d->want = RSH_SERIAL_HEADER_SIZE;  // pehle header poora karo
    return 0;
  }
  // Header/payload bytes jama karo.
  d->buf[d->pos++] = b;
  if (d->pos < d->want) return 0;
  if (d->want == RSH_SERIAL_HEADER_SIZE) {
    // Header poora: kind + len validate karo.
    uint8_t kind = d->buf[4];
    uint16_t payload_len = rsh_get_u16(d->buf + 5);
    if (!rsh_serial_kind_valid(kind) || payload_len > RSH_SERIAL_MAX_PAYLOAD) {
      d->bad_length++;
      d->pos = 0;
      d->want = 0;
      return 0;
    }
    d->want = (uint16_t)(RSH_SERIAL_HEADER_SIZE + payload_len + RSH_SERIAL_CRC_SIZE);
    if (d->pos >= d->want) {
      // Empty payload (len 0): header ke saath hi CRC bytes bhi aa chuke
      // honge? Nahi — len 0 par want = 9, pos = 7, is liye yahan nahi
      // aata. Ye branch sirf hifazati hai.
    } else {
      return 0;
    }
  }
  // Poora frame aa gaya (header + payload + CRC) — CRC check karo.
  uint16_t total = d->want;
  uint16_t payload_len = rsh_get_u16(d->buf + 5);
  if (rsh_crc16_ccitt(d->buf, total - RSH_SERIAL_CRC_SIZE) !=
      rsh_get_u16(d->buf + total - RSH_SERIAL_CRC_SIZE)) {
    d->bad_crc++;
    d->pos = 0;
    d->want = 0;
    return 0;
  }
  d->frame_kind = d->buf[4];
  d->frame_len = payload_len;
  d->frames_ok++;
  d->pos = 0;
  d->want = 0;
  return 1;
}

/// Buffer feed karo; complete frames ki ginti return karta hai.
/// Caller har complete frame ke baad decoder fields parh le — ye function
/// ek se zyada frame aane par sirf AAKHRI frame chhorta hai (caller ko
/// har frame ke baad feed rokni ho to ek-ek byte feed kare).
static inline uint32_t rsh_serial_decoder_feed_buf(rsh_serial_decoder_t *d,
                                                   const uint8_t *data, size_t len) {
  uint32_t n = 0;
  if (!d || !data) return 0;
  for (size_t i = 0; i < len; i++)
    if (rsh_serial_decoder_feed(d, data[i])) n++;
  return n;
}
