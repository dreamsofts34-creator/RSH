# ⚠️ SUPERSEDED — ye document ab authority NAHI hai (Architecture polish sweep, 2026-11-13)

**Ye firmware v3 (2026-11-10) ki test report hai — tareekhi record ke tor par rakhi gayi hai.
Current test evidence: `docs/RELEASE-CHECKLIST.md` (RUN LOGs: RSH.Tests 699 pass v2.0.3 gate,
rig 25/25, host tests 4374 checks 0 failures) + `test-env/REPORT.md`.**

# RSH Firmware — Test Report (2026-11-10, DevKit V1, v3 — advanced features)

## v3 advanced features (owner ki request par)
| Feature | Tafseel | Hifazat/notes |
|---|---|---|
| OTA update | WiFi par firmware update, hostname `rsh`, ArduinoOTA | Password lazmi (config mein badlo); update ke doran sampling band, LED 5 Hz |
| Auto WiFi channel | Boot par scan, 1/6/11 mein sab se khaali (padosi overlap score ke sath) | `WIFI_CHANNEL` 1-13 rakho to fix channel; scan sirf boot/retry par |
| Adaptive sampling | Governor: overruns zyada hon to samples 16/8 → 12/6 → 8/4; 5 saaf windows par wapas upar | Host tests se transitions verify; packet format/protocol bilkul tabdeel nahi |
| Remote restart | Port 4210 par `RSTX` packet → saaf restart | Sirf WPA2 AP se jura device bhej sakta hai; flag se band ho sakta hai |
| Low-heap safety | Free heap < 15,000 ho (10 s check) to atakne se pehle restart | Threshold config mein |

## v3 verification
- Host tests: **536 checks, 0 failures** (governor transitions ke naye tests shamil)
- DevKit V1 compile (default config, OTA ON): **SUCCESS** — Flash 78.6% (1,030,327 B) / RAM 15.8% (51,912 B)
- Compile ke doran ek asli ghalti pakri gayi aur fix hui: config.h include order (OTA include config se pehle tha) — pehla v3 build FAIL hua, fix ke baad PASS. Yehi wajah hai ke compile check har dafa lazmi hai.
- Board-par-only (pehle se open): OTA ka asal amal, auto channel ka tumhare mahol mein natija, governor ka asal timing par asar — teeno pehli flash par debug stats se confirm honge.

---

# Pichli report (v2 — deep audit)

## Deep audit (do lanes: apna pass + independent review) — mile hue bugs aur fixes

| # | Bug | Severity | Fix |
|---|---|---|---|
| A1 | Port 4210 par 16 byte se bara datagram aye to HELO reception **hamesha ke liye** ruk jati thi (core mein rx_buffer free hue baghair parsePacket hamesha 0; sirf reboot hal tha) | HIGH | Buffer 128B + har datagram ke baad `flush()` se remainder discard; core source se verify kiya |
| A2 | WiFi start fail ho to error LED foran aam blink se overwrite ho jata tha, retry kabhi nahi — fail-safe asal mein kaam hi nahi karta tha | HIGH | Latched error state: tick processing band, sirf error LED + har 5 s `net_begin()` retry |
| A3 | `analogReadMilliVolts` calibration fail par khamoshi se 0 — steering har packet mein 0 jata, flags phir bhi "mV mode" kehta | MODERATE | Startup cross-check (raw vs mV); mismatch par raw mode fallback, flags asal mode batate hain, debug output mein `mv=` |
| A4 | ADC channel switch ke baad pehli reading pichle channel ki taraf bias (steering ka asar pedals mein) | LOW–MOD | Har channel ki pehli conversion dummy read karke discard |
| A5 | `udpHello.begin()` ka result ignore hota tha — receive socket fail bhi "sab theek" kehlata | LOW | Return check, fail par net_begin false |

## Advance improvements (audit + apni lane)
- **Trimmed mean** steering par: 16 samples sort karke dono taraf ke 2 extreme hata kar average — ADC spike/dropout steering ko nahi hilata (host test shamil).
- **Loop yield:** deadline door ho to `delay(1)` — core be-wajah 100% spin/garam nahi hota.
- **Drain cap:** net_poll per call zyada se zyada 8 datagrams — flood sampler ko starve nahi kar sakta.
- **Password hifazat:** WPA2 length (8–63) par compile-time `static_assert` — ghalat password board par khamosh fail ki bajaye compile par hi rukega; default password par sakht warning comment.
- **Overrun accounting** durust: ek stall ab do dafa nahi gina jata; release build se dead counter hataya.
- Config comment durust: GPIO2 LED istisna ki wajah wazeh likhi.

