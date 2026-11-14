# RSH — Security Model

> **Versions:** App **v2.0.4** · Firmware **v4.0.1** · Protocol **v2.1**
> (2026-11-14).
> Full analysis: [`THREAT-MODEL.md`](THREAT-MODEL.md) (STRIDE),
> [`KEY-LIFECYCLE.md`](KEY-LIFECYCLE.md),
> [`SECURITY-RESCAN-ROUND2-W5.md`](SECURITY-RESCAN-ROUND2-W5.md).
> Latest re-scan: **CONDITIONAL PASS, no Critical/High findings.**

## Threat model in one paragraph

The wheel is a Wi-Fi access point in the user's home; the PC app
talks to it over UDP and serial; the firmware is updated over the air.
The model assumes an attacker on the same Wi-Fi network (or in range
of the AP) and a malicious firmware image offered for upload — but
**not** physical access to the board's flash, and **not** a
compromised PC (if the PC is owned, DPAPI and the key store go with
it). The biggest residual risks are the open AP during the pairing
window and the board-side flash-encryption steps that still need a
real board (see "Open items").

## What protects what

| Asset / risk | Protection |
|---|---|
| Control commands (reboot, config, key reset) | HMAC-SHA256 authenticated control channel; per-device key; nonce + counter (replay protection). Unauthenticated `RSTX` removed. |
| Firmware image | **Ed25519 signature, verified before apply**, anti-downgrade. The app verifies the signature too (NSec.Cryptography) before upload. |
| Per-device key | Random 32 B, generated on first boot, exchanged only in a **60 s pairing window**. Stored in DPAPI on the PC, in NVS on the board. Survives factory reset. |
| Wi-Fi password | Stored in NVS (obfuscated); on the PC it is kept in the DPAPI blob and read through the DPAPI path — no plaintext copy in `settings.json`. |
| Web OTA | HTTP Basic Auth on **every** handler, lockout after failed attempts, CSRF protection. ArduinoOTA is disabled. |
| vJoy / HID output | Buttons > 32 are rejected, never clamped onto 1–32 (no aliasing). |
| Logs | Contain **no secrets** (verified by re-scan, 0 hits). |
| Release integrity | Firmware is signed in CI via the `RSH_SIGNING_KEY_PEM` Actions secret; the workflow verifies the signature and fails closed. Signing keys live **outside this repo** and are never committed. |
| App distribution | The app is **unsigned** (user decision — OV certificate deferred). Expect a Windows SmartScreen warning; firmware Ed25519 signing is unaffected. |

## Key lifecycle (summary)

1. **First boot:** the board generates a random 32-byte key and a
   nonce; the pairing window opens for 60 s.
2. **Pairing:** the PC completes HELLO inside the window and stores
   the key in DPAPI, bound to the board's MAC.
3. **Normal use:** every control frame carries nonce + counter under
   HMAC-SHA256. A replayed frame is rejected (`REPLAY`).
4. **Key reset:** `KEY_RESET` (0x0B) requires a BOOT-hold on the
   board. `FACTORY_RESET` (0x08) resets config but **does not** wipe
   the key.
5. **Rotation:** the nonce rotates only on reboot or a new session.

Full lifecycle: [`KEY-LIFECYCLE.md`](KEY-LIFECYCLE.md). MAC-derived
passwords are never used (insecure by design).

## Flash encryption and Secure Boot

Prepared, not yet executed on hardware: RSA-3072 Secure Boot v2 +
flash encryption + encrypted NVS (`sdkconfig.secure.defaults`,
`partitions-secure.csv`,
[`FLASH-ENCRYPTION-PROCEDURE.md`](FLASH-ENCRYPTION-PROCEDURE.md)).
The eFuse steps can only run on the real board (A7 run,
[`A7-RUNBOOK.md`](A7-RUNBOOK.md)). Until then the Wi-Fi password in
NVS is obfuscated, not encrypted — a known, documented limitation.

## What is NOT protected (open items)

These are tracked, not hidden. See the security reviews for detail.

- **SEC-R2-01 (Medium):** no in-app firmware upload in classic mode
  (the wizard is the only upload UI and classic mode skips it).
  Workaround: browser Web OTA. Planned for v2.0.5.
- **SEC-R2-03:** after the 60 s pairing window, an unpaired board's
  AP stays open indefinitely — needs a design decision.
- **SEC-R2-04:** an unauthenticated HELO with `sessionFlag = 1`
  rotates the nonce and resets both control sessions (DoS vector).
- **SEC-R2-06:** no board-binding enforcement in the key store
  (evil-twin risk inside the 60 s window).
- **SEC-H-04:** board-side execution of the review items (A7).
- **OPEN-3:** flash encryption board-side (above).

## Reporting a vulnerability

This is a personal / DIY project, not a company product. If you find
a security issue, please open a GitHub issue on
[dreamsofts34-creator/RSH](https://github.com/dreamsofts34-creator/RSH)
with "SECURITY:" in the title, and do not publish an exploit before
it is fixed. Keys in `signing-keys/` (outside the repo) must never be
shared or committed — the `.gitignore` blocks `*.pem` and
`signing-keys/` outright.
