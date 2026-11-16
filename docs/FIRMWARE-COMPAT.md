# RSH Rust — Firmware Compatibility (FW-A1 / FW-A2)

**Tareekh:** 2026-11-14 · **Status:** FW-A1 + FW-A2 complete, FW-A4/A7 carried (neeche §8)
**Frozen triple:** Rust app **v1.0.0** · C++ firmware **v4.0.1** · protocol **v2.1**
**Authority:** `reference/docs/PROTOCOL-V2.1.md` (wire contract) ·
`reference/firmware-v4.0.1/` (C++ firmware source, unchanged) ·
`reference/tests-host/` (C++ host tests + Python reference vectors).

## 0. Faisla (user decision, RESEARCH.md §8)

C++ firmware v4.0.1 **stays** (Track A ships). Rust app v1.0.0 usi board se
baat karta hai, **koi reflash zaroori nahi** — protocol v2.1 frozen hai.
Firmware mein koi change nahi hua; is doc ka kaam sirf ye prove karna hai ke
Rust app C++ board ke saath byte-level compatible hai. esp-rs firmware
(Track B spike) is doc ka hissa nahi — us ka alag worker hai.

## 1. Frozen contract — protocol v2.1 wire format

| Cheez | Frozen value |
|---|---|
| App version | Rust v1.0.0 (nayi series; C# v2.0.4 se alag) |
| Firmware version | C++ v4.0.1 (`FIRMWARE_VERSION "4.0.1"`, `config.h:134`) |
| Protocol version | v2.1 (`0x21` har frame mein; HELLO proto `2.1`) |
| Stream (board → PC, UDP 4211) | `"RSH2" \| ver 0x21 \| flags \| seq u16 \| ts u32 \| axisMask \| nEnc \| buttons u32 \| rsv u16=0 \| axes u16×N (mV) \| encoders i16×nEnc (cumulative) \| CRC16` |
| Status (board → PC, UDP 4211, 1 Hz) | magic `0xA6`, 34 B base / 38 B extended (btnsEx u32), seqGap + txFail CRC16 se pehle |
| HELLO reply (board → PC, UDP 4211) | `"RSHR"`, 92 B fixed: proto 2.1, flags, sessionEcho, boardMac 6 B, fw, nonce 16 B, deviceKey 32 B, wifiPassword 24 B, CRC16 |
| HELO (PC → board, UDP 4210) | 5 B (`"RSH2"`… — §2.1; proto `0x01` = v1 legacy, `0x02` = v2.x; sessionFlag `0` resume / `1` naya session) |
| Control (dono taraf, UDP 4212) | `"RSHC" \| cmd \| reqId u16 \| counter u32 \| len u16 \| payload \| tag[16]` |
| HMAC tag | `HMAC-SHA256(key32, nonce \|\| counter_LE \|\| cmd \|\| payload)[0:16]` — `reqId`/`len` HMAC mein **nahi** (frozen faisla, PROTOCOL-V2.1.md §3.2) |
| Ping (§9, UDP 4212) | `"RSHN"`, 34 B fixed, magic se control se demux |
| PIN_PROBE stream (§3.8, UDP 4211) | `"RSHP" \| ver 0x21 \| seq u16 \| pin u8 \| mode u8 \| sampleCount u16 (1–64) \| samples \| CRC16`; mode 0 = u8 levels, mode 1 = u16 mV |
| Serial (USB, §10) | `"RSHS" \| kind u8 (1–7) \| len u16 LE \| payload \| CRC16`; **921600 baud, 8N1**, no handshake |
| CRC16 | CRC16-CCITT-FALSE (poly `0x1021`, init `0xFFFF`); check value `crc16("123456789") = 0x29B1`; hamesha aakhir mein, CRC field se pehle ke tamam bytes par |
| CRC32 (config blob trailer) | CRC-32/ISO-HDLC; check value `0xCBF43926` |

### 1.1 Opcodes (§3.3) — `0x01–0x0C`, reply mein `cmd = req | 0x80`

| cmd | Name | cmd | Name |
|---|---|---|---|
| `0x01` | GET_CAPS | `0x07` | PIN_PROBE |
| `0x02` | GET_CFG | `0x08` | FACTORY_RESET |
| `0x03` | SET_CFG | `0x09` | REBOOT (waahid restart rasta; unauthenticated `"RSTX"` hata diya gaya) |
| `0x04` | COMMIT | `0x0A` | LED_TEST |
| `0x05` | CONFIRM | `0x0B` | KEY_RESET (sirf key-reset-armed state mein) |
| `0x06` | REVERT | `0x0C` | SET_PASSWORD |

### 1.2 Error codes (§3.9)

| Code | Name | Code | Name |
|---|---|---|---|
| 0 | OK | 4 | TOO_BIG |
| 1 | BAD_TAG | 5 | PIN_CONFLICT |
| 2 | REPLAY | 6 | UNSUPPORTED |
| 3 | BAD_CRC | 7 | BUSY |

### 1.3 Config transaction (§3.5–§3.6, D28)

- `SET_CFG` chunk: `offset u16 | total u16 | chunkLen u16 | chunkCrc u16 (CRC16 sirf data par) | data`; chunk ≤ 1024 B, total ≤ 2048 B; pehla chunk offset 0; baad wale kisi bhi order mein; retry idempotent.
- Blob `RSHG`: header 8 B (`"RSHG" | schema 2 | moduleCount u8 | cfgVersion u16 LE`), records 16 B each (`type | slot | flags | pinA | pinB | pinC | params[10]`), trailer 4 B CRC32. Record types 1–12 (§3.6).
- Stage → COMMIT → CONFIRM (**30 s**, hamesha) → permanent; 30 s mein CONFIRM na ho → rollback; reboot kabhi confirm nahi karta (boot par pending-confirm rollback hota hai).
- Factory default blob: **188 B, 11 records, cfgVersion 1** — axes 0:34, 1:35, 2:36, 4:32; battery 0:39; buttons 0:27, 1:33, 2:4; encoders 0:23/19, 1:16/17; LED strip 0:21. Clutch OFF (koi AXIS_ADC slot 3 record nahi), shifter sequential (koi SHIFTER record nahi).
- Board limits (D6): axes ≤ 8, buttons ≤ 128 (wire slot 64 — 65..128 COMMIT par UNSUPPORTED, N1), encoders ≤ 4, LED strips ≤ 1 (≤ 60 LEDs/strip), batteries ≤ 1 (battery axis nahi gini jati).

## 2. Compat matrix

| Rust app | C++ firmware | Protocol | Status |
|---|---|---|---|
| v1.0.0 | **v4.0.1** | v2.1 | **SUPPORTED — shipped combination (is doc se verified)** |
| v1.0.0 | v4.0.0 | v2.1 | SUPPORTED (wire format same; HELLO fw bytes `4.0.0` hote hain — neeche note) |
| v1.0.0 | < v4.0.0 | v2.1 | UNSUPPORTED (verify nahi hua; v2.0.0 se pehle ka protocol alag tha) |
| v1.0.0 | koi bhi | v2.0 ya v1 | UNSUPPORTED (parser v2.1 mangta hai; v1 packets sirf legacy-detect hote hain, parse nahi hote) |
| (koi bhi) | v4.0.1 | v2.1 | C# app v2.0.4 bhi isi firmware ke saath ship hui thi (v2.0.4 release) |

**Note — golden HELLO vector mein fw `4.0.0`:** frozen test fixture
(`protocol_v2_vectors.py` `hello_reply`, `GET_CAPS` reply body) mein firmware
bytes `04 00 00` hain — fixture v4.0.1 bump se pehle freeze hua tha. Rust
parser version ko hard-check nahi karta (koi bhi `(major, minor, patch)`
parse hota hai); live v4.0.1 board HELLO/GET_CAPS mein `(4, 0, 1)` bhejta hai
(Rust `caps.rs` round-trip test isi value par hai). Fixture ke bytes tabdeel
nahi kiye gaye — parser dono ko theek parse karta hai.

### 2.1 v4.0.2 (RSH v2.0) — additive changes, wire UNCHANGED

- Firmware **v4.0.2** (`FIRMWARE_VERSION "4.0.2"`, single source
  `firmware/version.h`; `RSH_CAPS_FW_*` wahi se aate hain — F-12). Protocol
  **v2.1 frozen** — koi frame layout, opcode range, HMAC/CRC ya blob layout
  tabdeeli nahi; golden vectors dobara run hue, koi vector tabdeel nahi
  (`firmware/tests-host/verify_packet.py` 51 checks PASS).
- Additive control opcodes: `GET_DIAG 0x0D`, `HORN_TEST 0x0E` (purane
  opcodes 0x01–0x0C waisay hi). Purana app inhein kabhi nahi bhejta; purana
  firmware (4.0.1) unknown cmd par `UNSUPPORTED` deta hai (`ctrl.h`).
- Config blob: naya record type 13 `LED_CONFIG` (max 1, slot 0, flags 0,
  pins 0xFF, pattern 0..4, brightness 0..100); shifter sensitivity
  `params[4]` (0..100, default 50); handbrake AXIS slot-4 `flags` bit0 =
  button mode (baaqi flag bits reject; button pin par PULLUP rule, axis
  pin par ADC1 rule). Factory blob UNCHANGED (11 records, 188 B, koi
  type-13 record nahi — strip default off).
- HELLO mein pairing window ki jagah NAHI hai (F-16): byte 17 reserved
  hai aur parser nonzero reject karta hai — app conservative countdown
  istemal karti hai; asal `window_remaining_s` GET_DIAG se milta hai.

| Rust app / daemon | C++ firmware | Protocol | Status |
|---|---|---|---|
| v2.0 daemon/app | **v4.0.2** | v2.1 | **SUPPORTED — full** (pair button, GET_DIAG, HORN_TEST, LED persistent, handbrake button mode, shifter sensitivity) |
| v2.0 daemon/app | v4.0.1 | v2.1 | DEGRADED — pair-button screen "Use USB or reboot wheel" fallback; GET_DIAG/HORN_TEST UNSUPPORTED; LED persistent / handbrake button mode unavailable (app ko blob mein type 13 ya handbrake flag bit0 nahi bhejna chahiye) |
| v1.0.x app | v4.0.2 | v2.1 | SUPPORTED — purana app naye firmware se kaam karta hai (naye opcodes kabhi nahi bhejta; HELLO/GET_CAPS fw bytes `(4, 0, 2)`) |

- **F-13 single-PC model:** ek waqt mein sirf ek PC board se paired /
  stream karta hai (HELO session rules + F-05 same-IP relax: wahi PC,
  purana session >2 s stale ho to dobara session bana sakta hai; doosra IP
  10 s ka paband hai). Doosra PC "Observer" mode mein sirf dekh sakta hai
  — Observer mode abhi implement NAHI hua (baad ka feature); doosre PC ki
  stream request live session par reject hoti hai (SEC-D-08).
- **Rust-side dependency (Phase 5):** `crates/rsh-protocol/src/pin.rs`
  unknown record types reject karta hai — type 13, handbrake flag bit,
  aur shifter sensitivity ke liye Rust blob parser update chahiye, warna
  app aise blob push/read nahi kar sakti. Firmware side GET_CFG raw bytes
  deta hai (wire read-back kaam karta hai).

## 3. Version-bump rule (frozen contract ki hifazat)

1. **Wire format mein koi bhi tabdeeli** (frame layout, opcode, error code,
   HMAC formula, CRC, config blob layout, `params[]` semantics) →
   **protocol version bump lazmi** (v2.2 ya v3.0) + is doc mein **nayi matrix
   row** + naye golden vectors (Rust ↔ C++ ↔ Python, 3/3 byte-identical).
2. Wire tabdeeli ke baghair app/firmware bump (bug fix, performance) →
   matrix mein nayi row, vectors dobara run karo, koi vector tabdeel nahi.
3. Koi bhi wire tabdeeli **master plan §9 decision** ke baghair nahi hoti —
   default NO. Is track (FW-A) mein koi wire change propose nahi hua.

## 4. Golden-vector files (dono taraf)

| Side | File | Vectors |
|---|---|---|
| Rust | `crates/rsh-protocol/tests/golden_vectors.rs` | 18 tests — 11 positive + 7 negative (hex literals, Python output se verbatim copy). Note: doc pehle "10 + 6" kehta tha — ghalat tha; file mein 11 positive (`vector_stream_v2_1`, `vector_status_v2_1_base`, `vector_status_v2_1_extended`, `vector_hello_unpaired`, `vector_hello_paired`, `vector_ctrl_get_caps`, `vector_ctrl_set_cfg`, `vector_ctrl_get_caps_reply`, `vector_serial_stream`, `vector_serial_helo`, `vector_crc16_v1_golden`) + 7 negative (`negative_bad_tag`, `negative_wrong_key`, `negative_truncated_control_is_bad_length_not_too_short`, `negative_replay_and_duplicate_are_state_rules_not_tag_rules`, `negative_bad_magic_control`, `negative_stream_rejects_counted_per_reason`, `negative_serial_crc_flip_rejected`) hain; total 18 sahi hai. |
| Rust (unit) | `crates/rsh-protocol/src/{crc,stream,hello,control,config,ping,probe,caps,pin}.rs` | round-trip + property tests |
| C++ / Python | `firmware/tests-host/protocol_v2_vectors.py` (vendored, M9) | `VECTORS` (10) + `NEGATIVE` (6); `python3 -I protocol_v2_vectors.py` se regenerate hota hai |
| C++ / Python | `firmware/tests-host/verify_packet.py` (vendored, M9) | 3-language cross-verify (stream/status/hello/control/probe + negatives) |
| C++ / Python | `reference/tests-host/pin_rules_ref.py` | pin-rules reference validator + 62 PASS checks |
| C++ | `firmware/tests-host/test_main.cpp` (vendored, M9; §1–§34) | host tests, 4705 checks (incl. Phase 2 `FACTORY_RESET` block §25b + v4.0.2 block §34, 261 checks) |

## 5. FW-A2 — Interop verification (baghair board) — evidence

Sab kuch 2026-11-14 ko is sandbox mein dobara run hua hai. Board maujood
nahi tha — is liye live HELLO/handshake/board run **nahi** ho saka (wo
FW-A4/A7 hai, §8).

### 5.1 PlatformIO build (scratch copy, `reference/` ko haath nahi lagaya)

- Command: scratch copy `~/tmp/fw-v4.0.1-scratch` mein `pio run`
  (venv `~/workspace/projects/rsh/build/.venv-pio/bin/pio`).
- Result: **`[SUCCESS]`, 59.28 s.** `esp32dev` env, 1 succeeded.
- Merged bin: **`RSH-Firmware-DevKitV1-v4.0.1.bin`, 955,056 B** — C# series
  ke number se **byte-size mein barabar** (955,056 B). SHA-256 (is run ka):
  `f73392e2fe56d0315a3b813672b3c8526c1cb8380834d4961cac1d83431b433a`.
- RAM: **30.2% (98,864 / 327,680 B)** · Flash: **67.4% (882,937 / 1,310,720 B)**.
- App-only `firmware.bin`: 889,520 B → + 68 B signature trailer = 889,588 B
  signed bin (C# series ka signed size — is run mein key nahi thi, §7).
- `sign_firmware` ne key na hone par sahi fail-closed behaviour dikhaya:
  `RSH_SIGNING_KEY nahi — firmware-signed.bin nahi bana (dev build unsigned
  rehta hai…)`. Koi stale signed bin nahi bacha.

### 5.2 C++ host tests (scratch copy, `run_tests.sh`)

| Section | Result |
|---|---|
| 1. C++ host tests (`test_main.cpp`) | **4444 checks, 0 failures** (vendored copy `firmware/tests-host/`, incl. Phase 2 `FACTORY_RESET` block §25b; scratch copy run at FW-A2 was 4430 checks, 0 failures) |
| 2. Firmware `.cpp` on host (`inputs.cpp` + `status_led.cpp`) | **50 + 50 checks, 0 failures** (mv-ok + mv-fallback) |
| 3. Python pin-rules reference | **62 PASS, 0 FAIL** (`ALL PASS`) |
| 4. Python protocol v2.1 golden vectors | self-consistent, `OK` |
| 5. Python packet cross-verify (3-language) | **51 checks, PASSED** |
| 6. Merge-bin failure mock (F15) | **7 checks, 0 failures** |
| Brick-recovery host-mock (`test_brick_recovery.py`, alag se) | **9 checks, 0 failures** — asli merged bin validate hua (955,056 B) |
| **Total** | **4588 counted checks + pin-rules/vectors self-checks, 0 failures** |

Note: `run_tests.sh` ko chalane ke liye scratch layout mein `firmware/rsh/`
+ `tests-host/` + `docs/` (board JSON ke liye) chahiye thay — sirf scratch
dirs mein banaye gaye, `reference/` untouched raha.

### 5.3 Vector cross-check — Rust ↔ C++/Python, byte-for-byte

Python vectors ko Rust `golden_vectors.rs` ke hex literals se programmatically
milaya gaya (har vector ka poora hex, whitespace-tolerant):

| Family | Vectors | Match |
|---|---|---|
| Stream v2.1 (`data_v2_1`, 36 B) | 1 | **1/1 byte-identical** (parse + re-encode dono) |
| Status v2.1 base (34 B) + extended (38 B) | 2 | **2/2 byte-identical** |
| HELLO unpaired (92 B) + paired (92 B) | 2 | **2/2 byte-identical** |
| Control GET_CAPS (29 B) + SET_CFG (53 B) + GET_CAPS reply (47 B) | 3 | **3/3 byte-identical** (HMAC tag same formula) |
| Serial stream (17 B) + HELO (15 B) | 2 | **2/2 byte-identical** (`…d95d`, `…4749` CRC same) |
| CRC check values | 2 | `crc16("123456789") = 0x29B1` ✓ · v1 golden packet CRC `0xBD75` ✓ |
| Negative: bad tag / wrong key / truncated / bad magic / replay / duplicate | 6 | **6/6 same verdict** (4 literal hex match; bad-tag aur truncated Rust mein good frame ko flip/cut karke banaye jate hain — Python ke `_flip`/`[:-8]` jaisi hi construction; replay/duplicate tag-valid hain, counter-state se reject hote hain — dono taraf) |
| PIN_PROBE (§3.8) | 2 | Layout + field order + CRC same (Python `build_probe(3, 21, 1, [1200,3300,0])` = `52 53 48 50 21 03 00 15 01 03 00 b0 04 e4 0c 00 00 b2 76`; Rust `ProbeFrame::encode` wahi bytes banata hai; digital vector bhi same) |
| Config blob / SET_CFG chunk | — | Factory blob 188 B / 11 records dono taraf same (Rust `factory_default_blob` == C++ `rsh_cfg_factory_blob`); chunk header + chunk-CRC16 (sirf data par) same |

**Koi unexplained mismatch nahi mila.** Rust test suite: `cargo test
-p rsh-protocol` → **64 passed (35 unit + 18 golden-vector + 10 property +
1 doctest), 0 failed**; `cargo clippy -p rsh-protocol --all-targets -- -D
warnings` → **0 warnings**; `cargo fmt --check -p rsh-protocol` → **clean**.

## 6. A1 `validate_blob` cross-check (Phase A se carried)

C++ authority: `reference/firmware-v4.0.1/config_transaction.h`
(`rsh_cfg_blob_validate` + `rsh_cfg_pin_check_`) aur
`reference/tests-host/pin_rules_ref.py` (Rust `pin.rs` isi ko mirror karta
hai). Rust side: `crates/rsh-protocol/src/pin.rs`
(`validate_blob` / `blob_record_module` / `shifter_errors`).
`config.rs` khud **sahi** hai (record types 1–12, factory blob, chunk/blob
encode-decode sab C++ se match) — is liye `config.rs` mein **koi edit nahi**
ki gayi. Neeche ke farq `pin.rs` mein hain (meri file list se bahar — fix
yahan nahi, §6.3 mein bug report hai).

### 6.1 Match table — record type → Rust mapping vs C++ validator

| Record (type) | `params[]` / pins mapping | Verdict |
|---|---|---|
| AXIS_ADC (1) / AXIS_GENERIC (10) | pinA ADC1 rule; pinB/pinC sirf barred/dup/I2C check (C++) | **GAP — pinB/pinC Rust mein ignore (BUG-1)** |
| AXIS_ADS1115 (2) | I2C kind, pins 0xFF lazmi; channel `params[0]`; addr fixed 0x48 (Rust) | **Rust STRICTER — duplicate channel + channel > 3 reject (BUG-5)** |
| BUTTON (3) | pinA pull-up rule; pinB/pinC sirf barred/dup/I2C check (C++) | **GAP — pinB/pinC Rust mein ignore (BUG-1)** |
| BUTTON_MATRIX (4) | rows `params[0]` (0→4), cols `params[1]` (0→4); pins pinA/B/C + `params[2..]`; count rows×cols (Rust) vs **16 fixed** (C++) | **DIVERGENCE — count + params-pin checks (BUG-6, code-level)** |
| ENCODER (5) | pinA + pinB pull-up; pinC sirf barred/dup/I2C check (C++); dono pins hon to n_enc++ | **GAP — pinC ignore + 1-pin encoder drop (BUG-2)** |
| LED_STRIP (6) | pinA digital-out; count `params[0]` (Rust, 1–60) | **Rust STRICTER — count 61 reject (BUG-5)** |
| BATTERY (7) | pinA ADC1 rule; pinB/pinC sirf barred/dup/I2C check (C++) | **GAP — pinB/pinC Rust mein ignore (BUG-1)** |
| EXPANDER_MCP23017 (8) / PCF8574 (9) | I2C kind, pins 0xFF lazmi; expander pin `params[0]` | **Rust STRICTER — pin range + duplicate reject (BUG-5)** |
| AXIS_AS5600 (12) | flags sirf bit0, rotation `params[1]` ≤ 2, pins 0xFF, max 1/board, axis count | **GAP — flags/rotation/pins Rust mein ignore (BUG-3)** |
| SHIFTER (11) | gate mode `params[0]` (0 seq / 1 H); H gears pinA/B/C + `params[1..3]` (6 pins, pull-up, unique); H + ENCODER conflict | **GAP — gear pin 21/22 + I2C (BUG-4)**; baaqi match |
| Limits | axes ≤ 8, enc ≤ 4, bat ≤ 1, LED (pin wala) ≤ 1 | Match |
| Buttons limit | > 128 reject (dono); **65..128 → UNSUPPORTED err 3 (C++ N1)** | **DIVERGENCE — Rust 65..128 accept karta hai (BUG-6, code-level)** |
| Pin classes | forbidden/avoid/reserved (flash 6–11, UART 1/3, strapping 0/12/15, LED 2, nonexistent 20/24/28–31/37/38); pin 5 caution = allowed; ADC1 {32,33,34,35,36,39}; input-only {34,35,36,39} digital reject | Match (board JSON vs C++ barred set barabar) |
| Factory blob | 188 B / 11 records | **Match** (dono taraf same records) |

Record bytes layout (sab cases ke liye): `type | slot | flags | pinA |
pinB | pinC | params[10]` (16 B).

### 6.2 Bugs mile (Rust accept karta hai jo board COMMIT par reject karta hai — khatarnak direction)

Ye sab `/tmp` probe crate se empirically verify hue hain (repo untouched):
`validate_blob()` ne har case mein `[]` (koi error nahi) diya, jab ke C++
`rsh_cfg_blob_validate` `PIN_CONFLICT (5)` deta hai.

- **BUG-1 — pinB/pinC kabhi check nahi hote (AXIS_ADC / AXIS_GENERIC /
  BUTTON / BATTERY).** C++ generic loop har record ke pinB/pinC par
  barred/duplicate/I2C check lagata hai; Rust `blob_record_module` sirf
  pinA ko module mein leta hai, baaqi do pins phenk deta hai.
  - Repro: `01 00 00 22 06 ff 00 00 00 00 00 00 00 00 00 00`
    (AXIS_ADC, pinA 34, pinB 6 = flash pin) → Rust `[]`, C++ err 2.
  - Repro: `03 00 00 1b 06 ff 00 00 00 00 00 00 00 00 00 00`
    (BUTTON, pinA 27, pinB 6) → Rust `[]`, C++ err 2.
  - Asar: app "valid" config bhejti hai, board COMMIT par PIN_CONFLICT se
    reject karta hai — user ko der se, confusing failure milti hai.
- **BUG-2 — ENCODER pinC ignore + 1-pin encoder silently drop.**
  - Repro: `05 00 00 17 13 06 00 00 00 00 00 00 00 00 00 00`
    (ENCODER 23/19, pinC 6) → Rust `[]`, C++ err 2.
  - Repro: 5× ENCODER records jin mein pinB `0xFF` ho (1-pin encoders) →
    Rust sirf `["at least one input module required"]` (saare records drop
    ho gaye — error misleading hai, blob khaali nahi tha), C++ n_enc = 5 >
    4 → err 2 reject. Matlab encoder-limit check ko bypass kiya ja sakta hai.
- **BUG-3 — AXIS_AS5600 ke flags / rotation / pins ignore.**
  C++: flags sirf bit0, `params[1]` (rotation) ≤ 2, pinA/B/C `0xFF` lazmi.
  Rust `axis_as5600()` in mein se kuch nahi dekhta.
  - Repro: `0c 00 00 20 ff ff 00 00 00 00 00 00 00 00 00 00`
    (AS5600 pinA 32) → Rust `[]`, C++ err 2.
  - Repro: `0c 00 02 ff ff ff 00 00 00 00 00 00 00 00 00 00`
    (AS5600 flags `0x02`) → Rust `[]`, C++ err 2.
  - Repro: `0c 00 00 ff ff ff 00 03 00 00 00 00 00 00 00 00`
    (AS5600 rotation 3) → Rust `[]`, C++ err 2.
- **BUG-4 — SHIFTER H-pattern gear pin 21/22 + I2C record.**
  C++ `rsh_cfg_pin_check_` I2C kinds maujood hon to gear pin 21/22 par
  err 1 deta hai; Rust `validate_blob` mein gear pins check hote waqt I2C
  reservation `used` map mein abhi nahi hoti (wo record loop ke baad
  insert hoti hai), aur gear pins `validate()` ke apne `used` map mein
  kabhi nahi jate.
  - Repro: SHIFTER `0b 00 00 15 0d 0e 01 10 11 12 00 00 00 00 00 00`
    (gear pins 21,13,14,16,17,18) + ADS1115
    `02 00 00 ff ff ff 00 00 00 00 00 00 00 00 00 00` → Rust `[]`, C++ err 1.

### 6.3 Rust STRICTER cases (board accept karta hai, app reject karti hai — safe direction, lekin false rejection)

- **BUG-5 — Rust C++ se zyada sakht hai (3 jagah).**
  - ADS1115 duplicate channel: 2× `02 00 00 ff ff ff 00 …` (channel 0) →
    Rust `["axis_ads1115: channel 0 on addr 72 already used"]`, C++ accept
    (blob validator mein koi channel-dup check nahi).
  - ADS1115 channel 4: `params[0] = 4` → Rust
    `["axis_ads1115: channel 4 out of range (0-3)"]`, C++ accept.
  - LED count 61: `06 00 00 15 ff ff 3d …` → Rust
    `["led: count 61 out of range (1-60)"]`, C++ COMMIT par accept karta hai
    (count blob-validate mein check nahi hoti).
  - Expander pin 16 (MCP23017): `08 00 00 ff ff ff 10 …` → Rust
    `["expander_button: pin 16 out of range for MCP23017 (0-15)"]`,
    C++ accept (blob validator `params[0]` check nahi karta).
  - Asar sirf UX hai (board wali config app rok deti hai) — safety issue
    nahi. Faisla chahiye: Rust ko C++ jaisa loose karo ya sakht rehne do
    (Python reference validator bhi sakht hai — Rust us se match karta hai).
- **BUG-6 — count divergences (code-level, GPIO pin budget ki wajah se
  aam config mein unreachable).**
  - BUTTON_MATRIX: C++ hamesha 16 buttons ginta hai, Rust rows×cols
    (2×3 → 6). 9× 2×3 matrices C++ mein 144 buttons (> 128 reject), Rust
    mein 54 (accept) — lekin 45 distinct GPIO pins chahiye, board par
    nahi hain; aur C++ matrix `params[2..]` pins check nahi karta jab ke
    Rust karta hai (Rust stricter).
  - Buttons 65..128: C++ COMMIT par UNSUPPORTED (err 3, N1 — wire slot
    nahi hai); Rust accept karta hai. Aam GPIO-bound config se 65 buttons
    tak pahunchna pin budget ki wajah se mumkin nahi (4 matrices × 8 pins
    = 32 pins > ~17 usable GPIO).
  - Expander records: C++ `n_btn` mein gine hi nahi jate (129 expander
    buttons bhi COMMIT pass karte hain); Rust + Python ref unhein ginta
    hai (128 limit). Rust yahan Python ref se match karta hai, C++ se nahi.

### 6.4 Fix ka daira — FIXED (Phase D, 2026-11-14)

Fix **siraf `crates/rsh-protocol/src/pin.rs`** mein hua
(`blob_record_module` / `shifter_errors` / `validate_blob` + tests) —
`config.rs` mein koi mapping error nahi mila, is liye wahan **koi edit
nahi** ki gayi. Regression tests mein upar wale exact record bytes
hain (`pin.rs` tests, `blob_from_hex` helper).

- **BUG-1 — FIXED.** pinB/pinC ko C++ jaisa class-only check mila
  (barred / duplicate / I2C-reserved, koi electrical rule nahi):
  `AXIS_ADC` / `AXIS_GENERIC` / `BUTTON` / `BATTERY` ke pinB+pinC,
  `ENCODER` ka pinC, aur C++ generic loop ke mutabiq `LED_STRIP` ke
  pinB+pinC bhi. `0xFF` pins "absent" rehte hain, kabhi check nahi hote.
  Tests: `bug1_axis_adc_pinb_flash_rejected`,
  `bug1_button_pinb_flash_rejected`,
  `bug1_axis_adc_pinc_flash_rejected`,
  `bug1_pinb_duplicate_of_pina_rejected`,
  `bug1_pinb_i2c_reserved_rejected`,
  `bug1_led_strip_pinb_flash_rejected`.
- **BUG-2 — FIXED.** ENCODER pinC ko class-only check mila; 1-pin
  encoder record ab drop nahi hota — us ka ek pin module mein rehta hai
  (to `validate()` "needs exactly 2 pins" deta hai) aur har ENCODER
  record `n_enc` mein gina jata hai, is liye 5× 1-pin encoders
  encoder-limit error dete hain. Tests:
  `bug2_encoder_pinc_flash_rejected`,
  `bug2_single_one_pin_encoder_errors`,
  `bug2_five_one_pin_encoders_hit_limit`.
- **BUG-3 — FIXED.** AS5600: flags sirf bit0, rotation (`params[1]`)
  ≤ 2, pinA/B/C `0xFF` lazmi — teeno check `blob_record_module` mein
  hain (C++ wali tartib: flags, rotation, pins). Tests:
  `bug3_as5600_gpio_pin_rejected`, `bug3_as5600_flags_rejected`,
  `bug3_as5600_rotation_rejected`.
- **BUG-4 — FIXED.** `validate_blob` ab record loop se pehle I2C
  kinds ke liye pre-scan karta hai (C++ R-L9) aur GPIO21/22 ko `used`
  map mein reserve karta hai — is liye SHIFTER gear-pin check (aur
  pinB/pinC check) I2C reservation dekhte hain, record order se
  aazaad. Test: `bug4_shifter_gear_pin_21_with_i2c_rejected`.
- **BUG-6a — FIXED.** `BUTTON_MATRIX` count C++ jaisa 16 fixed hai
  (rows×cols nahi) — blob-level record count mein; module-level
  `validate()` Python reference jaisa rows×cols hi ginta hai. Rust ke
  sakht `params[2..]` pin checks unchanged hain. Test:
  `bug6a_nine_small_matrices_hit_button_limit` (9× 2×3 → 144 > 128).
- **BUG-6b — FIXED.** 65–128 buttons `validate_blob` mein reject hote
  hain: `too many buttons (N > 64): buttons 65-128 have no wire slot
  (firmware UNSUPPORTED)`. Tests: `bug6b_65_buttons_no_wire_slot`,
  `bug6b_64_buttons_wire_slot_ok` (64 boundary accept).
- **BUG-5 — KEPT STRICT (coordinator decision, koi code change nahi).**
  ADS1115 duplicate channel / channel 4, LED count 61, expander pin 16
  — Rust C++ se sakht rehta hai (safe direction, Python reference se
  match).
- **BUG-6c — KEPT STRICT (coordinator decision, koi code change nahi).**
  Expander records Rust ke `n_btn` mein gine jate hain (Python
  reference se match, C++ se nahi).
- Guard test: `factory_blob_still_valid` (factory blob ab bhi valid).
- `cargo test -p rsh-protocol` → **89 passed (60 unit + 18
  golden-vector + 10 property + 1 doctest), 0 failed**; `cargo clippy
  -p rsh-protocol --all-targets -- -D warnings` → **0 warnings**;
  `cargo fmt --check -p rsh-protocol` → **clean**.

### 6.5 Same-class follow-ups — FIXED (Phase D, coordinator, 2026-11-14)

The pin-fix worker's report flagged three more same-class gaps (Rust
accepts, C++ rejects). All three are now fixed in `pin.rs` too:

- **G1 — I²C kinds (ADS1115 / expander) GPIO pins — FIXED.** C++
  (`rsh_cfg_blob_validate`, I2C-kinds branch) requires pins r[3..5] all
  `0xFF` on ADS1115 / EXP_MCP23017 / EXP_PCF8574 records; Rust ignored
  them. `blob_record_module` now returns
  `axis_ads1115: uses I2C, no GPIO pin` /
  `expander_button: uses I2C, no GPIO pin`. Tests:
  `g1_ads1115_gpio_pin_rejected`, `g1_expander_gpio_pin_rejected`.
- **G2 — Sequential SHIFTER (mode 0) pins — FIXED.** C++ falls through
  to the generic loop for mode 0, so pinA/B/C get the class-only check;
  Rust returned early with no check at all. `shifter_errors` now
  class-only-checks mode-0 pins (inserting passing pins into `used`),
  and `validate_blob` cross-checks them against later module pins via
  the new `shifter_pins()` helper (`seq_pins`, same post-pass as the
  H-pattern gear pins). Tests:
  `g2_sequential_shifter_flash_pin_rejected`,
  `g2_sequential_shifter_pin_blocks_later_button`,
  `g2_sequential_shifter_valid_pins_ok`.
- **G3 — Record-based axes/battery limits — FIXED.** C++ counts EVERY
  axis record (`n_axes++` per AXIS_ADC/ADS1115/GENERIC/AS5600) and
  every battery record (`n_bat++`) toward the limits, including pinless
  ones that map to no module; Rust only counted modules, so 9× pinless
  axes or 2× pinless batteries passed. `blob_limit_errors` now counts
  records (`too many axes (N > 8)`, `too many battery modules (N > 1)`)
  and `validate()`'s own axes/battery limit errors are filtered out
  (same as encoders/buttons) so a blob is never rejected twice for one
  limit. (LED strips need no record count: C++ counts only pinned LED
  records, exactly like the module path.) Tests:
  `g3_nine_pinless_axes_hit_limit`,
  `g3_eight_pinless_axes_no_limit_error`,
  `g3_two_pinless_batteries_hit_limit`.

## 7. Release process (FW-A3 ka hissa — detail `REFLASH-GUIDE.md` mein)

- Signed bins: `pio run` ke baad `sign_firmware.py` (post-build script)
  `firmware-signed.bin` banata hai = `firmware.bin` + **68 B trailer**,
  sirf jab `RSH_SIGNING_KEY=<Ed25519 private key PEM ka path>` env var set
  ho. Key set nahi → signing skip, dev build unsigned; key set lekin file
  nahi / sign fail → **build zor se fail** (khamoshi se unsigned release
  nahi). Purana stale signed bin keyless build par delete kar diya jata hai.
- Signing key: **existing keypair reuse, naya nahi** — private key
  `~/workspace/projects/rsh/signing-keys/` mein (repo se bahar, 0600) aur
  CI secret `RSH_SIGNING_KEY_PEM`; public-key fingerprint
  `SHA-256 387764a4529337c437b233a148b48f9b2ffe3f3b5015e14c95737cb3a67a6411`.
  Key **kabhi** repo/chat/log mein nahi aati — is doc mein sirf path aur
  fingerprint hai.
- Board + app unsigned image ko reject karte hain (SEC-C-06, verify-before-
  apply + anti-downgrade). Release assets: merged bin
  `RSH-Firmware-DevKitV1-v4.0.1.bin` (956,032 B, Windows release runner;
  Linux `pio run`: 955,984 B — host-toolchain variance, same source) +
  `firmware-signed.bin` (890,564 B) + `SHA256SUMS`. Release workflow mein fail-closed verify hai
  (signature + signed-region hash vs `firmware.bin`).
- **Rust app v1.0.0 ko koi reflash nahi chahiye** — wahi board, wahi
  firmware v4.0.1, protocol frozen. Reflash sirf naye board ya brick
  recovery ke liye hai (procedure: `docs/REFLASH-GUIDE.md`).

## 8. Open items / carried conditions

- **FW-A4 / A7 real-board run — CARRIED, is sandbox mein impossible.**
  Live board ke baghair HELLO/handshake, session, GET_CAPS, SET_CFG
  commit/confirm/rollback, HMAC control, LED_TEST, PIN_PROBE, NTP ping aur
  wired pairing ka board-level proof nahi ho saka. A7 runbook
  (`reference/docs/A7-RUNBOOK.md`) ke mutabiq final release se pehle user
  ke board par: flash signed merged bin → app chalao → 10-min run
  (sent/s 195–205, overruns 0, maxCycleUs < 3500 µs, reboots 0). **A7 ke
  baghair Rust v1.0.0 final release nahi.**
- **BUG-1–BUG-4, BUG-6a, BUG-6b (§6) — FIXED (Phase D, §6.4).**
  BUG-5 / BUG-6c coordinator decision se sakht hi rakhe gaye hain.
- **4-hour soak** (C# series se carried, user ne ~16 min mein roka tha) —
  Rust rig soak FW-D mein.
- **Flash encryption + Secure Boot board-side (OPEN-3 / SEC-H-04)** — C++
  track mein existing procedure se, sacrificial board pehle; is doc ka
  hissa nahi.
- OV code-signing certificate deferred hai (user faisla 2026-11-13) — app
  release unsigned jayegi; firmware Ed25519 signing is se mutasir nahi.
