# RSH Round 2 — W7 Data & persistence (Data & AI team) — 2026-11-14

Repo local HEAD 87c2223 (app v2.0.4 / firmware v4.0.1 / protocol v2.1).
Scope: profiles/settings/persistence/telemetry gaps + Round 1
(`docs/POLISH-SWEEP-2026-11-13.md` §2/§6) deferred items. Koi git
command nahi chalaya gaya, koi nayi feature nahi, plan v2.3 edit nahi hua.

## 1. Fixed is stream mein (W7-1 – W7-6)

| # | Sev | Finding | Fix (file:line) | Verification |
|---|---|---|---|---|
| W7-1 | Medium-Low | `TelemetryWatcher` game band hone par `LatestStableSnapshot` saaf karta tha (Round 1) lekin `LatestRawSnapshot` purani car dikhata rehta tha — `ProfilesSystem.FillCarFromTelemetry` ("current car" button, `ViewModels/ProfilesSystem.cs:140`) game band hone ke baad bhi purani car bhar deta tha. | `Core/Telemetry/TelemetryWatcher.cs:113` — null/empty-car branch mein `LatestRawSnapshot = null;` | `DataAiPolishTests.WatcherClearsStableSnapshotWhenGameClosesAndReemitsOnReturn` extended (raw assert) — pass |
| W7-2 | Low | `NtpPingClient.PingAsync` mein race: `_nextSeq++` non-atomic + throttle check-then-Exchange — do concurrent calls ek hi 250 ms window mein do pings bhej sakti thin (SEC-C-24 flood guard bypass) aur ek hi seq dobara mil sakta tha. | `Core/Transport/NtpPing.cs` — seq `Interlocked.Increment` (int counter, ushort cast par wrap waisa hi), throttle claim `CompareExchange` loop | Naya test `PingClient_ConcurrentCalls_OnlyOnePingSent` (8 concurrent calls → 1 exchange, 1 sent, 7 throttled) — pass |
| W7-3 | Low | `EncoderTracker.Update` mein `cumulative[..n]` — caller span `encoderCount` se chhoti ho to `IndexOutOfRangeException` pipeline tick ke andar. Packet path par kabhi nahi hota (`WheelPacketV2` parse lambai pakka karta hai) — defense in depth. | `Core/Pipeline/EncoderTracker.cs` — `n` ko `cumulative.Length` par clamp; khaali span par baseline nahi banti | 2 naye tests (`ShortCumulativeSpan`, `EmptyCumulativeSpan`) — pass |
| W7-4 | Low | `PinMapStore.TryParseFile` sirf JSON parse check karta tha — GPIO 999 ya 15+ entries wali file bhi "valid" lagti thi, is liye `Save` ka backup guard use achi backup bana leta tha. (Restore path `DiagnosticsSettings.TryValidatePinMap` ye check pehle se karta tha.) | `App/ViewModels/PinMapStore.cs:292` — `PinMap.TryValidateEntries` bhi lagao | 3 naye tests (reject 999, reject 16 entries, accept valid) — pass |
| W7-5 | Low | `AppSettingsStore` ke paas sirf ek `.bak` tha — current + `.bak` dono kharab hon to settings hamesha ke liye defaults ho jati thin (ProfileStore ke paas 3-gen hai). | `App/Services/AppSettings.cs` — Load chain `.bak` → `.bak1` → `.bak2`; Save rotation ProfileStore wala guarded pattern (kharab `.bak`/`.bak1` aage nahi barhta, kharab `.bak1` achi `.bak2` ko overwrite nahi karta) | 3 naye tests (fallback `.bak1`, 4-generation rotation, corrupt `.bak1` vs good `.bak2`) — pass |
| W7-6 | Low | `HelloClient` comments ghalat: "HELO 5 byte" / "v2 HELO frame (5 B)" — asal frame 6 B hai (`"HELO"` + proto `0x02` + sessionFlag; `BuildHello` 6 bytes deta hai, protocol §2.1 mein sessionFlag 6th byte hai). | `Core/Transport/HelloClient.cs:11,48` — "6 byte" / "(6 B)" | Code comment only; `BuildHello` ka behaviour unchanged — existing Hello tests pass (full suite) |
| GAP-02 residue | Medium | Code floor Round 1 mein 19045 ho chuka tha, lekin docs peeche theen: `SUPPORT-PERIOD.md:21-22` abhi bhi "Windows floor: Windows 11 … Win10 best-effort, koi guarantee nahi" kehta tha; `USER-GUIDE.md:22` "Win10 build 19041+" kehta tha jab ke code floor 19045 hai (19041–19044 update check se refuse hota). | Dono docs theek ki gayin: floor = Windows 10 22H2 (build 19045); v2.0.4 note add kiya | Grep verification: `19041` ab sirf tareekhi `RELEASE-NOTES-v2.0.3.md:184` + code comment (19041 = unsupported ki misaal) mein hai |

