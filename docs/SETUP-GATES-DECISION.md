# Setup wizard gates vs IsSane + NeedsRecalibration triggers — faisla (Phase E WS7)

> Core only. XAML / ViewModels (SetupViewModel, CalibrationTuning) Wave 2 ki hain —
> is doc mein un ke liye exact values + exact strings hain, code change nahi.
> Authority: plan §0 D10 (span ≥ 20% full-scale mV, 0–3300 mV) + §5 Phase C/E.

## 1. Masla — wizard gates vs IsSane mismatch (root cause)

Setup wizard ke gates aur `IsSane` ka **unit aur value dono** mukhtalif thay:

| Jagah | Gate | mV mein (asliyat) | D10 (660 mV) se |
|---|---|---|---|
| `SetupViewModel` step 1 (steering) | 250 raw/side (`SteerSide`) | v1 raw: 250/side → kul span 500 raw ≈ **403 mV**; v2.1 mV: 250/side → **500 mV** | **kam** |
| `SetupViewModel` step 2 (pedals) | 150 raw span (`PedalSpan`) | v1 raw: ≈ **121 mV**; v2.1 mV: **150 mV** | **bohat kam** |
| `SteeringCalibration.IsSane` / `PedalCalibration.IsSane` | span ≥ 660 mV | **660 mV** | — |
| `AutoCalibrator` | 660 mV (Phase C mein fix ho chuka) | **660 mV** | — |

**Root cause:** wizard ke gates pre-D10 raw-ADC zamane ke hain (250/150 raw units) —
jab `IsSane` D10 (660 mV) par gaya to `AutoCalibrator` update hua, lekin
`SetupViewModel` ke private constants kabhi update nahi hue. Wizard live values ko
**packet units** mein napta tha, jab ke `IsSane` mV mein napta hai — v1 raw aur
v2.1 mV dono mein gate D10 se neeche tha.

**Asar (user-visible bug):** wizard "Left end ✓ captured" / "Accelerator ✓ captured"
keh kar aage barha deta tha, phir Finish par `IsSane` reject kar deta tha —
`"Those values don't look right. Press Restart and try again."` User ne wizard
ki har hidayat mani, phir bhi bina wajah bataye reject. Repro (unit test):
centre 2050, har taraf 250 → min 1800 / max 2300, span 500 mV → `IsSane == false`
(`PhaseERecalibrationTests.MinimumPassingWizardCapture_IsSane_OldGatesWereNot`).

## 2. Faisla — gates barhao (hint bhi dikhao, hint akela nahi)

**Faisla: gates D10 par barhao.** "Sirf hint dikhao, gates wahi rakho" reject kiya —
jo gate pass karwaye aur Finish reject kare, wo toota hua contract hai; wizard
kabhi aisi kamyabi ka waada na kare jo poori na ho sake. Hints phir bhi zaroori
hain (user ko batana hai kitna aur ghumana/dabana hai) — is liye dono.

### 2.1 Exact gate values (mV — Core: `RSH.Core.Processing.SetupGates`)

| Gate | Value | Core constant |
|---|---|---|
| Steering, har taraf centre se | **≥ 330 mV** | `SetupGates.MinSteeringSideMillivolts` (= `SteeringCalibration.MinSideMillivolts`) |
| Steering, kul span | **≥ 660 mV** (330 + 330 se khud ban jata hai) | `SetupGates.MinSteeringSpanMillivolts` |
| Pedal (accel / brake), span | **≥ 660 mV** | `SetupGates.MinPedalSpanMillivolts` |

Helpers: `SetupGates.SteeringSideComplete(mv)`, `SteeringRangeComplete(leftMv, rightMv)`,
`PedalComplete(spanMv)` — sab mV input lete hain.

**Per-side 330 mV — IsSane se jaan boojh kar sakht:** `IsSane` sirf kul span
(≥ 660 mV) + ordering mangta hai. Sirf kul span par gate lagate to 500/160 jesa
ektarfa safar pass ho jata — mapping to banti, lekin wheel asal mein ek taraf
adhoora hota. Per-side gate se wizard se guzri har calibration `IsSane` zaroor
pass karti hai (centre strictly between ho to). Alternative (kul-span-only gate)
consider kiya aur reject kiya — wajah oopar.

**Conversion rule (Wave 2 — lazmi):** gates se pehle live values ko mV mein badlo:
steering — packet ka mV flag set ho to seedha, warna
`InputMapper.RawToMillivolts(value)`; pedals v1 mein hamesha raw hote hain, is liye
hamesha `RawToMillivolts`. Finish mein ye conversion pehle se hai (V-C2) — gates
ko wahi conversion chahiye, warna mismatch wapas aayega. Raw units par gate
**kabhi mat lagao.**

### 2.2 Exact UI strings (Wave 2 verbatim dikhayega — professional English)

`SetupGates` mein code ke tor par bhi hain (single source of truth):

