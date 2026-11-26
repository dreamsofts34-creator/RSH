#pragma once
// RSH packet protocol — 18 bytes, little-endian (docs: 02-architecture.md)
// Ye file Arduino se bilkul free hai taake host par unit test ho sake.
#include <stdint.h>
#include <stddef.h>
#include <string.h>

#define RSH_PACKET_SIZE   18
#define RSH_MAGIC         0xA5
#define RSH_VERSION       1

// buttons byte (offset 14)
#define RSH_BTN_GEAR_UP    0x01
#define RSH_BTN_GEAR_DOWN  0x02
#define RSH_BTN_ACCEL      0x04
#define RSH_BTN_BRAKE      0x08
#define RSH_BTN_HORN       0x10   // FW-11: horn button (PIN_HORN_BTN, GPIO4)

// flags byte (offset 15)
#define RSH_FLAG_STEER_MV      0x01   // bit0: steering value millivolts mein hai (warna raw counts)
#define RSH_FLAG_AXIS_INVALID  0x02   // bit1: koi axis invalid hai (F2 floating/disconnected) — v2 stream flag bit3 (0x08) se alag, ye v1 packet ka bit hai

// CRC16-CCITT: poly 0x1021, init 0xFFFF (koi final XOR nahi)
// Table-driven (F14 — bit-by-bit nahi). Table pehli call par banti hai;
// har entry ka matlab bit-by-bit wale tareeqe jaisa hi hai.
static uint16_t rsh_crc16_table_[256];
static int rsh_crc16_table_ready_ = 0;
static inline void rsh_crc16_table_init_(void) {
  if (rsh_crc16_table_ready_) return;
  for (int i = 0; i < 256; i++) {
    uint16_t c = (uint16_t)i << 8;
    for (int b = 0; b < 8; b++)
      c = (c & 0x8000) ? (uint16_t)((c << 1) ^ 0x1021) : (uint16_t)(c << 1);
    rsh_crc16_table_[i] = c;
  }
  rsh_crc16_table_ready_ = 1;
}

static inline uint16_t rsh_crc16_ccitt(const uint8_t *data, size_t len) {
  rsh_crc16_table_init_();
  uint16_t crc = 0xFFFF;
  for (size_t i = 0; i < len; i++)
    crc = (uint16_t)((crc << 8) ^ rsh_crc16_table_[((crc >> 8) ^ data[i]) & 0xFF]);
  return crc;
}

static inline void rsh_put_u16(uint8_t *p, uint16_t v) {
  p[0] = (uint8_t)(v & 0xFF);
  p[1] = (uint8_t)((v >> 8) & 0xFF);
}
static inline void rsh_put_u32(uint8_t *p, uint32_t v) {
  p[0] = (uint8_t)(v & 0xFF);
  p[1] = (uint8_t)((v >> 8) & 0xFF);
  p[2] = (uint8_t)((v >> 16) & 0xFF);
  p[3] = (uint8_t)((v >> 24) & 0xFF);
}

// Packet banao. buf kam az kam RSH_PACKET_SIZE bytes ka ho. Hamesha 18 return.
static inline size_t rsh_build_packet(uint8_t *buf, uint16_t seq, uint32_t timestamp_us,
                                       uint16_t steering, uint16_t accel_analog,
                                       uint16_t brake_analog, uint8_t buttons, uint8_t flags) {
  buf[0] = RSH_MAGIC;
  buf[1] = RSH_VERSION;
  rsh_put_u16(buf + 2, seq);
  rsh_put_u32(buf + 4, timestamp_us);
  rsh_put_u16(buf + 8, steering);
  rsh_put_u16(buf + 10, accel_analog);
  rsh_put_u16(buf + 12, brake_analog);
  buf[14] = buttons;
  buf[15] = flags;
  uint16_t crc = rsh_crc16_ccitt(buf, 16);
  rsh_put_u16(buf + 16, crc);
  return RSH_PACKET_SIZE;
}

