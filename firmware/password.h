#pragma once
// RSH Wi-Fi/OTA password validation (PROTOCOL-V2.1.md §5.2, SET_PASSWORD
// 0x0C) — PURE (Arduino-free) taake host par unit test ho sake.
//
// Rule (§5.2): request payload = `newLen u8 (8..63)` + password bytes.
// Charset: printable ASCII `0x21–0x7E` (space nahi). Length/charset fail
// → reply status UNSUPPORTED (6). Yehi rule teen jagah laagu hota hai:
//   (a) firmware control handler (ctrl_handlers.h) — wire validation,
//   (b) keystore_set_password (keystore.cpp) — NVS likhne se pehle dobara
//       check (defense in depth — handler ke baghair bhi koi ghalat
//       password NVS mein nahi jata),
//   (c) C# ControlClient.SetPasswordAsync — client-side validation
//       (wahi numbers, wahi charset; test dono taraf hain).
//
// Notes:
// - Generated per-device password 24 chars ka hota hai (§5.1) — ye
//   validator usay bhi accept karta hai (8..63 ke andar hai).
// - WPA2-PSK ki had bhi 8..63 hai — is range se bahar password se AP
//   up hi nahi hota, is liye board aisa password kabhi accept nahi karta.
// - Password yahan kabhi log/store nahi hota — sirf validate hota hai.
#include <stdint.h>
#include <stddef.h>
#include "protocol.h"   // RSH_CTRL_OK / RSH_CTRL_UNSUPPORTED (§3.9)

#define RSH_PASSWORD_MIN_LEN 8
#define RSH_PASSWORD_MAX_LEN 63

/// Password bytes validate karo. Return RSH_CTRL_OK ya RSH_CTRL_UNSUPPORTED.
static inline uint8_t rsh_password_validate(const uint8_t *pw, uint16_t len) {
  if (!pw) return RSH_CTRL_UNSUPPORTED;
  if (len < RSH_PASSWORD_MIN_LEN || len > RSH_PASSWORD_MAX_LEN)
    return RSH_CTRL_UNSUPPORTED;
  for (uint16_t i = 0; i < len; i++) {
    if (pw[i] < 0x21 || pw[i] > 0x7E) return RSH_CTRL_UNSUPPORTED;
  }
  return RSH_CTRL_OK;
}
