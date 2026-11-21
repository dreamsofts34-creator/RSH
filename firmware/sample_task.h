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
// hain — lekin poora struct aik copy mein atomic NAHI hai, is liye
// torn-frame race thi (R3-FW-05): loop task ke copy karte waqt agla
// produce() overwrite kar sakta tha (naye frame ki v2len, purane frame
// ke v2pkt bytes). Is ka hal generation counter hai (seqlock jaisa,
// lock nahi): produce() apni writes ko gen++ ... gen++ se bracket
// karta hai (odd = beech mein likh raha hai); consume() copy ke baad
// gen dobara check karta hai — mismatch ya odd gen ho to snapshot
// torn hai, phenk do (frame drop hoti hai, ghalat interpret nahi hoti;
// CRC bhi wahi karta — lekin ab loss `skipped` accounting mein aata
// hai, CRC-drop mein nahi). Claim bhi single-byte hai (sirf `ready`
// consume likhta hai — v2len/v1len/skipped sirf produce likhta hai),
// taake claim ke doran aaya naya frame clobber na ho: claim ke baad
// gen badla ho to `ready` wapas 1 kar do (naye frame ke fields
// mukammal hain — produce `ready` aakhir mein likhta hai).
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
  uint32_t gen;    // R3-FW-05: generation counter — produce() isay apni
                   // writes ke gird ++ karta hai (odd = mid-write);
                   // consume() copy ke baad isay dobara check karta hai
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
  // R3-FW-05: writes ko gen++ ... gen++ se bracket karo (odd =
  // mid-write). `skipped` yahin maintain hota hai (consume isay
  // kabhi nahi likhta — sirf ready consume likhta hai): unread frame
  // overwrite ho to +1, warna 0 (pichla frame drain ho chuka hai).
  mb->gen++;
  mb->skipped = mb->ready ? (uint16_t)(mb->skipped + 1) : 0;
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
  mb->gen++;  // even = frame mukammal; is ke BAAD ready=1
  mb->ready = 1;
}

// Loop task se frame lo. Return 1 = frame mili (out bhari hui, skipped
// us frame ke saath overwrite hue frames ki tadaad), 0 = khaali ya
// torn snapshot (R3-FW-05: copy ke doran gen badla ya odd raha —
// snapshot phenk do, mailbox ko haath mat lagao; producer ka mukammal
// frame agle consume par milega). Claim sirf `ready` likhta hai:
// claim ke baad gen badla ho to naya frame beech mein aa gaya —
// `ready` wapas 1 kar do (us ke fields mukammal hain).
static inline uint8_t rsh_sample_mailbox_consume(RshSampleMailbox *mb,
                                                 RshSampleMailbox *out) {
  if (!mb || !out || !mb->ready) return 0;
  *out = *mb;  // struct copy (36+18 B — loop task mein theek hai)
  if (mb->gen != out->gen || (out->gen & 1u)) return 0;  // torn — drop
  mb->ready = 0;
  if (mb->gen != out->gen) mb->ready = 1;  // naya frame beech mein aaya
  return 1;
}