// ============================================================================
// Protocol v2.2 (no-pairing redesign, NO-PAIRING-PLAN.md §3 D1/D2/D13) —
// docs/PROTOCOL-V2.2.md authority hai. v2.1 se farq:
//   * HELLO 92 B barqarar, lekin nonce/device_key/wifi_password fields
//     hamesha zero/reserved hain (koi secret kabhi disclose nahi hota);
//     flags sirf bit2 safe_mode rakhta hai (bit0 unpaired, bit1
//     key_reset_armed, bit3 window_open — sab reserved 0; D7: password
//     clear seedha physical hai, koi armed state nahi).
//   * Control frames mein HMAC khatam (coordinator decision — WS-2 commit
//     b5f96c2 se byte-exact match): layout "RSHC" | cmd u8 | reqId u16 LE |
//     len u16 LE | payload[len] | CRC16. Header 9 B, kul 11 + len, max
//     frame 1035 B. Na tag field hai, na counter field. Duplicate cache
//     sirf reqId par hai (D2); reqId per session monotonic u16 hai.
//   * SET_PASSWORD (0x0C) payload v2.2: [curLen u8][curPw][newLen u8]
//     [newPw]; ghalat current password -> error code 1 (BadPassword).
//   * Version byte 0x21 -> 0x22 (HELLO proto bytes (2,1) -> (2,2)).
//   * BAD_TAG error code + RSH_V2_BAD_TAG result delete hue.
// Sab integers little-endian. CRC16 hamesha aakhir mein, CRC field se pehle
// ke tamam bytes par. Product naam RSH hai.
// ============================================================================

#define RSH_V2_VERSION            0x22   // wire version byte (§0)
#define RSH_V2_MAX_AXES           8
#define RSH_V2_MAX_ENC            4
#define RSH_V2_MAX_BUTTONS_WIRE   64     // wire: stream buttons + status btnsEx (§1.1 N1)
#define RSH_V2_MAX_BUTTONS_CONFIG 128    // config ceiling (§0)
#define RSH_V2_STREAM_HEADER_SIZE 20
#define RSH_V2_STATUS_BASE_SIZE   34
#define RSH_V2_STATUS_EXT_SIZE    38
#define RSH_V2_HELLO_SIZE         92
#define RSH_V2_CTRL_HEADER_SIZE   9
#define RSH_V2_CTRL_MAX_PAYLOAD   1024
#define RSH_V2_CTRL_MAX_FRAME     (RSH_V2_CTRL_HEADER_SIZE + RSH_V2_CTRL_MAX_PAYLOAD + 2) // 1035 (9 header + payload + CRC16)
#define RSH_V2_PING_SIZE          34     // NTP-style ping frame, fixed (§9.1)

// Stream flags (§1.3, D4)
#define RSH_V2_FLAG_CONFIG_CHANGED 0x01
#define RSH_V2_FLAG_LOW_SUPPLY     0x02
#define RSH_V2_FLAG_TICK_SKIPPED   0x04
#define RSH_V2_FLAG_AXIS_INVALID   0x08
// HELLO flags (§2.2, v2.2): sirf bit2 safe_mode bacha hai. bit0 (unpaired),
// bit1 (key_reset_armed) aur bit3 (window_open) reserved 0 hain — parser
// in mein se koi bhi bit set ho to RSV_NONZERO deta hai.
#define RSH_V2_HELLO_SAFE_MODE     0x04

// Control error codes (§3.9)
#define RSH_CTRL_OK           0
#define RSH_CTRL_BAD_PASSWORD 1   // v2.2: SET_PASSWORD current password ghalat
                                  // (D3 — freed BadTag slot ki value, Rust
                                  // ErrorCode::BadPassword = 1 se match).
#define RSH_CTRL_REPLAY       2   // v2.2: reserved — kabhi emit nahi hota
                                  // (reqId-only dedup mein replay ka koi alag
                                  // natija nahi; dobara reqId = duplicate,
                                  // cached reply milta hai).
#define RSH_CTRL_BAD_CRC      3
#define RSH_CTRL_TOO_BIG      4
#define RSH_CTRL_PIN_CONFLICT 5
#define RSH_CTRL_UNSUPPORTED  6
#define RSH_CTRL_BUSY         7
#define RSH_CTRL_NVS_FAIL     8   // F-07 (polish round, v4.0.6): SET_PASSWORD
                                  // ki synchronous NVS write fail hui —
