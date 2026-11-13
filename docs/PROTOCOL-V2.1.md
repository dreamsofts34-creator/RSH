# RSH Protocol v2.1 — byte-level contract (WP3, Phase A)

Status: **CONTRACT — Phase A WP3 deliverable. Authority for all v2.1 wire
formats.** Plan v2.3 §1 ke faisle (D1–D6, D27–D30) yahan byte-level likhe gaye
hain. Jahan is doc aur purani `PROTOCOL-V2.md` (v2 draft) mein farq ho, **ye
doc jeet-ti hai**. Reference encoder + golden vectors:
`tests-host/protocol_v2_vectors.py` (run it). Pin rules:
`docs/board-esp32-devkitv1.json` + `tests-host/pin_rules_ref.py`. Key lifecycle:
`docs/KEY-LIFECYCLE.md`.

Versions: firmware **v4.0.0**, app **v2.0.0**, protocol **v2.1** (D27).
Product naam: **RSH**.

## 0. Conventions (sab packets/frames par laagu)

- Sab integers **little-endian**, jab tak explicitly big-endian na likha ho.
- **CRC16 = CRC-16/CCITT-FALSE**: poly `0x1021`, init `0xFFFF`, refin/out
  false, xorout `0x0000`. Check value: `CRC16("123456789") = 0x29B1`.
  CRC hamesha **packet/frame ke aakhir mein** hota hai aur CRC field se pehle
  ke **tamam bytes** par banta hai (D2). Koi packet/frame CRC ke baad extra
  bytes nahi rakhta.
- **UDP ports** (unchanged): PC→board `4210` (HELO), board→PC `4211`
  (stream, status, HELLO reply, PIN_PROBE frames), control dono taraf `4212`.
- **Reserved bytes hamesha 0** bhejta hai; receiver nonzero reserved par
  packet/frame **drop + count** karta hai (D6).
- **Bounds (D6):** nAxes ≤ 8, nBtn ≤ 128 (config ceiling), nEnc ≤ 4. Is se
  zyada claim karne wala stream frame **BAD_FRAME — reject (drop + count)**.
  Wire par buttons 0–63 aate hain (neeche §1.1/§1.2); nBtn 65–128 ki config
  SET_CFG par `UNSUPPORTED` se reject hoti hai (dekho §1.1 note N1).
- **mvMask khatam (D6):** sab axes millivolt (mV) mein hain. Koi per-axis
  unit flag nahi hai. Axis value u16 mV hai (0–3300 typical; ADC raw kabhi
  wire par nahi aata).
- **Version byte:** v2.1 wire version byte = `0x21` (major 2, minor 1, ek
  byte mein `major<<4 | minor`). Legacy v1 packet pehchan: pehla byte `0xA5`
  aur doosra byte `0x01` → legacy mode foran (R-H18). Path selection **board
  version** se hoti hai (HELLO reply §2 / GET_CAPS §3 se), profile version
  se nahi.
- **Upgrade order (R-M10):** pehle app, phir firmware. App v2 purane board
  se legacy mode mein chalti hai. OTA ke baad app GET_CAPS se firmware
  version verify karegi; version < 4.0.0 ho to v2 features disable rahenge.
  Ulta order support nahi.
- **No HELO for 2000 ms ⇒ board goes silent** (unchanged, v1 jaisa).

## 1. Board → PC stream

### 1.1 Data packet v2.1 (port 4211) — variable length

| Bytes | Field | Type | Notes |
|---|---|---|---|
| 0–3 | magic | `"RSH2"` | ASCII `0x52 0x53 0x48 0x32` |
| 4 | version | u8 | `0x21` (= v2.1). `!= 0x21` → drop + count |
| 5 | flags | u8 | §1.3 flags table (D4) |
| 6–7 | seq | u16 | Stream sequence, har packet +1, wrap `0xFFFF → 0` |
| 8–11 | timestamp | u32 | µs, **ADC read ke baad** liya gaya (F4/D13) |
| 12 | axisMask | u8 | bit *n* = axis *n* present, n = 0..7 |
| 13 | nEnc | u8 | Encoder count, 0..4. `> 4` → BAD_FRAME drop |
| 14–17 | buttons | u32 | bit *n* = button *n*, n = 0..31 (D1) |
| 18–19 | reserved | u16 | Hamesha `0`. Nonzero → drop + count (D6) |
| 20… | axes | u16 × popcount(axisMask) | Ascending axis index, **mV** |
| … | encoders | i16 × nEnc | **Cumulative** counts (D1/D6 note), per encoder |
| last 2 | CRC16 | u16 | §0 ke mutabiq |

Length = `20 + 2·popcount(axisMask) + 2·nEnc + 2`.
`popcount(axisMask) > 8` mumkin nahi (mask 8 bit hai). Axis indices **slots**
hain, roles (steering, throttle…) config se map hote hain — PC mapping
`GET_CFG`/`GET_CAPS` se seekhta hai, hard-coded positions se nahi.

**Encoders cumulative (D1):** har encoder ka value boot se cumulative count
hai (i16, wrap allowed — PC delta `new - old` ko i16 wrap ke saath nikale).
v2 draft ke i8 detents **khatam**.

**Note N1 — buttons 64–127 (D1 vs D6 tension, coordinator review):**
D1 kehta hai buttons 33+ status packet mein `btnsEx u32` se aate hain, aur D6
kehta hai nBtn ≤ 128. Ek u32 sirf 32 buttons cover karta hai, is liye v2.1
wire par **buttons 0–63** aate hain: stream `buttons` (0–31) + status
`btnsEx` (32–63, §1.2). Config mein nBtn 65–128 D6 ceiling ke andar hai
lekin us ka wire slot v2.1 mein nahi hai — SET_CFG aisi config ko
`UNSUPPORTED` (6) se reject karta hai. v2.2 mein doosra extension word
define ho sakta hai. Parser bound ke tor par nBtn > 128 hamesha BAD_FRAME
hai. Board JSON (`board-esp32-devkitv1.json`) ka `limits.buttons = 32` v3.3
pin map ki limit hai, wire limit nahi.

