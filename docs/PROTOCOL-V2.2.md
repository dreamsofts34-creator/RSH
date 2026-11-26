# RSH — Protocol v2.2 (wire authority)

**Status:** FROZEN — app v2.0.6 · firmware v4.0.3 · protocol v2.2.
**Supersedes:** protocol v2.1 (no v2.1 wire document ever existed; the
spec lived in code comments — this document is the wire authority
from v2.2 on).

**Sources (byte-exact with this document):** `firmware/protocol.h`
(C++ parser/builder, Arduino-free, host-tested),
`crates/rsh-protocol/src/{hello,stream,control,caps,ping,serial}.rs`
(Rust), `crates/rsh-protocol/tests/golden_vectors.rs` (vectors —
a vector here and a vector there must match byte-identically or
the gate fails), `firmware/tests-host/protocol_v2_vectors.py`
(firmware vectors; WS-1 regenerates for v2.2).

**Conventions:** all integers little-endian. CRC16 is always
CRC16-CCITT-FALSE (poly `0x1021`, init `0xFFFF`, no final XOR),
always the last 2 bytes of the frame, always over every byte
before the CRC field. Reserved bytes/bits are always `0` on send
and rejected (`RSV_NONZERO` / parse error) when non-zero on
receive — a future version may define them, but v2.2 sends zero
and refuses anything else.

## 0. What changed from v2.1 (the whole point)

| v2.1 | v2.2 |
|---|---|
| Per-device 32 B key, disclosed once in the first HELLO inside a 60 s pairing window | **No device key.** The HELLO fields that carried it are reserved/zero (§2.2). |
| HMAC-SHA256 tags on every control frame (`nonce ‖ counter ‖ cmd ‖ payload`, tag = first 16 B) | **No HMAC.** Control frames are plain (§3.1): header + payload + CRC16 trailer. |
| Replay protection via HMAC-bound u32 counter; `BAD_TAG` (1) / `REPLAY` (2) | **No counter.** Duplicates are caught by **reqId only** (§2.3): a u16 reqId, monotonic per session, last-32 cache. `BAD_TAG` (1) is deleted as a concept; code 1 is reused by **`BAD_PASSWORD`** (SET_PASSWORD current password wrong, §3.6). `REPLAY` (2) is kept as a wire value but never sent (a duplicate is answered from the cache). |
| `KEY_RESET` (0x0B) wire opcode: BOOT-hold arms a 120 s window, app sends the opcode, board rotates key + password | **Opcode 0x0B deleted** (replies `UNSUPPORTED`). Password-clear is physical-only: hold BOOT ≥ 10 s and the firmware deletes the NVS password itself and reboots to an open AP (§5). |
| Pairing window; `Authenticating` state; `NeedsPairing` / `WindowClosed` / `NoStoredKey` / `StoredKeyExists` errors | **Gone.** Connection state machine is `Idle → LinkUp → Hello → Active` (§6). `Active` = unsigned `GET_CAPS` OK. |
| Stream gate: "≥1 HMAC-verified control per session" (SEC-E-14) | Stream gate: **plain `GET_CAPS` OK** (§6). |
| Version byte `0x21`, protocol `(2,1)` | Version byte **`0x22`**, protocol **`(2,2)`** (§0.1). |
| SET_PASSWORD: single new password, HMAC-signed | SET_PASSWORD (0x0C) keeps its opcode; payload now carries the **current password too when one is set** (§3.6, D3). |
| WebOTA auth = generated per-device password, always on | WebOTA auth = **user-set Wi-Fi password; disabled until a password is set** (§7, D9). |

> **D3 implemented on both sides:** the firmware enforces the
> §3.6 payload layout and the Rust `ControlClient::set_password`
> sends it (`crates/rsh-transport/src/control_client.rs`) — the
> stored Wi-Fi password travels as the current password, and a
> wrong current password comes back as `BAD_PASSWORD` (1).

### 0.1 Versioning and forced upgrade

- **Wire version byte: `0x22`** (was `0x21`). Stream, status,
  HELLO, control and ping frames all carry it; a frame with any
  other version byte is rejected (`BAD_VERSION`).
- **Protocol version: `(2,2)`** (was `(2,1)`). Reported in HELLO
  (§2.2) and in the `GET_CAPS` reply (§3.4).
