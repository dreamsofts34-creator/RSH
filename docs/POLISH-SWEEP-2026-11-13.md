# RSH — Final Polish Sweep (Architecture & Planning) — 2026-11-13

**Scope:** gaps + plan traceability + overall design review. Repo local HEAD 6838de5
(v2.0.3 checkpoint, app 2.0.3 / firmware 4.0.0 / protocol v2.1). Plans FINAL hain —
unhein edit nahi kiya gaya. Code edit nahi kiya gaya — code gaps yahan report hain,
coordinator route karega. Sirf `docs/` edit hui hain (neeche "Fixed" dekho).
**Method:** targeted grep/read + gate history (Phase A–H + v2.0.1/2/3 hotfix gates);
poora build/test nahi chalaya gaya — ye analysis hai.

## 1. Traceability verdict — sab phases complete hain? (CONDITIONAL YES)

| Track | Verdict |
|---|---|
| Phases A–H | **Sab gates CONDITIONAL PASS** (A 06:37, B 07:26, C 08:27, D 09:10, E 10:21, F 12:02, G 13:29, H 15:11) + hotfix gates v2.0.1 (17:49), v2.0.2 (18:41), v2.0.3 (19:20). Conditions §3 mein — koi bhi condition code-incomplete nahi, sab user-side / Windows-PC / review items hain. |
| UI-1–UI-12 | **Done, UI LOCKED** (`docs/UI-LOCKED.md` — parity checklist 11/11, 1 PARTIAL: U-P8/U-P9 runtime measure Windows PC par). Deviations: UI-3 clutch bar nahi (protocol v2.1 mein clutch channel hi nahi — UI-LOCKED mein darj, accepted); UI-12 runtime checks (theme switch, screen reader, High Contrast) Windows PC par baaqi. |
| FW-1–FW-14 | **FW-1–FW-13 code mein maujood** (spot-check: `firmware/rsh/` — pinmap/pin rules, `protocol.h` LED_TEST/PIN_PROBE, `safemode.h` BOOT-hold + 15 s AP deadline, `config_transaction.h`, `inputs.cpp`, `net.cpp` txFail, `ctrl_handlers.h` KEY_RESET, `webota.cpp` `authenticate()`, `heap_policy.h`, `modules.h` FW-11 Handbrake/Shifter/Horn, `encoders.h`, `led_sink_rmt`, `platformio.ini` `-Werror`). **FW-14 A7 board run PENDING** — user-side condition (§3). |
| SW-1–SW-16 | **SW-1–SW-13, SW-15, SW-16 (code hissa) done.** SW-14 PARTIAL (matrix DRAFT — real-game verify Windows PC par, §3). SW-16 PARTIAL by user faisla: OV cert deferred (unsigned ship), winget + MSIX install test pending. SW-1 CI: tag releases par green ho chuka hai (v2.0.3: Build and Release 43442136416, CI 43442105445). |

Spot-check evidence (code): `RSH.Core/Protocol/` (ProtocolV2, ControlFrame, Crc16, SequenceTracker,
StatusPacketV2), `Pipeline/` (WheelEngine, PipelinePlan, ModuleTuning), `Profiles/ProfileStore.cs`
(v1→v2 migration), `Transport/NtpPing.cs` (SW-13), `Windows/` (SingleInstanceCoordinator,
AutostartService, FirewallRule, WindowChromePolicy), `Services/GameLauncher.cs` (UI-4),
`Utilities/UpdateCheck.cs` (SW-11/A13), `Firmware/FirmwareUploadGate.cs` + `FirmwareImageValidator.cs`
(SW-11/A1), `software/src/RSH.RenderTest/` (v2.0.2 diagnostic build), `.github/workflows/release.yml`
(RenderTest asset, signed-bin verify — BUG-W1-1 fix maujood).

## 2. Gap list

### FIXED (docs — is sweep mein)