## Check kiya gaya — bug NAHI tha (dobara shak na ho)
- `udpData.begin()` ke baghair `beginPacket()` — core 3.3.12 socket khud banata hai (NetworkUdp.cpp source se verify).
- CRC, scheduler wrap idiom, millis subtraction, debounce, LED rates — sab spec ke mutabiq.

## v2 verification
- Host tests: **527 checks, 0 failures** (trimmed mean ke naye tests shamil)
- Python cross-verify: PASS (protocol tabdeel nahi hua)
- DevKit V1 compile: **SUCCESS** (fixes ke baad dobara)

## Verified (saboot ke saath)

| # | Item | Natija | Saboot |
|---|---|---|---|
| 1 | CRC-16/CCITT-FALSE standard vector "123456789" | PASS | Host test: 0x29B1 (RevEng catalogue value) |
| 2 | CRC do independent implementations (bitwise vs table) 500 random buffers par mutafiq | PASS | Host test |
| 3 | Packet layout golden bytes (18B, LE, docs 02 ke mutabiq) | PASS | Host test byte-by-byte memcmp |
| 4 | Packet boundary values (seq/ts/analog max) + CRC roundtrip | PASS | Host test |
| 5 | Button debounce (2-sample, bounce reject, press+release) | PASS | Host test |
| 6 | Scheduler wrap-safe comparison (micros 32-bit wrap ke paar) | PASS | Host test |
| 7 | Sequence wrap 0xFFFF → 0 | PASS | Host test |
| 8 | **Host tests total: 523 checks, 0 failures** | PASS | `tests-host/run_tests` |
| 9 | Independent Python parser (sirf docs spec se) — fields + CRC + button/flag bits | PASS | `tests-host/verify_packet.py` |
| 10 | Asli compile — ESP32-C3 target (pichla version) | PASS | PlatformIO SUCCESS, Flash 76.1% / RAM 11.7% |
| 11 | Asli compile — **ESP32 DevKit V1 (esp32dev) target, final version** | PASS | PlatformIO SUCCESS, Flash 71.4% (935,796 B) / RAM 14.5% (47,592 B), firmware.bin bana |

## Research se apply ki gayi corrections
- C3 par 11 dB ADC range asal 0–2500 mV thi (datasheet) — DevKit V1 par ye masla hi nahi (3.3 V tak, full-scale 3.9 V VDD-limited), resistor ki zaroorat khatam (owner ke paas resistor nahi tha).
- Tamam analog sirf ADC1 par (DevKit: GPIO34/35/36); ADC2 WiFi ke saath band.
- TX power default full — kam power sirf kharab-revision C3 boards ka workaround tha, fix nahi.
- analogReadMilliVolts eFuse calibration use karta hai (flags bit0 PC ko batata hai).
- UDP receive har loop mein drain hota hai (queue bharne par khamosh packet loss se bachao).
- setSleep(false) rakha hai lekin AP-mode latency is par munhasir nahi (research: ye STA mechanism hai).

## Sirf board par verify hoga (abhi दावा nahi)
1. **Cycle timing:** ek analogRead ka asal waqt C3/ESP32 par koi published bharosemand number nahi mila. Firmware debug mode (`RSH_DEBUG 1`) har second `sent/s`, `overrun`, `maxCycleUs`, steering stddev print karta hai — board par pehla test yehi hai. Hadaf: sent/s ≈ 200, overrun 0.
2. **ADC noise WiFi on** par (docs threshold ±6 counts raw mode mein; mV mode mein muqabla board par).
3. **LED polarity:** DevKit V1 ki onboard LED aam tor par GPIO2 active-HIGH hoti hai; clone boards mein farq ho sakta hai — ulti chale to `config.h` mein `LED_ACTIVE_LOW 1` kar do.
4. **AP connect/range** full power par tumhare mahol mein.
5. **Hall sensors (49E):** datasheet abhi tak kisi ne verify nahi kiya (docs 01 item 11 open hai) — lagane se pehle output range 3.3 V supply par check karna.
