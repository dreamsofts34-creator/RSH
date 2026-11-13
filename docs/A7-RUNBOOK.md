# A7 runbook — real-board measured run (user step-by-step)

> **Version 2.0.0 — app v2.0.0 · firmware v4.0.0 · protocol v2.1**
> Authority: `docs/A7-GATE-PROCEDURE.md` (procedure + pass criteria),
> Plan v2.3 §9 (freeze gate), `docs/PERF-BASELINE.md` §B.
> **Ye run aap khud karenge — is doc mein koi board result pehle se darj
> nahi hai.** Neeche har step, har command aur har expected number likha
> hai. Run ke baad result isi doc ke template mein record karein.

## 0. Kya hai A7 — aur kyun zaroori hai

A7 wo measured run hai jis ke baad hi plan "freeze" hota hai (Plan v2.3
§9): asal ESP32 board par 10 minute tak firmware chalao, serial log se
numbers nikalo, aur pass criteria se milao. Virtual Test Rig (jo ab tak
har test ke liye use hua) real ADC, real Wi-Fi aur real board timing ko
simulate karta hai — asal board ko nahi. Is liye A7 ko rig se pass nahi
kiya ja sakta; ye sirf board par hota hai.

**Pass hone par:** freeze + final release ki taraf. **Fail hone par:**
neeche §6 dekhein — fix karo, dobara run karo; release tab tak nahi.

## 1. Cheezein jo chahiye (checklist)

- [ ] ESP32 DevKit V1 board + **data** USB cable (charge-only cable se COM
      port kabhi nahi aata).
- [ ] PC (Windows 11, 64-bit) — RSH app v2.0.0 installed.
- [ ] Firmware v4.0.0 — merged factory bin:
      `RSH-Firmware-DevKitV1-v4.0.0.bin`
      (GitHub Release v2.0.0 se, ya local build se:
      `firmware/rsh/.pio/build/esp32dev/RSH-Firmware-DevKitV1-v4.0.0.bin`).
- [ ] `esptool.py` (PlatformIO ke saath aata hai) ya web flasher
      (Chrome/Edge — `docs/FLASH-GUIDE.md` dekhein).
- [ ] Serial monitor — 115200 baud (PlatformIO monitor ya koi bhi serial
      terminal).
- [ ] Wheel ke pot/pedal wired hon (steering move kar sakein — std-dev
      record karne ke liye steering ko run ke doran hilana hai, §4 dekhein).
- [ ] 10 minute ka waqt + log file save karne ki jagah.

## 2. Board flash karein (debug build)

Run ke liye firmware mein debug stats ON karne hote hain. Ye sirf run ke
liye hai — run ke baad wapas OFF karna hai (§5).

1. `firmware/rsh/config.h` kholein. Line 175 ke aas-paas ye milega:
   ```c
   #define RSH_DEBUG 0
   ```
   Ise badal kar `1` karein:
   ```c
   #define RSH_DEBUG 1
   ```
2. `FIRMWARE_VERSION` check karein (line 134) — is mein `"4.0.0"` hona
   chahiye:
   ```c
   #define FIRMWARE_VERSION "4.0.0"
   ```
3. Board ko USB se connect karein. COM port note karein (Windows:
   Device Manager → *Ports (COM & LPT)* — masalan `COM3`. Linux:
   `/dev/ttyUSB0`).
4. Flash karein — **in mein se ek tareeqa**:
   - PlatformIO (terminal, `firmware/rsh/` folder mein):
     ```bash
     pio run -t upload
     ```
   - Ya pehle se bani merged bin se (esptool — `<PORT>` apna daalein):
     ```bash
     esptool.py --chip esp32 --port COM3 --baud 921600 write_flash 0x0 RSH-Firmware-DevKitV1-v4.0.0.bin
     ```
     Linux par port `/dev/ttyUSB0` hota hai. Baud fail ho to `115200`
     try karein. Offset **0x0** hona chahiye — `firmware.bin` (app-only)
     se recovery/flash nahi hoti.
   - Ya web flasher se — `docs/FLASH-GUIDE.md` §Steps.
