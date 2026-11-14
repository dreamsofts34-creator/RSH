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
#include "stream_emit.h"     // Phase E WS5: v2.1 stream emission (v2 peer)
#include "inputs.h"
#include "heap_policy.h"   // FW-10 / D17 heap + restart policy
#include "keystore.h"      // FW-8: per-device key/nonce (setup mein net se pehle)
#include "secure_nvs.h"     // Phase H (SEC-E-01/OPEN-3): secure-build posture check
#include "safemode.h"      // FW-3 (Workstream 1): safe mode + boot counter
#include "config_transaction.h"  // FW-4 (Workstream 1): config transaction
#include "fw34_state.h"    // FW-7/8 wiring: net.cpp/keystore.cpp se accessors
#include <Preferences.h>   // FW-3/FW-4 NVS glue (boot counter + config slots)
#include <esp_system.h>  // Round 2 W3 (FIX-4): esp_reset_reason() (status packet)
#include "net.h"
#include "webota.h"
#include "status_led.h"
#include "serial_link.h"  // Phase G: wired USB-serial transport (D26/OPEN-20)

// Fail closed: RSH_DEBUG prints aur wired transport ek hi Serial istemal
// karte hain — dono ek saath on hon to prints "RSHS" frame stream kharab
// karte hain. Ek chuno: debug log (RSH_WIRED_SERIAL 0) ya transport (1).
#if RSH_DEBUG && RSH_WIRED_SERIAL
#error "RSH_DEBUG 1 aur RSH_WIRED_SERIAL 1 ek saath nahi — Serial ek hi kaam kar sakta hai (debug log ya transport)."
#endif

static uint16_t seq = 0;
static uint16_t statusSeq = 0;   // status packet sequence (data seq se alag)
static uint32_t lastStatusMs = 0;
static uint16_t seqGap = 0;      // D5: boot se skip hue stream ticks
static uint16_t lastSentSeqGap = 0;  // v2 tick-skipped flag ke liye (Phase E WS5)
static bool keyOk = false;       // FW-8: keystore (key/nonce) tayyar hai?
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
static HeapPolicy heapPolicy;   // FW-10 / D17: largest block + 5-min drop

// ---- FW-3 safe mode + FW-4 config transaction (Workstream 1) ----
// NVS layout (Preferences):
//   namespace "rsh-boot": total u32, fails u32, lkgFails u32
//   namespace "rsh-cfg": cur0/cur1 + curLen u16 (current config),
//     lkg0/lkg1 + lkgLen (last-known-good), prv0/prv1 + prvLen
//     (COMMIT se pehle wali — rollback copy), pend u8 (pending confirm).
// Config 1024 B chunks mein is liye ke ek NVS blob value ki page limit
// par bharosa na karna pare. Blob khud kabhi stack par nahi — scratch
// buffers static (BSS) hain; loop-task stack 8 KB hota hai.
static rsh_boot_counter_t bootCounter;
static rsh_boothold_t bootHold;
static rsh_safe_mode_t safeMode;
static rsh_cfg_txn_t cfgTxn;
static uint32_t bootStartMs = 0;
static uint8_t fw34_scratch[RSH_CFG_MAX_TOTAL];
static uint8_t fw34_lkg[RSH_CFG_MAX_TOTAL];
static uint16_t fw34_lkg_len = 0;

