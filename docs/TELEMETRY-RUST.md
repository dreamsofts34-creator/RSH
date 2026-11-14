# RSH — Telemetry Readers, Rust (Phase F1)

**Status: IMPLEMENTED 2026-11-14 (Phase F1)** — AC + ACC + iRacing
shared-memory readers, fixture-tested on Linux, Windows code
type-checked + clippy-clean for `x86_64-pc-windows-msvc` in the sandbox.
**Live-game end-to-end (real session on the owner PC) is still open** —
see §7.

**Sources:** `TELEMETRY-READER.md` (C# behaviour — the spec; offsets and
rules below are from it unless marked otherwise), Kunos AC/ACC
shared-memory documentation (community), iRacing SDK as implemented by
`pyirsdk` 1.1.7 (header/var-header/var-buffer layouts, session YAML
shape). No C# code was copied: the Rust readers are a fresh
implementation of the documented behaviour.

## 0. Spike verdict (verified 2026-11-14, in `/tmp/f1-spike`)

| Item | Verdict |
|---|---|
| `windows` crate | **0.62.2** (latest on crates.io, updated 2025-10-06), `default-features = false`, features `Win32_Foundation`, `Win32_System_Memory`, `Win32_System_Diagnostics_ToolHelp`. Declared as a **Windows-only** dependency (`[target.'cfg(windows)'.dependencies]`). API surface verified in the generated source of `windows-0.62.2`: `OpenFileMappingW` (`Memory/mod.rs:392`, returns `Result<HANDLE>`), `MapViewOfFile` (`:329`, returns `MEMORY_MAPPED_VIEW_ADDRESS`), `UnmapViewOfFile` (`:481`), `CloseHandle`, `FILE_MAP_READ` (`:633`), `CreateToolhelp32Snapshot` + `Process32FirstW`/`Process32NextW` + `PROCESSENTRY32W.szExeFile: [u16; 260]` (`ToolHelp/mod.rs`). |
| `windows` on Linux | **Does NOT compile** — reproduced in the spike (`/tmp/f1-spike/win-ungated`, `cargo check` fails with 12 errors in `windows-future 0.3.2`: `IMarshal` / `marshaler` / `submit` not found). `default-features = false` does **not** fix it. Consequence (matches `docs/DECISIONS.md` §S-10): the dependency must stay `cfg(windows)`-gated and Linux builds must never compile it. A gated minimal crate (`/tmp/f1-spike`, same dep under `[target.'cfg(windows)'.dependencies]`) builds cleanly on Linux — the dep is skipped there. |
| `windows-sys` (fallback) | Latest **0.61.2** (2025-10-06; `DECISIONS.md` recorded 0.61.0 — superseded). Raw bindings only; use only if the `windows` crate causes trouble on Windows CI. Not used. |
| `memmap2` | Latest **0.9.12** (2026-10-21). **Rejected**: it maps only from a `File` handle (`CreateFileMappingW` with a null name) — there is no `OpenFileMappingW` named-mapping API, so it cannot open a game's map. Verified in `memmap2-0.9.8` source (`src/windows.rs`, `src/lib.rs`). |
| `shared_memory` 0.12.4 | Rejected (already in `DECISIONS.md` §S-10): unmaintained since 2022-03-01, no process enumeration. |
| Windows target check | `cargo check -p rsh-telemetry --target x86_64-pc-windows-msvc` **passes** (toolchain 1.100.0 + `rustup target add x86_64-pc-windows-msvc`), and `cargo clippy --target x86_64-pc-windows-msvc -- -D warnings` **passes**. The `cfg(windows)` code is therefore type-checked and lint-clean in the sandbox; only a live run on Windows remains. |

## 1. Public API (`crates/rsh-telemetry`)