### 1.2 Status packet v2.1 (port 4211, 1 Hz) — 34 B base, 38 B extended

Base layout (nBtn ≤ 32, koi `btnsEx` nahi):

| Bytes | Field | Type | Notes |
|---|---|---|---|
| 0 | magic | u8 | `0xA6` (data packet se alag) |
| 1 | version | u8 | `0x21` |
| 2–3 | seq | u16 | Status sequence (data seq se alag counter) |
| 4–7 | uptime | u32 | ms since boot |
| 8 | rssi | i8 | dBm (PC link ka) |
| 9 | battery | u8 | %; `255` = no battery module |
| 10–11 | supply | u16 | mV; `0` = not measured |
| 12–15 | heap | u32 | Free heap bytes |
| 16–17 | overruns | u16 | Since boot |
| 18–19 | maxCycle | u16 | Max cycle µs (poori loop iteration, D15) |
| 20 | governor | u8 | Governor level |
| 21 | resetReason | u8 | 1 power-on, 2 brown-out, 3 watchdog, 4 software, baaqi reserved |
| 22–25 | cfgCrc | u32 | Config CRC32 |
| 26–27 | cfgVer | u16 | Config version |
| 28–29 | seqGap | u16 | **NEW (D5)** — boot se skip hue stream sequence numbers ki ginti (tick-skipped events). Wrap allowed |
| 30–31 | txFail | u16 | **NEW (D5/F3)** — boot se failed UDP transmissions ki ginti. Wrap allowed |
| 32–33 | CRC16 | u16 | §0 ke mutabiq, **aakhir mein** |

Total **34 B** (D5).

**Coordinator-review note (offsets):** Plan v2.3 D5 ke parenthetical offsets
(`seqGap (30–31) + txFail (32–33)`) CRC ko purani jagah (28–29) par rakhte
hain — us se CRC packet ke beech mein aa jata aur total 34 B ke saath CRC
"aakhir mein" na rehta. v2.1 CRC-last invariant (§0, D2) ko tarjeeh deta hai:
`seqGap` 28–29, `txFail` 30–31, CRC 32–33. Agar coordinator purane offsets
chahta hai to D5 ko 36 B karna hoga — faisla review mein confirm ho.

**Extended layout (nBtn 33–64):** jab board par 33–64 buttons configured
hain, status packet **38 B** ka hota hai — UDP datagram length se pehchana
jata hai (34 = base, 38 = extended; aur koi length valid nahi):

| Bytes | Field |
|---|---|
| 0–27 | Base fields (upar jaisi) |
| 28–31 | `btnsEx` u32 — bit *(n−32)* = button *n*, n = 32..63 (D1) |
| 32–33 | seqGap u16 |
| 34–35 | txFail u16 |
| 36–37 | CRC16 |

**Wi-Fi channel status mein nahi aata** — wo GET_CAPS reply mein hai (D5).

### 1.3 Flags byte (data packet byte 5) — D4

| Bit | Naam | Matlab (1 = true) |
|---|---|---|
| 0 | config-changed | Board ki config PC ke paas wali se badal gayi — PC `GET_CFG` kare |
| 1 | low-supply | Supply voltage low (threshold Phase D, F2/F10) |
| 2 | tick-skipped | Is packet se pehle ek ya zyada ticks skip hue (flash commit jaisi wajah) |
| 3 | axis-invalid | Koi axis invalid hai (F2 criteria, D12 — sirf §0.1 note dekho) |
| 4–7 | reserved | Hamesha `0` |

v2 draft ka bit0 "steering in mV" **khatam** — sab axes mV hain (D6).
Axis-invalid criteria (D12): invalid sirf tab jab (a) value 10 s tak exact
rail (0 ya 4095) par stuck ho **aur** doosre axes move kar rahe hon, ya (b)
variance threshold se zyada ho (floating noise). Sirf variance≈0 kabhi
invalid ki wajah nahi. PIN_PROBE se confirm karo.

### 1.4 Stream parser reject rules (PC side, T3)

Drop + count karo (counter alag-alag: `badMagic`, `badVersion`, `badCrc`,
`badFrame`, `rsvNonzero`):

- Pehle 4 bytes `!= "RSH2"` aur pehle 2 bytes `!= 0xA5 0x01` → `badMagic`.
- Version byte `!= 0x21` → `badVersion`.
- `nEnc > 4` ya datagram length != `20 + 2·axes + 2·nEnc + 2` → `badFrame`.
- Reserved bytes 18–19 `!= 0` → `rsvNonzero` (D6).
- CRC16 mismatch → `badCrc`.
- Flags bits 4–7 nonzero → drop + count (reserved violation).

## 2. HELLO / pairing (HELO + HELLO reply)

### 2.1 HELO request (PC → board, port 4210)

| Bytes | Field | Notes |
|---|---|---|
| 0–3 | `"HELO"` | ASCII |
| 4 | proto | u8: `0x01` = v1 app (legacy, koi reply nahi — v1 18 B packets); `0x02` = v2.x app |
| 5 | sessionFlag | u8, sirf proto `0x02` ke saath: `0` = resume (purana nonce wapas), `1` = naya session (naya nonce, counter 0 se). 5-byte HELO (sessionFlag ghair-maujood) = `0` mana jata hai (v2 draft compat) |

App routine HELO **1 Hz** bhejti hai (sessionFlag `0`); board har HELO ka
jawab HELLO reply se deta hai. 2000 ms tak koi HELO na aaye to board silent
ho jata hai.

### 2.2 HELLO reply (board → PC, port 4211, sender ke source port par) — 92 B fixed

