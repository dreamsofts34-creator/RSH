# Phase H WS2 — LED strip WS2812 driver + wiring (HANDOFF)

**Workstream:** Phase H WS2 — LED strip WS2812 driver + wiring.
**Date:** 2026-11-13. **Status:** Sandbox verify complete (neeche numbers);
sirf wo items baaqi hain jo **sach mein sandbox mein verify nahi ho
sakte** (real strip / board — A7 run mein).

Authority: `RSH-PLAN-v2.3.md` §5 (Phase H), `RSH-FIRMWARE-PLAN.md` FW-12,
`docs/PROTOCOL-V2.1.md` §3.7, `docs/A7-GATE-PROCEDURE.md` §25,
`docs/PHASE-G-WS5-HANDOFF.md` §3 (yahi kaam Phase H ko diya gaya tha).

---

## 1. Faisle (sink choice + W7 notes)

- **Sink: RMT (hardware), bit-bang NAHI.** ESP-IDF ka built-in legacy
  RMT driver (`driver/rmt.h`) — 200 Hz loop starve nahi hota (frame
  hardware bhejta hai; CPU sirf items likhta hai, aakhri chunk
  non-blocking). NeoPixel lib **nahi** li — external dependency +
  interrupt-sensitive timing.
- **Koi lib pin karne wali nahi:** koi external lib nahi hai
  (platformio.ini mein koi `lib_deps` izafa nahi). RMT driver
  installed Arduino core (3.20017 / IDF 4.4.7) ka hissa hai — W7 note
  (version+SHA pin) is liye laagu nahi hota; framework version
  platformio.ini (`espressif32@6.9.0`) se pehle se pinned hai.
- **GPIO21/I2C conflict check pehle se hai** (R-L9 — `led_patterns.h`):
  direct rig mein strip sirf GPIO21; expanders hon to 21/22 I2C hain
  aur strip freed pin (preset GPIO4) par jati hai. RMT sink config
  pin badalne par us pin par dobara lagta hai (net.cpp adapter).
- **Status LED (GPIO2) pehle se wired hai** (`status_led.cpp`) — is WS
  mein chhooa nahi; A7 §25-A usi ko cover karta hai.

## 2. Kya banaya (files)

### Firmware — `firmware/rsh/`

| File | Kaam |
|---|---|
| `ws2812_rmt.h` (naya) | Arduino-free RMT bitstream encoder: GRB order, MSB-first, T0 400/800 ns, T1 800/400 ns, reset 50 us LOW; frame time 16 LEDs ~511 us / 60 LEDs ~1778 us (loop budget 3500 us se neeche). Host-testable. |
| `led_sink_rmt.h` / `led_sink_rmt.cpp` (naya) | RMT hardware sink (`rsh_led_sink_fn`): channel 0, clk_div 8 (100 ns tick), mem_block_num 8 (512 items), chunked write (512 items/chunk; beech wale chunks wait, aakhri non-blocking), in-flight guard (pichli frame chalte hue skip), write error par mustaqil not-ready. Re-init pin change par (uninstall + dobara config). |
| `led_driver.h` | Header comment update — Phase H items ab lag gaye (sink + wiring + A7). Logic nahi badla. |
| `led_patterns.h` | **SEC-H-10 (W7, Low): total run time cap 60 s** — `RSH_LED_TEST_MAX_MS 60000u`; `rsh_led_pattern_start()` totalMs ko clamp karta hai (reject nahi). repeats 255 × 5000 ms ~21 min tha; ab 60 s par khud all-off. |
| `net.cpp` / `net.h` | Wiring: `ledDrv` + `led_test_start_` adapter (handler se pattern → driver; pin badla ho to RMT sink re-init, fail ho to start nahi); `net_begin()` mein sink+driver init — fail ho to `led_test_request` NULL (fail closed, Phase G jaisa); `net_led_tick()` naya. |
| `rsh.ino` | `net_led_tick(nowMs)` — 4 jagah `led_update()` ke baad (main path, !netOk branch, OTA branch, WebOTA branch): chalta LED test Wi-Fi down / OTA ke doran bhi khatam hota hai. |

### PC-side — `software/`

| File | Kaam |
|---|---|
| `RSH.Core/Transport/BoardSession.cs` | `SendLedTestAsync(..., brightnessPct = 100, ...)` — brightness PC-side scaling (r/g/b × brightness% / 100, integer math); range 0..100, warna (false, wajah), koi frame nahi. Wire format mein koi brightness field nahi (§3.7). |
| `RSH.App/ViewModels/CalibrationTuning.cs` | `LedPatternIndex` (default 4 RAINBOW) + `LedBrightnessPct` (double, default 100) + `LedPatternNames`; `LedTest()` chuna pattern + brightness bhejta hai (SOLID laal / BLINK hari / CHASE neeli — A7 §25 ke mutabiq; RAINBOW/OFF rang nahi lete); status mein pattern + brightness. |
| `RSH.App/Views/Pages/TuningPage.xaml` | LED panel mein ComboBox (pattern) + Slider (brightness 0..100, % text) — baqi panel waisa ka waisa. |
| `RSH.App/Resources/gen_strings.py` + `Strings.{resx,ur.resx,hi.resx}` + `UiStrings.cs` | `LEDTestLEDTESTNeeds` ka "not in this build yet" text hataya (naya text: pattern + brightness chuno, 5 s chalega); naye keys `LedPattern` / `LedBrightness` (en/ur/hi). Generator se regenerate — diff sirf intended entries (337 keys). |

