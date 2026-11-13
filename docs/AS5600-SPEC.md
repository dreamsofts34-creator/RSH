# RSH — AS5600 Magnetic Encoder: Spec + Firmware Integration Plan (Phase G Workstream 4)

**Status: SPEC + GROUNDWORK — Phase G. Datasheet-based spec (§1–§4),
firmware integration plan (§5), aur sandbox-verified groundwork (§6) is doc
mein hain. Board par wiring + real-wheel verify Phase H A7 ko handoff hai
(§7).**
**Sources:** ams OSRAM AS5600 datasheet v1-06 (2018-Jun-20) — register map
§1, STATUS/AGC/MAGNITUDE §2; Plan v2.3 D34 (§0), `docs/PIN-RULES.md`,
`docs/PROTOCOL-V2.1.md` §1.1/§3.4.1/§3.6, `firmware/rsh/encoders.h`
(quadrature pattern), `firmware/rsh/config_transaction.h` (blob types).
**D34 (authority):** `AXIS_AS5600` sirf **≤ 360°** rotation ke liye; 900° ke
liye **turn counter** lazmi hai, warna `ENCODER_STEERING` use karo.

## 1. Datasheet — chip kya hai

- **AS5600** (ams OSRAM): 12-bit programmable contactless magnetic rotary
  position sensor. Diametrically magnetized magnet chip ke oopar on-axis
  ghumta hai; on-chip Hall array + CORDIC arctangent se absolute angle
  nikalta hai. Koi physical contact nahi — pot ki tarah wear nahi hota.
- **Resolution:** 12-bit = **4096 positions/rev, 0.0879°/LSB**.
  Muqabla: RSH pot + 12-bit ADC ≈ 0.22°/step (900° par) — AS5600
  **~2.5× finer** hai, aur absolute hai (boot par position maloom hoti hai,
  quadrature encoder ki tarah homing nahi chahiye).
- **Supply:** 3.3 V ya 5 V (RSH: 3.3 V, ESP32 ke saath). Package SOIC-8.
- **Interface:** I²C (up to 1 MHz Fast-mode Plus), PWM, ya analog OUT.
  **RSH I²C istemal karega** — analog OUT pot jaisa hi hota (koi faida
  nahi), PWM ko capture karna loop mein mehenga hai.
- **I²C address: `0x36` fixed, change nahi hota.** Nateeja: **ek I²C bus
  par sirf ek AS5600** ho sakta hai — do AS5600 (masalan steering +
  handbrake) ke liye I²C mux chahiye, jo v2 mein nahi hai. Is liye
  **v2 mein max 1 AS5600 per board** (steering ke liye).
- **Output rate:** 12-bit par CORDIC output ≈ **1 kHz** max (vendor docs).
  RSH 200 Hz loop mein parhega — rate kabhi had nahi banti.
- **Max speed:** ~1000 RPM at 12-bit (vendor docs) — steering wheel ke liye
  beshumar zyada hai.
- **Programmable range:** ZPOS/MPOS se 18°–360° tak range set ho sakti hai
  (OTP burn se). **RSH burn nahi karega** (§3) — poora 0–360° raw parhega
  aur calibration PC side hogi (D10 model jaisa).

## 2. Register map (I²C, addr 0x36)

| Addr | Name | R/W | Matlab |
|---|---|---|---|
| 0x00 | ZMCO | R | Burn counter (OTP kitni dafa burn hua, max 3) |
| 0x01–0x02 | ZPOS | R/W | Zero/start position (12-bit) |
| 0x03–0x04 | MPOS | R/W | Maximum/stop position (12-bit) |
| 0x05–0x06 | MANG | R/W | Maximum angle (12-bit) |
| 0x07–0x08 | CONF | R/W | Config (output mode, hysteresis, power mode, slow filter, PWM freq, watchdog) |
| 0x0B | STATUS | R | **MD (bit5)** magnet detected; **ML (bit4)** magnet too weak (AGC max overflow); **MH (bit3)** magnet too strong (AGC min overflow) |
| 0x0C–0x0D | RAWANGLE | R | **Raw angle, 12-bit (0–4095)** — CORDIC seedha, koi scaling nahi. **RSH yehi parhega.** |
| 0x0E–0x0F | ANGLE | R | Angle ZPOS/MPOS/MANG + hysteresis ke baad — RSH istemal **nahi** karega (scaling PC-side calibration mein hai) |
| 0x1A | AGC | R | Automatic gain control (0–255 @5 V, **0–128 @3.3 V**); behtareen performance mid-range par |
| 0x1B–0x1C | MAGNITUDE | R | CORDIC magnitude (12-bit) — diagnostics |
| 0xFF | BURN | W | OTP burn command — **RSH kabhi nahi bhejega** (irreversible; max 3 burns) |

