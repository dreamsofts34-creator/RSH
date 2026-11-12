#pragma once
// ApexLink packet protocol — 18 bytes, little-endian (docs: 02-architecture.md)
// Ye file Arduino se bilkul free hai taake host par unit test ho sake.
#include <stdint.h>
#include <stddef.h>
#include <string.h>

#define APEX_PACKET_SIZE   18
#define APEX_MAGIC         0xA5
#define APEX_VERSION       1

// buttons byte (offset 14)
#define APEX_BTN_GEAR_UP    0x01
#define APEX_BTN_GEAR_DOWN  0x02
#define APEX_BTN_ACCEL      0x04
#define APEX_BTN_BRAKE      0x08

// flags byte (offset 15)
#define APEX_FLAG_STEER_MV  0x01   // bit0: steering value millivolts mein hai (warna raw counts)

// CRC16-CCITT: poly 0x1021, init 0xFFFF (koi final XOR nahi)
static inline uint16_t apex_crc16_ccitt(const uint8_t *data, size_t len) {
  uint16_t crc = 0xFFFF;
  for (size_t i = 0; i < len; i++) {
    crc ^= (uint16_t)data[i] << 8;
    for (int b = 0; b < 8; b++) {
      crc = (crc & 0x8000) ? (uint16_t)((crc << 1) ^ 0x1021) : (uint16_t)(crc << 1);
    }
  }
  return crc;
}

static inline void apex_put_u16(uint8_t *p, uint16_t v) {
  p[0] = (uint8_t)(v & 0xFF);
  p[1] = (uint8_t)((v >> 8) & 0xFF);
}
static inline void apex_put_u32(uint8_t *p, uint32_t v) {
  p[0] = (uint8_t)(v & 0xFF);
  p[1] = (uint8_t)((v >> 8) & 0xFF);
  p[2] = (uint8_t)((v >> 16) & 0xFF);
  p[3] = (uint8_t)((v >> 24) & 0xFF);
}

// Packet banao. buf kam az kam APEX_PACKET_SIZE bytes ka ho. Hamesha 18 return.
static inline size_t apex_build_packet(uint8_t *buf, uint16_t seq, uint32_t timestamp_us,
                                       uint16_t steering, uint16_t accel_analog,
                                       uint16_t brake_analog, uint8_t buttons, uint8_t flags) {
  buf[0] = APEX_MAGIC;
  buf[1] = APEX_VERSION;
  apex_put_u16(buf + 2, seq);
  apex_put_u32(buf + 4, timestamp_us);
  apex_put_u16(buf + 8, steering);
  apex_put_u16(buf + 10, accel_analog);
  apex_put_u16(buf + 12, brake_analog);
  buf[14] = buttons;
  buf[15] = flags;
  uint16_t crc = apex_crc16_ccitt(buf, 16);
  apex_put_u16(buf + 16, crc);
  return APEX_PACKET_SIZE;
}
