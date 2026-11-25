#pragma once
// RSH diagnostics ring log + GET_DIAG body (F-10, v4.0.2).
//
// Board ka error pehle sirf LED rang tha (field mein serial debug log
// nahi milta). Ab chhota ring buffer (32 events, RAM mein) boot reason,
// AP/Wi-Fi events aur heap-min rakhta hai; app GET_DIAG control command
// (0x0D) se snapshot lekar diagnostics bundle mein daal sakti hai. Koi
// secret kabhi log nahi hota (sirf event codes + chhoti numeric values).
// D7 exception: RSH_DIAG_PASSWORD_CLEARED reboot se bach jata hai —
// reboot se pehle NVS mein 1-byte pending-flag ("rsh-boot" / "pwClr")
// likha jata hai aur agle boot par setup() event dobara ring mein record
// karta hai; baqi ring ab bhi RAM-only hai (purana R2-47 krExp pattern,
// ab password-clear ke liye).
//
// GET_DIAG reply body (wire, v4.0.2 additive — maujooda frames nahi badle):
//   [0]    boot_reason u8 (PROTOCOL §1.2: 1 power-on, 2 brown-out,
//          3 watchdog, 4 software, 0 reserved)
//   [1..2] heap_min_kb u16 LE (boot se sab se kam free heap, KB)
//   [3..4] reserved u16 LE (hamesha 0 — v2.2 mein pairing window khatam,
//          D10: field layout ke liye barqarar hai, value hamesha zero)
//   [5]    event_count u8 (0..32, oldest-first)
//   [6..]  har event 7 B: ts_ms u32 LE | code u8 | value u16 LE
// Kul max 6 + 32*7 = 230 B (control body had 1023 B ke andar).
//
// LED error codes (blink count, status_led.cpp ErrorCode state):
// heap-low 2, config rollback 3, net/AP fail 4, OTA fail 5.
// Ye file Arduino se free hai — net.cpp / rsh.ino global instance
// istemal karte hain, host tests local struct se logic cover karte hain.
#include <stdint.h>
#include <stddef.h>
#include <string.h>

// ---- Event codes (wire values, kabhi mat badlo) ----
#define RSH_DIAG_BOOT            1   // value = reset reason code (§1.2)
#define RSH_DIAG_AP_UP           2   // value = channel
// 3 = AP_LOCKED, 4 = WINDOW_OPEN, 5 = PAIRED — v2.2 mein DELETE (pairing
// khatam; values dobara istemal nahi hoti).
#define RSH_DIAG_HELO_REJECT     6   // value: 0 = live session takeover, 1 = pc-change throttle
#define RSH_DIAG_HEAP_LOW        7   // value = free heap KB jab naya minimum bana
#define RSH_DIAG_CFG_ROLLBACK    8   // value = 0 (confirm timeout / boot recover)
#define RSH_DIAG_OTA_FAIL        9   // value = 0 (WebOTA upload fail/abort)
#define RSH_DIAG_FW_ROLLBACK     12  // value = 0 (Round 2C: 6 lagatar boot fail -> doosri OTA slot)
#define RSH_DIAG_RESCAN          10  // value = naya channel (F-07)
// 11 = KEY_RESET_ARMED — v2.2 mein DELETE (koi armed state nahi, D7;
// value dobara istemal nahi hoti).
#define RSH_DIAG_AP_RESTART_FAIL 13  // DS-11: value 0 = AP lock, 1 = new-password restart, 2 = rescan restart
// 14 = KEY_RESET_EXPIRED — v2.2 mein DELETE (koi armed window nahi, D7;
// value dobara istemal nahi hoti).
#define RSH_DIAG_PASSWORD_CLEARED 17  // D7: value = 0 (BOOT >= 10 s hold se password mita)
#define RSH_DIAG_WDT_STATE       15  // DS-64: value 0 = loop task subscribed now, 1 = already subscribed, 2 = subscribe failed
#define RSH_DIAG_KEYSTORE_NVS_FAIL 16  // DS-65: value 0 = keystore NVS write fail (set/clear password)
#define RSH_DIAG_CFG_SAVE_FAIL   18  // P2-2: value 0 = config NVS persist fail / verify mismatch (COMMIT/CONFIRM/rollback persist) — in-RAM config chalti rehti hai, agle boot par LKG/factory fallback

#define RSH_DIAG_MAX_EVENTS 32

// ---- LED error blink counts (status_led ErrorCode state) ----
#define RSH_DIAG_BLINKS_HEAP_LOW     2
#define RSH_DIAG_BLINKS_CFG_ROLLBACK 3
#define RSH_DIAG_BLINKS_NET_FAIL     4
#define RSH_DIAG_BLINKS_OTA_FAIL     5

typedef struct {
  uint32_t ts_ms;
  uint8_t code;
  uint16_t value;
} RshDiagEvent;