// Ek config slot save karo: "<pfx>Len" u16 + "<pfx>0"/"<pfx>1" 1024 B chunks.
// Return 1 = tamam NVS writes OK, 0 = koi write fail (L3: Preferences
// putUShort/putBytes ka return pehle ignore hota tha — ab har write check
// hoti hai; fail par fw34_cfg_save_failed flag lagta hai + RSH_DEBUG par
// log. Behaviour waisa hi hai: in-RAM config chalti rehti hai, agle boot
// par LKG/factory fallback pehle se hai).
static uint8_t fw34_cfg_save_failed = 0;  // L3: aakhri slot save fail hua
static uint8_t fw34_slot_save(Preferences &p, const char *pfx,
                           const uint8_t *blob, uint16_t len) {
  uint8_t ok = 1;
  char key[16];
  snprintf(key, sizeof(key), "%sLen", pfx);
  if (p.putUShort(key, len) != sizeof(uint16_t)) ok = 0;
  uint16_t first = len > 1024 ? 1024 : len;
  snprintf(key, sizeof(key), "%s0", pfx);
  if (p.putBytes(key, blob, first) != first) ok = 0;
  if (len > 1024) {
    snprintf(key, sizeof(key), "%s1", pfx);
    if (p.putBytes(key, blob + 1024, len - 1024) != (size_t)(len - 1024)) ok = 0;
  }
  if (!ok) {
    fw34_cfg_save_failed = 1;
#if RSH_DEBUG
    Serial.println("RSH: NVS config slot save FAILED");
#endif
  }
  return ok;
}

// Ek config slot load karo. Return length, ya 0 (maujood nahi / kharab).
static uint16_t fw34_slot_load(Preferences &p, const char *pfx, uint8_t *out) {
  char key[16];
  snprintf(key, sizeof(key), "%sLen", pfx);
  uint16_t len = p.getUShort(key, 0);
  if (len == 0 || len > RSH_CFG_MAX_TOTAL) return 0;
  uint16_t first = len > 1024 ? 1024 : len;
  snprintf(key, sizeof(key), "%s0", pfx);
  if (p.getBytes(key, out, first) != first) return 0;
  if (len > 1024) {
    snprintf(key, sizeof(key), "%s1", pfx);
    if (p.getBytes(key, out + 1024, len - 1024) != (size_t)(len - 1024)) return 0;
  }
  return len;
}

// Boot par: counter barhao, config source chuno (current -> LKG ->
// factory), pending confirm ho to rollback karo. Reboot confirm nahi karta.
static void fw34_boot_setup() {
  bootStartMs = millis();
  pinMode(0, INPUT);  // BOOT button (GPIO0, active-LOW). Strapping par asar
                      // nahi — setup() boot strapping ke baad chalta hai.
  rsh_boot_counter_init(&bootCounter);
  rsh_boothold_init(&bootHold);
  rsh_safe_mode_init(&safeMode);
  rsh_cfg_txn_init(&cfgTxn);

  Preferences bootPref;
  bootPref.begin("rsh-boot", false);
  bootCounter.total_boots = bootPref.getUInt("total", 0);
  bootCounter.consecutive_failures = bootPref.getUInt("fails", 0);
  bootCounter.lkg_consecutive_failures = bootPref.getUInt("lkgFails", 0);
  rsh_boot_begin(&bootCounter);
  bootPref.putUInt("total", bootCounter.total_boots);
  bootPref.putUInt("fails", bootCounter.consecutive_failures);
  bootPref.putUInt("lkgFails", bootCounter.lkg_consecutive_failures);
  bootPref.end();

  Preferences cfgPref;
  cfgPref.begin("rsh-cfg", false);
  uint16_t lkglen = fw34_slot_load(cfgPref, "lkg", fw34_lkg);
  fw34_lkg_len = (lkglen > 0
                  && rsh_cfg_blob_validate(fw34_lkg, lkglen, NULL, NULL) == RSH_CTRL_OK)
                 ? lkglen : 0;
  rsh_boot_src_t src = rsh_boot_decide_source(&bootCounter, fw34_lkg_len > 0 ? 1 : 0);
  bootCounter.using_lkg = (src == RSH_BOOT_SRC_LAST_KNOWN_GOOD) ? 1 : 0;
  uint8_t loaded = 0;
  uint16_t blen = fw34_slot_load(cfgPref, "cur", fw34_scratch);
  if (src == RSH_BOOT_SRC_CURRENT && blen > 0)
    loaded = (rsh_cfg_txn_load_current(&cfgTxn, fw34_scratch, blen) == RSH_CTRL_OK);
  if (!loaded && fw34_lkg_len > 0)
    loaded = (rsh_cfg_txn_load_current(&cfgTxn, fw34_lkg, fw34_lkg_len) == RSH_CTRL_OK);
  if (!loaded) {  // factory fallback — config kabhi brick nahi karti
    blen = (uint16_t)rsh_cfg_factory_blob(fw34_scratch);
    loaded = (rsh_cfg_txn_load_current(&cfgTxn, fw34_scratch, blen) == RSH_CTRL_OK);
  }
  (void)loaded;
  if (cfgPref.getUChar("pend", 0)) {
    uint16_t plen = fw34_slot_load(cfgPref, "prv", cfgTxn.previous);
    if (plen > 0) {
      cfgTxn.previous_len = plen;
      cfgTxn.has_previous = 1;
    }
    if (rsh_cfg_boot_recover(&cfgTxn, 1))
      fw34_slot_save(cfgPref, "cur", cfgTxn.current, cfgTxn.current_len);
    cfgPref.putUChar("pend", 0);
  }
  // Phase G (C3): purana factory blob (Phase D/E/F — BUTTON slots 0..3)
  // naye slot scheme mein horn/encoders/LED/handbrake band kar deta
  // (un ke records purane blob mein hain hi nahi). Aisa blob mile to
  // naye factory blob par migrate karo aur NVS saaf karo (cur slot).
  // PC se aaya blob kabhi legacy nahi hota (PC builder BUTTON slot 3
  // kabhi nahi bhejta — rsh_pinmap_blob_is_legacy).
  if (rsh_pinmap_blob_is_legacy(cfgTxn.current, cfgTxn.current_len)) {
    uint16_t mlen = (uint16_t)rsh_cfg_factory_blob(fw34_scratch);
    if (rsh_cfg_txn_load_current(&cfgTxn, fw34_scratch, mlen) == RSH_CTRL_OK)
      fw34_slot_save(cfgPref, "cur", cfgTxn.current, cfgTxn.current_len);
  }
  cfgPref.end();
}

