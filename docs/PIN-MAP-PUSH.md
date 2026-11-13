# RSH — Pin-Map Board Push (Phase F PC-side, Phase G board-apply C3)

**Status: COMPLETE end-to-end (sandbox) — 2026-11-13, Phase G WS2.
PC-side push (Phase F W3) + firmware board-apply (Phase G C3) + mDNS
responder (Phase G C2) sab implemented aur Virtual Test Rig mein verified.
Baaqi sirf real-board verify (Phase H A7) aur Windows `rsh.local`
verify (Phase H) — neeche § "Baaqi — Phase H".**

## Kya ban gaya (PC-side, Phase F W3)

- **Blob builder** — `RSH.Core/Protocol/ConfigBlob.cs` (RSHG blob, §3.6:
  header 8 B + 16 B records + CRC32 trailer) + `RSH.Core/Protocol/Crc32.cs`
  (ISO-HDLC, firmware `rsh_crc32` jaisa; check value `0xCBF43926`).
- **Pin-map → blob mapping** — `PinMapBoardPush.BuildBlob` (`RSH.App/ViewModels/PinMapStore.cs`):
  Steering/Accel/Brake/Clutch/Handbrake → AXIS_ADC (type 1, FIXED slots
  0/1/2/3/4 — assign/unassign se doosre slots nahi badalte); Battery →
  BATTERY (type 7); Shifter up/down + Horn → BUTTON (type 3, slots 0/1/2);
  Encoder 1/2 → ENCODER (type 5, pinA/pinB); LED strip → LED_STRIP
  (type 6); Status LED → blob mein NAHI (onboard LED, koi blob type nahi);
  not-assigned function → koi record nahi.
- **Transaction** — `PinMapBoardPush.PushAsync`: validate
  (`PinMap.ValidateAll`, error > 0 par koi frame nahi) → SET_CFG chunks
  (§3.5: offset|total|chunkLen|chunkCrc|data, chunk ≤ 1024 B, chunkCrc =
  CRC16-CCITT) → COMMIT → CONFIRM. Nakami par REVERT (best-effort) staged
  transaction discard karta hai.
- **Wiring + UI (done)** — `BoardSession.SendControlAsync`
  (`RSH.App/ViewModels/DiagnosticsSettings.cs:406-431`) + Settings page mein
  "Push to board" button (`RSH.App/Views/Pages/SettingsPage.xaml:75`,
  `PushPinsToBoardCommand` → `PinMapBoardPush.PushAsync`).
- **Tests** — `RSH.App.Tests` (fake board peer: firmware transaction
  semantics — stage + CRC16 check + COMMIT par blob validate + CONFIRM):
  happy path (SET_CFG → COMMIT → CONFIRM, blob mein steering pin 34),
  invalid map par 0 frames, COMMIT fail par REVERT.

## Board-apply (Phase G C3) — blocker RESOLVED

Phase F ka exact blocker tha: firmware SET_CFG blob NVS mein store karta
tha lekin `inputs.cpp` compile-time `PIN_*` se parhta rehta tha — push
accept hota tha, wheel purane pins par chalta tha. **Phase G WS2 mein fix
hua — ab blob hi pins ki sach hai:**

- **Pin table** — `firmware/rsh/pinmap.h` (NEW): `RshPinMap` struct,
  `rsh_pinmap_defaults()` (:82), `rsh_pinmap_from_blob()` (:112,
  validate-then-parse, first-wins duplicate slot, aadha encoder → dono
  absent, SHIFTER gear pins 1–6), `rsh_pinmap_blob_is_legacy()` (:215).
  **Replace semantics:** jis function ka record blob mein nahi, wo sample
  hi nahi hota (pin `0xFF`).
