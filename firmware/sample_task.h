#pragma once
// RSH sample mailbox — dedicated sampling task (F-06, v4.0.2) ka handoff.
//
// Design (WiFiUDP thread-safe NAHI hai — do tasks se ek socket par
// beginPacket/write/endPacket interleave ho sakta hai): sampling task
// sirf SAMPLE + PACKET BUILD karta hai; UDP/serial SEND loop task mein
// rehta hai. Mailbox single-slot hai (latest wins): loop task ke stall
// hone par purana frame overwrite hota hai aur `skipped` ginta hai
// (seqGap mein jata hai — burst catch-up kabhi nahi hota, purana
// behaviour barqarar). Sampling task aur loop task ke darmiyan sirf ye
// struct share hota hai; ESP32 par aligned u8/u16/u32 read/write atomic
// hain, is liye koi lock nahi chahiye (single producer, single consumer).
// Ye file Arduino se free hai; host tests produce/consume logic cover
// karte hain.
#include <stdint.h>
#include <stddef.h>
#include <string.h>

#define RSH_SAMPLE_MAX_V2_LEN 36  // max v2 stream packet (5 axes + 2 enc)
#define RSH_SAMPLE_MAX_V1_LEN 18  // v1 packet

typedef struct {
  uint8_t v2pkt[RSH_SAMPLE_MAX_V2_LEN];
  uint8_t v2len;   // 0 = is cycle mein v2 packet nahi bana
  uint8_t v1pkt[RSH_SAMPLE_MAX_V1_LEN];
  uint8_t v1len;   // 0 = is cycle mein v1 packet nahi bana
  uint32_t seq;    // is frame ka stream seq
  uint16_t skipped;  // produce ke waqt kitne unread frames overwrite hue
  uint8_t ready;   // 1 = frame maujood hai (loop ne abhi drain nahi kiya)
  uint8_t emit_wifi;    // RSH_EMIT_* (0 none, 1 v1, 2 v2) — Wi-Fi peer
  uint8_t emit_serial;  // RSH_EMIT_* — serial peer
} RshSampleMailbox;

static inline void rsh_sample_mailbox_init(RshSampleMailbox *mb) {
  if (mb) memset(mb, 0, sizeof(*mb));
}

// Sampling task se frame do. Agar pichla frame abhi drain nahi hua to
// wo overwrite hota hai aur skipped +1 (seqGap accounting).
static inline void rsh_sample_mailbox_produce(RshSampleMailbox *mb,
                                              const uint8_t *v2pkt, uint8_t v2len,
                                              const uint8_t *v1pkt, uint8_t v1len,
                                              uint32_t seq,
                                              uint8_t emit_wifi,
                                              uint8_t emit_serial) {
  if (!mb) return;
  if (mb->ready) mb->skipped++;
  if (v2pkt && v2len > 0 && v2len <= RSH_SAMPLE_MAX_V2_LEN) {
    memcpy(mb->v2pkt, v2pkt, v2len);
    mb->v2len = v2len;
  } else {
    mb->v2len = 0;
  }
  if (v1pkt && v1len > 0 && v1len <= RSH_SAMPLE_MAX_V1_LEN) {
    memcpy(mb->v1pkt, v1pkt, v1len);
    mb->v1len = v1len;
  } else {
    mb->v1len = 0;
  }
  mb->seq = seq;
  mb->emit_wifi = emit_wifi;
  mb->emit_serial = emit_serial;
  mb->ready = 1;
}

// Loop task se frame lo. Return 1 = frame mili (out bhari hui, skipped
// us frame ke saath overwrite hue frames ki tadaad), 0 = khaali.
static inline uint8_t rsh_sample_mailbox_consume(RshSampleMailbox *mb,
                                                 RshSampleMailbox *out) {
  if (!mb || !out || !mb->ready) return 0;
  *out = *mb;  // struct copy (36+18 B — loop task mein theek hai)
  mb->ready = 0;
  mb->skipped = 0;
  mb->v2len = 0;
  mb->v1len = 0;
  return 1;
}