5. Flash ke baad EN/RST dabayein (ya USB dobara lagayein).

**LED check (flash ke foran baad, PC connect karne se pehle):** onboard
status LED (GPIO2) ko **2 Hz blink** karna chahiye (250 ms on / 250 ms
off) — matlab AP up hai, PC ka intezar hai. LED bilkul off ho ya tez
blink (8 Hz = error) to §6 dekhein.

## 3. App connect karein

1. PC ko board ke Wi-Fi network **`RSH`** se connect karein.
   (Pairing ke baad AP WPA2 hota hai — password pehli pairing par app ne
   dikhaya tha. Password bhool gaye to KEY_RESET: BOOT ≥ 10 s hold,
   phir dobara pairing — `docs/FLASH-GUIDE.md`.)
2. RSH app v2.0.0 kholein. Dashboard par packets aane chahiye
   (~200 packets/sec). **Data tabhi banta hai jab PC active ho** (app
   HELO bhejti hai) — app band ho to board bhejta nahi.
3. LED dobara check karein: PC connect hone par status LED **solid on**
   honi chahiye (blink band). Connect ke baad bhi blink = FAIL — log
   mein likhein.

## 4. 10-minute run (serial monitor)

1. Serial monitor kholein — **115200 baud**:
   ```bash
   pio run -t monitor
   ```
   (Ya apna serial terminal 115200 baud par, wahi COM port.)
2. **10 minute** tak chalao. Har 1 second mein ek line aati hai:
   ```text
   sent/s=... seq=... overrun=... maxCycleUs=... steer min=... max=... mean=... std=... mv=...
   ```
3. **Steering ko run ke doran hilate rahein** (aahista, poori range mein
   kuch dafa) — warna std-dev 0 aayega aur baseline bekaar hogi.
4. Poora serial output log file mein save karein — naam:
   `a7-run-YYYY-MM-DD.log` (masalan `a7-run-2026-11-13.log`).
   PlatformIO monitor mein log save karne ke liye terminal output ko
   file mein copy karein; agar monitor log-to-file support karta hai to
   use karein.
5. Run ke doran board ko chhedein nahi (USB na hilayein, power na katrein).

### Expected numbers (har line mein)

| Field | Expected |
|---|---|
| `sent/s` | ≈ 200 (195–205) |
| `overrun` | 0 (har line mein) |
| `maxCycleUs` | < 3500 µs |
| `steer std` | record karein (koi threshold nahi — baseline ke liye) |

## 5. Run ke baad — debug wapas OFF + result record karein

1. `config.h` mein `RSH_DEBUG` wapas `0` karein aur dobara flash karein
   (release build mein debug OFF rehta hai):
   ```bash
   pio run -t upload
   ```
   Ya merged release bin dobara flash karein (`write_flash 0x0
   RSH-Firmware-DevKitV1-v4.0.0.bin`).
2. Neeche template bhar kar log file ke saath mehfooz rakhein:

