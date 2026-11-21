// RSH Wi-Fi password store — implementation (keystore.h dekho, v2.2).
#include "keystore.h"
#include "diag_log.h"  // DS-65: NVS write fail diag event
#include "pw_obfuscate.h"  // SEC-E-01 (Phase F) + D23 re-key: NVS password obfuscation
#include <Arduino.h>
#include <Preferences.h>
#include <esp_system.h>  // esp_random()
#include <mbedtls/sha256.h>  // keystream SHA-256 (mbedTLS, net.cpp jaisa)
#include <string.h>

static uint8_t s_obfkey[32];  // D23: fresh random NVS obfuscation key ("obfkey")
static char s_password[RSH_PASSWORD_MAX_LEN + 1] = "";  // user-set, "" = koi nahi (AP khula)
static bool s_ready = false;

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

// Obfuscation key load karo, ya pehli dafa generate + store karo.
// Corrupt key (maujood lekin lambai ghalat) + us se bana password blob
// mojood ho to fail closed (overwrite karne se blob hamesha ke liye
// unreadable ho jata); blob na ho to regenerate mehfooz hai (kuch
// depend nahi karta). false = fail closed.
static bool obfkey_load_or_generate_(Preferences &prefs) {
  if (prefs.isKey("obfkey")) {
    size_t n = prefs.getBytes("obfkey", s_obfkey, sizeof(s_obfkey));
    if (n == sizeof(s_obfkey)) return true;
    if (prefs.isKey("wifipw2")) return false;  // corrupt key + blob: fail closed
    // Blob nahi — corrupt key ko nayi key se badlo.
  }
  for (int attempt = 0; attempt < 3; attempt++) {
    rng_bytes_(s_obfkey, sizeof(s_obfkey));
    if (!key_weak_(s_obfkey, sizeof(s_obfkey))) break;
    if (attempt == 2) return false;
  }
  if (key_weak_(s_obfkey, sizeof(s_obfkey))) return false;
  return prefs.putBytes("obfkey", s_obfkey, sizeof(s_obfkey)) == sizeof(s_obfkey);
}

// Password ko obfuscated bytes ke tor par NVS "wifipw2" mein likho.
// s_obfkey pehle se set hona chahiye. false = NVS/SHA fail — caller fail-closed.
static bool pw_store_obfuscated_(Preferences &prefs, const char *pw, size_t len) {
  uint8_t cipher[RSH_PW_OBF_MAX_LEN];
  if (len > RSH_PW_OBF_MAX_LEN) return false;
  if (!rsh_pw_obfuscate(s_obfkey, (const uint8_t *)pw, len, cipher, sha256_)) {
    memset(cipher, 0, sizeof(cipher));
    return false;
  }
  bool ok = prefs.putBytes("wifipw2", cipher, len) == len;
  memset(cipher, 0, sizeof(cipher));
  return ok;
}

// Password NVS "wifipw2" se parho (deobfuscate + validate). Entry na ho /
// kharab ho to false. Legacy plaintext "wifipw" migrate NAHI hota — D6
// migration use pehli 4.0.3 boot par mita deti hai.
// out: NUL-terminated password, *out_len = lambai (NUL ke baghair).
static bool pw_load_(Preferences &prefs, char *out, size_t out_size, size_t *out_len) {
  if (out_size < RSH_PASSWORD_MAX_LEN + 1) return false;
  uint8_t buf[RSH_PW_OBF_MAX_LEN];
  size_t n = prefs.getBytes("wifipw2", buf, sizeof(buf));
  if (n < RSH_PASSWORD_MIN_LEN || n > RSH_PASSWORD_MAX_LEN) {
    memset(buf, 0, sizeof(buf));
    return false;
  }
  uint8_t plain[RSH_PW_OBF_MAX_LEN + 1];
  bool ok = rsh_pw_deobfuscate(s_obfkey, buf, n, plain, sha256_) &&
            rsh_password_validate(plain, (uint16_t)n) == RSH_CTRL_OK;
  memset(buf, 0, sizeof(buf));
  if (!ok) {
    memset(plain, 0, sizeof(plain));
    return false;
  }
  memcpy(out, plain, n);
  out[n] = '\0';
  *out_len = n;
  memset(plain, 0, sizeof(plain));
  return true;
}

