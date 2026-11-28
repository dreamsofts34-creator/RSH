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
#include "keystore.h"      // v2.2: user-set Wi-Fi password store (setup mein net se pehle)
#include "secure_nvs.h"     // Phase H (SEC-E-01/OPEN-3): secure-build posture check
#include "safemode.h"      // FW-3 (Workstream 1): safe mode + boot counter
#include "config_transaction.h"  // FW-4 (Workstream 1): config transaction
#include "fw34_state.h"    // FW-7/8 wiring: net.cpp/keystore.cpp se accessors
#include <Preferences.h>   // FW-3/FW-4 NVS glue (boot counter + config slots)
#include <esp_system.h>  // Round 2 W3 (FIX-4): esp_reset_reason() (status packet)
#include <esp_task_wdt.h>  // DS-64: task watchdog subscribe/feed (loop + sample task)
#include <esp_ota_ops.h> // Round 2C: firmware rollback — doosri OTA slot boot partition
#include "net.h"
#include "webota.h"
#include "status_led.h"
#include "serial_link.h"  // Phase G: wired USB-serial transport (D26/OPEN-20)
#include "sample_task.h"  // F-06: dedicated sampling task mailbox (pure)
#include "diag_log.h"    // F-10: diagnostics ring log + GET_DIAG

// Fail closed: RSH_DEBUG prints aur wired transport ek hi Serial istemal
// karte hain — dono ek saath on hon to prints "RSHS" frame stream kharab
// karte hain. Ek chuno: debug log (RSH_WIRED_SERIAL 0) ya transport (1).
#if RSH_DEBUG && RSH_WIRED_SERIAL
#error "RSH_DEBUG 1 aur RSH_WIRED_SERIAL 1 ek saath nahi — Serial ek hi kaam kar sakta hai (debug log ya transport)."
#endif

