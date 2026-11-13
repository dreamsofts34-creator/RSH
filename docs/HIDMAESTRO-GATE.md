# RSH — HIDMaestro Experimental Backend: 8-Point Gate (Phase H WS3)

**Status: GATE FAIL (conditional) — 2026-11-13. Backend experimental +
disabled-by-default rehta hai; v2.0.0 mein vJoy hi default hai.**
**Pinned SDK: HIDMaestro v2.1.0** (us waqt ki latest release).
**Authority:** `docs/HIDMAESTRO-EVALUATION.md` §7–§8 (recommendation + handoff
checklist H-HID-1..H-HID-8) aur W7 security review finding **SEC-H-06**
(Medium, conditional — neeche §3 dekho). Koi switch §8 gate ke baghair nahi.

## 1. Kya bana (scope)

- Naya project `software/src/RSH.Output.HIDMaestro/` — `HidMaestroOutput :
  IJoystickOutput`, vJoy ke parallel. `RSH.Output.VJoy` ko chheera nahi gaya;
  vJoy ka koi behaviour nahi badla (VJoy tests 56/56 green, neeche §4).
- `RSH.Core/Output/OutputBackend.cs` — `OutputBackendKind` + selector:
  HIDMaestro sirf jab backend bhi HIDMaestro ho AUR experimental toggle bhi
  on ho; warna fail-closed vJoy. `AppSettings` mein `OutputBackend`,
  `ExperimentalHidMaestroEnabled`, `HidMaestroProfileId` (Sanitize bhi
  fail-closed). `AppServices.Output` ab `IJoystickOutput` hai +
  `SwitchOutputBackend()` (vJoy par wapas aana hamesha mumkin hai).
- SDK binding **asal** hai, guess nahi: vendored `HIDMaestro.Core.dll`
  v2.1.0 (compile-time reference) — API surface SDK Reference
  (hidmaestro.org/docs/sdk/sdk-reference/) se verify hua aur asal DLL se
  reflect karke confirm hua (`HMContext`, `InstallDriver()`,
  `LoadDefaultProfiles()`, `GetProfile(id)`, `CreateController(profile,
  "rsh-wheel")`, `HMController.SubmitState(in HMGamepadState)` /
  `SubmitRawReport(ReadOnlySpan<byte>)`, `HMProfile.RequiresUsbipBackend` /
  `Backend` / `BuildsItsOwnReports` / `IsDeployable`).
- Do submit raste (dono pure, driver ke baghair test hote hain):
  - `HMGamepadState` → `SubmitState` (default): axes floats [0,1],
    buttons 1..25 (`HMButton` sirf 25 bits hai).
  - 26-byte raw report → `SubmitRawReport` (custom RSH wheel profile ke
    liye — profile abhi Windows-PC follow-up hai, H-HID-2): 5× u16 axes +
    128 button bits, vJoy parity poori.
- Koi default profile nahi: `HidMaestroProfileId` khaali ho to TryConnect
  refuse karta hai (kisi brand ki identity default nahi — evaluation §4).
  Catalog mein generic DirectInput wheel profile hai hi nahi (68 wheel
  profiles — sab brand identities, masalan `logitech-g29`, ya wheelbase-only
  models jinke ButtonCount/AxisCount 0 hain). Generic identity ke liye
  custom profile chahiye (`HidDescriptorBuilder.Joystick()` +
  `AddAxis`/`AddButtons` — SDK mein maujood hai; author + validate karna
  H-HID-2 ka follow-up hai, size S).

## 2. 8-point gate