// password NAHI badla; app dobara koshish kare ya purana password
                                  // istemal kare. Rust ErrorCode::NvsFail = 8
                                  // (crates/rsh-protocol/src/control.rs).

// Control commands (§3.3); reply mein cmd = reqCmd | 0x80
#define RSH_CMD_GET_CAPS     0x01
#define RSH_CMD_GET_CFG      0x02
#define RSH_CMD_SET_CFG      0x03
#define RSH_CMD_COMMIT       0x04
#define RSH_CMD_CONFIRM      0x05
#define RSH_CMD_REVERT       0x06
#define RSH_CMD_PIN_PROBE    0x07
#define RSH_CMD_FACTORY_RESET 0x08
#define RSH_CMD_REBOOT       0x09
#define RSH_CMD_LED_TEST     0x0A
// 0x0B = KEY_RESET — v2.2 mein DELETE (wire opcode khatam; password clear
// sirf physical BOOT-hold se hota hai, D7; value dobara istemal nahi hoti).
#define RSH_CMD_SET_PASSWORD 0x0C
// v4.0.2 additive commands (F-10 / D-5): maujooda frames nahi badle —
// purana app inhein kabhi nahi bhejta, naya app purane firmware (4.0.1)
// se UNSUPPORTED pata hai. GET_DIAG body diag_log.h mein defined hai;
// HORN_TEST payload: duration_ms u16 LE (100..5000).
#define RSH_CMD_GET_DIAG     0x0D
#define RSH_CMD_HORN_TEST    0x0E

typedef enum {
  RSH_V2_OK = 0,
  RSH_V2_TOO_SHORT,
  RSH_V2_BAD_MAGIC,
  RSH_V2_BAD_VERSION,
  RSH_V2_BAD_FRAME,    // nEnc > 4, length mismatch, mode/sampleCount ghalat
  RSH_V2_BAD_CRC,
  RSH_V2_RSV_NONZERO,  // reserved bytes != 0 ya flags bits 4-7 nonzero
  RSH_V2_BAD_LENGTH,   // status/control length valid set mein nahi
  RSH_V2_LEGACY_V1,    // pehle 2 bytes 0xA5 0x01 — v1 parser istemal karo
  RSH_V2_UNSUPPORTED   // nBtn 65-128 (SET_CFG rule, §1.1 N1)
} rsh_v2_result_t;

static inline uint8_t rsh_popcount8(uint8_t mask) {
  uint8_t n = 0;
  while (mask) { n += mask & 1; mask >>= 1; }
  return n;
}

static inline uint16_t rsh_get_u16(const uint8_t *p) {
  return (uint16_t)(p[0] | (p[1] << 8));
}
static inline uint32_t rsh_get_u32(const uint8_t *p) {
  return (uint32_t)p[0] | ((uint32_t)p[1] << 8) | ((uint32_t)p[2] << 16) | ((uint32_t)p[3] << 24);
}
static inline uint64_t rsh_get_u64(const uint8_t *p) {
  return (uint64_t)rsh_get_u32(p) | ((uint64_t)rsh_get_u32(p + 4) << 32);
}
static inline void rsh_put_u64(uint8_t *p, uint64_t v) {
  rsh_put_u32(p, (uint32_t)(v & 0xFFFFFFFFu));
  rsh_put_u32(p + 4, (uint32_t)(v >> 32));
}
static inline int16_t rsh_get_i16(const uint8_t *p) {
  return (int16_t)rsh_get_u16(p);
}

/// Button count ka faisla (§1.1 N1): <=64 OK, 65-128 UNSUPPORTED (SET_CFG),
/// >128 BAD_FRAME (parser bound).
static inline rsh_v2_result_t rsh_v2_check_button_count(int n_btn) {
  if (n_btn < 0 || n_btn > RSH_V2_MAX_BUTTONS_CONFIG) return RSH_V2_BAD_FRAME;
  if (n_btn > RSH_V2_MAX_BUTTONS_WIRE) return RSH_V2_UNSUPPORTED;
  return RSH_V2_OK;
}

