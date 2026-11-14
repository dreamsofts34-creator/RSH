# RSH — Firmware Architecture

> **Versions:** Firmware **v4.0.1** · App **v2.0.4** · Protocol **v2.1**
> (2026-11-14). System overview: [`ARCHITECTURE.md`](ARCHITECTURE.md).
> Full wire contract: [`PROTOCOL-V2.1.md`](PROTOCOL-V2.1.md).

## Board and toolchain

- **Board:** ESP32 DevKit V1 (original ESP32, WROOM module).
- **Framework:** Arduino via **PlatformIO** (`firmware/rsh/platformio.ini`).
  Environments: `esp32dev` (normal) and `esp32dev-secure` (signed /
  secure-boot build).
- **Build:** `pio run -e esp32dev` from `firmware/rsh/`.
  The merged factory image is produced by `merge_bin.py`
  (bootloader + partitions + app at `0x0`).
- **Cores:** Core 1 runs the 200 Hz sampling loop; Core 0 runs
  Wi-Fi / WebServer / OTA (bounded to ≤ 300 µs per pass).

## Design rules

- **Firmware dumb, PC smart.** The board samples, packs and sends.
  No calibration, curves or game logic on the board.
- **No heap allocation in the sampling loop.** Buffers are static
  (BSS); the loop-task stack is 8 KB. Heap health is watched
  (largest free block < 8 KB for 10 consecutive cycles, or a > 20%
  drop in 5 minutes, triggers a safe restart — see `heap_policy.h`).
- **Fail closed.** An unsupported request returns `UNSUPPORTED`,
  a bad pin map is rejected at validation, a bad firmware image is
  rejected before apply. Nothing silently degrades into an unsafe
  state.

## The 200 Hz loop

`rsh.ino` — one tick every 5 ms (`nextTickUs` driven):

1. Sample BOOT-hold, tick the boot counter / safe mode / config
   transaction state machine.
2. Read inputs (`inputs.cpp`): ADC1 axes in mV, digital buttons
   (debounced), encoders (cumulative i16 deltas — 200 turns, 0 drift),
   battery if present.
3. Pack a protocol v2.1 stream packet (`stream_emit.h`) and send it
   over UDP (port 4211) or serial, at 200 packets/s.
4. Once per second, send the v2.1 status packet (34 B, or 38 B
   extended): heap, uptime, seq gaps, TX failures, reset reason.
5. Track timing: `overrunCount` and `maxCycleUs` are kept on-board
   for the A7 measured run (pass: sent/s ≈ 200, overruns 0,
   maxCycleUs < 3500 µs).

An adaptive sampling governor sheds work if cycles start overrunning
instead of letting the loop fall behind silently.

## Source map (`firmware/rsh/`)

| File | Job |
|---|---|
| `rsh.ino` | Main loop, tick scheduling, boot/safe-mode/config-txn ticks |
| `config.h` | All settings in one place (pins, Wi-Fi, versions, limits) |
| `protocol.h` | Protocol v1 + v2.1 packet encode/decode, CRC16-CCITT, control opcodes |
| `inputs.cpp` / `inputs.h` | ADC/buttons/encoders sampling, debounce, floating-sensor detection |
| `modules.h` | Module table: which pin does what, capability flags |
| `pinmap.h` | Runtime pin map (applied from the config blob) |
| `config_transaction.h` | Config transaction state machine + PIN-RULES blob validator |
| `net.cpp` / `net.h` / `net_logic.h` | Wi-Fi SoftAP, UDP stream/HELO/control, mDNS (`rsh.local`) |
| `serial_link.cpp` / `serial_transport.h` | Wired USB-serial transport, `RSHS` frames, 921600 baud |
| `stream_emit.h` | v2.1 stream packet emission |
| `ctrl.h` / `ctrl_handlers.h` | Control-channel command handlers (GET_CAPS … SET_PASSWORD) |
| `keystore.cpp` / `keystore.h` | Per-device key + nonce in NVS (first-boot random 32 B) |
| `webota.cpp` / `webota.h` | Browser Web OTA page (auth, lockout, CSRF, progress UI) |
| `ota_validate.h` / `sign_firmware.py` / `sign-image.py` | Ed25519 verify-before-apply, anti-downgrade, signing |
| `signing_pubkey.h` | Baked Ed25519 public key (fingerprint must match the release key) |
| `safemode.h` | Safe mode: BOOT-hold + NVS boot counter |
| `heap_policy.h` | Heap health policy (D17) |
| `led_driver.h` / `led_sink_rmt.*` / `ws2812_rmt.h` / `led_patterns.h` | WS2812 LED strip driver (RMT), patterns, 60 s test cap |
| `as5600.h` | AS5600 magnetic encoder (≤ 360°) |
| `encoders.h` | Quadrature / cumulative encoder handling |
| `status_led.cpp` / `status_led.h` | Onboard LED (GPIO2) patterns |
| `password.h` / `pw_obfuscate.h` | Wi-Fi password storage (obfuscated; real fix is flash encryption — see below) |
| `secure_nvs.h` / `sdkconfig.secure.defaults` / `partitions-secure.csv` | Flash-encryption / Secure Boot v2 (RSA-3072) build |
| `fw34_state.h` | v3.4-era state carried forward (boot counter, config slots) |

