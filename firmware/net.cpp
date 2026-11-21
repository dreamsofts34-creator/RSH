#include "net.h"
#include "net_logic.h"   // FW-7: channel score / pick (pure, host-testable)
#include "protocol.h"    // FW-8: v2.2 builders / parsers
#include "ctrl.h"        // FW-8: plain control dispatcher (v2.2, no HMAC)
#include "ctrl_handlers.h"  // FW-3/FW-4: SET_CFG/COMMIT/.../SET_PASSWORD handler (pure)
#include "diag_log.h"    // F-10: diagnostics ring log + GET_DIAG body
#include "horn_test.h"   // D-5: HORN_TEST timer state (pure)
#include "led_driver.h"    // Phase G WS5: LED driver abstraction.
                           // Phase H WS2: net.cpp mein wired hai —
                           // led_test_request adapter + net_led_tick().
#include "led_sink_rmt.h"  // Phase H WS2: RMT WS2812 sink (GPIO21)
#include "fw34_state.h"    // FW-3/FW-4: safe mode + config txn state (rsh.ino)
#include "keystore.h"    // v2.2: user-set Wi-Fi password store (no device key)
#include "modules.h"     // FW-11: GET_CAPS default pin table
#include "config.h"
#include <Arduino.h>
#include <WiFi.h>
#include <WiFiUdp.h>
#include <ESPmDNS.h>   // Phase G (C2/D33): mDNS responder — board `rsh.local`
#include <esp_wifi.h>    // per-station RSSI (SEC-D-08 / status packet)
#include <esp_task_wdt.h>  // R3-FW-08: blocking scan / feed ke liye

static WiFiUDP udpHello;   // 4210 par HELO sunta hai
static WiFiUDP udpData;    // 4211 par data bhejta hai (+ HELLO/control/ping replies)
static WiFiUDP udpCtrl;    // 4212 par control + ping sunta hai
static IPAddress pcIp;
static bool havePc = false;
static uint32_t lastHelloMs = 0;
static uint32_t lastPcChangeMs = 0;
static uint32_t lastHelloReplyMs = 0;
static uint8_t lastHelloProto = 0;   // aakhri HELO ka proto (0x01 legacy / 0x02 v2)
static uint8_t currentChannel = 0;
static uint8_t forcedChannel = 0;    // rescan ne chuna ho to (dobara scan nahi)
static uint32_t lastScanMs = 0;
static uint32_t lastSwitchMs = 0;
static uint16_t txFailAtLastScan = 0;
static bool streamBusy = false;      // ping drop-when-busy (§9.3)
static uint32_t pingDropped = 0;
static uint64_t usBase = 0;          // micros() u64 monotonic (ping t1/t2)
static uint32_t lastMicros = 0;
static TxStats txStats;    // F3: failed UDP transmissions (status packet)
static rsh_ctrl_session_t ctrlSession;  // v2.2: reqId-only dedup state
static rsh_ctrl_cache_t ctrlCache;      // v2.2: duplicate cached replies
static rsh_ping_limiter_t pingLimiter;  // §9.4: per-source token bucket
static bool ctrlInitDone = false;       // session sirf ek dafa init ho
// D3 (v2.2 stream gate): is session mein GET_CAPS OK hua? Stream sirf
// is ke baad khulta hai (plain GET_CAPS OK — HMAC khatam). Naya peer /
// naya session flag saaf karta hai. Wi-Fi aur serial ke alag flags hain.
static bool wifiCapsOk = false;
static bool serialCapsOk = false;
// Phase G (D26/OPEN-20): wired serial peer ka ALAG session — apna HELO
// timer, apna control session/cache, apna GET_CAPS-OK flag.
static rsh_ctrl_session_t serialCtrlSession;
static rsh_ctrl_cache_t serialCtrlCache;
static bool serialHavePc = false;
static uint32_t serialLastHelloMs = 0;
static uint32_t serialLastHelloReplyMs = 0;
static uint8_t serialLastHelloProto = 0;
static bool rebootPending = false;      // REBOOT (0x09) mila — reply ke baad restart
static bool passwordChangePending = false;  // SET_PASSWORD (0x0C) mila — reply ke baad NVS + AP restart
static RshHornTest hornTestState;       // D-5 (v4.0.2): HORN_TEST timer state
static uint8_t pendingPassword[RSH_PASSWORD_MAX_LEN];  // deferred SET_PASSWORD bytes (reply ke baad apply)
static uint8_t pendingPasswordLen = 0;

// Control handler ka context: aakhri command + status (tests ke liye).
typedef struct {
  uint8_t lastCmd;
  uint8_t lastStatus;
} net_ctrl_ctx_t;
static net_ctrl_ctx_t ctrlCtx = {0, 0};
static net_ctrl_ctx_t serialCtrlCtx = {0, 0};

// SSID "RSH-XXXX" (D4): softAP MAC ke aakhri 2 bytes, uppercase hex.
// Wahi MAC jo HELLO board_mac mein jata hai — SSID suffix aur HELLO MAC
// hamesha milte hain (dono isi call se aate hain).
static char ssidBuf[16];
static const char *net_ssid_() {
  uint8_t mac[6];
  WiFi.softAPmacAddress(mac);
  snprintf(ssidBuf, sizeof(ssidBuf), RSH_SSID_PREFIX "%02X%02X", mac[4], mac[5]);
  return ssidBuf;
}

// FW-3/FW-4 handler callbacks (ctrl_handlers.h): persist + txn rsh.ino se,
// clock millis(). SET_PASSWORD deferred: password yahan sirf RAM mein rakho
// (reply ke baad keystore_set_password + AP restart hota hai — neeche
// net_poll_ctrl). Handler ne length/charset pehle hi validate kar li hai (§5.2).
// D3 current-password check: koi password stored nahi ho to proceed (1);
// stored ho to sirf match par proceed (keystore_check_password, constant-time).
static uint8_t fw34_password_verify_cb(void *user, const uint8_t *pw, uint16_t len) {
  (void)user;
  if (!keystore_has_password()) return 1;  // pehli setup — current ignore
  return keystore_check_password((const char *)pw, (uint8_t)len) ? 1 : 0;
}
static void fw34_password_cb(void *user, const uint8_t *pw, uint16_t len) {
  (void)user;
  if (!pw || len < RSH_PASSWORD_MIN_LEN || len > RSH_PASSWORD_MAX_LEN) return;
  memcpy(pendingPassword, pw, len);
  pendingPasswordLen = (uint8_t)len;
  passwordChangePending = true;
}
static void fw34_persist_cb(void *user, uint8_t pending) {
  (void)user;
  rsh_fw34_cfg_persist(pending);
}
static uint32_t fw34_now_cb(void *user) {
  (void)user;
  return millis();
}
static rsh_fw34_handler_ctx_t fw34Ctx;  // txn pointer har delegate par fresh

