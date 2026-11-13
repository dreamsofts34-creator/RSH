# RSH (Real Sim Hub) — Firmware Plan (v4.0 track)

Target board: **ESP32 DevKit V1** (original ESP32). Starting point: firmware **v3.3** (fixed pins in `config.h`, 18-byte protocol v1, SoftAP + UDP, Web OTA).
Product name is now **RSH — Real Sim Hub**; on-air identifiers were renamed with it in Step 0 (SSID `RSH`, OTA hostname `rsh`) — boards flashed with the old firmware must be reflashed, since the new app only joins the `RSH` network. **All requested features are in scope** (battery, LED/display, encoders, expanders, other boards via capability tables).

**Universal, modular wheel (owner requirement):** *everything runs on ONE ESP32* — steering, throttle, brake, clutch, handbrake, shifter, horn, buttons, encoders, LEDs. The user attaches only the parts they want; whatever is not attached simply does not exist (no module, no pin, no packet field, no UI). Power can come from **any source** (USB, battery, SMPS/buck converter) — the firmware does not care.

Goal of this track: **every pin, module and button is assigned from the PC app at runtime** — no re-flashing to change wiring — while keeping the principle *firmware dumb, PC smart*.

---

## 0. Performance guardrails (non-negotiable)

The v3.3 firmware is tested and reliable at **200 packets/s**. v4.0 must not degrade that.

| # | Rule |
|---|---|
| FP1 | **The 5 ms sampling/send tick has priority over everything.** Config, control commands, status, LED work happen in the idle gap between ticks, with a **hard time cap per loop pass** (e.g. ≤ 300 µs); anything bigger is split across passes. |
| FP2 | **No heap allocation in the tick.** Module table, packet buffers, command buffers are static. No `String`, no `new` after boot. |
| FP3 | **Flash writes (NVS) only on COMMIT/CONFIRM**, never in normal operation (wear + cache stalls). Sampling is paused for that moment exactly like OTA does today — an expected single skipped tick, flagged in the status packet. |
| FP4 | **Per-axis oversampling stays governed** by the existing adaptive governor; adding axes lowers samples per axis before it lets cycles overrun. ADS1115/expander reads are non-blocking and round-robin (one per cycle). |
| FP5 | **Encoders via PCNT hardware counters**, not per-edge interrupts/polling — zero CPU while turning. |
| FP6 | **Wi-Fi unchanged:** same SoftAP, unicast UDP, no extra broadcast traffic; status packet 1 Hz (~40 B); control channel is request/response only. |
| FP7 | **No-regression by construction + cheap checks.** The owner cannot take measurements, so: (a) the sampling/send path of v3.3 is kept as-is and only extended additively; (b) the status packet already carries `maxCycleUs`, overruns, free heap and governor level, and the app's Performance panel shows them live — so timing health is visible without any tool; (c) compile output (flash/RAM %) is checked every phase (v3.3: 71.4 % / 14.5 %, limit flash < 85 %); (d) host tests assert the logic (no allocation, bounded loops); (e) the owner's normal use ("feels the same, no drops") is the acceptance check. |
| FP8 | Default config reproduces v3.3 exactly — **an upgrade must not change timing or behaviour** for the existing wiring. |

## 1. Requirements (from owner)

| # | Requirement | Firmware consequence |
|---|---|---|
| F-1 | GPIO pins assigned from software | Pin map becomes runtime data (stored on the board), not `#define`s |
| F-2 | Per-module setup: steering, throttle, brake, clutch, handbrake, shifter, horn/buttons, encoders | A generic **module table** replaces hard-coded inputs |
| F-3 | Advanced tuning per module | Raw signal quality features on the board (oversampling, hysteresis, debounce); all *shaping* (curves, deadzones) stays on the PC |
| F-4 | Dashboard needs battery %, signal strength, link health | Periodic **status packet** from the board |
| F-5 | (UI preview) LED strip + display, rotary encoders, I²C expanders | Optional modules in later phases |
| F-6 | **Everything on one ESP32, user picks what to attach** | Modular config: any subset of modules is valid; the minimum is one input; pin budget helper tells the user what fits (section 3.3) |
| F-7 | **Any power source** (battery, SMPS, USB) | Power-agnostic firmware; battery monitoring is an *optional* module (section 3.4) |

**Design rule — what lives where**

