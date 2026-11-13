# RSH — Pin Rules (ESP32 DevKit V1)

**Board:** ESP32 DevKit V1 (ESP32-WROOM-32). **Scope:** FW-1 (WP1/WP2), Plan v2.3.
**Authority:** `docs/board-esp32-devkitv1.json` (machine-readable table — the app builds
its pin pickers from it via `GET_CAPS`, never hard-codes pins) and the reference
validator `tests-host/pin_rules_ref.py`. Firmware (C++) and app (C#) must match
both. **Status:** implementation-complete; verification PENDING — Virtual Test Rig.

Sources: Firmware Plan §3, Plan v2.3 §5, Decision Log D6 (bounds), D34 (AS5600),
D35 (full-rig preset), R-L9 (LED test rule).

## 1. Pin budget — what fits on one board

After removing flash (6–11), UART0 (1, 3), strapping/avoid (0, 5, 12, 15) and the
status LED (2):

| Group | Pins | Count |
|---|---|---|
| Analog-capable (ADC1) | 32, 33, 34, 35, 36, 39 | **6** (34/35/36/39 input-only, no pull-up) |
| Digital, pull-up capable | 4, 13, 14, 16, 17, 18, 19, 23, 25, 26, 27 | **11** |
| I²C (only if expanders used) | 21 (SDA), 22 (SCL) | 2 |
| **Total usable** | | **19** (+ GPIO5 with caution) |

Rules that shape the budget:

- **Analog axis ⇒ ADC1 only** (32, 33, 34, 35, 36, 39). ADC2 pins return garbage
  while Wi-Fi is on.
- **Digital button/encoder ⇒ pull-up capable pins only**, or input-only pins with
  the `ext_pullup` flag set by the user (external resistor fitted).
- **No pin used twice.** I²C pins (21, 22) are reserved when any expander is enabled.
- **17/19 fit only if GPIO21 is used as a digital pin** (no expanders) — see §5.

## 2. Pin table — every GPIO

`type` and `allowed` are the JSON fields; `class` drives the validator.

| GPIO | Type | ADC (Wi-Fi on) | Pull-up | Out | Class | Allowed kinds | Notes |
|---|---|---|---|---|---|---|---|
| 0 | strapping | — (ADC2) | yes | yes | avoid | — | Boot mode; rejected |
| 1 | UART | — | — | — | forbidden | — | UART0 TX |
| 2 | reserved | — (ADC2) | yes | yes | reserved | — | Status LED + strapping |
| 3 | UART | — | — | — | forbidden | — | UART0 RX |
| 4 | digital | — (ADC2) | yes | yes | ok | button, matrix, encoder, led | |
| 5 | strapping | — | yes | yes | caution | button, matrix, encoder, led | Boot log; allowed, app must warn |
| 6–11 | forbidden | — | — | — | forbidden | — | Connected to flash |
| 12 | strapping | — (ADC2) | yes | yes | avoid | — | Must be LOW at boot (flash voltage); rejected |
| 13, 14 | digital | — (ADC2) | yes | yes | ok | button, matrix, encoder, led | |
| 15 | strapping | — (ADC2) | yes | yes | avoid | — | Boot log; rejected |
| 16, 17 | digital | — | yes | yes | ok | button, matrix, encoder, led | PSRAM on WROVER; free on WROOM-32 |
| 18, 19, 23 | digital | — | yes | yes | ok | button, matrix, encoder, led | SPI pins — ok as digital |
| 20 | nonexistent | — | — | — | forbidden | — | No such GPIO on ESP32 |
| 21 | digital+I²C | — | yes | yes | ok | button, matrix, encoder, led | Default I²C SDA; digital only with no expanders (R-L9) |
| 22 | digital+I²C | — | yes | yes | ok | button, matrix, encoder, led | Default I²C SCL; digital only with no expanders |
| 24 | nonexistent | — | — | — | forbidden | — | No such GPIO on ESP32 |
| 25, 26, 27 | digital | — (ADC2) | yes | yes | ok | button, matrix, encoder, led | |
| 28–31 | nonexistent | — | — | — | forbidden | — | No such GPIO on ESP32 |
| 32 | analog+digital | **ADC1_CH4** | yes | yes | ok | axis, battery, button, matrix, encoder, led | Best analog pin + digital |
| 33 | analog+digital | **ADC1_CH5** | yes | yes | ok | axis, battery, button, matrix, encoder, led | Best analog pin + digital |
| 34 | input-only analog | **ADC1_CH6** | **no** | **no** | ok | axis, battery, button*, matrix*, encoder* | *digital use needs external pull-up |
| 35 | input-only analog | **ADC1_CH7** | **no** | **no** | ok | axis, battery, button*, matrix*, encoder* | *digital use needs external pull-up |
| 36 | input-only analog | **ADC1_CH0** | **no** | **no** | ok | axis, battery, button*, matrix*, encoder* | *digital use needs external pull-up |
| 37, 38 | not-broken-out | ADC1_CH1/CH2 on die | — | — | forbidden | — | Exist on the ESP32 die, not broken out on WROOM-32 / DevKit V1 |
| 39 | input-only analog | **ADC1_CH3** | **no** | **no** | ok | axis, battery, button*, matrix*, encoder* | *digital use needs external pull-up |

## 3. Pin classes — what the validator does

| Class | Validator | Meaning |
|---|---|---|
| `ok` | allowed (subject to kind rules) | Safe general-purpose pin |
| `caution` | allowed; app must warn | Usable, with a boot-time side effect (GPIO5: boot log) |
| `avoid` | **rejected** (`avoided ...`) | Strapping pin — use could stop boot (0, 12, 15) |
| `reserved` | **rejected** (`already used by status LED`) | GPIO2, the onboard status LED |
| `forbidden` | **rejected** (`forbidden ...`) | Flash (6–11), UART0 (1, 3), non-existent (20, 24, 28–31), not broken out (37, 38) |

`free_pins()` counts only `ok` pins, so GPIO5 (caution) is never reported as free —
the 19-pin budget above is conservative by design. `free_pins()` returns pins
sorted numerically (JSON key order is not numeric).

## 4. Module kinds and limits (T7)

| Kind | GPIO pins | Counts toward | Limit | Why (reason for the limit) |
|---|---|---|---|---|
| `axis` | 1 (ADC1) | axes | **≤ 8** (D6) | Protocol v2.1 axis mask (u8, FW-2); 8 axes × oversampling must fit the 5 ms tick (governor) |
| `axis_ads1115` | 0 (I²C, ch 0–3) | axes | **≤ 8** (D6) | Same axes budget; ADS channels are read round-robin, ≤ 50 Hz each (slow-axis flag in `GET_CAPS`) |
| `button` | 1 (pull-up, or ext pull-up) | buttons | **≤ 128** (D6) | D6 total bound; direct pins only allow 11 — 128 is reachable only via matrix/expanders |
| `button_matrix` | rows+cols (default 4×4 = 8 pins) | buttons (rows×cols) | **≤ 128** (D6) | Same buttons budget; 4×4 = 16 buttons on 8 pins |
| `encoder` | 2 (A, B; pull-up) | encoders | **≤ 4** (D6) | D6 protocol bound (FW-2); encoder push is a separate `button` module |
| `led` | 1 (digital-out) | led_strips | **≤ 1 strip, ≤ 60 LEDs** | Firmware Plan §3.1 / §11: one WS2812 strip, default ≤ 60 LEDs |
| `battery` | 1 (ADC1) | batteries | **≤ 1** | One supply per board; **a battery is NOT an axis** (T7 fix — see §6) |
| `expander_button` | 0 (I²C) | buttons | **≤ 128** (D6) | Same buttons budget; MCP23017 pin 0–15, PCF8574 pin 0–7, per (expander, addr, pin) |

Notes:

- **Axes total** = `axis` + `axis_ads1115` (direct ADC1 axes are pin-limited to 6).
- **Buttons total** = `button` + `button_matrix` (rows×cols each) + `expander_button`.
- **I²C kinds** (`axis_ads1115`, `expander_button`) require `expanders=True`, which
  reserves GPIO21/22 — a direct-pin module on 21/22 is then rejected.
- **AS5600** (`axis_as5600`, D34) is **not yet a validator kind**: it is only valid
  for ≤ 360° rotation; 900° needs a turn counter (index pulse or software turn
  tracking), otherwise use `ENCODER_STEERING`. It will be added as an I²C axis kind
  when FW-12 lands. Unknown kinds are rejected with `unknown kind`, never crash.
- `AXIS_GENERIC` (Firmware Plan §3.2) is likewise future — use `axis` for any extra
  pot/hall/slider until then.

## 5. Presets

Both presets live in `board-esp32-devkitv1.json` (`presets`) and are validated by
`pin_rules_ref.py` from that JSON (single source of truth). Modules: steering,
throttle, brake, clutch, handbrake, shifter up/down, horn, buttons, encoders, LED.

### 5.1 Full rig — direct pins (no expanders)

| Module | Pins |
|---|---|
| Steering, throttle, brake, clutch, handbrake (5 axes) | 32, 33, 34, 35, 36 |
| Battery | 39 |
| Shift up, shift down, horn + 4 buttons (7 buttons) | 4, 13, 14, 16, 17, 18, 19 |
| Encoder 1, encoder 2 | (23, 25), (26, 27) |
| LED strip | **21** |
| **Total** | **18 pins used, 1 free (GPIO22)** |

### 5.2 Full rig — 15 buttons via MCP23017 (I²C)

| Module | Pins |
|---|---|
| Steering, throttle, brake, clutch, handbrake (5 axes) | 32, 33, 34, 35, 36 |
| Battery | 39 |
| 15 buttons (MCP23017, addr 0x20, pins 0–14) | — (I²C: 21 SDA, 22 SCL) |
| Encoder 1, encoder 2 | (23, 25), (26, 27) |
| LED strip | **4** (freed because buttons moved to the expander) |
| **Total** | **11 direct pins used, 6 free (13, 14, 16, 17, 18, 19)** |

The old plan's 15 direct buttons do not fit (D35: only 11 digital pull-up pins) —
15 buttons need the MCP23017 preset (or a button matrix: 4×4 = 16 buttons on 8 pins).

### 5.3 LED test rule (R-L9)

In the direct full rig the LED fits **only on GPIO21** (every other digital pin is
taken; GPIO22 is the one spare pin). So the LED test fails unless GPIO21 is used
as a digital pin — i.e. no expanders, LED on 21. With expanders, 21/22 are I²C
and the LED must move to a freed digital pin (GPIO4 in the MCP23017 preset).
A 3rd encoder does not fit in the direct full rig + LED either (only GPIO22 is
free; an encoder needs 2 pins).

## 6. Validator rules (`validate(modules, expanders=False)`)

1. At least one input module is required.
2. GPIO2 is always reserved (status LED); GPIO21/22 are reserved when
   `expanders=True`.
3. Forbidden / avoid pins are rejected with the reason from the JSON table.
4. No pin is used twice (`already used by ...`).
5. `axis` / `battery` need an ADC1 pin; `button` / `encoder` / `button_matrix`
   need a pull-up pin or `ext_pullup=True`; `led` needs a digital-out pin.
6. Shape checks: `axis`/`battery`/`button`/`led` need exactly 1 pin;
   `encoder` exactly 2 pins (A, B); `button_matrix` exactly rows+cols pins
   (rows/cols 1–8, default 4×4); I²C kinds need 0 pins.
7. ADS1115 channels are 0–3 and unique per (addr, channel); expander pins are
   unique per (expander, addr, pin) and range-checked (MCP23017 0–15, PCF8574 0–7).
8. Count limits (§4) are enforced at the end, with the actual count in the message
   (e.g. `too many axes (9 > 8)`).
9. **Battery fix (T7):** a battery module increments the *battery* count only.
   The old line `n[kind if kind in n else "axis" if kind == "battery" else kind]
   += 1 if kind in n else 0` counted a battery as 0 axes through an unreadable
   expression; it is replaced by an explicit per-kind counter. Proof test: 8 axes
   (5 direct + 3 ADS1115) + battery is valid — if the battery counted as an axis
   it would be 9 and rejected.
10. Unknown kinds are rejected (`unknown kind '...'`), not a crash (the old code
    raised `KeyError` for any kind outside the counter dict).

## 7. Tests (`tests-host/pin_rules_ref.py`)

Run **in the Virtual Test Rig only**: `python3 -I tests-host/pin_rules_ref.py`.
14 original S0/F0 checks (default map, ADC2/flash/strapping/UART rejection,
duplicate pin, ext pull-up, status LED, I²C reservation, empty rig, LED on
input-only pin, full rig valid) plus FW-1 additions:

- **Presets:** both JSON presets valid; exact free-pin sets (`[22]` direct,
  `[13, 14, 16, 17, 18, 19]` MCP23017).
- **T7 limits:** 9 axes rejected / 8 axes valid; 5 encoders rejected / 4 valid;
  129 buttons rejected / 128 valid; matrix counts (16+113 rejected, 16+112 valid);
  battery not counted as an axis; 2 batteries rejected; 2 LED strips rejected;
  LED count 60 valid / 61 rejected; ADS1115 channel range + duplicate + no-I²C
  cases; expander pin range + duplicate cases; encoder pin-count cases; unknown
  kind cases.
- **T8 fixes:** vacuous `len(free_pins(full)) >= 0` replaced by exact free-pin
  assertions (`[21, 22]` after the full rig, `[]` with I²C reserved, `[22]` after
  full rig + LED on 21); dead expression `not (p in (21, 22) and False)` removed
  from `free_pins()` (the `used` set already reserves 21/22 with expanders);
  shared mutated `full` list replaced by a `make_full_rig()` factory — every test
  gets a fresh list.
- **R-L9 tests:** full rig + LED on 21 valid; LED on a used pin rejected; LED on
  21 with expanders rejected; 3rd encoder in full rig + LED rejected.

**Note on "3-encoder reject" (FW-1 text):** the D6 / FW-2 bound is nEnc ≤ 4, so a
standalone 3-encoder config is **valid** (tested). Only the 3rd encoder *inside
the direct full rig + LED* is rejected — pin budget, not the count limit (see
§5.3). If the plan intends a lower encoder bound, that is a D6 change, not a
validator fix.

## 8. Verification status

**Implementation-complete, verification PENDING (Virtual Test Rig).** Code and tests
were written and re-read (logic check, §6–§7), but no test run was done outside the
rig per the standing Virtual Test Rig rule — `pin_rules_ref.py` and `run_tests.sh`
must be executed inside `~/workspace/projects/rsh/test-env/` once the rig is ready.
