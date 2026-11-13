# Phase G WS5 — Phase H A7 Handoff (Clutch / Shifter / Encoder / LED)

**Workstream:** Phase G WS5 — Clutch/shifter/encoder board-side verify (sandbox) + LED groundwork.
**Date:** 2026-11-13. **Status:** Sandbox verify complete (neeche numbers); sirf wo items
yahan hain jo **sach mein sandbox mein verify nahi ho sakte** (real board / strip /
Windows vJoy chahiye). Baqi sab Phase G mein mukammal hai — koi implementable item
Phase H ko nahi diya gaya.

Authority: `RSH-PLAN-v2.3.md` §5 (Phase G), `RSH-FIRMWARE-PLAN.md` FW-11/FW-12,
`docs/A7-GATE-PROCEDURE.md` (§25 LED test), `docs/PROTOCOL-V2.1.md` §1.1/§3.6/§3.7.

---

## 1. Sandbox mein verify ho chuka hai (Phase H ko dobara nahi karna)

### Host tests — `tests-host/test_main.cpp` §26 (Phase G WS5, naya)

`./run_tests.sh` — **4035 checks, 0 failures** (Phase F ke baad 2834 thay; §26 ne
~1150 checks add kiye, C3 pinmap team ne ~46 add kiye).

| Item | Test | Result |
|---|---|---|
| Encoder 200-turn stress | 200 turns aage (+800) + 200 peeche = exact 0, 0 glitches | PASS |
| Encoder i16 wrap | 32760 + 40 steps → −32736 (delta +40); wapas 32760; neeche boundary −32760 − 40 → 32736 (delta −40) | PASS |
| Bounce/reversal stress | 300 seeded rounds (LCG), clean cycles + bounce pairs — aakhri count sirf clean cycles ka; glitches ≥ bounce pairs | PASS |
| Invalid-transition burst | 500× dono bits ek saath — count 0, glitches 1000; phir clean cycle +4 (decoder recover) | PASS |
| Shifter gate state machine | Sequential up/down bits, gear ignore; H-pattern gears 1–6 → bits 5–10, up/down ignore; gear 7 → 0; sequence 1→6→neutral; invalid 2-bits → neutral + foran recover; packet-level mode switch (sequential 0x03 vs H-pattern gear 3 → 0x80) | PASS |
| H-pattern + encoder conflict | Conflict flag 1; H-pattern mode mein encoders report nahi hote (n_enc 0) | PASS |
| Clutch slot sweep (GPIO33 path) | raw 0/1024/2048/3072/4095 → slot 3 mV 0/825/1650/2476/3300; clutch band ho to raw 4095 bhi slot 3 mein nahi (mask 0x17, axes[3] 0, handbrake slot 4 phir bhi 3300) | PASS |
| LED driver abstraction | init/start/tick/stop validation; SOLID 1 write/5 s; BLINK 10 writes/5 s; CHASE pixel rotation; RAINBOW frame change; OFF 1 write; repeats=2 → 2000 ms done; stop → all-off; R-L9 pin rule; handler → driver chain (LED_TEST dispatch se pattern lekar frames) | PASS |

### Firmware build — `pio run` (firmware/rsh)

**SUCCESS** — RAM 25.9% (84,952 B / 327,680), Flash 66.6% (872,765 B / 1,310,720).
0 warnings (`-Werror`). `led_driver.h` Arduino toolchain se bhi compile hota hai
(`net.cpp` mein include — sirf compile check, koi wiring nahi).

### Rig — Virtual Test Rig (quick, full run 2026-11-13 ~07:30 UTC)