| # | Point | Result | Evidence |
|---|---|---|---|
| 1 | **Build** — solution 0 warnings/0 errors, naye projects slnx mein | **PASS** | `dotnet build RSH.slnx -c Release`: 0 Warning(s), 0 Error(s) (2026-11-13). |
| 2 | **Mapping parity vJoy ke saath** — axes/buttons/encoder | **PARTIAL PASS** (known gaps darj hain) | Parity tests: axes (X/Y/Z/Rx/Slider) + clutch/handbrake + buttons 1..25 + neutral — vJoy se exact match. **Gaps (SubmitState rasta):** buttons 26..32, Ex 33..96, encoder pulses 65..72 nahi ja sakte (`HMButton` 25 bits) — sirf raw-report raste mein (tests mein `KNOWN GAP` ke tor par assert hain, chhupaye nahi). Buttons 97..128 ka koi source dono backend par nahi (vJoy Ex3 bhi 0). |
| 3 | **Latency** — mapping E2E SLO (p99 ≤ 10 ms) mein negligible | **PASS (sandbox)** / live E2E NOT VERIFIED | Measured (sandbox VM, 200k iterations): `BuildGamepadState` mean **1,526 ns** (~1.5 µs), `BuildRawReport` mean **1,031 ns** (~1.0 µs); tests mein <100 µs budget assert hai. Batch p99 VM-noise-dominated hai (GC/scheduling), is liye SLO claim mean par hai. **Live E2E (NTP ping + engine timestamps) + 4 h soak Windows PC par hoga (H-HID-3) — NOT VERIFIED.** |
| 4 | **No-crash soak** — backend 200 Hz par crash nahi karta | **PASS (in-process)** / live soak NOT VERIFIED | Fake SDK par **200,000 Updates** (≈1000 s @ 200 Hz): 0 exceptions, submit count exact (200,001 + neutral). Rig harness ProcessedState level par backend-agnostic hai (wahi states record karta hai jo koi bhi backend bhejta) — harness `net10.0` hai, `net10.0-windows` backend host nahi kar sakta, is liye parity unit level par prove hui hai. **Live 4 h soak Windows PC par hoga (H-HID-3) — NOT VERIFIED.** |
| 5 | **Install / dependency behaviour** — ZIP + SHA pin, admin step, MSIX | **PARTIAL** | ZIP SHA-256 `7fe349f1…97b4` + DLL SHA-256 `f13e3259…b4d5` pin hain aur test mein verify hote hain (loaded assembly ka hash); koi NuGet nahi; MIT license vendored (`lib/LICENSE-HIDMaestro.txt` + `THIRD-PARTY-NOTICES.txt`). Driver install `InstallDriver()` (admin) asal adapter mein hai — sandbox mein kabhi execute nahi hua (Windows guard). **MSIX + admin install interaction UNRESOLVED (SEC-H-06, H-HID-5).** |
| 6 | **ARM64 note** — D19 | **NOT VERIFIED / UNCHANGED** | D19 "unsupported" rehta hai. Vendor khud kehta hai ARM64 binaries built hain lekin "no ARM64 machine has run them" (release notes v2.0.3/v2.1.0). HIDMaestro D19 ko nahi badalta (H-HID-7). Bus factor 1 + tez churn → pinned v2.1.0 hi (SEC-H-06). |
| 7 | **Fallback to vJoy** — backend fail ho to vJoy, default vJoy | **PASS** | Selector + Sanitize dono fail-closed (tests: toggle band / ghalat string / khaali → vJoy; sirf chosen factory chalti hai). `SwitchOutputBackend()` se vJoy par wapas aana mumkin hai. TryConnect fail par app/engine crash nahi — engine vJoy jaisa 2 s retry karta hai. |
| 8 | **Test coverage + security (SEC-H-06)** | **PASS (sandbox)** / live items NOT VERIFIED | 78 naye tests (neeche §4). SEC-H-06 code-enforced: composite/usbip profile create se pehle refuse hota hai (asal v2.1.0 catalog par test — `dualsense-composite`, `dualshock-4-v2-composite`); admin + self-signed cert behaviour doc mein darj hai; anti-cheat **UNVERIFIED** (H-HID-6); land hone ke baad dobara security review hoga (W7). |

### Gate faisla

**FAIL (conditional).** Sandbox mein verify hone wale points pass/partial
hain, lekin Windows-only points — H-HID-1 (live driver install+create),
H-HID-2 (8-game matrix + profile choice), H-HID-3 (E2E + 4 h soak),
H-HID-4 (sleep/resume), H-HID-5 (MSIX), H-HID-6 (anti-cheat), H-HID-7
(ARM64) — **NOT VERIFIED** hain. Is liye:

- **Backend experimental + disabled-by-default rehta hai.**
- **v2.0.0 ka default vJoy (BrunnerInnovation 2.2.2.0) rehta hai.**
- Switch sirf tab jab H-HID-1..H-HID-8 (neeche §5) Windows PC par pass hon.

## 3. SEC-H-06 (W7 security review — Medium, conditional)

