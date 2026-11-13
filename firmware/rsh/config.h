#pragma once
// RSH firmware — tamam settings ek jagah
// Board: ESP32 DevKit V1 (asal ESP32). C3 Super Mini wala version
// ab is package mein shamil nahi (original handover mein tha).

// ---------- Pins (ESP32 DevKit V1) ----------
// Tamam ANALOG sirf ADC1 par (GPIO32-39) — ADC2 WiFi ke saath band ho jata hai.
// GPIO34/35/36 sirf input hain (analog ke liye theek), un par internal
// pullup NAHI hota — is liye buttons pullup wali aam pins par hain.
// Strapping pins (0, 2, 5, 12, 15) INPUT ke liye istemal nahi kiye.
// Ek istisna: GPIO2 sirf board ki apni onboard LED ke liye hai (output,
// boot ke BAAD drive hota hai — mehfooz). GPIO2 par bahar ka koi load
// mat lagana, warna reset ke waqt boot strapping bigar sakti hai.
#define PIN_STEER      34   // ADC1 — steering pot wiper
#define PIN_ACCEL_ADC  35   // ADC1 — accelerator hall (baad mein)
#define PIN_BRAKE_ADC  36   // ADC1 — brake hall (baad mein)
#define PIN_ACCEL_BTN  25   // digital button (INPUT_PULLUP)
#define PIN_BRAKE_BTN  26   // digital button (INPUT_PULLUP)
#define PIN_GEAR_UP    27   // digital button (INPUT_PULLUP)
#define PIN_GEAR_DOWN  33   // digital button (INPUT_PULLUP)
#define PIN_LED        2    // DevKit V1 ki onboard LED (aam tor par GPIO2)

// ---------- FW-11 naye modules (Phase D) ----------
// Handbrake analog: GPIO32 (ADC1_CH4, free — v3.3 map mein istemal nahi).
// Horn button: GPIO4 (digital pull-up, free). Shifter sequential =
// PIN_GEAR_UP/DOWN (upar); H-pattern alternate mode ke pins neeche.
// Encoders: ENC1 (23, 19), ENC2 (16, 17) — sab free digital pull-up pins.
// LED strip: GPIO21 digital (R-L9 — direct full rig mein LED sirf yahin
// fit hoti hai; expanders hon to strip freed pin par jati hai).
// Battery sense: GPIO39 (ADC1_CH3, input-only) — preset mein hai; read
// FW-4/config ke baad aayega (abhi sirf GET_CAPS row).
// Clutch analog (Phase F WS5): GPIO33 (ADC1_CH5). Ye compiled default
// map mein aakhri free ADC1 pin hai — baaqi 5 ADC1 pins (32, 34, 35, 36,
// 39) steering/accel/brake/handbrake/battery le chuke hain. GPIO33
// default map mein PIN_GEAR_DOWN (digital) bhi hai, is liye clutch
// AUR sequential gear-down ek saath NAHI chal sakte: config blob mein
// clutch record (AXIS_ADC slot 3, pin 33) ho to gear-down ka BUTTON
// record usi pin par duplicate-pin ki wajah se COMMIT par PIN_CONFLICT
// se reject hota hai (config_transaction.h validation). Clutch default
// OFF hai (factory blob mein clutch record nahi) — sirf config se on
// hota hai (modules.h rsh_modules_apply_config_blob). Phase G (C3):
// clutch pin blob se aata hai — kisi bhi ADC1 pin par ho sakta hai
// (PIN-RULES preset full_rig_direct mein clutch GPIO35 par hai, wo bhi
// ab kaam karta hai); PIN_CLUTCH_ADC sirf default map ka pin hai,
// pabandi nahi. PIN-RULES preset (full_rig_direct) mein clutch GPIO35
// par hai — wo preset poora map badalta hai (steering 32, throttle 33,
// brake 34, clutch 35, handbrake 36); preset map compiled default map
// se mukhtalif hai, is liye preset ke pins sirf preset build/config
// ke saath istemal hon.
#define PIN_CLUTCH_ADC    33  // ADC1 — clutch pot/hall (Phase F WS5)
#define PIN_HANDBRAKE_ADC 32  // ADC1 — handbrake pot/hall
#define PIN_HORN_BTN      4   // digital button (INPUT_PULLUP)
#define PIN_BATTERY_ADC   39  // ADC1 — battery sense (input-only)
#define PIN_ENC1_A        23  // encoder 1, channel A (INPUT_PULLUP)
#define PIN_ENC1_B        19  // encoder 1, channel B (INPUT_PULLUP)
#define PIN_ENC2_A        16  // encoder 2, channel A (INPUT_PULLUP)
#define PIN_ENC2_B        17  // encoder 2, channel B (INPUT_PULLUP)
#define PIN_LED_STRIP     21  // WS2812 strip data (digital-out, R-L9)
// H-pattern shifter (alternate mode): gears 1..6. NOTE: 16/17/19 encoder
// pins se takrate hain — H-pattern + encoders ek saath nahi chal sakte
// (modules.h: rsh_modules_hpattern_encoder_conflict, FW-4 validation).
#define PIN_SHIFT_H1      13
#define PIN_SHIFT_H2      14
#define PIN_SHIFT_H3      16
#define PIN_SHIFT_H4      17
#define PIN_SHIFT_H5      18
#define PIN_SHIFT_H6      19
// Shifter mode: 0 = sequential (GEAR_UP/DOWN), 1 = H-pattern (upar wale pins).
#define RSH_SHIFTER_MODE  0
// LED strip mein LEDs ki tadaad (validator limit ≤ 60, PIN-RULES.md §4).
#define LED_STRIP_COUNT   16
#define LED_ACTIVE_LOW 0    // DevKit ki LED aam tor par active-HIGH hai;
                            // tumhare board par ulti chale to 1 kar do