| Bytes | Field | Type | Notes |
|---|---|---|---|
| 0–3 | magic | `"RSHR"` | ASCII `0x52 0x53 0x48 0x52` |
| 4 | protoMajor | u8 | `2` |
| 5 | protoMinor | u8 | `1` |
| 6 | flags | u8 | bit0 `unpaired` (1 = unpaired, key+password fields valid); bit1 `keyResetArmed`; bit2 `safeMode`; bit3 `pairingWindowOpen`; bit4–7 reserved `0` |
| 7 | sessionEcho | u8 | Request ka sessionFlag (`0`/`1`) |
| 8–13 | boardMac | 6 B | Board Wi-Fi MAC (device identity; **key/password is se derive kabhi nahi** — R-H10) |
| 14 | fwMajor | u8 | `4` |
| 15 | fwMinor | u8 | `0` |
| 16 | fwPatch | u8 | `0` |
| 17 | reserved | u8 | `0` |
| 18–33 | nonce | 16 B | §2.3 nonce rules |
| 34–65 | deviceKey | 32 B | **Sirf** unpaired + pairing window ke andar: per-device key (OPEN-2). Paired ho ya window band ho to **sab `0x00`** |
| 66–89 | wifiPassword | 24 B | **Sirf** unpaired + window ke andar: per-device Wi-Fi/OTA password, ASCII (§5.1). Paired/window-band ho to **sab `0x00`** |
| 90–91 | CRC16 | u16 | §0 ke mutabiq |

Total **92 B fixed** — parser length se validate kare; `!= 92` → drop.

### 2.3 Nonce / counter / session rules (D3, D30)

- **Nonce:** 16 bytes, hardware RNG se. **Sirf tab rotate** hota hai jab
  board reboot ho ya PC naya session maange (HELO sessionFlag `1`).
  Routine 1 Hz HELO (sessionFlag `0`) **purana nonce** wapas deta hai.
- **Counter:** u32, **per-session**. Naya nonce = counter `0` se shuru.
  Session ke andar har **naye** control request par counter +1 (retry par
  bhi naya counter — §3.2). Counter LE u32 ke tor par HMAC input mein jata
  hai.
- **Session khatam:** naya nonce milne par purana nonce/counter dobara
  istemal nahi hota. Purane nonce se signed frame → `BAD_TAG`.
- **Pairing window (OPEN-5 — CLOSED):** window **SoftAP up hone se 60 s**
  tak khulti hai. Window khatam hone ki nishani: HELLO reply flags bit3
  `pairingWindowOpen = 0` aur `deviceKey`/`wifiPassword` fields zero. Window
  ke baad dobara pairing ke liye KEY_RESET (§4.4) lazmi hai.

## 3. Control frames (port 4212, dono taraf)

### 3.1 Frame layout

| Bytes | Field | Type | Notes |
|---|---|---|---|
| 0–3 | magic | `"RSHC"` | ASCII `0x52 0x53 0x48 0x43` |
| 4 | cmd | u8 | §3.3 table; reply mein `reqCmd \| 0x80` |
| 5–6 | reqId | u16 | PC chunta hai; retry mein **wahi** reqId (D3) |
| 7–10 | counter | u32 | Per-session monotonic (§2.3, §3.2) |
| 11–12 | len | u16 | Payload length, 0..1024. `> 1024` → `TOO_BIG` |
| 13… | payload | bytes | `len` bytes |
| last 16 | tag | 16 B | HMAC-SHA256 (§3.1) ke pehle 16 bytes |

Total = `13 + len + 16`. **Max frame 1053 B.**

### 3.2 HMAC + duplicate / replay rules (D3)

- **Key:** per-device random **32 bytes** (OPEN-2 — CLOSED). Koi password
  hash nahi (OPEN-1 — dekho §6.1). Key board pehle boot par hardware RNG se
  banata hai, encrypted NVS mein rehti hai; app DPAPI se store karti hai
  (`docs/KEY-LIFECYCLE.md` §4).
- **Tag:** `tag = HMAC-SHA256(key, nonce ‖ counter_LE ‖ cmd ‖ payload)[0:16]`
  — nonce 16 B (HELLO reply se), counter u32 LE, cmd 1 B, payload raw.
  **Dhyaan:** frame header (magic/reqId/len) HMAC mein **nahi** hai; sirf
  nonce, counter, cmd, payload hain.
- **Reply tag:** wahi formula, reply ke `cmd` (req|0x80) aur reply payload
  ke saath, request wala counter echo karke. PC reply ka tag verify kare;
  fail ho to reply drop kare aur request ko retry kare (naya counter).
- **Retry:** PC ek request **3×** tak resend karta hai (250 ms interval),
  har retry mein **reqId wahi, counter naya**. Board har retry ko **nayi
  execution** samajhta hai — is liye tamam commands **idempotent** honi
  chahiye (SET_CFG chunk dobara likhna, COMMIT/CONFIRM dobara — sab safe).
- **Duplicate (exact retransmit):** board pichle **32** `(reqId, counter)`
  pairs (per session) cache rakhta hai. Wahi pair dobara aaye to board
  **cached reply dobara bhejta hai, dobara execute nahi karta**.
- **Replay:** naya reqId lekin counter ≤ session ka max-seen counter →
  reply status `REPLAY` (2). Board koi bhi counter gap accept karta hai
  (counter > max-seen theek hai — khoya packet session ko wedge nahi karta).
- **Malformed frame** (bad magic, `len` datagram se match na kare, truncated)
  → **bina reply ke drop** (reqId par bharosa nahi kiya ja sakta). Tag fail
  hone par (frame parse ho gaya ho) → reply status `BAD_TAG` (1) — reply
  mein bhi tag hota hai (request ke counter/cmd se bana hua nahi; board
  apna tag `BAD_TAG` reply par usi formula se banata hai, lekin PC is reply
  ka tag verify **nahi** kar sakta jab key mismatch ho — is liye PC `BAD_TAG`
  reply ko sirf tab maanta hai jab reqId/counter match karein).
- **Timeout:** PC 250 ms mein reply na aaye to retry; 3 retries ke baad
  command fail (UI ko dikhaye). GET_CAPS timeout **2 s** sirf legacy
  fallback ke liye hai (R-H18) — normal command timeout 250 ms × 3 hai.

### 3.3 Commands

