#pragma once
// RSH safe mode + boot counter (FW-3, Phase D).
// Plan v2.3 §0 / RSH-FIRMWARE-PLAN.md FW-3 + docs/PROTOCOL-V2.1.md §4.4.
//
//   * BOOT-hold detection RUNTIME hoti hai (C1 fix) — power-up par nahi.
//     Loop har iteration GPIO0 (BOOT button, active-LOW) parhta hai; hold
//     ki muddat se faisla hota hai (v2.2 — 3 s pair branch khatam, D7):
//       hold < 5 s            -> normal boot (BOOT ignore)
//       5 s <= hold < 10 s    -> safe mode (factory pin map + AP; config
//                                apply nahi hoti; LED slow blink 1 Hz)
//       hold >= 10 s          -> password clear (NVS password seedha
//                                mitao + reboot -> open AP; koi armed
//                                state / window nahi — clear threshold
//                                par hota hai; 5-10 s ke beech SafeMode
//                                LED hi hold ki indication hai)
//   * Boot fail = power-on ke 15 s mein AP up na ho (FW-3).
//     3 lagatar fail -> last-known-good config; LKG bhi 3 dafa fail ho
//     to factory fallback. Factory source par bhi fail ho to masla
//     firmware mein hai — firmware rollback (Round 2C):
//     `rsh_boot_firmware_rollback_due` sach hota hai aur caller doosri
//     OTA slot par wapas jata hai (ping-pong rokne ke liye NVS "fwRb"
//     flag, kamyab boot par saaf). Kamyab boot (AP waqt par up) dono
//     counter zero karta hai.
//   * Safe mode mein config apply nahi hoti — config_transaction.h ka
//     safe-mode flag stage + commit dono ko BUSY (7) se rokta hai.
//
// Ye file Arduino se bilkul free hai (protocol.h / inputs.h ki tarah)
// taake host par unit test ho sake. NVS persistence caller ka kaam hai:
// rsh_boot_counter_t plain struct hai — .ino isay Preferences mein
// save/load karta hai. Koi static/global state yahan nahi.
#include <stdint.h>
#include <string.h>

// ---------- Constants (plan + contract) ----------
#define RSH_BOOT_AP_UP_DEADLINE_MS   15000u   // FW-3: AP up na ho to boot fail
#define RSH_BOOT_MAX_CONSEC_FAILURES 3u       // itne lagatar fail -> fallback
#define RSH_BOOTHOLD_SAFE_MS         5000u    // 5 s hold -> safe mode (§4.4)
#define RSH_BOOTHOLD_KEYRESET_MS     10000u   // 10 s hold -> password clear (D7)

// ---------- Boot counter + config source ----------
typedef enum {
  RSH_BOOT_SRC_CURRENT = 0,          // aam config (NVS "current" slot)
  RSH_BOOT_SRC_LAST_KNOWN_GOOD = 1,  // aakhri kamyab boot wali config
  RSH_BOOT_SRC_FACTORY = 2           // compiled-in factory default blob
} rsh_boot_src_t;

typedef struct {
  uint32_t total_boots;
  uint32_t consecutive_failures;      // current-source boot fail, lagatar
  uint32_t lkg_consecutive_failures;  // LKG-source boot fail, lagatar
  uint8_t using_lkg;                  // is boot mein LKG source istemal ho rahi hai
  uint8_t counted_this_boot;          // is boot ka natija darj ho chuka hai
} rsh_boot_counter_t;

static inline void rsh_boot_counter_init(rsh_boot_counter_t *c) {
  memset(c, 0, sizeof(*c));
}

// Boot shuru: total barhao, is boot ka natija abhi darj nahi hua.
static inline void rsh_boot_begin(rsh_boot_counter_t *c) {
  if (!c) return;
  c->total_boots++;
  c->counted_this_boot = 0;
}

