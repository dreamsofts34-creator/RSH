# ⚠️ SUPERSEDED — ye document ab authority NAHI hai (Architecture polish sweep, 2026-11-13)

**Ye 2026-11-10 ke project notes hain (v1.0.0-era; board ghalat "ESP32-C3 Super Mini" likha hai —
asal board ESP32 DevKit V1 original ESP32 hai; "SimWheel" rename pending wala note bhi purana hai,
rename Phase A/C/E mein mukammal ho chuka hai). Tareekhi record ke tor par rakha gaya hai.
Current authority: `RSH-PLAN-v2.3.md` + `docs/RELEASE-CHECKLIST.md` (v2.0.3 addendum).**

# RSH — project notes

- **Naam (final, owner 2026-11-10):** RSH — by Dreamsofts. Is chat ("Naya Project") ka naya project; fresh start ke baad pehla project.
- **Kya hai:** Wireless DIY sim racing steering wheel system — ESP32-C3 Super Mini firmware (WiFi SoftAP, UDP unicast 200 pps, 18-byte packet) + Windows companion app (C# .NET 10 LTS, WPF + WPF-UI 4.x) jo calibration/processing karke vJoy device ko feed karti hai. Principle: firmware dumb, PC smart.
- **Source docs:** `~/workspace/sim-wheel-docs/sim-wheel/` (README + 01–06; owner ne zip bheja 2026-11-10). Docs mein purana working naam "SimWheel"/SSID "SIMWHEEL" hai — rename pass baad mein hoga (app name, SSID, config path `%APPDATA%` folder).
- **BOARD CHANGE (owner, 2026-11-10):** Final board = **ESP32 DevKit V1** (original ESP32), C3 Super Mini dropped. Wajah: owner ke paas series resistor nahi; research-verified ke original ESP32 ka ADC 11 dB par 3.3 V tak parhta hai (full-scale 3.9 V, VDD-limited) is liye resistor ki zaroorat hi nahi; dual-core se sampling timing bhi asan. C3 version `firmware/rsh-c3/` mein mehfooz hai (compile-verified tha). DevKit pin map: steering GPIO34, hall ADC GPIO35/36, buttons GPIO25/26/27/33 (pullup wali), LED GPIO2 (active-high aam tor par). Research corrections jo lagu huin: C3 11 dB asal range 0–2500 mV (datasheet) thi; TX power kam karna sirf kharab-revision C3 boards ka workaround hai, fix nahi.
- **Firmware status (2026-11-10):** Complete firmware likha gaya (DevKit V1) — host virtual tests 523 checks pass, independent Python cross-verify pass, C3 target par asli compile PASS (Flash 76.1% / RAM 11.7%); DevKit target compile alag se verify ho raha hai. Board-par-only checks baqi: analogRead timing/cycle time (firmware mein measurement mojood hai), ADC noise (WiFi on), LED polarity.
- **Key verified decisions (docs 01/02):** .NET 10 LTS; vJoy 2.1.9.1 signed baseline (ViGEmBus retired — primary nahi); unicast + HELO handshake (4210/4211), broadcast nahi.
- **PERFORMANCE BUDGET (owner, 2026-11-10):** Software halka hoga — low-end PCs (owner ka AMD A8-7600 bhi) par stable chalega. Diye gaye benchmarks har release par run honge aur reports mein actual vs budget printed hoga; budget miss = "optimized" dawa nahi hoga. Budgets (SOFTWARE-PLAN.md mein tafseel): idle (wheel band, koi game nahi) CPU ≤1% avg 1 core, working set ≤120 MB, GC heap ≤60 MB, background me sirf 1 UDP keepalive/sec (traffic ~0); race (200 Hz pipeline + output) CPU ≤3%, working set ≤150 MB, Gen2 GC 0/sec; 8 h soak zero leak/bounded. Loaders: zero-alloc hot path, UI rendering isolation, on-demand rendering, published Release/trimmed build, synthetic slow-disk/CI budget tests.
- **Order of work:** Step 0 board/WiFi test (firmware) + PC side vJoy install-test aur solution scaffold (Simulator ki wajah se hardware ke baghair aage badh sakta hai) -> firmware milestones -> software milestones -> testing (doc 06).
- **Step 0 sketch tayyar:** `firmware/step0-ap-test/step0-ap-test.ino` (SSID RSH rakha hai).

- **Software status (2026-11-10, FINAL):** PC software v1.0.0 mukammal — Core (M1/M2) + vJoy backend (M3, BrunnerInnovation 2.2.2.0, struct layout asal header se verified) + WPF app (M4: 10 pages, wizard, tray, autostart, Mica/Fluent modern UI). Full solution Release build **0 warnings / 0 errors** (Linux par compile-verify). Tests: **27 pass, 0 fail, 1 skip** (UDP loopback VM policy ki wajah se; Windows par chalega). Benchmarks: parse 382 ns/0 alloc, pipeline 446 ns/tick/0 alloc, soak 200 ticks/s exact, 0 late, Gen2 0. Publish: self-contained single-file `RSH.exe` (195 MB) Linux se hi ban gaya (PE32+ verified). Delivery: `RSH-Software-v1.0.0.zip` (exe + USER-GUIDE + FINAL-REPORT + installer assets). Sirf Windows-par-verify baqi: asal window/tray look, real vJoy driver, real wheel end-to-end, kisi game mein binding, Inno installer build — sab FINAL-REPORT.md mein saaf alag likha hai, "all tested" mein mix nahi.

## Status 2026-11-10 (audit build)
- Deep audit done: 4 fixes (output-off neutral safety, UDP backoff, firmware sample clamp, calibration profile reload).
- Virtual user-scenario suite: 10 scenarios pass; totals 37 software tests passed + 1 skipped, firmware 536 checks.
- Real installer built: RSH-Setup-1.0.0.exe (NSIS, 54.7 MB). Firmware bumped to v3.1 (clamp fix).
- Delivered: setup exe, full zip (132 MB), LITE zip (10.5 MB), firmware v3.1 zip, DELIVERY-NOTE v2.
- Highly-compressed setup rebuilt 2026-11-10: 48.8 MB (was 54.7) — payload without R2R (167 MB) + NSIS LZMA dict 64MB. Note: single-file EnableCompressionInSingleFile exe alone was 74.7 MB, worse than installer LZMA, so installer route kept.
- Firmware v3.2 (2026-11-11): Web OTA added (browser page at 192.168.4.1, basic auth admin + OTA password, dual-partition safe, progress-bar UI in app accent). Audit round 2 fixes: AP max stations 2->4 (PC + phone for OTA), avg_pedal sample-count clamp. Compile SUCCESS, app 1,125,872 / 1,310,720 bytes. Merged factory bin delivered: RSH-Firmware-DevKitV1-v3.2.bin (1,191,408 bytes, flash at 0x0). Default passwords kept per owner.