typedef struct {
  RshDiagEvent events[RSH_DIAG_MAX_EVENTS];
  uint8_t count;       // bhare hue slots (<= 32)
  uint8_t next;        // agla overwrite index (ring)
  uint32_t total;      // boot se kul events (wrap nahi — u32)
  uint16_t heap_min_kb;  // boot se sab se kam free heap (KB, 0 = abhi set nahi)
  uint8_t boot_reason;   // §1.2 code (0 = abhi set nahi)
} RshDiagLog;

static inline void rsh_diag_init(RshDiagLog *log) {
  if (!log) return;
  memset(log, 0, sizeof(*log));
  log->heap_min_kb = 0xFFFF;  // pehla set_heap_min() hamesha jeette
}

// Event record karo (ring: bhar jaye to sab se purana overwrite).
static inline void rsh_diag_record(RshDiagLog *log, uint8_t code,
                                   uint16_t value, uint32_t now_ms) {
  if (!log || code == 0) return;
  RshDiagEvent *e = &log->events[log->next];
  e->ts_ms = now_ms;
  e->code = code;
  e->value = value;
  log->next = (uint8_t)((log->next + 1) % RSH_DIAG_MAX_EVENTS);
  if (log->count < RSH_DIAG_MAX_EVENTS) log->count++;
  log->total++;
}

// Free-heap minimum update karo. Return 1 = naya minimum bana (caller
// is par HEAP_LOW event record kar sakta hai), 0 = koi naya min nahi.
static inline uint8_t rsh_diag_set_heap_min(RshDiagLog *log, uint16_t heap_kb) {
  if (!log) return 0;
  if (heap_kb < log->heap_min_kb) {
    log->heap_min_kb = heap_kb;
    return 1;
  }
  return 0;
}

static inline void rsh_diag_set_boot_reason(RshDiagLog *log, uint8_t reason) {
  if (log) log->boot_reason = reason;
}

// GET_DIAG body banao (upar layout). out kam az kam 230 B ka ho.
// Return body length (6 + 7*count), ya 0 (ghalat input / out chhota).
static inline size_t rsh_diag_build_body(const RshDiagLog *log,
                                         uint16_t reserved_u16,
                                         uint8_t *out, size_t out_len) {
  if (!log || !out) return 0;
  size_t total = 6u + 7u * log->count;
  if (out_len < total) return 0;
  out[0] = log->boot_reason;
  out[1] = (uint8_t)(log->heap_min_kb & 0xFF);
  out[2] = (uint8_t)((log->heap_min_kb >> 8) & 0xFF);
  out[3] = (uint8_t)(reserved_u16 & 0xFF);  // reserved — caller hamesha 0 deta hai (D10)
  out[4] = (uint8_t)((reserved_u16 >> 8) & 0xFF);
  out[5] = log->count;
  // Oldest-first: ring mein sab se purana event `next` par hai (jab full
  // ho), warna index 0 par.
  uint8_t start = (log->count == RSH_DIAG_MAX_EVENTS) ? log->next : 0;
  for (uint8_t i = 0; i < log->count; i++) {
    const RshDiagEvent *e =
        &log->events[(uint8_t)((start + i) % RSH_DIAG_MAX_EVENTS)];
    uint8_t *p = out + 6u + 7u * i;
    p[0] = (uint8_t)(e->ts_ms & 0xFF);
    p[1] = (uint8_t)((e->ts_ms >> 8) & 0xFF);
    p[2] = (uint8_t)((e->ts_ms >> 16) & 0xFF);
    p[3] = (uint8_t)((e->ts_ms >> 24) & 0xFF);
    p[4] = e->code;
    p[5] = (uint8_t)(e->value & 0xFF);
    p[6] = (uint8_t)((e->value >> 8) & 0xFF);
  }
  return total;
}

// Error blink pattern (pure, status_led.cpp istemal karta hai):
// `blinks` tezi se blink (150 ms on / 150 ms off), phir 1 s pause,
// phir dobara. nowMs par LED on hai? 1 = on, 0 = off.
static inline uint8_t rsh_error_blink_on(uint8_t blinks, uint32_t now_ms) {
  if (blinks == 0) return 0;
  uint32_t cycle_ms = (uint32_t)blinks * 300u + 1000u;
  uint32_t phase = now_ms % cycle_ms;
  if (phase >= (uint32_t)blinks * 300u) return 0;  // pause
  return (phase % 300u) < 150u ? 1 : 0;
}

// Global instance (modules.h ke rsh_modules_state_() wala pattern —
// function-local static, har TU mein ek hi instance).
static inline RshDiagLog &rsh_diag_log_(void) {
  static RshDiagLog log;
  return log;
}

static inline RshDiagLog *rsh_diag_log(void) { return &rsh_diag_log_(); }

static inline void rsh_diag_record_global(uint8_t code, uint16_t value,
                                          uint32_t now_ms) {
  rsh_diag_record(&rsh_diag_log_(), code, value, now_ms);
}
