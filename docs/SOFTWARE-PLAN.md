# ⚠️ SUPERSEDED — ye document ab authority NAHI hai (Architecture polish sweep, 2026-11-13)

**Ye v1-era plan hai (2026-11-10, protocol v1 18-byte) — tareekhi record ke tor par rakha
gaya hai. Current authority: `~/workspace/projects/rsh/RSH-SOFTWARE-PLAN.md` (FINAL, SW-1–SW-16)
+ `RSH-PLAN-v2.3.md` §5. Is file ke protocol/architecture ke daawe current code par laagu nahi hote.**

# RSH — PC Software Plan (2026-11-10)

Usool: research first (stack facts official sources se verify ho rahe hain),
phir milestone-by-milestone build, har milestone ke apne tests. Firmware dumb,
PC smart. Protocol v1 (18-byte) firmware ke sath fixed contract — software use
bilkul waisa hi implement karega.

## Stack (research se confirm hone ke baad final)
- .NET 10 LTS, C#, WPF + WPF-UI (Fluent/Mica), CommunityToolkit.Mvvm
- ScottPlot 5 (live graphs), Serilog (file logs), Microsoft.Extensions.Hosting
- vJoy driver: **BrunnerInnovation v2.2.2.0** (research correction 2026-11-10:
  purana 2.1.9.1 sirf Windows 10 ke liye attestation-signed hai; 2.2.2.0
  EV-signed + attested, Win10/Win11 dono) + apni thin P/Invoke wrapper;
  driver aur vJoyInterface.dll HAMESHA ek hi release se. ViGEmBus retired
  hai — primary nahi. HVCI/Memory Integrity par load ki koi guarantee
  published nahi — asal PC par test hoga.
- xUnit tests; Simulator = ESP ki jagah UDP par wahi packets bhejne wala tool

## Solution layout (docs 04, naam RSH)
```
software/
  RSH.sln
  src/
    RSH.Core/        net10.0 — protocol, CRC, transport, pipeline,
                          processing, profiles, diagnostics (NO UI, NO vJoy)
    RSH.Output.VJoy/ net10.0-windows — vJoy P/Invoke backend
    RSH.App/         net10.0-windows — WPF GUI
  tools/
    RSH.Simulator/   net10.0 console — fake ESP (auto/manual/replay)
  tests/
    RSH.Tests/       xUnit — Core ke tamam tests
```

## Pipeline (docs 02 section 6)
- UDP listener :4211 receive thread; HELO sender har 1 s → 192.168.4.1:4210
- Latest-sample slot (lock-free): purana sample overwrite — queue barhti nahi
- Processing thread 200 Hz high-resolution loop:
  validate → calibrate/map → deadzone → curve → One Euro smoothing →
  button/pedal ramp → vJoy output object → UpdateVJD
- Watchdog: 250 ms se purana data → neutral output (wheel center, pedals 0,
  buttons off) — band phir bhi output thread chalta rehta hai
- UI updates 30 Hz par throttled; games sirf vJoy dekhte hain, GUI sirf
  monitoring/setup hai — GUI band = pipeline chalta rehta hai (tray)
- Latency budget: WiFi ~1-5 ms + process <1 ms + vJoy <1 ms; kul aam tor
  par <15 ms (docs ka budget; board par asal numbers M1/M4 mein measure
  honge, guess nahi)

## GUI pages (docs 04 ki 10 screens, sab lazmi)
1. Dashboard — connection state, live wheel/pedal bars, packet rate,
   latency, output state, start/stop, active profile
2. Input Monitor — firmware ke raw values, parsed values, live graphs
   (steering/pedals), CRC/drop counters, packet inspector
3. Calibration — steering min/center/max capture wizard, pedal min/max,
   test area, save/reset
4. Processing — deadzone, curve presets (linear/soft/aggressive/custom) +
   strength slider + curve preview graph, smoothing slider (0 = off),
   invert toggles, pedal combine option
5. Buttons — physical → vJoy button mapping table, gear shift indicator,
   button test panel
6. Profiles — create/rename/duplicate/delete/activate, import/export JSON
7. Output — backend status (vJoy detected/version/device state), device
   select, enable/disable, "open Windows game controller panel" shortcut
8. Connection — ESP IP/port display, HELO status, reconnect, protocol
   version of incoming packets
9. Diagnostics — FPS, receive rate, drop %, CRC errors, sequence gaps,
   CPU/RAM, session log export (zip bundle)
10. Settings — autostart, start minimized, tray behavior, theme (system/
    dark/light), log level, config folder kholna, firmware file ki taraf
    ishara (OTA guidance ke liye)
Extras: first-run wizard (vJoy check + calibration), global pause hotkey
(optional), theme Windows ke sath, window state yaad rakhta hai.

## Data & stability rules
- Profiles: %AppData%/RSH/profiles.json — atomic write (temp + replace)
  + .bak backup; kharab file → backup se restore, warna defaults; app
  corrupt profile par band nahi hoga