- **Old firmware (≤ 4.0.2, protocol `(2,1)`) still parses:** a
  v2.2 HELLO parser accepts a `(2,1)` HELLO (its key/password
  fields are parsed but **never acted on** — the credentials are
  dead on arrival). The app must detect it and offer the WebOTA
  update; it must **not** attempt control (a v2.1 control frame
  is rejected by a v2.2 board, and a v2.2 control frame is
  rejected by a v2.1 board).
  > **Scope note (R3-FW-06a):** "a v2.2 HELLO parser accepts a
  > `(2,1)` HELLO" is true of the **Rust (app-side)** parser
  > (`crates/rsh-protocol/src/hello.rs` — accepts `(2,2)` and
  > `(2,1)`), which is the parser that needs it (old-firmware
  > detection). The **firmware** parser
  > (`firmware/protocol.h` `rsh_v2_parse_hello`) accepts **only**
  > `(2,2)` and returns `RSH_V2_BAD_VERSION` for `(2,1)` — the
  > board never parses HELLOs (it only builds them), so this has
  > no runtime effect on the board; the two parsers differ by
  > design, not by accident.
- **Old app (≤ 2.0.5) vs new firmware (4.0.3): broken by design.**
  Upgrade order is **app first (v2.0.6), then firmware (v4.0.3
  via WebOTA)** — the new app still holds the old stored Wi-Fi
  password for the flash, and 4.0.3 first boot deletes the old
  NVS secrets (§8).

## 1. Transports

### 1.1 Wi-Fi (UDP)

Board SoftAP, default IP `192.168.4.1` (mDNS `rsh.local` first,
fallback IP). SSID **`RSH-XXXX`** — last 4 hex of the board Wi-Fi
MAC, uppercase, fixed per board (e.g. board `20:9B:A9:70:9A:55`
→ `RSH-9A55`). The SSID never changes on password-set or on
KEY_RESET. The AP is **open while no password is set** and
**WPA2-PSK once the user sets one** (Settings → Wi-Fi password,
§3.6). Stale Windows profiles named `RSH` are ignored; the app
joins by the `RSH-XXXX` SSID it finds by scanning.

| Port | Direction | Frames |
|---|---|---|
| 4210 | PC → board | HELO (§2.1), 1 Hz keep-alive |
| 4211 | Board → PC | Stream (§1.2) + HELLO replies (§2.2) + ping replies (§4) |
| 4212 | PC ↔ board | Control (§3) + ping (§4) |

### 1.2 USB-serial

`RSHS` envelope (unchanged from v2.1; see
`crates/rsh-protocol/src/serial.rs`): the same stream / HELLO /
control frames wrapped for wired serial at 921600 baud. Cable =
physical trust — USB control is unauthenticated-by-design, same
connect-play path as Wi-Fi, different transport. No password, no
pairing, no window.

### 1.3 Stream frames (board → PC, port 4211)

```
"RSH2" | ver 0x22 | flags u8 | seq u16 LE | timestamp_us u32 LE |
axis_mask u8 | n_enc u8 | buttons u32 LE | reserved u16 (= 0) |
axes (2 B each, mask order) | encoders (2 B each, i16 cumulative) |
CRC16
```

20-byte header; total 20 + 2×axes + 2×n_enc + 2. Flags:
`0x01` config-changed, `0x02` low-supply, `0x04` tick-skipped,
`0x08` axis-invalid; bits 4–7 reserved 0. Max 8 axes, 4
encoders, 64 wire buttons — **wire ceilings, not implemented
counts** (R3-FW-07): the firmware can ever emit at most **5 axes**
(steering, accel, brake, clutch, handbrake — `stream_emit.h`)
and **2 encoders** (`n_enc > 2` → stream builder returns 0).
`seq` is the stream's own u16
sequence (unchanged from v2.1); stream frames have no HMAC in
any version — an altered packet fails CRC16 and is dropped, and
status packets carry `seqGap` + `txFail` counters so loss is
visible.

### 1.3.1 Stream `buttons` bit assignment (PINNED, v2.0.13)

The `buttons u32` word is the board's button word — bit *n* is
game button *n+1*. Pinned here (HID P1-1, polish round) because
v2.1/v2.2 never wrote the numbers down and the host read the
horn on the wrong bit:

