# RSH — HIDMaestro Evaluation (D19, Phase G Workstream 4)

**Status: EVALUATION — 2026-11-13. Recommendation §7 mein hai. Koi code change
is doc ke saath nahi aaya — sirf evaluation + recommendation hai. Switch ka
faisla ho to implementation Phase H mein hogi.**
**Evaluated version: HIDMaestro v2.1.0** (GitHub releases, 2026-11-13 ko latest;
driver build v2.1.0; SDK `HIDMaestro.Core.dll`, 337 profiles).
**Sources:** https://hidmaestro.org/ , https://hidmaestro.org/specs.html ,
https://hidmaestro.org/docs/reference/umdf2-driver-internals/ ,
https://github.com/hifihedgehog/HIDMaestro/releases ,
https://padforge.org/docs/start/installation/ , https://padforge.org/specs.html ,
Plan v2.3 §5 (Phase G) / §6 / D19, `research-standards-2026-11-12.md` §1.
**Baseline (aaj):** vJoy BrunnerInnovation v2.2.2.0 (EV + attestation signed,
MIT/public domain), `RSH.Output.VJoy` (`VJoyOutput : IJoystickOutput`).

## 1. HIDMaestro kya hai

- **C# SDK + user-mode driver.** App ek profile chunti hai (337 device
  profiles, har ek JSON — Xbox 360 se HOTAS/wheels/keyboards tak) aur game
  doosri taraf "asal" device dekhta hai: VID/PID, strings, HID descriptor,
  layout, bus type — sab profile se aate hain.
- **UMDF2 driver:** `HIDMaestro.dll` ek UMDF2 lower-filter driver hai jo
  Microsoft ke inbox kernel driver `mshidumdf.sys` ke neeche chalta hai.
  Standard profiles SDK↔driver shared memory se baat karte hain (input/output
  per-packet signalled). SDK aur driver mein per-device code nahi — sab
  profile data hai.
- **Koi kernel driver sign nahi karna:** `InstallDriver()` khud ek locally
  trusted certificate banata hai, driver ko sign karta hai aur install karta
  hai. Na EV certificate, na test-signing mode, na reboot (vendor ka daawa).
- **Ek DLL:** `HIDMaestro.Core.dll` mein 337-profile catalog + x64 aur ARM64
  dono driver payloads + signing tools embedded hain. Koi NuGet package
  **nahi** mila — distribution release ZIP se hai (DLL + `HIDMaestroTest` +
  `HIDMaestroProfileExtractor` + `HIDMaestroUninstall.exe`).
- **License: MIT**, free forever, commercial use welcome, no restrictions
  (hidmaestro.org specs page). RSH is mein bundle kar sakta hai — vJoy ki
  tarah "bundle nahi" ka faisla yahan license ki wajah se nahi, install/admin
  UX ki wajah se hoga.
- **Ecosystem:** PadForge 5.x is par built hai (v2 se upgrade par ViGEmBus +
  vJoy uninstall karke HIDMaestro par le jata hai); j360More isay teesra
  backend (VIIPER / HIDMaestro / ViGEmBus) istemal karta hai; DirectXInput
  is ke zariye Xbox pads/keyboards/mice chalata hai.

## 2. Maintained? — HAAN, aur tezi se

- Releases: v1.1.20 → v1.1.30 → v2.0.0 → v2.0.2 → v2.0.3 → **v2.1.0**
  (2026 ke andar). v2.1.0 mein frame-delivery, sleep-resume (Xbox 360
  profiles sleep ke baad martay thay), out-of-order/duplicate frames, aur
  CPU-load stalls fix hue — matlab project **active aur honestly tested**
  hai (release notes mein measured numbers: DualShock 4 ReadFile p95
  166 ms → 64 µs load ke neeche).