| cmd | Name | Request payload → Reply payload (`status u8` pehle) |
|---|---|---|
| `0x01` | GET_CAPS | — → §3.4 GET_CAPS body |
| `0x02` | GET_CFG | `offset u16, wantLen u16 (≤1024)` → `status, totalLen u16, offset u16, data…` |
| `0x03` | SET_CFG | §3.5 chunk → `status, stagedBytes u16, totalExpected u16` |
| `0x04` | COMMIT | — → `status, confirmDeadlineSec u8 (= 30)`; §3.6 |
| `0x05` | CONFIRM | — → `status` |
| `0x06` | REVERT | — → `status` |
| `0x07` | PIN_PROBE | `pin u8, durationMs u16, mode u8` → `status, acceptedMs u16`; phir §3.8 stream frames |
| `0x08` | FACTORY_RESET | — → `status`; phir reboot (key+password **rehte** hain, §4.3) |
| `0x09` | REBOOT | — → `status`; phir reboot |
| `0x0A` | LED_TEST | §3.7 pattern → `status` |
| `0x0B` | KEY_RESET | — → `status`; sirf key-reset-armed state mein (§4.4, OPEN-10) |
| `0x0C` | SET_PASSWORD | `newLen u8 (8..63), password[newLen] ASCII` → `status`; §5.2 |

Unknown cmd → reply status `UNSUPPORTED` (6), cmd echo `req|0x80`.
Busy (flash commit chal raha ho, confirm window mein conflicting op) →
`BUSY` (7).

### 3.4 GET_CAPS reply body (status ke baad)

| Bytes (relative) | Field | Type | Notes |
|---|---|---|---|
| 0 | status | u8 | §3.9 |
| 1–6 | boardMac | 6 B | = HELLO reply boardMac |
| 7 | fwMajor | u8 | `4` |
| 8 | fwMinor | u8 | `0` |
| 9 | fwPatch | u8 | `0` |
| 10 | protoMajor | u8 | `2` |
| 11 | protoMinor | u8 | `1` |
| 12 | nAxesMax | u8 | `8` |
| 13 | nBtnMax | u8 | `64` (wire; config ceiling 128, §1.1 N1) |
| 14 | nEncMax | u8 | `4` |
| 15 | wifiChannel | u8 | Current SoftAP channel (1–13); `0` = unknown / AP nahi |
| 16 | capsFlags | u8 | bit0 `slowAxis` (ADS1115 axis maujood, ≤50 Hz); bit1 `safeMode`; bit2 `unpaired`; bit3 `keyResetArmed`; bit4–7 reserved `0` |
| 17 | pinCount | u8 | N — pin table entries (0..40) |
| 18… | pin table | 8 B × N | Har entry §3.4.1 |

Pin table entry (8 B):

| Bytes | Field | Notes |
|---|---|---|
| 0 | pin | GPIO number (0–39) |
| 1 | kind | §3.4.1 kind codes |
| 2 | module | §3.4.1 module codes (0 = NONE / unassigned) |
| 3 | caps | bit0 input, bit1 output, bit2 adc, bit3 pwm, bit4 pullup, bit5–7 reserved `0` |
| 4–7 | reserved | `0` |

#### 3.4.1 Kind / module codes

Kind: `0 NONE · 1 ADC_NATIVE · 2 ADC_ADS1115 · 3 DIGITAL_IN ·
4 DIGITAL_OUT · 5 LED_STRIP · 6 I2C_SDA · 7 I2C_SCL ·
8 EXPANDER_MCP23017 · 9 EXPANDER_PCF8574 · 10 AXIS_GENERIC ·
11 RESERVED_PIN` (strapping/forbidden — `docs/board-esp32-devkitv1.json`
`class` forbidden/avoid/reserved se map hota hai; WP1/WP2 pin rules hi
authority hain — GET_CAPS sirf board ki current table batata hai).

Module (roles, Plan v2.3 §4): `0 NONE · 1 STEERING · 2 ACCELERATOR ·
3 BRAKE · 4 CLUTCH · 5 HANDBRAKE · 6 SHIFTER · 7 HORN · 8 BUTTONS ·
9 ENCODERS · 10 LED · 11 BATTERY`.

### 3.5 SET_CFG chunk layout (request payload)

| Bytes | Field | Type | Notes |
|---|---|---|---|
| 0–1 | offset | u16 | Is chunk ka config blob mein offset (0-based) |
| 2–3 | total | u16 | Poori config blob ki length (≤ 2048). Har chunk mein **wahi** total |
| 4–5 | chunkLen | u16 | Is chunk mein data bytes (1..1024) |
| 6–7 | chunkCrc | u16 | CRC16 (§0) sirf `data` par |
| 8… | data | bytes | `chunkLen` bytes |

Rules: chunk ≤ 1024 B, total ≤ 2048 B (2 KB). Chunk `offset + chunkLen`
`total` se zyada ho → `TOO_BIG` (4). Chunk CRC mismatch → `BAD_CRC` (3).
Chunks kisi bhi order mein aa sakte hain; board staged buffer mein likhta
hai; poora blob milne par (staged == total) board blob CRC32 (trailer, §3.6)
verify karta hai — fail ho to agle COMMIT par `PIN_CONFLICT` nahi, `BAD_CRC`
(3) milega (COMMIT reply status byte mein). Naya SET_CFG session (offset 0,
alag total) purana staged buffer discard karta hai.

### 3.6 Config blob (staged / stored, §3.5 ka `data`)

Config blob magic **`"RSHG"`** hai (v2 draft ka `"RSHC"` **badal diya gaya**
— `"RSHC"` control frame magic se takrao khatam karne ke liye; blob sirf
SET_CFG payload ke andar aata hai, wire frame nahi hai).

```
Header (8 B): 'RSHG' | schema u8 (= 2) | moduleCount u8 | cfgVersion u16 | flags u8 | reserved u8
Module record (fixed 16 B): type u8 | slot u8 | flags u8 | pinA u8 | pinB u8 | pinC u8 | params[10]
Trailer (4 B): CRC32 (header + records par, CRC-32/ISO-HDLC, poly 0xEDB88320, init 0xFFFFFFFF)
```