| bits | meaning |
| --- | --- |
| 0 | shift-up (sequential) |
| 1 | shift-down (sequential) |
| 2 | accel pedal, button mode (digital pedal press) |
| 3 | brake pedal, button mode (digital pedal press) |
| 4 | horn |
| 5–10 | H-pattern gears 1–6 (one bit per gear) |
| 11–31 | wheel buttons (BUTTON slots) |
| 32–63 | extended buttons (`btns_ex` in the 38 B status packet) |

Canonical source: `firmware/modules.h` (`RSH_BTN2_HORN 4`,
`RSH_BTN2_ACCEL 2`, `RSH_BTN2_BRAKE 3`) and
`crates/rsh-engine/src/pipeline.rs` (`HORN_BIT`,
`PEDAL_BUTTON_BITS`). The host pipeline strips bits 2/3 before
debounce/output — they are NOT wheel buttons: in pedal
button-mode the firmware drives the accel/brake axis from the
debounced pedal bit (4095 pressed / 0 released) and the axis is
what the game sees. A bit-2 press must never honk and a bit-4
press must never reach the game as a pedal.

### 1.4 Status packets (board → PC, 1 Hz) — `0xA6`

Sent at 1 Hz to a v2 peer only (a legacy v1 app does not
understand the 34 B form). Two lengths: **34 B** base, **38 B**
extended (when `btns_ex` is present — buttons 32–63). The CRC
is always last; `seq_gap`/`tx_fail` sit directly before it, so
their offsets shift by 4 in the extended form
(`firmware/protocol.h` `rsh_v2_parse_status` /
`rsh_v2_build_status`; Rust `crates/rsh-protocol/src/stream.rs`).

```
offset  field (base 34 B)
0       magic 0xA6
1       ver 0x22
2..3    seq u16 LE (status packet's own sequence, NOT the stream seq)
4..7    uptime_ms u32 LE
8       rssi_dbm i8
9       battery_pct u8 (255 = no battery module)
10..11  supply_mv u16 LE (0 = not measured)
12..15  heap u32 LE (free heap, bytes)
16..17  overruns u16 LE (see note)
18..19  max_cycle_us u16 LE (longest loop/sample cycle, µs)
20      governor u8 (adaptive sampling level 0–3)
21      reset_reason u8 (1 power-on, 2 brown-out, 3 watchdog,
        4 software, 0 reserved)
22..25  cfg_crc u32 LE (CRC32 of the live config blob)
26..27  cfg_ver u16 LE (live config version)
28..29  seq_gap u16 LE (stream seq skipped since boot, D5)
30..31  tx_fail u16 LE (failed UDP transmissions since boot)
32..33  CRC16 (over bytes 0..31)

extended 38 B form: btns_ex u32 LE (buttons 32–63, bit n−32)
at 28..31; seq_gap at 32..33; tx_fail at 34..35; CRC16 at
36..37 (over bytes 0..35).
```

- **`overruns` is a wrap-aware u16 counter (R3-FW-09):** the
  board counts overruns in a u32 (`overrunCount`) and sends only
  its low 16 bits — the field wraps at 65,535. Read **deltas**
  between consecutive status packets (u16 wrap-safe), never the
  absolute value as a boot total. The layout is frozen (no
  32-bit field without a protocol bump).
- **`tx_fail` is likewise u16** (wrap allowed, D5) — deltas only.
- **`rssi_dbm` is the strongest associated station's RSSI
  (R3-FW-09), not the PC's:** the board reports the max RSSI
  over all SoftAP stations (it has MACs, not IPs, for them), so
  when a phone/browser is joined for WebOTA and is closer than
  the PC, the status packet carries the phone's (better) RSSI.
  `-128` = unknown (no station / API fail). The rescan decision
  uses the same value, but rescan is off by default
  (`RSH_AUTO_CHANNEL` 0).
- `battery_pct` is always 255 and `supply_mv` always 0 in
  v4.0.3 (no battery module, supply not measured) — the fields
  exist so a future module can fill them without a layout change.

## 2. Discovery — HELO / HELLO

### 2.1 HELO (PC → board, port 4210)

6 bytes: `"HELO" | proto u8 (0x02) | sessionFlag u8`.
A 5-byte HELO (no flag) means `sessionFlag 0` (v2 draft compat).