// Phase H WS2: LED strip driver (RMT sink). ledDrvReady=0 = sink/driver
// nahi hai — led_test_request NULL rehta hai, handler fail-closed
// UNSUPPORTED deta hai (Phase G jaisa). Yahan hai (fw34Ctx ke baad)
// kyun ke adapter fw34Ctx.led_pin / led_has_expanders parhta hai.
static RshLedDriver ledDrv;
static uint8_t ledDrvReady = 0;

// LED_TEST adapter (fw34Ctx.led_test_request): handler se pattern lekar
// driver start karo. led_pin/expanders har delegate par current blob
// se fresh hote hain (net_ctrl_handler). Config ne strip pin badla ho
// (expanders -> freed pin, R-L9) to RMT sink us pin par dobara lagta
// hai; re-init fail ho to pattern start mat karo (reply pehle hi OK ja
// chuka ho sakta hai — A7 visual check is window ko pakarta hai;
// dobara LED_TEST bhejne par sink dobara try hota hai).
static void led_test_start_(void *u, const RshLedPattern *p) {
  (void)u;
  if (!p || !ledDrvReady) return;
  uint8_t pin = fw34Ctx.led_pin;
  if (pin != rsh_led_rmt_sink_pin()) {
    if (!rsh_led_rmt_sink_init(pin)) return;
  }
  (void)rsh_led_driver_start(&ledDrv, p, millis(), pin,
                             fw34Ctx.led_has_expanders, 1);
}

// LED driver tick (rsh.ino loop se — netOk / OTA branches samet):
// pattern frame badla ho to RMT sink likhta hai, warna kuch nahi.
void net_led_tick(uint32_t nowMs) {
  if (ledDrvReady) (void)rsh_led_driver_tick(&ledDrv, nowMs);
}

// Board monotonic µs, u64 (micros() 71 min mein wrap hota hai — ping mein
// wrap nahi chahiye, §9.1). Har loop mein call hota hai, is liye wrap
// kabhi miss nahi hota.
static uint64_t net_micros64() {
  uint32_t now = micros();
  if (now < lastMicros) usBase += 0x100000000ULL;
  lastMicros = now;
  return usBase + (uint64_t)now;
}

uint32_t net_tx_fail_count() { return txStats.tx_fail; }
uint32_t net_tx_sent_count() { return txStats.sent; }
uint8_t net_channel() { return currentChannel; }
void net_set_stream_busy(bool busy) { streamBusy = busy; }
uint32_t net_ping_dropped_count() { return pingDropped; }

bool net_pc_is_v2(uint32_t nowMs) {
  (void)nowMs;
  return havePc && lastHelloProto == 0x02;
}

// Per-station RSSI: SoftAP se jure stations mein sab se taqatwar link.
// Station ka IP yahan nahi milta (list mein sirf MAC hai), is liye max
// RSSI report hota hai — PC aam tor par qareeb tareen station hota hai.
// R3-FW-09: ye PC ka RSSI NAHI hai, strongest station ka hai (jab
// phone/browser WebOTA ke liye jura ho aur PC se zyada qareeb ho to
// status packet mein us ka RSSI jata hai); doc §1.4 mein aisa hi likha
// hai. Koi station nahi / API fail → -128 (unknown, RSH_RSSI_UNKNOWN).
int8_t net_rssi_dbm() {
  wifi_sta_list_t list;
  if (esp_wifi_ap_get_sta_list(&list) != ESP_OK || list.num == 0)
    return RSH_RSSI_UNKNOWN;
  int8_t best = -127;
  for (int i = 0; i < list.num; i++) {
    if (list.sta[i].rssi > best) best = list.sta[i].rssi;
  }
  return best;
}

// WIFI_CHANNEL 0 ho to 1/6/11 mein se sab se khaali channel chuno:
// boot par ek dafa scan (STA mode mein), phir AP usi channel par.
// F8: RSSI-weighted load (sirf ginna nahi), hidden networks EXCLUDE,
// scan error (n < 0) par purana channel rakho — kharab scan se andha
// channel mat chuno. Scan ke doran AP band hota hai (sync scan ~1-3 s).
static uint8_t pick_channel() {
#if WIFI_CHANNEL
  return WIFI_CHANNEL;
#else
  if (forcedChannel) return forcedChannel;
  WiFi.mode(WIFI_STA);
  WiFi.disconnect();
  // R3-FW-08: sync scan ~1-3 s (dense RF mein zyada) — loop task WDT
  // (5 s) mein subscribed hai (setup mein net_begin se pehle), is liye
  // scan se pehle aur baad mein feed karo; warna scan + baaki setup
  // 5 s ke qareeb ja kar WDT reset + boot-counter fail ka silsila
  // shuru kar sakta hai.
  esp_task_wdt_reset();
  int n = WiFi.scanNetworks(false, false);  // show_hidden=false: hidden exclude
  esp_task_wdt_reset();
  if (n < 0) {
    WiFi.scanDelete();
    return currentChannel ? currentChannel : 1;
  }
  rsh_scan_entry_t entries[32];
  size_t count = 0;
  for (int i = 0; i < n && count < 32; i++) {
    entries[count].channel = (uint8_t)WiFi.channel(i);
    entries[count].rssi_dbm = (int8_t)WiFi.RSSI(i);
    entries[count].hidden = 0;  // show_hidden=false → hidden aate hi nahi
    count++;
  }
  WiFi.scanDelete();
  int load[14];
  rsh_scan_load(entries, count, load);
  return rsh_pick_channel(load);   // net_logic.h: 1/6/11 mein se sab se khaali
#endif
}

uint8_t net_horn_test_active(uint32_t nowMs) {
  return rsh_horn_test_active(&hornTestState, nowMs);
}

