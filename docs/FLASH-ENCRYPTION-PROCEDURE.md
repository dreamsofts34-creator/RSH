# RSH — Flash Encryption + Encrypted NVS Procedure (OPEN-3 / SEC-E-01)

**Status:** PROCEDURE — Phase H Workstream 1. **Board par abhi execute
NAHI hua** (board user ke paas hai; run wohi karega — A7 gate).
Sandbox mein verify hua: secure-build config files, partition table,
signing flow, tools ke exact commands, key fingerprints (neeche §0).
**eFuse burn IRREVERSIBLE hai — ghalti = brick. Is doc ke order se
hatna mana hai.**

**Scope:** SEC-H-01 (OPEN-3 / SEC-E-01 ka asal fix — NVS plaintext
password + device key ka flash-dump se bachao), SEC-H-04 (OPEN-19
key-compromise response, §7). Companion docs: `docs/KEY-LIFECYCLE.md`
§2/§9/§10, `docs/SIGNED-OTA.md`, `docs/SECURITY-REVIEW-PHASE-F-W1.md`
§6 (ye procedure us §6 ka executable version hai — wahi steps, exact
commands + checks ke saath), `docs/THREAT-MODEL.md` TM-W07.

## 0. TL;DR — kya hai, kya nahi

- **Asal fix = hardware:** flash encryption (release mode) + encrypted
  NVS. Is ke baad `read_flash` dump se sirf ciphertext milta hai —
  `devkey` + `wifipw2` + password plaintext mein kabhi nahi milte.
- **Phase F obfuscation (`wifipw2`, `pw_obfuscate.h`) encryption NAHI
  thi** — wo rehti hai (defense in depth, pre-encryption migration
  path ke liye), lekin asal hal ye procedure hai.
- **Sandbox mein kya hua (verify hua):**
  - Release Ed25519 signing keypair bani + public key firmware mein
    bake hui + `firmware-signed.bin` build/verify hua (SIGNED-OTA.md §5).
  - Secure Boot RSA-3072 keypair bani (neeche fingerprints).
  - Secure-build files: `firmware/rsh/partitions-secure.csv`
    (gen_esp32part.py round-trip verify), `sdkconfig.secure.defaults`,
    `[env:esp32dev-secure]` (platformio.ini), `secure_nvs.h` runtime
    fail-closed check (normal build mein no-op — `pio run` SUCCESS).
  - Tool commands exact verify hue (esptool v4.5.1, `--help` + `--virt`
    summary se — eFuse naam + options confirm).
- **Sandbox mein kya NAHI hua (board-only):** secure-env build
  (IDF source build, lamba), eFuse burn, pehla encrypted boot, dump
  verification, pairing/OTA re-test. Ye sab sacrificial board par §1–§5
  ke mutabiq hoga.
- **Keys (fingerprints — private keys kabhi yahan nahi):**
  - Ed25519 release signing key — public-key SHA-256:
    `387764a4529337c437b233a148b48f9b2ffe3f3b5015e14c95737cb3a67a6411`
  - Secure Boot RSA-3072 key — SBV2 public-key digest (eFuse mein
    burn hoga): `1bb10ab7aef15449d89a930d678b25e8fe4673857aa69b9a82cd07b27bf551e9`
  - Private keys: `~/workspace/projects/rsh/signing-keys/` (repo se
    bahar, 0600) — `signing-key.pem`, `secure-boot-key.pem`. Backup
    offline lazmi hai (us dir ki README dekho).

## 1. Preconditions — in ke baghair aage mat barho

1. **Sacrificial board** — kabhi user ka / production board pehle nahi.
   Us par maujooda (v4.0.0, normal build) firmware flash karo; pairing
   + SET_PASSWORD + KEY_RESET kaam karte hue verify karo (baseline).
   Baseline fail ho to STOP — procedure ki ghalti aur pehle se tooti
   board mein farq nahi kar paoge.
