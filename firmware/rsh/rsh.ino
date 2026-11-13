// RSH firmware — ESP32 DevKit V1 (Arduino)
// Kaam: raw inputs parho, 18-byte packet banao, WiFi SoftAP par PC ko unicast
// bhejo @ 200/sec. Calibration/processing sab PC par hai (firmware dumb, PC smart).
//
// Board: ESP32 Dev Module (Arduino IDE mein "ESP32 Dev Module" select karo)

#include <Arduino.h>
#include "config.h"          // pehle config — RSH_OTA wagaira yahin se ate hain
#if RSH_OTA
#include <ArduinoOTA.h>
#endif
#include "protocol.h"
#include "inputs.h"
#include "net.h"
#include "webota.h"
#include "status_led.h"

static uint16_t seq = 0;
static uint32_t nextTickUs = 0;
static uint32_t overrunCount = 0;     // cycles jin ka kaam period se zyada lamba hua
static uint32_t maxCycleUs = 0;       // sab se lamba cycle kaam (board par timing test ke liye)
static bool netOk = false;            // latched: fail ho to loop sirf retry karta hai
static uint32_t lastRetryMs = 0;
static bool prevCycleOverran = false;
static bool otaActive = false;
static uint32_t govWindowStartMs = 0; // adaptive sampling governor ki window
static uint32_t govOverrunAtStart = 0;
static uint32_t heapCheckMs = 0;

#if RSH_OTA
static void ota_begin() {
  ArduinoOTA.setHostname(OTA_HOSTNAME);
  ArduinoOTA.setPassword(OTA_PASSWORD);
  ArduinoOTA.onStart([]() {
    otaActive = true;
    led_set(LedState::Updating);
  });
  ArduinoOTA.onEnd([]() { otaActive = false; });
  ArduinoOTA.onError([](ota_error_t) { otaActive = false; });
  ArduinoOTA.begin();
}
#endif

#if RSH_DEBUG
static uint32_t dbgWindowStartMs = 0;
static uint32_t dbgSentInWindow = 0;
static uint32_t dbgOverrunAtWindowStart = 0;
static uint16_t dbgSteerMin = 0xFFFF, dbgSteerMax = 0;
static uint64_t dbgSteerSum = 0, dbgSteerSqSum = 0;
static uint32_t dbgSteerN = 0;
#endif

void setup() {
#if RSH_DEBUG
  Serial.begin(115200);
#endif
  led_begin();
  inputs_begin();
  netOk = net_begin();
  lastRetryMs = millis();
  govWindowStartMs = lastRetryMs;
  heapCheckMs = lastRetryMs;
  if (netOk) {
#if RSH_OTA
    ota_begin();
#endif
#if RSH_WEB_OTA
    webota_begin();
#endif
    led_set(LedState::ApUpWaitingPc);
    nextTickUs = micros() + SEND_PERIOD_US;
  } else {
    led_set(LedState::Error);   // loop() is state ko overwrite nahi karega
  }
#if RSH_DEBUG
  dbgWindowStartMs = millis();
  Serial.println(netOk ? "RSH up — HELO ka intezar" : "RSH: WiFi start FAIL — retry hota rahega");
#endif
}