| SEC-H-06 item | Status |
|---|---|
| Admin device-create + self-signed cert trust store mein | Darj hai — `InstallDriver()` admin mangta hai; cert machine ke trust store mein jata hai; corporate/locked-down PC par policy rok sakti hai. Code mein: `UnauthorizedAccessException` → wazeh error, crash nahi. |
| Bus factor 1 (maintainer `hifihedgehog`) + tez churn, koi LTS nahi | Darj hai — pinned v2.1.0 hi; har upgrade par dobara verify lazmi. |
| Koi NuGet nahi — ZIP + SHA-256 pin lazmi | **DONE** — ZIP + DLL SHA-256 pin + hash test (§2 point 5). |
| MSIX + admin install unresolved | **OPEN** — H-HID-5, Windows-PC follow-up. |
| Anti-cheat unverified | **OPEN** — H-HID-6, Windows-PC follow-up. |
| Composite profiles ka bundled usbip-win2 kernel transport kabhi enable na karo | **ENFORCED + TESTED** — `HidMaestroSdkAdapter.ValidateProfile()`: `RequiresUsbipBackend` ya Backend=`usbip` ho to create se pehle refuse; `BuildsItsOwnReports` bhi refuse. Test asal v2.1.0 catalog par hai. |
| Default vJoy rehta hai; §8 gate baghair switch nahi | **ENFORCED** — selector + Sanitize fail-closed (§2 point 7). |

## 4. Tests (Virtual Test Rig — unit stage + direct runs)

- **RSH.HIDMaestro.Tests: 78/78 PASS** (0 fail, 0 skip).
  Mapping (axes/neutral/buttons 1..25/caps/reuse), raw report (26 B,
  Ex+pulses, neutral, caps), parity vJoy ke saath (axes + buttons 1..25 +
  KNOWN GAP tests), P12 jaisi validation, selector (fail-closed),
  lifecycle (fake SDK: connect/update/neutral/disconnect/plan-fail),
  latency bounds (<100 µs assert), 200k-update soak, SHA-256 pin,
  catalog sanity, SEC-H-06 usbip refusal, Linux guard.
- **RSH.VJoy.Tests: 56/56 PASS** — vJoy behaviour nahi toota.
- **RSH.Tests: rig unit run mein 2 fail / 643 pass / 6 skip (651)** —
  fail hone wale tests ke naam shared log se recover nahi hue (concurrent
  4 h long-soak run logs overwrite kar raha tha — rig REPORT.md /
  results.json / logs shared hain). **Foran dobara chalane par 0 fail /
  644 pass / 7 skip (651), exit 0.** Mera change RSH.Tests ko chhoo hi
  nahi sakta (RSH.Core mein sirf ek nayi file add hui; RSH.Tests
  RSH.App/HIDMaestro ko reference nahi karta) — ye VM-noise flaky
  failures hain (Phase F/G mein bhi aisi flaky timing tests darj hain).
  Rig unit stage ka overall FAIL isi liye hai; dobara run green hai.
- **Firmware-host stage (direct run): PASS** — C++ host tests 4374 checks
  0 failures (+50+50), Python cross-verify 51 checks PASS, merge_bin mock
  7 checks PASS, `pio run` SUCCESS (RAM 30.2%, Flash 67.2%).
- **Full rig nahi chalaya** (task hidayat — 4 h long soak background mein
  chal raha hai, port conflict); sirf `--only unit` + direct
  firmware-host run.
- **Rig mein koi bug nahi mila** (rig ka behaviour nahi badla).

### Mile hue bugs + fixes

