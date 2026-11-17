// RSH device key store — implementation (keystore.h dekho).
#include "keystore.h"
#include "diag_log.h"  // DS-65: NVS write fail diag event
#include "fw34_state.h"  // FW-3 armed state (rsh.ino) — key_reset_armed ke liye
#include "pairing_window.h"  // F-02/F-03: transport-independent window deadline
#include "pw_obfuscate.h"  // SEC-E-01 (Phase F): NVS password obfuscation
#include <Arduino.h>
#include <Preferences.h>
#include <esp_system.h>  // esp_random()
#include <mbedtls/sha256.h>  // keystream SHA-256 (mbedTLS, net.cpp jaisa)
#include <string.h>

// Charset §5.1: A-Z a-z 2-9, ambiguous 0/1/I/O/l nikle hue.
// Gin lo to 57 symbols hain (24+25+8) — doc mein "58" likha hai, wo
// ghalat hai (SEC-D follow-up: doc fix). Entropy phir bhi ~140 bit
// (24 x log2(57) = 140.0).
static const char kPwCharset[] =
    "ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";
#define RSH_PW_CHARSET_LEN 57

static uint8_t s_key[RSH_KEY_LEN];
static char s_password[RSH_PASSWORD_MAX_LEN + 1];  // generated 24, user-set 8..63 (§5.2)
static uint8_t s_nonce[16];
static bool s_unpaired = true;
static bool s_haveKey = false;
static RshPairWindow s_window;  // F-02/F-03: pairing window deadline

static bool rng_bytes_(uint8_t *out, size_t n) {
  for (size_t i = 0; i < n; i += 4) {
    uint32_t r = esp_random();
    size_t take = (n - i < 4) ? (n - i) : 4;
    memcpy(out + i, &r, take);
  }
  return true;
}

// Kamzor key check: sab bytes 0 ya sab ek jaise → RNG fail mano.
static bool key_weak_(const uint8_t *k, size_t n) {
  uint8_t first = k[0];
  uint8_t allZero = 1, allSame = 1;
  for (size_t i = 0; i < n; i++) {
    if (k[i] != 0) allZero = 0;
    if (k[i] != first) allSame = 0;
  }
  return allZero || allSame;
}

// Rejection sampling: byte r < 228 (256 - 256%57) hi lo, phir r % 57 —
// modulo bias nahi (har symbol ke 4 preimages).
static char pw_char_() {
  for (;;) {
    uint8_t r = (uint8_t)(esp_random() & 0xFF);
    if (r < 228) return kPwCharset[r % RSH_PW_CHARSET_LEN];
  }
}

// SHA-256 one-shot (pw_obfuscate.h callback). mbedTLS _ret API (ESP-IDF
// mbedTLS 3.x). Fail ho to 0 — caller fail-closed (plaintext kabhi nahi).
static int sha256_(const uint8_t *data, size_t len, uint8_t out32[32]) {
  mbedtls_sha256_context ctx;
  mbedtls_sha256_init(&ctx);
  int ok = (mbedtls_sha256_starts_ret(&ctx, 0) == 0) &&
           (mbedtls_sha256_update_ret(&ctx, data, len) == 0) &&
           (mbedtls_sha256_finish_ret(&ctx, out32) == 0);
  mbedtls_sha256_free(&ctx);
  if (!ok) memset(out32, 0, 32);
  return ok ? 1 : 0;
}

// Password ko obfuscated bytes ke tor par NVS "wifipw2" mein likho aur
// purani plaintext "wifipw" entry (ho to) mitao. s_key pehle se set hona
// chahiye. false = NVS/SHA fail — caller fail-closed.
static bool pw_store_obfuscated_(Preferences &prefs, const char *pw, size_t len) {
  uint8_t cipher[RSH_PW_OBF_MAX_LEN];
  if (len > RSH_PW_OBF_MAX_LEN) return false;
  if (!rsh_pw_obfuscate(s_key, (const uint8_t *)pw, len, cipher, sha256_)) {
    memset(cipher, 0, sizeof(cipher));
    return false;
  }
  bool ok = prefs.putBytes("wifipw2", cipher, len) == len;
  memset(cipher, 0, sizeof(cipher));
  if (!ok) return false;
  prefs.remove("wifipw");  // legacy plaintext entry — migrate ho gayi
  return true;
}

