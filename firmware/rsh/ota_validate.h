// RSH WebOTA validation (FW-9, F1/F9, SEC-C-05/SEC-C-06, SEC-D-02/SEC-D-04).
// Pure logic, host-testable — yahi functions firmware (webota.cpp) aur host
// tests (tests-host/test_main.cpp) dono istemal karte hain. Koi Arduino
// include nahi; HTTP/Update glue webota.cpp mein hai.
//
// Rules:
// - F1 (SEC-D-02): har endpoint par authenticate() — baghair auth ke koi
//   endpoint (root/update/not-found) kuch nahi karta. Rate limit/lockout
//   ka state webota.cpp mein hai (millis chahiye, is liye pure nahi).
// - F9: pehle chunk mein magic (0xE9) + size cap (app partition 0x140000).
//   Version image ke andar "Version <b>" marker se nikali jati hai
//   (streaming scanner — marker chunk boundary par bhi pakra jata hai).
// - Anti-downgrade (SEC-C-05 / PROTOCOL-V2.1 OPEN-18 rule): image version
//   running version se KAM ho to reject; BARABAR = reinstall (allowed),
//   nayi = allowed. (Pehle yahan barabar bhi reject tha — ye SEC-C-05,
//   KEY-LIFECYCLE §9 aur C# FirmwareImageValidator se mutabiqat nahi
//   rakhta tha; FW-9 Phase D mein theek kiya gaya.)
// - Signed OTA (SEC-C-06 / SEC-D-04, OPEN-17 = Ed25519): image ke aakhir
//   mein 68-byte trailer: [Ed25519 signature 64 B][magic "RSHS" 4 B].
//   Signature = Ed25519(secret, SHA-256(image bytes, trailer ke baghair))
//   — hash-then-sign, is liye verify ke liye poori image RAM mein nahi
//   chahiye, sirf 32-byte digest (docs/SIGNED-OTA.md — yahi authority hai).
//   Trailer image ke aakhir mein hota hai; bootloader use ignore karta hai
//   (image apne SHA digest par khatam hoti hai).
// - NOTE (SEC-D-04): version marker sirf version batata hai, authenticity
//   nahi — marker ko hamlaawar bhi likh sakta hai (misal "9.9.9").
//   Asliyat sirf Ed25519 signature se sabit hoti hai; marker check us
//   ka badal nahi hai.
#pragma once

#include <stdint.h>
#include <stddef.h>
#include <string.h>

struct RshVersion {
  uint8_t major = 0;
  uint8_t minor = 0;
  uint8_t patch = 0;
};

static inline int rsh_version_compare(const RshVersion &a, const RshVersion &b) {
  if (a.major != b.major) return a.major < b.major ? -1 : 1;
  if (a.minor != b.minor) return a.minor < b.minor ? -1 : 1;
  if (a.patch != b.patch) return a.patch < b.patch ? -1 : 1;
  return 0;
}

// "4.0.0" jaisi string parse karo (strict: exactly a.b.c, har hissa 1-3
// digits, value <= 255, poori length consume ho). true = ok.
static inline bool rsh_parse_version(const char *s, size_t n, RshVersion *out) {
  if (!s || !out || n == 0 || n > 11) return false;
  uint16_t parts[3] = {0, 0, 0};
  int part = 0;
  size_t digits = 0;
  for (size_t i = 0; i < n; i++) {
    char c = s[i];
    if (c >= '0' && c <= '9') {
      if (++digits > 3) return false;
      parts[part] = (uint16_t)(parts[part] * 10 + (c - '0'));
      if (parts[part] > 255) return false;
    } else if (c == '.') {
      if (digits == 0 || part >= 2) return false;
      part++;
      digits = 0;
    } else {
      return false;
    }
  }
  if (part != 2 || digits == 0) return false;
  out->major = (uint8_t)parts[0];
  out->minor = (uint8_t)parts[1];
  out->patch = (uint8_t)parts[2];
  return true;
}

