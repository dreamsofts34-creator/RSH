# Firmware v4.0.2 — Gate (F-01..F-16)

**Date:** 2026-11-16
**Repo:** `dreamsofts34-creator/RSH` (local HEAD — see §9)
**Scope:** `firmware/*` + this doc + `docs/FIRMWARE-COMPAT.md` (§2.1 v4.0.2)
**Verdict: GATE PASS** (secure env: see §6 — build PASS; board verification of the secure image is OPEN, see §8)

Protocol v2.1 is FROZEN — no wire-format change in this release. All new
features are additive (new opcodes 0x0D/0x0E, new config record type 13,
new blob fields). Old app ↔ new firmware and new app ↔ old firmware both
work (compat matrix in `docs/FIRMWARE-COMPAT.md` §2.1).

## 1. What was built — per F-item

| Item | Pri | What | Where |
|---|---|---|---|
| F-01 | High | AP lock after pairing — deferred one loop iteration after the GET_CAPS reply was sent, then WPA2 lock. Diag event `AP_LOCKED` value 1 (paired) vs 0 (window expiry, old behaviour). | `net.cpp` (`net_lock_ap_wpa2_`, `net_poll_ctrl`, `net_lock_ap_after_pairing_window`) |
| F-02 | High | Pair button — BOOT held ≥3 s and released <5 s → `PAIR_WINDOW_OPEN` (unpaired board only, 120 s window, 3 Hz LED blink while open). Thresholds reorganised 3/5/10 s (3 s pair, 5 s safe mode, 10 s key reset). Paired board ignores the 3 s hold. | `safemode.h` (`RSH_BOOTHOLD_PAIR_MS 3000`, `RSH_HOLD_PAIR_WINDOW`), `rsh.ino` (`fw34_loop_tick`) |
| F-03 | Med | Transport-independent window — `RshPairWindow` deadline (`open_until_ms`), not AP-up clock. Serial HELO on an unpaired board opens/refreshes the 120 s window on demand (physical access = trust anchor). | `pairing_window.h`, `keystore.{h,cpp}`, `net.cpp` (`net_serial_helo`) |
| F-04 | Med | HELLO reply rate limit 500 ms → **250 ms**. App side waits 700–800 ms (daemon worker owns app side). | `net_logic.h` |
| F-05 | Med | Same-IP session limit relaxed — 10 s new-session reject applies only to a DIFFERENT IP; re-HELO from the same PC after the old session is >2 s stale (HELLO timeout) is allowed. | `net_logic.h` (`rsh_new_session_allowed`), `net.cpp` |
| F-06 | Med | Sampling + packet-build in a dedicated FreeRTOS task (core 1, priority 2, `vTaskDelayUntil` 5 ms/governor period); SEND stays in the loop task (WiFiUDP is not thread-safe). Single-slot mailbox, latest wins, overwrite counted into `seqGap`. Config apply via `cfg_dirty` flag — per-loop `memcmp` of the ≤2 KB blob REMOVED. Task-create failure → legacy in-loop sampling fallback. | `sample_task.h`, `rsh.ino` (`sample_task_fn`, `sample_once_`, `sample_send_frame_`, `sample_task_start_`) |
| F-07 | Med | Channel rescan only when no PC is active; **default OFF** (`RSH_AUTO_CHANNEL 0`). | `config.h`, `net_logic.h` (`rsh_rescan_allowed`), `net.cpp` |
| F-08 | Med | WebOTA lockout per-IP (not global) — 5 fails → 60 s lock for that IP only, max 4 IPs, LRU eviction. | `webota_auth.h`, `webota.cpp` |
| F-09 | Low | Secrets at rest — default image UNCHANGED (no silent change). Secure build (`RSH_SECURE_NVS`) compiles + host-tests. D-11 decision note in §6. | `platformio.ini` (`esp32dev-secure`), `firmware/CMakeLists.txt`, `sdkconfig.secure.defaults` |
| F-10 | Low | Ring-buffer diag log (32 events) + `GET_DIAG` (0x0D) control command: boot reason, heap min, `window_remaining_s`, event log. LED error codes (blink count: heap-low 2, config rollback 3, net fail 4, OTA fail 5). | `diag_log.h`, `status_led.{h,cpp}`, `net.cpp`, `rsh.ino` |
| F-11 | Low | 3V3 monitor documented as UNMEASURED — no free ADC1 pin (32/33/34/35/36/39 all used); `supply_mv` stays 0, `battery_pct` 255. No code change. | `docs/FIRMWARE-COMPAT.md` §2.1 |
| F-12 | Low | Single version header `version.h` — `RSH_FW_VERSION_*` (4/0/2) is the only source; `FIRMWARE_VERSION` and `RSH_CAPS_FW_*` both alias it; `static_assert` triple fits u8. `merge_bin.py` fixed to resolve the alias (was parsing a literal `#define FIRMWARE_VERSION "..."` that no longer exists — merged image was named `vunknown`). | `version.h`, `config.h`, `modules.h`, `merge_bin.py` |
| F-13 | Low | Single-PC model documented ("Observer" mode later, not implemented). | `docs/FIRMWARE-COMPAT.md` §2.1 |
| F-14 | — | Persistent LED config record in the config blob (type 13: pattern id 0..4, RGB, brightness 0..100, slot 0, max 1). LED_TEST stays separate and time-limited (D-6). | `config_transaction.h`, `led_driver.h`, `net.cpp` (`net_led_apply_persistent`) |
| F-15 | — | Handbrake button-mode (D-3): axis slot-4 record flags bit0 → digital input + pullup, 4095/0; GET_CAPS kind flips to DIGITAL_IN. Axis mode unchanged. | `config_transaction.h`, `pinmap.h`, `inputs.cpp`, `modules.h` |
| F-16 | — | Window/pairing state in HELLO: **NO ROOM** — HELLO byte 17 is reserved and must be 0 (verified in `firmware/protocol.h` and `crates/rsh-protocol/src/hello.rs`: nonzero → reject). Protocol frozen, so flag-only; app uses a conservative countdown. Real `window_remaining_s` is exposed via GET_DIAG (0x0D) instead. | `diag_log.h`, `net.cpp` |
| D-4 | — | Shifter sensitivity field (0..100, default 50) in the SHIFTER record params[4]; debounce/stability time 20 ms (0) → 5 ms (100); **default 50 → 13 ms** (formula 20 − sens·15/100). **Doc correction found during this work:** old code comment claimed 50 → 12 ms and `inputs.cpp` hard-coded 12; formula is the authority — fixed to 13 in code + comment. | `config_transaction.h`, `inputs.cpp` |
| D-5 | — | `HORN_TEST` command (0x0E, len 2, duration 100..5000 ms) — horn output forced ON for the duration, then restores. | `horn_test.h`, `protocol.h`, `net.cpp`, `rsh.ino` |
| D-6 | — | LED_TEST stays separate and time-limited (60 s cap unchanged); persistent pattern takes over in the same tick when a test ends (no all-off flash). | `led_driver.h` |