// ---- Stream v2.2 (§1.1) ----
typedef struct {
  uint16_t seq;
  uint32_t timestamp_us;
  uint8_t flags;
  uint8_t axis_mask;
  uint32_t buttons;       // bit n = button n, n = 0..31
  uint8_t n_enc;
  uint16_t axes[8];       // axis index par; sirf mask wale valid
  int16_t enc[4];         // cumulative counts (i16, wrap allowed)
} rsh_v2_stream_t;

static inline rsh_v2_result_t rsh_v2_parse_stream(const uint8_t *data, size_t len,
                                                  rsh_v2_stream_t *out) {
  if (!data || !out) return RSH_V2_TOO_SHORT;
  if (len < 2) return RSH_V2_TOO_SHORT;
  if (data[0] == 0xA5 && data[1] == 0x01) return RSH_V2_LEGACY_V1;
  if (len < 4) return RSH_V2_TOO_SHORT;
  if (data[0] != 'R' || data[1] != 'S' || data[2] != 'H' || data[3] != '2')
    return RSH_V2_BAD_MAGIC;
  if (len < 5) return RSH_V2_TOO_SHORT;
  if (data[4] != RSH_V2_VERSION) return RSH_V2_BAD_VERSION;
  if (len < RSH_V2_STREAM_HEADER_SIZE) return RSH_V2_TOO_SHORT;

  uint8_t flags = data[5];
  uint8_t axis_mask = data[12];
  uint8_t n_enc = data[13];
  if (n_enc > RSH_V2_MAX_ENC) return RSH_V2_BAD_FRAME;
  size_t expected = (size_t)RSH_V2_STREAM_HEADER_SIZE
      + 2u * rsh_popcount8(axis_mask) + 2u * n_enc + 2u;
  if (len != expected) return RSH_V2_BAD_FRAME;
  if (rsh_get_u16(data + 18) != 0) return RSH_V2_RSV_NONZERO;
  if (flags & 0xF0) return RSH_V2_RSV_NONZERO;  // flags bits 4-7 reserved 0 (§1.3)
  if (rsh_crc16_ccitt(data, len - 2) != rsh_get_u16(data + len - 2))
    return RSH_V2_BAD_CRC;

  memset(out, 0, sizeof(*out));
  out->seq = rsh_get_u16(data + 6);
  out->timestamp_us = rsh_get_u32(data + 8);
  out->flags = flags;
  out->axis_mask = axis_mask;
  out->buttons = rsh_get_u32(data + 14);
  out->n_enc = n_enc;
  size_t off = RSH_V2_STREAM_HEADER_SIZE;
  for (int i = 0; i < 8; i++) {
    if (axis_mask & (uint8_t)(1u << i)) { out->axes[i] = rsh_get_u16(data + off); off += 2; }
  }
  for (int i = 0; i < n_enc; i++) { out->enc[i] = rsh_get_i16(data + off); off += 2; }
  return RSH_V2_OK;
}

/// Stream v2.2 packet banao (parser ka aaina). axes: mask-order values.
/// n_enc > 4 ya flags reserved bits par 0 return (ghalat input).
static inline size_t rsh_v2_build_stream(uint8_t *buf, uint16_t seq, uint32_t timestamp_us,
                                         uint8_t flags, uint8_t axis_mask,
                                         const uint16_t *axes, uint8_t n_enc,
                                         const int16_t *enc, uint32_t buttons) {
  if (!buf) return 0;
  if (n_enc > RSH_V2_MAX_ENC) return 0;
  if (flags & 0xF0) return 0;
  uint8_t n_axes = rsh_popcount8(axis_mask);
  size_t total = (size_t)RSH_V2_STREAM_HEADER_SIZE + 2u * n_axes + 2u * n_enc + 2u;
  buf[0] = 'R'; buf[1] = 'S'; buf[2] = 'H'; buf[3] = '2';
  buf[4] = RSH_V2_VERSION;
  buf[5] = flags;
  rsh_put_u16(buf + 6, seq);
  rsh_put_u32(buf + 8, timestamp_us);
  buf[12] = axis_mask;
  buf[13] = n_enc;
  rsh_put_u32(buf + 14, buttons);
  rsh_put_u16(buf + 18, 0);  // reserved
  size_t off = RSH_V2_STREAM_HEADER_SIZE;
  for (int i = 0; i < n_axes; i++) { rsh_put_u16(buf + off, axes[i]); off += 2; }
  for (int i = 0; i < n_enc; i++) { rsh_put_u16(buf + off, (uint16_t)enc[i]); off += 2; }
  rsh_put_u16(buf + total - 2, rsh_crc16_ccitt(buf, total - 2));
  return total;
}