| Kab | String (`{n}` = baqi mV) |
|---|---|
| Left end baqi hai | `Turn the wheel further left — {n} mV more to reach the minimum range.` |
| Right end baqi hai | `Turn the wheel further right — {n} mV more to reach the minimum range.` |
| Pedal baqi hai | `Press the {accelerator\|brake} fully down — {n} mV more to reach the minimum range.` |
| Centre mashkook lage (ek taraf ka safar doosri se bohat kam, kul span kaafi) | `The centre looks off — hold the wheel straight ahead and restart this step.` |
| Finish par IsSane reject (hifazati jaal — naye gates se yahan nahi pohanchna chahiye) | `{Steering\|Accelerator\|Brake} range is too small — the wheel needs at least 660 mV from end to end. Turn it lock to lock and try again.` |

### 2.3 Wave 2 checklist (SetupViewModel — is workstream ka kaam nahi)

1. `SteerSide = 250` → `SetupGates.MinSteeringSideMillivolts` (330 mV), `PedalSpan = 150` →
   `SetupGates.MinPedalSpanMillivolts` (660 mV); `Refresh()` mein `_leftDone` / `_rightDone` /
   `_accelDone` / `_brakeDone` mV-converted values par `SteeringRangeComplete` / `PedalComplete` se.
2. `RangePercent` / `AccelPercent` / `BrakePercent` naye gates ke hisaab se (330 / 660).
3. Step 1/2 ke hint texts §2.2 wali strings se (baqi mV ke saath); centre-off case mein
   `CentreOffHint`.
4. Finish ka generic `"Those values don't look right..."` hata kar per-axis
   `CalibrationTooNarrowHint(axis)` dikhao.
5. Maloom kami (fix Wave 2 / follow-up, is WS ka scope nahi): step 0 centre ek hi
   live sample se pakra jata hai — wheel thora terra pakra ho to ek taraf ka safar
   kam par jata hai aur gate rok deta hai. `AutoCalibrator.TryGetCenter` (trimmed mean,
   20+ samples) jesa robust centre behtar hoga; tab tak centre-off hint hi sahara hai.

## 3. NeedsRecalibration flag — triggers + clear path

`WheelProfile.NeedsRecalibration` Phase C se mojood tha (sirf insane-calibration /
migration case). Phase E WS7 ne triggers barhaye — **persisted flag vs live check
ka farq samajhna zaroori hai:**

- **Persisted flag** (`NeedsRecalibration`, `Sanitized()` se dobara hisaab): sirf
  **insane SET calibration** (D10: mV tag nahi, ya span < 660 mV). Load par bhi
  yehi hisaab hota hai — no-board / migration case ke liye.
- **Live check** (`WheelProfile.CheckRecalibration(CalibrationContext)` — pure,
  kuch nahi badalta): persisted rule + neeche wale teeno triggers. **Wave 2 ki
  prompt yehi parhti hai** jab board connected ho. Live triggers persist nahi
  hote — wo live board state se derive hote hain, is liye `Sanitized()` inhein
  dobara hisaab nahi karta.

### 3.1 Trigger table

| # | Trigger | Kab set hota hai | Kab nahi lagta | Wajah (reason) |
|---|---|---|---|---|
| T1 | Insane calibration | Koi SET calibration D10 par insane ho (unit tag nahi / span < 660 mV / range se bahar) | Koi calibration set hi na ho (pehli dafa = normal flow) | `InsaneCalibration` |
| T2 | Pin map badla | Profile par saved `CalibrationPinMapFingerprint` ≠ board ka maujooda fingerprint (`CalibrationFreshness.PinMapFingerprint` — GET_CAPS pin table se, `pin:kind:module` pin-order mein) | Koi bhi taraf fingerprint null ho (legacy profile / GET_CAPS nahi mila) — andaza nahi lagaya jata | `PinMapChanged` |
| T3 | Firmware badla | Saved `CalibrationFirmwareVersion` vs board version — **major ya minor** mukhtalif ho | Patch-only (4.0.0 → 4.0.1) — faisla: semver ke mutabiq patch bug-fix hota hai, ADC path nahi badalta; ADC ko chhoone wali tabdeeli minor/major bump karti hai. Koi bhi taraf version null / na-parse ho to trigger nahi (parse na hone par seedha string muqabla — mukhtalif strings = badla) | `FirmwareChanged` |
| T4 | Calibration stale | `CalibratedAtUtc` se `NowUtc` ka farq **> 180 din** ho | 180 din ya kam; tareekh null ho; tareekh mustaqbil mein ho (clock skew — stale nahi) | `StaleCalibration` |

- **Stale policy (180 din) faisla hai, measured nahi:** potentiometer drift + 6 mahine ka
  nudge — koi safety daawa nahi. Constant: `CalibrationFreshness.StaleAfterDays = 180`.
- **Context triggers sirf tab lagte hain jab kam se kam ek calibration SET ho** —
  pehli dafa setup par (koi calibration nahi) pin map / firmware / tareekh kuch bhi
  kahe, flag nahi lagta.