Types: `1 AXIS_ADC · 2 AXIS_ADS1115 · 3 BUTTON · 4 BUTTON_MATRIX ·
5 ENCODER · 6 LED_STRIP · 7 BATTERY · 8 EXPANDER_MCP23017 ·
9 EXPANDER_PCF8574 · 10 AXIS_GENERIC`.
Role (steering, throttle, horn…) **PC-side naming** hai, profile mein
rehta hai; board sirf type/slot/pins/hardware params janta hai.
Factory default = v3.3 pin map.

**Transaction (D28):** stage (SET_CFG chunks) → COMMIT → CONFIRM (30 s) →
permanent. Confirm na ho 30 s mein → **rollback** (purani config wapas).
COMMIT reply mein `confirmDeadlineSec = 30` hota hai. COMMIT ke baad
board nayi config **live apply** karta hai (reboot nahi — D28: reboot ho ya
live apply, timer hamesha 30 s). REVERT: staged ya committed-lekin-unconfirmed
config discard karke purani config wapas. COMMIT par validation fail ho to
reply: `status = PIN_CONFLICT (5)` ya `UNSUPPORTED (6)` + `errCount u8` +
`errCode u8 × errCount` (errCode: `1` pin conflict, `2` value out of range,
`3` unknown module/type, `4` chunk/blob CRC mismatch, `5` total size
mismatch). Validation pin rules WP1/WP2 ke mutabiq hai.

### 3.7 LED_TEST pattern format (request payload, 8 B)

| Bytes | Field | Notes |
|---|---|---|
| 0 | patternId | `0 OFF · 1 SOLID · 2 BLINK · 3 CHASE · 4 RAINBOW` |
| 1 | r | Red 0–255 |
| 2 | g | Green 0–255 |
| 3 | b | Blue 0–255 |
| 4–5 | periodMs | u16, blink/chase period (100–5000); SOLID/RAINBOW par ignore |
| 6 | repeats | u8, pattern repeats; `0` = 5 s tak chalao phir stop |
| 7 | reserved | `0` |

Unknown patternId → `UNSUPPORTED`. LED module configured na ho → reply
`UNSUPPORTED`. **R-L9:** LED test fail hoga jab tak GPIO21 digital ke tor
par use na ho — matlab LED strip GPIO21 par digital-out configured hona
chahiye, warna `PIN_CONFLICT` (5). Test khatam hone par strip purani state
par wapas.

### 3.8 PIN_PROBE (request + stream frames)

Request payload (4 B): `pin u8, durationMs u16 (100–10000), mode u8`
(mode `0` = digital level, `1` = ADC mV). Reply: `status u8,
acceptedMs u16` (board ne kitni duration mani; `0` = reject ke saath).
Probe ke dauraan board port 4211 par probe frames bhejta hai (har 100 ms
ek frame, duration khatam hone tak):

| Bytes | Field | Notes |
|---|---|---|
| 0–3 | magic | `"RSHP"` |
| 4 | version | u8 `0x21` |
| 5–6 | seq | u16 (probe sequence, is probe ke andar 0 se) |
| 7 | pin | u8 (probe ho raha pin) |
| 8 | mode | u8 (0 digital / 1 ADC) |
| 9–10 | sampleCount | u16 (is frame mein samples, 1..64) |
| 11… | samples | mode 0: u8 × N (0/1 level); mode 1: u16 LE × N (mV) |
| last 2 | CRC16 | §0 ke mutabiq |

Probe ke dauraan normal data packets **rukte nahi** — probe frames alag
magic se aate hain. Doosra PIN_PROBE chalu probe ke dauraan → `BUSY` (7).
Pin forbidden/strapping ho (WP2 rules) → `PIN_CONFLICT` (5).

### 3.9 Error table (reply payload ka pehla byte — numeric)

| Code | Name | Matlab |
|---|---|---|
| 0 | OK | Kamiyab |
| 1 | BAD_TAG | HMAC tag verify fail (ghalat key / ghalat nonce / tampered frame) |
| 2 | REPLAY | Counter ≤ session max-seen, naya reqId (replay) |
| 3 | BAD_CRC | SET_CFG chunk ya config blob CRC mismatch |
| 4 | TOO_BIG | `len` > 1024, ya chunk `offset + chunkLen > total`, ya total > 2048 |
| 5 | PIN_CONFLICT | Pin validation fail (conflict / forbidden pin / LED_TEST GPIO21 rule) |
| 6 | UNSUPPORTED | Unknown cmd, unsupported feature, KEY_RESET jab armed na ho, LED module na ho |
| 7 | BUSY | Board busy (flash commit / probe chal raha / confirm window mein conflicting op) |

v2 draft ke string/status codes (`0 ok · 1 bad HMAC · 2 bad frame…`)
**superseded** — sirf ye numeric table authority hai.

## 4. Key / password lifecycle (contract hissa — WP4 docs ke saath parho)

### 4.1 Device key (OPEN-2 — CLOSED)

- **32 bytes**, per-device random, board pehle boot par hardware RNG se
  generate karta hai, encrypted NVS mein store hoti hai.
- Sirf **pehli HELLO** mein (pairing window 60 s ke andar) app ko di jati
  hai (§2.2). Window ke baad kabhi nahi.
- App DPAPI (CurrentUser) se store karti hai; kabhi plain text / log / UI /
  export mein nahi aati. Koi backup/export/cloud sync nahi.
- Key rotation sirf KEY_RESET, re-pairing, board replace, ya poora reflash
  par hoti hai. Reboot / OTA / FACTORY_RESET par **nahi**. Key ki koi expiry
  nahi.

### 4.2 Pairing bootstrap (OPEN-6 — CLOSED, **BLOCKER — coordinator review**)