- **On the wheel (persisted in flash):** hardware facts only — pin assignments, module on/off, sensor type, debounce times, sample counts, Wi-Fi settings, LED config.
- **On the PC (profiles):** everything that is a *preference* — curves, deadzones, rotation, smoothing, button actions, per-game settings.
- Reason: calibration and tuning survive reflashing and never depend on flash wear; swapping wheels/PCs is just loading a profile.

---

## 2. Gap analysis (v3.3 → v4.0)

| Area | v3.3 today | Needed |
|---|---|---|
| Pins | `#define PIN_*` in `config.h` | Runtime table in NVS, validated |
| Axes | steering, accel, brake (3) | up to 8 (adds clutch, handbrake, spare) |
| Buttons | 4 (bitmask in 1 byte) | up to 32 (+ expander inputs) |
| Encoders | none | quadrature via PCNT, delta reporting |
| Packet | v1, 18 B, fixed fields | v2, variable axes/buttons, v1 still accepted for old apps |
| Control | `HELO` / `RSTX` only | Reliable command channel (get/set config, probe pin, etc.) |
| Status | none | RSSI, battery, heap, overruns, uptime @ 1 Hz |
| Safety | LED error state | Config rollback, safe mode, factory reset |
| Build | none shipped | `platformio.ini` + merged-bin script (added in the cleaned package) |

---

## 3. Pin model (the part that can brick a board — validate hard)

### 3.1 ESP32 DevKit V1 capability table (the authority lives in firmware, UI reads it)

| GPIO | Digital in | Digital out | Pull-up | ADC (with Wi-Fi ON) | Notes |
|---|---|---|---|---|---|
| 0 | yes | yes | yes | **no** (ADC2) | strapping — avoid (boot mode) |
| 1, 3 | — | — | — | — | UART0 TX/RX — reserved |
| 2 | yes | yes | yes | no (ADC2) | strapping + onboard LED — reserved for status LED |
| 4 | yes | yes | yes | no (ADC2) | ok digital |
| 5 | yes | yes | yes | — | strapping (boot log) — digital ok with caution |
| 6–11 | — | — | — | — | connected to flash — **forbidden** |
| 12 | yes | yes | yes | no (ADC2) | strapping (flash voltage) — **must be LOW at boot**, avoid |
| 13, 14 | yes | yes | yes | no (ADC2) | ok digital |
| 15 | yes | yes | yes | no (ADC2) | strapping — avoid |
| 16, 17 | yes | yes | yes | — | ok on DevKit V1 (WROOM); PSRAM on WROVER |
| 18, 19, 23 | yes | yes | yes | — | SPI — ok |
| 21, 22 | yes | yes | yes | — | **default I²C** (SDA 21 / SCL 22) |
| 25, 26, 27 | yes | yes | yes | no (ADC2) | ok digital |
| 32, 33 | yes | yes | yes | **yes (ADC1)** | best analog pins + digital |
| 34, 35, 36, 39 | yes | **no** | **no** | **yes (ADC1)** | input-only, no internal pull-up — analog only, or digital with external resistor |

Rules enforced by the firmware (and mirrored greyed-out in the UI with the reason):

1. **Analog axis ⇒ ADC1 only** (32, 33, 34, 35, 36, 39). ADC2 pins return garbage while Wi-Fi is on.
2. **Digital button ⇒ pull-up capable pins only**, or input-only pins with `external pull-up` flag set by the user.
3. **Forbidden:** 6–11 always; 1, 3 unless debug is off; 0/2/5/12/15 flagged *"strapping — may stop boot"* and refused for modules that are pulled/driven at boot.
4. **No pin used twice.** I²C pins reserved when any expander is enabled.
5. Max counts: 8 axes, 32 buttons, 4 encoders, 1 LED strip. **Any subset is valid** (zero clutch, zero encoders, no LEDs …) as long as at least one input module exists.

### 3.2 Module types

| Type | Binding | Options stored on the wheel |
|---|---|---|
| `AXIS_ADC` | ADC1 pin | invert-at-source (no), samples (adaptive), mV mode, RC filter note |
| `AXIS_ADS1115` | I²C addr + channel | data rate, gain |
| `BUTTON` | GPIO or expander pin | active-low/high, pull mode, debounce ms |
| `SHIFT_UP` / `SHIFT_DOWN` | = BUTTON with role | debounce, optional lockout ms |
| `ENCODER` | pin A + pin B (+ optional push) | steps per detent, PCNT filter |
| `LED_STRIP` (phase F5) | GPIO + count + type (WS2812) | brightness |
| `BATTERY` (optional) | ADC1 pin + divider ratio | divider values, chemistry curve, or "external supply" |