`sessionFlag`: `0` = resume, `1` = new session. Retained for
wire compatibility; **a v2.2 board carries no session material,
so the flag has no effect on the board** — a new session is just
a new session (the reqId dedup cache resets on a new session /
peer change, §2.3). The 1 Hz HELO is also the keep-alive; the
app sends it until a HELLO arrives (3 attempts, then
`Unreachable`).

### 2.2 HELLO reply (board → PC, port 4211) — 92 B fixed

```
offset  field
0..3    "RSHR"
4       proto major = 2
5       proto minor = 2
6       flags (see below)
7       session_echo (echo of the HELO sessionFlag)
8..13   board_mac[6]
14      fw_major (4)
15      fw_minor (0)
16      fw_patch (3)
17      reserved = 0
18..33  nonce[16] — RESERVED, always 0x00
34..65  device_key[32] — RESERVED, always 0x00
66..89  wifi_password[24] — RESERVED, always 0x00
90..91  CRC16 (over bytes 0..89)
```

**Flags (byte 6): only bit2 `safe_mode` (`0x04`) is defined.**
bit0 (`unpaired`), bit1 (`key_reset_armed`) and bit3
(`window_open`) are **reserved 0** — the pairing window and the
armed state are gone (D7 resolved the plan's D1/D7 conflict:
there is no armed state anymore, so the flag is deleted, not
redefined). Any other flag bit set, byte 17 non-zero, or any byte
in 18..89 non-zero → `RSV_NONZERO` (reject).

**HELLO never discloses a secret.** The reserved fields exist
only so the layout stays 92 B (parsers, firmware, Rust and rig
all key on it); the firmware always sends `0x00` there and the
parsers never read them. An old `(2,1)` HELLO still parses
(§0.1) — its disclosed key/password are ignored.

**Golden vector (v2.2, firmware 4.0.3, MAC `aa:bb:cc:dd:ee:ff`,
flags 0):**

```
52 53 48 52 02 02 00 00 aa bb cc dd ee ff 04 00
03 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
00 00 00 00 00 00 00 00 00 00 ba f6
```

### 2.3 Duplicate detection — reqId only (D2)

Control frames carry a **u16 reqId, monotonic per session**
(wrap allowed). The board keeps the last 32 reqIds; a frame
whose reqId is in the cache is a **duplicate** — the cached
reply is re-sent and the command is **not** executed again. A
reqId older than the 32-entry window that reappears is accepted
(a monotonic sender never goes more than 32 ahead without a
reply, so this is the correct behaviour across a u16 wrap).
Stream frames keep their own u16 seq (§1.3); they are
unaffected by reqId dedup.

**When the cache resets (R3-FW-02 / R3-FW-11):** the dedup cache
(and the `GET_CAPS`-OK stream gate) reset **per transport**, on:
HELO `sessionFlag == 1` on that transport; a **new peer IP** on
Wi-Fi; a peer returning after the liveness timeout (no HELO for
> 2 s — the board already treats it as gone, `net_pc_active`
false, so the cache must not outlive the session it belonged
to); or a board reboot. A routine HELO (same IP, inside the
timeout) never resets. Why this matters: the app restarts its
reqIds at 0, so without the new-peer / return-after-timeout
triggers the first ~32 commands after an app restart could hit
the old session's cached replies and fail (wrong execution never
happens — the reply's cmd check stops that — but the command
fails its retry budget on the same stale reply). Serial's cache
is separate: a serial flag-1 resets only the serial session,
never Wi-Fi's.

There is **no replay window anymore**: a captured frame can be
replayed within the reqId-dedup scope. Accepted as a LAN-only
risk behind the WPA2 password (THREAT-MODEL-RUST TM-W03).

## 3. Control channel (port 4212, plain)

### 3.1 Frame layout

```
"RSHC" | cmd u8 | reqId u16 LE | len u16 LE | payload | CRC16 LE
```

9-byte header + payload (≤ 1024 B) + 2-byte CRC16 trailer.
Total 11 + len, max frame 1035 B. **No key, no nonce, no
counter, no tag.** A v2.1 HMAC frame (13-byte header + 16-byte
tag) fails here — length mismatch first (`BAD_LENGTH`), and its
CRC can never match — so an old app cannot control a new board
(forced upgrade, §0.1).

