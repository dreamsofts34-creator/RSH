# RSH — Protocol v2.1 (readable summary)

> **Versions:** Protocol **v2.1** · App **v2.0.4** · Firmware **v4.0.1**
> (2026-11-14).
> **This is a summary. The authority is
> [`PROTOCOL-V2.1.md`](PROTOCOL-V2.1.md) (1163 lines) — if this file
> and that file disagree, the v2.1 spec wins.**
> Golden vectors: `tests-host/protocol_v2_vectors.py`
> (51 checks, C# / C++ / Python cross-verified).

## Why v2.1 exists

Protocol v1 (18-byte packet, fixed pins, no authentication) could not
carry a modular wheel: no encoders, no config channel, no security.
v2.1 adds a stream format for up to 8 axes / 128 buttons / 4 encoders,
a 92-byte HELLO, an authenticated control channel, a status packet,
and an NTP-style ping — while still recognising v1 packets (legacy
detection by version byte) so an old board is diagnosed, not
mis-parsed.

## Transports and ports

| Direction | Port / link | Content |
|---|---|---|
| PC → board | UDP **4210** | HELO (~1 Hz) |
| Board → PC | UDP **4211** | Stream, 200 packets/s |
| PC ↔ board | UDP **4212** | Control channel (authenticated) |
| PC ↔ board | USB-serial, 921600 baud | Same frames, `RSHS` framing |

If no HELO arrives for 2000 ms, the board goes silent.
mDNS: the board announces `rsh.local` (`192.168.4.1` also works).

## Frames at a glance

| Frame | Magic / version | Size |
|---|---|---|
| v1 data (legacy) | `0xA5`, version `1` | 18 B |
| v2.1 stream | `0xA6`, version byte `0x21` | header 20 B + payload |
| v2.1 HELLO reply | — | 92 B |
| v2.1 status | — | 34 B (38 B extended) |
| v2.1 control | header + HMAC-SHA256 tag | header 13 B, tag 16 B, payload ≤ 1024 B |
| v2.1 ping (NTP-style) | — | 34 B |

All integers are **little-endian**. Integrity is **CRC16-CCITT**
(poly `0x1021`, init `0xFFFF`) over every byte before the CRC field.
Stream values are in **mV** for axes (0–3300 mV domain); encoders are
**cumulative i16** deltas.

## Stream packet

- Sequence (u16, wraps — the PC's `SequenceTracker` is reboot-safe:
  after a board reboot the baseline is re-established instead of
  counting ~1000 valid packets as duplicates).
- ESP timestamp (µs), axes (≤ 8, mV), buttons (wire ≤ 64, config ≤ 128),
  encoders (≤ 4, cumulative), flags: config-changed, low-supply,
  tick-skipped, axis-invalid.
- The status packet (1 Hz) adds: heap, uptime, seq gaps, TX failures,
  reset reason, CRC.

## HELLO (92 B)

The board's reply to HELO. Flags: unpaired, key-reset pending,
safe mode, pairing window open. It also carries the board's
capabilities pointer and the Wi-Fi password field — which the PC
**must** save to its key store (a v2.0.4 fix: dropping it meant the
PC could not rejoin the board's WPA2 network).

## Control channel

Authenticated with **HMAC-SHA256** (per-device key, nonce + counter
— replay protection; a retry re-uses the same `reqId` with a new
counter). Error codes: `OK = 0`, `BAD_TAG = 1`, `REPLAY = 2`,
`BAD_CRC = 3`, `TOO_BIG = 4`, `PIN_CONFLICT = 5`,
`UNSUPPORTED = 6`, `BUSY = 7`.

| Opcode | Command | Notes |
|---|---|---|
| `0x01` | GET_CAPS | What the board can do |
| `0x02` | GET_CFG | Read staged/committed config |
| `0x03` | SET_CFG | Stage a config blob (PIN-RULES validated) |
| `0x04` | COMMIT | Commit staged config to NVS |
| `0x05` | CONFIRM | Confirm within 30 s, else rollback |
| `0x06` | REVERT | Roll back to last-good config |
| `0x07` | PIN_PROBE | Probe a pin live |
| `0x08` | FACTORY_RESET | Resets config — **does not wipe the key** |
| `0x09` | REBOOT | Only via HMAC (the old unauthenticated `RSTX` is gone) |
| `0x0A` | LED_TEST | LED strip test (60 s cap) |
| `0x0B` | KEY_RESET | Needs a BOOT-hold |
| `0x0C` | SET_PASSWORD | Wi-Fi password change |

## Upgrade order

**App first, then firmware.** The app understands both v1 and v2.1;
a v2.1 board talking to a v1-only app is not supported. Firmware
v4.0.1 is **not** byte-identical to v4.0.0 — boards on v4.0.0 must be
reflashed (see [`FLASH-GUIDE.md`](FLASH-GUIDE.md)).

## Known gaps (honest)

- Ex3 buttons 97–128 have a protocol gap (documented in the v2.1
  spec; needs a v2.2 decision).
- Accepted-but-dead config records: AS5600 / ADS1115 / expander /
  matrix / battery records validate and commit, but no firmware
  consumer exists yet for some of them — either drivers land or
  COMMIT rejects them (decision open for v2.0.5).