// F-14: persistent LED config current blob se driver par lagao.
// Record na ho to persistent hat jata hai (strip off, LED_TEST ke ilawa).
// ledDrvReady=0 ho to kuch nahi (driver nahi hai — fail closed).
void net_led_apply_persistent(const uint8_t *blob, size_t len) {
  if (!ledDrvReady) return;
  RshLedConfig cfg;
  if (!rsh_cfg_led_config(blob, len, &cfg)) {
    rsh_led_driver_set_persistent(&ledDrv, NULL, 100, millis());
    return;
  }
  RshLedPattern p;
  p.patternId = cfg.pattern_id;
  p.r = cfg.r;
  p.g = cfg.g;
  p.b = cfg.b;
  p.periodMs = 1000;  // persistent pattern ka default cycle (record mein nahi)
  p.repeats = 0;
  p.reserved = 0;
  rsh_led_driver_set_persistent(&ledDrv, &p, cfg.brightness, millis());
}

// Forward declaration: defined below — the DS-11 retry block calls it
// first, and C++ requires the declaration before use.
static bool net_start_ap_and_sockets(uint8_t ch);

// DS-11: AP restart retry — net_start_ap_and_sockets() fail hone par
// (password restart ya rescan restart) AP band rehta hai; ye pending
// flag agle net_poll() calls par har 5 s mein dobara koshish karta hai.
// Pehli failure par diag event record hota hai; retry failures khamosh
// rehte hain (ring flood nahi), kamyabi par flag saaf ho jata hai.
static bool apRestartPending_ = false;
static uint32_t apRestartRetryAtMs_ = 0;
static uint8_t apRestartRetryCh_ = 0;

static void net_request_ap_restart_retry_(uint8_t ch) {
  apRestartPending_ = true;
  apRestartRetryCh_ = ch;
  apRestartRetryAtMs_ = millis() + 5000;
}

static void net_retry_ap_restart_() {
  if (!apRestartPending_) return;
  if ((int32_t)(millis() - apRestartRetryAtMs_) < 0) return;
  if (net_start_ap_and_sockets(apRestartRetryCh_)) {
    apRestartPending_ = false;
    return;
  }
  apRestartRetryAtMs_ = millis() + 5000;  // dobara 5 s baad, diag dobara nahi
}

// AP + sockets ek channel par shuru karo. AP KHULA hai jab koi password
// stored nahi (v2.2, D6 — pehli setup / password clear ke baad), warna
// WPA2-PSK (user-set password, keystore se).
// keystore_begin() setup mein pehle ho chuka hona chahiye.
static bool net_start_ap_and_sockets(uint8_t ch) {
  WiFi.mode(WIFI_AP);
  bool open = !keystore_has_password();
  bool apOk;
  if (open) apOk = WiFi.softAP(net_ssid_(), (const char *)NULL, ch, 0, 4);
  else apOk = WiFi.softAP(net_ssid_(), keystore_password(), ch, 0, 4);
  // max 4 stations: PC (data) + phone/PC browser (Web OTA) ek saath jur saken
  if (!apOk) return false;
#if RSH_SET_TX_POWER
  WiFi.setTxPower(TX_POWER);   // softAP ke BAAD (vendor doc ke mutabiq)
#endif
  WiFi.setSleep(false);        // nuqsan nahi; AP-mode latency is par munhasir nahi
  // Receive socket fail ho to bhi upar "sab theek" na kaho (audit fix)
  if (!udpHello.begin(PORT_HELLO)) return false;
  if (!udpCtrl.begin(PORT_CTRL)) return false;
  currentChannel = ch;
  // Phase G (C2/D33): mDNS responder — board `rsh.local` par milta hai;
  // PC EspAddress.ResolveAsync pehle rsh.local try karta hai, fallback
  // 192.168.4.1. Ek dafa start ho to dobara begin nahi (rescan par AP
  // restart hota hai — mDNS service netif se bandhi rehti hai, dobara
  // begin ki zaroorat nahi). Begin fail ho to agle net_start par dobara
  // koshish hoti hai (guard sirf kamyabi par set hota hai). Services:
  // `rsh` TCP 4212 (control) + `http` TCP 80 (WebOTA).
  static bool mdnsStarted = false;
  if (!mdnsStarted && MDNS.begin(RSH_MDNS_HOSTNAME)) {
    MDNS.addService("rsh", "tcp", PORT_CTRL);
    MDNS.addService("http", "tcp", 80);
    mdnsStarted = true;
  }
  // AP-up event sirf pehli dafa (rescan restart par dobara nahi).
  static bool apUpMarked = false;
  if (!apUpMarked) {
    apUpMarked = true;
    rsh_diag_record_global(RSH_DIAG_AP_UP, ch, millis());
  }
  return true;
}

// Naya session (HELO sessionFlag=1): SIRF us transport ka control
// session reset (reqId dedup cache saaf) aur us ka GET_CAPS-OK flag
// saaf — naye session mein stream ke liye dobara GET_CAPS lazmi hai
// (D3). R3-FW-11: pehle ye dono transports reset karta tha — serial ka
// naya session Wi-Fi ka cache bhi wipe kar deta tha (cross-transport).
// Ab per-transport hai: Wi-Fi flag-1 sirf Wi-Fi session, serial flag-1
// sirf serial session. Naya peer / timeout ke baad wapas aaya peer bhi
// Wi-Fi session reset karta hai (rsh_session_reset_needed, R3-FW-02).
static void net_new_session_wifi() {
  rsh_ctrl_session_init(&ctrlSession);
  rsh_ctrl_cache_init(&ctrlCache);
  wifiCapsOk = false;
}

static void net_new_session_serial() {
  rsh_ctrl_session_init(&serialCtrlSession);
  rsh_ctrl_cache_init(&serialCtrlCache);
  serialCapsOk = false;
}