// Password NVS se parho. Pehle "wifipw2" (obfuscated) — deobfuscate +
// validate; phir legacy plaintext "wifipw" — validate + foran "wifipw2"
// mein migrate kar ke plaintext mitao. Dono na hon / dono kharab hon to
// false (caller nayi key+password generate karta hai, fail-closed).
// out: NUL-terminated password, *out_len = lambai (NUL ke baghair).
static bool pw_load_(Preferences &prefs, char *out, size_t out_size, size_t *out_len) {
  if (out_size < RSH_PASSWORD_MAX_LEN + 1) return false;
  uint8_t buf[RSH_PW_OBF_MAX_LEN];
  size_t n = prefs.getBytes("wifipw2", buf, sizeof(buf));
  if (n >= RSH_PASSWORD_MIN_LEN && n <= RSH_PASSWORD_MAX_LEN) {
    uint8_t plain[RSH_PW_OBF_MAX_LEN + 1];
    if (rsh_pw_deobfuscate(s_key, buf, n, plain, sha256_) &&
        rsh_password_validate(plain, (uint16_t)n) == RSH_CTRL_OK) {
      memcpy(out, plain, n);
      out[n] = '\0';
      *out_len = n;
      memset(buf, 0, sizeof(buf));
      memset(plain, 0, sizeof(plain));
      return true;
    }
    memset(plain, 0, sizeof(plain));
  }
  memset(buf, 0, sizeof(buf));
  // Legacy: plaintext "wifipw" — migrate karo (obfuscate + store + remove).
  String legacy = prefs.getString("wifipw", "");
  if (legacy.length() < RSH_PASSWORD_MIN_LEN ||
      legacy.length() > RSH_PASSWORD_MAX_LEN) {
    return false;
  }
  if (rsh_password_validate((const uint8_t *)legacy.c_str(),
                            (uint16_t)legacy.length()) != RSH_CTRL_OK) {
    return false;
  }
  if (!pw_store_obfuscated_(prefs, legacy.c_str(), legacy.length())) {
    return false;
  }
  memcpy(out, legacy.c_str(), legacy.length());
  out[legacy.length()] = '\0';
  *out_len = legacy.length();
  return true;
}

// Nayi key + naya password banao aur NVS mein likho (pehla boot + KEY_RESET
// dono yahi ek rasta istemal karte hain). prefs khuli hui "rsh" namespace
// hai; end() caller karta hai. false = fail closed (§4.6).
static bool keystore_generate_and_save_(Preferences &prefs) {
  bool ok = false;
  for (int attempt = 0; attempt < 3 && !ok; attempt++) {
    rng_bytes_(s_key, sizeof(s_key));
    ok = !key_weak_(s_key, sizeof(s_key));
  }
  if (!ok) return false;
  for (int i = 0; i < RSH_PASSWORD_LEN; i++) s_password[i] = pw_char_();
  s_password[RSH_PASSWORD_LEN] = '\0';
  if (prefs.putBytes("devkey", s_key, sizeof(s_key)) != sizeof(s_key)) return false;
  // SEC-E-01: password plaintext "wifipw" mein kabhi nahi — obfuscated
  // "wifipw2" mein jata hai (pw_store_obfuscated_ legacy entry bhi mitata hai).
  if (!pw_store_obfuscated_(prefs, s_password, RSH_PASSWORD_LEN)) return false;
  if (!prefs.putBool("paired", false)) return false;
  s_unpaired = true;
  return true;
}