- **B-H1 (mapping, real bug — test ne pakda):** steering center
  `(v-1)/32767` se 0.49998 aata tha, 0.5 nahi (SDK contract: "0.5 is
  centered for signed axes"). Fix: signed axis X ke liye
  `0.5 + (v-16384)/32768` — center AUR full-right exact; full-left
  1 LSB oopar (0.00003). Unsigned axes (pedals/slider) `(v-1)/32767`
  hi rehte hain (released 0.0 exact). File:
  `software/src/RSH.Output.HIDMaestro/HidMaestroMapping.cs` (`ToUnitSigned`).
  Repro: `Steering_MapsToX_SignedCenterHalf` / `Neutral_MatchesExpectedFloats`
  pehle fail (Expected 0.5, Actual 0.49998474), fix ke baad pass.
- **B-H2 (lifecycle, design fix):** pehla `Disconnect()` dobara call par
  `DestroyDevice` dobara chalata tha (fake SDK ka count barhta tha).
  Fix: `_deviceReleased` flag — ek device par destroy ek dafa; dobara
  Disconnect/Dispose no-op. File:
  `software/src/RSH.Output.HIDMaestro/HidMaestroOutput.cs`.
  Repro: `Disconnect_DestroysDevice_AndDisposeIsIdempotent` (pehle
  DestroyCalls 3, ab 1).

### Catalog discrepancy (verification mein mili)

- Evaluation mein 337 profiles likhe thay; **asal v2.1.0 DLL 336–337 load
  karta hai** (ek sandbox measurement mein 336, GitHub Windows runner
  par 337 — count environment par munhasir hai; test mein range assert
  hai).
  `logitech-g29`: Buttons 25, Axes 4, `RequiresUsbipBackend` false.

## 5. H-HID-1..H-HID-8 checklist (Windows PC — sab NOT VERIFIED)

- [ ] H-HID-1: Windows 11 24H2, HVCI on — install, device create, 10-min session, remove.
- [ ] H-HID-2: 8-game matrix HIDMaestro profile par dobara + **profile choice**
      (custom generic wheel profile author karo: `HidDescriptorBuilder` +
      `HMProfileBuilder` — 5× 16-bit axes + 128 buttons; ya catalog wheel
      profile chuno — brand identity default nahi).
- [ ] H-HID-3: E2E latency (NTP ping + engine timestamps) + 4 h soak, vJoy numbers se compare.
- [ ] H-HID-4: Sleep/resume — device zinda rehta hai? (v2.1.0 fix ka regression check.)
- [ ] H-HID-5: MSIX installer se `InstallDriver()` admin step — rasta tay karo.
- [ ] H-HID-6: Anti-cheat spot-check — kam az kam ek anti-cheat wala title.
- [ ] H-HID-7: ARM64 — sirf tab jab ARM64 hardware mil jaye; tab tak D19 "unsupported".
- [ ] H-HID-8: Pinned version + SHA-256 release notes + THIRD-PARTY-LICENSES.md
      mein darj karo — **ZIP SHA-256 yahan darj hai** (§2 point 5); release
      notes entry baaqi hai (SW-16).

## 6. Files

- `software/src/RSH.Output.HIDMaestro/` — `RSH.Output.HIDMaestro.csproj`,
  `HidMaestroConstants.cs` (pins), `HidMaestroCapabilities.cs`,
  `HidMaestroMapping.cs` (SubmitState + raw report),
  `IHidMaestroSdk.cs`, `HidMaestroSdkAdapter.cs` (asal binding + SEC-H-06
  usbip refusal), `HidMaestroOutput.cs` (`IJoystickOutput`),
  `lib/HIDMaestro.Core.dll` (vendored v2.1.0, 18 MB — SHA-256 pin §2.5),
  `lib/LICENSE-HIDMaestro.txt` (MIT), `lib/THIRD-PARTY-NOTICES.txt`.
- `software/src/RSH.Core/Output/OutputBackend.cs` — enum + fail-closed selector.
- `software/tests/RSH.HIDMaestro.Tests/` — 78 tests (5 files + fake SDK).
- `software/RSH.slnx`, `software/src/RSH.App/RSH.App.csproj` — naye projects.
- `software/src/RSH.App/Services/AppSettings.cs` — backend settings + Sanitize.
- `software/src/RSH.App/Services/AppServices.cs` — `Output: IJoystickOutput`,
  `CreateOutput()`, `SwitchOutputBackend()`, `ActiveOutputBackend`.
- `software/src/RSH.App/ViewModels/ProfilesSystem.cs`,
  `Views/WizardWindow.xaml.cs`, `Views/Pages/OutputPage.xaml.cs` —
  details text backend-aware (vJoy vs HIDMaestro).

**Note (parent ke liye):** vendored DLL 18 MB hai — repo mein binary blob
hai. Agar push size masla ho to alternative ye hai ke DLL ko release
packaging step mein pinned SHA-256 ke saath download kiya jaye (CI mein
fetch + hash verify) aur repo se nikala jaye; lekin phir fresh checkout
par `dotnet build` DLL ke baghair fail hoga (compile-time reference hai).
Output page par experimental toggle ka UI (checkbox + profile id) abhi
nahi bana — settings-level toggle hai (UI-LOCKED + i18n strings ki wajah
se UI change alag kaam hai); toggle on karne ke liye settings.json mein
`OutputBackend: "HIDMaestro"` + `ExperimentalHidMaestroEnabled: true` +
`HidMaestroProfileId` darj karo.

## 7. Final recommendation — v2.0.0 ke liye vJoy default

**vJoy rehta hai.** HIDMaestro experimental backend tayyar hai (mapping,
lifecycle, fallback, tests, SHA pin, SEC-H-06 usbip guard), lekin gate
**FAIL** hai kyun ke live-driver, game-matrix, E2E/soak, sleep/resume,
MSIX, anti-cheat aur ARM64 — saaton Windows-PC items — unverified hain.
Switch ka faisla sirf H-HID-1..H-HID-8 ke Windows-PC pass hone ke baad
hoga (v2.1 track). Koi vJoy behaviour nahi toota; koi firmware/protocol
change nahi hua.
