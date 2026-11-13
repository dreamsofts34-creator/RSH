# RSH — Decision Log

**Status:** D1–D35 **CLOSED** (Plan v2.3 §0, 12 Nov 2026). Ye file Plan v2.3 §0 ki repo copy hai — §0 hi authority hai; yahan koi faisla badla nahi gaya.
**Scope:** Phase A work package **WP5** (Phase A, RSH-PLAN-v2.3.md §5).
**Naming:** Product ka naam **RSH** hai — purana naam final product mein kahin nahi hai (rename: Plan v2.3 §4).

## D1–D35

| ID | Faisla | Decision | Source | Phase |
|---|---|---|---|---|
| D1 | Button encoding | Stream packet mein buttons **u32 fixed** (bit n = button n, n ≤ 31); 33+ buttons ke liye status packet mein alag `btnsEx u32`. `ceil(nBtn/8)` design **khatam**. | T3 / R-H8 | A (WP3) |
| D2 | Stream CRC | **CRC16-CCITT** (poly 0x1021, init 0xFFFF, CRC field se pehle ke tamam bytes par) — v1 jaisa. crc8 ka idea **khatam**. F14 = stream + control dono ke liye table-driven CRC16. | R-H20 | A (WP3) / D (F14) |
| D3 | Control frame reqId | Layout: `magic "RSHC" \| cmd \| reqId u16 \| counter u32 \| len u16 \| payload \| tag[16]`. Retry mein **reqId wahi, counter naya**; board (reqId, counter) pair se duplicate pehchanta hai. Counter **per-session** hai — naya nonce = counter 0 se shuru. | R-H12 | A (WP3) |
| D4 | Flags bits | bit0 config-changed, bit1 low-supply, bit2 tick-skipped, bit3 axis-invalid (F2), bit4–7 reserved (0). | R-M22 | A (WP3) |
| D5 | Status packet | **34 B**: purane 30 B + seqGap u16 (30–31) + txFail u16 (32–33), CRC16 aakhir mein. Wi-Fi channel **GET_CAPS reply** mein jayega. | R-H9 | A (WP3) |
| D6 | Axes units | **Sab axes mV** honge — mvMask khatam. Bounds: nAxes ≤ 8, nBtn ≤ 128, nEnc ≤ 4 — is se zyada ho to `BAD_FRAME` reject. `rsv` hamesha 0; nonzero ho to packet drop + count. | R-M1 / R-M25 | A (WP3) |
| D7 | P1 steering scale | **Divide** hoga: `steer / scale`. Presets ka matlab rotation range hai (F1 360° = 0.4 → full lock), output limit nahi. Test: scale 0.4 + steer 0.4 → full lock. | R-M1 | B |
| D8 | P8 loss percent | **Sliding window** (last 1000 packets) — reset wala tareeqa khatam. | R-M1 | B |
| D9 | P2 buttons > 32 | Jab tak v2 na ho, button > 32 ko **reject karo — error dikhao**, clamp kabhi nahi (clamp bhi silent remapping hai). | R-M5 | B |
| D10 | P9 calibration span | Span ≥ 20% of **full-scale mV range (0–3300 mV)**. Unit tag `IsMillivolts` lazmi. | R-L10 | B |
| D11 | P12 vJoy validation | Validate **configured modules ke khilaf** karo — fixed X/Y/Z ya ButtonCount ≥ 2 ki shart nahi (steering-only wheel bhi valid hai). | R-H14 | B |
| D12 | F2 floating criteria | Invalid sirf tab jab (a) value 10 s tak exact rail (0 ya 4095) par stuck ho **aur** doosre axes move kar rahe hon, ya (b) variance threshold se zyada ho (floating noise). Sirf variance≈0 kabhi invalid ki wajah nahi. PIN_PROBE se confirm karo. | R-H11 | D |
| D13 | F4 timestamp | Timestamp **ADC read ke baad** liya jayega (midpoint). | — | D |
| D14 | F7 pedal analog | Button mode mein analog **skip** hoga (sirf 10 Hz diagnostic). | — | D |
| D15 | F11 loop timing | **Poori loop iteration** napo (OTA/WebServer/net_poll samet); WebServer/OTA ≤ 300 µs/pass ya Core 0 task. | — | D |
| D16 | F5 governor level 3 | Level 3: **4 samples/axis, 100 pps fallback**, flag status mein. Recovery: ≤1 overrun wali window bhi `goodWindows` gine. | R-L1 | D |
| D17 | F10 heap threshold | Restart jab largest free block **< 8 KB lagatar 10 cycles** tak ho, ya free heap 5 min mein 20% se zyada gire. Threshold NVS/config mein hoga, hard-code nahi. | R-M21 | D |
| D18 | A6 vJoy resolve | Resolve: registry → `%ProgramFiles%` / `%ProgramFiles(x86)%`. Na mile to vJoy download link dikhao — DLL **bundle kabhi nahi** hogi. | R-H13 | E |
| D19 | A7 ARM64 | **Explicitly unsupported** (win-x64 publish). Future path: user-mode driver (HIDMaestro) ARM64 builds rakhta hai — v2 mein evaluate nahi, Phase G/H mein dekha jayega. | — | E |
| D20 | M5 charts | **ScottPlot rehta hai** (already integrated, tested) — DrawingVisual par switch nahi. | R-L4 | E |
| D21 | N8 multi-device | v2 mein **single device** hi support hoga. Profile v2 mein `boardMac` (optional) field hogi; doosre board par profile khole to calibration warning ke saath dobara karni hogi. | R-M18 | C |
| D22 | Packaging | **MSIX** primary (Microsoft recommended). WiX drop. MSIX Identity Publisher == certificate subject (exact match) lazmi. | — | H |
| D23 | Signing | **OV certificate** ($150–300/year, procurement Phase A mein shuru — validation mein din se hafte lagte hain). **USER DECISION 2026-11-13: OV cert abhi NAHI khareeda jayega — v2.0.0 UNSIGNED ship hoga (app + MSIX); SmartScreen warning expected; cert baad mein. Procurement checklist (§6 SIGNING.md) "baad mein" ke tor par darj, release blocker nahi. MSIX Publisher abhi `CN=RSH` (unsigned identity); cert milne par Publisher = exact cert Subject + `-Sign` build.** Azure Artifact Signing **Pakistan-based individual ko nahi milta** — ye rasta band hai. Fallback: Microsoft Store MSIX (registration fee May 2026 se waived; Store re-sign karta hai — day-one SmartScreen warning se bachta hai). Signing SmartScreen warning khatam **nahi** karta — reputation hafton mein download volume se banti hai; publisher identity har release mein aik jaisi rakho. Firmware signing (Ed25519, local key) is faisle se mutaliq nahi. | R-M28 | A (WP8) / H |
| D24 | WebOTA | **Keep + auth fix** (F1) — band nahi hoga. Har endpoint par `authenticate()` lazmi. | F1 | D |
| D25 | Firmware framework | **Arduino rehta hai v2 ke liye** (tested code; Arduino-ESP32 3.x ESP-IDF par built hai). ESP-IDF documented target hai rewrite / naye board (S3) ke liye — incremental migration, rewrite nahi. | — | D |
| D26 | Wired transport | **DECIDED (Phase G)** — USB-serial transport implement hua (framing, CRC, reconnect, USB-serial recovery); dual transport (Wi-Fi primary, wired optional/alternate); pairing/key transfer dono par (OPEN-20). | R-M26 | G |
| D27 | Version scheme | Firmware **v4.0.0**, app **v2.0.0**, protocol **v2.1**. Tag: app release `v2.0.0`, firmware `fw-v4.0.0`. Dono versions GET_CAPS aur app About mein dikhengi. | R-M27 | A / H |
| D28 | Confirm timer | **Hamesha 30 s** — reboot ho ya live apply, ek hi value. | R-M6 | A (WP3) / C |
| D29 | Boot fail | Boot fail = power-on ke **15 s** ke andar AP up na ho. 3 lagatar fail ⇒ last-known-good, phir factory. | R-M7 | D |
| D30 | HELO nonce | Nonce **sirf tab** rotate hota hai jab board reboot ho ya PC naya session maange (HELLO mein session flag); routine 1 Hz HELO purana nonce wapas deta hai. Counter session ke andar monotonic rehta hai. | R-M30 | A (WP3) |
| D31 | Telemetry default | Default **OFF**; pehli crash-dump export par user se poocha jayega. | R-L7 | E / H |
| D32 | vJoy keepalive | Keepalive **rehta hai**, lekin Phase B mein verify hoga — agar zaroori na ho to hatao (kuch games idle device ko drop karte hain — unverified). | R-L2 | B |
| D33 | Default address | Default **192.168.4.1 configurable** rahega; mDNS `rsh.local` pehle try hoga, fallback default IP. | R-L3 | A (WP3) |
| D34 | AS5600 | AXIS_AS5600 sirf **≤ 360°** rotation ke liye; 900° ke liye **turn counter** (index pulse ya software turn tracking) lazmi hai, warna ENCODER_STEERING use karo. | R-M8 | G |
| D35 | Full-rig preset | Purane plan ke 15 buttons MCP23017 ke bagair fit nahi hote (11 digital pins), is liye direct-pin full-rig **7 buttons** hai; 15 buttons ke liye MCP23017 preset use karo. | R-M19 | D |

**Count check:** D1–D35 = **35/35** — koi faisla miss nahi.

## Naye faisle

Naye faisle yahan add honge — har faisle ke saath date aur phase.