static constexpr uint8_t kEspImageMagic = 0xE9;
// Default partition table (default.csv): app slot 0x140000 (1.25 MB).
static constexpr uint32_t kOtaMaxBytes = 0x140000;
static constexpr uint32_t kOtaMinBytes = 65536;  // chhoti se chhoti maqool image
// Signature trailer: 64-byte Ed25519 signature + 4-byte magic.
static constexpr size_t kOtaSigLen = 64;
static constexpr char kOtaSigMagic[5] = "RSHS";
static constexpr size_t kOtaTrailerSize = kOtaSigLen + 4;  // 68

// Har endpoint par auth lazmi (F1) — baghair auth ke request deny.
static inline bool ota_endpoint_allowed(bool authenticated) {
  return authenticated;
}

// Pehle chunk ki validation: magic + candidate version current se kam NAHI
// honi chahiye (anti-downgrade; barabar = reinstall, allowed). len < 1 par false.
static inline bool ota_first_chunk_ok(const uint8_t *chunk, size_t len,
                                      const RshVersion &current,
                                      const RshVersion &candidate) {
  if (!chunk || len < 1) return false;
  if (chunk[0] != kEspImageMagic) return false;
  return rsh_version_compare(candidate, current) >= 0;
}

// Kul upload size cap (F9) — cap se bara image shuru mein hi reject.
static inline bool ota_total_size_ok(size_t total_bytes) {
  return total_bytes > 0 && total_bytes <= (size_t)kOtaMaxBytes;
}

// ---- Version marker scanner (streaming) ----
// WebOTA page mein version chip hoti hai: "Version <b>" + FIRMWARE_VERSION.
// Firmware .rodata mein ye do alag string literals hain, is liye marker aur
// version ke darmiyan NUL byte hota hai: "Version <b>\0 4.0.0\0" (C#
// FirmwareImageValidator wahi tareeqa istemal karta hai).
// Scanner chunks mein feed hota hai; marker chunk boundary par bhi mile to
// pakra jata hai (aakhri 35 bytes hamesha window mein rehte hain — match ki
// max length marker 11 + skip 8 + version 16 = 35 hai).
static constexpr char kOtaVersionMarker[] = "Version <b>";
static constexpr size_t kOtaVersionMarkerLen = 11;

struct RshVersionScan {
  uint8_t win[40] = {};
  size_t win_len = 0;
  bool found = false;  // version mil gaya
  RshVersion version{};
};

static inline void rsh_version_scan_init(RshVersionScan *s) {
  if (!s) return;
  *s = RshVersionScan{};  // memset nahi: non-trivial type par -Werror=class-memaccess
}

static inline void rsh_version_scan_feed(RshVersionScan *s, const uint8_t *data, size_t len) {
  if (!s || s->found || !data || len == 0) return;
  for (size_t i = 0; i < len; i++) {
    if (s->win_len < sizeof(s->win)) {
      s->win[s->win_len++] = data[i];
    } else {
      memmove(s->win, s->win + 1, sizeof(s->win) - 1);
      s->win[sizeof(s->win) - 1] = data[i];
    }
  }
  if (s->win_len > 35) {
    size_t drop = s->win_len - 35;
    memmove(s->win, s->win + drop, 35);
    s->win_len = 35;
  }
  for (size_t i = 0; i + kOtaVersionMarkerLen <= s->win_len; i++) {
    if (memcmp(s->win + i, kOtaVersionMarker, kOtaVersionMarkerLen) != 0) continue;
    size_t j = i + kOtaVersionMarkerLen;
    size_t skip_end = j + 8 < s->win_len ? j + 8 : s->win_len;
    while (j < skip_end && (s->win[j] < '0' || s->win[j] > '9')) j++;
    if (j >= s->win_len) return;  // version abhi aaya hi nahi — agle chunk ka intezar
    size_t k = j;
    while (k < s->win_len && k < j + 16 &&
           ((s->win[k] >= '0' && s->win[k] <= '9') || s->win[k] == '.')) k++;
    if (k == s->win_len) return;  // run abhi khatam nahi hua — agle chunk ka intezar
    if (k == j) continue;         // marker ke baad version nahi — false positive
    RshVersion v{};
    if (rsh_parse_version((const char *)(s->win + j), k - j, &v)) {
      s->version = v;
      s->found = true;
      return;
    }
    // Parse fail — is marker ko chhoro, aage scan karo.
  }
}