## 2. Round 1 ke daawe dobara verify kiye (koi dobara fix nahi chahiye tha)

- **ProfileStore** (`Core/Profiles/ProfileStore.cs`): v1→v2 migration
  (`.v1.bak` sirf asal v1 0/1 ke liye, corrupt file ke liye nahi —
  `OnDiskSchemaVersion` parse-fail par -1), 3-gen backup (kharab
  `.bak1` achi `.bak2` ko overwrite nahi karta), negative
  `SchemaVersion` reject, `Save` caller ki list truncate nahi karta,
  1 MB cap + capped read, `TryValidateFile` strict (skip entry →
  reject). Sab maujood + Round 1 tests pass.
- **PinMapStore** (`App/ViewModels/PinMapStore.cs`): 64 KB cap,
  tolerant per-entry parse, guarded `.bak` — maujood.
- **KeyStore** (`Core/Security/KeyStore.cs`): 64 KB blob cap, capped
  read, OOM catch, atomic temp+move save — maujood. Koi gap nahi mila.
- **AppSettings** (`App/Services/AppSettings.cs`): corrupt/tolerant
  load (capped read, Sanitize, backup/defaults), password kabhi disk
  par nahi (`[JsonIgnore]` + legacy migration) — maujood. (3-gen
  backup W7-5 mein add hua — yehi ek gap tha.)
- **Telemetry readers**: iRacing missing `SessionState` var → Unknown
  (not Menu) (`IRacingSharedMemoryReader.cs`); AC/ACC status mapping
  theek; `TelemetryWatcher` stable clear (Round 1) + raw clear (W7-1);
  `CarProfileResolver` exact-game-beats-any-game — maujood, koi gap
  nahi mila.
- **GAP-04 (Low) — VERIFIED FIXED, koi action nahi:** version ek hi
  hai — `software/Directory.Build.props:13` = 2.0.4,
  `RSH.App.csproj:22` = 2.0.4, `RSH.RenderTest.csproj:11` = 2.0.4.
- **F4 (release workflow, Major) — VERIFIED FIXED, is stream se
  bahir:** `release.yml:254-289` mein `actions/checkout@v4` +
  `gh release create --target "$GITHUB_SHA"` maujood hai
  (dispatch galat commit par release ka rasta band hai). Koi action nahi.
- **Sync file write UI thread par — VERIFIED FIXED (QA F3/U-P1):**
  `SaveSettings()` / `SaveProfilesAndApply()` sync methods abhi bhi
  maujood hain lekin koi UI-thread caller nahi: 3 sites
  (`MainWindow.xaml.cs:319`, `App.xaml.cs:296`,
  `WizardWindow.xaml.cs:553`) `SaveSettingsAsync()` istemal karti
  hain; auto-switch (`App.xaml.cs:356`) async hai; aakhri sync caller
  `ProfilesSystem.cs:183` `Debouncer` (ThreadPool, `Core/Utilities/
  Debouncer.cs:28`) se chalta hai; `PinMapStore.Save` dono sites
  (`DiagnosticsSettings.cs:405,448`) `Task.Run` mein hain.

## 3. Deferred items ke faisle

| Item | Faisla | Wajah |
|---|---|---|
| GAP-02 (Medium) | **FAISLA: shipped floor = Windows 10 22H2 (build 19045).** Code (`RuntimeCheck.MinimumWindowsBuild = 19045`, `UpdateCheck.cs:38-42`) + docs (§1 GAP-02 residue) ab ek hain. Plan v2.3 §6 ka "Windows 11 floor" text FINAL hai — edit nahi hua; ye shipped-support deviation hai, code comments mein darj hai. | User ka asal PC 19045 hai; v2.0.3+ Win10 ko officially support karta hai (auto-classic). |
| GAP-04 (Low) | **CLOSED — Round 1 mein fix ho chuka** (§2). | — |
| GAP-05 (Low) | **FAISLA: DELETE (v2.0.5 mein, coordinator execute kare).** `Views/Pages/ConnectionPage.xaml(.cs)` + `CalibrationPage.xaml(.cs)` ka repo mein koi navigation/code reference nahi (grep = sirf apni files + generated `.g.cs`); Setup flow unke VMs reuse karta hai, pages nahi. Delete safe hai (SDK globbing — files hatane se build se nikal jayengi; build gate se verify hoga). Is Round 2 mein delete NAHI kiya — UI LOCKED, sirf faisla + doc (task ki shart). `docs/UI-LOCKED.md` kept-pages note bhi update kar diya gaya. | Orphan pages dead weight hain; link karna Setup flow mein nayi navigation (UI change) hota, jab ke VMs pehle se reuse ho rahe hain — link ka koi faida nahi. |
| F4 | **CLOSED — verified fixed** (§2). Is stream ka kaam nahi tha. | — |
| i18n (~283 strings — asal mein 337) | **DEFERRED — is stream se bahir.** resx layer mukammal hai: `Strings.resx` / `Strings.ur.resx` / `Strings.hi.resx` — 337/337/337 keys, 0 khaali values. Baaqi kaam ~35 hardcoded XAML literals (`Header`/`Content`/`Text`) ka UI-string sweep hai — UI LOCKED + UI stream ka kaam, data stream ka nahi. Agle UI sweep mein uthaya jaye. | Data integrity par koi asar nahi; resx coverage poori hai. |