// Har loop mein: BOOT-hold sample karo, boot deadline + confirm deadline
// tick karo. Net down ho tab bhi chalta hai (is liye !netOk branch se pehle).
static void fw34_loop_tick(uint32_t nowMs) {
  rsh_hold_event_t ev = rsh_boothold_update(&bootHold, digitalRead(0) == LOW, nowMs);
  if (ev == RSH_HOLD_SAFE_MODE) {
    rsh_safe_mode_enter(&safeMode, 0, nowMs);
    rsh_cfg_set_safe_mode(&cfgTxn, 1);
    led_set(LedState::SafeMode);
  } else if (ev == RSH_HOLD_KEY_RESET_ARMED) {
    rsh_safe_mode_enter(&safeMode, 1, nowMs);
    rsh_cfg_set_safe_mode(&cfgTxn, 1);
    led_set(LedState::KeyResetArmed);
  }
  if (rsh_safe_mode_tick(&safeMode, nowMs)) ESP.restart();  // armed window khatam
  rsh_boot_tick_t bt = rsh_boot_tick(&bootCounter, netOk ? 1 : 0, nowMs, bootStartMs);
  if (bt == RSH_BOOT_SUCCESS || bt == RSH_BOOT_FAILED) {
    Preferences bootPref;
    if (bootPref.begin("rsh-boot", false)) {
      bootPref.putUInt("fails", bootCounter.consecutive_failures);
      bootPref.putUInt("lkgFails", bootCounter.lkg_consecutive_failures);
      bootPref.end();
    }
    if (bt == RSH_BOOT_SUCCESS) {
      // Kamyab boot: current config last-known-good ban gayi.
      Preferences cfgPref;
      if (cfgPref.begin("rsh-cfg", false)) {
        fw34_slot_save(cfgPref, "lkg", cfgTxn.current, cfgTxn.current_len);
        cfgPref.end();
      }
    }
  }
  if (rsh_cfg_tick(&cfgTxn, nowMs)) {
    // Confirm timeout -> rollback: restored current persist karo, pend clear.
    rsh_fw34_cfg_persist(0);
  }
  // Phase F WS5: config badli ho (SET_CFG COMMIT / confirm / revert /
  // boot recover) to clutch enable + shifter gate mode dobara lagao.
  // Phase G (C3): inputs pin table bhi yahin lagti hai (COMMIT par
  // inputs re-init, rollback par purani pins wapas — dono isi detector
  // se guzarte hain: COMMIT/REVERT/tick-rollback sab cfgTxn.current
  // badalte hain). Pehli loop tick par bhi chalta hai (boot config
  // apply — sentinel).
  // Detector POORA BLOB compare karta hai (memcmp) — version+length
  // kaafi nahi: PC har push par version 1 bhej sakta hai, aur do mukhtalif
  // maps ki length aksar barabar hoti hai (Phase G mein pakda gaya
  // latent bug: version+length sentinel same-length re-push par apply
  // chhor deta tha — pin badle hue, inputs purane rehte).
  static uint8_t lastAppliedBlob[RSH_CFG_MAX_TOTAL];
  static uint16_t lastAppliedLen = 0xFFFF;  // sentinel: pehli tick par apply ho
  if (cfgTxn.current_len != lastAppliedLen ||
      memcmp(cfgTxn.current, lastAppliedBlob, cfgTxn.current_len) != 0) {
    lastAppliedLen = cfgTxn.current_len;
    memcpy(lastAppliedBlob, cfgTxn.current, cfgTxn.current_len);
    rsh_modules_apply_config_blob(cfgTxn.current, cfgTxn.current_len);
    inputs_apply_config_blob(cfgTxn.current, cfgTxn.current_len);
  }
}