bool keystore_begin() {
  // Nonce har boot par naya (RAM only) — reboot = naya session.
  rng_bytes_(s_nonce, sizeof(s_nonce));

  Preferences prefs;
  if (!prefs.begin("rsh", false)) return false;
  size_t keyLen = prefs.getBytes("devkey", s_key, sizeof(s_key));
  bool paired = prefs.getBool("paired", false);

  // Password: "wifipw2" (obfuscated) pehle; legacy plaintext "wifipw" ho
  // to pw_load_ use migrate kar deta hai (SEC-E-01). Key load hone se
  // pehle password parha nahi ja sakta (keystream key se banta hai) —
  // is liye keyLen check pehle.
  char pwBuf[RSH_PASSWORD_MAX_LEN + 1];
  size_t pwLen = 0;
  bool havePw = false;
  if (keyLen == sizeof(s_key)) {
    havePw = pw_load_(prefs, pwBuf, sizeof(pwBuf), &pwLen);
  }

  // DS-66: "no credentials" (pehla boot / NVS erase) vs "credentials
  // mojood lekin parhe nahi ja rahe" (transient NVS glitch / corrupt)
  // alag karo. Key hi na ho to generate karo (pehla boot). Key mojood
  // ho lekin lambai ghalat ho to corrupt hai — fail closed, overwrite
  // NAHI (warna pairing hamesha ke liye khatam). Password entry mojood
  // ho lekin load fail ho to ek dafa dobara parho (transient glitch);
  // phir bhi fail ho to fail closed WITHOUT overwriting — keystore_begin()
  // false deta hai aur rsh.ino 5 s retry karta hai, pairing mehfooz rehti
  // hai. Password entry bilkul na ho (generate ke beech crash jaisi
  // aadhi state) to regenerate hi ek rasta hai — purani key ka koi
  // usable password hai hi nahi.
  bool keyExists = prefs.isKey("devkey");
  bool pwExists = prefs.isKey("wifipw2") || prefs.isKey("wifipw");
  if (keyLen != sizeof(s_key)) {
    if (keyExists) { prefs.end(); return false; }  // corrupt key: fail closed
    if (!keystore_generate_and_save_(prefs)) { prefs.end(); return false; }
  } else if (!havePw) {
    if (pwExists) {
      // Transient read glitch ho sakta hai — ek dafa dobara parho.
      havePw = pw_load_(prefs, pwBuf, sizeof(pwBuf), &pwLen);
      if (!havePw) { prefs.end(); return false; }  // unreadable: fail closed
    } else {
      // Key hai, password entry hi nahi (aadhi generate state).
      if (!keystore_generate_and_save_(prefs)) { prefs.end(); return false; }
      // generate ne s_password bhar diya; neeche else-branch skip karo.
      prefs.end();
      s_haveKey = true;
      return true;
    }
    memcpy(s_password, pwBuf, pwLen);
    s_password[pwLen] = '\0';
    s_unpaired = !paired;
  } else {
    memcpy(s_password, pwBuf, pwLen);
    s_password[pwLen] = '\0';
    s_unpaired = !paired;
  }
  prefs.end();
  s_haveKey = true;
  return true;
}

const uint8_t *keystore_key(void) { return s_haveKey ? s_key : NULL; }
const char *keystore_password(void) { return s_haveKey ? s_password : ""; }
const uint8_t *keystore_nonce(void) { return s_nonce; }

void keystore_rotate_nonce(void) { rng_bytes_(s_nonce, sizeof(s_nonce)); }

bool keystore_unpaired(void) { return s_unpaired; }

void keystore_mark_ap_up(uint32_t now_ms) {
  // Pehli AP-up: 60 s boot window (F-03 — deadline model; dobara AP-up
  // (rescan) window dobara NAHI kholta — net.cpp apUpMarked guard).
  if (!s_window.is_set)
    rsh_pair_window_open(&s_window, now_ms, RSH_PAIR_WINDOW_BOOT_MS, 1);
}