```rust
pub trait TelemetryReader {
    fn game(&self) -> GameId;                 // AssettoCorsa | Acc | IRacing
    fn process_name(&self) -> &'static str;   // "acs.exe" | "acc.exe" | "iRacingSim64DX11.exe"
    fn read(&self) -> Option<TelemetrySnapshot>;
    fn read_car(&self) -> Option<String>;     // normalised car key (default impl)
}

pub struct TelemetrySnapshot {
    pub game: GameId,
    pub car_model: String,    // normalised key, e.g. "ferrari_488_gt3_evo"
    pub car_display: String,  // as the game reports it, e.g. "Ferrari 488 GT3 Evo"
    pub track_name: String,
    pub state: SessionState,  // Unknown | Menu | Loading | OnTrack | Replay | Paused
    pub seen_at: Instant,     // monotonic read time, for the watcher debounce (F2)
}

pub fn all_readers() -> [Box<dyn TelemetryReader + Send>; 3]; // AC, ACC, iRacing
pub fn normalize_car_model(raw: &str) -> String;
pub trait MapAccessor { fn len(&self) -> usize; fn read_into(&self, offset: usize, buf: &mut [u8]) -> bool; }
pub struct SliceMap<'a>;      // MapAccessor over a byte slice (fixtures)
pub fn read_consistent(map: &dyn MapAccessor, offset: usize, len: usize) -> Option<Vec<u8>>;
```

Pure entry points (platform-independent, what tests use):
`AcReader::read_from_maps(static, graphics)`,
`AccReader::read_from_maps(static, graphics)`,
`IRacingReader::read_from_map(map)`. `GameId` discriminants match
`rsh-profiles::csharp_import::GameId` (0 Unknown, 1 AC, 2 ACC,
3 iRacing).

## 2. Layouts / offsets used

### 2.1 Assetto Corsa + ACC (Kunos `acpmf_*` maps)

| Block | Field | Offset | Type |
|---|---|---|---|
| `acpmf_static` | `smVersion` | 0 | `wchar_t[15]` UTF-16LE |
| `acpmf_static` | `acVersion` | 30 | `wchar_t[15]` UTF-16LE |
| `acpmf_static` | `numberOfSessions` | 60 | `i32` |
| `acpmf_static` | `numCars` | 64 | `i32` |
| `acpmf_static` | **`carModel`** | **68** | `wchar_t[33]` UTF-16LE |
| `acpmf_static` | **`track`** | **134** | `wchar_t[33]` UTF-16LE |
| `acpmf_graphics` | `packetId` | 0 | `i32` |
| `acpmf_graphics` | **`status` (`AC_STATUS`)** | **4** | `i32`: 0 OFF, 1 REPLAY, 2 LIVE, 3 PAUSE |
| `acpmf_graphics` | `session` | 8 | `i32` |

ACC uses the **same map names and the same identity offsets** as AC;
the readers differ only by the process gate (`acs.exe` vs `acc.exe`).
ACC's later static fields differ (air/road temp instead of the
deprecated slots) — nothing after `track` is read, so one parser serves
both games.

**Size check (deliberate deviation from exact-size):** Kunos appended
fields to the static struct across AC versions (`trackConfiguration`,
`ersMaxJ`, `carSkin`, …), so an exact-size check would false-reject
newer games. The reader requires only the **minimum** size covering
what it reads (static ≥ 200 B, graphics ≥ 8 B) and parses
NUL-terminated UTF-16 strictly (unpaired surrogate → reject). There is
no magic/version field in the Kunos maps; an all-zero map yields an
empty car name and is rejected.

### 2.2 iRacing (`Local\IRSDKMemMapFileName`, 780 KiB)

