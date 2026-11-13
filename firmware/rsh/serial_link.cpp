// RSH wired USB-serial link — Arduino glue (Phase G, D26 / OPEN-20).
// serial_link.h dekho.
#include "serial_link.h"
#include "serial_transport.h"
#include "config.h"
#include "net.h"
#include "protocol.h"
#include <Arduino.h>

#if RSH_WIRED_SERIAL
static rsh_serial_decoder_t decoder;
static uint32_t droppedInbound = 0;  // board ko na chahiye wale inbound frames (STREAM/STATUS/HELLO/PROBE PC se)
#endif

void serial_link_begin() {
#if RSH_WIRED_SERIAL
  Serial.begin(RSH_SERIAL_BAUD);
  rsh_serial_decoder_init(&decoder);
  droppedInbound = 0;
  net_serial_begin();
#endif
}

bool serial_link_send_frame(uint8_t kind, const uint8_t *payload, uint16_t payload_len) {
#if RSH_WIRED_SERIAL
  static uint8_t frame[RSH_SERIAL_MAX_FRAME];  // static: no heap, no stack blow
  size_t n = rsh_serial_build_frame(frame, sizeof(frame), kind, payload, payload_len);
  if (n == 0) return false;
  return Serial.write(frame, n) == n;
#else
  (void)kind; (void)payload; (void)payload_len;
  return false;
#endif
}

uint32_t serial_link_frames_ok() {
#if RSH_WIRED_SERIAL
  return decoder.frames_ok;
#else
  return 0;
#endif
}

uint32_t serial_link_bad_frames() {
#if RSH_WIRED_SERIAL
  return decoder.bad_magic + decoder.bad_crc + decoder.bad_length;
#else
  return 0;
#endif
}

uint32_t serial_link_dropped_inbound() {
#if RSH_WIRED_SERIAL
  return droppedInbound;
#else
  return 0;
#endif
}

#if RSH_WIRED_SERIAL
// Ek complete frame ka amal: kind ke mutabiq net_serial_* par bhejo,
// reply ho to wapas Serial par frame mein lapet kar bhejo.
static void serial_link_handle_frame(uint8_t kind, const uint8_t *payload,
                                     uint16_t len, uint32_t nowMs) {
  switch (kind) {
    case RSH_SERIAL_KIND_HELO: {
      // HELO → HELLO reply (92 B, §2.2). Legacy v1 HELO par 0 milta hai
      // (koi reply nahi — sirf active mark hota hai).
      static uint8_t hello[RSH_V2_HELLO_SIZE];
      size_t n = net_serial_helo(payload, len, nowMs, hello);
      if (n > 0) serial_link_send_frame(RSH_SERIAL_KIND_HELLO, hello, (uint16_t)n);
      break;
    }
    case RSH_SERIAL_KIND_CONTROL: {
      // HMAC control — reply ho to pehle bhejo, PHIR deferred actions
      // (REBOOT/KEY_RESET/SET_PASSWORD — SEC-D-01 pattern).
      static uint8_t reply[RSH_V2_CTRL_MAX_FRAME];
      size_t replyLen = 0;
      if (net_serial_dispatch_control(payload, len, reply, sizeof(reply), &replyLen))
        serial_link_send_frame(RSH_SERIAL_KIND_CONTROL, reply, (uint16_t)replyLen);
      net_serial_after_reply();
      break;
    }
    case RSH_SERIAL_KIND_PING: {
      // Ping reply (34 B, §9) — drop ho to khamoshi (koi error reply nahi).
      static uint8_t reply[RSH_V2_PING_SIZE];
      size_t n = net_serial_ping(payload, (int)len, reply);
      if (n > 0) serial_link_send_frame(RSH_SERIAL_KIND_PING, reply, (uint16_t)n);
      break;
    }
    default:
      // STREAM/STATUS/HELLO/PROBE PC se board ko nahi aane chahiye —
      // drop + count (engine ko chhoo nahi — decoder ne CRC pehle hi check kiya).
      droppedInbound++;
      break;
  }
}
#endif

void serial_link_poll(uint32_t nowMs) {
#if RSH_WIRED_SERIAL
  // Per call cap: datagram flood ki tarah serial flood bhi loop ko
  // starve na kare (net_poll ka 8-datagram cap jaisa).
  for (int i = 0; i < 512; i++) {
    int b = Serial.read();
    if (b < 0) break;  // kuch nahi aaya
    if (rsh_serial_decoder_feed(&decoder, (uint8_t)b)) {
      serial_link_handle_frame(decoder.frame_kind,
                               decoder.buf + RSH_SERIAL_HEADER_SIZE,
                               decoder.frame_len, nowMs);
    }
  }
#else
  (void)nowMs;
#endif
}
