# RSH — Signed OTA (SEC-C-06 / SEC-D-04, Phase D)

**Status:** IMPLEMENTED — Phase D, FW-9 workstream. Authority for the
signed-image format, the board verify flow, and the signing tool.
Companion docs: `docs/KEY-LIFECYCLE.md` §9 (signing keys), `docs/PROTOCOL-V2.1.md`
§5 (password), `docs/THREAT-MODEL.md` TM-O01/TM-O02.

## 0. TL;DR

- Har OTA image **signed** hoti hai (Ed25519, OPEN-17 CLOSED).
- Board image **apply karne se pehle** signature verify karta hai
  (verify-before-apply). Unsigned ya ghalat-signature wali image reject
  hoti hai; purana firmware chalta rehta hai.
- App upload se pehle sirf trailer ki maujoodgi + format check karta
  hai (fail-fast) — asliyat board-side sabit hoti hai.
- Signing key **offline** rehti hai, kabhi repo/git/image mein nahi.

## 1. Signed-image format

```
[ESP32 app image, apne trailing SHA-256 digest samet]
[Ed25519 signature, 64 B] [magic "RSHS", 4 B]     <- trailer, 68 B
```

- **Signed region:** file ke pehle `len - 68` bytes (poori ESP image,
  us ka SHA digest samet).
- **Digest:** `SHA-256(signed region)` (32 bytes).
- **Signature:** `Ed25519(secret_key, digest)` — hash-then-sign.
  Hash-then-sign is liye hai ke verify ke liye poori image (1.3 MB)
  RAM mein nahi chahiye — board streaming SHA-256 karta hai aur
  aakhir mein 32-byte digest par one-shot Ed25519 verify karta hai.
  (Ye wohi construction hai jo Ed25519ph andar se karta hai, SHA-512
  ki jagah SHA-256 ke saath; security SHA-256 ki collision resistance
  par munhasir hai.)
- **Trailer placement:** image ke aakhir mein. Bootloader image ko us
  ke SHA digest par khatam samajhta hai — trailer ke extra bytes
  ignore hote hain. `Update.end(true)` ESP image SHA bhi verify karta
  hai (doosri layer).

## 2. Board verify flow (firmware/rsh/webota.cpp)

Upload stream hote hue, har chunk par:

1. **Magic** — pehle chunk ka pehla byte `0xE9` hona chahiye (F9).
2. **Size cap** — cumulative bytes > app partition (`0x140000`) ho to
   abort (incremental — multipart `totalSize` running total hai,
   declared size nahi).
3. **Version scan** — `"Version <b>"` marker se version nikalo
   (streaming, chunk boundary par bhi — `ota_validate.h`).
4. **Digest** — signed region ka SHA-256 streaming (aakhri 68 bytes
   holdback — `RshSignHash`).

`UPLOAD_FILE_END` par, `Update.end()` se PEHLE:

5. **Trailer** — aakhri 68 bytes mein magic `"RSHS"` hona chahiye,
   warna `unsigned image` reject.
6. **Version** — marker na mile to reject; image version < running
   version ho to `downgrade` reject (barabar = reinstall, allowed).
7. **Signature** — `Ed25519(pubkey, digest)` verify (libsodium,
   `crypto_sign_verify_detached`). Fail ho to reject.
8. Sirf sab pass hon to `Update.end(true)` (ESP SHA bhi verify hoti
   hai) aur restart schedule hota hai. Koi bhi check fail ho to
   `Update.abort()` — purana firmware chalta rehta hai.

Auth is se pehle hai: har endpoint par `authenticate()` + 5 fails
par 60 s lockout (SEC-D-02), aur `/update` par per-boot CSRF token
header `X-RSH-CSRF` (SEC-D-12). Bina auth ke restart/DoS nahi hota.

## 3. App-side checks (RSH.Core/Firmware)

`FirmwareImageValidator.Validate` (in-memory buffer par, SEC-C-09):

- Size cap, magic `0xE9`, merged-image reject, segment bounds,
  ESP SHA-256 re-compute, version marker + floor v4.0.0.