bool keystore_pairing_open(uint32_t now_ms) {
  return rsh_pair_window_is_open(&s_window, now_ms, s_unpaired ? 1 : 0) ? true : false;
}

void keystore_open_pair_window(uint32_t now_ms) {
  // Pair button (F-02) ya serial HELO (F-03): 120 s, sirf unpaired.
  rsh_pair_window_open(&s_window, now_ms, RSH_PAIR_WINDOW_BUTTON_MS,
                       s_unpaired ? 1 : 0);
}

uint16_t keystore_window_remaining_s(uint32_t now_ms) {
  return rsh_pair_window_remaining_s(&s_window, now_ms, s_unpaired ? 1 : 0);
}

void keystore_mark_paired(void) {
  if (!s_unpaired) return;
  // DS-65: RAM (s_unpaired) sirf tab flip karo jab NVS write kamyab ho.
  // Pehle s_unpaired unconditional flip hota tha aur NVS write best-effort
  // thi — write fail ho to board reboot tak paired dikhta, phir dobara
  // unpaired (AP khula, key disclosable). Fail ho to ek dafa retry karo;
  // phir bhi fail ho to unpaired hi raho + diag event (agla GET_CAPS OK
  // dobara koshish karega).
  Preferences prefs;
  if (!prefs.begin("rsh", false)) {
    rsh_diag_record_global(RSH_DIAG_KEYSTORE_NVS_FAIL, 0, millis());
    return;
  }
  bool ok = prefs.putBool("paired", true);
  if (!ok) {
    prefs.end();
    if (prefs.begin("rsh", false)) {
      ok = prefs.putBool("paired", true);
      prefs.end();
    }
  } else {
    prefs.end();
  }
  if (!ok) {
    rsh_diag_record_global(RSH_DIAG_KEYSTORE_NVS_FAIL, 0, millis());
    return;  // s_unpaired true rehta hai — board unpaired hi hai
  }
  s_unpaired = false;
}

bool keystore_key_reset_armed(void) {
  return rsh_fw34_key_reset_armed() != 0;  // FW-3 owns armed state (§4.4)
}

// KEY_RESET (§4.4): nayi key + naya password, unpaired. Config NAHI
// badalti (sirf credentials). Caller reply bhejne KE BAAD bulata hai,
// phir reboot karta hai — naye boot par open AP + nayi pairing window.
// false = generation/NVS fail → caller restart NAHI karta (fail closed).
bool keystore_key_reset(void) {
  if (!s_haveKey) return false;
  Preferences prefs;
  if (!prefs.begin("rsh", false)) return false;
  bool ok = keystore_generate_and_save_(prefs);
  prefs.end();
  return ok;
}

// SET_PASSWORD (§5.2): user ka naya password NVS mein likho + RAM copy
// update karo. Keystore khud AP restart NAHI karta — caller (net.cpp)
// reply bhejne ke baad AP restart karta hai taake naya password AP +
// WebOTA dono par laagu ho.
// SEC-E-01: NVS mein obfuscated "wifipw2" jata hai — plaintext "wifipw"
// kabhi nahi (pw_store_obfuscated_ legacy entry bhi mita deta hai).
bool keystore_set_password(const char *pw, uint8_t len) {
  if (!s_haveKey || !pw) return false;
  if (rsh_password_validate((const uint8_t *)pw, len) != RSH_CTRL_OK)
    return false;
  char tmp[RSH_PASSWORD_MAX_LEN + 1];
  memcpy(tmp, pw, len);
  tmp[len] = '\0';
  Preferences prefs;
  if (!prefs.begin("rsh", false)) return false;
  bool ok = pw_store_obfuscated_(prefs, tmp, len);
  prefs.end();
  if (!ok) {
    memset(tmp, 0, sizeof(tmp));
    return false;
  }
  memcpy(s_password, tmp, len + 1);
  memset(tmp, 0, sizeof(tmp));
  return true;
}
