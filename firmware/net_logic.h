#pragma once
// RSH network pure logic (FW-7, F3/F8). Arduino-free — host par test hota
// hai; net.cpp isi ko istemal karta hai (single source of truth, T4).
//
// - Channel score: apne channel ka load sab se zyada wazan, padosi
//   channels (overlap) kam wazan (net.cpp se move, behaviour same).
// - pick_channel: 1/6/11 mein se sab se khaali; tie par chhota channel.
// - pc_active: HELO timeout, millis() wrap-safe.
// - TxStats: net_send ka natija — fail par txFail counter (F3, status
//   packet mein jata hai).
#include <stdint.h>
#include <stddef.h>

// load[1..13]: har channel par dikhe networks ki ginti.
static inline int rsh_channel_score(const int *load, int c) {
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

static inline uint8_t rsh_pick_channel(const int *load) {
  const uint8_t cand[3] = {1, 6, 11};
  uint8_t best = cand[0];
  for (int i = 1; i < 3; i++) {
    if (rsh_channel_score(load, cand[i]) < rsh_channel_score(load, best))
      best = cand[i];
  }
  return best;
}

static inline bool rsh_pc_active(uint32_t last_hello_ms, uint32_t now_ms,
                                 uint32_t timeout_ms) {
  return (uint32_t)(now_ms - last_hello_ms) < timeout_ms;
}

struct TxStats {
  uint32_t sent = 0;
  uint32_t tx_fail = 0;  // F3: failed UDP transmissions (boot se)

  // net_send ka natija record karo. Return: ok (bheja gaya ya nahi).
  bool record(bool ok) {
    if (ok) sent++;
    else tx_fail++;
    return ok;
  }
};

// ---- FW-7 additions (Phase D, Workstream 3) --------------------------------
#define RSH_RSSI_UNKNOWN ((int8_t)-128)  // station RSSI naapa na ja sake

// Scan entry: ek nazar aane wala network (net.cpp WiFi.scanNetworks se).
typedef struct {
  uint8_t channel;    // 1..13 (is se bahar ignore)
  int8_t rssi_dbm;    // dBm
  uint8_t hidden;     // 1 = hidden SSID → load mein NAHI gina jata (F8)
} rsh_scan_entry_t;

// RSSI ka wazan: qareebi/taqatwar network channel ko zyada "bharta" hai.
// Sirf ginna ghalat tha: 5 kamzor networks 1 taqatwar se behtar channel ko
// hara dete thay. Hidden + invalid channel skip hote hain.
static inline int rsh_rssi_weight_(int8_t rssi) {
  if (rssi >= -50) return 4;
  if (rssi >= -65) return 3;
  if (rssi >= -80) return 2;
  return 1;
}

/// Scan entries se RSSI-weighted load table banao (load[1..13]) — phir
/// maujooda rsh_pick_channel(load) / rsh_channel_score(load, c) istemal karo.
static inline void rsh_scan_load(const rsh_scan_entry_t *entries, size_t n, int *load) {
  for (int ch = 0; ch < 14; ch++) load[ch] = 0;
  if (!entries || !load) return;
  for (size_t i = 0; i < n; i++) {
    if (entries[i].hidden) continue;
    uint8_t ch = entries[i].channel;
    if (ch < 1 || ch > 13) continue;
    load[ch] += rsh_rssi_weight_(entries[i].rssi_dbm);
  }
}

// ---- Rescan policy (F8): rescan SIRF zaroorat par ------------------------
// Har retry / har HELO par scan NAHI — scan STA mode mangta hai aur AP ko
// 1-3 s rokta hai, is liye: kam az kam 60 s waqfa + link degraded ho.
// Degraded = RSSI <= -80 dBm (naapa ja sake to) YA aakhri scan se txFail
// mein 20+ izafa. Channel tabdeel sirf tab jab naya score sakhti se
// behtar ho AUR aakhri tabdeeli ko 5 min guzar gaye hon (flap rok).
#define RSH_RESCAN_MIN_INTERVAL_MS 60000u
#define RSH_RESCAN_SWITCH_MIN_MS   300000u
#define RSH_RESCAN_RSSI_BAD_DBM    (-80)
#define RSH_RESCAN_TXFAIL_DELTA    20u

static inline uint8_t rsh_rescan_needed(int8_t rssi_dbm, uint16_t tx_fail_delta,
                                        uint32_t ms_since_scan) {
  if (ms_since_scan < RSH_RESCAN_MIN_INTERVAL_MS) return 0;
  uint8_t degraded = 0;
  if (rssi_dbm != RSH_RSSI_UNKNOWN && rssi_dbm <= RSH_RESCAN_RSSI_BAD_DBM) degraded = 1;
  if (tx_fail_delta >= RSH_RESCAN_TXFAIL_DELTA) degraded = 1;
  return degraded;
}

static inline uint8_t rsh_channel_switch_allowed(int new_score, int cur_score,
                                                 uint32_t ms_since_switch) {
  if (ms_since_switch < RSH_RESCAN_SWITCH_MIN_MS) return 0;
  return new_score < cur_score ? 1 : 0;
}

// ---- HELO acceptance (SEC-D-08) ------------------------------------------
// Pehle 4 bytes "HELO" dekhte hi pcIp overwrite karna hijack tha: AP par
// koi bhi station ek packet se 200 Hz stream apni taraf mor sakta tha.
// Ab: live session (timeout ke andar HELO aaya ho) ka pcIp koi doosra IP
// overwrite NAHI kar sakta; idle board par bhi pcIp change 5 s mein ek
// se zyada nahi (flap throttle). Pehla peer (have_pc=0) hamesha qabool.
// Residual: idle board (koi active PC nahi) par koi bhi HELO bhej kar
// stream le sakta hai — unauthenticated UDP bootstrap ki fitri had hai
// (OPEN-6); pairing window + WPA2-after-pairing is ki had bandhte hain.
static inline uint8_t rsh_helo_accept(uint8_t have_pc, uint8_t pc_active, uint8_t same_ip,
                                      uint32_t ms_since_pc_change) {
  if (same_ip) return 1;
  if (!have_pc) return 1;
  if (pc_active) return 0;   // live session — takeover mana
  if (ms_since_pc_change < 5000u) return 0;
  return 1;
}

// HELLO reply rate limit (SEC-D-08): 92 B reply vs 5 B request = 18x
// amplification; flood par board reflector na bane — 500 ms mein ek reply.
static inline uint8_t rsh_hello_reply_allowed(uint32_t ms_since_last_reply) {
  return ms_since_last_reply >= 500u ? 1 : 0;
}

// ---- Ping rate limiter (PROTOCOL §9.4, SEC-C-21) --------------------------
// Per-source token bucket: max 10 replies/s, burst 5. Bucket table bounded
// (64 sources) — bhar jaye to sab se purana (LRU) source nikal kar naya.
// Tokens tenths mein (capacity 50 = 5.0 token, refill 100 tenths/s).
#define RSH_PING_BUCKET_MAX 64
#define RSH_PING_TOKENS_CAP_TENTHS 50
#define RSH_PING_TOKENS_PER_SEC_TENTHS 100
#define RSH_PING_COST_TENTHS 10  // ek reply = 1.0 token

typedef struct {
  uint32_t ip;              // source IPv4 (bytes pack kiye hue)
  uint16_t tokens_tenths;   // 0..50
  uint32_t last_ms;         // aakhri refill/use (LRU ke liye)
  uint8_t used;
} rsh_ping_bucket_t;

typedef struct {
  rsh_ping_bucket_t buckets[RSH_PING_BUCKET_MAX];
} rsh_ping_limiter_t;

static inline void rsh_ping_limiter_init(rsh_ping_limiter_t *lim) {
  if (!lim) return;
  for (int i = 0; i < RSH_PING_BUCKET_MAX; i++) {
    lim->buckets[i].ip = 0;
    lim->buckets[i].tokens_tenths = 0;
    lim->buckets[i].last_ms = 0;
    lim->buckets[i].used = 0;
  }
}

/// Ping reply bhejne ki ijazat? 1 = allow (1 token kata), 0 = silent drop.
static inline uint8_t rsh_ping_allow(rsh_ping_limiter_t *lim, uint32_t src_ip, uint32_t now_ms) {
  if (!lim) return 0;
  rsh_ping_bucket_t *b = NULL;
  for (int i = 0; i < RSH_PING_BUCKET_MAX; i++) {
    if (lim->buckets[i].used && lim->buckets[i].ip == src_ip) { b = &lim->buckets[i]; break; }
  }
  if (!b) {
    // Naya source: khaali slot dhoondo, warna sab se purana (LRU) nikalo.
    int slot = -1;
    for (int i = 0; i < RSH_PING_BUCKET_MAX; i++) {
      if (!lim->buckets[i].used) { slot = i; break; }
    }
    if (slot < 0) {
      slot = 0;
      for (int i = 1; i < RSH_PING_BUCKET_MAX; i++) {
        if ((int32_t)(lim->buckets[i].last_ms - lim->buckets[slot].last_ms) < 0) slot = i;
      }
    }
    b = &lim->buckets[slot];
    b->ip = src_ip;
    b->tokens_tenths = RSH_PING_TOKENS_CAP_TENTHS;  // naya source burst 5 se shuru
    b->last_ms = now_ms;
    b->used = 1;
  }
  // Refill: 5 s idle = full (50 tenths); is se kam par elapsed/10 tenths.
  uint32_t elapsed = now_ms - b->last_ms;
  if (elapsed >= 5000u) {
    b->tokens_tenths = RSH_PING_TOKENS_CAP_TENTHS;
  } else if (elapsed > 0) {
    uint32_t add = elapsed / 10u;  // 100 tenths/s
    uint32_t t = (uint32_t)b->tokens_tenths + add;
    b->tokens_tenths = (uint16_t)(t > RSH_PING_TOKENS_CAP_TENTHS ? RSH_PING_TOKENS_CAP_TENTHS : t);
  }
  b->last_ms = now_ms;
  if (b->tokens_tenths < RSH_PING_COST_TENTHS) return 0;
  b->tokens_tenths = (uint16_t)(b->tokens_tenths - RSH_PING_COST_TENTHS);
  return 1;
}