// ---- Status v2.2 (§1.2) ----
typedef struct {
  uint16_t seq;
  uint32_t uptime_ms;
  int8_t rssi_dbm;
  uint8_t battery_pct;    // 255 = no battery module
  uint16_t supply_mv;     // 0 = not measured
  uint32_t heap;
  uint16_t overruns;
  uint16_t max_cycle_us;
  uint8_t governor;
  uint8_t reset_reason;
  uint32_t cfg_crc;
  uint16_t cfg_ver;
  uint16_t seq_gap;       // boot se skip hue stream seq (D5)
  uint16_t tx_fail;       // boot se failed UDP transmissions (D5/F3)
  uint32_t btns_ex;       // buttons 32-63, bit (n-32) (D1)
  uint8_t has_btns_ex;    // 1 = extended 38 B packet
} rsh_v2_status_t;

static inline rsh_v2_result_t rsh_v2_parse_status(const uint8_t *data, size_t len,
                                                  rsh_v2_status_t *out) {
  if (!data || !out) return RSH_V2_TOO_SHORT;
  if (len < 2) return RSH_V2_TOO_SHORT;
  if (data[0] != 0xA6) return RSH_V2_BAD_MAGIC;
  if (data[1] != RSH_V2_VERSION) return RSH_V2_BAD_VERSION;
  if (len < RSH_V2_STATUS_BASE_SIZE) return RSH_V2_TOO_SHORT;
  if (len != RSH_V2_STATUS_BASE_SIZE && len != RSH_V2_STATUS_EXT_SIZE)
    return RSH_V2_BAD_LENGTH;
  if (rsh_crc16_ccitt(data, len - 2) != rsh_get_u16(data + len - 2))
    return RSH_V2_BAD_CRC;

  uint8_t extended = (len == RSH_V2_STATUS_EXT_SIZE) ? 1 : 0;
  size_t seq_gap_off = extended ? 32 : 28;  // CRC-last invariant (§1.2)
  memset(out, 0, sizeof(*out));
  out->seq = rsh_get_u16(data + 2);
  out->uptime_ms = rsh_get_u32(data + 4);
  out->rssi_dbm = (int8_t)data[8];
  out->battery_pct = data[9];
  out->supply_mv = rsh_get_u16(data + 10);
  out->heap = rsh_get_u32(data + 12);
  out->overruns = rsh_get_u16(data + 16);
  out->max_cycle_us = rsh_get_u16(data + 18);
  out->governor = data[20];
  out->reset_reason = data[21];
  out->cfg_crc = rsh_get_u32(data + 22);
  out->cfg_ver = rsh_get_u16(data + 26);
  out->seq_gap = rsh_get_u16(data + seq_gap_off);
  out->tx_fail = rsh_get_u16(data + seq_gap_off + 2);
  out->btns_ex = extended ? rsh_get_u32(data + 28) : 0;
  out->has_btns_ex = extended;
  return RSH_V2_OK;
}

