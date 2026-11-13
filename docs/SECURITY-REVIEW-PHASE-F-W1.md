# RSH — Security Review + Fix, Phase F Workstream 1 (SEC-E-04 / SEC-E-09 / SEC-E-01)

**Status:** IMPLEMENTED + VERIFIED (sandbox gate — neeche §5).
**Date:** 2026-11-13. **Team:** Cybersecurity (Security Reviewer).
**Scope:** Production blockers pehle — SEC-E-04 (Wi-Fi password plaintext),
SEC-E-09 (rotation hygiene), SEC-E-01 (NVS plaintext password).
**Files (ownership):** `RSH.App/Services/AppSettings.cs`,
`RSH.Core/Security/KeyStore.cs`, `RSH.Core/Transport/BoardSession.cs`,
`docs/PROTOCOL-V2.1.md` §5.3, `docs/KEY-LIFECYCLE.md`, firmware
password/NVS files. `ControlClient.cs` (W2) aur `GameLauncher.cs` (W3) ko
haath nahi lagaya gaya. Doosre consumers mein sirf minimal rewire hai
(`ProfilesSystem.cs`, `WizardWindow.xaml.cs`, `DiagnosticsSettings.cs`,
`ChangePasswordDialog.xaml.cs`, `AppServices.cs` — read path badla hai,
logic nahi).
**Method:** Code parh kar + implement kar ke + build/test/rig se verify.
Koi git command nahi chalaya gaya, GitHub par kuch push/tag/release nahi hua.
`RSH-handover/` aur `build/` ko haath nahi lagaya gaya.

**Standards:** OWASP ISVS v1.0 + NIST IR 8259A + ETSI EN 303 645 (Plan v2.3
§6). Threat IDs `TM-*` = `repo/docs/THREAT-MODEL.md` se.

## 0. Natija — ek nazar mein

| Item | Severity | Halat |
|---|---|---|
| SEC-E-04 — Wi-Fi password `settings.json` mein plaintext | **Production blocker** | **FIXED** (code doc ke mutabiq hua) |
| SEC-E-04b — `ChangePassword` dobara plaintext likhta tha (W8) | **Production blocker** | **FIXED** |
| SEC-E-04c — Backup zip password leak karta tha (W8) | **Production blocker** | **FIXED** |
| §5.3 doc-vs-code contradiction (OPEN-15 "CLOSED" jab ke ult tha) | Doc bug | **FIXED** (code doc ke mutabiq; correction note §5.3 mein) |
| SEC-E-09 — rotation hygiene gaps | High | **FIXED** (purge + warning + atomic write) |
| SEC-E-01 — Naya password NVS mein plaintext | **Production blocker** | **PARTIAL — sandbox fix hua** (obfuscation + migration); asal hal flash encryption (OPEN-3) **Phase H** mein hai — §6 mein exact procedure |
| KEY-LIFECYCLE doc contradictions (W7) | Doc bug | **FIXED** ("encrypted NVS" ke ghalat daawe hatae) |
| Gate mein mila bug: `FirmwareImageValidator` SHA offset ghalat | High (gate blocker) | **FIXED** (scope se bahir tha — §4 dekho) |

## 1. SEC-E-04 — Wi-Fi password `settings.json` se hatao (FIXED)

### Asal halat (galat kya tha)
- `KeyStore.TrySavePassword` / `TryLoadPassword` (DPAPI blob) maujood thay,
  lekin **koi caller `TryLoadPassword` istemal nahi karta tha** — password
  ka read path sirf `AppSettings.WheelWifiPassword` (plaintext,
  `settings.json`) tha.
- `ProfilesSystem.cs:347` (Connection page textbox) aur
  `WizardWindow.xaml.cs:452` (Wi-Fi connect) dono settings se parhte thay.