2. **Chip revision check (STOP condition):** Secure Boot V2 ESP32 par
   sirf **ECO3 (chip revision 3+)** par hai (ESP-IDF docs — Secure Boot
   V2 page). Check karo:
   ```bash
   esptool.py --port PORT chip_id
   espefuse.py --port PORT summary | grep -i "CHIP_VER\|WAFER_VERSION"
   ```
   - Rev ≥ 3 → poora procedure (Secure Boot V2 + flash encryption).
   - Rev < 3 → **Secure Boot V2 STOP.** Flash encryption (release mode)
     akela phir bhi ho sakta hai (§4) — NVS-dump ka asal khatra band
     hota hai (dump sirf ciphertext hai) — lekin boot-integrity ke
     baghair residual rehta hai (TM-W07 partial): Secure Boot ke baghair
     bootloader verify nahi hota — physical access wala hamlaawar flash
     chip nikaal kar / dobara likh kar malicious bootloader chala kar
     eFuse key se decrypted contents nikal sakta hai (UART download
     release mode mein band hai, is liye serial se nayi image nahi
     aa sakti — residual sirf chip-level physical attack ka hai).
     Is surat mein user ko ECO3+ board lena chahiye (full posture ke
     liye) aur ye decision release notes mein darj karo.
3. **Tools:** esptool v4.5.1 (PlatformIO `tool-esptoolpy` — `esptool.py`,
   `espefuse.py`, `espsecure.py` usi package mein hain), openssl 3.x,
   PlatformIO venv (`~/workspace/projects/rsh/build/.venv-pio`).
   `espefuse.py` ko `cryptography`, `ecdsa`, `bitstring==4.2.3`,
   `reedsolo`, `intelhex` chahiye (sandbox venv mein install hue —
   release machine par `pip install` karo agar missing hon).
4. **Keys haazir hon:** `signing-keys/signing-key.pem` (Ed25519),
   `signing-keys/secure-boot-key.pem` (RSA-3072). Fingerprints §0 se
   milao — ghalat key = brick.
5. **Backup:** sacrificial board ki maujooda flash ka dump lo
   (`esptool.py --port PORT read_flash 0x0 0x400000 backup.bin`) —
   brick recovery ke liye nahi (eFuse wapas nahi hota), sirf baseline
   analysis ke liye.
6. **Time + power:** pehle encrypted boot ke doran power NAHI jani
   chahiye (neeche §4 — production boards par pre-encrypt flow is
   window ko band karta hai). Sacrificial board par stable USB power.

## 2. Step 0 — Secure build (kisi bhi irreversible step se pehle)

```bash
cd ~/workspace/projects/rsh/repo/firmware/rsh
# Secure-boot private key ko build dir mein copy karo (gitignored *.pem)
cp ~/workspace/projects/rsh/signing-keys/secure-boot-key.pem ./secure-boot-key.pem
chmod 600 ./secure-boot-key.pem
# Ed25519 OTA signing key env var se (sign_firmware.py)
RSH_SIGNING_KEY=$HOME/workspace/projects/rsh/signing-keys/signing-key.pem \
  pio run -e esp32dev-secure
# Build ke baad private key wapas mita do:
rm -f ./secure-boot-key.pem
```

**Verify (FAIL ho to STOP — eFuse ko haath mat lagao):**
- Build SUCCESS; `firmware-signed.bin` bana (size = firmware.bin + 68,
  trailer `RSHS`); `sign-image.py --verify` OK.
- Generated sdkconfig mein flags hon (build dir mein
  `.pio/build/esp32dev-secure/sdkconfig`):
  `CONFIG_SECURE_BOOT_V2_ENABLED=y`,
  `CONFIG_SECURE_FLASH_ENCRYPTION_MODE_RELEASE=y`,
  `CONFIG_NVS_ENCRYPTION=y`, `CONFIG_ESP32_REV_MIN_3=y`.
- `bootloader.bin` + `partitions.bin` secure env wale hon (nayi
  partition table — `nvs_keys` partition maujood ho; verify:
  `gen_esp32part.py .pio/build/esp32dev-secure/partitions.bin`).
- Merged factory bin (`RSH-Firmware-DevKitV1-v4.0.0.bin`) bana ho.

**Ye build sandbox mein verify NAHI hua** (IDF source build lamba hai;
procedure ka yehi step us ka build check hai). Agar build fail ho to
`platformio.ini` env / `sdkconfig.secure.defaults` theek karo — eFuse
burn tab tak bilkul nahi.