/// Status packet banao (parser ka aaina). has_btns_ex=1 → 38 B, warna 34 B.
static inline size_t rsh_v2_build_status(uint8_t *buf, const rsh_v2_status_t *s) {
  if (!buf || !s) return 0;
  size_t total = s->has_btns_ex ? RSH_V2_STATUS_EXT_SIZE : RSH_V2_STATUS_BASE_SIZE;
  buf[0] = 0xA6;
  buf[1] = RSH_V2_VERSION;
  rsh_put_u16(buf + 2, s->seq);
  rsh_put_u32(buf + 4, s->uptime_ms);
  buf[8] = (uint8_t)s->rssi_dbm;
  buf[9] = s->battery_pct;
  rsh_put_u16(buf + 10, s->supply_mv);
  rsh_put_u32(buf + 12, s->heap);
  rsh_put_u16(buf + 16, s->overruns);
  rsh_put_u16(buf + 18, s->max_cycle_us);
  buf[20] = s->governor;
  buf[21] = s->reset_reason;
  rsh_put_u32(buf + 22, s->cfg_crc);
  rsh_put_u16(buf + 26, s->cfg_ver);
  size_t seq_gap_off = s->has_btns_ex ? 32 : 28;
  if (s->has_btns_ex) rsh_put_u32(buf + 28, s->btns_ex);
  rsh_put_u16(buf + seq_gap_off, s->seq_gap);
  rsh_put_u16(buf + seq_gap_off + 2, s->tx_fail);
  rsh_put_u16(buf + total - 2, rsh_crc16_ccitt(buf, total - 2));
  return total;
}

// ---- HELLO reply (§2.2, 92 B fixed, v2.2) ----
// Layout: "RSHR" | proto (2,2) | flags | sessionEcho | boardMac 6B |
// fw (major, minor, patch) | rsv | nonce[16] | device_key[32] |
// wifi_password[24] | CRC16. nonce/device_key/wifi_password hamesha
// zero/reserved hain (v2.2 mein board kabhi secret disclose nahi karta —
// struct mein fields sirf layout ki wajah se maujood hain, values hamesha 0).
// flags: sirf bit2 safe_mode; bit0/bit1/bit3 reserved 0.
typedef struct {
  uint8_t flags;
  uint8_t session_echo;
  uint8_t board_mac[6];
  uint8_t fw_major, fw_minor, fw_patch;
  uint8_t nonce[16];         // v2.2: hamesha zero (reserved)
  uint8_t device_key[32];    // v2.2: hamesha zero (reserved)
  uint8_t wifi_password[24]; // v2.2: hamesha zero (reserved)
} rsh_v2_hello_t;

static inline rsh_v2_result_t rsh_v2_parse_hello(const uint8_t *data, size_t len,
                                                 rsh_v2_hello_t *out) {
  if (!data || !out) return RSH_V2_TOO_SHORT;
  if (len < 6) return RSH_V2_TOO_SHORT;
  if (data[0] != 'R' || data[1] != 'S' || data[2] != 'H' || data[3] != 'R')
    return RSH_V2_BAD_MAGIC;
  if (data[4] != 2 || data[5] != 2) return RSH_V2_BAD_VERSION;  // v2.2: (2,2)
  if (len < RSH_V2_HELLO_SIZE) return RSH_V2_TOO_SHORT;
  if (len != RSH_V2_HELLO_SIZE) return RSH_V2_BAD_LENGTH;
  if (data[17] != 0) return RSH_V2_RSV_NONZERO;
  if (data[6] & ~0x04u) return RSH_V2_RSV_NONZERO;  // sirf bit2 safe_mode allowed
  for (int i = 18; i < 90; i++)  // nonce + device_key + wifi_password = zero
    if (data[i] != 0) return RSH_V2_RSV_NONZERO;
  if (rsh_crc16_ccitt(data, 90) != rsh_get_u16(data + 90)) return RSH_V2_BAD_CRC;

  memset(out, 0, sizeof(*out));
  out->flags = data[6];
  out->session_echo = data[7];
  memcpy(out->board_mac, data + 8, 6);
  out->fw_major = data[14]; out->fw_minor = data[15]; out->fw_patch = data[16];
  memcpy(out->nonce, data + 18, 16);
  memcpy(out->device_key, data + 34, 32);
  memcpy(out->wifi_password, data + 66, 24);
  return RSH_V2_OK;
}