**Magnet placement (datasheet guidance):** diametric magnet, chip ke
center par on-axis, air gap aisa ke AGC mid-range mein rahe. 3.3 V par
AGC **~64 ideal** hai (range 0–128); AGC neeche (~0) = magnet too strong
(MH), oopar (~128) = too weak (ML). Gap aam tor par **0.5–3 mm** se tune
hota hai — exact gap magnet par munhasir hai, A7 par AGC dekh kar set
hoga. **STATUS check lazmi hai:** MD=0 ho to angle par bharosa nahi —
axis-invalid (§4).

## 3. Angle → stream value (math)

- Raw `r` (0–4095) = angle `r × 360 / 4096` degrees (0–359.912°).
- **Stream mein axis mV mein jata hai (D6 — sab axes mV):**
  `mV = r × 3300 / 4095` (0–3300 mV). PC side mapper isay baqi axes
  jaisa hi treat karta hai — **PC code mein koi tabdeeli nahi** (calibration
  D10: span ≥ 660 mV, `IsMillivolts` tag — AS5600 axis par bhi laagu).
- **Resolution:** 3300/4095 ≈ **0.81 mV/LSB** — ADC pot (3300/4095 bhi,
  lekin noise zyada) se behtar SNR, kyun ke magnetic reading mein pot
  jaisa wiper noise nahi hota.
- **Turn counter (D34 — 900° steering):** raw angle har 360° par wrap
  hota hai. Firmware wrap-aware delta nikaalega (i12 serial arithmetic,
  `encoders.h` ke i16 delta jaisa): `delta = (int16_t)((new - old) << 4)
  >> 4` (12-bit wrap), phir `turns += delta`, total = `turns` (i32
  cumulative, 0.088°/step). **Boot par turns = 0** — power loss se turn
  count bhool jata hai (absolute sirf ek turn ke andar hai). Is liye
  multi-turn mode mein boot par wheel **center** hona chahiye (USER-GUIDE
  mein likha jayega) ya calibration se center set hoga. ≤360° mode mein
  turn counter ki zaroorat nahi — raw angle hi kaafi hai.
- **I²C cost (loop budget, F11/D15):** 400 kHz par 2-byte read ≈ 70 µs
  (address + register + 2 data bytes + ACKs); STATUS+RAWANGLE (3 bytes,
  ek burst mein 0x0B se nahi — registers contiguous nahi, is liye 2
  transactions ≈ 120 µs). 3500 µs maxCycle budget ke andar aaram se hai.
  ADS1115 axes (≤50 Hz, slow-axis flag) se farq: AS5600 **200 Hz** par
  parha jayega — slow-axis flag nahi lagta.

## 4. Fault handling (F2/D12 ke saath)

| Haalat | Pehchan | Amal |
|---|---|---|
| Magnet ghayab / galat gap | STATUS MD=0 | Axis-invalid (flag bit3, §1.3) + diagnostics counter; angle 0 nahi bhejo — aakhri valid value + invalid flag |
| Magnet too weak / too strong | ML=1 / MH=1 | Warning (diagnostics, USER-GUIDE mein gap adjust karne ki hidayat); angle phir bhi bhejo (degraded accuracy) |
| I²C NACK / timeout | Read fail | Axis-invalid + `txFail`-jaisa counter (status packet mein nahi — diagnostics); 3 lagatar fail → axis ko stream se hatao (mask bit clear) jab tak dobara na mile |
| Device ghayab (boot par) | Probe par NACK | AS5600 axis disabled, error GET_CAPS/Diagnostics mein; baqi board normal chale |
| Wrap glitch (ek tick mein bada jump) | \|delta\| > 2048 (180°) ek 5 ms tick mein | Glitch mano, count na badhao (quadrature `encoders.h` ka invalid-transition rule jaisa) |

## 5. Firmware integration plan (Phase H, board par)

1. **Config blob type 12** — `RSH_CFG_TYPE_AXIS_AS5600 = 12`
   (`config_transaction.h`, Phase G mein add ho gaya — §6). Record layout
   (16 B, baqi types jaisa): `type=12, slot=<axis slot>, flags:
   bit0 = multi-turn (0 = ≤360° direct, 1 = turn counter on),
   pinA/pinB = 0xFF (koi GPIO nahi — I²C bus hai), params[0] = 0
   (reserved)`. Validator: type 12 sirf tab valid jab I²C on ho
   (expanders=True — GPIO21/22 reserved) aur board par **ek se zyada**
   AS5600 record na ho (addr fixed 0x36).
2. **GET_CAPS kind 12** — `AXIS_AS5600 = 12` (`PROTOCOL-V2.1.md` §3.4.1,
   Phase G mein add ho gaya — §6). Pin table entry: `pin = 21` (SDA,
   bus ki numaindagi), `kind = 12`, `module = STEERING` (ya jis module
   ko assign ho), `caps = I2C` (bit pattern: kind se pata chalta hai,
   alag bit nahi).
3. **Pin validator kind `axis_as5600`** (`pin_rules_ref.py`, Phase G mein
   add ho gaya — §6): I²C kind hai — koi GPIO pin nahi, `expanders=True`
   lazmi, axes limit (≤8) mein gina jata hai, max 1 per board.