// ---------- Sampling / bhejna ----------
#define SEND_PERIOD_US   5000   // 200 packets/sec (governor level 3 par 10000 — 100 pps)
#define STEER_SAMPLES    16     // buffer ka size (max) — asal tadaad adaptive
#define PEDAL_SAMPLES    8      // governor decide karta hai (16/12/8/4, 8/6/4/4)
#define DEBOUNCE_MS      10     // time-based debounce (F12): raw 10 ms stable = state change

// mV fallback (F13): raw mode mein girne ke baad har 30 s dobara check karo —
// calibration dobara theek ho to mV mode wapas aaye (sirf boot par nahi).
#define RSH_MV_RECHECK_MS 30000
// Floating axis (F2/D12): rail par stuck rehne ki muddat + jitter threshold.
#define RSH_AXIS_RAIL_MS           10000
#define RSH_AXIS_JITTER_THRESHOLD  150    // |diff| EMA (counts/sample) is se zyada = floating

// Steering ka read tareeqa: 1 = analogReadMilliVolts (eFuse factory
// calibration use karta hai; packet flags bit0 set hota hai taake PC ko
// pata ho) — 0 = raw analogRead counts.
#define STEER_USE_MILLIVOLTS 1

// HARDWARE NOTE: DevKit V1 ke ADC ki range 11 dB par 3.3 V tak hai
// (Espressif doc: full-scale 3.9 V, had supply voltage) — is board par
// series resistor ki ZAROORAT NAHI, pot seedha 3V3 par chalega.
// Wiper par 0.1 uF capacitor ESP pin ke paas phir bhi lagao (noise ke liye).
// Sab se accurate hissa 150-2450 mV hai; upar wale hisay ka halki si
// non-linearity PC ki calibration sambhal leti hai (endpoints map hote hain).

// ---------- Wired USB-serial transport (Phase G, D26 / OPEN-20) ----------
// ESP32 DevKit V1 par native USB NAHI — USB-serial = CP2102 USB-UART
// bridge (Serial). Baud 921600 (serial_transport.h: 115200 par 200 Hz
// stream + overhead ~78% utilization — headroom nahi; 921600 par ~10%).
// RSH_WIRED_SERIAL 1 ho to Serial transport hai (debug prints band —
// Serial par sirf "RSHS" frames jate hain); 0 ho to purana behaviour
// (Serial sirf RSH_DEBUG 1 par 115200 debug log).
#define RSH_WIRED_SERIAL 1
#define RSH_SERIAL_BAUD 921600