Header (112 B): `ver` @ 0, `status` @ 4 (bit 0 = connected),
`tickRate` @ 8, `sessionInfoUpdate` @ 12, `sessionInfoLen` @ 16,
`sessionInfoOffset` @ 20, `numVars` @ 24, `varHeaderOffset` @ 28,
`numBuf` @ 32, `bufLen` @ 36, `pad[2]` @ 40, `varBuf[4]` @ 48 (each
16 B: `tickCount` @ +0, `bufOffset` @ +4). Var header (144 B): `type`
@ 0, `offset` @ 4, `count` @ 8, `countAsTime` @ 12, `name[32]` @ 16,
`desc[64]` @ 48, `unit[32]` @ 112. Var types: 0 char, 1 bool, 2 int,
3 bitfield, 4 float, 5 double. `#[repr(C)]` mirrors
(`HeaderRaw` 112 B, `VarBufRaw` 16 B, `VarHeaderRaw` 144 B) exist
solely for layout asserts (`size_of` / `offset_of!` in tests); parsing
reads fields byte-by-byte, never by casting a slice.

- **State:** the `SessionState` var (type int): 0 invalid → Unknown,
  1 get-in-car / 2 warmup → Loading, 3 parade / 4 racing / 5 checkered /
  6 cool-down → OnTrack. Absent var → state Unknown (snapshot still
  returned if a car resolves).
- **Car:** session YAML — `DriverInfo → DriverCarIdx` selects the
  `Drivers` entry with matching `CarIdx` (first entry when the key is
  absent); name is `CarScreenName`, falling back to `CarPath`.
  Track is `WeekendInfo → TrackName`. The YAML is parsed by a small
  hand-rolled line parser (flat `Key: Value` lines) — no YAML
  dependency in a fail-safe reader.
- **Bounds (SEC-H-05c):** YAML ≤ 256 KiB, `numVars` ≤ 10000,
  `numBuf` 1..=4, `bufLen` 1..=1 MiB, every offset/length pair checked
  against the real map size before any read. `ver` must be ≥ 1 and the
  connected bit set.

## 3. Fail-safe design

- **Total readers:** `read()` returns `Option<TelemetrySnapshot>`;
  every failure mode (game absent, map absent, short/torn/corrupt
  bytes, bad header, oversize YAML, empty car) is `None`. No panics on
  untrusted bytes (bounds-checked reads only; the only `unsafe` is the
  Windows view copy, with a `SAFETY` comment). A reader failure can
  therefore never affect the engine — worst case is "no snapshot this
  poll". Readers are stateless and `Send`; the watcher (F2) owns the
  polling task.
- **Double-read (SEC-H-05a):** every multi-byte read goes through
  `read_consistent` — same range read twice, accepted only when two
  consecutive reads agree; one retry, then reject. iRacing additionally
  re-reads the chosen buffer's `tickCount` after copying the
  `SessionState` value (retry once, then `None`). Latest buffer is
  picked by max `tickCount` using wrapping subtraction (correct across
  `i32` wrap).
- **Process gate (SEC-H-05b):** before opening any map, the reader
  checks the game process is running via a ToolHelp snapshot
  (`szExeFile`, case-insensitive). This is defense-in-depth only —
  no complete user-mode proof of map ownership exists (as the C# doc
  notes); the damage a spoofed map can do is limited to a
  tuning-profile switch. Residual gap: the check compares the exe
  **file name**, not the full `MainModule` path (that needs PSAPI
  `GetModuleFileNameEx` — follow-up, see §7).
- **Read-only:** `OpenFileMappingW(FILE_MAP_READ)` + `MapViewOfFile`;
  the game memory is never written. Views are partial on purpose
  (AC/ACC: first 200 / 8 bytes; iRacing: the full 780 KiB map, which
  the SDK creates at exactly that size).
- **Normalisation:** `normalize_car_model` lowercases and unifies runs
  of space/`_`/`-` to one `_`, trimmed — so AC's `ferrari_488_gt3`
  and ACC's `Ferrari 488 GT3` share the key `ferrari_488_gt3`.

## 4. Fixtures

`crates/rsh-telemetry/tests/fixtures/`, generated by
`generate_fixtures.py` (stdlib only; rerun it in that directory to
regenerate). Synthetic images built from the published layouts — **not**
captures from a live game (no game ran in the sandbox):