- **Lekin:** maintainer ek shakhs hai (`hifihedgehog`) — bus factor 1.
  Release cadence tez hai aur behaviour badalta rehta hai (v1.1.20 mein SWD
  enumerator migration, v2.1.0 mein frame-delivery dobara likhi gayi). Koi
  LTS/stable branch nahi. RSH jaisi release ke liye ye **API/behaviour churn
  risk** hai — pin ki hui version par rehna hoga, aur har upgrade par
  dobara verify karna hoga.
- ViGEmBus ke muqable mein (archived Nov 2023 — research §1) HIDMaestro
  waazeh tor par zinda hai; vJoy Brunner fork (2.2.2.0) bhi maintained hai
  lekin us ki release cadence HIDMaestro se sust hai.

## 3. Windows 11 + HVCI behaviour

- **HVCI (Memory Integrity):** HIDMaestro ka driver user mode mein hai;
  kernel mein sirf Microsoft ka inbox `mshidumdf.sys` + `WUDFRd.sys`
  (reflector) chalte hain — dono Microsoft-signed inbox components hain.
  Is liye **HVCI blocklist ka wohi risk nahi** jo vJoy (`vjoy.sys`, third-party
  kernel driver) par hai. Ye HIDMaestro ka sab se bara architectural faida
  hai aur plan v2.3 §7 ke "vJoy HVCI block" risk ka seedha jawab hai.
- **Verify nahi hua:** HVCI **on** Windows 11 24H2/25H2 par HIDMaestro ka
  install + device-create is evaluation mein measure nahi hua (sandbox mein
  Windows nahi hai). Architecture se expect karte hain ke chalega, lekin
  "HVCI-proof" ka daawa sirf Windows PC par measured test ke baad likha
  jayega (Phase H gate, §8).
- **Admin lazmi hai:** device-creating operations ke liye administrator
  privileges chahiye (release requirements). RSH aaj admin ke baghair
  chalti hai (vJoy install ek dafa admin se hota hai, runtime nahi) —
  HIDMaestro par har device-create admin mangta hai; agar RSH admin ke
  baghair chalni hai to device-create ko ek baar admin helper/install step
  mein karna hoga, ya app ko elevated chalana hoga. Ye UX faisla hai,
  blocker nahi.
- **Locally trusted cert:** InstallDriver apna certificate banata hai.
  Ye test-signing **nahi** hai, lekin machine ke trust store mein ek
  self-signed cert daalna hai — corporate/locked-down PC par policy rok
  sakti hai. Anti-cheat ka radde-amal **unverified** hai (vJoy kernel
  driver bhi anti-cheat ki nazar mein aata hai; HIDMaestro user-mode hai,
  is liye kam az kam kernel-driver jitna bura nahi hona chahiye — phir bhi
  per-game verify hoga).
- **Composite personas ka kernel hissa:** Sony/Valve/Nintendo composite
  profiles bundled **Microsoft-signed usbip-win2 0.9.8.1** kernel transport
  istemal karte hain (pinned SHA-256 se check hota hai). RSH wheel ke liye
  ye **zaroori nahi** — DirectInput wheel standard profile hai, composite
  nahi. Is liye RSH ke install mein koi kernel transport nahi aayega.
- **Sleep/resume:** v2.1.0 se pehle Xbox 360 profiles sleep ke baad martay
  thay; v2.1.0 mein fix hai. RSH wheel DirectInput profile hai — sleep test
  Phase H gate mein shamil hoga.

## 4. DirectInput games compatibility

- **DirectInput wheel profiles maujood hain:** PadForge specs ke mutabiq
  337 profiles mein se **136 HOTAS/wheel (DirectInput)** profiles hain.
  Custom profile bhi ban sakta hai: arbitrary VID/PID, axis count, button
  count, resolution — apni JSON profile likh kar, ya `HMDeviceExtractor`
  se apne controller ko capture karke. RSH ke liye matlab: vJoy jaisa
  generic DirectInput device **ya** kisi mashhoor wheel (masalan Logitech /
  Thrustmaster) ki identity — doosra rasta games ke liye dilchasp hai jo
  known wheels ko whitelist karte hain, lekin **identity spoofing jaisa**
  lag sakta hai aur har game mein alag behave karega. Default: generic
  DirectInput wheel profile (vJoy jaisa), kisi brand ki identity na lo.
