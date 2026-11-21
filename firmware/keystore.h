#pragma once
// RSH Wi-Fi password store (v2.2, no-pairing redesign — PROTOCOL-V2.2.md §5,
// NO-PAIRING-PLAN.md §3 D3/D6/D7/D23).
// Arduino-only (Preferences + esp_random) — host tests is ko cover nahi
// karte; pure logic (control dispatch, password validation, obfuscation)
// ctrl.h / password.h / pw_obfuscate.h mein test hota hai.
//
// Model (v2.2 — device key / pairing khatam):
// - Sirf EK credential hai: user-set Wi-Fi password (AP + WebOTA dono ke
//   liye). Koi device key nahi, koi generated password nahi, koi `paired`
//   flag nahi, koi pairing window nahi.
// - Koi password stored nahi hai to AP KHULA hai (open hotspot, pehli
//   setup). User Settings se password set karta hai (SET_PASSWORD 0x0C);
//   us ke baad AP WPA2-PSK hai.
// - Password NVS "wifipw2" mein obfuscated hai (pw_obfuscate.h) —
//   keystream key ek fresh random NVS key ("obfkey", 32 B) hai jo pehli
//   4.0.3 boot par generate hoti hai (D23). Ye OBFUSCATION hai,
//   encryption nahi (OPEN-3: asal hal flash encryption hai).
// - Migration (D6): pehli 4.0.3 boot (NVS marker "mig403") purane
//   secrets — "devkey", "paired", "wifipw2", "wifipw" — mita deti hai.
//   AP us ke baad khula aata hai; user naya password set karta hai.
// - Password clear sirf physical hai (D7): BOOT >= 10 s hold ->
//   keystore_clear_password() + reboot -> open AP. Koi wire opcode nahi.
// - NVS fail -> begin()/set/clear false -> board fail closed (rsh.ino
//   5 s retry karta hai; ghalat/na-maloom password ke saath AP kabhi
//   up nahi hota).
#include <stdint.h>
#include <stddef.h>
#include "password.h"   // SET_PASSWORD length/charset (8..63, §5.2)

bool keystore_begin();  // false = migration/obfkey/NVS fail (fail closed)
bool keystore_has_password(void);   // true = user-set password stored hai
const char *keystore_password(void);  // stored password, ya "" (koi nahi)
// Stored password se constant-time match (false jab koi password na ho).
bool keystore_check_password(const char *pw, uint8_t len);
// SET_PASSWORD (§5.2, D3): user ka naya Wi-Fi/OTA password set karo.
// NVS "wifipw2" (obfuscated) update hoti hai. Length/charset dobara
// validate hota hai (password.h) — ghalat input par false, purana
// password rehta hai. Caller reply bhejne KE BAAD bulaye, phir AP restart
// kare (reply OK restart se pehle jata hai). false = NVS fail → caller AP
// restart NAHI karta.
bool keystore_set_password(const char *pw, uint8_t len);
// D7: password mitao (NVS + RAM) — BOOT >= 10 s hold se. Clear ke baad
// AP khula aata hai. false = NVS remove fail (fail closed — RAM password
// rehta hai, caller reboot NAHI karta). R3-FW-03: clear `s_ready` par
// munhasir NAHI hai — corrupt obfkey + blob wali bricked state mein bhi
// kaam karta hai (recovery path); obfkey bhi mit-ti hai taake agla
// begin() nayi key generate kar sake.
bool keystore_clear_password(void);