A reply frame is `cmd = reqCmd | 0x80`, payload `status ‖ body`,
where `status` is the first payload byte (§3.9).

### 3.2 Retries

The PC retries a control frame with the **same reqId** (new
frame, same id) until a reply arrives or the retry budget is
spent. The board answers a retried reqId from its duplicate
cache (§2.3) — a retry never executes twice.

### 3.3 Opcodes

| Code | Command | Notes |
|---|---|---|
| 0x01 | `GET_CAPS` | §3.4. Unsigned — this is what opens the session. |
| 0x02 | `GET_CFG` | Config transaction (§3.5). |
| 0x03 | `SET_CFG` | Chunked config write (chunk ≤ 1024 B, total ≤ 2048 B). |
| 0x04 | `COMMIT` | Config transaction. |
| 0x05 | `CONFIRM` | Config transaction (30 s confirm window). |
| 0x06 | `REVERT` | Config transaction. |
| 0x07 | `PIN_PROBE` | `RSHP` probe frames. |
| 0x08 | `FACTORY_RESET` | Clears password + config, reboots to open AP. **Unauthenticated by design** (§3.7). |
| 0x09 | `REBOOT` | **Unauthenticated by design** (§3.7). |
| 0x0A | `LED_TEST` | |
| 0x0B | `KEY_RESET` | **DELETED.** Replies `UNSUPPORTED`. See §5. |
| 0x0C | `SET_PASSWORD` | §3.6. |
| 0x0D | `GET_DIAG` | §3.8. |
| 0x0E | `HORN_TEST` | Payload: `duration_ms` u16 LE (100..5000). |

Unknown opcodes → `UNSUPPORTED` (6).

**Golden vector (v2.2, `GET_CAPS` reqId 1, empty payload):**

```
52 53 48 43 01 01 00 00 00 77 e7
```

### 3.4 GET_CAPS reply body

Reply payload after the 1-byte `status`:

```
boardMac[6] | fwMajor | fwMinor | fwPatch | protoMajor |
protoMinor | nAxesMax | nBtnMax | nEncMax | wifiChannel |
capsFlags | pinCount | pin table (8 B × N)
```

Pin table entry (8 B): `pin | kind | module | caps |
reserved[4]`. Kind codes: 0 NONE, 1 `ADC_NATIVE`, 2 `ADC_ADS1115`,
3 `DIGITAL_IN`, 4 `DIGITAL_OUT`, 5 `LED_STRIP`, 6 `I2C_SDA`,
7 `I2C_SCL`, 8 `EXPANDER_MCP23017`, 9 `EXPANDER_PCF8574`,
10 `AXIS_GENERIC`, 11 `RESERVED_PIN`, 12 `AXIS_AS5600`.
Module codes: 0 NONE, 1 STEERING, 2 ACCELERATOR, 3 BRAKE,
4 CLUTCH, 5 HANDBRAKE, 6 SHIFTER, 7 HORN, 8 BUTTONS, 9 ENCODERS,
10 LED, 11 BATTERY. Max 40 pin entries. `caps` bits: bit0 input,
bit1 output, bit2 adc, bit3 pwm, bit4 pullup; bits 5–7 reserved 0.

**`capsFlags` bits (R3-FW-06c / R3-FW-04):** bit0 `slowAxis` —
a slow (ADS1115) axis exists (≤ 50 Hz); bit1 `safeMode` — the
board is in safe mode. Bits 2–7 reserved 0. **Bit0 is set in
4.0.4+ whenever an ADS1115 axis is live** (Round 3 WS-3, FW-01
+ FW-04 — `net.cpp` GET_CAPS handler, `rsh_pinmap_has_slow_axis`);
in 4.0.3 it was never set (I2C axes validated but never sampled).
The app reads it (`caps.rs` `slow_axis()`) and must treat 0 as
"no slow axis".