// Is boot mein kaunsi config source istemal ho? Caller `using_lkg` ko
// is ke jawab ke mutabiq set kare (phir rsh_boot_record_outcome sahi
// counter barhata hai). LKG valid na ho to seedha factory.
static inline rsh_boot_src_t rsh_boot_decide_source(const rsh_boot_counter_t *c,
                                                    uint8_t lkg_valid) {
  if (!c) return RSH_BOOT_SRC_FACTORY;
  if (c->consecutive_failures < RSH_BOOT_MAX_CONSEC_FAILURES)
    return RSH_BOOT_SRC_CURRENT;
  if (lkg_valid && c->lkg_consecutive_failures < RSH_BOOT_MAX_CONSEC_FAILURES)
    return RSH_BOOT_SRC_LAST_KNOWN_GOOD;
  return RSH_BOOT_SRC_FACTORY;
}

// Boot ka natija ek dafa darj karo. Kamyab boot (AP waqt par up) dono
// counter zero karta hai; fail sirf us source ka counter barhata hai jo
// is boot mein istemal ho rahi thi. Dobara call no-op hai.
static inline void rsh_boot_record_outcome(rsh_boot_counter_t *c, uint8_t ap_up_in_time) {
  if (!c || c->counted_this_boot) return;
  c->counted_this_boot = 1;
  if (ap_up_in_time) {
    c->consecutive_failures = 0;
    c->lkg_consecutive_failures = 0;
  } else if (c->using_lkg) {
    c->lkg_consecutive_failures++;
  } else {
    c->consecutive_failures++;
  }
}

// ---------- Firmware rollback (Round 2C, WebOTA) ----------
// Config fallback khatam (current 3 fail + LKG 3 fail -> factory source)
// hone ke bawajood boot kamyab na ho to masla config mein nahi, FIRMWARE
// mein hai: is slot ki image AP up nahi kar sakti (crash / panic loop).
// Caller (rsh.ino) is par doosri OTA slot ko boot partition bana kar
// restart karta hai — purana firmware wapas. Ye sirf faisla hai (pure);
// NVS/ESP glue caller ka kaam hai, aur ping-pong rokne ke liye caller
// ek "rollback ho chuka" flag (NVS "rsh-boot" / "fwRb") rakhta hai jo
// kamyab boot par saaf hota hai.
// NOTE: bootloader-level rollback (CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE)
// plain Arduino env mein nahi chal sakta (precompiled bootloader,
// sdkconfig flags laagu nahi hote — platformio.ini), is liye ye
// app-level mechanism hi dono envs mein crash-loop rollback deta hai.
static inline uint8_t rsh_boot_firmware_rollback_due(const rsh_boot_counter_t *c) {
  if (!c) return 0;
  return (c->consecutive_failures >= RSH_BOOT_MAX_CONSEC_FAILURES &&
          c->lkg_consecutive_failures >= RSH_BOOT_MAX_CONSEC_FAILURES)
             ? 1
             : 0;
}

// ---------- Boot tick: AP-up deadline (har loop mein call karo) ----------
typedef enum {
  RSH_BOOT_WAITING = 0,  // abhi faisla nahi — AP ka intezar / natija darj ho chuka
  RSH_BOOT_SUCCESS = 1,  // AP deadline ke andar up — natija darj ho gaya
  RSH_BOOT_FAILED = 2    // deadline guzar gayi, AP up nahi — fail darj ho gaya
} rsh_boot_tick_t;