**Faisla: pehli pairing ke liye SoftAP khula (open, koi password nahi) hota
hai jab tak board pair na ho jaye. Pairing kamyab hote hi (pehli HMAC-signed
GET_CAPS OK) board AP ko WPA2-PSK par band kar deta hai, password wahi
per-device password hota hai jo HELLO mein diya gaya tha.**

Reasoning:
- Board ke paas **koi display, button ya label printer nahi** hai — default
  password dikhane ka koi rasta nahi hai (label par factory password chhapna
  "koi factory nahi" usool ke khilaf hai aur shared-default ban jata).
- USB-serial se pehli pairing primary rasta nahi ho sakta: us ke liye cable
  + driver + alag spec (OPEN-20, Phase G) chahiye, aur har user ke paas
  cable nahi hoti. USB pairing Phase G mein **optional** rasta ho sakta hai.
- Khula AP sirf **pehle boot se pairing tak** rehta hai (60 s window +
  pairing complete hone tak; pairing complete na ho to window khatam hone
  par board AP band kar ke dobara kholta hai? Nahi — dekho neeche).

**Security asar (honest):** pairing window mein link-layer encryption
**nahi** hoti — key + password ka tabadla proximity (Wi-Fi range) + 60 s
window par munhasir hai. Window ke andar range mein maujood hamlaawar
tabadla sun sakta hai (THREAT-MODEL TM-W02 residual risk). Mitigations:
(a) window sirf 60 s, AP-up se; (b) disclosure ek dafa; (c) pairing sirf
apne ghar/room mein karne ki hidayat USER-GUIDE mein lazmi (Phase H);
(d) pairing ke foran baad AP WPA2 ho jata hai aur key dobara kabhi nahi di
jati; (e) agar user ko shak ho to KEY_RESET (§4.4) se nayi key+password.
**Window expire hone par (pairing na ho):** board AP **khula hi rehta hai**
lekin HELLO mein key/password **zero** bhejta hai (flags bit3 = 0) aur
pairing refuse karta hai — dobara pairing ke liye KEY_RESET lazmi hai.
(Khula AP rehne ka matlab sirf ye hai ke board nazar aata hai; bina key ke
us se kuch control nahi ho sakta.)

### 4.3 FACTORY_RESET vs KEY_RESET

|  | FACTORY_RESET (`0x08`, HMAC-signed) | KEY_RESET (`0x0B`, physical) |
|---|---|---|
| Wipe | Sirf pin map / config (factory defaults = v3.3 pin map) | Sirf device key + pairing state (+ password, §5.3) |
| Key | **Rehti hai** — purani key se HMAC valid rehta hai | **Wipe + nayi key generate** — purani foran invalid |
| Password | **Rehta hai** (§5.3) | **Naya generate** hota hai (§5.3) |
| Auth | HMAC-signed hona lazmi (purani key se) | Physical BOOT-hold lazmi; purani key ki zaroorat **nahi** |

### 4.4 KEY_RESET + BOOT-hold (OPEN-9, OPEN-10, OPEN-8 — CLOSED)

- **Cmd byte: `0x0B`** (OPEN-10 — CLOSED). FACTORY_RESET `0x08` barqarar.
- **BOOT-hold ka farq (OPEN-9 — CLOSED):** dono istemal **hold ki muddat**
  se farq karte hain, power-up par GPIO0 (BOOT) hold karke:
  - **5 s ≤ hold < 10 s → safe mode** (factory pin map + AP; purana
    behaviour). LED: slow blink, 1 Hz.
  - **hold ≥ 10 s → key-reset-armed state.** LED: fast blink, 4 Hz.
  - Hold < 5 s → normal boot (BOOT ignore).
  - Board hold duration power-up ke waqt naapta hai (BOOT release ka
    waqt); band edges par ±0.5 s tolerance hai — firmware Phase D mein
    real board par measure karega (A7 gate).
- **Armed state (OPEN-8 — CLOSED):** armed state ki pehchan LED fast blink
  (4 Hz) + HELLO/GET_CAPS flags bit `keyResetArmed = 1` hai. Armed window
  **120 s** hai (boot se); waqt guzar jaye to board normal mode mein reboot
  hota hai — dobara BOOT-hold karna hoga. Armed state mein (aur sirf is
  mein) app `KEY_RESET` bhejti hai; armed na ho to reply `UNSUPPORTED` (6).
- KEY_RESET kamyab hone par: board purani key mitata hai, **nayi key +
  naya password** generate karta hai, unpaired mark hota hai, **nayi 60 s
  pairing window** kholta hai (§2.2/§4.2 se pairing dobara). Config **nahi**
  badalti.
- **Safe mode mein KEY_RESET nahi hota** — safe mode sirf factory pin map +
  AP deta hai; key wahi rehti hai.

### 4.5 Key kho jaye / re-pairing

App reinstall / DPAPI key kho jaye / naya PC / board kisi aur ko dena →
§4.4 KEY_RESET → nayi key → §2.2/§4.2 pairing dobara. **Hamesha nayi key**
banti hai; purani dobara kabhi valid nahi hoti; koi "purani key recover"
ka rasta nahi hai. Board replace hone par naye board se pehli pairing
(§2.2); profile v2 mein `boardMac` (D21) se app pehchan sakti hai ke board
badla hai — calibration dobara karni hogi (warning ke saath).

### 4.6 RNG failure / NVS (OPEN-4 — CLOSED behaviour, mechanism Phase D)

**Fail closed:** agar pehle boot par RNG tayyar na ho ya key generation
fail ho, board **normal boot mein aage nahi barhta**. Tareeqa: RNG init
3× retry; phir bhi fail ho to board fault state mein rehta hai — LED
fault pattern (3 short blinks, repeat), AP up nahi hota, koi key nahi
banti, koi pairing nahi. Kamzor ya repeat key ke saath boot **kabhi** aage
nahi barhega. ESP-IDF RNG API + NVS encryption ka exact mechanism
(OPEN-3) Phase D ka hai — contract sirf ye behaviour mangta hai: key /
password **kabhi plain flash mein nahi**, hamesha encrypted NVS mein.