## 3. Step 1 — Secure Boot V2 eFuse burn (IRREVERSIBLE)

Sirf §2 pass hone + §1 revision check (ECO3+) ke baad. Board ROM
download mode mein ho (BOOT hold kar ke connect).

```bash
PORT=/dev/ttyUSB0   # apna port likho
# 1. Pehle digest dobara nikalo aur §0 fingerprint se milao:
espsecure.py digest_sbv2_public_key --keyfile secure-boot-key.pem --output sbv2-digest.bin
sha256sum sbv2-digest.bin
#    Expected: 1bb10ab7aef15449d89a930d678b25e8fe4673857aa69b9a82cd07b27bf551e9
#    Mismatch ho to STOP — ghalat key hai.
# 2. Digest burn karo (BLOCK_KEY2, default write-protected):
espefuse.py --port $PORT burn_key_digest secure-boot-key.pem
#    (ye command khud digest banati + burn karti hai; upar wala step sirf
#    pre-check hai.)
# 3. Verify:
espefuse.py --port $PORT summary | grep -E "ABS_DONE_1|SECURE_BOOT"
```

- `ABS_DONE_1` = True hona chahiye (Secure Boot V2 enabled).
- **Ghalat key burn ho gayi to board us key ke baghair kabhi boot nahi
  hoga** — is liye step 1 ka fingerprint check lazmi hai.
- Secure Boot key revoke: ESP32 V2 mein 3 key digests tak (revoke bits
  se) — compromise response §7 dekho.

## 4. Step 2 — Flash encryption (release mode)

Do flows hain — sacrificial trial ke liye **A** (device-generated key,
pehla boot encrypt karta hai), production boards ke liye **B**
(host-generated key + pre-encrypt — pehle-boot power-loss window band).

### Flow A — sacrificial board (device-generated key)

```bash
# Secure-build merged image flash karo (pehli dafa, plaintext — abhi
# tak encryption on nahi, is liye plaintext flash allowed hai):
esptool.py --port $PORT write_flash 0x0 RSH-Firmware-DevKitV1-v4.0.0.bin
# Pehla boot: bootloader flash-encryption key generate karta hai (eFuse,
# read-protected), poori flash encrypt karta hai, FLASH_CRYPT_CNT set
# karta hai, aur UART download band kar deta hai (release mode).
# Boot ke doran power NA JAYE — 1-2 minute lag sakte hain.
# Boot log mein "flash encryption" + "Flash encryption mode is RELEASE"
# jaisi lines aani chahiye.
```

### Flow B — production boards (host-generated key + pre-encrypt)

```bash
espsecure.py generate_flash_encryption_key --keylen 256 flash-encryption-key.bin
# Key PER BOARD alag honi chahiye — kabhi boards ke darmiyan share nahi.
# Key file ko per-board offline backup mein rakho (eFuse read-protected
# hone ke baad key wapas nahi milti; re-flash ke liye chahiye hogi).
espefuse.py --port $PORT burn_key flash_encryption flash-encryption-key.bin
espefuse.py --port $PORT burn_efuse FLASH_CRYPT_CONFIG 0xF
# Images pre-encrypt karo (har bin apne flash address ke saath):
espsecure.py encrypt_flash_data --keyfile flash-encryption-key.bin \
  --address 0x0 --output merged-encrypted.bin RSH-Firmware-DevKitV1-v4.0.0.bin
esptool.py --port $PORT write_flash 0x0 merged-encrypted.bin
```

**Verify (§4 dono flows):**
```bash
espefuse.py --port $PORT summary | grep -E "FLASH_CRYPT_CNT|FLASH_CRYPT_CONFIG"
# FLASH_CRYPT_CNT odd (0x01...) hona chahiye = encryption enabled.
# FLASH_CRYPT_CONFIG = 0xF.
esptool.py --port $PORT read_flash 0x0 0x400000 dump.bin
strings dump.bin | grep -i -E "rsh|password|devkey"   # kuch nahi milna chahiye
# Dump ciphertext hona chahiye — plaintext strings (version marker
# "Version <b>" samet) nazar NAHI aane chahiye.
```

