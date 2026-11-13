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

// flags byte (offset 15)
#define RSH_FLAG_STEER_MV  0x01   // bit0: steering value millivolts mein hai (warna raw counts)

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
// Protocol v2.1 (WP3, Phase A) — docs/PROTOCOL-V2.1.md authority hai.
// v1 code (upar) barqarar hai; legacy mode 0xA5 0x01 se pehchana jata hai.
// Sab integers little-endian. CRC16 hamesha aakhir mein, CRC field se pehle
// ke tamam bytes par. Product naam RSH hai.
// ============================================================================

#define RSH_V2_VERSION            0x21   // wire version byte (§0)
#define RSH_V2_MAX_AXES           8
#define RSH_V2_MAX_ENC            4
#define RSH_V2_MAX_BUTTONS_WIRE   64     // wire: stream buttons + status btnsEx (§1.1 N1)
#define RSH_V2_MAX_BUTTONS_CONFIG 128    // config ceiling (§0)
#define RSH_V2_STREAM_HEADER_SIZE 20
#define RSH_V2_STATUS_BASE_SIZE   34
#define RSH_V2_STATUS_EXT_SIZE    38
#define RSH_V2_HELLO_SIZE         92
#define RSH_V2_CTRL_HEADER_SIZE   13
#define RSH_V2_CTRL_TAG_SIZE      16
#define RSH_V2_CTRL_MAX_PAYLOAD   1024
#define RSH_V2_CTRL_MAX_FRAME     (RSH_V2_CTRL_HEADER_SIZE + RSH_V2_CTRL_MAX_PAYLOAD + RSH_V2_CTRL_TAG_SIZE) // 1053

// Stream flags (§1.3, D4)
#define RSH_V2_FLAG_CONFIG_CHANGED 0x01
#define RSH_V2_FLAG_LOW_SUPPLY     0x02
#define RSH_V2_FLAG_TICK_SKIPPED   0x04
#define RSH_V2_FLAG_AXIS_INVALID   0x08
// HELLO flags (§2.2)
#define RSH_V2_HELLO_UNPAIRED      0x01
#define RSH_V2_HELLO_KEY_RESET     0x02
#define RSH_V2_HELLO_SAFE_MODE     0x04
#define RSH_V2_HELLO_WINDOW_OPEN   0x08

// Control error codes (§3.9)
#define RSH_CTRL_OK           0
#define RSH_CTRL_BAD_TAG      1
#define RSH_CTRL_REPLAY       2
#define RSH_CTRL_BAD_CRC      3
#define RSH_CTRL_TOO_BIG      4
#define RSH_CTRL_PIN_CONFLICT 5
#define RSH_CTRL_UNSUPPORTED  6
#define RSH_CTRL_BUSY         7

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
#define RSH_CMD_KEY_RESET    0x0B
#define RSH_CMD_SET_PASSWORD 0x0C

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
  RSH_V2_BAD_TAG,      // HMAC tag verify fail
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

// ---- Stream v2.1 (§1.1) ----
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

/// Stream v2.1 packet banao (parser ka aaina). axes: mask-order values.
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

// ---- Status v2.1 (§1.2) ----
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

// ---- HELLO reply (§2.2, 92 B fixed) ----
typedef struct {
  uint8_t flags;
  uint8_t session_echo;
  uint8_t board_mac[6];
  uint8_t fw_major, fw_minor, fw_patch;
  uint8_t nonce[16];
  uint8_t device_key[32];   // unpaired + window ke ilawa sab 0x00
  uint8_t wifi_password[24];
} rsh_v2_hello_t;