bool keystore_begin() {
  Preferences prefs;
  if (!prefs.begin("rsh", false)) return false;

  // D6 migration (pehli 4.0.3 boot — marker "mig403" ke baghair): purane
  // secrets mitao (devkey + paired + wifipw2 + wifipw). Purana password
  // migrate NAHI hota (purani device key ke baghair unreadable hai, aur
  // use rakhne se AP WPA2 aata ek unknown password ke saath — "AP open
  // jab tak password set na ho" ke khilaf). Marker aakhir mein set hota
  // hai — crash ho to agle boot par migration dobara chalti hai (idempotent).
  if (!prefs.getBool("mig403", false)) {
    prefs.remove("devkey");
    prefs.remove("paired");
    prefs.remove("wifipw2");
    prefs.remove("wifipw");
    if (!obfkey_load_or_generate_(prefs)) { prefs.end(); return false; }
    if (!prefs.putBool("mig403", true)) { prefs.end(); return false; }
  } else {
    if (!obfkey_load_or_generate_(prefs)) { prefs.end(); return false; }
  }

  // Password: 4.0.3 ki likhi "wifipw2" ho to load karo; na ho to AP khula
  // (s_password "" rehta hai — koi auto-generation nahi). Blob kharab ho
  // to fail closed (overwrite nahi — rsh.ino 5 s retry karta hai).
  s_password[0] = '\0';
  if (prefs.isKey("wifipw2")) {
    char pwBuf[RSH_PASSWORD_MAX_LEN + 1];
    size_t pwLen = 0;
    if (!pw_load_(prefs, pwBuf, sizeof(pwBuf), &pwLen)) {
      prefs.end();
      return false;
    }
    memcpy(s_password, pwBuf, pwLen);
    s_password[pwLen] = '\0';
    memset(pwBuf, 0, sizeof(pwBuf));
  }
  prefs.end();
  s_ready = true;
  return true;
}

bool keystore_has_password(void) { return s_password[0] != '\0'; }

const char *keystore_password(void) { return s_password; }

bool keystore_check_password(const char *pw, uint8_t len) {
  if (!pw) return false;
  size_t stored_len = strlen(s_password);
  if (stored_len == 0) return false;  // koi password stored nahi
  if (len != stored_len) return false;
  uint8_t diff = 0;  // constant-time compare
  for (size_t i = 0; i < stored_len; i++)
    diff |= (uint8_t)(s_password[i] ^ pw[i]);
  return diff == 0;
}

// SET_PASSWORD (§5.2, D3): user ka naya password NVS mein likho + RAM copy
// update karo. Keystore khud AP restart NAHI karta — caller (net.cpp)
// reply bhejne ke baad AP restart karta hai taake naya password AP +
// WebOTA dono par laagu ho.
// SEC-E-01: NVS mein obfuscated "wifipw2" jata hai — plaintext "wifipw"
// kabhi nahi.
bool keystore_set_password(const char *pw, uint8_t len) {
  if (!s_ready || !pw) return false;
  if (rsh_password_validate((const uint8_t *)pw, len) != RSH_CTRL_OK)
    return false;
  char tmp[RSH_PASSWORD_MAX_LEN + 1];
  memcpy(tmp, pw, len);
  tmp[len] = '\0';
  Preferences prefs;
  if (!prefs.begin("rsh", false)) {
    memset(tmp, 0, sizeof(tmp));
    return false;
  }
  // Obfkey lazily ensure (defensive — begin() use aam tor par load kar chuka hai).
  if (!obfkey_load_or_generate_(prefs)) {
    prefs.end();
    memset(tmp, 0, sizeof(tmp));
    return false;
  }
  bool ok = pw_store_obfuscated_(prefs, tmp, len);
  prefs.end();
  if (!ok) {
    rsh_diag_record_global(RSH_DIAG_KEYSTORE_NVS_FAIL, 0, millis());
    memset(tmp, 0, sizeof(tmp));
    return false;
  }
  memcpy(s_password, tmp, len + 1);
  memset(tmp, 0, sizeof(tmp));
  return true;
}

// D7: password mitao (NVS + RAM) — BOOT >= 10 s hold se. R3-FW-03:
// recovery path `s_ready` par munhasir NAHI hai — bricked state
// (corrupt obfkey + password blob) mein begin() kabhi kamyab nahi hota,
// is liye s_ready hamesha false rehta hai; purana gate (`!s_ready` par
// foran false) documented physical recovery ko bhi dead kar deta tha.
// Clear ka matlab hi hai "state phenk do": password blob + legacy
// plaintext key + obfuscation key teeno mit-ti hain, taake agle boot
// par begin() nayi obfkey generate karke khula AP la sake. Password
// READS fail-closed rehte hain (ye sirf delete karta hai — koi read,
// koi overwrite nahi). NVS remove fail ho to RAM password rehta hai
// (fail closed — caller reboot NAHI karta, warna reboot ke baad
// purana password wapas aa jata).
bool keystore_clear_password(void) {
  Preferences prefs;
  if (!prefs.begin("rsh", false)) return false;
  bool ok = true;
  if (prefs.isKey("wifipw2") && !prefs.remove("wifipw2")) ok = false;
  if (prefs.isKey("wifipw") && !prefs.remove("wifipw")) ok = false;
  // Corrupt obfkey bhi mitao — warna agla begin() usi corrupt key +
  // (ab mit chuke) blob ke baghair bhi regenerate karta (blob na ho to
  // regenerate mehfooz hai), lekin key ko rakhne ka koi faida nahi:
  // password ja chuka hai, is liye key ka waahid consumer khatam hai.
  if (prefs.isKey("obfkey") && !prefs.remove("obfkey")) ok = false;
  prefs.end();
  if (!ok) {
    rsh_diag_record_global(RSH_DIAG_KEYSTORE_NVS_FAIL, 0, millis());
    return false;
  }
  s_password[0] = '\0';
  return true;
}