Plus two generic types so the system stays universal: `AXIS_GENERIC` (any extra pot/hall/slider the user wants) and `BUTTON_MATRIX` (rows × cols with per-key diode assumption; e.g. 4×4 = 16 buttons on 8 pins).

Expanders (phase F4): PCF8574, MCP23017 (buttons), ADS1115 (axes). All matching the Settings → "I²C bus — expanders" section in the UI preview.

### 3.3 Pin budget — what fits on ONE DevKit V1

Safe, usable GPIOs after removing flash (6–11), UART0 (1, 3), strapping/avoid (0, 5, 12, 15) and the status LED (2):

| Group | Pins | Count |
|---|---|---|
| Analog-capable (ADC1) | 32, 33, 34, 35, 36, 39 | **6** (34/35/36/39 are input-only, no pull-up) |
| Digital, pull-up capable | 4, 13, 14, 16, 17, 18, 19, 23, 25, 26, 27 | **11** |
| I²C (only if expanders used) | 21 (SDA), 22 (SCL) | 2 |
| Total usable | | **19** (+ GPIO5 with caution) |

Example **full build on one board, no expanders**:

| Module | Pins used |
|---|---|
| Steering, throttle, brake, clutch, handbrake (5 analog) | 5 of the 6 ADC1 pins (1 spare, e.g. battery monitor) |
| Shifter up/down + horn + 4 more buttons (7) | 7 digital |
| 2 encoders (A/B) | 4 digital |
| LED strip (data) | 1 digital |
| **Total** | **~17 of 19** — it fits, with little room to spare |

More than that (e.g. 15+ buttons) ⇒ use a **button matrix** (16 buttons = 8 pins) or an **MCP23017** (16 buttons = 2 pins, shared I²C). The app shows a live **"pins used / free"** counter and tells the user which option to use when the budget runs out. Because the pin table comes from the board (`GET_CAPS`), this also works for other ESP32 variants later.

### 3.4 Power — any source, optional monitoring

- **Firmware is power-agnostic.** USB, Li-ion (with a charger/protection board), or an SMPS/buck converter into the DevKit's 5 V/VIN pin all work. Nothing in the firmware depends on the source.
- **Battery monitoring is an optional `BATTERY` module** (ADC1 pin + resistor-divider ratio set in the app). Not attached ⇒ no battery tile, no field, no code path. With it: percent is estimated from voltage using a selectable chemistry curve (Li-ion / LiFePO4 / custom min–max); the user can also choose **"external supply / no battery"** to just show supply voltage.
- **Wiring cautions the app will show** when a power module is enabled: ADC pins must never see > 3.3 V (use a divider); take the pots' reference from **3V3**, not 5 V; an SMPS adds ripple — add a 0.1 µF cap at each analog input (the trimmed-mean oversampling already rejects spikes); brown-out on weak supplies shows up as a reboot counter in the status packet.

---

## 4. Configuration storage & safety

- Stored in **NVS** (`Preferences`) as one versioned binary blob + CRC32. Two slots (A/B) written alternately; boot picks the newest valid one.
- **Transaction flow** (protects against a bad pin map that stops the board from reaching Wi-Fi):
  1. PC `SET_CFG` → board validates fully (rules in 3.1) → stages it, replies `OK` or a list of errors.
  2. PC `COMMIT` → board applies it live (or reboots if I²C/bus changed) and starts a **10 s confirm timer**.
  3. PC `CONFIRM` within 10 s → config becomes permanent. No confirm (PC lost, wrong pins, Wi-Fi never came back) → **automatic rollback** to the previous config.
- **Boot-loop guard:** 3 consecutive boots that never reach "AP up" ⇒ load last-known-good, else factory defaults, and blink the error pattern.
- **Safe mode:** hold BOOT (GPIO0) ≥ 5 s at power-up ⇒ factory pin map + AP with default credentials-from-`config.h`.
- **Factory defaults** = today's v3.3 pin map (34/35/36 analog, 25/26/27/33 buttons) so existing wiring keeps working after upgrade.
- Config writes need the same password gate as OTA (see 6.3).

---

## 5. Protocol v2

Keep v1 working: the board answers in the version the PC asked for in `HELO`.