## 5. Wi-Fi / OTA password (OPEN-13/14/15/16 — CLOSED)

### 5.1 Ek credential, format (OPEN-13, OPEN-14 — CLOSED)

- **Faisla (OPEN-13): Wi-Fi (AP join) aur OTA (WebOTA auth) ke liye EK hi
  credential hai** — ek per-device password. Reasoning: dono auth ek hi
  board, ek hi local trust boundary (AP) ke hain; do alag passwords ka
  matlab double provisioning, double dikhana, double bhoolna — user burden
  bina security faide ke. Compromise ka asar bhi ek jaisa hai (dono local
  hain). Is liye ek password, do jagah istemal.
- **Faisla (OPEN-14): 24 characters**, charset `A–Z, a–z, 2–9` — ambiguous
  characters `0, 1, I, O, l` **nikal diye gaye** (58 symbols). Entropy ≈
  `24 × log2(58) ≈ 140 bit`. Generation: hardware RNG, uniform (modulo bias
  se bachne ke liye rejection sampling — Phase D). WPA2-PSK minimum 8
  characters se zyada hai; 24 user ke liye type karne ke qaabil hai.
- **Display:** app password ko pehli pairing par **ek dafa** QR / label ke
  tor par dikhati hai — 4 groups of 6, hyphen se (`XXXXXX-XXXXXX-XXXXXX-
  XXXXXX`). **Wire/store format mein hyphen nahi hote** — HELLO reply
  (§2.2) aur NVS mein 24 ASCII bytes, baghair hyphen.
- **Forced change (NIST 8259A):** pehli pairing ke baad user ko apna
  password set karne ka mauqa/taqaza hoga — exact UX Phase D/E (OPEN-12),
  mechanism `SET_PASSWORD` (§5.2) yahan define hai.

### 5.2 SET_PASSWORD (`0x0C`) — mechanism

- Request: `newLen u8 (8..63)` + password ASCII bytes. Charset: printable
  ASCII `0x21–0x7E` (space nahi). Length/charset fail → `UNSUPPORTED` (6).
- Kamyabi par board password AP + WebOTA dono ke liye apply karta hai aur
  AP restart karta hai (connected clients drop honge — dobara join karna
  hoga). Reply `OK` restart se **pehle** bheja jata hai.
- SET_PASSWORD HMAC-signed hona lazmi hai (purani key se) — bina key ke
  password nahi badal sakta.

### 5.3 App-side storage (OPEN-15 — CLOSED)

- **Wi-Fi join:** OS Wi-Fi profile (Windows WLAN profile) — app password
  ko apni file mein **nahi** rakhti join ke liye; OS profile is ka ghar hai.
- **WebOTA auth ke liye:** app password ko **DPAPI-encrypted blob** mein
  store karti hai (device key wali store ke saath, `%AppData%\RSH` mein,
  alag blob). Reasoning: Windows WLAN profile se PSK plaintext wapas
  nikalna public API se nahi hota (admin `netsh` chahiye) — WebOTA auth ke
  liye app ko password chahiye, is liye DPAPI blob zaroori hai. Protection
  level device key jaisa hi hai (user-bound; §4.1).
- Password kabhi plain file / log / UI (ek dafa dikhane ke baad) / export /
  crash dump mein nahi aata. In-memory sirf zaroorat ke waqt, app band
  hone par clear.

### 5.4 Password bhool jaye to (OPEN-16 — CLOSED)

- **Faisla: KEY_RESET (§4.4) password ko bhi reset karta hai.** KEY_RESET
  par board **nayi key + naya password dono** generate karta hai, phir
  dobara pairing (§4.2). FACTORY_RESET password ko **nahi** chheerta
  (credentials config nahi hain — key wale rule jaisa).
- Reasoning: password reset ko physical presence chahiye (warna jis ke
  paas key ho wo password badal kar malik ko lockout kar sakta hai —
  FACTORY_RESET bhi HMAC-signed hai lekin wo config ka kaam hai,
  credentials ka nahi). KEY_RESET hi woh rasta hai jo key kho jane aur
  password bhoolne dono ko ek physical action se hal karta hai.

## 6. Purani doc se farq (OPEN-1 aur v2 draft supersede list)

### 6.1 OPEN-1 — CLOSED: key derivation badal gayi

Purani `PROTOCOL-V2.md` mein `key = SHA-256(OTA password)` tha aur tag 8
bytes ka tha. **Ye superseded hai.** v2.1 mein key **per-device random
32 bytes** hai (board generate karta hai, §4.1), tag **16 bytes** hai, aur
HMAC input `nonce ‖ counter ‖ cmd ‖ payload` hai (password is mein kahin
nahi aata). `PROTOCOL-V2.md` ko is design ke mutabiq update kar diya gaya
hai — v2.1 ke liye **ye doc authority** hai.

### 6.2 v2 draft se badla hua (summary)

| Cheez | v2 draft (`PROTOCOL-V2.md` purani) | v2.1 (ye doc) |
|---|---|---|
| Stream magic | `0xA5` + ver byte | `"RSH2"` + ver `0x21` |
| Stream flags | bit0 steering-mV | D4 table (§1.3) |
| Buttons | u32 (draft mein bhi) | u32 + `btnsEx` status mein (D1) |
| Encoders | i8 detents | i16 cumulative |
| Axes | u16 (mvMask tha) | u16 mV, mvMask khatam (D6) |
| Status | 30 B | 34 B base / 38 B extended (D5) |
| Control magic | `0xC7` + ver 1 | `"RSHC"` + cmd + reqId + counter + len |
| Control tag | HMAC[0:8], key=SHA256(pw) | HMAC[0:16], key=random 32 B |
| Control counter | nahi tha | u32 per-session (D3) |
| Error codes | string-ish 0–6 | numeric 0–7 (§3.9) |
| HELLO reply | nahi tha | 92 B fixed (§2.2) |
| GET_CAPS body | "board id, limits, pin table" (no layout) | byte layout (§3.4) |
| SET_CFG | chunk(offset,total,bytes) (no layout) | byte layout + CRC16 (§3.5) |
| LED_TEST | "pattern" (no layout) | 8 B pattern (§3.7) |
| PIN_PROBE | "pin, ms" (no stream frame) | request + `"RSHP"` stream frame (§3.8) |
| Config blob magic | `"RSHC"` | `"RSHG"` (§3.6) |
| KEY_RESET / SET_PASSWORD | nahi thay | `0x0B` / `0x0C` (§3.3) |
| Confirm timer | 10 s (draft) | **30 s** (D28) |