| # | Sev | Gap | Fix + verification |
|---|---|---|---|
| GAP-01 | High | `docs/RELEASE-CHECKLIST.md` stale tha: header "Version 2.0.0", §2 "CI green — PENDING", "Push/tag/GitHub Release mat karo" — jab ke v2.0.1/2/2/2.0.3 release ho chuke hain aur CI tag runs par SUCCESS hai. Checklist parhne wala ghalat samajhta ke release abhi honi hai. | **v2.0.3 ADDENDUM** section add kiya (checklist ke shuru mein): current version 2.0.3, releases + CI run numbers, pending items unchanged. Upar ka v2.0.0-era body tareekhi record ke tor par chheda nahi gaya; koi pending item tick nahi kiya gaya. |
| GAP-03 | Medium | 5 purani v1-era docs bina kisi nishan ke `docs/` mein pari theen aur current daawe karti theen: `FINAL-REPORT.md` (v1.0.0, 2026-11-10, 195 MB single-file exe), `SOFTWARE-PLAN.md` (protocol v1 18-byte), `FIRMWARE-PLAN.md` (starting point v3.3), `PROJECT.md` (board ghalat "ESP32-C3 Super Mini" — asal DevKit V1 hai; "SimWheel rename pending" — rename mukammal hai), `TEST-REPORT.md` (firmware v3). | Har file ke shuru mein **⚠️ SUPERSEDED banner** (precedent: `RELEASE-WORKFLOW-CHECK.md` B-W9-4). Body tareekhi record ke tor par rehti hai; current authority har banner mein likhi hai. |
| GAP-06 | Low | `docs/SUPPORT-PERIOD.md` sirf "v2.0.0" kehta tha — hotfix line ka status wazeh nahi tha. | Note add kiya: v2.0.1–v2.0.3 isi v2.0.x line ke hain; support period v2.0.0 release se 12 mahine, unchanged. |

### DEFERRED / REPORTED (code — coordinator route karega, is sweep ne code nahi chheda)

| # | Sev | Gap | File:line + suggested fix |
|---|---|---|---|
| GAP-02 | Medium | **Win10 vs RuntimeCheck mismatch:** v2.0.3 Win10 (build 19045) ko officially support karta hai (auto-classic + plain-nav, `App.xaml.cs:171-181`), lekin `RuntimeCheck` abhi bhi Windows 11 floor (build ≥ 22000) mangta hai aur Win10 par update check ko rok deta hai: "RSH needs .NET 10+ on Windows 11." (`software/src/RSH.Core/Utilities/UpdateCheck.cs:14-30`, use: `software/src/RSH.App/ViewModels/DiagnosticsSettings.cs:759-762`). Startup block nahi hai — sirf update-check path hai — lekin user ke asal PC par ghalat message hai. | Floor ko Win10 19045 karo ya update check mein "supported with classic window" messaging alag karo; plan v2.3 §6 ka "Windows 11 floor" bhi is ke saath update karna hoga (plan FINAL hai — coordinator/parent faisla kare). |
| GAP-04 | Low | `software/Directory.Build.props:13` mein `<Version>2.0.0</Version>` stale hai jab ke `software/src/RSH.App/RSH.App.csproj:22` mein `2.0.3` hai (v2.0.2 gate ki known cosmetic C3, abhi bhi maujood). | Version ek jagah rakho (App csproj authority) ya props ko 2.0.3 karo. Cosmetic — release par asar nahi (workflow App csproj se version leta hai). |
| GAP-05 | Low | **Orphan pages:** `Views/Pages/ConnectionPage.xaml(.cs)` aur `CalibrationPage.xaml(.cs)` ka koi navigation/code reference nahi (sirf apni files + generated `.g.cs`); Setup flow unke VMs reuse karta hai, pages nahi. UI-LOCKED pending #7 mein pehle se darj hai. | Agle version mein faisla: delete karo ya Setup se link karo. Abhi chhedna nahi (UI LOCKED). |
| GAP-07 | Medium | **SW-14 PARTIAL:** `docs/GAME-COMPETIBILITY.md` abhi bhi "UNVERIFIED DRAFT" hai — koi game real build + Windows PC + asal game par verify nahi hua (sandbox scope sirf input chain tak hai). UI-4 launcher bhi real Steam launch ke baghair hai. | Condition (§3) — user-side Windows PC par verify hoga. Code gap nahi; matrix mein jhoota "verified" daawa kahin nahi hai (file khud saaf kehti hai). |
| GAP-08 | Low | **UI-3 deviation (accepted):** Dashboard par clutch pedal bar nahi hai — protocol v2.1 mein clutch channel hi nahi. UI-LOCKED mein darj hai; fake bar kabhi nahi banana. | Koi fix nahi — protocol change (v2.2) ka faisla ho to dobara dekha jayega. |

### VERIFIED — koi gap nahi (dobara check kiya gaya)