**I2C axes (R3-FW-01, implemented in firmware 4.0.4):** `AXIS_AS5600`
(type 12) and `AXIS_ADS1115` (type 2) records are sampled after
COMMIT — the axis is live, not just validated. Pin-table rows for
I2C axes carry `pin` 0xFF (no GPIO) with kind 2 (`ADC_ADS1115`) /
12 (`AXIS_AS5600`) and the axis module code. ADS1115: I2C address
0x48 fixed, channel = record `params[0]` (0–3, validator rejects
> 3 and duplicate channels), single-shot, PGA ±6.144 V, 128 SPS,
polled on a 20 ms cadence (≤ 50 Hz per axis, round-robin across
channels — this is what `capsFlags` bit0 reports). AS5600: I2C
address 0x36 fixed, `flags` bit0 = multi-turn (0 = ≤ 360° direct,
1 = turn counter on), `params[1]` = rotation (0 = 360°, 1 = 540°,
2 = 900°), max 1 per board, sampled every cycle; I2C failure or
magnet-not-detected keeps the last value and sets the axis-invalid
flag (never sends 0). Both produce millivolt values (0..3300 mV).
Expander records (`EXP_MCP23017` / `EXP_PCF8574`) are **rejected
at COMMIT** (firmware 4.0.4+, `UNSUPPORTED`, err 3) — expander-button
sampling is not implemented, so accepting them would leave silently
dead buttons. The app rejects them first with a plain message
(`validate_blob` in `rsh-protocol`); the board rejection is the
backstop for old apps / direct `SET_CFG`.

**`nAxesMax` / `nEncMax` are wire ceilings, not implemented
counts (R3-FW-07):** `nAxesMax` 8 / `nEncMax` 4 are the protocol's
maxima, but the firmware can ever emit at most **5 axes** and
**2 encoders** (§1.3/§1.4). Do not size UI slots from these
fields — slots beyond 5 axes / 2 encoders would stay dead
forever.

### 3.5 Config transaction (GET_CFG / SET_CFG / COMMIT / CONFIRM / REVERT)

Unchanged from v2.1: chunked `SET_CFG` into staging, `COMMIT`
stages, `CONFIRM` applies within the 30 s window, `REVERT`
discards. Only the auth wrapping is gone — the transaction
itself is plain.

### 3.6 SET_PASSWORD (0x0C, D3)

Payload (exact length enforced):

```
curLen u8 | currentPw[curLen] | newLen u8 | newPw[newLen]
```

- **No password set yet** (open AP, first setup): `curLen` is
  `0` — the command is allowed without a current password.
- **A password is set:** `currentPw` **must** match the stored
  password, or the board replies `BAD_PASSWORD` (1). This closes the
  worst hole: a random AP joiner cannot lock the owner out by
  setting a password first — but only after one is set. Before
  that, the open-AP window is the exposure (§9). Shape or
  new-password failures reply `UNSUPPORTED` instead.
- **New password:** 8–63 printable ASCII (`0x21..=0x7E`),
  validated on the board (`password.h` rules, unchanged).
  Anything else → `UNSUPPORTED`.
- On success the board restarts its AP WPA2-PSK with the new
  password; the PC writes the KeyStore `:wifi` entry **and**
  updates the WLAN profile in the same transaction, then
  reconnects. The reply body is empty (status only).

### 3.7 REBOOT / FACTORY_RESET — unauthenticated, accepted risk

Both are accepted from anyone on the AP, with no credential.
Once a Wi-Fi password is set, the WPA2 password is the only
boundary; on an open AP (the pre-password setup window only)
anyone in range can send them. `FACTORY_RESET` clears the
password + config and the board reboots to an open AP. This is
recorded in the threat model (THREAT-MODEL-RUST TM-W08) — it is
an accepted risk, not an oversight.

### 3.8 GET_DIAG (0x0D)

Reply body (max 230 B = 6 + 32×7):

```
[0]     boot_reason u8 (1 power-on, 2 brown-out, 3 watchdog,
        4 software, 0 reserved)
[1..2]  heap_min_kb u16 LE (lowest free heap since boot, KB)
[3..4]  reserved u16 LE — ALWAYS 0 (v2.2 D10: the old
        `window_remaining_s` field stays in the layout as
        reserved/zero; it means nothing)
[5]     event_count u8 (0..32, oldest-first)
[6..]   one event per 7 B: ts_ms u32 LE | code u8 | value u16 LE
```

Diag event codes: `AP_LOCKED` (3), `WINDOW_OPEN` (4),
`PAIRED` (5), `KEY_RESET_ARMED` (11), `KEY_RESET_EXPIRED`
(14) are **deleted** with the pairing window / armed state
(D10). `KEYSTORE_NVS_FAIL` (16) is kept.