- **Koi bhi trigger `CheckRecalibration` se milta hai:** natija `RecalibrationCheck`
  (`NeedsRecalibration`, `Reasons` flags, `PromptMessage`) — ek se zyada wajah ek
  saath lag sakti hai, prompt mein sab jumle aate hain.

### 3.2 Prompt strings (exact — Wave 2 verbatim)

| Reason | Prompt jumla |
|---|---|
| `InsaneCalibration` | `Your calibration looks out of range, so the wheel needs to be calibrated again.` |
| `PinMapChanged` | `The wiring (pin assignment) changed since this calibration was made, so the wheel needs to be calibrated again.` |
| `FirmwareChanged` | `The board firmware changed since this calibration was made, so the wheel needs to be calibrated again.` |
| `StaleCalibration` | `This calibration is over 6 months old, so the wheel needs to be calibrated again.` |

Code: `CalibrationFreshness.PromptMessageFor(reasons)` — `CheckRecalibration` yehi deta hai.

### 3.3 Clear path — kab saaf hota hai

Kamyaab recalibration par Wave 2 wizard ke Finish mein ye kare:

1. Nayi (sane) calibrations profile par lagao (pehle jaisa).
2. `profile.MarkCalibrated(currentPinMapFingerprint, currentFirmwareVersion, DateTimeOffset.UtcNow)`
   call karo — ye stamps (pin map + firmware + waqt) lagata hai aur `Sanitized()` se
   guzarta hai. Context `CalibrationContext.FromBoard(boardCaps, now)` se banao.
3. Us ke baad usi context par `CheckRecalibration` → `NeedsRecalibration == false`;
   sane calibrations hon to persisted flag bhi saaf ho jata hai.

Stamps profile JSON mein save hote hain (`CalibrationPinMapFingerprint`,
`CalibrationFirmwareVersion`, `CalibratedAtUtc`) — `Sanitized()` inhein normalise
karke mehfooz rakhta hai (khaali/ghalat → null). `ProfileStore` round-trip test hai.

### 3.4 Core API (nayi / badli hui)

| API | Kaam |
|---|---|
| `SteeringCalibration.MinSideMillivolts` (= 330) | Per-side gate value |
| `SetupGates` (`RSH.Core.Processing`) | Gate constants + `SteeringRangeComplete` / `PedalComplete` + §2.2 hint strings |
| `RecalibrationReason` (flags) | `None / InsaneCalibration / PinMapChanged / FirmwareChanged / StaleCalibration` |
| `CalibrationContext` (+ `FromBoard(BoardCaps, now)`) | Live context: pin-map fingerprint + firmware version + waqt |
| `CalibrationFreshness` | `StaleAfterDays = 180`, `PinMapFingerprint(caps)`, `FirmwareChanged(old, new)`, `PromptMessageFor(reasons)` |
| `WheelProfile.CheckRecalibration(ctx)` → `RecalibrationCheck` | Live check — Wave 2 prompt yehi parhe |
| `WheelProfile.MarkCalibrated(fp, fw, at)` | Recalibration par stamps lagao + flag saaf karo |
| `WheelProfile.CalibrationPinMapFingerprint / CalibrationFirmwareVersion / CalibratedAtUtc` | Stamps (JSON mein save hote hain) |

## 4. Tests + verification

- Naye tests: `software/tests/RSH.Tests/PhaseERecalibrationTests.cs` — **19 tests**
  (gates values/boundaries, purane gates ka regression, exact hint strings,
  har trigger + boundary + unknown-stamp cases, multi-reason, MarkCalibrated clear,
  Sanitized normalise, fingerprint determinism, ProfileStore round-trip).
- `dotnet test` (RSH.Tests): **356 pass / 0 fail / 5 skip (kul 361)** — do dafa green
  (my files ke ilawa tree tab saaf tha). Baad mein parallel workstreams ke edits se
  tree mein 394 tests hue; aakhri run **387 pass / 1 fail / 6 skip** — fail sirf
  A9 workstream ka `ProfileImportAsyncTests.SaveAsync_CancelledToken_ThrowsBeforeWriting`
  hai (exact `OperationCanceledException` vs `TaskCanceledException`), WS7 ka nahi.
- Rig: `steering-calibration` scenario — §5 dekho.

## 5. Rig verification

`./run-virtual-rig.sh --only steering-calibration` (test-env) — **PASS**
(2026-11-13, quick mode, 20 s): steeringAxisMin=1, steeringAxisMax=32768,
accelAxisMax=32639, brakeAxisMax=32603, validPackets=1891, lossPercent=0,
duplicatesOrLate=0. Note: is run mein rate 113/s aur tick jitter p99 ~74 ms
baseline (196/s, p99 ~935 µs) se kharab thay — VM par parallel workstreams ke
builds/tests chal rahe thay (shared 1.5 CPU); scenario ke apne assertions
(sirf rails) sab pass hue, aur WS7 ki tabdeeli harness path ko chhooti hi nahi
(additive Core API), is liye ye environment noise hai, regression nahi.