static inline rsh_v2_result_t rsh_v2_parse_hello(const uint8_t *data, size_t len,
                                                 rsh_v2_hello_t *out) {
  if (!data || !out) return RSH_V2_TOO_SHORT;
  if (len < 6) return RSH_V2_TOO_SHORT;
  if (data[0] != 'R' || data[1] != 'S' || data[2] != 'H' || data[3] != 'R')
    return RSH_V2_BAD_MAGIC;
  if (data[4] != 2 || data[5] != 1) return RSH_V2_BAD_VERSION;
  if (len < RSH_V2_HELLO_SIZE) return RSH_V2_TOO_SHORT;
  if (len != RSH_V2_HELLO_SIZE) return RSH_V2_BAD_LENGTH;
  if (data[17] != 0) return RSH_V2_RSV_NONZERO;
  if (data[6] & 0xF0) return RSH_V2_RSV_NONZERO;
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

// ---- Control frames (§3.1/§3.2) ----
//
// HMAC-SHA256 yahan implement NAHI hai: firmware par mbedTLS maujood hai aur
// host tests apni independent SHA256 dete hain. Caller ek callback deta hai jo
// tag = HMAC-SHA256(key32, nonce16 || counter_LE || cmd || payload)[0:16]
// compute karta hai. Key kabhi yahan hard-code nahi hoti — hamesha parameter.
typedef void (*rsh_hmac16_fn)(const uint8_t *key32, const uint8_t *nonce16,
                              uint32_t counter, uint8_t cmd,
                              const uint8_t *payload, uint16_t payload_len,
                              uint8_t tag_out[16]);

typedef struct {
  uint8_t cmd;
  uint16_t req_id;
  uint32_t counter;
  uint16_t len;
  const uint8_t *payload;  // frame ke andar point karta hai — copy nahi
  uint8_t tag[16];
} rsh_v2_ctrl_t;

/// Sirf structure parse karo (tag verify NAHI) — BAD_TAG reply ka reqId/counter
/// parhne ke liye lazmi hai (§3.2: PC BAD_TAG reply ko sirf reqId/counter match
/// par maanta hai).
static inline rsh_v2_result_t rsh_v2_ctrl_parse_structure(const uint8_t *data, size_t len,
                                                          rsh_v2_ctrl_t *out) {
  if (!data || !out) return RSH_V2_TOO_SHORT;
  // TOO_SHORT sirf tab jab 13 B ka header hi parha na ja sake (§3.1).
  // Header parh liya gaya ho to har length mismatch (truncation samet)
  // → BAD_LENGTH (RSH_V2_BAD_LENGTH ki doc comment dekho).
  if (len < (size_t)RSH_V2_CTRL_HEADER_SIZE)
    return RSH_V2_TOO_SHORT;
  if (data[0] != 'R' || data[1] != 'S' || data[2] != 'H' || data[3] != 'C')
    return RSH_V2_BAD_MAGIC;
  uint16_t payload_len = rsh_get_u16(data + 11);
  if (payload_len > RSH_V2_CTRL_MAX_PAYLOAD) return RSH_V2_BAD_LENGTH;
  if (len != (size_t)RSH_V2_CTRL_HEADER_SIZE + payload_len + RSH_V2_CTRL_TAG_SIZE)
    return RSH_V2_BAD_LENGTH;
  out->cmd = data[4];
  out->req_id = rsh_get_u16(data + 5);
  out->counter = rsh_get_u32(data + 7);
  out->len = payload_len;
  out->payload = data + RSH_V2_CTRL_HEADER_SIZE;
  memcpy(out->tag, data + RSH_V2_CTRL_HEADER_SIZE + payload_len, RSH_V2_CTRL_TAG_SIZE);
  return RSH_V2_OK;
}

/// Structure + HMAC tag verify (§3.2). Tag fail → RSH_V2_BAD_TAG (BAD_TAG = 1).
static inline rsh_v2_result_t rsh_v2_ctrl_verify(const uint8_t *data, size_t len,
                                                 const uint8_t *key32,
                                                 const uint8_t *nonce16,
                                                 rsh_hmac16_fn hmac,
                                                 rsh_v2_ctrl_t *out) {
  rsh_v2_ctrl_t tmp;
  rsh_v2_result_t r = rsh_v2_ctrl_parse_structure(data, len, &tmp);
  if (r != RSH_V2_OK) return r;
  if (!key32 || !nonce16 || !hmac) return RSH_V2_BAD_TAG;
  uint8_t expected[16];
  hmac(key32, nonce16, tmp.counter, tmp.cmd, tmp.payload, tmp.len, expected);
  uint8_t diff = 0;  // constant-time compare
  for (int i = 0; i < 16; i++) diff |= (uint8_t)(expected[i] ^ tmp.tag[i]);
  if (diff != 0) return RSH_V2_BAD_TAG;
  if (out) *out = tmp;
  return RSH_V2_OK;
}

/// Control frame banao (tag sameet). 0 return = payload > 1024, buf chhota, ya hmac NULL.
static inline size_t rsh_v2_ctrl_build(uint8_t *buf, size_t buf_len, uint8_t cmd,
                                       uint16_t req_id, uint32_t counter,
                                       const uint8_t *payload, uint16_t payload_len,
                                       const uint8_t *key32, const uint8_t *nonce16,
                                       rsh_hmac16_fn hmac) {
  if (!buf || !hmac || !key32 || !nonce16) return 0;
  if (payload_len > RSH_V2_CTRL_MAX_PAYLOAD) return 0;
  size_t total = (size_t)RSH_V2_CTRL_HEADER_SIZE + payload_len + RSH_V2_CTRL_TAG_SIZE;
  if (buf_len < total) return 0;
  buf[0] = 'R'; buf[1] = 'S'; buf[2] = 'H'; buf[3] = 'C';
  buf[4] = cmd;
  rsh_put_u16(buf + 5, req_id);
  rsh_put_u32(buf + 7, counter);
  rsh_put_u16(buf + 11, payload_len);
  if (payload_len > 0) memcpy(buf + RSH_V2_CTRL_HEADER_SIZE, payload, payload_len);
  hmac(key32, nonce16, counter, cmd, payload, payload_len,
       buf + RSH_V2_CTRL_HEADER_SIZE + payload_len);
  return total;
}

// ---- Control session: duplicate / replay (§2.3/§3.2) ----
typedef enum {
  RSH_DEC_ACCEPT = 0,   // naya pair, counter > max-seen — execute karo
  RSH_DEC_DUPLICATE,    // wahi (reqId, counter) dobara — cached reply, dobara execute nahi
  RSH_DEC_REPLAY        // naya reqId lekin counter <= max-seen — status REPLAY (2)
} rsh_ctrl_decision_t;

typedef struct {
  uint32_t max_seen;
  uint8_t has_seen;
  uint16_t req_ids[32];
  uint32_t counters[32];
  uint8_t count;
  uint8_t next;
} rsh_ctrl_session_t;

static inline void rsh_ctrl_session_init(rsh_ctrl_session_t *s) {
  memset(s, 0, sizeof(*s));
}

static inline rsh_ctrl_decision_t rsh_ctrl_session_classify(rsh_ctrl_session_t *s,
                                                            uint16_t req_id,
                                                            uint32_t counter) {
  for (int i = 0; i < s->count; i++) {
    if (s->req_ids[i] == req_id && s->counters[i] == counter)
      return RSH_DEC_DUPLICATE;
  }
  if (s->has_seen && counter <= s->max_seen) return RSH_DEC_REPLAY;
  s->max_seen = counter;
  s->has_seen = 1;
  s->req_ids[s->next] = req_id;
  s->counters[s->next] = counter;
  s->next = (uint8_t)((s->next + 1) % 32);
  if (s->count < 32) s->count++;
  return RSH_DEC_ACCEPT;
}