// ---- Signature trailer ----

// tail = upload ke aakhri 68 bytes. true = trailer maujood (magic theek).
static inline bool ota_trailer_has_magic(const uint8_t *tail, size_t tail_len) {
  if (!tail || tail_len != kOtaTrailerSize) return false;
  return memcmp(tail + kOtaSigLen, kOtaSigMagic, 4) == 0;
}

// Trailer se 64-byte signature ka pointer (caller pehle has_magic check kare).
static inline const uint8_t *ota_trailer_sig(const uint8_t *tail) { return tail; }

// ---- Signed-region digest (streaming, tail holdback) ----
// Signature upload ke aakhri 68 bytes (trailer) ke ilawa tamam bytes ke
// SHA-256 par banti hai. Ye helper har feed par sirf woh bytes hash-sink ko
// deta hai jo yaqeeni tor par trailer ka hissa nahi hain (aakhri 68 bytes
// hamesha rok kar rakhta hai); final par tail bahir nikalta hai.
typedef void (*RshOtaHashUpdateFn)(void *ctx, const uint8_t *data, size_t len);

struct RshSignHash {
  uint8_t tail[kOtaTrailerSize] = {};
  size_t tail_len = 0;
  RshOtaHashUpdateFn update = nullptr;
  void *ctx = nullptr;
};

static inline void rsh_sign_hash_init(RshSignHash *h, RshOtaHashUpdateFn update, void *ctx) {
  if (!h) return;
  *h = RshSignHash{};  // memset nahi: non-trivial type par -Werror=class-memaccess
  h->update = update;
  h->ctx = ctx;
}

static inline void rsh_sign_hash_feed(RshSignHash *h, const uint8_t *data, size_t len) {
  if (!h || !data || len == 0) return;
  size_t total = h->tail_len + len;
  if (total <= kOtaTrailerSize) {
    memcpy(h->tail + h->tail_len, data, len);
    h->tail_len = total;
    return;
  }
  size_t excess = total - kOtaTrailerSize;  // ab hash hone wale sab se purane bytes
  size_t from_tail = excess < h->tail_len ? excess : h->tail_len;
  if (from_tail > 0) h->update(h->ctx, h->tail, from_tail);
  size_t from_data = excess - from_tail;
  if (from_data > 0) h->update(h->ctx, data, from_data);
  // Nayi tail = (purani tail ++ data) ke aakhri 68 bytes.
  size_t keep_from_tail = h->tail_len - from_tail;
  memmove(h->tail, h->tail + from_tail, keep_from_tail);
  size_t need_from_data = kOtaTrailerSize - keep_from_tail;
  memcpy(h->tail + keep_from_tail, data + (len - need_from_data), need_from_data);
  h->tail_len = kOtaTrailerSize;
}

static inline void rsh_sign_hash_final(const RshSignHash *h, uint8_t *tail_out, size_t *tail_len_out) {
  memcpy(tail_out, h->tail, h->tail_len);
  *tail_len_out = h->tail_len;
}

// ---- Ed25519 verify (libsodium) ----
// Verify sirf deterministic hai — RNG nahi chahiye, is liye sodium_init()
// ki zaroorat nahi. Firmware aur host dono wahi libsodium primitive istemal
// karte hain; host par headers nahi hain, is liye ABI declaration yahan hai.
#if defined(ARDUINO)
#include <sodium.h>
#else
extern "C" {
extern int crypto_sign_verify_detached(const unsigned char *sig, const unsigned char *m,
                                       unsigned long long mlen, const unsigned char *pk);
}
#endif

// sig = Ed25519(secret, digest); digest = SHA-256(image bytes, trailer ke baghair).
// true = signature theek, false = ghalat/najaiz.
static inline bool rsh_verify_signature(const uint8_t *sig, const uint8_t *digest, const uint8_t *pubkey) {
  if (!sig || !digest || !pubkey) return false;
  return crypto_sign_verify_detached(sig, digest, 32, pubkey) == 0;
}