```text
A7 run — date: ________  firmware: v4.0.0  app: v2.0.0  board: ESP32 DevKit V1
sent/s:  min ___  max ___  avg ___   (pass: ≈200, 195–205)
overruns (kul 10 min): ___          (pass: 0)
maxCycleUs (kisi bhi window mein): ___ µs   (pass: < 3500)
steering std-dev: ___               (record only)
steering min: ___  max: ___  mean: ___  mv: ___
reboots: ___  (pass: 0)
LED — status (GPIO2): boot 2 Hz [OK/FAIL] → PC connect solid [OK/FAIL]
LED — strip (GPIO21): SOLID [OK/FAIL] · BLINK [OK/FAIL] · CHASE [OK/FAIL] · RAINBOW [OK/FAIL] · OFF [OK/FAIL]
LED — bad patternId 9 → UNSUPPORTED [OK/FAIL] · test ke baad purani state [OK/FAIL]
LED driver maujood: [haan / nahi — nahi ho to strip step UNSUPPORTED expected, PASS nahi]
OTA — unsigned upload → reject [OK/FAIL] · signed upload → accept + version confirm [OK/FAIL] · downgrade → reject [OK/FAIL]
Flash dump — Wi-Fi password plaintext mein [mila / nahi mila] · flash encryption [enabled / nahi — OPEN-3 pending]
NVS migration — purana password upgrade ke baad bhi chalta hai [OK/FAIL/N/A]
RESULT: PASS / FAIL
log file: a7-run-<date>.log
```

**LED strip step ke baare mein:** strip test (LED_TEST patterns) sirf
tab PASS ho sakta hai jab firmware mein WS2812 driver laga ho. Driver
na ho to strip step ka expected result `UNSUPPORTED` hai (fail-closed
behaviour — bug nahi) aur use PASS ke tor par darj **mat** karein.
Detail: `docs/A7-GATE-PROCEDURE.md` §B.

## 6. Board-only security checks (B5 — final release se pehle lazmi)

Ye teeno checks sirf board par hote hain (sandbox mein verify nahi ho
sakte) aur final release se pehle lazmi hain. In ke baghair A7 PASS nahi.

### 6a. Signed OTA — unsigned reject, signed accept, downgrade reject

Background: sirf signed official release upload hoti hai — board image ki
Ed25519 signature verify karta hai (68 B trailer: 64 B signature +
`"RSHS"`, `docs/SIGNED-OTA.md`), aur app upload se pehle gate karti hai
(`FirmwareUploadGate` — unsigned = reject, downgrade = reject, upload ke
baad GET_CAPS se version confirm lazmi). Ye test hi prove karta hai ke
trailer + `Update.end()` real board par saath chalte hain (SIGNED-OTA §7
residual risk).

**Pehle ye samjhein — kaunsi image kaunsi hai:**

| Image | Kahan se | Signed? |
|---|---|---|
| `firmware.bin` (local `pio run`, bina key ke) | `.pio/build/esp32dev/firmware.bin` | **Nahi** — `RSH_SIGNING_KEY` env var na ho to `sign_firmware.py` skip karta hai aur build unsigned rehta hai (jaan-boojh kar — unsigned OTA reject honi chahiye) |
| `firmware-signed.bin` | `.pio/build/esp32dev/firmware-signed.bin` (sirf jab key set ho — size = `firmware.bin` + 68 B, aakhir mein `"RSHS"`) | **Haan** — lekin sirf us key se jo flashed firmware ke `signing_pubkey.h` mein hai |
| Release image | GitHub Release v2.0.0 | **Haan** — release key se (Phase H offline keypair) |

**Procedure (10-min run ke baad, isi board par):**

