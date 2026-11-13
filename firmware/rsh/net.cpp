#include "net.h"
#include "net_logic.h"   // FW-7: channel score / pick (pure, host-testable)
#include "protocol.h"    // FW-8: v2.1 builders / parsers
#include "ctrl.h"        // FW-8: HMAC control dispatcher (SEC-D-06)
#include "ctrl_handlers.h"  // FW-3/FW-4: SET_CFG/COMMIT/.../KEY_RESET handler (pure)
#include "led_driver.h"    // Phase G WS5: LED driver abstraction.
                           // Phase H WS2: net.cpp mein wired hai —
                           // led_test_request adapter + net_led_tick().
#include "led_sink_rmt.h"  // Phase H WS2: RMT WS2812 sink (GPIO21)
#include "fw34_state.h"    // FW-3/FW-4: safe mode + config txn state (rsh.ino)
#include "keystore.h"    // FW-8: per-device key/nonce/pairing window
#include "modules.h"     // FW-11: GET_CAPS default pin table
#include "config.h"
#include <Arduino.h>
#include <WiFi.h>
#include <WiFiUdp.h>
#include <ESPmDNS.h>   // Phase G (C2/D33): mDNS responder — board `rsh.local`
#include <esp_wifi.h>    // per-station RSSI (SEC-D-08 / status packet)
#include <mbedtls/md.h>  // HMAC-SHA256 (mbedTLS, firmware ka crypto)

static WiFiUDP udpHello;   // 4210 par HELO sunta hai
static WiFiUDP udpData;    // 4211 par data bhejta hai (+ HELLO/control/ping replies)
static WiFiUDP udpCtrl;    // 4212 par control (HMAC) + ping sunta hai
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
static rsh_ctrl_session_t ctrlSession;  // FW-8: (reqId, counter) state
static rsh_ctrl_cache_t ctrlCache;      // FW-8: duplicate cached replies
static rsh_ping_limiter_t pingLimiter;  // §9.4: per-source token bucket
static bool ctrlInitDone = false;       // session sirf ek dafa init ho
static bool sessionAuthenticated = false;  // SEC-E-14: is session mein HMAC-verified control ACCEPT hua? (net_ctrl_handler set karta hai; peer/session badle to clear)
// Phase G (D26/OPEN-20): wired serial peer ka ALAG session — apna HELO
// timer, apna control session/cache, apna SEC-E-14 auth flag. Nonce/key
// keystore se shared hain (board-global).
static rsh_ctrl_session_t serialCtrlSession;
static rsh_ctrl_cache_t serialCtrlCache;
static bool serialSessionAuthenticated = false;
static bool serialHavePc = false;
static uint32_t serialLastHelloMs = 0;
static uint32_t serialLastHelloReplyMs = 0;
static uint8_t serialLastHelloProto = 0;
static bool rebootPending = false;      // HMAC REBOOT (0x09) mila — reply ke baad restart
static bool keyResetPending = false;    // HMAC KEY_RESET (0x0B) mila — reply ke baad rotate + restart
static bool passwordChangePending = false;  // HMAC SET_PASSWORD (0x0C) mila — reply ke baad NVS + AP restart
static uint8_t pendingPassword[RSH_PASSWORD_MAX_LEN];  // deferred SET_PASSWORD bytes (reply ke baad apply)
static uint8_t pendingPasswordLen = 0;

// Control handler ka context: aakhri command + status (pairing + tests ke liye).
// auth_flag: is transport ka SEC-E-14 flag (Wi-Fi → &sessionAuthenticated,
// serial → &serialSessionAuthenticated) — handler sirf ACCEPT par chalta hai,
// is liye yahan pahunchna hi us transport ki authentication hai.
typedef struct {
  uint8_t lastCmd;
  uint8_t lastStatus;
  bool *auth_flag;
} net_ctrl_ctx_t;
static net_ctrl_ctx_t ctrlCtx = {0, 0, NULL};
static net_ctrl_ctx_t serialCtrlCtx = {0, 0, NULL};