## Networking

- **SoftAP:** SSID `RSH`, address `192.168.4.1`, up to 4 stations,
  automatic channel selection. mDNS hostname `rsh` → `rsh.local`.
- **Ports:** HELO **4210** (PC → board, ~1 Hz), stream **4211**
  (board → PC, 200 pps), control **4212**. If no HELO arrives for
  2000 ms, the board goes silent.
- **Wired:** USB-serial at 921600 baud, `RSHS` magic on ping frames,
  same protocol. Serial is the transport when `RSH_WIRED_SERIAL` is 1
  (debug prints are off then — serial carries only frames).
- **OTA:** Web OTA only. **ArduinoOTA is disabled** (`RSH_OTA 0`).

## Pins and pin rules

Default map (`config.h`):

| Function | GPIO |
|---|---|
| Steering potentiometer | 34 (ADC1) |
| Accelerator hall | 35 (ADC1) |
| Brake hall | 36 (ADC1) |
| Buttons (accel / brake) | 25 / 26 (INPUT_PULLUP) |
| Onboard status LED | 2 |
| WS2812 strip data | 21 |

Rules (enforced by the validator in `config_transaction.h` and by
`tests-host/pin_rules_ref.py` — single source of truth,
[`PIN-RULES.md`](PIN-RULES.md)):

- Analogue inputs **only on ADC1** (32, 33, 34, 35, 36, 39) — ADC2
  is unusable while Wi-Fi is on.
- GPIO 20/24/28–31 and 37/38 are forbidden; input-only pins cannot
  drive an encoder, button or LED; I2C pins 21/22 are reserved when
  an expander is used.
- Limits: axes ≤ 8, buttons ≤ 128, encoders ≤ 4, LED strips ≤ 1
  (≤ 60 LEDs), batteries ≤ 1.
- GPIO25/26 aliasing is guarded (`rsh_pinmap_uses_pin()`).

The runtime pin map comes from the PC's config blob (see
[`ARCHITECTURE.md`](ARCHITECTURE.md#configuration-stage--commit--confirm--rollback));
the compile-time `PIN_*` values are only the defaults.

## Safe mode and config transactions

- **Safe mode:** holding BOOT at power-on (or a boot counter
  threshold after repeated failed boots) puts the board in safe mode:
  it comes up with defaults, no user config, so a bad config can
  never brick the board. Recovery steps: [`BRICK-RECOVERY.md`](BRICK-RECOVERY.md).
- **Config transaction:** `stage → commit → confirm → rollback`.
  The PC stages a blob, the board validates it against PIN-RULES,
  commits to NVS, and rolls back automatically if the PC does not
  confirm within 30 s. The last-good config is always kept.
- **Key reset:** `KEY_RESET` (0x0B) needs a BOOT-hold; `FACTORY_RESET`
  does **not** wipe the per-device key.

## Signed OTA

1. The PC (or the Web OTA page) uploads a firmware image.
2. The board verifies the **Ed25519 signature before applying**
   (`ota_validate.h`); a bad signature or a downgrade is rejected.
3. Dual-partition layout keeps the previous image as fallback.
4. The signing key lives **outside this repo**; the release workflow
   signs via the `RSH_SIGNING_KEY_PEM` Actions secret. The baked
   public key is in `signing_pubkey.h`.
5. The app verifies the signature itself before uploading
   (NSec.Cryptography) — a bad image never reaches the board.

See [`SIGNED-OTA.md`](SIGNED-OTA.md) and [`SIGNING.md`](SIGNING.md).

## Flash encryption and Secure Boot (board-side, pending)

The secure build (`esp32dev-secure`, RSA-3072 Secure Boot v2 +
flash encryption, encrypted NVS) is prepared in the repo
(`sdkconfig.secure.defaults`, `partitions-secure.csv`,
[`FLASH-ENCRYPTION-PROCEDURE.md`](FLASH-ENCRYPTION-PROCEDURE.md)),
but the eFuse steps can only run on the real board — they are part of
the A7 run ([`A7-RUNBOOK.md`](A7-RUNBOOK.md)). Until then, the Wi-Fi
password in NVS is only obfuscated, not encrypted — a known, accepted
limitation documented in the security reviews.

## Host tests

`tests-host/` runs the firmware logic on the PC (plain C++17, no
Arduino): protocol vectors, pin rules, config transactions, heap
policy, LED driver, AS5600, merge-bin. **4430 checks, 0 failures**
at the v2.0.4 gate. Build: `tests-host/run_tests.sh`.
See [`TESTING.md`](TESTING.md).
