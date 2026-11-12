#include "net.h"
#include "config.h"
#include <Arduino.h>
#include <WiFi.h>
#include <WiFiUdp.h>

static WiFiUDP udpHello;   // 4210 par HELO sunta hai
static WiFiUDP udpData;    // 4211 par data bhejta hai
static IPAddress pcIp;
static bool havePc = false;
static uint32_t lastHelloMs = 0;
static bool restartRequested = false;

bool net_restart_requested() { return restartRequested; }

// Ek channel ka "bheer score": us channel ke networks sab se zyada
// wazan ke sath, padosi channels (overlap) kam wazan ke sath.
static int channel_score(const int *load, int c) {
  int score = 0;
  for (int ch = 1; ch <= 13; ch++) {
    int d = ch - c;
    if (d < 0) d = -d;
    if (d == 0) score += load[ch] * 4;
    else if (d == 1) score += load[ch] * 2;
    else if (d == 2) score += load[ch];
  }
  return score;
}

// WIFI_CHANNEL 0 ho to 1/6/11 mein se sab se khaali channel chuno:
// boot par ek dafa scan (STA mode mein), phir AP usi channel par.
static uint8_t pick_channel() {
#if WIFI_CHANNEL
  return WIFI_CHANNEL;
#else
  WiFi.mode(WIFI_STA);
  WiFi.disconnect();
  int n = WiFi.scanNetworks(false, true);
  int load[14] = {0};
  for (int i = 0; i < n; i++) {
    int ch = WiFi.channel(i);
    if (ch >= 1 && ch <= 13) load[ch]++;
  }
  WiFi.scanDelete();
  const uint8_t cand[3] = {1, 6, 11};
  uint8_t best = cand[0];
  for (int i = 1; i < 3; i++) {
    if (channel_score(load, cand[i]) < channel_score(load, best)) best = cand[i];
  }
  return best;
#endif
}

bool net_begin() {
  uint8_t ch = pick_channel();
  WiFi.mode(WIFI_AP);
  // max 4 stations: PC (data) + phone/PC browser (Web OTA) ek saath jur saken
  if (!WiFi.softAP(WIFI_SSID, WIFI_PASS, ch, 0, 4)) return false;
#if APEX_SET_TX_POWER
  WiFi.setTxPower(TX_POWER);   // softAP ke BAAD (vendor doc ke mutabiq)
#endif
  WiFi.setSleep(false);        // nuqsan nahi; AP-mode latency is par munhasir nahi
  // Receive socket fail ho to bhi upar "sab theek" na kaho (audit fix)
  if (!udpHello.begin(PORT_HELLO)) return false;
  return true;
}

void net_poll() {
  // Per call cap: datagram flood sampler ko starve na kare (audit fix).
  // Har datagram ke baad flush() lazmi hai — core mein jab tak pichla
  // rx_buffer mojood ho, parsePacket() hamesha 0 deta hai; sirf 128 byte
  // parh kar chor dene se lamba datagram link ko hamesha ke liye band
  // kar deta tha (audit BUG #1). flush() baqi hissa discard kar deta hai.
  for (int i = 0; i < 8; i++) {
    int n = udpHello.parsePacket();
    if (n <= 0) break;
    char buf[128];
    int len = udpHello.read(buf, sizeof(buf));
    if (len >= 4 && memcmp(buf, "HELO", 4) == 0) {
      pcIp = udpHello.remoteIP();
      havePc = true;
      lastHelloMs = millis();
    }
#if APEXLINK_ALLOW_REMOTE_RESTART
    if (len >= 4 && memcmp(buf, "RSTX", 4) == 0) restartRequested = true;
#endif
    udpHello.flush();   // remainder discard — rx_buffer hamesha clear
  }
}

bool net_pc_active(uint32_t nowMs) {
  return havePc && (uint32_t)(nowMs - lastHelloMs) < HELLO_TIMEOUT_MS;
}

bool net_send(const uint8_t *buf, size_t len) {
  if (!havePc) return false;
  if (!udpData.beginPacket(pcIp, PORT_DATA)) return false;
  udpData.write(buf, len);
  return udpData.endPacket() == 1;
}