static uint16_t seq = 0;
static uint16_t statusSeq = 0;   // status packet sequence (data seq se alag)
static uint32_t lastStatusMs = 0;
static volatile uint16_t seqGap = 0;      // D5: boot se skip hue stream ticks
static uint16_t lastSentSeqGap = 0;  // v2 tick-skipped flag ke liye (Phase E WS5)
static bool keyOk = false;       // v2.2: keystore (password/obfkey) tayyar hai?
static uint32_t nextTickUs = 0;
static volatile uint32_t overrunCount = 0;     // cycles jin ka kaam period se zyada lamba hua
static volatile uint32_t maxCycleUs = 0;       // sab se lamba cycle kaam (board par timing test ke liye)
static bool netOk = false;            // latched: fail ho to loop sirf retry karta hai
static uint32_t lastRetryMs = 0;
static volatile bool prevCycleOverran = false;
static bool otaActive = false;
#if RSH_SAMPLE_TASK
static RshSampleMailbox sampleMailbox;  // F-06: sampling task -> loop handoff
static TaskHandle_t sampleTaskHandle = NULL;
static bool sampleTaskActive = false;   // task chal rahi hai (warna legacy loop sampling)
#endif
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
  // F-02: pichle boot ka source (persisted "lkg") — crash record ko pata
  // chale ke pichla boot kis source par tha (current/LKG), taake failure
  // sahi counter mein jaye. decide_source() neeche is boot ka source set
  // karke dobara persist karta hai.
  bootCounter.using_lkg = bootPref.getUChar("lkg", 0);
  rsh_boot_begin(&bootCounter);
  // F-02: PANIC / TASK_WDT / INT_WDT / WDT reset = pichle boot ki failure
  // (post-AP-up panic loop bhi accumulate ho — pehle har AP-up SUCCESS
  // counter zero kar deta tha). Power-on / brown-out / software reboot
  // is mein nahi aate (wo crash nahi hain).
  {
    esp_reset_reason_t rr = esp_reset_reason();
    if (rr == ESP_RST_PANIC || rr == ESP_RST_TASK_WDT ||
        rr == ESP_RST_INT_WDT || rr == ESP_RST_WDT) {
      rsh_boot_record_crash(&bootCounter);
    }
  }
  bootPref.putUInt("total", bootCounter.total_boots);
  bootPref.putUInt("fails", bootCounter.consecutive_failures);
  bootPref.putUInt("lkgFails", bootCounter.lkg_consecutive_failures);
  // Round 2C — firmware rollback: config fallback khatam (current 3 fail
  // + LKG 3 fail) hone ke bawajood boot kamyab na ho to is slot ki image
  // kharab hai (crash/panic loop) — doosri OTA slot par wapas jao.
  // "fwRb" flag ping-pong rokta hai: rollback sirf ek dafa hota hai jab
  // tak kamyab boot use saaf na kare (fw34_loop_tick, RSH_BOOT_SUCCESS).
  // Rollback mumkin na ho (doosri slot nahi / set_boot fail) to normal
  // boot jari rakho — aaj jaisa behaviour; USB reflash hi recovery hai.
  if (rsh_boot_firmware_rollback_due(&bootCounter) &&
      bootPref.getUChar("fwRb", 0) == 0) {
    const esp_partition_t *running = esp_ota_get_running_partition();
    const esp_partition_t *other =
        running ? esp_ota_get_next_update_partition(running) : NULL;
    if (other != NULL && esp_ota_set_boot_partition(other) == ESP_OK) {
      bootPref.putUChar("fwRb", 1);
      bootPref.end();
      rsh_diag_record_global(RSH_DIAG_FW_ROLLBACK, 0, millis());
#if RSH_DEBUG
      Serial.println("RSH: firmware rollback — doosri OTA slot par reboot");
#endif
      ESP.restart();
    }
#if RSH_DEBUG
    else {
      Serial.println("RSH: firmware rollback mumkin nahi (doosri slot nahi)");
    }
#endif
  }
  bootPref.end();

  Preferences cfgPref;
  cfgPref.begin("rsh-cfg", false);
  uint16_t lkglen = fw34_slot_load(cfgPref, "lkg", fw34_lkg);
  fw34_lkg_len = (lkglen > 0
                  && rsh_cfg_blob_validate(fw34_lkg, lkglen, NULL, NULL) == RSH_CTRL_OK)
                 ? lkglen : 0;
  rsh_boot_src_t src = rsh_boot_decide_source(&bootCounter, fw34_lkg_len > 0 ? 1 : 0);
  bootCounter.using_lkg = (src == RSH_BOOT_SRC_LAST_KNOWN_GOOD) ? 1 : 0;
  // F-02: is boot ka source agle boot ke crash record ke liye persist karo.
  {
    Preferences bootPref2;
    if (bootPref2.begin("rsh-boot", false)) {
      bootPref2.putUChar("lkg", bootCounter.using_lkg);
      bootPref2.end();
    }
  }
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
    if (rsh_cfg_boot_recover(&cfgTxn, 1)) {
      fw34_slot_save(cfgPref, "cur", cfgTxn.current, cfgTxn.current_len);
    } else {
      // R3-FW-10: pend=1 lekin `prv` maujood/kharab nahi — invariant
      // (config_transaction.h) ke mutabiq caller factory fallback le:
      // unconfirmed current config live NAHI reh sakti ("reboot confirm
      // nahi karta"). Diag event bhi record karo taake nazar aaye.
      uint16_t flen = (uint16_t)rsh_cfg_factory_blob(fw34_scratch);
      if (rsh_cfg_txn_load_current(&cfgTxn, fw34_scratch, flen) == RSH_CTRL_OK)
        fw34_slot_save(cfgPref, "cur", cfgTxn.current, cfgTxn.current_len);
      rsh_diag_record_global(RSH_DIAG_CFG_ROLLBACK, 0, millis());
    }
    cfgPref.putUChar("pend", 0);
  }
  // P2-5: purana factory blob (Phase D/E/F — BUTTON slot 3) ab validate
  // nahi hota (BUTTON slot > 2 reject) — load upar hi LKG/factory
  // fallback leta hai (wahi end state jo purani legacy-migration deti
  // thi), is liye alag migration block ki zaroorat nahi.
  cfgPref.end();
}

// D7 (R2-47 krExp pattern): password-cleared diag event RAM ring mein
// record karne ke baad ESP.restart() use mita deta tha — event kabhi
// diagnostics mein nazar nahi aata tha. Is liye reboot se PEHLE ek
// pending-flag NVS ("rsh-boot" namespace, key "pwClr") mein likho; agle
// boot par setup() flag dekh kar event dobara ring mein record karta hai
// aur flag saaf karta hai. Sirf ye ek event persist hota hai — poora ring
// NVS mein nahi (flash wear + boot-time cost); flag 1 byte hai.
static void rsh_password_cleared_persist_() {
  Preferences pref;
  if (pref.begin("rsh-boot", false)) {
    pref.putUChar("pwClr", 1);
    pref.end();
  }
  // begin fail ho to bhi restart hota hai — event is boot par kho jata
  // hai (purana behaviour), lekin board kabhi atak ta nahi.
}

// Pichle boot ka password-clear (agar flag set ho) is boot ke ring mein
// sab se pehle record karo (BOOT event se pehle — oldest-first order mein
// clear reboot se pehle dikhe), phir flag saaf karo.
static void rsh_password_cleared_replay_() {
  Preferences pref;
  if (!pref.begin("rsh-boot", false)) return;
  uint8_t pending = pref.getUChar("pwClr", 0);
  if (pending) {
    rsh_diag_record(rsh_diag_log(), RSH_DIAG_PASSWORD_CLEARED, 0, millis());
    pref.putUChar("pwClr", 0);
  }
  pref.end();
}