**25/25 scenarios PASS** (long-soak SKIPPED — sirf `--long` se chalta hai),
firmware-host stage PASS. Khaali machine par (VM reboot ke baad): soak loss 0%,
rate 202/s, jitter p99 934 µs, engineTicks 12010/12000. WS5 targeted:
clutch-sweep PASS (rate 202/s, 6009 ticks), clutch-range PASS (validPackets 1984,
rate 197/s, 2008 ticks), shifter-sequential PASS (6008 ticks, gear bits 0x7E0),
shifter-hpattern PASS (gearMaskSeen 126), encoder-glitch-v2 PASS (5942 packets,
6008 ticks, maxAbsEncDelta ≤ 32). **Unit stage FAIL — sirf dotnet-test ki wajah
se** (neeche §4.3 — doosri WS ke 9 tests; host-tests / pin-rules / vectors PASS).

### LED groundwork — kya ban chuka hai (`firmware/rsh/led_driver.h`)

- **Pattern engine** (`led_patterns.h`, pehle se tha): OFF/SOLID/BLINK/CHASE/RAINBOW,
  hue wheel, R-L9 validation — host-testable, PASS.
- **Driver abstraction** (`led_driver.h`, Phase G WS5 — naya, asal code, koi
  placeholder nahi): `RshLedDriver` — `start(cfg, nowMs, ledPin, expanders,
  ledConfigured)` (validate pehle, ghalat request chalta pattern nahi bigarti),
  `tick(nowMs)` (200 Hz loop se; sink sirf frame badalne par — SOLID 1 write,
  BLINK phase-change par, CHASE step-change par), `stop()` (all-off ek dafa),
  done par aakhri frame all-off (strip purani state par, §3.7).
- **Sink boundary:** `rsh_led_sink_fn(user, pixels, count)` — hardware sink
  (WS2812 wire protocol) is ke peeche hai. Driver ko wire timing se koi matlab
  nahi; sink Phase H mein aayega (§3).
- **Fail-closed barqarar:** `net.cpp` mein `led_test_request` abhi bhi NULL hai —
  LED_TEST handler `UNSUPPORTED` deta hai. Ye bug nahi hai (false PASS se behtar).

---

## 2. Board-only items (sirf real board par — A7 run mein)

### 2.1 Clutch — GPIO33 analog read (compiled default map)

Host test mein sirf raw counts → packet slot 3 ka rasta verify hua hai; **asal ADC
read sirf board par** hota hai (`inputs.cpp`: `avg_pedal(PIN_CLUTCH_ADC)`).

**Procedure (A7):**
1. Clutch config push karo (SET_CFG: AXIS_ADC slot 3, pin 33) + COMMIT + CONFIRM
   (30 s). GET_CAPS se verify karo ke clutch on hai (axis mask bit 3).
2. Clutch pot ko haath se ghumao: chora hua → raw ~0 (mV ~0); aadha → raw ~2048
   (mV ~1650); poora daba → raw ~4095 (mV ~3300). App Dashboard par clutch axis
   0–100% smooth chalna chahiye, bina jumps ke.
3. **Expected:** sweep ke doran packet loss 0%, axis monotonic (wapas jaane par
   wahi values ±30 counts). Floating/disconnected: clutch wire nikalo → 10 s
   rail-stuck + doosre axes move karne par axis-invalid bit 3 set ho (F2); still
   wheel par false invalid nahi.
4. **Clutch + gear-down conflict (board):** clutch on hone par gear-down BUTTON
   record (pin 33) COMMIT par PIN_CONFLICT se reject hona chahiye (host test
   §25 mein covered; board par live SET_CFG se dobara confirm karo).
5. **Preset map (full-rig):** preset mein clutch **GPIO35** par hai (steering 32,
   throttle 33, brake 34, clutch 35, handbrake 36). Phase G C3 se clutch **kisi
   bhi ADC1 pin** par chal sakta hai (pehle sirf pin 33 — ab pinmap se parha jata
   hai). A7 mein preset map bhi test karo: clutch pin 35 par sweep → slot 3 mein
   wahi mV values.

### 2.2 Shifter — H-pattern pins + gate-mode switch (live)

Host test mein decode + packet bits verify hue hain; **asal pin reads + mode
switch live sirf board par** hote hain.