4. **Read path** (`firmware/rsh/inputs.cpp`, Phase H): `as5600.h` ka
   `rsh_as5600_*` API `Wire` (I²C) backend se joro — `Wire.begin(21, 22)`,
   400 kHz; axis read mein jab slot ka type AS5600 ho to `rsh_as5600_read_mv()`
   se mV lo (ADC read ki jagah); F7 on/off rule yahin laagu (band module
   ka I²C read nahi hota).
5. **Stream:** koi protocol change nahi — AS5600 axis aam mV axis hai
   (axisMask bit + u16 mV). Multi-turn mode mein PC ko total angle mV
   range se bahar ja sakta hai — **nahi:** multi-turn total ko PC-side
   steering scale (D7 divide) se pehle **wrap/normalize** karo: firmware
   total angle ko configured rotation (900°) par map karke 0–3300 mV
   bhejega (turns × 360 + raw → degrees → mV = deg × 3300 / rotation).
   Rotation config blob `params[1]` mein (0 = 360°, 1 = 540°, 2 = 900°;
   default 360°). Ye mapping `as5600.h` mein hai
   (`rsh_as5600_steps_to_mv`, steps mein — 1 step = 360/4096°).
6. **PC side:** koi change nahi (axis mV hai; calibration + per-module
   tuning aam raasta). Diagnostics mein AS5600 status (MD/ML/MH/AGC)
   dikhana Phase H UI ka chhota item hai (GET_CAPS kind 12 se pata
   chalega ke axis AS5600 hai).

## 6. Sandbox mein verify hua (Phase G — is workstream ka kaam)

- **`firmware/rsh/as5600.h`** (naya, Arduino-free — `encoders.h` jaisa
  pattern; I²C backend caller-supplied callbacks se aata hai, taake host
  par mock ho sake): register map + STATUS decode (`rsh_as5600_status_decode`),
  RAWANGLE→mV (`rsh_as5600_raw_to_mv`), degrees→mV rotation mapping
  (`rsh_as5600_degrees_to_mv`), turn tracking unwrap
  (`rsh_as5600_turn_update`, i12 wrap-aware, glitch reject), fault flags,
  mock I²C se full read path (`rsh_as5600_read`).
- **Host tests** (`tests-host/test_main.cpp` §24, 30+ checks): raw→mV
  boundaries (0 → 0, 4095 → 3300, 2048 → ~1650), STATUS bits (MD/ML/MH),
  AGC mid-range check, turn unwrap (359° → 1° = +2°, ulta −2°),
  3-turn accumulate (1080°), glitch reject (>180° jump), magnet-ghayab
  par invalid, mock I²C read path (STATUS + RAWANGLE + AGC),
  multi-turn 900° mapping (450° → 1650 mV), config blob type 12 validate
  + axis count.
- **`config_transaction.h`:** `RSH_CFG_TYPE_AXIS_AS5600 = 12` + blob
  validation mein axis count (type 12 axes mein gina jata hai).
- **`PROTOCOL-V2.1.md` §3.4.1:** kind `12 AXIS_AS5600` darj.
- **`PIN-RULES.md` + `pin_rules_ref.py`:** `axis_as5600` ab validator kind
  hai (I²C, expanders lazmi, max 1/board, axes limit mein); purana
  "not yet a validator kind" test naye tests se badal gaya.
- **Full rig green:** `tests-host/run_tests.sh` (C++ host tests + Python
  pin rules + protocol vectors + packet cross-verify + merge_bin mock) —
  run §7 dekho.

## 7. Phase H A7 handoff (sirf board par verify ho sakta hai)

- [ ] H-AS-1: Asal AS5600 module + diametric magnet, ESP32 DevKit V1
  (SDA GPIO21, SCL GPIO22, 3.3 V, 4.7 kΩ pull-ups) — I²C scan par
  `0x36` mile.
- [ ] H-AS-2: Magnet gap tune karo — AGC mid-range (~64 @3.3 V), STATUS
  MD=1, ML=MH=0. Gap USER-GUIDE mein darj karo.
- [ ] H-AS-3: 200 Hz loop mein I²C read timing naapo (maxCycleUs <
  3500 µs, D15 — poori loop iteration).
- [ ] H-AS-4: Real wheel par 900° multi-turn: 3 turns aage + 3 wapas =
  wapas center (±1 LSB); boot par center behaviour check karo.
- [ ] H-AS-5: Magnet hatakar F2 invalid criteria check karo (axis-invalid
  flag + Diagnostics warning; stream mein stale value nahi).
- [ ] H-AS-6: ADS1115/expander ke saath I²C bus sharing check karo
  (AS5600 + ADS1115 ek hi bus par — address takrao nahi: 0x36 vs 0x48).
- [ ] H-AS-7: `inputs.cpp` wiring (§5 step 4) + GET_CAPS kind 12 live
  board par + PC app mein axis nazar aaye.
