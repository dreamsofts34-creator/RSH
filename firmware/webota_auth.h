#pragma once
// RSH WebOTA auth rate limit — PER-IP (F-08, v4.0.2).
//
// Purana model: ek global counter (5 fails → 60 s lockout SAB ke liye).
// AP par koi bhi station 5 ghalat password bhej kar legit upload rok
// sakta tha. Naya model: har client IP ka apna counter + lockout hai
// (AP par max 4 stations, table bhi 4 ki hai). Ek IP ka lockout doosre
// IP ko nahi rokta. Ye file Arduino se free hai (webota.cpp WebServer
// se IP lekar yahin poochta hai); host tests yahin se cover karte hain.
#include <stdint.h>
#include <string.h>

#define RSH_WEBOTA_AUTH_MAX_IP    4   // SoftAP max stations (net.cpp)
#define RSH_WEBOTA_AUTH_MAX_FAILS 5   // itni fails → lockout
#define RSH_WEBOTA_AUTH_LOCK_MS   60000u  // lockout muddat

typedef struct {
  uint32_t ip;            // client IPv4 (u32 pack)
  uint8_t fails;          // lagatar fails (is lockout window mein)
  uint32_t lock_until_ms; // 0 = koi lockout nahi
  uint32_t last_ms;       // aakhri attempt (LRU eviction ke liye)
  uint8_t used;
} RshWebotaAuthEntry;

typedef struct {
  RshWebotaAuthEntry entries[RSH_WEBOTA_AUTH_MAX_IP];
} RshWebotaAuth;

static inline void rsh_webota_auth_init(RshWebotaAuth *a) {
  if (a) memset(a, 0, sizeof(*a));
}

// Is IP ke liye lockout khatam hua? (Expired lock lazily saaf hota hai.)
static inline uint8_t rsh_webota_auth_locked_(RshWebotaAuthEntry *e,
                                              uint32_t now_ms) {
  if (!e || !e->used || e->lock_until_ms == 0) return 0;
  if ((int32_t)(now_ms - e->lock_until_ms) >= 0) {
    e->lock_until_ms = 0;
    e->fails = 0;
    return 0;
  }
  return 1;
}

static inline RshWebotaAuthEntry *rsh_webota_auth_find_(RshWebotaAuth *a,
                                                        uint32_t ip) {
  if (!a) return NULL;
  for (int i = 0; i < RSH_WEBOTA_AUTH_MAX_IP; i++) {
    if (a->entries[i].used && a->entries[i].ip == ip) return &a->entries[i];
  }
  return NULL;
}

// Is IP ko abhi auth try karne ki ijazat hai? 1 = haan, 0 = locked.
static inline uint8_t rsh_webota_auth_allowed(RshWebotaAuth *a, uint32_t ip,
                                              uint32_t now_ms) {
  if (!a) return 0;
  RshWebotaAuthEntry *e = rsh_webota_auth_find_(a, ip);
  if (!e) return 1;  // naya IP — kabhi lock nahi hua
  return rsh_webota_auth_locked_(e, now_ms) ? 0 : 1;
}

// Ek auth attempt ka natija darj karo. ok=1 → is IP ke fails reset;
// ok=0 → fails++; 5 par lockout (sirf is IP ke liye).
static inline void rsh_webota_auth_note(RshWebotaAuth *a, uint32_t ip,
                                        uint32_t now_ms, uint8_t ok) {
  if (!a) return;
  RshWebotaAuthEntry *e = rsh_webota_auth_find_(a, ip);
  if (!e) {
    // Naya IP: khaali slot lo, warna sab se purana (LRU) nikalo.
    int slot = -1;
    for (int i = 0; i < RSH_WEBOTA_AUTH_MAX_IP; i++) {
      if (!a->entries[i].used) { slot = i; break; }
    }
    if (slot < 0) {
      slot = 0;
      for (int i = 1; i < RSH_WEBOTA_AUTH_MAX_IP; i++) {
        if ((int32_t)(a->entries[i].last_ms - a->entries[slot].last_ms) < 0)
          slot = i;
      }
    }
    e = &a->entries[slot];
    memset(e, 0, sizeof(*e));
    e->ip = ip;
    e->used = 1;
  }
  e->last_ms = now_ms;
  if (ok) {
    e->fails = 0;
    e->lock_until_ms = 0;
    return;
  }
  if (rsh_webota_auth_locked_(e, now_ms)) return;  // locked rehte hue fails na gino
  e->fails++;
  if (e->fails >= RSH_WEBOTA_AUTH_MAX_FAILS) {
    e->lock_until_ms = now_ms + RSH_WEBOTA_AUTH_LOCK_MS;
    e->fails = 0;
  }
}