**Procedure (A7):**
1. Sequential mode (default): gear-up (GPIO27) dabao → vJoy button 0 (shift-up);
   gear-down (GPIO33, clutch off ho to) → button 1. Debounce: ek press = ek
   button event (bounce se double-shift nahi).
2. H-pattern mode par switch karo (SET_CFG type 11, mode 1) — **reboot ke bagair**,
   agle hi cycle se gear decode badalna chahiye (runtime mode check, inputs.cpp).
   Gears 1–6 ke switches (GPIO 13/14/16/17/18/19) ek-ek karke dabao → vJoy
   buttons 5–10 (gear g → bit 4+g). Neutral (koi switch nahi) → koi gear bit nahi.
3. Do gears ek saath dabao → neutral (koi gear bit nahi, kabhi ghalat gear nahi).
4. H-pattern mode mein encoders **report 0** hone chahiye (shared pins 16/17/19 —
   conflict by design); sequential mode par wapas aao to encoders wahi se
   continue karein jahan ruke thay (decoder prev state mehfooz hai).
5. Mode wapas sequential karo → up/down bits wapas aayein, gear bits clear.

### 2.3 Encoders — physical rotation + bounce (board)

Host test mein decoder logic + 200-turn + wrap verify hua hai; **asal quadrature
signals (bounce, signal integrity, speed) sirf board par** hote hain.

**Procedure (A7):**
1. Encoder 1 (GPIO23/19) ko haath se 200 turns aage ghumao, phir 200 peeche —
   PC par cumulative count wapas **exact 0** hona chahiye (0 drift). Har turn = 4
   counts (1 detent/cycle — encoder ke PPR ke mutabiq adjust karo; ratio
   recorded karo).
2. Tez ghumao (musalsal ek disha, ~5 turns/s) — undercount nahi hona chahiye
   (reversal-cancellation sirf foran ulta hone wala step cancel karta hai).
3. Bounce: encoder ko detent boundary par halka hilao (aage-peeche) — net count
   0 rehna chahiye (glitch filtering).
4. i16 wrap: 8192 turns ek disha mein (32768 counts) — wrap ke paar PC delta
   sahi rehna chahiye (wrap-aware delta, §26 host test mein covered; board par
   spot-check: wrap ke aas-paas delta +1 per step, kabhi −32768 jump nahi).
   *Note: 8192 turns lamba hai — kam az kam 1000 turns ka spot-check karo aur
   baaqi host-test proof par bharosa karo.*
5. Encoder → vJoy buttons 65–72 (PC app mapping): har encoder step se vJoy
   button press aana chahiye (Windows PC par, real vJoy — rig mein sirf
   recording stub hai).

### 2.4 LED strip — driver + visual check (Phase H ka kaam, §3 ke baad)

`A7-GATE-PROCEDURE.md` §25 (LED test) poora procedure hai — yahan sirf
WS5-se mutaliq summary hai:
- Status LED (GPIO2): boot 2 Hz → PC connect solid on → update 5 Hz.
- Strip (GPIO21, R-L9): SOLID red, BLINK green 500 ms, CHASE blue 500 ms,
  RAINBOW, OFF — har pattern reply OK + strip par wahi nazar aaye; ghalat
  patternId 9 → UNSUPPORTED; test ke baad strip purani state par.
- **Driver se pehle strip step ka expected result `UNSUPPORTED` hai** (fail-closed,
  bug nahi) — usay PASS ke tor par darj mat karo.

---

## 3. LED — Phase H mein baaqi kaam (exact)

Ye **ek hi baaqi implementable hissa** hai (sandbox mein strip nahi hai, is liye
wire timing verify nahi ho sakta):