// Har loop mein: BOOT-hold sample karo, boot deadline + confirm deadline
// tick karo. Net down ho tab bhi chalta hai (is liye !netOk branch se pehle).
static void fw34_loop_tick(uint32_t nowMs) {
  rsh_hold_event_t ev = rsh_boothold_update(&bootHold, digitalRead(0) == LOW, nowMs);
  if (ev == RSH_HOLD_SAFE_MODE) {
    rsh_safe_mode_enter(&safeMode, nowMs);
    rsh_cfg_set_safe_mode(&cfgTxn, 1);
    // F-06 (polish round): safe mode FACTORY pin map lagata hai — pehle
    // sirf future config changes block hote thay, live map wahi rehta
    // tha (bad pin assignment se recovery ka waada poora nahi hota tha).
    // Factory blob sirf RAM mein cfgTxn.current mein aata hai (persist
    // NAHI hota — reboot par user ki config wapas aati hai) aur
    // cfg_dirty apply (modules + inputs + LED) isay live karta hai.
    // Safe-mode gate stage/commit ko rokta rehta hai, is liye apply ke
    // baad bhi koi config change nahi ho sakta.
    {
      uint16_t flen = (uint16_t)rsh_cfg_factory_blob(fw34_scratch);
      if (flen > 0 && flen <= sizeof(cfgTxn.current)) {
        memcpy(cfgTxn.current, fw34_scratch, flen);
        cfgTxn.current_len = flen;
        // cfg_version blob header (offset 6) se — warna status packet
        // factory blob ka CRC purani version ke saath bhejta.
        cfgTxn.cfg_version = rsh_get_u16(fw34_scratch + 6);
        cfgTxn.cfg_dirty = 1;
      }
    }
    led_set(LedState::SafeMode);
  } else if (ev == RSH_HOLD_KEY_RESET_CLEAR) {
    // D7: BOOT >= 10 s hold — password seedha mitao + reboot -> open AP.
    // Clear threshold par hota hai (koi armed window nahi). Clear fail ho
    // to reboot NAHI (fail closed — purana password rehta hai; user hold
    // dobara kare). Kamyabi par NVS flag likho taake diag event agle boot
    // par replay ho (ring reboot se mit jata hai).
    if (keystore_clear_password()) {
      rsh_password_cleared_persist_();
      ESP.restart();
    }
  }
  rsh_boot_tick_t bt = rsh_boot_tick(&bootCounter, netOk ? 1 : 0, nowMs, bootStartMs);
  if (bt == RSH_BOOT_SUCCESS || bt == RSH_BOOT_FAILED) {
    Preferences bootPref;
    if (bootPref.begin("rsh-boot", false)) {
      bootPref.putUInt("fails", bootCounter.consecutive_failures);
      bootPref.putUInt("lkgFails", bootCounter.lkg_consecutive_failures);
      // Round 2C: kamyab boot — firmware-rollback flag saaf, taake
      // mustaqbil mein kharab firmware par rollback dobara mumkin ho.
      if (bt == RSH_BOOT_SUCCESS) bootPref.putUChar("fwRb", 0);
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
    rsh_diag_record_global(RSH_DIAG_CFG_ROLLBACK, 0, nowMs);  // F-10
    rsh_fw34_cfg_persist(0);
  }
  // F-06: config apply DIRTY FLAG se — pehle har loop mein poora blob
  // memcmp hota tha (<= 2 KB). Ab sirf mutation points (load/commit/
  // revert/boot-recover/confirm-rollback) flag lagate hain; yahan ek
  // dafa apply hota hai (modules + inputs + persistent LED), phir flag
  // saaf. Apply functions khud validate karte hain (fail closed).
  if (cfgTxn.cfg_dirty) {
    cfgTxn.cfg_dirty = 0;
    rsh_modules_apply_config_blob(cfgTxn.current, cfgTxn.current_len);
    inputs_apply_config_blob(cfgTxn.current, cfgTxn.current_len);
    net_led_apply_persistent(cfgTxn.current, cfgTxn.current_len);
  }
}

// ---- fw34_state.h accessors (FW-7/8 wiring: net.cpp / keystore.cpp) ----
// State yahin rehti hai; baahar se sirf in functions se pahuncha jata hai.
uint8_t rsh_fw34_safe_mode_active(void) {
  return rsh_safe_mode_blocks_config(&safeMode);
}

rsh_cfg_txn_t *rsh_fw34_cfg_txn(void) {
  return &cfgTxn;
}

// Config persist karo (NVS "rsh-cfg"): previous sirf jab has_previous
// ho (COMMIT ke baad), pend flag caller ke mutabiq, current aakhir mein.
// P2-2 atomicity — order + verify:
//   prv -> pend -> cur, phir cur read-back verify.
//   * pend=1 se PEHLE prv durable hota hai (invariant: pend=1 ho to
//     prv maujood hai) — beech mein power loss ho to boot ya to purani
//     confirmed config dekhta hai (pend=0), ya prv se rollback karta
//     hai (pend=1). Kisi bhi waqt unconfirmed config confirmed nahi banti.
//   * cur aakhir mein likha jata hai — torn cur + pend=1 ho to boot
//     prv se recover karta hai (LKG/factory fallback bhi hai).
//   * cur pehle se RAM jaisa ho (CONFIRM — bytes waisi ki waisi) to
//     dobara NAHI likha jata (torn-write ka koi faida nahi).
//   * Koi write ya verify fail ho to FAIL LOUDLY: flag + diag event
//     (GET_DIAG, RSH_DIAG_CFG_SAVE_FAIL) + RSH_DEBUG log — pehle sirf
//     unread flag lagta tha. In-RAM config chalti rehti hai; agla boot
//     LKG/factory se recover karta hai.
void rsh_fw34_cfg_persist(uint8_t pending) {
  Preferences cfgPref;
  if (!cfgPref.begin("rsh-cfg", false)) {
    fw34_cfg_save_failed = 1;
    rsh_diag_record_global(RSH_DIAG_CFG_SAVE_FAIL, 0, millis());
#if RSH_DEBUG
    Serial.println("RSH: NVS config persist FAILED (begin)");
#endif
    return;
  }
  uint8_t ok = 1;
  if (cfgTxn.has_previous &&
      !fw34_slot_save(cfgPref, "prv", cfgTxn.previous, cfgTxn.previous_len))
    ok = 0;
  if (cfgPref.putUChar("pend", pending ? 1 : 0) != 1) {
    fw34_cfg_save_failed = 1;
    ok = 0;
  }
  // Cur sirf tab likho jab NVS wala RAM se mukhtalif ho (CONFIRM par
  // bytes waisi ki waisi hoti hain — dobara likhna sirf torn-write risk hai).
  // Ye read-back skip case mein hi verify hai.
  uint16_t cur_len = fw34_slot_load(cfgPref, "cur", fw34_scratch);
  if (cur_len != cfgTxn.current_len ||
      memcmp(fw34_scratch, cfgTxn.current, cfgTxn.current_len) != 0) {
    if (!fw34_slot_save(cfgPref, "cur", cfgTxn.current, cfgTxn.current_len))
      ok = 0;
    // Verify: cur read-back RAM se mile (torn write pakro).
    uint16_t vlen = fw34_slot_load(cfgPref, "cur", fw34_scratch);
    if (vlen != cfgTxn.current_len ||
        memcmp(fw34_scratch, cfgTxn.current, cfgTxn.current_len) != 0) {
      fw34_cfg_save_failed = 1;
      ok = 0;
    }
  }
  if (!ok) {
    rsh_diag_record_global(RSH_DIAG_CFG_SAVE_FAIL, 0, millis());
#if RSH_DEBUG
    Serial.println("RSH: NVS config persist FAILED (write/verify)");
#endif
  }
  cfgPref.end();
}

// SEC-D-02: ArduinoOTA band hai (RSH_OTA 0) — doosra OTA rasta band.
// Function hamesha defined rehti hai: Arduino .ino auto-prototype banata
// hai, aur #if ke andar definition "declared 'static' but never defined"
// warning deti hai. RSH_OTA 0 par ye no-op hai.
static void ota_begin() {
#if RSH_OTA
  ArduinoOTA.setHostname(OTA_HOSTNAME);
  ArduinoOTA.setPassword(keystore_password());  // runtime user-set password (koi default nahi)
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

// ---- F-06: sampling helpers (dedicated task + legacy loop dono yahi istemal karte hain) ----

// Link state ke mutabiq status LED set karo. Safe mode ki LED ko overwrite
// nahi karta (fw34_loop_tick ne set ki hai). Warna PcConnected / ApUpWaitingPc.
static void led_set_for_link_(uint8_t active, uint32_t nowMs) {
  (void)nowMs;
  if (rsh_safe_mode_blocks_config(&safeMode)) return;
  led_set(active ? LedState::PcConnected : LedState::ApUpWaitingPc);
}

// Ek sample cycle: inputs parho, horn-test OR karo (D-5), emit faisla
// karo (SEC-E-14: sirf active + authenticated session), packet(s) banao.
// frame mein bharo (v2len/v1len 0 = is cycle mein koi packet nahi).
// seq tabhi barhta hai jab packet bana ho (purana behaviour).
// sample_out NULL na ho to aakhri InputSample bhi deta hai (RSH_DEBUG
// stats ke liye — legacy loop istemal karta hai, task NULL deta hai).
// Return 1 = frame mein packet hai, 0 = koi packet nahi (inputs_read
// phir bhi hua — debouncer/health/mV probe zinda rehte hain).
static uint8_t sample_once_(RshSampleMailbox *frame, uint32_t nowMs,
                            InputSample *sample_out) {
  if (!frame) return 0;
  memset(frame, 0, sizeof(*frame));
  InputSample s = inputs_read();
  if (sample_out) *sample_out = s;
  // F4/D13: timestamp ADC read ke BAAD (midpoint).
  uint32_t tsUs = micros();
  // D-5: HORN_TEST active ho to horn bit set karo (synthetic press).
  if (net_horn_test_active(nowMs)) s.buttons |= RSH_BTN_HORN;

  // D3: stream emission sirf GET_CAPS-OK session ko — HELO
  // (unauthenticated bootstrap) se stream nahi khulta; pehle is session
  // mein GET_CAPS OK lazmi hai. Status packet (1 Hz) is se alag hai —
  // neeche waisa hi rehta hai.
  // Phase G: DUAL transport — Wi-Fi aur serial ke alag emit faisle hain
  // (alag peer, alag GET_CAPS-OK flag). Packet ek dafa banta hai, dono
  // transports ko jata hai (har transport ko sirf us ka version).
  rsh_emit_kind_t emitKind = rsh_stream_emit_kind(
      net_pc_active(nowMs) ? 1 : 0, net_pc_is_v2(nowMs) ? 1 : 0,
      net_caps_ok(nowMs) ? 1 : 0);
  rsh_emit_kind_t serialEmitKind = RSH_EMIT_NONE;
#if RSH_WIRED_SERIAL
  serialEmitKind = rsh_stream_emit_kind(
      net_serial_pc_active(nowMs) ? 1 : 0, net_serial_pc_is_v2(nowMs) ? 1 : 0,
      net_serial_caps_ok(nowMs) ? 1 : 0);
#endif
  frame->emit_wifi = (uint8_t)emitKind;
  frame->emit_serial = (uint8_t)serialEmitKind;
  if (emitKind == RSH_EMIT_NONE && serialEmitKind == RSH_EMIT_NONE) return 0;

  // Phase E WS5 — v2.1 stream emission (§1.1): v2 peer ko v2 stream
  // packet (axes mV, encoders hamesha 0 — v2.0.19), legacy v1 peer ko v1 packet.
  if (emitKind == RSH_EMIT_V2 || serialEmitKind == RSH_EMIT_V2) {
    uint8_t flags2 = 0;
    if (inputs_axis_invalid()) flags2 |= RSH_V2_FLAG_AXIS_INVALID;  // F2
    if (seqGap != lastSentSeqGap) flags2 |= RSH_V2_FLAG_TICK_SKIPPED;  // D5
    // P1-1: har slot ka mV bit (I2C axes pehle se mV hain — dobara
    // raw->mV nahi lagta; native axes raw counts hain).
    uint8_t axes_mv = 0;
    for (uint8_t ax = 0; ax < 5; ax++)
      if (inputs_axis_is_mv(ax)) axes_mv |= (uint8_t)(1u << ax);
    size_t n2 = rsh_build_v2_stream_packet(
        frame->v2pkt, sizeof(frame->v2pkt), seq, tsUs, flags2, s.steering,
        axes_mv, s.accelAnalog, s.brakeAnalog,
        s.handbrake, s.enc, 2,  // n_enc hamesha 2, enc hamesha {0,0} (v2.0.19)
        s.buttons, rsh_modules_enabled(), s.clutch);
    if (n2 > 0) frame->v2len = (uint8_t)n2;
    lastSentSeqGap = seqGap;
  }
  if (emitKind == RSH_EMIT_V1 || serialEmitKind == RSH_EMIT_V1) {
    uint8_t flags = inputs_steering_is_mv() ? RSH_FLAG_STEER_MV : 0;
    if (inputs_axis_invalid()) flags |= RSH_FLAG_AXIS_INVALID;  // F2
    // F-05 (polish round): v1 packet mein accel/brake RAW counts hote
    // hain (sirf steering ka RSH_FLAG_STEER_MV flag hai). I2C axis ki
    // value pehle se mV mein hai — use raw mein wapas convert karo
    // (mV * 4095 / 3300), warna v1 peer dobara convert karke 1650 mV ko
    // 1330 mV parhta hai (v2.0.11 wala bug, v1 path par abhi bhi live tha).
    // Steering mV flag se cover hai (v1 peer flag dekh kar mV samajhta hai).
    uint16_t v1Accel = s.accelAnalog, v1Brake = s.brakeAnalog;
    if (inputs_axis_is_mv(RSH_PINMAP_AXIS_ACCEL))
      v1Accel = (uint16_t)(((uint32_t)s.accelAnalog * 4095u) / 3300u);
    if (inputs_axis_is_mv(RSH_PINMAP_AXIS_BRAKE))
      v1Brake = (uint16_t)(((uint32_t)s.brakeAnalog * 4095u) / 3300u);
    rsh_build_packet(frame->v1pkt, seq, tsUs, s.steering, v1Accel,
                     v1Brake, s.buttons, flags);
    frame->v1len = RSH_PACKET_SIZE;
  }
  frame->seq = seq;
  seq++;   // uint16 khud wrap hota hai — PC wrap-safe tareeqe se parhta hai
  return (frame->v2len > 0 || frame->v1len > 0) ? 1 : 0;
}

// Frame ke packet(s) bhejo (Wi-Fi unicast + serial frame, emit kind ke
// mutabiq). SEND loop task mein rehta hai — WiFiUDP thread-safe nahi
// (sample_task.h), is liye sampling task sirf produce karta hai.
static void sample_send_frame_(const RshSampleMailbox *frame) {
  if (!frame) return;
  if (frame->v2len > 0) {
    if (frame->emit_wifi == RSH_EMIT_V2 && net_send(frame->v2pkt, frame->v2len)) {
#if RSH_DEBUG
      dbgSentInWindow++;
#endif
    }
#if RSH_WIRED_SERIAL
    if (frame->emit_serial == RSH_EMIT_V2)
      serial_link_send_frame(RSH_SERIAL_KIND_STREAM, frame->v2pkt, frame->v2len);
#endif
  }
  if (frame->v1len > 0) {
    if (frame->emit_wifi == RSH_EMIT_V1 && net_send(frame->v1pkt, frame->v1len)) {
#if RSH_DEBUG
      dbgSentInWindow++;
#endif
    }
#if RSH_WIRED_SERIAL
    if (frame->emit_serial == RSH_EMIT_V1)
      serial_link_send_frame(RSH_SERIAL_KIND_STREAM, frame->v1pkt, frame->v1len);
#endif
  }
}

#if RSH_SAMPLE_TASK
// F-06: dedicated sampling task (core 1, loop task se zyada priority).
// vTaskDelayUntil se 5 ms (governor level 3 par 10 ms) period. NVS /
// WebServer / net_poll loop task mein rehte hain — un ka stall sampling
// ko nahi rokta. Cycle timing (overrun/maxCycle) task khud napta hai;
// loop task status packet mein wahi numbers parhta hai.
static void sample_task_fn(void *arg) {
  (void)arg;
  // DS-64: sampling task ko bhi task watchdog mein subscribe karo
  // (loop task ki tarah) — 5 ms cycle par feed hota hai, stall ho to
  // WDT reset karega. Subscribe fail ho to feed no-op hai.
  esp_task_wdt_add(NULL);
  TickType_t lastWake = xTaskGetTickCount();
  for (;;) {
    uint32_t cycleStartUs = micros();
    uint32_t nowMs = millis();
    RshSampleMailbox frame;
    sample_once_(&frame, nowMs, NULL);
    if (frame.v2len > 0 || frame.v1len > 0) {
      rsh_sample_mailbox_produce(&sampleMailbox, frame.v2pkt, frame.v2len,
                                 frame.v1pkt, frame.v1len, frame.seq,
                                 frame.emit_wifi, frame.emit_serial);
    }
    uint32_t cycleUs = micros() - cycleStartUs;
    if (cycleUs > maxCycleUs) maxCycleUs = cycleUs;
    uint32_t periodUs = inputs_governor().sendPeriodUs();
    prevCycleOverran = cycleUs > periodUs;
    if (prevCycleOverran) overrunCount++;
    net_set_stream_busy(prevCycleOverran);  // FW-8: ping drop-when-busy (§9.3)
    // Peeche reh gaye (lamba block) to burst catch-up mat karo — agla
    // tick ab se gino (loop task wala rule, yahan task ke liye).
    TickType_t ticks = pdMS_TO_TICKS(periodUs / 1000u);
    if (ticks == 0) ticks = 1;  // tick rate 100 Hz ho to bhi kam az kam 1 tick
    TickType_t nowTicks = xTaskGetTickCount();
    if ((TickType_t)(nowTicks - lastWake) >= ticks) lastWake = nowTicks;
    esp_task_wdt_reset();  // DS-64: sample task feed (har cycle)
    vTaskDelayUntil(&lastWake, ticks);
  }
}

// Sampling task ek dafa shuru karo (setup + net-retry dono yahin se).
// Dobara call no-op hai (task pehle se chal rahi ho to).
static void sample_task_start_() {
  if (sampleTaskActive || sampleTaskHandle != NULL) return;
  rsh_sample_mailbox_init(&sampleMailbox);
  BaseType_t created = xTaskCreatePinnedToCore(
      sample_task_fn, "rsh-sample", 4096, NULL, 2, &sampleTaskHandle, 1);
  sampleTaskActive = (created == pdPASS && sampleTaskHandle != NULL);
  if (!sampleTaskActive) sampleTaskHandle = NULL;  // legacy fallback
}
#endif

void setup() {
  rsh_diag_init(rsh_diag_log());  // F-10: ring log tayyar (heap_min 0xFFFF se)
  // D7: pichle boot ka password-clear (NVS flag "pwClr") ring mein
  // wapas — BOOT event se pehle, taake oldest-first order barqarar rahe.
  rsh_password_cleared_replay_();
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
    led_set_error_blinks(RSH_DIAG_BLINKS_CFG_ROLLBACK);  // F-10: 3 blinks
    for (;;) { led_update(millis()); delay(100); }
  }
#endif
  fw34_boot_setup();   // FW-3/FW-4: boot counter + config source + boot recover
  // F-10: boot reason + pehla heap-min snapshot diagnostics mein.
  rsh_diag_set_boot_reason(rsh_diag_log(), rsh_reset_reason_code());
  rsh_diag_record(rsh_diag_log(), RSH_DIAG_BOOT, rsh_reset_reason_code(),
                  millis());
  rsh_diag_set_heap_min(rsh_diag_log(), (uint16_t)(ESP.getFreeHeap() / 1024));
  // DS-64: task watchdog — loop task ko subscribe karo aur state record
  // karo (GET_DIAG se app dekh sakti hai). Arduino core aksar loop task
  // ko pehle se subscribe karta hai (ESP_ERR_INVALID_STATE = pehle se
  // watched, theek hai); ESP_OK = abhi subscribe hua; baqi = fail.
  // Feed loop() ke shuru mein hota hai (har iteration, har early-return
  // path se pehle). Timeout Arduino-core default hai — shipped esp32dev
  // build mein platformio.ini ke sdkconfig flags laagu nahi hote (pre-
  // compiled IDF), lekin core ki shipped sdkconfig mein
  // CONFIG_ESP_TASK_WDT_TIMEOUT_S=5, PANIC=y hai; R2-48 check
  // (firmware/tools/check-wdt-config.sh, CI firmware job) is value ko
  // har build par assert karta hai — platform bump par dobara verify
  // karna hoga. Board par WDT ka asal fire hona is check se prove
  // nahi hota (board test chahiye).
  {
    esp_err_t wdtErr = esp_task_wdt_add(NULL);
    uint16_t wdtVal = (wdtErr == ESP_OK)               ? 0
                      : (wdtErr == ESP_ERR_INVALID_STATE) ? 1
                                                        : 2;
    rsh_diag_record(rsh_diag_log(), RSH_DIAG_WDT_STATE, wdtVal, millis());
  }
  inputs_begin();
  // Phase G (C3): boot-from-NVS — NVS pin map (ya factory fallback)
  // inputs par lagao. inputs_begin sirf compiled defaults lagata hai;
  // boot config yahan apply hoti hai (loop detector bhi pehli tick par
  // yahi karta — dono ek hi apply hain, doosri dafa no-op hai).
  inputs_apply_config_blob(cfgTxn.current, cfgTxn.current_len);
  keyOk = keystore_begin();  // v2.2: migration/obfkey/NVS fail → AP up nahi (fail closed)
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
#if RSH_SAMPLE_TASK
    // F-06: sampling task shuru karo (core 1, priority 2 — loop task se
    // upar). Create fail ho to sampleTaskActive false rehta hai aur loop
    // legacy in-loop sampling karta hai (neeche loop() mein).
    sample_task_start_();
#endif
  } else {
    led_set_error_blinks(RSH_DIAG_BLINKS_NET_FAIL);  // F-10: 4 blinks (AP fail)
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

  esp_task_wdt_reset();  // DS-64: loop task feed — sab se pehle,
  // taake !netOk / OTA early-return paths par bhi har iteration feed ho.

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
#if RSH_SAMPLE_TASK
        sample_task_start_();  // F-06: retry recovery par bhi task chahiye
#endif
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
    // F-10: free-heap minimum diagnostics mein (naya min + 32 KB se kam
    // ho to HEAP_LOW event — ring spam nahi, sirf naya minimum par).
    uint16_t heapKb = (uint16_t)(ESP.getFreeHeap() / 1024);
    if (rsh_diag_set_heap_min(rsh_diag_log(), heapKb) && heapKb < 32)
      rsh_diag_record_global(RSH_DIAG_HEAP_LOW, heapKb, nowMs);
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
  net_poll_ctrl();              // FW-8: port 4212 — plain control + ping
#if RSH_WIRED_SERIAL
  serial_link_poll(nowMs);      // Phase G: USB-serial frames (HELO/control/ping)
#endif
  net_maybe_rescan(nowMs);      // FW-7: degraded link par rescan (60 s+ waqfa)
  led_update(nowMs);
  net_led_tick(nowMs);  // Phase H WS2: LED strip driver tick (200 Hz loop)

  // SEC-D-01: unauthenticated RSTX restart KHATAM — restart sirf
  // REBOOT (0x09) se hota hai (net_poll_ctrl → dispatcher).

  // Adaptive sampling: har 2 s window ke overruns governor ko do
  if (nowMs - govWindowStartMs >= 2000) {
    inputs_governor().update(overrunCount - govOverrunAtStart);
    govOverrunAtStart = overrunCount;
    govWindowStartMs = nowMs;
  }

  // (Heap check upar ho gaya — FW-10, OTA early return se pehle.)

  // Governor period: level 3 par 100 pps fallback (D16 — 10 ms period).
  uint32_t periodUs = inputs_governor().sendPeriodUs();
  // RSH_DEBUG stats ke liye aakhri sample (legacy path bharta hai;
  // task mode mein zero rehta hai — debug block us mode mein skip hota hai).
  InputSample s{};
#if RSH_SAMPLE_TASK
  if (sampleTaskActive) {
    // F-06 task mode: sampling task karta hai; yahan sirf mailbox drain
    // + send (WiFiUDP loop task mein hi — thread-safe nahi).
    RshSampleMailbox frame;
    // PERF P2-5 (v2.0.13 polish): mailbox frame har 5 ms aata hai —
    // frame na mile to delay(4) (~200 iter/s, net/LED phir bhi
    // 200 Hz); frame mile to delay(1) (packet processing ke doran
    // 1 ms spin). Pehle har iteration delay(1) thi (~1000 iter/s,
    // idle core-0 ~3-10% busy). Net control timeouts 250 ms hain,
    // is liye 4 ms service latency kuch nahi bigaadti; sampling
    // task core 1 par hai, is liye delay use rokta nahi.
    // HW-VERIFY: maxCycleUs / overruns board par pehle/baad.
    bool frameConsumed = rsh_sample_mailbox_consume(&sampleMailbox, &frame);
    if (frameConsumed) {
      // Overwrite hue frames seqGap mein (D5 tick-skipped flag).
      seqGap = (uint16_t)(seqGap + frame.skipped);
      sample_send_frame_(&frame);
    }
    // LED state har loop iteration par (task mode mein tick-wait nahi).
    bool active = net_pc_active(nowMs);
#if RSH_WIRED_SERIAL
    if (net_serial_pc_active(nowMs)) active = true;
#endif
    led_set_for_link_(active, nowMs);
    // Task mode mein loop jaldi spin kare — delay se CPU yield.
    delay(frameConsumed ? 1 : 4);
  } else {
#endif
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

  // Legacy in-loop sampling (task create fail ho to fallback — F-06).
  // Sample + build sample_once_ mein, send sample_send_frame_ mein
  // (task path wahi helpers istemal karta hai — ek hi code).
  RshSampleMailbox frame;
  sample_once_(&frame, nowMs, &s);
  sample_send_frame_(&frame);

  // Koi bhi transport (Wi-Fi ya serial) active ho to PC connected hai (LED).
  bool active = net_pc_active(nowMs);
#if RSH_WIRED_SERIAL
  if (net_serial_pc_active(nowMs)) active = true;
#endif
  // FW-3: safe mode ki LED ko normal state overwrite na kare.
  led_set_for_link_(active, nowMs);
#if RSH_SAMPLE_TASK
  }
#endif

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
    st.overruns = (uint16_t)overrunCount;  // R3-FW-09: u32 counter ka low
    // 16 bits — wrap-aware counter hai (layout change nahi, protocol
    // bump nahi); PC sirf deltas parhe (PROTOCOL-V2.2.md §1.4).
    st.max_cycle_us = maxCycleUs > 0xFFFF ? 0xFFFF : (uint16_t)maxCycleUs;
    // F-20 (polish round): windowed max — har status window (1 s) ke
    // baad reset karo, taake status packet hamesha aakhri 1 s ka max
    // bataye, lifetime max nahi (pehle ek early spike hamesha ke liye
    // max ko inflate kar deta tha). Sample task maxCycleUs likhta rehta
    // hai (volatile u32 — reset ke saath race mein zyada se zyada ek
    // cycle ka max kho jata hai, koi torn read nahi).
    maxCycleUs = 0;
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
  // F-06 task mode mein cycle timing sampling task khud napta hai
  // (overrunCount/maxCycleUs wahan update hote hain) — yahan sirf
  // legacy mode mein gino (warna dono paths ek hi counter bigarte).
  if (!sampleTaskActive) {
    uint32_t cycleUs = micros() - loopStartUs;
    if (cycleUs > maxCycleUs) maxCycleUs = cycleUs;
    prevCycleOverran = cycleUs > periodUs;
    net_set_stream_busy(prevCycleOverran);  // FW-8: ping drop-when-busy (§9.3)
    if (prevCycleOverran) overrunCount++;
  }

#if RSH_DEBUG
  // F-06 task mode mein s zero rehta hai aur maxCycleUs task ka hai —
  // debug stats sirf legacy mode mein (yahan maxCycleUs reset bhi hota hai).
  if (!sampleTaskActive) {
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
  }
#endif
}