## 7. OPEN items ka status (WP3)

| # | Item | Status | Faisla / wajah |
|---|---|---|---|
| OPEN-1 | Key = SHA256(OTA pw) vs per-device random | **CLOSED** | §6.1 — per-device random 32 B; `PROTOCOL-V2.md` update ho gayi |
| OPEN-2 | Key length | **CLOSED** | §4.1 — **32 bytes** (HMAC-SHA256) |
| OPEN-3 | Encrypted NVS mechanism + flash encryption on/off | **OPEN — Phase D** | Contract sirf behaviour mangta hai (§4.6: kabhi plain flash nahi). ESP-IDF mechanism firmware ka faisla hai |
| OPEN-4 | RNG tayyari + key-gen failure behaviour | **CLOSED** (behaviour) | §4.6 — fail closed, 3× retry, phir fault state (LED 3-short-blink). API/mechanism Phase D |
| OPEN-5 | Pairing window start trigger | **CLOSED** | §2.3 — AP-up se 60 s; expiry nishani flags bit3 + zero fields |
| OPEN-6 | Pairing bootstrap circular dependency | **CLOSED** (**BLOCKER**, coordinator review) | §4.2 — pehli pairing tak AP khula; phir WPA2. Security asar likha hai |
| OPEN-7 | HELLO reply byte layout | **CLOSED** | §2.2 — 92 B fixed, nonce 16 B samet |
| OPEN-8 | KEY_RESET armed state pehchan + window | **CLOSED** | §4.4 — LED fast blink 4 Hz + flags bit; window 120 s |
| OPEN-9 | BOOT-hold do istemal | **CLOSED** (coordinator review) | §4.4 — hold duration: 5–<10 s safe mode, ≥10 s key-reset-armed |
| OPEN-10 | KEY_RESET cmd byte | **CLOSED** | §3.3/§4.4 — **`0x0B`** |
| OPEN-11 | OTA key NVS ko nahi chheerti | **OPEN — Phase D** | Contract requirement: OTA NVS key namespace ko preserve kare. Firmware mein confirm karna hai |
| OPEN-12 | Forced password change UX/flow | **OPEN — Phase D/E** | Mechanism `SET_PASSWORD` (§5.2) define hai; app UX + board apply flow Phase D/E |
| OPEN-13 | Wi-Fi vs OTA password: ek ya do | **CLOSED** | §5.1 — **ek credential** dono ke liye |
| OPEN-14 | Password length / charset | **CLOSED** | §5.1 — **24 chars**, `A–Z a–z 2–9` (58 symbols, ≈140 bit) |
| OPEN-15 | App-side password storage | **CLOSED** | §5.3 — Wi-Fi: OS profile; WebOTA: DPAPI blob (key store ke saath) |
| OPEN-16 | Password bhoolne par reset rasta | **CLOSED** | §5.4 — **KEY_RESET** (key + password dono naye); FACTORY_RESET nahi |
| OPEN-17 | App-level signature: Ed25519 vs ECDSA P-256 | **CLOSED** (coordinator review) | **Ed25519** — 64 B signatures, deterministic verify, verify ke liye RNG nahi chahiye, ESP-IDF/mbedTLS support. ECDSA P-256 alternate nahi — ek algorithm, wahi verify-before-apply |
| OPEN-18 | Anti-downgrade exact mechanism | **CLOSED** (rule) / mechanism **Phase D** | Rule: OTA image ka version (fwMajor/Minor/Patch, image header mein) running version se **kam ho to reject**. eFuse counter istemal karna hai ya nahi — Phase D |
| OPEN-19 | Signing/Secure Boot key compromise response | **OPEN — Phase H** | Release process ka kaam (revocation / re-sign / purane boards) |
| OPEN-20 | Wired (USB-serial) par pairing | **OPEN — Phase G** | v2.1 mein pairing sirf Wi-Fi SoftAP par define hai; wired spec D26 Phase G mein |

## 8. Verification status

- **Golden vectors:** `tests-host/protocol_v2_vectors.py` — stream v2.1,
  status 34 B + 38 B, HELLO reply 92 B, control HMAC (positive + negative:
  bad tag, wrong key, replay, truncated, duplicate reqId) — sab regenerate
  hue hain. Script chalana = vectors produce karna (implementation), **gate
  pass nahi**.
- **3-language match gate (T1/T2/T3): PENDING — Virtual Test Rig ka
  intezar.** Rig (`~/workspace/projects/rsh/test-env/`) abhi tayyar nahi
  hai. Is doc ke bahar alag test setup **nahi** banaya gaya. Gate sirf rig
  ke andar dobara green hone par "pass" likha jayega.
- **Coordinator review ke liye flagged:** OPEN-6 (bootstrap), OPEN-9
  (BOOT-hold bands), §1.2 status offsets note, §1.1 N1 (buttons 64–127),
  OPEN-17 (Ed25519).

## Appendix A — v1 legacy packet (reference, unchanged)

v1 data packet (18 B): `0xA5 | 0x01 | seq u16 | ts u32 | steer u16 |
accel u16 | brake u16 | buttons u8 | flags u8 | CRC16`. Golden:
`a5 01 34 12 04 03 02 01 fc 08 e8 03 d0 07 05 01 75 bd`
(CRC re-verified, `protocol_v2_vectors.py`). Legacy mode: version byte
`0x01` dekhte hi app legacy path leti hai; v1 app ko v2.1 board v1 packets
bhejta hai (HELO 4-byte, §2.1).