- Calibration profiles ke andar; har profile: calibration, deadzone, curve,
  smoothing, inverts, button map
- Single instance (mutex); crash par agla start "last session crashed" ko
  pehchanta hai aur safe defaults se khulta hai
- Logs: Serilog rolling files (7 din), Diagnostics page se export

## Testing strategy (Simulator-first, hardware ke baghair)
- Unit: CRC vectors (firmware ke golden packet ke sath cross-check!),
  parser ke invalid cases, mapping math, deadzone/curve kinare,
  One Euro step response, pedal ramp, profile roundtrip + corruption
  recovery, seq-gap/loss hisaab, watchdog timing (fake clock)
- Integration: Simulator → UDP → Core pipeline → FakeOutput; ek poora
  scenario: connect, calibrate, drive, disconnect → neutral check
- Soak: Simulator 200 pps par lambi race (short version har test run mein;
  full lambi race release se pehle)
- Exit rule: har milestone = apne tests pass + build saaf; koi milestone
  "code likh diya" par khatam nahi hota

## Milestones
- **M1 Core + Simulator:** protocol/CRC/parser, UDP source, HELO client,
  seq/loss stats, Simulator (auto/manual/replay). YAHIN (Linux VM) build +
  test hoga.
- **M2 Processing + Profiles:** calibration, deadzone, curve, One Euro,
  ramp, watchdog, profiles store. Tests yahin pass honge.
- **M3 vJoy backend:** P/Invoke wrapper (verified signatures), capability
  check, acquire/update/relinquish, status mapping. Compile yahin;
  asal driver test sirf Windows par.
- **M4 WPF app:** 10 pages + tray + autostart + themes + first-run wizard.
  Research correction: WPF Linux par **compile ho sakta hai**
  (EnableWindowsTargeting=true), sirf **chal nahi sakta** — to M4 bhi
  yahin compile-verify hoga; asal window/driver test Windows par rahega.
  Tray ke liye alag package: WPF-UI.Tray 4.3.0.
- **Release:** publish script (self-contained single-file win-x64), Inno
  Setup script, user guide (install → vJoy → calibrate → game).

## Risks (pehle se likhe hue, taake beech mein surprise na ho)
- vJoy driver signing Windows 11 par — research verify karega; fork
  zaroorat pari to bata kar faisla hoga
- WPF-UI ka .NET 10 support — research verify karega
- 200 Hz loop ki Windows timer resolution — high-resolution wait +
  Diagnostics mein asal measured rate nazar ayega
- vJoy device kisi aur app ke paas BUSY ho — Output page wazeh wajah +
  hal batayega (Relinquish/reset ka button)
- Games ka apna deadzone dohra asar — guide mein har mashhoor game ke
  liye setting note hoga (docs 06 ki roshni mein)

## Quality gates (har delivery par)
1. `dotnet test` poora green
2. Golden packet cross-check: firmware ke wohi bytes software par bhi
   same parse hon
3. Simulator scenario pass (connect→drive→disconnect→neutral)
4. Jo cheez yahan verify nahi ho sakti (asal vJoy driver, asal WPF
   window, asal game), wo report mein alag saaf list hogi — mila kar
   "sab tested" kabhi nahi kaha jayega

## Performance budget (owner directive 2026-11-10 — low-end PCs par stable)
Owner ka PC AMD A8-7600 (2014) hai; software halka hona lazmi hai.
"Dawa nahi, number" — har release par `tools/RSH.Benchmarks`
(Release build) chalega aur actual vs budget report mein ayega.

| Halat | Budget |
|---|---|
| Idle (wheel band, koi game nahi) | CPU ≤1% avg (1 core), working set ≤120 MB, GC heap ≤60 MB, network sirf 1 HELO/sec |
| Race (200 Hz pipeline + vJoy output) | CPU ≤3%, working set ≤150 MB, Gen2 GC 0/sec |
| 8 ghante soak | memory growth bounded (leak = fail), counters overflow-safe |

Design loaders jo ye budget mumkin karte hain:
- Hot path zero-allocation: packet parse spans par, pipeline structs,
  koi per-tick heap allocation nahi (benchmark allocated B/tick gin'ta hai)
- Latest-sample slot — koi queue nahi, memory kabhi barhti nahi
- UI rendering pipeline se alag: sirf visible page render hota hai,
  graphs 30 Hz cap, window minimized/tray mein rendering band
- Engine mein tick timing instrumentation (avg/max/late) — susti chupti nahi
- Log default Warning level; per-packet logging kabhi nahi
- Release publish: ReadyToRun + workstation GC (server GC nahi — memory bachat)
- App start par Windows timer resolution theek karna app layer ka kaam
  (timeBeginPeriod) taake engine ka Sleep sust CPU par bhi sahi chale
  aur busy-spin bilkul na ho