## 5. Step 3 — Encrypted NVS verify (sacrificial board par)

1. **Boot + AP:** board boot hota hai, AP up hota hai (15 s ke andar).
   `secure_nvs.h` check (flash encryption on + nvs_keys partition) pass
   hota hai — fail hota to board Error LED par halt hota (fail closed).
2. **Pairing (60 s window):** pehli HELLO par key + password milte hain;
   GET_CAPS HMAC OK.
3. **NVS dump check:** `esptool.py read_flash` se NVS region
   (0x9000, 0x5000) + `nvs_keys` (0x3EF000, 0x1000) dump karo —
   `devkey` / `wifipw2` / password plaintext mein **nahi** milte.
4. **SET_PASSWORD:** naya password set karo → AP + WebOTA naye password
   se kaam karte hain; reboot ke baad barqarar.
5. **KEY_RESET:** BOOT-hold + KEY_RESET → nayi key + naya password,
   dobara pairing.
6. **OTA:** signed image (is release key se) upload → OK, naya version
   GET_CAPS se confirm. Unsigned image → reject (purana firmware
   chalta rehta hai).
7. **Reboot persistence:** reboot ke baad key/password barqarar hain
   (NVS encryption keys eFuse-derived hain, erase nahi hoti).
8. **NVS migration (purana plaintext → obfuscated):** is procedure mein
   boards **erase + dobara flash** hote hain (§6), is liye purani
   plaintext `wifipw` entry ka migration board par nahi hota — pehla
   boot nayi key + naya password banata hai. (`wifipw` → `wifipw2`
   migration code `keystore.cpp` mein rehta hai — agar kisi board par
   purani NVS bach jaye to wo migrate ho jati hai; procedure mein is
   par bharosa nahi.)

Sab 8 pass hon to sacrificial board **PASS** — tab hi production boards
par §3–§4 dohrayo (har board apni flash-encryption key ke saath — keys
kabhi share nahi hoti).

## 6. Migration note — pehle se flashed boards

- Bina encryption ke flashed boards par flash encryption **in-place
  enable nahi hoti** — unhein **erase + dobara flash + dobara pairing**
  karna hoga (nayi key + naya password). Purani pairing / purana
  password khatam.
- Release notes mein user-facing tareeqa likhna hoga (USER-GUIDE.md +
  release notes): "v2.0.0 secure release ke liye board erase + USB
  reflash + dobara pairing lazmi hai — purani pairing kaam nahi karegi."
- OTA se encryption enable nahi hoti (OTA app partition hi badalta hai;
  bootloader/eFuse/partition table nahi). Secure release mein aana =
  USB flash.

## 7. OPEN-19 — Key-compromise response (SEC-H-04)

### 7.1 Ed25519 OTA signing key compromise (ya shak)

1. **Foran:** nayi release band karo (koi nayi signed image mat banao).
2. **Naya keypair** banao (`sign-image.py --gen-keypair` — nayi dir),
   fingerprint record karo.
3. **`signing_pubkey.h`** nayi public key se badlo, rebuild karo.
4. **Har board USB se reflash karo** — OTA se nahi (purani key se
   signed image hi OTA ho sakti hai; attacker ke paas purani key hai to
   wo bhi "valid" signed image bhej sakta hai — is liye OTA rasta band
   samjho jab tak nayi pubkey wali firmware USB se na lag jaye).
5. **Purani key ko revoked samjho** — us se signed koi bhi image ab
   trust nahi (nayi pubkey wali firmware use reject karti hai).
6. **Users ko batao:** release notes + USER-GUIDE mein — kaun se
   version se nayi key hai, USB reflash lazmi hai, purani key se signed
   images install mat karo.
7. **Purani private key wipe karo** (`shred -u`) aur compromise ka
   record likho (kab pata chala, kya expose hua, kaun se boards
   mutasir).

### 7.2 Secure Boot RSA-3072 key compromise (ya shak)