/// HELLO reply banao (parser ka aaina, §2.2 v2.2). Secret fields
/// (nonce/device_key/wifi_password) builder khud hamesha zero likhta hai —
/// caller unhein de hi nahi sakta (leak ka rasta hi band hai).
/// Hamesha RSH_V2_HELLO_SIZE (92) return, 0 = bad input.
static inline size_t rsh_v2_build_hello(uint8_t *buf, uint8_t flags, uint8_t session_echo,
                                        const uint8_t *board_mac,
                                        uint8_t fw_major, uint8_t fw_minor, uint8_t fw_patch) {
  if (!buf || !board_mac) return 0;
  buf[0] = 'R'; buf[1] = 'S'; buf[2] = 'H'; buf[3] = 'R';
  buf[4] = 2; buf[5] = 2;  // v2.2
  buf[6] = flags; buf[7] = session_echo;
  memcpy(buf + 8, board_mac, 6);
  buf[14] = fw_major; buf[15] = fw_minor; buf[16] = fw_patch;
  buf[17] = 0;  // reserved
  memset(buf + 18, 0, 72);  // nonce (16) + device_key (32) + wifi_password (24) = zero
  rsh_put_u16(buf + 90, rsh_crc16_ccitt(buf, 90));
  return RSH_V2_HELLO_SIZE;
}

// ---- PIN_PROBE frame (§3.8) ----
typedef struct {
  uint16_t seq;
  uint8_t pin;
  uint8_t mode;           // 0 = digital level, 1 = ADC mV
  uint16_t sample_count;  // 1..64
  uint8_t samples_level[64];
  uint16_t samples_mv[64];
} rsh_v2_probe_t;

static inline rsh_v2_result_t rsh_v2_parse_probe(const uint8_t *data, size_t len,
                                                 rsh_v2_probe_t *out) {
  if (!data || !out) return RSH_V2_TOO_SHORT;
  if (len < 6) return RSH_V2_TOO_SHORT;
  if (data[0] != 'R' || data[1] != 'S' || data[2] != 'H' || data[3] != 'P')
    return RSH_V2_BAD_MAGIC;
  if (data[4] != RSH_V2_VERSION) return RSH_V2_BAD_VERSION;
  if (len < 11) return RSH_V2_TOO_SHORT;
  uint8_t mode = data[8];
  uint16_t n = rsh_get_u16(data + 9);
  if (mode > 1) return RSH_V2_BAD_FRAME;
  if (n < 1 || n > 64) return RSH_V2_BAD_FRAME;
  size_t expected = 11u + (size_t)n * (mode == 1 ? 2u : 1u) + 2u;
  if (len != expected) return RSH_V2_BAD_FRAME;
  if (rsh_crc16_ccitt(data, len - 2) != rsh_get_u16(data + len - 2))
    return RSH_V2_BAD_CRC;

  memset(out, 0, sizeof(*out));
  out->seq = rsh_get_u16(data + 5);
  out->pin = data[7];
  out->mode = mode;
  out->sample_count = n;
  if (mode == 0) {
    memcpy(out->samples_level, data + 11, n);
  } else {
    for (int i = 0; i < n; i++) out->samples_mv[i] = rsh_get_u16(data + 11 + 2 * i);
  }
  return RSH_V2_OK;
}

// ---- Control frames v2.2 (§3.1/§3.2) ----
//
// Layout (coordinator decision — WS-2 `control.rs` commit b5f96c2 se
// byte-exact match): "RSHC" | cmd u8 | reqId u16 LE | len u16 LE |
// payload[len] | CRC16 (CRC field se pehle ke tamam bytes par, CCITT).
// Header 9 B, kul 11 + len, max frame 1035 B. Na HMAC tag hai, na counter
// field. reqId per session monotonic u16 hai (wrap allowed); duplicate
// suppression sirf reqId par hai (D2). CRC fail / malformed frame ->
// dispatcher silent DROP karta hai (koi reply nahi — reqId par bharosa
// nahi kiya ja sakta).
typedef struct {
  uint8_t cmd;
  uint16_t req_id;
  uint16_t len;
  const uint8_t *payload;  // frame ke andar point karta hai — copy nahi
} rsh_v2_ctrl_t;

