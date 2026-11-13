#pragma once
// RSH Wi-Fi/OTA password at-rest obfuscation (SEC-E-01, Phase F W1,
// PROTOCOL-V2.1.md §5.3 / KEY-LIFECYCLE.md §2).
//
// MASLA: password NVS ("wifipw", Preferences) mein PLAINTEXT tha — flash
// dump / `strings` se seedha parha jata tha (OPEN-3 flash encryption khula
// hai, Phase H). FIX: NVS mein likhne se pehle password ko device key se
// derived keystream se XOR karo; parhte waqt wapas XOR (same operation).
// Naya NVS key "wifipw2" hai (obfuscated bytes, putBytes); purana plaintext
// "wifipw" pehli load par migrate ho kar mit jata hai (keystore.cpp).
//
// *** IMAANDARI SE NOTE (chhupana mana hai): ye OBFUSCATION hai,
// encryption nahi. *** Device key khud usi NVS mein plaintext hai
// ("devkey") — jo poora flash dump le sakta hai wo scheme (ye file open
// source hai) se password wapas bana sakta hai. Is fix se milta hai:
// (1) flash par `strings` / NVS dump mein password nazar nahi aata;
// (2) sirf "wifipw" entry parhne wala (misal NVS tooling) password nahi
//     le ja sakta; (3) ghalat key se deobfuscate karne par validation
//     fail hoti hai (password.h), is liye corrupt/stale entry kabhi
//     istemal nahi hoti. Asal hal flash encryption + encrypted NVS hai
//     (OPEN-3) — wo Phase H mein aata hai, is file se nahi badalta.
//
// Construction: keystream block j (32 B) = SHA256(key32 || "RSH-PW-V1" ||
// u32le(j)); ciphertext[i] = plaintext[i] ^ keystream[i]. SHA-256 caller
// deta hai (firmware: mbedTLS; host tests: independent SHA-256) — is
// header mein koi crypto implement nahi, sirf XOR + framing (protocol.h
// ke rsh_hmac16_fn pattern jaisa).
//
// Bounds: password 0..63 bytes (password.h: 8..63 valid, 0 = "koi entry
// nahi"). Key hamesha 32 B. out buffer kam az kam len ka ho. Invalid input
// (null, ghalat key len, len > 63) → false, out ko haath nahi lagata.
#include <stdint.h>
#include <stddef.h>
#include <string.h>

#define RSH_PW_OBF_MAX_LEN 63
#define RSH_PW_OBF_DOMAIN "RSH-PW-V1"
#define RSH_PW_OBF_DOMAIN_LEN 9

/// SHA-256 one-shot callback (caller-supplied crypto). Return 1 on
/// success, 0 on failure — fail ho to obfuscate/deobfuscate bhi fail hota
/// hai (fail-closed; zero keystream se plaintext likhna kabhi nahi hota).
typedef int (*rsh_sha256_fn)(const uint8_t *data, size_t len, uint8_t out32[32]);

static inline int rsh_pw_obfuscate(const uint8_t *key32,
                                   const uint8_t *in, size_t len,
                                   uint8_t *out, rsh_sha256_fn sha256) {
  if (!key32 || !in || !out || !sha256) return 0;
  if (len > RSH_PW_OBF_MAX_LEN) return 0;
  size_t done = 0;
  uint32_t block = 0;
  while (done < len) {
    // Block input: key32 || "RSH-PW-V1" || u32le(block).
    uint8_t msg[32 + RSH_PW_OBF_DOMAIN_LEN + 4];
    memcpy(msg, key32, 32);
    memcpy(msg + 32, RSH_PW_OBF_DOMAIN, RSH_PW_OBF_DOMAIN_LEN);
    msg[32 + RSH_PW_OBF_DOMAIN_LEN + 0] = (uint8_t)(block & 0xFF);
    msg[32 + RSH_PW_OBF_DOMAIN_LEN + 1] = (uint8_t)((block >> 8) & 0xFF);
    msg[32 + RSH_PW_OBF_DOMAIN_LEN + 2] = (uint8_t)((block >> 16) & 0xFF);
    msg[32 + RSH_PW_OBF_DOMAIN_LEN + 3] = (uint8_t)((block >> 24) & 0xFF);
    uint8_t ks[32];
    if (!sha256(msg, sizeof(msg), ks)) {
      memset(ks, 0, sizeof(ks));
      memset(msg, 0, sizeof(msg));
      return 0;
    }
    size_t take = (len - done < sizeof(ks)) ? (len - done) : sizeof(ks);
    for (size_t i = 0; i < take; i++) out[done + i] = (uint8_t)(in[done + i] ^ ks[i]);
    memset(ks, 0, sizeof(ks));
    memset(msg, 0, sizeof(msg));
    done += take;
    block++;
  }
  return 1;
}

/// Deobfuscate = obfuscate (XOR apna inverse hai).
static inline int rsh_pw_deobfuscate(const uint8_t *key32,
                                     const uint8_t *in, size_t len,
                                     uint8_t *out, rsh_sha256_fn sha256) {
  return rsh_pw_obfuscate(key32, in, len, out, sha256);
}