void loop() {
  uint32_t nowMs = millis();

  // Fail-safe (audit fix): WiFi start fail ho to tick processing bilkul band —
  // sirf error LED aur har 5 s mein dobara koshish. Pehle error state foran
  // aam blink se overwrite ho jata tha aur retry kabhi nahi hota tha.
  if (!netOk) {
    led_update(nowMs);
    if (nowMs - lastRetryMs >= 5000) {
      lastRetryMs = nowMs;
      netOk = net_begin();
      if (netOk) {
#if RSH_OTA
        ota_begin();
#endif
#if RSH_WEB_OTA
        webota_begin();
#endif
        led_set(LedState::ApUpWaitingPc);
        nextTickUs = micros() + SEND_PERIOD_US;
      }
    }
    return;
  }

#if RSH_OTA
  ArduinoOTA.handle();
  if (otaActive) {          // update ke doran sampling band — sirf LED
    led_update(nowMs);
    return;
  }
#endif

#if RSH_WEB_OTA
  webota_handle();
  if (webota_active()) {    // web upload ke doran bhi sampling band
    led_update(nowMs);
    return;
  }
#endif

  net_poll();
  led_update(nowMs);

  // Remote restart: PC ne kaha to saaf restart (wheel kholna na pare)
  if (net_restart_requested()) {
    delay(50);
    ESP.restart();
  }

  // Adaptive sampling: har 2 s window ke overruns governor ko do
  if (nowMs - govWindowStartMs >= 2000) {
    inputs_governor().update(overrunCount - govOverrunAtStart);
    govOverrunAtStart = overrunCount;
    govWindowStartMs = nowMs;
  }

  // Low-memory safety: heap khatre mein ho to atakne se pehle restart
  if (nowMs - heapCheckMs >= 10000) {
    heapCheckMs = nowMs;
    if (ESP.getFreeHeap() < RSH_MIN_FREE_HEAP) ESP.restart();
  }

  // Wrap-safe scheduler: micros() ~71.6 min mein wrap hota hai,
  // signed difference wala tareeqa hi sahi hai (research-verified).
  uint32_t nowUs = micros();
  int32_t untilTick = (int32_t)(nextTickUs - nowUs);
  if (untilTick > 0) {
    // Deadline door ho to core ko spin mein garam mat karo (audit fix)
    if (untilTick > 1500) delay(1);
    return;
  }
  nextTickUs += SEND_PERIOD_US;
  // Agar bohat peeche reh gaye (lamba block), to burst karke catch-up mat karo —
  // agla tick ab se gino, purani readings ka koi faida nahi. Overrun yahan
  // sirf tab gino jab pichle cycle ka kaam khud lamba na raha ho
  // (warna ek hi stall do dafa gin liya jata tha — audit fix).
  if ((int32_t)(micros() - nextTickUs) >= (int32_t)SEND_PERIOD_US) {
    nextTickUs = micros() + SEND_PERIOD_US;
    if (!prevCycleOverran) overrunCount++;
  }

  uint32_t cycleStartUs = micros();

  InputSample s = inputs_read();

  bool active = net_pc_active(nowMs);
  led_set(active ? LedState::PcConnected : LedState::ApUpWaitingPc);
  if (active) {
    uint8_t pkt[RSH_PACKET_SIZE];
    uint8_t flags = inputs_steering_is_mv() ? RSH_FLAG_STEER_MV : 0;
    rsh_build_packet(pkt, seq, cycleStartUs, s.steering, s.accelAnalog,
                      s.brakeAnalog, s.buttons, flags);
    if (net_send(pkt, sizeof(pkt))) {
#if RSH_DEBUG
      dbgSentInWindow++;
#endif
    }
    seq++;   // uint16 khud wrap hota hai — PC wrap-safe tareeqe se parhta hai
  }

  uint32_t cycleUs = micros() - cycleStartUs;
  if (cycleUs > maxCycleUs) maxCycleUs = cycleUs;
  prevCycleOverran = cycleUs > SEND_PERIOD_US;
  if (prevCycleOverran) overrunCount++;

#if RSH_DEBUG
  if (s.steering < dbgSteerMin) dbgSteerMin = s.steering;
  if (s.steering > dbgSteerMax) dbgSteerMax = s.steering;
  dbgSteerSum += s.steering;
  dbgSteerSqSum += (uint64_t)s.steering * s.steering;
  dbgSteerN++;

  if (nowMs - dbgWindowStartMs >= 1000) {
    double mean = dbgSteerN ? (double)dbgSteerSum / dbgSteerN : 0;
    double var = dbgSteerN ? (double)dbgSteerSqSum / dbgSteerN - mean * mean : 0;
    if (var < 0) var = 0;
    Serial.printf("sent/s=%lu seq=%u overrun=%lu maxCycleUs=%lu steer min=%u max=%u mean=%.1f std=%.2f mv=%d\n",
                  (unsigned long)dbgSentInWindow, seq,
                  (unsigned long)(overrunCount - dbgOverrunAtWindowStart),
                  (unsigned long)maxCycleUs, dbgSteerMin, dbgSteerMax, mean, sqrt(var),
                  inputs_steering_is_mv() ? 1 : 0);
    dbgWindowStartMs = nowMs;
    dbgSentInWindow = 0;
    dbgOverrunAtWindowStart = overrunCount;
    dbgSteerMin = 0xFFFF; dbgSteerMax = 0;
    dbgSteerSum = 0; dbgSteerSqSum = 0; dbgSteerN = 0;
    maxCycleUs = 0;
  }
#endif
}