// HELLO reply banao (92 B, §2.2 v2.2) — Wi-Fi aur serial dono yahi istemal
// karte hain (net_poll + net_serial_helo). Secret fields hamesha zero
// hain (builder khud zero likhta hai). Flags: sirf bit2 safe_mode.
// out kam az kam RSH_V2_HELLO_SIZE (92) bytes ka ho.
static size_t net_build_hello_reply(uint8_t sessionEcho, uint8_t *out) {
  uint8_t flags = 0;
  if (rsh_fw34_safe_mode_active()) flags |= RSH_V2_HELLO_SAFE_MODE;  // FW-3 bit2
  uint8_t mac[6];
  WiFi.softAPmacAddress(mac);
  // Version modules.h ke RSH_CAPS_FW_* constants se aati hai
  // (FIRMWARE_VERSION se match, F-12 single source).
  return rsh_v2_build_hello(out, flags, sessionEcho, mac, RSH_CAPS_FW_MAJOR,
                            RSH_CAPS_FW_MINOR, RSH_CAPS_FW_PATCH);
}

bool net_begin() {
  uint8_t ch = pick_channel();
  if (!net_start_ap_and_sockets(ch)) return false;
  if (!ctrlInitDone) {
    // Session/cache/limiter sirf ek dafa init hon — net retry (Wi-Fi
    // fail) par session reset NAHI hota (warna purane reqIds dobara
    // valid ho jate). Naya session sirf HELO sessionFlag=1 ya reboot par.
    rsh_ctrl_session_init(&ctrlSession);
    rsh_ctrl_cache_init(&ctrlCache);
    rsh_ping_limiter_init(&pingLimiter);
    // Phase G: serial control session bhi yahin init (alag reqIds —
    // Wi-Fi aur serial ke reqId ek doosre se takra na jayen).
    rsh_ctrl_session_init(&serialCtrlSession);
    rsh_ctrl_cache_init(&serialCtrlCache);
    // FW-3/FW-4 handler callbacks (txn pointer har delegate par fresh).
    rsh_fw34_handler_ctx_init(&fw34Ctx, NULL);
    fw34Ctx.password_verify_current = fw34_password_verify_cb;
    fw34Ctx.password_change_request = fw34_password_cb;
    fw34Ctx.cfg_persist = fw34_persist_cb;
    fw34Ctx.now_ms = fw34_now_cb;
    // LED_TEST (Phase H WS2): RMT WS2812 sink + driver. Sink ya driver
    // init fail ho to led_test_request NULL rehta hai — handler
    // fail-closed UNSUPPORTED deta hai (Phase G jaisa; test accept
    // karke strip na chalana false PASS hota). Kamyabi par adapter
    // (led_test_start_, upar) handler se pattern lekar driver chalata
    // hai; tick rsh.ino se net_led_tick() ke zariye aata hai.
    if (rsh_led_rmt_sink_init(PIN_LED_STRIP) &&
        rsh_led_driver_init(&ledDrv, LED_STRIP_COUNT, rsh_led_rmt_sink, NULL)) {
      fw34Ctx.led_test_request = led_test_start_;
      ledDrvReady = 1;
    } else {
      fw34Ctx.led_test_request = NULL;
    }
    fw34Ctx.led_pin = 0xFF;   // har delegate par current blob se fresh
    fw34Ctx.led_has_expanders = 0;
    fw34Ctx.user = NULL;
    ctrlInitDone = true;
  }
  lastScanMs = millis();
  lastSwitchMs = millis();
  txFailAtLastScan = (uint16_t)txStats.tx_fail;
  return true;
}

// FW-7/F8 rescan: sirf degraded link par + kam az kam 60 s waqfe se.
// Scan STA mode mangta hai — AP 1-3 s ke liye band hota hai, is liye ye
// sirf tab chalta hai jab link pehle se kharab ho. Channel tabdeel sirf
// tab jab naya score sakhti se behtar ho AUR 5 min guzar gaye hon.
// F-07 (v4.0.2): rescan default OFF hai (RSH_AUTO_CHANNEL) aur sirf jab
// koi PC active na ho (Wi-Fi ya serial) — active stream ke beech AP
// band karna stream drop + PC disassociate hai.
void net_maybe_rescan(uint32_t nowMs) {
  if (!rsh_rescan_allowed(RSH_AUTO_CHANNEL ? 1 : 0,
                          (net_pc_active(nowMs) || net_serial_pc_active(nowMs))
                              ? 1 : 0))
    return;
  if (!currentChannel) return;
  uint16_t delta = (uint16_t)(txStats.tx_fail - txFailAtLastScan);
  if (!rsh_rescan_needed(net_rssi_dbm(), delta, (uint32_t)(nowMs - lastScanMs))) return;
  WiFi.mode(WIFI_STA);
  WiFi.disconnect();
  // R3-FW-08: rescan bhi sync scan hai (loop iteration ke andar) — feed
  // karo taake scan + AP restart 5 s WDT window mein rahe.
  esp_task_wdt_reset();
  int n = WiFi.scanNetworks(false, false);
  esp_task_wdt_reset();
  uint8_t picked = currentChannel;
  int newScore = 0, curScore = 0;
  bool scanned = false;
  if (n >= 0) {
    rsh_scan_entry_t entries[32];
    size_t count = 0;
    for (int i = 0; i < n && count < 32; i++) {
      entries[count].channel = (uint8_t)WiFi.channel(i);
      entries[count].rssi_dbm = (int8_t)WiFi.RSSI(i);
      entries[count].hidden = 0;
      count++;
    }
    int load[14];
    rsh_scan_load(entries, count, load);
    picked = rsh_pick_channel(load);
    newScore = rsh_channel_score(load, picked);
    curScore = rsh_channel_score(load, currentChannel);
    scanned = true;
  }
  WiFi.scanDelete();
  lastScanMs = nowMs;
  txFailAtLastScan = (uint16_t)txStats.tx_fail;
  uint8_t target = currentChannel;
  if (scanned && rsh_channel_switch_allowed(newScore, curScore,
                                            (uint32_t)(nowMs - lastSwitchMs))) {
    target = picked;
    forcedChannel = picked;
    lastSwitchMs = nowMs;
  }
  // AP wapas lao (scan ne mode STA kar diya) — channel switch ho ya na ho.
  udpHello.stop();
  udpCtrl.stop();
  WiFi.softAPdisconnect(true);
  // DS-11: return check karo — fail ho to AP band rehta hai; diag event
  // + net_poll() retry (5 s throttle) warna board rescan ke baad
  // unreachable rehta aur koi retry nahi hoti (rescan 60 s+ waqfe se hai).
  if (!net_start_ap_and_sockets(target)) {
    rsh_diag_record_global(RSH_DIAG_AP_RESTART_FAIL, 2, nowMs);
    net_request_ap_restart_retry_(target);
  }
}