- **API coverage:** DirectInput, XInput, SDL3, browser, WGI, RawInput —
  ek hi device sab ko nazar aata hai. vJoy sirf DirectInput deta hai;
  HIDMaestro se **XInput-only titles bhi** khul jayenge (GAME-COMPATIBILITY
  matrix mein Forza jaisi entries ke liye faida).
- **FFB:** HID PID (DirectInput FFB) support hai — rumble/output reports
  code tak uthti hain. RSH mein FFB motor nahi hai, is liye v2 mein FFB
  phir bhi bahar hai; lekin HIDMaestro FFB ka **extension point** vJoy se
  behtar deta hai (vJoy mein FFB ka rasta nahi tha).
- **Latency (vendor claim, unverified):** "~35 µs median single-press,
  10,000 iterations, reproducible with one command" (hidmaestro.org).
  Vendor ka number hai — RSH isay apna SLO nahi banayega jab tak apne
  harness mein na naapa jaye. RSH ka E2E SLO (p99 ≤ 10 ms Wi-Fi) is se
  orders bara hai; output backend latency is mein negligible honi chahiye
  — measure Phase H mein hoga.
- **Game-matrix dobara karni hogi:** output backend badalne se Phase F ki
  8-game matrix (GAME-COMPATIBILITY.md) dobara verify hogi — har game
  HIDMaestro device ko pehli dafa dekhega, bindings dobara banengi.

## 5. C# interop — RSH ke liye kaisa hoga

- **Aasan:** .NET 10, ek DLL reference (`HIDMaestro.Core.dll`), "whole
  lifecycle is one page of C#" (vendor). RSH ka `IJoystickOutput`
  interface pehle se maujood hai (`TryConnect` / `Disconnect` /
  `Update(in ProcessedState)` / `SendNeutral`) — naya project
  `RSH.Output.HIDMaestro` mein `HidMaestroOutput : IJoystickOutput`
  banega; `VJoyOutput` ko chheerna nahi hoga. `ProcessedState` → HID
  report mapping: axes X/Y/Z (+ Rx/Ry jahan profile de) aur buttons —
  `VJoyOutput.BuildPosition` jaisa code, nayi report layout ke saath.
- **Distribution:** release ZIP se DLL lena + SHA-256 pin karna hoga
  (vJoy DLL ki tarah; koi NuGet nahi). Driver payload DLL ke andar
  embedded hai — alag driver download nahi.
- **Install flow:** pehli run par `InstallDriver()` (admin) + device
  create; uninstall ke liye `HIDMaestroUninstall.exe` (NativeAOT, .NET
  runtime nahi chahiye). RSH installer (MSIX, Phase H) mein ye step
  add hoga — MSIX + admin driver install ka interaction Phase H mein
  test hoga (MSIX container se driver install seedha nahi hota — ye ek
  khula sawal hai, §8).
- **Failure mode:** driver na ho / admin na mile to `TryConnect` wazeh
  error de — vJoy backend jaisa contract (app kabhi crash nahi).

## 6. Migration effort (estimate)

| Kaam | Size | Note |
|---|---|---|
| `RSH.Output.HIDMaestro` (naya project, `IJoystickOutput`) | S | Report mapping + connect/disconnect + neutral |
| Output backend selector (Settings/Output page, vJoy vs HIDMaestro) | S | Dono backend parallel, default vJoy |
| Installer: `InstallDriver()` admin step + uninstall | S–M | MSIX interaction khula sawal (§8) |
| Diagnostics: driver version/status, HVCI state | S | Output page mein |
| Game matrix dobara (8 games, Phase F procedure) | M | Har game HIDMaestro device par dobara |
| Latency/soak verification (rig + Windows PC) | S | E2E SLO dobara naapo |
| **Kul** | **M (~1 workstream, Phase H)** | Firmware/protocol **zero change** |