- **Factory blob** — `firmware/rsh/config_transaction.h:177`
  `rsh_cfg_factory_blob()`: poora 11-record map, 188 B, cfgVersion 1 —
  AXIS 0:34, 1:35, 2:36, 4:32 (handbrake); BATTERY 0:39; BUTTON 0:27,
  1:33, 2:4 (horn); ENCODER 0:23/19, 1:16/17; LED_STRIP 0:21. Clutch
  default OFF, shifter sequential default. Purana 7-record factory blob
  (BUTTON slots 0..3) legacy hai — boot par naye factory blob se migrate
  hota hai (`rsh.ino:156-160`).
- **Inputs live map se** — `firmware/rsh/inputs.cpp:55`
  `inputs_apply_config_blob()`: invalid blob par no-op (fail closed);
  map badle to pinMode re-init + debouncer/encoder/health/prev-state reset
  + mV probe force; same map dobara apply ho to no-op.
  `inputs_pin_map()` (`inputs.cpp:88`) live table deta hai. Tamam reads
  `pinMap` se hain (steering/accel/brake/handbrake/clutch/encoders/H-pattern
  gear pins/gear-up-down/horn); absent axes rail-stuck health check skip
  karte hain. Clutch ab kisi bhi ADC1 pin par ho sakta hai (pehle sirf 33).
- **Modules presence-driven** — `firmware/rsh/modules.h:346`
  `rsh_modules_apply_config_blob()`: clutch/handbrake/encoders/LED/horn/
  shifter enable blob-record presence se.
- **Apply trigger** — `firmware/rsh/rsh.ino`: boot-from-NVS apply
  (`inputs_apply_config_blob`, :288, `inputs_begin()` ke baad) + loop
  detector (:206-218) jo `cfgTxn.current` par **poora-blob `memcmp`**
  karta hai aur badalne par modules + inputs dono apply karta hai
  (COMMIT, REVERT, tick-rollback, boot recover — sab cover).
- **GET_CAPS live table se** — `firmware/rsh/net.cpp:505-520`: pin table
  current blob se banti hai (`modules.h:185`
  `rsh_caps_table_from_pinmap()`); blob invalid ho to compiled default
  table (fail closed). Factory map ke liye table default table ke barabar
  hai (15 rows); clutch record ho to clutch row bhi aati hai.
- **Observability** — status packet mein `st.cfg_crc` (current blob ka
  CRC32) aur `st.cfg_ver` (`rsh.ino:513`).
- **30 s rollback** — confirm timer ke mutabiq: COMMIT ke 30 s mein
  CONFIRM na ho to transaction rollback (purani config + purane pins
  wapas); REVERT bhi wahi karta hai. Rig mein tick-rollback verify hua
  (wheel.py `ControlChannel.tick`, self-check §4d).

## mDNS responder (Phase G C2)

- **Firmware** — `firmware/rsh/net.cpp:221`: `MDNS.begin(RSH_MDNS_HOSTNAME)`
  + services `rsh` TCP 4212 (control) aur `http` TCP 80 (WebOTA); ek dafa
  start ho to dobara begin nahi; begin fail ho to agle `net_start` par
  dobara koshish. Hostname `rsh` (`firmware/rsh/config.h:116`
  `RSH_MDNS_HOSTNAME "rsh"`) — PC-side `rsh.local` se milta hai
  (`RSH.Core/Net/EspAddress.cs`, `MdnsHostname = "rsh.local"`, fallback
  192.168.4.1).
- **Sandbox verify:** `pio run` se compile + hostname contract; Windows par
  `rsh.local` resolution Phase H mein (neeche).

## Verification (Phase G WS2, Virtual Test Rig — LOCAL builds)

- `pio run` **SUCCESS** (RAM 25.9% — 84,952/327,680 B; Flash 66.6% —
  872,865/1,310,720 B; merged bin `RSH-Firmware-DevKitV1-v4.0.0.bin`).
- Host tests `run_tests.sh` **ALL PASS** — `test_main.cpp` 4239 checks
  (naya §25a pinmap: defaults, factory parse, custom blob, invalid
  fail-closed, legacy detection, first-wins duplicate, half-encoder,
  H-pattern, GET_CAPS table comparisons), `test_host_cpp.cpp` 50 checks
  ×2 modes (pin-apply end-to-end real `inputs.cpp` se: steering read
  pin 34→32 move, gear-up debounce nayi pin par, absent horn par koi bit
  nahi, same-blob re-apply no-op, bad-CRC reject, factory rollback),
  Python cross-verify 51, merge_bin mock 7.