### Docs

- `docs/PROTOCOL-V2.1.md` §3.7: sink (RMT) + brightness (PC-side scaling) + SEC-H-10 cap 60 s darj.
- `docs/A7-GATE-PROCEDURE.md` §25: "Driver note" update — driver maujood, strip step ab PASS ho sakta hai; sandbox list mein §27 izafa; §25 bullet (callback NULL) ab "sink init fail ka rasta" likha hai.

## 3. Gate results (sandbox — 2026-11-13)

### Host tests — `tests-host/run_tests.sh`

**ALL PASS — C++ 4374 checks, 0 failures** (Phase G: 4239 thay; §27 ne
~135 checks add kiye) + host .cpp 50 checks + pin rules + vectors +
packet cross-verify (51 checks) + merge_bin mock — sab PASS.

§27 (naya): WS2812 encode guards (NULL/0/>60/kam capacity → 0),
timing constants datasheet range mein, GRB/MSB-first bit-for-bit
(0x11/0x22/0x33 pixel), reset item 50 us LOW, frame times
(16→510 us, 60→1778 us, 1→78 us), driver→encode chain (SOLID red
16 LEDs → 385 items), SEC-H-10 cap (255×5000→60000; boundary
60×1000=60000; 61×1000→60000 cap; repeats 0→5000; OFF→0; driver
se bhi cap — tick 55000 par on, 60000 par all-off + inactive).

### Firmware build — `pio run` (firmware/rsh)

**SUCCESS** — Flash 67.2% (881,117 B / 1,310,720), RAM ~98,864 B
(30.2% — `.dram0.data` 24,656 + `.dram0.bss` 74,208; static RMT
buffers ~11 KB: encode items 1441 + HW chunk 512). 0 warnings
(`-Werror`). Merged bin `RSH-Firmware-DevKitV1-v4.0.0.bin` 952,464 B.
(Dev build unsigned — signing key nahi thi, SEC-C-06 ke tehat
unsigned OTA reject hoti hai; signing WS ka kaam hai.)

### dotnet — `software/`

- **RSH.Tests: 642 pass / 2 fail / 7 skip (651 total).** Mere 9 naye
  tests (`PhaseHWorkstream2LedTests`: brightness 50→127 / 100 / 0,
  range reject −1/101, unknown pattern 5, BLINK period 50, OFF,
  board UNSUPPORTED propagate, bina key fail) — **9/9 PASS** (na fail
  list mein, na skip list mein).
- **VJoy.Tests: 56/56 PASS.**
- **RSH.App build: 0 warnings / 0 errors** (XAML + ViewModel +
  strings compile verify).
- 2 fail **WS2 scope se bahar** (Phase H WS6, firewall):
  `PhaseHWorkstream6Tests.FirewallRule_EnsureCommand_ProgramScoped_UdpInbound_Private`
  aur `Artifact_FirewallScripts_ProgramScoped` — un ki files is run
  ke doran bhi badal rahi thi; WS6 ko ye list de di jaye.
- 7 skip: UDP loopback permission / real-bin environment skips
  (pehle se maujood pattern).
- RSH.App.Tests **Linux par chal hi nahi sakte** (pehle se maujood
  environmental limit): `Microsoft.WindowsDesktop.App 10.0.0`
  runtime nahi mila — WPF Windows-only hai. Ye meri change ki wajah
  se nahi hai; App build se compile verify hai.

### Rig — Virtual Test Rig (4-hour long soak background mein chal raha tha)

- **Unit stage (`--only unit`): PASS** (exit 0, `[rig] unit: PASS`,
  2026-11-13 ~08:59 UTC) — dotnet tests + host tests + Python checks,
  rig runner ke andar.
- **Firmware-host stage (`--only firmware-host`): PASS** (exit 0,
  `[rig] firmware-host: PASS`, 2026-11-13 09:01 UTC) — run-host-tests.sh
  (72.5 s): C++ host tests + `pio run` build check. Note: REPORT.md /
  results.json aakhri run (firmware-host) ki hain — unit run ki report
  us se overwrite ho gayi (rig ek hi REPORT.md likhta hai; unit ka
  saboot us ke apne log mein hai: `[rig] unit: PASS`, exit 0).