- **Trailer presence/format** — aakhri 68 bytes + `"RSHS"` magic.
  Unsigned image yahin reject (fail-fast).
- **App signature verify NAHI karta** (.NET BCL mein Ed25519 verify
  nahi) — asliyat board-side hai. Ye jaan-boojh kar hai, kami nahi.

`FirmwareUploadGate` (Phase D): upload se pehle GET_CAPS se board ka
live version mila kar anti-downgrade check; upload ke baad GET_CAPS se
naya version confirm (SEC-C-02) — HTTP 200 + `"OK"` akela kamyabi nahi.

**SEC-D-04 note (marker forgeable):** version marker aur SHA-256
self-consistent hona authenticity NAHI hai — hamlaawar marker daal
kar version `9.9.9` likh sakta hai aur sahi SHA bhi bana sakta hai.
Is liye board-side Ed25519 verify lazmi hai; marker/SHA checks sirf
ghalti se ghalat file chunnay se bachate hain, hamle se nahi.

## 4. Signing tool (firmware/rsh/sign-image.py)

Sirf openssl CLI + hashlib (koi nayi dependency nahi):

```bash
# Nayi keypair (OFFLINE machine par — private key kabhi repo mein nahi)
python3 sign-image.py --gen-keypair <dir>
#  -> <dir>/signing-key.pem (0600) + <dir>/signing_pubkey.h

# Image sign karo (app-only firmware.bin — merged image nahi)
python3 sign-image.py --sign <key.pem> firmware.bin
#  -> firmware-signed.bin (image + 68 B trailer)

# Verify karo (private ya public key se)
python3 sign-image.py --verify <key.pem|pub.pem> firmware-signed.bin

# Public-key header banao (firmware/rsh/signing_pubkey.h mein copy karo)
python3 sign-image.py --pubkey-header <key.pem>
```

Pehle se signed image dobara sign nahi hoti (error). Merged/factory
image sign nahi hoti (magic check).

### 4.1 Build mein signing step (firmware/rsh/sign_firmware.py)

`pio run` ke baad PlatformIO post-action `sign_firmware.py` chalta hai:

```bash
# Key configure ho to signed artifact banta hai:
RSH_SIGNING_KEY=/path/to/signing-key.pem pio run
#  -> .pio/build/esp32dev/firmware-signed.bin (OTA ke liye ye file use karo)
# Key configure na ho to step skip hota hai — build fail NAHI hota,
# dev build unsigned rehta hai (unsigned OTA board/app reject karte hain).
```

- Key env var se aati hai, kabhi repo mein nahi. Var set ho lekin key
  file na mile, ya sign fail ho, to build zor se fail hota hai — release
  build khamoshi se unsigned nahi rehni chahiye.
- **Production (Phase H):** release build offline release keypair se
  sign hoti hai (private key secure storage mein, CI/release machine par
  `RSH_SIGNING_KEY` set); release workflow GitHub Release mein
  `firmware-signed.bin` dalta hai, raw `firmware.bin` nahi. Public key
  usi release build mein `signing_pubkey.h` se bake hoti hai.
- **Tests:** `RSH.Tests.PhaseCAppTests.RealFirmwareBin_Passes` raw
  `firmware.bin` ko test-signing flow se validate karta hai (in-memory
  format-valid trailer — app validator sirf format check karta hai;
  crypto board-side hai). Jab `firmware-signed.bin` maujood ho to
  `RealSignedFirmwareBin_Passes_IfPresent` us ko seedha validate karta
  hai. Test key kabhi production key nahi banti.

## 5. Key provisioning

- **Board par:** `firmware/rsh/signing_pubkey.h` mein 32-byte Ed25519
  public key (compile-time). Private key board par kabhi nahi hoti.
