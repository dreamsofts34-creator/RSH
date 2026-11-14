# ⚠️ SUPERSEDED — ye document ab authority NAHI hai (final polish sweep, 2026-11-13)

**Ye 10 Nov 2026 ki audit-build delivery note hai (software v1.0.0 / firmware v3.1) — tareekhi record ke tor par rakhi gayi hai. Current authority: `docs/RELEASE-NOTES-v2.0.4.md` (app v2.0.4 / firmware v4.0.1 / protocol v2.1) + `docs/RELEASE-CHECKLIST.md`. Is file ke version/asset/password ke daawe (RSH-Setup-1.0.0.exe, "change-me-123") current release par laagu nahi hote.**

# RSH — FINAL DELIVERY (Audit Build, 10 Nov 2026)

Ali bhai, deep analysis + fixes + virtual user testing mukammal hai.
Ye final package hai — firmware v3.1 aur software v1.0.0 (audit build).

## Setup installer — ban gaya hai

**RSH-Setup-1.0.0.exe (54.7 MB)** — asal setup installer hai,
professional software ki tarah: install wizard, Start Menu + Desktop
shortcut, Add/Remove Programs entry, uninstaller. Per-user install hai,
admin rights nahi chahiye. Andar wahi self-contained RSH hai, to
.NET waghera kuch install nahi karna parega. Bas vJoy driver alag se
chahiye hoga (neeche dekho).

## Deep audit ma jo mila aur fix hua (4 cheezein)

1. **Safety bug (sab se ahem):** software ma Output OFF karne par vJoy ma
   purani steering/pedal values phansi rehti thin — game ma gaari waisa
   hi behave karti rehti. Ab output band karte hi ek neutral frame jata
   hai, sab centre/zero. Isi bug ka regression test bhi add hai.
2. **Network robustness:** UDP socket error par receive loop garam
   (busy-spin) ho sakta tha; ab 100 ms backoff hai.
3. **Firmware safety:** steering sample count ab buffer size se clamp
   hai — future ma config badlo to bhi buffer overflow nahi ho sakta.
4. **Calibration page:** profile change karne par purani values dikhti
   thin; ab active profile ki asal values reload hoti hain.

Baaki poora code dobara parha gaya — protocol, CRC, filters, governor,
vJoy layout — sab theek paya gaya, koi aur bug nahi mila.

## Virtual user testing ("as user" simulator)

Ek poora scenario suite chalaya gaya jo engine ko bilkul user ki tarah
use karta hai — nakli wheel se packets andar, joystick output record:

- Pehli drive: centre = 16384, full right = 32768, full left = 1 — exact
- Pedals: poori range sahi; gear buttons ke bits sahi (up/down alag)
- Wheel ghayab (Wi-Fi drop): 250 ms ma output neutral, wapas ane par
  khud recover
- Output OFF safety: neutral frame lazmi (upar wala bug fix)
- vJoy busy ho to engine zinda rehta hai, device free hote hi auto-connect
- Packet loss statistics bilkul durust (5 gire = 5 lost)
- Drive ke darmiyan profile change (mirror mode) sahi apply hota hai
- Button wale pedals ka ramp: dabaye rakhne par smooth full, chorne par zero
- 30 second soak: 6000 ticks, ek bhi tick late nahi

## Kul test numbers (fixes ke baad dobara verify)

- Software tests: **37 passed**, 1 skipped (UDP loopback test is
  sandbox ma khud skip hota hai; uski logic socket ke baghair cover hai)
- Firmware host tests: **536 checks, 0 failures** (source se dobara build)
- Packet cross-verify (Python): PASS — golden packet same
- Firmware asal compile (PlatformIO, ESP32 DevKit V1): SUCCESS
- Poora solution Release build: 0 errors, 0 warnings
- Performance pehle wali hi: parse 382 ns/packet, engine tick 446 ns,
  heap flat, Gen2 GC = 0 — low-end PC par bhi halka rahega

## Files jo mil rahi hain

1. **RSH-Setup-1.0.0.exe** (54.7 MB) — recommended, yahi chalao
2. **RSH-Software-v1.0.0-LITE.zip** (10.5 MB) — halka download;
   pehle ek dafa .NET 10 Desktop Runtime (free, Microsoft) chahiye
3. **RSH-Software-v1.0.0.zip** (132 MB) — portable exe + setup +
   guide + report + dono installer scripts, sab ek jagah
4. **RSH-Firmware-DevKitV1-v3.1.zip** — firmware ka naya version
   (clamp fix ke sath); purani v3 ki jagah yahi flash karna

## Sirf ye cheezein aap ke PC/board par hi test ho sakti hain

Ye kisi bhi virtual environment ma mumkin nahi, is liye alag likh raha
hun taake koi ghalat fehmi na ho: asal vJoy driver ke sath output,
Windows Memory Integrity (HVCI) ke sath driver ka behaviour, asal
wheel ke sath calibration ka feel, kisi asal game ma binding, aur
firmware ki asal board par timing/ADC noise. Baaki sab kuch jo machine
par test ho sakta tha, ho chuka hai.