1. **Naya Secure Boot keypair** banao (RSA-3072, `--version 2`).
2. **eFuse key revoke:** ESP32 Secure Boot V2 mein 3 key digests tak
   hote hain; compromised digest ko revoke bits se revoke karo
   (`espefuse.py burn_efuse SECURE_BOOT_KEY_REVOKE<n>` — exact efuse
   naam `espefuse.py summary` se confirm karo; revoke IRREVERSIBLE hai).
3. **Naya digest burn karo** (khali slot mein) — nayi key se signed
   bootloader + app dobara flash karo (USB).
4. **Jo boards revoke nahi kar sakte** (slots khatam / ghalat procedure):
   wo boards compromised key se signed image boot karte rahenge — unhein
   replace karna hoga. Ye release notes mein saaf likho.
5. Agar Secure Boot key + flash-encryption key **dono** compromise hon
   to board ki poori trust toot gayi — board replace karo (eFuse keys
   wapas nahi hoti).

### 7.3 Flash-encryption key compromise

- Per-board key hai — **sirf us board** mutasir hota hai (dusre boards
  ki keys alag hain — isi liye keys share nahi karte, §4 Flow B).
- Mutasir board: erase + nayi key + dobara flash + dobara pairing
  (eFuse mein nayi key burn nahi ho sakti — BLOCK_KEY0 ek dafa hi
  burn hota hai — is liye **board replace** hi asal hal hai agar
  attacker ke paas key + physical access dono hon; erase/reflash
  sirf tab kaafi hai jab key sirf backup se leak hui ho aur board
  attacker ke haath na laga ho). Faisla case-by-case, record karo.

### 7.4 Signing key kho jaye (compromise nahi, loss)

- Backup nahi hai to purani key se signed images dobara sign nahi ho
  sakti — **naya keypair + har board USB reflash** (§7.1 jaisa, lekin
  urgency kam — koi attacker nahi, sirf rotation).
- Is liye offline backup (§0) lazmi hai — loss bhi compromise jaisa
  mehenga hai.

## 8. SEC-E-01 / OPEN-3 closure criteria

OPEN-3 **CLOSED** tab hoga jab:
- [ ] Sacrificial board §5 ke 8 checks pass kare (dump mein plaintext nahi).
- [ ] Production boards par §3–§4 ho chuka ho.
- [ ] Release notes + USER-GUIDE mein §6 migration note likhi ho.
- [ ] Us ke baad docs update hon: `KEY-LIFECYCLE.md` §2/§8/§10 (OPEN-3
      CLOSED), `PROTOCOL-V2.1.md` §4.6/§7 (OPEN-3 CLOSED), `THREAT-MODEL.md`
      TM-W07 (residual se band), `SIGNED-OTA.md` §7 (OPEN-3 hatao).
- [ ] `keystore.h` note + `pw_obfuscate.h` note update hon (obfuscation
      ab defense-in-depth hai, asal hal flash encryption hai).

Tab tak OPEN-3 **khula** hai aur production release **blocker** hai —
sandbox mein koi bhi code/doc change isay band nahi karta.

## 9. Checklist (run ke waqt tick karo)

- [ ] §1 preconditions (sacrificial board, baseline PASS, rev check, tools, key fingerprints, backup dump)
- [ ] §2 secure build SUCCESS + sdkconfig verify + signed-bin verify
- [ ] §3 Secure Boot digest burn (fingerprint match) + summary verify (ABS_DONE_1)
- [ ] §4 flash encryption (Flow A ya B) + summary verify (FLASH_CRYPT_CNT odd, CONFIG 0xF) + dump `strings` clean
- [ ] §5 verify 1–8 (boot/AP, pairing, NVS dump, SET_PASSWORD, KEY_RESET, OTA signed/unsigned, reboot, migration)
- [ ] §6 migration note release notes + USER-GUIDE mein
- [ ] §8 closure docs update (sirf §5 pass ke baad)

---

*Sandbox verification notes (Phase H W1, 2026-11-13): partition table
`gen_esp32part.py` round-trip PASS; `espefuse.py`/`espsecure.py` exact
commands `--help` + `--virt summary` se verify hue (esptool v4.5.1);
key fingerprints §0 mein; secure-env build + board steps board-only
hain (board user ke paas hai). Koi git command nahi chalaya gaya.*