void net_poll() {
  // DS-11: pehle kisi failed AP restart (password/rescan) ki retry.
  net_retry_ap_restart_();
  // Per call cap: datagram flood sampler ko starve na kare (audit fix).
  // Har datagram ke baad flush() lazmi hai — core mein jab tak pichla
  // rx_buffer mojood ho, parsePacket() hamesha 0 deta hai; sirf 128 byte
  // parh kar chor dene se lamba datagram link ko hamesha ke liye band
  // kar deta tha (audit BUG #1). flush() baqi hissa discard kar deta hai.
  for (int i = 0; i < 8; i++) {
    int n = udpHello.parsePacket();
    if (n <= 0) break;
    IPAddress sender = udpHello.remoteIP();
    uint16_t senderPort = udpHello.remotePort();
    char buf[128];
    int len = udpHello.read(buf, sizeof(buf));
    udpHello.flush();   // remainder discard — rx_buffer hamesha clear
    if (len < 4 || memcmp(buf, "HELO", 4) != 0) continue;

    // Proto check (§2.1): 5th byte 0x01 legacy / 0x02 v2; kuch aur → ignore.
    // 4-byte HELO = legacy v1 (koi reply nahi, sirf active mark).
    uint8_t proto = 0x01;
    uint8_t sessionFlag = 0;
    if (len >= 5) {
      proto = (uint8_t)buf[4];
      if (proto != 0x01 && proto != 0x02) continue;
      if (len >= 6) {
        sessionFlag = (uint8_t)buf[5];
        if (sessionFlag > 1) continue;
      }
    }

    // SEC-D-08: live session ka pcIp koi doosra IP overwrite nahi kar
    // sakta (hijack); idle board par bhi 5 s mein ek se zyada change nahi.
    uint32_t nowMs = millis();
    uint8_t sameIp = (havePc && sender == pcIp) ? 1 : 0;
    if (!rsh_helo_accept(havePc ? 1 : 0, net_pc_active(nowMs) ? 1 : 0, sameIp,
                         (uint32_t)(nowMs - lastPcChangeMs))) {
      rsh_diag_record_global(RSH_DIAG_HELO_REJECT,
                             net_pc_active(nowMs) ? 0 : 1, nowMs);
      continue;
    }
    // R3-FW-02: naya peer, ya liveness timeout ke baad wapas aaya peer =
    // naya session — Wi-Fi dedup cache reset karo (warna app restart ke
    // baad reqIds 0 se shuru hote hain aur purane session ke cached
    // replies se pehle ~32 commands takrate hain). wasActive is HELO
    // se PEHLE ki state hai (lastHelloMs abhi purana hai). Routine
    // HELO (same IP, timeout ke andar) kabhi reset nahi karta.
    uint8_t wasActive = net_pc_active(nowMs) ? 1 : 0;
    if (!sameIp) {
      pcIp = sender;
      lastPcChangeMs = nowMs;
    }
    if (rsh_session_reset_needed(havePc ? 1 : 0, wasActive, sameIp))
      net_new_session_wifi();  // wifiCapsOk bhi saaf karta hai (D3)
    havePc = true;
    lastHelloMs = nowMs;
    lastHelloProto = proto;
    if (proto != 0x02) continue;  // legacy v1: koi HELLO reply nahi (§2.1)

    // Naya session: Wi-Fi control session reset + GET_CAPS-OK flag saaf
    // (§2.3/§3.2). Routine HELO (sessionFlag 0) purana session wapas
    // deta hai — jab tak peer naya na ho ya timeout ke baad wapas na
    // aaya ho (upar rsh_session_reset_needed, R3-FW-02).
    if (sessionFlag == 1) net_new_session_wifi();

    // HELLO reply rate limit (SEC-D-08): 250 ms mein ek se zyada nahi (F-04).
    if (!rsh_hello_reply_allowed((uint32_t)(nowMs - lastHelloReplyMs))) continue;
    lastHelloReplyMs = nowMs;

    uint8_t hello[RSH_V2_HELLO_SIZE];
    size_t hlen = net_build_hello_reply(sessionFlag, hello);
    // Reply sender ke source port par (§2.2) — udpData se (PC port 4211
    // par net_send nahi, kyun ke reply source port par jata hai).
    if (hlen > 0 && udpData.beginPacket(sender, senderPort)) {
      udpData.write(hello, hlen);
      txStats.record(udpData.endPacket() == 1);
    } else {
      txStats.record(false);
    }
  }
}

bool net_pc_active(uint32_t nowMs) {
  return havePc && rsh_pc_active(lastHelloMs, nowMs, HELLO_TIMEOUT_MS);
}

// D3: is session mein GET_CAPS OK hua ho + PC active ho. Timeout par PC
// inactive hote hi false — purana GET_CAPS-OK zinda nahi rehta.
bool net_caps_ok(uint32_t nowMs) {
  return wifiCapsOk && net_pc_active(nowMs);
}

bool net_send(const uint8_t *buf, size_t len) {
  if (!havePc) { txStats.record(false); return false; }
  if (!udpData.beginPacket(pcIp, PORT_DATA)) { txStats.record(false); return false; }
  udpData.write(buf, len);
  return txStats.record(udpData.endPacket() == 1);
}