**Live event codes (R3-FW-06d)** — the full set the board can
emit (`firmware/diag_log.h`; wire values never change):

| Code | Name | Value |
|---|---|---|
| 1 | `BOOT` | reset reason code (§1.2) |
| 2 | `AP_UP` | channel |
| 6 | `HELO_REJECT` | 0 = live-session takeover attempt, 1 = peer-change throttle |
| 7 | `HEAP_LOW` | free heap KB at the new minimum |
| 8 | `CFG_ROLLBACK` | 0 (confirm timeout / boot recover, incl. factory fallback) |
| 9 | `OTA_FAIL` | 0 (WebOTA upload fail / abort / timeout) |
| 10 | `RESCAN` | new channel (F-07) |
| 12 | `FW_ROLLBACK` | 0 (6 consecutive boot fails → other OTA slot) |
| 13 | `AP_RESTART_FAIL` | 0 = AP lock, 1 = new-password restart, 2 = rescan restart |
| 15 | `WDT_STATE` | 0 = loop task subscribed now, 1 = already subscribed, 2 = subscribe failed |
| 16 | `KEYSTORE_NVS_FAIL` | 0 (keystore NVS write fail) |
| 17 | `PASSWORD_CLEARED` | 0 (BOOT ≥ 10 s hold cleared the password; replayed from NVS after the reboot) |

### 3.9 Reply status codes

| Code | Meaning |
|---|---|
| 0 | `OK` |
| 1 | `BAD_PASSWORD` — SET_PASSWORD current password wrong (§3.6). (v2.1's `BAD_TAG` was deleted; code 1 was reused for this, §0.) |
| 2 | `REPLAY` — wire value kept so numbering does not shift; the dispatcher never sends it (a duplicate is answered from the cache, §2.3). |
| 3 | `BAD_CRC` |
| 4 | `TOO_BIG` (`len > 1024`, chunk `offset + len > total`, total > 2048) |
| 5 | `PIN_CONFLICT` |
| 6 | `UNSUPPORTED` |
| 7 | `BUSY` |
| 8 | `NVS_FAIL` — SET_PASSWORD synchronous NVS write failed (firmware 4.0.6, F-07): the password was NOT changed; retry or keep the old one. |

## 4. Ping (NTP-style, 34 B fixed)

`"RSHN" | ver 0x22 | type u8 (0 request, 1 reply) | seq u16 |
t0 u64 | t1 u64 | t2 u64 | CRC16`. All timestamps µs; `t0` is
the PC monotonic send time, `t1`/`t2` are board monotonic
receive/send times. `t3` (PC receive time) is never on the wire.
`delay = (t3 − t0) − (t2 − t1)`,
`offset = ((t1 − t0) + (t2 − t3)) / 2`. A reply is accepted
only when magic/version/type/length/CRC are valid, `seq` and
`t0` match the request, and `t2 ≥ t1`, `t3 ≥ t0`, `delay ≥ 0`.
Ping timestamps are measurement only — they must never drive an
auth / replay / expiry decision.

## 5. KEY_RESET — physical only (D7)

There is **no wire KEY_RESET anymore** (opcode 0x0B deleted,
§3.3). To clear the password:

1. Hold the board's BOOT button for **≥ 10 s** (LED indication
   during the hold).
2. At the threshold the firmware **deletes the NVS password
   itself** and reboots into an open AP.
3. The user repeats first-connect (§8) and sets a fresh
   password.

No 120 s armed window, no app-sent command, no app needed at
all — recovery works with no app (forgot password → hold BOOT
10 s → open AP → set new password). The 5 s safe-mode hold is
unchanged. The 3 s pair-button hold is dead input and removed.

## 6. Connection state machine (app side)

```
Idle → LinkUp → Hello → Active
  ▲        │           │
  └────────┴── Error (reason, hint, retry) ←──┘
       Backoff (2 s → 30 s) ←── Active (no frames 2 s / transport lost)
```

- **`Active` = unsigned `GET_CAPS` OK.** That is the only
  "Connected" state. There is no `Authenticating` state and no
  pairing-gated step.
- **Wizard Step 1 unlock** is on `board_reachable` (a sticky
  latch set on every HELLO observed, cleared on disconnect /
  transport switch / `NoRoute` / HELLO-timeout) — HELLO-or-better,
  never on `Active` alone.
