# A7 gate procedure — real-board measured run (FW-14, Plan v2.3 §9)

> **Status: A7 READY (board run pending, Phase H).** Procedure, checklist
> aur pass criteria tayyar hain. Asal board run Phase H mein hoga — user
> faisla: real board ke tamam tests sirf final production release par ek hi
> dafa honge. Is liye A7 yahan gate blocker NAHI hai; freeze sirf A7 pass
> ke baad (Plan v2.3 §9).

Authority: `RSH-PLAN-v2.3.md` §9 (R-M15), `docs/PERF-BASELINE.md` §B.

## Pass criteria (sab lazmi)

| Metric | Pass |
|---|---|
| `sent/s` (1 Hz serial line) | ≈ 200 (195–205) |
| Overruns / 10 min | **0** |
| `maxCycleUs` | **< 3500 µs** |
| Steering std-dev | Record karo (koi threshold nahi — baseline ke liye) |
| Board 10 min tak up rahe, koi reboot/restart nahi | — |

## Prerequisites (checklist)

- [ ] ESP32 DevKit V1 board + **data** USB cable (charge-only nahi).
- [ ] Firmware `v4.0.0` (ya jo version test ho raha hai) — `config.h` mein
      `FIRMWARE_VERSION` check karo.
- [ ] `RSH_DEBUG 1` in `firmware/rsh/config.h` (run ke baad wapas `0`).
- [ ] Merged factory bin maujood: `RSH-Firmware-DevKitV1-vX.Y.Z.bin`
      (local `pio run` se ya GitHub Release se).
- [ ] PC par RSH app (v2.0.0) tayyar — wheel AP `RSH` se connect hogi.
- [ ] Serial monitor: 115200 baud.
- [ ] Board par pehle se koi purana firmware ho to factory bin se flash
      karo (dekho `docs/BRICK-RECOVERY.md` — recovery steps wahi hain).

## Procedure (10 min)

1. `config.h` mein `RSH_DEBUG 1` karo, `pio run -t upload` se flash karo
   (ya merged bin `esptool.py write_flash 0x0 <bin>` se).
2. Board ko power do, serial monitor 115200 kholo.
3. PC ko board ke `RSH` Wi-Fi AP se connect karo, RSH app kholo —
   app ko HELO bhejna hai (data tabhi banta hai jab PC active ho).
4. **10 min** tak chalao. Har 1 s ek line aati hai:
   `sent/s=... seq=... overrun=... maxCycleUs=... steer min=... max=... mean=... std=... mv=...`
5. Har line ko log file mein save karo (`a7-run-<date>.log`).
6. Run ke baad `RSH_DEBUG` wapas `0` karo aur dobara flash karo
   (release build mein debug off rehta hai).

## Recording template

```text
A7 run — date: ________  firmware: ________  app: ________  board: ESP32 DevKit V1
sent/s:  min ___  max ___  avg ___   (pass: ≈200)
overruns (kul 10 min): ___          (pass: 0)
maxCycleUs (kisi bhi window mein): ___ µs   (pass: < 3500)
steering std-dev: ___               (record only)
reboots: ___  (pass: 0)
RESULT: PASS / FAIL
log file: a7-run-<date>.log
```

## Agar fail ho

- `sent/s` < 195: Wi-Fi/channel ya governor level check karo (level 3 par
  100 pps fallback expected hai — is surat mein run dobara karo, governor
  level note karo).
- Overruns > 0: `maxCycleUs` dekho — kaunsa hissa lamba hai (net/OTA/ADC);
  FW-6 governor + F11 full-loop timing ke mutabiq debug karo.
- `maxCycleUs` ≥ 3500: wahi — loop timing analyse karo, phir fix + dobara run.
- Board reboot: `docs/BRICK-RECOVERY.md` se recover karo, phir dobara run.

## LED test (Phase H A7 — board-only)