// FW-3/FW-4 handler callbacks (ctrl_handlers.h): armed state keystore se
// (wo FW-3 state parhta hai), persist + txn rsh.ino se, clock millis().
// key_reset_request sirf pending flag lagata hai — asal rotation + reboot
// reply bhejne KE BAAD hota hai (neeche net_poll_ctrl mein).
static uint8_t fw34_armed_cb(void *user) {
  (void)user;
  return keystore_key_reset_armed() ? 1 : 0;
}
static void fw34_key_reset_cb(void *user) {
  (void)user;
  keyResetPending = true;
}
// SET_PASSWORD deferred: password yahan sirf RAM mein rakho (reply ke
// baad keystore_set_password + AP restart hota hai — neeche net_poll_ctrl).
// Handler ne length/charset pehle hi validate kar li hai (§5.2).
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

// HMAC-SHA256(key, nonce || counter_LE || cmd || payload)[0:16] (§3.2).
// Static buffer — koi heap allocation nahi (ping/control path §9.3 style).
static uint8_t hmacMsg[16 + 4 + 1 + RSH_V2_CTRL_MAX_PAYLOAD];
static void net_hmac16(const uint8_t *key32, const uint8_t *nonce16, uint32_t counter,
                       uint8_t cmd, const uint8_t *payload, uint16_t payload_len,
                       uint8_t tag_out[16]) {
  memcpy(hmacMsg, nonce16, 16);
  hmacMsg[16] = (uint8_t)(counter & 0xFF);
  hmacMsg[17] = (uint8_t)((counter >> 8) & 0xFF);
  hmacMsg[18] = (uint8_t)((counter >> 16) & 0xFF);
  hmacMsg[19] = (uint8_t)((counter >> 24) & 0xFF);
  hmacMsg[20] = cmd;
  if (payload_len > 0) memcpy(hmacMsg + 21, payload, payload_len);
  uint8_t full[32];
  mbedtls_md_hmac(mbedtls_md_info_from_type(MBEDTLS_MD_SHA256),
                  key32, RSH_KEY_LEN, hmacMsg, 21u + payload_len, full);
  memcpy(tag_out, full, 16);
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
// Koi station nahi / API fail → -128 (unknown, RSH_RSSI_UNKNOWN).
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
  int n = WiFi.scanNetworks(false, false);  // show_hidden=false: hidden exclude
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

// AP + sockets ek channel par shuru karo. Pehli pairing tak AP KHULA hai
// (§4.2 — pairing ke baad WPA2, password per-device keystore se).
// keystore_begin() setup mein pehle ho chuka hona chahiye.
static bool net_start_ap_and_sockets(uint8_t ch) {
  WiFi.mode(WIFI_AP);
  bool open = keystore_unpaired();
  bool apOk;
  if (open) apOk = WiFi.softAP(WIFI_SSID, (const char *)NULL, ch, 0, 4);
  else apOk = WiFi.softAP(WIFI_SSID, keystore_password(), ch, 0, 4);
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
  // Pairing window SIRF pehli AP-up se 60 s (§2.3) — rescan par AP
  // restart ho to window dobara NAHI khulti (warna window barh jati).
  static bool apUpMarked = false;
  if (!apUpMarked) {
    keystore_mark_ap_up(millis());
    apUpMarked = true;
  }
  return true;
}

// Naya session (HELO sessionFlag=1, kisi bhi transport par): nonce
// rotate + DONO transports ke control sessions reset (§2.3/§3.2).
// Nonce board-global hai (keystore), is liye ek transport ka naya session
// doosre transport ke purane counters ko bhi invalid kar deta hai —
// doosre peer ko dobara HELLO (naya session) + GET_CAPS karna hoga.
// Yehi wajah hai ke dual-transport mein aam tor par ek waqt mein ek hi
// transport istemal hota hai (§10).
static void net_new_session() {
  keystore_rotate_nonce();
  rsh_ctrl_session_init(&ctrlSession);
  rsh_ctrl_cache_init(&ctrlCache);
  sessionAuthenticated = false;
  rsh_ctrl_session_init(&serialCtrlSession);
  rsh_ctrl_cache_init(&serialCtrlCache);
  serialSessionAuthenticated = false;
}

// HELLO reply banao (92 B, §2.2) — Wi-Fi aur serial dono yahi istemal
// karte hain (net_poll + net_serial_helo). Key/password sirf unpaired +
// window ke andar (keystore_pairing_open) — window ke baad zero (§2.2).
// out kam az kam RSH_V2_HELLO_SIZE (92) bytes ka ho.
static size_t net_build_hello_reply(uint8_t sessionEcho, uint8_t *out) {
  uint8_t flags = 0;
  uint8_t zeroKey[RSH_KEY_LEN] = {0};
  uint8_t zeroPw[RSH_PASSWORD_LEN] = {0};
  const uint8_t *dk = zeroKey;
  const uint8_t *pw = zeroPw;
  if (keystore_unpaired()) {
    flags |= RSH_V2_HELLO_UNPAIRED;
    if (keystore_pairing_open(millis()) && keystore_key()) {
      flags |= RSH_V2_HELLO_WINDOW_OPEN;
      dk = keystore_key();
      // wifi_password field sirf 24 B hai (§2.2): user ne SET_PASSWORD
      // se lamba/chhota password set kiya ho to field ZERO rehta hai —
      // app ke paas password pehle se hai (us ne khud set kiya tha aur
      // DPAPI mein rakha hai), is liye kuch khota nahi. Generated
      // password hamesha 24 chars ka hota hai (§5.1).
      if (strlen(keystore_password()) == RSH_PASSWORD_LEN)
        pw = (const uint8_t *)keystore_password();
    }
  }
  if (keystore_key_reset_armed()) flags |= RSH_V2_HELLO_KEY_RESET;
  if (rsh_fw34_safe_mode_active()) flags |= RSH_V2_HELLO_SAFE_MODE;  // FW-3 bit2
  uint8_t mac[6];
  WiFi.softAPmacAddress(mac);
  return rsh_v2_build_hello(out, flags, sessionEcho, mac, 4, 0, 0,
                            keystore_nonce(), dk, pw);
}

bool net_begin() {
  uint8_t ch = pick_channel();
  if (!net_start_ap_and_sockets(ch)) return false;
  if (!ctrlInitDone) {
    // Session/cache/limiter sirf ek dafa init hon — net retry (Wi-Fi
    // fail) par session reset NAHI hota (warna purane counters dobara
    // valid ho jate). Naya session sirf HELO sessionFlag=1 ya reboot par.
    rsh_ctrl_session_init(&ctrlSession);
    rsh_ctrl_cache_init(&ctrlCache);
    rsh_ping_limiter_init(&pingLimiter);
    // Phase G: serial control session bhi yahin init (alag counters —
    // Wi-Fi aur serial ke (reqId, counter) pair ek doosre se takra na jayen).
    rsh_ctrl_session_init(&serialCtrlSession);
    rsh_ctrl_cache_init(&serialCtrlCache);
    ctrlCtx.auth_flag = &sessionAuthenticated;
    serialCtrlCtx.auth_flag = &serialSessionAuthenticated;
    // FW-3/FW-4 handler callbacks (txn pointer har delegate par fresh).
    rsh_fw34_handler_ctx_init(&fw34Ctx, NULL);
    fw34Ctx.key_reset_armed = fw34_armed_cb;
    fw34Ctx.key_reset_request = fw34_key_reset_cb;
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
void net_maybe_rescan(uint32_t nowMs) {
  if (!currentChannel) return;
  uint16_t delta = (uint16_t)(txStats.tx_fail - txFailAtLastScan);
  if (!rsh_rescan_needed(net_rssi_dbm(), delta, (uint32_t)(nowMs - lastScanMs))) return;
  WiFi.mode(WIFI_STA);
  WiFi.disconnect();
  int n = WiFi.scanNetworks(false, false);
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
  net_start_ap_and_sockets(target);  // fail ho to agle rescan/retry par dobara
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
                         (uint32_t)(nowMs - lastPcChangeMs)))
      continue;
    if (!sameIp) {
      pcIp = sender;
      lastPcChangeMs = nowMs;
      sessionAuthenticated = false;  // SEC-E-14: naya peer = naya session — dobara authenticate lazmi
    }
    havePc = true;
    lastHelloMs = nowMs;
    lastHelloProto = proto;
    if (proto != 0x02) continue;  // legacy v1: koi HELLO reply nahi (§2.1)

    // Naya session: nonce rotate + control session reset (§2.3/§3.2).
    // Routine HELO (sessionFlag 0) purana nonce wapas deta hai.
    if (sessionFlag == 1) net_new_session();

    // HELLO reply rate limit (SEC-D-08): 500 ms mein ek se zyada nahi.
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

// SEC-E-14: session authenticated hai? (is boot mein is peer se kam az
// kam ek HMAC-verified control command ACCEPT hua ho) + PC active ho.
// Timeout par PC inactive hote hi false — purani authentication zinda nahi rehti.
bool net_session_authenticated(uint32_t nowMs) {
  return sessionAuthenticated && net_pc_active(nowMs);
}

bool net_send(const uint8_t *buf, size_t len) {
  if (!havePc) { txStats.record(false); return false; }
  if (!udpData.beginPacket(pcIp, PORT_DATA)) { txStats.record(false); return false; }
  udpData.write(buf, len);
  return txStats.record(udpData.endPacket() == 1);
}

// ---- Control handler (FW-8): sirf dispatcher bulata hai, aur sirf
// ACCEPT par (BAD_TAG / REPLAY / DUPLICATE par kabhi nahi — ctrl.h).
// GET_CAPS: live config blob se pin table (Phase G, C3 — board ki
// table hi jeet-ti hai; fallback compiled default) + capsFlags.
// REBOOT (0x09): reply ke BAAD restart (SEC-D-01 — RSTX khatam).
// SET_CFG / COMMIT / CONFIRM / REVERT / GET_CFG (FW-4), KEY_RESET
// (FW-3, 0x0B), SET_PASSWORD (Phase E WS5, 0x0C) aur LED_TEST
// (Phase F WS5, 0x0A): ctrl_handlers.h ko delegate (rsh_fw34_handler) —
// txn + armed state rsh.ino se, NVS persist + clock callbacks se.
static uint8_t net_ctrl_handler(uint8_t cmd, const uint8_t *payload, uint16_t len,
                                uint8_t *reply_body, uint16_t *reply_body_len, void *ctx) {
  (void)payload;
  (void)len;
  // SEC-E-14: handler sirf HMAC-verified ACCEPT par chalta hai (ctrl.h:
  // BAD_TAG / REPLAY / malformed kabhi yahan nahi pahunchte) — is liye
  // yahan pahunchna hi is transport ki authentication hai: is ke baad hi
  // stream emission khulti hai (rsh.ino / stream_emit.h). Kaunsa flag
  // set ho, ye ctx batata hai (Wi-Fi ya serial — alag sessions).
  net_ctrl_ctx_t *c = (net_ctrl_ctx_t *)ctx;
  if (c && c->auth_flag) *c->auth_flag = true;
  if (c) c->lastCmd = cmd;
  uint8_t st = RSH_CTRL_UNSUPPORTED;
  *reply_body_len = 0;
  switch (cmd) {
    case RSH_CMD_GET_CFG:
    case RSH_CMD_SET_CFG:
    case RSH_CMD_COMMIT:
    case RSH_CMD_CONFIRM:
    case RSH_CMD_REVERT:
    case RSH_CMD_KEY_RESET:
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
    case RSH_CMD_GET_CAPS: {
      // Phase G (C3): board ki CURRENT pin table — live config blob se
      // (board ki table hi jeet-ti hai). Blob invalid ho to compiled
      // default table (fail closed — khaali/ghalat table kabhi nahi).
      RshCapsEntry table[40];
      uint8_t n = 0;
      {
        rsh_cfg_txn_t *txn = rsh_fw34_cfg_txn();
        if (txn) {
          RshPinMap map;
          if (rsh_pinmap_from_blob(txn->current, txn->current_len, &map))
            n = rsh_caps_table_from_pinmap(&map, table, 40);
        }
      }
      if (n == 0) n = rsh_caps_default_table(table, 40);
      uint8_t mac[6];
      WiFi.softAPmacAddress(mac);
      uint8_t flags = 0;
      if (keystore_unpaired()) flags |= 0x04;  // capsFlags bit2 unpaired (§3.4)
      if (keystore_key_reset_armed()) flags |= 0x08;  // bit3 keyResetArmed (FW-3)
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
  if (len != 34 || req[4] != RSH_V2_VERSION || req[5] != 0) {
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
// taake session (key/nonce/counters) aur stream state barqarar rahe.
// Password set karne ka matlab hai user ne AP lock kar diya: restart
// hamesha WPA2 hota hai (khula AP sirf pairing se pehle hota hai, §4.2).
// Pairing window dobara NAHI khulti (apUpMarked guard
// net_start_ap_and_sockets mein hai — ye function use nahi karta).
// Connected clients drop honge — dobara naye password se join karna hoga.
static void net_restart_ap_with_new_password() {
  udpHello.stop();
  udpCtrl.stop();
  WiFi.softAPdisconnect(true);
  WiFi.softAP(WIFI_SSID, keystore_password(), currentChannel, 0, 4);
  udpHello.begin(PORT_HELLO);
  udpCtrl.begin(PORT_CTRL);
  // udpData send-only hai (begin nahi hota) — dobara begin ki zaroorat nahi.
  // WebOTA auth har request par keystore_password() se hoti hai, is liye
  // naya password WebOTA par foran laagu hai (A7 par real board par verify).
}

// Deferred actions — reply bhejne KE BAAD (SEC-D-01 pattern):
// HMAC KEY_RESET (rotate + restart), SET_PASSWORD (NVS + AP restart),
// REBOOT (restart). UDP (net_poll_ctrl) aur serial (net_serial_after_reply)
// dono yahin se chalate hain — flags clear hote hain, is liye dobara
// call mehfooz hai (koi double action nahi).
static void net_run_deferred_actions() {
  // HMAC KEY_RESET: reply bhejne KE BAAD credentials rotate karo (nayi
  // key+password NVS mein, unpaired), phir restart — naye boot par open
  // AP + nayi pairing window (§4.4). Rotate fail ho to restart NAHI
  // (fail closed — purani key rehti hai; armed window expiry par
  // normal reboot hota hai). REBOOT wala deferred pattern (SEC-D-01).
  if (keyResetPending) {
    keyResetPending = false;
    if (keystore_key_reset()) {
      delay(50);
      ESP.restart();
    }
  }
  // HMAC SET_PASSWORD: reply bhejne KE BAAD NVS write + AP restart
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
  // HMAC REBOOT: reply bhejne ke BAAD restart (SEC-D-01).
  if (rebootPending) {
    rebootPending = false;
    delay(50);
    ESP.restart();
  }
}

// FW-8 control channel (SEC-D-06): port 4212 par asal dispatcher.
// Demux magic se: "RSHN" = ping (§9), "RSHC" = control (§3), baqi drop.
// Control sirf HELO wale peer (pcIp) se — ya jab koi peer abhi na ho
// (pairing se pehle GET_CAPS ke liye HELO lazmi hai, lekin pehla HELO
// aane se pehle control ko poori tarah band karna pairing rok dega).
// Ping is pabandi se bahar hai (§9.5: pairing/HELO se pehle bhi chale).
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
    if (!keystore_key() || !keystore_nonce()) continue;  // keystore down → fail closed

    static uint8_t replyBuf[RSH_V2_CTRL_MAX_FRAME + 8];
    size_t replyLen = 0;
    rsh_ctrl_disp_t d = rsh_ctrl_dispatch(
        rxBuf, (size_t)len, keystore_key(), keystore_nonce(), net_hmac16,
        &ctrlSession, &ctrlCache, net_ctrl_handler, &ctrlCtx,
        replyBuf, sizeof(replyBuf), &replyLen);
    if (d != RSH_CTRL_DISP_REPLY) continue;
    // GET_CAPS OK par pairing complete (§3 step 7): ab HELLO mein key nahi.
    if (ctrlCtx.lastCmd == RSH_CMD_GET_CAPS && ctrlCtx.lastStatus == RSH_CTRL_OK)
      keystore_mark_paired();
    if (udpData.beginPacket(sender, senderPort)) {
      udpData.write(replyBuf, replyLen);
      txStats.record(udpData.endPacket() == 1);
    } else {
      txStats.record(false);
    }
    // Reply bhejne ke baad deferred actions (KEY_RESET / SET_PASSWORD / REBOOT).
    net_run_deferred_actions();
  }
}

// ============================================================================
// Wired USB-serial transport (Phase G, D26 / OPEN-20 — net.h dekho).
//
// Serial peer Wi-Fi peer se ALAG session hai (apna HELO timer, apna
// control session/cache, apna SEC-E-14 auth flag). Framing
// (serial_transport.h) serial_link.cpp ki taraf hai — yahan sirf wohi
// logic hai jo Wi-Fi path se share hoti hai (HELLO build, HMAC dispatch,
// ping reply, deferred actions). Nonce/key keystore se shared hain.
// ============================================================================

void net_serial_begin() {
  // Idempotent: serial control session/cache + ctx auth flag. net_begin
  // ke ctrlInitDone block mein bhi yehi init hota hai (dobara init
  // memset hai — mehfooz). Zero-init BSS bhi session ke liye kaafi hota
  // (rsh_ctrl_session_init sirf memset hai), lekin auth_flag pointer
  // lazmi set hona chahiye — warna handler kisi flag ko set nahi karta
  // aur SEC-E-14 gate kabhi nahi khulta.
  static bool serialInitDone = false;
  if (serialInitDone) return;
  rsh_ctrl_session_init(&serialCtrlSession);
  rsh_ctrl_cache_init(&serialCtrlCache);
  serialCtrlCtx.auth_flag = &serialSessionAuthenticated;
  serialInitDone = true;
}

bool net_serial_pc_active(uint32_t nowMs) {
  return serialHavePc && rsh_pc_active(serialLastHelloMs, nowMs, HELLO_TIMEOUT_MS);
}

bool net_serial_pc_is_v2(uint32_t nowMs) {
  (void)nowMs;
  return serialHavePc && serialLastHelloProto == 0x02;
}

// SEC-E-14 (serial): is session mein HMAC-verified control ACCEPT hua ho
// + serial peer active ho. Timeout par PC inactive hote hi false.
bool net_serial_session_authenticated(uint32_t nowMs) {
  return serialSessionAuthenticated && net_serial_pc_active(nowMs);
}

// Serial HELO (serial frame kind=HELO ki payload, §2.1) → HELLO reply
// (92 B, §2.2) hello_out mein, ya 0 (legacy v1 HELO — koi reply nahi;
// ghalat proto/sessionFlag — ignore; rate limit — khamoshi).
// Pairing window ka rule Wi-Fi jaisa hi hai (§2.3: AP-up/boot se 60 s) —
// window ka waqt keystore_mark_ap_up se chalta hai (net_start_ap_and_
// sockets), transport se nahi. USB cable physical possession mangta hai,
// is liye wired pairing Wi-Fi pairing se KAMZOR nahi — window phir bhi
// 60 s hi hai (conservative reuse, §10).
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
  if (sessionFlag == 1) net_new_session();  // naya nonce + dono sessions reset
  // HELLO reply rate limit (SEC-D-08 jaisa): 500 ms mein ek se zyada nahi.
  if (!rsh_hello_reply_allowed((uint32_t)(nowMs - serialLastHelloReplyMs))) return 0;
  serialLastHelloReplyMs = nowMs;
  return net_build_hello_reply(sessionFlag, hello_out);
}

// Serial control frame dispatch — Wi-Fi path (net_poll_ctrl) jaisa hi:
// structure → HMAC verify → session classify → handler (sirf ACCEPT par).
// Reply reply_out mein (RSH_V2_CTRL_MAX_FRAME bytes ka buffer chahiye).
// GET_CAPS OK par pairing complete (§3 step 7): ab HELLO mein key nahi.
bool net_serial_dispatch_control(const uint8_t *frame, size_t len,
                                 uint8_t *reply_out, size_t reply_out_len,
                                 size_t *reply_len_out) {
  if (!keystore_key() || !keystore_nonce()) return false;  // keystore down → fail closed
  rsh_ctrl_disp_t d = rsh_ctrl_dispatch(
      frame, len, keystore_key(), keystore_nonce(), net_hmac16,
      &serialCtrlSession, &serialCtrlCache, net_ctrl_handler, &serialCtrlCtx,
      reply_out, reply_out_len, reply_len_out);
  if (d != RSH_CTRL_DISP_REPLY) return false;
  if (serialCtrlCtx.lastCmd == RSH_CMD_GET_CAPS &&
      serialCtrlCtx.lastStatus == RSH_CTRL_OK)
    keystore_mark_paired();
  return true;
}

// Serial ping reply (34 B, §9) — net_ping_build_reply shared hai
// (rate limit bucket: point-to-point link ke liye ek fixed key).
size_t net_serial_ping(const uint8_t *req, int len, uint8_t *reply_out) {
  if (!req || !reply_out) return 0;
  return net_ping_build_reply(req, len, RSH_SERIAL_PING_KEY, reply_out);
}

// Serial reply bhejne KE BAAD deferred actions (REBOOT/KEY_RESET/
// SET_PASSWORD) — net_poll_ctrl wala hi rasta (flags clear hote hain).
void net_serial_after_reply() {
  net_run_deferred_actions();
}