// ---- Control handler (FW-8): sirf dispatcher bulata hai, aur sirf
// ACCEPT par (malformed / duplicate-cache-miss ke ilawa — ctrl.h).
// GET_CAPS: live config blob se pin table (Phase G, C3 — board ki
// table hi jeet-ti hai; fallback compiled default) + capsFlags. Stream
// gate (D3): GET_CAPS OK par wifiCapsOk lagta hai (net_poll_ctrl mein).
// REBOOT (0x09): reply ke BAAD restart (SEC-D-01 — RSTX khatam).
// SET_CFG / COMMIT / CONFIRM / REVERT / GET_CFG (FW-4), SET_PASSWORD
// (Phase E WS5, 0x0C, D3 current-password check), FACTORY_RESET
// (Phase 2 M-5, 0x08 — config defaults, password ko haath nahi) aur
// LED_TEST (Phase F WS5, 0x0A): ctrl_handlers.h ko delegate
// (rsh_fw34_handler) — txn rsh.ino se, NVS persist + clock callbacks se.
static uint8_t net_ctrl_handler(uint8_t cmd, const uint8_t *payload, uint16_t len,
                                uint8_t *reply_body, uint16_t *reply_body_len, void *ctx) {
  (void)payload;
  (void)len;
  net_ctrl_ctx_t *c = (net_ctrl_ctx_t *)ctx;
  if (c) c->lastCmd = cmd;
  uint8_t st = RSH_CTRL_UNSUPPORTED;
  *reply_body_len = 0;
  switch (cmd) {
    case RSH_CMD_GET_CFG:
    case RSH_CMD_SET_CFG:
    case RSH_CMD_COMMIT:
    case RSH_CMD_CONFIRM:
    case RSH_CMD_REVERT:
    case RSH_CMD_FACTORY_RESET:  // Phase 2 M-5: config defaults, password untouched
    case RSH_CMD_SET_PASSWORD:  // Phase E WS5: fw34_password_cb -> deferred NVS + AP restart
    case RSH_CMD_LED_TEST: {    // Phase F WS5: LED_TEST 0x0A — pattern validate;
      // Phase H WS2: driver hai (led_test_request = adapter, net_begin
      // mein set); sink init fail ho to callback NULL rehta hai aur
      // handler UNSUPPORTED deta hai (fail closed).
      fw34Ctx.txn = rsh_fw34_cfg_txn();  // har delegate par fresh pointer
      // LED state har delegate par current blob se fresh (COMMIT ke baad
      // LED_STRIP record badal sakta hai — stale pin par test na chale).
      fw34Ctx.led_pin = 0xFF;
      {
        rsh_cfg_txn_t *txn = rsh_fw34_cfg_txn();
        if (txn)
          rsh_cfg_led_pin(txn->current, txn->current_len, &fw34Ctx.led_pin);
        fw34Ctx.led_has_expanders =
            txn ? rsh_cfg_has_expanders(txn->current, txn->current_len) : 0;
      }
      st = rsh_fw34_handler(cmd, payload, len, reply_body, reply_body_len, &fw34Ctx);
      break;
    }
    case RSH_CMD_GET_DIAG: {
      // F-10 (v4.0.2): diagnostics snapshot — boot reason, heap min,
      // reserved field (hamesha 0, D10), ring-log events (diag_log.h
      // layout). Koi secret nahi (sirf codes + numeric values).
      // Len != 0 → UNSUPPORTED.
      if (len != 0) {
        st = RSH_CTRL_UNSUPPORTED;
        break;
      }
      size_t n = rsh_diag_build_body(rsh_diag_log(), 0, reply_body,
                                     RSH_V2_CTRL_MAX_PAYLOAD - 1);
      if (n == 0) {
        st = RSH_CTRL_UNSUPPORTED;
        break;
      }
      *reply_body_len = (uint16_t)n;
      st = RSH_CTRL_OK;
      break;
    }
    case RSH_CMD_HORN_TEST: {
      // D-5 (v4.0.2): horn button ka synthetic test pulse — payload
      // duration_ms u16 LE (100..5000). Stream mein horn bit us muddat
      // tak set rehta hai (net_horn_test_active, rsh.ino poochta hai).
      if (len != 2) {
        st = RSH_CTRL_UNSUPPORTED;
        break;
      }
      uint16_t dur = rsh_get_u16(payload);
      if (!rsh_horn_test_start(&hornTestState, dur, millis())) {
        st = RSH_CTRL_UNSUPPORTED;
        break;
      }
      st = RSH_CTRL_OK;
      break;
    }
    case RSH_CMD_GET_CAPS: {
      // Phase G (C3): board ki CURRENT pin table — live config blob se
      // (board ki table hi jeet-ti hai). Blob invalid ho to compiled
      // default table (fail closed — khaali/ghalat table kabhi nahi).
      RshCapsEntry table[40];
      uint8_t n = 0;
      uint8_t slowAxis = 0;  // FW-04: capsFlags bit0 (ADS1115 live ho to)
      {
        rsh_cfg_txn_t *txn = rsh_fw34_cfg_txn();
        if (txn) {
          RshPinMap map;
          if (rsh_pinmap_from_blob(txn->current, txn->current_len, &map)) {
            n = rsh_caps_table_from_pinmap(&map, table, 40);
            slowAxis = rsh_pinmap_has_slow_axis(&map);
          }
        }
      }
      if (n == 0) n = rsh_caps_default_table(table, 40);
      uint8_t mac[6];
      WiFi.softAPmacAddress(mac);
      uint8_t flags = 0;
      if (slowAxis) flags |= 0x01;  // bit0 slowAxis (FW-04): ADS1115 axis live
      if (rsh_fw34_safe_mode_active()) flags |= 0x02;  // bit1 safeMode (FW-3)
      uint8_t caps[18 + 8 * 40];
      size_t total = rsh_caps_build_body(caps, sizeof(caps), mac, net_channel(),
                                         flags, table, n);
      if (total == 0 || total < 1 ||
          total - 1 > (size_t)(RSH_V2_CTRL_MAX_PAYLOAD - 1))
        break;
      // rsh_caps_build_body status byte [0] mein deta hai — dispatcher
      // status khud prepend karta hai, is liye body [1..] lo.
      memcpy(reply_body, caps + 1, total - 1);
      *reply_body_len = (uint16_t)(total - 1);
      st = RSH_CTRL_OK;
      break;
    }
    case RSH_CMD_REBOOT:
      rebootPending = true;
      st = RSH_CTRL_OK;
      break;
    default:
      st = RSH_CTRL_UNSUPPORTED;
      break;
  }
  if (c) c->lastStatus = st;
  return st;
}