/// Control frame parse karo: structure + CRC16 verify (§3.1/§3.2).
static inline rsh_v2_result_t rsh_v2_ctrl_parse_structure(const uint8_t *data, size_t len,
                                                          rsh_v2_ctrl_t *out) {
  if (!data || !out) return RSH_V2_TOO_SHORT;
  // TOO_SHORT sirf tab jab 9 B ka header hi parha na ja sake (§3.1).
  // Header parh liya gaya ho to har length mismatch (truncation samet)
  // → BAD_LENGTH.
  if (len < (size_t)RSH_V2_CTRL_HEADER_SIZE)
    return RSH_V2_TOO_SHORT;
  if (data[0] != 'R' || data[1] != 'S' || data[2] != 'H' || data[3] != 'C')
    return RSH_V2_BAD_MAGIC;
  uint16_t payload_len = rsh_get_u16(data + 7);
  if (payload_len > RSH_V2_CTRL_MAX_PAYLOAD) return RSH_V2_BAD_LENGTH;
  if (len != (size_t)RSH_V2_CTRL_HEADER_SIZE + payload_len + 2u)
    return RSH_V2_BAD_LENGTH;
  if (rsh_crc16_ccitt(data, len - 2) != rsh_get_u16(data + len - 2))
    return RSH_V2_BAD_CRC;
  out->cmd = data[4];
  out->req_id = rsh_get_u16(data + 5);
  out->len = payload_len;
  out->payload = data + RSH_V2_CTRL_HEADER_SIZE;
  return RSH_V2_OK;
}

/// Control frame banao (CRC16 sameet). 0 return = payload > 1024 ya buf chhota.
static inline size_t rsh_v2_ctrl_build(uint8_t *buf, size_t buf_len, uint8_t cmd,
                                       uint16_t req_id,
                                       const uint8_t *payload, uint16_t payload_len) {
  if (!buf) return 0;
  if (payload_len > RSH_V2_CTRL_MAX_PAYLOAD) return 0;
  size_t total = (size_t)RSH_V2_CTRL_HEADER_SIZE + payload_len + 2u;
  if (buf_len < total) return 0;
  buf[0] = 'R'; buf[1] = 'S'; buf[2] = 'H'; buf[3] = 'C';
  buf[4] = cmd;
  rsh_put_u16(buf + 5, req_id);
  rsh_put_u16(buf + 7, payload_len);
  if (payload_len > 0) memcpy(buf + RSH_V2_CTRL_HEADER_SIZE, payload, payload_len);
  rsh_put_u16(buf + total - 2, rsh_crc16_ccitt(buf, total - 2));
  return total;
}

// ---- Control session: reqId-only duplicate cache (§2.3/§3.2, D2) ----
// Sirf do natije hain: naya reqId -> ACCEPT (execute karo); pehle dekha
// hua reqId -> DUPLICATE (cached reply, dobara execute nahi). Koi counter
// nahi, is liye koi REPLAY natija nahi (RSH_CTRL_REPLAY code reserved hai).
typedef enum {
  RSH_DEC_ACCEPT = 0,   // naya reqId — execute karo
  RSH_DEC_DUPLICATE     // wahi reqId dobara — cached reply, dobara execute nahi
} rsh_ctrl_decision_t;

typedef struct {
  uint16_t req_ids[32];
  uint8_t count;
  uint8_t next;
} rsh_ctrl_session_t;

static inline void rsh_ctrl_session_init(rsh_ctrl_session_t *s) {
  memset(s, 0, sizeof(*s));
}

static inline rsh_ctrl_decision_t rsh_ctrl_session_classify(rsh_ctrl_session_t *s,
                                                            uint16_t req_id) {
  for (int i = 0; i < s->count; i++) {
    if (s->req_ids[i] == req_id)
      return RSH_DEC_DUPLICATE;
  }
  s->req_ids[s->next] = req_id;
  s->next = (uint8_t)((s->next + 1) % 32);
  if (s->count < 32) s->count++;
  return RSH_DEC_ACCEPT;
}