// ---------- WiFi / Network ----------
#define WIFI_SSID    "RSH"
// mDNS hostname (Phase G, C2/D33): board `rsh.local` par announce hota
// hai (net.cpp MDNS.begin). PC EspAddress.MdnsHostname ("rsh.local")
// isi se banta hai — hostname badlo to dono jagah badlo (aur OTA_HOSTNAME
// bhi yahi hai — teeno ek naam hon).
#define RSH_MDNS_HOSTNAME "rsh"
// WIFI_PASS: KOI DEFAULT NAHI (SEC-C-04). Har board pehle boot par apna
// 24-char random password banata hai (keystore.h/.cpp, NVS mein) —
// Wi-Fi join aur WebOTA login dono ke liye yahi ek password hai
// (PROTOCOL-V2.1 §5.1). Compile-time password kabhi nahi hota.
#define WIFI_CHANNEL 0                 // 0 = auto: boot par 1/6/11 scan karke
                                       // sab se khaali channel khud chunta hai.
                                       // 1-13 rakho to wahi channel fix rahega.
// TX power default full rehti hai (kuch set nahi hota). Zaroorat pare to
// neeche wala 1 karke value di ja sakti hai (softAP ke BAAD apply hoti hai).
#define RSH_SET_TX_POWER 0
#define TX_POWER          WIFI_POWER_19_5dBm
#define PORT_HELLO       4210   // PC -> ESP "HELO"
#define PORT_DATA        4211   // ESP -> PC data
#define PORT_CTRL        4212   // PC <-> ESP control (HMAC, §3) + ping (§9) — demux magic se
#define HELLO_TIMEOUT_MS 2000   // itni der HELO na aye to bhejna band

// ---------- Advanced features ----------
#define FIRMWARE_VERSION "4.0.0"

// OTA: WiFi par firmware update (USB cable ke baghair). Arduino IDE mein
// network port "rsh" nazar ayega. Update ke doran sampling ruk jati
// hai aur LED 5 Hz blink karti hai.
// RSH_OTA 0 = ArduinoOTA BAND (SEC-D-02): ye doosra OTA rasta tha, us ka
// apna auth tha aur koi rate limit nahi — WebOTA hi waahid OTA rasta hai.
// Dobara kholo to rsh.ino ota_begin() runtime per-device password istemal
// karta hai (koi default nahi); baghair lockout ke kabhi enable mat karo.
#define RSH_OTA 0
#define OTA_HOSTNAME "rsh"
// OTA_PASSWORD: KOI DEFAULT NAHI (SEC-C-04) — dekho WIFI_PASS upar;
// ArduinoOTA/WebOTA dono runtime password istemal karte hain.

// Web OTA: browser se update — RSH WiFi se jur kar
// http://192.168.4.1 kholo, login (user neeche, password = per-device
// password) karke signed .bin upload karo. Dual partition ki wajah se
// failed upload par purana firmware mehfooz rehta hai.
#define RSH_WEB_OTA 1
#define WEB_OTA_USER "admin"

// SEC-D-01: unauthenticated remote restart ("RSTX") KHATAM — RSH_ALLOW_
// REMOTE_RESTART hata diya gaya. Restart sirf HMAC-signed REBOOT (0x09,
// PROTOCOL §3.3) se hota hai (net_poll_ctrl → dispatcher). PC side:
// HelloClient.SendRestartAsync (RSTX) ko ControlClient REBOOT par
// migrate karna hai — PC-side team ko bata diya gaya hai.

// Low-memory safety (FW-10, Plan v2.3 D17): restart jab largest free
// block < 8 KB lagatar 10 heap-check cycles tak ho, ya free heap 5 min
// mein 20% se zyada gire. Purana RSH_MIN_FREE_HEAP 15000 magic KHATAM —
// wo free heap par tha, largest block par nahi (fragmented heap mein
// free heap kaafi dikh sakta hai jab ke 8 KB block allocate na ho).
// Thresholds heap_policy.h mein istemal hote hain (config = yahi file).
#define RSH_HEAP_MIN_LARGEST_BLOCK 8192
#define RSH_HEAP_LOW_CYCLES        10
#define RSH_HEAP_DROP_PCT          20
#define RSH_HEAP_DROP_WINDOW_MS    300000u

// ---------- Debug ----------
// 1 = 1 Hz Serial stats (seq, send rate, overrun, max cycle time,
// steering min/max/stddev). Release build mein 0 rakho.
#define RSH_DEBUG 0
