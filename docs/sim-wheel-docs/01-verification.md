# 01 - Plan Verification (research ke baad)

Verdict: ✔ = confirm, ⚠ = correction zaroori, ➕ = addition

| # | Plan ka claim | Verdict | Kya mila |
|---|---|---|---|
| 1 | `WiFi.setTxPower(WIFI_POWER_8_5dBm)` Super Mini ka fix hai | ✔ ⚠ | Kai Arduino forum / Hackaday / Home Assistant posts mein yehi workaround hai. Lekin ye **workaround** hai, har board ko zaroorat nahi. Call **WiFi start hone ke baad** karo (`softAP()` ke baad). Ek user ka masla header ke **pin 21** ki wajah se tha (pin hatane par bina power kam kiye chala). Pin 20/21 par kuch mat lagao. |
| 2 | Pehle revisions mein antenna design kharab | ✔ | Arduino forum aur mikrocontroller.net: alag revisions hain, ek kharab hai. Seller ke hisaab se farq. Antenna wala hissa metal/wires/haath se door rakho. |
| 3 | Analog sirf GPIO0-4 (ADC1) | ✔ | GPIO5 = ADC2 (WiFi ke saath masla). GPIO5/6 ko **digital** buttons ke liye use karna theek hai. |
| 4 | GPIO2, 8, 9 aur 18, 19 se bacho | ✔ | 2/8/9 strapping pins. GPIO8 aksar onboard LED, GPIO9 BOOT button. 18/19 USB D-/D+. |
| 5 | 225° pot travel ADC ke kinaron se door rehta hai | ⚠ **bara masla** | Pot 3.3V par ho to 225°/270° ≈ 8%-92% = ~0.27V-3.03V. Lekin C3 ADC 11dB par ~0.15-2.45V tak linear hai aur ~2.5-2.8V par 4095 par saturate ho jata hai. Yaani ghoomne ka upar wala hissa **flat** ho jayega. |
| 6 | Steering reading hilna ±3 counts | ⚠ | ESP32 ADC WiFi chalte mein noisy hota hai. 16-avg ke baad ±3 ideal hai, lekin test mein **WiFi on** rakh kar ±6 tak pass maano. |
| 7 | UDP **broadcast** bhejo | ⚠ | Broadcast mein ACK/retry nahi aur power-save wale clients ke liye buffer ho kar der se mil sakta hai. Forums mein 10 ms se 300 ms tak jitter ki shikayat (modem sleep, listen interval 3 = ~300 ms). **Unicast + handshake** behtar (neeche dekho). Ye 802.11 ka aam behavior hai, kisi ek source ka direct claim nahi. |
| 8 | WiFi sleep band rakho | ✔ | `WiFi.setSleep(false)` latency ke liye recommended. SoftAP mein **PC ke WiFi adapter ka power saving** bhi band karo. |
| 9 | `vgamepad` + ViGEmBus (v2 mein primary nahi, vJoy primary) | ✔ ⚠ | `vgamepad` API confirm (`left_joystick_float`, `right_trigger_float`, shoulder buttons) aur ViGEmBus bundled hai. **Lekin ViGEmBus retired/archived (2 Nov 2023)**: chalta hai, par updates nahi. Isliye v2 mein vJoy primary, ViGEm optional future backend. |
| 10 | vJoy baad mein | ✔ | `pyvjoystick` package vJoy aur ViGEm dono ke bindings deta hai. |
| 11 | Pedals ke liye 49E hall sensors | ❔ research nahi hua | Datasheet khud check karo. Sensor ko **3.3V** se power karo (5V se output ADC pin ke liye zyada ho sakta hai). Output swing chhota hota hai, calibration zaroori. |
| 12 | Pot 4:1 gear = 225° | ✔ math / ❔ | 900/4 = 225 sahi. Aapke pot ka asli electrical travel (aam taur par ~270-300°) datasheet se confirm karo. |
| 13 | 5 ms par 16 readings | ❔ | `analogRead` ka asli time measure karo (step 1 mein `micros()` se). |

## Plan mein changes (summary)

1. **Pot ke upar series resistor (~15 kΩ)** ya `analogReadMilliVolts()`; ADC saturation se bachne ke liye. Detail: `03-firmware-plan.md`.
2. **Unicast + handshake** (PC "HELO" bhejta hai, ESP us IP par data bhejta hai), broadcast nahi.
3. **Firmware dono pedal types bhejta hai** (button bits aur analog raw). PC ka JSON config chunta hai. Buttons se hall par jaate waqt reflash nahi.
4. **Step 0 add**: pehle sirf WiFi SoftAP + ping/UDP test, taake kharab board revision shuru mein hi pata chal jaye.
5. **ViGEmBus retired** note + vJoy fallback pehle se design mein.
6. Test thresholds realistic: noise ±6 (WiFi on), p99 packet gap < 15 ms.

## Sources (jo research mein dekhe)

- Arduino forum: "No Wifi connect with ESP32-C3 super mini" (antenna revision, tx power, pin 21)
- mikrocontroller.net topic 582687 (Super Mini revisions)
- Hackaday.io: "ESP32 C3 Super Mini WiFi Fix - 3 Methods That Work"
- esp32.co.uk: ESP32-C3 SuperMini pinout (ADC1/ADC2, strapping, USB)
- Arduino forum: "ESP32-C3 ADC issue - reading 4095 at 2.8V" (saturation, `analogReadMilliVolts`)
- esp32.com: UDP delay thread (modem sleep, listen interval)
- PyPI `vgamepad`; GitHub `nefarius/ViGEmBus` (retired); PyPI `pyvjoystick`


## Update v2: stack decisions (research)

| Topic | Finding | Decision |
|---|---|---|
| .NET version | .NET 8 aur 9 ka end of support **10 Nov 2026**; .NET 10 LTS Nov 2028 tak | **.NET 10 LTS** |
| WPF UI library | WPF-UI 4.1+ ne .NET 10 support add kiya (4.3.0 tak releases) | WPF + **WPF-UI 4.x** |
| WPF future | WPF maintenance mode mein lekin .NET 10 par Fluent style coverage barha; stable | WPF theek, kyun ke stable aur mature |
| vJoy maintenance | Original author maintain nahi karta; forks: jshafer817 (2.1.9.1), njz3 (2.2.x), Brunner (signed 2.2.2.0 ka zikr) | Baseline **2.1.9.1** (Win11 par kaam karta report hua), 2.2.x sirf zaroorat par |
| vJoy install issues | Win11 par 2.2.1.x installs mein masle aur test-signing ki zaroorat (2022 reports); test-signing se kuch anti-cheat games nahi chalte | Signed build use karo; app driver version check kare |
| Driver/DLL mismatch | "Driver version does NOT match DLL version" reports (e.g. SimHub/FreePIE) | DLL install folder se load, bundle nahi; apni thin P/Invoke |
| ViGEmBus | Retired (Nov 2023) | Primary nahi; optional future backend |

Sources (v2): Microsoft .NET Blog (.NET 8/9 EOS), lepoco/wpfui releases, GitHub issues shauleiz/vJoy #57, njz3/vJoy #11, FreePIE #208, blekenbleu vJoy notes, BrunnerInnovation/vJoy README.

Note: vJoy ke exact release builds aur unki Windows 11 compatibility aapke PC par verify karni hogi (setup step 1-2). Ye plan mein "install test" ke taur par hai.
