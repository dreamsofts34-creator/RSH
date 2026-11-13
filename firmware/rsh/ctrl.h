#pragma once
// RSH control-channel dispatcher (FW-8, SEC-D-06/SEC-D-07) — PURE
// (Arduino-free) taake host par unit test ho sake.
//
// Wire flow (§3.2): frame → structure parse → HMAC verify → session
// classify → handler (sirf ACCEPT par) → signed reply.
//
// - Malformed (bad magic / too short / bad length / truncated) → SILENT
//   DROP, koi reply nahi (reqId par bharosa nahi kiya ja sakta).
// - Bad tag / wrong key → BAD_TAG (1) reply, handler KABHI nahi chalta.
// - Replay (naya reqId, purana counter) → REPLAY (2) reply, no execute.
// - Duplicate (wahi reqId+counter) → CACHED reply dobara, no execute.
//   Cache mein reply na ho (reply RSH_CTRL_CACHED_REPLY_MAX se bara tha)
//   to idempotent dobara-execute hota hai — §3.2 kehta hai tamam commands
//   idempotent honi chahiye, is liye ye fallback mehfooz hai.
// - Unknown cmd → handler UNSUPPORTED (6) deta hai, execute nahi hota.
// - Reply tag hamesha board key/nonce se banta hai (rsh_v2_ctrl_build).
//
// Handler contract: status return karo; reply_body mein status KE BAGHAIR
// body bharo (dispatcher status khud prepend karta hai); *reply_body_len
// set karo. Body > 1023 ho to dispatcher UNSUPPORTED bhejta hai.
#include <stdint.h>
#include <stddef.h>
#include <string.h>
#include "protocol.h"

#define RSH_CTRL_CACHED_REPLY_MAX 128  // is se bara reply cache nahi hota
#define RSH_CTRL_CACHE_SLOTS 32        // session ring jitna

typedef uint8_t (*rsh_ctrl_handler_fn)(uint8_t cmd, const uint8_t *payload, uint16_t len,
                                       uint8_t *reply_body, uint16_t *reply_body_len,
                                       void *ctx);

typedef struct {
  uint16_t req_id;
  uint32_t counter;
  uint16_t len;  // cached reply frame ki length (bytes[])
  uint8_t bytes[RSH_CTRL_CACHED_REPLY_MAX];
  uint8_t has_pair;   // slot mein (reqId, counter) maujood hai
  uint8_t has_reply;  // bytes[] mein poora reply frame maujood hai
} rsh_ctrl_cache_entry_t;

typedef struct {
  rsh_ctrl_cache_entry_t slots[RSH_CTRL_CACHE_SLOTS];
  uint8_t next;  // round-robin overwrite index
} rsh_ctrl_cache_t;

static inline void rsh_ctrl_cache_init(rsh_ctrl_cache_t *c) {
  if (!c) return;
  memset(c, 0, sizeof(*c));
}

static inline const rsh_ctrl_cache_entry_t *rsh_ctrl_cache_find(rsh_ctrl_cache_t *c,
                                                                uint16_t req_id,
                                                                uint32_t counter) {
  if (!c) return NULL;
  for (int i = 0; i < RSH_CTRL_CACHE_SLOTS; i++) {
    if (c->slots[i].has_pair && c->slots[i].req_id == req_id &&
        c->slots[i].counter == counter)
      return &c->slots[i];
  }
  return NULL;
}

static inline void rsh_ctrl_cache_store(rsh_ctrl_cache_t *c, uint16_t req_id, uint32_t counter,
                                        const uint8_t *reply, uint16_t reply_len) {
  if (!c || !reply) return;
  // Pehle wahi pair dhoondo (update), warna round-robin slot lo.
  rsh_ctrl_cache_entry_t *slot = NULL;
  for (int i = 0; i < RSH_CTRL_CACHE_SLOTS; i++) {
    if (c->slots[i].has_pair && c->slots[i].req_id == req_id &&
        c->slots[i].counter == counter) { slot = &c->slots[i]; break; }
  }
  if (!slot) {
    slot = &c->slots[c->next];
    c->next = (uint8_t)((c->next + 1) % RSH_CTRL_CACHE_SLOTS);
  }
  memset(slot, 0, sizeof(*slot));
  slot->req_id = req_id;
  slot->counter = counter;
  slot->has_pair = 1;
  if (reply_len <= RSH_CTRL_CACHED_REPLY_MAX) {
    memcpy(slot->bytes, reply, reply_len);
    slot->len = reply_len;
    slot->has_reply = 1;
  }
  // reply bara ho to has_pair=1, has_reply=0: duplicate aaye to dispatcher
  // dobara execute karega (idempotent fallback — upar dekho).
}

typedef enum {
  RSH_CTRL_DISP_DROP = 0,  // koi reply nahi (malformed — silent drop)
  RSH_CTRL_DISP_REPLY      // reply_out mein reply frame hai
} rsh_ctrl_disp_t;