| File | Contents |
|---|---|
| `ac_static.bin` (1024 B) | Static block: `ferrari_488_gt3` @ 68, `monza` @ 134 |
| `ac_graphics.bin` (512 B) | Graphics block: status LIVE (2) |
| `ac_graphics_menu.bin` (512 B) | Graphics block: status OFF (0) |
| `ac_static_short.bin` (100 B) | Truncated static block — must be rejected |
| `acc_static.bin` (1024 B) | Static block: `Ferrari 488 GT3 Evo` @ 68, `spa` @ 134 |
| `acc_graphics.bin` (512 B) | Graphics block: status LIVE (2) |
| `iracing_shm.bin` (621 B) | Minimal valid iRacing image: header (ver 2, connected, 1 var, 1 buffer) + `SessionState` var header + 64 B buffer (`SessionState` = 4, tickCount 1234) + YAML (player `Ferrari 488 GT3 Evo 2020`, track Spa-Francorchamps, second driver as distractor). **Not** the real 780 KiB map — all bounds are relative to the image size. |

Acceptance (per game, green on Linux via fixtures): see
`tests/readers.rs` — AC car resolve, ACC car resolve (normalised key
matches AC's), iRacing car resolve (player entry, not the distractor),
menu state never `OnTrack`, short/corrupt fixtures rejected.

## 5. Tests (2026-11-14, Linux sandbox)

`cargo test -p rsh-telemetry`: **39 passed, 0 failed** (32 unit tests
in `src/` + 7 integration tests in `tests/readers.rs`), including
layout asserts, UTF-16 edge cases, header/YAML validation rejections,
torn-read rejection (flipping accessor), and the Linux live-read
contract (`read()` → `None` without Windows).
`cargo clippy -p rsh-telemetry --all-targets -- -D warnings`: **pass**
(Linux and `--target x86_64-pc-windows-msvc`). `cargo fmt --check`:
**pass**.

## 6. What F1 does NOT do (other phases' scope)

- No watcher / polling loop / debounce — Phase F2 (SW-R7.2).
- No resolver / auto-switch / manual override — Phase F2.
- No physics fields (speed/rpm/gear) — future LED/dash work.
- No UDP games (F1/Dirt/Forza) and no rF2 plugin — v1.1 backlog
  (`TELEMETRY-READER.md` §1).

## 7. Windows-only gaps (all need the owner PC / Windows CI)

1. **Live-game end-to-end (OPEN):** real session per game — car
   detect, 2 s-stable switch is F2's, menu/loading no-switch, game
   exit stability. The sandbox has no game and no Windows APIs at
   runtime; the Windows code is type-checked + clippy-clean for the
   target but has never executed.
2. **`windows` 0.62.2 compiling on Windows CI (OPEN):** the crate
   compiles for the Windows target in the sandbox, but a real Windows
   CI build + run has not happened.
3. **Full-path process check (follow-up):** the ToolHelp gate compares
   the exe file name only; verifying `MainModule`'s full path needs
   PSAPI `GetModuleFileNameEx` (extra `windows` feature
   `Win32_System_ProcessStatus`).
4. **Live captures:** the fixtures are synthetic; replace/extend them
   with real shared-memory captures from the owner PC during live
   verification (note any layout drift, e.g. a newer AC static size).
5. **ACC map-name assumption:** the C# doc's Phase H correction says
   ACC maps are the same `acpmf_*` names as AC (readers differ by
   process gate). This is the documented behaviour; live ACC runs
   should confirm the maps open while `acc.exe` runs and `acs.exe`
   does not (and vice versa).
6. **Anti-cheat spot-check per game:** read-only shared memory is the
   SimHub-style posture, but each game still needs the
   `GAME-COMPATIBILITY.md` step-9 check on the owner PC.