- **Stream gate (SEC-E-14, redefined):** stream frames are
  emitted only after a plain `GET_CAPS` OK in this session.
- **Errors (kept):** `Unreachable`, `Timeout`,
  `TransportClosed`, `BindFailed` (typed kind), `BoardMismatch`
  (HELLO board MAC does not match the board this session is
  bound to — the `RSH-XXXX` SSID is per-board), `WatchdogStall`,
  `Unknown`. Nothing latches on a pairing reason anymore —
  every remaining failure retries with backoff (except
  `AddrInUse` and the watchdog latch).
- **Board identity (D11):** the app binds to the board it joined
  (SSID `RSH-XXXX` is per-board; HELLO `board_mac` must match
  the SSID suffix). Multi-board: the user picks an SSID from the
  scan list.

## 7. WebOTA (firmware update over HTTP)

- **Disabled until a password is set (D9):** a 4.0.3 board with
  no password set (fresh, or after KEY_RESET) does **not** serve
  WebOTA — every request gets `401`, and an empty password never
  authenticates. The app's firmware-update flow prompts "set a
  Wi-Fi password in Settings first".
- **Auth password = the user-set AP password** (one credential
  for Wi-Fi and WebOTA). HTTP Basic Auth on **every** handler
  (root, `/update`, restart, not-found included); **per-IP**
  rate limit (5 wrong attempts from one IP → 60 s lockout **for
  that IP only** — other IPs are unaffected; table holds 4 IPs =
  max stations, `firmware/webota_auth.h`). The lockout is itself
  an accepted tradeoff, but it is per-IP, not global — one
  station cannot lock everyone else out (R3-FW-06d). The
  password check itself is the Arduino WebServer's
  `authenticate()` (no separate constant-time compare in
  firmware code); per-boot random CSRF token (`X-RSH-CSRF`) on
  state-changing requests.
- **Image checks (unchanged):** Ed25519 signature verified
  **before apply** (app verifies before upload, board verifies
  again after upload), anti-downgrade (only equal or newer
  version accepted), size/magic check (magic `0xE9`, size cap =
  app partition), stalled-upload DoS guard. The image carries a
  68 B `RSHS` trailer; the signature is over the SHA-256 digest
  of the image, not the raw image.
- WebOTA stays on HTTP (local AP, no TLS — a per-device
  certificate is not feasible).

## 8. Migration (firmware ≤ 4.0.2 → 4.0.3, D6)

1. Update the **app first** to v2.0.6 (it still parses old
   HELLOs, §0.1, and still holds the old stored Wi-Fi password
   for WebOTA).
2. Flash 4.0.3 via WebOTA using the **old stored Wi-Fi password**
   (Credential Manager entry `board:<MAC>:wifi` — do **not**
   delete it before the flash, or the board is stranded).
3. 4.0.3 first boot deletes NVS `devkey` + `paired` + `wifipw2` +
   `wifipw` (all old secrets — keeping `wifipw2` would bring
   the AP up WPA2 with an unknown generated password,
   contradicting "AP open until a password is set"). The AP
   comes up **open**.
4. App migration on first v2.0.6 run: delete every
   `board:<MAC>` **device-key** entry under service `RSH` at
   daemon startup. Keep `:wifi` entries until the board reports
   firmware ≥ 4.0.3, then delete them too (the user sets a fresh
   password, stored per §9).

## 9. Security boundary (summary — see THREAT-MODEL-RUST.md)

**The WPA2 Wi-Fi password is now the only security boundary.**
Anyone who knows it can control the board and trigger WebOTA.
Anyone on the open AP (before a password is set) can send
control frames (`REBOOT` / `SET_CFG` / `FACTORY_RESET`) and set
a password first (locking the owner out until a BOOT-hold
KEY_RESET). Mitigations: `SET_PASSWORD` requires the current
password once one is set (§3.6); KEY_RESET is physical-only
(§5); WebOTA is disabled until a password is set (§7); the
open-AP window should be as short as possible (the wizard
prompts for a password right after first connect). The board
password is obfuscated in NVS with a fresh random NVS key
generated once at first 4.0.3 boot (never the deleted device
key). There is no per-device key, no HMAC, no replay window —
a captured frame can be replayed within the reqId-dedup scope;
accepted as a LAN-only risk behind WPA2.