> **Status: procedure tayyar; board run Phase H mein hoga.** LED ka jo
> hissa sandbox mein verify ho sakta hai, wo ho chuka hai (neeche
> "Sandbox mein verify" dekho); baaqi sirf board par hota hai — yahan
> verify hone ka daawa nahi.

Do alag LEDs hain — inhein milao mat:

| LED | Pin | Kaam |
|---|---|---|
| Status LED (onboard) | GPIO2 (`PIN_LED`, `config.h:21`) | Board ki halat — blink pattern se (neeche table) |
| LED strip (WS2812) | GPIO21 (`PIN_LED_STRIP`, `config.h:54`) | `LED_TEST` control `0x0A` se patterns (§3.7) |

### A. Status LED — patterns (board par aankh se)

Authority: `firmware/rsh/status_led.h` (`LedState`), `status_led.cpp`.

| Halat | Pattern | Kab dikhta hai |
|---|---|---|
| `ApUpWaitingPc` | 2 Hz blink (250 ms on / 250 ms off) | AP up, PC ka HELO abhi nahi aaya |
| `PcConnected` | Solid on | PC connected (HELO mil gaya) |
| `Error` | 8 Hz tez blink (~62 ms on/off) | Error halat |
| `Updating` | 5 Hz blink (100 ms on/off) | OTA update chal raha hai |
| `SafeMode` | 1 Hz slow blink | Safe mode (BOOT-hold 5 s, §4.4) |
| `KeyResetArmed` | 4 Hz fast blink | Key-reset armed (BOOT-hold 10 s, §4.4) |

Procedure (A7 run ke doran, 10 min):

1. Board ko power do, **PC connect karne se pehle** — status LED
   2 Hz blink karni chahiye (AP up, PC ka intezar).
2. PC ko board ke `RSH` AP se connect karo, app kholo — LED **solid
   on** honi chahiye (PC connected).
3. Agar update chalao to update ke doran **5 Hz blink** dikhni chahiye,
   phir wapas solid on.
4. Har pattern + waqt log mein likho.

Pass: boot ke baad 2 Hz, connect ke baad solid on, update ke doran
5 Hz. Koi bhi pattern ghalat ho (masalan connect ke baad bhi blink,
ya LED bilkul off) → FAIL — `status_led.cpp` / `net.cpp` state
machine dekho.

### B. LED strip — LED_TEST patterns (board par, driver ke baad)

Authority: `docs/PROTOCOL-V2.1.md` §3.7, `firmware/rsh/led_patterns.h`,
`firmware/rsh/net.cpp` (LED_TEST dispatch).

- Request (8 B): `patternId` (0 OFF · 1 SOLID · 2 BLINK · 3 CHASE ·
  4 RAINBOW) | `r` | `g` | `b` | `periodMs` u16 (100–5000) |
  `repeats` u8 (0 = 5 s tak chalao phir stop) | reserved `0`.
- **R-L9 rule:** strip test sirf tab chalta hai jab LED strip GPIO21
  par digital-out configured ho (direct full rig); expanders hon to
  21/22 I2C hain aur strip freed digital pin (preset mein GPIO4) par
  jati hai. Ghalat pin → `PIN_CONFLICT` (5). LED module configured
  na ho ya unknown patternId → `UNSUPPORTED` (6).
- Test khatam hone par strip purani state par wapas aati hai.

Procedure (A7 run ke baad, alag step):

1. Board par LED-strip config maujood ho (GET_CAPS pin table mein
   GPIO21 `LED_STRIP` row check karo; na ho to SET_CFG se
   `LED_STRIP` record (type 6, pinA 21) push karo aur COMMIT karo).
2. Har pattern ek dafa bhejo (SOLID red, BLINK green 500 ms,
   CHASE blue 500 ms, RAINBOW, phir OFF) — har dafa reply status
   `OK` (0) aana chahiye aur strip par wahi pattern nazar aana chahiye.
3. Ghalat patternId (masalan 9) bhejo — reply `UNSUPPORTED` aana
   chahiye (fail closed, strip ki halat na badle).