### 5.1 Data packet v2 (board → PC, UDP 4211, 200 pps default)

```
0      magic 0xA5
1      version 2
2-3    sequence u16
4-7    timestamp µs u32
8      axis mask  (bit n = axis n present)          up to 8 axes
9      flags      (bit0 steering in mV, bit1 config-changed, bit2 low-battery …)
10-13  buttons u32 (bit n = button n)
14-15  encoder count nE (u8) + reserved     (or fold into flags)
16..   axes u16 × popcount(mask)
..     encoder deltas i8 × nE
last2  CRC16-CCITT over everything before
```

Packet size is typically 24–32 B — still a single UDP datagram. Final layout is frozen in `docs/PROTOCOL-V2.md` (phase F0) with a **golden test vector** cross-checked in C++ and C#, exactly as was done for v1.

### 5.2 Status packet (board → PC, 1 Hz, port 4211, type byte distinguishes it)

RSSI of the PC link, battery mV, free heap, overrun count, max cycle µs, uptime, config version/CRC, governor level.

### 5.3 Control channel (PC ↔ board, UDP 4212, request/response with id, retry ×3)

`GET_CAPS` (board model, firmware version, pin capability table, limits) · `GET_CFG` · `SET_CFG` · `COMMIT` · `CONFIRM` · `REVERT` · `PIN_PROBE(pin, ms)` (stream live level/ADC so the UI can verify wiring) · `FACTORY_RESET` · `REBOOT` (replaces `RSTX`) · `LED_TEST`.

`GET_CAPS` is the key: **the app never hard-codes pins** — it builds its pin pickers from what the board reports, so other boards (S3, C3) just report a different table.

### 5.4 Downstream telemetry (PC → board, optional, phase F5)

RPM, gear, flags, speed @ ~30–60 Hz for LED strip / display modules.

---

## 6. Feature detail per module (what the firmware provides)

| Module | On the board | Left to the PC |
|---|---|---|
| Steering | trimmed-mean oversampling (exists), mV mode (exists), spike rejection | range, centre, curve, deadzone, smoothing, rotation |
| Throttle / Brake / Clutch / Handbrake | oversampling, optional hysteresis around the last value (kills last-bit flicker), analog or digital-threshold source | calibration, deadzones, curve, ramp |
| Shifter | debounce, optional lockout between up/down | pulse length, H-pattern logic, repeat |
| Horn / buttons | debounce per button, 32-bit bitmask | press mode, toggle/long-press/layers, actions |
| Encoders | PCNT hardware counting, glitch filter, deltas | step mapping, button/axis mode, repeat |

### 6.1 Timing budget (200 pps = 5 ms/cycle)

8 axes × 16 samples of `analogReadMilliVolts` ≈ 8 × 16 × ~20 µs ≈ 2.6 ms — fits, but only just. The existing **adaptive governor** (16/12/8 samples) is kept and extended to *per-axis* sample counts; ADS1115 channels are read round-robin one channel per cycle (860 SPS ≈ 1.2 ms/conversion), so each ADS axis updates at ≤ 50 Hz and is flagged as "slow axis" in `GET_CAPS`.

### 6.2 Module-table runtime

`inputs_read()` becomes a loop over a small fixed array of `Channel {type, pin, params}` — no heap allocation after boot. Host tests keep the pure logic (debounce, trimmed mean, governor, **config validator**, **packet v2 builder/parser**) in header-only files, as today.

### 6.3 Security

Config/control commands are HMAC-SHA256 signed with a **per-device random
32-byte key** (board pehle boot par hardware RNG se banata hai, encrypted
NVS — `docs/KEY-LIFECYCLE.md` §2, `docs/PROTOCOL-V2.1.md` §4.1; purani
`PROTOCOL-V2.md` wali "key = SHA-256(OTA password)" derivation **superseded**
hai, KEY-LIFECYCLE OPEN-1 — use kabhi implement mat karo). Only stations on
the board's own AP can reach port 4212 anyway; the HMAC stops a second device
on the AP from rewriting the pin map. WebOTA: har endpoint par auth +
lockout, signed image verify-before-apply (Ed25519, `docs/SIGNED-OTA.md`).

