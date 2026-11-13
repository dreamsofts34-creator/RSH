# RSH — Per-Car Profiles: Telemetry Reader Definition (Phase G Workstream 4)

**Status: IMPLEMENTED (Phase H, sandbox-verified 2026-11-13 — `RSH.Core/Telemetry/`,
`RSH.Tests/PhaseHTelemetryTests.cs`): AC + ACC + iRacing readers, watcher
(2 Hz / 2 s debounce), resolver, auto-switcher + manual override — sab
fake shared-memory buffers se test hue. Real-game end-to-end (har game par
real session mein car detect + switch) abhi bhi baaqi hai — Windows PC par
hoga (§4 acceptance). Koi game is doc se "supported" nahi hota — har game
Phase H mein real game par verify hoga.**
**Phase H corrections is doc par:** AC/ACC strings `wchar_t` hain
(carModel @68), ACC ke map naam AC wale hi hain (`acpmf_*`), SEC-H-05
rules (§2 rule 6: double-read, process-ownership check, YAML size bound).
**Sources:** Kunos AC/ACC shared-memory docs (community + `acc_shared_memory`
ports), iRacing SDK (pyirsdk docs), StintLab AC telemetry notes, RacePulse
install docs; RSH: Plan v2.3 §4 (per-module tuning) / §5 Phase G ("per-car
profiles"), `WheelProfile.GameProcessName` (auto-switch ki maujooda bunyaad),
`GAME-COMPATIBILITY.md` (8-game matrix), `PROTOCOL-V2.1.md` (output path
tabdeel nahi hota).
**Maqsad:** game jo car chala raha hai, us ke hisaab se profile khud badal
jaye — masalan GT3 car par steering scale 0.6 + brake curve X, Formula car
par scale 0.4 + brake curve Y. Aaj profile sirf **game process** se bandhta
hai (`WheelProfile.GameProcessName`); car ka pata hi nahi chalta. Ye doc wo
khalai poora karta hai: car identity kahan se aayegi, kis rate par, aur
switch kab hoga.

## 1. Kaunsi games telemetry deti hain (Phase H v1 scope)

| Game | Transport | Car identity field | Rate / notes | Phase H v1? |
|---|---|---|---|---|
| Assetto Corsa (AC) | Windows shared memory (3 blocks: physics, graphics, static — `acpmf_*`), same machine only, koi setup/plugin nahi | Static block: `carModel` (string); graphics block: session/state | Physics game rate par update hoti hai; identity ke liye 2 Hz poll kaafi hai (StintLab 10 Hz parhta hai) | **HAAN** |
| Assetto Corsa Competizione (ACC) | Windows shared memory (PhysicsMap, GraphicsMap, StaticMap — AC jaisi) | Static map: `carModel`; graphics: session type/state | Physics ~333 Hz, graphics ~60 Hz (rates vendor/community docs se); identity ke liye 2 Hz kaafi | **HAAN** |
| iRacing | Windows shared memory (official SDK) — telemetry variables (VarBuffer, triple-buffered) + session info (YAML) | Session YAML: `DriverInfo` / `CarSetup` — car path/name; telemetry var `CarIdx...` nahi, car identity YAML se | Telemetry 60 Hz (kuch vars 360 Hz); session info event-driven (session load par badalta hai) | **HAAN** |
| F1 24 / F1 25 | UDP telemetry (game settings mein ON, port + format 2025/2026 chuno) | UDP packet: car/team fields (session packet) | Game rate par broadcast; UDP hai — remote bhi mumkin | NAHI (v1.1) |
| Dirt Rally 2.0 | UDP telemetry (in-game enable) | Car identity UDP session data mein (game-specific) | Game rate | NAHI (v1.1) |
| rFactor 2 | Shared memory **plugin** chahiye (rF2 Shared Memory Map plugin install karna hota hai) | Plugin map: vehicle/car name | Plugin par munhasir | NAHI (v1.1) |
| Automobilista 2 | Shared memory (Project CARS 2 jaisi) | Car name static block mein | Same-machine only | NAHI (v1.1) |
| Forza (Motorsport / Horizon) | UDP telemetry (in-game "Data Out" ON + port) | Car identity UDP mein (title-specific) | Game rate | NAHI (v1.1) |

**Scope faisla:** Phase H v1 sirf **AC + ACC + iRacing** — teeno shared
memory, teeno mein **koi user setup / plugin / game setting nahi** chahiye
(game chalao, reader khud dhoond leta hai). UDP wali games (F1, Dirt,
Forza) mein user ko game settings mein telemetry ON + port set karna hota
hai — v1.1 mein, alag reader (`UdpTelemetryReader`) se. rFactor 2 plugin
mangta hai — v1.1 mein, plugin-install guide ke saath.
**Same-machine only:** shared memory sirf usi PC par parhi ja sakti hai
jis par game chal raha hai — RSH app waise bhi usi PC par chalti hai, is
liye ye had nahi hai. Remote PC ka koi plan nahi.

## 2. Reader architecture

```
Game process (AC / ACC / iRacing)
        │  Windows shared memory (read-only mmap)
        ▼
┌─────────────────────────────────────────────────┐
│ RSH.Core/Telemetry/                              │
│  ITelemetryReader (interface, §4)                │
│   ├─ AcSharedMemoryReader   (acpmf_physics/      │
│   │   graphics/static)                           │
│   ├─ AccSharedMemoryReader  (acc physics/        │
│   │   graphics/static)                           │
│   └─ IRacingSharedMemoryReader (VarBuffer + YAML)│
│  TelemetryWatcher (poll loop, 2 Hz identity)     │
│        │ GameTelemetrySnapshot                    │
│        ▼                                          │
│  CarProfileResolver (carModel → WheelProfile)    │
│        │                                          │
│  ProfileAutoSwitcher (maujooda GameProcessName   │
│  mechanism ko extend karta hai — §5)             │
└─────────────────────────────────────────────────┘
        │  (koi tabdeeli nahi)
        ▼
WheelEngine / ProcessingPipeline (per-module tuning, Plan v2.3 §4)
```

**Design usool:**

1. **Read-only, no injection.** Reader sirf game ki published shared
   memory parhta hai (SimHub/StintLab/RacePulse jaisa tareeqa). Game mein
   kuch inject nahi hota, koi DLL nahi, koi memory write nahi — anti-cheat
   posture wahi rehta hai jo GAME-COMPATIBILITY.md mein darj hai
   (app inject nahi karti). Har game par phir bhi Phase H mein anti-cheat
   spot-check hoga.
2. **Identity poll, physics poll nahi.** Profile switch ko car ka naam
   chahiye, 333 Hz physics nahi. Reader **2 Hz** par identity parhta hai
   (carModel + track + session state). Physics fields (speed/rpm/gear)
   v1 mein **nahi** parhe jayenge — wo LED/dash features ka kaam hai, baad
   mein. Is se CPU cost negligible rehti hai aur reader chhota rehta hai.
3. **Game detect pehle, map baad mein.** Reader pehle game process
   (`acs.exe` / `acc.exe` / `iRacingSim64DX11.exe` — GameLauncher catalogue
   wale naam) dhoondta hai, phir us ka shared-memory map kholta hai.
   Map na mile (game menu mein hai, session load nahi hua) to reader
   `NotInSession` deta hai — switch nahi hota, error nahi.
4. **Struct layout version-sensitive hai.** AC/ACC ke shared-memory structs
   community-documented hain aur saalon se stable hain; phir bhi har reader
   map ka **size + magic/version field** check karega — mismatch ho to
   reader `UnsupportedVersion` dega, kabhi ghalat offset se nahi parhega.
   iRacing ke var headers **self-describing** hain (offset/type/count
   header mein) — wahan layout risk sab se kam hai.
5. **Sandbox mein testable seam.** `MemoryMappedFile` ko seedha istemal
   mat karo — `IMemoryMapAccessor` interface ke peeche rakho, taake host
   tests mein fake map (captured bytes) se snapshot parsing test ho sake.
   Real game ke baghair reader ka **parse + resolve + switch** logic
   poora test hoga; sirf real-game end-to-end Phase H mein Windows PC par
   hoga.
6. **SEC-H-05 (Phase H security review — Medium, tuning-only asar):**
   (a) **TOCTOU / atomic snapshot:** game static block / VarBuffer ko
   likh raha ho sakta hai — reader har block **do dafa parhta hai aur
   milata hai** (double-read); na mile to ek dafa dobara, phir bhi na
   mile to snapshot reject (aadha snapshot kabhi nahi lo). iRacing mein
   buffer copy se pehle aur baad mein `tickCount` dobara parha jata hai.
   (b) **Map spoofing:** koi bhi local process `acpmf_static` jaisi map
   pehle bana sakta hai — is liye map kholne se **pehle** game-process
   ownership check hota hai (process naam + `MainModule.FileName` ka
   `<name>.exe` par khatam hona). Ye poora hal nahi hai (map ke asal
   owner ka user-mode se seedha saboot nahi milta) — defense-in-depth
   hai; asar tuning-only switch tak mehdood hai.
   (c) **YAML size bound:** iRacing session YAML zyada se zyada 256 KB
   (`MaxSessionInfoBytes`) — is se bari YAML reject hoti hai; `numVars`
   ≤ 10000, `numBuf` 1..4, tamam offsets map ke andar hone chahiye.
   **Layout corrections (Phase H mein verify hue):** AC/ACC strings
   `wchar_t` (UTF-16LE) hain — `carModel` offset **68** hai (ASCII
   samajh kar 38 par parhna ghalat hai); ACC ke map naam AC wale hi
   hain (`acpmf_*`) — AC aur ACC reader process gate (`acs.exe` vs
   `acc.exe`) se alag hote hain, map naam se nahi.

## 3. Data — kya parha jayega, kis rate par

| Field | Source | Istemal | Rate |
|---|---|---|---|
| `GameId` (AC / ACC / iRacing) | Process detect | Kaunsa reader + kaunsi profile family | Poll 2 Hz |
| `CarModel` (string, normalized lowercase) | AC static `carModel`; ACC static `carModel`; iRacing session YAML car path/name | **Profile switch ki key** | Poll 2 Hz |
| `TrackName` | Graphics block / YAML `WeekendInfo.TrackName` | Profile key ka optional doosra hissa (car+track profile) | Poll 2 Hz |
| `SessionState` (Menu / Loading / OnTrack / Replay / Paused) | Graphics `AC_STATUS` / iRacing `SessionState` | Switch sirf `OnTrack` par (menu mein car badalna switch nahi karega) | Poll 2 Hz |
| `SessionId` (lap/session counter ya timestamp) | Graphics/session | Debounce: car tabdeel hone ki tasdeeq | Poll 2 Hz |
| Physics (speed, rpm, gear, throttle…) | — | **v1 mein NAHI** (future: LED/dash) | — |

**Switch trigger (exact rules):**

1. Snapshot mein `SessionState == OnTrack` ho aur `CarModel` khaali na ho.
2. Wahi `CarModel` **2 s tak stable** rahe (4 musalsal polls @ 2 Hz) —
   loading ke dauraan aadhi identity par switch nahi hoga.
3. Resolver `CarModel` → profile dhoondta hai: pehle **car-specific**
   profile (`GameId + CarModel`), phir **game profile**
   (`GameProcessName` — aaj ka mechanism), phir `Default`. Koi match na
   ho to switch nahi hota (current profile rehta hai).
4. Switch sirf **profile ka tuning hissa** badalta hai (per-module tuning,
   Plan v2.3 §4). **Calibration nahi badalti** — calibration board-bound
   hai (`BoardMac`, D21) aur car se us ka koi talluq nahi.
5. **Manual override jeetta hai:** user ne haath se profile chuni ho to
   auto-switch band rehta hai jab tak user dobara "Auto" na kare (UI mein
   lock indicator). Switch hone par Dashboard par toast: "Profile:
   Ferrari 488 GT3 (auto)".
6. Game band ho ya menu mein jaye to profile **wapas nahi** badalti —
   aakhri profile rehti hai (A2 restore-on-exit sirf launcher ki profile
   ko restore karta hai, is se alag hai).

## 4. Phase H implementation — exact file/function plan

**Naye files (sab `software/src/RSH.Core/Telemetry/`):**

| File | Zimmedari | Key types / functions |
|---|---|---|
| `TelemetryTypes.cs` | Shared types | `enum GameId { Unknown, AssettoCorsa, Acc, IRacing }`, `enum SessionState { Unknown, Menu, Loading, OnTrack, Replay, Paused }`, `record GameTelemetrySnapshot(GameId Game, string CarModel, string TrackName, SessionState State, DateTimeOffset SeenAt)` (CarModel normalized: trim + lowercase) |
| `ITelemetryReader.cs` | Reader contract | `interface ITelemetryReader { GameId Game { get; } bool TryRead(out GameTelemetrySnapshot snapshot); }` — `TryRead` kabhi throw nahi karta (game na ho / map na mile to `false`) |
| `IMemoryMapAccessor.cs` | Sandbox seam | `interface IMemoryMapAccessor : IDisposable { bool TryOpen(string mapName, int size); int ReadBytes(long offset, byte[] buffer, int count); }` + `WindowsMemoryMapAccessor` (asal `MemoryMappedFile`) + tests mein `FakeMemoryMapAccessor` (captured bytes) |
| `AcSharedMemoryReader.cs` | AC reader | `acpmf_physics` / `acpmf_graphics` / `acpmf_static` maps; static struct se `carModel` (fixed char array, null-terminated); graphics se `AC_STATUS`; size check + `UnsupportedVersion` path |
| `AccSharedMemoryReader.cs` | ACC reader | ACC maps (AC jaisi layout, alag map naam); physics/graphics/static; wahi size/version check |
| `IRacingSharedMemoryReader.cs` | iRacing reader | VarBuffer header parse (self-describing offsets); session YAML se car name + track (`WeekendInfo`); `SessionState` var se state; triple-buffer latest snapshot lo |
| `TelemetryWatcher.cs` | Poll loop | `Start()/Stop()`, 2 Hz timer, `event SnapshotChanged(GameTelemetrySnapshot)`, sirf tab event jab `(Game, CarModel, State)` badle; debounce (2 s stable) yahin |
| `CarProfileResolver.cs` | Resolve | `WheelProfile? Resolve(GameTelemetrySnapshot snap, IReadOnlyList<WheelProfile> profiles)` — order: car-specific → game → null (caller Default rakhta hai); matching normalized strings par |

**Badli hui files:**

| File | Tabdeeli |
|---|---|
| `software/src/RSH.Core/Profiles/WheelProfile.cs` | Naye optional fields: `string CarModel { get; init; } = ""` (normalized lowercase; khaali = kisi bhi car par), `GameId Game { get; init; } = GameId.Unknown` (khaali = kisi bhi game par — `GameProcessName` ke saath dono chalenge). **Schema bump nahi** — optional fields hain, purani files baghair in ke load hoti hain (ProfileStore `Sanitized()` inhein normalize karega; naam allowlist jaisi validation: car model mein sirf `A-Z a-z 0-9 space _ - .` — path separators kabhi nahi). |
| `software/src/RSH.Core/Profiles/ProfileStore.cs` | `Sanitized()` mein `CarModel` normalize + `Game` validate (unknown enum → Unknown). |
| `software/src/RSH.App/ViewModels/ProfilesSystem.cs` | `AutoSwitchRuleRow` mein `CarModel` column; UI mein per-car rule add/edit; `TelemetryWatcher` + `CarProfileResolver` ko `AppServices` se wire karo; manual-override lock yahin. |
| `software/src/RSH.App/Services/AppServices.cs` | `TelemetryWatcher` lifecycle (app start par start, exit par stop; game na ho to idle — koi error nahi). |
| `software/src/RSH.App/Views/Pages/ProfilesPage.xaml(.cs)` | Per-car profile editor: game chuno → car model likho (ya "current car" button — reader se live car bharo) → tuning. |

**Tests (RSH.Tests / RSH.Core.Tests — sandbox mein green hone chahiye):**

| Test | Kya cover karta hai |
|---|---|
| `AcReaderParsesCapturedStatic` | Fake map mein captured AC static bytes → `CarModel` sahi nikle |
| `AccReaderRejectsWrongSize` | Ghalat size ka map → `TryRead == false`, kabhi crash nahi |
| `IRacingReaderParsesYamlCar` | Fake YAML session info → car name + track sahi |
| `WatcherDebouncesCarChange` | Car 1 s ke liye badle phir wapas → koi switch event nahi; 2 s stable → ek event |
| `ResolverPrefersCarOverGame` | Car profile + game profile dono hon → car wali mile |
| `ResolverFallsBackToGameThenNull` | Car match na ho → game profile; wo bhi na ho → null |
| `ManualOverrideBlocksAutoSwitch` | Lock on ho to `SnapshotChanged` par bhi switch nahi |
| `SnapshotNeverThrows` | Map ghayab / corrupt bytes / aadhi YAML → `false`, koi exception bahar nahi |

**Phase H acceptance (gate):** AC + ACC + iRacing — har game par real
session mein: (a) reader car detect kare, (b) 2 s stable ke baad sahi
car profile lage, (c) menu/loading mein switch na ho, (d) manual override
kaam kare, (e) game band hone par app stable rahe. UDP games (F1/Dirt/
Forza) aur rF2 plugin v1.1 mein — is doc ka §1 table hi un ka backlog hai.

## 5. Risks / khule sawal (Phase H mein band honge)

- **Struct drift:** AC/ACC ke structs Kunos ke update se badal sakte hain —
  mitigation §2 rule 4 (size/version check + `UnsupportedVersion`).
  iRacing self-describing hai — sab se mehfooz.
- **CarModel strings alag-alag hain:** AC mein `ferrari_488_gt3`, ACC mein
  `Ferrari 488 GT3 Evo` jaisi shakal ho sakti hai — resolver **normalized**
  match karega (lowercase, `_`/space/`-` ko ek jaisa mano), aur UI mein
  "current car se bharo" button hoga taake user ko string yaad na karni pare.
- **Ek car, kai profiles:** user ek car ke liye do profiles bana le to
  resolver pehli (naam ke hisaab se) lega aur UI mein warning dikhayega —
  Phase H mein duplicate check.
- **Anti-cheat:** read-only shared memory hai; SimHub jaisi tools yahi
  karti hain. Phir bhi har game par Phase H spot-check (GAME-COMPATIBILITY
  procedure step 9 jaisa).