Pass: paanchon patterns OK + strip par sahi nazar aayein; ghalat
patternId par UNSUPPORTED; test ke baad strip purani state par.

> **Driver note (Phase H WS2 — DONE):** RMT WS2812 driver lag gaya hai
> (`firmware/rsh/led_sink_rmt.cpp` — RMT hardware sink; bitstream encode
> `ws2812_rmt.h`; wiring `net.cpp` — `led_test_request` adapter +
> `net_led_tick()` loop se). LED_TEST handler ab `OK` deta hai (sink
> init fail ho to fail-closed `UNSUPPORTED`, pehle jaisa). **Strip test
> A7 mein ab PASS ho sakta hai** — paanchon patterns + bad-patternId
> + purani-state restore, neeche procedure ke mutabiq. SEC-H-10: total
> run time 60 s par cap hai (repeats 255 bhi ~21 min nahi).

### Sandbox mein verify (ho chuka hai)

- Host tests (`tests-host/test_host_cpp.cpp`, Arduino stub):
  status LED ke patterns — off LOW, `PcConnected` solid HIGH,
  `ApUpWaitingPc` 2 Hz (250 ms on/off), `Error` 8 Hz (~62 ms),
  `Updating` 5 Hz (100 ms) — sab PASS.
- Host tests (`tests-host/test_main.cpp` §LED / §3.7): pattern
  engine (pixel output per pattern + timing) aur R-L9 pin rule
  (GPIO21 digital, expanders par GPIO4) — PASS.
- Host tests (`tests-host/test_main.cpp` §27, Phase H WS2): WS2812
  RMT bitstream encode (GRB order, MSB-first, T0/T1 timing datasheet
  range mein, reset 50 us, frame 16 LEDs ~511 us / 60 LEDs ~1778 us),
  driver → encode chain (SOLID red frame), SEC-H-10 cap (repeats 255
  × 5000 ms → 60 s; boundary 60 × 1000 ms; driver se bhi cap) — PASS
  (4374 checks, 0 failures, host suite).
- Host tests (`tests-host/test_main.cpp` §25, Phase F WS5): LED_TEST
  handler dispatch (`ctrl_handlers.h`, `net.cpp` route) — valid pattern
  `OK`, unknown pattern / reserved byte / period out of range /
  length ≠ 8 → `UNSUPPORTED`, callback NULL (sink init fail ka
  fail-closed rasta — Phase H WS2 mein production callback adapter
  par hai) → fail-closed `UNSUPPORTED`; R-L9 negatives — pin 25 (ADC-only)
  → `PIN_CONFLICT`, expanders hon to pin 21 → `PIN_CONFLICT`, pin 4
  → OK. Sab PASS (2834 checks, 0 failures, host suite).
- Board-only (sandbox mein verify NAHI ho sakta): status LED ki
  asal roshni (GPIO2), strip ki asal roshni (GPIO21), LED_TEST ka
  live control reply, driver ke baad strip patterns.

### Recording template (LED — A7 log mein izafa)

```text
LED — status (GPIO2): boot 2 Hz [OK/FAIL] → PC connect solid [OK/FAIL] → update 5 Hz [OK/FAIL]
LED — strip (GPIO21): SOLID [OK/FAIL] · BLINK [OK/FAIL] · CHASE [OK/FAIL] · RAINBOW [OK/FAIL] · OFF [OK/FAIL]
LED — bad patternId 9 → UNSUPPORTED [OK/FAIL] · test ke baad purani state [OK/FAIL]
LED driver maujood: [haan — Phase H WS2, RMT sink; nahi ho to strip step UNSUPPORTED expected, PASS nahi]
LED RESULT: PASS / FAIL
```

## Freeze rule (Plan v2.3 §9)

Freeze sirf A7 pass ke baad. Freeze ke baad bhi koi "frozen" daawa nahi —
har change ke liye naya version.