- **Full rig NAHI chalaya** (task hidayat — long soak ke saath port
  conflict). **modules-onoff scenario bhi live NAHI chalaya** (wohi
  wajah): LED module on/off ka rig-observable hissa ye hai —
  - Virtual wheel mein LED output-only hai (`wheel.py:
    apply_disable` — "LED output-only hai — is ka koi data hota hi
    nahi"): stream mein LED ka data kabhi nahi aata, is liye
    modules-onoff (accel disable) LED ko stream se verify nahi kar
    sakta — ye rig ki limit hai, meri code ki nahi.
  - LED on/off ka asal rig-observable rasta config blob hai:
    LED_STRIP record ho to GET_CAPS mein LED row (pin 21) + LED_TEST
    `OK`; record na ho to row nahi + LED_TEST `UNSUPPORTED`. Ye host
    tests mein covered hai (§25: pin 0xFF → UNSUPPORTED; §27 chain).
  - **A7 mein live verify hoga** (neeche §4).

## 4. Board-only items (sirf real board par — A7 run mein)

A7-GATE-PROCEDURE.md §25 poora procedure hai; yahan summary:

1. Status LED (GPIO2): boot 2 Hz → PC connect solid on → update 5 Hz
   (pehle se wired — is WS mein nahi badla).
2. Strip (GPIO21): SOLID red, BLINK green 500 ms, CHASE blue 500 ms,
   RAINBOW, OFF — har pattern reply `OK` + strip par wahi nazar aaye;
   ghalat patternId 9 → `UNSUPPORTED` (strip ki halat na badle); test
   ke baad strip purani state par.
3. SEC-H-10 spot-check (board): BLINK 5000 ms repeats 255 bhejo —
   pattern 60 s ke baad khud band hona chahiye (21 min nahi); OFF
   bhejne par foran band.
4. Brightness spot-check (app): Tuning → LED → brightness 50% par
   SOLID — strip aadhi roshni par laal; 0% par kaali (pattern phir
   bhi 5 s chalta hai).
5. Expanders case (agar strip GPIO4 par ho): LED_STRIP record pin 4
   par push karo → LED_TEST `OK` aur strip GPIO4 par chale (RMT
   sink re-init — sandbox mein verify nahi ho sakta).
6. Loop timing: A7 measured run mein `maxCycleUs` < 3500 us confirm
   karo (RAINBOW har tick RMT write karta hai — frame ~511 us,
   hardware; CPU block nahi hota).

## 5. WS2 doran mile hue issues (gate report ke liye)

1. **Meri test-expectation bug (fix ho chuki):** §27 mein 0x33 ke
   bit0 ka T0 likha tha — sahi T1 hai (0x33 = 00110011, bit0 = 1).
   Host test ne pakda (1 failure), fix kiya, phir 4374/0.
2. **Meri C# test-expectation bug (fix ho chuki):** brightness error
   mein "Brightness" (capital B) dhoondha tha — asal string mein
   "brightness" lowercase hai. Test ne pakda (2 failures), fix kiya.
3. **Build bug (fix ho chuka):** `net.cpp` mein `led_test_start_`
   adapter `fw34Ctx` ki declaration se pehle tha — pio build ne
   pakda (`'fw34Ctx' was not declared in this scope`, net.cpp:75).
   Adapter ko `fw34Ctx` ke baad move kiya.
4. **Concurrent pio run race (code bug nahi — infra):** pehli
   `pio run` link ke baad `elf2image` mein fail hui
   (`firmware.elf` nahi mila) — usi waqt doosri WS (signing) bhi
   `pio run` chala rahi thi (PID 22418, `RSH_SIGNING_KEY` ke saath),
   shared `.pio/build` dir race hui. Dobara chalane par **SUCCESS**.
   Parallel WS builds ke liye shared build dir ek race hai — parent
   ko note karo.
5. **Doosri WS ke 2 failing tests (WS2 scope se bahar):** §3 dekho
   (WS6 firewall — `FirewallRule.cs:51` + artifact scripts).
6. **Flaky test (WS2 scope se bahar):** `PhaseGWorkstream1Tests.
   Detector_FindsLivePort_SkipsDeadPort` ek run mein fail hua (pehle
   do runs mein pass) — Phase G se jana-pehchana wired-loopback
   flake hai (run-to-run flip).

## 6. Gate verdict (WS2 scope)

- **Firmware logic (host tests + pio build): PASS** — 4374 checks /
  0 failures; pio SUCCESS (Flash 67.2%, RAM 30.2%).
- **PC-side (dotnet + App build): PASS (WS2 scope)** — mere 9/9
  tests PASS, VJoy 56/56, App build 0/0. 2 fail WS6 ke hain.
- **Rig stages:** unit PASS + firmware-host PASS (exit 0 dono).
- **SEC-H-10: FIXED + VERIFIED** — cap 60 s (clamp), host tests
  §27 mein (engine + driver dono levels par), board spot-check A7
  mein (§4.3).
- **LED:** driver + wiring COMPLETE; hardware (strip ki asal roshni)
  + board verify **A7** (§4) — sandbox mein verify nahi ho sakta.