**SEC-D-04 doc fix (Phase D):** is section ki purani line ("key derived
from the OTA password") ghalat thi — OTA password se key derive karna
KEY-LIFECYCLE OPEN-1 ke mutabiq superseded hai. Upar wali line hi sahi hai.

---

## 7. Phases

| Phase | Scope | Exit criteria |
|---|---|---|
| **F0 — Contract (DONE)** | `PROTOCOL-V2.md`, config schema, board pin-table JSON, reference encoder + golden vectors, reference pin validator (14 checks pass); no firmware change | Frozen by decision (owner delegated approval) |
| **F1 — Config engine** | NVS storage (A/B + CRC), validator, factory defaults = v3.3 map, module table driving `inputs_read()`, packet v2 (v1 still works) | Host tests: validator accepts/rejects every rule in 3.1; bench: same behaviour as v3.3 with default config |
| **F2 — Control channel** | UDP 4212 commands, HMAC, `GET_CAPS`, stage/commit/confirm/rollback, boot-loop guard, safe mode | Bench: deliberately bad pin map ⇒ board auto-rolls back; BOOT-hold ⇒ factory map |
| **F3 — Status + extra axes** | Status packet (RSSI, battery, heap…), clutch + handbrake axes, 32 buttons, hysteresis option, `PIN_PROBE` | Dashboard values real (incl. battery if present); wiring test works from the app; perf checklist |
| **F4 — Encoders + I²C** | PCNT encoders, ADS1115, PCF8574, MCP23017 | Rotary detents counted without loss at hand speed; expander buttons reach the PC; perf checklist |
| **F5 — LED/display** | WS2812 strip, downstream telemetry | RPM bar + flags from game data; perf checklist (LED refresh must not move `maxCycleUs`) |
| **F6 — Release** | OTA from v3.3 → v4.0 keeps the old pin map, full soak (≥ 4 h @ 200 pps, no overrun growth), docs, merged `.bin` | Checklist in section 9 |

Dependency: **F1 + F2 unblock the software Settings → pins UI** (software plan S3). F0 must be finished before either side starts coding.

---

## 8. Testing plan

- **Host (no board):** extend `tests-host/` — pin validator (every forbidden/strapping/ADC2 case), config blob round-trip + CRC corruption, A/B slot selection, packet v1/v2 golden vectors, debounce/governor as today. Target: all green before any flash.
- **Python cross-check:** `verify_packet.py` extended to v2 and to the control-channel frame format.
- **Compile:** `pio run` for `esp32dev`, flash/RAM % recorded (v3.3 baseline: 71.4 % flash, 14.5 % RAM — dual-partition limits growth, so track it per phase).
- **Bench (owner's board):** timing (max cycle µs, overruns) with 3/5/8 axes; ADC noise with Wi-Fi on; rollback scenarios; OTA upgrade from v3.3; 4-hour soak.

## 9. Release checklist

- [ ] All host tests green; golden vectors identical across C++/Python/C#
- [ ] `pio run` clean, flash < 85 %
- [ ] Upgrade from v3.3 keeps working wiring with no user action
- [ ] Bad config always recovers (rollback, boot-loop guard, BOOT-hold)
- [ ] Passwords/SSID still in `config.h` + documented how to change them
- [ ] `FLASH-GUIDE.md`, `PROTOCOL-V2.md`, `HANDOVER.md` updated

## 10. Risks

| Risk | Mitigation |
|---|---|
| User picks a pin that stops boot | Validator + confirm-or-rollback + safe mode |
| 8 axes blow the 5 ms budget | Per-axis governor, ADS round-robin, measure on the real board before promising 8 |
| Flash gets tight with dual OTA partitions | Track size per phase; drop Arduino-OTA before Web-OTA if needed |
| Extra UDP port / commands increase attack surface | HMAC, AP-only, strict length checks (fuzz the parser in host tests) |
| Packet v2 breaks the old app | Version negotiation in `HELO`; v1 path stays tested |

## 11. Decisions

**All decided** (owner delegated the remaining choices): everything on **one ESP32**; modules optional and user-chosen; power source irrelevant, battery monitor optional; runtime GPIO assignment with pin-budget helper; all features in scope; "race" = throttle; 10 s rollback confirm; SSID/OTA names renamed in Step 0 (SSID `RSH`, hostname `rsh`); **protocol v2 frozen as drafted**; LED strip default WS2812 ≤ 60 LEDs; full-rig preset = 15 buttons + 2 encoders (matrix or MCP23017 once direct pins run out); performance protected by construction (§0), no measured baseline required.

Nothing is waiting on the owner. **No firmware code is written until the owner says "start F1".**