// Ek control frame dispatch karo. reply_out kam az kam
// RSH_V2_CTRL_MAX_FRAME (1053) bytes ka ho. *reply_len_out = reply bytes
// (sirf REPLY par). Return DROP = kuch mat bhejo.
static inline rsh_ctrl_disp_t rsh_ctrl_dispatch(
    const uint8_t *frame, size_t frame_len,
    const uint8_t *key32, const uint8_t *nonce16, rsh_hmac16_fn hmac,
    rsh_ctrl_session_t *session, rsh_ctrl_cache_t *cache,
    rsh_ctrl_handler_fn handler, void *ctx,
    uint8_t *reply_out, size_t reply_out_len, size_t *reply_len_out) {
  if (reply_len_out) *reply_len_out = 0;
  if (!frame || !key32 || !nonce16 || !hmac || !session || !cache || !handler ||
      !reply_out || reply_out_len < (size_t)RSH_V2_CTRL_HEADER_SIZE + RSH_V2_CTRL_TAG_SIZE)
    return RSH_CTRL_DISP_DROP;

  rsh_v2_ctrl_t f;
  // Malformed → silent drop (§3.2).
  if (rsh_v2_ctrl_parse_structure(frame, frame_len, &f) != RSH_V2_OK)
    return RSH_CTRL_DISP_DROP;

  uint8_t reply_cmd = (uint8_t)(f.cmd | 0x80);
  uint8_t status_payload[1];

  // Tag fail → BAD_TAG reply (reqId/counter echo; tag board key se).
  // Handler NAHI chalta — ghalat frame kabhi execute nahi hota.
  if (rsh_v2_ctrl_verify(frame, frame_len, key32, nonce16, hmac, NULL) != RSH_V2_OK) {
    status_payload[0] = RSH_CTRL_BAD_TAG;
    size_t n = rsh_v2_ctrl_build(reply_out, reply_out_len, reply_cmd, f.req_id, f.counter,
                                 status_payload, 1, key32, nonce16, hmac);
    if (n == 0) return RSH_CTRL_DISP_DROP;
    *reply_len_out = n;
    return RSH_CTRL_DISP_REPLY;
  }

  rsh_ctrl_decision_t dec = rsh_ctrl_session_classify(session, f.req_id, f.counter);
  if (dec == RSH_DEC_DUPLICATE) {
    const rsh_ctrl_cache_entry_t *hit = rsh_ctrl_cache_find(cache, f.req_id, f.counter);
    if (hit && hit->has_reply) {
      if ((size_t)hit->len > reply_out_len) return RSH_CTRL_DISP_DROP;
      memcpy(reply_out, hit->bytes, hit->len);
      *reply_len_out = hit->len;
      return RSH_CTRL_DISP_REPLY;
    }
    // Cached reply nahi (reply bohat bara tha) → neeche dobara execute
    // (idempotent fallback).
  } else if (dec == RSH_DEC_REPLAY) {
    status_payload[0] = RSH_CTRL_REPLAY;
    size_t n = rsh_v2_ctrl_build(reply_out, reply_out_len, reply_cmd, f.req_id, f.counter,
                                 status_payload, 1, key32, nonce16, hmac);
    if (n == 0) return RSH_CTRL_DISP_DROP;
    *reply_len_out = n;
    return RSH_CTRL_DISP_REPLY;
  }

  // ACCEPT (ya DUPLICATE-baghair-cache): handler chalao — yehi waahid
  // jagah hai jahan koi command execute hoti hai.
  static uint8_t body_buf[RSH_V2_CTRL_MAX_PAYLOAD];  // static: no heap/stack blow
  static uint8_t payload_buf[RSH_V2_CTRL_MAX_PAYLOAD + 1];
  uint16_t body_len = 0;
  uint8_t status = handler(f.cmd, f.payload, f.len, body_buf, &body_len, ctx);
  if (body_len > RSH_V2_CTRL_MAX_PAYLOAD - 1) {
    // Handler ne had se zyada body di — execute ho chuka, lekin reply
    // nahi ban sakta: UNSUPPORTED bhejo (ye firmware bug hoga, test mein
    // pakra jana chahiye — production handler aisa kabhi na kare).
    status = RSH_CTRL_UNSUPPORTED;
    body_len = 0;
  }
  payload_buf[0] = status;
  if (body_len > 0) memcpy(payload_buf + 1, body_buf, body_len);
  size_t n = rsh_v2_ctrl_build(reply_out, reply_out_len, reply_cmd, f.req_id, f.counter,
                               payload_buf, (uint16_t)(1 + body_len), key32, nonce16, hmac);
  if (n == 0) return RSH_CTRL_DISP_DROP;
  rsh_ctrl_cache_store(cache, f.req_id, f.counter, reply_out, (uint16_t)n);
  *reply_len_out = n;
  return RSH_CTRL_DISP_REPLY;
}