## Chalane ka tareeqa (yaad dehani)

1. vJoy driver install karo — **BrunnerInnovation v2.2.2.0** (driver aur
   DLL same version). Ye ek dafa ka kaam hai.
2. RSH-Setup chalao, app kholo — pehli dafa 5-step wizard ayega
   (vJoy check, wheel connect, calibration, buttons, finish).
3. ESP32 ko **RSH** Wi-Fi se joro (firmware ma password config.h
   ma hai — flash se pehle `change-me-123` aur OTA password lazmi badlo).
4. Calibration wizard se steering centre/left/right aur pedals set karo,
   phir game ma vJoy device bind karo.

---

# UPDATE ROUND — Software v1.1.0 + Firmware v3.2 (11 Nov 2026)

## Naya: Advanced Auto-Setup Wizard (7 steps, sab automatic)

Industry research (Fanatec App / Moza Pit House / Simucube Tuner) ki
tartib par dobara bana: 1) Welcome 2) vJoy check 3) Wheel link 4)
Firmware update (optional) 5) Steering auto-calibration 6) Pedals
auto-calibration 7) Button + output test.

- Checks khud pass hote hain (vJoy connected, packets zinda) to
  wizard 1.5 second ma khud aage barh jata hai.
- Calibration ma numbers likhne khatam: centre wheel ko still rakhne
  se khud set hota hai (trimmed mean); lock-to-lock ghumane se
  endpoints khud pakre jate hain (1%/99% percentile — ADC spike
  endpoint kharab nahi karti); pedals bhi aise hi (choro, phir poora
  dabao). Span kaafi na ho to wizard mana kar deta hai, galat
  calibration save nahi hoti.
- Firmware step: .bin chuno, app khud board ke Web OTA ko upload
  karta hai (Basic Auth admin + OTA password) — browser/USB ki
  zarurat nahi. Calibration se pehle firmware wali tartib industry
  ki hai (purani firmware calibration accuracy par asar dalti hai).
- Test step: 4 buttons ka live test + vJoy par asal output values +
  poora summary (vJoy/Link/Steering/Pedals/Buttons) ek nazar ma.

## Naye features (research se)

- **Per-game auto profile switch:** Profiles page par har profile ka
  game process naam (masalan AssettoCorsa) likh do — game samne aane
  par wo profile khud load ho jata hai. Settings ma on/off maujood.
- **Rotation presets:** Profiles page se ek click par GT 540° aur
  F1 360° preset — active profile ki calibration copy karke, sirf
  rotation scale badal kar (SteeringScale, soft-lock output clamp).
- **App ke andar firmware update** (upar wizard step); browser wala
  Web OTA bhi v3.2 firmware ma maujood hai.

## Audit round 2 — jo bugs mile aur fix hue (30+)

- **Core (15):** UDP/Hello dispose races, pipeline filters reconnect
  par purani halat se shuru hote thay (ab reset), One Euro filter
  fixed 200 Hz farz karta tha (ab asal dt), cross-thread state ki
  visibility (lock/volatile), stats baseline ESP reboot par galat
  loss gin-ta tha, ProfileStore .bak kharab file se overwrite ho
  sakta tha + duplicate naam, gear up/down ek hi button par khamoshi
  se takra sakte thay (ab nudge + page warning), late-tick ki galat
  tareef, Dispose ke baad Start ki guard.
- **vJoy (5):** DLL resolver app-folder ki copy ke bajaye Program
  Files wali purani DLL pehle uthata tha (ab bundled pehle), aadha
  connect ho kar fail hone par device acquire reh jata tha, Disconnect
  ma neutral ki kharabi relinquish rok sakti thi, tamam P/Invoke par
  __cdecl convention wazeh, axis param USHORT durust.
- **App (15):** Diagnostics CPU% flicker, tray/Dashboard toggle
  desync, Profiles Export crash risk + global exception handler,
  log file shared lock (Diagnostics zip), ApplyIp validation, Output
  status har tick overwrite, mid-capture profile switch par capture
  leak, Input Monitor ka safed ScottPlot block dark theme ma (ab
  dark styled), 920px par rows ka clip (wrap/grid), steering % ka
  L/R sign ghayab (ab "L 42%" style), Monitor/Calibration offline
  empty states, capture button ki halat nazar na ana, wizard ke
  captures step-change par na rukna + Skip ka galat "completed"
  mark karna, About ma hardcoded version (ab assembly se).

## Test numbers (is round ke baad)

- Software tests: **42 passed** (38 Core incl. 5 naye AutoCalibrator
  + 4 vJoy), 1 skipped (UDP loopback sandbox ma skip; logic covered)
- Firmware: v3.2 is round ma change nahi hua — pehle se delivered
  (536 host checks + asal compile SUCCESS, Web OTA ke sath)