1. **Sink chuno aur implement karo** — ek rasta:
   - (a) **RMT driver** (preferred — ESP32 hardware RMT, timing exact, CPU free;
     `rmt` driver se WS2812 800 kHz bitstream), ya
   - (b) **Adafruit NeoPixel lib** (`platformio.ini` mein `lib_deps =
     adafruit/Adafruit NeoPixel`) — aasan, lekin timing interrupt-sensitive hai.
   - Faisla Phase H mein karo; dono ka `rsh_led_sink_fn` signature ek jaisa hai
     (`(user, pixels, count)`), is liye driver (`led_driver.h`) ko chhoona nahi
     parega.
2. **Wiring (`net.cpp` + `rsh.ino`):**
   ```c
   // net.cpp — static RshLedDriver + adapter (led_test_request ki jagah):
   static RshLedDriver ledDrv;
   static void led_test_start_(void *u, const RshLedPattern *p) {
     (void)u;
     // ledPin/expanders/configured: fw34Ctx se (GET_CAPS/config blob);
     // nowMs: millis().
     rsh_led_driver_start(&ledDrv, p, millis(), ctxLedPin, ctxExpanders, 1);
   }
   // fw34Ctx.led_test_request = led_test_start_;  (NULL ki jagah)
   // rsh.ino loop mein: rsh_led_driver_tick(&ledDrv, millis());
   // Init (boot par, LED_STRIP record milne par):
   //   rsh_led_driver_init(&ledDrv, LED_STRIP_COUNT, <sink_fn>, <user>);
   ```
   - Sink init fail ho (strip maujood nahi / lib init fail) to
     `led_test_request` NULL hi rakho — handler phir bhi fail-closed
     UNSUPPORTED deta hai.
   - Tick cost: 16 LEDs × integer pixel math — 200 Hz loop budget (< 3500 µs)
     par asar na hone ke barabar hai; A7 measured run mein `maxCycleUs` se
     confirm karo.
3. **Board verify (A7, §2.4):** paanchon patterns + bad-patternId + purani-state
   restore + status LED patterns — aankh se + log mein.
4. **Docs update:** `A7-GATE-PROCEDURE.md` §25 ka "Driver note" hatakar driver
   maujood likho; `docs/PROTOCOL-V2.1.md` §3.7 mein sink choice darj karo.

---

## 4. WS5 doran mile hue issues (gate report ke liye)

1. **Rig throughput FAILs — VM contention (code bug nahi):** `clutch-range`
   (engineTicks 712–961 vs 1500 expected; wheel 102–152 pps vs 200 target) aur
   `encoder-glitch-v2` (engineTicks 3654 vs 4500; wheel 143.9 pps) — failing
   assertions sirf throughput wali hain; functional assertions (steering range,
   validPackets validity, loss 0%, auth gate) PASS. Saboot: wheel khud target
   rate nahi bana saka (Python wheel 102–144 pps), harness ticks usi ke saath
   girte hain; Phase F gate mein ye scenarios 25/25 PASS thay (machine khaali
   thi); meri changes C# harness / wheel.py ko chhooti nahi (sirf firmware
   headers + host tests). **Dobara run se confirm:** VM reboot ke baad khaali
   machine par full rig chalaya to **dono PASS** (clutch-range: 1984 packets,
   197/s, 2008 ticks; encoder-glitch-v2: 5942 packets, 201/s, 6008 ticks).