// Ping reply banao (34 B, §9.1/§9.3/§9.4) — UDP aur serial dono yahi
// istemal karte hain. Return reply length (34) ya 0 = SILENT drop
// (ghalat frame / busy / rate limit — koi error reply nahi, wo bhi
// amplification hai). srcKey: UDP par sender IP (u32), serial par
// RSH_SERIAL_PING_KEY (point-to-point link ka ek hi bucket).
// Reply SIRF caller ki zimmedari hai (UDP: udpData, serial: Serial).
#define RSH_SERIAL_PING_KEY 0xFFFFFFFEu
static size_t net_ping_build_reply(const uint8_t *req, int len, uint32_t srcKey,
                                   uint8_t *reply_out) {
  uint64_t t1 = net_micros64();  // receive ka waqt (pehle lo)
  // L1: magic check SHARED hai — UDP demux (neeche, "RSHN" check) aur
  // serial rasta (net_serial_ping — frame kind se aata hai, payload ka
  // magic nahi dekhta) dono yahin se guzarte hain. Baghair "RSHN" magic
  // ke frame drop (PROTOCOL-V2.2 §9: magic bytes 0-3).
  if (len != 34 || memcmp(req, "RSHN", 4) != 0 || req[4] != RSH_V2_VERSION || req[5] != 0) {
    pingDropped++;
    return 0;
  }
  if (rsh_crc16_ccitt(req, 32) != rsh_get_u16(req + 32)) {
    pingDropped++;
    return 0;
  }
  if (streamBusy) {  // §9.3: busy ho to drop — stream loop kabhi na roko
    pingDropped++;
    return 0;
  }
  if (!rsh_ping_allow(&pingLimiter, srcKey, millis())) {  // §9.4 token bucket
    pingDropped++;
    return 0;
  }
  memcpy(reply_out, req, 34);
  reply_out[5] = 1;  // reply
  rsh_put_u64(reply_out + 16, t1);
  rsh_put_u64(reply_out + 24, net_micros64());  // t2 = send ka waqt
  rsh_put_u16(reply_out + 32, rsh_crc16_ccitt(reply_out, 32));
  return 34;
}

static void handle_ping(const uint8_t *req, int len, const IPAddress &sender,
                        uint16_t senderPort) {
  uint32_t srcKey = ((uint32_t)sender[0] << 24) | ((uint32_t)sender[1] << 16) |
                    ((uint32_t)sender[2] << 8) | (uint32_t)sender[3];
  uint8_t reply[34];
  size_t n = net_ping_build_reply(req, len, srcKey, reply);
  if (n == 0) return;
  if (udpData.beginPacket(sender, senderPort)) {
    udpData.write(reply, n);
    txStats.record(udpData.endPacket() == 1);
  } else {
    txStats.record(false);
  }
}

// SET_PASSWORD ke baad AP restart (§5.2): naya password AP + WebOTA
// dono par laagu hota hai. Full ESP.restart NAHI — sirf AP restart,
// taake session (reqId dedup) aur stream state barqarar rahe.
// Password set karne ka matlab hai user ne AP lock kar diya: restart
// hamesha WPA2 hota hai (khula AP sirf password set hone se pehle hota
// hai, §4.2). Connected clients drop honge — dobara naye password se
// join karna hoga. DS-11: bool return — fail ho to caller (deferred
// action) retry flag ke zariye net_poll() retry karwata hai.
static bool net_restart_ap_with_new_password() {
  udpHello.stop();
  udpCtrl.stop();
  WiFi.softAPdisconnect(true);
  if (!WiFi.softAP(net_ssid_(), keystore_password(), currentChannel, 0, 4)) {
    rsh_diag_record_global(RSH_DIAG_AP_RESTART_FAIL, 1, millis());
    net_request_ap_restart_retry_(currentChannel);
    return false;
  }
  if (!udpHello.begin(PORT_HELLO) || !udpCtrl.begin(PORT_CTRL)) {
    rsh_diag_record_global(RSH_DIAG_AP_RESTART_FAIL, 1, millis());
    net_request_ap_restart_retry_(currentChannel);
    return false;
  }
  // udpData send-only hai (begin nahi hota) — dobara begin ki zaroorat nahi.
  // WebOTA auth har request par keystore_password() se hoti hai, is liye
  // naya password WebOTA par foran laagu hai (A7 par real board par verify).
  return true;
}

// Deferred actions — reply bhejne KE BAAD (SEC-D-01 pattern):
// SET_PASSWORD (NVS + AP restart), REBOOT (restart). UDP
// (net_poll_ctrl) aur serial (net_serial_after_reply) dono yahin se
// chalate hain — flags clear hote hain, is liye dobara call mehfooz hai
// (koi double action nahi).
static void net_run_deferred_actions() {
  // SET_PASSWORD: reply bhejne KE BAAD NVS write + AP restart
  // (§5.2 — reply OK restart se PEHLE jata hai). NVS fail ho to AP
  // restart NAHI (fail closed — purana password rehta hai). Pending
  // buffer hamesha saaf karo (password RAM mein na rahe).
  if (passwordChangePending) {
    passwordChangePending = false;
    uint8_t pwLen = pendingPasswordLen;
    bool pwOk = keystore_set_password((const char *)pendingPassword, pwLen);
    memset(pendingPassword, 0, sizeof(pendingPassword));
    pendingPasswordLen = 0;
    if (pwOk) {
      delay(50);
      net_restart_ap_with_new_password();
    }
  }
  // REBOOT: reply bhejne ke BAAD restart (SEC-D-01).
  if (rebootPending) {
    rebootPending = false;
    delay(50);
    ESP.restart();
  }
}

// FW-8 control channel (v2.2): port 4212 par asal dispatcher.
// Demux magic se: "RSHN" = ping (§9), "RSHC" = control (§3), baqi drop.
// Control sirf HELO wale peer (pcIp) se — ya jab koi peer abhi na ho
// (pehla HELO aane se pehle control ko poori tarah band karna first
// connect rok dega). Ping is pabandi se bahar hai (§9.5: HELO se pehle
// bhi chale).
void net_poll_ctrl() {
  for (int i = 0; i < 8; i++) {
    int n = udpCtrl.parsePacket();
    if (n <= 0) break;
    IPAddress sender = udpCtrl.remoteIP();
    uint16_t senderPort = udpCtrl.remotePort();
    static uint8_t rxBuf[RSH_V2_CTRL_MAX_FRAME + 8];
    int len = udpCtrl.read(rxBuf, sizeof(rxBuf));
    udpCtrl.flush();
    if (len < 4) continue;
    if (memcmp(rxBuf, "RSHN", 4) == 0) {
      handle_ping(rxBuf, len, sender, senderPort);
      continue;
    }
    if (memcmp(rxBuf, "RSHC", 4) != 0) continue;  // unknown magic → drop
    if (havePc && !(sender == pcIp)) continue;    // sirf HELO wala peer

    static uint8_t replyBuf[RSH_V2_CTRL_MAX_FRAME + 8];
    size_t replyLen = 0;
    rsh_ctrl_disp_t d = rsh_ctrl_dispatch(
        rxBuf, (size_t)len,
        &ctrlSession, &ctrlCache, net_ctrl_handler, &ctrlCtx,
        replyBuf, sizeof(replyBuf), &replyLen);
    if (d != RSH_CTRL_DISP_REPLY) continue;
    // D3 stream gate: GET_CAPS OK par is session mein stream khulta hai.
    if (ctrlCtx.lastCmd == RSH_CMD_GET_CAPS && ctrlCtx.lastStatus == RSH_CTRL_OK)
      wifiCapsOk = true;
    if (udpData.beginPacket(sender, senderPort)) {
      udpData.write(replyBuf, replyLen);
      txStats.record(udpData.endPacket() == 1);
    } else {
      txStats.record(false);
    }
    // Reply bhejne ke baad deferred actions (SET_PASSWORD / REBOOT).
    net_run_deferred_actions();
  }
}