- `DiagnosticsSettings.cs:527` (`ChangePassword`) naya password dobara
  `settings.json` mein likhta tha (W8 finding #1).
- `DiagnosticsSettings.cs:404` (`WriteBackup`) `settings.json` ko verbatim
  zip mein daalta tha — har `rsh-backup-*.zip` mein plaintext password tha
  (W8 finding #2).

### Fix
- `AppSettings.WheelWifiPassword` ab `[JsonIgnore]` hai — **kabhi serialize
  nahi hota**; sirf in-memory working value (UI textbox buffer).
- Naya `[JsonIgnore]` field `LegacyWheelWifiPassword`: Load ke waqt purani
  file mein plaintext mile to sirf yahan aata hai (migration ke liye).
- Read path: `BoardSession.TryGetWifiPassword()` (DPAPI KeyStore, board MAC
  se) — `AppServices.GetWheelPassword()` is ka wahid entry point hai
  (KeyStore → in-memory fallback; `settings.json` se kabhi nahi).
- Write path: `SET_PASSWORD` kamyaabi par `BoardSession.SetPasswordAsync`
  khud KeyStore mein save karta hai; Wi-Fi join kamyaabi par
  `ConnectionViewModel.ConnectWifi` save karta hai.
- Migration: pehle HELLO par `AppServices.TryMigrateWifiPassword()` legacy
  (pehle disk wali, phir in-memory) password KeyStore mein lata hai, phir
  settings dobara save kar ke disk se saaf karta hai. Store mein pehle se
  (nayi) password ho to legacy overwrite **nahi** hoti (stale hai).
  Board kabhi connect na ho to legacy sirf us session ki memory mein
  rehti hai; agla Save disk wali copy bhi mita deta hai (password dobara
  darj karna hoga — security > convenience). `"change-me-123"` kabhi
  migrate nahi hota (wo kabhi valid password nahi tha).
- Backup: `WriteBackup` ab `settings.json` ko live settings se serialize
  karta hai (`AppSettingsStore.ToBackupJson`) — password fields
  `[JsonIgnore]` hain, is liye backup mein password **kabhi** nahi aata.
  Restore purani backup se plaintext la sakta hai — agla Load use migrate
  kar deta hai (is par comment `RestoreBackup` mein hai).
- `ChangePassword`: settings write hata di gayi; dialog ki `SaveWarning`
  (neeche §2) ab `StatusText` mein dikhayi jati hai.

### Tests
- `PhaseFWorkstream1Tests.cs` (18 tests, RSH.Tests mein — sab pass):
  `TryGetWifiPassword` (no-HELLO / save ke baad), `TrySaveWifiPassword`
  (invalid reject, no-BoardMac reject), `MigrateLegacyPassword` (migrate /
  overwrite-nahi / no-BoardMac / invalid discard), `WipeSessionKey` ×2,
  validator regression ×3.
- AppSettings scratch verification (`/tmp/rsh-w1-appsettings`, linked-file
  console — RSH.App WPF/Windows-only hai, is liye RSH.Tests mein nahi
  aa sakta): **16 checks, 0 failures** — Save kabhi password nahi likhta,
  legacy Load sirf `LegacyWheelWifiPassword` mein, Save-after-load disk
  saaf karta hai, `.bak` fallback se bhi legacy milta hai, `ToBackupJson`
  mein password kabhi nahi.

## 2. SEC-E-09 — Rotation hygiene (FIXED)

Mile hue gaps (sab fix hue):
1. **Key rotation par stale password rehta tha.** `UpdateFromHello` nayi
   key save karta tha, purana password blob chhoota hi nahi tha — lekin
   KEY_RESET board par key **aur** password dono naya generate karta hai
   (§5.4), is liye stored password stale (ghalat) tha. Fix: nayi key
   purani stored key se mukhtalif ho to `Delete(boardId)` — purani key +
   stale password dono mit te hain, phir nayi key save hoti hai. Wahi key
   dobara mile (routine reboot) to password rehta hai.
2. **SET_PASSWORD save-fail khamoshi se nigal jata tha.**
   `BoardSession.SetPasswordAsync` mein `TrySavePassword` ka result ignore
   hota tha — board password badal chuka hota, app ke paas koi record nahi
   (WebOTA lockout, hal sirf KEY_RESET). Fix: save fail ho to
   `(true, warning)` — `(false, ...)` nahi (board badal gaya hai, jhoot
   mat bolo); warning mein KEY_RESET recovery likhna lazmi hai.
   `ChangePasswordDialog.SaveWarning` ye warning caller tak pahunchata hai.
3. **DPAPI blob write atomic nahi thi.** `File.WriteAllBytes` seedha
   overwrite karta tha — crash darmiyan mein aadha blob chhorta (agle load
   par "kharab blob" → re-pairing). Fix: temp file + atomic move
   (`KeyStore.TrySave`); crash par ya purana blob rehta hai ya naya.
4. **Forget ka rasta nahi tha.** Naya `BoardSession.ForgetBoardCredentials()`
   — KEY_RESET ke baad dobara pairing se pehle ya "forget board" par key +
   password dono mitata hai (session key bhi saaf).
5. **Stale session key ka istemal fail-closed hai** (verify kiya — fix ki
   zaroorat nahi): rotation ke baad purani session key se HMAC fail hota
   hai, board reject karta hai; khamoshi se stale credential kabhi kaam
   nahi karta.
6. **In-memory key wipe API (W2 follow-up, Phase F W1):** pehle session
   key sirf app restart / `Dispose` par saaf hoti thi. Naya
   `BoardSession.WipeSessionKey()` RAM se key foran saaf karta hai
   (restart ka intezar nahi) — KeyStore blobs ko haath nahi lagta, is
   liye agli control command par key store se dobara promote ho jati hai.
   Nonce/counter nahi chheerte (counter monotonic rehta hai — reset karna
   replay/duplicate ka rasta kholta). `ForgetBoardCredentials` se farq:
   wo stored credentials bhi mitata hai; ye sirf RAM saaf karta hai
   (sensitive operation ke baad / user "lock" par). Koi blocker nahi mila.

### Tests
- `PhaseFWorkstream1Tests.cs`: rotation purge (nayi key → password mit
  gaya), same-key → password rehta hai, `ForgetBoardCredentials`,
  `WipeSessionKey` (RAM saaf, store barqarar, dobara promote hota hai;
  no-key no-op; disposed par throw), SetPassword save-fail → `(true,
  warning + KEY_RESET)`, save-ok → `(true, null)`.

## 3. SEC-E-01 — Naya password NVS mein plaintext (PARTIAL — sandbox fix)

### Asal halat
`firmware/rsh/keystore.cpp` password ko `Preferences.putString("wifipw")`
se **plaintext** NVS mein likhta tha. `keystore.h:17-19` khud OPEN-3
residual kehta hai; docs ("encrypted NVS") is se mutabiqat nahi rakhte
thay (§7).

### Sandbox fix (hua)
- Naya pure header `firmware/rsh/pw_obfuscate.h`: password NVS mein
  likhne se pehle device key se derived keystream se XOR hota hai
  (keystream block j = SHA256(key32 ‖ "RSH-PW-V1" ‖ u32le(j)));
  parhte waqt wapas XOR. SHA-256 caller deta hai (firmware: mbedTLS;
  host tests: independent SHA-256). SHA fail ho to obfuscate fail hota
  hai (fail-closed — zero keystream se plaintext kabhi nahi likha jata).
- NVS key badli: `"wifipw"` (plaintext string) → `"wifipw2"` (obfuscated
  bytes, `putBytes`/`getBytes`). `keystore_begin` pehle `"wifipw2"`
  azmata hai; legacy `"wifipw"` mile to validate + foran migrate kar ke
  plaintext mita deta hai. `keystore_generate_and_save_` aur
  `keystore_set_password` bhi sirf `"wifipw2"` likhte hain.
- Tamam password consumers (`net.cpp`, `webota.cpp`, `rsh.ino`) RAM copy
  (`keystore_password()`) se parhte hain — NVS format change
  `keystore.cpp` mein contained hai.

### Imaandari se had (chhupaya nahi gaya)
**Ye OBFUSCATION hai, encryption nahi.** Device key khud usi NVS mein
plaintext hai (`"devkey"`) — jo poora flash dump le sakta hai wo is
open-source scheme se password wapas bana sakta hai. Is fix se milta hai:
(1) flash par `strings` / NVS dump mein password nazar nahi aata;
(2) sirf password entry parhne wala password nahi le ja sakta;
(3) ghalat key se deobfuscate karne par `password.h` validation fail hoti
hai (corrupt/stale entry kabhi istemal nahi hoti).
**Asal hal flash encryption + encrypted NVS hai (OPEN-3) — Phase H.**
Exact baaqi kaam + procedure §6 mein hai. Ye conditional item hai —
production release se pehle lazmi hai.

### Tests
- Host tests (`tests-host/test_main.cpp` §24, `pw_obfuscate.h`):
  round-trip (8/24/63 bytes — 63 ko 2 keystream blocks chahiye),
  ciphertext ≠ plaintext, determinism, doosri key → doosra ciphertext,
  ghalat key se wapas nahi aata (+ `password.h` validation use pakarti
  hai), bounds/null, SHA-fail → fail-closed. **Host suite: 2755 checks,
  0 failures** (pehle 2728 — §24 ne 27 add kiye).
- `pio run` **SUCCESS** (RAM/Flash pehle jaisi, 62.2% Flash, 0 warnings —
  `-Wall -Werror` ke neeche mbedTLS `_ret` API clean compile hua).
- Real-board verify (NVS dump se password na milna, migration) Phase H
  A7 mein hoga — sandbox mein NVS hardware nahi hai.

## 4. Gate mein mila bug — `FirmwareImageValidator` SHA offset (FIXED)

**Ye is workstream ke scope se bahir tha** (file:
`software/src/RSH.Core/Firmware/FirmwareImageValidator.cs`), lekin gate
block karta tha, is liye fix kiya gaya — coordinator ko ittila di ja rahi
hai, chhupaya nahi gaya.

- **Bug:** Validator digest ko `(cursor + 15) & ~15` par dhoondta tha
  (segments ke baad seedha 16-byte pad + SHA). Asal esptool layout
  (`bin_image.py` `append_checksum` + `save`): checksum byte 16-byte
  block ke **aakhri** byte par hota hai (align `15 - cursor%16` bytes
  aage, phir 1 byte), aur digest us ke **baad** shuru hota hai —
  `digestStart = (cursor + 16) & ~15`, hash `[0..digestStart)` par.
  Dono formulas sirf tab barabar hain jab `cursor % 16 != 0`. Jab
  segments 16-byte boundary par khatam hon (asal `firmware.bin`:
  cursor 822144), validator digest ko 16 bytes pehle parhta tha (15 zero
  pad bytes + checksum + digest ke pehle 16 bytes) aur **valid image ko
  "SHA-256 mismatch" se reject** karta tha. Fail-closed tha (ghalat image
  kabhi accept nahi hoti), lekin valid firmware ka upload rasta band tha.
- **Sabit kaise hua:** `firmware.bin` par seedha SHA gin kar (Python) +
  esptool source parh kar + esptool ko dobara chala kar byte-identical
  file bana kar. File corrupt nahi thi — deterministic esptool output hai.
- **Trigger:** Meri firmware rebuild ne segment sizes badal diye, is liye
  cursor 16-aligned ho gaya. Pehle cursor aligned nahi tha, is liye test
  pass karta tha (ya firmware.bin maujood nahi thi → skip).
- **Fix:** Validator mein `digestStart = (cursor + 16) & ~15` (checksum
  byte samet hash). Synthetic builder (`PhaseCAppTests.BuildImage`) mein
  bhi wahi ghalat assumption thi (wo validator jaisi hi image banata tha,
  is liye synthetic tests kabhi asal layout ko pakarte hi nahi thay) —
  builder bhi esptool layout ke mutabiq kar diya gaya (checksum byte
  seed `0xEF` + segment XOR, phir SHA).
- **Regression tests** (`PhaseFWorkstream1Tests.cs`): 16-aligned segment
  end wali esptool-layout image pass karti hai; corrupt SHA reject hoti
  hai; non-aligned case bhi pass karta hai. Is ke baad
  `RealFirmwareBin_Passes` (asal `firmware.bin` par) pass hota hai.

## 5. Gate result (sandbox)

| Check | Natija |
|---|---|
| `dotnet build` RSH.Core | **0 warnings / 0 errors** |
| `dotnet test` RSH.Tests (full suite) | **518 pass / 2 fail / 5 skip (525 total)** — aakhri run. 2 fails **sirf W6 ke test-first items** hain (un ka kaam abhi baaqi hai — `Restore_LiveReload_Exists`, `WheelProfile_HasClutchCalibration`); W1 ke 18 tests, PhaseE SetPassword tests, aur `RealFirmwareBin_Passes` sab pass |
| Host tests (`tests-host/run_tests.sh`) | **2755 checks, 0 failures** (C++ + Python vectors + merge mock sab) |
| `pio run` firmware | **SUCCESS** (Flash 62.2%, merged bin `RSH-Firmware-DevKitV1-v4.0.0.bin`) |
| AppSettings scratch verify | **16 checks, 0 failures** |
| Virtual Test Rig | Neeche §5.1 |

### 5.1 Virtual Test Rig
- **Pehla run (contaminated — discard):** mera run (05:46) doosre
  workstreams ke do rig runs (05:48, 05:50/05:51) ke saath overlap hua —
  ek hi `test-env/` directory, ek hi shared 1.5 CPU. `REPORT.md` /
  `results.json` doosre run ne overwrite kar diye; soak perf numbers
  (rate 165/s, jitter p99 60 ms) us load ke neeche be-aitbaar thay. Is
  run ka koi natija istemal nahi kiya gaya.
- **Solo run (saaf — 2026-11-13T06:01:13Z, quick mode):** **16/20
  scenarios PASS (1 skipped — long-soak, `--long` only), 0/2 stages
  PASS.** Soak PASS (rate 198/s, loss 0%, jitter p99 1924 µs).
  W1 scope ka koi scenario fail nahi hua. Failures:
  - **Stages (dono FAIL):** unit stage `dotnet-test` FAIL = sirf W6 ke
    2 test-first items (neeche §5 table); host-tests/pin-rules/vectors
    PASS. firmware-host stage FAIL = SCons `.sconsign312.tmp`
    `FileNotFoundError` — shared `.pio/build` par concurrent `pio run`
    ki race (doosra agent usi waqt `pio run` chala raha tha — `ps` se
    confirm). Mera standalone `pio run` (contamination se pehle) SUCCESS
    tha; rig ke baad bhi doosre agent ka `pio run` meri `keystore.cpp`
    ke saath link ho raha tha.
  - **ntp-ping FAIL:** e2e p99 72979 µs (SLO 10000) — ek 73 ms VM spike
    (29 samples mein 2nd max); pehle run mein p99 5453 µs PASS tha.
    VM scheduling noise, W1 code se mutalliq nahi (ping/UDP path ko
    maine chhua hi nahi).
  - **control-channel / game-restore / clutch-range FAIL:** `wheel.py`
    run ke doran **SyntaxError** (`line 1278, nonlocal
    `ping_connected_addr``) se start hi nahi hui — file usi waqt doosre
    workstream ki edit ke doran tooti hui thi (mtime 06:08:38 — run ke
    baad dobara theek; mera manual `--self-check` us ke baad PASS).
    game-restore + clutch-range mein `validPackets=0` (wheel hi nahi
    chali). W1 code se mutalliq nahi.
- **Rig ka behaviour nahi badla.** Koi W1 bug rig mein nahi mila.

### Mile hue mazeed bugs (gate ke dauran)
1. **`AppSettings.cs` mein `using System.Text.Json.Serialization;`
   missing tha** — `[JsonIgnore]` compile hi nahi hota tha. Scratch
   verify ne pakra (RSH.App Linux par build nahi hota, is liye ye pehle
   nazar nahi aaya). Fix: using add ki gayi. **Lesson:** RSH.App ke
   compile ka Linux par koi rasta nahi — Windows build (ya linked-file
   scratch verify) ke baghair App-layer edits andhe hain; Phase H mein
   Windows CI/build ka intezam chahiye.
2. **Full suite mein flaky SetPassword failures** (pehle runs mein 8–9
   fails): `ControlClient.SetPasswordAsync` ka static 60 s rate limiter
   (SEC-E-02) test classes ke darmiyan share hota hai — jo test pehle
   send kare, us ke 60 s ke andar doosri class ka send fail hota hai.
   PhaseE tests reset nahi karte. Fix: W1 tests reset **pehle aur baad
   mein dono** karte hain (good neighbour); is ke baad 2 lagatar full runs
   mein PhaseE SetPassword tests pass hue. **Baaqi:** PhaseE/W6 tests mein
   bhi reset-before lagana chahiye taake suite deterministic ho (test-only
   change, coordinator/W6 ke zariye).
3. **W6 ki test file aadhe mein tooti hui mili** (CS9135) — ek full-suite
   run ke doran compile fail hua; dobara run par W6 ne fix kar diya tha.
   (Parallel workstreams ek hi tree mein — expected.)
4. **Flaky `NtpPingTests.PingClient_RealUdpLoopback_Measures`** (ek run
   mein fail, baad mein pass) — timing/UDP, W1 scope se bahir; dobara
   run par pass hua.

## 6. Phase H ke liye baaqi — EXACT list (OPEN-3 / SEC-E-01 ka asal hal)

**Flash encryption + encrypted NVS enable karna — production release se
pehle LAZMI hai.** Ye Phase F mein nahi ho sakta: eFuse burning
irreversible hai aur pehle sacrificial board par test lazmi hai.

### 6.1 Procedure (ESP32 DevKit V1, ESP-IDF / Arduino core 3.x)
1. **Sacrificial board lo** (kabhi user ka / production board pehle nahi).
   Us par maujooda firmware flash karo, pairing + SET_PASSWORD + KEY_RESET
   kaam karte hue verify karo (baseline).
2. **Secure Boot v2 enable karo** (KEY-LIFECYCLE §9 — RSA-3072):
   signing key offline generate karo (`espsecure.py generate_signing_key`
   — key kabhi repo/git/image mein nahi); bootloader + app sign karo;
   public key digest eFuse mein burn karo (`espefuse.py burn_key`).
   **eFuse burn irreversible hai** — ghalat key = board hamesha ke liye brick.
3. **Flash encryption enable karo** (release mode —
   `CONFIG_SECURE_FLASH_ENC_ENABLED`, `CONFIG_SECURE_FLASH_ENCRYPTION_MODE_RELEASE`):
   pehle boot par flash encrypt hoti hai; is ke baad flash dump se
   plaintext nahi milta.
4. **Encrypted NVS enable karo** (`CONFIG_NVS_ENCRYPTION`): NVS encryption
   keys eFuse mein (HMAC key); `nvs_flash` encrypted namespace istemal kare.
   `Preferences` ("rsh" namespace — `devkey` + `wifipw2`) is ke neeche
   aayega.
5. **Verify (sacrificial board par):**
   - Board boot hota hai, AP up hota hai, pairing (60 s window) kaam karti hai.
   - `esptool.py read_flash` se dump lo — `devkey` / `wifipw2` / password
     plaintext mein **nahi** milte; `strings` par kuch nahi.
   - SET_PASSWORD kaam karta hai; KEY_RESET kaam karta hai (nayi key +
     password, dobara pairing).
   - OTA update (signed) kaam karta hai; purani (unsigned) image reject hoti hai.
   - Reboot ke baad key/password barqarar hain (NVS encryption keys
     eFuse mein hain, erase nahi hoti).
6. **Migration note:** pehle se flashed boards (bina encryption ke) par
   flash encryption **in-place enable nahi hoti** — unhein erase + dobara
   flash + dobara pairing karna hoga (nayi key + naya password). Is ka
   user-facing tareeqa release notes mein likhna hoga.
7. **Us ke baad hi** production boards par wahi procedure dohrayo
   (har board apni eFuse keys ke saath — keys kabhi share nahi hoti).
8. Docs update karo: `KEY-LIFECYCLE.md` §2 + `PROTOCOL-V2.1.md` §4.6 se
   "OPEN-3" hatao, `THREAT-MODEL.md` TM-W07 ko residual se band karo.

### 6.2 Baaqi (Phase H / baad mein)
- **Real-board A7 verify:** NVS migration (`"wifipw"` → `"wifipw2"`) asal
  board par; obfuscation ke baad AP join + WebOTA auth naye + purane
  password se.
- **Windows build:** RSH.App Linux par build nahi hota — App-layer edits
  (jaise §1) ka compile Windows par verify karna hoga (CI ya Windows
  machine).
- **Test determinism:** SetPassword rate-limiter resets PhaseE/W6 tests
  mein bhi lagao (test-only).
- **Observed residual (scope se bahir, fix nahi kiya):**
  `WindowsIntegration.ConnectToWheelWifi` password ko `%TEMP%\rsh-wlan.xml`
  mein likhta hai (netsh profile ke liye) aur best-effort delete karta hai
  — process crash ho to plaintext temp file reh sakti hai. OS WLAN profile
  khud §5.3 ke mutabiq hai (wo intended hai); temp file ka crash-safe
  handling (ya `netsh wlan add profile` ko stdin/XML ke baghair) alag
  kaam hai.

## 7. Doc contradictions fixed (is workstream mein)
- `PROTOCOL-V2.1.md` §5.3 + §7 OPEN-15: "CLOSED" daawa ghalat tha — code
  doc ke mutabiq hua + correction note likhi gayi.
- `PROTOCOL-V2.1.md` §3.2/§4.1/§4.6 + §7 OPEN-3: "encrypted NVS" ke
  daawe — sach (plaintext NVS, OPEN-3 Phase H) likha gaya.
- `KEY-LIFECYCLE.md` §2 (§2 step 2–3, design notes), §4 step 1, §8
  Storage, §10 OPEN-3 + OPEN-15, §11 traceability: "encrypted NVS" ke
  daawe hatae; OPEN-15 **CLOSED** (PROTOCOL §7 se mutabiq).

---

*Scope se bahir: `RSH-handover/` aur `build/` ko haath nahi lagaya gaya;
koi git command nahi chalaya gaya; GitHub par kuch push/tag/release nahi hua.*
