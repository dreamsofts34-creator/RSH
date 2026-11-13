# RSH Protocol v2 — contract (S0/F0 draft, SUPERSEDED by v2.1)

Status: **SUPERSEDED — v2.1 authority hai: `docs/PROTOCOL-V2.1.md`.**
Ye file sirf history ke liye rakhi gayi hai. v2.1 implement karne wali koi
bhi cheez (firmware, app, parser, vectors) is file ko authority **na mane**.

All integers little-endian. CRC16 = CCITT, poly `0x1021`, init `0xFFFF`, over
every byte before the CRC field. Ports unchanged: PC→board `4210`, board→PC
`4211`; new control channel `4212`.

## OPEN-1 fix (WP3, Phase A)

Is file ke purane §4 mein likha tha `key = SHA-256(OTA password)` aur tag
8 bytes ka tha. **Ye design superseded hai** (Plan v2.3 §2, `docs/KEY-LIFECYCLE.md`
§10 OPEN-1):

- v2.1 mein key **per-device random 32 bytes** hai — board pehle boot par
  hardware RNG se generate karta hai, encrypted NVS mein rehti hai; app
  DPAPI se store karti hai. Password se key derive **kabhi nahi** hoti.
- Tag **16 bytes** hai: `HMAC-SHA256(key, nonce ‖ counter ‖ cmd ‖ payload)[0:16]`.
- Control frame layout, error codes, HELLO reply, GET_CAPS body, SET_CFG
  chunk layout — sab `docs/PROTOCOL-V2.1.md` mein hain.

## v2 draft se v2.1 mein kya badla (summary — detail PROTOCOL-V2.1.md §6.2 mein)

| Cheez | v2 draft (ye file, purani) | v2.1 (authority) |
|---|---|---|
| Stream magic | `0xA5` + ver byte | `"RSH2"` + ver `0x21` |
| Stream flags | bit0 steering-mV | D4 table (config-changed, low-supply, tick-skipped, axis-invalid) |
| Encoders | i8 detents | i16 cumulative |
| Axes | u16 (mvMask tha) | u16 mV, mvMask khatam (D6) |
| Status | 30 B | 34 B base / 38 B extended (D5: seqGap + txFail, `btnsEx`) |
| Control magic | `0xC7` + ver 1 | `"RSHC"` + cmd + reqId u16 + counter u32 + len u16 |
| Control tag | HMAC[0:8], key=SHA256(password) | HMAC[0:16], key=random 32 B |
| Error codes | 0–6 (draft) | numeric 0–7 (OK, BAD_TAG, REPLAY, BAD_CRC, TOO_BIG, PIN_CONFLICT, UNSUPPORTED, BUSY) |
| HELLO reply | nahi tha | 92 B fixed (nonce 16 B, key 32 B, password 24 B) |
| Config blob magic | `"RSHC"` | `"RSHG"` (control magic se takrao khatam) |
| Confirm timer | 10 s (draft) | **30 s** (D28) |
| Commands | 0x01–0x0A | + `0x0B` KEY_RESET, `0x0C` SET_PASSWORD |

## Neeche purana v2 draft (reference only — implement mat karo)

### 1. Version negotiation (v1 apps keep working)

| PC sends to 4210 | Meaning | Board replies with |
|---|---|---|
| `"HELO"` (4 B) | v1 app | v1 18-byte data packets (unchanged) |
| `"HELO"` + `0x02` (5 B) | v2 app | v2 data packets + 1 Hz status packets |
| `"RSTX"` | restart (legacy, kept) | — |

No HELO for 2000 ms ⇒ board goes silent (unchanged).

### 2. Data packet v2 (draft — superseded by PROTOCOL-V2.1.md §1.1)

| Bytes | Field |
|---|---|
| 0 | magic `0xA5` |
| 1 | version `2` |
| 2–3 | sequence u16 |
| 4–7 | timestamp µs u32 |
| 8 | axis mask u8 (bit *n* = axis *n* present, n = 0..7) |
| 9 | flags: bit0 steering in mV · bit1 config changed · bit2 low supply · bit3 tick skipped (flash commit) |
| 10–13 | buttons u32 (bit *n* = button *n*) |
| 14 | encoder count *E* u8 |
| 15 | reserved (0) |
| 16… | axes: u16 × popcount(mask), ascending axis index |
| … | encoder deltas: i8 × *E* (signed detents since last packet) |
| last 2 | CRC16 |