- Poora solution Release build: **0 errors, 0 warnings**
- Publish: LITE 28 MB folder / Essentials zip 11.3 MB

## Is dafa ki files

1. **RSH-Essentials-v1.1.0.zip** (11.3 MB) — updated LITE app
   (v1.1.0) + firmware v3.2 .bin + README. Setup installer owner ke
   kehne par deferred hai (download slow tha); essentials hi
   current tareeqa hai.
2. Firmware v3.2 .bin pehle se mila hua hai (merged, 0x0 flash);
   zip ke andar bhi wahi hai.

## Windows par hi verify ho sakta hai (pehle ki tarah, alag)

Asal vJoy driver ke sath output + HVCI, wizard ka asal windows/
tray look, board ke sath end-to-end (flash, ADC noise, OTA asal
upload), game ma binding. Code-level sab kuch jo machine par test
ho sakta tha, ho gaya hai.

---

# DESIGN ROUND — Software v1.2.0 + Firmware v3.3 (11 Nov 2026)

Owner feedback par: UI purani lagti thi, lambe labels thay, aur auto
setup sirf check karta tha, khud setup nahi karta tha. Reference
image (halka pastel dashboard, safed rounded cards, violet accent)
ke style ma poora redesign:

- **Design v2:** light theme default (ek-dafa migration; Settings se
  badal sakte ho), pastel background, bara ControlCornerRadius (12)
  — cards/buttons/inputs ek hi golai; brand violet accent (#6C4CF1)
  poori app ma; sidebar ma brand header (violet A tile + RSH);
  Dashboard par reference-style icon chips; Monitor plot ab theme ke
  sath (light ma safed/violet); gauges pehle se theme resources par
  thay, khud naye rangon par aa gaye.
- **Minimal copy:** tamam 10 pages + wizard se lambe explanatory
  labels khatam — 30+ text blocks ek line ya us se kam, professional
  English. Sirf wohi lines rahi jo kaam ki hain.
- **Auto setup ab asal ma setup karta hai:** Wizard ke Link step par
  "Connect automatically" — app netsh se RSH ka profile bana
  kar PC ko khud jor deta hai (password Connection page par editable,
  settings ma save). vJoy step par action buttons: "Open vJoy
  Configure" aur "Get vJoy driver" — sirf check nahi, hal bhi wahi.
  Calibration pehle se automatic thi (AutoCalibrator), ab tartib:
  vJoy → Wi-Fi auto-connect → firmware → auto-calibration → test.
- **Firmware v3.3 (polish):** Web OTA page ka design app ke naye
  light theme se match (pastel background, safed card, violet
  accent); logic bilkul nahi badla. Verify: host tests 536/536,
  packet cross-verify PASS, PlatformIO compile SUCCESS, merged bin
  1,191,424 B (0x0 flash).
- **Tests:** software 42 passed + 1 skipped, solution build 0/0,
  publish LITE 28 MB, Essentials zip 11.3 MB.

Windows par hi confirm ho sakta hai (har dafa ki tarah): naya design
asal screen par kaisa lagta hai, Wi-Fi auto-connect asal network par,
vJoy Configure button ka asal behaviour, game ma binding.

---

## UI UPDATE ROUNDS (owner-supplied patches, built + delivered by assistant)

### Software v1.2.1 — UI Update v1 (2026-11-11)
- Owner uploaded `RSH-UI-Update.zip` (8 files, authored with another Claude): new Setup page + SetupViewModel, Home/Setup/Tune changes in MainWindow, ViewModelStore, CalibrationTuning, ProcessingPage.
- Applied over the v1.2.0 source (backup: `software-backup-v1.2.0/`). Build 0 errors / 0 warnings; tests 42 passed + 1 skipped. Version bumped to 1.2.1.
- Delivered as `RSH-Essentials-v1.2.1.zip` via Google Drive (shared to owner) + email with the link.

### Software v1.2.2 — UI Update v2 (2026-11-11)
- Owner uploaded `RSH-UI-Update-v2.zip` (13 files): new design round — sidebar, Home, Setup, Tune; new `Themes/Theme.xaml`, App.xaml, Dashboard + Processing + Setup pages reworked.
- Applied over v1.2.1 (backup: `software-backup-v1.2.1/`). Build 0/0; tests 42 passed + 1 skipped. Version 1.2.2.
- Delivered as `RSH-Essentials-v1.2.2.zip` via Google Drive + email.

### Delivery route (standing)
Chat and Gmail both block the release zips (Gmail policy on archives containing executables; app is unsigned). Working route: upload to Google Drive, share reader access to the owner's Gmail (coder6171@gmail.com), email the Drive link. Firmware flashing: owner connected the board on 2026-11-11 (ESP32-D0WD-V3 detected in the Adafruit web flasher) and was guided to the first offset row at 0x0; flash completion not yet confirmed.