// ---- fw34_state.h accessors (FW-7/8 wiring: net.cpp / keystore.cpp) ----
// State yahin rehti hai; baahar se sirf in functions se pahuncha jata hai.
uint8_t rsh_fw34_safe_mode_active(void) {
  return rsh_safe_mode_blocks_config(&safeMode);
}

uint8_t rsh_fw34_key_reset_armed(void) {
  return (safeMode.active && safeMode.key_reset_armed) ? 1 : 0;
}

rsh_cfg_txn_t *rsh_fw34_cfg_txn(void) {
  return &cfgTxn;
}

// Config persist karo (NVS "rsh-cfg"): current hamesha, previous sirf jab
// has_previous ho (COMMIT ke baad), pend flag caller ke mutabiq.
void rsh_fw34_cfg_persist(uint8_t pending) {
  Preferences cfgPref;
  if (!cfgPref.begin("rsh-cfg", false)) return;
  fw34_slot_save(cfgPref, "cur", cfgTxn.current, cfgTxn.current_len);
  if (cfgTxn.has_previous)
    fw34_slot_save(cfgPref, "prv", cfgTxn.previous, cfgTxn.previous_len);
  cfgPref.putUChar("pend", pending ? 1 : 0);
  cfgPref.end();
}

// SEC-D-02: ArduinoOTA band hai (RSH_OTA 0) — doosra OTA rasta band.
// Function hamesha defined rehti hai: Arduino .ino auto-prototype banata
// hai, aur #if ke andar definition "declared 'static' but never defined"
// warning deti hai. RSH_OTA 0 par ye no-op hai.
static void ota_begin() {
#if RSH_OTA
  ArduinoOTA.setHostname(OTA_HOSTNAME);
  ArduinoOTA.setPassword(keystore_password());  // runtime per-device password (koi default nahi)
  ArduinoOTA.onStart([]() {
    otaActive = true;
    led_set(LedState::Updating);
  });
  ArduinoOTA.onEnd([]() { otaActive = false; });
  ArduinoOTA.onError([](ota_error_t) { otaActive = false; });
  ArduinoOTA.begin();
#endif
}