- **ApexLink:** code mein `grep = 0` — sirf 3 mentions, teeno docs mein (UI-LOCKED/CHECKLIST mein grep-evidence ke tor par). SW-15 gate ("sirf history/docs mein zikr") poora hai.
- **Duplicate engines/config:** `WheelEngine` ek hi hai (`RSH.Core/Pipeline/WheelEngine.cs`); `config_transaction.h` ek hi hai — `config_txn.h` sirf comments mein zikr hota hai (`config_transaction.h:32,37`, `tests-host/test_main.cpp:1743`), koi include nahi. Phase E ka fix barkarar hai.
- **Duplicate pipeline:** `ProcessingPipeline` vs `PipelinePlan` duplicate nahi — ek hi pipeline hai jo `PipelinePlan` se chalti hai (`WheelEngine.cs:33,61,106-115`); `ProcessingPipeline.cs` header mein P11 wiring saaf likhi hai.
- **Layering:** App → Core + VJoy + HIDMaestro; dono outputs → Core only; Core ke `ProjectReference` = 0 (koi cycle nahi). Public API surface: `IJoystickOutput` / `OutputBackend` / `PipelinePlan` / `ProfileStore` — sane, koi App type Core mein nahi.
- **Versions:** App 2.0.3, firmware 4.0.0, protocol v2.1 — `USER-GUIDE.md` header bhi 2.0.3 hai. SBOM 36 components (`docs/SBOM.cyclonedx.json`) — checklist se match.
- **D26:** `docs/DECISION-LOG.md:36` DECIDED (Phase G) — KEY-LIFECYCLE se match.
- **Release workflow:** `release.yml` mein RenderTest asset + signed-bin verify maujood (BUG-W1-1 fix).

## 3. Conditions list (block nahi — user-side / Windows-PC / review)

1. **A7 real-board run** (user karega — runbook `docs/A7-RUNBOOK.md` + gate `docs/A7-GATE-PROCEDURE.md`; pass criteria: ~200 sent/s, overruns 0, maxCycleUs < 3500 µs, reboots 0). Is ke baad freeze.
2. **4-hour long soak INCOMPLETE** (~16 min / 240 min, user order se roka gaya; koi metrics nahi — dobara chalana ho to poore 4 ghante `--long`).
3. **SEC-H-04 review baaqi** (key-compromise response doc `docs/FLASH-ENCRYPTION-PROCEDURE.md` §7 maujood hai, review pending).
4. **OPEN-3 flash encryption board-side baaqi** (NVS obfuscation hai, encryption nahi — SEC-E-01 PARTIAL).
5. **Unsigned MSIX build + clean-PC install test** (Windows PC; SmartScreen warning expected).
6. **Winget submit** — signed MSIX + OV cert par blocked (cert user faisle se deferred).
7. **Rollback E2E test** (downgrade install → v1 restore → OTA fallback — Windows PC + board).
8. **Update-check live test** (asal GitHub API ke khilaf — Windows PC; GAP-02 se mutaliq).
9. **Firewall rule Settings wiring** (uninstall par remove — Settings mein "Remove firewall rule" nahi hai; uninstall script maujood hai).
10. **SPDX SBOM** (optional — CycloneDX hi release SBOM hai).
11. **Real-game verification** (SW-14 matrix + Steam launch + auto-switch restore — Windows PC + asal games).
12. **UI runtime checks** (Windows PC: theme switch, screen reader, High Contrast, U-P8 startup ≤ 1200 ms, U-P9 10-min soak memory flat, 30-min real use).

## 4. Design review — mukhtasar

- **Layering saaf hai:** Core (engine/protocol/profiles/transport/output contracts) → App (WPF shell/pages/VMs) + output backends (vJoy default, HIDMaestro experimental). Koi cycle nahi, koi App type Core mein nahi.
- **Dead code:** GAP-05 ke 2 orphan pages ke ilawa kuch nahi mila (DiagnosticsPage/ProcessingPage UI-12 mein delete ho chuke hain). `bin/`/`obj/` build outputs repo mein hain — `.gitignore` ka masla nahi (working tree mein the, commit scope coordinator ka hai).
- **Naming:** RSH consistent hai (mutex `Global\RSH.SingleInstance`, `%AppData%\RSH`, `rsh.local`, SSID `RSH`); ApexLink sirf docs-history mein hai.
- **Sab se bara design risk (pehle se maloom):** Win10 support plan se bahar nikla (GAP-02) — v2.0.3 ne window/rendering side fix kar diya, lekin OS-floor checks/docs abhi bhi Win11 kehte hain. Agle version mein floor ka faisla ek jagah band hona chahiye (plan §6 + RuntimeCheck + SUPPORT-PERIOD + USER-GUIDE).