## 4. Lightweight verdict

- Profile/settings I/O: chand KB files, capped reads, `SaveAsync` /
  `Task.Run` off-UI-thread (§2). Backup rotation cost negligible
  hai — `Save` par zyada se zyada 3 chhoti files dobara parse hoti
  hain (`ParsesOk`), koi UI-thread I/O nahi.
- `ProfileStore` / `AppSettingsStore` rotation: sirf tab jab maujooda
  file parse ho (guarded) — kharab file achi backup ko kabhi overwrite
  nahi karti; rozana ek `.corrupt-YYYYMMDD` copy (overwrite, jama
  nahi hoti).

## 5. Open / cross-stream flags (is stream mein fix nahi hue)

- **OPEN (Low):** `firmware/rsh/serial_transport.h:23` comment mein
  bhi HELO length ghalat hai ("6 HELO — HELO request (§2.1, 5 B
  "HELO" + proto + sessionFlag)" — 4+1+1 = 6, "5 B" ghalat). Firmware
  stream ka file hai — W7 ne chheda nahi; firmware team ko flag.
- **OPEN (Low, pre-existing):** `docs/EX3-EXTENSION.md:165` mein
  "7-byte HELO (appMinor = 2)" — ye v2.2 ka PROPOSED extension hai
  (us doc ki migration table khud kehti hai current board "6-byte
  HELO" dekhta hai), is liye stale NAHI — chheda nahi gaya.
- **Conditions (Round 1 se, unchanged):** A7 real-board run, 4-hour
  soak, SEC-H-04 review, flash encryption board-side, real-game
  verification — user-side / Windows-PC items.

## 6. Verification (is stream, 2026-11-14)

- `dotnet test tests/RSH.Tests -c Release`: **758 pass / 0 fail /
  5 skip (763 total)** — Round 1 baseline 749 pass / 5 skip + 9 naye
  tests (8 `DataAiRound2Tests` + 1 `PingClient_ConcurrentCalls`).
  Targeted: `DataAiRound2Tests` 8/8, `DataAiPolishTests` +
  `NtpPingTests` 41/41, 0 fail.
- `dotnet test tests/RSH.VJoy.Tests -c Release`: **56/56**.
- `dotnet test tests/RSH.HIDMaestro.Tests -c Release`: **78/78**.
- Note: pehli full-solution build ek dafa DLL lock (`RSH.dll` in use)
  par fail hui — doosri Round 2 teams usi waqt isi shared repo tree
  mein build kar rahi thin (transient contention, code issue nahi);
  dobara chalane par build + tests green. Ek beech ki full-suite run
  mein 756 pass / 7 skip aaye thay (doosri team ki in-flight tabdeeli
  ki wajah se) — final run 758/5 baseline se match karta hai.
- RSH.App.Tests (WPF) Linux par nahi chal sakte — Windows CI.

## 7. Badli hui files (sirf is stream ki)

- `software/src/RSH.Core/Telemetry/TelemetryWatcher.cs` (W7-1)
- `software/src/RSH.Core/Transport/NtpPing.cs` (W7-2)
- `software/src/RSH.Core/Pipeline/EncoderTracker.cs` (W7-3)
- `software/src/RSH.App/ViewModels/PinMapStore.cs` (W7-4)
- `software/src/RSH.App/Services/AppSettings.cs` (W7-5)
- `software/src/RSH.Core/Transport/HelloClient.cs` (W7-6, comments)
- `software/tests/RSH.Tests/DataAiRound2Tests.cs` (nayi — 8 tests)
- `software/tests/RSH.Tests/DataAiPolishTests.cs` (watcher test extended)
- `software/tests/RSH.Tests/NtpPingTests.cs` (concurrency test)
- `software/tests/RSH.Tests/RSH.Tests.csproj` (AppSettings.cs link — WPF-free file, maujooda pattern)
- `docs/SUPPORT-PERIOD.md`, `docs/USER-GUIDE.md` (GAP-02 residue)
- `docs/UI-LOCKED.md` (GAP-05 faisla note)
- `docs/ROUND2-W7-DATA-AI.md` (ye file)