// ============================================================================
// Wired USB-serial transport (Phase G, D26 / OPEN-20 — net.h dekho).
//
// Serial peer Wi-Fi peer se ALAG session hai (apna HELO timer, apna
// control session/cache, apna GET_CAPS-OK flag). Framing
// (serial_transport.h) serial_link.cpp ki taraf hai — yahan sirf wohi
// logic hai jo Wi-Fi path se share hoti hai (HELLO build, plain
// dispatch, ping reply, deferred actions).
// ============================================================================

void net_serial_begin() {
  // Idempotent: serial control session/cache init. net_begin ke
  // ctrlInitDone block mein bhi yehi init hota hai (dobara init
  // memset hai — mehfooz). Zero-init BSS bhi session ke liye kaafi hota
  // (rsh_ctrl_session_init sirf memset hai).
  static bool serialInitDone = false;
  if (serialInitDone) return;
  rsh_ctrl_session_init(&serialCtrlSession);
  rsh_ctrl_cache_init(&serialCtrlCache);
  serialInitDone = true;
}

bool net_serial_pc_active(uint32_t nowMs) {
  return serialHavePc && rsh_pc_active(serialLastHelloMs, nowMs, HELLO_TIMEOUT_MS);
}

bool net_serial_pc_is_v2(uint32_t nowMs) {
  (void)nowMs;
  return serialHavePc && serialLastHelloProto == 0x02;
}

// D3 (serial): is session mein GET_CAPS OK hua ho + serial peer active ho.
// Timeout par PC inactive hote hi false.
bool net_serial_caps_ok(uint32_t nowMs) {
  return serialCapsOk && net_serial_pc_active(nowMs);
}

// Serial HELO (serial frame kind=HELO ki payload, §2.1) → HELLO reply
// (92 B, §2.2) hello_out mein, ya 0 (legacy v1 HELO — koi reply nahi;
// ghalat proto/sessionFlag — ignore; rate limit — khamoshi).
size_t net_serial_helo(const uint8_t *helo, size_t len, uint32_t nowMs,
                       uint8_t *hello_out) {
  if (!helo || !hello_out) return 0;
  if (len < 4 || memcmp(helo, "HELO", 4) != 0) return 0;
  // Proto check (§2.1): 5th byte 0x01 legacy / 0x02 v2; kuch aur → ignore.
  // 4-byte HELO = legacy v1 (koi reply nahi, sirf active mark).
  uint8_t proto = 0x01;
  uint8_t sessionFlag = 0;
  if (len >= 5) {
    proto = helo[4];
    if (proto != 0x01 && proto != 0x02) return 0;
    if (len >= 6) {
      sessionFlag = helo[5];
      if (sessionFlag > 1) return 0;
    }
  }
  // Serial point-to-point hai — IP takeover ka sawal hi nahi (SEC-D-08
  // sirf Wi-Fi AP par laagu hai). Har HELO peer ko active karta hai.
  serialHavePc = true;
  serialLastHelloMs = nowMs;
  serialLastHelloProto = proto;
  if (proto != 0x02) return 0;  // legacy v1: koi HELLO reply nahi (§2.1)
  if (sessionFlag == 1) net_new_session_serial();  // naya session — sirf serial reset (R3-FW-11)
  // HELLO reply rate limit (SEC-D-08 jaisa): 250 ms mein ek se zyada nahi (F-04).
  if (!rsh_hello_reply_allowed((uint32_t)(nowMs - serialLastHelloReplyMs))) return 0;
  serialLastHelloReplyMs = nowMs;
  return net_build_hello_reply(sessionFlag, hello_out);
}

// Serial control frame dispatch — Wi-Fi path (net_poll_ctrl) jaisa hi:
// structure + CRC parse → reqId classify → handler (sirf ACCEPT par).
// Reply reply_out mein (RSH_V2_CTRL_MAX_FRAME bytes ka buffer chahiye).
// GET_CAPS OK par serial stream gate khulta hai (D3).
bool net_serial_dispatch_control(const uint8_t *frame, size_t len,
                                 uint8_t *reply_out, size_t reply_out_len,
                                 size_t *reply_len_out) {
  rsh_ctrl_disp_t d = rsh_ctrl_dispatch(
      frame, len,
      &serialCtrlSession, &serialCtrlCache, net_ctrl_handler, &serialCtrlCtx,
      reply_out, reply_out_len, reply_len_out);
  if (d != RSH_CTRL_DISP_REPLY) return false;
  if (serialCtrlCtx.lastCmd == RSH_CMD_GET_CAPS &&
      serialCtrlCtx.lastStatus == RSH_CTRL_OK)
    serialCapsOk = true;
  return true;
}

// Serial ping reply (34 B, §9) — net_ping_build_reply shared hai
// (rate limit bucket: point-to-point link ke liye ek fixed key).
size_t net_serial_ping(const uint8_t *req, int len, uint8_t *reply_out) {
  if (!req || !reply_out) return 0;
  return net_ping_build_reply(req, len, RSH_SERIAL_PING_KEY, reply_out);
}

// Serial reply bhejne KE BAAD deferred actions (REBOOT/SET_PASSWORD) —
// net_poll_ctrl wala hi rasta (flags clear hote hain).
void net_serial_after_reply() {
  net_run_deferred_actions();
}