- Wheel self-check **PASS** — §4c GET_CAPS dynamic table (15 rows) +
  §4d pin-map transaction: out-of-order SET_CFG chunks → COMMIT (deadline
  30) → GET_CAPS nayi map → GET_CFG round-trip → CONFIRM persistent;
  REVERT purani map; 30 s timeout rollback + late CONFIRM UNSUPPORTED;
  bad-CRC blob COMMIT BAD_CRC; duplicate pin PIN_CONFLICT; bad chunk CRC
  reject; SET_CFG jab COMMITTED ho to BUSY.
- Full rig (quick, run 2026-11-13T07:30:39Z) — **25/25 scenarios PASS**
  (1 skipped: long-soak, `--long` only), jin mein `config-transaction`,
  `pin-map-push`, `control-channel`, `fault-reboot` (duplicates 0),
  soak 60 s (loss 0%, rate 202/s, jitter p99 934 µs) shamil hain.
  Firmware-host stage PASS. Unit stage mein `dotnet-test` FAIL tha —
  9 failures **sirf WS1 (serial transport) ke tests** mein, WS2 scope se
  bahar (koi C# change nahi); host-tests / pin-rules / protocol-vectors
  PASS. (Detail: Phase G WS2 gate report.)

## Bugs mile + fix hue (Phase G C3)

1. **W4-reported tree breakage** (stale report — fix se pehle hi ho chuka
   tha): `pinmap.h` mein `RSH_SHIFTER_HPATTERN` macro-order + `inputs.cpp`
   mein movement statics order. `pio run` green.
2. **`.ino` change-detector latent bug:** version+length sentinel same-version,
   same-length re-push par apply skip kar deta (PC `PushAsync` default
   cfgVersion = 1). Fix: poora-blob `memcmp`.
3. **Factory-blob / BUTTON slot-scheme inconsistency:** purana factory blob
   BUTTON slots 0..3, PC builder 0/1/2. Fix: naya 11-record factory blob +
   legacy detection + boot migration.
4. **Clutch preset pin 35 silently disabled:** purani `modules.h` clutch
   sirf pin 33 par allow karti thi — full-rig preset (clutch 35) kabhi
   kaam hi nahi karta tha. Fix: pinmap-driven clutch reads.
5. **GET_CAPS mein clutch row ghaib thi:** default table mein clutch row
   is liye nahi thi ke pin 33 gear-down row mein tha — blob mein clutch
   aur gear-down ek pin par kabhi nahi hote (validator PIN_CONFLICT), is
   liye clutch row add ki (factory table ab bhi default ke barabar hai).
6. **PROTOCOL §3.6 SHIFTER text ghalat tha** (doc bug): pinA/B/C +
   params[1..3] = gears 1–6 hai (doc kehta tha gear up/down + gears 1–4).
   §3.6 mein slot semantics table + factory blob + §3.5 chunk-order rule
   (pehla chunk offset 0 ka hona chahiye) bhi theek kiye gaye.
7. **Wheel emulation fidelity gap (pre-existing):** emulated GET_CAPS
   pinCount 0 kehta tha; ab asli pin table deta hai (factory map ke liye
   purani fixed table jaisi hi rows).

## Baaqi — Phase H

1. **Real-board verify (A7):** pin badlo → SET_CFG push → nayi pin par
   input aaye (serial log se), phir 30 s mein CONFIRM na ho to rollback
   purani pins par; legacy NVS blob wala board boot par migrate ho.
2. **Windows `rsh.local` verify:** mDNS responder compile-verified hai;
   Windows PC par `rsh.local` resolution + fallback 192.168.4.1 check karna.
3. Har push ke baad recalibration prompt (pin badle to calibration stale —
   `CheckRecalibration` pehle se hai).