2. **Cross-team tree breaks (doosri WS ki in-flight edits, fix ho chuki hain):**
   - C# harness build: wired-transport WS ne `ISerialPort.cs` (System.IO.Ports)
     baghair PackageReference ke add kiya → build FAIL; unhon ne khud
     `System.IO.Ports 10.0.0` reference add ki.
   - `WlanProfileTempFile.cs:113` (C9): `File.SetAccessControl(path, security)`
     .NET Framework API hai — .NET 10 mein CS1501; C9 team ne khud
     `new FileInfo(path).SetAccessControl(security)` + `System.IO.FileSystem.
     AccessControl 5.0.0` reference se fix kiya (maine scratch project mein
     dono forms test karke confirm kiya tha).
   - Firmware: C3 pinmap WS ne `modules.h`/`inputs.cpp`/`pinmap.h`/
     `config_transaction.h`/`test_main.cpp` rewrite kiye — beech mein ek
     `run_tests.sh` run 2 failures ke saath aaya (file mere build ke doran
     badal gayi thi); dobara chalane par **4035 checks, 0 failures**. Un ka
     `RSH_SHIFTER_HPATTERN` macro-order issue (W4 report) current tree mein
     nahi hai — `pinmap.h:178` literal `1` istemal karta hai (modules.h ko
     include karna circular hota); `healthClutch` error bhi ab nahi hai
     (`pio run` SUCCESS).
   - **Note:** C3 ne factory blob badla hai — ab full 11-record map hai (LED
     strip pin 21 shamil; clutch phir bhi nahi — default OFF barqarar). Clutch
     ab **kisi bhi ADC1 pin** par chal sakta hai (pehle sirf pin 33).
3. **Meri khud ki test-expectation bug (fix ho chuki):** §26 mein H-pattern gear
   3 ka expected buttons value `0x20` likha tha — sahi `0x80` hai (bit 4+3=7).
   Host test ne pakda (1 failure), fix kiya, phir **3989 checks, 0 failures**.
4. **Unit stage dotnet-test FAIL — doosri WS ke 9 tests (WS5 scope se bahar):**
   Full quick rig mein `dotnet test`: RSH.Tests 9 failed / 548 passed / 6
   skipped (VJoy.Tests 56/56 PASS). **9 ke 9** `PhaseGWorkstream1Tests` hain
   (wired-transport WS1 ke naye loopback serial tests) — `LoopbackSerialPort.
   Write` se `InvalidOperationException: "Port band hai"` (fake port Write se
   pehle open nahi hota) aur us ke nateejay mein null replies:
   `Detector_FindsLivePort_SkipsDeadPort`, `SerialControlTransport_
   ExchangeAsync_Loopback`, `SerialWheelSource_V2StreamAndStatus_Loopback`,
   `SerialPingTransport_ExchangeAsync_Echoes_Loopback`, `Link_BadHelloCounted_
   UnsolicitedControlCounted`, `Link_ControlExchange_GetCaps_Loopback`,
   `Link_HelloExchange_Loopback_ReturnsHello`, `Link_SecondControlExchangeWhile
   Busy_Throws`, `Link_StreamStatusProbeEvents_Loopback` (PhaseGWorkstream1Tests.
   cs). Maine inhein fix nahi kiya — ye wired WS ka in-flight code hai (un ki
   files is run ke doran bhi badal rahi thi); un ko ye list de di jaye. Counts
   run-to-run halke se badalte hain (548/6 vs 549/5) — ek test pass/skip ke
   beech flip karta hai (sandbox UDP loopback permission par munhasir skips).

---

## 5. Gate verdict (WS5 scope)

- **Firmware logic (host tests + pio build): PASS** — report waqt par 4239
  checks / 0 failures (meri §26 + C3 pinmap additions); pio SUCCESS
  (RAM 25.9%, Flash 66.6%).
- **Rig scenarios: 25/25 PASS** — full quick rig (VM reboot ke baad khaali
  machine), firmware-host stage PASS, soak loss 0% / rate 202/s / jitter p99
  934 µs.
- **Unit stage: FAIL — WS5 se bahar** (dotnet-test ke 9 failures, sab
  PhaseGWorkstream1Tests wired-loopback — §4.4). Ye WS5 gate ko block nahi
  karte (WS5 mein koi C# change nahi), lekin overall Phase G gate ke liye
  wired WS ko fix karne honge.
- **LED:** groundwork COMPLETE (driver abstraction + host tests); hardware sink
  + board verify **Phase H** (§3) — yehi ek baaqi item hai jo sandbox mein
  verify nahi ho sakta.
- **Board-only handoff:** §2 (clutch GPIO33/35, shifter H-pattern, encoder
  200-turn/wrap, LED strip) — A7 run mein.
