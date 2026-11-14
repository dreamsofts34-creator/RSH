#pragma once
// RSH device key store (FW-8, KEY-LIFECYCLE.md §2/§3, PROTOCOL-V2.1.md §4).
// Arduino-only (Preferences + esp_random) — host tests is ko cover nahi
// karte; pure logic (HMAC/session) ctrl.h + protocol.h mein test hota hai.
//
// - Pehle boot par board khud 32 B random key + 24-char password banata
//   hai (hardware RNG, rejection sampling) aur NVS ("rsh" namespace) mein
//   likhta hai. FACTORY_RESET key/password ko NAHI chheerta (KEY_RESET
//   alag command hai — FW-3/§4.4 owns armed state; yahan sirf read API).
// - Nonce (16 B) SIRF RAM mein hai — har boot par naya (reboot = naya
//   session, §2.3). NVS mein kabhi nahi.
// - Pairing window: AP-up se 60 s, sirf unpaired board par. Window ke
//   baad HELLO mein key/password ZERO bheje jate hain.
// - RNG/NVS fail → begin() false → board normal boot mein aage NAHI
//   barhta (fail closed, §4.6 — kamzor/repeat key ke saath boot kabhi nahi).
//
// NOTE (OPEN-3, residual — SEC-E-01, Phase F W1): Preferences (NVS)
// bina flash-encryption ke plaintext-ish store karta hai — flash dump se
// key parhi ja sakti hai. Phase F mein password ke liye ek behtari aayi:
// "wifipw" (plaintext string) ki jagah "wifipw2" (device key se obfuscated
// bytes, pw_obfuscate.h) — purana plaintext pehli load par migrate ho kar
// mit jata hai. Ye OBFUSCATION hai, encryption nahi (device key khud usi
// NVS mein plaintext hai — poora dump + open-source scheme = recoverable).
// Asal hal encrypted NVS / flash encryption hai (OPEN-3) — us ka exact
// mechanism + procedure Phase H mein hai (docs/FLASH-ENCRYPTION-PROCEDURE.md
// — Secure Boot v2 RSA-3072 + flash encryption release mode + encrypted
// NVS; secure-build runtime check secure_nvs.h mein hai), is code se
// nahi badalta, sirf note karta hai.
#include <stdint.h>
#include <stddef.h>
#include "password.h"   // SET_PASSWORD length/charset (8..63, §5.2)

#define RSH_KEY_LEN 32
#define RSH_PASSWORD_LEN 24  // generated per-device password (§5.1); wire/store format mein hyphen NAHI

bool keystore_begin();  // false = key/password generate ya NVS fail
const uint8_t *keystore_key(void);       // 32 B device key (HMAC)
const char *keystore_password(void);     // 24 ASCII chars + NUL (AP + WebOTA)
const uint8_t *keystore_nonce(void);     // 16 B session nonce (RAM only)
void keystore_rotate_nonce(void);        // naya session (HELO sessionFlag=1)
bool keystore_unpaired(void);            // true = abhi tak pair nahi hua
bool keystore_pairing_open(uint32_t now_ms);  // unpaired + AP-up se 60 s andar
void keystore_mark_paired(void);         // pehli HMAC GET_CAPS OK par (§3 step 7)
void keystore_mark_ap_up(uint32_t now_ms);    // AP up hote hi — window yahin se
bool keystore_key_reset_armed(void);     // FW-3 owns armed state (§4.4 — rsh.ino se)
bool keystore_key_reset(void);           // KEY_RESET (§4.4): nayi key+password,
// unpaired; config nahi badalti. Caller reply ke BAAD bulaye, phir reboot.
// false = fail (caller restart NAHI karta — fail closed, purani key rehti hai).
// SET_PASSWORD (§5.2, Phase E WS5): user ka naya Wi-Fi/OTA password set karo
// (ek credential, AP + WebOTA dono ke liye). NVS "wifipw2" (obfuscated,
// SEC-E-01 Phase F) update hoti hai; key UNPAIRED state nahi badalti.
// Length/charset dobara validate hota hai (password.h) — ghalat input par
// false, purana password rehta hai. Caller reply bhejne KE BAAD bulaye,
// phir AP restart kare (reply OK restart se pehle jata hai). false = NVS
// fail → caller AP restart NAHI karta.
bool keystore_set_password(const char *pw, uint8_t len);