## 2. Host tests

- Command (from `firmware/`):
  `g++ -std=c++17 -O1 -g -Wall -Wextra -Werror -fsanitize=address,undefined -I.. -o /tmp/run_tests firmware/tests-host/test_main.cpp /usr/lib/x86_64-linux-gnu/libsodium.so.23`
  (libsodium: no `-dev` symlink in this sandbox — link the versioned lib directly; `-lsodium` fails.)
- **ALL TESTS PASSED — 4705 checks, 0 failures** (baseline 4444 + 261 new §34 checks).
- §34 (new, firmware v4.0.2) covers: pair-button thresholds (2 s/2.999 s NONE; 3 s+release PAIR_WINDOW; 4.999 s release PAIR_WINDOW; 5 s SAFE_MODE + release NONE; 10 s KEY_RESET_ARMED; re-press; wrap-around at 0xFFFFFF00); pair window (boot 60 s / button 120 s boundaries, remaining_s, refresh extends, paired no-op, wrap); HELLO 250 ms boundary (249→0, 250→1), new-session matrix, rescan gate; WebOTA per-IP lockout (4 fails OK, 5th locks only that IP, other IP unaffected, 60 s expiry, success resets, LRU eviction); LED config record (valid SOLID 10/20/30/50%; factory blob → 0; negatives: pattern 5, brightness 101, pins set, slot≠0, duplicate, type 14); handbrake button mode (pin 25 button → DIGITAL_IN + PULLUP, no ADC cap; pin 32 axis → ADC_NATIVE; negatives flags 0x03, button pin 34; factory → axis); shifter sensitivity (75, default 50, >100 reject, debounce mapping 0→20, 25→17, 50→13, 75→9, 100→5); dirty flag (load→1; stage→0; commit→1; timely confirm→0; revert/tick-at-deadline/late-confirm/boot-recover→1; **tick boundary is deadline-exact: tick(deadline−1)==0, tick(deadline)==1**); diag log (heap min, body layout oldest-first, ring wrap 35 events → count 32 starting at event #4, blink pattern 3 blinks 150 ms + 1 s pause = 1900 ms cycle); horn test (99/5001/0 reject, 100/5000 OK, expiry, restart resets); sample mailbox (produce/consume, overwrite skipped=1, oversize len→0, NULL guards); version single-source (triple 4.0.2, string parse matches CAPS, opcodes 0x0D/0x0E, GET_CAPS still 0x01); LED driver persistent (brightness 50% → 255→128; 0→0; 100→255; >100 clamps; test overrides persistent, persistent resumes SAME tick after test ends with no off flash; no-persistent → all-off once; OFF pattern clears).
- Pre-existing tests updated for v4.0.2 (intentional): unknown-type test type 13 → 14 (type 13 is now valid LED_CONFIG); HELLO throttle test 499/500 ms → 249/250 ms (F-04); three 4.0.1 version checks → 4.0.2.
- Python cross-checks: `protocol_v2_vectors.py` OK; `verify_packet.py` "PYTHON CROSS-VERIFY PASSED — 51 checks".
- NOT host-testable (wiring only, covered by `pio run` + real-board HIL §8): AP lock after GET_CAPS reply, serial HELO window open, LED PairWindow state, horn OR into the stream, sample-task FreeRTOS behaviour, WebOTA per-IP with real stations.

## 3. pio builds (default env)

Toolchain: PlatformIO 6.2.0 (venv `~/workspace/projects/rsh/build/.venv-pio`), `TMPDIR=~/tmp`.

- `pio run` (env `esp32dev`): **SUCCESS**.
  - RAM 29.9% (97,816 / 327,680 B), Flash 67.7% (887,993 / 1,310,720 B).
  - Merged factory image `RSH-Firmware-DevKitV1-v4.0.2.bin`: **960,112 B**, SHA-256 `356b1b4947422b34d03951b3311300800ce72ec4f5543a6220ad11760feddefc` (final rebuild 08:39; an earlier identical-size build hashed `c47f5641…` — the ESP32 app descriptor embeds the build date/time, so rebuilds hash differently at the same size; code identical).
- Throwaway signing (test only — keypair generated in `/tmp/rsh-signing-throwaway/`, private key 0600, never in repo or signing-keys):
  - `RSH_SIGNING_KEY=/tmp/rsh-signing-throwaway/signing-key.pem pio run` → `firmware-signed.bin` **894,644 B**, SHA-256 `1dcc119f9e1928acbe7676333d4930934b30b2c4be9f9437e7039eb1629896bc` (final rebuild 08:39; earlier identical-size build hashed `49cebfa9…` — same build-time embedding note as the merged image).
  - `sign-image.py --verify` with the throwaway key: **VERIFY OK** (image SHA-256 `0fea961ecc79cd5b…`).
  - Negative check: verify against the PRODUCTION pubkey (parsed from `signing_pubkey.h`, DER SPKI built offline) → rc=1 (expected: a throwaway-signed image does NOT verify on-board — build/sign pipeline verification only).
  - The baked pubkey in `signing_pubkey.h` is the production one, so the throwaway-signed image is for pipeline verification only, never for a board.

## 4. F-06 jitter evidence

Host tests cover the mailbox (produce/consume, overwrite→skipped, NULL guards) and the dirty-flag apply path only. Real jitter is **OPEN — needs the real board** (§8). Design facts that bound the risk:

- Sample task: core 1, priority 2 (above the Arduino loop task priority 1), `vTaskDelayUntil` 5 ms (governor 5/10 ms), stack 4096. Tick rate assumption: Arduino ESP32 FreeRTOS tick 1000 Hz (confirmed by the secure-env fix — Arduino requires `CONFIG_FREERTOS_HZ=1000`; the secure defaults had 100, which the Arduino version check rejects).
- SEND stays in the loop task because WiFiUDP is not thread-safe; the mailbox is single-slot, latest wins, no burst catch-up (overwrites counted into `seqGap`, visible in the status packet).
- Known benign race: `net_set_stream_busy()` is now called from the sample task while `net_poll` (loop task) reads the flag — single-byte hint for ping drop-when-busy only, no correctness impact.
- Legacy in-loop sampling is the automatic fallback if task creation fails (same `sample_once_`/`sample_send_frame_` helpers — one code path).

## 5. Rust-side follow-up (NOT this worker's paths — Phase 5 / daemon worker)

`crates/rsh-protocol/src/pin.rs` rejects unknown blob record types (`other => Err("unknown record type {other}")`). Before the app can push/read blobs containing the new fields, the Rust blob parser needs a matching update for: record type 13 (LED_CONFIG), handbrake flags bit0, shifter sensitivity param[4]. Firmware-side GET_CFG returns raw bytes, so wire read-back works regardless. Flagged here; do not touch `crates/`.

## 6. F-09 secure build + D-11 decision note

### Secure build outcome

- `pio run -e esp32dev-secure`: **SUCCESS** (see §6.1 for the fixes it took — this env was never build-verified in this sandbox before).
  - RAM 25.7% (84,188 / 327,680 B), Flash 70.6% (925,941 / 1,310,720 B).
  - Merged image `RSH-Firmware-DevKitV1-v4.0.2.bin` (secure env): **998,048 B**, SHA-256 `2a98fabd8fec91074680775e043e6ac26ea4dd6934d051767faeb8ecbd8ede3d`.
  - `firmware.bin` (secure env): 932,512 B. `sign_firmware` skipped — no `RSH_SIGNING_KEY` in the env for this run (unsigned dev build; the secure image was built to prove compile + NVS-encryption config, not to be flashed).
- Throwaway secure-boot key `firmware/rsh/secure-boot-key.pem` (ECDSA P-256, 0600, gitignored `*.pem`) was generated for this build and **deleted after the build** (it is never the production key, never committed).

### 6.1 What it took to get the secure env building (all pre-existing toolchain/layout issues, none in RSH application code)

1. Sandbox CMake 3.16.4 → 3.28.3 (backup `/tmp/tool-cmake-3.16-backup`; tarball `~/tmp/cmake-3.28.3.tar.gz`). IDF 4.4.7's `kconfig.cmake:1` does `include(ExternalProject)` in CMake script mode — `define_property` is not scriptable in any CMake version. Upgrading CMake alone did NOT fix it.
2. Guard in IDF's `kconfig.cmake` (backup `/tmp/kconfig.cmake.bak`): `if(NOT CMAKE_SCRIPT_MODE_FILE) include(ExternalProject) endif()` — `ExternalProject` is included but never used in that file (verified by grep).
3. Top-level `firmware/CMakeLists.txt` (generated by PlatformIO, untracked — now committed): script-mode guard around the `project.cmake` include, because IDF's `idf.cmake` calls `__build_init` (`add_library`) unconditionally at include time.
4. `sdkconfig.secure.defaults`: `CONFIG_FREERTOS_HZ` 100 → 1000 (Arduino requires it; F-06 assumes 1000 Hz ticks).
5. **Root cause of "Couldn't find the main target":** the firmware component never registered. PlatformIO expects `__idf_firmware` (`__idf_<basename(PROJECT_SRC_DIR)>`). The component IS discovered (EXTRA_COMPONENT_DIRS contains the project dir), but IDF's requirements pass runs each component's CMakeLists in script mode — and a component that does not call `idf_component_register` there gets `__COMPONENT_REGISTERED=0`, so `__build_expand_requirements` drops it from the build list (verified in `framework-espidf/tools/cmake/build.cmake:216-218`). Fix: `firmware/CMakeLists.txt` now calls `idf_component_register` in BOTH modes (script mode uses IDF's stub macro that only records requirements; project mode does the real register) and `return()`s.
6. `platformio.ini` secure env: `-Wno-error=missing-field-initializers` — Arduino's own `I2S.cpp` does not initialise IDF 4.4.7's newer struct fields (`i2s_driver_config_t.tx_desc_auto_clear/fixed_mclk/mclk_multiple/bits_per_chan`, `i2s_pin_config_t.mck_io_num`) and `-Werror` killed the build. This flag applies to the whole secure build (not just the Arduino lib) — RSH code keeps full `-Werror` in the default env and host tests.
7. `sdkconfig.secure.defaults`: `CONFIG_MBEDTLS_PSK_MODES=y` — Arduino's `ssl_client.cpp` has a `#warning` (→ error under `-Werror=cpp`) unless PSK ciphersuites are enabled.
8. `sdkconfig.secure.defaults`: `CONFIG_AUTOSTART_ARDUINO=y` — default is `n`; with `n`, `cores/esp32/main.cpp` compiles `app_main` away entirely (`#if CONFIG_AUTOSTART_ARDUINO` wraps the whole function) and the link dies with `undefined reference to app_main`. Lesson recorded: `sdkconfig.secure.defaults` IS the live sdkconfig file (SDKCONFIG points at it) — IDF's kconfig pass rewrites it in place, so duplicate entries are resolved in favour of the FIRST occurrence. An appended `CONFIG_AUTOSTART_ARDUINO=y` was silently dropped because line 162 already said `# CONFIG_AUTOSTART_ARDUINO is not set`; the fix edits that line in place.

### D-11 decision (for the parent, at release time)

- **Default (normal) image:** ships as-is today. NVS in clear; no secure boot / flash encryption. Same posture as v4.0.1.
- **Secure image (`RSH_SECURE_NVS`, `esp32dev-secure` env):** now builds. It enables NVS encryption + secure-boot signing flow, but it is **not yet board-verified** (see §8) and the eFuse steps are manual and out of scope for this phase. A secure image flashed to a board without the matching eFuse/key provisioning bricks the update path — do not ship it until the sacrificial-board verification in §8 passes and the key-provisioning runbook exists.
- **Recommendation:** release v2.0 on the normal image (unchanged default, D-11 default). Revisit the secure image only after the §8 sacrificial-board check.

## 7. Other notes

- Factory blob UNCHANGED (11 records, 188 B, no LED_CONFIG record → strip off by default).
- LED persistent pattern period is fixed 1000 ms (not stored in the record); brightness 0..100.
- Handbrake button mode: stream axis slot 4 unchanged (4095 pressed / 0 released); GET_CAPS kind flips to DIGITAL_IN. Note: `rsh_caps_pin_caps_` adds the ADC cap for ADC1 pins 32/33/34/35/36/39 regardless — a button-mode handbrake on pin 32 still advertises the ADC cap; the host test uses pin 25 to assert no-ADC.
- F-11 (3V3 monitor): unmeasured, no free ADC1 pin — documented, no code change.
- F-13 (single-PC model): documented in `docs/FIRMWARE-COMPAT.md` §2.1; Observer mode not implemented.
- Sandbox-only changes (NOT in the repo): CMake 3.16.4 → 3.28.3 (backup `/tmp/tool-cmake-3.16-backup`), `kconfig.cmake` guard (backup `/tmp/kconfig.cmake.bak`). These live in `~/.platformio/packages/` and do not affect the repo; a fresh sandbox will need them again for the secure env (documented here so the next run is not a mystery).
- `firmware/rsh.ino.cpp` is a transient PlatformIO artifact (created by the `.ino` conversion, deleted at pio exit) and is NOT committed; it is also not gitignored — recommend adding `*.ino.cpp` to `.gitignore` (parent decision, root file, out of this worker's paths).

## 8. Open real-board items (HIL — plan §7, NOT guessed)

1. **F-06 jitter + 4-hour soak** (plan §7/§8 — the soak was skipped for v1.0.0 by user order 2026-11-14, it runs this time): status packet `max_cycle_us`/`overruns` + stream rate/packet capture over the soak; pass bar = 200 Hz nominal, no sustained drop, `seqGap` only from genuine overruns.
2. **Pair button end-to-end:** BOOT hold 3 s → PairWindow LED (3 Hz) + window opens 120 s; HELO within the window pairs; AP locks WPA2 after GET_CAPS OK; PC rejoins the locked AP (Phase 6 rejoin helper); reboot + pair again.
3. **Serial window:** USB-serial HELO on an unpaired board opens the 120 s window (diag `WINDOW_OPEN` value 2); pairing over serial then works without the AP window.
4. **WebOTA per-IP lockout with two real stations** (AP max 4): 5 bad passwords from station A locks only A for 60 s; station B still works.
5. **LED persistent + brightness on the strip** (visual): pattern + RGB + brightness from a type-13 record; LED_TEST overrides then persistent resumes with no off-flash.
6. **Handbrake button mode + shifter sensitivity feel:** button press → axis 4095/0; sensitivity slider changes the H-pattern stability time (20→5 ms).
7. **HORN_TEST end-to-end** (0x0E, 500 ms): horn output on for the duration, then off.
8. **Secure image on a sacrificial board** before any D-11 ship decision: flash secure image, verify boot + NVS encryption + OTA update path; eFuse steps manual.
9. **F-06 tick-rate assumption** (1000 Hz) is now enforced by the Arduino version check in the secure env; the default env uses Arduino's own default (1000 Hz) — confirm on the board via the status packet timing if jitter looks off.

## 9. Files changed + commit

- Modified: `firmware/config.h`, `config_transaction.h`, `inputs.cpp`, `keystore.{h,cpp}`, `led_driver.h`, `merge_bin.py`, `modules.h`, `net.{h,cpp}`, `net_logic.h`, `pinmap.h`, `protocol.h`, `rsh.ino`, `safemode.h`, `status_led.{h,cpp}`, `webota.cpp`, `platformio.ini`, `sdkconfig.secure.defaults`, `firmware/tests-host/test_main.cpp`, `docs/FIRMWARE-COMPAT.md`.
- New: `firmware/version.h`, `pairing_window.h`, `diag_log.h`, `webota_auth.h`, `horn_test.h`, `sample_task.h`, `firmware/CMakeLists.txt`, this doc.
- Commit (local only, my paths only): `Firmware v4.0.2 — F-01..F-16 (gate PASS)`.
- Daemon worker's paths (`crates/*`, `tools/rsh-rig`, `Cargo.toml`, `Cargo.lock`, `README.md`, `docs/V2-PHASE-2-GATE.md`) were NOT touched by this worker.

## 10. BOOST — redundant runs cut

- Full host-test suite + full `pio run` only at the gate; small fixes re-ran only affected checks. The secure-env cascade (builds 1–12) reused the incremental build dir — only the failing step was re-run, never a full clean rebuild after the first one. The default-env `pio run` was run once for the gate (plus once for the throwaway-sign verification); the merged image was re-generated once after the build dir was found missing. Estimated saving vs re-running full suites per fix: ~2–3 hours of build/test time.