- **Release key (Phase H — PROVISIONED 2026-11-13):** release keypair
  ban gaya hai — private key `~/workspace/projects/rsh/signing-keys/
  signing-key.pem` (0600, repo se bahar), public key
  `firmware/rsh/signing_pubkey.h` mein bake hai. Public-key SHA-256
  fingerprint: `387764a4529337c437b233a148b48f9b2ffe3f3b5015e14c95737cb3a67a6411`.
  Is key se signed images hi board accept karta hai (rebuild + USB
  flash ke baad). Key handling + backup rules us dir ki README mein
  hain. **Sandbox note:** keypair is sandbox VM mein generate hui hai
  (ideal offline machine nahi) — production release se pehle owner
  faisla kare: isi keypair ko manzoor karo ya offline machine par naya
  keypair banao (procedure wahi — §4).
- **Dev key (purani, hat gayi):** header mein pehle DEV ONLY key thi —
  us ki private key generate kar ke discard kar di gayi thi, is liye is
  key se aaj koi image sign nahi kar sakta tha. Phase H mein us ki jagah
  release key aa gayi hai (upar).
- **Key rotation:** public key badalne par firmware dobara USB se
  flash karni hogi (OTA se nahi — purani key se signed image hi OTA
  ho sakti hai). Is liye release key badalna ek planned migration hai,
  khamoshi se nahi hoga. Compromise response OPEN-19 hai —
  `docs/FLASH-ENCRYPTION-PROCEDURE.md` §7.

## 6. Load-bearing strings (na toro)

- WebOTA page mein `"Version <b>"` + `FIRMWARE_VERSION` **ek hi
  string literal** mein hone chahiye (`webota.cpp` kPageB) — marker aur
  version digits flashed binary mein aas-paas rehte hain; scanner aur
  C# validator isi par munhasir hain. Literal toro ge to anti-downgrade
  har image reject karne lagega (fail closed, lekin OTA band).
- Trailer magic `"RSHS"` aur size 68 B dono taraf (firmware
  `ota_validate.h`, C# `FirmwareImageValidator`, `sign-image.py`)
  ek jaise hone chahiye — teeno jagah ek saath badlo.

## 7. Residual risks

- **Trailer + Update.end interaction, real board par verify nahi hua:**
  host tests verify logic (trailer parse, digest, signature) cover
  karte hain, lekin ESP-IDF `Update.end()` ka trailing-68-bytes wali
  image ke saath rawayya sirf real board par prove hoga (A7 gate —
  pehli signed OTA real board par test karo). Agar `Update.end()`
  trailing bytes par fail ho to format badalna hoga (misal signature
  alag multipart field mein).
- **Dev pubkey unusable (purana note — ab badal gaya):** pehle checked-in
  dev key se koi image sign nahi kar sakta tha (private key discarded).
  Phase H mein release keypair ban gaya hai (§5) — ab is key se signed
  images hi board accept karta hai.
- **OPEN-3 (flash encryption):** NVS mein password/key plain NVS mein
  hain — physical flash dump se parhe ja sakte hain (TM-W07). Signing
  key is se mutasir nahi hoti (wo board par hoti hi nahi — sirf public
  key hoti hai). Asal fix ka procedure `docs/FLASH-ENCRYPTION-PROCEDURE.md`
  mein hai (Secure Boot v2 RSA-3072 + flash encryption release mode +
  encrypted NVS; secure-build files + runtime check Phase H W1 mein ban
  gaye, board execution A7 gate mein hoga). Tab tak OPEN-3 khula hai.
- **OPEN-19 (compromise response):** signing / Secure Boot key compromise
  hone par kya hoga — likha gaya hai: `docs/FLASH-ENCRYPTION-PROCEDURE.md`
  §7 (revocation, re-sign, purane boards, key loss). OPEN-19 CLOSED
  (doc) — board execution §7 ke mutabiq hoga agar kabhi zaroorat pare.
- **No Secure Boot yet:** app-level signature Secure Boot ka badal
  nahi hai — Secure Boot enablement (RSA-3072, eFuse) Phase D/H ka
  alag kaam hai. Aaj koi bhi USB se kuch bhi flash kar sakta hai
  (physical access = authority, threat model §5).