#if RSH_DEBUG
static uint32_t dbgWindowStartMs = 0;
static uint32_t dbgSentInWindow = 0;
static uint32_t dbgOverrunAtWindowStart = 0;
static uint16_t dbgSteerMin = 0xFFFF, dbgSteerMax = 0;
static uint64_t dbgSteerSum = 0, dbgSteerSqSum = 0;
static uint32_t dbgSteerN = 0;
#endif

// Round 2 W3 (FIX-4): status packet ka reset_reason asal wajah bataye —
// pehle hamesha 1 (power-on) likha jata tha, watchdog/software reboot
// bhi power-on dikhte thay. Mapping PROTOCOL-V2.1.md §1.2 ke mutabiq:
// 1 power-on, 2 brown-out, 3 watchdog, 4 software; baaqi 0 (reserved).
static uint8_t rsh_reset_reason_code() {
  switch (esp_reset_reason()) {
    case ESP_RST_POWERON: return 1;
    case ESP_RST_BROWNOUT: return 2;
    case ESP_RST_TASK_WDT:
    case ESP_RST_INT_WDT:
    case ESP_RST_WDT: return 3;
    case ESP_RST_SW:
    case ESP_RST_PANIC: return 4;
    default: return 0;
  }
}

void setup() {
#if RSH_WIRED_SERIAL
  serial_link_begin();  // Serial = wired transport (921600) — debug prints band
#elif RSH_DEBUG
  Serial.begin(115200);
#endif
  led_begin();
#if defined(RSH_SECURE_NVS)
  // Phase H (SEC-E-01/OPEN-3): secure build sirf encrypted flash par
  // chalta hai. Check fail ho to FAIL CLOSED — Error LED + halt (normal
  // boot nahi, plaintext NVS kabhi istemal nahi hota). Normal build mein
  // rsh_secure_boot_check() no-op true hai.
  if (!rsh_secure_boot_check()) {
    led_set(LedState::Error);
    for (;;) { led_update(millis()); delay(100); }
  }
#endif
  fw34_boot_setup();   // FW-3/FW-4: boot counter + config source + boot recover
  inputs_begin();
  // Phase G (C3): boot-from-NVS — NVS pin map (ya factory fallback)
  // inputs par lagao. inputs_begin sirf compiled defaults lagata hai;
  // boot config yahan apply hoti hai (loop detector bhi pehli tick par
  // yahi karta — dono ek hi apply hain, doosri dafa no-op hai).
  inputs_apply_config_blob(cfgTxn.current, cfgTxn.current_len);
  keyOk = keystore_begin();  // FW-8: RNG/NVS fail → AP up nahi (fail closed, §4.6)
  netOk = keyOk && net_begin();
  lastRetryMs = millis();
  govWindowStartMs = lastRetryMs;
  heapCheckMs = lastRetryMs;
  if (netOk) {
    ota_begin();  // RSH_OTA 0 par no-op (SEC-D-02)
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
  // F11/D15: poori loop iteration napo — cycleStartUs loop ke shuru mein
  // liya jata hai (OTA/WebServer/net_poll/LED samet), sirf inputs+send nahi.
  uint32_t loopStartUs = micros();
  uint32_t nowMs = millis();

  fw34_loop_tick(nowMs);   // FW-3/FW-4: BOOT-hold + boot/confirm deadlines

  // Fail-safe (audit fix): WiFi start fail ho to tick processing bilkul band —
  // sirf error LED aur har 5 s mein dobara koshish. Pehle error state foran
  // aam blink se overwrite ho jata tha aur retry kabhi nahi hota tha.
  if (!netOk) {
    led_update(nowMs);
    net_led_tick(nowMs);  // Phase H WS2: chalta LED test Wi-Fi down par bhi khatam ho
    if (nowMs - lastRetryMs >= 5000) {
      lastRetryMs = nowMs;
      if (!keyOk) keyOk = keystore_begin();
      netOk = keyOk && net_begin();
      if (netOk) {
        ota_begin();  // RSH_OTA 0 par no-op (SEC-D-02)
#if RSH_WEB_OTA
        webota_begin();
#endif
        led_set(LedState::ApUpWaitingPc);
        nextTickUs = micros() + SEND_PERIOD_US;
      }
    }
    return;
  }

  // FW-10 / D17 heap policy — OTA ke doran bhi chalta hai (F10 fix):
  // ye check OTA/WebOTA early return se PEHLE hai, is liye update ke
  // doran bhi fragmentation / heap drop par restart hota hai. Pehle
  // heap check early return ke baad tha aur OTA mein kabhi nahi chalta tha.
  if (nowMs - heapCheckMs >= 10000) {
    heapCheckMs = nowMs;
    if (heap_policy_update(&heapPolicy, ESP.getFreeHeap(),
                           ESP.getMaxAllocHeap(), nowMs))
      ESP.restart();
  }

#if RSH_OTA
  ArduinoOTA.handle();
  if (otaActive) {          // update ke doran sampling band — sirf LED
    led_update(nowMs);
    net_led_tick(nowMs);  // Phase H WS2: OTA ke doran bhi pattern tick kare
    return;
  }
#endif

#if RSH_WEB_OTA
  webota_handle();
  if (webota_active()) {    // web upload ke doran bhi sampling band
    led_update(nowMs);
    net_led_tick(nowMs);  // Phase H WS2: WebOTA ke doran bhi pattern tick kare
    return;
  }
#endif

  net_poll();
  net_poll_ctrl();              // FW-8: port 4212 — HMAC control + ping
#if RSH_WIRED_SERIAL
  serial_link_poll(nowMs);      // Phase G: USB-serial frames (HELO/control/ping)
#endif
  net_maybe_rescan(nowMs);      // FW-7: degraded link par rescan (60 s+ waqfa)
  led_update(nowMs);
  net_led_tick(nowMs);  // Phase H WS2: LED strip driver tick (200 Hz loop)

  // SEC-D-01: unauthenticated RSTX restart KHATAM — restart sirf
  // HMAC-signed REBOOT (0x09) se hota hai (net_poll_ctrl → dispatcher).

  // Adaptive sampling: har 2 s window ke overruns governor ko do
  if (nowMs - govWindowStartMs >= 2000) {
    inputs_governor().update(overrunCount - govOverrunAtStart);
    govOverrunAtStart = overrunCount;
    govWindowStartMs = nowMs;
  }

  // (Heap check upar ho gaya — FW-10, OTA early return se pehle.)

  // Governor period: level 3 par 100 pps fallback (D16 — 10 ms period).
  uint32_t periodUs = inputs_governor().sendPeriodUs();
  // Wrap-safe scheduler: micros() ~71.6 min mein wrap hota hai,
  // signed difference wala tareeqa hi sahi hai (research-verified).
  uint32_t nowUs = micros();
  int32_t untilTick = (int32_t)(nextTickUs - nowUs);
  if (untilTick > 0) {
    // Deadline door ho to core ko spin mein garam mat karo (audit fix)
    if (untilTick > 1500) delay(1);
    return;
  }
  nextTickUs += periodUs;
  // Agar bohat peeche reh gaye (lamba block), to burst karke catch-up mat karo —
  // agla tick ab se gino, purani readings ka koi faida nahi. Overrun yahan
  // sirf tab gino jab pichle cycle ka kaam khud lamba na raha ho
  // (warna ek hi stall do dafa gin liya jata tha — audit fix).
  if ((int32_t)(micros() - nextTickUs) >= (int32_t)periodUs) {
    // Kitne ticks skip hue (seqGap, D5): peeche rehne ka waqt / period.
    uint32_t behindUs = (uint32_t)(micros() - nextTickUs);
    seqGap = (uint16_t)(seqGap + (uint16_t)(behindUs / periodUs));
    nextTickUs = micros() + periodUs;
    if (!prevCycleOverran) overrunCount++;
  }

  InputSample s = inputs_read();

  // F4/D13: timestamp ADC read ke BAAD (midpoint) — pehle cycle shuru
  // hone par liya jata tha, jis se packet ka waqt asal reading se pehle ka
  // hota tha aur PC ki latency measurement ghalat hoti thi.
  uint32_t tsUs = micros();

  // Koi bhi transport (Wi-Fi ya serial) active ho to PC connected hai (LED).
  bool active = net_pc_active(nowMs);
#if RSH_WIRED_SERIAL
  if (net_serial_pc_active(nowMs)) active = true;
#endif
  // FW-3: safe mode / key-reset-armed ki LED ko normal state overwrite na kare.
  if (!rsh_safe_mode_blocks_config(&safeMode))
    led_set(active ? LedState::PcConnected : LedState::ApUpWaitingPc);
  // SEC-E-14: stream emission sirf authenticated session ko — HELO
  // (unauthenticated bootstrap) se stream nahi khulta; pehle ek
  // HMAC-verified control command (aam tor par GET_CAPS) lazmi hai.
  // Status packet (1 Hz) is se alag hai — neeche waisa hi rehta hai.
  // Phase G: DUAL transport — Wi-Fi aur serial ke alag emit faisle hain
  // (alag peer, alag auth). Packet ek dafa banta hai, dono transports ko
  // jata hai (har transport ko sirf us ka version: v2 peer ko v2 packet,
  // legacy v1 peer ko v1 packet). Aam tor par ek waqt mein ek hi peer
  // hota hai; dono hon to seq ek hi rehta hai (ek sequence space).
  rsh_emit_kind_t emitKind = rsh_stream_emit_kind(
      net_pc_active(nowMs) ? 1 : 0, net_pc_is_v2(nowMs) ? 1 : 0,
      net_session_authenticated(nowMs) ? 1 : 0);
  rsh_emit_kind_t serialEmitKind = RSH_EMIT_NONE;
#if RSH_WIRED_SERIAL
  serialEmitKind = rsh_stream_emit_kind(
      net_serial_pc_active(nowMs) ? 1 : 0, net_serial_pc_is_v2(nowMs) ? 1 : 0,
      net_serial_session_authenticated(nowMs) ? 1 : 0);
#endif
  if (emitKind != RSH_EMIT_NONE || serialEmitKind != RSH_EMIT_NONE) {
    // Phase E WS5 — v2.1 stream emission (§1.1): v2 peer ko v2 stream
    // packet (axes mV, cumulative encoders), legacy v1 peer ko v1 packet.
    if (emitKind == RSH_EMIT_V2 || serialEmitKind == RSH_EMIT_V2) {
      uint8_t flags2 = 0;
      if (inputs_axis_invalid()) flags2 |= RSH_V2_FLAG_AXIS_INVALID;  // F2
      if (seqGap != lastSentSeqGap) flags2 |= RSH_V2_FLAG_TICK_SKIPPED;  // D5
      uint8_t pkt2[36];  // max v2 stream: 20 + 2*5 axes + 2*2 enc + 2 CRC
                         // (Phase F WS5: 5 axes — clutch slot 3 + handbrake
                         // slot 4; pehle 34 B the — 4 axes ke liye kaafi
                         // the, 5 axes overflow karte the — buffer bug fix)
      size_t n2 = rsh_build_v2_stream_packet(
          pkt2, sizeof(pkt2), seq, tsUs, flags2, s.steering,
          inputs_steering_is_mv() ? 1 : 0, s.accelAnalog, s.brakeAnalog,
          s.handbrake, s.enc, rsh_modules_encoders_enabled() ? 2 : 0,
          s.buttons, rsh_modules_enabled(), s.clutch, s.shifterGear);
      if (n2 > 0) {
        if (emitKind == RSH_EMIT_V2 && net_send(pkt2, n2)) {
#if RSH_DEBUG
          dbgSentInWindow++;
#endif
        }
#if RSH_WIRED_SERIAL
        if (serialEmitKind == RSH_EMIT_V2)
          serial_link_send_frame(RSH_SERIAL_KIND_STREAM, pkt2, (uint16_t)n2);
#endif
      }
      lastSentSeqGap = seqGap;
    }
    if (emitKind == RSH_EMIT_V1 || serialEmitKind == RSH_EMIT_V1) {
      uint8_t pkt[RSH_PACKET_SIZE];
      uint8_t flags = inputs_steering_is_mv() ? RSH_FLAG_STEER_MV : 0;
      if (inputs_axis_invalid()) flags |= RSH_FLAG_AXIS_INVALID;  // F2
      rsh_build_packet(pkt, seq, tsUs, s.steering, s.accelAnalog,
                        s.brakeAnalog, s.buttons, flags);
      if (emitKind == RSH_EMIT_V1 && net_send(pkt, sizeof(pkt))) {
#if RSH_DEBUG
        dbgSentInWindow++;
#endif
      }
#if RSH_WIRED_SERIAL
      if (serialEmitKind == RSH_EMIT_V1)
        serial_link_send_frame(RSH_SERIAL_KIND_STREAM, pkt, sizeof(pkt));
#endif
    }
    seq++;   // uint16 khud wrap hota hai — PC wrap-safe tareeqe se parhta hai
  }

  // Status packet v2.1 (1 Hz, D5/F3): txFail + seqGap + RSSI + heap +
  // governor. Sirf v2 peer ko (legacy v1 app 34 B status nahi samajhti).
  // Phase G: Wi-Fi v2 peer ko UDP par, serial v2 peer ko serial frame par.
  bool udpStatusPeer = net_pc_active(nowMs) && net_pc_is_v2(nowMs);
  bool serialStatusPeer = false;
#if RSH_WIRED_SERIAL
  serialStatusPeer = net_serial_pc_active(nowMs) && net_serial_pc_is_v2(nowMs);
#endif
  if ((udpStatusPeer || serialStatusPeer) && (uint32_t)(nowMs - lastStatusMs) >= 1000) {
    lastStatusMs = nowMs;
    rsh_v2_status_t st;
    memset(&st, 0, sizeof(st));
    st.seq = statusSeq++;
    st.uptime_ms = nowMs;
    st.rssi_dbm = net_rssi_dbm();
    st.battery_pct = 255;  // no battery module
    st.supply_mv = 0;      // not measured
    st.heap = ESP.getFreeHeap();
    st.overruns = (uint16_t)overrunCount;
    st.max_cycle_us = maxCycleUs > 0xFFFF ? 0xFFFF : (uint16_t)maxCycleUs;
    st.governor = inputs_governor().level;
    st.reset_reason = rsh_reset_reason_code();  // FIX-4: asal wajah (pehle hamesha 1)
    st.cfg_crc = rsh_crc32(cfgTxn.current, cfgTxn.current_len);  // Phase G (C3):
    st.cfg_ver = cfgTxn.cfg_version;  // live config ki pehchan — PC push ke
                                      // baad board-apply status packet se nazar aata hai
    st.seq_gap = seqGap;
    st.tx_fail = (uint16_t)net_tx_fail_count();  // u16 wrap allowed (D5)
    uint8_t sbuf[RSH_V2_STATUS_BASE_SIZE];
    size_t slen = rsh_v2_build_status(sbuf, &st);
    if (slen > 0) {
      if (udpStatusPeer) net_send(sbuf, slen);
#if RSH_WIRED_SERIAL
      if (serialStatusPeer)
        serial_link_send_frame(RSH_SERIAL_KIND_STATUS, sbuf, (uint16_t)slen);
#endif
    }
  }

  // F11: cycleUs poori iteration hai (loopStartUs loop ke shuru se) —
  // net_poll + OTA/WebServer + LED + inputs + send sab shamil.
  uint32_t cycleUs = micros() - loopStartUs;
  if (cycleUs > maxCycleUs) maxCycleUs = cycleUs;
  prevCycleOverran = cycleUs > periodUs;
  net_set_stream_busy(prevCycleOverran);  // FW-8: ping drop-when-busy (§9.3)
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