// ap_up = SoftAP up hai (net_begin kamyab). boot_start_ms = power-on
// (millis) ka waqt. Deadline ke baad AP up ho bhi jaye to is boot ka
// fail wapas nahi hota — der se up hona bhi fail hai (counter sach bole).
// (uint32 subtraction wrap-safe hai — millis ~49 din par wrap hota hai.)
static inline rsh_boot_tick_t rsh_boot_tick(rsh_boot_counter_t *c, uint8_t ap_up,
                                            uint32_t now_ms, uint32_t boot_start_ms) {
  if (!c || c->counted_this_boot) return RSH_BOOT_WAITING;
  if (ap_up) {
    rsh_boot_record_outcome(c, 1);
    return RSH_BOOT_SUCCESS;
  }
  if ((uint32_t)(now_ms - boot_start_ms) >= RSH_BOOT_AP_UP_DEADLINE_MS) {
    rsh_boot_record_outcome(c, 0);
    return RSH_BOOT_FAILED;
  }
  return RSH_BOOT_WAITING;
}

// ---------- Runtime BOOT-hold detector (C1 fix: runtime, power-up nahi) ----------
typedef enum {
  RSH_HOLD_NONE = 0,
  RSH_HOLD_SAFE_MODE = 1,        // 5 s hold paar — safe mode mein jao
  RSH_HOLD_KEY_RESET_CLEAR = 2   // 10 s hold paar — password clear + reboot (D7)
} rsh_hold_event_t;

typedef struct {
  uint32_t hold_start_ms;  // musalsal press kab se hai
  uint8_t pressed;         // pichli sample mein BOOT daba hua tha
  uint8_t safe_fired;      // 5 s event is hold mein ja chuka hai
  uint8_t clear_fired;     // 10 s event is hold mein ja chuka hai
} rsh_boothold_t;

static inline void rsh_boothold_init(rsh_boothold_t *d) {
  memset(d, 0, sizeof(*d));
}

// Har loop mein call karo: pressed = BOOT daba hua hai (GPIO0 LOW).
// Release hote hi detector reset — chhoti dabao (bounce / < 5 s) kuch nahi karti.
// Har threshold apne hold mein sirf ek dafa fire hota hai. 10 s event
// (password clear) threshold par fire hota hai — release ka intezar nahi
// (D7: clear threshold par, phir reboot).
static inline rsh_hold_event_t rsh_boothold_update(rsh_boothold_t *d, uint8_t pressed,
                                                    uint32_t now_ms) {
  if (!d) return RSH_HOLD_NONE;
  if (!pressed) {
    d->pressed = 0;
    d->safe_fired = 0;
    d->clear_fired = 0;
    return RSH_HOLD_NONE;
  }
  if (!d->pressed) {
    d->pressed = 1;
    d->hold_start_ms = now_ms;
    return RSH_HOLD_NONE;
  }
  uint32_t held_ms = (uint32_t)(now_ms - d->hold_start_ms);
  if (held_ms >= RSH_BOOTHOLD_KEYRESET_MS && !d->clear_fired) {
    d->clear_fired = 1;
    return RSH_HOLD_KEY_RESET_CLEAR;
  }
  if (held_ms >= RSH_BOOTHOLD_SAFE_MS && !d->safe_fired) {
    d->safe_fired = 1;
    return RSH_HOLD_SAFE_MODE;
  }
  return RSH_HOLD_NONE;
}

// ---------- Safe-mode state ----------
typedef struct {
  uint8_t active;            // safe mode active hai
  uint32_t entered_ms;       // safe mode kab shuru hua
} rsh_safe_mode_t;

static inline void rsh_safe_mode_init(rsh_safe_mode_t *s) {
  memset(s, 0, sizeof(*s));
}

// Safe mode mein jao. Reboot ke baghair safe mode se bahar nahi nikla
// jata — ye jaan-boojh kar hai. (v2.2: key-reset-armed state khatam —
// 10 s hold seedha password clear + reboot hai, D7.)
static inline void rsh_safe_mode_enter(rsh_safe_mode_t *s, uint32_t now_ms) {
  if (!s) return;
  s->active = 1;
  s->entered_ms = now_ms;
}

// Safe mode mein config apply nahi hoti (FW-3 gate).
static inline uint8_t rsh_safe_mode_blocks_config(const rsh_safe_mode_t *s) {
  return (s && s->active) ? 1 : 0;
}