## 7. RECOMMENDATION — v2.0.0 mein vJoy rehta hai; HIDMaestro Phase H experimental backend

**Faisla: ADOPT NAHI — abhi nahi. vJoy BrunnerInnovation 2.2.2.0 hi v2.0.0
ka output backend rehta hai. HIDMaestro ko Phase H mein parallel
experimental backend ke tor par banao (vJoy ke saath, default vJoy), aur
switch sirf us waqt jab §8 ka gate pass ho.**

Wajah (tarteeb se):

1. **vJoy kaam kar raha hai aur verified hai.** 2.2.2.0 pinned hai, sandbox
   mein unit tests green hain, Phase F game-matrix procedure is par bana
   hai. Release se pehle output backend badalna poori matrix dobara
   kholta hai — Phase H ka kaam, v2.0.0 ka nahi.
2. **HIDMaestro jawan aur single-maintainer hai.** v1.1→v2.1 chand maheenon
   mein, behaviour badalta rehta hai (frame delivery, SWD migration,
   sleep fix). Koi LTS nahi. Bus factor 1 + tez churn = v2.0.0 ke liye
   ghalat waqt.
3. **ARM64 ka waada abhi kagaz par hai.** D19 ka future path HIDMaestro
   se tha, lekin vendor khud kehta hai "no ARM64 machine has run them"
   (ARM64 binaries built hain, hardware par validate nahi hue). ARM64
   is se abhi bhi **unverified** hai — D19 tab hi badlega jab ARM64
   hardware par measured test ho.
4. **Admin + local cert naya trust/UX hai.** Har device-create admin +
   self-signed cert trust store mein — corporate PC, anti-cheat, aur MSIX
   installer ke saath interaction teenon unverified hain.
5. **Faida asal hai, lekin v2.0.0 ka nahi:** HVCI-proof architecture,
   XInput titles, FFB extension point, ARM64 path — ye sab v2.1 ki
   wajah hain, v2.0.0 ki nahi. Is liye evaluation ka natija "reject" nahi,
   "**defer with a concrete track**" hai.

**Agar Phase H mein switch ho to shartein (§8 gate):** Windows 11 24H2
HVCI-on par install+create+remove measure ho; 8-game matrix HIDMaestro
par dobara pass ho; E2E SLO (p99 ≤ 10 ms) dobara naapa jaye; sleep/resume
test pass ho; MSIX installer se driver install ka rasta tay ho; pinned
version (v2.1.0 ya us waqt ki latest stable) + SHA-256 darj ho.

## 8. Phase H handoff (gate checklist)

- [ ] H-HID-1: Windows 11 24H2, HVCI (Memory Integrity) **on** — install,
  device create, 10-min session, remove. Koi block/warning record karo.
- [ ] H-HID-2: 8-game matrix (GAME-COMPATIBILITY.md procedure) HIDMaestro
  generic DirectInput wheel profile par dobara — har game.
- [ ] H-HID-3: E2E latency (NTP ping + engine timestamps) + 4 h soak,
  vJoy numbers se compare karo.
- [ ] H-HID-4: Sleep/resume — device zinda rehta hai? Handle dobara parhta
  hai? (v2.1.0 fix ka regression check.)
- [ ] H-HID-5: MSIX installer se `InstallDriver()` admin step — rasta tay
  karo (installer helper vs first-run elevated prompt vs Store path).
- [ ] H-HID-6: Anti-cheat spot-check — kam az kam ek anti-cheat wala
  title (jahan applicable ho) launch + session, koi warning/block nahi.
- [ ] H-HID-7: ARM64 — sirf tab jab ARM64 hardware mil jaye; tab tak D19
  "unsupported" rehta hai, HIDMaestro isay nahi badalta.
- [ ] H-HID-8: Pinned version + SHA-256 (`HIDMaestro-vX.Y.Z.zip` + DLL)
  release notes + THIRD-PARTY-LICENSES.md mein darj karo (MIT).