### 3. Status packet (draft 30 bytes — superseded by PROTOCOL-V2.1.md §1.2)

| Bytes | Field |
|---|---|
| 0 | magic `0xA6` (distinguishes from data) |
| 1 | version `2` |
| 2–3 | status sequence u16 |
| 4–7 | uptime ms u32 |
| 8 | RSSI i8 dBm (of the PC link) |
| 9 | battery % u8 (`255` = no battery module) |
| 10–11 | supply mV u16 (`0` = not measured) |
| 12–15 | free heap u32 |
| 16–17 | overruns u16 (since boot) |
| 18–19 | max cycle µs u16 |
| 20 | governor level u8 |
| 21 | reset reason u8 (1 power-on, 2 brown-out, 3 watchdog, 4 software …) |
| 22–25 | config CRC32 u32 |
| 26–27 | config version u16 |
| 28–29 | CRC16 |

### 4. Control frame (draft — superseded by PROTOCOL-V2.1.md §3)

```
0 magic 0xC7 | 1 ver 1 | 2 cmd u8 | 3-4 reqId u16 | 5-6 payloadLen u16 | payload… | tag[8]
tag = HMAC-SHA256(key, bytes 0..end-of-payload)[0:8],  key = SHA-256(OTA password)
Reply: same layout, cmd = request cmd | 0x80, payload = status u8 + body.
Retry: PC resends the same reqId up to 3× (250 ms); board answers duplicates idempotently.
```

| cmd | Name | Payload → reply body |
|---|---|---|
| 0x01 | GET_CAPS | — → board id, fw version, limits, pin table (from the board JSON), module types supported |
| 0x02 | GET_CFG | — → config blob (chunked) |
| 0x03 | SET_CFG | chunk(offset, total, bytes) → ok / list of validation errors |
| 0x04 | COMMIT | — → ok; starts 10 s confirm timer |
| 0x05 | CONFIRM | — → ok (config becomes permanent) |
| 0x06 | REVERT | — → ok |
| 0x07 | PIN_PROBE | pin, ms → ok, then level/ADC samples in status-style frames |
| 0x08 | FACTORY_RESET | — → ok |
| 0x09 | REBOOT | — → ok |
| 0x0A | LED_TEST | pattern → ok |

Status codes: 0 ok · 1 bad HMAC · 2 bad frame · 3 validation failed · 4 busy · 5 not staged · 6 unsupported.

### 5. Config blob (draft — blob magic v2.1 mein `"RSHG"` hai)

```
Header: 'RSHC' | schema u8 | moduleCount u8 | cfgVersion u16 | flags u8 | reserved
Module record (fixed 16 B): type u8 | slot u8 | flags u8 | pinA u8 | pinB u8 | pinC u8 | params[10]
Trailer: CRC32
Types: 1 AXIS_ADC · 2 AXIS_ADS1115 · 3 BUTTON · 4 BUTTON_MATRIX · 5 ENCODER · 6 LED_STRIP ·
       7 BATTERY · 8 EXPANDER_MCP23017 · 9 EXPANDER_PCF8574 · 10 AXIS_GENERIC
```
Role of each slot (steering, throttle, horn …) is **PC-side naming** stored in the profile; the board only knows type/slot/pins/hardware params. Factory default = the v3.3 pin map.

## Golden vectors (v1 — abhi bhi valid; v2 draft vectors superseded)

```
v1 golden      a5 01 34 12 04 03 02 01 fc 08 e8 03 d0 07 05 01 75 bd   (still valid, CRC re-verified)
```

v2 draft vectors (`data_v2`, `status_v2`, `ctrl_get_caps` purane script mein)
**superseded** hain — v2.1 vectors `tests-host/protocol_v2_vectors.py` mein
hain aur `docs/PROTOCOL-V2.1.md` ke mutabiq hain. Pin rules:
`docs/board-esp32-devkitv1.json` + `tests-host/pin_rules_ref.py`.
