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
#define LED_ACTIVE_LOW 0    // DevKit ki LED aam tor par active-HIGH hai;
                            // tumhare board par ulti chale to 1 kar do

// ---------- Sampling / bhejna ----------
#define SEND_PERIOD_US   5000   // 200 packets/sec
#define STEER_SAMPLES    16     // buffer ka size (max) — asal tadaad adaptive
#define PEDAL_SAMPLES    8      // governor decide karta hai (16/12/8, 8/6/4)
#define DEBOUNCE_SAMPLES 2      // 2 x 5 ms = 10 ms stable = state change

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

// ---------- WiFi / Network ----------
#define WIFI_SSID    "RSH"
#define WIFI_PASS    "change-me-123"   // <-- ZAROOR apna password rakho;
                                       // default rehne doge to AP har kisi
                                       // ke liye khula jesa hai jis ko ye
                                       // file mili ho
#define WIFI_CHANNEL 0                 // 0 = auto: boot par 1/6/11 scan karke
                                       // sab se khaali channel khud chunta hai.
                                       // 1-13 rakho to wahi channel fix rahega.
// WPA2 password 8-63 characters ka hona chahiye — ghalat length par
// compile yahin ruk jayega, board par khamosh fail nahi hoga.
static_assert(sizeof(WIFI_PASS) - 1 >= 8 && sizeof(WIFI_PASS) - 1 <= 63,
              "WIFI_PASS 8-63 characters ka rakho");
// TX power default full rehti hai (kuch set nahi hota). Zaroorat pare to
// neeche wala 1 karke value di ja sakti hai (softAP ke BAAD apply hoti hai).
#define RSH_SET_TX_POWER 0
#define TX_POWER          WIFI_POWER_19_5dBm
#define PORT_HELLO       4210   // PC -> ESP "HELO"
#define PORT_DATA        4211   // ESP -> PC data
#define HELLO_TIMEOUT_MS 2000   // itni der HELO na aye to bhejna band

// ---------- Advanced features ----------
#define FIRMWARE_VERSION "4.0.0"

// OTA: WiFi par firmware update (USB cable ke baghair). Arduino IDE mein
// network port "rsh" nazar ayega. Update ke doran sampling ruk jati
// hai aur LED 5 Hz blink karti hai.
#define RSH_OTA 1
#define OTA_HOSTNAME "rsh"
#define OTA_PASSWORD "rsh-ota"    // <-- ye bhi badal lo

// Web OTA: browser se update — RSH WiFi se jur kar
// http://192.168.4.1 kholo, login (user neeche, password = OTA_PASSWORD)
// karke .bin upload karo. Dual partition ki wajah se failed upload par
// purana firmware mehfooz rehta hai.
#define RSH_WEB_OTA 1
#define WEB_OTA_USER "admin"

// Remote restart: PC port 4210 par "RSTX" bheje to board restart ho jata
// hai (wheel kholay baghair). Sirf AP se jura hua device bhej sakta hai.
#define RSH_ALLOW_REMOTE_RESTART 1

// Low-memory safety: free heap is se neeche jaye to atakne ki bajaye
// saaf restart (24x7 chalne ke liye insurance).
#define RSH_MIN_FREE_HEAP 15000

// ---------- Debug ----------
// 1 = 1 Hz Serial stats (seq, send rate, overrun, max cycle time,
// steering min/max/stddev). Release build mein 0 rakho.
#define RSH_DEBUG 0