1. **Unsigned → reject.** Local unsigned `firmware.bin` (bina key ke
   `pio run` se bana) ko app wizard ke Firmware step se upload karne ki
   koshish karein. Expected: app foran reject kare ("Image signed nahi
   hai — sirf signed official RSH release upload karo") aur board par
   kuch nahi badle — board abhi bhi v4.0.0 chalaye (GET_CAPS se check
   karein). Agar app ne upload bhej bhi di to board reject kare.
   Upload kamyab ho jaye to **FAIL** — ye critical hai.
2. **Signed → accept.** Signed image (`firmware-signed.bin` — release
   image, ya dev keypair se signed: `sign-image.py --gen-keypair` +
   pubkey header rebuild + sign, `sign_firmware.py` header dekhein)
   upload karein — app wizard se ya board ke update page
   (`192.168.4.1`, login `admin` + per-device password) se. Expected:
   upload accept ho, board restart ho, aur app GET_CAPS se naya version
   confirm kare (version == image version). Confirm na ho to update
   kamyab **nahi** mani jati — dobara koshish karein.
   > **Note:** local build dir mein pehle se ek `firmware-signed.bin`
   > maujood ho sakta hai (kisi purani dev key se signed). Use andha
   > istemal mat karein — agar us ki key flashed firmware ke pubkey se
   > match nahi karti to board reject karega (ye sahi behaviour hai).
   > Signed test ke liye wahi image istemal karein jo maujooda flashed
   > firmware ki key se signed ho.
3. **Downgrade → reject.** Purani version wali image (masalan v3.3-era
   ya v4.0.0 se kam version) upload karne ki koshish karein. Expected:
   reject — app: "Downgrade reject", board: `downgrade reject (purani
   version)` (`webota.cpp`). Barabar version = reinstall (allowed).
   Downgrade accept ho jaye to **FAIL**.

### 6b. Flash-encryption verify — dump mein password/devkey plaintext na mile

**Honest status (pehle parhein):** flash encryption abhi **enabled nahi
hai** (OPEN-3 — Phase H mein khula hai; `platformio.ini` mein koi
`CONFIG_SECURE_FLASH_ENC` nahi, koi `sdkconfig` nahi). Is liye NVS
entry names (`devkey`, `wifipw2`) dump mein nazar aana **expected** hai,
aur `wifipw2` sirf obfuscated hai (device key se XOR — encryption nahi,
`keystore.cpp`/`pw_obfuscate.h`). Asal hal flash encryption + encrypted
NVS hai (OPEN-3). Is check ka matlab ye hai:

1. Board ko USB se connect karein, download mode mein layein (BOOT hold
   + EN/RST — `docs/BRICK-RECOVERY.md` step 2).
2. NVS partition dump karein (partition table `partitions.bin` se:
   `nvs` offset `0x9000`, size `0x5000`):
   ```bash
   esptool.py --chip esp32 --port COM3 --baud 921600 read_flash 0x9000 0x5000 nvs-dump.bin
   ```
   (Poora flash dump chahiye ho to: `read_flash 0x0 0x400000 full-dump.bin`.)
3. Apna Wi-Fi password dump mein dhoondein (password ko `<PW>` ki jagah
   likhein — is command ko kisi ko bhejein nahi, log mein password mat likhein):
   ```bash
   strings nvs-dump.bin | grep -i "<PW>"
   ```
   **Expected: kuch nahi mile** (password plaintext mein nahi hona
   chahiye — obfuscated `wifipw2` bytes mein password readable nahi
   hota). Password mil jaye to **FAIL** (obfuscation tooti hui hai).
4. `strings nvs-dump.bin | grep -i "devkey"` — entry ka **naam**
   milna pre-OPEN-3 expected hai (NVS names plaintext hote hain); is
   ka matlab ye nahi ke key leak hui (devkey 32 random bytes hai —
   readable text nahi). Record karein: `devkey` label [mila / nahi mila].
5. **OPEN-3 ke baad (agar A7 se pehle flash encryption enable ho jaye):**
   poora dump unreadable hona chahiye — `strings` mein na password, na
   `devkey`/`wifipw2` labels, na koi readable NVS entry. Us surat mein
   ye check "flash encryption enabled" ke tor par PASS hota hai; warna
   (aaj ki surat) isay **"OPEN-3 pending — residual risk TM-W07"** ke
   tor par darj karein, PASS ke tor par nahi.

### 6c. NVS migration check — purana password upgrade ke baad bhi chale

Background: purane firmware mein Wi-Fi password plaintext NVS entry
`wifipw` mein tha; v4.0.0 mein `wifipw2` (obfuscated) hai. Upgrade par
board purani entry ko parh kar nayi mein migrate karta hai
(`keystore.cpp` — pehle `wifipw2`, phir legacy `wifipw` + foran migrate).
**Is ki koi host-test coverage nahi hai — ye check sirf board par hota hai.**

Procedure (sirf tab jab board pehle purane firmware — v3.3-era, plaintext
`wifipw` — par chal raha tha; naye board par N/A likhein):

1. Purane firmware par board ka Wi-Fi password note karein (app mein
   saved hai).
2. Signed OTA se v4.0.0 par upgrade karein (§6a step 2).
3. Board restart ke baad **wahi purana password** se Wi-Fi connect karein
   (app ya phone se). Expected: connect ho jaye — migration kamyab.
   Password kaam na kare to **FAIL** (migration tooti — KEY_RESET +
   dobara pairing se recover karein, phir dobara test karein).
4. (Optional, dump se) `strings nvs-dump.bin | grep -i "wifipw"` mein
   sirf `wifipw2` hona chahiye — legacy `wifipw` entry migrate ke baad
   nahi rehni chahiye.

## 7. Fail ho to kya karein

| Masla | Kya karein |
|---|---|
| `sent/s` < 195 | Wi-Fi/channel ya governor level check karein. Governor level 3 par 100 pps fallback expected hai — is surat mein run dobara karein, governor level note karein. |
| Overruns > 0 | `maxCycleUs` dekhein — kaunsa hissa lamba hai (net/OTA/ADC). Fix + dobara run. |
| `maxCycleUs` ≥ 3500 | Loop timing analyse karein, fix karein, dobara run karein. |
| Board reboot (run ke doran) | FAIL. `docs/BRICK-RECOVERY.md` se recover karein, phir dobara run karein. |
| LED pattern ghalat (connect ke baad bhi blink, ya LED off) | FAIL — `firmware/rsh/status_led.cpp` / `net.cpp` state machine dekhein. |
| Board boot hi nahi hota / RSH Wi-Fi nahi aata | **Brick recovery:** `docs/BRICK-RECOVERY.md` — BOOT hold + EN/RST, phir `esptool.py --chip esp32 --port <PORT> --baud 921600 write_flash 0x0 RSH-Firmware-DevKitV1-v4.0.0.bin`. Aakhri rasta: `esptool.py erase_flash` phir dobara flash karein. |
| COM port nazar nahi aata | Data cable badlein → driver install karein (CP210x/CH340) → doosra USB port try karein. |
| "Failed to connect" (flash ke waqt) | BOOT-button trick + baud 115200. |
| Unsigned OTA upload accept ho jaye | **CRITICAL FAIL** — release roko. Signing key/pubkey header check karo (`signing_pubkey.h` vs sign karne wali key). |
| Signed OTA reject ho jaye | Key mismatch (image kisi aur key se signed) ya trailer kharab — sahi key se dobara sign karo, phir test karo. |
| Downgrade accept ho jaye | **FAIL** — anti-downgrade toota hai (`ota_validate.h` / `webota.cpp` + `FirmwareUploadGate`). |
| Dump mein Wi-Fi password plaintext mile | **FAIL** — obfuscation tooti hai (`keystore.cpp` / `pw_obfuscate.h`). |
| NVS migration ke baad purana password kaam na kare | **FAIL** — KEY_RESET + dobara pairing se recover karo, phir dobara test karo. |

## 8. Freeze rule

Freeze sirf A7 **PASS** ke baad (Plan v2.3 §9). Freeze ke baad bhi koi
"frozen" daawa nahi — har change ke liye naya version.

## 9. Mutaliqa docs

- `docs/A7-GATE-PROCEDURE.md` — procedure + pass criteria (authority)
- `docs/BRICK-RECOVERY.md` — brick recovery steps
- `docs/FLASH-GUIDE.md` — flashing (web flasher + OTA)
- `docs/SIGNED-OTA.md` — signed OTA format + key handling (OPEN-19)
- `docs/PERF-BASELINE.md` §B — baseline procedure
- `docs/USER-GUIDE.md` — rozana istemal
